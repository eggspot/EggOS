//! The calls that change files and directories (D125), on file systems that can be changed
//! (the tmpfs on `/tmp` and `/home`; the initramfs answers `EROFS`): `write`, `writev` and
//! `pwrite64` on regular files, `ftruncate`, `mkdirat`, `unlinkat`, `renameat`, `renameat2`,
//! `utimensat`, `symlinkat`, `linkat`, `fchmodat`, `fchmodat2` and `fchmod` (D170), with
//! x86_64's `mkdir`, `rmdir`, `unlink`, `rename`, `symlink`, `link` and `chmod`, and `sync`,
//! `syncfs`, `fsync` and `fdatasync`, which put changes on the disk (D131); `openat` creates and
//! truncates files through [`create_file`] and [`truncate_on_open`]. Errors are Linux's, in
//! Linux's order where it has one. User memory is copied through a buffer on the kernel stack,
//! never while a file system's lock is held. Times are the wall clock's seconds. Each change is
//! reported to the `inotify` watches of what it changed ([`inotify`], D179).

use super::descriptors::{FileKind, OpenFile, open_flag};
use super::files::{PathBuffer, read_path, starting_points, walk_error};
use super::inotify::{self, mask as watched};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec};
use crate::process::Process;
use crate::vfs::{self, NAME_MAX, Node, NodeKind, Refused, WriteAt};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;

/// `unlinkat`'s flag that removes a directory instead (`AT_REMOVEDIR`).
pub const AT_REMOVEDIR: u64 = 0x200;

/// `renameat2`'s flag that refuses to replace an existing name (`RENAME_NOREPLACE`).
pub const RENAME_NOREPLACE: u64 = 1;

/// `linkat`'s flag that follows a symbolic link named as the existing file (`AT_SYMLINK_FOLLOW`).
pub const AT_SYMLINK_FOLLOW: u64 = 0x400;

pub(super) const UMASK: u32 = 0o022;

const UTIME_NOW: i64 = (1 << 30) - 1;
const UTIME_OMIT: i64 = (1 << 30) - 2;

/// The wall clock's seconds ([`crate::wall_clock`], D150).
pub fn now<P: Platform>() -> u64 {
    crate::wall_clock::seconds::<P>()
}

/// The Linux error for a file system's refusal.
pub fn refused(refused: Refused) -> Errno {
    match refused {
        Refused::ReadOnly => Errno::EROFS,
        Refused::Exists => Errno::EEXIST,
        Refused::NotFound => Errno::ENOENT,
        Refused::NotDirectory => Errno::ENOTDIR,
        Refused::IsDirectory => Errno::EISDIR,
        Refused::NotEmpty => Errno::ENOTEMPTY,
        Refused::NoSpace => Errno::ENOSPC,
        Refused::NameTooLong => Errno::ENAMETOOLONG,
        Refused::InvalidArgument => Errno::EINVAL,
        Refused::TooBig => Errno::EFBIG,
        Refused::Busy => Errno::EBUSY,
        Refused::Storage => Errno::EIO,
        Refused::TooManyLinks => Errno::EMLINK,
        Refused::NotPermitted => Errno::EPERM,
    }
}

pub(super) fn parent_and_name(
    root: Node,
    start: Node,
    path: &[u8],
    dots_are_names: bool,
    caller: u32,
) -> Result<(Node, &[u8]), Errno> {
    parent_and_name_walking(root, start, path, dots_are_names, caller, true)
}

