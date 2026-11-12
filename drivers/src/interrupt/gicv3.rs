//! The Arm Generic Interrupt Controller version 3 and 4 (Arm IHI 0069), with affinity routing,
//! for one security state as the kernel sees it from non-secure EL1 (D237): the distributor
//! and the redistributors as memory-mapped registers, the CPU interface as system registers,
//! which the HAL reaches through [`CpuInterfaceRegisters`].

use crate::io::RegisterIo;

pub use super::gicv2::Trigger;

const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ICFGR: usize = 0x0c00;
const GICD_IGRPMODR: usize = 0x0d00;
const GICD_IROUTER: usize = 0x6000;

const GICD_CTLR_ENABLE_GROUP_1: u32 = 1 << 1;
const GICD_CTLR_AFFINITY_ROUTING: u32 = 1 << 4;
const GICD_CTLR_REGISTER_WRITE_PENDING: u32 = 1 << 31;

const GICR_TYPER: usize = 0x0008;
const GICR_WAKER: usize = 0x0014;
const GICR_TYPER_LAST: u32 = 1 << 4;
const GICR_TYPER_VIRTUAL_LPIS: u32 = 1 << 1;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

const SGI_FRAME: usize = 0x1_0000;
const GICR_IGROUPR0: usize = SGI_FRAME + 0x0080;
const GICR_ISENABLER0: usize = SGI_FRAME + 0x0100;
const GICR_IPRIORITYR: usize = SGI_FRAME + 0x0400;
const GICR_ICFGR1: usize = SGI_FRAME + 0x0c04;

/// Bytes of one redistributor: its control frame and its SGI frame, and two more frames on a
/// GICv4 redistributor that supports virtual LPIs.
pub const REDISTRIBUTOR_BYTES: usize = 0x2_0000;
pub const REDISTRIBUTOR_BYTES_WITH_VIRTUAL_LPIS: usize = 0x4_0000;

/// Bytes of the distributor's register block.
pub const DISTRIBUTOR_BLOCK_SIZE: usize = 0x1_0000;

pub const FIRST_PPI_ID: u32 = 16;
pub const FIRST_SPI_ID: u32 = 32;
pub const MAX_SPI_ID: u32 = 1019;
const SPURIOUS_FIRST: u32 = 1020;
const MAX_SGI_ID: u32 = 15;
const ICFGR_EDGE_TRIGGERED: u32 = 0b10;
const PRIORITY_MASK_ALLOW_ALL: u64 = 0xff;
const POLLS_FOR_WRITES: usize = 1_000_000;

/// The CPU interface's system registers (`ICC_*_EL1`), which only the architecture can reach.
pub trait CpuInterfaceRegisters {
    /// `ICC_SRE_EL1`.
    fn read_system_register_enable(&self) -> u64;
    fn write_system_register_enable(&self, value: u64);
    /// `ICC_PMR_EL1`.
    fn write_priority_mask(&self, value: u64);
    /// `ICC_BPR1_EL1`.
    fn write_binary_point(&self, value: u64);
    /// `ICC_IGRPEN1_EL1`.
    fn write_group_1_enable(&self, value: u64);
    /// `ICC_IAR1_EL1`.
    fn read_acknowledge(&self) -> u64;
    /// `ICC_EOIR1_EL1`.
    fn write_end_of_interrupt(&self, value: u64);
    /// `ICC_SGI1R_EL1`.
    fn write_software_interrupt(&self, value: u64);
}

/// An interrupt acknowledged at the CPU interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acknowledged {
    raw: u32,
}

impl Acknowledged {
    pub const fn id(self) -> u32 {
        self.raw & 0x00ff_ffff
    }
}

/// An MPIDR affinity: Aff3 in bits 39:32, Aff2 to Aff0 in bits 23:0.
pub fn affinity_routing_value(affinity: u64) -> u64 {
    let low = affinity & 0x00ff_ffff;
    let aff3 = (affinity >> 32) & 0xff;
    low | (aff3 << 32)
}

/// The redistributor's affinity as `GICR_TYPER` bits 63:32 give it (Aff3.Aff2.Aff1.Aff0), in the
/// MPIDR layout [`affinity_routing_value`] takes.
pub fn affinity_of_typer_high(high: u32) -> u64 {
    let aff0 = u64::from(high & 0xff);
    let aff1 = u64::from((high >> 8) & 0xff);
    let aff2 = u64::from((high >> 16) & 0xff);
    let aff3 = u64::from((high >> 24) & 0xff);
    aff0 | (aff1 << 8) | (aff2 << 16) | (aff3 << 32)
}

/// The `ICC_SGI1R_EL1` value that sends SGI `id` to the CPU with MPIDR affinity `affinity`.
pub fn software_interrupt_value(id: u32, affinity: u64) -> u64 {
    let aff0 = affinity & 0xff;
    let aff1 = (affinity >> 8) & 0xff;
    let aff2 = (affinity >> 16) & 0xff;
    let aff3 = (affinity >> 32) & 0xff;
    let target_list = 1u64 << (aff0 % 16);
    let range_selector = aff0 / 16;
    target_list
        | (aff1 << 16)
        | (u64::from(id & 0xf) << 24)
        | (aff2 << 32)
        | (range_selector << 44)
        | (aff3 << 48)
}

