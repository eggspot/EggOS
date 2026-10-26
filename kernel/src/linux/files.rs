//! Files (D102): the calls that open, read, seek and list the file system's nodes (`openat`,
//! `open` on x86_64, `read`, `readv` and `pread64` on regular files, `lseek`, `getdents64`),
//! and the working directory (`getcwd`, `chdir`, `fchdir`).
//!
//! Paths are copied from the program first, as Linux copies one before it looks anything up:
//! `EFAULT` if a byte up to the terminating NUL is not the program's memory, `ENAMETOOLONG`
//! if there is no NUL within `PATH_MAX` bytes. Then [`crate::vfs::resolve`] walks them: from
//! the root if absolute; otherwise from the working directory for `AT_FDCWD`, or from the
//! directory descriptor `dirfd` names (`EBADF` if it is not open, `ENOTDIR` if it is not a
//! directory). An absolute path ignores `dirfd`.
//!
//! The working directory belongs to the process: its threads share it ([`WorkingDirectory`],
//! held by each thread as its descriptor table is), and a thread started without
//! `CLONE_FS` gets a copy, as `fork` will.
//!
//! The only file system so far is the read-only initramfs, so opening for writing,
//! truncating or creating fails with `EROFS` as on a read-only Linux mount. A read copies
//! the file's bytes straight from the archive in the kernel image to the program's buffer.

use super::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use super::stat::at;
use super::thread::LinuxThread;
use super::{Errno, MAX_RW_COUNT};
use crate::process::Process;
use crate::sync::LeafSpinLock;
use crate::vfs::{self, NAME_MAX, Node, NodeKind, PATH_MAX};
use alloc::sync::Arc;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// A process's working directory, which the threads that share it each hold.
#[derive(Debug)]
pub struct WorkingDirectory(Arc<LeafSpinLock<Node>>);

impl WorkingDirectory {
    /// The root directory, a new process's working directory.
    pub fn at_root() -> Self {
        Self(Arc::new(LeafSpinLock::new_leaf(vfs::root())))
    }

    /// The same working directory, for a thread that shares it (`CLONE_FS`).
    pub fn share(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// A working directory of its own, starting where this one is.
    pub fn copy(&self) -> Self {
        Self(Arc::new(LeafSpinLock::new_leaf(self.get())))
    }

    /// The directory.
    #[inline]
    pub fn get(&self) -> Node {
        *self.0.lock()
    }

    /// Changes the directory to `directory`.
    pub fn set(&self, directory: Node) {
        *self.0.lock() = directory;
    }
}

/// The Linux error for a walk's error.
pub fn walk_error(error: vfs::Error) -> Errno {
    match error {
        vfs::Error::NotFound => Errno::ENOENT,
        vfs::Error::NotDirectory => Errno::ENOTDIR,
        vfs::Error::NameTooLong => Errno::ENAMETOOLONG,
    }
}

/// How many bytes of a path are copied at a time.
const PATH_CHUNK: usize = 64;

/// Copies the NUL-terminated path at `address` into `buffer` and returns it, without its NUL:
/// see the module documentation. Chunks never cross a page, so no byte past the NUL is read
/// from a page that may not exist.
pub fn read_path<'a, P: Platform>(
    process: &Process<P>,
    address: u64,
    buffer: &'a mut [u8; PATH_MAX],
) -> Result<&'a [u8], Errno> {
    let mut offset = 0;
    while offset < PATH_MAX {
        let at = address.checked_add(offset as u64).ok_or(Errno::EFAULT)?;
        let length = (PATH_MAX - offset)
            .min(PATH_CHUNK)
            .min((FRAME_SIZE - at % FRAME_SIZE) as usize);
        let chunk = &mut buffer[offset..offset + length];
        process
            .read_user_memory(at, chunk)
            .map_err(|_| Errno::EFAULT)?;
        if let Some(end) = chunk.iter().position(|&byte| byte == 0) {
            return Ok(&buffer[..offset + end]);
        }
        offset += length;
    }
    Err(Errno::ENAMETOOLONG)
}

