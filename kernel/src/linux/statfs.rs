//! How full file systems are (D171): `statfs` and `fstatfs`, which `df`, Files' free space and
//! build tools (cargo checks a target folder's file system, glibc's `pathconf` reads it) call.
//!
//! Both write Linux's `struct statfs`, one layout on the three architectures (each field a
//! 64-bit word but the file system id's two 32-bit halves): the file system's type number,
//! its block size, its blocks, free blocks and the blocks a program may still take (all of the
//! free ones: EggOS keeps none back for root), its nodes and free nodes (0 when it does not
//! count them), its id, the longest name, the fragment size (the block size) and its flags
//! (`ST_VALID`, and `ST_RDONLY` for one nothing can be written to). A file system answers
//! from what it keeps in memory ([`crate::vfs::FileSystem::usage`]): nothing is read from a
//! disk. A descriptor that names no node reports the pseudo file system Linux keeps such
//! files on: pipes `pipefs`, sockets `sockfs`, memory files `tmpfs`, event files
//! `anon_inodefs`, and the console, `/dev/window` and `/dev/agent` `/dev`'s.

use super::descriptors::{Descriptors, FileKind};
use super::files;
use super::thread::LinuxThread;
use super::{CWD, Errno};
use crate::process::Process;
use crate::vfs::{NAME_MAX, Usage, magic};
use eggos_hal::Platform;

/// The size of `struct statfs`.
pub const STATFS_SIZE: usize = 120;

/// `struct statfs`'s `f_flags` (`ST_*`).
pub mod flag {
    /// Nothing can be written.
    pub const RDONLY: u64 = 0x1;
    /// The flags are valid (Linux sets it on every answer).
    pub const VALID: u64 = 0x20;
}

const DEVICE_FILES: (u32, u32) = (0, 5);

mod offset {
    pub const TYPE: usize = 0;
    pub const BLOCK_SIZE: usize = 8;
    pub const BLOCKS: usize = 16;
    pub const FREE: usize = 24;
    pub const AVAILABLE: usize = 32;
    pub const FILES: usize = 40;
    pub const FREE_FILES: usize = 48;
    pub const ID: usize = 56;
    pub const NAME_LENGTH: usize = 64;
    pub const FRAGMENT_SIZE: usize = 72;
    pub const FLAGS: usize = 80;
}

/// `usage` of the file system with device number `device` as `struct statfs`.
pub fn statfs_bytes(usage: &Usage, device: (u32, u32)) -> [u8; STATFS_SIZE] {
    let mut bytes = [0u8; STATFS_SIZE];
    let mut put = |at: usize, value: u64| bytes[at..at + 8].copy_from_slice(&value.to_ne_bytes());
    put(offset::TYPE, usage.magic);
    put(offset::BLOCK_SIZE, usage.block_size);
    put(offset::BLOCKS, usage.blocks);
    put(offset::FREE, usage.free_blocks);
    put(offset::AVAILABLE, usage.free_blocks);
    put(offset::FILES, usage.nodes);
    put(offset::FREE_FILES, usage.free_nodes);
    put(offset::ID, u64::from(encoded_device(device)));
    put(offset::NAME_LENGTH, NAME_MAX as u64);
    put(offset::FRAGMENT_SIZE, usage.block_size);
    let read_only = if usage.read_only { flag::RDONLY } else { 0 };
    put(offset::FLAGS, flag::VALID | read_only);
    bytes
}

fn encoded_device((major, minor): (u32, u32)) -> u32 {
    (minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12)
}

