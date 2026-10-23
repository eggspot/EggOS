//! AArch64 FP/SIMD registers of user programs (D54): `V0`-`V31`, `FPCR` and `FPSR`, saved and
//! loaded from EL1, which `CPACR_EL1.FPEN` never traps (0b01 traps EL0 only, 0b11 neither);
//! the kernel itself, built soft-float (D10), uses none of them. SVE and SME stay trapped
//! (`ZEN` and `SMEN` 0).

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .arch_extension fp
    .arch_extension simd
    .balign 4
    .global eggos_fp_save
    // x0: the state, laid out as `Aarch64State`.
eggos_fp_save:
    stp     q0, q1, [x0, #0]
    stp     q2, q3, [x0, #32]
    stp     q4, q5, [x0, #64]
    stp     q6, q7, [x0, #96]
    stp     q8, q9, [x0, #128]
    stp     q10, q11, [x0, #160]
    stp     q12, q13, [x0, #192]
    stp     q14, q15, [x0, #224]
    stp     q16, q17, [x0, #256]
    stp     q18, q19, [x0, #288]
    stp     q20, q21, [x0, #320]
    stp     q22, q23, [x0, #352]
    stp     q24, q25, [x0, #384]
    stp     q26, q27, [x0, #416]
    stp     q28, q29, [x0, #448]
    stp     q30, q31, [x0, #480]
    mrs     x1, fpcr
    mrs     x2, fpsr
    str     x1, [x0, #512]
    str     x2, [x0, #520]
    ret

    .global eggos_fp_restore
    // x0: the state.
eggos_fp_restore:
    ldp     q0, q1, [x0, #0]
    ldp     q2, q3, [x0, #32]
    ldp     q4, q5, [x0, #64]
    ldp     q6, q7, [x0, #96]
    ldp     q8, q9, [x0, #128]
    ldp     q10, q11, [x0, #160]
    ldp     q12, q13, [x0, #192]
    ldp     q14, q15, [x0, #224]
    ldp     q16, q17, [x0, #256]
    ldp     q18, q19, [x0, #288]
    ldp     q20, q21, [x0, #320]
    ldp     q22, q23, [x0, #352]
    ldp     q24, q25, [x0, #384]
    ldp     q26, q27, [x0, #416]
    ldp     q28, q29, [x0, #448]
    ldp     q30, q31, [x0, #480]
    ldr     x1, [x0, #512]
    ldr     x2, [x0, #520]
    msr     fpcr, x1
    msr     fpsr, x2
    ret
    .arch_extension nosimd
    .arch_extension nofp
    .popsection
"#
);

const _: () = assert!(core::mem::offset_of!(crate::fp::Aarch64State, fpcr) == 512);
const _: () = assert!(core::mem::offset_of!(crate::fp::Aarch64State, fpsr) == 520);

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// `CPACR_EL1.FPEN`: 0b11, FP/SIMD for EL0 and EL1; 0b01, for EL1 only (EL0 traps, EC 0x07).
pub(super) const CPACR_FP_ON: u64 = 0b11 << 20;
pub(super) const CPACR_FP_EL1_ONLY: u64 = 0b01 << 20;

/// [`crate::Platform::set_user_fp`].
pub(super) unsafe fn set_user(on: bool) {
    let cpacr = if on { CPACR_FP_ON } else { CPACR_FP_EL1_ONLY };
    // SAFETY: `CPACR_EL1` only decides which FP/SIMD accesses trap; the `isb` makes the change
    // apply to what follows. The caller loads the program's registers before turning them on.
    unsafe { core::arch::asm!("msr cpacr_el1, {}", "isb", in(reg) cpacr, options(nostack)) };
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable; `FpState` is 64-byte aligned and
    // `Aarch64State` the layout the routine stores. It clobbers x1 and x2, which the C calling
    // convention lets a call clobber; it leaves d8-d15, which a caller may expect kept, as
    // they were.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; the caller guarantees `state` holds a saved or initial
    // state, so `FPCR` gets only values a program could set itself. It also overwrites d8-d15
    // and `FPCR`, which the calling convention says a call preserves: sound only because the
    // kernel is soft-float (D10) and holds nothing there.
    unsafe { eggos_fp_restore(state) }
}
