//! `inotify` (inotify(7), D179): `inotify_init1` (and x86_64's `inotify_init`) makes an instance,
//! a file whose reads take records of what changed in the files and directories it watches;
//! `inotify_add_watch` watches a path for a mask of events and returns its watch descriptor,
//! `inotify_rm_watch` stops watching it. Qt's `QFileSystemWatcher`, GLib's `GFileMonitor` and
//! file managers use it to see a folder change without reading it again and again.
//!
//! The calls that change files ([`super::file_changes`], `openat`'s creation, `close`) report
//! here: a name made in a watched directory (`IN_CREATE`), removed (`IN_DELETE`) or moved
//! (`IN_MOVED_FROM` and `IN_MOVED_TO`, one cookie for both), a watched file written or truncated
//! (`IN_MODIFY`), its mode or times changed or a name added or removed (`IN_ATTRIB`), closed
//! (`IN_CLOSE_WRITE`, `IN_CLOSE_NOWRITE`), moved (`IN_MOVE_SELF`) or removed with its last name
//! (`IN_DELETE_SELF`, then `IN_IGNORED` as the watch ends). Each record is Linux's `struct
//! inotify_event`: the watch descriptor, the mask (`IN_ISDIR` for a directory), the cookie, the
//! name's length padded with NULs to 16 bytes, and the name. A record the same as the last one
//! queued is not queued again; past [`MAX_QUEUED_EVENTS`] one `IN_Q_OVERFLOW` record says some
//! were lost. A read takes as many whole records as fit (`EINVAL` if the first does not), waits
//! while there are none unless `IN_NONBLOCK` (`EAGAIN`); `poll` reports the instance readable
//! while it holds records, and `ioctl(FIONREAD)` says how many bytes they take.
//!
//! While no instance watches anything, a change costs one relaxed load ([`WATCHES`]).

use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::files::{PathBuffer, read_path, starting_points, walk_error};
use super::poll::{self, INPUT_EVENTS, WakeList};
use super::terminal::Waited;
use super::thread::LinuxThread;
use super::{Errno, Iovecs};
use crate::process::Process;
use crate::sync::SpinLock;
use crate::vfs::{self, Node};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The events and flags of a watch's mask and a record's (`IN_*`).
pub mod mask {
    pub const ACCESS: u32 = 0x1;
    pub const MODIFY: u32 = 0x2;
    pub const ATTRIB: u32 = 0x4;
    pub const CLOSE_WRITE: u32 = 0x8;
    pub const CLOSE_NOWRITE: u32 = 0x10;
    pub const OPEN: u32 = 0x20;
    pub const MOVED_FROM: u32 = 0x40;
    pub const MOVED_TO: u32 = 0x80;
    pub const CREATE: u32 = 0x100;
    pub const DELETE: u32 = 0x200;
    pub const DELETE_SELF: u32 = 0x400;
    pub const MOVE_SELF: u32 = 0x800;
    /// Every event a watch may ask for.
    pub const ALL_EVENTS: u32 = 0xfff;
    pub const UNMOUNT: u32 = 0x2000;
    pub const Q_OVERFLOW: u32 = 0x4000;
    pub const IGNORED: u32 = 0x8000;
    pub const ONLYDIR: u32 = 0x0100_0000;
    pub const DONT_FOLLOW: u32 = 0x0200_0000;
    pub const EXCL_UNLINK: u32 = 0x0400_0000;
    pub const MASK_CREATE: u32 = 0x1000_0000;
    pub const MASK_ADD: u32 = 0x2000_0000;
    pub const ISDIR: u32 = 0x4000_0000;
    pub const ONESHOT: u32 = 0x8000_0000;
    /// The flags `inotify_add_watch` takes besides the events.
    pub const WATCH_FLAGS: u32 =
        ONLYDIR | DONT_FOLLOW | EXCL_UNLINK | MASK_CREATE | MASK_ADD | ONESHOT;
}