fn usage_of(kind: &FileKind) -> (Usage, (u32, u32)) {
    if let Some(node) = kind.node() {
        let file_system = node.file_system();
        return (file_system.usage(), file_system.device());
    }
    let pseudo = |kind| (Usage::without_blocks(kind, false), (0, 0));
    match kind {
        FileKind::Pipe(_) => pseudo(magic::PIPEFS),
        FileKind::Socket(_) | FileKind::InetSocket(_) => pseudo(magic::SOCKFS),
        FileKind::MemoryFile(_) => pseudo(magic::TMPFS),
        FileKind::Event(_) => pseudo(magic::ANON_INODE_FS),
        FileKind::Console
        | FileKind::PseudoTerminal(_)
        | FileKind::Window(_)
        | FileKind::AgentChannel(_)
        | FileKind::MemoryDevice(..)
        | FileKind::Regular(_)
        | FileKind::Directory(_)
        | FileKind::Path(_)
        | FileKind::Snapshot(_) => (Usage::without_blocks(magic::TMPFS, true), DEVICE_FILES),
    }
}

fn write<P: Platform>(
    process: &Process<P>,
    buffer: u64,
    usage: &Usage,
    device: (u32, u32),
) -> Result<u64, Errno> {
    process
        .write_user_memory(buffer, &statfs_bytes(usage, device))
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// `statfs(path, buffer)`: writes the `struct statfs` of the file system holding what `path`
/// names (from the working directory, every symbolic link followed) to `buffer`. As Linux:
/// the path's errors (`ENOENT` for an empty one), then `EFAULT` for `buffer`.
pub fn statfs<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    path: u64,
    buffer: u64,
) -> Result<u64, Errno> {
    let node = files::lookup(process, thread, CWD, path)?;
    let file_system = node.file_system();
    write(process, buffer, &file_system.usage(), file_system.device())
}

/// `fstatfs(fd, buffer)`: as [`statfs`] for the file `fd` names, `O_PATH` descriptors
/// included. As Linux: `EBADF` if `fd` is not open, then `EFAULT` for `buffer`. `fd` is an
/// `unsigned int`.
pub fn fstatfs<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    fd: u64,
    buffer: u64,
) -> Result<u64, Errno> {
    let file = files.file(fd)?;
    let (usage, device) = usage_of(file.kind());
    drop(file);
    write(process, buffer, &usage, device)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(bytes: &[u8; STATFS_SIZE], at: usize) -> u64 {
        u64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn statfs_bytes_are_linux_s_layout() {
        let usage = Usage {
            magic: magic::EGGFS,
            block_size: 4096,
            blocks: 1000,
            free_blocks: 600,
            nodes: 128,
            free_nodes: 100,
            read_only: false,
        };
        let bytes = statfs_bytes(&usage, (254, 2));
        assert_eq!(word(&bytes, offset::TYPE), magic::EGGFS);
        assert_eq!(word(&bytes, offset::BLOCK_SIZE), 4096);
        assert_eq!(word(&bytes, offset::BLOCKS), 1000);
        assert_eq!(word(&bytes, offset::FREE), 600);
        assert_eq!(word(&bytes, offset::AVAILABLE), 600);
        assert_eq!(word(&bytes, offset::FILES), 128);
        assert_eq!(word(&bytes, offset::FREE_FILES), 100);
        assert_eq!(word(&bytes, offset::ID), 254 << 8 | 2);
        assert_eq!(word(&bytes, offset::NAME_LENGTH), 255);
        assert_eq!(word(&bytes, offset::FRAGMENT_SIZE), 4096);
        assert_eq!(word(&bytes, offset::FLAGS), flag::VALID);
        assert!(bytes[88..].iter().all(|&byte| byte == 0));
        let read_only = Usage {
            read_only: true,
            ..usage
        };
        assert_eq!(
            word(&statfs_bytes(&read_only, (0, 5)), offset::FLAGS),
            flag::VALID | flag::RDONLY
        );
    }

    #[test]
    fn device_numbers_are_encoded_as_linux_s_new_encode_dev() {
        assert_eq!(encoded_device((0, 5)), 5);
        assert_eq!(encoded_device((254, 2)), 0xfe02);
        assert_eq!(encoded_device((8, 0x123)), 0x0010_0823);
    }

    #[test]
    fn files_with_no_node_report_linux_s_pseudo_file_systems() {
        assert_eq!(usage_of(&FileKind::Console).0.magic, magic::TMPFS);
        assert_eq!(usage_of(&FileKind::Console).1, DEVICE_FILES);
    }
}
