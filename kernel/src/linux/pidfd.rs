//! Process file descriptors (D557): `pidfd_open` gives a descriptor that names a process, which
//! reads as ready (`POLLIN`) once the process has ended, and through which `pidfd_send_signal`
//! signals it and `pidfd_getfd` takes a copy of one of its descriptors. A pidfd names the process
//! by its id, which the table keeps until the process is reaped.

use super::Errno;
use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::poll::{WakeList, event};
use super::process_table::TABLE;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::sync::SpinLock;
use alloc::sync::Arc;
use eggos_hal::Platform;

/// A process named by a descriptor.
#[derive(Debug)]
pub struct PidFd {
    pid: u32,
    wake_list: Arc<WakeList>,
}

/// Woken whenever a process ends, so a wait on a pidfd looks again.
static ENDED: SpinLock<Option<Arc<WakeList>>> = SpinLock::new(None);

fn ended_list() -> Arc<WakeList> {
    ENDED.lock().get_or_insert_with(WakeList::shared).clone()
}

/// Wakes the waits on pidfds, as a process ends.
pub fn process_ended() {
    if let Some(list) = ENDED.lock().as_ref() {
        list.wake();
    }
}

impl PidFd {
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// The events a wait for `requested` sees: `POLLIN` once the process has ended or is gone.
    pub fn events(&self, requested: u16) -> u16 {
        let running = TABLE
            .lock()
            .target(self.pid)
            .is_some_and(|target| target.running.is_some());
        match running {
            true => 0,
            false => requested & event::IN,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

const PIDFD_NONBLOCK: u64 = open_flag::NONBLOCK as u64;

/// `pidfd_open(pid, flags)`: a descriptor naming process `pid`, close-on-exec. As Linux: `EINVAL`
/// for flags other than `PIDFD_NONBLOCK` or a pid of 0 or less, `ESRCH` when no such process runs.
pub fn pidfd_open<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pid: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags & !PIDFD_NONBLOCK != 0 || pid as i32 <= 0 {
        return Err(Errno::EINVAL);
    }
    let pid = pid as u32;
    if TABLE.lock().target(pid).and_then(|target| target.running).is_none() {
        return Err(Errno::ESRCH);
    }
    let handle = Arc::new(PidFd {
        pid,
        wake_list: ended_list(),
    });
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::PidFd(handle)),
        open_flag::RDWR | (flags as u32 & open_flag::NONBLOCK),
    ));
    let limit = open_file_limit(process);
    let fd = thread.files().lock().open(file, true, limit)?;
    Ok(u64::from(fd))
}

fn pid_of(thread: &LinuxThread, fd: u64) -> Result<u32, Errno> {
    let file = thread.files().file(fd)?;
    match file.kind() {
        FileKind::Event(EventFile::PidFd(handle)) => Ok(handle.pid()),
        _ => Err(Errno::EBADF),
    }
}

/// `pidfd_send_signal(pidfd, signal, info, flags)`: sends `signal` to the process `pidfd` names,
/// as `kill` would with its id (a given `siginfo_t` is not used: the signal reads as `kill`'s).
/// As Linux: `EINVAL` for flags, `EBADF` for a descriptor that is not a pidfd, `ESRCH` once the
/// process has ended.
pub fn pidfd_send_signal<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    pidfd: u64,
    signal: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    let pid = pid_of(thread, pidfd)?;
    if TABLE.lock().target(pid).and_then(|target| target.running).is_none() {
        return Err(Errno::ESRCH);
    }
    super::signals::kill(process, thread, u64::from(pid), signal)
}

/// `pidfd_getfd(pidfd, targetfd, flags)`: a new descriptor, close-on-exec, for the open file
/// `targetfd` is in the process `pidfd` names. As Linux: `EINVAL` for flags, `EPERM` unless the
/// caller is root or has the process's real user id, `ESRCH` once it ended, `EBADF` for a
/// descriptor it does not have.
pub fn pidfd_getfd<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    pidfd: u64,
    target: u64,
    flags: u64,
) -> Result<u64, Errno> {
    if flags != 0 {
        return Err(Errno::EINVAL);
    }
    let pid = pid_of(thread, pidfd)?;
    let running = {
        let table = TABLE.lock();
        let found = table.target(pid).ok_or(Errno::ESRCH)?;
        found.running.ok_or(Errno::ESRCH)?
    };
    let owner = running
        .process
        .upgrade()
        .and_then(|other| other.downcast::<Process<P>>().ok())
        .map(|other| {
            other
                .credentials_and_limits()
                .lock()
                .credentials
                .ids(super::credentials::Kind::User)
                .real
        })
        .ok_or(Errno::ESRCH)?;
    let caller = process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(super::credentials::Kind::User);
    if caller.effective != 0 && caller.real != owner {
        return Err(Errno::EPERM);
    }
    let target = u32::try_from(target).map_err(|_| Errno::EBADF)?;
    let file = running.files.file(target).ok_or(Errno::EBADF)?;
    let limit = open_file_limit(process);
    let fd = thread.files().lock().open(file, true, limit)?;
    Ok(u64::from(fd))
}
