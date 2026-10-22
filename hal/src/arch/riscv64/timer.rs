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
/// `sie` / `sip` bit 1: supervisor software interrupt (enable / pending), which the SBI IPI
/// extension raises: the wake-up interrupt between harts (D41).
const SIE_SSIE: usize = 1 << 1;

/// SBI IPI extension, `sbi_send_ipi` function (SBI spec v2.0, chapter 7).
const SBI_EXT_IPI: usize = 0x73_5049;
const SBI_IPI_SEND_IPI: usize = 0;

/// Disarms this hart's timer and enables the supervisor timer and software interrupts in `sie`,
/// so both are taken once `sstatus.SIE` is set. Called by each hart before it runs the kernel's
/// code: the boot hart's entry code before `kernel_main`, a secondary hart's before its entry.
pub(super) extern "C" fn init() {
    disarm();
    // SAFETY: setting `sie.STIE` and `sie.SSIE` only lets the (now disarmed) timer interrupt and
    // the wake-up interrupt reach the trap vector, which is installed, once interrupts are
    // unmasked in `sstatus`.
    unsafe {
        core::arch::asm!("csrs sie, {}", in(reg) SIE_STIE | SIE_SSIE, options(nomem, nostack))
    };
}

/// Raises the supervisor software interrupt on hart `hart` through SBI.
pub(super) fn wake(hart: u64) {
    // SAFETY: a barrier only: the caller's writes to memory are complete before the SBI
    // firmware raises the interrupt on the other hart, which then sees them.
    unsafe { core::arch::asm!("fence rw, rw", options(nostack)) };
    // SAFETY: `sbi_send_ipi` with hart mask 1 at base `hart` names that hart alone and touches
    // none of our memory. A hart that does not exist is an error the firmware returns, which
    // the caller's contract rules out.
    let (error, _) =
        unsafe { super::sbi_call(SBI_EXT_IPI, SBI_IPI_SEND_IPI, [1, hart as usize, 0]) };
    // The IPI extension is in every SBI from v0.2 on, which the kernel needs anyway (HSM, D40).
    debug_assert_eq!(error, 0, "SBI could not send a wake-up to hart {hart}");
}

/// Clears this hart's pending wake-up interrupt.
pub(super) fn end_wakeup() {
    // SAFETY: clearing `sip.SSIP` only marks the software interrupt handled.
    unsafe { core::arch::asm!("csrc sip, {}", in(reg) SIE_SSIE, options(nomem, nostack)) };
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