/// [`parent_and_name`] whose walk to the directory follows no symbolic link unless
/// `links_allowed` (`openat2`'s `RESOLVE_NO_SYMLINKS`: `ELOOP`).
pub(super) fn parent_and_name_walking(
    root: Node,
    start: Node,
    path: &[u8],
    dots_are_names: bool,
    caller: u32,
    links_allowed: bool,
) -> Result<(Node, &[u8]), Errno> {
    let end = path
        .iter()
        .rposition(|&byte| byte != b'/')
        .map_or(0, |at| at + 1);
    let trimmed = &path[..end];
    let split = vfs::split_last(trimmed).or_else(|| {
        let name_start = trimmed
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(0, |at| at + 1);
        let name = &trimmed[name_start..];
        (dots_are_names && matches!(name, b"." | b"..")).then(|| match name_start {
            0 => (&b"."[..], name),
            _ => (&trimmed[..name_start], name),
        })
    });
    let Some((directory, name)) = split else {
        return Err(match end {
            0 if path.is_empty() => Errno::ENOENT,
            0 => Errno::EBUSY,
            _ => Errno::ENOENT,
        });
    };
    let directory = match links_allowed {
        true => vfs::resolve_as(root, start, directory, caller, true),
        false => vfs::resolve_without_links(root, start, directory, caller, true),
    }
    .map_err(walk_error)?;
    if !directory.is_directory() {
        return Err(Errno::ENOTDIR);
    }
    if name.len() > NAME_MAX {
        return Err(Errno::ENAMETOOLONG);
    }
    Ok((directory, name))
}

fn parent_and_name_at<'p, P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    buffer: &'p mut PathBuffer,
    dots_are_names: bool,
) -> Result<(Node, &'p [u8]), Errno> {
    let path = read_path(process, path, buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (root, start) = starting_points(thread, dirfd, path)?;
    parent_and_name(root, start, path, dots_are_names, process.id().get())
}

/// Creates regular file `path` (from `start`, or the root) for `openat` with `O_CREAT`, which
/// found nothing there, walking as process `caller` and through no symbolic link unless
/// `links_allowed`: the new file's node. `EROFS` on a file system that cannot be changed,
/// `EISDIR` for a path ending in `/`.
pub fn create_file<P: Platform>(
    root: Node,
    start: Node,
    path: &[u8],
    mode: u64,
    caller: u32,
    links_allowed: bool,
) -> Result<Node, Errno> {
    let (directory, name) =
        parent_and_name_walking(root, start, path, false, caller, links_allowed)?;
    if path.ends_with(b"/") {
        return Err(Errno::EISDIR);
    }
    let file_system = directory.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    let permissions = mode as u32 & 0o7777 & !UMASK;
    let id = file_system
        .create(
            directory.id(),
            name,
            NodeKind::Regular,
            permissions,
            now::<P>(),
        )
        .map_err(refused)?;
    inotify::created(directory, name, false);
    vfs::resolve_as(root, directory, name, caller, false)
        .map_err(walk_error)
        .and_then(|node| match node.id() == id {
            true => Ok(node),
            false => Err(Errno::ENOENT),
        })
}

/// What `openat` does to existing regular file `node` opened with `flags` for writing or with
/// `O_TRUNC`: `EROFS` unless it can be written to; with `O_TRUNC` and write access,
/// empties it.
pub fn truncate_on_open<P: Platform>(node: Node, flags: u32) -> Result<(), Errno> {
    let file_system = node.file_system();
    if !file_system.is_node_writable(node.id()) {
        return Err(Errno::EROFS);
    }
    let writes = flags & open_flag::ACCMODE != open_flag::RDONLY;
    if writes && flags & open_flag::TRUNC != 0 {
        file_system
            .truncate(node.id(), 0, now::<P>())
            .map_err(refused)?;
        vfs::shared_pages::truncated(node, 0);
        inotify::changed(node, watched::MODIFY);
    }
    Ok(())
}

/// `write` or `writev` of `buffers` to regular file `node` through `file`, at its offset or,
/// with `O_APPEND`, at the file's end, moving the offset past what was written; `pwrite64`
/// with `at`, leaving the offset. As Linux: `EFAULT` if nothing could be copied, `ENOSPC`
/// when memory ran out before the first byte. A write of at least one byte is `IN_MODIFY` to
/// the file's watches ([`inotify`]).
pub(super) fn write_regular<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    node: Node,
    buffers: &Iovecs,
    at: Option<u64>,
) -> Result<u64, Errno> {
    let written = write_into(process, file, node, buffers, at);
    if matches!(written, Ok(count) if count > 0) {
        inotify::changed(node, watched::MODIFY);
    }
    written
}

