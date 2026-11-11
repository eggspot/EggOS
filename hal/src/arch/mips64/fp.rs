//! MIPS64 FP registers of user programs (D313): `$f0`-`$f31`, 64 bits each with `Status.FR` set,
//! and `FCSR`. The kernel is soft-float, so it never touches them; only these routines do.
//! `Status.CU1` turns them on and off; a program's first FP instruction with it off raises the
//! coprocessor-unusable exception, which the HAL reports as an illegal instruction, as the
//! kernel's lazy start expects (D54).

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .set        push
    .set        hardfloat
    .balign 4
    .global eggos_fp_save
eggos_fp_save:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    sdc1        $f\i, \i * 8($4)
    .endr
    cfc1        $12, $31
    sd          $12, 256($4)
    jr          $31

    .global eggos_fp_restore
eggos_fp_restore:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ldc1        $f\i, \i * 8($4)
    .endr
    ld          $12, 256($4)
    ctc1        $12, $31
    jr          $31
    .set        pop
    .popsection
"#,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::Mips64State, fcsr) == 256);

/// `Status.CU1`: the FP unit is usable.
const STATUS_CU1: u64 = 1 << 29;

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// [`crate::Platform::set_user_fp`]: makes the FP unit usable or not for the next program, and
/// for these routines.
pub(super) unsafe fn set_user(on: bool) {
    let mut status: u64;
    // SAFETY: reading `Status` has no side effects.
    unsafe { core::arch::asm!("mfc0 {0}, $12", out(reg) status, options(nomem, nostack)) };
    status = match on {
        true => status | STATUS_CU1,
        false => status & !STATUS_CU1,
    };
    // SAFETY: `Status.CU1` only decides whether FP instructions trap; the kernel uses none, and
    // the rest of `Status` is written back as it was.
    unsafe { core::arch::asm!("mtc0 {0}, $12", "ehb", in(reg) status, options(nomem, nostack)) };
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable and the FP unit on; `Mips64State` is the
    // layout the routine stores. It clobbers t0, which a call may clobber.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; it also overwrites `$f24`-`$f31`, which the calling
    // convention says a call preserves: sound only because the kernel is soft-float and holds
    // nothing there.
    unsafe { eggos_fp_restore(state) }
}