/// The GICv3 distributor.
#[derive(Debug)]
pub struct Gicv3Distributor<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Gicv3Distributor<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn wait_for_writes(&self) {
        for _ in 0..POLLS_FOR_WRITES {
            if self.regs.read32(GICD_CTLR) & GICD_CTLR_REGISTER_WRITE_PENDING == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Turns affinity routing and group 1 on.
    pub fn enable(&self) {
        self.regs.write32(GICD_CTLR, 0);
        self.wait_for_writes();
        self.regs
            .write32(GICD_CTLR, GICD_CTLR_AFFINITY_ROUTING | GICD_CTLR_ENABLE_GROUP_1);
        self.wait_for_writes();
    }

    /// How many interrupt ids the distributor implements, SGIs and PPIs included.
    pub fn lines(&self) -> u32 {
        let lines_field = self.regs.read32(GICD_TYPER) & 0x1f;
        ((lines_field + 1) * 32).min(MAX_SPI_ID + 1)
    }

    /// Disables shared interrupt `id`, so it signals nothing more (a line given back, D413).
    pub fn disable_shared_interrupt(&self, id: u32) {
        if !(FIRST_SPI_ID..=MAX_SPI_ID).contains(&id) {
            return;
        }
        let word = (id / 32) as usize * 4;
        self.regs.write32(GICD_ICENABLER + word, 1u32 << (id % 32));
        self.wait_for_writes();
    }

    /// Routes shared interrupt `id` to the CPU with MPIDR affinity `affinity` as a group 1
    /// interrupt of `priority`, signalled as `trigger` says, and enables it.
    pub fn route_shared_interrupt(&self, id: u32, priority: u8, affinity: u64, trigger: Trigger) {
        if !(FIRST_SPI_ID..=MAX_SPI_ID).contains(&id) {
            return;
        }
        let word = (id / 32) as usize * 4;
        let bit = 1u32 << (id % 32);
        self.regs.write32(GICD_ICENABLER + word, bit);
        self.wait_for_writes();
        let group = self.regs.read32(GICD_IGROUPR + word);
        self.regs.write32(GICD_IGROUPR + word, group | bit);
        let modifier = self.regs.read32(GICD_IGRPMODR + word);
        self.regs.write32(GICD_IGRPMODR + word, modifier & !bit);
        self.regs.write8(GICD_IPRIORITYR + id as usize, priority);
        let configuration_word = GICD_ICFGR + (id / 16) as usize * 4;
        let shift = (id % 16) * 2;
        let mut configuration = self.regs.read32(configuration_word) & !(0b11 << shift);
        if trigger == Trigger::Edge {
            configuration |= ICFGR_EDGE_TRIGGERED << shift;
        }
        self.regs.write32(configuration_word, configuration);
        self.set_route(id, affinity);
        self.regs.write32(GICD_ISENABLER + word, bit);
    }

    /// Sends shared interrupt `id` to the CPU with MPIDR affinity `affinity` from now on,
    /// leaving the rest of its setup alone (D536).
    pub fn set_route(&self, id: u32, affinity: u64) {
        if !(FIRST_SPI_ID..=MAX_SPI_ID).contains(&id) {
            return;
        }
        let route = affinity_routing_value(affinity);
        let router = GICD_IROUTER + id as usize * 8;
        self.regs.write32(router, route as u32);
        self.regs.write32(router + 4, (route >> 32) as u32);
    }
}

/// Finds the redistributor of the CPU with MPIDR affinity `affinity` in a discovery range read
/// through `regs` (`GICR` entries of the MADT, or the `GICC` entry's own base), `length` bytes:
/// its offset in the range.
pub fn find_redistributor<R: RegisterIo>(regs: &R, length: usize, affinity: u64) -> Option<usize> {
    let mut offset = 0;
    while offset + REDISTRIBUTOR_BYTES <= length {
        let low = regs.read32(offset + GICR_TYPER);
        let high = regs.read32(offset + GICR_TYPER + 4);
        if affinity_of_typer_high(high) == affinity & 0xff_00ff_ffff {
            return Some(offset);
        }
        if low & GICR_TYPER_LAST != 0 {
            return None;
        }
        offset += if low & GICR_TYPER_VIRTUAL_LPIS != 0 {
            REDISTRIBUTOR_BYTES_WITH_VIRTUAL_LPIS
        } else {
            REDISTRIBUTOR_BYTES
        };
    }
    None
}

/// One CPU's redistributor: its control frame and SGI frame.
#[derive(Debug)]
pub struct Gicv3Redistributor<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Gicv3Redistributor<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    /// Wakes the redistributor so it forwards interrupts to its CPU.
    pub fn wake(&self) {
        let waker = self.regs.read32(GICR_WAKER);
        self.regs
            .write32(GICR_WAKER, waker & !GICR_WAKER_PROCESSOR_SLEEP);
        for _ in 0..POLLS_FOR_WRITES {
            if self.regs.read32(GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Enables the private interrupt `id` (an SGI or a PPI) as group 1 at `priority`; PPIs are
    /// level-triggered, as the timers are.
    pub fn enable_interrupt(&self, id: u32, priority: u8) {
        if id >= FIRST_SPI_ID {
            return;
        }
        let bit = 1u32 << id;
        let group = self.regs.read32(GICR_IGROUPR0);
        self.regs.write32(GICR_IGROUPR0, group | bit);
        self.regs.write8(GICR_IPRIORITYR + id as usize, priority);
        if id >= FIRST_PPI_ID {
            let shift = (id - FIRST_PPI_ID) * 2;
            let configuration = self.regs.read32(GICR_ICFGR1) & !(0b11 << shift);
            self.regs.write32(GICR_ICFGR1, configuration);
        }
        self.regs.write32(GICR_ISENABLER0, bit);
    }
}

/// The CPU interface, through its system registers.
#[derive(Debug)]
pub struct Gicv3CpuInterface<C: CpuInterfaceRegisters> {
    registers: C,
}

impl<C: CpuInterfaceRegisters> Gicv3CpuInterface<C> {
    pub const fn new(registers: C) -> Self {
        Self { registers }
    }

    /// Turns the system register interface and group 1 on, every priority allowed.
    pub fn enable(&self) {
        let enable = self.registers.read_system_register_enable();
        self.registers.write_system_register_enable(enable | 1);
        self.registers.write_priority_mask(PRIORITY_MASK_ALLOW_ALL);
        self.registers.write_binary_point(0);
        self.registers.write_group_1_enable(1);
    }

    /// Acknowledges the highest-priority pending interrupt; `None` for a spurious one.
    pub fn acknowledge(&self) -> Option<Acknowledged> {
        let raw = self.registers.read_acknowledge() as u32;
        let interrupt = Acknowledged { raw };
        (interrupt.id() < SPURIOUS_FIRST).then_some(interrupt)
    }

    pub fn end_of_interrupt(&self, interrupt: Acknowledged) {
        self.registers.write_end_of_interrupt(u64::from(interrupt.raw));
    }

    /// Sends SGI `id` to the CPU with MPIDR affinity `affinity`.
    pub fn send_sgi(&self, id: u32, affinity: u64) {
        if id > MAX_SGI_ID {
            return;
        }
        self.registers
            .write_software_interrupt(software_interrupt_value(id, affinity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn routing_values_follow_the_mpidr_layout() {
        assert_eq!(affinity_routing_value(0x01_0002_0304), 0x01_0002_0304);
        assert_eq!(affinity_of_typer_high(0x0102_0304), 0x01_0002_0304);
    }

    #[test]
    fn an_sgi_to_a_cpu_names_its_cluster_and_its_place_in_it() {
        let value = software_interrupt_value(0, 0x0000_0102);
        assert_eq!(value & 0xffff, 1 << 2);
        assert_eq!((value >> 16) & 0xff, 1);
        assert_eq!((value >> 24) & 0xf, 0);
        let high = software_interrupt_value(3, 0x0000_0011);
        assert_eq!(high & 0xffff, 1 << 1);
        assert_eq!((high >> 44) & 0xf, 1);
        assert_eq!((high >> 24) & 0xf, 3);
    }

    #[test]
    fn the_redistributor_of_a_cpu_is_found_by_its_affinity() {
        let regs = FakeRegs::default();
        regs.script_reads(GICR_TYPER, &[0]);
        regs.script_reads(GICR_TYPER + 4, &[0x0000_0000]);
        regs.script_reads(REDISTRIBUTOR_BYTES + GICR_TYPER, &[GICR_TYPER_LAST]);
        regs.script_reads(REDISTRIBUTOR_BYTES + GICR_TYPER + 4, &[0x0000_0001]);
        assert_eq!(
            find_redistributor(&regs, 2 * REDISTRIBUTOR_BYTES, 1),
            Some(REDISTRIBUTOR_BYTES)
        );
    }

    #[test]
    fn steering_a_shared_interrupt_rewrites_only_its_router() {
        let distributor = Gicv3Distributor::new(FakeRegs::default());
        distributor.set_route(40, 0x01_0000_0002);
        distributor.set_route(27, 0x0000_0002);
        assert_eq!(
            distributor.regs.writes(),
            vec![(GICD_IROUTER + 40 * 8, 2), (GICD_IROUTER + 40 * 8 + 4, 1)]
        );
    }

    #[test]
    fn routing_a_shared_interrupt_writes_its_route_and_enables_it() {
        let regs = FakeRegs::default();
        let distributor = Gicv3Distributor::new(regs);
        distributor.route_shared_interrupt(40, 0xc0, 0x0000_0001, Trigger::Edge);
        let writes = distributor.regs.writes();
        assert!(writes.contains(&(GICD_IROUTER + 40 * 8, 1)));
        assert!(writes.contains(&(GICD_ISENABLER + 4, 1 << 8)));
    }
}
