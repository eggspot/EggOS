//! Driver for the Arm PrimeCell PL011 UART, the serial port of most Arm boards
//! (including QEMU's AArch64 `virt` machine).
//!
//! Register layout and bit meanings are from Arm's public PL011 Technical
//! Reference Manual (DDI 0183).

use crate::io::Mmio;
use crate::{CharDevice, Driver, DriverError};

/// Data register.
const DR: usize = 0x00;
/// Flag register.
const FR: usize = 0x18;
/// Integer baud rate divisor.
const IBRD: usize = 0x24;
/// Fractional baud rate divisor.
const FBRD: usize = 0x28;
/// Line control register.
const LCR_H: usize = 0x2c;
/// Control register.
const CR: usize = 0x30;
/// Interrupt mask set/clear register.
const IMSC: usize = 0x38;
/// Peripheral identification register 0: the low byte of the part number.
const PERIPH_ID0: usize = 0xfe0;

const FR_BUSY: u32 = 1 << 3;
const FR_TXFF: u32 = 1 << 5;
const LCR_H_FEN: u32 = 1 << 4;
const LCR_H_WLEN_8: u32 = 0b11 << 5;
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;
/// Low byte of the PL011 part number, read from `PERIPH_ID0`.
const PL011_PART_NUMBER_LOW: u32 = 0x11;

/// A PL011 UART and the frequency of the clock that drives it.
pub struct Pl011 {
    regs: Mmio,
    clock_hz: u32,
}

impl Pl011 {
    /// Create a driver for the PL011 at `regs`, clocked at `clock_hz`.
    pub const fn new(regs: Mmio, clock_hz: u32) -> Self {
        Self { regs, clock_hz }
    }

    fn write_byte(&self, byte: u8) {
        while self.regs.read32(FR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        self.regs.write32(DR, u32::from(byte));
    }
}

/// Integer and fractional divisors (in 64ths) for `baud` from `clock_hz`.
fn baud_divisors(clock_hz: u32, baud: u32) -> (u32, u32) {
    // divisor = clock / (16 * baud); scaled by 64 and rounded to nearest.
    let scaled = (u64::from(clock_hz) * 4 + u64::from(baud) / 2) / u64::from(baud);
    ((scaled >> 6) as u32, (scaled & 0x3f) as u32)
}

impl Driver for Pl011 {
    fn name(&self) -> &'static str {
        "pl011"
    }

    fn init(&mut self) -> Result<(), DriverError> {
        if self.regs.read32(PERIPH_ID0) & 0xff != PL011_PART_NUMBER_LOW {
            return Err(DriverError::DeviceNotResponding);
        }
        // The TRM's programming sequence: disable, drain, configure, enable.
        self.regs.write32(CR, 0);
        while self.regs.read32(FR) & FR_BUSY != 0 {
            core::hint::spin_loop();
        }
        let (integer, fraction) = baud_divisors(self.clock_hz, 115_200);
        self.regs.write32(IBRD, integer);
        self.regs.write32(FBRD, fraction);
        self.regs.write32(LCR_H, LCR_H_WLEN_8 | LCR_H_FEN);
        self.regs.write32(IMSC, 0); // Polled output: no interrupts yet.
        self.regs.write32(CR, CR_UARTEN | CR_TXE | CR_RXE);
        Ok(())
    }
}

impl CharDevice for Pl011 {
    fn write_bytes(&self, bytes: &[u8]) {
        for &byte in bytes {
            // Serial terminals expect CR LF line endings.
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::baud_divisors;

    #[test]
    fn baud_divisors_match_the_trm_example() {
        // TRM section 3.3.6: 4 MHz clock, 230400 baud gives 1 and 5 (1.085).
        assert_eq!(baud_divisors(4_000_000, 230_400), (1, 5));
    }

    #[test]
    fn baud_divisors_for_a_24_mhz_clock() {
        // 24 MHz / (16 * 115200) = 13.0208, fraction 0.0208 * 64 = 1.33 -> 1.
        assert_eq!(baud_divisors(24_000_000, 115_200), (13, 1));
    }
}
