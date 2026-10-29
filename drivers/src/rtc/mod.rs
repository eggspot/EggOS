//! Real-time clocks (D150): the clock that keeps the date and time while the machine is off, or,
//! in a virtual machine, follows the host's. The kernel reads one once at boot to set the wall
//! clock and writes it when the clock is set. Each driver reaches its registers through
//! [`RegisterIo`](crate::io::RegisterIo), so it is tested on the host against fake ones, and
//! its calendar arithmetic is `eggos-calendar`'s.

pub mod cmos;
pub mod goldfish;
pub mod pl031;

pub use cmos::Cmos;
pub use goldfish::Goldfish;
pub use pl031::Pl031;

/// Nanoseconds in a second.
pub const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// Why a clock could not be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcError {
    /// It was updating through every attempt to read it.
    Busy,
    /// Its registers hold no valid date and time.
    Invalid,
    /// The time cannot be stored in it.
    OutOfRange,
}

impl RtcError {
    /// What went wrong, for the boot log and `hwclock`.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Busy => "the clock kept updating while it was read",
            Self::Invalid => "the clock holds no valid date and time",
            Self::OutOfRange => "the clock cannot hold that time",
        }
    }
}

/// A real-time clock.
pub trait RealTimeClock {
    /// Its driver's name, as the boot log, `devices` and `/proc/driver/rtc` show it.
    fn name(&self) -> &'static str;

    /// The time it keeps, in nanoseconds since 1970-01-01 00:00:00 UTC.
    fn read(&self) -> Result<u64, RtcError>;

    /// Sets it to `nanoseconds` since the epoch, as precisely as it keeps time (whole seconds
    /// for most).
    fn write(&self, nanoseconds: u64) -> Result<(), RtcError>;
}
