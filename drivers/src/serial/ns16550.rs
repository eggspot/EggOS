//! NS16550-compatible UART (the PC serial port and most SoC UARTs).
//!
//! Registers are 8 bits wide, spaced `1 << reg_shift` bytes apart (0 on QEMU `virt` and for
//! x86 port I/O, 2 on many SoCs; the device tree's `reg-shift` property). The firmware has set
//! the baud rate and line format and left the divisor latch closed.

use super::Uart;
use crate::io::RegisterIo;

const RECEIVE_BUFFER: usize = 0;
const TRANSMIT_HOLDING: usize = 0;
const INTERRUPT_ENABLE: usize = 1;
const FIFO_CONTROL: usize = 2;
const MODEM_CONTROL: usize = 4;
const LINE_STATUS: usize = 5;

const LSR_DATA_READY: u8 = 1 << 0;
const LSR_TRANSMIT_HOLDING_EMPTY: u8 = 1 << 5;
const IER_RECEIVED_DATA_AVAILABLE: u8 = 1 << 0;
const FCR_ENABLE_FIFOS_TRIGGER_AT_ONE_BYTE: u8 = 1 << 0;
const MCR_DATA_TERMINAL_READY: u8 = 1 << 0;
const MCR_REQUEST_TO_SEND: u8 = 1 << 1;
const MCR_OUT2_GATES_INTERRUPT_LINE: u8 = 1 << 3;

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

    #[inline]
    fn receive_byte(&self) -> Option<u8> {
        (self.regs.read8(self.offset(LINE_STATUS)) & LSR_DATA_READY != 0)
            .then(|| self.regs.read8(self.offset(RECEIVE_BUFFER)))
    }

    /// Also raises OUT2 (with DTR and RTS), which a PC's serial port needs to connect its
    /// interrupt to the interrupt controller.
    fn enable_receive_interrupt(&self) {
        self.regs.write8(
            self.offset(FIFO_CONTROL),
            FCR_ENABLE_FIFOS_TRIGGER_AT_ONE_BYTE,
        );
        let modem = self.regs.read8(self.offset(MODEM_CONTROL));
        self.regs.write8(
            self.offset(MODEM_CONTROL),
            modem | MCR_DATA_TERMINAL_READY | MCR_REQUEST_TO_SEND | MCR_OUT2_GATES_INTERRUPT_LINE,
        );
        self.regs
            .write8(self.offset(INTERRUPT_ENABLE), IER_RECEIVED_DATA_AVAILABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    const EMPTY: u32 = LSR_TRANSMIT_HOLDING_EMPTY as u32;
    const READY: u32 = LSR_DATA_READY as u32;

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

    #[test]
    fn reads_bytes_while_data_is_ready_and_stops_when_the_buffer_is_full() {
        let uart = Ns16550::new(FakeRegs::default(), 0);
        uart.regs
            .script_reads(LINE_STATUS, &[READY, READY | EMPTY, 0]);
        uart.regs
            .script_reads(RECEIVE_BUFFER, &[u32::from(b'o'), u32::from(b'k')]);
        let mut buffer = [0; 4];
        assert_eq!(uart.read_bytes(&mut buffer), 2);
        assert_eq!(&buffer[..2], b"ok");
        assert_eq!(uart.regs.reads_of(RECEIVE_BUFFER), 2);

        uart.regs.script_reads(LINE_STATUS, &[READY, READY]);
        uart.regs
            .script_reads(RECEIVE_BUFFER, &[u32::from(b'a'), 0]);
        assert_eq!(uart.read_bytes(&mut buffer[..1]), 1);
        assert_eq!(buffer[0], b'a');
        assert_eq!(uart.regs.reads_of(RECEIVE_BUFFER), 3);
    }

    #[test]
    fn receive_interrupt_turns_the_fifo_on_raises_out2_and_keeps_other_modem_bits() {
        let uart = Ns16550::new(FakeRegs::default(), 0);
        let loopback = 1 << 4;
        uart.regs.script_reads(MODEM_CONTROL, &[loopback]);
        uart.enable_receive_interrupt();
        assert_eq!(
            uart.regs.writes(),
            vec![
                (FIFO_CONTROL, 1),
                (MODEM_CONTROL, loopback | 0b1011),
                (INTERRUPT_ENABLE, 1),
            ]
        );
    }
}
