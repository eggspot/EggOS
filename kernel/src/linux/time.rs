//! Clocks and sleeping (M6, D73): `clock_gettime`, `clock_getres`, `gettimeofday`, x86_64's
//! `time`, `nanosleep` and `clock_nanosleep`, and `clock_settime` and `settimeofday`, which no
//! process may use yet. Rust's std reads `CLOCK_MONOTONIC` for `Instant` and `CLOCK_REALTIME`
//! for `SystemTime`, and sleeps with `nanosleep`; the C libraries' `time`, `gettimeofday` and
//! `sleep` reach the same calls when there is no vDSO.
//!
//! Every clock is the monotonic clock ([`Platform::clock_ticks`]) in nanoseconds. The wall
//! clock starts at the Unix epoch with it, as Linux's does on a machine with no real-time
//! clock, until a real-time clock driver sets it (after M5). The CPU-time clocks wait for
//! per-thread CPU time; until then they are invalid, as Linux's are for a process that does
//! not exist, and `clock()` reports that it cannot tell.

use super::signals::{self, Interrupted};
use super::thread::{self, LinuxThread};
use super::{Errno, poll};
use crate::process::Process;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::time::{mul_div_ceil, ratio_ceil};
use eggos_hal::user::UserContext;

/// The clock ids (`CLOCK_*`) Linux numbers, the generic values, which x86_64 shares.
pub mod clock {
    pub const REALTIME: i32 = 0;
    pub const MONOTONIC: i32 = 1;
    pub const PROCESS_CPUTIME_ID: i32 = 2;
    pub const THREAD_CPUTIME_ID: i32 = 3;
    pub const MONOTONIC_RAW: i32 = 4;
    pub const REALTIME_COARSE: i32 = 5;
    pub const MONOTONIC_COARSE: i32 = 6;
    pub const BOOTTIME: i32 = 7;
    pub const REALTIME_ALARM: i32 = 8;
    pub const BOOTTIME_ALARM: i32 = 9;
    pub const TAI: i32 = 11;
}

/// `clock_nanosleep`'s flag for a deadline on the clock instead of a duration.
pub const TIMER_ABSTIME: u64 = 1;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// The size of a `struct timespec` and of a `struct timeval`: two 64-bit fields.
const TIMESPEC_SIZE: usize = 16;

/// The first wall-clock second that may not be set, as on Linux: room for 30 years of uptime
/// before the clock's nanoseconds overflow a signed 64-bit count.
const UNSETTABLE_SECONDS_FROM: i64 = i64::MAX / NANOSECONDS_PER_SECOND as i64 - 30 * 365 * 86_400;

/// The size of a `struct timezone`: two `int`s.
const TIMEZONE_SIZE: usize = 8;

/// A clock the calls answer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Clock {
    /// Whether `clock_nanosleep` sleeps on it (Linux's high-resolution clocks); the others
    /// are `EOPNOTSUPP` there.
    can_sleep: bool,
}

/// The clock `id` (a `clockid_t`, an `int`) names, if the calls answer for it: the realtime,
/// monotonic, boot-time and TAI clocks, with their raw and coarse variants. Not the CPU-time
/// clocks (no per-thread CPU time yet), the alarm clocks (Linux's need a real-time clock
/// device), or the clocks of a descriptor.
fn clock(id: u64) -> Option<Clock> {
    match id as u32 as i32 {
        clock::REALTIME | clock::MONOTONIC | clock::BOOTTIME | clock::TAI => {
            Some(Clock { can_sleep: true })
        }
        clock::MONOTONIC_RAW | clock::REALTIME_COARSE | clock::MONOTONIC_COARSE => {
            Some(Clock { can_sleep: false })
        }
        _ => None,
    }
}

/// Nanoseconds per clock tick as a 32.32 fixed-point factor, rounded up, computed on first
/// use; 0 until then.
static NANOSECONDS_PER_TICK: AtomicU64 = AtomicU64::new(0);

fn nanoseconds_per_tick<P: Platform>() -> u64 {
    match NANOSECONDS_PER_TICK.load(Ordering::Relaxed) {
        0 => {
            let ratio = ratio_ceil(NANOSECONDS_PER_SECOND, P::clock_frequency());
            NANOSECONDS_PER_TICK.store(ratio, Ordering::Relaxed);
            ratio
        }
        ratio => ratio,
    }
}

/// The monotonic clock in nanoseconds: one multiplication of [`Platform::clock_ticks`]. The
/// factor is rounded up, so the clock never shows less time passing than the ticks did, and
/// a sleep of a duration always reads as at least that long.
pub fn monotonic_nanoseconds<P: Platform>() -> u64 {
    nanoseconds_at(P::clock_ticks(), nanoseconds_per_tick::<P>())
}

