//! The s390x timer (D321): the TOD clock, whose bit 51 counts microseconds (4096 ticks a
//! microsecond, 4.096 GHz), and each CPU's clock comparator, which raises an external
//! interruption (code 0x1004) once the clock passes it; wake-ups between CPUs are SIGP
//! emergency signals (code 0x1201).

/// Ticks a second.
pub(super) const FREQUENCY: u64 = 4_096_000_000;
/// The external interruption codes the HAL takes.
pub(super) const CLOCK_COMPARATOR: u16 = 0x1004;
pub(super) const EMERGENCY_SIGNAL: u16 = 0x1201;
pub(super) const SERVICE_SIGNAL: u16 = 0x2401;
const SIGP_EMERGENCY_SIGNAL: u64 = 0x03;

/// The TOD clock.
#[inline]
pub(super) fn now() -> u64 {
    let mut clock = 0u64;
    // SAFETY: `stckf` stores the clock into `clock`, which this function owns.
    unsafe { core::arch::asm!("stckf 0({0})", in(reg) &raw mut clock, options(nostack)) };
    clock
}

fn set_comparator(value: u64) {
    // SAFETY: `sckc` only sets when this CPU's next clock-comparator interruption comes.
    unsafe { core::arch::asm!("sckc 0({0})", in(reg) &raw const value, options(nostack, readonly)) };
}

/// Raises the clock-comparator interruption once the clock passes `deadline`; `u64::MAX`
/// disarms it.
pub(super) fn set_deadline(deadline: u64) {
    set_comparator(deadline);
}

/// Disarms this CPU's comparator.
pub(super) extern "C" fn init() {
    set_comparator(u64::MAX);
}

/// Marks the comparator's interruption handled: disarmed until the kernel sets a deadline.
pub(super) fn end_timer_interrupt() {
    set_comparator(u64::MAX);
}

/// Sends CPU `cpu` an emergency signal, its wake-up.
pub(super) fn wake(cpu: u64) {
    super::doorbell_barrier();
    // SAFETY: `sigp` with the emergency-signal order only raises that CPU's external
    // interruption; the status it may store goes to the clobbered register.
    unsafe {
        core::arch::asm!(
            "sigp {status}, {cpu}, {order}",
            status = out(reg) _,
            cpu = in(reg) cpu,
            order = const SIGP_EMERGENCY_SIGNAL,
            options(nostack, nomem),
        )
    };
}
