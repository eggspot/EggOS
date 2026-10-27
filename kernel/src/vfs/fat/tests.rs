use super::*;
use crate::block::BlockError;
use crate::tests::TestPlatform;
use crate::vfs::{Node, Refused, WriteAt, resolve};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Clone, Default)]
struct Image {
    sectors: u64,
    written: BTreeMap<u64, [u8; SECTOR_SIZE]>,
}

impl Image {
    fn new(sectors: u64) -> Self {
        Self {
            sectors,
            written: BTreeMap::new(),
        }
    }

    fn put(&mut self, at: u64, bytes: &[u8]) {
        for (index, &byte) in bytes.iter().enumerate() {
            let position = at + index as u64;
            let sector = self
                .written
                .entry(position / SECTOR_SIZE as u64)
                .or_insert([0; SECTOR_SIZE]);
            sector[(position % SECTOR_SIZE as u64) as usize] = byte;
        }
    }

    fn get(&self, at: u64) -> u8 {
        self.written
            .get(&(at / SECTOR_SIZE as u64))
            .map_or(0, |sector| sector[(at % SECTOR_SIZE as u64) as usize])
    }
}

#[derive(Debug, Clone, Default)]
struct Probes {
    reads: Arc<AtomicUsize>,
    failing: Arc<AtomicBool>,
}

struct MemoryDisk {
    image: Image,
    probes: Probes,
}

impl BlockDevice for MemoryDisk {
    fn sectors(&self) -> u64 {
        self.image.sectors
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        _deadline: &mut Deadline,
    ) -> Result<(), BlockError> {
        if !buffer.len().is_multiple_of(SECTOR_SIZE)
            || sector + (buffer.len() / SECTOR_SIZE) as u64 > self.image.sectors
        {
            return Err(BlockError::OutOfRange);
        }
        if self.probes.failing.load(Ordering::Relaxed) {
            return Err(BlockError::Device("the test disk fails"));
        }
        self.probes.reads.fetch_add(1, Ordering::Relaxed);
        for (index, chunk) in buffer.chunks_mut(SECTOR_SIZE).enumerate() {
            match self.image.written.get(&(sector + index as u64)) {
                Some(data) => chunk.copy_from_slice(data),
                None => chunk.fill(0),
            }
        }
        Ok(())
    }

    fn write(&mut self, _: u64, _: &[u8], _: &mut Deadline) -> Result<(), BlockError> {
        Err(BlockError::ReadOnly)
    }
}

fn test_deadline(milliseconds: u64) -> Deadline {
    Deadline::after::<TestPlatform>(milliseconds)
}

