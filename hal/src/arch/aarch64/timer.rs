//! The Arm generic timer (Arm ARM, chapter D12): the virtual counter as the clock and the EL1
//! virtual timer as the one-shot timer, signalled through the GICv2 as a private interrupt; and
//! the wake-up interrupt between CPUs, a GICv2 software-generated interrupt (D41).

use super::DIRECT_MAP_OFFSET;
use crate::atomic::AtomicU64;
use crate::interrupt::SourceLines;
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use eggos_drivers::interrupt::gicv2::{self, Gicv2CpuInterface, Gicv2Distributor, Trigger};
use eggos_drivers::interrupt::gicv3::{
    self, CpuInterfaceRegisters, Gicv3CpuInterface, Gicv3Distributor, Gicv3Redistributor,
};
use eggos_drivers::io::Mmio;

/// `CNTKCTL_EL1` with only `EL0VCTEN` set: user mode may read the virtual counter and its
/// frequency (`CNTVCT_EL0`, `CNTFRQ_EL0`), as on Linux, but not the physical counter, and may
/// not touch either timer (D210).
pub(super) const CNTKCTL_USER_VIRTUAL_COUNTER: u64 = 1 << 1;

const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

static GIC_VERSION: AtomicU8 = AtomicU8::new(2);
static GIC_DISTRIBUTOR: AtomicUsize = AtomicUsize::new(GICD_BASE);
static GIC_SECOND_BLOCK: AtomicUsize = AtomicUsize::new(GICC_BASE);
static GIC_SECOND_BLOCK_BYTES: AtomicUsize = AtomicUsize::new(gicv2::CPU_INTERFACE_BLOCK_SIZE);
/// The boot CPU's MPIDR affinity.
pub(super) static BOOT_AFFINITY: AtomicU64 = AtomicU64::new(0);

fn is_gicv3() -> bool {
    GIC_VERSION.load(Ordering::Relaxed) >= 3
}

fn discover() {
    let Some(gic) = super::madt().and_then(crate::acpi::madt_gic) else {
        discover_in_tree();
        return;
    };
    let Ok(distributor) = usize::try_from(gic.distributor) else {
        return;
    };
    let (second, bytes) = if gic.version >= 3 {
        (gic.redistributors, gic.redistributor_bytes)
    } else {
        (gic.cpu_interface, gicv2::CPU_INTERFACE_BLOCK_SIZE as u64)
    };
    let (Ok(second), Ok(bytes)) = (usize::try_from(second), usize::try_from(bytes)) else {
        return;
    };
    if distributor == 0 || second == 0 || bytes == 0 {
        return;
    }
    GIC_DISTRIBUTOR.store(distributor, Ordering::Relaxed);
    GIC_SECOND_BLOCK.store(second, Ordering::Relaxed);
    GIC_SECOND_BLOCK_BYTES.store(bytes, Ordering::Relaxed);
    GIC_VERSION.store(gic.version.max(2), Ordering::Relaxed);
}

fn discover_in_tree() {
    let Some(gic) = super::device_tree_bytes().and_then(crate::fdt::gic) else {
        return;
    };
    let (Ok(distributor), Ok(second), Ok(bytes)) = (
        usize::try_from(gic.distributor.0),
        usize::try_from(gic.second.0),
        usize::try_from(gic.second.1),
    ) else {
        return;
    };
    if distributor == 0 || second == 0 || bytes == 0 {
        return;
    }
    GIC_DISTRIBUTOR.store(distributor, Ordering::Relaxed);
    GIC_SECOND_BLOCK.store(second, Ordering::Relaxed);
    let second_bytes = if gic.version >= 3 {
        bytes
    } else {
        gicv2::CPU_INTERFACE_BLOCK_SIZE
    };
    GIC_SECOND_BLOCK_BYTES.store(second_bytes, Ordering::Relaxed);
    GIC_VERSION.store(gic.version, Ordering::Relaxed);
}