fn write_into<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    node: Node,
    buffers: &Iovecs,
    at: Option<u64>,
) -> Result<u64, Errno> {
    let file_system = node.file_system();
    let append = file.status_flags() & open_flag::APPEND != 0;
    let mut chunk = [0u8; FRAME_SIZE as usize];
    let mut offset = at.unwrap_or_else(|| file.offset());
    let mut written = 0u64;
    let partial = |written: u64, error: Errno| match written {
        0 => Err(error),
        _ => Ok(written),
    };
    let now = now::<P>();
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(MAX_RW_COUNT - written);
        let mut done = 0;
        while done < length {
            let size = (length - done).min(FRAME_SIZE - (base + done) % FRAME_SIZE) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                return partial(written, Errno::EFAULT);
            }
            let to = match (append, at) {
                (true, None) => WriteAt::End,
                _ => WriteAt::Offset(offset),
            };
            let (count, reached) = match file_system.write(node.id(), to, &chunk[..size], now) {
                Ok(done) => done,
                Err(error) => return partial(written, refused(error)),
            };
            vfs::shared_pages::wrote(node, reached - count as u64, &chunk[..count]);
            offset = reached;
            if at.is_none() {
                file.set_offset(reached);
            }
            written += count as u64;
            done += count as u64;
            if count < size {
                return Ok(written);
            }
        }
    }
    Ok(written)
}

/// `pwrite64(fd, buffer, count, offset)`: writes at `offset` without moving the description's
/// offset. As Linux: `EINVAL` for a negative offset, `EBADF` unless `fd` is open for writing,
/// `ESPIPE` for the console and pipes, `EFAULT` for a buffer outside user space.
pub fn pwrite64<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    buffer: u64,
    count: u64,
    offset: u64,
) -> Result<u64, Errno> {
    if (offset as i64) < 0 {
        return Err(Errno::EINVAL);
    }
    let file = thread.files().file(fd)?;
    if !file.is_writable() {
        return Err(Errno::EBADF);
    }
    let node = match file.kind() {
        FileKind::Console
        | FileKind::PseudoTerminal(_)
        | FileKind::Pipe(_)
        | FileKind::Socket(_)
        | FileKind::InetSocket(_)
        | FileKind::Event(_) => {
            return Err(Errno::ESPIPE);
        }
        FileKind::Regular(node) => *node,
        FileKind::MemoryFile(memory) => {
            if !super::ends_in_user_space::<P>(buffer, count) {
                return Err(Errno::EFAULT);
            }
            let buffers = Iovecs::one(buffer, count);
            return super::memfd::write(process, &file, memory, &buffers, Some(offset));
        }
        FileKind::MemoryDevice(device, _) => {
            if !super::ends_in_user_space::<P>(buffer, count) {
                return Err(Errno::EFAULT);
            }
            let buffers = core::iter::once((buffer, count));
            return super::memory_devices::write(process, *device, buffers);
        }
        _ => return Err(Errno::EBADF),
    };
    if !super::ends_in_user_space::<P>(buffer, count) {
        return Err(Errno::EFAULT);
    }
    write_regular(
        process,
        &file,
        node,
        &Iovecs::one(buffer, count),
        Some(offset),
    )
}

