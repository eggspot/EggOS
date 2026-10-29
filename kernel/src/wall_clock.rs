//! The wall clock (D150): what `CLOCK_REALTIME`, `gettimeofday`, `time`, file times, the
//! desktop's clock and `date` read. It is the monotonic clock plus an offset, so a read costs
//! one relaxed load and one saturating addition over the monotonic read (D73), with no lock and
//! no allocation. At boot, once the drivers have probed, [`start`] reads the real-time clock
//! once and sets the offset from it; [`set`] moves the offset and writes the real-time clock,
//! the only place a set time outlives a restart. Without a real-time clock the wall clock
//! starts at the epoch at boot, as Linux's does, and is not set ([`is_set`]) until [`set`].
//! All in UTC: there are no time zones.
//!
//! The real-time clock is slow hardware (the CMOS clock is some forty port accesses a read), so
//! its lock is an ordinary [`SpinLock`] whose waiters do their wait duty, never a leaf lock.
//! Setting the clock moves the offset and writes the clock in one hold of that lock, so two sets
//! never leave the wall clock and the real-time clock disagreeing. [`read_clock`] reads the
//! hardware at most once in [`READING_KEPT_NANOSECONDS`] and in between answers the last
//! reading plus the time since, so no reader can keep the hardware busy.

use crate::linux::time::monotonic_nanoseconds;
use crate::sync::SpinLock;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use eggos_drivers::rtc::{NANOSECONDS_PER_SECOND, RealTimeClock, RtcError};
use eggos_hal::Platform;

/// How long [`read_clock`] answers from its last reading of the hardware: a second, the
/// real-time clocks' resolution.
pub const READING_KEPT_NANOSECONDS: u64 = NANOSECONDS_PER_SECOND;

static OFFSET_NANOSECONDS: AtomicI64 = AtomicI64::new(0);

enum Clock {
    Device(Box<dyn RealTimeClock + Send>),
    Platform,
}

#[derive(Debug, Clone, Copy)]
struct Reading {
    monotonic: u64,
    nanoseconds: u64,
}

impl Reading {
    fn extended_to(self, monotonic: u64) -> Option<u64> {
        let since = monotonic.checked_sub(self.monotonic)?;
        (since < READING_KEPT_NANOSECONDS).then(|| self.nanoseconds.saturating_add(since))
    }
}

struct InstalledClock {
    clock: Clock,
    last_reading: Option<Reading>,
}

static CLOCK: SpinLock<Option<Box<InstalledClock>>> = SpinLock::new(None);

static HAS_CLOCK: AtomicBool = AtomicBool::new(false);

static WALL_CLOCK_SET: AtomicBool = AtomicBool::new(false);

/// The wall clock in nanoseconds since 1970-01-01 00:00:00 UTC.
#[inline]
pub fn nanoseconds<P: Platform>() -> u64 {
    monotonic_nanoseconds::<P>().saturating_add_signed(OFFSET_NANOSECONDS.load(Ordering::Relaxed))
}

/// The wall clock's whole seconds since the epoch.
pub fn seconds<P: Platform>() -> u64 {
    nanoseconds::<P>() / NANOSECONDS_PER_SECOND
}

/// What the monotonic clock reads when the wall clock reads `nanoseconds`, as the wall clock is
/// set now: a deadline on the wall clock (an absolute `CLOCK_REALTIME` sleep or futex wait),
/// which setting the clock later does not move.
pub fn monotonic_at(nanoseconds: u64) -> u64 {
    let offset = OFFSET_NANOSECONDS.load(Ordering::Relaxed);
    match offset >= 0 {
        true => nanoseconds.saturating_sub(offset.unsigned_abs()),
        false => nanoseconds.saturating_add(offset.unsigned_abs()),
    }
}

/// Makes `clock`, a device driver's (D110), the machine's real-time clock; `false`, and
/// `clock` dropped, if one already is: the first found wins.
pub fn register(clock: Box<dyn RealTimeClock + Send>) -> bool {
    let clock = Box::new(InstalledClock {
        clock: Clock::Device(clock),
        last_reading: None,
    });
    let mut held = CLOCK.lock();
    if held.is_some() {
        return false;
    }
    *held = Some(clock);
    HAS_CLOCK.store(true, Ordering::Release);
    true
}