/// The root and the directory a relative `path` starts from, given `dirfd`: see the module
/// documentation.
pub fn starting_points(
    thread: &LinuxThread,
    dirfd: u64,
    path: &[u8],
) -> Result<(Node, Node), Errno> {
    let root = vfs::root();
    if path.first() == Some(&b'/') {
        return Ok((root, root));
    }
    if dirfd as u32 as i32 == at::FDCWD {
        return Ok((root, thread.working_directory().get()));
    }
    let file = thread.files().file(dirfd)?;
    match file.kind().node() {
        Some(node) if node.is_directory() => Ok((root, node)),
        _ => Err(Errno::ENOTDIR),
    }
}

/// What `dirfd` and the program's path at `path` name, the path not empty (`ENOENT` if it
/// is).
pub fn lookup<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
) -> Result<Node, Errno> {
    let mut buffer = [0; PATH_MAX];
    let path = read_path(process, path, &mut buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (root, start) = starting_points(thread, dirfd, path)?;
    vfs::resolve(root, start, path).map_err(walk_error)
}

/// `openat(dirfd, path, flags, mode)`: opens what `dirfd` and `path` name and returns the
/// lowest free descriptor on it, close-on-exec with `O_CLOEXEC`. As Linux, in this order:
/// `EINVAL` for `O_CREAT` with `O_DIRECTORY`, and for `O_TMPFILE` without write access; the
/// path's errors (module documentation; `ENOENT` for an empty one); with `O_CREAT`, a missing
/// last component is `EROFS` if its directory exists (`EISDIR` for a path ending in `/`);
/// `EEXIST` for `O_CREAT` with `O_EXCL`; `EISDIR` for a directory with `O_CREAT` or write
/// access; `ENOTDIR` for anything else with `O_DIRECTORY`; `EROFS` for a regular file opened
/// for writing or with `O_TRUNC`, and for `O_TMPFILE`; `EMFILE` when every descriptor below
/// the open-file limit is taken. `O_PATH` opens a location only, ignoring the other flags
/// but `O_CLOEXEC`, `O_DIRECTORY` and `O_NOFOLLOW`. With no symbolic links yet, `O_NOFOLLOW`
/// changes nothing; `O_NONBLOCK` and `O_APPEND` are kept as status flags, and the other
/// flags (`O_NOCTTY`, `O_LARGEFILE`, `O_SYNC`, ...) are ignored. `mode` matters only to a
/// file created, which none is.
pub fn openat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    let only_path = flags & open_flag::PATH != 0;
    let writes = flags & open_flag::ACCMODE != open_flag::RDONLY;
    if !only_path {
        if flags & open_flag::CREAT != 0 && flags & open_flag::DIRECTORY != 0 {
            return Err(Errno::EINVAL);
        }
        if flags & open_flag::TMPFILE == open_flag::TMPFILE && !writes {
            return Err(Errno::EINVAL);
        }
    }
    let mut buffer = [0; PATH_MAX];
    let path = read_path(process, path, &mut buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (root, start) = starting_points(thread, dirfd, path)?;
    let node = match vfs::resolve(root, start, path) {
        Ok(node) => node,
        Err(vfs::Error::NotFound) if !only_path && flags & open_flag::CREAT != 0 => {
            return Err(creation_error(root, start, path));
        }
        Err(error) => return Err(walk_error(error)),
    };
    let directory = node.is_directory();
    let kind = if only_path {
        if flags & open_flag::DIRECTORY != 0 && !directory {
            return Err(Errno::ENOTDIR);
        }
        FileKind::Path(node)
    } else {
        if flags & (open_flag::CREAT | open_flag::EXCL) == open_flag::CREAT | open_flag::EXCL {
            return Err(Errno::EEXIST);
        }
        if directory && (flags & open_flag::CREAT != 0 || writes) {
            return Err(Errno::EISDIR);
        }
        if flags & open_flag::DIRECTORY != 0 && !directory {
            return Err(Errno::ENOTDIR);
        }
        if flags & open_flag::TMPFILE == open_flag::TMPFILE
            || (!directory && (writes || flags & open_flag::TRUNC != 0))
        {
            return Err(Errno::EROFS);
        }
        match directory {
            true => FileKind::Directory(node),
            false => FileKind::Regular(node),
        }
    };
    let status = match only_path {
        true => open_flag::RDONLY,
        false => flags,
    };
    let file = Arc::new(OpenFile::new(kind, status));
    let limit = open_file_limit(process);
    let close_on_exec = flags & open_flag::CLOEXEC != 0;
    let fd = thread.files().lock().open(file, close_on_exec, limit)?;
    Ok(u64::from(fd))
}

#[cold]
fn creation_error(root: Node, start: Node, path: &[u8]) -> Errno {
    let Some((directory, name)) = vfs::split_last(path) else {
        return Errno::EISDIR;
    };
    match vfs::resolve(root, start, directory) {
        Ok(_) if name.len() > NAME_MAX => Errno::ENAMETOOLONG,
        Ok(directory) if directory.is_directory() => Errno::EROFS,
        Ok(_) => Errno::ENOTDIR,
        Err(error) => walk_error(error),
    }
}

/// Copies up to `count` bytes of regular file `node` from `offset` to the program's memory at
/// `address`, a range that ends within user space, and returns how many it copied: 0 at the
/// end of the file; fewer than asked at its end or at a byte that is not the program's
/// writable memory; `EFAULT` if that is the first. Chunks never cross a page of the program's,
/// so the count is exact.
pub fn copy_out<P: Platform>(
    process: &Process<P>,
    node: Node,
    offset: u64,
    address: u64,
    count: u64,
) -> Result<u64, Errno> {
    let count = count.min(MAX_RW_COUNT);
    let mut copied = 0;
    while copied < count {
        let at = address + copied;
        let page_left = FRAME_SIZE - at % FRAME_SIZE;
        let bytes = node.read(offset + copied, (count - copied).min(page_left) as usize);
        if bytes.is_empty() {
            break;
        }
        if process.write_user_memory(at, bytes).is_err() {
            return match copied {
                0 => Err(Errno::EFAULT),
                _ => Ok(copied),
            };
        }
        copied += bytes.len() as u64;
    }
    Ok(copied)
}

/// Reads regular file `node` through `file`, its open file description, into the buffers
/// `buffers` lists (base, length), from the description's offset, which moves past what was
/// read: `read` and `readv`. Two calls on one description never read the same bytes: if
/// another moved the offset meanwhile, the read is done again from where it moved to. As
/// Linux: `EFAULT` if nothing could be copied.
pub fn read_regular<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    node: Node,
    buffers: impl Iterator<Item = (u64, u64)> + Clone,
) -> Result<u64, Errno> {
    loop {
        let offset = file.offset();
        let mut total = 0;
        for (base, length) in buffers.clone() {
            let length = length.min(MAX_RW_COUNT - total);
            let copied = match copy_out(process, node, offset + total, base, length) {
                Ok(copied) => copied,
                Err(_) if total > 0 => break,
                Err(error) => return Err(error),
            };
            total += copied;
            if copied < length {
                break;
            }
        }
        if file.move_offset(offset, offset + total) {
            return Ok(total);
        }
    }
}

