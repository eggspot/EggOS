//! The virtual file system (D102): the architecture-independent core every file system plugs
//! into, and the mounted root.
//!
//! A file system implements [`FileSystem`]: its nodes are numbered by the file system
//! ([`Node`] pairs the number with the file system), and it answers lookups of a name in a
//! directory, attributes, reads at an offset and a directory's entries by position. The core
//! walks paths ([`resolve`]) as Linux does: absolute from the root, relative from a starting
//! directory, empty and `.` components skipped, `..` to the parent (the root's own parent is
//! the root), every component but the last required to be a directory, and a trailing slash
//! or `/.` requiring the last to be one too. Errors come in the order a walk meets them.
//! A symbolic link met on the way is followed (D168): its target, which its file system makes
//! for the walking process ([`FileSystem::link_target`]: `/proc/self` names the caller's
//! directory), is walked from the link's directory, or from the root if it is absolute; the
//! last component is followed only when the caller asks ([`resolve_as`]); more than
//! [`MAX_LINKS`] links in one walk, or links nested deeper than [`MAX_NESTED_LINKS`], are
//! [`Error::TooManyLinks`] (`ELOOP`), as on Linux. [`resolve_without_links`] refuses every
//! link instead (`openat2`'s `RESOLVE_NO_SYMLINKS`, D170).
//!
//! Writable file systems make symbolic links ([`FileSystem::symlink`]) and give a file more
//! names ([`FileSystem::link`], D170). A file whose last name goes while it is held
//! ([`Node::hold`]: an open file description, a mapping, a running program's image) stays
//! readable and writable until its last hold is released ([`HeldNode`]), as on Linux.
//!
//! The root is mounted once at boot ([`mount_embedded_initramfs`]) and never unmounted, so a
//! node holds a `'static` reference to its file system and costs no reference count; mounts
//! that can go away (tmpfs, devfs, disks) will count references in their mount. Other file
//! systems are mounted on directories at boot ([`mount`], D125), at most [`MAX_MOUNTS`],
//! never unmounted either: a lookup that finds a mount point gives the mounted root, and `..`
//! from a mounted root leaves through its mount point.

pub mod cpio;
pub mod devfs;
pub mod eggfs;
pub mod fat;
pub mod initramfs;
pub mod shared_pages;
pub mod tmpfs;

use core::sync::atomic::{AtomicPtr, Ordering};

/// The longest path, its terminating NUL included (`PATH_MAX`).
pub const PATH_MAX: usize = 4096;

/// The longest name of one path component (`NAME_MAX`).
pub const NAME_MAX: usize = 255;

/// Why a path cannot be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A component does not exist (`ENOENT`).
    NotFound,
    /// A component looked up in, or required to be, a directory is not one (`ENOTDIR`).
    NotDirectory,
    /// A component is longer than [`NAME_MAX`] (`ENAMETOOLONG`).
    NameTooLong,
    /// The walk met more symbolic links than it follows (`ELOOP`).
    TooManyLinks,
}

/// The most symbolic links one walk follows (Linux's `MAXSYMLINKS`).
pub const MAX_LINKS: u32 = 40;

/// The most symbolic links a walk follows inside one another's targets.
pub const MAX_NESTED_LINKS: u32 = 8;

/// The process a walk is for, which `/proc/self` names: none for the kernel's own walks.
pub const KERNEL_CALLER: u32 = 0;

/// Why a file system refused a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// It cannot be changed (`EROFS`).
    ReadOnly,
    /// The name is taken (`EEXIST`).
    Exists,
    /// Nothing has that name, or the node is gone (`ENOENT`).
    NotFound,
    /// A directory was needed (`ENOTDIR`).
    NotDirectory,
    /// A directory is not what this takes (`EISDIR`).
    IsDirectory,
    /// The directory has entries (`ENOTEMPTY`).
    NotEmpty,
    /// Its memory is used up (`ENOSPC`).
    NoSpace,
    /// The name is longer than [`NAME_MAX`] (`ENAMETOOLONG`).
    NameTooLong,
    /// The change makes no sense, such as a directory moved into itself (`EINVAL`).
    InvalidArgument,
    /// A file would be larger than the largest offset (`EFBIG`).
    TooBig,
    /// The name is `.` or `..`, which a rename cannot target (`EBUSY`).
    Busy,
    /// The storage under the file system failed, or what it holds is inconsistent (`EIO`).
    Storage,
    /// The file has as many names as it may have (`EMLINK`).
    TooManyLinks,
    /// The change is not allowed on that node, such as a hard link to a directory (`EPERM`).
    NotPermitted,
}

