use crate::check::{Bits, CheckReport, valid_name};
use crate::journal::{count_clear, replay};
use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Block, DESCRIPTOR_MAGIC, Descriptor, FIRST_FREE_INODE, Geometry,
    Inode, InodeKind, MAGIC, MAX_DIRECTORY_BLOCKS, MAX_LINK_TARGET, POINTERS_PER_BLOCK, ROOT_INODE,
    Record, VERSION, put_u32, put_u64, record_at, u32_at, write_record, zeroed_block,
};
use crate::volume::{MAX_FILE_BYTES, Volume, tree_first};
use crate::{Disk, DiskError, Error, Kind, MountError, inode_of, kind_of};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const NO_PARENT: u32 = 0;
const NAME_ATTEMPTS: u32 = 10;
const LOST_AND_FOUND: &str = "lost+found";

/// What [`check_disk`] does about the problems it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMode {
    /// Only report: nothing is written to the disk.
    ReadOnly,
    /// Repair what can be repaired safely, through the journal.
    Repair,
}

/// A problem [`check_disk`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The superblock fails its checksum or describes an impossible layout.
    DamagedSuperblock,
    /// The superblock describes more blocks than the disk has.
    SuperblockLargerThanDisk,
    /// The journal holds a committed transaction that cannot be replayed; it is dropped.
    DamagedJournal,
    /// The root directory's inode is damaged; an empty root replaces it.
    DamagedRoot,
    /// An inode fails its checksum; the entries naming it are removed.
    UnreadableInode,
    /// A directory entry names a free inode; the entry is removed.
    EntryToFreeInode,
    /// A directory entry names a reserved inode or one past the inode table; it is removed.
    EntryToInvalidInode,
    /// A directory entry's kind is not its inode's; the entry is corrected.
    EntryKindWrong,
    /// A directory entry's name is empty, `.`, `..` or holds `/` or NUL; it is removed.
    BadName,
    /// A directory holds a name twice; the later entry is removed.
    DuplicateName,
    /// A directory has a second name; the extra entry is removed.
    DirectoryNamedTwice,
    /// A directory block holds a malformed record or bytes after its last record.
    MalformedDirectoryBlock,
    /// A directory has a block missing inside it; an empty block fills it.
    DirectoryHole,
    /// A directory's size is not the whole blocks it maps.
    DirectorySize,
    /// A directory counts its entries or subdirectories wrongly.
    DirectoryCounts,
    /// An inode names a parent that does not list it.
    WrongParent,
    /// A file's inode counts its names wrongly.
    LinkCount,
    /// An inode that has a name is marked removed while open.
    NamedOrphan,
    /// A file removed while open was left behind; it is freed.
    OrphanLeft,
    /// An inode in use has no name; it is moved to `lost+found`.
    Unreachable,
    /// A block pointer leads outside the data blocks; it is cleared.
    PointerOutsideData,
    /// A block is used twice; the later file gets a copy, a later directory or pointer block
    /// loses it.
    CrossLinkedBlock,
    /// A file maps a block past its end; the block is released.
    BlockPastEnd,
    /// A file is larger than the format allows; its size is cut.
    FileTooLarge,
    /// A symbolic link's target is empty, too long, missing or holds NUL; the link is removed.
    BadSymbolicLink,
    /// The block bitmap does not match the blocks in use; it is rewritten.
    BlockBitmap,
    /// The inode bitmap does not match the inodes in use; it is rewritten.
    InodeBitmap,
    /// A version 1 volume holds symbolic links or hard links; it is upgraded to version 2.
    OldVersion,
    /// An unreachable inode could not be given a name in `lost+found`.
    LostAndFoundFull,
    /// The repair stopped on an error; nothing more could be done.
    RepairFailed,
    /// After the repair the volume is still inconsistent.
    RepairIncomplete,
}