/// The GIC register blocks this module drives, for the kernel's page tables.
pub(super) fn device_regions(region: &mut dyn FnMut(PhysicalRange)) {
    let distributor_bytes = if is_gicv3() {
        gicv3::DISTRIBUTOR_BLOCK_SIZE
    } else {
        gicv2::DISTRIBUTOR_BLOCK_SIZE
    };
    region(PhysicalRange::from_start_length(
        GIC_DISTRIBUTOR.load(Ordering::Relaxed) as u64,
        distributor_bytes as u64,
    ));
    region(PhysicalRange::from_start_length(
        GIC_SECOND_BLOCK.load(Ordering::Relaxed) as u64,
        GIC_SECOND_BLOCK_BYTES.load(Ordering::Relaxed) as u64,
    ));
}

#[derive(Debug, Clone, Copy)]
struct SystemRegisters;

macro_rules! system_register {
    ($read:ident, $write:ident, $name:literal) => {
        fn $read(&self) -> u64 {
            let value: u64;
            // SAFETY: reading a GICv3 CPU interface register of this CPU has no side effect
            // beyond the GIC's own state, which only this module drives.
            unsafe { core::arch::asm!(concat!("mrs {}, ", $name), out(reg) value, options(nomem, nostack)) };
            value
        }
        fn $write(&self, value: u64) {
            // SAFETY: writing a GICv3 CPU interface register of this CPU, which only this module
            // drives; `isb` makes it take effect before what follows.
            unsafe {
                core::arch::asm!(concat!("msr ", $name, ", {}"), "isb", in(reg) value, options(nomem, nostack))
            };
        }
    };
}

#[allow(dead_code)]
impl SystemRegisters {
    system_register!(read_sre, write_sre, "S3_0_C12_C12_5");
    system_register!(read_pmr, write_pmr, "S3_0_C4_C6_0");
    system_register!(read_bpr1, write_bpr1, "S3_0_C12_C12_3");
    system_register!(read_igrpen1, write_igrpen1, "S3_0_C12_C12_7");
    system_register!(read_iar1, write_iar1, "S3_0_C12_C12_0");
    system_register!(read_eoir1, write_eoir1, "S3_0_C12_C12_1");
    system_register!(read_sgi1r, write_sgi1r, "S3_0_C12_C11_5");
}

impl CpuInterfaceRegisters for SystemRegisters {
    fn read_system_register_enable(&self) -> u64 {
        self.read_sre()
    }
    fn write_system_register_enable(&self, value: u64) {
        self.write_sre(value);
    }
    fn write_priority_mask(&self, value: u64) {
        self.write_pmr(value);
    }
    fn write_binary_point(&self, value: u64) {
        self.write_bpr1(value);
    }
    fn write_group_1_enable(&self, value: u64) {
        self.write_igrpen1(value);
    }
    fn read_acknowledge(&self) -> u64 {
        self.read_iar1()
    }
    fn write_end_of_interrupt(&self, value: u64) {
        self.write_eoir1(value);
    }
    fn write_software_interrupt(&self, value: u64) {
        self.write_sgi1r(value);
    }
}

fn gicv3_distributor() -> Gicv3Distributor<Mmio> {
    // SAFETY: the GICv3 distributor's registers are at `GIC_DISTRIBUTOR` (the MADT's), which the
    // kernel's page tables map as device memory at the direct map offset ([`device_regions`]);
    // only this module drives the GIC.
    Gicv3Distributor::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GIC_DISTRIBUTOR.load(Ordering::Relaxed),
            gicv3::DISTRIBUTOR_BLOCK_SIZE,
        )
    })
}

fn gicv3_redistributors() -> Mmio {
    // SAFETY: as in `gicv3_distributor`, for the redistributors' range the MADT gives.
    unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GIC_SECOND_BLOCK.load(Ordering::Relaxed),
            GIC_SECOND_BLOCK_BYTES.load(Ordering::Relaxed),
        )
    }
}

