//! Arm PrimeCell PL011 UART (Arm DDI 0183).
//!
//! Firmware or the emulator has already configured baud rate and line format; programming
//! them from the device tree's clock is part of the driver model milestone (M5).

use super::Uart;
use crate::io::RegisterIo;

const DATA: usize = 0x00;
const FLAGS: usize = 0x18;
const LINE_CONTROL: usize = 0x2c;
const CONTROL: usize = 0x30;
const INTERRUPT_MASK_SET_CLEAR: usize = 0x38;

const FR_BUSY: u32 = 1 << 3;
const FR_RECEIVE_FIFO_EMPTY: u32 = 1 << 4;
const FR_TRANSMIT_FIFO_FULL: u32 = 1 << 5;
const DATA_BYTE_MASK: u32 = 0xff;
const LCR_H_ENABLE_FIFOS: u32 = 1 << 4;
const CR_UART_ENABLE: u32 = 1 << 0;
const IMSC_RECEIVE: u32 = 1 << 4;
const IMSC_RECEIVE_TIMEOUT: u32 = 1 << 6;

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
        while self.regs.read32(FLAGS) & FR_TRANSMIT_FIFO_FULL != 0 {
            core::hint::spin_loop();
        }
        self.regs.write32(DATA, byte.into());
    }

    #[inline]
    fn receive_byte(&self) -> Option<u8> {
        (self.regs.read32(FLAGS) & FR_RECEIVE_FIFO_EMPTY == 0)
            .then(|| (self.regs.read32(DATA) & DATA_BYTE_MASK) as u8)
    }

    /// The line control register may only change while the UART is disabled and idle (DDI 0183,
    /// 3.3.8), so this waits for the transmitter to finish and disables the UART around it.
    fn enable_receive_interrupt(&self) {
        while self.regs.read32(FLAGS) & FR_BUSY != 0 {
            core::hint::spin_loop();
        }
        let control = self.regs.read32(CONTROL);
        self.regs.write32(CONTROL, control & !CR_UART_ENABLE);
        let line = self.regs.read32(LINE_CONTROL);
        self.regs.write32(LINE_CONTROL, line | LCR_H_ENABLE_FIFOS);
        self.regs.write32(CONTROL, control);
        let mask = self.regs.read32(INTERRUPT_MASK_SET_CLEAR);
        self.regs.write32(
            INTERRUPT_MASK_SET_CLEAR,
            mask | IMSC_RECEIVE | IMSC_RECEIVE_TIMEOUT,
        );
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
        let expected: Vec<(usize, u32)> = b"a\r\nb".iter().map(|&b| (DATA, b.into())).collect();
        assert_eq!(uart.regs.writes(), expected);
    }

    #[test]
    fn waits_while_transmit_fifo_is_full() {
        let uart = Pl011::new(FakeRegs::default());
        uart.regs
            .script_reads(FLAGS, &[FR_TRANSMIT_FIFO_FULL, FR_TRANSMIT_FIFO_FULL, 0]);
        uart.put_byte(b'x');
        assert_eq!(uart.regs.reads_of(FLAGS), 3);
        assert_eq!(uart.regs.writes(), vec![(DATA, u32::from(b'x'))]);
    }

    #[test]
    fn receives_until_the_fifo_is_empty_dropping_the_error_bits() {
        let uart = Pl011::new(FakeRegs::default());
        let overrun = 1 << 11;
        uart.regs
            .script_reads(FLAGS, &[0, 0, FR_RECEIVE_FIFO_EMPTY, FR_RECEIVE_FIFO_EMPTY]);
        uart.regs
            .script_reads(DATA, &[overrun | u32::from(b'h'), u32::from(b'i')]);
        let mut buffer = [0; 8];
        assert_eq!(uart.read_bytes(&mut buffer), 2);
        assert_eq!(&buffer[..2], b"hi");
        assert_eq!(uart.receive_byte(), None);
    }

    #[test]
    fn receive_interrupt_reprograms_the_fifos_only_while_the_uart_is_disabled() {
        let uart = Pl011::new(FakeRegs::default());
        let enabled_for_both_directions = 0x301;
        let eight_bits = 0b11 << 5;
        let transmit_interrupt = 1 << 5;
        uart.regs.script_reads(FLAGS, &[FR_BUSY, 0]);
        uart.regs
            .script_reads(CONTROL, &[enabled_for_both_directions]);
        uart.regs.script_reads(LINE_CONTROL, &[eight_bits]);
        uart.regs
            .script_reads(INTERRUPT_MASK_SET_CLEAR, &[transmit_interrupt]);
        uart.enable_receive_interrupt();
        assert_eq!(uart.regs.reads_of(FLAGS), 2);
        assert_eq!(
            uart.regs.writes(),
            vec![
                (CONTROL, 0x300),
                (LINE_CONTROL, eight_bits | LCR_H_ENABLE_FIFOS),
                (CONTROL, enabled_for_both_directions),
                (
                    INTERRUPT_MASK_SET_CLEAR,
                    transmit_interrupt | IMSC_RECEIVE | IMSC_RECEIVE_TIMEOUT
                ),
            ]
        );
    }
}
