//! Google's Goldfish real-time clock, which QEMU's `virt` machine for RISC-V has (D150): a
//! 64-bit count of nanoseconds since the epoch in two 32-bit registers. Reading the low half
//! latches the high half, so the low one is read first; a write stores the high half, then the
//! low one, which sets the clock (QEMU's `goldfish_rtc`, Linux's `rtc-goldfish`).

use super::{RealTimeClock, RtcError};
use crate::io::RegisterIo;

/// The registers this driver uses: the time's low and high halves.
pub const REGISTER_BLOCK_SIZE: usize = 0x08;

/// Its device-tree `compatible`.
pub const COMPATIBLE: &[u8] = b"google,goldfish-rtc";

const TIME_LOW: usize = 0x00;
const TIME_HIGH: usize = 0x04;

/// A Goldfish real-time clock.
#[derive(Debug)]
pub struct Goldfish<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Goldfish<R> {
    /// The clock behind `regs` ([`REGISTER_BLOCK_SIZE`] bytes).
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }
}

impl<R: RegisterIo> RealTimeClock for Goldfish<R> {
    fn name(&self) -> &'static str {
        "rtc-goldfish"
    }

    fn read(&self) -> Result<u64, RtcError> {
        let low = self.regs.read32(TIME_LOW);
        let high = self.regs.read32(TIME_HIGH);
        Ok((u64::from(high) << 32) | u64::from(low))
    }

    fn write(&self, nanoseconds: u64) -> Result<(), RtcError> {
        self.regs.write32(TIME_HIGH, (nanoseconds >> 32) as u32);
        self.regs.write32(TIME_LOW, nanoseconds as u32);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::{Access, FakeRegs};

    const NOW: u64 = 1_793_282_709_123_456_789;

    #[test]
    fn it_reads_the_low_half_first_and_writes_it_last() {
        let clock = Goldfish::new(FakeRegs::default());
        clock.regs.script_reads(TIME_LOW, &[NOW as u32]);
        clock.regs.script_reads(TIME_HIGH, &[(NOW >> 32) as u32]);
        assert_eq!(clock.read(), Ok(NOW));
        assert_eq!(
            *clock.regs.log.borrow(),
            [Access::Read(TIME_LOW), Access::Read(TIME_HIGH)]
        );
        clock.write(NOW + 1).unwrap();
        assert_eq!(
            clock.regs.writes(),
            [(TIME_HIGH, ((NOW + 1) >> 32) as u32), (TIME_LOW, (NOW + 1) as u32)]
        );
        assert_eq!(clock.name(), "rtc-goldfish");
    }
}
