//! The RISC-V timer: the `time` counter and the supervisor timer interrupt, armed through the
//! SBI Timer extension (SBI spec v2.0, chapter 6), which works with or without the Sstc
//! extension.

/// QEMU `virt`'s `/cpus/timebase-frequency`: the `time` counter ticks at 10 MHz. Read from the
/// device tree once the kernel parses it (M5).
pub(super) const TIMEBASE_FREQUENCY: u64 = 10_000_000;

/// SBI Timer extension, `sbi_set_timer` function.
const SBI_EXT_TIME: usize = 0x5449_4d45;
const SBI_TIME_SET_TIMER: usize = 0;

/// `sie` bit 5: supervisor timer interrupt enable.
const SIE_STIE: usize = 1 << 5;

/// Disarms the timer and enables the supervisor timer interrupt in `sie`, so it is taken once
/// `sstatus.SIE` is set. Called once by the entry code, before `kernel_main`.
pub(super) extern "C" fn init() {
    disarm();
    // SAFETY: setting `sie.STIE` only lets the (now disarmed) timer interrupt reach the trap
    // vector, which is installed, once interrupts are unmasked in `sstatus`.
    unsafe { core::arch::asm!("csrs sie, {}", in(reg) SIE_STIE, options(nomem, nostack)) };
}

/// The `time` counter.
#[inline]
pub(super) fn now() -> u64 {
    let ticks: u64;
    // SAFETY: reading `time` has no side effects (the SBI firmware enables it for S-mode,
    // directly or by emulating the read).
    unsafe { core::arch::asm!("rdtime {}", out(reg) ticks, options(nomem, nostack)) };
    ticks
}

/// Raises the supervisor timer interrupt once `time` >= `deadline`; this also clears a pending
/// timer interrupt whose deadline is replaced.
pub(super) fn set_deadline(deadline: u64) {
    // SAFETY: an SBI call per the SBI calling convention: a7 = extension, a6 = function,
    // a0 = the 64-bit deadline; a0/a1 return error and value. It touches none of our memory.
    unsafe {
        core::arch::asm!(
            "ecall",
            inout("a0") deadline as usize => _,
            out("a1") _,
            in("a6") SBI_TIME_SET_TIMER,
            in("a7") SBI_EXT_TIME,
            options(nomem, nostack),
        );
    }
}

/// Pushes the deadline out of reach, which also clears the pending timer interrupt.
pub(super) fn disarm() {
    set_deadline(u64::MAX);
}