/// The storage under a file system failed: a disk did not answer or reported an error, or
/// what it holds is inconsistent (`EIO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageError;

/// Where a write goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAt {
    /// At this offset.
    Offset(u64),
    /// At the end of the file, wherever it is when the write is made (`O_APPEND`).
    End,
}

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A directory: it has entries.
    Directory,
    /// A regular file: it has contents.
    Regular,
    /// A character device (`/dev`, D127): opening it opens the device.
    CharacterDevice,
    /// A socket file (D138): `bind` makes it, `connect` finds its socket by it, and opening it
    /// is refused.
    Socket,
    /// A symbolic link (D168): a walk goes on at its target ([`FileSystem::link_target`]).
    SymbolicLink,
}

/// A node's attributes, as `stat` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    /// What it is.
    pub kind: NodeKind,
    /// Its permission bits (`st_mode & 07777`).
    pub permissions: u32,
    /// Its number on its file system (`st_ino`).
    pub file_id: u64,
    /// How many names it has (`st_nlink`): 2 plus its subdirectories for a directory.
    pub links: u32,
    /// Its owner's user id.
    pub owner: u32,
    /// Its group id.
    pub group: u32,
    /// Its size in bytes.
    pub size: u64,
    /// When its contents last changed, in seconds since the Unix epoch.
    pub modified: u64,
}

/// A name of one path component, at most [`NAME_MAX`] bytes, held by value: a file system
/// whose names change gives copies of them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME_MAX],
    length: u8,
}

impl core::fmt::Debug for Name {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("Name")
            .field(&core::str::from_utf8(self.as_bytes()))
            .finish()
    }
}

