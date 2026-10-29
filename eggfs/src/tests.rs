use crate::layout::{BLOCK_SIZE, Geometry, INODE_SIZE, Inode, InodeKind};
use crate::memory::MemoryDisk;
use crate::{Contents, Error, Kind, MountError, Volume, format, probe};
use std::collections::BTreeMap;

type Tree = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

fn formatted(blocks: usize) -> MemoryDisk {
    let mut disk = MemoryDisk::new(blocks);
    format(&mut disk, 1_700_000_000).unwrap();
    disk
}

fn mounted(blocks: usize) -> Volume<MemoryDisk> {
    Volume::mount(formatted(blocks)).unwrap().0
}

fn remount(volume: Volume<MemoryDisk>) -> Volume<MemoryDisk> {
    let disk = MemoryDisk::from_image(volume.into_disk().image().to_vec());
    Volume::mount(disk).unwrap().0
}

fn read_all(volume: &mut Volume<MemoryDisk>, node: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 3 * BLOCK_SIZE + 100];
    loop {
        let length = (out.len() % 7000 + 1).min(buffer.len());
        let read = volume
            .read(node, out.len() as u64, &mut buffer[..length])
            .unwrap();
        if read == 0 {
            return out;
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

fn tree(volume: &mut Volume<MemoryDisk>) -> Tree {
    let mut tree = Tree::new();
    let mut pending = vec![(Vec::new(), volume.root())];
    while let Some((path, directory)) = pending.pop() {
        let mut position = 0;
        while let Some(entry) = volume.entry(directory, position).unwrap() {
            let mut child = path.clone();
            child.push(b'/');
            child.extend_from_slice(entry.name());
            match crate::kind_of(entry.node) {
                Kind::Directory => {
                    tree.insert(child.clone(), None);
                    pending.push((child, entry.node));
                }
                Kind::Regular => {
                    let contents = read_all(volume, entry.node);
                    tree.insert(child, Some(contents));
                }
            }
            position += 1;
        }
    }
    tree
}

fn pattern(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

#[test]
fn a_formatted_disk_mounts_empty_and_checks_clean() {
    let mut disk = MemoryDisk::new(1024);
    assert_eq!(probe(&mut disk), Ok(Contents::Blank));
    format(&mut disk, 5).unwrap();
    assert_eq!(probe(&mut disk), Ok(Contents::EggFs));
    let (mut volume, mounted) = Volume::mount(disk).unwrap();
    assert_eq!(mounted.recovered_blocks, 0);
    let report = volume.check().unwrap();
    assert_eq!((report.directories, report.files), (1, 0));
    let root = volume.root();
    assert_eq!(volume.parent(root), Ok(root));
    let attributes = volume.attributes(root).unwrap();
    assert_eq!(
        (attributes.kind, attributes.links, attributes.permissions),
        (Kind::Directory, 2, 0o755)
    );
    assert_eq!(volume.entry(root, 0), Ok(None));
}

#[test]
fn disks_are_told_apart_by_what_they_hold() {
    let mut disk = MemoryDisk::new(256);
    disk.image_mut()[40 * BLOCK_SIZE] = 1;
    assert_eq!(probe(&mut disk), Ok(Contents::Blank));
    disk.image_mut()[31 * BLOCK_SIZE + 7] = 1;
    assert_eq!(probe(&mut disk), Ok(Contents::Other));
    let mut fat = MemoryDisk::new(256);
    fat.image_mut()[0] = 0xeb;
    fat.image_mut()[510] = 0x55;
    assert_eq!(probe(&mut fat), Ok(Contents::Other));
    assert!(matches!(Volume::mount(fat), Err((MountError::NotEggFs, _))));
    let mut interrupted = MemoryDisk::new(256);
    interrupted.cut_power_after(3);
    assert_eq!(format(&mut interrupted, 0), Err(crate::FormatError::Io));
    let mut after = interrupted.after_power_cut(|_, _| true);
    assert_eq!(probe(&mut after), Ok(Contents::Blank));
    assert_eq!(
        format(&mut MemoryDisk::new(255), 0),
        Err(crate::FormatError::TooSmall)
    );
}

#[test]
fn files_and_directories_are_made_written_renamed_and_removed() {
    let mut volume = mounted(2048);
    let root = volume.root();
    let home = volume
        .create(root, b"home", Kind::Directory, 0o755, 10)
        .unwrap();
    let file = volume
        .create(home, b"a.txt", Kind::Regular, 0o644, 11)
        .unwrap();
    assert_eq!(
        volume.create(home, b"a.txt", Kind::Regular, 0o644, 11),
        Err(Error::Exists)
    );
    for bad in [&b""[..], b".", b".."] {
        assert_eq!(
            volume.create(home, bad, Kind::Regular, 0, 0),
            Err(Error::Exists)
        );
    }
    assert_eq!(
        volume.create(home, b"a/b", Kind::Regular, 0, 0),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        volume.create(home, &[b'x'; 256], Kind::Regular, 0, 0),
        Err(Error::NameTooLong)
    );
    assert_eq!(
        volume.create(file, b"x", Kind::Regular, 0, 0),
        Err(Error::NotDirectory)
    );
    assert_eq!(volume.write(file, Some(0), b"hello\n", 12), Ok((6, 6)));
    assert_eq!(volume.write(file, None, b"more\n", 13), Ok((5, 11)));
    assert_eq!(read_all(&mut volume, file), b"hello\nmore\n");
    assert_eq!(volume.lookup(home, b"a.txt"), Ok(Some(file)));
    assert_eq!(volume.parent(file), Ok(home));
    let mut name = [0u8; crate::MAX_NAME];
    let length = volume.name(file, &mut name).unwrap();
    assert_eq!(&name[..length], b"a.txt");
    let attributes = volume.attributes(file).unwrap();
    assert_eq!(
        (attributes.size, attributes.modified, attributes.links),
        (11, 13, 1)
    );
    assert_eq!(volume.attributes(root).unwrap().links, 3);
    let deep = volume
        .create(home, b"deep", Kind::Directory, 0o700, 14)
        .unwrap();
    volume
        .rename(home, b"a.txt", deep, b"b.txt", false, 15)
        .unwrap();
    assert_eq!(volume.lookup(home, b"a.txt"), Ok(None));
    assert_eq!(volume.lookup(deep, b"b.txt"), Ok(Some(file)));
    assert_eq!(volume.parent(file), Ok(deep));
    assert_eq!(
        volume.rename(root, b"home", deep, b"x", false, 0),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        volume.rename(home, b"deep", home, b"..", false, 0),
        Err(Error::Busy)
    );
    assert_eq!(volume.remove(home, b"deep", true, 16), Err(Error::NotEmpty));
    assert_eq!(
        volume.remove(home, b"deep", false, 16),
        Err(Error::IsDirectory)
    );
    assert_eq!(
        volume.remove(deep, b"b.txt", true, 16),
        Err(Error::NotDirectory)
    );
    volume.check().unwrap();
    volume.commit().unwrap();
    let mut volume = remount(volume);
    let home = volume.lookup(volume.root(), b"home").unwrap().unwrap();
    let deep = volume.lookup(home, b"deep").unwrap().unwrap();
    let file = volume.lookup(deep, b"b.txt").unwrap().unwrap();
    assert_eq!(read_all(&mut volume, file), b"hello\nmore\n");
    volume.remove(deep, b"b.txt", false, 17).unwrap();
    assert_eq!(volume.read(file, 0, &mut [0; 4]), Err(Error::NotFound));
    volume.remove(home, b"deep", true, 18).unwrap();
    assert_eq!(volume.entry(home, 0), Ok(None));
    volume.check().unwrap();
}

#[test]
fn renames_replace_as_linux_s_do() {
    let mut volume = mounted(2048);
    let root = volume.root();
    let a = volume.create(root, b"a", Kind::Regular, 0o644, 0).unwrap();
    let b = volume.create(root, b"b", Kind::Regular, 0o644, 0).unwrap();
    volume.write(a, None, b"new", 0).unwrap();
    volume.write(b, None, &pattern(1, 50_000), 0).unwrap();
    assert_eq!(
        volume.rename(root, b"a", root, b"b", true, 0),
        Err(Error::Exists)
    );
    let (_, free_before) = volume.space();
    volume.rename(root, b"a", root, b"b", false, 1).unwrap();
    assert_eq!(volume.lookup(root, b"b"), Ok(Some(a)));
    assert_eq!(volume.lookup(root, b"a"), Ok(None));
    assert_eq!(read_all(&mut volume, a), b"new");
    assert!(volume.space().1 > free_before);
    let d = volume
        .create(root, b"d", Kind::Directory, 0o755, 0)
        .unwrap();
    let e = volume
        .create(root, b"e", Kind::Directory, 0o755, 0)
        .unwrap();
    volume
        .create(e, b"inside", Kind::Regular, 0o644, 0)
        .unwrap();
    assert_eq!(
        volume.rename(root, b"d", root, b"b", false, 0),
        Err(Error::NotDirectory)
    );
    assert_eq!(
        volume.rename(root, b"b", root, b"d", false, 0),
        Err(Error::IsDirectory)
    );
    assert_eq!(
        volume.rename(root, b"d", root, b"e", false, 0),
        Err(Error::NotEmpty)
    );
    volume.remove(e, b"inside", false, 0).unwrap();
    volume.rename(root, b"d", root, b"e", false, 0).unwrap();
    assert_eq!(volume.lookup(root, b"e"), Ok(Some(d)));
    assert_eq!(volume.attributes(root).unwrap().links, 3);
    volume.rename(root, b"e", root, b"e", false, 0).unwrap();
    volume.check().unwrap();
}

#[test]
fn large_sparse_and_overwritten_files_read_back_and_survive_a_remount() {
    let mut volume = mounted(8192);
    let root = volume.root();
    let file = volume
        .create(root, b"big", Kind::Regular, 0o644, 0)
        .unwrap();
    let big = pattern(7, 6 * 1024 * 1024 + 123);
    let mut done = 0;
    while done < big.len() {
        let chunk = (big.len() - done).min(4096 - done % 4096);
        done += volume
            .write(file, Some(done as u64), &big[done..done + chunk], 0)
            .unwrap()
            .0;
    }
    assert_eq!(read_all(&mut volume, file), big);
    let mut expected = big.clone();
    let patch = pattern(9, 70_000);
    volume.write(file, Some(5000), &patch, 1).unwrap();
    expected[5000..75_000].copy_from_slice(&patch);
    assert_eq!(read_all(&mut volume, file), expected);
    let sparse = volume
        .create(root, b"sparse", Kind::Regular, 0o600, 0)
        .unwrap();
    volume.write(sparse, Some(5_000_000), b"end", 0).unwrap();
    let back = read_all(&mut volume, sparse);
    assert_eq!(back.len(), 5_000_003);
    assert!(back[..5_000_000].iter().all(|&byte| byte == 0));
    volume.truncate(file, 4100, 2).unwrap();
    volume.truncate(file, 9000, 3).unwrap();
    let back = read_all(&mut volume, file);
    assert_eq!(&back[..4100], &expected[..4100]);
    assert!(back[4100..].iter().all(|&byte| byte == 0));
    volume.check().unwrap();
    volume.commit().unwrap();
    let mut volume = remount(volume);
    volume.check().unwrap();
    let file = volume.lookup(volume.root(), b"big").unwrap().unwrap();
    assert_eq!(read_all(&mut volume, file).len(), 9000);
}

#[test]
fn many_changes_commit_on_their_own_before_the_journal_fills() {
    let mut volume = mounted(4096);
    let root = volume.root();
    for index in 0..1000 {
        let name = format!("file number {index}");
        volume
            .create(root, name.as_bytes(), Kind::Regular, 0o644, 0)
            .unwrap();
    }
    assert!(volume.commits() > 0);
    volume.check().unwrap();
    assert_eq!(tree(&mut volume).len(), 1000);
    let mut volume = remount(volume);
    assert!(tree(&mut volume).len() < 1000);
    volume.check().unwrap();
}

#[test]
fn a_full_disk_refuses_with_no_space_and_removing_gives_it_back() {
    let mut volume = mounted(512);
    let root = volume.root();
    let file = volume
        .create(root, b"fill", Kind::Regular, 0o644, 0)
        .unwrap();
    let chunk = vec![0x5a; 4096];
    let mut written = 0u64;
    let error = loop {
        match volume.write(file, None, &chunk, 0) {
            Ok((count, _)) => written += count as u64,
            Err(error) => break error,
        }
    };
    assert_eq!(error, Error::NoSpace);
    assert!(written > 300 * 4096);
    assert_eq!(volume.attributes(file).unwrap().size, written);
    volume.check().unwrap();
    volume.commit().unwrap();
    let mut created = 0;
    loop {
        let name = format!("n{created}");
        match volume.create(root, name.as_bytes(), Kind::Regular, 0o644, 0) {
            Ok(_) => created += 1,
            Err(Error::NoSpace) => break,
            Err(error) => panic!("{error:?}"),
        }
        if created > 10_000 {
            panic!("inodes never ran out");
        }
    }
    volume.check().unwrap();
    volume.remove(root, b"fill", false, 0).unwrap();
    let again = volume.create(root, b"again", Kind::Regular, 0o644, 0);
    assert!(again.is_ok() || again == Err(Error::NoSpace));
    volume.remove(root, b"n0", false, 0).unwrap();
    let again = volume
        .create(root, b"again2", Kind::Regular, 0o644, 0)
        .unwrap();
    assert_eq!(volume.write(again, None, &chunk, 0), Ok((4096, 4096)));
    volume.check().unwrap();
}

#[test]
fn a_committed_transaction_cut_before_its_blocks_land_is_recovered() {
    let mut volume = mounted(1024);
    let root = volume.root();
    volume
        .create(root, b"kept", Kind::Directory, 0o755, 0)
        .unwrap();
    volume.commit().unwrap();
    volume
        .create(root, b"new", Kind::Directory, 0o755, 0)
        .unwrap();
    let dirty = volume.cache.dirty_count() as u64;
    let journal_writes = 1 + dirty.div_ceil(16) + 1;
    volume.disk.cut_power_after(journal_writes);
    assert_eq!(volume.commit(), Err(Error::Io));
    assert_eq!(
        volume.create(root, b"refused", Kind::Regular, 0o644, 0),
        Err(Error::Io)
    );
    let disk = volume.into_disk();
    let (mut recovered, mounted) = Volume::mount(disk.after_power_cut(|_, _| true)).unwrap();
    assert!(mounted.recovered_blocks > 0);
    let root = recovered.root();
    assert!(recovered.lookup(root, b"new").unwrap().is_some());
    recovered.check().unwrap();
    let (mut lost, mounted) = Volume::mount(disk.after_power_cut(|_, _| false)).unwrap();
    assert_eq!(mounted.recovered_blocks, 0);
    let root = lost.root();
    assert!(lost.lookup(root, b"new").unwrap().is_none());
    assert!(lost.lookup(root, b"kept").unwrap().is_some());
    lost.check().unwrap();
    let (mut again, mounted) = Volume::mount(recovered.into_disk()).unwrap();
    assert_eq!(mounted.recovered_blocks, 0);
    again.check().unwrap();
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

struct Workload {
    volume: Volume<MemoryDisk>,
    model: Tree,
    committed: Vec<Tree>,
    random: Random,
}

const DIRECTORIES: [&[u8]; 3] = [b"", b"/d0", b"/d0/d1"];

impl Workload {
    fn node(&mut self, path: &[u8]) -> Option<u64> {
        let mut node = self.volume.root();
        for name in path
            .split(|&byte| byte == b'/')
            .filter(|name| !name.is_empty())
        {
            node = self.volume.lookup(node, name).ok()??;
        }
        Some(node)
    }

    fn files(&self) -> Vec<Vec<u8>> {
        self.model
            .iter()
            .filter(|(_, contents)| contents.is_some())
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn step(&mut self, index: usize) -> Result<(), Error> {
        let before = self.volume.commits();
        let model_before = self.model.clone();
        let result = self.operation(index);
        if self.volume.commits() > before {
            self.committed.push(model_before);
        }
        let changed = result?;
        if !changed {
            self.model = self.committed_view_or(self.model.clone());
        }
        if index % 6 == 5 {
            let before = self.volume.commits();
            self.volume.commit()?;
            if self.volume.commits() > before {
                self.committed.push(self.model.clone());
            }
        }
        Ok(())
    }

    fn committed_view_or(&self, model: Tree) -> Tree {
        model
    }

    fn split(path: &[u8]) -> (&[u8], &[u8]) {
        let at = path.iter().rposition(|&byte| byte == b'/').unwrap();
        (&path[..at], &path[at + 1..])
    }

    fn operation(&mut self, index: usize) -> Result<bool, Error> {
        let choice = self.random.below(12);
        let files = self.files();
        let directory = DIRECTORIES[self.random.below(3) as usize];
        let name = format!("f{}", self.random.below(4));
        let mut path = directory.to_vec();
        path.push(b'/');
        path.extend_from_slice(name.as_bytes());
        let seed = index as u64 + 1;
        match choice {
            0..=2 if directory.is_empty() || self.model.contains_key(directory) => {
                if self.model.contains_key(&path) {
                    return Ok(true);
                }
                let parent = self.node(directory).unwrap();
                self.volume
                    .create(parent, name.as_bytes(), Kind::Regular, 0o644, index as u64)?;
                self.model.insert(path, Some(Vec::new()));
            }
            3..=6 if !files.is_empty() => {
                let file = &files[self.random.below(files.len() as u64) as usize];
                let node = self.node(file).unwrap();
                let contents = self.model.get_mut(file).unwrap().as_mut().unwrap();
                let block_start = contents.len() as u64 / 4096 * 4096;
                let offset = match choice {
                    3 | 4 => contents.len() as u64,
                    _ => self.random.below(contents.len() as u64 + 1),
                };
                let room = 4096 - (offset % 4096) as usize;
                let length = (self.random.below(1500) as usize + 1).min(room);
                let bytes = pattern(seed, length);
                let _ = block_start;
                self.volume
                    .write(node, Some(offset), &bytes, index as u64)?;
                let end = offset as usize + length;
                if contents.len() < end {
                    contents.resize(end, 0);
                }
                contents[offset as usize..end].copy_from_slice(&bytes);
            }
            7 if !files.is_empty() => {
                let file = &files[self.random.below(files.len() as u64) as usize];
                let node = self.node(file).unwrap();
                let contents = self.model.get_mut(file).unwrap().as_mut().unwrap();
                let length = self.random.below(contents.len() as u64 + 3000);
                self.volume.truncate(node, length, index as u64)?;
                contents.resize(length as usize, 0);
            }
            8 if !files.is_empty() => {
                let file = files[self.random.below(files.len() as u64) as usize].clone();
                let (from_directory, from_name) = Self::split(&file);
                let from = self.node(from_directory).unwrap();
                let to_directory = DIRECTORIES[self.random.below(3) as usize];
                if !to_directory.is_empty() && !self.model.contains_key(to_directory) {
                    return Ok(true);
                }
                let to_name = format!("f{}", self.random.below(4));
                let mut target = to_directory.to_vec();
                target.push(b'/');
                target.extend_from_slice(to_name.as_bytes());
                if matches!(self.model.get(&target), Some(None)) {
                    return Ok(true);
                }
                let to = self.node(to_directory).unwrap();
                self.volume
                    .rename(from, from_name, to, to_name.as_bytes(), false, index as u64)?;
                let contents = self.model.remove(&file).unwrap();
                self.model.insert(target, contents);
            }
            9 if !files.is_empty() => {
                let file = files[self.random.below(files.len() as u64) as usize].clone();
                let (directory, name) = Self::split(&file);
                let parent = self.node(directory).unwrap();
                self.volume.remove(parent, name, false, index as u64)?;
                self.model.remove(&file);
            }
            10 => {
                let make = DIRECTORIES[1 + self.random.below(2) as usize];
                let (parent, name) = Self::split(make);
                if self.model.contains_key(make)
                    || (!parent.is_empty() && !self.model.contains_key(parent))
                {
                    return Ok(true);
                }
                let parent = self.node(parent).unwrap();
                self.volume
                    .create(parent, name, Kind::Directory, 0o755, index as u64)?;
                self.model.insert(make.to_vec(), None);
            }
            11 => {
                let remove = DIRECTORIES[1 + self.random.below(2) as usize];
                let empty = !self.model.keys().any(|path| {
                    path.len() > remove.len()
                        && path.starts_with(remove)
                        && path[remove.len()] == b'/'
                });
                if !self.model.contains_key(remove) || !empty {
                    return Ok(true);
                }
                let (parent, name) = Self::split(remove);
                let parent = self.node(parent).unwrap();
                self.volume.remove(parent, name, true, index as u64)?;
                self.model.remove(remove);
            }
            _ => {}
        }
        Ok(true)
    }
}

const CRASH_STEPS: usize = 240;

fn run_workload(disk: MemoryDisk, seed: u64) -> Workload {
    let volume = Volume::mount(disk).unwrap().0;
    let mut workload = Workload {
        volume,
        model: Tree::new(),
        committed: vec![Tree::new()],
        random: Random(seed),
    };
    for index in 0..CRASH_STEPS {
        if workload.step(index).is_err() {
            break;
        }
    }
    workload
}

#[test]
fn a_power_cut_after_any_write_recovers_a_committed_state() {
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0x1234_5678_9abc_def1,
        0x0bad_5eed_0000_0042,
    ] {
        let formatted = formatted(512);
        let base = formatted.image().to_vec();
        let reference = run_workload(MemoryDisk::from_image(base.clone()), seed);
        assert!(reference.committed.len() > 8);
        let writes = reference.volume.disk().writes();
        assert!(writes > 50);
        let mut seen_recovery = false;
        for cut in 0..=writes {
            let mut disk = MemoryDisk::from_image(base.clone());
            disk.cut_power_after(cut);
            let run = run_workload(disk, seed);
            let completed = run.volume.commits() as usize;
            let disk = run.volume.into_disk();
            let mut random = Random(cut * 7919 + 1);
            let variants = [
                disk.after_power_cut(|_, _| false),
                disk.after_power_cut(|_, _| true),
                disk.after_power_cut(|_, _| random.below(2) == 0),
            ];
            for (variant, crashed) in variants.into_iter().enumerate() {
                let (mut volume, mounted) = Volume::mount(crashed).unwrap_or_else(|(error, _)| {
                    panic!("seed {seed:#x}, cut {cut}, variant {variant}: {error:?}")
                });
                seen_recovery |= mounted.recovered_blocks > 0;
                if let Err(problem) = volume.check() {
                    panic!("seed {seed:#x}, cut {cut}, variant {variant}: {problem}");
                }
                let found = tree(&mut volume);
                let candidates =
                    &reference.committed[completed..(completed + 2).min(reference.committed.len())];
                assert!(
                    candidates.contains(&found),
                    "seed {seed:#x}, cut {cut}, variant {variant}: the state is no committed one"
                );
            }
        }
        assert!(seen_recovery);
    }
}

fn sample_image() -> (Geometry, Vec<u8>) {
    let mut volume = mounted(512);
    let root = volume.root();
    let directory = volume
        .create(root, b"dir", Kind::Directory, 0o755, 0)
        .unwrap();
    for index in 0..12u64 {
        let name = format!("file{index}");
        let file = volume
            .create(directory, name.as_bytes(), Kind::Regular, 0o644, 0)
            .unwrap();
        volume
            .write(
                file,
                None,
                &pattern(index + 1, (index as usize * 3000) % 20_000 + 10),
                0,
            )
            .unwrap();
    }
    let big = volume
        .create(root, b"big", Kind::Regular, 0o644, 0)
        .unwrap();
    volume.write(big, Some(60_000), b"x", 0).unwrap();
    volume.commit().unwrap();
    volume
        .create(root, b"late", Kind::Directory, 0o755, 0)
        .unwrap();
    volume.commit().unwrap();
    let geometry = volume.geometry;
    (geometry, volume.into_disk().image().to_vec())
}

fn exercise(mut volume: Volume<MemoryDisk>) {
    let _ = volume.check();
    let mut pending = vec![volume.root()];
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        visited += 1;
        if visited > 200 {
            break;
        }
        for position in 0..64 {
            let Ok(Some(entry)) = volume.entry(directory, position) else {
                break;
            };
            let _ = volume.attributes(entry.node);
            let _ = volume.parent(entry.node);
            let mut name = [0u8; crate::MAX_NAME];
            let _ = volume.name(entry.node, &mut name);
            match crate::kind_of(entry.node) {
                Kind::Directory => pending.push(entry.node),
                Kind::Regular => {
                    let mut buffer = [0u8; 9000];
                    let mut offset = 0;
                    while offset < 200_000 {
                        match volume.read(entry.node, offset, &mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => offset += read as u64,
                        }
                    }
                    let _ = volume.write(entry.node, Some(10), b"overwrite", 1);
                    let _ = volume.truncate(entry.node, 5, 1);
                }
            }
        }
        let _ = volume.create(directory, b"new", Kind::Regular, 0o644, 1);
        let _ = volume.rename(directory, b"new", volume.root(), b"moved", false, 1);
        let _ = volume.remove(directory, b"file3", false, 1);
    }
    let _ = volume.commit();
    let _ = volume.check();
}

#[test]
fn corrupt_images_never_panic_or_hang() {
    let (geometry, base) = sample_image();
    let metadata_end = (geometry.data_start as usize + 24) * BLOCK_SIZE;
    let mut random = Random(0xdead_beef_cafe_f00d);
    let mut mounted_count = 0;
    for iteration in 0..600 {
        let mut image = base.clone();
        let flips = 1 + random.below(12);
        for _ in 0..flips {
            let limit = match iteration % 3 {
                0 => image.len(),
                _ => metadata_end,
            };
            let at = BLOCK_SIZE + random.below((limit - BLOCK_SIZE) as u64) as usize;
            image[at] = random.next() as u8;
        }
        if iteration % 50 == 0 {
            let block = 1 + random.below(geometry.data_start as u64 + 20) as usize;
            for byte in &mut image[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE] {
                *byte = random.next() as u8;
            }
        }
        let Ok((volume, _)) = Volume::mount(MemoryDisk::from_image(image)) else {
            continue;
        };
        mounted_count += 1;
        exercise(volume);
    }
    assert!(mounted_count > 100);
    let _ = Geometry::decode(&base);
}

fn inode_bytes(geometry: &Geometry, number: u32) -> core::ops::Range<usize> {
    let (block, at) = geometry.inode_location(number);
    let start = block as usize * BLOCK_SIZE + at;
    start..start + INODE_SIZE
}

fn live_inodes(geometry: &Geometry, image: &[u8]) -> Vec<(u32, Inode)> {
    (1..geometry.inodes)
        .filter_map(|number| {
            let inode = Inode::decode(number, &image[inode_bytes(geometry, number)])?;
            (inode.kind != InodeKind::Free).then_some((number, inode))
        })
        .collect()
}

#[test]
fn hostile_inodes_with_valid_checksums_never_panic_or_hang() {
    let (geometry, base) = sample_image();
    let live = live_inodes(&geometry, &base);
    assert!(live.len() > 14, "the root, two directories and the files");
    let data_blocks = u64::from(geometry.blocks - geometry.data_start);
    let mut random = Random(0x0bad_cafe_f00d_d00d);
    let mut mounted_count = 0;
    for _ in 0..500 {
        let mut image = base.clone();
        for _ in 0..1 + random.below(4) {
            let (number, mut inode) = live[random.below(live.len() as u64) as usize];
            let wild = random.next() as u32;
            let data_block = geometry.data_start + random.below(data_blocks) as u32;
            let metadata_block = random.below(u64::from(geometry.data_start)) as u32;
            let slot = random.below(crate::layout::DIRECT_POINTERS as u64) as usize;
            let level = random.below(3) as usize;
            let other = live[random.below(live.len() as u64) as usize].0;
            let sizes = [
                u64::MAX,
                u64::from(wild) << 20,
                crate::volume::MAX_FILE_BYTES,
            ];
            match random.below(12) {
                0 => inode.direct[slot] = wild,
                1 => inode.direct[slot] = metadata_block,
                2 => inode.direct[slot] = geometry.blocks,
                3 => inode.indirect[level] = metadata_block,
                4 => {
                    inode.indirect[level] = data_block;
                    let at = data_block as usize * BLOCK_SIZE;
                    image[at..at + 4].copy_from_slice(&data_block.to_le_bytes());
                }
                5 => inode.indirect[level] = inode.direct[0],
                6 => inode.parent = [0, number, other, wild, geometry.inodes][slot % 5],
                7 => inode.entries = [u32::MAX, wild][slot % 2],
                8 => inode.size = sizes[slot % 3],
                9 => inode.child_directories = u32::MAX,
                10 => {
                    inode.kind = match inode.kind {
                        InodeKind::Directory => InodeKind::Regular,
                        _ => InodeKind::Directory,
                    }
                }
                _ => {
                    inode.size = (BLOCK_SIZE as u64) * u64::from(wild % 20_000);
                    inode.direct = [data_block; crate::layout::DIRECT_POINTERS];
                }
            }
            inode.encode(number, &mut image[inode_bytes(&geometry, number)]);
        }
        let Ok((volume, _)) = Volume::mount(MemoryDisk::from_image(image)) else {
            continue;
        };
        mounted_count += 1;
        exercise(volume);
    }
    assert!(mounted_count > 250);
}

#[test]
fn a_superblock_with_a_valid_checksum_and_impossible_geometry_is_refused() {
    let (geometry, base) = sample_image();
    let impossible = [
        Geometry {
            blocks: u32::MAX,
            ..geometry
        },
        Geometry {
            data_start: geometry.blocks,
            ..geometry
        },
        Geometry {
            inode_table_blocks: geometry.inode_table_blocks + 1,
            ..geometry
        },
        Geometry {
            inodes: 0,
            ..geometry
        },
        Geometry {
            journal_blocks: 1,
            ..geometry
        },
    ];
    for hostile in impossible {
        let mut image = base.clone();
        image[..BLOCK_SIZE].copy_from_slice(&hostile.encode()[..]);
        assert!(Geometry::decode(&image).is_none(), "{hostile:?}");
        assert!(Volume::mount(MemoryDisk::from_image(image)).is_err());
    }
}