impl Problem {
    /// A short stable name, for scripts and agents.
    pub const fn code(self) -> &'static str {
        match self {
            Problem::DamagedSuperblock => "damaged-superblock",
            Problem::SuperblockLargerThanDisk => "superblock-larger-than-disk",
            Problem::DamagedJournal => "damaged-journal",
            Problem::DamagedRoot => "damaged-root",
            Problem::UnreadableInode => "unreadable-inode",
            Problem::EntryToFreeInode => "entry-to-free-inode",
            Problem::EntryToInvalidInode => "entry-to-invalid-inode",
            Problem::EntryKindWrong => "entry-kind-wrong",
            Problem::BadName => "bad-name",
            Problem::DuplicateName => "duplicate-name",
            Problem::DirectoryNamedTwice => "directory-named-twice",
            Problem::MalformedDirectoryBlock => "malformed-directory-block",
            Problem::DirectoryHole => "directory-hole",
            Problem::DirectorySize => "directory-size",
            Problem::DirectoryCounts => "directory-counts",
            Problem::WrongParent => "wrong-parent",
            Problem::LinkCount => "link-count",
            Problem::NamedOrphan => "named-orphan",
            Problem::OrphanLeft => "orphan-left",
            Problem::Unreachable => "unreachable",
            Problem::PointerOutsideData => "pointer-outside-data",
            Problem::CrossLinkedBlock => "cross-linked-block",
            Problem::BlockPastEnd => "block-past-end",
            Problem::FileTooLarge => "file-too-large",
            Problem::BadSymbolicLink => "bad-symbolic-link",
            Problem::BlockBitmap => "block-bitmap",
            Problem::InodeBitmap => "inode-bitmap",
            Problem::OldVersion => "old-version",
            Problem::LostAndFoundFull => "lost-and-found-full",
            Problem::RepairFailed => "repair-failed",
            Problem::RepairIncomplete => "repair-incomplete",
        }
    }

    /// What is wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            Problem::DamagedSuperblock => "the superblock is damaged",
            Problem::SuperblockLargerThanDisk => "the file system is larger than the disk",
            Problem::DamagedJournal => "the journal holds a transaction that cannot be replayed",
            Problem::DamagedRoot => "the root directory is damaged",
            Problem::UnreadableInode => "an inode is damaged",
            Problem::EntryToFreeInode => "an entry names a free inode",
            Problem::EntryToInvalidInode => "an entry names an inode that cannot be used",
            Problem::EntryKindWrong => "an entry's kind is not its inode's",
            Problem::BadName => "an entry has a name that is not allowed",
            Problem::DuplicateName => "a directory holds a name twice",
            Problem::DirectoryNamedTwice => "a directory has more than one name",
            Problem::MalformedDirectoryBlock => "a directory block is malformed",
            Problem::DirectoryHole => "a directory has a missing block",
            Problem::DirectorySize => "a directory's size is wrong",
            Problem::DirectoryCounts => "a directory counts its entries wrongly",
            Problem::WrongParent => "an inode names the wrong parent",
            Problem::LinkCount => "a file counts its names wrongly",
            Problem::NamedOrphan => "a named file is marked removed",
            Problem::OrphanLeft => "a file removed while open was left behind",
            Problem::Unreachable => "an inode in use has no name",
            Problem::PointerOutsideData => "a block pointer leads outside the data blocks",
            Problem::CrossLinkedBlock => "a block is used twice",
            Problem::BlockPastEnd => "a file maps a block past its end",
            Problem::FileTooLarge => "a file is larger than the format allows",
            Problem::BadSymbolicLink => "a symbolic link's target is invalid",
            Problem::BlockBitmap => "the block bitmap does not match what is used",
            Problem::InodeBitmap => "the inode bitmap does not match what is used",
            Problem::OldVersion => "a version 1 volume holds version 2 features",
            Problem::LostAndFoundFull => "an unreachable inode could not be put in lost+found",
            Problem::RepairFailed => "the repair stopped on an error",
            Problem::RepairIncomplete => "the volume is still inconsistent after the repair",
        }
    }
}

/// What became of a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A repair would fix it; the check was read-only.
    Repairable,
    /// The repair fixed it.
    Repaired,
    /// It cannot be fixed safely.
    NotRepairable,
}

/// One problem [`check_disk`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    /// What is wrong.
    pub problem: Problem,
    /// The inode it was found at, 0 for none.
    pub inode: u32,
    /// The block it was found at, or the first mismatching number of a bitmap; 0 for none.
    pub block: u32,
    /// What became of it.
    pub outcome: Outcome,
}

/// What [`check_disk`] found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCheck {
    /// The mode it ran in.
    pub mode: CheckMode,
    /// Every problem, in the order found.
    pub findings: Vec<Finding>,
    /// The volume as the repair leaves it (in read-only mode, as it would leave it), when it
    /// is then consistent.
    pub summary: Option<CheckReport>,
    /// Blocks of [`crate::BLOCK_SIZE`] bytes in the volume.
    pub blocks: u64,
    /// Free blocks after the repair (in read-only mode, after the repair it would make).
    pub free_blocks: u64,
}

impl DiskCheck {
    /// Whether nothing was wrong.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Whether every problem was, or in read-only mode would be, repaired.
    pub fn is_repairable(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.outcome != Outcome::NotRepairable)
    }
}

/// Why [`check_disk`] could not check a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckError {
    /// The disk holds no EggFS file system.
    NotEggFs,
    /// The disk failed a read or a write.
    Io,
}

impl CheckError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            CheckError::NotEggFs => "no EggFS file system on the disk",
            CheckError::Io => "the disk could not be read or written",
        }
    }
}

struct CheckDisk<'a, D: Disk + ?Sized> {
    disk: &'a mut D,
    kept: Option<BTreeMap<u64, Box<Block>>>,
}

