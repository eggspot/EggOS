//! The High Precision Event Timer (D594, IA-PC HPET specification 1.0a): its main counter,
//! and its comparators as one-shot timers that raise a message (front-side-bus delivery).

use crate::io::RegisterIo;

const CAPABILITIES: usize = 0x000;
const CONFIGURATION: usize = 0x010;
const COUNTER: usize = 0x0f0;
const COMPARATORS: usize = 0x100;
const COMPARATOR_STRIDE: usize = 0x20;
const COMPARATOR_VALUE: usize = 0x08;
const COMPARATOR_ROUTE: usize = 0x10;
const ENABLE: u64 = 1;
const COUNTER_64_BIT: u64 = 1 << 13;
const COMPARATOR_COUNT_SHIFT: u32 = 8;
const COMPARATOR_COUNT_MASK: u64 = 0x1f;
const PERIOD_SHIFT: u32 = 32;
const INTERRUPT_ENABLE: u64 = 1 << 2;
const PERIODIC: u64 = 1 << 3;
const LEVEL_TRIGGERED: u64 = 1 << 1;
const MESSAGE_ENABLE: u64 = 1 << 14;
const MESSAGE_CAPABLE: u64 = 1 << 15;
const FEMTOSECONDS_PER_SECOND: u64 = 1_000_000_000_000_000;

/// An HPET's register block.
#[derive(Debug)]
pub struct Hpet<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Hpet<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn capabilities(&self) -> u64 {
        self.regs.read64(CAPABILITIES)
    }

    /// Its counter's rate in hertz, from the period it states; `None` for a period of 0.
    pub fn frequency(&self) -> Option<u64> {
        let period = self.capabilities() >> PERIOD_SHIFT;
        (period != 0).then(|| FEMTOSECONDS_PER_SECOND / period)
    }

    /// How many comparators it has.
    pub fn comparators(&self) -> usize {
        ((self.capabilities() >> COMPARATOR_COUNT_SHIFT & COMPARATOR_COUNT_MASK) + 1) as usize
    }

    /// Starts its main counter, if it has not.
    pub fn enable(&self) {
        let configuration = self.regs.read64(CONFIGURATION);
        if configuration & ENABLE == 0 {
            self.regs.write64(CONFIGURATION, configuration | ENABLE);
        }
    }

    /// The main counter; a 32-bit counter reads as its 32 bits.
    pub fn counter(&self) -> u64 {
        match self.capabilities() & COUNTER_64_BIT {
            0 => u64::from(self.regs.read32(COUNTER)),
            _ => self.regs.read64(COUNTER),
        }
    }

    fn comparator(index: usize, register: usize) -> usize {
        COMPARATORS + index * COMPARATOR_STRIDE + register
    }

    /// Whether comparator `index` can raise a message.
    pub fn can_send_messages(&self, index: usize) -> bool {
        index < self.comparators()
            && self.regs.read64(Self::comparator(index, 0)) & MESSAGE_CAPABLE != 0
    }

    /// Has comparator `index` write `data` to `address` once, when the main counter reaches
    /// `deadline`: edge-triggered, one-shot.
    pub fn arm_message(&self, index: usize, deadline: u64, address: u32, data: u32) {
        let configuration = Self::comparator(index, 0);
        let old = self.regs.read64(configuration);
        self.regs
            .write64(configuration, old & !(INTERRUPT_ENABLE | PERIODIC | LEVEL_TRIGGERED));
        self.regs.write64(
            Self::comparator(index, COMPARATOR_ROUTE),
            u64::from(address) << 32 | u64::from(data),
        );
        self.regs
            .write64(Self::comparator(index, COMPARATOR_VALUE), deadline);
        self.regs.write64(
            configuration,
            old & !(PERIODIC | LEVEL_TRIGGERED) | MESSAGE_ENABLE | INTERRUPT_ENABLE,
        );
    }

    /// Stops comparator `index` from raising anything.
    pub fn disarm(&self, index: usize) {
        let configuration = Self::comparator(index, 0);
        let old = self.regs.read64(configuration);
        self.regs.write64(configuration, old & !INTERRUPT_ENABLE);
    }
}
