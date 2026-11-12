//! The LoongArch timer (D290): the stable counter `rdtime.d` reads, at the frequency `CPUCFG`
//! words 4 and 5 give, and each core's one-shot timer (`TCFG`), which counts down at that
//! frequency and raises interrupt 11; wake-ups between cores are IPIs (interrupt 12) through
//! the IOCSR interrupt block.

use crate::atomic::AtomicU64;
use super::{csr, csr_read, csr_write, iocsr};
use core::sync::atomic::Ordering;

/// `ECFG.LIE`'s timer and IPI bits.
const TIMER_INTERRUPT: u64 = 1 << 11;
const IPI_INTERRUPT: u64 = 1 << 12;
/// `TCFG`: enabled, one-shot; the count is the rest of the value, a multiple of 4.
const TCFG_ENABLE: u64 = 1;
const TCFG_COUNT: u64 = !3;
/// The longest one-shot count `TCFG` takes, about 2^46 ticks.
const LONGEST_COUNT: u64 = (1 << 46) - 4;
/// The IPI vector the kernel's wake-ups use.
const WAKE_VECTOR: u32 = 0;

static FREQUENCY: AtomicU64 = AtomicU64::new(100_000_000);

fn cpucfg(word: u64) -> u64 {
    let value: u64;
    // SAFETY: `cpucfg` only reads the CPU's configuration words.
    unsafe { core::arch::asm!("cpucfg {0}, {1}", out(reg) value, in(reg) word, options(nomem, nostack)) };
    value
}

/// The stable counter's frequency: `CPUCFG` word 4's crystal frequency times word 5's
/// multiplier over its divisor.
#[inline]
pub(super) fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Relaxed)
}

/// Reads this CPU's counter frequency, disarms its timer, clears its pending IPIs and enables the
/// timer and IPI interrupts in `ECFG`, so both are taken once interrupts are unmasked.
pub(super) extern "C" fn init() {
    let base = cpucfg(4) & 0xffff_ffff;
    let scale = cpucfg(5);
    let (multiplier, divisor) = (scale & 0xffff, (scale >> 16) & 0xffff);
    if base != 0 && multiplier != 0 && divisor != 0 {
        FREQUENCY.store(base * multiplier / divisor, Ordering::Relaxed);
    }
    disarm();
    csr_write!(csr::TICLR, 1);
    super::iocsr_write32(iocsr::IPI_ENABLE, u32::MAX);
    super::iocsr_write32(iocsr::IPI_CLEAR, super::iocsr_read32(iocsr::IPI_STATUS));
    let mask = TIMER_INTERRUPT | IPI_INTERRUPT;
    // SAFETY: setting `ECFG.LIE`'s timer and IPI bits only lets those interrupts reach the
    // entry, which is installed, once `CRMD.IE` is set.
    unsafe {
        core::arch::asm!(
            "csrxchg {value}, {mask}, 0x4",
            value = inout(reg) mask => _,
            mask = in(reg) mask,
            options(nomem, nostack),
        )
    };
}

/// The stable counter.
#[inline]
pub(super) fn now() -> u64 {
    let ticks: u64;
    // SAFETY: reading the stable counter has no side effects.
    unsafe { core::arch::asm!("rdtime.d {0}, $zero", out(reg) ticks, options(nomem, nostack)) };
    ticks
}

/// Raises the timer interrupt once the counter reaches `deadline`; `u64::MAX` disarms it.
pub(super) fn set_deadline(deadline: u64) {
    if deadline == u64::MAX {
        return disarm();
    }
    let count = deadline.saturating_sub(now()).clamp(4, LONGEST_COUNT);
    csr_write!(csr::TICLR, 1);
    csr_write!(csr::TCFG, count & TCFG_COUNT | TCFG_ENABLE);
}

/// Stops this CPU's timer.
pub(super) fn disarm() {
    csr_write!(csr::TCFG, 0);
}

/// Marks this CPU's timer interrupt handled.
pub(super) fn end_timer_interrupt() {
    csr_write!(csr::TICLR, 1);
}

/// Sends the wake-up IPI to CPU `cpu`.
pub(super) fn wake(cpu: u64) {
    // SAFETY: a barrier only: the caller's writes are complete before the other CPU sees the IPI.
    unsafe { core::arch::asm!("dbar 0", options(nostack)) };
    let command = iocsr::SEND_BLOCKING as u32 | (cpu as u32) << iocsr::SEND_CPU_SHIFT | WAKE_VECTOR;
    super::iocsr_write32(iocsr::IPI_SEND, command);
}

/// Clears this CPU's pending IPIs, before the kernel looks at why it was woken, so a wake-up
/// sent meanwhile is taken again.
pub(super) fn end_wakeup() {
    super::iocsr_write32(iocsr::IPI_CLEAR, super::iocsr_read32(iocsr::IPI_STATUS));
}

/// Whether `estat`'s pending interrupts hold the timer's or an IPI.
pub(super) fn pending(estat: u64) -> (bool, bool) {
    (estat & TIMER_INTERRUPT != 0, estat & IPI_INTERRUPT != 0)
}

#[allow(dead_code)]
fn timer_config() -> u64 {
    csr_read!(csr::TCFG)
}