/// `ftruncate(fd, length)`: makes the regular file `fd` names `length` bytes long. As Linux:
/// `EINVAL` for a negative length, `EBADF` for a descriptor not open, `EINVAL` for one not open
/// for writing or not naming a regular file, `EROFS` on a file system that cannot be changed.
pub fn ftruncate<P: Platform>(thread: &LinuxThread, fd: u64, length: u64) -> Result<u64, Errno> {
    if (length as i64) < 0 {
        return Err(Errno::EINVAL);
    }
    let file = thread.files().file(fd)?;
    if let FileKind::MemoryFile(memory) = file.kind() {
        if !file.is_writable() {
            return Err(Errno::EINVAL);
        }
        return memory.truncate(length).map(|()| 0);
    }
    let FileKind::Regular(node) = file.kind() else {
        return Err(Errno::EINVAL);
    };
    if !file.is_writable() {
        return Err(Errno::EINVAL);
    }
    node.file_system()
        .truncate(node.id(), length, now::<P>())
        .map_err(refused)?;
    vfs::shared_pages::truncated(*node, length);
    inotify::changed(*node, watched::MODIFY);
    Ok(0)
}

/// `mkdirat(dirfd, path, mode)`: makes a directory. As Linux: the path's errors, `EEXIST` if
/// the name is taken, `EROFS` on a file system that cannot be changed, `ENOSPC` without memory.
pub fn mkdirat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    mode: u64,
) -> Result<u64, Errno> {
    let mut buffer = PathBuffer::uninit();
    let (directory, name) =
        match parent_and_name_at(process, thread, dirfd, path, &mut buffer, true) {
            Ok(found) => found,
            Err(Errno::EBUSY) => return Err(Errno::EEXIST),
            Err(error) => return Err(error),
        };
    if matches!(name, b"." | b"..") || directory.lookup(name).is_ok() {
        return Err(Errno::EEXIST);
    }
    let file_system = directory.file_system();
    let permissions = mode as u32 & 0o7777 & !UMASK;
    file_system
        .create(
            directory.id(),
            name,
            NodeKind::Directory,
            permissions,
            now::<P>(),
        )
        .map_err(refused)?;
    inotify::created(directory, name, true);
    Ok(0)
}

/// `unlinkat(dirfd, path, flags)`: removes a file, or with `AT_REMOVEDIR` an empty directory.
/// As Linux: `EINVAL` for another flag, the path's errors, `EISDIR` for a directory without
/// `AT_REMOVEDIR`, `ENOTDIR` for a file with it, `EINVAL` for a last component `.` and
/// `ENOTEMPTY` for `..` with it, `ENOTEMPTY` for a directory with entries, `EBUSY` for a mount
/// point, `EROFS` on a file system that cannot be changed.
pub fn unlinkat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags & !AT_REMOVEDIR != 0 {
        return Err(Errno::EINVAL);
    }
    let directories = flags & AT_REMOVEDIR != 0;
    let mut buffer = PathBuffer::uninit();
    let (directory, name) = parent_and_name_at(process, thread, dirfd, path, &mut buffer, true)?;
    match name {
        b"." if directories => return Err(Errno::EINVAL),
        b".." if directories => return Err(Errno::ENOTEMPTY),
        b"." | b".." => return Err(Errno::EISDIR),
        _ => {}
    }
    let target = directory.lookup(name).map_err(walk_error)?;
    if target.file_system().root() == target.id()
        && !core::ptr::addr_eq(target.file_system(), directory.file_system())
    {
        return Err(Errno::EBUSY);
    }
    let watching = inotify::watching();
    let last_name = watching && target.attributes().links <= 1;
    directory
        .file_system()
        .remove(directory.id(), name, directories, now::<P>())
        .map_err(|error| match (error, directories) {
            (Refused::ReadOnly, _) => Errno::EROFS,
            (Refused::IsDirectory, false) => Errno::EISDIR,
            (error, _) => refused(error),
        })?;
    if watching {
        inotify::removed(directory, name, target, last_name);
    }
    Ok(0)
}