/// `pread64(fd, buffer, count, offset)` on what `file` (descriptor `fd`'s) names: reads from
/// `offset` without moving the description's. As Linux, in this order: `EINVAL` for a
/// negative offset, `EBADF` unless open for reading, `ESPIPE` for the console, `EISDIR` for a
/// directory, `EFAULT` if the buffer does not end within user space or nothing could be
/// copied.
pub fn pread64<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    buffer: u64,
    count: u64,
    offset: u64,
) -> Result<u64, Errno> {
    if (offset as i64) < 0 {
        return Err(Errno::EINVAL);
    }
    if !file.is_readable() || matches!(file.kind(), FileKind::Path(_)) {
        return Err(Errno::EBADF);
    }
    let node = match file.kind() {
        FileKind::Console => return Err(Errno::ESPIPE),
        FileKind::Directory(_) => return Err(Errno::EISDIR),
        FileKind::Regular(node) => *node,
        FileKind::Path(_) => return Err(Errno::EBADF),
    };
    if !super::ends_in_user_space::<P>(buffer, count) {
        return Err(Errno::EFAULT);
    }
    copy_out(process, node, offset, buffer, count)
}

/// `lseek`'s `whence` values (`SEEK_*`).
pub mod whence {
    /// From the start.
    pub const SET: u32 = 0;
    /// From the current offset.
    pub const CURRENT: u32 = 1;
    /// From the end.
    pub const END: u32 = 2;
    /// To the next data at or after the offset.
    pub const DATA: u32 = 3;
    /// To the next hole at or after the offset: the end, for a file without holes.
    pub const HOLE: u32 = 4;
}

