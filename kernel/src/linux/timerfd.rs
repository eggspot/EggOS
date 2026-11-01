//! `timerfd_create`, `timerfd_settime` and `timerfd_gettime` (timerfd_create(2)): a timer that
//! is read as a file, as event loops (GLib, Qt, SDL, systemd) run their timeouts with
//! `epoll`. A timer counts its expirations on its clock (the monotonic clock for
//! `CLOCK_MONOTONIC` and `CLOCK_BOOTTIME`, as EggOS never suspends; the wall clock for
//! `CLOCK_REALTIME`); a read takes the count since the last read or setting, 8 bytes, waiting
//! for the next expiry while there is none, unless `O_NONBLOCK` (`EAGAIN`). `poll` reports it
//! readable while expirations wait. Nothing runs when a timer expires: the count is worked out
//! from the clock whenever it is looked at, and a wait for a timer ends at its next expiry
//! ([`TimerFd::next_tick`]) to look again. Setting it wakes its waiters ([`WakeList`]) so they
//! wait for the new expiry.
//!
//! `TFD_TIMER_CANCEL_ON_SET` is refused with `EINVAL`: a timer is not told when the wall clock
//! is set. The alarm clocks are taken by a privileged process as their plain clocks, since
//! EggOS never suspends, and are `EPERM` for others, as Linux without `CAP_WAKE_ALARM`.

use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::poll::{self, Deadline, INPUT_EVENTS, WakeList};
use super::terminal::Waited;
use super::thread::LinuxThread;
use super::time::{self, clock};
use super::{Errno, Iovecs};
use crate::process::Process;
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// `timerfd_create`'s flags (`TFD_*`).
pub mod flag {
    use super::open_flag;

    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = open_flag::CLOEXEC;
    /// Reads that would wait fail with `EAGAIN` instead.
    pub const NONBLOCK: u32 = open_flag::NONBLOCK;
}

/// `timerfd_settime`'s flags (`TFD_TIMER_*`).
pub mod setting {
    /// The expiry is a time on the timer's clock, not a duration from now.
    pub const ABSTIME: u32 = 1;
    /// Cancel the timer when the wall clock is set: refused (see the module documentation).
    pub const CANCEL_ON_SET: u32 = 2;
}

/// The size of a `struct itimerspec`: the interval, then the value, each a `struct timespec`.
pub const ITIMERSPEC_SIZE: usize = 32;

const COUNT_SIZE: u64 = 8;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// A timer: see the module documentation.
#[derive(Debug)]
pub struct TimerFd {
    wall: bool,
    state: LeafSpinLock<TimerState>,
    wake_list: Arc<WakeList>,
}

#[derive(Debug, Default, Clone, Copy)]
struct TimerState {
    expiry: Option<u64>,
    interval: u64,
    unread: u64,
    counted: u64,
}

impl TimerState {
    fn count_until(&mut self, now: u64) {
        let Some(expiry) = self.expiry.filter(|&expiry| now >= expiry) else {
            return;
        };
        let expiries = match self.interval {
            0 => {
                self.expiry = None;
                1
            }
            interval => {
                let expiries = (now - expiry) / interval + 1;
                self.expiry = expiry.checked_add(expiries.saturating_mul(interval));
                expiries
            }
        };
        self.unread = self.unread.saturating_add(expiries);
        self.counted = self.counted.wrapping_add(expiries);
    }
}

impl TimerFd {
    /// A disarmed timer on the wall clock if `wall`, else on the monotonic clock.
    pub fn new(wall: bool) -> Arc<Self> {
        Arc::new(Self {
            wall,
            state: LeafSpinLock::new_leaf(TimerState::default()),
            wake_list: WakeList::shared(),
        })
    }

