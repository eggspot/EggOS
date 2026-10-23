//! Arm Generic Interrupt Controller, version 2 (Arm IHI 0048B): the distributor, which routes
//! interrupts, and each CPU's interface, which signals them to the CPU.
//!
//! Every interrupt is left in Group 0 and signalled as an IRQ: QEMU `virt` (and any GIC
//! without the Security Extensions, or with the kernel in Secure state) delivers Group 0 as
//! IRQ while `GICC_CTLR.FIQEn` is 0.

use crate::io::RegisterIo;

const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;
const GICD_SGIR: usize = 0xf00;
const BANKED_TARGET_BYTE_OF_SGI_0: usize = GICD_ITARGETSR;

const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_IAR: usize = 0x00c;
const GICC_EOIR: usize = 0x010;

const CTLR_ENABLE_GROUP_0: u32 = 1;
const PMR_ALLOW_EVERY_PRIORITY: u32 = 0xff;
const IAR_INTERRUPT_ID_MASK: u32 = 0x3ff;
const FIRST_SPECIAL_INTERRUPT_ID: u32 = 1020;
const MAX_INTERRUPT_ID: u32 = 1019;
const MAX_SGI_ID: u32 = 15;
const SGIR_TARGET_LIST_SHIFT: u32 = 16;
const SGIR_FILTER_TO_TARGET_LIST: u32 = 0b00 << 24;

/// Size of the distributor's register block in bytes.
pub const DISTRIBUTOR_BLOCK_SIZE: usize = 0x1000;
/// Size of the CPU interface's register block in bytes.
pub const CPU_INTERFACE_BLOCK_SIZE: usize = 0x2000;

/// An acknowledged interrupt, to be passed back to
/// [`Gicv2CpuInterface::end_of_interrupt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acknowledged {
    raw_iar: u32,
}

impl Acknowledged {
    /// The interrupt ID: 0-15 SGIs, 16-31 PPIs (per CPU), 32-1019 SPIs.
    pub const fn id(self) -> u32 {
        self.raw_iar & IAR_INTERRUPT_ID_MASK
    }
}

/// The GICv2 distributor.
#[derive(Debug)]
pub struct Gicv2Distributor<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Gicv2Distributor<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Starts forwarding enabled interrupts to the CPU interfaces.
    pub fn enable(&self) {
        self.regs.write32(GICD_CTLR, CTLR_ENABLE_GROUP_0);
    }

    /// Sets `id`'s priority (0 is the highest) and enables it. For SGIs and PPIs (IDs 0-31)
    /// this applies to the calling CPU only: those registers are banked per CPU.
    pub fn enable_interrupt(&self, id: u32, priority: u8) {
        assert!(id <= MAX_INTERRUPT_ID, "GIC interrupt ID {id} out of range");
        let id = id as usize;
        self.regs.write8(GICD_IPRIORITYR + id, priority);
        self.regs
            .write32(GICD_ISENABLER + id / 32 * 4, 1 << (id % 32));
    }

    /// The calling CPU's interface, as a target list with one bit set (bit n for CPU interface
    /// n), for [`Self::send_sgi`]. 0 on a uniprocessor GIC, which has no targets to name.
    pub fn this_cpu_target(&self) -> u8 {
        self.regs.read8(BANKED_TARGET_BYTE_OF_SGI_0)
    }

    /// Sends SGI `id` to the CPU interfaces in `targets` (a target list from
    /// [`Self::this_cpu_target`] on each target CPU).
    pub fn send_sgi(&self, id: u32, targets: u8) {
        assert!(id <= MAX_SGI_ID, "GIC SGI ID {id} out of range");
        self.regs.write32(
            GICD_SGIR,
            SGIR_FILTER_TO_TARGET_LIST | u32::from(targets) << SGIR_TARGET_LIST_SHIFT | id,
        );
    }
}