/// `renameat2(olddirfd, oldpath, newdirfd, newpath, flags)`: moves a name, replacing what the
/// new one names unless `RENAME_NOREPLACE`. As Linux: `EINVAL` for another flag, the paths'
/// errors, `EXDEV` across file systems, `EINVAL` for a directory moved into itself, `EEXIST`
/// with `RENAME_NOREPLACE`, `EISDIR`, `ENOTDIR` and `ENOTEMPTY` for what cannot replace what,
/// `EBUSY` for `.` and `..`, `EROFS` on a file system that cannot be changed. `renameat` is this
/// with no flags.
pub fn renameat2<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    old_dirfd: u64,
    old_path: u64,
    new_dirfd: u64,
    new_path: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags & !RENAME_NOREPLACE != 0 {
        return Err(Errno::EINVAL);
    }
    let mut old_buffer = PathBuffer::uninit();
    let mut new_buffer = PathBuffer::uninit();
    let (from, from_name) =
        parent_and_name_at(process, thread, old_dirfd, old_path, &mut old_buffer, true)?;
    let (to, to_name) =
        parent_and_name_at(process, thread, new_dirfd, new_path, &mut new_buffer, true)?;
    if [from_name, to_name]
        .iter()
        .any(|name| matches!(*name, b"." | b".."))
    {
        return Err(Errno::EBUSY);
    }
    let moved = from.lookup(from_name).map_err(walk_error)?;
    let file_system = from.file_system();
    if !core::ptr::addr_eq(file_system, to.file_system())
        || !core::ptr::addr_eq(file_system, moved.file_system())
    {
        return Err(Errno::EXDEV);
    }
    file_system
        .rename(
            from.id(),
            from_name,
            to.id(),
            to_name,
            flags & RENAME_NOREPLACE != 0,
            now::<P>(),
        )
        .map_err(refused)?;
    inotify::moved(from, from_name, to, to_name, moved);
    Ok(0)
}

/// `symlinkat(target, newdirfd, linkpath)`: makes `linkpath` a symbolic link whose target is
/// the text `target` (D170; `symlink` on x86_64 from the working directory). As Linux: the
/// paths' errors (`ENOENT` for an empty target or link path), `EEXIST` if the name is taken
/// (a dangling link included), `EROFS` on a file system that cannot be changed, `EPERM` on one
/// that cannot hold a link (FAT), `ENOSPC`.
pub fn symlinkat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    target: u64,
    dirfd: u64,
    path: u64,
) -> Result<u64, Errno> {
    let mut target_buffer = PathBuffer::uninit();
    let target = read_path(process, target, &mut target_buffer)?;
    if target.is_empty() {
        return Err(Errno::ENOENT);
    }
    let mut buffer = PathBuffer::uninit();
    let (directory, name) =
        match parent_and_name_at(process, thread, dirfd, path, &mut buffer, true) {
            Ok(found) => found,
            Err(Errno::EBUSY) => return Err(Errno::EEXIST),
            Err(error) => return Err(error),
        };
    if matches!(name, b"." | b"..") || directory.lookup(name).is_ok() {
        return Err(Errno::EEXIST);
    }
    let file_system = directory.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    file_system
        .symlink(directory.id(), name, target, now::<P>())
        .map_err(|error| match error {
            Refused::ReadOnly | Refused::InvalidArgument => Errno::EPERM,
            error => refused(error),
        })?;
    inotify::created(directory, name, false);
    Ok(0)
}

