//! x86 local APIC in xAPIC mode, memory-mapped (Intel SDM vol. 3A, chapter 11): each CPU's
//! interrupt controller, its timer, and the inter-processor interrupts that start and wake the
//! other CPUs.

use crate::io::RegisterIo;

const ID: usize = 0x020;
const ID_SHIFT: u32 = 24;
const TASK_PRIORITY: usize = 0x080;
const END_OF_INTERRUPT: usize = 0x0b0;
const SPURIOUS_INTERRUPT_VECTOR: usize = 0x0f0;
const ICR_LOW: usize = 0x300;
const ICR_HIGH: usize = 0x310;
const LVT_TIMER: usize = 0x320;
const TIMER_INITIAL_COUNT: usize = 0x380;
const TIMER_CURRENT_COUNT: usize = 0x390;
const TIMER_DIVIDE_CONFIGURATION: usize = 0x3e0;

const SVR_APIC_SOFTWARE_ENABLE: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_MODE_ONE_SHOT: u32 = 0b00 << 17;
const DIVIDE_BY_1: u32 = 0b1011;

const ICR_DELIVERY_FIXED: u32 = 0b000 << 8;
const ICR_DELIVERY_INIT: u32 = 0b101 << 8;
const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
const ICR_SEND_PENDING: u32 = 1 << 12;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_ALL_EXCLUDING_SELF: u32 = 0b11 << 18;
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
        self.regs.write32(TASK_PRIORITY, 0);
        self.regs.write32(
            SPURIOUS_INTERRUPT_VECTOR,
            SVR_APIC_SOFTWARE_ENABLE | u32::from(spurious_vector),
        );
    }

    /// Sets the timer up in one-shot mode, counting at the APIC's input clock, raising
    /// `vector` when it reaches zero (or nothing, when `masked`). The timer stays stopped
    /// until [`Self::start_timer`].
    pub fn configure_one_shot_timer(&self, vector: u8, masked: bool) {
        self.stop_timer_before_changing_its_lvt();
        self.regs.write32(TIMER_DIVIDE_CONFIGURATION, DIVIDE_BY_1);
        let mask = if masked { LVT_MASKED } else { 0 };
        self.regs.write32(
            LVT_TIMER,
            LVT_TIMER_MODE_ONE_SHOT | mask | u32::from(vector),
        );
    }

    fn stop_timer_before_changing_its_lvt(&self) {
        self.regs.write32(TIMER_INITIAL_COUNT, 0);
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
        self.regs.write32(END_OF_INTERRUPT, 0);
    }

    /// This CPU's local APIC id.
    pub fn id(&self) -> u8 {
        (self.regs.read32(ID) >> ID_SHIFT) as u8
    }

    /// Sends INIT to every other CPU, which resets it into waiting for a start-up interrupt
    /// (SDM 9.4.4.1). Waits until any earlier interrupt was accepted first.
    pub fn send_init_to_others(&self) {
        self.wait_until_sent();
        self.send(ICR_ALL_EXCLUDING_SELF | ICR_LEVEL_ASSERT | ICR_DELIVERY_INIT);
    }

    /// Sends a start-up interrupt to the CPU whose local APIC id is `destination`: a CPU
    /// waiting after INIT starts in real mode at physical address `page` * 4 KiB.
    pub fn send_startup(&self, destination: u8, page: u8) {
        self.wait_until_sent();
        self.regs
            .write32(ICR_HIGH, u32::from(destination) << ICR_DESTINATION_SHIFT);
        self.send(ICR_LEVEL_ASSERT | ICR_DELIVERY_STARTUP | u32::from(page));
    }

    /// Sends the interrupt `vector` to the CPU whose local APIC id is `destination` (fixed
    /// delivery, physical destination mode).
    pub fn send_fixed(&self, destination: u8, vector: u8) {
        self.wait_until_sent();
        self.regs
            .write32(ICR_HIGH, u32::from(destination) << ICR_DESTINATION_SHIFT);
        self.send(ICR_LEVEL_ASSERT | ICR_DELIVERY_FIXED | u32::from(vector));
    }

    fn send(&self, command: u32) {
        self.regs.write32(ICR_LOW, command);
    }

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
        assert_eq!(
            apic.regs.writes(),
            vec![(TASK_PRIORITY, 0), (SPURIOUS_INTERRUPT_VECTOR, 0x1ff)]
        );
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
                (TIMER_DIVIDE_CONFIGURATION, 0b1011),
                (LVT_TIMER, 0x30),
                (TIMER_INITIAL_COUNT, 0),
                (TIMER_DIVIDE_CONFIGURATION, 0b1011),
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
    fn init_waits_through_two_pending_reads_then_startup_through_none() {
        let apic = LocalApic::new(FakeRegs::default());
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
    fn a_fixed_interrupt_names_its_destination_then_its_vector() {
        let apic = LocalApic::new(FakeRegs::default());
        apic.regs.script_reads(ICR_LOW, &[ICR_SEND_PENDING, 0]);
        apic.send_fixed(2, 0x31);
        assert_eq!(apic.regs.reads_of(ICR_LOW), 2);
        assert_eq!(
            apic.regs.writes(),
            vec![(ICR_HIGH, 2 << 24), (ICR_LOW, 0x4031)]
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
            vec![(TIMER_INITIAL_COUNT, 5000), (END_OF_INTERRUPT, 0)]
        );
    }
}
