//! The MIPS timer (D313): each CPU's 32-bit `Count`, which runs at half the CPU clock, and its
//! `Compare`, which raises interrupt 7 when `Count` reaches it; wake-ups between CPUs are IPIs
//! (interrupt 6) through the Loongson IPI block. `Count` wraps every few tens of seconds, so the
//! clock the kernel reads is one 64-bit count every CPU extends from its own: each read adds
//! what `Count` moved on since the last read anywhere, which needs a read at least every 2^31
//! ticks; with no deadline, `Compare` is set half a wrap ahead, so a tick comes by itself.

use crate::atomic::AtomicU64;
use super::{cpu_number, ipi_register};
use core::sync::atomic::Ordering;

/// `Cause.IP` bits of the timer and the IPIs.
const TIMER_PENDING: u64 = 1 << 15;
const IPI_PENDING: u64 = 1 << 14;
/// The IPI block's registers: status, enable, set and clear.
const IPI_STATUS: u64 = 0x00;
const IPI_ENABLE: u64 = 0x04;
const IPI_SET: u64 = 0x08;
const IPI_CLEAR: u64 = 0x0c;
/// The IPI the kernel's wake-ups use.
const WAKE_BIT: u32 = 1;
/// The furthest ahead `Compare` is set: half a wrap, so the extension never misses one.
const LONGEST_COUNT: u64 = 1 << 31;
/// The shortest: a deadline already passed still raises the interrupt.
const SHORTEST_COUNT: u64 = 64;

static FREQUENCY: AtomicU64 = AtomicU64::new(100_000_000);
/// The 64-bit clock as last read on any CPU.
static CLOCK: AtomicU64 = AtomicU64::new(0);

/// Sets the counter's frequency from the boot parameters' CPU clock.
pub(super) fn set_frequency(frequency: u64) {
    FREQUENCY.store(frequency, Ordering::Relaxed);
}

#[inline]
pub(super) fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Relaxed)
}

fn count() -> u32 {
    let value: u32;
    // SAFETY: reading `Count` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $9", out(reg) value, options(nomem, nostack)) };
    value
}

fn set_compare(value: u32) {
    // SAFETY: writing `Compare` only sets when the timer interrupt is raised next, and clears
    // the one pending.
    unsafe { core::arch::asm!("mtc0 {0}, $11", "ehb", in(reg) value, options(nomem, nostack)) };
}

/// Disarms this CPU's timer (half a wrap ahead), enables and clears its IPIs.
pub(super) extern "C" fn init() {
    disarm();
    let block = |offset| ipi_register(cpu_number(), offset);
    // SAFETY: this CPU's IPI block, through the uncached window: enabling and clearing its IPIs
    // only decides which raise interrupt 6, which the entry takes once interrupts are unmasked.
    unsafe {
        core::ptr::write_volatile(block(IPI_ENABLE) as *mut u32, u32::MAX);
        let pending = core::ptr::read_volatile(block(IPI_STATUS) as *const u32);
        core::ptr::write_volatile(block(IPI_CLEAR) as *mut u32, pending);
    }
}

/// The 64-bit clock: the last value read anywhere plus how far this CPU's `Count` moved past
/// it; a `Count` a little behind (another CPU's ran ahead) reads as that last value.
#[inline]
pub(super) fn now() -> u64 {
    let mut last = CLOCK.load(Ordering::Relaxed);
    loop {
        let moved = count().wrapping_sub(last as u32);
        if moved >= 1 << 31 {
            return last;
        }
        let next = last + u64::from(moved);
        match CLOCK.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => last = current,
        }
    }
}

/// Raises the timer interrupt once the clock reaches `deadline`; `u64::MAX` disarms it.
pub(super) fn set_deadline(deadline: u64) {
    if deadline == u64::MAX {
        return disarm();
    }
    let ahead = deadline.saturating_sub(now()).clamp(SHORTEST_COUNT, LONGEST_COUNT);
    set_compare(count().wrapping_add(ahead as u32));
}

/// Sets `Compare` half a wrap ahead: no deadline, but a tick that keeps [`now`] extending.
pub(super) fn disarm() {
    set_compare(count().wrapping_add(LONGEST_COUNT as u32));
}

/// Marks this CPU's timer interrupt handled: `Compare` half a wrap ahead until the kernel sets
/// a deadline.
pub(super) fn end_timer_interrupt() {
    now();
    disarm();
}

/// Sends the wake-up IPI to CPU `cpu`.
pub(super) fn wake(cpu: u64) {
    super::doorbell_barrier();
    // SAFETY: CPU `cpu`'s IPI set register, through the uncached window: it only raises that
    // CPU's interrupt 6.
    unsafe { core::ptr::write_volatile(ipi_register(cpu, IPI_SET) as *mut u32, WAKE_BIT) };
}

/// Clears this CPU's pending IPIs, before the kernel looks at why it was woken, so a wake-up
/// sent meanwhile is taken again.
pub(super) fn end_wakeup() {
    let block = |offset| ipi_register(cpu_number(), offset);
    // SAFETY: as in `init`.
    unsafe {
        let pending = core::ptr::read_volatile(block(IPI_STATUS) as *const u32);
        core::ptr::write_volatile(block(IPI_CLEAR) as *mut u32, pending);
    }
}

/// Whether `cause` holds the timer's interrupt or an IPI.
pub(super) fn pending(cause: u64) -> (bool, bool) {
    (cause & TIMER_PENDING != 0, cause & IPI_PENDING != 0)
}
