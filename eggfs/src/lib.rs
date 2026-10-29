//! EggFS (D131): EggOS's own journaled file system for disks, as docs/eggfs.md specifies its
//! on-disk format.
//!
//! - **Blocks** of [`BLOCK_SIZE`] bytes: a superblock, a block bitmap, an inode bitmap, the
//!   journal, the inode table, then data. Files and directories map their contents through 12
//!   direct pointers and single, double and triple indirect blocks; a directory's contents are
//!   packed records of an inode number, a kind and a name. A symbolic link keeps its target
//!   as its contents; a regular file or a symbolic link may have several names (hard links).
//! - **Crash safety.** Metadata (bitmaps, inodes, directory and indirect blocks) changes in a
//!   cache and reaches the disk only through the journal, one transaction at a time: the
//!   blocks are written to the journal, flushed, a commit record holding the transaction's
//!   CRC-64 is written and flushed, then the blocks go to their places and are flushed. File data is written copy on
//!   write, to blocks the last committed state does not use, before the commit. A crash
//!   anywhere leaves exactly the state of the last commit (an operation that frees more than
//!   a transaction holds commits as it goes, each step consistent), which mounting restores
//!   ([`Volume::mount`] replays a committed transaction whose blocks did not all reach their
//!   places).
//! - **Use.** [`probe`] says what a disk holds, [`format`] writes an empty file system,
//!   [`Volume::mount`] mounts one, and a [`Volume`]'s methods read and change it; changes are
//!   committed by [`Volume::commit`] and, before an operation, when the journal would not hold
//!   more. [`Volume::check`] verifies everything a mounted volume holds.
//! - **Files removed while open.** [`Volume::hold`] keeps a file from being freed when its
//!   last name goes: it becomes an orphan, still usable through its node until
//!   [`Volume::release`] lets go of its last hold; mounting frees the orphans a crash left.
//! - **Versions.** [`format`] writes version [`VERSION`]; a version 1 volume mounts and is
//!   upgraded, in the same transaction, by the first change that needs version 2.
//! - **Work disks** (D166): [`make_work_disk`] marks a volume as a host's folder for `/work`,
//!   [`is_work_disk`] tells one apart and [`work_folder`] finds its folder.
//!
//! A `Volume` is not shared: whoever owns it serialises its use.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod cache;
mod check;
mod checksum;
mod journal;
mod layout;
pub mod memory;
mod volume;
mod work_disk;

#[cfg(test)]
mod tests;

pub use check::{CheckReport, Inconsistency};
pub use journal::{Contents, FormatError, format, probe};
pub use layout::{BLOCK_SIZE, MAX_LINK_TARGET, MAX_LINKS, MAX_NAME, VERSION};
pub use volume::{Attributes, DirectoryEntry, Volume};
pub use work_disk::{WORK_DISK_MARKER, WORK_FOLDER, is_work_disk, make_work_disk, work_folder};

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A directory.
    Directory,
    /// A regular file.
    Regular,
    /// A symbolic link, whose contents are its target.
    SymbolicLink,
}

const DIRECTORY_BIT: u64 = 1 << 32;
const SYMBOLIC_LINK_BIT: u64 = 1 << 33;
const GENERATION_SHIFT: u32 = 34;
pub(crate) const GENERATION_MASK: u32 = 0x3fff_ffff;

/// The kind of node `node` numbers, which its number holds.
pub fn kind_of(node: u64) -> Kind {
    if node & DIRECTORY_BIT != 0 {
        Kind::Directory
    } else if node & SYMBOLIC_LINK_BIT != 0 {
        Kind::SymbolicLink
    } else {
        Kind::Regular
    }
}

fn node_number(inode: u32, generation: u32, kind: Kind) -> u64 {
    let kind_bits = match kind {
        Kind::Directory => DIRECTORY_BIT,
        Kind::SymbolicLink => SYMBOLIC_LINK_BIT,
        Kind::Regular => 0,
    };
    u64::from(generation & GENERATION_MASK) << GENERATION_SHIFT | kind_bits | u64::from(inode)
}

fn inode_of(node: u64) -> u32 {
    node as u32
}

fn generation_of(node: u64) -> u32 {
    (node >> GENERATION_SHIFT) as u32
}

/// A disk failed a transfer or a flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskError;

/// A disk, addressed in blocks of [`BLOCK_SIZE`] bytes.
pub trait Disk {
    /// Its size in blocks.
    fn blocks(&self) -> u64;
    /// Reads the blocks from `block` on into `buffer`, a whole number of blocks.
    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError>;
    /// Writes `buffer`, a whole number of blocks, to the blocks from `block` on.
    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError>;
    /// Returns once every write before it is on stable storage.
    fn flush(&mut self) -> Result<(), DiskError>;
}

/// Why an operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Nothing has that name, or the node is gone.
    NotFound,
    /// The name is taken, or is empty, `.` or `..` where a new name is made.
    Exists,
    /// A directory was needed.
    NotDirectory,
    /// A directory is not what this takes.
    IsDirectory,
    /// The directory has entries.
    NotEmpty,
    /// The disk or its inodes are used up.
    NoSpace,
    /// The name is longer than [`MAX_NAME`] bytes.
    NameTooLong,
    /// The change makes no sense: a name with `/` or NUL, a directory moved into itself, a
    /// symbolic link's target that is empty, too long or holds NUL, a write to a symbolic link.
    InvalidArgument,
    /// The file would be larger than the format allows.
    TooBig,
    /// A rename's new name is empty, `.` or `..`.
    Busy,
    /// What the disk holds is inconsistent.
    Corrupt,
    /// The disk failed, now or in an earlier commit (the volume then refuses every change).
    Io,
    /// The file already has [`MAX_LINKS`] names.
    TooManyLinks,
}

/// Why [`Volume::mount`] refused a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    /// The disk holds no EggFS superblock.
    NotEggFs,
    /// The superblock describes more blocks than the disk has, or the root is not a directory.
    Corrupt,
    /// The disk could not be read or the journal could not be replayed.
    Io,
}

impl MountError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            MountError::NotEggFs => "no EggFS file system on the disk",
            MountError::Corrupt => "the EggFS file system on the disk is damaged",
            MountError::Io => "the disk could not be read or written",
        }
    }
}

/// What mounting did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mounted {
    /// Blocks a committed transaction had not put in place, which the journal restored.
    pub recovered_blocks: u32,
    /// Files removed while open that a crash or a power-off left behind, which mounting freed.
    pub freed_orphans: u32,
}
