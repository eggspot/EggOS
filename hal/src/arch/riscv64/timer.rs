//! The RISC-V timer: the `time` counter and the supervisor timer interrupt, armed through the
//! SBI Timer extension (SBI spec v2.0, chapter 6), which works with or without the Sstc
//! extension.

/// QEMU `virt`'s `/cpus/timebase-frequency`: the `time` counter ticks at 10 MHz. Read from the
/// device tree once the kernel parses it (M5).
pub(super) const TIMEBASE_FREQUENCY: u64 = 10_000_000;

static FREQUENCY: crate::atomic::AtomicU64 = crate::atomic::AtomicU64::new(TIMEBASE_FREQUENCY);

/// The `time` counter's frequency.
#[inline]
pub(super) fn frequency() -> u64 {
    FREQUENCY.load(core::sync::atomic::Ordering::Relaxed)
}

const SBI_EXT_TIME: usize = 0x5449_4d45;
const SBI_TIME_SET_TIMER: usize = 0;

const SIE_STIE: usize = 1 << 5;
const SIE_SSIE: usize = 1 << 1;

const SBI_EXT_IPI: usize = 0x73_5049;
const SBI_IPI_SEND_IPI: usize = 0;

/// Disarms this hart's timer and enables the supervisor timer and software interrupts in `sie`,
/// so both are taken once `sstatus.SIE` is set. Called by each hart before it runs the kernel's
/// code: the boot hart's entry code before `kernel_main`, a secondary hart's before its entry.
pub(super) extern "C" fn init() {
    if let Some(tree) = super::device_tree_bytes() {
        if crate::fdt::riscv_has_extension(tree, b"sstc") {
            SSTC.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        if let Some(hertz) = crate::fdt::timebase_frequency(tree).filter(|&hertz| hertz != 0) {
            FREQUENCY.store(hertz, core::sync::atomic::Ordering::Relaxed);
        }
    }
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

/// Whether the harts have Sstc (D261): the deadline is written to `stimecmp` directly, with no
/// call into the SBI firmware. Set by the boot hart from the device tree.
pub(super) static SSTC: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Raises the supervisor timer interrupt once `time` >= `deadline`; this also clears a pending
/// timer interrupt whose deadline is replaced.
pub(super) fn set_deadline(deadline: u64) {
    if SSTC.load(core::sync::atomic::Ordering::Relaxed) {
        // SAFETY: `stimecmp` (CSR 0x14d) exists with Sstc, which the device tree said the harts
        // have and the SBI firmware enables for S-mode (`menvcfg.STCE`); writing it only moves
        // this hart's timer interrupt.
        unsafe { core::arch::asm!("csrw 0x14d, {}", in(reg) deadline, options(nomem, nostack)) };
        return;
    }
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
