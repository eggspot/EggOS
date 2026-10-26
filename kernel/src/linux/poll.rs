//! Waiting for descriptors (M6, D61): `poll` (x86_64 only) and `ppoll`. Rust's std polls
//! descriptors 0, 1 and 2 at startup to learn whether they are open, and aborts if the call
//! fails for any reason but a few; C programs and event loops poll before reading or writing,
//! and `poll` with no descriptors is a common way to sleep.
//!
//! Descriptors so far name the console (D101): always
//! writable, as the console takes every byte at once, and readable when a read would take
//! input without waiting for more (D100, superseding D61's "never readable"): a whole line in
//! canonical mode, otherwise `VMIN` bytes (at least one; one when `VTIME` is set). Every other
//! descriptor is closed (`POLLNVAL`). A call that finds nothing ready waits until its timeout,
//! or for good without one, unless `ppoll`'s signal mask lets a pending signal interrupt it
//! first (D71); one that asks for the console's input is woken when it becomes readable, and
//! its thread becomes the console's foreground ([`terminal::become_foreground`]).

use super::descriptors::{Descriptors, FileKind, OpenFile};
use super::terminal::{self, Waited};
use super::thread::{self, LinuxThread};
use super::{Errno, ends_in_user_space, limits, signals};
use crate::console;
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;
use eggos_hal::user::UserContext;

/// The events of a `struct pollfd` (`POLL*`), the generic values, which x86_64 shares.
pub mod event {
    /// Data to read.
    pub const IN: u16 = 0x1;
    /// Urgent data to read.
    pub const PRI: u16 = 0x2;
    /// Room to write.
    pub const OUT: u16 = 0x4;
    /// An error (reported whether asked for or not).
    pub const ERR: u16 = 0x8;
    /// The other end hung up (reported whether asked for or not).
    pub const HUP: u16 = 0x10;
    /// The descriptor is not open (reported whether asked for or not).
    pub const NVAL: u16 = 0x20;
    /// Normal data to read: as [`IN`].
    pub const RDNORM: u16 = 0x40;
    /// Priority data to read.
    pub const RDBAND: u16 = 0x80;
    /// Room to write normal data: as [`OUT`].
    pub const WRNORM: u16 = 0x100;
    /// Room to write priority data.
    pub const WRBAND: u16 = 0x200;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's POLL* values for this architecture (MIPS and SPARC differ)");
}

/// The size of a `struct pollfd`: an `int` descriptor, then `short` requested and returned
/// events.
const POLLFD_SIZE: usize = 8;

const _: () = assert!(limits::MAX_OPEN_FILES * POLLFD_SIZE as u64 <= usize::MAX as u64);

/// How many `struct pollfd`s the calls read onto the kernel stack: Rust's std polls three.
/// More go through the heap.
const STACK_POLLFDS: usize = 8;

/// The size of a `struct timespec`: seconds and nanoseconds, both 64-bit.
const TIMESPEC_SIZE: usize = 16;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

fn events(file: Option<&FileKind>, requested: u16, readable: bool) -> u16 {
    match file {
        None => event::NVAL,
        Some(FileKind::Console) if readable => {
            requested & (event::OUT | event::WRNORM | event::IN | event::RDNORM)
        }
        Some(FileKind::Console) => requested & (event::OUT | event::WRNORM),
    }
}

const INPUT_EVENTS: u16 = event::IN | event::RDNORM;

fn pollfd(entry: &[u8; POLLFD_SIZE]) -> (i32, u16) {
    (
        i32::from_ne_bytes(entry[..4].try_into().unwrap()),
        u16::from_ne_bytes(entry[4..6].try_into().unwrap()),
    )
}

fn fill_in_events(entries: &mut [[u8; POLLFD_SIZE]], files: &Descriptors, readable: bool) -> u64 {
    let mut ready = 0;
    for entry in entries {
        let (fd, requested) = pollfd(entry);
        let returned = match fd {
            0.. => {
                let file = files.file(fd as u64).ok();
                events(file.as_deref().map(OpenFile::kind), requested, readable)
            }
            _ => 0,
        };
        entry[6..].copy_from_slice(&returned.to_ne_bytes());
        ready += u64::from(returned != 0);
    }
    ready
}

/// When a wait ends, in clock ticks ([`Platform::clock_ticks`]); `u64::MAX` for never.
pub type Deadline = u64;

/// What a wait for descriptors came to.
enum Polled {
    /// How many descriptors report events, after the wait if none did at first.
    Ready(u64),
    /// None did, and a signal was due, so the wait did not start.
    Interrupted,
}