/// `inotify_init1`'s flags (`IN_*`).
pub mod flag {
    use super::open_flag;

    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = open_flag::CLOEXEC;
    /// Reads that would wait fail with `EAGAIN` instead.
    pub const NONBLOCK: u32 = open_flag::NONBLOCK;
}

/// The size of a record without its name (`struct inotify_event`), and the unit its name is
/// padded to.
pub const EVENT_SIZE: usize = 16;
/// The most records one instance holds (`max_queued_events`).
pub const MAX_QUEUED_EVENTS: usize = 16_384;
/// The most watches one instance holds (`max_user_watches`, here per instance).
pub const MAX_WATCHES: usize = 8_192;

/// How many watches every instance holds together: while it is 0, a change reports nothing.
pub static WATCHES: AtomicUsize = AtomicUsize::new(0);
static INSTANCES: SpinLock<Option<Box<Vec<Weak<Inotify>>>>> = SpinLock::new(None);
static COOKIES: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy)]
struct Watch {
    descriptor: i32,
    node: Node,
    mask: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    descriptor: i32,
    mask: u32,
    cookie: u32,
    name: Vec<u8>,
}

impl Record {
    fn size(&self) -> usize {
        EVENT_SIZE + padded_name_length(self.name.len())
    }

    fn encode_into(&self, bytes: &mut Vec<u8>) {
        let length = padded_name_length(self.name.len());
        bytes.extend_from_slice(&self.descriptor.to_ne_bytes());
        bytes.extend_from_slice(&self.mask.to_ne_bytes());
        bytes.extend_from_slice(&self.cookie.to_ne_bytes());
        bytes.extend_from_slice(&(length as u32).to_ne_bytes());
        bytes.extend_from_slice(&self.name);
        bytes.resize(bytes.len() + length - self.name.len(), 0);
    }
}

/// The bytes a record's name takes: the name, its NUL and the NULs that pad it to a multiple
/// of [`EVENT_SIZE`]; 0 for no name.
pub fn padded_name_length(name: usize) -> usize {
    match name {
        0 => 0,
        name => (name + 1).div_ceil(EVENT_SIZE) * EVENT_SIZE,
    }
}

#[derive(Debug, Default)]
struct State {
    watches: Vec<Watch>,
    records: VecDeque<Record>,
    queued_bytes: usize,
    last_descriptor: i32,
    registered: bool,
}

impl State {
    fn push(&mut self, record: Record) {
        if self.records.back() == Some(&record) {
            return;
        }
        let overflow = self.records.len() + 1 >= MAX_QUEUED_EVENTS;
        if overflow
            && self
                .records
                .back()
                .is_some_and(|last| last.mask == mask::Q_OVERFLOW)
        {
            return;
        }
        let record = match overflow {
            true => Record {
                descriptor: -1,
                mask: mask::Q_OVERFLOW,
                cookie: 0,
                name: Vec::new(),
            },
            false => record,
        };
        if self.records.try_reserve(1).is_err() {
            return;
        }
        self.queued_bytes += record.size();
        self.records.push_back(record);
    }

    fn ignored(&mut self, descriptor: i32) {
        self.push(Record {
            descriptor,
            mask: mask::IGNORED,
            cookie: 0,
            name: Vec::new(),
        });
    }
}

/// An `inotify` instance: its watches and the records waiting to be read.
#[derive(Debug)]
pub struct Inotify {
    state: SpinLock<State>,
    wake_list: Arc<WakeList>,
}

