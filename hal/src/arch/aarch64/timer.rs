//! The Arm generic timer (Arm ARM, chapter D12): the virtual counter as the clock and the EL1
//! virtual timer as the one-shot timer, signalled through the GICv2 as a private interrupt.

use crate::memory::PhysicalRange;
use eggos_drivers::interrupt::gicv2::{self, Acknowledged, Gicv2CpuInterface, Gicv2Distributor};
use eggos_drivers::io::Mmio;

/// QEMU `virt` (`-machine gic-version=2`): GICv2 distributor and CPU interface. Device-tree
/// discovery replaces these in M5, along with GICv3 support.
const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

/// The GIC register blocks this module drives, for the kernel's page tables.
pub(super) const DEVICE_REGIONS: [PhysicalRange; 2] = [
    PhysicalRange::from_start_length(GICD_BASE as u64, gicv2::DISTRIBUTOR_BLOCK_SIZE as u64),
    PhysicalRange::from_start_length(GICC_BASE as u64, gicv2::CPU_INTERFACE_BLOCK_SIZE as u64),
];

/// The EL1 virtual timer's private interrupt (PPI 11, Arm's Server Base System Architecture
/// assignment, used by every GIC-based board).
pub(super) const VIRTUAL_TIMER_INTERRUPT: u32 = 27;
/// Middle priority, leaving room above and below for interrupts added later.
const TIMER_PRIORITY: u8 = 0x80;

/// `CNTV_CTL_EL0` bit 0: timer enabled (bit 1, IMASK, left clear: interrupt not masked).
const CNTV_CTL_ENABLE: u64 = 1;
/// `CNTV_CTL_EL0` bit 2: the timer condition is met (read-only).
const CNTV_CTL_ISTATUS: u64 = 1 << 2;

fn distributor() -> Gicv2Distributor<Mmio> {
    // SAFETY: on QEMU `virt` the GICv2 distributor's registers are at GICD_BASE, accessible
    // with the MMU off; only this module drives the GIC.
    Gicv2Distributor::new(unsafe { Mmio::new(GICD_BASE, gicv2::DISTRIBUTOR_BLOCK_SIZE) })
}

fn cpu_interface() -> Gicv2CpuInterface<Mmio> {
    // SAFETY: as in `distributor`, for the CPU interface at GICC_BASE.
    Gicv2CpuInterface::new(unsafe { Mmio::new(GICC_BASE, gicv2::CPU_INTERFACE_BLOCK_SIZE) })
}

/// Disarms the timer and routes its interrupt through the GIC to this CPU, so it is taken once
/// `PSTATE.I` is cleared. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn init() {
    // The panic handler reports this; the clock would be useless.
    assert!(
        frequency() != 0,
        "CNTFRQ_EL0 is 0: firmware did not set the counter frequency"
    );
    disarm();
    let gicd = distributor();
    gicd.enable_interrupt(VIRTUAL_TIMER_INTERRUPT, TIMER_PRIORITY);
    gicd.enable();
    cpu_interface().enable();
}

/// The virtual counter, `CNTVCT_EL0`.
#[inline]
pub(super) fn now() -> u64 {
    let ticks: u64;
    // SAFETY: `isb` keeps the counter read from being performed early, so a deadline check
    // after it sees the time it was made; reading `CNTVCT_EL0` has no side effects.
    unsafe {
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks, options(nomem, nostack))
    };
    ticks
}

/// `CNTFRQ_EL0`, the counter frequency the firmware programmed.
#[inline]
pub(super) fn frequency() -> u64 {
    let frequency: u64;
    // SAFETY: reading `CNTFRQ_EL0` has no side effects.
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack)) };
    frequency
}

/// Fires the timer interrupt once `CNTVCT_EL0` >= `deadline`.
pub(super) fn set_deadline(deadline: u64) {
    // SAFETY: programming the EL1 virtual timer only affects when its interrupt is raised;
    // the compare value is written before the timer is enabled, so an old one cannot fire.
    unsafe { core::arch::asm!("msr cntv_cval_el0, {}", in(reg) deadline, options(nomem, nostack)) };
    // SAFETY: as above; `isb` makes the new setting take effect before we go on.
    unsafe {
        core::arch::asm!(
            "msr cntv_ctl_el0, {}",
            "isb",
            in(reg) CNTV_CTL_ENABLE,
            options(nomem, nostack)
        )
    };
}

/// Whether the timer is enabled and its deadline has been reached. An interrupt the GIC
/// latched for an earlier deadline can still arrive after `set_deadline` moved it later; this
/// tells the two apart.
pub(super) fn expired() -> bool {
    let control: u64;
    // SAFETY: reading `CNTV_CTL_EL0` has no side effects.
    unsafe { core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) control, options(nomem, nostack)) };
    control & (CNTV_CTL_ENABLE | CNTV_CTL_ISTATUS) == CNTV_CTL_ENABLE | CNTV_CTL_ISTATUS
}

/// Disables the timer, which also drops its interrupt (it stays asserted while the timer is
/// enabled and past its compare value).
pub(super) fn disarm() {
    // SAFETY: disabling the EL1 virtual timer only stops its interrupt.
    unsafe { core::arch::asm!("msr cntv_ctl_el0, xzr", "isb", options(nomem, nostack)) };
}

/// Acknowledges the pending interrupt at the GIC; `None` for a spurious one.
pub(super) fn acknowledge() -> Option<Acknowledged> {
    cpu_interface().acknowledge()
}

pub(super) fn end_of_interrupt(interrupt: Acknowledged) {
    cpu_interface().end_of_interrupt(interrupt);
}