/// The deadline of a wait for `nanoseconds`, rounded up so it is never reached early: `None`
/// for none, no wait at all.
pub fn deadline_in<P: Platform>(nanoseconds: u64) -> Option<Deadline> {
    (nanoseconds != 0).then(|| {
        let ticks = mul_div_ceil(nanoseconds, P::clock_frequency(), NANOSECONDS_PER_SECOND);
        P::clock_ticks().saturating_add(ticks)
    })
}

/// `poll(fds, nfds, timeout)`: as [`ppoll`] with a timeout of `timeout` milliseconds (an
/// `int`: negative waits for good, 0 not at all) and no signal mask, failing with `EINTR` when
/// a signal is due, which is delivered after it. x86_64 only: AArch64 and RISC-V have only
/// `ppoll`, which their C libraries' `poll` calls.
#[cfg(target_arch = "x86_64")]
pub fn poll<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fds: u64,
    count: u64,
    timeout: u64,
) -> Result<u64, Errno> {
    let milliseconds = timeout as u32 as i32;
    let deadline = if milliseconds < 0 {
        Some(Deadline::MAX)
    } else {
        deadline_in::<P>(milliseconds as u64 * 1_000_000)
    };
    match poll_fds(process, thread, fds, count, deadline)? {
        Polled::Ready(ready) => Ok(ready),
        Polled::Interrupted => Err(Errno::EINTR),
    }
}

/// `ppoll(fds, nfds, timeout, sigmask, sigsetsize)`, with the program's registers `context`:
/// reports, in each `struct pollfd` of the `nfds` at `fds`, the events its descriptor has of those
/// asked for (see `events`), and returns how many report any. If none does, it first waits for one
/// until the `struct timespec` at `timeout` has passed, for good if that is null, not at all if it
/// is zero. With a `sigmask`, the signals in that set are blocked instead of the thread's during
/// the call (D71). If none reports events and a signal is due, before or during the wait (another
/// thread sent it, D95), the call delivers it and, once a handler starts, fails with `EINTR`, the
/// handler's frame saving the thread's own mask, as `rt_sigsuspend` does (D70); a signal its action
/// ignores is taken and the call goes on, as Linux restarts it, unless the time left could not be
/// written back (below): then it fails with `EINTR`, as Linux's does, since it cannot restart with
/// the time left. Otherwise the thread's mask is back when it returns, and a signal the call's mask
/// unblocked stays pending. A timeout that is not zero gets the time not slept written back once
/// the signal mask passed its checks, whatever the result: zero after a wait that ended with it,
/// about all of it when something was ready at once (D62); one too long to count in nanoseconds is
/// written back as it was. Linux's system call does that (its C libraries hide it), and ignores a
/// timeout the program cannot write, as here, but for that `EINTR`. As Linux, in this order:
/// `EFAULT` if `timeout` is not the program's memory; `EINVAL` for negative seconds or nanoseconds
/// outside 0 to 999999999; with a `sigmask`, `EINVAL` if `sigsetsize` is not 8 and `EFAULT` if the
/// mask is not the program's memory; `EINVAL` for more descriptors than the open-file limit
/// (`RLIMIT_NOFILE`'s soft limit); `EFAULT` if the array is not the program's writable memory;
/// `ENOMEM` if there is no memory to copy it. `nfds` is an `unsigned int`: the register's upper
/// half is ignored. Sets the call's result; breaks when a signal ends the process.
pub fn ppoll<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fds, count, timeout, mask, set_size, _] = context.syscall_arguments();
    let Wait {
        requested_timeout,
        deadline,
        mask,
    } = match checked_wait(process, timeout, mask, set_size) {
        Ok(wait) => wait,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    let thread_mask = mask.map(|mask| signals::replace_blocked(process, thread, mask));
    loop {
        let polled = poll_fds(process, thread, fds, count, deadline);
        let time_left_written =
            write_back_time_left::<P>(process, timeout, &requested_timeout, deadline);
        let result = match polled {
            Ok(Polled::Interrupted) => {
                context.set_syscall_result(Errno::EINTR.result());
                if signals::deliver_restoring_mask(process, thread, context, thread_mask)? {
                    return ControlFlow::Continue(());
                }
                if time_left_written {
                    continue;
                }
                Err(Errno::EINTR)
            }
            Ok(Polled::Ready(ready)) => Ok(ready),
            Err(errno) => Err(errno),
        };
        if let Some(thread_mask) = thread_mask {
            signals::replace_blocked(process, thread, thread_mask);
        }
        context.set_syscall_result(result.unwrap_or_else(Errno::result));
        return ControlFlow::Continue(());
    }
}

/// How `ppoll` waits, from its timeout and signal mask.
struct Wait {
    /// The timeout the time left is written back to: `None` for a null or zero one.
    requested_timeout: Option<RequestedTimeout>,
    /// When the wait ends: `None` for no wait.
    deadline: Option<Deadline>,
    /// The signals blocked during the call instead of the thread's, if any.
    mask: Option<u64>,
}

