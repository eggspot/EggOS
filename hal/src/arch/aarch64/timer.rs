//! The Arm generic timer (Arm ARM, chapter D12): the virtual counter as the clock and the EL1
//! virtual timer as the one-shot timer, signalled through the GICv2 as a private interrupt; and
//! the wake-up interrupt between CPUs, a GICv2 software-generated interrupt (D41).

use super::DIRECT_MAP_OFFSET;
use crate::interrupt::SourceLines;
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicU64, Ordering};
pub(super) use eggos_drivers::interrupt::gicv2::Acknowledged;
use eggos_drivers::interrupt::gicv2::{self, Gicv2CpuInterface, Gicv2Distributor, Trigger};
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
/// The software-generated interrupt that wakes a CPU ([`crate::Platform::wake_cpu`]).
pub(super) const WAKEUP_SGI: u32 = 0;
const WAKEUP_PRIORITY: u8 = 0x80;
/// QEMU `virt`: UART0's interrupt, SPI 1, level-triggered. Device-tree discovery replaces it
/// in M5.
pub(super) const UART0_INTERRUPT: u32 = 33;
/// Below the timer and wake-up: input can wait for them.
const CONSOLE_INPUT_PRIORITY: u8 = 0xa0;
/// The first shared peripheral interrupt's id: UART0's and the devices' are from here on.
pub(super) const FIRST_SHARED_INTERRUPT: u32 = 32;
const DEVICE_PRIORITY: u8 = 0xc0;
/// The shared peripheral interrupts routed for devices, by line.
pub(super) static DEVICE_LINES: SourceLines = SourceLines::new();

/// The CPU interfaces a GICv2 has.
const GIC_CPU_INTERFACES: usize = 8;
/// Which CPU each GIC CPU interface belongs to, for [`wake`]: the MPIDR affinity (as the device
/// tree's `/cpus` gives it) plus 1, or 0 for none yet. Each CPU fills its own entry as it sets its
/// interrupts up, before it runs the kernel's code.
static GIC_CPUS: [AtomicU64; GIC_CPU_INTERFACES] =
    [const { AtomicU64::new(0) }; GIC_CPU_INTERFACES];

/// `CNTV_CTL_EL0` bit 0: timer enabled (bit 1, IMASK, left clear: interrupt not masked).
const CNTV_CTL_ENABLE: u64 = 1;
/// `CNTV_CTL_EL0` bit 2: the timer condition is met (read-only).
const CNTV_CTL_ISTATUS: u64 = 1 << 2;

fn distributor() -> Gicv2Distributor<Mmio> {
    // SAFETY: on QEMU `virt` the GICv2 distributor's registers are at GICD_BASE, which the
    // boot page tables and the kernel's own map as device memory at the direct map offset;
    // only this module drives the GIC.
    Gicv2Distributor::new(unsafe {
        Mmio::new(DIRECT_MAP_OFFSET + GICD_BASE, gicv2::DISTRIBUTOR_BLOCK_SIZE)
    })
}

fn cpu_interface() -> Gicv2CpuInterface<Mmio> {
    // SAFETY: as in `distributor`, for the CPU interface at GICC_BASE.
    Gicv2CpuInterface::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GICC_BASE,
            gicv2::CPU_INTERFACE_BLOCK_SIZE,
        )
    })
}

/// Sets the boot CPU's timer and interrupts up ([`init_this_cpu`]) and turns the GIC's
/// distributor on. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn init() {
    // The panic handler reports this; the clock would be useless.
    assert!(
        frequency() != 0,
        "CNTFRQ_EL0 is 0: firmware did not set the counter frequency"
    );
    init_this_cpu();
    distributor().enable();
}

/// Disarms this CPU's timer and routes its interrupt and the wake-up SGI through the GIC to this
/// CPU (their enables and priorities are banked per CPU), so both are taken once `PSTATE.I` is
/// cleared; records this CPU's GIC interface for [`wake`]. Called by each CPU before it runs the
/// kernel's code.
pub(super) fn init_this_cpu() {
    disarm();
    let gicd = distributor();
    gicd.enable_interrupt(VIRTUAL_TIMER_INTERRUPT, TIMER_PRIORITY);
    gicd.enable_interrupt(WAKEUP_SGI, WAKEUP_PRIORITY);
    let target = gicd.this_cpu_target();
    // 0 on a uniprocessor GIC: there is no other CPU to wake this one.
    if target != 0 {
        GIC_CPUS[target.trailing_zeros() as usize].store(affinity() + 1, Ordering::Release);
    }
    cpu_interface().enable();
}

/// Routes UART0's interrupt to the calling CPU and enables it.
pub(super) fn route_console_input() {
    let gicd = distributor();
    gicd.route_shared_interrupt(
        UART0_INTERRUPT,
        CONSOLE_INPUT_PRIORITY,
        gicd.this_cpu_target(),
        Trigger::Level,
    );
}

/// Routes the shared peripheral interrupt `id`, signalled as `trigger` says, to the calling CPU
/// and enables it, as a device's line; a line routed before is the same line.
pub(super) fn route_device_interrupt(
    id: u32,
    trigger: Trigger,
) -> Result<crate::DeviceLine, &'static str> {
    if id == UART0_INTERRUPT {
        return Err("the interrupt is the console's");
    }
    let (line, new) = DEVICE_LINES
        .claim(id)
        .ok_or("every device interrupt line is taken")?;
    if new {
        let gicd = distributor();
        gicd.route_shared_interrupt(id, DEVICE_PRIORITY, gicd.this_cpu_target(), trigger);
    }
    Ok(line)
}

/// This CPU's MPIDR affinity, as the device tree's `/cpus` and the PSCI calls give it: Aff3 in
/// bits 39:32, Aff2-Aff0 in bits 23:0.
fn affinity() -> u64 {
    let mpidr: u64;
    // SAFETY: reading `MPIDR_EL1` has no side effects.
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)) };
    mpidr & 0xff_00ff_ffff
}

/// Sends the wake-up SGI to the CPU whose MPIDR affinity is `cpu`, the calling CPU included. A
/// CPU that has not set its interrupts up yet is not woken (the caller's contract excludes it).
pub(super) fn wake(cpu: u64) {
    // SAFETY: a barrier only. It completes the caller's writes to memory before the write that
    // sends the SGI (device memory, which a store to normal memory does not otherwise precede),
    // so the woken CPU sees them.
    unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
    if cpu == affinity() {
        distributor().send_sgi_to_this_cpu(WAKEUP_SGI);
        return;
    }
    let Some(interface) = GIC_CPUS
        .iter()
        .position(|entry| entry.load(Ordering::Acquire) == cpu + 1)
    else {
        debug_assert!(
            false,
            "woke CPU {cpu:#x}, which has not set its interrupts up"
        );
        return;
    };
    distributor().send_sgi(WAKEUP_SGI, 1 << interface);
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
