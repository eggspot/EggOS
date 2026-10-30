use crate::checksum::Crc64;
use crate::index::DirectoryKey;
use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Descriptor, Geometry, INODE_SIZE, Inode, InodeKind, encode_commit,
    records_end, write_record,
};
use crate::memory::MemoryDisk;
use crate::{
    CheckError, CheckMode, DiskCheck, Error, FormatError, FormatOptions, Kind, MAX_LABEL, Outcome,
    Problem, Volume, check_disk, format, format_with, generation_of, inode_of,
};
use std::collections::BTreeMap;
use std::ops::Range;

const NOW: u64 = 1_700_000_000;

struct Sample {
    image: Vec<u8>,
    geometry: Geometry,
    inodes: BTreeMap<&'static str, u32>,
    big: Vec<u8>,
}

impl Sample {
    fn inode(&self, name: &str) -> u32 {
        self.inodes[name]
    }

    fn read_inode(&self, number: u32) -> Inode {
        Inode::decode(number, &self.image[inode_range(&self.geometry, number)]).unwrap()
    }

    fn edit_inode(&mut self, number: u32, change: impl FnOnce(&mut Inode)) {
        let range = inode_range(&self.geometry, number);
        let mut inode = Inode::decode(number, &self.image[range.clone()]).unwrap();
        change(&mut inode);
        inode.encode(number, &mut self.image[range]);
    }

    fn block_mut(&mut self, block: u32) -> &mut [u8] {
        &mut self.image[block_range(block)]
    }
}

fn inode_range(geometry: &Geometry, number: u32) -> Range<usize> {
    let (block, at) = geometry.inode_location(number);
    let start = block as usize * BLOCK_SIZE + at;
    start..start + INODE_SIZE
}

fn block_range(block: u32) -> Range<usize> {
    let start = block as usize * BLOCK_SIZE;
    start..start + BLOCK_SIZE
}

fn pattern(length: usize, seed: u64) -> Vec<u8> {
    (0..length as u64)
        .map(|index| (index.wrapping_mul(31) ^ seed) as u8)
        .collect()
}

fn sample() -> Sample {
    let mut disk = MemoryDisk::new(1024);
    format(&mut disk, NOW).unwrap();
    let mut volume = Volume::mount(disk).unwrap().0;
    let root = volume.root();
    let docs = volume
        .create(root, b"docs", Kind::Directory, 0o755, NOW)
        .unwrap();
    let deep = volume
        .create(docs, b"deep", Kind::Directory, 0o755, NOW)
        .unwrap();
    let big = volume
        .create(docs, b"big", Kind::Regular, 0o644, NOW)
        .unwrap();
    let contents = pattern(5 * BLOCK_SIZE + 17, 7);
    volume.write(big, None, &contents, NOW).unwrap();
    let small = volume
        .create(deep, b"small", Kind::Regular, 0o644, NOW)
        .unwrap();
    volume.write(small, None, b"small file", NOW).unwrap();
    volume.link(small, root, b"small-again", NOW).unwrap();
    let link = volume.symlink(root, b"link", b"docs/big", NOW).unwrap();
    volume.commit().unwrap();
    volume.check().unwrap();
    let named = [
        ("docs", docs),
        ("deep", deep),
        ("big", big),
        ("small", small),
        ("link", link),
    ];
    let inodes = named
        .into_iter()
        .map(|(name, node)| (name, inode_of(node)))
        .collect();
    let image = volume.into_disk().image().to_vec();
    let geometry = Geometry::decode(&image[..BLOCK_SIZE]).unwrap();
    Sample {
        image,
        geometry,
        inodes,
        big: contents,
    }
}

fn has(check: &DiskCheck, problem: Problem) -> bool {
    check
        .findings
        .iter()
        .any(|finding| finding.problem == problem)
}

fn problems(check: &DiskCheck) -> Vec<Problem> {
    check
        .findings
        .iter()
        .map(|finding| finding.problem)
        .collect()
}