impl Name {
    /// The first [`NAME_MAX`] bytes of `bytes`.
    pub fn new(bytes: &[u8]) -> Self {
        let length = bytes.len().min(NAME_MAX);
        let mut name = Self {
            bytes: [0; NAME_MAX],
            length: length as u8,
        };
        name.bytes[..length].copy_from_slice(&bytes[..length]);
        name
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

/// One entry of a directory, as [`FileSystem::entry`] lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry {
    /// Its name.
    pub name: Name,
    /// The node it names.
    pub node: u64,
    /// What that node is.
    pub kind: NodeKind,
}

/// A file system: see the module documentation. Node numbers are the file system's own; the
/// core only passes back numbers the file system gave it, directories' to the directory calls.
pub trait FileSystem: Sync + Send {
    /// Its root directory.
    fn root(&self) -> u64;
    /// Its device number (major, minor), as `st_dev` reports it.
    fn device(&self) -> (u32, u32);
    /// What `node` is.
    fn kind(&self, node: u64) -> NodeKind;
    /// The attributes of `node`.
    fn attributes(&self, node: u64) -> Attributes;
    /// The node named `name` in `directory`, if there is one.
    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64>;
    /// The directory that holds `node`; the root for the root.
    fn parent(&self, node: u64) -> u64;
    /// The name `node` has in its parent; empty for the root.
    fn name(&self, node: u64) -> Name;
    /// Up to `length` bytes of regular file `node`'s contents from `offset`, at least one if
    /// any are there: fewer than asked at its end, or where the file system's storage breaks.
    fn read(&self, node: u64, offset: u64, length: usize) -> &[u8];
    /// The contents of regular file `node` made now, for a file system whose files are made
    /// when they are opened (`/proc`); `None` for one whose contents [`Self::read`] gives;
    /// [`StorageError`] if the device a file reports on did not answer (`/proc/driver/rtc`).
    fn generate(&self, _node: u64) -> Result<Option<alloc::vec::Vec<u8>>, StorageError> {
        Ok(None)
    }
    /// Whether reads go through [`Self::read_into`], for contents that may change, rather than
    /// [`Self::read`].
    fn copies_contents(&self) -> bool {
        false
    }
    /// Copies regular file `node`'s contents from `offset` into `buffer`: how many bytes, at
    /// least one if any are there, 0 at the end; [`StorageError`] if the storage under the file
    /// system failed.
    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
        let bytes = self.read(node, offset, buffer.len());
        buffer[..bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    /// Whether it can be changed: the methods below do something.
    fn is_writable(&self) -> bool {
        false
    }
    /// Writes `bytes` to regular file `node` `at` an offset or its end at time `now` (seconds
    /// since the epoch): how many it wrote, fewer if memory ran out, and the offset after them.
    fn write(
        &self,
        _node: u64,
        _at: WriteAt,
        _bytes: &[u8],
        _now: u64,
    ) -> Result<(usize, u64), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Makes regular file `node` `length` bytes long, cutting it or adding zeros.
    fn truncate(&self, _node: u64, _length: u64, _now: u64) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Makes an empty `kind` named `name` in `directory`, with `permissions`: its node.
    fn create(
        &self,
        _directory: u64,
        _name: &[u8],
        _kind: NodeKind,
        _permissions: u32,
        _now: u64,
    ) -> Result<u64, Refused> {
        Err(Refused::ReadOnly)
    }
    /// Removes `name` from `directory`: an empty directory if `directories`, else a file.
    fn remove(
        &self,
        _directory: u64,
        _name: &[u8],
        _directories: bool,
        _now: u64,
    ) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Moves `from_name` in `from` to `to_name` in `to`, replacing what is there unless
    /// `no_replace`.
    fn rename(
        &self,
        _from: u64,
        _from_name: &[u8],
        _to: u64,
        _to_name: &[u8],
        _no_replace: bool,
        _now: u64,
    ) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// The device number (major, minor) of character device `node`, if it is one.
    fn character_device(&self, _node: u64) -> Option<(u32, u32)> {
        None
    }
    /// The target of symbolic link `node` as a walk for process `caller` reads it (`readlink`
    /// and the walk, D168); `None` if it names nothing now (a process's link once it ended).
    fn link_target(&self, _node: u64, _caller: u32) -> Option<alloc::vec::Vec<u8>> {
        None
    }
    /// Makes a symbolic link named `name` in `directory` whose target is `target` (D170): its
    /// node. [`Refused::InvalidArgument`] for a target the file system cannot keep.
    fn symlink(
        &self,
        _directory: u64,
        _name: &[u8],
        _target: &[u8],
        _now: u64,
    ) -> Result<u64, Refused> {
        Err(Refused::ReadOnly)
    }
    /// Gives `node`, a file or a symbolic link of this file system, the name `name` in
    /// `directory` too (a hard link, D170): [`Refused::NotPermitted`] for a directory.
    fn link(&self, _node: u64, _directory: u64, _name: &[u8], _now: u64) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Sets `node`'s permission bits (`chmod`, D170) to `permissions & 0o7777`.
    fn set_permissions(&self, _node: u64, _permissions: u32) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Keeps `node` alive while something uses it: when its last name goes, its contents stay
    /// until the matching [`Self::release`] (D170). A file system whose nodes never go does
    /// nothing.
    fn hold(&self, _node: u64) {}
    /// Ends one [`Self::hold`] of `node`; the last one frees a node that has no name left.
    fn release(&self, _node: u64) {}

    /// Sets when `node`'s contents last changed.
    fn set_modified(&self, _node: u64, _time: u64) -> Result<(), Refused> {
        Err(Refused::ReadOnly)
    }
    /// Entry `position` of `directory`, in a stable order, `.` and `..` not included.
    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry>;
    /// Puts every change made so far on stable storage (`sync`, `syncfs`, `fsync`); a file
    /// system with nothing to put there has nothing to do.
    fn sync(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

/// A node of a mounted file system.
#[derive(Clone, Copy)]
pub struct Node {
    file_system: &'static dyn FileSystem,
    id: u64,
}

impl core::fmt::Debug for Node {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("Node").field(&self.id).finish()
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && core::ptr::addr_eq(
                self.file_system as *const dyn FileSystem,
                other.file_system as *const dyn FileSystem,
            )
    }
}

impl Eq for Node {}

impl crate::regions::FilePages for Node {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
        let mut filled = 0;
        while filled < buffer.len() {
            let at = offset.saturating_add(filled as u64);
            match self
                .file_system
                .read_into(self.id, at, &mut buffer[filled..])
            {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(StorageError) => return None,
            }
        }
        shared_pages::read_over(*self, offset, &mut buffer[..filled]);
        Some(filled)
    }

    fn path_into(&self, buffer: &mut [u8]) -> Option<usize> {
        Node::path_into(self, buffer)
    }

    fn identity(&self) -> ((u32, u32), u64) {
        (self.file_system.device(), self.attributes().file_id)
    }
}

impl Node {
    /// The root of `file_system`.
    pub fn root_of(file_system: &'static dyn FileSystem) -> Self {
        Self {
            file_system,
            id: file_system.root(),
        }
    }

    fn with_id(self, id: u64) -> Self {
        Self { id, ..self }
    }

    /// Its file system.
    pub fn file_system(&self) -> &'static dyn FileSystem {
        self.file_system
    }

    /// Its number on its file system.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// What it is.
    #[inline]
    pub fn kind(&self) -> NodeKind {
        self.file_system.kind(self.id)
    }