    /// The threads and `epoll` instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    fn now<P: Platform>(&self) -> u64 {
        match self.wall {
            true => crate::wall_clock::nanoseconds::<P>(),
            false => time::monotonic_nanoseconds::<P>(),
        }
    }

    fn counted<P: Platform>(&self) -> TimerState {
        let now = self.now::<P>();
        let mut state = self.state.lock();
        state.count_until(now);
        *state
    }

    /// The `poll` events of `requested` it reports: readable while expiries wait.
    pub fn events<P: Platform>(&self, requested: u16) -> u16 {
        match self.counted::<P>().unread {
            0 => 0,
            _ => requested & INPUT_EVENTS,
        }
    }

    /// The expiries ever counted: changes with each one, for edge-triggered `epoll`.
    pub fn counted_expiries<P: Platform>(&self) -> u64 {
        self.counted::<P>().counted
    }

    /// The clock tick of its next expiry, if it is armed: a wait for it ends there.
    pub fn next_tick<P: Platform>(&self) -> Option<Deadline> {
        let expiry = self.state.lock().expiry?;
        let monotonic = match self.wall {
            true => crate::wall_clock::monotonic_at(expiry),
            false => expiry,
        };
        Some(time::tick_at::<P>(monotonic))
    }

    /// What a read takes: the expiries since the last read or setting, `None` if there are
    /// none.
    pub fn take<P: Platform>(&self) -> Option<u64> {
        let now = self.now::<P>();
        let mut state = self.state.lock();
        state.count_until(now);
        match state.unread {
            0 => None,
            unread => {
                state.unread = 0;
                Some(unread)
            }
        }
    }

    /// Its setting as `timerfd_gettime` reports it: the interval and the time to the next
    /// expiry, in nanoseconds, both 0 while disarmed.
    pub fn setting<P: Platform>(&self) -> (u64, u64) {
        let now = self.now::<P>();
        let state = self.counted::<P>();
        let left = state
            .expiry
            .map_or(0, |expiry| expiry.saturating_sub(now).max(1));
        (state.interval, left)
    }

    /// Arms it to expire at `expiry` on its clock and every `interval` nanoseconds after (0
    /// for once), or disarms it with `None`, forgetting the expiries not read; returns its
    /// setting before ([`Self::setting`]) and wakes its waiters.
    pub fn set<P: Platform>(&self, expiry: Option<u64>, interval: u64) -> (u64, u64) {
        let now = self.now::<P>();
        let before = {
            let mut state = self.state.lock();
            state.count_until(now);
            let left = state
                .expiry
                .map_or(0, |expiry| expiry.saturating_sub(now).max(1));
            let before = (state.interval, left);
            state.expiry = expiry;
            state.interval = if expiry.is_some() { interval } else { 0 };
            state.unread = 0;
            before
        };
        self.wake_list.wake();
        before
    }
}

impl Drop for TimerFd {
    fn drop(&mut self) {
        self.wake_list.wake();
    }
}

/// `timerfd_create(clockid, flags)`: a new disarmed timer on clock `clockid`, open for reading,
/// its descriptor returned. As Linux: `EINVAL` for flags other than `TFD_CLOEXEC` and
/// `TFD_NONBLOCK` or a clock other than `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, `CLOCK_BOOTTIME`
/// and their alarm clocks, `EPERM` for an alarm clock unless the process is privileged,
/// `EMFILE` without a free descriptor, `ENOMEM` without memory.
pub fn timerfd_create<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    clock_id: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !(flag::CLOEXEC | flag::NONBLOCK) != 0 {
        return Err(Errno::EINVAL);
    }
    let (wall, alarm) = match clock_id as u32 as i32 {
        clock::REALTIME => (true, false),
        clock::MONOTONIC | clock::BOOTTIME => (false, false),
        clock::REALTIME_ALARM => (true, true),
        clock::BOOTTIME_ALARM => (false, true),
        _ => return Err(Errno::EINVAL),
    };
    if alarm && !time::is_privileged(process) {
        return Err(Errno::EPERM);
    }
    if !crate::heap::may_grow_for_user(size_of::<TimerFd>() + size_of::<WakeList>()) {
        return Err(Errno::ENOMEM);
    }
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::TimerFd(TimerFd::new(wall))),
        open_flag::RDONLY | flags & flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & flag::CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

fn timer_of(thread: &LinuxThread, fd: u64) -> Result<Arc<TimerFd>, Errno> {
    match thread.files().file(u64::from(fd as u32))?.kind() {
        FileKind::Event(EventFile::TimerFd(timer)) => Ok(Arc::clone(timer)),
        _ => Err(Errno::EINVAL),
    }
}

fn duration(bytes: &[u8]) -> Result<u64, Errno> {
    let timespec = bytes.try_into().map_err(|_| Errno::EINVAL)?;
    Ok(poll::parse_timeout(timespec)?.unwrap_or(u64::MAX))
}

fn itimerspec(interval: u64, value: u64) -> [u8; ITIMERSPEC_SIZE] {
    let mut bytes = [0u8; ITIMERSPEC_SIZE];
    for (index, nanoseconds) in [interval, value].into_iter().enumerate() {
        let at = index * 16;
        bytes[at..at + 8].copy_from_slice(&(nanoseconds / NANOSECONDS_PER_SECOND).to_ne_bytes());
        bytes[at + 8..at + 16]
            .copy_from_slice(&(nanoseconds % NANOSECONDS_PER_SECOND).to_ne_bytes());
    }
    bytes
}

