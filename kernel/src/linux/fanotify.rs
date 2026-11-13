//! `fanotify` (D560): notification groups that watch files, directories (with their entries
//! when `FAN_EVENT_ON_CHILD`) or whole mounts, for root alone. A read gives
//! `struct fanotify_event_metadata` records, each with a descriptor opened read-only on the file
//! the event happened to (opened at the read, in the reader's table) and the id of the process
//! that caused it, 0 here. Events come from the kernel's file events (`inotify`'s): access,
//! modify, attributes, open (`openat`), close for writing and not, and creation, removal and
//! moves of entries. Permission events (`FAN_*_PERM`) are refused with `EINVAL`: EggOS gives no
//! program a veto over another's opens.

use super::Errno;
use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::files::{PathBuffer, read_path, starting_points, walk_error};
use super::inotify::mask as inotify_mask;
use super::poll::{WakeList, event};
use super::thread::LinuxThread;
use crate::process::Process;
use crate::sync::SpinLock;
use crate::vfs::{self, Node};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_hal::Platform;

pub mod mask {
    pub const ACCESS: u64 = 0x1;
    pub const MODIFY: u64 = 0x2;
    pub const ATTRIB: u64 = 0x4;
    pub const CLOSE_WRITE: u64 = 0x8;
    pub const CLOSE_NOWRITE: u64 = 0x10;
    pub const OPEN: u64 = 0x20;
    pub const MOVED_FROM: u64 = 0x40;
    pub const MOVED_TO: u64 = 0x80;
    pub const CREATE: u64 = 0x100;
    pub const DELETE: u64 = 0x200;
    pub const OPEN_PERM: u64 = 0x1_0000;
    pub const ACCESS_PERM: u64 = 0x2_0000;
    pub const OPEN_EXEC_PERM: u64 = 0x4_0000;
    pub const ONDIR: u64 = 0x4000_0000;
    pub const EVENT_ON_CHILD: u64 = 0x0800_0000;
    pub const PERMISSION: u64 = OPEN_PERM | ACCESS_PERM | OPEN_EXEC_PERM;
}

const FAN_CLOEXEC: u64 = 0x1;
const FAN_NONBLOCK: u64 = 0x2;
const FAN_CLASS_NOTIF: u64 = 0x0;
const FAN_CLASS_MASK: u64 = 0xc;
const FAN_MARK_ADD: u64 = 0x1;
const FAN_MARK_REMOVE: u64 = 0x2;
const FAN_MARK_DONT_FOLLOW: u64 = 0x4;
const FAN_MARK_ONLYDIR: u64 = 0x8;
const FAN_MARK_MOUNT: u64 = 0x10;
const FAN_MARK_FLUSH: u64 = 0x80;
const FAN_MARK_FILESYSTEM: u64 = 0x100;
const METADATA_VERSION: u8 = 3;
const METADATA_LENGTH: usize = 24;
const QUEUE_LIMIT: usize = 16384;
const FAN_Q_OVERFLOW: u64 = 0x4000;

/// What a mark watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watched {
    Node(Node),
    FileSystem(usize),
}

#[derive(Debug, Clone, Copy)]
struct Mark {
    watched: Watched,
    mask: u64,
}

#[derive(Debug)]
struct Pending {
    mask: u64,
    node: Option<Node>,
}

/// A notification group.
#[derive(Debug)]
pub struct Fanotify {
    marks: SpinLock<Vec<Mark>>,
    queue: SpinLock<VecDeque<Pending>>,
    wake_list: Arc<WakeList>,
    event_flags: u32,
}

/// Every group, weakly, for the file events to reach.
static GROUPS: SpinLock<Vec<alloc::sync::Weak<Fanotify>>> = SpinLock::new(Vec::new());
/// How many marks exist, so file events with none cost one load.
static MARKS: AtomicUsize = AtomicUsize::new(0);

/// Whether any group watches anything.
pub fn active() -> bool {
    MARKS.load(Ordering::Relaxed) != 0
}

fn file_system_key(node: Node) -> usize {
    node.file_system() as *const dyn vfs::FileSystem as *const () as usize
}

