//! File status (M6, D74, D102): `fstat`, `newfstatat` (`fstatat` in the generic table) and
//! `statx` on descriptors and paths. glibc's stdio asks `fstat` whether a stream is a
//! character device before it line-buffers a terminal, and Rust's std asks `statx` first for
//! a file's metadata, falling back to `fstatat`.
//!
//! The console answers as Linux's `/dev/console` does for `init`: a character device, major 5
//! minor 1, owned by root with mode `0600`, empty, with the page size as its block size, on a
//! device file system ([`CONSOLE`]). Its times are the Unix epoch, when the wall clock starts
//! (D73). A file system's node answers with its own attributes ([`node_status`]).
//!
//! `newfstatat` and `statx` look up what `dirfd` and `path` name as `openat` does
//! ([`super::files`]). Their synchronisation flags change nothing: there is no remote file
//! system to ask, and with no symbolic links yet `AT_SYMLINK_NOFOLLOW` changes nothing either.
//! A null path with `AT_EMPTY_PATH` is not read, as Linux has accepted since 6.11. An empty
//! path is `ENOENT` without `AT_EMPTY_PATH`, and with it names `dirfd` itself: `EBADF` for a
//! descriptor that is not open, the working directory for `AT_FDCWD`.

use super::Errno;
use super::descriptors::{Descriptors, FileKind};
use super::files;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::vfs::{self, Node, NodeKind};
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
    /// The device that holds the file (`st_dev`).
    pub device: Device,
    /// Its inode number on that device (`st_ino`).
    pub inode: u64,
    /// Its type and permission bits (`st_mode`).
    pub mode: u32,
    /// How many hard links it has (`st_nlink`).
    pub links: u32,
    /// The user that owns it (`st_uid`).
    pub owner: u32,
    /// Its group (`st_gid`).
    pub group: u32,
    /// The device a device file represents (`st_rdev`).
    pub represented_device: Device,
    /// Its size in bytes (`st_size`).
    pub size: u64,
    /// The block size for efficient input and output (`st_blksize`).
    pub block_size: u32,
    /// How many 512-byte blocks it takes (`st_blocks`).
    pub blocks: u64,
    /// When its contents last changed, in seconds since the Unix epoch (`st_mtime`), also
    /// reported as its last access and status change.
    pub modified: u64,
}

/// A device number: its major and minor parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    /// The driver's number.
    pub major: u32,
    /// The device's number among the driver's.
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

/// A pipe's file type bits (`S_IFIFO`).
pub const S_IFIFO: u32 = 0o010000;

/// The device Linux's pipes report (`st_dev`): the anonymous device of its pipe file system.
pub const PIPES: Device = Device {
    major: 0,
    minor: 12,
};

/// A directory's file type bits (`S_IFDIR`).
pub const S_IFDIR: u32 = 0o040000;

/// A regular file's file type bits (`S_IFREG`).
pub const S_IFREG: u32 = 0o100000;

/// A socket's file type bits (`S_IFSOCK`).
pub const S_IFSOCK: u32 = 0o140000;

/// The device Linux's sockets report (`st_dev`): the anonymous device of its socket file system.
pub const SOCKETS: Device = Device { major: 0, minor: 8 };

/// The device memory files report (`st_dev`): the anonymous device of Linux's internal tmpfs.
pub const MEMORY_FILES: Device = Device { major: 0, minor: 1 };

/// The status of `node`: its attributes, on its file system's device, in blocks of the page
/// size as a memory file system reports them.
pub fn node_status(node: Node) -> Status {
    let attributes = node.attributes();
    let (major, minor) = node.file_system().device();
    let file_type = match attributes.kind {
        NodeKind::Directory => S_IFDIR,
        NodeKind::Regular => S_IFREG,
        NodeKind::CharacterDevice => S_IFCHR,
        NodeKind::Socket => S_IFSOCK,
    };
    let (represented_major, represented_minor) = node.character_device().unwrap_or((0, 0));
    Status {
        device: Device { major, minor },
        inode: attributes.file_id,
        mode: file_type | attributes.permissions,
        links: attributes.links,
        owner: attributes.owner,
        group: attributes.group,
        represented_device: Device {
            major: represented_major,
            minor: represented_minor,
        },
        size: attributes.size,
        block_size: FRAME_SIZE as u32,
        blocks: attributes.size.div_ceil(FRAME_SIZE) * (FRAME_SIZE / 512),
        modified: attributes.modified,
    }
}

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
    block_size: FRAME_SIZE as u32,
    blocks: 0,
    modified: 0,
};

/// `/dev/window`'s status (D127): a character device on [`DEVICE_FILES`].
pub const WINDOW: Status = Status {
    device: DEVICE_FILES,
    inode: crate::vfs::devfs::WINDOW,
    mode: S_IFCHR | 0o666,
    represented_device: Device {
        major: crate::vfs::devfs::WINDOW_DEVICE.0,
        minor: crate::vfs::devfs::WINDOW_DEVICE.1,
    },
    ..CONSOLE
};

/// `/dev/agent`'s status (D136): a character device on [`DEVICE_FILES`].
pub const AGENT: Status = Status {
    inode: crate::vfs::devfs::AGENT,
    mode: S_IFCHR | 0o600,
    represented_device: Device {
        major: crate::vfs::devfs::AGENT_DEVICE.0,
        minor: crate::vfs::devfs::AGENT_DEVICE.1,
    },
    ..WINDOW
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

const STAT_TIMES: [usize; 3] = [72, 104, 88];
const STATX_TIMES: [usize; 3] = [64, 96, 112];

/// `status` as the architecture's `struct stat`, every time its modification time.
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
    for offset in STAT_TIMES {
        put(&mut bytes, offset, &status.modified.to_ne_bytes());
    }
    bytes
}

