//! A program's FP, vector and VSX registers (D54, D304): the 64 VSX registers (the FP
//! registers are the first 32's high doublewords, the vector registers the last 32), `FPSCR`,
//! `VSCR` and `VRSAVE`. They are off for programs until first used (`MSR.FP`, `VEC` and `VSX`,
//! which each CPU's way into user mode sets from its scratch area's flag), and off for the
//! kernel, which turns them on only around a save or a restore.

use super::{exception, msr};
use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .machine power9
    .balign 4
    .global ppc64_fp_save
ppc64_fp_save:
    .irp        n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    stxv        \n, (\n * 16)(%r3)
    .endr
    .irp        n, 32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63
    stxv        \n, (\n * 16)(%r3)
    .endr
    mffs        0
    stfd        0, 1024(%r3)
    mfvscr      0
    li          %r4, 1032
    stvewx      0, %r3, %r4
    mfspr       %r4, 256
    stw         %r4, 1036(%r3)
    blr

    .global ppc64_fp_restore
ppc64_fp_restore:
    lfd         0, 1024(%r3)
    mtfsf       0xff, 0
    li          %r4, 1032
    lvewx       0, %r3, %r4
    mtvscr      0
    lwz         %r4, 1036(%r3)
    mtspr       256, %r4
    .irp        n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    lxv         \n, (\n * 16)(%r3)
    .endr
    .irp        n, 32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63
    lxv         \n, (\n * 16)(%r3)
    .endr
    blr
    .popsection
"#
);

unsafe extern "C" {
    fn ppc64_fp_save(state: *mut u8);
    fn ppc64_fp_restore(state: *const u8);
}

fn with_units(body: impl FnOnce()) {
    let saved: u64;
    // SAFETY: turning the units on only lets the next instructions use them; `MSR` is put back
    // after.
    unsafe {
        core::arch::asm!(
            "mfmsr {saved}",
            "or {on}, {saved}, {units}",
            "mtmsrd {on}, 0",
            "isync",
            saved = out(reg) saved,
            on = out(reg) _,
            units = in(reg) msr::USER_FP,
            options(nomem, nostack),
        )
    };
    body();
    // SAFETY: puts `MSR` back as it was.
    unsafe { core::arch::asm!("mtmsrd {}, 0", "isync", in(reg) saved, options(nomem, nostack)) };
}

/// [`crate::Platform::set_user_fp`]: whether programs on this CPU run with the units on.
///
/// # Safety
/// As for [`crate::Platform::set_user_fp`].
pub(super) unsafe fn set_user(on: bool) {
    exception::set_user_fp(on);
}

/// [`crate::Platform::save_fp`].
///
/// # Safety
/// As for [`crate::Platform::save_fp`]: `state` is valid and the registers hold the program's.
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is valid; it is 64-byte aligned and holds 1040 bytes.
    with_units(|| unsafe { ppc64_fp_save(state.cast()) });
}

/// [`crate::Platform::restore_fp`].
///
/// # Safety
/// As for [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`.
    with_units(|| unsafe { ppc64_fp_restore(state.cast()) });
}