    /// The device number of the character device it is, if it is one.
    pub fn character_device(&self) -> Option<(u32, u32)> {
        self.file_system.character_device(self.id)
    }

    /// Its target, if it is a symbolic link, as process `caller` reads it
    /// ([`FileSystem::link_target`]).
    pub fn link_target(&self, caller: u32) -> Option<alloc::vec::Vec<u8>> {
        self.file_system.link_target(self.id, caller)
    }

    /// Keeps it alive until [`Self::release`] ([`FileSystem::hold`]).
    #[inline]
    pub fn hold(&self) {
        self.file_system.hold(self.id);
    }

    /// Ends one [`Self::hold`].
    #[inline]
    pub fn release(&self) {
        self.file_system.release(self.id);
    }

    /// Whether it is a directory.
    #[inline]
    pub fn is_directory(&self) -> bool {
        self.kind() == NodeKind::Directory
    }

    /// Its attributes.
    pub fn attributes(&self) -> Attributes {
        self.file_system.attributes(self.id)
    }

    /// The directory that holds it; the root's is the root, and a mounted root's is its
    /// mount point's.
    pub fn parent(&self) -> Self {
        match covered_by(*self) {
            Some(point) => point.parent(),
            None => self.with_id(self.file_system.parent(self.id)),
        }
    }

    /// The contents of a regular file made when it is opened ([`FileSystem::generate`]).
    pub fn generate(&self) -> Result<Option<alloc::vec::Vec<u8>>, StorageError> {
        self.file_system.generate(self.id)
    }

    /// Up to `length` bytes of its contents from `offset` ([`FileSystem::read`]).
    #[inline]
    pub fn read(&self, offset: u64, length: usize) -> &'static [u8] {
        self.file_system.read(self.id, offset, length)
    }

    /// Its entry `position` ([`FileSystem::entry`]), with the node it names.
    pub fn entry(&self, position: u64) -> Option<(Name, Self, NodeKind)> {
        let entry = self.file_system.entry(self.id, position)?;
        Some((entry.name, self.with_id(entry.node), entry.kind))
    }

    /// The node `name` names in this directory: [`Error::NotDirectory`] if this is not a
    /// directory, [`Error::NameTooLong`] for a name longer than [`NAME_MAX`],
    /// [`Error::NotFound`] if there is none. `.` and `..` are the walk's, not looked up here.
    #[inline]
    pub fn lookup(&self, name: &[u8]) -> Result<Self, Error> {
        if !self.is_directory() {
            return Err(Error::NotDirectory);
        }
        if name.len() > NAME_MAX {
            return Err(Error::NameTooLong);
        }
        let id = self
            .file_system
            .lookup(self.id, name)
            .ok_or(Error::NotFound)?;
        Ok(mounted_on(self.with_id(id)))
    }

    /// Writes its absolute path, from the root of the file system it is on, to the end of
    /// `buffer` and returns where it starts; `None` if it does not fit.
    pub fn path_into(&self, buffer: &mut [u8]) -> Option<usize> {
        let mut start = buffer.len();
        let mut node = *self;
        loop {
            if node.id == node.file_system.root() {
                match covered_by(node) {
                    Some(point) => {
                        node = point;
                        continue;
                    }
                    None => break,
                }
            }
            let name = node.file_system.name(node.id);
            let name = name.as_bytes();
            start = start.checked_sub(name.len() + 1)?;
            buffer[start] = b'/';
            buffer[start + 1..start + 1 + name.len()].copy_from_slice(name);
            node = node.parent();
        }
        if start == buffer.len() {
            start = start.checked_sub(1)?;
            buffer[start] = b'/';
        }
        Some(start)
    }
}

/// A node held while it is used ([`Node::hold`]): made by [`HeldNode::new`], released when
/// dropped. A mapping's file and a running program's image are held this way.
#[derive(Debug)]
pub struct HeldNode(Node);

impl HeldNode {
    /// Holds `node`.
    pub fn new(node: Node) -> Self {
        node.hold();
        Self(node)
    }

    /// The node.
    pub fn node(&self) -> Node {
        self.0
    }
}

impl Drop for HeldNode {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl crate::regions::FilePages for HeldNode {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
        crate::regions::FilePages::read_at(&self.0, offset, buffer)
    }

    fn path_into(&self, buffer: &mut [u8]) -> Option<usize> {
        self.0.path_into(buffer)
    }

    fn identity(&self) -> ((u32, u32), u64) {
        crate::regions::FilePages::identity(&self.0)
    }
}

