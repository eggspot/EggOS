//! File status (M6, D74): `fstat`, `newfstatat` (`fstatat` in the generic table) and `statx`
//! on the standard descriptors, which are the console (D61). glibc's stdio asks `fstat`
//! whether a stream is a character device before it line-buffers a terminal, and Rust's std
//! asks `statx` first for a file's metadata, falling back to `fstatat`.
//!
//! The console answers as Linux's `/dev/console` does for `init`: a character device, major 5
//! minor 1, owned by root with mode `0600`, empty, on a device file system ([`CONSOLE`]). Its
//! times are the Unix epoch, when the wall clock starts (D73). Paths wait for the file system
//! (M7): a call that names a file, rather than a descriptor with `AT_EMPTY_PATH`, finds none.

use super::{Errno, is_console};
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// The flags `newfstatat` and `statx` take (`AT_*`), the generic values, which x86_64 shares.
pub mod at {
    /// The descriptor the path is relative to: the working directory.
    pub const FDCWD: i32 = -100;
    /// Report a symbolic link itself.
    pub const SYMLINK_NOFOLLOW: u64 = 0x100;
    /// Do not mount an automount point.
    pub const NO_AUTOMOUNT: u64 = 0x800;
    /// An empty path names the descriptor itself.
    pub const EMPTY_PATH: u64 = 0x1000;
    /// `statx`'s synchronisation choice: two bits.
    pub const STATX_SYNC_TYPE: u64 = 0x6000;
}

/// `statx`'s mask bits (`STATX_*`).
pub mod statx_mask {
    /// The fields `stat` has: type, mode, links, owner, group, times, inode, size, blocks.
    pub const BASIC_STATS: u32 = 0x7ff;
    /// A bit Linux keeps for a larger `struct statx`: asking for it is invalid.
    pub const RESERVED: u32 = 0x8000_0000;
}

/// A file's status, as the calls report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub device: Device,
    pub inode: u64,
    pub mode: u32,
    pub links: u32,
    pub owner: u32,
    pub group: u32,
    pub represented_device: Device,
    pub size: u64,
    pub block_size: u32,
    pub blocks: u64,
}

/// A device number: its major and minor parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub major: u32,
    pub minor: u32,
}

impl Device {
    /// The device number as a 64-bit `dev_t`, in the encoding glibc's and musl's `major` and
    /// `minor` decode.
    pub const fn encoded(self) -> u64 {
        let (major, minor) = (self.major as u64, self.minor as u64);
        (major & 0xffff_f000) << 32
            | (major & 0xfff) << 8
            | (minor & 0xffff_ff00) << 12
            | minor & 0xff
    }
}

/// A character device's file type bits (`S_IFCHR`).
pub const S_IFCHR: u32 = 0o020000;

/// The device of the file system that holds the device files (`/dev`): Linux's devtmpfs
/// usually gets the anonymous device 0:5.
pub const DEVICE_FILES: Device = Device { major: 0, minor: 5 };

/// The console's inode number on [`DEVICE_FILES`]: neither 0, which some programs take for
/// unknown, nor 1, a file system's root.
pub const CONSOLE_INODE: u64 = 5;

/// The console's status: `/dev/console`'s on Linux.
pub const CONSOLE: Status = Status {
    device: DEVICE_FILES,
    inode: CONSOLE_INODE,
    mode: S_IFCHR | 0o600,
    links: 1,
    owner: 0,
    group: 0,
    represented_device: Device { major: 5, minor: 1 },
    size: 0,
    block_size: 4096,
    blocks: 0,
};

/// The size of the architecture's `struct stat`: x86_64's own, or the generic one AArch64 and
/// RISC-V share.
pub const STAT_SIZE: usize = if cfg!(target_arch = "x86_64") {
    144
} else {
    128
};

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
)))]
compile_error!("check Linux's struct stat for this architecture");

/// The size of a `struct statx`.
pub const STATX_SIZE: usize = 256;

fn put(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}

