//! Arm Generic Interrupt Controller, version 2 (Arm IHI 0048B): the distributor, which routes
//! interrupts, and each CPU's interface, which signals them to the CPU.
//!
//! Every interrupt is left in Group 0 and signalled as an IRQ: QEMU `virt` (and any GIC
//! without the Security Extensions, or with the kernel in Secure state) delivers Group 0 as
//! IRQ while `GICC_CTLR.FIQEn` is 0.

use crate::io::RegisterIo;

/// Distributor control register.
const GICD_CTLR: usize = 0x000;
/// Interrupt set-enable registers, one bit per interrupt ID.
const GICD_ISENABLER: usize = 0x100;
/// Interrupt priority registers, one byte per interrupt ID.
const GICD_IPRIORITYR: usize = 0x400;
/// Interrupt processor targets registers, one byte per interrupt ID. The bytes of IDs 0-31 are
/// banked and read-only: each names the reading CPU's own interface.
const GICD_ITARGETSR: usize = 0x800;
/// Software generated interrupt register: writing it sends an SGI.
const GICD_SGIR: usize = 0xf00;

/// CPU interface control register.
const GICC_CTLR: usize = 0x000;
/// Priority mask register: only interrupts with a numerically lower priority are signalled.
const GICC_PMR: usize = 0x004;
/// Interrupt acknowledge register.
const GICC_IAR: usize = 0x00c;
/// End of interrupt register.
const GICC_EOIR: usize = 0x010;

/// `GICD_CTLR` / `GICC_CTLR` bit 0: enable (Group 0 forwarding / signalling).
const CTLR_ENABLE: u32 = 1;
/// Lets every priority through (the lowest priority a GIC implements reads back as 0xff or
/// with its unimplemented low bits clear).
const PMR_ALLOW_ALL: u32 = 0xff;
/// `GICC_IAR` bits 9:0: the interrupt ID.
const IAR_INTERRUPT_ID: u32 = 0x3ff;
/// Interrupt IDs 1020-1023 are special; 1022 and 1023 mean "nothing to acknowledge".
const FIRST_SPECIAL_ID: u32 = 1020;
/// The architecture's largest interrupt ID.
const MAX_INTERRUPT_ID: u32 = 1019;
/// The largest SGI ID.
const MAX_SGI_ID: u32 = 15;
/// `GICD_SGIR` bits 23:16: the CPU target list.
const SGIR_TARGET_LIST_SHIFT: u32 = 16;

/// Size of the distributor's register block in bytes.
pub const DISTRIBUTOR_BLOCK_SIZE: usize = 0x1000;
/// Size of the CPU interface's register block in bytes.
pub const CPU_INTERFACE_BLOCK_SIZE: usize = 0x2000;

/// An acknowledged interrupt, to be passed back to
/// [`Gicv2CpuInterface::end_of_interrupt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acknowledged {
    /// The raw `GICC_IAR` value (interrupt ID plus, for SGIs, the source CPU).
    iar: u32,
}

impl Acknowledged {
    /// The interrupt ID: 0-15 SGIs, 16-31 PPIs (per CPU), 32-1019 SPIs.
    pub const fn id(self) -> u32 {
        self.iar & IAR_INTERRUPT_ID
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
        self.regs.write32(GICD_CTLR, CTLR_ENABLE);
    }

    /// Sets `id`'s priority (0 is the highest) and enables it. For SGIs and PPIs (IDs 0-31)
    /// this applies to the calling CPU only: those registers are banked per CPU.
    pub fn enable_interrupt(&self, id: u32, priority: u8) {
        assert!(id <= MAX_INTERRUPT_ID, "GIC interrupt ID {id} out of range");
        let id = id as usize;
        self.regs.write8(GICD_IPRIORITYR + id, priority);
        // Writing 0 bits to ISENABLER has no effect, so no read-modify-write is needed.
        self.regs
            .write32(GICD_ISENABLER + id / 32 * 4, 1 << (id % 32));
    }

    /// The calling CPU's interface, as a target list with one bit set (bit n for CPU interface
    /// n), for [`Self::send_sgi`]. 0 on a uniprocessor GIC, which has no targets to name.
    pub fn this_cpu_target(&self) -> u8 {
        // SGI 0's byte; any byte of IDs 0-31 names this CPU's interface.
        self.regs.read8(GICD_ITARGETSR)
    }

    /// Sends SGI `id` to the CPU interfaces in `targets` (a target list from
    /// [`Self::this_cpu_target`] on each target CPU).
    pub fn send_sgi(&self, id: u32, targets: u8) {
        assert!(id <= MAX_SGI_ID, "GIC SGI ID {id} out of range");
        // Target list filter (bits 25:24) 0: the CPUs in the list.
        self.regs
            .write32(GICD_SGIR, u32::from(targets) << SGIR_TARGET_LIST_SHIFT | id);
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

    /// Lets interrupts of every priority through and starts signalling them to this CPU.
    pub fn enable(&self) {
        self.regs.write32(GICC_PMR, PMR_ALLOW_ALL);
        self.regs.write32(GICC_CTLR, CTLR_ENABLE);
    }

    /// Acknowledges the highest-priority pending interrupt, or returns `None` if there is
    /// none (a spurious interrupt, ID 1023 or 1022). Each `Some` must be passed to
    /// [`Self::end_of_interrupt`].
    pub fn acknowledge(&self) -> Option<Acknowledged> {
        let iar = self.regs.read32(GICC_IAR);
        let acknowledged = Acknowledged { iar };
        (acknowledged.id() < FIRST_SPECIAL_ID).then_some(acknowledged)
    }

    /// Ends `interrupt`, so the GIC can signal it (and anything it held back) again.
    pub fn end_of_interrupt(&self, interrupt: Acknowledged) {
        self.regs.write32(GICC_EOIR, interrupt.iar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn enabling_a_ppi_sets_its_priority_byte_and_enable_bit() {
        let gicd = Gicv2Distributor::new(FakeRegs::default());
        gicd.enable();
        gicd.enable_interrupt(27, 0x80);
        assert_eq!(
            gicd.regs.writes(),
            vec![(GICD_CTLR, 1), (0x400 + 27, 0x80), (0x100, 1 << 27)]
        );
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
        // An SGI from CPU 2 (bits 12:10) with ID 5.
        gicc.regs.script_reads(GICC_IAR, &[2 << 10 | 5]);
        let interrupt = gicc.acknowledge().expect("pending interrupt");
        assert_eq!(interrupt.id(), 5);
        gicc.end_of_interrupt(interrupt);
        assert_eq!(gicc.regs.writes(), vec![(GICC_EOIR, 2 << 10 | 5)]);
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
