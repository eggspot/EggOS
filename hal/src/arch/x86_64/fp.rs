//! x86_64 FP/SIMD registers of user programs (D54): the x87, MMX and SSE registers and `MXCSR`,
//! saved with `FXSAVE64` and loaded with `FXRSTOR64`. `CR0.TS` turns them on and off, for the
//! kernel as well as ring 3 (#NM); the kernel, built without SSE or x87
//! (`x86_64-unknown-none`), never touches them. Each CPU sets `CR4.OSFXSR` and `OSXMMEXCPT`
//! (`set_up_user_mode`) and leaves `CR4.OSXSAVE` clear: no AVX state yet, and `CPUID` tells
//! programs so.

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .balign 16
    .global eggos_fp_save
    // rdi: the state, `FXSAVE64`'s 512-byte image, 16-byte aligned.
eggos_fp_save:
    fxsave64 (%rdi)
    retq
    int3

    .global eggos_fp_restore
    // rdi: the state. AMD CPUs without CPUID 0x80000008:EBX[2] leave the x87 last-instruction
    // and last-operand pointers as they were when `FXRSTOR` finds no exception pending
    // (CVE-2006-1056): an x87 load first sets them to the kernel's, not the previous program's.
    // `fnclex` keeps it from raising a pending exception, and freeing st(7) gives it room.
eggos_fp_restore:
    fnclex
    ffree   %st(7)
    fildl   eggos_fp_zero(%rip)
    fxrstor64 (%rdi)
    retq
    int3
    .popsection

    .pushsection .rodata.fp, "a"
    .balign 4
eggos_fp_zero:
    .long   0
    .popsection
"#,
    options(att_syntax)
);

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// `CR0.TS`: x87, MMX and SSE instructions raise #NM, in ring 0 and ring 3.
const CR0_TS: u64 = 1 << 3;

/// [`crate::Platform::set_user_fp`]. A `CR0` write serializes the CPU: the scheduler turns
/// the registers off only for a program that has none, when the CPU's are on.
pub(super) unsafe fn set_user(on: bool) {
    if on {
        // SAFETY: `CR0.TS` only decides whether x87, MMX and SSE instructions raise #NM; the
        // kernel uses none of them.
        unsafe { core::arch::asm!("clts", options(nomem, nostack, preserves_flags)) };
    } else {
        // SAFETY: as above.
        unsafe {
            core::arch::asm!(
                "mov {tmp}, cr0",
                "or {tmp}, {ts}",
                "mov cr0, {tmp}",
                tmp = out(reg) _,
                ts = in(reg) CR0_TS,
                options(nomem, nostack),
            )
        };
    }
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable and `CR0.TS` clear; `FpState` is
    // 64-byte aligned, more than `FXSAVE64` needs, and its 512 bytes are the image.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read. The image came from `FXSAVE64` or `FpState::new`, so its
    // `MXCSR` has no reserved bit set, which would raise #GP. The x87 load in the routine
    // changes only x87 state, which `FXRSTOR64` then replaces. It overwrites `MXCSR` and the
    // x87 control word, which the System V ABI says a call preserves: sound only because the
    // kernel target has no SSE or x87 and holds nothing there.
    unsafe { eggos_fp_restore(state) }
}
