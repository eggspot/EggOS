//! The wall clock (D150): what `CLOCK_REALTIME`, `gettimeofday`, `time`, file times, the
//! desktop's clock and `date` read. It is the monotonic clock plus an offset, so a read costs
//! one relaxed load and one addition over the monotonic read (D73), with no lock and no
//! allocation. At boot, once the drivers have probed, [`start`] reads the real-time clock once
//! and sets the offset from it; [`set`] moves the offset and writes the real-time clock, the
//! only place a set time outlives a restart. Without a real-time clock the wall clock starts
//! at the epoch at boot, as Linux's does. All in UTC: there are no time zones.

use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicI64, Ordering};
use eggos_drivers::rtc::{NANOSECONDS_PER_SECOND, RealTimeClock, RtcError};
use eggos_hal::Platform;

static OFFSET_NANOSECONDS: AtomicI64 = AtomicI64::new(0);

enum Clock {
    Device(Box<dyn RealTimeClock + Send>),
    Platform,
}

static CLOCK: LeafSpinLock<Option<Box<Clock>>> = LeafSpinLock::new_leaf(None);

/// The wall clock in nanoseconds since 1970-01-01 00:00:00 UTC.
#[inline]
pub fn nanoseconds<P: Platform>() -> u64 {
    crate::linux::time::monotonic_nanoseconds::<P>()
        .saturating_add_signed(OFFSET_NANOSECONDS.load(Ordering::Relaxed))
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
    let clock = Box::new(Clock::Device(clock));
    let mut held = CLOCK.lock();
    if held.is_some() {
        return false;
    }
    *held = Some(clock);
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
    let has_device = CLOCK.lock().is_some();
    if !has_device {
        let mut found = false;
        P::platform_clock(&mut |_| found = true);
        if found {
            let platform = Box::new(Clock::Platform);
            let mut held = CLOCK.lock();
            if held.is_none() {
                *held = Some(platform);
            }
        }
    }
    match with_clock::<P, _>(|clock| (clock.name(), clock.read())) {
        Some((name, Ok(nanoseconds))) => {
            set_offset::<P>(nanoseconds);
            Started::FromClock {
                name,
                seconds: nanoseconds / NANOSECONDS_PER_SECOND,
            }
        }
        Some((name, Err(error))) => Started::Unreadable { name, error },
        None => Started::NoClock,
    }
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
    set_offset::<P>(nanoseconds);
    match with_clock::<P, _>(|clock| clock.write(nanoseconds)) {
        Some(Ok(())) => Saved::InClock,
        Some(Err(error)) => Saved::ClockFailed(error),
        None => Saved::NoClock,
    }
}

/// The real-time clock's name and its time now, in nanoseconds since the epoch, read from the
/// clock itself (`/proc/driver/rtc`, so `hwclock`); `None` without one.
pub fn read_clock<P: Platform>() -> Option<(&'static str, Result<u64, RtcError>)> {
    with_clock::<P, _>(|clock| (clock.name(), clock.read()))
}

/// Whether the machine has a real-time clock (known once [`start`] ran).
pub fn has_clock() -> bool {
    CLOCK.lock().is_some()
}

fn set_offset<P: Platform>(nanoseconds: u64) {
    let monotonic = crate::linux::time::monotonic_nanoseconds::<P>();
    let offset = i128::from(nanoseconds) - i128::from(monotonic);
    let offset = offset.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    OFFSET_NANOSECONDS.store(offset, Ordering::Relaxed);
}

fn with_clock<P: Platform, R>(use_clock: impl FnOnce(&dyn RealTimeClock) -> R) -> Option<R> {
    let held = CLOCK.lock();
    match held.as_deref()? {
        Clock::Device(device) => Some(use_clock(&**device)),
        Clock::Platform => {
            let mut use_clock = Some(use_clock);
            let mut result = None;
            P::platform_clock(&mut |platform| {
                if let Some(use_clock) = use_clock.take() {
                    result = Some(use_clock(platform));
                }
            });
            result
        }
    }
}

#[cfg(test)]
pub(crate) mod host {
    use super::*;

    pub(crate) fn reset() {
        OFFSET_NANOSECONDS.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform;

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
        let later = now + 3600 * NANOSECONDS_PER_SECOND;
        assert_eq!(set::<TestPlatform>(later), Saved::InClock);
        assert_eq!(written.load(Ordering::Relaxed), later);
        assert!(seconds::<TestPlatform>() >= 1_793_282_709 + 3600);
        assert_eq!(
            read_clock::<TestPlatform>().map(|(name, _)| name),
            Some("rtc-test")
        );
        *CLOCK.lock() = before;
        host::reset();
        assert!(nanoseconds::<TestPlatform>() < now);
    }
}
