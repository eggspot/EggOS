use crate::layout::{BLOCK_SIZE, Geometry, INODE_SIZE, Inode, InodeKind};
use crate::memory::MemoryDisk;
use crate::{
    Contents, Error, Kind, MAX_LINK_TARGET, MAX_LINKS, MountError, VERSION, Volume, format, probe,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Directory,
    File(Vec<u8>),
    Link(Vec<u8>),
}

type Tree = BTreeMap<Vec<u8>, Item>;

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
                    tree.insert(child.clone(), Item::Directory);
                    pending.push((child, entry.node));
                }
                Kind::Regular => {
                    let contents = read_all(volume, entry.node);
                    tree.insert(child, Item::File(contents));
                }
                Kind::SymbolicLink => {
                    let target = volume.link_target(entry.node).unwrap();
                    tree.insert(child, Item::Link(target));
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
    assert_eq!((mounted.recovered_blocks, mounted.freed_orphans), (0, 0));
    assert_eq!(volume.version(), VERSION);
    let report = volume.check().unwrap();
    assert_eq!((report.directories, report.files), (1, 0));
    assert_eq!((report.symbolic_links, report.orphans), (0, 0));
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
            .filter(|(_, item)| matches!(item, Item::File(_)))
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn links(&self) -> Vec<Vec<u8>> {
        self.model
            .iter()
            .filter(|(_, item)| matches!(item, Item::Link(_)))
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn contents<'a>(model: &'a mut Tree, path: &[u8]) -> &'a mut Vec<u8> {
        match model.get_mut(path) {
            Some(Item::File(contents)) => contents,
            _ => panic!("not a file in the model"),
        }
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
        let choice = self.random.below(14);
        let files = self.files();
        let links = self.links();
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
                self.model.insert(path, Item::File(Vec::new()));
            }
            3..=6 if !files.is_empty() => {
                let file = &files[self.random.below(files.len() as u64) as usize];
                let node = self.node(file).unwrap();
                let contents = Self::contents(&mut self.model, file);
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
                let contents = Self::contents(&mut self.model, file);
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
                if matches!(self.model.get(&target), Some(Item::Directory)) {
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
                self.model.insert(make.to_vec(), Item::Directory);
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
            12 if directory.is_empty() || self.model.contains_key(directory) => {
                if self.model.contains_key(&path) {
                    return Ok(true);
                }
                let parent = self.node(directory).unwrap();
                let target = format!("../target {index}").into_bytes();
                self.volume
                    .symlink(parent, name.as_bytes(), &target, index as u64)?;
                self.model.insert(path, Item::Link(target));
            }
            13 if !links.is_empty() => {
                let link = links[self.random.below(links.len() as u64) as usize].clone();
                let (directory, name) = Self::split(&link);
                let parent = self.node(directory).unwrap();
                self.volume.remove(parent, name, false, index as u64)?;
                self.model.remove(&link);
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
                Kind::SymbolicLink => {
                    let _ = volume.link_target(entry.node);
                    let _ = volume.read(entry.node, 0, &mut [0u8; 64]);
                    let _ = volume.write(entry.node, None, b"refused", 1);
                }
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
                    let _ = volume.link(entry.node, volume.root(), b"alias", 1);
                    let _ = volume.hold(entry.node);
                }
            }
        }
        let _ = volume.create(directory, b"new", Kind::Regular, 0o644, 1);
        let _ = volume.symlink(directory, b"link", b"new", 1);
        let _ = volume.remove(volume.root(), b"alias", false, 1);
        let _ = volume.rename(directory, b"new", volume.root(), b"moved", false, 1);
        let _ = volume.remove(directory, b"file3", false, 1);
    }
    for position in 0..64 {
        let Ok(Some(entry)) = volume.entry(volume.root(), position) else {
            break;
        };
        let _ = volume.release(entry.node);
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
            match random.below(15) {
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
                11 => inode.next_orphan = [wild, other, number][slot % 3],
                12 => inode.more_names = [wild, 1, MAX_LINKS][slot % 3],
                13 => {
                    inode.orphan = true;
                    inode.kind = [InodeKind::SymbolicLink, inode.kind][slot % 2];
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

#[test]
fn a_work_disk_is_told_apart_by_its_marker_and_keeps_its_folder_across_a_remount() {
    let mut volume = mounted(1024);
    assert_eq!(crate::is_work_disk(&mut volume), Ok(false));
    assert_eq!(crate::work_folder(&mut volume), Err(Error::NotFound));
    let root = volume.root();
    volume
        .create(root, b"home", Kind::Directory, 0o755, 2)
        .unwrap();
    assert_eq!(crate::is_work_disk(&mut volume), Ok(false));
    let folder = crate::make_work_disk(&mut volume, 3).unwrap();
    assert_eq!(crate::kind_of(folder), Kind::Directory);
    assert_eq!(crate::make_work_disk(&mut volume, 4), Err(Error::Exists));
    let file = volume
        .create(folder, b"Cargo.toml", Kind::Regular, 0o644, 5)
        .unwrap();
    volume.write(file, Some(0), b"[package]\n", 5).unwrap();
    volume.commit().unwrap();
    let mut volume = remount(volume);
    assert_eq!(crate::is_work_disk(&mut volume), Ok(true));
    let folder = crate::work_folder(&mut volume).unwrap();
    let file = volume.lookup(folder, b"Cargo.toml").unwrap().unwrap();
    assert_eq!(read_all(&mut volume, file), b"[package]\n");
    volume.check().unwrap();
}

#[test]
fn a_directory_named_like_the_marker_does_not_make_a_work_disk() {
    let mut volume = mounted(1024);
    let root = volume.root();
    volume
        .create(root, crate::WORK_DISK_MARKER, Kind::Directory, 0o755, 1)
        .unwrap();
    assert_eq!(crate::is_work_disk(&mut volume), Ok(false));
    let mut volume = mounted(1024);
    let root = volume.root();
    volume
        .create(root, crate::WORK_DISK_MARKER, Kind::Regular, 0o444, 1)
        .unwrap();
    volume
        .create(root, crate::WORK_FOLDER, Kind::Regular, 0o644, 1)
        .unwrap();
    assert_eq!(crate::work_folder(&mut volume), Err(Error::NotDirectory));
}

fn as_version_1(image: &mut [u8]) {
    let mut geometry = Geometry::decode(&image[..BLOCK_SIZE]).unwrap();
    geometry.version = 1;
    image[..BLOCK_SIZE].copy_from_slice(&geometry.encode()[..]);
}

fn version_1_volume() -> Volume<MemoryDisk> {
    let mut volume = mounted(1024);
    let root = volume.root();
    let directory = volume
        .create(root, b"d", Kind::Directory, 0o755, 1)
        .unwrap();
    let file = volume
        .create(directory, b"a.txt", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.write(file, None, b"version one\n", 1).unwrap();
    volume.commit().unwrap();
    let mut image = volume.into_disk().image().to_vec();
    as_version_1(&mut image);
    Volume::mount(MemoryDisk::from_image(image)).unwrap().0
}

#[test]
fn symbolic_links_are_made_read_back_and_survive_a_remount() {
    let mut volume = mounted(1024);
    let root = volume.root();
    let file = volume
        .create(root, b"a.txt", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.write(file, None, b"hello", 1).unwrap();
    volume.commit().unwrap();
    let (_, free_before) = volume.space();
    let link = volume.symlink(root, b"l", b"a.txt", 2).unwrap();
    assert_eq!(crate::kind_of(link), Kind::SymbolicLink);
    assert_eq!(volume.link_target(link), Ok(b"a.txt".to_vec()));
    assert_eq!(read_all(&mut volume, link), b"a.txt");
    let attributes = volume.attributes(link).unwrap();
    assert_eq!((attributes.kind, attributes.size), (Kind::SymbolicLink, 5));
    assert_eq!((attributes.permissions, attributes.links), (0o777, 1));
    assert_eq!(volume.lookup(root, b"l"), Ok(Some(link)));
    assert_eq!(volume.parent(link), Ok(root));
    let mut name = [0u8; crate::MAX_NAME];
    let length = volume.name(link, &mut name).unwrap();
    assert_eq!(&name[..length], b"l");
    assert_eq!(
        volume.write(link, None, b"x", 3),
        Err(Error::InvalidArgument)
    );
    assert_eq!(volume.truncate(link, 0, 3), Err(Error::InvalidArgument));
    assert_eq!(volume.link_target(file), Err(Error::InvalidArgument));
    assert_eq!(volume.link_target(root), Err(Error::InvalidArgument));
    assert_eq!(
        volume.create(root, b"x", Kind::SymbolicLink, 0o777, 3),
        Err(Error::InvalidArgument)
    );
    let too_long = vec![b'x'; MAX_LINK_TARGET + 1];
    for bad in [&b""[..], b"a\0b", &too_long[..]] {
        assert_eq!(
            volume.symlink(root, b"bad", bad, 3),
            Err(Error::InvalidArgument)
        );
    }
    assert_eq!(volume.symlink(root, b"l", b"other", 3), Err(Error::Exists));
    assert_eq!(volume.symlink(root, b"", b"other", 3), Err(Error::Exists));
    assert_eq!(
        volume.symlink(file, b"x", b"other", 3),
        Err(Error::NotDirectory)
    );
    let longest_target = vec![b'y'; MAX_LINK_TARGET];
    let longest = volume
        .symlink(root, b"longest", &longest_target, 3)
        .unwrap();
    assert_eq!(volume.link_target(longest), Ok(longest_target.clone()));
    let report = volume.check().unwrap();
    assert_eq!((report.files, report.symbolic_links), (1, 2));
    volume.commit().unwrap();
    let mut volume = remount(volume);
    let root = volume.root();
    let link = volume.lookup(root, b"l").unwrap().unwrap();
    assert_eq!(volume.link_target(link), Ok(b"a.txt".to_vec()));
    let longest = volume.lookup(root, b"longest").unwrap().unwrap();
    assert_eq!(volume.link_target(longest), Ok(longest_target));
    volume.check().unwrap();
    volume.remove(root, b"longest", false, 4).unwrap();
    assert_eq!(volume.link_target(longest), Err(Error::NotFound));
    volume.rename(root, b"a.txt", root, b"l", false, 5).unwrap();
    assert_eq!(volume.link_target(link), Err(Error::NotFound));
    let renamed = volume.lookup(root, b"l").unwrap().unwrap();
    assert_eq!(crate::kind_of(renamed), Kind::Regular);
    assert_eq!(read_all(&mut volume, renamed), b"hello");
    assert_eq!(volume.space().1, free_before);
    let report = volume.check().unwrap();
    assert_eq!((report.files, report.symbolic_links), (1, 0));
}

#[test]
fn hard_links_count_names_and_keep_the_file_until_its_last_name_goes() {
    let mut volume = mounted(2048);
    let root = volume.root();
    let directory = volume
        .create(root, b"d", Kind::Directory, 0o755, 1)
        .unwrap();
    volume
        .create(directory, b"placeholder", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.remove(directory, b"placeholder", false, 1).unwrap();
    volume.commit().unwrap();
    let (_, free_blocks) = volume.space();
    let (_, free_inodes) = volume.inodes();
    let file = volume.create(root, b"a", Kind::Regular, 0o644, 1).unwrap();
    volume.write(file, None, &pattern(4, 9000), 1).unwrap();
    volume.link(file, directory, b"b", 5).unwrap();
    volume.link(file, root, b"c", 6).unwrap();
    assert_eq!(volume.attributes(file).unwrap().links, 3);
    assert_eq!(volume.attributes(directory).unwrap().modified, 5);
    assert_eq!(volume.lookup(directory, b"b"), Ok(Some(file)));
    assert_eq!(volume.lookup(root, b"c"), Ok(Some(file)));
    assert_eq!(
        volume.link(directory, root, b"x", 7),
        Err(Error::IsDirectory)
    );
    assert_eq!(volume.link(file, root, b"a", 7), Err(Error::Exists));
    assert_eq!(volume.link(file, root, b"", 7), Err(Error::Exists));
    assert_eq!(volume.link(file, file, b"x", 7), Err(Error::NotDirectory));
    let link = volume.symlink(root, b"s", b"a", 7).unwrap();
    volume.link(link, directory, b"s2", 7).unwrap();
    assert_eq!(volume.attributes(link).unwrap().links, 2);
    assert_eq!(volume.lookup(directory, b"s2"), Ok(Some(link)));
    let report = volume.check().unwrap();
    assert_eq!((report.files, report.symbolic_links), (1, 1));
    volume.rename(root, b"a", root, b"c", false, 8).unwrap();
    assert_eq!(volume.lookup(root, b"a"), Ok(Some(file)));
    assert_eq!(volume.lookup(root, b"c"), Ok(Some(file)));
    volume.remove(root, b"a", false, 9).unwrap();
    assert_eq!(volume.attributes(file).unwrap().links, 2);
    assert_eq!(volume.parent(file), Ok(root));
    let mut name = [0u8; crate::MAX_NAME];
    let length = volume.name(file, &mut name).unwrap();
    assert_eq!(&name[..length], b"c");
    volume.remove(root, b"c", false, 10).unwrap();
    assert_eq!(volume.parent(file), Ok(directory));
    let length = volume.name(file, &mut name).unwrap();
    assert_eq!(&name[..length], b"b");
    assert_eq!(read_all(&mut volume, file), pattern(4, 9000));
    volume.check().unwrap();
    volume.commit().unwrap();
    let mut volume = remount(volume);
    let root = volume.root();
    let directory = volume.lookup(root, b"d").unwrap().unwrap();
    let file = volume.lookup(directory, b"b").unwrap().unwrap();
    assert_eq!(volume.attributes(file).unwrap().links, 1);
    assert_eq!(read_all(&mut volume, file), pattern(4, 9000));
    volume.check().unwrap();
    let other = volume
        .create(root, b"other", Kind::Regular, 0o644, 11)
        .unwrap();
    volume
        .rename(root, b"other", directory, b"s2", false, 12)
        .unwrap();
    assert_eq!(volume.lookup(directory, b"s2"), Ok(Some(other)));
    let link = volume.lookup(root, b"s").unwrap().unwrap();
    assert_eq!(volume.attributes(link).unwrap().links, 1);
    assert_eq!(volume.parent(link), Ok(root));
    volume.check().unwrap();
    volume.remove(directory, b"b", false, 13).unwrap();
    assert_eq!(volume.read(file, 0, &mut [0; 4]), Err(Error::NotFound));
    volume.remove(root, b"s", false, 13).unwrap();
    volume.remove(directory, b"s2", false, 13).unwrap();
    assert_eq!(volume.space().1, free_blocks);
    assert_eq!(volume.inodes().1, free_inodes);
    volume.check().unwrap();
}

#[test]
fn a_file_with_the_most_names_refuses_another() {
    let mut volume = mounted(1024);
    let root = volume.root();
    let file = volume.create(root, b"a", Kind::Regular, 0o644, 1).unwrap();
    let number = volume.attributes(file).unwrap().inode;
    let mut inode = volume.inode(number).unwrap();
    inode.more_names = MAX_LINKS - 2;
    volume.write_inode(number, &inode).unwrap();
    volume.link(file, root, b"one more", 2).unwrap();
    assert_eq!(volume.attributes(file).unwrap().links, MAX_LINKS);
    assert_eq!(
        volume.link(file, root, b"too many", 3),
        Err(Error::TooManyLinks)
    );
    assert_eq!(volume.lookup(root, b"too many"), Ok(None));
}

#[test]
fn a_file_removed_while_held_stays_usable_until_its_last_release() {
    let mut volume = mounted(2048);
    let root = volume.root();
    volume
        .create(root, b"keep", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.commit().unwrap();
    let (_, free_blocks) = volume.space();
    let (_, free_inodes) = volume.inodes();
    let file = volume
        .create(root, b"held", Kind::Regular, 0o644, 1)
        .unwrap();
    let contents = pattern(5, 20_000);
    volume.write(file, None, &contents, 1).unwrap();
    volume.hold(file).unwrap();
    volume.hold(file).unwrap();
    volume.hold(root).unwrap();
    volume.remove(root, b"held", false, 2).unwrap();
    assert_eq!(volume.lookup(root, b"held"), Ok(None));
    assert_eq!(read_all(&mut volume, file), contents);
    assert_eq!(volume.attributes(file).unwrap().links, 0);
    let mut name = [0u8; crate::MAX_NAME];
    assert_eq!(volume.name(file, &mut name), Err(Error::NotFound));
    assert_eq!(volume.parent(file), Err(Error::NotFound));
    assert_eq!(volume.link(file, root, b"back", 3), Err(Error::NotFound));
    assert_eq!(volume.write(file, None, b"more", 3), Ok((4, 20_004)));
    volume.truncate(file, 10_000, 3).unwrap();
    let report = volume.check().unwrap();
    assert_eq!((report.files, report.orphans), (1, 1));
    volume.commit().unwrap();
    volume.release(file).unwrap();
    assert_eq!(read_all(&mut volume, file), contents[..10_000]);
    volume.release(file).unwrap();
    assert_eq!(volume.read(file, 0, &mut [0; 4]), Err(Error::NotFound));
    volume.release(root).unwrap();
    assert_eq!(volume.release(file), Err(Error::NotFound));
    assert_eq!(volume.space().1, free_blocks);
    assert_eq!(volume.inodes().1, free_inodes);
    let report = volume.check().unwrap();
    assert_eq!((report.files, report.orphans), (1, 0));
    let replaced = volume
        .create(root, b"replaced", Kind::Regular, 0o644, 4)
        .unwrap();
    volume.write(replaced, None, b"old", 4).unwrap();
    volume.hold(replaced).unwrap();
    volume
        .rename(root, b"keep", root, b"replaced", false, 5)
        .unwrap();
    assert_eq!(read_all(&mut volume, replaced), b"old");
    assert_eq!(volume.check().unwrap().orphans, 1);
    volume.release(replaced).unwrap();
    assert_eq!(volume.check().unwrap().orphans, 0);
    let held = volume
        .create(root, b"unremoved", Kind::Regular, 0o644, 6)
        .unwrap();
    volume.hold(held).unwrap();
    volume.release(held).unwrap();
    assert_eq!(volume.lookup(root, b"unremoved"), Ok(Some(held)));
    volume.check().unwrap();
}

#[test]
fn orphans_left_by_a_power_off_are_freed_at_the_next_mount() {
    let mut volume = mounted(1024);
    let root = volume.root();
    volume
        .create(root, b"keep", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.commit().unwrap();
    let (_, free_blocks) = volume.space();
    let (_, free_inodes) = volume.inodes();
    for (index, name) in [&b"one"[..], b"two", b"three"].into_iter().enumerate() {
        let file = volume.create(root, name, Kind::Regular, 0o644, 2).unwrap();
        volume
            .write(file, None, &pattern(index as u64, 6000), 2)
            .unwrap();
        volume.hold(file).unwrap();
        volume.remove(root, name, false, 3).unwrap();
    }
    let link = volume.symlink(root, b"link", b"keep", 3).unwrap();
    volume.hold(link).unwrap();
    volume.remove(root, b"link", false, 3).unwrap();
    assert_eq!(volume.link_target(link), Ok(b"keep".to_vec()));
    volume.commit().unwrap();
    assert_eq!(volume.check().unwrap().orphans, 4);
    let image = volume.into_disk().image().to_vec();
    let (mut volume, mounted) = Volume::mount(MemoryDisk::from_image(image)).unwrap();
    assert_eq!(mounted.freed_orphans, 4);
    assert_eq!(volume.space().1, free_blocks);
    assert_eq!(volume.inodes().1, free_inodes);
    assert_eq!(volume.check().unwrap().orphans, 0);
    let (_, mounted) = Volume::mount(volume.into_disk()).unwrap();
    assert_eq!(mounted.freed_orphans, 0);
}

#[test]
fn a_version_1_volume_mounts_and_its_first_link_upgrades_it() {
    let mut volume = version_1_volume();
    assert_eq!(volume.version(), 1);
    assert_eq!(volume.disk().image()[8], 1);
    let root = volume.root();
    let directory = volume.lookup(root, b"d").unwrap().unwrap();
    let file = volume.lookup(directory, b"a.txt").unwrap().unwrap();
    assert_eq!(read_all(&mut volume, file), b"version one\n");
    volume
        .create(directory, b"plain", Kind::Regular, 0o644, 2)
        .unwrap();
    volume.remove(directory, b"plain", false, 2).unwrap();
    volume.check().unwrap();
    volume.commit().unwrap();
    let volume = remount(volume);
    assert_eq!(volume.version(), 1);
    for change in 0..3 {
        let mut volume = version_1_volume();
        let root = volume.root();
        let directory = volume.lookup(root, b"d").unwrap().unwrap();
        let file = volume.lookup(directory, b"a.txt").unwrap().unwrap();
        match change {
            0 => {
                volume.symlink(root, b"l", b"d/a.txt", 3).unwrap();
            }
            1 => volume.link(file, root, b"b.txt", 3).unwrap(),
            _ => {
                volume.hold(file).unwrap();
                volume.remove(directory, b"a.txt", false, 3).unwrap();
            }
        }
        assert_eq!(volume.version(), VERSION, "change {change}");
        volume.check().unwrap();
        volume.commit().unwrap();
        assert_eq!(volume.disk().image()[8], 2);
        let mut volume = remount(volume);
        assert_eq!(volume.version(), VERSION);
        volume.check().unwrap();
    }
}

#[test]
fn superblock_versions_are_read_strictly_except_for_a_torn_upgrade() {
    let volume = version_1_volume();
    let mut image = volume.into_disk().image().to_vec();
    let version = |image: &[u8]| Geometry::decode(image).map(|geometry| geometry.version);
    let mut torn = image.clone();
    torn[8] = 2;
    assert_eq!(version(&torn[..]), Some(1));
    let mut upgraded = Geometry::decode(&image).unwrap();
    upgraded.version = 2;
    let encoded = upgraded.encode();
    let mut torn = image.clone();
    torn[BLOCK_SIZE - 4..BLOCK_SIZE].copy_from_slice(&encoded[BLOCK_SIZE - 4..]);
    assert_eq!(version(&torn[..]), Some(2));
    let mut future = image.clone();
    let mut geometry = Geometry::decode(&image).unwrap();
    geometry.version = 3;
    future[..BLOCK_SIZE].copy_from_slice(&geometry.encode()[..]);
    assert!(Geometry::decode(&future).is_none());
    assert!(matches!(
        Volume::mount(MemoryDisk::from_image(future)),
        Err((MountError::NotEggFs, _))
    ));
    image[8] = 3;
    assert!(Geometry::decode(&image).is_none());
}

#[test]
fn a_version_1_volume_holding_a_link_fails_its_check() {
    let mut volume = mounted(1024);
    let root = volume.root();
    volume.symlink(root, b"l", b"target", 1).unwrap();
    volume.commit().unwrap();
    let mut image = volume.into_disk().image().to_vec();
    as_version_1(&mut image);
    let mut volume = Volume::mount(MemoryDisk::from_image(image)).unwrap().0;
    let problem = volume.check().unwrap_err();
    assert!(problem.what.contains("version 1"), "{problem}");
}

#[test]
fn permissions_change_on_files_directories_and_orphans_but_not_links() {
    let mut volume = mounted(1024);
    let root = volume.root();
    let directory = volume
        .create(root, b"d", Kind::Directory, 0o755, 1)
        .unwrap();
    let file = volume
        .create(directory, b"a", Kind::Regular, 0o644, 2)
        .unwrap();
    let link = volume.symlink(root, b"l", b"d/a", 3).unwrap();
    volume.set_permissions(file, 0o10_4750).unwrap();
    volume.set_permissions(directory, 0o700).unwrap();
    assert_eq!(
        volume.set_permissions(link, 0o600),
        Err(Error::InvalidArgument)
    );
    let attributes = volume.attributes(file).unwrap();
    assert_eq!((attributes.permissions, attributes.modified), (0o4750, 2));
    assert_eq!(volume.attributes(link).unwrap().permissions, 0o777);
    volume.check().unwrap();
    volume.commit().unwrap();
    let mut volume = remount(volume);
    let root = volume.root();
    let directory = volume.lookup(root, b"d").unwrap().unwrap();
    let file = volume.lookup(directory, b"a").unwrap().unwrap();
    assert_eq!(volume.attributes(directory).unwrap().permissions, 0o700);
    assert_eq!(volume.attributes(file).unwrap().permissions, 0o4750);
    volume.hold(file).unwrap();
    volume.remove(directory, b"a", false, 4).unwrap();
    volume.set_permissions(file, 0o600).unwrap();
    assert_eq!(volume.attributes(file).unwrap().permissions, 0o600);
    volume.release(file).unwrap();
    assert_eq!(volume.set_permissions(file, 0o644), Err(Error::NotFound));
    volume.check().unwrap();
}

fn scenario_base() -> Vec<u8> {
    let mut volume = mounted(512);
    let root = volume.root();
    volume
        .create(root, b"d", Kind::Directory, 0o755, 1)
        .unwrap();
    let file = volume.create(root, b"a", Kind::Regular, 0o644, 1).unwrap();
    volume.write(file, None, &pattern(3, 9000), 1).unwrap();
    let big = volume
        .create(root, b"big", Kind::Regular, 0o644, 1)
        .unwrap();
    volume.write(big, None, &pattern(8, 30_000), 1).unwrap();
    volume.commit().unwrap();
    let mut image = volume.into_disk().image().to_vec();
    as_version_1(&mut image);
    image
}

fn commit_step(volume: &mut Volume<MemoryDisk>, states: &mut Vec<Tree>) -> Result<(), Error> {
    let before = volume.commits();
    volume.commit()?;
    if volume.commits() > before {
        states.push(tree(volume));
    }
    Ok(())
}

fn scenario_steps(volume: &mut Volume<MemoryDisk>, states: &mut Vec<Tree>) -> Result<(), Error> {
    let root = volume.root();
    let directory = volume.lookup(root, b"d")?.ok_or(Error::NotFound)?;
    let file = volume.lookup(root, b"a")?.ok_or(Error::NotFound)?;
    let big = volume.lookup(root, b"big")?.ok_or(Error::NotFound)?;
    volume.symlink(root, b"l", b"a", 2)?;
    commit_step(volume, states)?;
    volume.link(file, directory, b"b", 3)?;
    commit_step(volume, states)?;
    volume.hold(file)?;
    volume.remove(root, b"a", false, 4)?;
    volume.remove(directory, b"b", false, 4)?;
    commit_step(volume, states)?;
    volume.write(file, None, &pattern(9, 5000), 5)?;
    commit_step(volume, states)?;
    volume.release(file)?;
    commit_step(volume, states)?;
    volume.link(big, directory, b"big2", 6)?;
    volume.rename(root, b"l", directory, b"big2", false, 6)?;
    commit_step(volume, states)?;
    volume.hold(big)?;
    volume.remove(root, b"big", false, 7)?;
    commit_step(volume, states)
}

fn run_scenario(disk: MemoryDisk) -> (Volume<MemoryDisk>, Vec<Tree>) {
    let mut volume = Volume::mount(disk).unwrap().0;
    let mut states = vec![tree(&mut volume)];
    let _ = scenario_steps(&mut volume, &mut states);
    (volume, states)
}

#[test]
fn a_power_cut_while_linking_or_removing_a_held_file_recovers_a_committed_state() {
    let base = scenario_base();
    let (reference, states) = run_scenario(MemoryDisk::from_image(base.clone()));
    assert_eq!(states.len(), 8);
    assert_eq!(reference.commits() as usize, states.len() - 1);
    assert_eq!(reference.version(), VERSION);
    let writes = reference.disk().writes();
    let (mut after, mounted) = Volume::mount(reference.into_disk()).unwrap();
    assert_eq!(mounted.freed_orphans, 1);
    after.check().unwrap();
    let mut seen_freed = false;
    let mut seen_versions = [false; 2];
    for cut in 0..=writes {
        let mut disk = MemoryDisk::from_image(base.clone());
        disk.cut_power_after(cut);
        let (run, _) = run_scenario(disk);
        let completed = run.commits() as usize;
        let disk = run.into_disk();
        let mut random = Random(cut * 104_729 + 3);
        let variants = [
            disk.after_power_cut(|_, _| false),
            disk.after_power_cut(|_, _| true),
            disk.after_power_cut(|_, _| random.below(2) == 0),
        ];
        for (variant, crashed) in variants.into_iter().enumerate() {
            let (mut volume, mounted) = Volume::mount(crashed)
                .unwrap_or_else(|(error, _)| panic!("cut {cut}, variant {variant}: {error:?}"));
            seen_freed |= mounted.freed_orphans > 0;
            seen_versions[volume.version() as usize - 1] = true;
            if let Err(problem) = volume.check() {
                panic!("cut {cut}, variant {variant}: {problem}");
            }
            let found = tree(&mut volume);
            let candidates = &states[completed..(completed + 2).min(states.len())];
            assert!(
                candidates.contains(&found),
                "cut {cut}, variant {variant}: the state is no committed one"
            );
        }
    }
    assert!(seen_freed);
    assert_eq!(seen_versions, [true, true]);
}
