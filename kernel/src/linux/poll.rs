//! Waiting for descriptors (M6, D61): `poll` (x86_64 only) and `ppoll`. Rust's std polls
//! descriptors 0, 1 and 2 at startup to learn whether they are open, and aborts if the call
//! fails for any reason but a few; C programs and event loops poll before reading or writing,
//! and `poll` with no descriptors is a common way to sleep.
//!
//! The only descriptors so far are the console's, 0 to 2 ([`super::is_console`]): always
//! writable, as the console takes every byte at once, and never readable, as there is no
//! console input yet (M8). Every other descriptor is closed (`POLLNVAL`). So nothing becomes
//! ready during a wait: a call that finds nothing ready sleeps until its timeout, or for good
//! without one, as Linux does when no signal comes. Waking a poller early comes with the
//! first descriptor whose readiness changes (console input, pipes).

use super::{Errno, ends_in_user_space, is_console, limits, signals};
use crate::process::Process;
use crate::thread;
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;

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

/// How many `struct pollfd`s the calls read onto the kernel stack: Rust's std polls three.
/// More go through the heap.
const STACK_POLLFDS: usize = 8;

/// The size of a `struct timespec`: seconds and nanoseconds, both 64-bit.
const TIMESPEC_SIZE: usize = 16;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// The events descriptor `fd` (an `int`) reports when `requested` are asked for: none for a
/// negative one, which the program uses to skip an entry; `POLLNVAL` for one that is not open;
/// for the console's, room to write when asked for.
fn events(fd: i32, requested: u16) -> u16 {
    if fd < 0 {
        0
    } else if !is_console(fd as u64) {
        event::NVAL
    } else {
        requested & (event::OUT | event::WRNORM)
    }
}

/// When a wait ends, in clock ticks ([`Platform::clock_ticks`]); `u64::MAX` for never.
type Deadline = u64;

/// The deadline `nanoseconds` from now, rounded up so it is never reached early.
fn deadline_in<P: Platform>(nanoseconds: u64) -> Deadline {
    let ticks = mul_div_ceil(nanoseconds, P::clock_frequency(), NANOSECONDS_PER_SECOND);
    P::clock_ticks().saturating_add(ticks)
}

/// `poll(fds, nfds, timeout)`: as [`ppoll`] with a timeout of `timeout` milliseconds (an
/// `int`: negative waits for good, 0 not at all) and no signal mask. x86_64 only: AArch64 and
/// RISC-V have only `ppoll`, which their C libraries' `poll` calls.
#[cfg(target_arch = "x86_64")]
pub fn poll<P: Platform>(
    process: &Process<P>,
    fds: u64,
    count: u64,
    timeout: u64,
) -> Result<u64, Errno> {
    let milliseconds = timeout as u32 as i32;
    let deadline = if milliseconds < 0 {
        Deadline::MAX
    } else {
        deadline_in::<P>(milliseconds as u64 * 1_000_000)
    };
    poll_fds(process, fds, count, deadline).map(|(ready, _)| ready)
}