/// `timerfd_settime(fd, flags, new_value, old_value)`: arms the timer `fd` names with the
/// `struct itimerspec` at `new_value`, its value the first expiry (a time on the timer's clock
/// with `TFD_TIMER_ABSTIME`, otherwise from now; zero disarms it) and its interval the time
/// between the next ones (zero for none), forgetting the expiries not read, and writes its
/// setting before to `old_value` unless that is null, as `timerfd_gettime` would have. As
/// Linux, in this order: `EFAULT` if `new_value` is not the program's memory; `EINVAL` for
/// unknown flags or `TFD_TIMER_CANCEL_ON_SET` (see the module documentation), or a negative
/// time or nanoseconds outside 0 to 999999999; `EBADF` if `fd` is not open, `EINVAL` if it is
/// not a timer; `EFAULT` if `old_value` is not the program's writable memory, the timer set.
pub fn timerfd_settime<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    flags: u64,
    new_value: u64,
    old_value: u64,
) -> Result<u64, Errno> {
    let mut bytes = [0u8; ITIMERSPEC_SIZE];
    process
        .read_user_memory(new_value, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let flags = flags as u32;
    if flags & !setting::ABSTIME != 0 {
        return Err(Errno::EINVAL);
    }
    let interval = duration(&bytes[..16])?;
    let value = duration(&bytes[16..])?;
    let timer = timer_of(thread, fd)?;
    let expiry = match (value, flags & setting::ABSTIME != 0) {
        (0, _) => None,
        (value, true) => Some(value),
        (value, false) => Some(timer.now::<P>().saturating_add(value)),
    };
    let (old_interval, old_left) = timer.set::<P>(expiry, interval);
    if old_value != 0 {
        process
            .write_user_memory(old_value, &itimerspec(old_interval, old_left))
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

/// `timerfd_gettime(fd, curr_value)`: writes the setting of the timer `fd` names to the
/// `struct itimerspec` at `curr_value`: its interval and the time to its next expiry, both
/// zero while disarmed. As Linux: `EBADF` if `fd` is not open, `EINVAL` if it is not a timer,
/// `EFAULT` if `curr_value` is not the program's writable memory.
pub fn timerfd_gettime<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    curr_value: u64,
) -> Result<u64, Errno> {
    let (interval, left) = timer_of(thread, fd)?.setting::<P>();
    process
        .write_user_memory(curr_value, &itimerspec(interval, left))
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

/// `read` or `readv` of `buffers` from `timer`, whose description is `file`: the expiries
/// since the last read or setting, 8 bytes, waiting for one unless `O_NONBLOCK` (`EAGAIN`);
/// `EINVAL` for fewer than 8 bytes. A signal interrupts the wait as it does a pipe's. Sets the
/// call's result.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    timer: &TimerFd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    if buffers.total() < COUNT_SIZE {
        context.set_syscall_result(Errno::EINVAL.result());
        return ControlFlow::Continue(());
    }
    poll::blocking(process, thread, context, |process, thread| {
        loop {
            if let Some(expiries) = timer.take::<P>() {
                let copied = poll::copy_to_buffers(process, buffers, 0, &expiries.to_ne_bytes());
                return ControlFlow::Continue(Some(copied.map(|()| COUNT_SIZE)));
            }
            if file.is_nonblocking() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            let lists = [timer.wake_list()];
            let deadline = timer.next_tick::<P>().unwrap_or(u64::MAX);
            let ready = || timer.events::<P>(INPUT_EVENTS) != 0;
            if poll::wait_for(process, thread, &lists, deadline, ready)? == Waited::Signalled {
                return ControlFlow::Continue(None);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timer_counts_its_expiries_from_the_clock_when_looked_at() {
        let mut state = TimerState {
            expiry: Some(100),
            interval: 10,
            ..TimerState::default()
        };
        state.count_until(99);
        assert_eq!(state.unread, 0);
        state.count_until(100);
        assert_eq!((state.unread, state.expiry), (1, Some(110)));
        state.count_until(135);
        assert_eq!((state.unread, state.expiry), (4, Some(140)));
        let mut once = TimerState {
            expiry: Some(5),
            ..TimerState::default()
        };
        once.count_until(1000);
        assert_eq!((once.unread, once.expiry, once.counted), (1, None, 1));
        once.count_until(2000);
        assert_eq!(once.unread, 1);
    }

    #[test]
    fn an_itimerspec_holds_the_interval_then_the_value() {
        let bytes = itimerspec(1_500_000_000, 20);
        assert_eq!(duration(&bytes[..16]), Ok(1_500_000_000));
        assert_eq!(duration(&bytes[16..]), Ok(20));
        assert_eq!(&bytes[..8], &1u64.to_ne_bytes());
    }
}