fn gicv3_this_redistributor() -> Option<Gicv3Redistributor<Mmio>> {
    let range = gicv3_redistributors();
    let length = GIC_SECOND_BLOCK_BYTES.load(Ordering::Relaxed);
    let offset = gicv3::find_redistributor(&range, length, affinity())?;
    // SAFETY: as in `gicv3_redistributors`, for this CPU's redistributor inside that range.
    Some(Gicv3Redistributor::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GIC_SECOND_BLOCK.load(Ordering::Relaxed) + offset,
            gicv3::REDISTRIBUTOR_BYTES,
        )
    }))
}

const GICV3_CPU_INTERFACE: Gicv3CpuInterface<SystemRegisters> =
    Gicv3CpuInterface::new(SystemRegisters);

/// An interrupt acknowledged at the GIC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Acknowledged {
    V2(gicv2::Acknowledged),
    V3(gicv3::Acknowledged),
}

impl Acknowledged {
    pub(super) fn id(self) -> u32 {
        match self {
            Acknowledged::V2(interrupt) => interrupt.id(),
            Acknowledged::V3(interrupt) => interrupt.id(),
        }
    }
}

/// The EL1 virtual timer's private interrupt (PPI 11, Arm's Server Base System Architecture
/// assignment, used by every GIC-based board).
pub(super) const VIRTUAL_TIMER_INTERRUPT: u32 = 27;
const TIMER_PRIORITY: u8 = 0x80;
/// The software-generated interrupt that wakes a CPU ([`crate::Platform::wake_cpu`]).
pub(super) const WAKEUP_SGI: u32 = 0;
const WAKEUP_PRIORITY: u8 = 0x80;
/// QEMU `virt`: UART0's interrupt, SPI 1, level-triggered. Device-tree discovery replaces it
/// in M5.
pub(super) const UART0_INTERRUPT: u32 = 33;
const CONSOLE_INPUT_PRIORITY: u8 = 0xa0;
/// The first shared peripheral interrupt's id: UART0's and the devices' are from here on.
pub(super) const FIRST_SHARED_INTERRUPT: u32 = 32;
const DEVICE_PRIORITY: u8 = 0xc0;
/// The shared peripheral interrupts routed for devices, by line.
pub(super) static DEVICE_LINES: SourceLines = SourceLines::new();

const GIC_CPU_INTERFACES: usize = 8;
static GIC_CPUS: [AtomicU64; GIC_CPU_INTERFACES] =
    [const { AtomicU64::new(0) }; GIC_CPU_INTERFACES];

const CNTV_CTL_ENABLE: u64 = 1;
const CNTV_CTL_ISTATUS: u64 = 1 << 2;

fn distributor() -> Gicv2Distributor<Mmio> {
    // SAFETY: the GICv2 distributor's registers are at `GIC_DISTRIBUTOR` (QEMU `virt`'s
    // GICD_BASE, or the MADT's), which the boot page tables and the kernel's own map as device
    // memory at the direct map offset; only this module drives the GIC.
    Gicv2Distributor::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GIC_DISTRIBUTOR.load(Ordering::Relaxed),
            gicv2::DISTRIBUTOR_BLOCK_SIZE,
        )
    })
}

fn cpu_interface() -> Gicv2CpuInterface<Mmio> {
    // SAFETY: as in `distributor`, for the CPU interface at `GIC_SECOND_BLOCK`.
    Gicv2CpuInterface::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + GIC_SECOND_BLOCK.load(Ordering::Relaxed),
            gicv2::CPU_INTERFACE_BLOCK_SIZE,
        )
    })
}

/// Sets the boot CPU's timer and interrupts up ([`init_this_cpu`]) and turns the GIC's
/// distributor on. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn init() {
    assert!(
        frequency() != 0,
        "CNTFRQ_EL0 is 0: firmware did not set the counter frequency"
    );
    discover();
    BOOT_AFFINITY.store(affinity(), Ordering::Relaxed);
    if is_gicv3() {
        gicv3_distributor().enable();
        init_this_cpu();
    } else {
        init_this_cpu();
        distributor().enable();
    }
}

