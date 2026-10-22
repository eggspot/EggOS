//! x86_64 clock and timer: the time-stamp counter (TSC) as the clock, and the local APIC
//! timer in one-shot mode as the timer (Intel SDM vol. 3A, 11.5.4 and 18.17).
//!
//! Neither frequency is architectural, so both are measured at boot against the 8254 PIT,
//! whose frequency is fixed: [`MEASUREMENT_PIT_TICKS`] periods (about 2 ms). The legacy 8259
//! PICs are remapped and masked first, so the firmware's PC timer cannot arrive on an
//! exception vector.
//!
//! The APIC timer counts in its own clock and only 32 bits, so a deadline is converted to an
//! APIC count rounded up, and a timer that still fires early (measurement error, or a deadline
//! beyond 32 bits of count) is re-armed for the rest instead of being delivered.
//!
//! Per-CPU state is a set of statics while only the boot CPU runs; it moves to per-CPU data
//! with SMP (M4).

use super::port::PortIo;
use crate::Interrupt;
use crate::time::mul_div_ceil;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use eggos_drivers::interrupt::{LegacyPic, LocalApic, i8259, local_apic};
use eggos_drivers::io::Mmio;
use eggos_drivers::timer::{Pit, i8254};

/// Vectors of the legacy 8259 inputs 0-15 (all masked; only spurious interrupts arrive).
pub(super) const LEGACY_PIC_VECTORS: core::ops::RangeInclusive<u64> = 0x20..=0x2f;
/// The legacy primary's and secondary's spurious interrupts (input 7 of each).
const LEGACY_PIC_PRIMARY_SPURIOUS: u64 = 0x27;
const LEGACY_PIC_SECONDARY_SPURIOUS: u64 = 0x2f;
/// The local APIC timer's vector.
pub(super) const TIMER_VECTOR: u8 = 0x30;
/// The local APIC's spurious-interrupt vector (its low four bits must be 1s on old CPUs).
pub(super) const APIC_SPURIOUS_VECTOR: u8 = 0xff;

/// `IA32_APIC_BASE` MSR: bits 12 and up hold the local APIC's physical address.
const IA32_APIC_BASE: u32 = 0x1b;
const APIC_BASE_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
/// `IA32_APIC_BASE` bit 11: the local APIC is globally enabled (its registers respond).
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;

/// PIC and PIT I/O ports.
const PIC_PRIMARY_PORT: u16 = 0x20;
const PIC_SECONDARY_PORT: u16 = 0xa0;
const PIT_PORT: u16 = 0x40;
const SYSTEM_CONTROL_PORT_B: u16 = 0x61;

/// Length of the frequency measurement in PIT periods: 2 ms, keeping the boot cost small. The
/// PIT's granularity limits the error to 0.05%; in QEMU, emulation jitter makes it about 0.2%.
/// Deadlines are still never delivered early (see the module comment).
const MEASUREMENT_PIT_TICKS: u16 = 2386;
/// How long to wait for the measurement before deciding there is no working PIT, in TSC
/// ticks: about 2 s at 1 GHz, generous for any real TSC. (ACPI PM timer or HPET as fallback
/// reference clocks: follow-up.)
const MEASUREMENT_TIMEOUT_TSC_TICKS: u64 = 1 << 31;

/// Disarmed: no deadline.
const NO_DEADLINE: u64 = u64::MAX;

/// Physical (= identity-mapped) address of the local APIC's registers, set by [`init`].
static APIC_BASE: AtomicUsize = AtomicUsize::new(0);
/// Measured TSC and APIC timer frequencies in Hz, set by [`init`].
static TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);
static APIC_TIMER_FREQUENCY: AtomicU64 = AtomicU64::new(0);
/// The deadline the timer is armed for, in TSC ticks.
static DEADLINE: AtomicU64 = AtomicU64::new(NO_DEADLINE);

fn legacy_pic() -> LegacyPic<PortIo> {
    // SAFETY: ports 0x20-0x21 are the PC's primary 8259; only this module drives it.
    let primary = unsafe { PortIo::new(PIC_PRIMARY_PORT, i8259::REGISTER_BLOCK_SIZE) };
    // SAFETY: ports 0xa0-0xa1 are the secondary 8259; likewise.
    let secondary = unsafe { PortIo::new(PIC_SECONDARY_PORT, i8259::REGISTER_BLOCK_SIZE) };
    LegacyPic::new(primary, secondary)
}

fn local_apic() -> LocalApic<Mmio> {
    // SAFETY: `APIC_BASE` holds the address `IA32_APIC_BASE` reports (set before any caller
    // runs), identity-mapped by the boot page tables; only this module drives the local APIC.
    // The boot map does not set a memory type, so the firmware's MTRRs keep it uncached, as
    // the SDM requires; the kernel's own page tables (M2) map device memory uncached.
    LocalApic::new(unsafe {
        Mmio::new(
            APIC_BASE.load(Ordering::Relaxed),
            local_apic::REGISTER_BLOCK_SIZE,
        )
    })
}