fn fanotify_mask_of(inotify_event: u32) -> u64 {
    let mut out = 0;
    for (from, to) in [
        (inotify_mask::ACCESS, mask::ACCESS),
        (inotify_mask::MODIFY, mask::MODIFY),
        (inotify_mask::ATTRIB, mask::ATTRIB),
        (inotify_mask::CLOSE_WRITE, mask::CLOSE_WRITE),
        (inotify_mask::CLOSE_NOWRITE, mask::CLOSE_NOWRITE),
        (inotify_mask::OPEN, mask::OPEN),
        (inotify_mask::MOVED_FROM, mask::MOVED_FROM),
        (inotify_mask::MOVED_TO, mask::MOVED_TO),
        (inotify_mask::CREATE, mask::CREATE),
        (inotify_mask::DELETE, mask::DELETE),
    ] {
        if inotify_event & from != 0 {
            out |= to;
        }
    }
    out
}

/// A file event: `inotify_event` on `node`, which is a directory's entry event when `entry`.
pub fn report(node: Node, inotify_event: u32, entry: bool) {
    if !active() {
        return;
    }
    let mut wanted = fanotify_mask_of(inotify_event);
    if wanted == 0 {
        return;
    }
    if node.is_directory() {
        wanted |= mask::ONDIR;
    }
    let groups: Vec<Arc<Fanotify>> = GROUPS.lock().iter().filter_map(|group| group.upgrade()).collect();
    for group in groups {
        group.deliver(node, wanted, entry);
    }
}

impl Fanotify {
    fn deliver(&self, node: Node, event: u64, entry: bool) {
        let base = event & !mask::ONDIR;
        let marks = self.marks.lock();
        let hits = marks.iter().any(|mark| {
            let place = match mark.watched {
                Watched::Node(watched) => {
                    watched == node && (!entry || mark.mask & mask::EVENT_ON_CHILD != 0)
                }
                Watched::FileSystem(key) => key == file_system_key(node),
            };
            let directory_ok = event & mask::ONDIR == 0 || mark.mask & mask::ONDIR != 0;
            place && mark.mask & base != 0 && directory_ok
        });
        drop(marks);
        if !hits {
            return;
        }
        let mut queue = self.queue.lock();
        if queue.len() >= QUEUE_LIMIT {
            if queue.back().is_none_or(|last| last.mask != FAN_Q_OVERFLOW) && queue.try_reserve(1).is_ok() {
                queue.push_back(Pending {
                    mask: FAN_Q_OVERFLOW,
                    node: None,
                });
            }
        } else if queue.try_reserve(1).is_ok() {
            queue.push_back(Pending {
                mask: base,
                node: Some(node),
            });
        }
        drop(queue);
        self.wake_list.wake();
    }

    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// The events a wait for `requested` sees.
    pub fn events(&self, requested: u16) -> u16 {
        match self.queue.lock().is_empty() {
            true => 0,
            false => requested & (event::IN | event::RDNORM),
        }
    }
}

fn is_root<P: Platform>(process: &Process<P>) -> bool {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(super::credentials::Kind::User)
        .effective
        == 0
}