fn repaired(image: Vec<u8>, expected: &[Problem]) -> Volume<MemoryDisk> {
    let mut disk = MemoryDisk::from_image(image.clone());
    let writes = disk.writes();
    let found = check_disk(&mut disk, CheckMode::ReadOnly, NOW).unwrap();
    for &problem in expected {
        assert!(has(&found, problem), "{problem:?} missing from {found:?}");
    }
    assert!(
        found
            .findings
            .iter()
            .all(|finding| finding.outcome == Outcome::Repairable),
        "{found:?}"
    );
    assert!(found.summary.is_some(), "{found:?}");
    assert_eq!(disk.writes(), writes);
    assert!(disk.image() == &image[..]);
    let fixed = check_disk(&mut disk, CheckMode::Repair, NOW).unwrap();
    assert_eq!(problems(&fixed), problems(&found));
    assert!(
        fixed
            .findings
            .iter()
            .all(|finding| finding.outcome == Outcome::Repaired),
        "{fixed:?}"
    );
    assert_eq!(fixed.summary, found.summary);
    assert_eq!(fixed.free_blocks, found.free_blocks);
    let writes = disk.writes();
    let again = check_disk(&mut disk, CheckMode::Repair, NOW).unwrap();
    assert!(again.is_clean(), "{again:?}");
    assert_eq!(disk.writes(), writes);
    let mut volume = Volume::mount(disk).unwrap().0;
    volume.check().unwrap();
    volume
}