impl Inotify {
    /// An instance with no watches.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: SpinLock::new(State::default()),
            wake_list: WakeList::shared(),
        })
    }

    /// The threads and `epoll` instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// Whether records wait to be read.
    pub fn has_records(&self) -> bool {
        !self.state.lock().records.is_empty()
    }

    /// The bytes the records waiting take, as `FIONREAD` reports them.
    pub fn queued_bytes(&self) -> usize {
        self.state.lock().queued_bytes
    }

    /// The `poll` events of `requested` it reports: readable while records wait.
    pub fn events(&self, requested: u16) -> u16 {
        match self.has_records() {
            true => requested & INPUT_EVENTS,
            false => 0,
        }
    }

    /// Watches `node` for the events of `mask`, or changes its watch: replaces the watch's
    /// mask, or adds to it with `IN_MASK_ADD`. Its watch descriptor. As Linux: `EEXIST` for a
    /// node already watched with `IN_MASK_CREATE`, `ENOSPC` past [`MAX_WATCHES`] or without
    /// memory.
    pub fn add_watch(self: &Arc<Self>, node: Node, wanted: u32) -> Result<i32, Errno> {
        let (descriptor, added) = {
            let mut state = self.state.lock();
            if let Some(watch) = state.watches.iter_mut().find(|watch| watch.node == node) {
                if wanted & mask::MASK_CREATE != 0 {
                    return Err(Errno::EEXIST);
                }
                watch.mask = match wanted & mask::MASK_ADD {
                    0 => wanted,
                    _ => watch.mask | wanted,
                } & !mask::MASK_ADD;
                (watch.descriptor, false)
            } else {
                if state.watches.len() >= MAX_WATCHES || state.watches.try_reserve(1).is_err() {
                    return Err(Errno::ENOSPC);
                }
                state.last_descriptor += 1;
                let descriptor = state.last_descriptor;
                state.watches.push(Watch {
                    descriptor,
                    node,
                    mask: wanted & !(mask::MASK_ADD | mask::MASK_CREATE),
                });
                (descriptor, true)
            }
        };
        if added {
            WATCHES.fetch_add(1, Ordering::Relaxed);
            self.register();
        }
        Ok(descriptor)
    }

    /// Stops watch `descriptor` and queues its `IN_IGNORED` record. `EINVAL` if it is not one
    /// of this instance's watches.
    pub fn remove_watch(&self, descriptor: i32) -> Result<(), Errno> {
        {
            let mut state = self.state.lock();
            let at = state
                .watches
                .iter()
                .position(|watch| watch.descriptor == descriptor)
                .ok_or(Errno::EINVAL)?;
            state.watches.swap_remove(at);
            state.ignored(descriptor);
        }
        WATCHES.fetch_sub(1, Ordering::Relaxed);
        self.wake_list.wake();
        Ok(())
    }

    /// Takes the records that fit in `room` bytes, encoded as Linux's records; `None` while
    /// there are none. `EINVAL` if the first does not fit.
    pub fn take(&self, room: usize) -> Result<Option<Vec<u8>>, Errno> {
        let mut state = self.state.lock();
        let Some(first) = state.records.front() else {
            return Ok(None);
        };
        if first.size() > room {
            return Err(Errno::EINVAL);
        }
        let mut bytes = Vec::new();
        while let Some(record) = state.records.front() {
            let size = record.size();
            if bytes.len() + size > room || bytes.try_reserve(size).is_err() {
                break;
            }
            record.encode_into(&mut bytes);
            state.queued_bytes -= size;
            state.records.pop_front();
        }
        Ok(Some(bytes))
    }

    fn report(&self, node: Node, event: u32, cookie: u32, name: &[u8], is_directory: bool) -> bool {
        let mut reported = false;
        let mut ended = 0usize;
        {
            let mut state = self.state.lock();
            let mut at = 0;
            while at < state.watches.len() {
                let watch = state.watches[at];
                let ends = event & mask::DELETE_SELF != 0;
                let asked = watch.mask & event & mask::ALL_EVENTS != 0;
                if watch.node != node || (!asked && !ends) {
                    at += 1;
                    continue;
                }
                if asked {
                    let directory = match is_directory {
                        true => mask::ISDIR,
                        false => 0,
                    };
                    let mut copied = Vec::new();
                    if copied.try_reserve_exact(name.len()).is_ok() {
                        copied.extend_from_slice(name);
                        state.push(Record {
                            descriptor: watch.descriptor,
                            mask: event | directory,
                            cookie,
                            name: copied,
                        });
                        reported = true;
                    }
                }
                if ends || watch.mask & mask::ONESHOT != 0 {
                    state.watches.swap_remove(at);
                    state.ignored(watch.descriptor);
                    reported = true;
                    ended += 1;
                } else {
                    at += 1;
                }
            }
        }
        if ended > 0 {
            WATCHES.fetch_sub(ended, Ordering::Relaxed);
        }
        if reported {
            self.wake_list.wake();
        }
        reported
    }

    fn register(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.registered {
                return;
            }
            state.registered = true;
        }
        let mut instances = INSTANCES.lock();
        let list = instances.get_or_insert_with(|| Box::new(Vec::new()));
        list.retain(|instance| instance.strong_count() > 0);
        if list.try_reserve(1).is_ok() {
            list.push(Arc::downgrade(self));
        }
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        let watches = self.state.get_mut().watches.len();
        if watches > 0 {
            WATCHES.fetch_sub(watches, Ordering::Relaxed);
        }
        self.wake_list.wake();
    }
}