/// Masks the legacy PICs, enables the local APIC and measures the clocks. Called once by the
/// entry code, after the IDT is loaded and before `kernel_main`.
pub(super) extern "C" fn init() {
    legacy_pic().remap_and_mask(*LEGACY_PIC_VECTORS.start() as u8);

    let apic_base_msr = read_msr(IA32_APIC_BASE);
    // Set at reset; firmware that clears it hides the APIC until the next reset.
    assert!(
        apic_base_msr & APIC_BASE_GLOBAL_ENABLE != 0,
        "local APIC globally disabled by firmware"
    );
    let apic_base = apic_base_msr & APIC_BASE_ADDRESS_MASK;
    // Below 4 GiB at reset; firmware could move it. The panic handler reports a failure.
    assert!(
        apic_base < super::IDENTITY_MAP_END,
        "local APIC outside the boot identity map"
    );
    APIC_BASE.store(apic_base as usize, Ordering::Relaxed);
    let apic = local_apic();
    apic.enable(APIC_SPURIOUS_VECTOR);

    // Count the TSC and the (masked) APIC timer over a known number of PIT periods.
    apic.configure_one_shot_timer(TIMER_VECTOR, true);
    // SAFETY: ports 0x40-0x43 are the 8254; nothing else drives it.
    let pit_ports = unsafe { PortIo::new(PIT_PORT, i8254::REGISTER_BLOCK_SIZE) };
    // SAFETY: port 0x61 is system control port B; the driver touches only its 8254 channel 2
    // gate and speaker bits (keeping the speaker off), and nothing else drives those.
    let control_port = unsafe { PortIo::new(SYSTEM_CONTROL_PORT_B, i8254::CONTROL_BLOCK_SIZE) };
    let pit = Pit::new(pit_ports, control_port);
    let mut start = (0, 0);
    let wait_start = now();
    let counted_down = pit.run_channel2(
        MEASUREMENT_PIT_TICKS,
        || {
            apic.start_timer(u32::MAX);
            start = (now(), apic.timer_count());
        },
        || now().wrapping_sub(wait_start) > MEASUREMENT_TIMEOUT_TSC_TICKS,
    );
    let (tsc_end, apic_end) = (now(), apic.timer_count());
    // The panic handler reports these on the console; without a clock there is no timer.
    assert!(
        counted_down,
        "8254 PIT did not count down: no reference clock"
    );
    let window = u64::from(MEASUREMENT_PIT_TICKS);
    let tsc_frequency = mul_div_ceil(tsc_end.wrapping_sub(start.0), i8254::FREQUENCY_HZ, window);
    let apic_frequency = mul_div_ceil(
        start.1.saturating_sub(apic_end).into(),
        i8254::FREQUENCY_HZ,
        window,
    );
    assert!(
        tsc_frequency > 0 && apic_frequency > 0,
        "TSC or local APIC timer did not count during the PIT measurement"
    );
    TSC_FREQUENCY.store(tsc_frequency, Ordering::Relaxed);
    APIC_TIMER_FREQUENCY.store(apic_frequency, Ordering::Relaxed);
    apic.configure_one_shot_timer(TIMER_VECTOR, false);
}

/// The TSC.
#[inline]
pub(super) fn now() -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: `lfence` keeps the TSC read from being performed before earlier instructions,
    // so a deadline check after it sees the time it was made; `rdtsc` has no side effects.
    unsafe {
        core::arch::asm!("lfence", "rdtsc", out("eax") low, out("edx") high,
            options(nomem, nostack, preserves_flags));
    }
    u64::from(high) << 32 | u64::from(low)
}

#[inline]
pub(super) fn frequency() -> u64 {
    TSC_FREQUENCY.load(Ordering::Relaxed)
}

pub(super) fn set_deadline(deadline: u64) {
    DEADLINE.store(deadline, Ordering::Relaxed);
    arm(deadline);
}

/// Starts the APIC timer to expire at TSC `deadline` (at the earliest representable count).
fn arm(deadline: u64) {
    let remaining = deadline.saturating_sub(now());
    let count = mul_div_ceil(
        remaining,
        APIC_TIMER_FREQUENCY.load(Ordering::Relaxed),
        TSC_FREQUENCY.load(Ordering::Relaxed),
    );
    // 0 would stop the timer instead of firing it at once.
    local_apic().start_timer(count.clamp(1, u32::MAX.into()) as u32);
}

/// Handles the APIC timer's interrupt.
pub(super) fn interrupt() {
    local_apic().end_of_interrupt();
    let deadline = DEADLINE.load(Ordering::Relaxed);
    if deadline == NO_DEADLINE {
        return;
    }
    if now() < deadline {
        arm(deadline);
        return;
    }
    DEADLINE.store(NO_DEADLINE, Ordering::Relaxed);
    crate::interrupt::deliver(Interrupt::Timer);
}

/// Handles a vector in [`LEGACY_PIC_VECTORS`]; returns `false` if it was not a spurious
/// interrupt (every input is masked, so anything else is unexpected).
pub(super) fn legacy_pic_interrupt(vector: u64) -> bool {
    match vector {
        LEGACY_PIC_PRIMARY_SPURIOUS => true,
        LEGACY_PIC_SECONDARY_SPURIOUS => {
            legacy_pic().end_secondary_spurious();
            true
        }
        _ => false,
    }
}

fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: `rdmsr` of an architectural MSR (here IA32_APIC_BASE, present on every x86_64
    // CPU) at ring 0 has no side effects.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high,
            options(nomem, nostack, preserves_flags));
    }
    u64::from(high) << 32 | u64::from(low)
}