fn read_all(volume: &mut Volume<MemoryDisk>, node: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = vec![0u8; BLOCK_SIZE];
    loop {
        let read = volume.read(node, out.len() as u64, &mut buffer).unwrap();
        if read == 0 {
            return out;
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

fn path(volume: &mut Volume<MemoryDisk>, names: &[&[u8]]) -> Option<u64> {
    let mut node = volume.root();
    for name in names {
        node = volume.lookup(node, name).unwrap()?;
    }
    Some(node)
}

fn lost_name(number: u32) -> Vec<u8> {
    format!("#{number}").into_bytes()
}

#[test]
fn a_clean_volume_checks_clean_and_a_repair_writes_nothing() {
    let sample = sample();
    let mut disk = MemoryDisk::from_image(sample.image.clone());
    let found = check_disk(&mut disk, CheckMode::ReadOnly, NOW).unwrap();
    assert!(found.is_clean(), "{found:?}");
    let summary = found.summary.unwrap();
    assert_eq!(
        (summary.directories, summary.files, summary.symbolic_links),
        (3, 2, 1)
    );
    assert_eq!(found.blocks, 1024);
    assert_eq!(found.blocks - found.free_blocks, summary.used_blocks);
    let fixed = check_disk(&mut disk, CheckMode::Repair, NOW).unwrap();
    assert!(fixed.is_clean() && fixed.is_repairable(), "{fixed:?}");
    assert_eq!(disk.writes(), 0);
}

#[test]
fn a_disk_without_eggfs_is_refused_and_a_damaged_superblock_cannot_be_repaired() {
    let mut blank = MemoryDisk::new(300);
    assert_eq!(
        check_disk(&mut blank, CheckMode::Repair, NOW),
        Err(CheckError::NotEggFs)
    );
    let mut sample = sample();
    sample.image[20] ^= 0x10;
    let mut disk = MemoryDisk::from_image(sample.image.clone());
    let found = check_disk(&mut disk, CheckMode::Repair, NOW).unwrap();
    assert_eq!(problems(&found), [Problem::DamagedSuperblock]);
    assert_eq!(found.findings[0].outcome, Outcome::NotRepairable);
    assert!(!found.is_repairable() && found.summary.is_none());
    assert!(disk.image() == &sample.image[..]);
}

#[test]
fn an_unreadable_inode_loses_its_names_and_its_blocks_are_freed() {
    let mut sample = sample();
    let big = sample.inode("big");
    let range = inode_range(&sample.geometry, big);
    sample.image[range.start + 50] ^= 0x01;
    let mut volume = repaired(
        sample.image,
        &[
            Problem::UnreadableInode,
            Problem::DirectoryCounts,
            Problem::BlockBitmap,
            Problem::InodeBitmap,
        ],
    );
    assert_eq!(path(&mut volume, &[b"docs", b"big"]), None);
    assert!(path(&mut volume, &[b"docs", b"deep", b"small"]).is_some());
}

#[test]
fn a_zeroed_directory_block_sends_what_it_named_to_lost_and_found() {
    let mut sample = sample();
    let docs = sample.read_inode(sample.inode("docs"));
    sample.block_mut(docs.direct[0]).fill(0);
    let (big, deep) = (sample.inode("big"), sample.inode("deep"));
    let contents = sample.big.clone();
    let mut volume = repaired(
        sample.image,
        &[Problem::Unreachable, Problem::DirectoryCounts],
    );
    let lost = path(&mut volume, &[b"lost+found"]).unwrap();
    let found_big = volume.lookup(lost, &lost_name(big)).unwrap().unwrap();
    assert_eq!(read_all(&mut volume, found_big), contents);
    let found_deep = volume.lookup(lost, &lost_name(deep)).unwrap().unwrap();
    assert!(volume.lookup(found_deep, b"small").unwrap().is_some());
    assert_eq!(volume.parent(found_deep), Ok(lost));
    assert_eq!(path(&mut volume, &[b"docs", b"big"]), None);
}

#[test]
fn link_counts_parents_and_entries_naming_free_inodes_are_corrected() {
    let mut sample = sample();
    let (small, big, link) = (
        sample.inode("small"),
        sample.inode("big"),
        sample.inode("link"),
    );
    sample.edit_inode(small, |inode| inode.more_names = 5);
    sample.edit_inode(link, |inode| inode.parent = 77);
    sample.edit_inode(big, |inode| inode.kind = InodeKind::Free);
    let mut volume = repaired(
        sample.image,
        &[
            Problem::LinkCount,
            Problem::WrongParent,
            Problem::EntryToFreeInode,
            Problem::BlockBitmap,
            Problem::InodeBitmap,
        ],
    );
    let small_node = path(&mut volume, &[b"small-again"]).unwrap();
    assert_eq!(volume.attributes(small_node).unwrap().links, 2);
    assert_eq!(path(&mut volume, &[b"docs", b"big"]), None);
}

#[test]
fn repeated_and_bad_names_are_removed() {
    let mut sample = sample();
    let root = sample.read_inode(1);
    let big = sample.inode("big");
    let block = sample.block_mut(root.direct[0]);
    let end = records_end(block).unwrap();
    write_record(block, end, big, InodeKind::Regular, b"docs");
    write_record(block, end + 10, big, InodeKind::Regular, b"a/b");
    let mut volume = repaired(sample.image, &[Problem::DuplicateName, Problem::BadName]);
    let docs = path(&mut volume, &[b"docs"]).unwrap();
    assert_eq!(crate::kind_of(docs), Kind::Directory);
    let big = path(&mut volume, &[b"docs", b"big"]).unwrap();
    assert_eq!(volume.attributes(big).unwrap().links, 1);
}

#[test]
fn files_removed_while_open_and_left_behind_are_freed() {
    let sample = sample();
    let mut volume = Volume::mount(MemoryDisk::from_image(sample.image))
        .unwrap()
        .0;
    let free_before = volume.space().1;
    let root = volume.root();
    let held = volume
        .create(root, b"held", Kind::Regular, 0o644, NOW)
        .unwrap();
    volume
        .write(held, None, &pattern(3 * BLOCK_SIZE, 3), NOW)
        .unwrap();
    volume.hold(held).unwrap();
    volume.remove(root, b"held", false, NOW).unwrap();
    volume.commit().unwrap();
    let image = volume.into_disk().image().to_vec();
    let volume = repaired(image, &[Problem::OrphanLeft]);
    assert_eq!(volume.space().1, free_before);
}

#[test]
fn a_block_used_by_two_files_is_copied_for_one_of_them() {
    let mut sample = sample();
    let shared = sample.read_inode(sample.inode("big")).direct[0];
    sample.edit_inode(sample.inode("small"), |inode| inode.direct[0] = shared);
    let contents = sample.big.clone();
    let mut volume = repaired(
        sample.image,
        &[Problem::CrossLinkedBlock, Problem::BlockBitmap],
    );
    let big = path(&mut volume, &[b"docs", b"big"]).unwrap();
    assert_eq!(read_all(&mut volume, big), contents);
    let small = path(&mut volume, &[b"small-again"]).unwrap();
    assert_eq!(read_all(&mut volume, small), contents[..10]);
}

#[test]
fn a_file_cut_short_releases_the_blocks_past_its_end() {
    let mut sample = sample();
    sample.edit_inode(sample.inode("big"), |inode| inode.size = 100);
    let contents = sample.big.clone();
    let mut volume = repaired(sample.image, &[Problem::BlockPastEnd, Problem::BlockBitmap]);
    let big = path(&mut volume, &[b"docs", b"big"]).unwrap();
    assert_eq!(read_all(&mut volume, big), contents[..100]);
}

#[test]
fn a_symbolic_link_with_an_empty_target_is_removed() {
    let mut sample = sample();
    sample.edit_inode(sample.inode("link"), |inode| inode.size = 0);
    let mut volume = repaired(
        sample.image,
        &[
            Problem::BadSymbolicLink,
            Problem::BlockBitmap,
            Problem::InodeBitmap,
        ],
    );
    assert_eq!(path(&mut volume, &[b"link"]), None);
}

#[test]
fn a_damaged_root_is_rebuilt_and_its_contents_go_to_lost_and_found() {
    let mut sample = sample();
    let range = inode_range(&sample.geometry, 1);
    sample.image[range.start + 20] ^= 0x40;
    let (docs, link) = (sample.inode("docs"), sample.inode("link"));
    let mut volume = repaired(sample.image, &[Problem::DamagedRoot, Problem::Unreachable]);
    let lost = path(&mut volume, &[b"lost+found"]).unwrap();
    let found_docs = volume.lookup(lost, &lost_name(docs)).unwrap().unwrap();
    assert!(volume.lookup(found_docs, b"big").unwrap().is_some());
    let found_link = volume.lookup(lost, &lost_name(link)).unwrap().unwrap();
    assert_eq!(volume.link_target(found_link).unwrap(), b"docs/big");
}

#[test]
fn a_used_block_marked_free_in_the_bitmap_is_marked_used_again() {
    let mut sample = sample();
    let block = sample.read_inode(sample.inode("big")).direct[0];
    let bitmap = sample.geometry.block_bitmap + block / BITS_PER_BLOCK;
    let bit = block % BITS_PER_BLOCK;
    sample.block_mut(bitmap)[(bit / 8) as usize] &= !(1 << (bit % 8));
    let mut disk = MemoryDisk::from_image(sample.image.clone());
    let found = check_disk(&mut disk, CheckMode::ReadOnly, NOW).unwrap();
    assert_eq!(problems(&found), [Problem::BlockBitmap]);
    assert_eq!(found.findings[0].block, block);
    repaired(sample.image, &[Problem::BlockBitmap]);
}

#[test]
fn a_journal_transaction_that_cannot_be_replayed_is_dropped() {
    let mut sample = sample();
    let journal = sample.geometry.journal;
    let descriptor = Descriptor {
        sequence: 7,
        targets: vec![journal + 1],
    };
    let header = descriptor.encode();
    let image = vec![0x5au8; BLOCK_SIZE];
    let mut crc = Crc64::new();
    crc.update(&header[..]);
    crc.update(&image);
    let commit = encode_commit(7, 1, crc.finish());
    sample.block_mut(journal).copy_from_slice(&header[..]);
    sample.block_mut(journal + 1).copy_from_slice(&image);
    sample.block_mut(journal + 2).copy_from_slice(&commit[..]);
    assert!(Volume::mount(MemoryDisk::from_image(sample.image.clone())).is_err());
    let mut volume = repaired(sample.image, &[Problem::DamagedJournal]);
    assert!(path(&mut volume, &[b"docs", b"big"]).is_some());
}

#[test]
fn a_missing_directory_block_is_filled_and_the_names_it_held_are_recounted() {
    let mut disk = MemoryDisk::new(1024);
    format(&mut disk, NOW).unwrap();
    let mut volume = Volume::mount(disk).unwrap().0;
    let root = volume.root();
    let many = volume
        .create(root, b"many", Kind::Directory, 0o755, NOW)
        .unwrap();
    let file = volume
        .create(root, b"file", Kind::Regular, 0o644, NOW)
        .unwrap();
    for index in 0..400 {
        let name = format!("name-{index:04}");
        volume.link(file, many, name.as_bytes(), NOW).unwrap();
    }
    volume.commit().unwrap();
    let many_inode = inode_of(many);
    let mut image = volume.into_disk().image().to_vec();
    let geometry = Geometry::decode(&image[..BLOCK_SIZE]).unwrap();
    let range = inode_range(&geometry, many_inode);
    let mut inode = Inode::decode(many_inode, &image[range.clone()]).unwrap();
    assert!(inode.size >= 2 * BLOCK_SIZE as u64);
    inode.direct[0] = 0;
    inode.encode(many_inode, &mut image[range]);
    let mut volume = repaired(
        image,
        &[
            Problem::DirectoryHole,
            Problem::DirectoryCounts,
            Problem::LinkCount,
            Problem::BlockBitmap,
        ],
    );
    let many = path(&mut volume, &[b"many"]).unwrap();
    assert!(volume.lookup(many, b"name-0399").unwrap().is_some());
    assert!(volume.lookup(many, b"name-0000").unwrap().is_none());
}

#[test]
fn a_big_directory_finds_every_name_through_its_index() {
    let mut disk = MemoryDisk::new(2048);
    format(&mut disk, NOW).unwrap();
    let mut volume = Volume::mount(disk).unwrap().0;
    let root = volume.root();
    let directory = volume
        .create(root, b"big", Kind::Directory, 0o755, NOW)
        .unwrap();
    let file = volume
        .create(root, b"file", Kind::Regular, 0o644, NOW)
        .unwrap();
    let name = |index: u32| format!("n{index}").into_bytes();
    for index in 0..3000 {
        volume.link(file, directory, &name(index), NOW).unwrap();
    }
    for index in 0..3000 {
        assert_eq!(volume.lookup(directory, &name(index)), Ok(Some(file)));
    }
    let key = DirectoryKey {
        inode: inode_of(directory),
        generation: generation_of(directory),
    };
    assert!(volume.indexes.find(key).is_some());
    assert_eq!(volume.lookup(directory, b"missing"), Ok(None));
    volume.remove(directory, &name(10), false, NOW).unwrap();
    assert_eq!(volume.lookup(directory, &name(10)), Ok(None));
    volume
        .rename(directory, &name(11), directory, b"renamed", false, NOW)
        .unwrap();
    assert_eq!(volume.lookup(directory, &name(11)), Ok(None));
    assert_eq!(volume.lookup(directory, b"renamed"), Ok(Some(file)));
    volume.link(file, directory, b"added", NOW).unwrap();
    assert_eq!(volume.lookup(directory, b"added"), Ok(Some(file)));
    assert_eq!(
        volume.link(file, directory, &name(12), NOW),
        Err(Error::Exists)
    );
    volume.commit().unwrap();
    volume.check().unwrap();
    let mut volume = Volume::mount(MemoryDisk::from_image(volume.into_disk().image().to_vec()))
        .unwrap()
        .0;
    for index in (12..3000).step_by(97) {
        assert_eq!(volume.lookup(directory, &name(index)), Ok(Some(file)));
    }
    assert_eq!(volume.lookup(directory, b"renamed"), Ok(Some(file)));
}

#[test]
fn format_with_writes_a_label_and_uses_only_the_blocks_asked_for() {
    let mut disk = MemoryDisk::new(1024);
    let options = FormatOptions {
        blocks: Some(512),
        label: "Photos",
    };
    format_with(&mut disk, NOW, &options).unwrap();
    let volume = Volume::mount(disk).unwrap().0;
    assert_eq!(volume.label(), b"Photos");
    assert_eq!(volume.space().0, 512);
    let mut disk = volume.into_disk();
    let found = check_disk(&mut disk, CheckMode::ReadOnly, NOW).unwrap();
    assert!(found.is_clean() && found.blocks == 512, "{found:?}");
    let too_long = "x".repeat(MAX_LABEL + 1);
    let refused = [
        (Some(2048), "", FormatError::LargerThanDisk),
        (None, too_long.as_str(), FormatError::BadLabel),
        (None, "a\0b", FormatError::BadLabel),
        (Some(100), "", FormatError::TooSmall),
    ];
    for (blocks, label, error) in refused {
        let options = FormatOptions { blocks, label };
        assert_eq!(format_with(&mut disk, NOW, &options), Err(error));
    }
    let mut plain = MemoryDisk::new(300);
    format(&mut plain, NOW).unwrap();
    assert_eq!(Volume::mount(plain).unwrap().0.label(), b"");
}
