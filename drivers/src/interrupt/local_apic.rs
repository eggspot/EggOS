//! x86 local APIC in xAPIC mode, memory-mapped (Intel SDM vol. 3A, chapter 11): each CPU's
//! interrupt controller and its timer.

use crate::io::RegisterIo;

/// Task priority register.
const TPR: usize = 0x080;
/// End of interrupt register.
const EOI: usize = 0x0b0;
/// Spurious interrupt vector register.
const SVR: usize = 0x0f0;
/// LVT timer register.
const LVT_TIMER: usize = 0x320;
/// Timer initial count register.
const TIMER_INITIAL_COUNT: usize = 0x380;
/// Timer current count register.
const TIMER_CURRENT_COUNT: usize = 0x390;
/// Timer divide configuration register.
const TIMER_DIVIDE: usize = 0x3e0;

/// SVR bit 8: APIC software enable.
const SVR_ENABLE: u32 = 1 << 8;
/// LVT bit 16: masked.
const LVT_MASKED: u32 = 1 << 16;
/// Divide configuration 0b1011: divide the timer's input clock by 1.
const DIVIDE_BY_1: u32 = 0b1011;

/// Size of the local APIC's register block in bytes.
pub const REGISTER_BLOCK_SIZE: usize = 0x1000;

/// One CPU's local APIC. Every CPU sees its own at the same address.
#[derive(Debug)]
pub struct LocalApic<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> LocalApic<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Enables the APIC with `spurious_vector` for spurious interrupts, and accepts
    /// interrupts of every priority.
    pub fn enable(&self, spurious_vector: u8) {
        self.regs.write32(TPR, 0);
        self.regs
            .write32(SVR, SVR_ENABLE | u32::from(spurious_vector));
    }

    /// Sets the timer up in one-shot mode, counting at the APIC's input clock, raising
    /// `vector` when it reaches zero (or nothing, when `masked`). The timer stays stopped
    /// until [`Self::start_timer`].
    pub fn configure_one_shot_timer(&self, vector: u8, masked: bool) {
        // Stop it first: changing the LVT of a running timer is not defined to stop it.
        self.regs.write32(TIMER_INITIAL_COUNT, 0);
        self.regs.write32(TIMER_DIVIDE, DIVIDE_BY_1);
        let mask = if masked { LVT_MASKED } else { 0 };
        // Mode bits 18:17 = 00: one-shot.
        self.regs.write32(LVT_TIMER, mask | u32::from(vector));
    }

    /// Starts the timer counting down from `count`; 0 stops it.
    pub fn start_timer(&self, count: u32) {
        self.regs.write32(TIMER_INITIAL_COUNT, count);
    }

    /// The timer's current count (0 once it has expired).
    pub fn timer_count(&self) -> u32 {
        self.regs.read32(TIMER_CURRENT_COUNT)
    }

    /// Signals the end of the interrupt being handled.
    pub fn end_of_interrupt(&self) {
        self.regs.write32(EOI, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn enable_accepts_every_priority_and_sets_the_spurious_vector() {
        let apic = LocalApic::new(FakeRegs::default());
        apic.enable(0xff);
        assert_eq!(apic.regs.writes(), vec![(TPR, 0), (SVR, 0x1ff)]);
    }

    #[test]
    fn one_shot_timer_is_stopped_then_programmed() {
        let apic = LocalApic::new(FakeRegs::default());
        apic.configure_one_shot_timer(0x30, false);
        apic.configure_one_shot_timer(0x30, true);
        assert_eq!(
            apic.regs.writes(),
            vec![
                (TIMER_INITIAL_COUNT, 0),
                (TIMER_DIVIDE, 0b1011),
                (LVT_TIMER, 0x30),
                (TIMER_INITIAL_COUNT, 0),
                (TIMER_DIVIDE, 0b1011),
                (LVT_TIMER, 1 << 16 | 0x30),
            ]
        );
    }

    #[test]
    fn timer_counts_and_eoi() {
        let apic = LocalApic::new(FakeRegs::default());
        apic.regs.script_reads(TIMER_CURRENT_COUNT, &[1234]);
        apic.start_timer(5000);
        assert_eq!(apic.timer_count(), 1234);
        apic.end_of_interrupt();
        assert_eq!(
            apic.regs.writes(),
            vec![(TIMER_INITIAL_COUNT, 5000), (EOI, 0)]
        );
    }
}
