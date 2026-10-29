//! Arm's PrimeCell PL031 real-time clock (its technical reference manual, DDI 0224), which
//! QEMU's `virt` machine for AArch64 has (D150): a 32-bit count of seconds since the epoch
//! that runs once started, read from the data register and set through the load register.

use super::{NANOSECONDS_PER_SECOND, RealTimeClock, RtcError};
use crate::io::RegisterIo;

/// The registers this driver uses: data, match, load and control.
pub const REGISTER_BLOCK_SIZE: usize = 0x10;

/// Its device-tree `compatible`.
pub const COMPATIBLE: &[u8] = b"arm,pl031";

const DATA: usize = 0x00;
const LOAD: usize = 0x08;
const CONTROL: usize = 0x0c;
const CONTROL_START: u32 = 1;

/// A PL031.
#[derive(Debug)]
pub struct Pl031<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Pl031<R> {
    /// The clock behind `regs` ([`REGISTER_BLOCK_SIZE`] bytes).
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Starts its counter if the firmware left it stopped, as Linux's driver does.
    pub fn start(&self) {
        let control = self.regs.read32(CONTROL);
        if control & CONTROL_START == 0 {
            self.regs.write32(CONTROL, control | CONTROL_START);
        }
    }
}

impl<R: RegisterIo> RealTimeClock for Pl031<R> {
    fn name(&self) -> &'static str {
        "rtc-pl031"
    }

    fn read(&self) -> Result<u64, RtcError> {
        Ok(u64::from(self.regs.read32(DATA)) * NANOSECONDS_PER_SECOND)
    }

    fn write(&self, nanoseconds: u64) -> Result<(), RtcError> {
        let seconds = u32::try_from(nanoseconds / NANOSECONDS_PER_SECOND)
            .map_err(|_| RtcError::OutOfRange)?;
        self.regs.write32(LOAD, seconds);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn it_reads_and_loads_whole_seconds() {
        let clock = Pl031::new(FakeRegs::default());
        clock.regs.script_reads(DATA, &[1_793_282_709]);
        assert_eq!(clock.read(), Ok(1_793_282_709 * NANOSECONDS_PER_SECOND));
        clock
            .write(1_793_282_710 * NANOSECONDS_PER_SECOND + 500)
            .unwrap();
        assert_eq!(clock.regs.writes(), [(LOAD, 1_793_282_710)]);
        assert_eq!(
            clock.write(u64::from(u32::MAX) * NANOSECONDS_PER_SECOND + NANOSECONDS_PER_SECOND),
            Err(RtcError::OutOfRange)
        );
        assert_eq!(clock.name(), "rtc-pl031");
    }

    #[test]
    fn start_sets_the_start_bit_only_when_it_is_clear() {
        let clock = Pl031::new(FakeRegs::default());
        clock.regs.script_reads(CONTROL, &[0, CONTROL_START]);
        clock.start();
        clock.start();
        assert_eq!(clock.regs.writes(), [(CONTROL, CONTROL_START)]);
    }
}