/// Disarms this CPU's timer and routes its interrupt and the wake-up SGI through the GIC to this
/// CPU (their enables and priorities are banked per CPU), so both are taken once `PSTATE.I` is
/// cleared; records this CPU's GIC interface for [`wake`]. Called by each CPU before it runs the
/// kernel's code.
pub(super) fn init_this_cpu() {
    disarm();
    if is_gicv3() {
        if let Some(redistributor) = gicv3_this_redistributor() {
            redistributor.wake();
            redistributor.enable_interrupt(VIRTUAL_TIMER_INTERRUPT, TIMER_PRIORITY);
            redistributor.enable_interrupt(WAKEUP_SGI, WAKEUP_PRIORITY);
        }
        GICV3_CPU_INTERFACE.enable();
        return;
    }
    let gicd = distributor();
    gicd.enable_interrupt(VIRTUAL_TIMER_INTERRUPT, TIMER_PRIORITY);
    gicd.enable_interrupt(WAKEUP_SGI, WAKEUP_PRIORITY);
    let target = gicd.this_cpu_target();
    if target != 0 {
        GIC_CPUS[target.trailing_zeros() as usize].store(affinity() + 1, Ordering::Release);
    }
    cpu_interface().enable();
}

/// Routes UART0's interrupt to the calling CPU and enables it.
pub(super) fn route_console_input() {
    if is_gicv3() {
        gicv3_distributor().route_shared_interrupt(
            UART0_INTERRUPT,
            CONSOLE_INPUT_PRIORITY,
            affinity(),
            Trigger::Level,
        );
        return;
    }
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
    if new && is_gicv3() {
        gicv3_distributor().route_shared_interrupt(id, DEVICE_PRIORITY, affinity(), trigger);
    } else if new {
        let gicd = distributor();
        gicd.route_shared_interrupt(id, DEVICE_PRIORITY, gicd.this_cpu_target(), trigger);
    }
    Ok(line)
}

/// Gives `line` back: its shared interrupt is disabled at the distributor (D413).
pub(super) fn release_device_interrupt(line: crate::DeviceLine) {
    let Some(id) = DEVICE_LINES.release(line) else {
        return;
    };
    match is_gicv3() {
        true => gicv3_distributor().disable_shared_interrupt(id),
        false => distributor().disable_interrupt(id),
    }
}

/// Sends `line`'s shared interrupt to the CPU whose MPIDR affinity is `cpu` (D536): through
/// its router on a GICv3, else to the CPU interface that CPU reported when it set its
/// interrupts up.
pub(super) fn steer_device_interrupt(
    line: crate::DeviceLine,
    cpu: u64,
) -> Result<(), &'static str> {
    let id = DEVICE_LINES
        .source_of(line)
        .ok_or("the line is not routed")?;
    if is_gicv3() {
        gicv3_distributor().set_route(id, cpu);
        return Ok(());
    }
    let interface = GIC_CPUS
        .iter()
        .position(|entry| entry.load(Ordering::Acquire) == cpu + 1)
        .ok_or("the CPU has not set its interrupts up")?;
    distributor().set_targets(id, 1 << interface);
    Ok(())
}

/// This CPU's MPIDR affinity, as the device tree's `/cpus` and the PSCI calls give it: Aff3 in
/// bits 39:32, Aff2-Aff0 in bits 23:0.
pub(super) fn affinity() -> u64 {
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
    if is_gicv3() {
        GICV3_CPU_INTERFACE.send_sgi(WAKEUP_SGI, cpu);
        return;
    }
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
    if is_gicv3() {
        GICV3_CPU_INTERFACE.acknowledge().map(Acknowledged::V3)
    } else {
        cpu_interface().acknowledge().map(Acknowledged::V2)
    }
}

pub(super) fn end_of_interrupt(interrupt: Acknowledged) {
    match interrupt {
        Acknowledged::V2(interrupt) => cpu_interface().end_of_interrupt(interrupt),
        Acknowledged::V3(interrupt) => GICV3_CPU_INTERFACE.end_of_interrupt(interrupt),
    }
}