/// What [`start`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// The real-time clock `name` read `seconds` since the epoch, which the wall clock reads
    /// from now on.
    FromClock { name: &'static str, seconds: u64 },
    /// The real-time clock `name` could not be read: the wall clock starts at the epoch.
    Unreadable { name: &'static str, error: RtcError },
    /// There is no real-time clock: the wall clock starts at the epoch.
    NoClock,
}

/// Sets the wall clock from the real-time clock, once at boot after the drivers probed: the
/// driver model's clock if one was registered, else the platform's own
/// ([`Platform::platform_clock`]).
pub fn start<P: Platform>() -> Started {
    let mut platform = Some(Box::new(InstalledClock {
        clock: Clock::Platform,
        last_reading: None,
    }));
    let mut held = CLOCK.lock();
    if held.is_none() {
        let mut found = false;
        // SAFETY: `held` holds `CLOCK`, the lock every call of `platform_clock` is made under.
        unsafe { P::platform_clock(crate::sync::do_wait_duty, &mut |_| found = true) };
        if found {
            *held = platform.take();
            HAS_CLOCK.store(true, Ordering::Release);
        }
    }
    let read = held.as_deref().and_then(|installed| {
        on_clock::<P, _>(&installed.clock, |clock| (clock.name(), clock.read()))
    });
    let started = match read {
        Some((name, Ok(nanoseconds))) => {
            set_offset::<P>(nanoseconds);
            WALL_CLOCK_SET.store(true, Ordering::Relaxed);
            Started::FromClock {
                name,
                seconds: nanoseconds / NANOSECONDS_PER_SECOND,
            }
        }
        Some((name, Err(error))) => Started::Unreadable { name, error },
        None => Started::NoClock,
    };
    drop(held);
    drop(platform);
    started
}

/// What [`set`] did with the real-time clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// The real-time clock holds the new time.
    InClock,
    /// There is no real-time clock: the time lasts until power-off.
    NoClock,
    /// The real-time clock refused it.
    ClockFailed(RtcError),
}

/// Sets the wall clock to `nanoseconds` since the epoch: from now on it reads that plus the
/// time since. The real-time clock, if there is one, is set to it too, as Windows and macOS do
/// (decided by research: with no network time, it is the only thing that keeps a set time
/// across a restart). The wall clock is set whatever the real-time clock answers.
pub fn set<P: Platform>(nanoseconds: u64) -> Saved {
    let mut held = CLOCK.lock();
    set_offset::<P>(nanoseconds);
    WALL_CLOCK_SET.store(true, Ordering::Relaxed);
    let Some(installed) = held.as_deref_mut() else {
        return Saved::NoClock;
    };
    let written = on_clock::<P, _>(&installed.clock, |clock| clock.write(nanoseconds));
    installed.last_reading = match written {
        Some(Ok(())) => Some(Reading {
            monotonic: monotonic_nanoseconds::<P>(),
            nanoseconds,
        }),
        _ => None,
    };
    match written {
        Some(Ok(())) => Saved::InClock,
        Some(Err(error)) => Saved::ClockFailed(error),
        None => Saved::NoClock,
    }
}

/// The real-time clock's name and its time now, in nanoseconds since the epoch
/// (`/proc/driver/rtc`, so `hwclock`); `None` without one. It is read from the clock itself at
/// most once in [`READING_KEPT_NANOSECONDS`]: in between, the last good reading (or what a set
/// wrote) plus the monotonic time since.
pub fn read_clock<P: Platform>() -> Option<(&'static str, Result<u64, RtcError>)> {
    let mut held = CLOCK.lock();
    let installed = held.as_deref_mut()?;
    let recent = installed
        .last_reading
        .and_then(|reading| reading.extended_to(monotonic_nanoseconds::<P>()));
    let (name, read) = on_clock::<P, _>(&installed.clock, |clock| {
        (clock.name(), recent.map_or_else(|| clock.read(), Ok))
    })?;
    if recent.is_none() {
        installed.last_reading = read.ok().map(|nanoseconds| Reading {
            monotonic: monotonic_nanoseconds::<P>(),
            nanoseconds,
        });
    }
    Some((name, read))
}

/// Whether the machine has a real-time clock (known once [`start`] ran): one load, no lock.
pub fn has_clock() -> bool {
    HAS_CLOCK.load(Ordering::Acquire)
}

/// Whether the wall clock has been set: read from the real-time clock at boot, or set since
/// ([`set`]). Until then it counts from the epoch at boot: one load, no lock.
pub fn is_set() -> bool {
    WALL_CLOCK_SET.load(Ordering::Relaxed)
}

fn set_offset<P: Platform>(nanoseconds: u64) {
    let monotonic = monotonic_nanoseconds::<P>();
    let offset = i128::from(nanoseconds) - i128::from(monotonic);
    let offset = offset.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    OFFSET_NANOSECONDS.store(offset, Ordering::Relaxed);
}