/// `fanotify_init(flags, event_f_flags)`: a notification group's descriptor. As Linux: `EPERM`
/// for a caller other than root, `EINVAL` for unknown flags or a content or pre-content class.
pub fn fanotify_init<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    flags: u64,
    event_flags: u64,
) -> Result<u64, Errno> {
    if !is_root(process) {
        return Err(Errno::EPERM);
    }
    if flags & FAN_CLASS_MASK != FAN_CLASS_NOTIF {
        return Err(Errno::EINVAL);
    }
    let group = Arc::new(Fanotify {
        marks: SpinLock::new(Vec::new()),
        queue: SpinLock::new(VecDeque::new()),
        wake_list: WakeList::shared(),
        event_flags: event_flags as u32 & (open_flag::ACCMODE | open_flag::NONBLOCK | open_flag::CLOEXEC),
    });
    {
        let mut groups = GROUPS.lock();
        groups.retain(|group| group.strong_count() > 0);
        groups.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        groups.push(Arc::downgrade(&group));
    }
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::Fanotify(group)),
        open_flag::RDONLY | if flags & FAN_NONBLOCK != 0 { open_flag::NONBLOCK } else { 0 },
    ));
    let limit = open_file_limit(process);
    let fd = thread.files().lock().open(file, flags & FAN_CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

/// `fanotify_mark(fd, flags, mask, dirfd, path)`: adds, removes or flushes the group's marks on a
/// file, a directory, or the mount or file system it is on.
pub fn fanotify_mark<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    [fd, flags, events, dirfd, path, _]: [u64; 6],
) -> Result<u64, Errno> {
    let file = thread.files().file(fd)?;
    let FileKind::Event(EventFile::Fanotify(group)) = file.kind() else {
        return Err(Errno::EINVAL);
    };
    if events & mask::PERMISSION != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & FAN_MARK_FLUSH != 0 {
        let removed = core::mem::take(&mut *group.marks.lock()).len();
        MARKS.fetch_sub(removed, Ordering::Relaxed);
        return Ok(0);
    }
    let node = if path == 0 {
        let directory = thread.files().file(dirfd)?;
        directory.kind().node().ok_or(Errno::EBADF)?
    } else {
        let mut buffer = PathBuffer::uninit();
        let path = read_path(process, path, &mut buffer)?;
        let (root, start) = starting_points(thread, dirfd, path)?;
        let follow = flags & FAN_MARK_DONT_FOLLOW == 0;
        vfs::resolve_as(root, start, path, process.id().get(), follow).map_err(walk_error)?
    };
    if flags & FAN_MARK_ONLYDIR != 0 && !node.is_directory() {
        return Err(Errno::ENOTDIR);
    }
    let watched = match flags & (FAN_MARK_MOUNT | FAN_MARK_FILESYSTEM) {
        0 => Watched::Node(node),
        _ => Watched::FileSystem(file_system_key(node)),
    };
    let mut marks = group.marks.lock();
    match flags & (FAN_MARK_ADD | FAN_MARK_REMOVE) {
        FAN_MARK_ADD => {
            match marks.iter_mut().find(|mark| mark.watched == watched) {
                Some(mark) => mark.mask |= events,
                None => {
                    marks.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
                    marks.push(Mark {
                        watched,
                        mask: events,
                    });
                    MARKS.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(0)
        }
        FAN_MARK_REMOVE => {
            let index = marks
                .iter()
                .position(|mark| mark.watched == watched)
                .ok_or(Errno::ENOENT)?;
            marks[index].mask &= !events;
            if marks[index].mask & !(mask::ONDIR | mask::EVENT_ON_CHILD) == 0 {
                marks.swap_remove(index);
                MARKS.fetch_sub(1, Ordering::Relaxed);
            }
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// A read of the group: as many whole records as fit in `room` bytes, each with a new
/// descriptor on its file in the reader's table; `EAGAIN` with none waiting, `EINVAL` when not
/// even one fits.
pub fn read_records<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    group: &Fanotify,
    room: usize,
) -> Result<Vec<u8>, Errno> {
    if room < METADATA_LENGTH {
        return Err(Errno::EINVAL);
    }
    let mut out = Vec::new();
    while out.len() + METADATA_LENGTH <= room {
        let Some(pending) = group.queue.lock().pop_front() else {
            break;
        };
        let fd: i32 = match pending.node {
            Some(node) => {
                let kind = match node.is_directory() {
                    true => FileKind::Directory(node),
                    false => FileKind::Regular(node),
                };
                let file = Arc::new(OpenFile::new(
                    kind,
                    (group.event_flags & !open_flag::CLOEXEC) | open_flag::RDONLY,
                ));
                let limit = open_file_limit(process);
                let close_on_exec = group.event_flags & open_flag::CLOEXEC != 0;
                match thread.files().lock().open(file, close_on_exec, limit) {
                    Ok(fd) => fd as i32,
                    Err(_) => -1,
                }
            }
            None => -1,
        };
        let mut record = [0u8; METADATA_LENGTH];
        record[..4].copy_from_slice(&(METADATA_LENGTH as u32).to_ne_bytes());
        record[4] = METADATA_VERSION;
        record[6..8].copy_from_slice(&(METADATA_LENGTH as u16).to_ne_bytes());
        record[8..16].copy_from_slice(&pending.mask.to_ne_bytes());
        record[16..20].copy_from_slice(&fd.to_ne_bytes());
        record[20..24].copy_from_slice(&0i32.to_ne_bytes());
        out.extend_from_slice(&record);
    }
    match out.is_empty() {
        true => Err(Errno::EAGAIN),
        false => Ok(out),
    }
}
