//! LoongArch64 FP registers of user programs (D290): `f0`-`f31`, the eight condition flags and
//! `fcsr0`. The kernel is built soft-float (`loongarch64-unknown-none-softfloat`), so it never
//! touches them; only these routines do. `EUEN.FPE` turns them on and off; a program's first
//! FP instruction with it off raises the FP-disabled exception, which the HAL reports as an
//! illegal instruction, as the kernel's lazy start expects (D54). LSX and LASX stay off.

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .balign 4
    .global eggos_fp_save
eggos_fp_save:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fst.d       $f\i, $a0, \i * 8
    .endr
    move        $t1, $zero
    .irp        i, 7,6,5,4,3,2,1,0
    movcf2gr    $t0, $fcc\i
    slli.d      $t1, $t1, 8
    or          $t1, $t1, $t0
    .endr
    st.d        $t1, $a0, 256
    movfcsr2gr  $t0, $fcsr0
    st.d        $t0, $a0, 264
    jirl        $zero, $ra, 0

    .global eggos_fp_restore
eggos_fp_restore:
    .irp        i, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fld.d       $f\i, $a0, \i * 8
    .endr
    ld.d        $t1, $a0, 256
    .irp        i, 0,1,2,3,4,5,6,7
    andi        $t0, $t1, 1
    movgr2cf    $fcc\i, $t0
    srli.d      $t1, $t1, 8
    .endr
    ld.d        $t0, $a0, 264
    movgr2fcsr  $fcsr0, $t0
    jirl        $zero, $ra, 0
    .popsection
"#,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::Loongarch64State, fcc) == 256);
const _: () = assert!(core::mem::offset_of!(crate::fp::Loongarch64State, fcsr) == 264);

const EUEN_FPE: u64 = 1;

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// [`crate::Platform::set_user_fp`]: turns the FP unit on or off for the next program.
pub(super) unsafe fn set_user(on: bool) {
    let value = if on { EUEN_FPE } else { 0 };
    // SAFETY: `EUEN.FPE` only decides whether FP instructions trap; the kernel uses none.
    unsafe {
        core::arch::asm!(
            "csrxchg {value}, {mask}, 0x2",
            value = inout(reg) value => _,
            mask = in(reg) EUEN_FPE,
            options(nomem, nostack),
        )
    };
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable and the FP unit on; `Loongarch64State`
    // is the layout the routine stores. It clobbers t0 and t1, which a call may clobber.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; it also overwrites fs0-fs7, which the calling convention
    // says a call preserves: sound only because the kernel is soft-float and holds nothing there.
    unsafe { eggos_fp_restore(state) }
}
