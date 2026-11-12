//! I2C buses (D462): the two-wire bus laptops, tablets and phones hang their touchpads,
//! touch screens, keyboards and sensors on. A controller driver implements [`I2cBus`]: a
//! transfer that writes some bytes to a target and reads some back, with a repeated start in
//! between. Device protocols ([`crate::input::i2c_hid`]) are written against the trait, so
//! they are tested on the host against fake buses.

pub mod designware;

pub use designware::DesignWare;

/// The bus speeds a controller is set up for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    /// 100 kHz.
    Standard,
    /// 400 kHz, and Fast-mode Plus targets at 400 kHz too.
    Fast,
}

impl Speed {
    /// The speed a target's connection names, in Hz: Fast from 400 kHz on.
    pub const fn of_hz(hz: u32) -> Self {
        match hz >= 400_000 {
            true => Self::Fast,
            false => Self::Standard,
        }
    }
}

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cError {
    /// No target answered its address, or it refused a byte (the controller's abort source).
    NotAcknowledged(u32),
    /// The controller did not finish in time.
    TimedOut,
    /// The registers are not a controller this driver drives.
    NotThisController,
    /// A transfer with nothing to write and nothing to read, or too long for the bus.
    Invalid,
}

impl I2cError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            Self::NotAcknowledged(_) => "the I2C target did not acknowledge",
            Self::TimedOut => "the I2C controller did not finish in time",
            Self::NotThisController => "not a DesignWare I2C controller",
            Self::Invalid => "an empty or oversized I2C transfer",
        }
    }
}

/// A bus a driver talks to targets on.
pub trait I2cBus {
    /// Writes `write` to the target at `address` (7-bit, or 10-bit with `ten_bit`), then, after a
    /// repeated start, reads `read.len()` bytes from it, polling while `keep_waiting` says to;
    /// either part may be empty, not both.
    fn transfer(
        &mut self,
        address: u16,
        ten_bit: bool,
        write: &[u8],
        read: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), I2cError>;
}