/// The node `path` names, from `start` if it is relative, from `root` if it is absolute: the
/// module documentation's walk, for the kernel, following a last component that is a link. An
/// empty path names `start`; callers that refuse one (`ENOENT`) check first.
pub fn resolve(root: Node, start: Node, path: &[u8]) -> Result<Node, Error> {
    resolve_as(root, start, path, KERNEL_CALLER, true)
}

/// [`resolve_as`] that follows no symbolic link (`RESOLVE_NO_SYMLINKS`): a link met on the
/// way, or as the last component when `follow_last`, is [`Error::TooManyLinks`] (`ELOOP`); a
/// last link is the node found when not `follow_last`.
pub fn resolve_without_links(
    root: Node,
    start: Node,
    path: &[u8],
    caller: u32,
    follow_last: bool,
) -> Result<Node, Error> {
    let mut links = 0;
    let walk_rule = Walk {
        caller,
        follow_last,
        links_allowed: false,
    };
    walk(root, start, path, walk_rule, 0, &mut links)
}

/// [`resolve`] for process `caller` (`/proc/self` is its directory), following the last
/// component if it is a symbolic link only when `follow_last` (not for `lstat`, `readlink`,
/// `O_NOFOLLOW`).
pub fn resolve_as(
    root: Node,
    start: Node,
    path: &[u8],
    caller: u32,
    follow_last: bool,
) -> Result<Node, Error> {
    let mut links = 0;
    let walk_rule = Walk {
        caller,
        follow_last,
        links_allowed: true,
    };
    walk(root, start, path, walk_rule, 0, &mut links)
}

#[derive(Clone, Copy)]
struct Walk {
    caller: u32,
    follow_last: bool,
    links_allowed: bool,
}

fn walk(
    root: Node,
    start: Node,
    path: &[u8],
    rule: Walk,
    nesting: u32,
    links: &mut u32,
) -> Result<Node, Error> {
    let mut node = match path.first() {
        Some(b'/') => root,
        _ => start,
    };
    let mut must_be_directory = false;
    let mut components = path.split(|&byte| byte == b'/');
    while let Some(component) = components.next() {
        match component {
            b"" => {
                must_be_directory = true;
                continue;
            }
            b"." | b".." => {
                if !node.is_directory() {
                    return Err(Error::NotDirectory);
                }
                if component == b".." && node != root {
                    node = node.parent();
                }
                must_be_directory = true;
            }
            name => {
                let directory = node;
                node = node.lookup(name)?;
                must_be_directory = false;
                let is_last = components.clone().next().is_none();
                if node.kind() == NodeKind::SymbolicLink && (!is_last || rule.follow_last) {
                    node = follow(root, directory, node, rule, nesting, links)?;
                }
            }
        }
    }
    if must_be_directory && !node.is_directory() {
        return Err(Error::NotDirectory);
    }
    Ok(node)
}

#[cold]
#[inline(never)]
fn follow(
    root: Node,
    directory: Node,
    link: Node,
    rule: Walk,
    nesting: u32,
    links: &mut u32,
) -> Result<Node, Error> {
    *links += 1;
    if !rule.links_allowed || *links > MAX_LINKS || nesting >= MAX_NESTED_LINKS {
        return Err(Error::TooManyLinks);
    }
    let target = link.link_target(rule.caller).ok_or(Error::NotFound)?;
    if target.is_empty() {
        return Err(Error::NotFound);
    }
    let inner = Walk {
        follow_last: true,
        ..rule
    };
    walk(root, directory, &target, inner, nesting + 1, links)
}

/// The last component of `path` and the path of the directory that holds it (`.` for a
/// single relative component); `None` if `path` has no last name (it is empty, the root, or
/// ends in `/`, `.` or `..`).
pub fn split_last(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let name_start = path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |at| at + 1);
    let name = &path[name_start..];
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    let directory = match name_start {
        0 => &b"."[..],
        _ => &path[..name_start],
    };
    Some((directory, name))
}

struct Mount {
    root: Node,
}

/// The most file systems [`mount`] mounts besides the root.
pub const MAX_MOUNTS: usize = 16;

struct Mounted {
    point: Node,
    root: Node,
}

static MOUNTS: [AtomicPtr<Mounted>; MAX_MOUNTS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_MOUNTS];

fn mounts() -> impl Iterator<Item = &'static Mounted> {
    MOUNTS.iter().map_while(|slot| {
        let mounted = slot.load(Ordering::Acquire);
        // SAFETY: a slot that is not null points to a leaked `Mounted`, published once by
        // `mount` and never freed or changed.
        (!mounted.is_null()).then(|| unsafe { &*mounted })
    })
}

#[inline]
fn mounted_on(node: Node) -> Node {
    mounts()
        .find(|mounted| mounted.point == node)
        .map_or(node, |mounted| mounted.root)
}

