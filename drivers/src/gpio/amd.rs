//! AMD's GPIO controller (`AMDI0030`, D789): pin `n` is the 32-bit register at `4 n`, with its
//! trigger, polarity, interrupt enable and mask, and its sticky interrupt status, cleared by
//! writing it back; the controller's shared interrupt ends with the end-of-interrupt bit in its
//! master register. Places and bits are AMD's published register layout.

use super::{PinController, Trigger};
use crate::io::RegisterIo;

const PINS: u16 = 256;
const LEVEL_TRIGGER: u32 = 1 << 8;
const ACTIVE_SHIFT: u32 = 9;
const ACTIVE_MASK: u32 = 0x3 << ACTIVE_SHIFT;
const ACTIVE_HIGH: u32 = 0;
const ACTIVE_LOW: u32 = 1;
const BOTH_EDGES: u32 = 2;
const INTERRUPT_ENABLE: u32 = 1 << 11;
const INTERRUPT_UNMASKED: u32 = 1 << 12;
const INTERRUPT_STATUS: u32 = 1 << 28;
const WAKE_STATUS: u32 = 1 << 29;
const MASTER: usize = 0xfc;
const END_OF_INTERRUPT: u32 = 1 << 29;

/// AMD's GPIO controller at `regs`.
#[derive(Debug)]
pub struct AmdGpio<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> AmdGpio<R> {
    pub fn new(regs: R) -> Self {
        Self { regs }
    }

    fn place(pin: u16) -> usize {
        4 * usize::from(pin)
    }
}

impl<R: RegisterIo> PinController for AmdGpio<R> {
    fn arm(&mut self, pin: u16, trigger: Trigger) -> bool {
        if pin >= PINS {
            return false;
        }
        let at = Self::place(pin);
        let mut value = self.regs.read32(at) & !(LEVEL_TRIGGER | ACTIVE_MASK);
        if !trigger.edge {
            value |= LEVEL_TRIGGER;
        }
        let active = match (trigger.both_edges, trigger.active_low) {
            (true, _) => BOTH_EDGES,
            (false, true) => ACTIVE_LOW,
            (false, false) => ACTIVE_HIGH,
        };
        value |= active << ACTIVE_SHIFT | INTERRUPT_ENABLE | INTERRUPT_UNMASKED;
        self.regs
            .write32(at, value & !(INTERRUPT_STATUS | WAKE_STATUS));
        true
    }

    fn pending(&mut self, pin: u16) -> bool {
        pin < PINS && self.regs.read32(Self::place(pin)) & INTERRUPT_STATUS != 0
    }

    fn acknowledge(&mut self, pin: u16) {
        let at = Self::place(pin);
        let value = self.regs.read32(at);
        self.regs.write32(at, value | INTERRUPT_STATUS);
    }

    fn end_of_interrupt(&mut self) {
        let value = self.regs.read32(MASTER);
        self.regs.write32(MASTER, value | END_OF_INTERRUPT);
    }
}
