//! Clocks and sleeping (M6, D73, D150): `clock_gettime`, `clock_getres`, `gettimeofday`,
//! x86_64's `time`, `nanosleep` and `clock_nanosleep`, and `clock_settime` and `settimeofday`.
//! Rust's std reads `CLOCK_MONOTONIC` for `Instant` and `CLOCK_REALTIME` for `SystemTime`, and
//! sleeps with `nanosleep`; the C libraries' `time`, `gettimeofday` and `sleep` reach the same
//! calls when there is no vDSO.
//!
//! The monotonic and boot-time clocks are the monotonic clock ([`Platform::clock_ticks`]) in
//! nanoseconds; the realtime and TAI clocks, `gettimeofday` and `time` are the wall clock
//! ([`crate::wall_clock`]), the monotonic clock plus the offset the real-time clock set at boot,
//! or the epoch at boot on a machine without one, as on Linux. A privileged process (effective
//! user id 0, D75) may set the wall clock, which also sets the real-time clock. The CPU-time
//! clocks wait for per-thread CPU time; until then they are invalid, as Linux's are for a
//! process that does not exist, and `clock()` reports that it cannot tell.

use super::futex;
use super::signals::{self, Interrupted};
use super::thread::{self, LinuxThread};
use super::{Errno, poll};
use crate::process::Process;
use crate::sync::LeafSpinLock;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Clock {
    can_sleep: bool,
    wall: bool,
}

impl Clock {
    fn now<P: Platform>(self) -> u64 {
        match self.wall {
            true => crate::wall_clock::nanoseconds::<P>(),
            false => monotonic_nanoseconds::<P>(),
        }
    }

    fn monotonic_at(self, time: u64) -> u64 {
        match self.wall {
            true => crate::wall_clock::monotonic_at(time),
            false => time,
        }
    }
}