/// Clock tick `ticks` in nanoseconds, with `per_tick` from [`nanoseconds_per_tick`].
fn nanoseconds_at(ticks: u64, per_tick: u64) -> u64 {
    let nanoseconds = (u128::from(ticks) * u128::from(per_tick)) >> 32;
    u64::try_from(nanoseconds).unwrap_or(u64::MAX)
}

/// The first clock tick whose nanoseconds ([`nanoseconds_at`]) reach `nanoseconds`: a
/// deadline never early.
fn first_tick_at(nanoseconds: u64, per_tick: u64) -> u64 {
    mul_div_ceil(nanoseconds, 1 << 32, per_tick)
}

/// The clock tick a wait for the `struct timespec` timeout at `address` ends at, measured
/// from now: `u64::MAX` for one too long to count. Errors as [`poll::read_timeout`]'s.
pub fn deadline_after<P: Platform>(process: &Process<P>, address: u64) -> Result<u64, Errno> {
    Ok(match poll::read_timeout(process, address)? {
        Some(duration) => first_tick_at(
            monotonic_nanoseconds::<P>().saturating_add(duration),
            nanoseconds_per_tick::<P>(),
        ),
        None => u64::MAX,
    })
}

/// The clock tick a wait until the `struct timespec` at `address` ends at, a time on the
/// monotonic clock or the realtime clock, which reads the same until it can be set: `u64::MAX`
/// for one too late to count. Errors as [`poll::read_timeout`]'s.
pub fn deadline_at<P: Platform>(process: &Process<P>, address: u64) -> Result<u64, Errno> {
    Ok(match poll::read_timeout(process, address)? {
        Some(time) => first_tick_at(time, nanoseconds_per_tick::<P>()),
        None => u64::MAX,
    })
}

/// Sleeps `thread`, the calling thread of `process`, until the monotonic clock reads
/// `deadline` nanoseconds, never earlier, then sets the call's result, 0, and delivers what is
/// due, as Linux does once the time is up, a signal due or not. Until then, a due signal
/// interrupts the sleep: the time left is written to the `struct timespec` at `remaining`,
/// if it is not `None` or null (`EFAULT` if it is not the program's writable memory, the
/// sleep over), and the call fails with `EINTR` if a handler starts; if none does (the
/// signals were discarded), the sleep goes on to its deadline, as Linux restarts it. Breaks
/// when the process ends.
fn sleep_until_nanoseconds<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    deadline: u64,
    remaining: Option<u64>,
) -> ControlFlow<()> {
    let deadline_tick = first_tick_at(deadline, nanoseconds_per_tick::<P>());
    loop {
        thread::sleep_until::<P>(process, thread, deadline_tick)?;
        if P::clock_ticks() >= deadline_tick {
            break;
        }
        if signals::is_due(thread) {
            let left = timespec(deadline.saturating_sub(monotonic_nanoseconds::<P>()));
            if let Some(remaining) = remaining.filter(|&address| address != 0)
                && write(process, remaining, &left).is_err()
            {
                context.set_syscall_result(Errno::EFAULT.result());
                return signals::deliver(process, thread, context);
            }
            if signals::deliver_interrupting(process, thread, context, Interrupted::Fails)? {
                return ControlFlow::Continue(());
            }
        }
    }
    context.set_syscall_result(0);
    signals::deliver(process, thread, context)
}

fn timespec(nanoseconds: u64) -> [u8; TIMESPEC_SIZE] {
    let mut bytes = [0; TIMESPEC_SIZE];
    bytes[..8].copy_from_slice(&(nanoseconds / NANOSECONDS_PER_SECOND).to_ne_bytes());
    bytes[8..].copy_from_slice(&(nanoseconds % NANOSECONDS_PER_SECOND).to_ne_bytes());
    bytes
}

fn write<P: Platform>(process: &Process<P>, address: u64, bytes: &[u8]) -> Result<(), Errno> {
    process
        .write_user_memory(address, bytes)
        .map_err(|_| Errno::EFAULT)
}

/// `clock_gettime(clock, tp)`: writes clock `clock`'s time to the `struct timespec` at `tp`.
/// As Linux: `EINVAL` for a clock it does not answer for (see [`clock`]), then `EFAULT` if
/// `tp` is not the program's writable memory.
pub fn clock_gettime<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    tp: u64,
) -> Result<u64, Errno> {
    clock(clock_id).ok_or(Errno::EINVAL)?;
    write(process, tp, &timespec(monotonic_nanoseconds::<P>()))?;
    Ok(0)
}

