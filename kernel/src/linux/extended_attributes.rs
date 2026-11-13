//! Extended attributes (D557): `setxattr`, `getxattr`, `listxattr` and `removexattr`, with
//! their `l` forms (a last symbolic link itself) and `f` forms (an open file). Names carry their
//! namespace (`user.`, `trusted.`, `security.`, `system.`); `trusted.` is root's alone. A file
//! system that keeps none answers `EOPNOTSUPP`: tmpfs keeps them in memory, EggFS not yet.

use super::Errno;
use super::file_changes::refused;
use super::files::{PathBuffer, read_path, starting_points, walk_error};
use super::thread::LinuxThread;
use crate::process::Process;
use crate::vfs::{self, Node};
use alloc::vec::Vec;
use eggos_hal::Platform;

/// The longest name, as Linux's `XATTR_NAME_MAX`.
const NAME_MAX: usize = 255;
/// The largest value, as Linux's `XATTR_SIZE_MAX`.
const VALUE_MAX: usize = 64 * 1024;
const CREATE: u64 = 1;
const REPLACE: u64 = 2;

/// How the call names its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// A path, following a last symbolic link.
    Path,
    /// A path, acting on a last symbolic link itself (`l` forms).
    PathNoFollow,
    /// An open descriptor (`f` forms).
    Descriptor,
}

fn node_of<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    named: Named,
    target: u64,
) -> Result<Node, Errno> {
    if named == Named::Descriptor {
        let file = thread.files().file(target)?;
        return file.kind().node().ok_or(Errno::EBADF);
    }
    let mut buffer = PathBuffer::uninit();
    let path = read_path(process, target, &mut buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (root, start) = starting_points(thread, super::stat::at::FDCWD as u64, path)?;
    vfs::resolve_as(root, start, path, process.id().get(), named == Named::Path)
        .map_err(walk_error)
}

fn read_name<P: Platform>(process: &Process<P>, address: u64) -> Result<Vec<u8>, Errno> {
    let mut name = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        process
            .read_user_memory(address + name.len() as u64, &mut byte)
            .map_err(|_| Errno::EFAULT)?;
        if byte[0] == 0 {
            break;
        }
        if name.len() == NAME_MAX {
            return Err(Errno::ERANGE);
        }
        name.push(byte[0]);
    }
    if name.is_empty() {
        return Err(Errno::ERANGE);
    }
    Ok(name)
}

fn check_namespace<P: Platform>(process: &Process<P>, name: &[u8]) -> Result<(), Errno> {
    let root = process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(super::credentials::Kind::User)
        .effective
        == 0;
    match name {
        _ if name.starts_with(b"user.") || name.starts_with(b"security.") => Ok(()),
        _ if name.starts_with(b"system.") => Ok(()),
        _ if name.starts_with(b"trusted.") && root => Ok(()),
        _ if name.starts_with(b"trusted.") => Err(Errno::EPERM),
        _ => Err(Errno::EOPNOTSUPP),
    }
}

/// `setxattr(path, name, value, size, flags)` and its forms.
pub fn set<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    named: Named,
    [target, name, value, size, flags]: [u64; 5],
) -> Result<u64, Errno> {
    if flags & !(CREATE | REPLACE) != 0 || flags == CREATE | REPLACE {
        return Err(Errno::EINVAL);
    }
    let size = usize::try_from(size).map_err(|_| Errno::E2BIG)?;
    if size > VALUE_MAX {
        return Err(Errno::E2BIG);
    }
    let name = read_name(process, name)?;
    check_namespace(process, &name)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Errno::ENOMEM)?;
    bytes.resize(size, 0);
    process
        .read_user_memory(value, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let node = node_of(process, thread, named, target)?;
    let file_system = node.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    file_system
        .set_extended_attribute(node.id(), &name, &bytes, flags == CREATE, flags == REPLACE)
        .map_err(refused)?;
    super::inotify::changed(node, super::inotify::mask::ATTRIB);
    Ok(0)
}

/// `getxattr(path, name, value, size)` and its forms: the value's length; with `size` 0, only
/// the length.
pub fn get<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    named: Named,
    [target, name, value, size]: [u64; 4],
) -> Result<u64, Errno> {
    let name = read_name(process, name)?;
    check_namespace(process, &name)?;
    let node = node_of(process, thread, named, target)?;
    let held = node
        .file_system()
        .extended_attribute(node.id(), &name)
        .map_err(refused)?;
    give(process, &held, value, size)
}

/// `listxattr(path, list, size)` and its forms: every name, each ended by a zero byte.
pub fn list<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    named: Named,
    [target, list, size]: [u64; 3],
) -> Result<u64, Errno> {
    let node = node_of(process, thread, named, target)?;
    let names = match node.file_system().extended_attribute_names(node.id()) {
        Ok(names) => names,
        Err(crate::vfs::Refused::NotSupported) => Vec::new(),
        Err(error) => return Err(refused(error)),
    };
    give(process, &names, list, size)
}

/// `removexattr(path, name)` and its forms.
pub fn remove<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    named: Named,
    [target, name]: [u64; 2],
) -> Result<u64, Errno> {
    let name = read_name(process, name)?;
    check_namespace(process, &name)?;
    let node = node_of(process, thread, named, target)?;
    let file_system = node.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    file_system
        .remove_extended_attribute(node.id(), &name)
        .map_err(refused)?;
    super::inotify::changed(node, super::inotify::mask::ATTRIB);
    Ok(0)
}

fn give<P: Platform>(process: &Process<P>, bytes: &[u8], address: u64, size: u64) -> Result<u64, Errno> {
    if size == 0 {
        return Ok(bytes.len() as u64);
    }
    if (size as usize) < bytes.len() {
        return Err(Errno::ERANGE);
    }
    process
        .write_user_memory(address, bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes.len() as u64)
}