/// `status` as a `struct statx` reporting [`statx_mask::BASIC_STATS`], every time its
/// modification time.
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
    for offset in STATX_TIMES {
        put(&mut bytes, offset, &status.modified.to_ne_bytes());
    }
    bytes
}

fn write<P: Platform>(process: &Process<P>, address: u64, bytes: &[u8]) -> Result<u64, Errno> {
    process
        .write_user_memory(address, bytes)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

fn descriptor_status(files: &Descriptors, fd: u64) -> Result<Status, Errno> {
    match files.file(fd)?.kind() {
        FileKind::Console => Ok(CONSOLE),
        FileKind::Pipe(end) => Ok(Status {
            device: PIPES,
            inode: end.inode(),
            mode: S_IFIFO | 0o600,
            links: 1,
            represented_device: Device { major: 0, minor: 0 },
            size: 0,
            blocks: 0,
            ..CONSOLE
        }),
        FileKind::Regular(node) | FileKind::Directory(node) | FileKind::Path(node) => {
            Ok(node_status(*node))
        }
        FileKind::Snapshot(snapshot) => Ok(node_status(snapshot.node)),
        FileKind::Window(_) => Ok(WINDOW),
        FileKind::AgentChannel(_) => Ok(AGENT),
        FileKind::MemoryFile(memory) => {
            let size = memory.size();
            Ok(Status {
                device: MEMORY_FILES,
                inode: memory.inode(),
                mode: S_IFREG | 0o777,
                links: 1,
                represented_device: Device { major: 0, minor: 0 },
                size,
                blocks: memory.frames_held() as u64 * (FRAME_SIZE / 512),
                ..CONSOLE
            })
        }
        FileKind::Socket(socket) => Ok(Status {
            device: SOCKETS,
            inode: socket.inode(),
            mode: S_IFSOCK | 0o777,
            links: 1,
            represented_device: Device { major: 0, minor: 0 },
            size: 0,
            blocks: 0,
            ..CONSOLE
        }),
    }
}

/// `fstat(fd, statbuf)`: writes descriptor `fd`'s `struct stat` to `statbuf`. As Linux:
/// `EBADF` for a descriptor that is not open, then `EFAULT` if `statbuf` is not the program's
/// writable memory.
pub fn fstat<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    fd: u64,
    statbuf: u64,
) -> Result<u64, Errno> {
    let status = descriptor_status(files, fd)?;
    write(process, statbuf, &stat_bytes(&status))
}

/// The status `dirfd` and `path` name with `flags`: the module documentation's lookup.
fn lookup<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    flags: u64,
) -> Result<Status, Errno> {
    let empty_path = flags & at::EMPTY_PATH != 0;
    if path == 0 && empty_path {
        return named_descriptor_status(thread, dirfd);
    }
    let mut buffer = files::PathBuffer::uninit();
    let path = files::read_path(process, path, &mut buffer)?;
    if path.is_empty() {
        return match empty_path {
            true => named_descriptor_status(thread, dirfd),
            false => Err(Errno::ENOENT),
        };
    }
    let (root, start) = files::starting_points(thread, dirfd, path)?;
    let node = vfs::resolve(root, start, path).map_err(files::walk_error)?;
    Ok(node_status(node))
}

fn named_descriptor_status(thread: &LinuxThread, dirfd: u64) -> Result<Status, Errno> {
    match dirfd as u32 as i32 == at::FDCWD {
        true => Ok(node_status(thread.working_directory().get())),
        false => descriptor_status(thread.files(), dirfd),
    }
}

/// `newfstatat(dirfd, path, statbuf, flags)` (`fstatat` in the generic table): as [`fstat`]
/// for `dirfd` with an empty path and [`at::EMPTY_PATH`] (the module documentation's lookup).
/// As Linux: `EINVAL` for a flag other than `AT_SYMLINK_NOFOLLOW`, `AT_NO_AUTOMOUNT`,
/// `AT_EMPTY_PATH` and `statx`'s synchronisation bits (either or both, which Linux has taken
/// since 4.11 though `fstatat(2)` does not list them), first, and `EFAULT` for `statbuf` last;
/// `flags` is an `int`.
pub fn newfstatat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    statbuf: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = u64::from(flags as u32);
    let known = at::SYMLINK_NOFOLLOW | at::NO_AUTOMOUNT | at::EMPTY_PATH | at::STATX_SYNC_TYPE;
    if flags & !known != 0 {
        return Err(Errno::EINVAL);
    }
    let status = lookup(process, thread, dirfd, path, flags)?;
    write(process, statbuf, &stat_bytes(&status))
}

/// `statx(dirfd, path, flags, mask, statxbuf)`: writes the `struct statx` of what `dirfd` and
/// `path` name (the module documentation's lookup) to `statxbuf`, with every basic field
/// whatever `mask` asks, as Linux may. As Linux, before the path is read: `EINVAL` for
/// [`statx_mask::RESERVED`] in `mask`, for both synchronisation bits at once, and for a flag
/// other than `newfstatat`'s and the synchronisation choice; `EFAULT` for `statxbuf` last.
/// `flags` and `mask` are 32-bit. Rust's std probes for the call with a null path and buffer
/// and no flags, and takes only `EFAULT` to mean it exists.
pub fn statx<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
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
    let status = lookup(process, thread, dirfd, path, flags)?;
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