/// `linkat(olddirfd, oldpath, newdirfd, newpath, flags)`: gives the file `olddirfd` and
/// `oldpath` name the new name `newpath` too (a hard link, D170; `link` on x86_64). The old
/// path's last link is the file linked unless `AT_SYMLINK_FOLLOW`; `AT_EMPTY_PATH` with an
/// empty old path links `olddirfd`'s own file. As Linux: `EINVAL` for another flag, the paths'
/// errors, `EEXIST` if the new name is taken, `EXDEV` across file systems, `EPERM` for a
/// directory or on a file system that cannot hold another name (FAT), `EROFS` on one that
/// cannot be changed, `EMLINK` for a file with as many names as it may have, `ENOENT` for a
/// file whose last name went while it was open.
pub fn linkat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    old_dirfd: u64,
    old_path: u64,
    new_dirfd: u64,
    new_path: u64,
    flags: u64,
) -> Result<u64, Errno> {
    use super::stat::at;
    if flags & !(AT_SYMLINK_FOLLOW | at::EMPTY_PATH) != 0 {
        return Err(Errno::EINVAL);
    }
    let caller = process.id().get();
    let mut old_buffer = PathBuffer::uninit();
    let old = read_path(process, old_path, &mut old_buffer)?;
    let node = if old.is_empty() {
        if flags & at::EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        let file = thread.files().file(old_dirfd)?;
        file.kind().node().ok_or(Errno::ENOENT)?
    } else {
        let (root, start) = starting_points(thread, old_dirfd, old)?;
        vfs::resolve_as(root, start, old, caller, flags & AT_SYMLINK_FOLLOW != 0)
            .map_err(walk_error)?
    };
    let mut new_buffer = PathBuffer::uninit();
    let (directory, name) =
        match parent_and_name_at(process, thread, new_dirfd, new_path, &mut new_buffer, true) {
            Ok(found) => found,
            Err(Errno::EBUSY) => return Err(Errno::EEXIST),
            Err(error) => return Err(error),
        };
    if matches!(name, b"." | b"..") || directory.lookup(name).is_ok() {
        return Err(Errno::EEXIST);
    }
    let file_system = directory.file_system();
    if !core::ptr::addr_eq(file_system, node.file_system()) {
        return Err(Errno::EXDEV);
    }
    if node.is_directory() {
        return Err(Errno::EPERM);
    }
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    file_system
        .link(node.id(), directory.id(), name, now::<P>())
        .map_err(|error| match error {
            Refused::ReadOnly => Errno::EPERM,
            error => refused(error),
        })?;
    inotify::created(directory, name, false);
    inotify::changed(node, watched::ATTRIB);
    Ok(0)
}

/// `fchmodat2(dirfd, path, mode, flags)`: sets the permission bits of the file `dirfd` and
/// `path` name to `mode & 07777` (D170; `fchmodat` is this with no flags, `chmod` on x86_64
/// from the working directory). `AT_SYMLINK_NOFOLLOW` acts on a last link itself, which has no
/// mode of its own: `EOPNOTSUPP`, as Linux; `AT_EMPTY_PATH` with an empty path acts on
/// `dirfd`'s own file. As Linux: `EINVAL` for another flag, the path's errors, `EPERM` unless
/// the caller's file system user id owns the file or is root's, `EROFS` on a file system that
/// cannot be changed.
pub fn fchmodat2<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    mode: u64,
    flags: u64,
) -> Result<u64, Errno> {
    use super::stat::at;
    if flags & !(at::SYMLINK_NOFOLLOW | at::EMPTY_PATH) != 0 {
        return Err(Errno::EINVAL);
    }
    let mut buffer = PathBuffer::uninit();
    let path = read_path(process, path, &mut buffer)?;
    let node = if path.is_empty() {
        if flags & at::EMPTY_PATH == 0 {
            return Err(Errno::ENOENT);
        }
        let file = thread.files().file(dirfd)?;
        file.kind().node().ok_or(Errno::EBADF)?
    } else {
        let (root, start) = starting_points(thread, dirfd, path)?;
        let follow_last = flags & at::SYMLINK_NOFOLLOW == 0;
        vfs::resolve_as(root, start, path, process.id().get(), follow_last).map_err(walk_error)?
    };
    change_mode(process, node, mode)
}

