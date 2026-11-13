//! s390x FP registers of user programs (D321): `f0`-`f15` and the FP control register. The
//! kernel is soft-float. CR0's additional-floating-point control turns `f1`, `f3`, `f5`, `f7`
//! and `f8`-`f15` on and off; with it off, an instruction naming one raises a data exception
//! with DXC 1, which the HAL reports as an illegal instruction, as the kernel's lazy start
//! expects (D54). `f0`, `f2`, `f4` and `f6` are usable either way, so the kernel's load of a
//! new state before a program without FP state runs is what keeps them from leaking.

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .balign 4
    .global eggos_fp_save
eggos_fp_save:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    std         %f\i, \i * 8(%r2)
    .endr
    stfpc       128(%r2)
    br          %r14

    .global eggos_fp_restore
eggos_fp_restore:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    ld          %f\i, \i * 8(%r2)
    .endr
    lfpc        128(%r2)
    br          %r14
    .popsection
"#,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::S390xState, fpc) == 128);

const CR0_AFP: u64 = 1 << 18;

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// [`crate::Platform::set_user_fp`]: the additional FP registers on or off for the next program,
/// and for these routines.
pub(super) unsafe fn set_user(on: bool) {
    let mut cr0 = 0u64;
    // SAFETY: `stctg` stores CR0 into `cr0`, which this function owns.
    unsafe { core::arch::asm!("stctg %c0, %c0, 0({0})", in(reg) &raw mut cr0, options(nostack)) };
    cr0 = match on {
        true => cr0 | CR0_AFP,
        false => cr0 & !CR0_AFP,
    };
    // SAFETY: only the AFP control changes, which decides which FP registers trap; the kernel
    // uses none.
    unsafe { core::arch::asm!("lctlg %c0, %c0, 0({0})", in(reg) &raw const cr0, options(nostack)) };
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable and the registers on.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`; it also overwrites `f8`-`f15`, which the ABI says a call preserves:
    // sound only because the kernel is soft-float.
    unsafe { eggos_fp_restore(state) }
}
