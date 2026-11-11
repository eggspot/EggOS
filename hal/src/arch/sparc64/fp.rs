//! SPARC64 FP registers of user programs (D326): `%d0`-`%d62`, `%fsr` and `%gsr`. The kernel
//! is soft-float, so only these routines touch them. `PSTATE.PEF` and `%fprs.FEF` turn them on;
//! a program's first FP instruction with them off raises `fp_disabled`, which the HAL reports as
//! an illegal instruction, as the kernel's lazy start expects (D54).

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .balign 4
    .global eggos_fp_save
eggos_fp_save:
    .irp        i, 0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62
    std         %f\i, [%o0 + \i * 4]
    .endr
    stx         %fsr, [%o0 + 256]
    rd          %gsr, %o1
    retl
     stx        %o1, [%o0 + 264]

    .global eggos_fp_restore
eggos_fp_restore:
    .irp        i, 0,2,4,6,8,10,12,14,16,18,20,22,24,26,28,30,32,34,36,38,40,42,44,46,48,50,52,54,56,58,60,62
    ldd         [%o0 + \i * 4], %f\i
    .endr
    ldx         [%o0 + 256], %fsr
    ldx         [%o0 + 264], %o1
    retl
     wr         %o1, 0, %gsr
    .popsection
"#,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::Sparc64State, fsr) == 256);

/// `%fprs.FEF`: the FP registers are enabled.
const FPRS_FEF: u64 = 1 << 2;

/// Whether the next program gets the FP unit; the way into user mode sets `%fprs` from it.
pub(super) static USER_FP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

fn set_fprs(value: u64) {
    // SAFETY: `%fprs.FEF` only decides whether FP instructions trap; the kernel uses none.
    unsafe { core::arch::asm!("wr {0}, 0, %fprs", in(reg) value, options(nomem, nostack)) };
}

/// [`crate::Platform::set_user_fp`]: makes the FP unit usable or not for the next program, and
/// for these routines.
pub(super) unsafe fn set_user(on: bool) {
    USER_FP.store(on, core::sync::atomic::Ordering::Relaxed);
    set_fprs(if on { FPRS_FEF } else { 0 });
}

/// The `%fprs` a program starts with.
pub(super) fn user_fprs() -> u64 {
    match USER_FP.load(core::sync::atomic::Ordering::Relaxed) {
        true => FPRS_FEF,
        false => 0,
    }
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable and the FP unit on; `Sparc64State` is
    // the layout the routine stores. It clobbers `%o1`, which a call may clobber.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; the kernel is soft-float and holds nothing in the FP
    // registers it overwrites.
    unsafe { eggos_fp_restore(state) }
}