/// `fchmod(fd, mode)`: [`fchmodat2`] on the file `fd` is open on; a descriptor that names no
/// node (a pipe, a socket, a memory file) keeps no mode, so nothing changes. As Linux: `EBADF`
/// for a descriptor not open or `O_PATH`.
pub fn fchmod<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    mode: u64,
) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    if file.is_location_only() {
        return Err(Errno::EBADF);
    }
    match file.kind().node() {
        Some(node) => change_mode(process, node, mode),
        None => Ok(0),
    }
}

fn change_mode<P: Platform>(process: &Process<P>, node: Node, mode: u64) -> Result<u64, Errno> {
    if node.kind() == NodeKind::SymbolicLink {
        return Err(Errno::EOPNOTSUPP);
    }
    let user = process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(super::credentials::Kind::User)
        .filesystem;
    if user != 0 && user != node.attributes().owner {
        return Err(Errno::EPERM);
    }
    let file_system = node.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    file_system
        .set_permissions(node.id(), mode as u32 & 0o7777)
        .map_err(refused)?;
    inotify::changed(node, watched::ATTRIB);
    Ok(0)
}

/// `utimensat(dirfd, path, times, flags)`: sets when a file last changed, the second of the
/// two `struct timespec`s at `times` (`UTIME_NOW` or now when `times` is null, `UTIME_OMIT`
/// for no change); the access time is not kept. A null `path` names `dirfd` itself. As Linux:
/// `EINVAL` for a flag other than `AT_SYMLINK_NOFOLLOW` or `AT_EMPTY_PATH`, or a bad
/// `tv_nsec`, `EFAULT`, the path's errors, `EROFS` on a file system that cannot be changed.
pub fn utimensat<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    dirfd: u64,
    path: u64,
    times: u64,
    flags: u64,
) -> Result<u64, Errno> {
    use super::stat::at;
    if flags & !(at::SYMLINK_NOFOLLOW | at::EMPTY_PATH) != 0 {
        return Err(Errno::EINVAL);
    }
    let modified = match times {
        0 => Some(now::<P>()),
        _ => {
            let mut bytes = [0u8; 32];
            process
                .read_user_memory(times, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let field = |at: usize| i64::from_ne_bytes(bytes[at..at + 8].try_into().unwrap());
            for nanoseconds in [field(8), field(24)] {
                if !(0..1_000_000_000).contains(&nanoseconds)
                    && nanoseconds != UTIME_NOW
                    && nanoseconds != UTIME_OMIT
                {
                    return Err(Errno::EINVAL);
                }
            }
            match field(24) {
                UTIME_OMIT => None,
                UTIME_NOW => Some(now::<P>()),
                _ => Some(field(16).max(0) as u64),
            }
        }
    };
    let node = match path {
        0 => {
            let file = thread.files().file(dirfd)?;
            file.kind().node().ok_or(Errno::EBADF)?
        }
        _ => super::files::lookup(process, thread, dirfd, path)?,
    };
    let Some(modified) = modified else {
        return Ok(0);
    };
    node.file_system()
        .set_modified(node.id(), modified)
        .map_err(refused)?;
    inotify::changed(node, watched::ATTRIB);
    Ok(0)
}

/// `sync()`: puts every file system's changes on stable storage. As Linux, it cannot fail.
pub fn sync() -> Result<u64, Errno> {
    let _ = vfs::sync_all();
    Ok(0)
}

/// `fsync(fd)` and `fdatasync(fd)`: puts the changes of the file system `fd`'s file is on on
/// stable storage, all of them (a file's own changes are not committed apart). As Linux:
/// `EBADF` for a descriptor not open or `O_PATH`, `EINVAL` for the console, a pipe or a window,
/// `EIO` if the disk failed.
pub fn fsync(thread: &LinuxThread, fd: u64) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    let node = match file.kind() {
        FileKind::Path(_) => return Err(Errno::EBADF),
        FileKind::MemoryFile(_) => return Ok(0),
        kind => kind.node().ok_or(Errno::EINVAL)?,
    };
    let written = vfs::shared_pages::write_back(node);
    node.file_system()
        .sync()
        .and(written)
        .map(|()| 0)
        .map_err(|_| Errno::EIO)
}

