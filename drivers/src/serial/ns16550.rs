//! NS16550-compatible UART (the PC serial port and most SoC UARTs), transmit path.
//!
//! Registers are 8 bits wide, spaced `1 << reg_shift` bytes apart (0 on QEMU `virt` and for
//! x86 port I/O, 2 on many SoCs; the device tree's `reg-shift` property).

use super::Uart;
use crate::io::RegisterIo;

const TRANSMIT_HOLDING: usize = 0;
const LINE_STATUS: usize = 5;
const LSR_TRANSMIT_HOLDING_EMPTY: u8 = 1 << 5;

#[derive(Debug)]
pub struct Ns16550<R: RegisterIo> {
    regs: R,
    reg_shift: u32,
}

impl<R: RegisterIo> Ns16550<R> {
    pub const fn new(regs: R, reg_shift: u32) -> Self {
        Self { regs, reg_shift }
    }

    /// Size of the register block in bytes for a given `reg_shift`.
    pub const fn register_block_size(reg_shift: u32) -> usize {
        8 << reg_shift
    }

    #[inline]
    fn offset(&self, register: usize) -> usize {
        register << self.reg_shift
    }
}

impl<R: RegisterIo> Uart for Ns16550<R> {
    #[inline]
    fn put_byte(&self, byte: u8) {
        while self.regs.read8(self.offset(LINE_STATUS)) & LSR_TRANSMIT_HOLDING_EMPTY == 0 {
            core::hint::spin_loop();
        }
        self.regs.write8(self.offset(TRANSMIT_HOLDING), byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    const EMPTY: u32 = LSR_TRANSMIT_HOLDING_EMPTY as u32;

    #[test]
    fn waits_for_empty_holding_register_then_writes() {
        let uart = Ns16550::new(FakeRegs::default(), 0);
        uart.regs.script_reads(LINE_STATUS, &[0, 0, EMPTY]);
        uart.put_byte(b'x');
        assert_eq!(uart.regs.reads_of(LINE_STATUS), 3);
        assert_eq!(
            uart.regs.writes(),
            vec![(TRANSMIT_HOLDING, u32::from(b'x'))]
        );
    }

    #[test]
    fn honours_register_shift() {
        let uart = Ns16550::new(FakeRegs::default(), 2);
        uart.regs.script_reads(LINE_STATUS << 2, &[EMPTY, EMPTY]);
        uart.write_bytes(b"\n");
        assert_eq!(uart.regs.reads_of(LINE_STATUS << 2), 2);
        assert_eq!(
            uart.regs.writes(),
            vec![(0, u32::from(b'\r')), (0, u32::from(b'\n'))]
        );
        assert_eq!(Ns16550::<FakeRegs>::register_block_size(2), 32);
    }
}
