//! x86 local APIC in xAPIC mode, memory-mapped (Intel SDM vol. 3A, chapter 11): each CPU's
//! interrupt controller, its timer, and the inter-processor interrupts that start the other
//! CPUs.

use crate::io::RegisterIo;

/// Local APIC ID register: the id in bits 24-31.
const ID: usize = 0x020;
/// Task priority register.
const TPR: usize = 0x080;
/// End of interrupt register.
const EOI: usize = 0x0b0;
/// Spurious interrupt vector register.
const SVR: usize = 0x0f0;
/// Interrupt command register, low and high halves (SDM 11.6.1). Writing the low half sends.
const ICR_LOW: usize = 0x300;
const ICR_HIGH: usize = 0x310;
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

/// ICR delivery modes (bits 8-10): INIT and start-up.
const ICR_INIT: u32 = 0b101 << 8;
const ICR_STARTUP: u32 = 0b110 << 8;
/// ICR bit 12: the previous interrupt has not been accepted yet (read-only).
const ICR_SEND_PENDING: u32 = 1 << 12;
/// ICR bit 14: level assert, which INIT and start-up interrupts require.
const ICR_ASSERT: u32 = 1 << 14;
/// ICR destination shorthand (bits 18-19): every local APIC but the sender's.
const ICR_ALL_EXCLUDING_SELF: u32 = 0b11 << 18;
/// ICR high half: the destination APIC id in bits 24-31.
const ICR_DESTINATION_SHIFT: u32 = 24;

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

    /// This CPU's local APIC id.
    pub fn id(&self) -> u8 {
        (self.regs.read32(ID) >> 24) as u8
    }

    /// Sends INIT to every other CPU, which resets it into waiting for a start-up interrupt
    /// (SDM 9.4.4.1). Waits until any earlier interrupt was accepted first.
    pub fn send_init_to_others(&self) {
        self.wait_until_sent();
        self.regs
            .write32(ICR_LOW, ICR_ALL_EXCLUDING_SELF | ICR_ASSERT | ICR_INIT);
    }

    /// Sends a start-up interrupt to the CPU whose local APIC id is `destination`: a CPU
    /// waiting after INIT starts in real mode at physical address `page` * 4 KiB.
    pub fn send_startup(&self, destination: u8, page: u8) {
        self.wait_until_sent();
        self.regs
            .write32(ICR_HIGH, u32::from(destination) << ICR_DESTINATION_SHIFT);
        self.regs
            .write32(ICR_LOW, ICR_ASSERT | ICR_STARTUP | u32::from(page));
    }

    /// Waits until the last interrupt this CPU sent was accepted, so the ICR can be written.
    fn wait_until_sent(&self) {
        while self.regs.read32(ICR_LOW) & ICR_SEND_PENDING != 0 {
            core::hint::spin_loop();
        }
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
    fn reads_the_id_from_the_top_byte() {
        let apic = LocalApic::new(FakeRegs::default());
        apic.regs.script_reads(ID, &[0x0300_0000]);
        assert_eq!(apic.id(), 3);
    }

    #[test]
    fn init_and_startup_wait_for_the_previous_interrupt_then_send() {
        let apic = LocalApic::new(FakeRegs::default());
        // The first send waits through two pending reads.
        apic.regs
            .script_reads(ICR_LOW, &[ICR_SEND_PENDING, ICR_SEND_PENDING, 0, 0]);
        apic.send_init_to_others();
        apic.send_startup(7, 0x08);
        assert_eq!(apic.regs.reads_of(ICR_LOW), 4);
        assert_eq!(
            apic.regs.writes(),
            vec![(ICR_LOW, 0xc_4500), (ICR_HIGH, 7 << 24), (ICR_LOW, 0x4608),]
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