/// `status` as the architecture's `struct stat`, every time the epoch.
pub fn stat_bytes(status: &Status) -> [u8; STAT_SIZE] {
    let mut bytes = [0; STAT_SIZE];
    put(&mut bytes, 0, &status.device.encoded().to_ne_bytes());
    put(&mut bytes, 8, &status.inode.to_ne_bytes());
    if cfg!(target_arch = "x86_64") {
        put(&mut bytes, 16, &u64::from(status.links).to_ne_bytes());
        put(&mut bytes, 24, &status.mode.to_ne_bytes());
        put(&mut bytes, 28, &status.owner.to_ne_bytes());
        put(&mut bytes, 32, &status.group.to_ne_bytes());
        put(
            &mut bytes,
            40,
            &status.represented_device.encoded().to_ne_bytes(),
        );
        put(&mut bytes, 48, &status.size.to_ne_bytes());
        put(&mut bytes, 56, &u64::from(status.block_size).to_ne_bytes());
    } else {
        put(&mut bytes, 16, &status.mode.to_ne_bytes());
        put(&mut bytes, 20, &status.links.to_ne_bytes());
        put(&mut bytes, 24, &status.owner.to_ne_bytes());
        put(&mut bytes, 28, &status.group.to_ne_bytes());
        put(
            &mut bytes,
            32,
            &status.represented_device.encoded().to_ne_bytes(),
        );
        put(&mut bytes, 48, &status.size.to_ne_bytes());
        put(&mut bytes, 56, &status.block_size.to_ne_bytes());
    }
    put(&mut bytes, 64, &status.blocks.to_ne_bytes());
    bytes
}

/// `status` as a `struct statx` reporting [`statx_mask::BASIC_STATS`], every time the epoch.
pub fn statx_bytes(status: &Status) -> [u8; STATX_SIZE] {
    let mut bytes = [0; STATX_SIZE];
    put(&mut bytes, 0, &statx_mask::BASIC_STATS.to_ne_bytes());
    put(&mut bytes, 4, &status.block_size.to_ne_bytes());
    put(&mut bytes, 16, &status.links.to_ne_bytes());
    put(&mut bytes, 20, &status.owner.to_ne_bytes());
    put(&mut bytes, 24, &status.group.to_ne_bytes());
    put(&mut bytes, 28, &(status.mode as u16).to_ne_bytes());
    put(&mut bytes, 32, &status.inode.to_ne_bytes());
    put(&mut bytes, 40, &status.size.to_ne_bytes());
    put(&mut bytes, 48, &status.blocks.to_ne_bytes());
    put(
        &mut bytes,
        128,
        &status.represented_device.major.to_ne_bytes(),
    );
    put(
        &mut bytes,
        132,
        &status.represented_device.minor.to_ne_bytes(),
    );
    put(&mut bytes, 136, &status.device.major.to_ne_bytes());
    put(&mut bytes, 140, &status.device.minor.to_ne_bytes());
    bytes
}

fn write<P: Platform>(process: &Process<P>, address: u64, bytes: &[u8]) -> Result<u64, Errno> {
    process
        .write_user_memory(address, bytes)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// The status of descriptor `fd`: `EBADF` unless it is open.
fn descriptor_status(fd: u64) -> Result<Status, Errno> {
    if is_console(fd) {
        Ok(CONSOLE)
    } else {
        Err(Errno::EBADF)
    }
}

/// `fstat(fd, statbuf)`: writes descriptor `fd`'s `struct stat` to `statbuf`. As Linux:
/// `EBADF` for a descriptor that is not open, then `EFAULT` if `statbuf` is not the program's
/// writable memory.
pub fn fstat<P: Platform>(process: &Process<P>, fd: u64, statbuf: u64) -> Result<u64, Errno> {
    let status = descriptor_status(fd)?;
    write(process, statbuf, &stat_bytes(&status))
}

/// What a path names, as far as a lookup can tell before there are files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Empty,
    Absolute,
    Relative,
}

/// The longest path, its terminating zero included (`PATH_MAX`).
const PATH_MAX: u64 = 4096;

/// How much of a path is copied at a time.
const PATH_CHUNK: usize = 64;

/// Reads the zero-terminated path at `address`, as Linux copies one before it looks anything
/// up: `EFAULT` if a byte up to its end is not the program's memory, `ENAMETOOLONG` if it has
/// no end within [`PATH_MAX`] bytes. Chunks never cross a page, so no byte past the end is
/// read from a page that may not exist.
fn read_path<P: Platform>(process: &Process<P>, address: u64) -> Result<Path, Errno> {
    let mut chunk = [0u8; PATH_CHUNK];
    let mut first = None;
    let mut offset = 0;
    while offset < PATH_MAX {
        let at = address.checked_add(offset).ok_or(Errno::EFAULT)?;
        let length = (PATH_MAX - offset)
            .min(PATH_CHUNK as u64)
            .min(FRAME_SIZE - at % FRAME_SIZE) as usize;
        let bytes = &mut chunk[..length];
        process
            .read_user_memory(at, bytes)
            .map_err(|_| Errno::EFAULT)?;
        let first = *first.get_or_insert(bytes[0]);
        if bytes.contains(&0) {
            return Ok(match first {
                0 => Path::Empty,
                b'/' => Path::Absolute,
                _ => Path::Relative,
            });
        }
        offset += length as u64;
    }
    Err(Errno::ENAMETOOLONG)
}