/// `lseek(fd, offset, whence)` on `file`, which names a node: moves its offset and returns
/// it. As Linux: `EINVAL` for an unknown `whence` or a negative result; for a directory only
/// `SEEK_SET` and `SEEK_CUR` (positions `getdents64` gave), `EINVAL` otherwise; `ENXIO` for
/// `SEEK_DATA` or `SEEK_HOLE` at or past the end; `EBADF` for an `O_PATH` descriptor.
pub fn lseek_node(file: &OpenFile, offset: u64, whence: u64) -> Result<u64, Errno> {
    let (node, directory) = match file.kind() {
        FileKind::Regular(node) => (*node, false),
        FileKind::Directory(node) => (*node, true),
        FileKind::Path(_) | FileKind::Console => return Err(Errno::EBADF),
    };
    let offset = offset as i64;
    let size = node.attributes().size as i64;
    let current = file.offset() as i64;
    let target = match whence as u32 {
        whence::SET => Some(offset),
        whence::CURRENT => current.checked_add(offset),
        whence::END if !directory => size.checked_add(offset),
        whence::DATA | whence::HOLE if !directory => {
            if offset < 0 {
                return Err(Errno::EINVAL);
            }
            if offset >= size {
                return Err(Errno::ENXIO);
            }
            Some(match whence as u32 {
                whence::DATA => offset,
                _ => size,
            })
        }
        _ => return Err(Errno::EINVAL),
    };
    match target {
        Some(target) if target >= 0 => {
            file.set_offset(target as u64);
            Ok(target as u64)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// A directory entry's type (`d_type`): a directory.
pub const DT_DIR: u8 = 4;
/// A directory entry's type (`d_type`): a regular file.
pub const DT_REG: u8 = 8;

/// The size of a `struct linux_dirent64` before its name: inode, next offset, record length
/// and type.
pub const DIRENT_HEADER_SIZE: usize = 19;

/// Writes one `struct linux_dirent64` to the start of `buffer` (inode `inode`, `next` the
/// offset of the entry after it, type `kind`, `name` with its NUL, padded to 8 bytes) and
/// returns its length; `None` if it does not fit.
pub fn pack_dirent(
    buffer: &mut [u8],
    inode: u64,
    next: u64,
    kind: u8,
    name: &[u8],
) -> Option<usize> {
    let length = dirent_length(name);
    let record = buffer.get_mut(..length)?;
    record[0..8].copy_from_slice(&inode.to_ne_bytes());
    record[8..16].copy_from_slice(&next.to_ne_bytes());
    record[16..18].copy_from_slice(&(length as u16).to_ne_bytes());
    record[18] = kind;
    record[DIRENT_HEADER_SIZE..DIRENT_HEADER_SIZE + name.len()].copy_from_slice(name);
    record[DIRENT_HEADER_SIZE + name.len()..].fill(0);
    Some(length)
}

/// How many bytes of entries `getdents64` gathers on the kernel stack before it copies them
/// out: room for at least one entry with the longest name.
const DIRENT_CHUNK: usize = 512;

/// The entry at `position` of `directory`: `.` at 0, `..` at 1, then its own entries.
fn directory_entry(directory: Node, position: u64) -> Option<(u64, u8, &'static [u8])> {
    match position {
        0 => Some((directory.attributes().file_id, DT_DIR, b".")),
        1 => Some((directory.parent().attributes().file_id, DT_DIR, b"..")),
        _ => {
            let (name, node, kind) = directory.entry(position - 2)?;
            let kind = match kind {
                NodeKind::Directory => DT_DIR,
                NodeKind::Regular => DT_REG,
            };
            Some((node.attributes().file_id, kind, name))
        }
    }
}

/// `getdents64(fd, entries, count)` on `file`, descriptor `fd`'s: writes as many of the
/// directory's entries from the description's offset (a position: `.` is 0, `..` 1) as fit
/// in `count` bytes at `entries`, moves the offset past them and returns how many bytes it
/// wrote: 0 at the end. As Linux: `EBADF` for an `O_PATH` descriptor, `ENOTDIR` for anything
/// but a directory, `EFAULT` if the buffer does not end within user space, `EINVAL` if the
/// next entry does not fit, `EFAULT` if nothing could be copied. `count` is an `unsigned
/// int`.
pub fn getdents64<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    entries: u64,
    count: u64,
) -> Result<u64, Errno> {
    let directory = match file.kind() {
        FileKind::Directory(node) => *node,
        FileKind::Path(_) => return Err(Errno::EBADF),
        _ => return Err(Errno::ENOTDIR),
    };
    let count = u64::from(count as u32);
    if !super::ends_in_user_space::<P>(entries, count) {
        return Err(Errno::EFAULT);
    }
    let mut chunk = [0u8; DIRENT_CHUNK];
    let start = file.offset();
    let mut position = start;
    let mut chunk_start = start;
    let mut written = 0u64;
    let mut gathered = 0;
    let mut failed = false;
    while let Some((inode, kind, name)) = directory_entry(directory, position) {
        let length = dirent_length(name);
        if written + (gathered + length) as u64 > count {
            break;
        }
        if gathered + length > DIRENT_CHUNK {
            if process
                .write_user_memory(entries + written, &chunk[..gathered])
                .is_err()
            {
                failed = true;
                break;
            }
            written += gathered as u64;
            gathered = 0;
            chunk_start = position;
        }
        gathered += pack_dirent(&mut chunk[gathered..], inode, position + 1, kind, name)
            .expect("a chunk holds the longest entry");
        position += 1;
    }
    if !failed && gathered > 0 {
        match process.write_user_memory(entries + written, &chunk[..gathered]) {
            Ok(()) => written += gathered as u64,
            Err(_) => failed = true,
        }
    }
    if failed {
        position = chunk_start;
    }
    if written == 0 {
        if failed {
            return Err(Errno::EFAULT);
        }
        if directory_entry(directory, position).is_some() {
            return Err(Errno::EINVAL);
        }
    }
    file.set_offset(position);
    Ok(written)
}

fn dirent_length(name: &[u8]) -> usize {
    (DIRENT_HEADER_SIZE + name.len() + 1).next_multiple_of(8)
}

/// `getcwd(buffer, size)`: writes the working directory's absolute path with its NUL to
/// `buffer` and returns its length, NUL included. As Linux: `ERANGE` if `size` bytes are
/// too few, `EFAULT` if the buffer is not the program's writable memory.
pub fn getcwd<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    buffer: u64,
    size: u64,
) -> Result<u64, Errno> {
    let mut path = [0u8; PATH_MAX];
    let directory = thread.working_directory().get();
    let start = directory
        .path_into(&mut path[..PATH_MAX - 1])
        .ok_or(Errno::ENAMETOOLONG)?;
    let length = PATH_MAX - start;
    if length as u64 > size {
        return Err(Errno::ERANGE);
    }
    process
        .write_user_memory(buffer, &path[start..])
        .map_err(|_| Errno::EFAULT)?;
    Ok(length as u64)
}

/// `chdir(path)`: makes what `path` names the working directory. As Linux: the path's errors
/// (module documentation), then `ENOTDIR` if it is not a directory.
pub fn chdir<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    path: u64,
) -> Result<u64, Errno> {
    let node = lookup(process, thread, at::FDCWD as u64, path)?;
    change_directory(thread, node)
}

/// `fchdir(fd)`: makes the directory descriptor `fd` names the working directory. As Linux:
/// `EBADF` if `fd` is not open, `ENOTDIR` if it is not a directory.
pub fn fchdir(thread: &LinuxThread, fd: u64) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    let node = file.kind().node().ok_or(Errno::ENOTDIR)?;
    change_directory(thread, node)
}

fn change_directory(thread: &LinuxThread, node: Node) -> Result<u64, Errno> {
    if !node.is_directory() {
        return Err(Errno::ENOTDIR);
    }
    thread.working_directory().set(node);
    Ok(0)
}