/// `syncfs(fd)`: puts the changes of the file system `fd`'s file is on on stable storage;
/// nothing to do for the console, a pipe or a window. As Linux: `EBADF` for a descriptor not
/// open or `O_PATH`, `EIO` if the disk failed.
pub fn syncfs(thread: &LinuxThread, fd: u64) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    match file.kind() {
        FileKind::Path(_) => Err(Errno::EBADF),
        kind => match kind.node() {
            Some(node) => node
                .file_system()
                .sync()
                .map(|()| 0)
                .map_err(|_| Errno::EIO),
            None => Ok(0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::tmpfs::{Budget, Tmpfs};
    use alloc::boxed::Box;

    fn tmpfs() -> Node {
        let budget = Box::leak(Box::new(Budget::new(1 << 20)));
        let fs = Box::leak(Box::new(Tmpfs::new(budget, (0, 40), 0o1777).unwrap()));
        Node::root_of(fs)
    }

    #[test]
    fn a_path_splits_into_its_directory_and_last_name_trailing_slashes_ignored() {
        let root = tmpfs();
        let fs = root.file_system();
        fs.create(root.id(), b"d", NodeKind::Directory, 0o755, 0)
            .unwrap();
        let (directory, name) = parent_and_name(root, root, b"/d/x//", false, 0).unwrap();
        assert_eq!((directory, name), (root.lookup(b"d").unwrap(), &b"x"[..]));
        assert_eq!(
            parent_and_name(root, root, b"x", false, 0).unwrap(),
            (root, &b"x"[..])
        );
        assert_eq!(
            parent_and_name(root, root, b"/", false, 0),
            Err(Errno::EBUSY)
        );
        assert_eq!(
            parent_and_name(root, root, b"d/..", false, 0),
            Err(Errno::ENOENT)
        );
        assert_eq!(
            parent_and_name(root, root, b"d/..", true, 0).unwrap(),
            (root.lookup(b"d").unwrap(), &b".."[..])
        );
        assert_eq!(
            parent_and_name(root, root, b"/missing/x", false, 0),
            Err(Errno::ENOENT)
        );
        let long = [b'a'; NAME_MAX + 1];
        assert_eq!(
            parent_and_name(root, root, &long, false, 0),
            Err(Errno::ENAMETOOLONG)
        );
    }

    #[test]
    fn refusals_are_linux_s_errors() {
        assert_eq!(refused(Refused::NoSpace), Errno::ENOSPC);
        assert_eq!(refused(Refused::NotEmpty), Errno::ENOTEMPTY);
        assert_eq!(refused(Refused::ReadOnly), Errno::EROFS);
        assert_eq!(refused(Refused::TooBig), Errno::EFBIG);
    }

    #[test]
    fn an_open_file_description_keeps_a_removed_file_until_it_is_dropped() {
        let root = tmpfs();
        let fs = root.file_system();
        let id = fs
            .create(root.id(), b"f", NodeKind::Regular, 0o644, 0)
            .unwrap();
        fs.write(id, WriteAt::End, b"kept", 0).unwrap();
        let node = root.lookup(b"f").unwrap();
        let file = OpenFile::new(FileKind::Regular(node), open_flag::RDONLY);
        fs.remove(root.id(), b"f", false, 1).unwrap();
        let mut buffer = [0u8; 8];
        assert_eq!(fs.read_into(id, 0, &mut buffer), Ok(4));
        assert_eq!(&buffer[..4], b"kept");
        drop(file);
        assert_eq!(fs.read_into(id, 0, &mut buffer), Ok(0));
        assert_eq!(fs.attributes(id).links, 0);
    }
}