impl<D: Disk + ?Sized> Disk for CheckDisk<'_, D> {
    fn blocks(&self) -> u64 {
        self.disk.blocks()
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError> {
        let Some(kept) = &self.kept else {
            return self.disk.read(block, buffer);
        };
        for (index, chunk) in buffer.chunks_mut(BLOCK_SIZE).enumerate() {
            let at = block + index as u64;
            match kept.get(&at) {
                Some(data) => chunk.copy_from_slice(&data[..]),
                None => self.disk.read(at, chunk)?,
            }
        }
        Ok(())
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError> {
        let Some(kept) = &mut self.kept else {
            return self.disk.write(block, buffer);
        };
        for (index, chunk) in buffer.chunks(BLOCK_SIZE).enumerate() {
            let mut data = zeroed_block();
            data.copy_from_slice(chunk);
            kept.insert(block + index as u64, data);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        match self.kept {
            Some(_) => Ok(()),
            None => self.disk.flush(),
        }
    }
}

fn unrepairable(problem: Problem, block: u32) -> Finding {
    Finding {
        problem,
        inode: 0,
        block,
        outcome: Outcome::NotRepairable,
    }
}

#[derive(Debug, Clone, Copy)]
struct FileNames {
    count: u32,
    first_directory: u32,
    parent_listed: bool,
    stated_parent: u32,
}

#[derive(Debug, Clone, Copy)]
struct Contents {
    number: u32,
    limit: u64,
    directory: bool,
}

struct Scan {
    used: Bits,
    reached: Bits,
    files: BTreeMap<u32, FileNames>,
    directories: Vec<(u32, u32)>,
    findings: Vec<Finding>,
    copies: Vec<(u32, u64)>,
    holes: Vec<(u32, u64)>,
    lost: Vec<u32>,
    discarded: BTreeSet<u32>,
    needs_version_2: bool,
}

impl Scan {
    fn new(geometry: &Geometry) -> Self {
        let mut used = Bits::new(geometry.blocks);
        for block in 0..geometry.data_start {
            used.set(block);
        }
        let mut reached = Bits::new(geometry.inodes);
        reached.set(0);
        reached.set(ROOT_INODE);
        Self {
            used,
            reached,
            files: BTreeMap::new(),
            directories: Vec::new(),
            findings: Vec::new(),
            copies: Vec::new(),
            holes: Vec::new(),
            lost: Vec::new(),
            discarded: BTreeSet::new(),
            needs_version_2: false,
        }
    }

    fn found(&mut self, problem: Problem, inode: u32, block: u32) {
        self.findings.push(Finding {
            problem,
            inode,
            block,
            outcome: Outcome::Repairable,
        });
    }

    fn claim_data(
        &mut self,
        geometry: &Geometry,
        contents: Contents,
        block: u32,
        index: u64,
    ) -> bool {
        if index >= contents.limit {
            self.found(Problem::BlockPastEnd, contents.number, block);
            return false;
        }
        if !geometry.is_data_block(block) {
            self.found(Problem::PointerOutsideData, contents.number, block);
            return false;
        }
        if self.used.get(block) {
            self.found(Problem::CrossLinkedBlock, contents.number, block);
            if contents.directory {
                return false;
            }
            self.copies.push((contents.number, index));
            return true;
        }
        self.used.set(block);
        true
    }

    fn claim_pointer_block(
        &mut self,
        geometry: &Geometry,
        contents: Contents,
        block: u32,
        first: u64,
    ) -> bool {
        if first >= contents.limit {
            self.found(Problem::BlockPastEnd, contents.number, block);
            return false;
        }
        if !geometry.is_data_block(block) {
            self.found(Problem::PointerOutsideData, contents.number, block);
            return false;
        }
        if self.used.get(block) {
            self.found(Problem::CrossLinkedBlock, contents.number, block);
            return false;
        }
        self.used.set(block);
        true
    }
}

/// Checks the EggFS file system on `disk` as it is unmounted and, in [`CheckMode::Repair`],
/// repairs what can be repaired safely: it replays the journal as mounting does, walks every
/// directory from the root, frees files removed while open that were left behind, moves
/// inodes in use that have no name to `lost+found`, copies blocks used twice, corrects
/// counts, parents and entries, and rewrites the bitmaps; then it checks the result as
/// [`Volume::check`] does. Repairs go through the journal, a commit at a time; a repair that
/// stops midway is finished by running it again. A second run after a repair finds nothing.
/// In [`CheckMode::ReadOnly`] the same work runs against a copy kept in memory, so the
/// findings say what a repair would do, and nothing is written to `disk`. `now` stamps a new
/// root and `lost+found`.
pub fn check_disk<D: Disk + ?Sized>(
    disk: &mut D,
    mode: CheckMode,
    now: u64,
) -> Result<DiskCheck, CheckError> {
    let mut target = CheckDisk {
        disk,
        kept: match mode {
            CheckMode::ReadOnly => Some(BTreeMap::new()),
            CheckMode::Repair => None,
        },
    };
    let mut findings = Vec::new();
    let Some((geometry, sequence)) = open(&mut target, &mut findings)? else {
        return Ok(finish(mode, findings, None, (0, 0)));
    };
    let io = |_: DiskError| CheckError::Io;
    let free_blocks = count_clear(
        &mut target,
        geometry.block_bitmap,
        geometry.block_bitmap_blocks,
        geometry.blocks,
    )
    .map_err(io)?;
    let free_inodes = count_clear(
        &mut target,
        geometry.inode_bitmap,
        geometry.inode_bitmap_blocks,
        geometry.inodes,
    )
    .map_err(io)?;
    let mut volume = Volume::assemble(target, geometry, sequence);
    volume.free_blocks = free_blocks;
    volume.free_inodes = free_inodes;
    let summary = match volume.repair(now, &mut findings) {
        Ok(summary) => summary,
        Err(Error::Io) => return Err(CheckError::Io),
        Err(_) => {
            findings.push(unrepairable(Problem::RepairFailed, 0));
            None
        }
    };
    let space = volume.space();
    Ok(finish(mode, findings, summary, space))
}

fn finish(
    mode: CheckMode,
    mut findings: Vec<Finding>,
    summary: Option<CheckReport>,
    (blocks, free_blocks): (u64, u64),
) -> DiskCheck {
    if mode == CheckMode::Repair {
        for finding in &mut findings {
            if finding.outcome == Outcome::Repairable {
                finding.outcome = Outcome::Repaired;
            }
        }
    }
    DiskCheck {
        mode,
        findings,
        summary,
        blocks,
        free_blocks,
    }
}

fn open<D: Disk>(
    disk: &mut D,
    findings: &mut Vec<Finding>,
) -> Result<Option<(Geometry, u64)>, CheckError> {
    let io = |_: DiskError| CheckError::Io;
    if disk.blocks() == 0 {
        return Err(CheckError::NotEggFs);
    }
    let mut block = zeroed_block();
    disk.read(0, &mut block[..]).map_err(io)?;
    let Some(geometry) = Geometry::decode(&block[..]) else {
        if block[..8] != MAGIC {
            return Err(CheckError::NotEggFs);
        }
        findings.push(unrepairable(Problem::DamagedSuperblock, 0));
        return Ok(None);
    };
    if u64::from(geometry.blocks) > disk.blocks() {
        findings.push(unrepairable(Problem::SuperblockLargerThanDisk, 0));
        return Ok(None);
    }
    let sequence = match replay(disk, &geometry) {
        Ok((sequence, _)) => sequence,
        Err(MountError::Io) => return Err(CheckError::Io),
        Err(_) => {
            findings.push(Finding {
                problem: Problem::DamagedJournal,
                inode: 0,
                block: geometry.journal,
                outcome: Outcome::Repairable,
            });
            retire_journal(disk, &geometry).map_err(io)?
        }
    };
    disk.read(0, &mut block[..]).map_err(io)?;
    match Geometry::decode(&block[..]) {
        Some(replayed) if geometry.same_layout(&replayed) => Ok(Some((replayed, sequence))),
        _ => {
            findings.push(unrepairable(Problem::DamagedSuperblock, 0));
            Ok(None)
        }
    }
}

pub(crate) fn retire_journal<D: Disk>(disk: &mut D, geometry: &Geometry) -> Result<u64, DiskError> {
    let journal = u64::from(geometry.journal);
    let mut block = zeroed_block();
    disk.read(journal, &mut block[..])?;
    let sequence =
        Descriptor::decode(&block[..]).map_or(1, |descriptor| descriptor.sequence.wrapping_add(1));
    block.fill(0);
    block[..8].copy_from_slice(&DESCRIPTOR_MAGIC);
    put_u64(&mut block[..], 8, sequence);
    disk.write(journal, &block[..])?;
    disk.flush()?;
    Ok(sequence.wrapping_add(2))
}

fn lost_name(number: u32, attempt: u32) -> String {
    match attempt {
        0 => format!("#{number}"),
        _ => format!("#{number}.{attempt}"),
    }
}

impl<D: Disk> Volume<D> {
    fn repair(
        &mut self,
        now: u64,
        findings: &mut Vec<Finding>,
    ) -> Result<Option<CheckReport>, Error> {
        let mut scan = Scan::new(&self.geometry);
        let repaired = self.repair_scanned(&mut scan, now);
        findings.append(&mut scan.findings);
        repaired?;
        match self.check() {
            Ok(report) => Ok(Some(report)),
            Err(inconsistency) => {
                findings.push(Finding {
                    problem: Problem::RepairIncomplete,
                    inode: inconsistency.inode,
                    block: inconsistency.block,
                    outcome: Outcome::NotRepairable,
                });
                Ok(None)
            }
        }
    }

    fn repair_scanned(&mut self, scan: &mut Scan, now: u64) -> Result<(), Error> {
        let geometry = self.geometry;
        self.repair_root(scan, now)?;
        scan.directories.push((ROOT_INODE, ROOT_INODE));
        self.walk(scan)?;
        let mut orphans = self.find_lost(scan)?;
        orphans.retain(|&number| !scan.reached.get(number));
        for &number in &orphans {
            scan.found(Problem::OrphanLeft, number, 0);
        }
        self.fix_names(scan)?;
        if geometry.version < VERSION && scan.needs_version_2 {
            scan.found(Problem::OldVersion, 0, 0);
            self.ensure_room()?;
            self.upgrade()?;
        }
        self.reclaim(&orphans)?;
        let discarded: Vec<u32> = scan.discarded.iter().copied().collect();
        self.reclaim(&discarded)?;
        let free_blocks = self.rebuild_bitmap(
            (geometry.block_bitmap, geometry.block_bitmap_blocks),
            &scan.used,
            geometry.blocks,
            Problem::BlockBitmap,
            &mut scan.findings,
        )?;
        let free_inodes = self.rebuild_bitmap(
            (geometry.inode_bitmap, geometry.inode_bitmap_blocks),
            &scan.reached,
            geometry.inodes,
            Problem::InodeBitmap,
            &mut scan.findings,
        )?;
        self.free_blocks = free_blocks;
        self.free_inodes = free_inodes;
        self.commit()?;
        self.indexes.clear();
        self.copy_shared_blocks(&scan.copies)?;
        self.fill_holes(&scan.holes)?;
        let lost = core::mem::take(&mut scan.lost);
        self.adopt_lost(&lost, now, scan)?;
        self.commit()
    }

    fn repair_root(&mut self, scan: &mut Scan, now: u64) -> Result<(), Error> {
        let root = match self.inode(ROOT_INODE) {
            Ok(root) if root.kind == InodeKind::Directory => root,
            Ok(_) | Err(Error::Corrupt) => {
                scan.found(Problem::DamagedRoot, ROOT_INODE, 0);
                let generation = self
                    .inode(ROOT_INODE)
                    .map_or(1, |old| old.generation.wrapping_add(1));
                let fresh = Inode::new(InodeKind::Directory, 0o755, ROOT_INODE, generation, now);
                self.write_inode(ROOT_INODE, &fresh)?;
                fresh
            }
            Err(error) => return Err(error),
        };
        self.root_generation = root.generation;
        Ok(())
    }

    fn walk(&mut self, scan: &mut Scan) -> Result<(), Error> {
        while let Some((number, parent)) = scan.directories.pop() {
            self.sweep_directory(number, parent, scan)?;
        }
        Ok(())
    }

    fn sweep_tree(
        &mut self,
        block: u32,
        depth: usize,
        first: u64,
        contents: Contents,
        scan: &mut Scan,
        blocks: &mut Vec<(u64, u32)>,
    ) -> Result<(), Error> {
        let mut pointers = zeroed_block();
        pointers.copy_from_slice(&self.cache.get(&mut self.disk, block)?[..]);
        let span = POINTERS_PER_BLOCK.pow(depth as u32);
        let geometry = self.geometry;
        let mut changed = false;
        for slot in 0..POINTERS_PER_BLOCK {
            let at = slot as usize * 4;
            let child = u32_at(&pointers[..], at);
            if child == 0 {
                continue;
            }
            let index = first + slot * span;
            let keep = match depth {
                0 => {
                    let keep = scan.claim_data(&geometry, contents, child, index);
                    if keep && contents.directory {
                        blocks.push((index, child));
                    }
                    keep
                }
                _ => {
                    let keep = scan.claim_pointer_block(&geometry, contents, child, index);
                    if keep {
                        self.sweep_tree(child, depth - 1, index, contents, scan, blocks)?;
                    }
                    keep
                }
            };
            if !keep {
                put_u32(&mut pointers[..], at, 0);
                changed = true;
            }
        }
        if changed {
            self.ensure_room()?;
            self.cache
                .get_mut(&mut self.disk, block, false)?
                .copy_from_slice(&pointers[..]);
        }
        Ok(())
    }

    fn sweep_pointers(
        &mut self,
        inode: &mut Inode,
        contents: Contents,
        scan: &mut Scan,
        blocks: &mut Vec<(u64, u32)>,
    ) -> Result<(), Error> {
        let geometry = self.geometry;
        for (slot, pointer) in inode.direct.iter_mut().enumerate() {
            if *pointer == 0 {
                continue;
            }
            if !scan.claim_data(&geometry, contents, *pointer, slot as u64) {
                *pointer = 0;
            } else if contents.directory {
                blocks.push((slot as u64, *pointer));
            }
        }
        for (level, pointer) in inode.indirect.iter_mut().enumerate() {
            if *pointer == 0 {
                continue;
            }
            let first = tree_first(level);
            match scan.claim_pointer_block(&geometry, contents, *pointer, first) {
                true => self.sweep_tree(*pointer, level, first, contents, scan, blocks)?,
                false => *pointer = 0,
            }
        }
        Ok(())
    }

    fn valid_link_target(&mut self, inode: &Inode) -> Result<bool, Error> {
        let block = inode.direct[0];
        if inode.size == 0
            || inode.size > MAX_LINK_TARGET as u64
            || !self.geometry.is_data_block(block)
        {
            return Ok(false);
        }
        let length = inode.size as usize;
        let target = self.read_data_block(block)?;
        Ok(!target[..length].contains(&0))
    }

    fn examine(&mut self, number: u32, scan: &mut Scan) -> Result<Option<Inode>, Error> {
        if scan.discarded.contains(&number) {
            return Ok(None);
        }
        let mut inode = match self.inode(number) {
            Ok(inode) => inode,
            Err(Error::Corrupt) => {
                scan.found(Problem::UnreadableInode, number, 0);
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        match inode.kind {
            InodeKind::Free => {
                scan.found(Problem::EntryToFreeInode, number, 0);
                return Ok(None);
            }
            InodeKind::Directory => return Ok(Some(inode)),
            InodeKind::SymbolicLink => {
                if !self.valid_link_target(&inode)? {
                    scan.found(Problem::BadSymbolicLink, number, inode.direct[0]);
                    scan.discarded.insert(number);
                    return Ok(None);
                }
                scan.needs_version_2 = true;
            }
            InodeKind::Regular => {}
        }
        let original = inode;
        if inode.size > MAX_FILE_BYTES {
            scan.found(Problem::FileTooLarge, number, 0);
            inode.size = MAX_FILE_BYTES;
        }
        let contents = Contents {
            number,
            limit: inode.size.div_ceil(BLOCK_SIZE as u64),
            directory: false,
        };
        self.sweep_pointers(&mut inode, contents, scan, &mut Vec::new())?;
        if inode != original {
            self.ensure_room()?;
            self.write_inode(number, &inode)?;
        }
        Ok(Some(inode))
    }

    fn accept_entry(
        &mut self,
        directory: u32,
        block: u32,
        record: &Record<'_>,
        names: &BTreeSet<Vec<u8>>,
        scan: &mut Scan,
    ) -> Result<Option<InodeKind>, Error> {
        if !valid_name(record.name) {
            scan.found(Problem::BadName, directory, block);
            return Ok(None);
        }
        if names.contains(record.name) {
            scan.found(Problem::DuplicateName, directory, block);
            return Ok(None);
        }
        let number = record.inode;
        if number < FIRST_FREE_INODE || number >= self.geometry.inodes {
            scan.found(Problem::EntryToInvalidInode, directory, block);
            return Ok(None);
        }
        let kind = match scan.reached.get(number) {
            true => {
                let Some(names) = scan.files.get_mut(&number) else {
                    scan.found(Problem::DirectoryNamedTwice, number, block);
                    return Ok(None);
                };
                names.count += 1;
                names.parent_listed |= names.stated_parent == directory;
                scan.needs_version_2 = true;
                self.inode(number)?.kind
            }
            false => {
                let Some(inode) = self.examine(number, scan)? else {
                    return Ok(None);
                };
                scan.reached.set(number);
                match inode.kind {
                    InodeKind::Directory => scan.directories.push((number, directory)),
                    _ => {
                        let names = FileNames {
                            count: 1,
                            first_directory: directory,
                            parent_listed: inode.parent == directory,
                            stated_parent: inode.parent,
                        };
                        scan.files.insert(number, names);
                    }
                }
                inode.kind
            }
        };
        if kind != record.kind {
            scan.found(Problem::EntryKindWrong, directory, block);
        }
        Ok(Some(kind))
    }

    fn sweep_records(
        &mut self,
        number: u32,
        blocks: &[(u64, u32)],
        scan: &mut Scan,
    ) -> Result<(u32, u32), Error> {
        let mut names: BTreeSet<Vec<u8>> = BTreeSet::new();
        let (mut entries, mut subdirectories) = (0u32, 0u32);
        let mut data = zeroed_block();
        let mut kept = zeroed_block();
        for &(_, block) in blocks {
            data.copy_from_slice(&self.cache.get(&mut self.disk, block)?[..]);
            kept.fill(0);
            let found_before = scan.findings.len();
            let (mut offset, mut end) = (0, 0);
            loop {
                let record = match record_at(&data[..], offset) {
                    Ok(Some(record)) => record,
                    Ok(None) => break,
                    Err(()) => {
                        scan.found(Problem::MalformedDirectoryBlock, number, block);
                        break;
                    }
                };
                offset += record.length;
                let Some(kind) = self.accept_entry(number, block, &record, &names, scan)? else {
                    continue;
                };
                names.insert(record.name.to_vec());
                entries += 1;
                subdirectories += u32::from(kind == InodeKind::Directory);
                write_record(&mut kept[..], end, record.inode, kind, record.name);
                end += record.length;
            }
            if kept[..] != data[..] {
                if scan.findings.len() == found_before {
                    scan.found(Problem::MalformedDirectoryBlock, number, block);
                }
                self.ensure_room()?;
                self.cache
                    .get_mut(&mut self.disk, block, false)?
                    .copy_from_slice(&kept[..]);
            }
        }
        Ok((entries, subdirectories))
    }

    fn sweep_directory(&mut self, number: u32, parent: u32, scan: &mut Scan) -> Result<(), Error> {
        let mut inode = self.inode(number)?;
        let original = inode;
        let contents = Contents {
            number,
            limit: MAX_DIRECTORY_BLOCKS,
            directory: true,
        };
        let mut blocks = Vec::new();
        self.sweep_pointers(&mut inode, contents, scan, &mut blocks)?;
        let length = blocks.last().map_or(0, |&(index, _)| index + 1);
        if inode.size != length * BLOCK_SIZE as u64 {
            scan.found(Problem::DirectorySize, number, 0);
            inode.size = length * BLOCK_SIZE as u64;
        }
        let mut expected = 0;
        let holes_before = scan.holes.len();
        for &(index, _) in &blocks {
            scan.holes
                .extend((expected..index).map(|hole| (number, hole)));
            expected = index + 1;
        }
        if scan.holes.len() > holes_before {
            scan.found(Problem::DirectoryHole, number, 0);
        }
        let (entries, subdirectories) = self.sweep_records(number, &blocks, scan)?;
        if (entries, subdirectories) != (inode.entries, inode.child_directories) {
            scan.found(Problem::DirectoryCounts, number, 0);
            inode.entries = entries;
            inode.child_directories = subdirectories;
        }
        if parent != NO_PARENT && inode.parent != parent {
            scan.found(Problem::WrongParent, number, 0);
            inode.parent = parent;
        }
        if number != ROOT_INODE && (inode.orphan || inode.next_orphan != 0) {
            scan.found(Problem::NamedOrphan, number, 0);
        }
        inode.orphan = false;
        inode.next_orphan = 0;
        if inode.more_names != 0 {
            scan.found(Problem::LinkCount, number, 0);
            inode.more_names = 0;
        }
        if inode != original {
            self.ensure_room()?;
            self.write_inode(number, &inode)?;
        }
        Ok(())
    }

    fn named_directories(&mut self, number: u32, named: &mut BTreeSet<u32>) -> Result<(), Error> {
        let inode = self.inode(number)?;
        let blocks = (inode.size / BLOCK_SIZE as u64).min(MAX_DIRECTORY_BLOCKS);
        for index in 0..blocks {
            let block = match self.locate(&inode, index) {
                Ok(block) if self.geometry.is_data_block(block) => block,
                Err(Error::Io) => return Err(Error::Io),
                Ok(_) | Err(_) => continue,
            };
            let data = self.cache.get(&mut self.disk, block)?;
            let mut offset = 0;
            while let Ok(Some(record)) = record_at(&data[..], offset) {
                if record.kind == InodeKind::Directory {
                    named.insert(record.inode);
                }
                offset += record.length;
            }
        }
        Ok(())
    }

    fn find_lost(&mut self, scan: &mut Scan) -> Result<Vec<u32>, Error> {
        let (mut directories, mut files, mut orphans) = (Vec::new(), Vec::new(), Vec::new());
        for number in FIRST_FREE_INODE..self.geometry.inodes {
            if scan.reached.get(number) || scan.discarded.contains(&number) {
                continue;
            }
            let inode = match self.inode(number) {
                Ok(inode) => inode,
                Err(Error::Corrupt) => continue,
                Err(error) => return Err(error),
            };
            match inode.kind {
                InodeKind::Free => {}
                _ if inode.orphan => orphans.push(number),
                InodeKind::Directory => directories.push(number),
                _ => files.push(number),
            }
        }
        let mut named = BTreeSet::new();
        for &number in &directories {
            self.named_directories(number, &mut named)?;
        }
        let tops = directories
            .iter()
            .copied()
            .filter(|number| !named.contains(number));
        for number in tops.chain(directories.iter().copied()).chain(files) {
            if !scan.reached.get(number) && !scan.discarded.contains(&number) {
                self.attach_lost(number, scan)?;
            }
        }
        Ok(orphans)
    }

    fn attach_lost(&mut self, number: u32, scan: &mut Scan) -> Result<(), Error> {
        let Some(inode) = self.examine(number, scan)? else {
            scan.discarded.insert(number);
            return Ok(());
        };
        scan.reached.set(number);
        scan.lost.push(number);
        scan.found(Problem::Unreachable, number, 0);
        if inode.kind == InodeKind::Directory {
            scan.directories.push((number, NO_PARENT));
            return self.walk(scan);
        }
        let names = FileNames {
            count: 0,
            first_directory: NO_PARENT,
            parent_listed: false,
            stated_parent: inode.parent,
        };
        scan.files.insert(number, names);
        Ok(())
    }

    fn fix_names(&mut self, scan: &mut Scan) -> Result<(), Error> {
        let files = core::mem::take(&mut scan.files);
        for (&number, names) in &files {
            if names.count == 0 {
                continue;
            }
            let mut inode = self.inode(number)?;
            let original = inode;
            inode.more_names = names.count - 1;
            if !names.parent_listed {
                inode.parent = names.first_directory;
            }
            inode.orphan = false;
            inode.next_orphan = 0;
            if inode.more_names != original.more_names {
                scan.found(Problem::LinkCount, number, 0);
            }
            if inode.parent != original.parent {
                scan.found(Problem::WrongParent, number, 0);
            }
            if original.orphan || original.next_orphan != 0 {
                scan.found(Problem::NamedOrphan, number, 0);
            }
            if inode != original {
                self.ensure_room()?;
                self.write_inode(number, &inode)?;
            }
        }
        Ok(())
    }

    fn reclaim(&mut self, numbers: &[u32]) -> Result<(), Error> {
        for &number in numbers {
            let old = match self.inode(number) {
                Ok(old) => old,
                Err(Error::Corrupt) => continue,
                Err(error) => return Err(error),
            };
            let generation = old.generation.wrapping_add(1);
            let free = Inode::new(InodeKind::Free, 0, 0, generation, old.modified);
            self.ensure_room()?;
            self.write_inode(number, &free)?;
        }
        Ok(())
    }

    fn rebuild_bitmap(
        &mut self,
        (start, blocks): (u32, u32),
        used: &Bits,
        count: u32,
        problem: Problem,
        findings: &mut Vec<Finding>,
    ) -> Result<u64, Error> {
        let keep_committed = problem == Problem::BlockBitmap;
        let mut clear = 0u64;
        let mut reported = false;
        let mut expected = zeroed_block();
        for index in 0..blocks {
            expected.fill(0);
            for bit in 0..BITS_PER_BLOCK {
                let number = index * BITS_PER_BLOCK + bit;
                if number >= count || used.get(number) {
                    expected[(bit / 8) as usize] |= 1 << (bit % 8);
                } else {
                    clear += 1;
                }
            }
            let current = self.cache.get(&mut self.disk, start + index)?;
            let first_difference = (0..BITS_PER_BLOCK).find(|&bit| {
                let byte = (bit / 8) as usize;
                (current[byte] ^ expected[byte]) & (1 << (bit % 8)) != 0
            });
            let Some(first) = first_difference else {
                continue;
            };
            if !reported {
                reported = true;
                findings.push(Finding {
                    problem,
                    inode: 0,
                    block: index * BITS_PER_BLOCK + first,
                    outcome: Outcome::Repairable,
                });
            }
            self.ensure_room()?;
            self.cache
                .get_mut(&mut self.disk, start + index, keep_committed)?
                .copy_from_slice(&expected[..]);
        }
        Ok(clear)
    }

    fn copy_shared_blocks(&mut self, copies: &[(u32, u64)]) -> Result<(), Error> {
        let mut copy = zeroed_block();
        for &(number, index) in copies {
            self.ensure_room()?;
            let mut inode = self.inode(number)?;
            let shared = self.locate(&inode, index)?;
            if shared == 0 {
                continue;
            }
            copy.copy_from_slice(&self.read_data_block(shared)?[..]);
            let block = self.allocate_block()?;
            self.disk
                .write(u64::from(block), &copy[..])
                .map_err(|_: DiskError| Error::Io)?;
            self.set_mapping(&mut inode, index, block)?;
            self.write_inode(number, &inode)?;
        }
        Ok(())
    }

    fn fill_holes(&mut self, holes: &[(u32, u64)]) -> Result<(), Error> {
        for &(number, index) in holes {
            self.ensure_room()?;
            let mut inode = self.inode(number)?;
            let block = self.allocate_block()?;
            self.cache.insert_zeroed(block);
            self.set_mapping(&mut inode, index, block)?;
            self.write_inode(number, &inode)?;
        }
        Ok(())
    }

    fn lost_and_found(&mut self, now: u64) -> Result<u32, Error> {
        let root = self.root();
        for attempt in 0..NAME_ATTEMPTS {
            let name = match attempt {
                0 => String::from(LOST_AND_FOUND),
                _ => format!("{LOST_AND_FOUND}.{attempt}"),
            };
            match self.lookup(root, name.as_bytes())? {
                Some(node) if kind_of(node) == Kind::Directory => return Ok(inode_of(node)),
                Some(_) => continue,
                None => {
                    let node = self.create(root, name.as_bytes(), Kind::Directory, 0o700, now)?;
                    return Ok(inode_of(node));
                }
            }
        }
        Err(Error::Exists)
    }

    fn adopt_lost(&mut self, lost: &[u32], now: u64, scan: &mut Scan) -> Result<(), Error> {
        if lost.is_empty() {
            return Ok(());
        }
        let directory = self.lost_and_found(now)?;
        for &number in lost {
            let mut adopted = false;
            for attempt in 0..NAME_ATTEMPTS {
                match self.adopt(
                    directory,
                    lost_name(number, attempt).as_bytes(),
                    number,
                    now,
                ) {
                    Ok(()) => {
                        adopted = true;
                        break;
                    }
                    Err(Error::Exists) => continue,
                    Err(error) => return Err(error),
                }
            }
            if !adopted {
                scan.findings.push(Finding {
                    problem: Problem::LostAndFoundFull,
                    inode: number,
                    block: 0,
                    outcome: Outcome::NotRepairable,
                });
            }
        }
        Ok(())
    }
}