fn covered_by(node: Node) -> Option<Node> {
    if node.id != node.file_system.root() {
        return None;
    }
    mounts()
        .find(|mounted| mounted.root == node)
        .map(|mounted| mounted.point)
}

/// Why [`mount`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    /// The path does not name a directory of the root file system.
    NoMountPoint,
    /// Something is mounted there already, or [`MAX_MOUNTS`] file systems are.
    Taken,
}

/// Whether something is mounted on the directory `path` names from the root.
pub fn is_mount_point(path: &[u8]) -> bool {
    let root = root();
    resolve(root, root, path).is_ok_and(|node| node != root && covered_by(node).is_some())
}

/// Mounts `file_system` on the directory `path` names from the root, for good.
pub fn mount(path: &[u8], file_system: &'static dyn FileSystem) -> Result<(), MountError> {
    let root = root();
    let point = resolve(root, root, path).map_err(|_| MountError::NoMountPoint)?;
    if !point.is_directory() || point == root {
        return Err(MountError::NoMountPoint);
    }
    add_mount(point, file_system)
}

/// Mounts `file_system`, whose root is a regular file, over the regular file `path` names from
/// the root, for good, as Linux mounts one file over another (D157): opening the path opens
/// the mounted file.
pub fn mount_file(path: &[u8], file_system: &'static dyn FileSystem) -> Result<(), MountError> {
    let root = root();
    let point = resolve(root, root, path).map_err(|_| MountError::NoMountPoint)?;
    if point.kind() != NodeKind::Regular
        || file_system.kind(file_system.root()) != NodeKind::Regular
    {
        return Err(MountError::NoMountPoint);
    }
    add_mount(point, file_system)
}

fn add_mount(point: Node, file_system: &'static dyn FileSystem) -> Result<(), MountError> {
    if covered_by(point).is_some() || mounts().any(|mounted| mounted.point == point) {
        return Err(MountError::Taken);
    }
    let mounted = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Mounted {
        point,
        root: Node::root_of(file_system),
    }));
    for slot in &MOUNTS {
        if slot
            .compare_exchange(
                core::ptr::null_mut(),
                mounted,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return Ok(());
        }
    }
    // SAFETY: `mounted` came from `Box::into_raw` above and was never published.
    drop(unsafe { alloc::boxed::Box::from_raw(mounted) });
    Err(MountError::Taken)
}

/// Puts every mounted file system's changes on stable storage (`sync`, and before the
/// machine goes down); [`StorageError`] if any failed, after trying them all.
pub fn sync_all() -> Result<(), StorageError> {
    let mut result = shared_pages::write_back_all();
    for mounted in mounts() {
        if mounted.root.file_system.sync().is_err() {
            result = Err(StorageError);
        }
    }
    result
}

static ROOT: AtomicPtr<Mount> = AtomicPtr::new(core::ptr::null_mut());

/// The root directory of the mounted root file system, which is mounted from the embedded
/// initramfs on first use if boot has not mounted it yet.
#[inline]
pub fn root() -> Node {
    let mut mount = ROOT.load(Ordering::Acquire);
    if mount.is_null() {
        mount_embedded_initramfs();
        mount = ROOT.load(Ordering::Acquire);
    }
    // SAFETY: `ROOT` is not null once a mount was published, and then points to a leaked
    // `Mount`, never freed or changed.
    unsafe { (*mount).root }
}

/// What mounting the embedded initramfs found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountReport {
    /// How many nodes the root file system has, its root included.
    pub nodes: usize,
    /// The archive's size in bytes.
    pub archive_size: usize,
    /// Why the archive could not be read, if it could not: the root is then empty.
    pub error: Option<initramfs::Error>,
}

