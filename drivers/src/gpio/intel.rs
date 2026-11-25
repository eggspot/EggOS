//! Intel's GPIO pin controllers from Cannon Lake on (D789): one register block per community,
//! each listing where its pad configuration starts (`PADBAR`); pads are numbered across the
//! communities in their order, 32 to an interrupt status and enable word. A pad armed for an
//! interrupt is a GPIO input routed to the controller's own interrupt with its trigger and
//! inversion, its status cleared by writing it back. Places and fields are from Intel's
//! published PCH datasheets' GPIO chapters.

use super::{PinController, Trigger};
use crate::io::RegisterIo;
use alloc::vec::Vec;

const PAD_BASE: usize = 0x00c;
const INTERRUPT_STATUS: usize = 0x100;
const INTERRUPT_ENABLE: usize = 0x120;
const PAD_BYTES: usize = 16;
const PAD_MODE_MASK: u32 = 0x7 << 10;
const RX_DISABLE: u32 = 1 << 9;
const TX_DISABLE: u32 = 1 << 8;
const ROUTE_MASK: u32 = 0xf << 17;
const RX_INVERT: u32 = 1 << 23;
const RX_EVENT_SHIFT: u32 = 25;
const RX_EVENT_MASK: u32 = 0x3 << RX_EVENT_SHIFT;
const EVENT_LEVEL: u32 = 0;
const EVENT_EDGE: u32 = 1;
const EVENT_BOTH: u32 = 3;

/// One community: its registers and how many pads it has.
#[derive(Debug)]
pub struct Community<R: RegisterIo> {
    pub regs: R,
    pub pads: u16,
}

/// Intel's pin controller: its communities in order.
#[derive(Debug)]
pub struct IntelGpio<R: RegisterIo> {
    communities: Vec<Community<R>>,
}

impl<R: RegisterIo> IntelGpio<R> {
    pub fn new(communities: Vec<Community<R>>) -> Self {
        Self { communities }
    }

    fn find(&self, pin: u16) -> Option<(&Community<R>, usize)> {
        let mut first = 0u16;
        for community in &self.communities {
            if pin < first + community.pads {
                return Some((community, usize::from(pin - first)));
            }
            first += community.pads;
        }
        None
    }
}

impl<R: RegisterIo> PinController for IntelGpio<R> {
    fn arm(&mut self, pin: u16, trigger: Trigger) -> bool {
        let Some((community, pad)) = self.find(pin) else {
            return false;
        };
        let regs = &community.regs;
        let config = regs.read32(PAD_BASE) as usize + pad * PAD_BYTES;
        let event = match (trigger.both_edges, trigger.edge) {
            (true, _) => EVENT_BOTH,
            (false, true) => EVENT_EDGE,
            (false, false) => EVENT_LEVEL,
        };
        let mut value = regs.read32(config);
        value &= !(PAD_MODE_MASK | RX_DISABLE | ROUTE_MASK | RX_EVENT_MASK | RX_INVERT);
        value |= TX_DISABLE | event << RX_EVENT_SHIFT;
        if trigger.active_low && !trigger.both_edges {
            value |= RX_INVERT;
        }
        regs.write32(config, value);
        let word = 4 * (pad / 32);
        let bit = 1u32 << (pad % 32);
        regs.write32(INTERRUPT_STATUS + word, bit);
        let enabled = regs.read32(INTERRUPT_ENABLE + word);
        regs.write32(INTERRUPT_ENABLE + word, enabled | bit);
        true
    }

    fn pending(&mut self, pin: u16) -> bool {
        self.find(pin).is_some_and(|(community, pad)| {
            community.regs.read32(INTERRUPT_STATUS + 4 * (pad / 32)) & 1 << (pad % 32) != 0
        })
    }

    fn acknowledge(&mut self, pin: u16) {
        if let Some((community, pad)) = self.find(pin) {
            community
                .regs
                .write32(INTERRUPT_STATUS + 4 * (pad / 32), 1 << (pad % 32));
        }
    }
}