enum Item {
    File(Vec<u8>),
    Directory(Vec<(String, Item, Naming)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    Long,
    ShortOnly {
        lower_base: bool,
        lower_extension: bool,
    },
    BadChecksum,
}

const MODIFIED_DATE: u16 = (44 << 9) | (2 << 5) | 29;
const MODIFIED_TIME: u16 = (13 << 11) | (45 << 5) | 15;
const MODIFIED_UNIX: u64 = 1_709_214_330;

struct Builder {
    fat_type: FatType,
    image: Image,
    fat_start: u64,
    fat_bytes: u64,
    fats: u64,
    root_start: u64,
    data_start: u64,
    cluster_bytes: u64,
    clusters: u32,
    next_free: u32,
    short_names: u32,
    fragment: bool,
    directory_stride: u32,
}

struct Format {
    fat_type: FatType,
    clusters: u32,
    sectors_per_cluster: u8,
    partition_at: Option<u64>,
    fragment: bool,
    directory_stride: u32,
}

impl Format {
    fn of(fat_type: FatType) -> Self {
        let clusters = match fat_type {
            FatType::Fat12 => 3000,
            FatType::Fat16 => 6000,
            FatType::Fat32 => 70_000,
        };
        Self {
            fat_type,
            clusters,
            sectors_per_cluster: 1,
            partition_at: None,
            fragment: true,
            directory_stride: 1,
        }
    }
}

impl Builder {
    fn new(format: &Format) -> Self {
        let fat_type = format.fat_type;
        let spc = u64::from(format.sectors_per_cluster);
        let reserved: u64 = match fat_type {
            FatType::Fat32 => 32,
            _ => 1,
        };
        let root_entries: u64 = match fat_type {
            FatType::Fat32 => 0,
            _ => 512,
        };
        let entries = u64::from(format.clusters) + 2;
        let fat_bytes = match fat_type {
            FatType::Fat12 => entries * 3 / 2 + 1,
            FatType::Fat16 => entries * 2,
            FatType::Fat32 => entries * 4,
        };
        let fat_sectors = fat_bytes.div_ceil(512);
        let root_sectors = root_entries * 32 / 512;
        let fats = 2;
        let metadata = reserved + fats * fat_sectors + root_sectors;
        let total = metadata + u64::from(format.clusters) * spc;
        let start_sector = format.partition_at.unwrap_or(0);
        let start = start_sector * 512;
        let mut image = Image::new(start_sector + total);
        let mut boot = [0u8; 512];
        boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        boot[3..11].copy_from_slice(b"EGGTEST ");
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = format.sectors_per_cluster;
        boot[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
        boot[16] = fats as u8;
        boot[17..19].copy_from_slice(&(root_entries as u16).to_le_bytes());
        match u16::try_from(total) {
            Ok(total) if fat_type != FatType::Fat32 => {
                boot[19..21].copy_from_slice(&total.to_le_bytes())
            }
            _ => boot[32..36].copy_from_slice(&(total as u32).to_le_bytes()),
        }
        boot[21] = 0xf8;
        let label_at = match fat_type {
            FatType::Fat32 => {
                boot[36..40].copy_from_slice(&(fat_sectors as u32).to_le_bytes());
                boot[44..48].copy_from_slice(&2u32.to_le_bytes());
                71
            }
            _ => {
                boot[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
                43
            }
        };
        boot[label_at..label_at + 11].copy_from_slice(b"EGG TEST   ");
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        image.put(start, &boot);
        if let Some(lba) = format.partition_at {
            let mut entry = [0u8; 16];
            entry[4] = match fat_type {
                FatType::Fat32 => 0x0c,
                _ => 0x06,
            };
            entry[8..12].copy_from_slice(&(lba as u32).to_le_bytes());
            entry[12..16].copy_from_slice(&(total as u32).to_le_bytes());
            image.put(446, &entry);
            image.put(510, &[0x55, 0xaa]);
        }
        let mut builder = Self {
            fat_type,
            image,
            fat_start: start + reserved * 512,
            fat_bytes: fat_sectors * 512,
            fats,
            root_start: start + (reserved + fats * fat_sectors) * 512,
            data_start: start + metadata * 512,
            cluster_bytes: spc * 512,
            clusters: format.clusters,
            next_free: 2,
            short_names: 0,
            fragment: format.fragment,
            directory_stride: format.directory_stride,
        };
        builder.set_fat(0, 0x0fff_fff8);
        builder.set_fat(1, 0x0fff_ffff);
        builder
    }

    fn set_fat(&mut self, cluster: u32, value: u32) {
        for copy in 0..self.fats {
            let base = self.fat_start + copy * self.fat_bytes;
            match self.fat_type {
                FatType::Fat12 => {
                    let at = base + u64::from(cluster) + u64::from(cluster / 2);
                    let old = u16::from_le_bytes([self.image.get(at), self.image.get(at + 1)]);
                    let value = (value & 0xfff) as u16;
                    let new = if cluster % 2 == 1 {
                        (old & 0x000f) | (value << 4)
                    } else {
                        (old & 0xf000) | value
                    };
                    self.image.put(at, &new.to_le_bytes());
                }
                FatType::Fat16 => self
                    .image
                    .put(base + 2 * u64::from(cluster), &(value as u16).to_le_bytes()),
                FatType::Fat32 => self.image.put(
                    base + 4 * u64::from(cluster),
                    &(value & 0x0fff_ffff).to_le_bytes(),
                ),
            }
        }
    }

    fn allocate(&mut self, bytes: u64, stride: u32) -> Vec<u32> {
        let count = bytes.div_ceil(self.cluster_bytes).max(1);
        let chain: Vec<u32> = (0..count as u32)
            .map(|index| self.next_free + index * stride)
            .collect();
        self.next_free = chain.last().unwrap() + 1;
        assert!(
            self.next_free - 2 <= self.clusters,
            "the test image is full"
        );
        for pair in chain.windows(2) {
            self.set_fat(pair[0], pair[1]);
        }
        self.set_fat(*chain.last().unwrap(), 0x0fff_ffff);
        chain
    }

    fn write_chain(&mut self, chain: &[u32], bytes: &[u8]) {
        for (cluster, chunk) in chain.iter().zip(bytes.chunks(self.cluster_bytes as usize)) {
            let at = self.data_start + u64::from(cluster - 2) * self.cluster_bytes;
            self.image.put(at, chunk);
        }
    }

    fn short_entry(&mut self, name: &str, naming: Naming) -> [u8; 11] {
        let mut short = [b' '; 11];
        match naming {
            Naming::ShortOnly { .. } => {
                let (base, extension) = name.split_once('.').unwrap_or((name, ""));
                short[..base.len()].copy_from_slice(base.to_ascii_uppercase().as_bytes());
                short[8..8 + extension.len()]
                    .copy_from_slice(extension.to_ascii_uppercase().as_bytes());
            }
            _ => {
                self.short_names += 1;
                let base = format!("N{:05}~1", self.short_names);
                short[..8].copy_from_slice(base.as_bytes());
            }
        }
        short
    }

    fn entries_for(&mut self, name: &str, naming: Naming, short: [u8; 11]) -> Vec<[u8; 32]> {
        if let Naming::ShortOnly { .. } = naming {
            return Vec::new();
        }
        let mut units: Vec<u16> = name.encode_utf16().collect();
        if !units.len().is_multiple_of(13) {
            units.push(0);
        }
        while !units.len().is_multiple_of(13) {
            units.push(0xffff);
        }
        let checksum = ShortEntry({
            let mut raw = [0u8; 32];
            raw[..11].copy_from_slice(&short);
            raw
        })
        .checksum();
        let checksum = match naming {
            Naming::BadChecksum => checksum.wrapping_add(1),
            _ => checksum,
        };
        let count = units.len() / 13;
        (1..=count)
            .rev()
            .map(|ordinal| {
                let mut raw = [0u8; 32];
                raw[0] = ordinal as u8 | if ordinal == count { 0x40 } else { 0 };
                raw[11] = 0x0f;
                raw[13] = checksum;
                let part = &units[(ordinal - 1) * 13..ordinal * 13];
                let places = (0..5)
                    .map(|i| 1 + 2 * i)
                    .chain((0..6).map(|i| 14 + 2 * i))
                    .chain((0..2).map(|i| 28 + 2 * i));
                for (unit, place) in part.iter().zip(places) {
                    raw[place..place + 2].copy_from_slice(&unit.to_le_bytes());
                }
                raw
            })
            .collect()
    }

    fn directory_entry(
        &self,
        short: [u8; 11],
        attributes: u8,
        cluster: u32,
        size: u32,
        naming: Naming,
    ) -> [u8; 32] {
        let mut raw = [0u8; 32];
        raw[..11].copy_from_slice(&short);
        raw[11] = attributes;
        if let Naming::ShortOnly {
            lower_base,
            lower_extension,
        } = naming
        {
            raw[12] = (u8::from(lower_base) * 0x08) | (u8::from(lower_extension) * 0x10);
        }
        raw[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        raw[22..24].copy_from_slice(&MODIFIED_TIME.to_le_bytes());
        raw[24..26].copy_from_slice(&MODIFIED_DATE.to_le_bytes());
        raw[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        raw[28..32].copy_from_slice(&size.to_le_bytes());
        raw
    }

    fn write_directory(
        &mut self,
        items: &[(String, Item, Naming)],
        own: Option<(Vec<u32>, u32)>,
        prefix: Vec<[u8; 32]>,
    ) {
        let mut entries = prefix;
        for (name, item, naming) in items {
            let short = self.short_entry(name, *naming);
            let lfn = self.entries_for(name, *naming, short);
            let parent = own.as_ref().map_or(0, |(chain, _)| chain[0]);
            let entry = match item {
                Item::File(bytes) if bytes.is_empty() => {
                    self.directory_entry(short, 0x20, 0, 0, *naming)
                }
                Item::File(bytes) => {
                    let stride = if self.fragment { 3 } else { 1 };
                    let chain = self.allocate(bytes.len() as u64, stride);
                    self.write_chain(&chain, bytes);
                    self.directory_entry(short, 0x20, chain[0], bytes.len() as u32, *naming)
                }
                Item::Directory(children) => {
                    let needed = 2 + entry_count(children);
                    let chain = self.allocate(needed * 32, self.directory_stride);
                    let mut dot = self.directory_entry(*b".          ", 0x10, chain[0], 0, *naming);
                    dot[12] = 0;
                    let parent_cluster = match (self.fat_type, parent) {
                        (FatType::Fat32, cluster) if Some(cluster) == self.root_cluster() => 0,
                        (_, cluster) => cluster,
                    };
                    let mut dot_dot =
                        self.directory_entry(*b"..         ", 0x10, parent_cluster, 0, *naming);
                    dot_dot[12] = 0;
                    let first = chain[0];
                    self.write_directory(children, Some((chain, parent)), vec![dot, dot_dot]);
                    self.directory_entry(short, 0x10, first, 0, *naming)
                }
            };
            entries.extend(lfn);
            entries.push(entry);
        }
        let bytes: Vec<u8> = entries.concat();
        match own {
            Some((chain, _)) => self.write_chain(&chain, &bytes),
            None => {
                let at = self.root_start;
                self.image.put(at, &bytes);
            }
        }
    }

    fn root_cluster(&self) -> Option<u32> {
        (self.fat_type == FatType::Fat32).then_some(2)
    }

    fn build(mut self, items: &[(String, Item, Naming)]) -> Image {
        let mut label = [0u8; 32];
        label[..11].copy_from_slice(b"EGG TEST   ");
        label[11] = 0x08;
        match self.root_cluster() {
            Some(_) => {
                let needed = (1 + entry_count(items)) * 32;
                let chain = self.allocate(needed, 1);
                assert_eq!(chain[0], 2);
                self.write_directory(items, Some((chain, 0)), vec![label]);
            }
            None => self.write_directory(items, None, vec![label]),
        }
        self.image
    }
}

fn entry_count(items: &[(String, Item, Naming)]) -> u64 {
    items
        .iter()
        .map(|(name, _, naming)| match naming {
            Naming::ShortOnly { .. } => 1,
            _ => 1 + (name.encode_utf16().count() as u64).div_ceil(13),
        })
        .sum()
}

fn file(name: &str, bytes: &[u8]) -> (String, Item, Naming) {
    (name.into(), Item::File(bytes.to_vec()), Naming::Long)
}

fn folder(name: &str, items: Vec<(String, Item, Naming)>) -> (String, Item, Naming) {
    (name.into(), Item::Directory(items), Naming::Long)
}

fn pattern(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|index| (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3] ^ seed)
        .collect()
}

const LONG_FOLDER: &str = "A folder with a long name";

fn tree() -> Vec<(String, Item, Naming)> {
    vec![
        file("readme.txt", b"hello from the host folder\n"),
        folder(
            LONG_FOLDER,
            vec![
                file("program", &pattern(20_000, 1)),
                file("empty", b""),
                folder("deeper", vec![file("note.md", b"deep\n")]),
            ],
        ),
        file("grüße aus EggOS 🥚.txt", b"utf-16\n"),
        (
            "SHORT.TXT".into(),
            Item::File(b"short\n".to_vec()),
            Naming::ShortOnly {
                lower_base: false,
                lower_extension: false,
            },
        ),
        (
            "lower.TXT".into(),
            Item::File(b"lower\n".to_vec()),
            Naming::ShortOnly {
                lower_base: true,
                lower_extension: false,
            },
        ),
        (
            "orphaned long name.bin".into(),
            Item::File(b"orphan\n".to_vec()),
            Naming::BadChecksum,
        ),
        file(&"x".repeat(255), b"longest\n"),
        file(&"é".repeat(200), b"too long as utf-8\n"),
        file("big.bin", &pattern(150_000, 7)),
    ]
}

fn mounted_with(image: Image, probes: &Probes) -> &'static Fat {
    let disk = Box::new(MemoryDisk {
        image,
        probes: probes.clone(),
    });
    match Fat::mount(disk, test_deadline, Sleeping::spinning(), (254, 0)) {
        Ok(fat) => Box::leak(Box::new(fat)),
        Err((error, _)) => panic!("not mounted: {error:?}"),
    }
}

fn mounted(format: Format) -> &'static Fat {
    mounted_with(Builder::new(&format).build(&tree()), &Probes::default())
}

fn names(fat: &Fat, directory: u64) -> Vec<String> {
    (0..)
        .map_while(|position| fat.entry(directory, position))
        .map(|entry| String::from_utf8(entry.name.as_bytes().to_vec()).unwrap())
        .collect()
}

fn read_all(fat: &Fat, node: u64, chunk: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = vec![0u8; chunk];
    loop {
        let read = fat.read_into(node, out.len() as u64, &mut buffer).unwrap();
        if read == 0 {
            return out;
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

fn path(fat: &'static Fat, path: &str) -> Node {
    let root = Node::root_of(fat);
    resolve(root, root, path.as_bytes()).unwrap()
}

fn check_tree(fat: &'static Fat) {
    let expected_root = [
        "readme.txt",
        LONG_FOLDER,
        "grüße aus EggOS 🥚.txt",
        "SHORT.TXT",
        "lower.TXT",
        "N00008~1",
        &"x".repeat(255),
        "N00010~1",
        "big.bin",
    ];
    assert_eq!(names(fat, ROOT), expected_root);
    let readme = path(fat, "/readme.txt");
    assert_eq!(
        read_all(fat, readme.id(), 4096),
        b"hello from the host folder\n"
    );
    let folder = path(fat, "/A folder with a long name");
    assert!(folder.is_directory());
    assert_eq!(names(fat, folder.id()), ["program", "empty", "deeper"]);
    let program = path(fat, "/a FOLDER with a long NAME/Program");
    assert_eq!(read_all(fat, program.id(), 4096), pattern(20_000, 1));
    assert_eq!(program.attributes().size, 20_000);
    assert_eq!(
        read_all(fat, path(fat, "/A folder with a long name/empty").id(), 512),
        b""
    );
    let note = path(fat, "/A folder with a long name/deeper/note.md");
    assert_eq!(read_all(fat, note.id(), 100), b"deep\n");
    assert_eq!(
        path(fat, "/A folder with a long name/deeper/../program"),
        program
    );
    assert_eq!(
        path(fat, "/A folder with a long name/deeper/../.."),
        Node::root_of(fat)
    );
    let mut buffer = [0u8; crate::vfs::PATH_MAX];
    let start = note.path_into(&mut buffer).unwrap();
    assert_eq!(
        &buffer[start..],
        b"/A folder with a long name/deeper/note.md"
    );
    assert_eq!(
        read_all(fat, path(fat, "/grüße aus eggos 🥚.txt").id(), 3),
        b"utf-16\n"
    );
    assert_eq!(read_all(fat, path(fat, "/short.txt").id(), 64), b"short\n");
    assert_eq!(read_all(fat, path(fat, "/LOWER.txt").id(), 64), b"lower\n");
    assert_eq!(read_all(fat, path(fat, "/n00008~1").id(), 64), b"orphan\n");
    let root = Node::root_of(fat);
    assert_eq!(
        resolve(root, root, b"/orphaned long name.bin"),
        Err(crate::vfs::Error::NotFound)
    );
    assert_eq!(
        read_all(fat, path(fat, &"X".repeat(255)).id(), 64),
        b"longest\n"
    );
    assert_eq!(
        read_all(fat, path(fat, "/N00010~1").id(), 64),
        b"too long as utf-8\n"
    );
    assert_eq!(
        read_all(fat, path(fat, "/big.bin").id(), 4096),
        pattern(150_000, 7)
    );
    assert_eq!(
        resolve(root, root, b"/readme.txt/x"),
        Err(crate::vfs::Error::NotDirectory)
    );
    assert_eq!(
        resolve(root, root, b"/missing"),
        Err(crate::vfs::Error::NotFound)
    );
}

#[test]
fn a_fat16_volume_lists_reads_and_walks_paths_by_long_and_short_names() {
    let fat = mounted(Format::of(FatType::Fat16));
    assert_eq!(fat.fat_type(), FatType::Fat16);
    assert_eq!(fat.label(), b"EGG TEST");
    check_tree(fat);
}

#[test]
fn a_fat32_volume_in_the_first_partition_reads_the_same() {
    let fat = mounted(Format {
        partition_at: Some(63),
        ..Format::of(FatType::Fat32)
    });
    assert_eq!(fat.fat_type(), FatType::Fat32);
    check_tree(fat);
}

#[test]
fn a_fat12_volume_with_larger_clusters_reads_the_same() {
    let fat = mounted(Format {
        sectors_per_cluster: 4,
        partition_at: Some(1),
        ..Format::of(FatType::Fat12)
    });
    assert_eq!(fat.fat_type(), FatType::Fat12);
    check_tree(fat);
}

#[test]
fn a_file_on_a_scattered_chain_reads_whole_in_any_chunk_size() {
    for fat_type in [FatType::Fat12, FatType::Fat16, FatType::Fat32] {
        let fat = mounted(Format::of(fat_type));
        let big = path(fat, "/big.bin");
        for chunk in [1, 100, 511, 512, 4096, 70_000, 200_000] {
            assert_eq!(
                read_all(fat, big.id(), chunk),
                pattern(150_000, 7),
                "{fat_type:?}, chunks of {chunk}"
            );
        }
        let mut middle = [0u8; 1000];
        let read = fat.read_into(big.id(), 99_999, &mut middle).unwrap();
        assert!(read > 0);
        assert_eq!(middle[..read], pattern(150_000, 7)[99_999..99_999 + read]);
        assert_eq!(fat.read_into(big.id(), 150_000, &mut middle), Ok(0));
        assert_eq!(fat.read_into(big.id(), u64::MAX, &mut middle), Ok(0));
    }
}

#[test]
fn contiguous_clusters_are_read_in_one_request() {
    let probes = Probes::default();
    let image = Builder::new(&Format {
        fragment: false,
        sectors_per_cluster: 8,
        ..Format::of(FatType::Fat16)
    })
    .build(&tree());
    let fat = mounted_with(image, &probes);
    let big = path(fat, "/big.bin");
    let mut buffer = vec![0u8; 150_000];
    fat.read_into(big.id(), 0, &mut buffer).unwrap();
    let before = probes.reads.load(Ordering::Relaxed);
    let read = fat.read_into(big.id(), 0, &mut buffer).unwrap();
    assert_eq!(read, 150_000 / SECTOR_SIZE * SECTOR_SIZE);
    assert_eq!(probes.reads.load(Ordering::Relaxed) - before, 1);
    assert_eq!(buffer[..read], pattern(150_000, 7)[..read]);
}

#[test]
fn a_fat32_volume_without_mirroring_reads_its_active_fat() {
    let format = Format::of(FatType::Fat32);
    let mut image = Builder::new(&format).build(&tree());
    let fat_bytes = (70_002u64 * 4).div_ceil(512) * 512;
    let fat_start = 32 * 512;
    for offset in 0..fat_bytes {
        let byte = image.get(fat_start + offset);
        image.put(fat_start + offset, &[0]);
        image.put(fat_start + fat_bytes + offset, &[byte]);
    }
    image.put(40, &(0x80u16 | 1).to_le_bytes());
    let fat = mounted_with(image.clone(), &Probes::default());
    check_tree(fat);
    image.put(40, &(0x80u16 | 2).to_le_bytes());
    let disk = Box::new(MemoryDisk {
        image,
        probes: Probes::default(),
    });
    assert!(matches!(
        Fat::mount(disk, test_deadline, Sleeping::spinning(), (254, 0)),
        Err((MountError::NotFat, _))
    ));
}

#[test]
fn long_names_across_scattered_directory_clusters_are_read_in_linear_time() {
    const FILES: usize = 1500;
    let name = |number: usize| format!("a file with a rather long name, number {number:05}");
    let files = (0..FILES).map(|number| file(&name(number), b"")).collect();
    let image = Builder::new(&Format {
        directory_stride: 128,
        ..Format::of(FatType::Fat32)
    })
    .build(&[folder("many", files)]);
    let probes = Probes::default();
    let fat = mounted_with(image, &probes);
    let many = path(fat, "/many").id();
    let clusters = (2 + FILES as u64 * 5).div_ceil(16);
    let before = probes.reads.load(Ordering::Relaxed);
    let last = fat.lookup(many, name(FILES - 1).as_bytes());
    assert!(last.is_some());
    let lookup_reads = probes.reads.load(Ordering::Relaxed) - before;
    assert!(
        lookup_reads as u64 <= 3 * clusters,
        "{lookup_reads} reads for {clusters} clusters"
    );
    let before = probes.reads.load(Ordering::Relaxed);
    let listed = names(fat, many);
    assert_eq!(listed.len(), FILES);
    assert_eq!(listed[FILES - 1], name(FILES - 1));
    let listing_reads = probes.reads.load(Ordering::Relaxed) - before;
    assert!(
        listing_reads as u64 <= 3 * clusters,
        "{listing_reads} reads for {clusters} clusters"
    );
}

#[test]
fn listing_positions_give_the_same_entries_in_any_order() {
    let fat = mounted(Format::of(FatType::Fat16));
    let in_order: Vec<DirectoryEntry> = (0..)
        .map_while(|position| fat.entry(ROOT, position))
        .collect();
    assert_eq!(in_order.len(), 9);
    for position in [5, 0, 8, 8, 3, 4, 1, 7, 2, 6] {
        assert_eq!(fat.entry(ROOT, position), Some(in_order[position as usize]));
    }
    assert_eq!(fat.entry(ROOT, 9), None);
    let folder = path(fat, "/A folder with a long name");
    assert_eq!(fat.entry(folder.id(), 1).unwrap().name.as_bytes(), b"empty");
    assert_eq!(fat.entry(ROOT, 2), Some(in_order[2]));
    let readme = path(fat, "/readme.txt");
    assert_eq!(fat.entry(readme.id(), 0), None);
}

#[test]
fn attributes_report_kind_size_permissions_and_the_write_time() {
    let fat = mounted(Format::of(FatType::Fat32));
    let root = fat.attributes(ROOT);
    assert_eq!(root.kind, NodeKind::Directory);
    assert_eq!(root.file_id, ROOT_FILE_ID);
    let readme = path(fat, "/readme.txt").attributes();
    assert_eq!(readme.kind, NodeKind::Regular);
    assert_eq!(readme.size, 27);
    assert_eq!(readme.permissions, 0o755);
    assert_eq!(readme.modified, MODIFIED_UNIX);
    assert_ne!(readme.file_id, 0);
    assert_ne!(readme.file_id, ROOT_FILE_ID);
    let folder = path(fat, "/A folder with a long name").attributes();
    assert_eq!(
        (folder.kind, folder.size, folder.links),
        (NodeKind::Directory, 0, 1)
    );
    assert_eq!(fat.device(), (254, 0));
    assert_eq!(fat.kind(ROOT), NodeKind::Directory);
}

#[test]
fn fat_times_are_utc_seconds_since_the_epoch() {
    assert_eq!(unix_time(MODIFIED_DATE, MODIFIED_TIME), MODIFIED_UNIX);
    assert_eq!(unix_time((1 << 5) | 1, 0), 315_532_800);
    assert_eq!(unix_time(0, 0), 0);
    assert_eq!(unix_time((10 << 9) | (13 << 5) | 1, 0), 0);
}

#[test]
fn every_change_is_refused_as_read_only() {
    let fat = mounted(Format::of(FatType::Fat16));
    let readme = path(fat, "/readme.txt").id();
    assert!(!fat.is_writable());
    assert_eq!(
        fat.write(readme, WriteAt::End, b"x", 0),
        Err(Refused::ReadOnly)
    );
    assert_eq!(fat.truncate(readme, 0, 0), Err(Refused::ReadOnly));
    assert_eq!(
        fat.create(ROOT, b"new", NodeKind::Regular, 0o644, 0),
        Err(Refused::ReadOnly)
    );
    assert_eq!(
        fat.remove(ROOT, b"readme.txt", false, 0),
        Err(Refused::ReadOnly)
    );
    assert_eq!(fat.set_modified(readme, 0), Err(Refused::ReadOnly));
    assert_eq!(fat.read(readme, 0, 10), b"");
    assert!(fat.copies_contents());
}

#[test]
fn a_disk_that_fails_reads_as_storage_errors_and_finds_nothing() {
    let probes = Probes::default();
    let fat = mounted_with(
        Builder::new(&Format::of(FatType::Fat16)).build(&tree()),
        &probes,
    );
    let big = path(fat, "/big.bin").id();
    probes.failing.store(true, Ordering::Relaxed);
    let mut buffer = [0u8; 4096];
    assert_eq!(fat.read_into(big, 8192, &mut buffer), Err(StorageError));
    assert_eq!(fat.lookup(ROOT, b"never-seen"), None);
    probes.failing.store(false, Ordering::Relaxed);
    let read = fat.read_into(big, 8192, &mut buffer).unwrap();
    assert!(read > 0);
    assert_eq!(buffer[..read], pattern(150_000, 7)[8192..8192 + read]);
}

#[test]
fn a_disk_without_a_fat_volume_is_given_back() {
    let probes = Probes::default();
    let blank = Box::new(MemoryDisk {
        image: Image::new(4096),
        probes: probes.clone(),
    });
    let Err((error, disk)) = Fat::mount(blank, test_deadline, Sleeping::spinning(), (254, 0))
    else {
        panic!("a blank disk mounted");
    };
    assert_eq!(error, MountError::NotFat);
    assert_eq!(disk.sectors(), 4096);
    probes.failing.store(true, Ordering::Relaxed);
    let failing = Box::new(MemoryDisk {
        image: Image::new(4096),
        probes,
    });
    assert!(matches!(
        Fat::mount(failing, test_deadline, Sleeping::spinning(), (254, 0)),
        Err((MountError::Unreadable, _))
    ));
}

#[test]
fn a_volume_larger_than_its_disk_or_with_a_short_fat_is_refused() {
    let mut image = Builder::new(&Format::of(FatType::Fat16)).build(&tree());
    image.sectors -= 1;
    let disk = Box::new(MemoryDisk {
        image: image.clone(),
        probes: Probes::default(),
    });
    assert!(matches!(
        Fat::mount(disk, test_deadline, Sleeping::spinning(), (254, 0)),
        Err((MountError::NotFat, _))
    ));
    image.sectors += 1;
    image.put(22, &1u16.to_le_bytes());
    let disk = Box::new(MemoryDisk {
        image,
        probes: Probes::default(),
    });
    assert!(matches!(
        Fat::mount(disk, test_deadline, Sleeping::spinning(), (254, 0)),
        Err((MountError::NotFat, _))
    ));
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn walk(fat: &Fat, directory: u64, depth: usize, budget: &mut usize) {
    let mut position = 0;
    while let Some(entry) = fat.entry(directory, position) {
        position += 1;
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return;
        }
        let _ = fat.attributes(entry.node);
        let _ = fat.name(entry.node);
        let _ = fat.parent(entry.node);
        let _ = fat.lookup(directory, entry.name.as_bytes());
        match entry.kind {
            NodeKind::Directory if depth < 6 => walk(fat, entry.node, depth + 1, budget),
            NodeKind::Regular => {
                let mut buffer = [0u8; 3000];
                let mut offset = 0;
                while let Ok(read @ 1..) = fat.read_into(entry.node, offset, &mut buffer) {
                    offset += read as u64;
                    *budget = budget.saturating_sub(1);
                    if *budget == 0 {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

#[test]
fn corrupt_volumes_never_panic() {
    let small = vec![
        file("a.txt", &pattern(3000, 2)),
        folder(
            "dir",
            vec![file("b", &pattern(1500, 3)), folder("c", vec![])],
        ),
        file("long name here.bin", &pattern(9000, 4)),
    ];
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    for fat_type in [FatType::Fat12, FatType::Fat16, FatType::Fat32] {
        let pristine = Builder::new(&Format::of(fat_type)).build(&small);
        let interesting: Vec<u64> = pristine.written.keys().copied().collect();
        for _ in 0..300 {
            let mut image = pristine.clone();
            for _ in 0..(1 + random.next() % 24) {
                let sector = interesting[(random.next() % interesting.len() as u64) as usize];
                let at = sector * 512 + random.next() % 512;
                image.put(at, &[random.next() as u8]);
            }
            let disk = Box::new(MemoryDisk {
                image,
                probes: Probes::default(),
            });
            if let Ok(fat) = Fat::mount(disk, test_deadline, Sleeping::spinning(), (254, 0)) {
                let mut budget = 2000;
                walk(&fat, ROOT, 0, &mut budget);
            }
        }
    }
}