/// `ppoll`'s checks of its timeout and signal mask, in Linux's order (see [`ppoll`]).
fn checked_wait<P: Platform>(
    process: &Process<P>,
    timeout: u64,
    mask: u64,
    set_size: u64,
) -> Result<Wait, Errno> {
    let mut requested_timeout = None;
    let deadline = if timeout == 0 {
        Some(Deadline::MAX)
    } else {
        let as_written = read_timespec(process, timeout)?;
        let nanoseconds = parse_timeout(as_written)?;
        requested_timeout = (nanoseconds != Some(0)).then_some(RequestedTimeout {
            as_written,
            nanoseconds,
        });
        deadline_in::<P>(nanoseconds.unwrap_or(u64::MAX))
    };
    let mask = if mask == 0 {
        None
    } else {
        Some(signals::read_set(process, mask, set_size)?)
    };
    Ok(Wait {
        requested_timeout,
        deadline,
        mask,
    })
}

/// A timeout that is not zero, which `ppoll` writes the time left back to.
struct RequestedTimeout {
    /// The `struct timespec` as the program wrote it.
    as_written: [u8; TIMESPEC_SIZE],
    /// It in nanoseconds: `None` if it is too long to count.
    nanoseconds: Option<u64>,
}

/// Writes the time left until `deadline` back to the `struct timespec` at `timeout`, at most the
/// time asked for, as the deadline was rounded up, and returns whether it could: `false` for a
/// timeout the program cannot write, which Linux ignores unless a signal interrupted the call.
/// A null or zero timeout has nothing to write back. One too long to count in nanoseconds (over
/// 584 years) or to reach in clock ticks is written back as it was: about what is left of it.
fn write_back_time_left<P: Platform>(
    process: &Process<P>,
    timeout: u64,
    requested_timeout: &Option<RequestedTimeout>,
    deadline: Option<Deadline>,
) -> bool {
    let Some(RequestedTimeout {
        as_written,
        nanoseconds,
    }) = requested_timeout
    else {
        return true;
    };
    let bytes = match (nanoseconds, deadline) {
        (Some(nanoseconds), Some(deadline)) if deadline != Deadline::MAX => {
            let left = deadline.saturating_sub(P::clock_ticks());
            let left =
                mul_div_ceil(left, NANOSECONDS_PER_SECOND, P::clock_frequency()).min(*nanoseconds);
            let mut bytes = [0; TIMESPEC_SIZE];
            bytes[..8].copy_from_slice(&(left / NANOSECONDS_PER_SECOND).to_ne_bytes());
            bytes[8..].copy_from_slice(&(left % NANOSECONDS_PER_SECOND).to_ne_bytes());
            bytes
        }
        _ => *as_written,
    };
    process.write_user_memory(timeout, &bytes).is_ok()
}

/// The `struct timespec` at `address`, a timeout, in nanoseconds, or `None` if it is too long
/// for a `u64` (over 584 years). As Linux: `EFAULT` if it is not the program's memory, `EINVAL`
/// for negative seconds or nanoseconds outside 0 to 999999999.
pub fn read_timeout<P: Platform>(process: &Process<P>, address: u64) -> Result<Option<u64>, Errno> {
    parse_timeout(read_timespec(process, address)?)
}

fn read_timespec<P: Platform>(
    process: &Process<P>,
    address: u64,
) -> Result<[u8; TIMESPEC_SIZE], Errno> {
    let mut bytes = [0; TIMESPEC_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes)
}

fn parse_timeout(bytes: [u8; TIMESPEC_SIZE]) -> Result<Option<u64>, Errno> {
    let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let nanoseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..NANOSECONDS_PER_SECOND as i64).contains(&nanoseconds) {
        return Err(Errno::EINVAL);
    }
    Ok((seconds as u64)
        .checked_mul(NANOSECONDS_PER_SECOND)
        .and_then(|whole| whole.checked_add(nanoseconds as u64)))
}