fn clock(id: u64) -> Option<Clock> {
    let (can_sleep, wall) = match id as u32 as i32 {
        clock::REALTIME | clock::TAI => (true, true),
        clock::MONOTONIC | clock::BOOTTIME => (true, false),
        clock::REALTIME_ALARM if crate::wall_clock::has_clock() => (true, true),
        clock::BOOTTIME_ALARM if crate::wall_clock::has_clock() => (true, false),
        clock::REALTIME_COARSE => (false, true),
        clock::MONOTONIC_RAW | clock::MONOTONIC_COARSE => (false, false),
        _ => return None,
    };
    Some(Clock { can_sleep, wall })
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

/// The first clock tick at which the monotonic clock reads `nanoseconds`: a deadline never
/// early.
pub fn tick_at<P: Platform>(nanoseconds: u64) -> u64 {
    first_tick_at(nanoseconds, nanoseconds_per_tick::<P>())
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

/// The clock tick a wait until the `struct timespec` at `address` ends at, a time on the wall
/// clock if `wall` (as the wall clock is set now: setting it later does not move the deadline),
/// else on the monotonic clock: `u64::MAX` for one too late to count. Errors as
/// [`poll::read_timeout`]'s.
pub fn deadline_at<P: Platform>(
    process: &Process<P>,
    address: u64,
    wall: bool,
) -> Result<u64, Errno> {
    let clock = Clock {
        can_sleep: true,
        wall,
    };
    Ok(match poll::read_timeout(process, address)? {
        Some(time) => first_tick_at(clock.monotonic_at(time), nanoseconds_per_tick::<P>()),
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
    if let Some(spent) = cpu_clock(process, clock_id) {
        write(process, tp, &timespec(spent))?;
        return Ok(0);
    }
    let clock = clock(clock_id).ok_or(Errno::EINVAL)?;
    write(process, tp, &timespec(clock.now::<P>()))?;
    Ok(0)
}

/// The CPU time clocks (D198): `CLOCK_PROCESS_CPUTIME_ID`, the nanoseconds the calling
/// process's threads have run, and `CLOCK_THREAD_CPUTIME_ID`, the calling thread's; `None` for
/// another clock.
fn cpu_clock<P: Platform>(process: &Process<P>, clock_id: u64) -> Option<u64> {
    let ticks = match clock_id as u32 as i32 {
        clock::PROCESS_CPUTIME_ID => process.cpu_ticks(true),
        clock::THREAD_CPUTIME_ID => crate::thread::running_thread_ticks::<P>(),
        _ => return None,
    };
    Some(nanoseconds_at(ticks, nanoseconds_per_tick::<P>()))
}

const RUSAGE_SELF: i64 = 0;
const RUSAGE_CHILDREN: i64 = -1;
const RUSAGE_THREAD: i64 = 1;
const RUSAGE_SIZE: usize = 144;
const USER_HZ: u64 = 100;

/// `getrusage(who, usage)` (D198): the calling process's (`RUSAGE_SELF`) or thread's
/// (`RUSAGE_THREAD`) CPU time as user time in the `struct rusage` at `usage`, every other field
/// 0; `RUSAGE_CHILDREN` reads all 0, as the time of waited-for children is not kept yet.
/// `EINVAL` for another `who`, `EFAULT` for `usage` not the program's writable memory.
pub fn getrusage<P: Platform>(process: &Process<P>, who: u64, usage: u64) -> Result<u64, Errno> {
    let nanoseconds = match who as i64 as i32 as i64 {
        RUSAGE_SELF => cpu_clock(process, clock::PROCESS_CPUTIME_ID as u64).unwrap_or(0),
        RUSAGE_THREAD => cpu_clock(process, clock::THREAD_CPUTIME_ID as u64).unwrap_or(0),
        RUSAGE_CHILDREN => 0,
        _ => return Err(Errno::EINVAL),
    };
    let mut bytes = [0u8; RUSAGE_SIZE];
    bytes[..8].copy_from_slice(&(nanoseconds / NANOSECONDS_PER_SECOND).to_ne_bytes());
    bytes[8..16].copy_from_slice(&(nanoseconds % NANOSECONDS_PER_SECOND / 1000).to_ne_bytes());
    write(process, usage, &bytes)?;
    Ok(0)
}

/// `times(buf)` (D198): writes the calling process's CPU time as user time in the `struct tms`
/// at `buf` (unless null), in `USER_HZ` (100) ticks, its system time and its children's 0;
/// returns the ticks since boot.
pub fn times<P: Platform>(process: &Process<P>, buf: u64) -> Result<u64, Errno> {
    if buf != 0 {
        let spent = cpu_clock(process, clock::PROCESS_CPUTIME_ID as u64).unwrap_or(0);
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&(spent / (NANOSECONDS_PER_SECOND / USER_HZ)).to_ne_bytes());
        write(process, buf, &bytes)?;
    }
    Ok(monotonic_nanoseconds::<P>() / (NANOSECONDS_PER_SECOND / USER_HZ))
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
    if clock(clock_id).is_none() && cpu_clock(process, clock_id).is_none() {
        return Err(Errno::EINVAL);
    }
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
        let now = crate::wall_clock::nanoseconds::<P>();
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
    let seconds = crate::wall_clock::seconds::<P>();
    if tloc != 0 {
        write(process, tloc, &seconds.to_ne_bytes())?;
    }
    Ok(seconds)
}

/// `clock_settime(clock, tp)`: sets the wall clock, and with it the real-time clock (D150), to
/// the `struct timespec` at `tp`. As Linux: `EINVAL` for any clock but `CLOCK_REALTIME`, the
/// only one that can be set, then `EFAULT` if `tp` is not the program's memory, then `EINVAL`
/// for negative seconds, seconds past Linux's latest settable second or nanoseconds outside 0
/// to 999999999, then `EPERM` unless the process is privileged (Linux's `CAP_SYS_TIME`: an
/// effective user id of 0, D75), as the clock is the whole machine's.
pub fn clock_settime<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    tp: u64,
) -> Result<u64, Errno> {
    if clock_id as u32 as i32 != clock::REALTIME {
        return Err(Errno::EINVAL);
    }
    let nanoseconds = poll::read_timeout(process, tp)?
        .filter(|nanoseconds| nanoseconds / NANOSECONDS_PER_SECOND < UNSETTABLE_SECONDS_FROM as u64)
        .ok_or(Errno::EINVAL)?;
    may_set_the_clock(process)?;
    set_wall_clock::<P>(nanoseconds);
    Ok(0)
}

/// Sets the wall clock, and the real-time clock with it, to `nanoseconds` since the epoch, and
/// wakes the sleeps until a wall-clock time to measure their deadlines again (D194).
pub fn set_wall_clock<P: Platform>(nanoseconds: u64) {
    crate::wall_clock::set::<P>(nanoseconds);
    wake_wall_sleepers::<P>();
}

fn may_set_the_clock<P: Platform>(process: &Process<P>) -> Result<(), Errno> {
    match is_privileged(process) {
        true => Ok(()),
        false => Err(Errno::EPERM),
    }
}

/// Whether `process` is privileged (an effective user id of 0, D75): it may set the clock
/// and use the alarm clocks.
pub fn is_privileged<P: Platform>(process: &Process<P>) -> bool {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .is_privileged()
}

/// `settimeofday(tv, tz)`: sets the wall clock to the `struct timeval` at `tv` as
/// [`clock_settime`] does, unless `tv` is null; the time zone at `tz` is read and dropped, as
/// EggOS keeps UTC (D150). As Linux, each unless null: `EFAULT` if `tv` is not the program's
/// memory, `EINVAL` for microseconds outside 0 to 1000000, `EFAULT` if `tz` is not the
/// program's memory, `EINVAL` for negative seconds, seconds past Linux's latest settable
/// second or a whole second of microseconds, then `EPERM` unless the process is privileged,
/// with both null too.
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
    may_set_the_clock(process)?;
    if let Some((seconds, microseconds)) = seconds_and_microseconds {
        let nanoseconds = seconds as u64 * NANOSECONDS_PER_SECOND + microseconds as u64 * 1000;
        set_wall_clock::<P>(nanoseconds);
    }
    Ok(0)
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
/// `flags`, until the clock reads it (at once if it has; on the realtime, TAI and realtime alarm
/// clocks the deadline is measured again each time the wall clock is set, as on Linux, D194);
/// its other bits are ignored, as on Linux. The alarm clocks are their plain clocks, as EggOS
/// never suspends, for a privileged process on a machine with a real-time clock. It never wakes early, and a sleep too long to count
/// lasts for good. As Linux:
/// `EINVAL` for a clock it does not answer for, `EOPNOTSUPP` for one it cannot sleep on (the
/// raw, coarse and thread CPU-time ones), then `EFAULT` if `req` is not the program's memory,
/// then `EINVAL` for negative seconds or nanoseconds outside 0 to 999999999; the alarm clocks
/// are `EOPNOTSUPP` after those checks without a real-time clock device, as on Linux without
/// one, and `EPERM` for a process that is not privileged. A signal that is due during the sleep interrupts it, as on Linux: a relative
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
    if !relative
        && let Ok(clock) = clock_for_sleep(process, clock_id, req)
        && clock.wall
    {
        return match poll::read_timeout(process, req) {
            Ok(Some(time)) => sleep_until_wall::<P>(process, thread, context, time),
            Ok(None) => sleep_until_nanoseconds::<P>(process, thread, context, u64::MAX, None),
            Err(errno) => fail(context, errno),
        };
    }
    let deadline = clock_for_sleep(process, clock_id, req).and_then(|clock| match relative {
        true => deadline_after_request::<P>(process, req),
        false => {
            Ok(poll::read_timeout(process, req)?.map_or(u64::MAX, |time| clock.monotonic_at(time)))
        }
    });
    match deadline {
        Ok(deadline) => {
            let remaining = relative.then_some(rem);
            sleep_until_nanoseconds::<P>(process, thread, context, deadline, remaining)
        }
        Err(errno) => fail(context, errno),
    }
}

/// The most threads whose sleeps until a wall-clock time are measured again when the clock is
/// set; a thread past them sleeps as the clock read when it started.
const MAX_WALL_SLEEPERS: usize = 64;

static WALL_SLEEPERS: LeafSpinLock<[Option<futex::Key>; MAX_WALL_SLEEPERS]> =
    LeafSpinLock::new_leaf([None; MAX_WALL_SLEEPERS]);

fn hold_wall_sleeper(key: futex::Key) -> Option<usize> {
    let mut sleepers = WALL_SLEEPERS.lock();
    let slot = sleepers.iter().position(Option::is_none)?;
    sleepers[slot] = Some(key);
    Some(slot)
}

fn release_wall_sleeper(slot: Option<usize>) {
    if let Some(slot) = slot {
        WALL_SLEEPERS.lock()[slot] = None;
    }
}

/// Wakes every thread sleeping until a wall-clock time, after the clock was set, so each
/// measures its deadline again (D194).
fn wake_wall_sleepers<P: Platform>() {
    let sleepers = *WALL_SLEEPERS.lock();
    for key in sleepers.into_iter().flatten() {
        futex::wake::<P>(key, u32::MAX, 1);
    }
}

/// Sleeps until the wall clock reads `time` nanoseconds since the epoch, measuring again each
/// time the clock is set; interrupted by a signal as [`sleep_until_nanoseconds`] is, with no
/// time left to write (an absolute sleep has none).
fn sleep_until_wall<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    time: u64,
) -> ControlFlow<()> {
    let slot = hold_wall_sleeper(thread::sleep_key_of(process, thread));
    let per_tick = nanoseconds_per_tick::<P>();
    let outcome = loop {
        let deadline_tick = first_tick_at(crate::wall_clock::monotonic_at(time), per_tick);
        if P::clock_ticks() >= deadline_tick {
            break ControlFlow::Continue(true);
        }
        if thread::sleep_once_until::<P>(process, thread, deadline_tick).is_break() {
            break ControlFlow::Break(());
        }
        if signals::is_due(thread) {
            match signals::deliver_interrupting(process, thread, context, Interrupted::Fails) {
                ControlFlow::Break(()) => break ControlFlow::Break(()),
                ControlFlow::Continue(true) => break ControlFlow::Continue(false),
                ControlFlow::Continue(false) => {}
            }
        }
    };
    release_wall_sleeper(slot);
    match outcome {
        ControlFlow::Break(()) => ControlFlow::Break(()),
        ControlFlow::Continue(false) => ControlFlow::Continue(()),
        ControlFlow::Continue(true) => {
            context.set_syscall_result(0);
            signals::deliver(process, thread, context)
        }
    }
}

fn clock_for_sleep<P: Platform>(
    process: &Process<P>,
    clock_id: u64,
    req: u64,
) -> Result<Clock, Errno> {
    let clock = match clock_id as u32 as i32 {
        clock::THREAD_CPUTIME_ID => return Err(Errno::EOPNOTSUPP),
        clock::REALTIME_ALARM | clock::BOOTTIME_ALARM if !crate::wall_clock::has_clock() => {
            poll::read_timeout(process, req)?;
            return Err(Errno::EOPNOTSUPP);
        }
        clock::REALTIME_ALARM | clock::BOOTTIME_ALARM if !is_privileged(process) => {
            poll::read_timeout(process, req)?;
            return Err(Errno::EPERM);
        }
        _ => clock(clock_id).ok_or(Errno::EINVAL)?,
    };
    match clock.can_sleep {
        true => Ok(clock),
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
