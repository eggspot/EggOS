//! File leases and directory notifications (D558): `fcntl`'s `F_SETLEASE` and `F_GETLEASE`, and
//! `F_NOTIFY` (Linux's dnotify). A lease holder is sent `SIGIO` when another process opens the
//! file in a way that conflicts (any open for a write lease, an open for writing for a read
//! lease), and the lease ends; the opener does not wait for it to be given up, as it would on
//! Linux for up to `lease-break-time`. A directory watched with `F_NOTIFY` sends its watcher
//! `SIGIO` when an entry in it is accessed, changed, created, removed, renamed or has its
//! attributes changed, as asked; without `DN_MULTISHOT` the watch then ends.

use super::Errno;
use super::process_table::TABLE;
use super::signal::SIGIO;
use super::signals::Info;
use crate::sync::SpinLock;
use crate::vfs::Node;
use alloc::vec::Vec;
use eggos_hal::Platform;

/// `F_SETLEASE`, `F_GETLEASE` and `F_NOTIFY`.
pub mod command {
    pub const SETLEASE: u32 = 1024;
    pub const GETLEASE: u32 = 1025;
    pub const NOTIFY: u32 = 1026;
}

/// A lease's kind: `F_RDLCK`, `F_WRLCK`, `F_UNLCK`.
const READ: u32 = 0;
const WRITE: u32 = 1;
const NONE: u32 = 2;

/// `F_NOTIFY`'s events and `DN_MULTISHOT`.
pub mod notify {
    pub const ACCESS: u32 = 0x1;
    pub const MODIFY: u32 = 0x2;
    pub const CREATE: u32 = 0x4;
    pub const DELETE: u32 = 0x8;
    pub const RENAME: u32 = 0x10;
    pub const ATTRIB: u32 = 0x20;
    pub const MULTISHOT: u32 = 0x8000_0000;
}

#[derive(Debug, Clone, Copy)]
struct Lease {
    node: Node,
    holder: u32,
    kind: u32,
}

#[derive(Debug, Clone, Copy)]
struct DirectoryWatch {
    directory: Node,
    watcher: u32,
    events: u32,
}

static LEASES: SpinLock<Vec<Lease>> = SpinLock::new(Vec::new());
static WATCHES: SpinLock<Vec<DirectoryWatch>> = SpinLock::new(Vec::new());

/// `F_SETLEASE` on `node` for process `holder`: `kind` `F_RDLCK`, `F_WRLCK` or `F_UNLCK`. As
/// Linux: `EINVAL` for another kind, `EAGAIN` when another process holds a conflicting lease.
pub fn set(node: Node, holder: u32, kind: u32) -> Result<u64, Errno> {
    let mut leases = LEASES.lock();
    leases.retain(|lease| !(lease.node == node && lease.holder == holder));
    match kind {
        NONE => Ok(0),
        READ | WRITE => {
            if leases
                .iter()
                .any(|lease| lease.node == node && (kind == WRITE || lease.kind == WRITE))
            {
                return Err(Errno::EAGAIN);
            }
            leases.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
            leases.push(Lease { node, holder, kind });
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// `F_GETLEASE`: the lease `holder` has on `node`, `F_UNLCK` for none.
pub fn get(node: Node, holder: u32) -> u64 {
    let leases = LEASES.lock();
    u64::from(
        leases
            .iter()
            .find(|lease| lease.node == node && lease.holder == holder)
            .map_or(NONE, |lease| lease.kind),
    )
}

/// Breaks the leases another process than `opener` holds on `node` that an open for writing
/// (`writing`) or reading conflicts with: their holders are sent `SIGIO`.
pub fn opened(node: Node, opener: u32, writing: bool) {
    let broken: Vec<u32> = {
        let mut leases = LEASES.lock();
        if leases.is_empty() {
            return;
        }
        let mut broken = Vec::new();
        leases.retain(|lease| {
            let conflicts = lease.node == node
                && lease.holder != opener
                && (writing || lease.kind == WRITE);
            if conflicts {
                broken.push(lease.holder);
            }
            !conflicts
        });
        broken
    };
    for holder in broken {
        signal(holder);
    }
}

/// `F_NOTIFY` on directory `directory` for process `watcher`: `events` (0 ends the watch).
pub fn watch(directory: Node, watcher: u32, events: u32) -> Result<u64, Errno> {
    if !directory.is_directory() {
        return Err(Errno::ENOTDIR);
    }
    let mut watches = WATCHES.lock();
    watches.retain(|watch| !(watch.directory == directory && watch.watcher == watcher));
    if events & !notify::MULTISHOT != 0 {
        watches.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        watches.push(DirectoryWatch {
            directory,
            watcher,
            events,
        });
    }
    Ok(0)
}

/// Something happened to an entry of `directory`: its watchers that asked for `event` get
/// `SIGIO`, and single-shot watches end.
pub fn changed_in(directory: Node, event: u32) {
    let told: Vec<u32> = {
        let mut watches = WATCHES.lock();
        if watches.is_empty() {
            return;
        }
        let mut told = Vec::new();
        watches.retain(|watch| {
            let wants = watch.directory == directory && watch.events & event != 0;
            if wants {
                told.push(watch.watcher);
            }
            !wants || watch.events & notify::MULTISHOT != 0
        });
        told
    };
    for watcher in told {
        signal(watcher);
    }
}

/// How leases and watches send `SIGIO`: set once the kernel knows its platform
/// ([`set_platform`]), so the file system's paths, which are not generic, can signal.
static SENDER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Lets leases and directory watches signal processes on platform `P`.
pub fn set_platform<P: Platform>() {
    SENDER.store(send_sigio::<P> as fn(u32) as usize, core::sync::atomic::Ordering::Release);
}

fn send_sigio<P: Platform>(pid: u32) {
    let running = TABLE.lock().target(pid).and_then(|target| target.running);
    if let Some(running) = running {
        super::wait::send_to_running::<P>(&running, Info::from_kernel(SIGIO));
    }
}

fn signal(pid: u32) {
    let sender = SENDER.load(core::sync::atomic::Ordering::Acquire);
    if sender == 0 {
        return;
    }
    // SAFETY: only `set_platform` stores a non-zero value, a `fn(u32)`.
    let sender: fn(u32) = unsafe { core::mem::transmute(sender) };
    sender(pid);
}

/// The `F_NOTIFY` event an `inotify` event on an entry of a directory stands for, if any.
pub fn notify_event_of(inotify_event: u32) -> u32 {
    use super::inotify::mask;
    let mut events = 0;
    for (from, to) in [
        (mask::ACCESS, notify::ACCESS),
        (mask::MODIFY, notify::MODIFY),
        (mask::CREATE, notify::CREATE),
        (mask::DELETE, notify::DELETE),
        (mask::MOVED_FROM | mask::MOVED_TO, notify::RENAME),
        (mask::ATTRIB, notify::ATTRIB),
    ] {
        if inotify_event & from != 0 {
            events |= to;
        }
    }
    events
}