/// What both calls share once the timeout is a `deadline` (`None` for no wait): checks
/// `count` against the open-file limit, copies the `struct pollfd`s at `fds` in once, fills in
/// their returned events and copies them back, waiting until `deadline` if none reports any.
/// A signal due to `thread`, the caller, before or during the wait ([`signals::is_due`]: one
/// another thread sent, or one the call's mask unblocked) interrupts it, as on Linux even
/// with no deadline.
fn poll_fds<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fds: u64,
    count: u64,
    deadline: Option<Deadline>,
) -> Result<Polled, Errno> {
    let count = u64::from(count as u32);
    let open_files = process
        .credentials_and_limits()
        .lock()
        .limits
        .get(limits::resource::NOFILE)
        .map_or(u64::MAX, |limit| limit.soft);
    if count > open_files {
        return Err(Errno::EINVAL);
    }
    let bytes = count as usize * POLLFD_SIZE;
    if !ends_in_user_space::<P>(fds, bytes as u64) {
        return Err(Errno::EFAULT);
    }
    let mut on_stack = [0u8; STACK_POLLFDS * POLLFD_SIZE];
    let mut on_heap = alloc::vec::Vec::new();
    let array = if bytes <= on_stack.len() {
        &mut on_stack[..bytes]
    } else {
        on_heap
            .try_reserve_exact(bytes)
            .map_err(|_| Errno::ENOMEM)?;
        on_heap.resize(bytes, 0);
        &mut on_heap[..]
    };
    process
        .read_user_memory(fds, array)
        .map_err(|_| Errno::EFAULT)?;
    let entries = array.as_chunks_mut::<POLLFD_SIZE>().0;
    let watches_input = entries.iter().any(|entry| {
        let (fd, requested) = pollfd(entry);
        fd >= 0
            && requested & INPUT_EVENTS != 0
            && thread
                .files()
                .file(fd as u64)
                .is_ok_and(|file| *file.kind() == FileKind::Console)
    });
    if watches_input {
        terminal::become_foreground(process, thread);
    }
    let readable = watches_input && console::input_ready();
    let mut ready = fill_in_events(entries, thread.files(), readable);
    let mut interrupted = ready == 0 && signals::is_due(thread);
    if !interrupted && let Some(deadline) = deadline.filter(|_| ready == 0) {
        (interrupted, ready) = match watches_input {
            true => wait_for_input::<P>(process, thread, entries, deadline),
            false => (sleep::<P>(process, thread, deadline), 0),
        };
    }
    process
        .write_user_memory(fds, array)
        .map_err(|_| Errno::EFAULT)?;
    Ok(if interrupted {
        Polled::Interrupted
    } else {
        Polled::Ready(ready)
    })
}

fn sleep<P: Platform>(process: &Process<P>, thread: &mut LinuxThread, deadline: Deadline) -> bool {
    while P::clock_ticks() < deadline {
        if thread::sleep_until::<P>(process, thread, deadline).is_break() {
            return false;
        }
        if signals::is_due(thread) {
            return true;
        }
    }
    false
}

fn wait_for_input<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    entries: &mut [[u8; POLLFD_SIZE]],
    deadline: Deadline,
) -> (bool, u64) {
    loop {
        match terminal::wait_for_input::<P>(process, thread, deadline, console::input_ready) {
            ControlFlow::Break(()) | ControlFlow::Continue(Waited::TimedOut) => return (false, 0),
            ControlFlow::Continue(Waited::Signalled) => return (true, 0),
            ControlFlow::Continue(Waited::Ready) => {
                let ready = fill_in_events(entries, thread.files(), true);
                if ready > 0 {
                    return (false, ready);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_descriptors_are_skipped_and_closed_ones_report_pollnval() {
        let files = Descriptors::with_console_streams();
        assert!(files.lock().remove(1).is_some());
        let mut entries = [-1, 1, 3, i32::MIN, 0].map(|fd| {
            let mut entry = [0; POLLFD_SIZE];
            entry[..4].copy_from_slice(&fd.to_ne_bytes());
            entry[4..6].copy_from_slice(&event::OUT.to_ne_bytes());
            entry
        });
        assert_eq!(fill_in_events(&mut entries, &files, false), 3);
        let returned = entries.map(|entry| u16::from_ne_bytes([entry[6], entry[7]]));
        assert_eq!(returned, [0, event::NVAL, event::NVAL, 0, event::OUT]);
    }

    #[test]
    fn closed_descriptors_report_pollnval_whatever_was_asked() {
        assert_eq!(events(None, 0, true), event::NVAL);
        assert_eq!(events(None, event::IN, true), event::NVAL);
    }

    #[test]
    fn the_console_has_room_to_write_when_asked_and_data_to_read_only_when_readable() {
        let all = u16::MAX;
        let console = Some(&FileKind::Console);
        assert_eq!(events(console, 0, true), 0);
        assert_eq!(
            events(console, event::IN | event::RDNORM | event::PRI, false),
            0
        );
        assert_eq!(
            events(console, event::IN | event::RDNORM | event::PRI, true),
            event::IN | event::RDNORM
        );
        assert_eq!(events(console, event::OUT, true), event::OUT);
        assert_eq!(events(console, all, false), event::OUT | event::WRNORM);
        assert_eq!(
            events(console, all, true),
            event::OUT | event::WRNORM | event::IN | event::RDNORM
        );
    }
}