/// `clock_getres(clock, res)`: writes clock `clock`'s resolution, 1 ns for each, to the
/// `struct timespec` at `res`, unless that is null. Coarse clocks read as precisely as the
/// others, so they report 1 ns too. As Linux: `EINVAL` for a clock it does not answer for,
/// then `EFAULT` if `res` is not the program's writable memory.
pub fn clock_getres<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    res: u64,
) -> Result<u64, Errno> {
    clock(clock_id).ok_or(Errno::EINVAL)?;
    if res != 0 {
        write(process, res, &timespec(1))?;
    }
    Ok(0)
}

/// `gettimeofday(tv, tz)`: writes the wall clock's time to the `struct timeval` at `tv`, and a
/// zeroed `struct timezone` (UTC, no daylight saving) to `tz`, each unless null; `EFAULT` for
/// one that is not the program's writable memory.
pub fn gettimeofday<P: Platform>(process: &Process<P>, tv: u64, tz: u64) -> Result<u64, Errno> {
    if tv != 0 {
        let now = monotonic_nanoseconds::<P>();
        let mut bytes = [0; TIMESPEC_SIZE];
        bytes[..8].copy_from_slice(&(now / NANOSECONDS_PER_SECOND).to_ne_bytes());
        bytes[8..].copy_from_slice(&(now % NANOSECONDS_PER_SECOND / 1000).to_ne_bytes());
        write(process, tv, &bytes)?;
    }
    if tz != 0 {
        write(process, tz, &[0; TIMEZONE_SIZE])?;
    }
    Ok(0)
}

/// `time(tloc)`: the wall clock's seconds, also written to `tloc` unless that is null;
/// `EFAULT` if it is not the program's writable memory. x86_64 only: the generic table has no
/// `time`, and its C libraries call `clock_gettime`.
#[cfg(target_arch = "x86_64")]
pub fn time<P: Platform>(process: &Process<P>, tloc: u64) -> Result<u64, Errno> {
    let seconds = monotonic_nanoseconds::<P>() / NANOSECONDS_PER_SECOND;
    if tloc != 0 {
        write(process, tloc, &seconds.to_ne_bytes())?;
    }
    Ok(seconds)
}

/// `clock_settime(clock, tp)`: as Linux, `EINVAL` for any clock but `CLOCK_REALTIME`, the only
/// one that can be set, then `EFAULT` if `tp` is not the program's memory, then `EINVAL` for
/// negative seconds, seconds past Linux's latest settable second or nanoseconds outside 0 to
/// 999999999, then `EPERM`: no process may set the wall clock yet, not even a privileged one,
/// which Linux allows (`CAP_SYS_TIME`); that waits for a settable clock and its own decision
/// (D75). The clock is shared by every process, and an agent confined to its own permissions
/// (D21) must not move it.
pub fn clock_settime<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    tp: u64,
) -> Result<u64, Errno> {
    if clock_id as u32 as i32 != clock::REALTIME {
        return Err(Errno::EINVAL);
    }
    let nanoseconds = poll::read_timeout(process, tp)?;
    if nanoseconds.is_none_or(|nanoseconds| {
        nanoseconds / NANOSECONDS_PER_SECOND >= UNSETTABLE_SECONDS_FROM as u64
    }) {
        return Err(Errno::EINVAL);
    }
    Err(Errno::EPERM)
}

/// `settimeofday(tv, tz)`: as Linux, each unless null: `EFAULT` if `tv` is not the program's
/// memory, `EINVAL` for microseconds outside 0 to 1000000, `EFAULT` if `tz` is not the
/// program's memory, `EINVAL` for negative seconds, seconds past Linux's latest settable
/// second or a whole second of microseconds, then `EPERM`, as for [`clock_settime`].
pub fn settimeofday<P: Platform>(process: &Process<P>, tv: u64, tz: u64) -> Result<u64, Errno> {
    let mut seconds_and_microseconds = None;
    if tv != 0 {
        let mut bytes = [0; TIMESPEC_SIZE];
        process
            .read_user_memory(tv, &mut bytes)
            .map_err(|_| Errno::EFAULT)?;
        let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
        let microseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
        if !(0..=1_000_000).contains(&microseconds) {
            return Err(Errno::EINVAL);
        }
        seconds_and_microseconds = Some((seconds, microseconds));
    }
    if tz != 0 {
        let mut bytes = [0; TIMEZONE_SIZE];
        process
            .read_user_memory(tz, &mut bytes)
            .map_err(|_| Errno::EFAULT)?;
    }
    if let Some((seconds, microseconds)) = seconds_and_microseconds
        && (!(0..UNSETTABLE_SECONDS_FROM).contains(&seconds) || microseconds == 1_000_000)
    {
        return Err(Errno::EINVAL);
    }
    Err(Errno::EPERM)
}

