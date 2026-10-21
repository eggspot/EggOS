//! Driver for the 16550-compatible UART, the serial port of PCs and of many
//! other boards (including QEMU's RISC-V `virt` machine).
//!
//! Register layout and bit meanings are from the public PC16550D datasheet.

use crate::io::ByteRegisters;
use crate::{CharDevice, Driver, DriverError};

/// Transmit holding register (write) / receive buffer (read); divisor latch low
/// byte while `LCR_DLAB` is set.
const THR: usize = 0;
/// Interrupt enable register; divisor latch high byte while `LCR_DLAB` is set.
const IER: usize = 1;
/// FIFO control register (write only).
const FCR: usize = 2;
/// Line control register.
const LCR: usize = 3;
/// Modem control register.
const MCR: usize = 4;
/// Line status register.
const LSR: usize = 5;
/// Scratch register: no hardware effect, used to check the UART is present.
const SCR: usize = 7;

const LCR_DLAB: u8 = 1 << 7;
const LCR_8N1: u8 = 0b0000_0011;
const FCR_ENABLE_AND_CLEAR: u8 = 0b0000_0111;
const MCR_DTR_RTS: u8 = 0b0000_0011;
const LSR_THR_EMPTY: u8 = 1 << 5;

/// Divisor for 115200 baud from the standard 1.8432 MHz UART clock.
const DIVISOR_115200: u16 = 1;

/// A 16550 UART reached through `R`.
pub struct Uart16550<R> {
    regs: R,
}

impl<R: ByteRegisters> Uart16550<R> {
    /// Create a driver for the UART whose registers are `regs`.
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn write_byte(&self, byte: u8) {
        while self.regs.read8(LSR) & LSR_THR_EMPTY == 0 {
            core::hint::spin_loop();
        }
        self.regs.write8(THR, byte);
    }
}

impl<R: ByteRegisters> Driver for Uart16550<R> {
    fn name(&self) -> &'static str {
        "uart16550"
    }

    fn init(&mut self) -> Result<(), DriverError> {
        self.regs.write8(SCR, 0x5a);
        if self.regs.read8(SCR) != 0x5a {
            return Err(DriverError::DeviceNotResponding);
        }
        let [divisor_low, divisor_high] = DIVISOR_115200.to_le_bytes();
        self.regs.write8(IER, 0); // Polled output: no interrupts yet.
        self.regs.write8(LCR, LCR_DLAB);
        self.regs.write8(THR, divisor_low);
        self.regs.write8(IER, divisor_high);
        self.regs.write8(LCR, LCR_8N1);
        self.regs.write8(FCR, FCR_ENABLE_AND_CLEAR);
        self.regs.write8(MCR, MCR_DTR_RTS);
        Ok(())
    }
}

impl<R: ByteRegisters> CharDevice for Uart16550<R> {
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
    use super::*;
    use crate::io::fake::FakeRegisters;

    fn idle_uart() -> Uart16550<FakeRegisters> {
        let mut values = [0; 8];
        values[LSR] = LSR_THR_EMPTY;
        Uart16550::new(FakeRegisters::new(values))
    }

    #[test]
    fn init_programs_115200_8n1_with_fifo() {
        let mut uart = idle_uart();
        assert_eq!(uart.init(), Ok(()));
        let writes = uart.regs.writes.borrow();
        assert_eq!(
            writes[1..],
            [
                (IER, 0),
                (LCR, LCR_DLAB),
                (THR, 1),
                (IER, 0),
                (LCR, LCR_8N1),
                (FCR, FCR_ENABLE_AND_CLEAR),
                (MCR, MCR_DTR_RTS),
            ]
        );
    }

    #[test]
    fn init_fails_when_scratch_register_does_not_hold_a_value() {
        struct Absent;
        impl ByteRegisters for Absent {
            fn read8(&self, _: usize) -> u8 {
                0xff
            }
            fn write8(&self, _: usize, _: u8) {}
        }
        assert_eq!(
            Uart16550::new(Absent).init(),
            Err(DriverError::DeviceNotResponding)
        );
    }

    #[test]
    fn write_bytes_translates_newline_to_crlf() {
        let uart = idle_uart();
        uart.write_bytes(b"a\nb");
        let sent: Vec<u8> = uart
            .regs
            .writes
            .borrow()
            .iter()
            .map(|&(_, byte)| byte)
            .collect();
        assert_eq!(sent, b"a\r\nb");
    }
}
