//! Arm PrimeCell PL011 UART (Arm DDI 0183), transmit path.
//!
//! Firmware or the emulator has already configured baud rate and line format; programming
//! them from the device tree's clock is part of the driver model milestone (M5).

use super::Uart;
use crate::io::RegisterIo;

/// Data register.
const DR: usize = 0x00;
/// Flag register.
const FR: usize = 0x18;
/// FR: transmit FIFO full.
const FR_TXFF: u32 = 1 << 5;

/// Size of the PL011 register block in bytes.
pub const REGISTER_BLOCK_SIZE: usize = 0x1000;

#[derive(Debug)]
pub struct Pl011<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Pl011<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }
}

impl<R: RegisterIo> Uart for Pl011<R> {
    #[inline]
    fn put_byte(&self, byte: u8) {
        while self.regs.read32(FR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        self.regs.write32(DR, byte.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn writes_bytes_to_data_register_translating_newlines() {
        let uart = Pl011::new(FakeRegs::default());
        uart.write_bytes(b"a\nb");
        let expected: Vec<(usize, u32)> = b"a\r\nb".iter().map(|&b| (DR, b.into())).collect();
        assert_eq!(uart.regs.writes(), expected);
    }

    #[test]
    fn waits_while_transmit_fifo_is_full() {
        let uart = Pl011::new(FakeRegs::default());
        uart.regs.script_reads(FR, &[FR_TXFF, FR_TXFF, 0]);
        uart.put_byte(b'x');
        assert_eq!(uart.regs.reads_of(FR), 3);
        assert_eq!(uart.regs.writes(), vec![(DR, u32::from(b'x'))]);
    }
}