/// `ppoll(fds, nfds, timeout, sigmask, sigsetsize)`: reports, in each `struct pollfd` of the
/// `nfds` at `fds`, the events its descriptor has of those asked for (see [`events`]), and
/// returns how many report any. If none does, it first waits for one until the `struct
/// timespec` at `timeout` has passed, for good if that is null, not at all if it is zero; a
/// wait that ends with the timeout returns 0 and writes a zero remaining time back to
/// `timeout`, as Linux's system call does (its C libraries hide that), ignoring a timeout the
/// program cannot write, as Linux does. As Linux, in this order: `EFAULT` if `timeout` is not
/// the program's memory; `EINVAL` for negative seconds or nanoseconds outside 0 to 999999999;
/// with a `sigmask`, `EINVAL` if `sigsetsize` is not 8 and `EFAULT` if the mask is not the
/// program's memory; `EINVAL` for more descriptors than the open-file limit (`RLIMIT_NOFILE`'s
/// soft limit); `EFAULT` if the array is not the program's writable memory; `ENOMEM` if there
/// is no memory to copy it. `nfds` is an `unsigned int`: the register's upper half is ignored.
///
/// The signal mask would apply during the wait; with no signal delivered yet (D60), it changes
/// nothing, so it is only checked.
pub fn ppoll<P: Platform>(
    process: &Process<P>,
    fds: u64,
    count: u64,
    timeout: u64,
    mask: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    let deadline = if timeout == 0 {
        Deadline::MAX
    } else {
        let mut bytes = [0; TIMESPEC_SIZE];
        process
            .read_user_memory(timeout, &mut bytes)
            .map_err(|_| Errno::EFAULT)?;
        let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
        let nanoseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
        if seconds < 0 || !(0..NANOSECONDS_PER_SECOND as i64).contains(&nanoseconds) {
            return Err(Errno::EINVAL);
        }
        deadline_in::<P>(
            (seconds as u64)
                .saturating_mul(NANOSECONDS_PER_SECOND)
                .saturating_add(nanoseconds as u64),
        )
    };
    if mask != 0 {
        if set_size != signals::SET_SIZE {
            return Err(Errno::EINVAL);
        }
        let mut bytes = [0; signals::SET_SIZE as usize];
        process
            .read_user_memory(mask, &mut bytes)
            .map_err(|_| Errno::EFAULT)?;
    }
    let (ready, timed_out) = poll_fds(process, fds, count, deadline)?;
    if timed_out {
        let _ = process.write_user_memory(timeout, &[0; TIMESPEC_SIZE]);
    }
    Ok(ready)
}

/// What both calls share once the timeout is a `deadline`: checks `count` against the
/// open-file limit, copies the `struct pollfd`s at `fds` in once, fills in their returned
/// events and copies them back, waiting until `deadline` if none reports any. Returns how
/// many do and whether a wait ended at the deadline.
fn poll_fds<P: Platform>(
    process: &Process<P>,
    fds: u64,
    count: u64,
    deadline: Deadline,
) -> Result<(u64, bool), Errno> {
    let count = u64::from(count as u32);
    let open_files = process
        .resource_limits()
        .lock()
        .get(limits::resource::NOFILE)
        .map_or(u64::MAX, |limit| limit.soft);
    if count > open_files {
        return Err(Errno::EINVAL);
    }
    // At most the open-file limit's 2^20 (`limits::MAX_OPEN_FILES`) entries: no overflow.
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
    let mut ready = 0;
    for entry in array.as_chunks_mut::<POLLFD_SIZE>().0 {
        let fd = i32::from_ne_bytes(entry[..4].try_into().unwrap());
        let requested = u16::from_ne_bytes(entry[4..6].try_into().unwrap());
        let returned = events(fd, requested);
        entry[6..].copy_from_slice(&returned.to_ne_bytes());
        ready += u64::from(returned != 0);
    }
    // Nothing becomes ready during a wait (see the module documentation): sleep it out.
    let timed_out = ready == 0 && P::clock_ticks() < deadline;
    if timed_out {
        thread::sleep_until::<P>(deadline);
    }
    process
        .write_user_memory(fds, array)
        .map_err(|_| Errno::EFAULT)?;
    Ok((ready, timed_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_report_linux_s_events() {
        let all = u16::MAX;
        // Skipped.
        assert_eq!(events(-1, all), 0);
        assert_eq!(events(i32::MIN, all), 0);
        // Closed: reported whatever was asked.
        assert_eq!(events(3, 0), event::NVAL);
        assert_eq!(events(i32::MAX, event::IN), event::NVAL);
        // The console: room to write, when asked; never data to read.
        for fd in 0..=2 {
            assert_eq!(events(fd, 0), 0);
            assert_eq!(events(fd, event::IN | event::RDNORM | event::PRI), 0);
            assert_eq!(events(fd, event::OUT), event::OUT);
            assert_eq!(events(fd, all), event::OUT | event::WRNORM);
        }
    }
}