/// The status `dirfd` and `path` name with `flags` (`newfstatat`'s and `statx`'s lookup): the
/// descriptor itself for an empty or null path with [`at::EMPTY_PATH`]. As Linux: the path is
/// read first ([`read_path`]; a null one with `AT_EMPTY_PATH` is not, as Linux has accepted
/// since 6.11); then `ENOENT` for an empty path without `AT_EMPTY_PATH`, and `EBADF` for a
/// descriptor that is not open, or `ENOENT` for the working directory, which does not exist
/// yet. A path that names a file finds none: `ENOENT`, or `ENOTDIR` relative to the console,
/// which is not a directory, or `EBADF` relative to a descriptor that is not open.
fn lookup<P: Platform>(
    process: &Process<P>,
    dirfd: u64,
    path: u64,
    flags: u64,
) -> Result<Status, Errno> {
    let empty_path = flags & at::EMPTY_PATH != 0;
    let path = match path {
        0 if empty_path => Path::Empty,
        _ => read_path(process, path)?,
    };
    let working_directory = dirfd as u32 as i32 == at::FDCWD;
    match path {
        Path::Empty if !empty_path => Err(Errno::ENOENT),
        Path::Absolute => Err(Errno::ENOENT),
        _ if working_directory => Err(Errno::ENOENT),
        Path::Empty => descriptor_status(dirfd),
        Path::Relative => descriptor_status(dirfd).and(Err(Errno::ENOTDIR)),
    }
}

/// `newfstatat(dirfd, path, statbuf, flags)` (`fstatat` in the generic table): as [`fstat`]
/// for `dirfd` with an empty path and [`at::EMPTY_PATH`] (see [`lookup`]). As Linux: `EINVAL`
/// for a flag other than `AT_SYMLINK_NOFOLLOW`, `AT_NO_AUTOMOUNT` and `AT_EMPTY_PATH`, first,
/// and `EFAULT` for `statbuf` last; `flags` is an `int`.
pub fn newfstatat<P: Platform>(
    process: &Process<P>,
    dirfd: u64,
    path: u64,
    statbuf: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = u64::from(flags as u32);
    if flags & !(at::SYMLINK_NOFOLLOW | at::NO_AUTOMOUNT | at::EMPTY_PATH) != 0 {
        return Err(Errno::EINVAL);
    }
    let status = lookup(process, dirfd, path, flags)?;
    write(process, statbuf, &stat_bytes(&status))
}

/// `statx(dirfd, path, flags, mask, statxbuf)`: writes the `struct statx` of what `dirfd` and
/// `path` name (see [`lookup`]) to `statxbuf`, with every basic field whatever `mask` asks, as
/// Linux may. As Linux, before the path is read: `EINVAL` for [`statx_mask::RESERVED`] in
/// `mask`, for both synchronisation bits at once, and for a flag other than `newfstatat`'s and
/// the synchronisation choice; `EFAULT` for `statxbuf` last. `flags` and `mask` are 32-bit.
/// Rust's std probes for the call with a null path and buffer and no flags, and takes only
/// `EFAULT` to mean it exists.
pub fn statx<P: Platform>(
    process: &Process<P>,
    dirfd: u64,
    path: u64,
    flags: u64,
    mask: u64,
    statxbuf: u64,
) -> Result<u64, Errno> {
    let flags = u64::from(flags as u32);
    let known = at::SYMLINK_NOFOLLOW | at::NO_AUTOMOUNT | at::EMPTY_PATH | at::STATX_SYNC_TYPE;
    if mask as u32 & statx_mask::RESERVED != 0
        || flags & at::STATX_SYNC_TYPE == at::STATX_SYNC_TYPE
        || flags & !known != 0
    {
        return Err(Errno::EINVAL);
    }
    let status = lookup(process, dirfd, path, flags)?;
    write(process, statxbuf, &statx_bytes(&status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_numbers_are_encoded_as_the_c_libraries_decode_them() {
        assert_eq!(CONSOLE.represented_device.encoded(), 0x501);
        assert_eq!(DEVICE_FILES.encoded(), 5);
        let large = Device {
            major: 0x12345,
            minor: 0x6789ab,
        };
        assert_eq!(large.encoded(), 0x0001_2006_7893_45ab);
    }
}