/// Whether any instance watches anything: a caller that must look before a change to report it
/// looks only then.
#[inline]
pub fn watching() -> bool {
    WATCHES.load(Ordering::Relaxed) > 0
}

fn each_instance(mut report: impl FnMut(&Inotify)) {
    let mut at = 0;
    loop {
        let instance = {
            let instances = INSTANCES.lock();
            let Some(weak) = instances.as_ref().and_then(|list| list.get(at)) else {
                return;
            };
            weak.upgrade()
        };
        if let Some(instance) = instance {
            report(&instance);
        }
        at += 1;
    }
}

/// Reports `event` on `node` to the watches of `node` itself, with `name` when `node` is the
/// directory holding what changed. One relaxed load while nothing is watched.
#[inline]
pub fn changed_in(node: Node, event: u32, cookie: u32, name: &[u8], is_directory: bool) {
    if WATCHES.load(Ordering::Relaxed) == 0 {
        return;
    }
    each_instance(|instance| {
        instance.report(node, event, cookie, name, is_directory);
    });
}

/// Reports `event` on `node` to its own watches, without a name.
#[inline]
pub fn changed(node: Node, event: u32) {
    if WATCHES.load(Ordering::Relaxed) == 0 {
        return;
    }
    let is_directory = node.is_directory();
    changed_in(node, event, 0, &[], is_directory);
}

/// A name `name` was made in `directory` (`IN_CREATE`).
pub fn created(directory: Node, name: &[u8], is_directory: bool) {
    changed_in(directory, mask::CREATE, 0, name, is_directory);
}

/// The name `name` of `node` was removed from `directory`: `IN_DELETE` there, and on `node`
/// `IN_DELETE_SELF` when it was its last name (`last_name`), `IN_ATTRIB` otherwise.
pub fn removed(directory: Node, name: &[u8], node: Node, last_name: bool) {
    if WATCHES.load(Ordering::Relaxed) == 0 {
        return;
    }
    let is_directory = node.is_directory();
    changed_in(directory, mask::DELETE, 0, name, is_directory);
    let event = match last_name || is_directory {
        true => mask::DELETE_SELF,
        false => mask::ATTRIB,
    };
    changed_in(node, event, 0, &[], is_directory);
}

/// `node` moved from `from_name` in `from` to `to_name` in `to`: `IN_MOVED_FROM` and
/// `IN_MOVED_TO` with one cookie, and `IN_MOVE_SELF` on `node`.
pub fn moved(from: Node, from_name: &[u8], to: Node, to_name: &[u8], node: Node) {
    if WATCHES.load(Ordering::Relaxed) == 0 {
        return;
    }
    let is_directory = node.is_directory();
    let cookie = COOKIES
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
        .max(1);
    changed_in(from, mask::MOVED_FROM, cookie, from_name, is_directory);
    changed_in(to, mask::MOVED_TO, cookie, to_name, is_directory);
    changed_in(node, mask::MOVE_SELF, 0, &[], is_directory);
}