/// `nanosleep(req, rem)`, with the program's registers `context`: sleeps for the `struct
/// timespec` at `req` on the monotonic clock, as [`clock_nanosleep`] does with
/// `CLOCK_MONOTONIC` and no flags. Sets the call's result; breaks when the process ends.
pub fn nanosleep<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [req, rem, ..] = context.syscall_arguments();
    match deadline_after_request::<P>(process, req) {
        Ok(deadline) => sleep_until_nanoseconds::<P>(process, thread, context, deadline, Some(rem)),
        Err(errno) => fail(context, errno),
    }
}

/// `clock_nanosleep(clock, flags, req, rem)`, with the program's registers `context`: sleeps
/// on clock `clock` for the `struct timespec` at `req`, or, with [`TIMER_ABSTIME`] in
/// `flags`, until the clock reads it (at once if it has); its other bits are ignored, as on
/// Linux. It never wakes early, and a sleep too long to count lasts for good. As Linux:
/// `EINVAL` for a clock it does not answer for, `EOPNOTSUPP` for one it cannot sleep on (the
/// raw, coarse and thread CPU-time ones), then `EFAULT` if `req` is not the program's memory,
/// then `EINVAL` for negative seconds or nanoseconds outside 0 to 999999999; the alarm clocks,
/// which need a real-time clock device, are `EOPNOTSUPP` after those checks, as on Linux
/// without one. A signal that is due during the sleep interrupts it, as on Linux: a relative
/// sleep writes the time left to `rem`, unless it is null (`EFAULT`, the sleep over, if it
/// cannot), and the call fails with `EINTR` if a handler starts, whatever its
/// [`super::signals::flag::RESTART`]; if none starts, the sleep goes on. Sets the call's
/// result; breaks when the process ends.
pub fn clock_nanosleep<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [clock_id, flags, req, rem, ..] = context.syscall_arguments();
    let relative = flags & TIMER_ABSTIME == 0;
    let deadline = clock_for_sleep(process, clock_id, req).and_then(|()| match relative {
        true => deadline_after_request::<P>(process, req),
        false => Ok(poll::read_timeout(process, req)?.unwrap_or(u64::MAX)),
    });
    match deadline {
        Ok(deadline) => {
            let remaining = relative.then_some(rem);
            sleep_until_nanoseconds::<P>(process, thread, context, deadline, remaining)
        }
        Err(errno) => fail(context, errno),
    }
}

/// Checks that `clock_nanosleep` can sleep on clock `clock_id`, reading the request at `req`
/// first for the alarm clocks, as Linux does.
fn clock_for_sleep<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    req: u64,
) -> Result<(), Errno> {
    let clock = match clock_id as u32 as i32 {
        clock::THREAD_CPUTIME_ID => return Err(Errno::EOPNOTSUPP),
        clock::REALTIME_ALARM | clock::BOOTTIME_ALARM => {
            poll::read_timeout(process, req)?;
            return Err(Errno::EOPNOTSUPP);
        }
        _ => clock(clock_id).ok_or(Errno::EINVAL)?,
    };
    match clock.can_sleep {
        true => Ok(()),
        false => Err(Errno::EOPNOTSUPP),
    }
}

/// The monotonic clock's nanoseconds once the duration in the `struct timespec` at `req` has
/// passed from now, `u64::MAX` for one too long to count.
fn deadline_after_request<P: Platform>(process: &Process<P>, req: u64) -> Result<u64, Errno> {
    let duration = poll::read_timeout(process, req)?.unwrap_or(u64::MAX);
    Ok(monotonic_nanoseconds::<P>().saturating_add(duration))
}

fn fail(context: &mut UserContext, errno: Errno) -> ControlFlow<()> {
    context.set_syscall_result(errno.result());
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tick_at_a_time_reads_at_least_it_and_the_tick_before_reads_less() {
        let frequencies = [
            1_000_000,
            1_193_182,
            10_000_000,
            62_500_000,
            999_152_405,
            2_400_000_001,
            3_696_000_000,
        ];
        let times = [
            0,
            1,
            999,
            1_000_000,
            2_000_001,
            1_000_000_007,
            86_400 * NANOSECONDS_PER_SECOND + 17,
            40 * 365 * 86_400 * NANOSECONDS_PER_SECOND + 3,
        ];
        for frequency in frequencies {
            let per_tick = ratio_ceil(NANOSECONDS_PER_SECOND, frequency);
            for time in times {
                let tick = first_tick_at(time, per_tick);
                assert!(
                    nanoseconds_at(tick, per_tick) >= time,
                    "{time} at {frequency}"
                );
                if tick > 0 {
                    assert!(
                        nanoseconds_at(tick - 1, per_tick) < time,
                        "{time} at {frequency}"
                    );
                }
                let exact = mul_div_ceil(tick, NANOSECONDS_PER_SECOND, frequency);
                assert!(
                    nanoseconds_at(tick, per_tick) + 1 >= exact,
                    "{time} at {frequency}"
                );
            }
        }
    }
}