fn on_clock<P: Platform, R>(
    clock: &Clock,
    use_clock: impl FnOnce(&dyn RealTimeClock) -> R,
) -> Option<R> {
    match clock {
        Clock::Device(device) => Some(use_clock(&**device)),
        Clock::Platform => {
            let mut use_clock = Some(use_clock);
            let mut result = None;
            // SAFETY: every caller passes a `clock` borrowed from `CLOCK`'s guard, so `CLOCK`,
            // the lock every call of `platform_clock` is made under, is held.
            unsafe {
                P::platform_clock(crate::sync::do_wait_duty, &mut |platform| {
                    if let Some(use_clock) = use_clock.take() {
                        result = Some(use_clock(platform));
                    }
                })
            };
            result
        }
    }
}

#[cfg(test)]
pub(crate) mod host {
    use super::*;

    pub(crate) fn reset() {
        OFFSET_NANOSECONDS.store(0, Ordering::Relaxed);
        WALL_CLOCK_SET.store(false, Ordering::Relaxed);
    }

    pub(crate) fn with_clock<R>(
        clock: Box<dyn RealTimeClock + Send>,
        run: impl FnOnce() -> R,
    ) -> R {
        let installed = Box::new(InstalledClock {
            clock: Clock::Device(clock),
            last_reading: None,
        });
        let before = CLOCK.lock().replace(installed);
        let had_clock = HAS_CLOCK.swap(true, Ordering::Relaxed);
        let result = run();
        *CLOCK.lock() = before;
        HAS_CLOCK.store(had_clock, Ordering::Relaxed);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform;
    use alloc::sync::Arc;

    #[derive(Debug)]
    struct FixedClock {
        nanoseconds: u64,
        written: alloc::sync::Arc<core::sync::atomic::AtomicU64>,
    }

    impl RealTimeClock for FixedClock {
        fn name(&self) -> &'static str {
            "rtc-test"
        }

        fn read(&self) -> Result<u64, RtcError> {
            Ok(self.nanoseconds)
        }

        fn write(&self, nanoseconds: u64) -> Result<(), RtcError> {
            self.written.store(nanoseconds, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn the_wall_clock_reads_the_real_time_clock_plus_the_time_since_and_writes_it_when_set() {
        let _state = crate::thread::host::frames_for_test();
        let now = 1_793_282_709 * NANOSECONDS_PER_SECOND;
        let written = alloc::sync::Arc::new(core::sync::atomic::AtomicU64::new(0));
        let before = CLOCK.lock().take();
        let had_clock = HAS_CLOCK.load(Ordering::Relaxed);
        assert!(register(Box::new(FixedClock {
            nanoseconds: now,
            written: written.clone(),
        })));
        assert!(!register(Box::new(FixedClock {
            nanoseconds: 0,
            written: written.clone(),
        })));
        assert_eq!(
            start::<TestPlatform>(),
            Started::FromClock {
                name: "rtc-test",
                seconds: 1_793_282_709
            }
        );
        let read = nanoseconds::<TestPlatform>();
        assert!(read >= now && read - now < 60 * NANOSECONDS_PER_SECOND);
        assert!(has_clock());
        assert!(is_set());
        let later = now + 3600 * NANOSECONDS_PER_SECOND;
        assert_eq!(set::<TestPlatform>(later), Saved::InClock);
        assert_eq!(written.load(Ordering::Relaxed), later);
        assert!(seconds::<TestPlatform>() >= 1_793_282_709 + 3600);
        assert_eq!(
            read_clock::<TestPlatform>().map(|(name, _)| name),
            Some("rtc-test")
        );
        *CLOCK.lock() = before;
        HAS_CLOCK.store(had_clock, Ordering::Relaxed);
        host::reset();
        assert!(nanoseconds::<TestPlatform>() < now);
        assert!(!is_set());
    }

    #[derive(Debug)]
    struct CountedClock {
        nanoseconds: u64,
        reads: Arc<core::sync::atomic::AtomicU32>,
        fails: bool,
    }

    impl RealTimeClock for CountedClock {
        fn name(&self) -> &'static str {
            "rtc-counted"
        }

        fn read(&self) -> Result<u64, RtcError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            match self.fails {
                true => Err(RtcError::Busy),
                false => Ok(self.nanoseconds),
            }
        }

        fn write(&self, _nanoseconds: u64) -> Result<(), RtcError> {
            Ok(())
        }
    }

    #[test]
    fn a_reading_is_kept_for_a_second_and_extended_by_the_time_since() {
        let reading = Reading {
            monotonic: 5_000,
            nanoseconds: 1_793_282_709 * NANOSECONDS_PER_SECOND,
        };
        assert_eq!(reading.extended_to(5_000), Some(reading.nanoseconds));
        assert_eq!(reading.extended_to(5_017), Some(reading.nanoseconds + 17));
        assert_eq!(
            reading.extended_to(5_000 + READING_KEPT_NANOSECONDS - 1),
            Some(reading.nanoseconds + READING_KEPT_NANOSECONDS - 1)
        );
        assert_eq!(reading.extended_to(5_000 + READING_KEPT_NANOSECONDS), None);
        assert_eq!(reading.extended_to(4_999), None);
    }

    #[test]
    fn the_hardware_is_read_at_most_once_a_second_and_a_set_is_what_it_reads_next() {
        let _state = crate::thread::host::frames_for_test();
        let before = CLOCK.lock().take();
        let had_clock = HAS_CLOCK.load(Ordering::Relaxed);
        let now = 1_793_282_709 * NANOSECONDS_PER_SECOND;
        let reads = Arc::new(core::sync::atomic::AtomicU32::new(0));
        assert!(register(Box::new(CountedClock {
            nanoseconds: now,
            reads: reads.clone(),
            fails: false,
        })));
        let first = read_clock::<TestPlatform>().unwrap().1.unwrap();
        let second = read_clock::<TestPlatform>().unwrap().1.unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert_eq!(first, now);
        assert!(second >= now && second - now < NANOSECONDS_PER_SECOND);
        let later = now + 3600 * NANOSECONDS_PER_SECOND;
        assert_eq!(set::<TestPlatform>(later), Saved::InClock);
        let after_set = read_clock::<TestPlatform>().unwrap().1.unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert!(after_set >= later && after_set - later < NANOSECONDS_PER_SECOND);

        *CLOCK.lock() = None;
        let failing = Box::new(CountedClock {
            nanoseconds: now,
            reads: reads.clone(),
            fails: true,
        });
        assert!(register(failing));
        assert_eq!(
            read_clock::<TestPlatform>(),
            Some(("rtc-counted", Err(RtcError::Busy)))
        );
        assert_eq!(
            read_clock::<TestPlatform>(),
            Some(("rtc-counted", Err(RtcError::Busy)))
        );
        assert_eq!(reads.load(Ordering::Relaxed), 3);
        *CLOCK.lock() = before;
        HAS_CLOCK.store(had_clock, Ordering::Relaxed);
        host::reset();
    }

    #[derive(Debug)]
    struct LockWitness {
        offset_at_write: Arc<AtomicI64>,
        locked_at_write: Arc<AtomicBool>,
    }

    impl RealTimeClock for LockWitness {
        fn name(&self) -> &'static str {
            "rtc-witness"
        }

        fn read(&self) -> Result<u64, RtcError> {
            Ok(0)
        }

        fn write(&self, _nanoseconds: u64) -> Result<(), RtcError> {
            self.locked_at_write
                .store(CLOCK.try_lock().is_none(), Ordering::Relaxed);
            self.offset_at_write.store(
                OFFSET_NANOSECONDS.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            Ok(())
        }
    }

    #[test]
    fn a_set_moves_the_offset_and_writes_the_clock_in_one_hold_of_the_clock_s_lock() {
        let _state = crate::thread::host::frames_for_test();
        let before = CLOCK.lock().take();
        let had_clock = HAS_CLOCK.load(Ordering::Relaxed);
        let offset_at_write = Arc::new(AtomicI64::new(0));
        let locked_at_write = Arc::new(AtomicBool::new(false));
        assert!(register(Box::new(LockWitness {
            offset_at_write: offset_at_write.clone(),
            locked_at_write: locked_at_write.clone(),
        })));
        assert!(has_clock());
        let later = 1_893_456_000 * NANOSECONDS_PER_SECOND;
        assert_eq!(set::<TestPlatform>(later), Saved::InClock);
        assert!(locked_at_write.load(Ordering::Relaxed));
        assert_ne!(offset_at_write.load(Ordering::Relaxed), 0);
        assert_eq!(
            offset_at_write.load(Ordering::Relaxed),
            OFFSET_NANOSECONDS.load(Ordering::Relaxed)
        );
        *CLOCK.lock() = before;
        HAS_CLOCK.store(had_clock, Ordering::Relaxed);
        host::reset();
    }
}
