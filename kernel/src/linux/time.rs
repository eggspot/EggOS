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

use super::{Errno, poll};
use crate::process::Process;
use crate::thread;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::time::{mul_div_ceil, ratio_ceil};

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
    nanoseconds_at::<P>(P::clock_ticks())
}

fn nanoseconds_at<P: Platform>(ticks: u64) -> u64 {
    let nanoseconds = (u128::from(ticks) * u128::from(nanoseconds_per_tick::<P>())) >> 32;
    u64::try_from(nanoseconds).unwrap_or(u64::MAX)
}

/// The first clock tick whose nanoseconds reach `nanoseconds`: a deadline never early.
fn first_tick_at<P: Platform>(nanoseconds: u64) -> u64 {
    mul_div_ceil(nanoseconds, 1 << 32, nanoseconds_per_tick::<P>())
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
/// negative seconds or nanoseconds outside 0 to 999999999, then `EPERM`: no process may set
/// the wall clock (Linux's `CAP_SYS_TIME`) until processes have credentials, as none may raise
/// a hard resource limit (D59). The clock is shared by every process, and an agent confined to
/// its own permissions (D21) must not move it.
pub fn clock_settime<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    tp: u64,
) -> Result<u64, Errno> {
    if clock_id as u32 as i32 != clock::REALTIME {
        return Err(Errno::EINVAL);
    }
    poll::read_timeout(process, tp)?;
    Err(Errno::EPERM)
}

/// `settimeofday(tv, tz)`: as Linux, each unless null: `EFAULT` if `tv` is not the program's
/// memory, `EINVAL` for microseconds outside 0 to 1000000, `EFAULT` if `tz` is not the
/// program's memory, `EINVAL` for negative seconds or a whole second of microseconds, then
/// `EPERM`, as for [`clock_settime`].
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
        && (seconds < 0 || microseconds == 1_000_000)
    {
        return Err(Errno::EINVAL);
    }
    Err(Errno::EPERM)
}

/// `nanosleep(req, rem)`: sleeps for the `struct timespec` at `req` on the monotonic clock, as
/// [`clock_nanosleep`] does with `CLOCK_MONOTONIC` and no flags.
pub fn nanosleep<P: Platform>(process: &Process<P>, req: u64) -> Result<u64, Errno> {
    sleep_for::<P>(process, req)
}

/// `clock_nanosleep(clock, flags, req, rem)`: sleeps on clock `clock` for the `struct
/// timespec` at `req`, or, with [`TIMER_ABSTIME`] in `flags`, until the clock reads it (at
/// once if it has); its other bits are ignored, as on Linux. It never wakes early, and a
/// sleep too long to count lasts for good. As Linux: `EINVAL` for a clock it does not answer
/// for, `EOPNOTSUPP` for one it cannot sleep on (the raw and coarse ones), then `EFAULT` if
/// `req` is not the program's memory, then `EINVAL` for negative seconds or nanoseconds outside
/// 0 to 999999999. Nothing can send the thread a signal while it sleeps yet, so the sleep is
/// never interrupted and `rem` is never written.
pub fn clock_nanosleep<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    flags: u64,
    req: u64,
) -> Result<u64, Errno> {
    let clock = clock(clock_id).ok_or(Errno::EINVAL)?;
    if !clock.can_sleep {
        return Err(Errno::EOPNOTSUPP);
    }
    if flags & TIMER_ABSTIME == 0 {
        return sleep_for::<P>(process, req);
    }
    let deadline = poll::read_timeout(process, req)?.unwrap_or(u64::MAX);
    thread::sleep_until::<P>(first_tick_at::<P>(deadline));
    Ok(0)
}

fn sleep_for<P: Platform>(process: &Process<P>, req: u64) -> Result<u64, Errno> {
    let duration = poll::read_timeout(process, req)?.unwrap_or(u64::MAX);
    let deadline = monotonic_nanoseconds::<P>().saturating_add(duration);
    thread::sleep_until::<P>(first_tick_at::<P>(deadline));
    Ok(0)
}