/// One CPU's GICv2 interface. Its registers are banked: every CPU sees its own at the same
/// address.
#[derive(Debug)]
pub struct Gicv2CpuInterface<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Gicv2CpuInterface<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Lets interrupts of every priority through and starts signalling them to this CPU. (The
    /// priority mask is written as 0xff: the lowest priority a GIC implements reads back as
    /// 0xff, or with its unimplemented low bits clear.)
    pub fn enable(&self) {
        self.regs.write32(GICC_PMR, PMR_ALLOW_EVERY_PRIORITY);
        self.regs.write32(GICC_CTLR, CTLR_ENABLE_GROUP_0);
    }

    /// Acknowledges the highest-priority pending interrupt, or returns `None` if there is
    /// none (a spurious interrupt, ID 1023 or 1022). Each `Some` must be passed to
    /// [`Self::end_of_interrupt`].
    pub fn acknowledge(&self) -> Option<Acknowledged> {
        let raw_iar = self.regs.read32(GICC_IAR);
        let acknowledged = Acknowledged { raw_iar };
        (acknowledged.id() < FIRST_SPECIAL_INTERRUPT_ID).then_some(acknowledged)
    }

    /// Ends `interrupt`, so the GIC can signal it (and anything it held back) again.
    pub fn end_of_interrupt(&self, interrupt: Acknowledged) {
        self.regs.write32(GICC_EOIR, interrupt.raw_iar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    const IAR_SOURCE_CPU_SHIFT: u32 = 10;

    #[test]
    fn enabling_a_ppi_sets_its_priority_byte_and_writes_only_its_enable_bit() {
        let gicd = Gicv2Distributor::new(FakeRegs::default());
        gicd.enable();
        gicd.enable_interrupt(27, 0x80);
        assert_eq!(
            gicd.regs.writes(),
            vec![(GICD_CTLR, 1), (0x400 + 27, 0x80), (0x100, 1 << 27)]
        );
        assert_eq!(gicd.regs.reads_of(GICD_ISENABLER), 0);
    }

    #[test]
    fn enabling_an_spi_uses_the_right_enable_register() {
        let gicd = Gicv2Distributor::new(FakeRegs::default());
        gicd.enable_interrupt(33, 0);
        assert_eq!(gicd.regs.writes(), vec![(0x400 + 33, 0), (0x104, 1 << 1)]);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn special_interrupt_ids_cannot_be_enabled() {
        Gicv2Distributor::new(FakeRegs::default()).enable_interrupt(1020, 0);
    }

    #[test]
    fn an_sgi_names_its_targets_and_id() {
        let gicd = Gicv2Distributor::new(FakeRegs::default());
        gicd.send_sgi(1, 0b0100);
        assert_eq!(gicd.regs.writes(), vec![(0xf00, 0b0100 << 16 | 1)]);
    }

    #[test]
    fn this_cpus_target_is_read_from_the_banked_target_byte() {
        let gicd = Gicv2Distributor::new(FakeRegs::default());
        gicd.regs.script_reads(GICD_ITARGETSR, &[0b1000]);
        assert_eq!(gicd.this_cpu_target(), 0b1000);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn only_sgi_ids_can_be_sent() {
        Gicv2Distributor::new(FakeRegs::default()).send_sgi(16, 1);
    }

    #[test]
    fn cpu_interface_unmasks_every_priority_then_enables() {
        let gicc = Gicv2CpuInterface::new(FakeRegs::default());
        gicc.enable();
        assert_eq!(gicc.regs.writes(), vec![(GICC_PMR, 0xff), (GICC_CTLR, 1)]);
    }

    #[test]
    fn acknowledge_then_end_of_interrupt_writes_back_the_raw_iar() {
        let gicc = Gicv2CpuInterface::new(FakeRegs::default());
        let sgi_5_from_cpu_2 = 2 << IAR_SOURCE_CPU_SHIFT | 5;
        gicc.regs.script_reads(GICC_IAR, &[sgi_5_from_cpu_2]);
        let interrupt = gicc.acknowledge().expect("pending interrupt");
        assert_eq!(interrupt.id(), 5);
        gicc.end_of_interrupt(interrupt);
        assert_eq!(gicc.regs.writes(), vec![(GICC_EOIR, sgi_5_from_cpu_2)]);
    }

    #[test]
    fn spurious_interrupts_are_not_acknowledged() {
        let gicc = Gicv2CpuInterface::new(FakeRegs::default());
        gicc.regs.script_reads(GICC_IAR, &[1023, 1022]);
        assert_eq!(gicc.acknowledge(), None);
        assert_eq!(gicc.acknowledge(), None);
        assert!(gicc.regs.writes().is_empty());
    }
}