/// Mounts the initramfs embedded in the kernel image as the root and reports it; `None` if a
/// root was mounted already. An archive that cannot be read leaves an empty root.
pub fn mount_embedded_initramfs() -> Option<MountReport> {
    if !ROOT.load(Ordering::Acquire).is_null() {
        return None;
    }
    let archive = initramfs::embedded();
    let (file_system, error) = match initramfs::Initramfs::parse(archive) {
        Ok(file_system) => (file_system, None),
        Err(error) => (initramfs::Initramfs::empty(), Some(error)),
    };
    let report = MountReport {
        nodes: file_system.node_count(),
        archive_size: archive.len(),
        error,
    };
    let file_system = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(file_system));
    // SAFETY: `file_system` came from `Box::into_raw` just above; it is freed below only if it
    // is never published.
    let root = Node::root_of(unsafe { &*file_system });
    let mount = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Mount { root }));
    match ROOT.compare_exchange(
        core::ptr::null_mut(),
        mount,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Some(report),
        Err(_) => {
            // SAFETY: both came from `Box::into_raw` above and were never published.
            drop(unsafe { alloc::boxed::Box::from_raw(mount) });
            // SAFETY: as above; nothing refers to it any more.
            drop(unsafe { alloc::boxed::Box::from_raw(file_system) });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::cpio::tests::archive;
    use super::initramfs::Initramfs;
    use super::*;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    fn file_system() -> Node {
        let bytes = archive(&[
            ("etc", 0o040755, 0, b""),
            ("etc/hostname", 0o100644, 0, b"eggos\n"),
            ("etc/deep", 0o040755, 0, b""),
            ("etc/deep/er", 0o040700, 0, b""),
            ("etc/deep/er/file", 0o100600, 0, b"x"),
            ("top", 0o100644, 0, b"top"),
        ]);
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let file_system = Box::leak(Box::new(Initramfs::parse(bytes).unwrap()));
        Node::root_of(file_system)
    }

    fn path(node: Node) -> Vec<u8> {
        let mut buffer = [0u8; PATH_MAX];
        let start = node.path_into(&mut buffer).unwrap();
        buffer[start..].to_vec()
    }

    #[test]
    fn absolute_and_relative_paths_resolve_through_dot_dot_and_repeated_slashes() {
        let root = file_system();
        let etc = resolve(root, root, b"/etc").unwrap();
        assert!(etc.is_directory());
        assert_eq!(path(etc), b"/etc");
        let hostname = resolve(root, etc, b"hostname").unwrap();
        assert_eq!(hostname.read(0, 100), b"eggos\n");
        assert_eq!(resolve(root, etc, b"//etc///hostname"), Ok(hostname));
        assert_eq!(resolve(root, etc, b"./deep/../hostname"), Ok(hostname));
        assert_eq!(resolve(root, etc, b"../../../etc/./hostname"), Ok(hostname));
        assert_eq!(resolve(root, etc, b""), Ok(etc));
        assert_eq!(resolve(root, etc, b"/"), Ok(root));
        assert_eq!(resolve(root, root, b".."), Ok(root));
        assert_eq!(
            resolve(root, etc, b"deep/er/"),
            resolve(root, root, b"etc/deep/er")
        );
        let file = resolve(root, root, b"/etc/deep/er/file").unwrap();
        assert_eq!(path(file), b"/etc/deep/er/file");
        assert_eq!(path(root), b"/");
        assert_eq!(path(file.parent().parent()), b"/etc/deep");
    }

    #[test]
    fn walks_fail_as_linux_s_in_the_order_they_meet_the_error() {
        let root = file_system();
        let at = |path: &[u8]| resolve(root, root, path);
        assert_eq!(at(b"/missing"), Err(Error::NotFound));
        assert_eq!(at(b"/missing/hostname"), Err(Error::NotFound));
        assert_eq!(at(b"/etc/hostname/"), Err(Error::NotDirectory));
        assert_eq!(at(b"/etc/hostname/."), Err(Error::NotDirectory));
        assert_eq!(at(b"/etc/hostname/.."), Err(Error::NotDirectory));
        assert_eq!(at(b"/etc/hostname/x"), Err(Error::NotDirectory));
        assert_eq!(at(b"top/x"), Err(Error::NotDirectory));
        let long = [b'a'; NAME_MAX + 1];
        let mut long_path = b"/etc/".to_vec();
        long_path.extend_from_slice(&long);
        assert_eq!(at(&long_path), Err(Error::NameTooLong));
        assert_eq!(at(&long[..NAME_MAX]), Err(Error::NotFound));
        long_path.extend_from_slice(b"/x");
        assert_eq!(at(&long_path), Err(Error::NameTooLong));
        let mut missing_first = b"/missing/".to_vec();
        missing_first.extend_from_slice(&long);
        assert_eq!(at(&missing_first), Err(Error::NotFound));
    }

    #[test]
    fn a_path_s_last_name_splits_from_its_directory() {
        assert_eq!(split_last(b"/etc/x"), Some((&b"/etc/"[..], &b"x"[..])));
        assert_eq!(split_last(b"x"), Some((&b"."[..], &b"x"[..])));
        assert_eq!(split_last(b"/x"), Some((&b"/"[..], &b"x"[..])));
        for none in [&b""[..], b"/", b"a/", b"a/.", b".."] {
            assert_eq!(split_last(none), None);
        }
    }

    #[test]
    fn a_path_that_does_not_fit_the_buffer_is_none() {
        let root = file_system();
        let file = resolve(root, root, b"/etc/hostname").unwrap();
        let mut small = [0u8; 13];
        assert_eq!(file.path_into(&mut small), Some(0));
        assert_eq!(&small, b"/etc/hostname");
        assert_eq!(file.path_into(&mut small[..12]), None);
        assert_eq!(root.path_into(&mut small[..1]), Some(0));
        assert_eq!(root.path_into(&mut []), None);
    }
    #[derive(Debug)]
    struct Links;

    const LINKS_ROOT: u64 = 1;
    const LINKS_DIRECTORY: u64 = 2;
    const LINKS_FILE: u64 = 3;
    const LINKS: [(&[u8], u64, &[u8]); 5] = [
        (b"loop", 4, b"loop"),
        (b"up", 5, b"/d"),
        (b"near", 6, b"d/file"),
        (b"mine", 7, b""),
        (b"gone", 8, b"nothing"),
    ];

    impl FileSystem for Links {
        fn root(&self) -> u64 {
            LINKS_ROOT
        }

        fn device(&self) -> (u32, u32) {
            (0, 99)
        }

        fn kind(&self, node: u64) -> NodeKind {
            match node {
                LINKS_ROOT | LINKS_DIRECTORY => NodeKind::Directory,
                LINKS_FILE => NodeKind::Regular,
                _ => NodeKind::SymbolicLink,
            }
        }

        fn attributes(&self, node: u64) -> Attributes {
            Attributes {
                kind: self.kind(node),
                permissions: 0o777,
                file_id: node,
                links: 1,
                owner: 0,
                group: 0,
                size: 0,
                modified: 0,
            }
        }

        fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
            match (directory, name) {
                (LINKS_ROOT, b"d") => Some(LINKS_DIRECTORY),
                (LINKS_DIRECTORY, b"file") => Some(LINKS_FILE),
                (LINKS_ROOT, name) => LINKS
                    .iter()
                    .find(|(link, _, _)| *link == name)
                    .map(|&(_, node, _)| node),
                _ => None,
            }
        }

        fn parent(&self, node: u64) -> u64 {
            match node {
                LINKS_FILE => LINKS_DIRECTORY,
                _ => LINKS_ROOT,
            }
        }

        fn name(&self, _node: u64) -> Name {
            Name::new(b"")
        }

        fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
            &[]
        }

        fn link_target(&self, node: u64, caller: u32) -> Option<Vec<u8>> {
            if node == 7 {
                return (caller != KERNEL_CALLER).then(|| b"d".to_vec());
            }
            LINKS
                .iter()
                .find(|&&(_, link, _)| link == node)
                .map(|(_, _, target)| target.to_vec())
        }

        fn entry(&self, _directory: u64, _position: u64) -> Option<DirectoryEntry> {
            None
        }
    }

    #[test]
    fn links_are_followed_from_their_directory_up_to_linux_s_limits() {
        let root = Node::root_of(Box::leak(Box::new(Links)));
        let at = |path: &[u8], follow_last| resolve_as(root, root, path, 7, follow_last);
        assert_eq!(at(b"/up/file", true).map(|node| node.id()), Ok(LINKS_FILE));
        assert_eq!(at(b"near", true).map(|node| node.id()), Ok(LINKS_FILE));
        assert_eq!(at(b"near", false).map(|node| node.id()), Ok(6));
        assert_eq!(at(b"up/", false).map(|node| node.id()), Ok(LINKS_DIRECTORY));
        assert_eq!(
            at(b"mine/file", false).map(|node| node.id()),
            Ok(LINKS_FILE)
        );
        assert_eq!(
            resolve(root, root, b"mine/file"),
            Err(Error::NotFound),
            "the kernel has no directory of its own"
        );
        assert_eq!(at(b"loop", true), Err(Error::TooManyLinks));
        assert_eq!(at(b"loop", false).map(|node| node.id()), Ok(4));
        assert_eq!(at(b"gone", true), Err(Error::NotFound));
        assert_eq!(at(b"near/x", true), Err(Error::NotDirectory));
        let without =
            |path: &[u8], follow_last| resolve_without_links(root, root, path, 7, follow_last);
        assert_eq!(without(b"up/file", true), Err(Error::TooManyLinks));
        assert_eq!(without(b"up/file", false), Err(Error::TooManyLinks));
        assert_eq!(without(b"near", true), Err(Error::TooManyLinks));
        assert_eq!(without(b"near", false).map(|node| node.id()), Ok(6));
        assert_eq!(
            without(b"/d/file", true).map(|node| node.id()),
            Ok(LINKS_FILE)
        );
    }
}
