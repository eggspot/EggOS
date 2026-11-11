//! The SPARC64 timer (D326): `%tick`, which counts CPU cycles, and the UltraSPARC's
//! `TICK_COMPARE` (`%asr23`), which raises `SOFTINT.TICK_INT` and interrupt level 14 when
//! `%tick` reaches it. QEMU's `sun4u` has one CPU, so there are no wake-ups between CPUs.

use core::sync::atomic::{AtomicU64, Ordering};

/// `TICK_COMPARE`'s interrupt-disable bit: the comparator raises nothing while it is set.
const COMPARE_DISABLED: u64 = 1 << 63;
/// `SOFTINT.TICK_INT`.
pub(super) const TICK_INTERRUPT: u64 = 1;
/// The interrupt level `TICK_INT` raises.
pub(super) const TICK_LEVEL: u64 = 14;

static FREQUENCY: AtomicU64 = AtomicU64::new(100_000_000);

/// Sets the counter's frequency from the CPU node's `clock-frequency`.
pub(super) fn set_frequency(frequency: u64) {
    if frequency != 0 {
        FREQUENCY.store(frequency, Ordering::Relaxed);
    }
}

#[inline]
pub(super) fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Relaxed)
}

/// `%tick`, without its non-privileged-trap bit.
#[inline]
pub(super) fn now() -> u64 {
    let value: u64;
    // SAFETY: reading `%tick` has no side effects.
    unsafe { core::arch::asm!("rd %tick, {0}", out(reg) value, options(nomem, nostack)) };
    value & !(1 << 63)
}

fn set_compare(value: u64) {
    // SAFETY: `TICK_COMPARE` only sets when this CPU's timer interrupt comes next.
    unsafe { core::arch::asm!("wr {0}, 0, %asr23", in(reg) value, options(nomem, nostack)) };
}

/// Clears `SOFTINT.TICK_INT`.
pub(super) fn clear_interrupt() {
    // SAFETY: `%clear_softint` (`%asr21`) clears only the bits written.
    unsafe {
        core::arch::asm!("wr {0}, 0, %asr21", in(reg) TICK_INTERRUPT, options(nomem, nostack))
    };
}

/// Disarms this CPU's timer and clears its pending interrupt.
pub(super) extern "C" fn init() {
    set_compare(COMPARE_DISABLED);
    clear_interrupt();
}

/// Raises the timer interrupt once `%tick` passes `deadline`; `u64::MAX` disarms it.
pub(super) fn set_deadline(deadline: u64) {
    match deadline {
        u64::MAX => set_compare(COMPARE_DISABLED),
        _ => set_compare(deadline.max(now() + 1) & !COMPARE_DISABLED),
    }
}

/// Marks the timer's interrupt handled: disarmed until the kernel sets a deadline.
pub(super) fn end_timer_interrupt() {
    set_compare(COMPARE_DISABLED);
    clear_interrupt();
}