/// The description of regular file `node`, opened for writing if `wrote`, was closed.
#[inline]
pub fn closed(node: Node, wrote: bool) {
    let event = match wrote {
        true => mask::CLOSE_WRITE,
        false => mask::CLOSE_NOWRITE,
    };
    changed(node, event);
}

/// `inotify_init1(flags)`: a new instance, open for reading, its descriptor returned. As
/// Linux: `EINVAL` for flags other than `IN_CLOEXEC` and `IN_NONBLOCK`, `EMFILE` without a free
/// descriptor, `ENOMEM` without memory. x86_64's `inotify_init()` is this with no flags.
pub fn inotify_init1<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !(flag::CLOEXEC | flag::NONBLOCK) != 0 {
        return Err(Errno::EINVAL);
    }
    if !crate::heap::may_grow_for_user(size_of::<Inotify>() + size_of::<WakeList>()) {
        return Err(Errno::ENOMEM);
    }
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::Inotify(Inotify::new())),
        open_flag::RDONLY | flags & flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & flag::CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

/// `inotify_add_watch(fd, path, mask)`: watches what `path` names (from the working
/// directory) for `mask`'s events, its watch descriptor returned. As Linux, in this order:
/// `EBADF` for a descriptor not open, `EINVAL` for one that is not an instance, for a mask
/// without events, with unknown bits, or with both `IN_MASK_ADD` and `IN_MASK_CREATE`; the
/// path's errors (its last symbolic link not followed with `IN_DONT_FOLLOW`); `ENOTDIR` for
/// something not a directory with `IN_ONLYDIR`; then [`Inotify::add_watch`]'s errors.
pub fn inotify_add_watch<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    path: u64,
    wanted: u64,
) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    let FileKind::Event(EventFile::Inotify(instance)) = file.kind() else {
        return Err(Errno::EINVAL);
    };
    let wanted = wanted as u32;
    let both = mask::MASK_ADD | mask::MASK_CREATE;
    if wanted & mask::ALL_EVENTS == 0
        || wanted & !(mask::ALL_EVENTS | mask::WATCH_FLAGS) != 0
        || wanted & both == both
    {
        return Err(Errno::EINVAL);
    }
    let mut buffer = PathBuffer::uninit();
    let path = read_path(process, path, &mut buffer)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (root, start) = starting_points(thread, super::stat::at::FDCWD as u64, path)?;
    let follow = wanted & mask::DONT_FOLLOW == 0;
    let node =
        vfs::resolve_as(root, start, path, process.id().get(), follow).map_err(walk_error)?;
    if wanted & mask::ONLYDIR != 0 && !node.is_directory() {
        return Err(Errno::ENOTDIR);
    }
    let wanted = wanted & !(mask::ONLYDIR | mask::DONT_FOLLOW | mask::EXCL_UNLINK);
    instance
        .add_watch(node, wanted)
        .map(|descriptor| descriptor as u32 as u64)
}

/// `inotify_rm_watch(fd, wd)`: stops watch `wd`. As Linux: `EBADF` for a descriptor not open,
/// `EINVAL` for one that is not an instance or a watch descriptor that is not one of its.
pub fn inotify_rm_watch(thread: &LinuxThread, fd: u64, descriptor: u64) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    let FileKind::Event(EventFile::Inotify(instance)) = file.kind() else {
        return Err(Errno::EINVAL);
    };
    instance.remove_watch(descriptor as u32 as i32).map(|()| 0)
}

/// `read` or `readv` of `buffers` from `instance`, whose description is `file`: the records
/// that fit, as the module documentation says. Sets the call's result.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    instance: &Inotify,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let room = usize::try_from(buffers.total()).unwrap_or(usize::MAX);
    poll::blocking(process, thread, context, |process, thread| {
        loop {
            match instance.take(room) {
                Err(errno) => return ControlFlow::Continue(Some(Err(errno))),
                Ok(Some(bytes)) => {
                    let copied = poll::copy_to_buffers(process, buffers, 0, &bytes);
                    return ControlFlow::Continue(Some(copied.map(|()| bytes.len() as u64)));
                }
                Ok(None) => {}
            }
            if file.is_nonblocking() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            let lists = [instance.wake_list()];
            let ready = || instance.has_records();
            if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
                return ControlFlow::Continue(None);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::poll::event;
    use crate::vfs::NodeKind;
    use crate::vfs::tmpfs::{Budget, Tmpfs};

    fn tmpfs(device: u32) -> Node {
        let budget = Box::leak(Box::new(Budget::new(1 << 20)));
        let fs = Box::leak(Box::new(Tmpfs::new(budget, (0, device), 0o1777).unwrap()));
        Node::root_of(fs)
    }

    fn made(directory: Node, name: &[u8], kind: NodeKind) -> Node {
        directory
            .file_system()
            .create(directory.id(), name, kind, 0o644, 0)
            .unwrap();
        directory.lookup(name).unwrap()
    }

    struct Read {
        descriptor: i32,
        mask: u32,
        cookie: u32,
        name: Vec<u8>,
    }

    fn records(bytes: &[u8]) -> Vec<Read> {
        let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut read = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let length = word(at + 12) as usize;
            let name = &bytes[at + EVENT_SIZE..at + EVENT_SIZE + length];
            let end = name.iter().position(|&byte| byte == 0).unwrap_or(length);
            read.push(Read {
                descriptor: word(at) as i32,
                mask: word(at + 4),
                cookie: word(at + 8),
                name: name[..end].to_vec(),
            });
            at += EVENT_SIZE + length;
        }
        read
    }

    #[test]
    fn names_are_padded_with_nuls_to_sixteen_bytes() {
        assert_eq!(padded_name_length(0), 0);
        assert_eq!(padded_name_length(1), 16);
        assert_eq!(padded_name_length(15), 16);
        assert_eq!(padded_name_length(16), 32);
        let record = Record {
            descriptor: 3,
            mask: mask::CREATE,
            cookie: 0,
            name: b"note.txt".to_vec(),
        };
        let mut bytes = Vec::new();
        record.encode_into(&mut bytes);
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[12..16], &16u32.to_ne_bytes());
        assert_eq!(&bytes[16..24], b"note.txt");
        assert!(bytes[24..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_watched_directory_reports_names_made_moved_and_removed_in_it() {
        let root = tmpfs(70);
        let folder = made(root, b"folder", NodeKind::Directory);
        let instance = Inotify::new();
        let watched = mask::CREATE | mask::DELETE | mask::MOVED_FROM | mask::MOVED_TO;
        let descriptor = instance.add_watch(folder, watched).unwrap();
        assert_eq!(descriptor, 1);
        assert_eq!(instance.events(event::IN), 0);
        let file = made(folder, b"a.txt", NodeKind::Regular);
        created(folder, b"a.txt", false);
        created(folder, b"a.txt", false);
        moved(folder, b"a.txt", folder, b"b.txt", file);
        removed(folder, b"b.txt", file, true);
        created(root, b"elsewhere", false);
        assert_eq!(instance.events(event::IN), event::IN);
        let queued = instance.queued_bytes();
        let bytes = instance.take(4096).unwrap().unwrap();
        assert_eq!(bytes.len(), queued);
        let read = records(&bytes);
        let masks: Vec<u32> = read.iter().map(|record| record.mask).collect();
        assert_eq!(
            masks,
            [mask::CREATE, mask::MOVED_FROM, mask::MOVED_TO, mask::DELETE]
        );
        assert!(read.iter().all(|record| record.descriptor == descriptor));
        assert_eq!(read[0].name, b"a.txt");
        assert_eq!(read[2].name, b"b.txt");
        assert_ne!(read[1].cookie, 0);
        assert_eq!(read[1].cookie, read[2].cookie);
        assert_eq!(instance.take(4096), Ok(None));
        assert_eq!(instance.queued_bytes(), 0);
    }

    #[test]
    fn a_watched_file_reports_changes_to_itself_and_its_watch_ends_with_its_last_name() {
        let root = tmpfs(71);
        let file = made(root, b"note.txt", NodeKind::Regular);
        let instance = Inotify::new();
        let wanted = mask::MODIFY | mask::ATTRIB | mask::CLOSE_WRITE | mask::DELETE_SELF;
        let descriptor = instance.add_watch(file, wanted).unwrap();
        changed(file, mask::MODIFY);
        changed(file, mask::OPEN);
        closed(file, true);
        closed(file, false);
        removed(root, b"note.txt", file, false);
        removed(root, b"note.txt", file, true);
        changed(file, mask::MODIFY);
        let read = records(&instance.take(4096).unwrap().unwrap());
        let masks: Vec<u32> = read.iter().map(|record| record.mask).collect();
        assert_eq!(
            masks,
            [
                mask::MODIFY,
                mask::CLOSE_WRITE,
                mask::ATTRIB,
                mask::DELETE_SELF,
                mask::IGNORED
            ]
        );
        assert!(read.iter().all(|record| record.descriptor == descriptor));
        assert_eq!(instance.remove_watch(descriptor), Err(Errno::EINVAL));
    }

    #[test]
    fn watches_change_their_mask_end_once_or_when_removed_and_a_small_read_is_refused() {
        let root = tmpfs(72);
        let folder = made(root, b"d", NodeKind::Directory);
        let instance = Inotify::new();
        let first = instance.add_watch(folder, mask::CREATE).unwrap();
        assert_eq!(
            instance.add_watch(folder, mask::DELETE | mask::MASK_CREATE),
            Err(Errno::EEXIST)
        );
        assert_eq!(
            instance.add_watch(folder, mask::DELETE | mask::MASK_ADD),
            Ok(first)
        );
        created(folder, b"x", true);
        assert_eq!(instance.take(EVENT_SIZE), Err(Errno::EINVAL));
        let read = records(&instance.take(32).unwrap().unwrap());
        assert_eq!(read[0].mask, mask::CREATE | mask::ISDIR);
        let other = made(root, b"e", NodeKind::Directory);
        let once = instance
            .add_watch(other, mask::CREATE | mask::ONESHOT)
            .unwrap();
        assert_eq!(once, first + 1);
        created(other, b"y", false);
        created(other, b"z", false);
        let read = records(&instance.take(4096).unwrap().unwrap());
        let masks: Vec<(i32, u32)> = read
            .iter()
            .map(|record| (record.descriptor, record.mask))
            .collect();
        assert_eq!(masks, [(once, mask::CREATE), (once, mask::IGNORED)]);
        assert_eq!(instance.remove_watch(first), Ok(()));
        let read = records(&instance.take(4096).unwrap().unwrap());
        assert_eq!((read[0].descriptor, read[0].mask), (first, mask::IGNORED));
        created(folder, b"w", false);
        assert_eq!(instance.take(4096), Ok(None));
    }

    #[test]
    fn a_full_queue_says_it_overflowed_once() {
        let root = tmpfs(73);
        let instance = Inotify::new();
        instance.add_watch(root, mask::CREATE).unwrap();
        for index in 0..MAX_QUEUED_EVENTS + 10 {
            let name = alloc::format!("{index}");
            created(root, name.as_bytes(), false);
        }
        let mut overflows = 0;
        let mut total = 0;
        while let Some(bytes) = instance.take(1 << 16).unwrap() {
            for record in records(&bytes) {
                total += 1;
                if record.mask == mask::Q_OVERFLOW {
                    overflows += 1;
                    assert_eq!(record.descriptor, -1);
                }
            }
        }
        assert_eq!(overflows, 1);
        assert_eq!(total, MAX_QUEUED_EVENTS);
    }
}
