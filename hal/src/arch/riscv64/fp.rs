//! RISC-V FP registers of user programs (D54): `f0`-`f31` and `fcsr`. The kernel is built for
//! `riscv64imac-unknown-none-elf`, soft-float, so it never touches them; only these routines
//! do, assembled with D on. `sstatus.FS` turns them on and off, for S-mode as well as U-mode,
//! and says whether a program changed them since they were last saved or loaded (Dirty), so
//! a save of unchanged registers is skipped. No V yet (`sstatus.VS` stays Off).

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .option push
    .option arch, +d
    .balign 4
    .global eggos_fp_save
    // a0: the state, laid out as `Riscv64State`. Only if FS is Dirty; leaves it Clean.
eggos_fp_save:
    csrr    t0, sstatus
    li      t1, {fs_dirty}
    and     t0, t0, t1
    bne     t0, t1, 1f
    fsd     f0, 0(a0)
    fsd     f1, 8(a0)
    fsd     f2, 16(a0)
    fsd     f3, 24(a0)
    fsd     f4, 32(a0)
    fsd     f5, 40(a0)
    fsd     f6, 48(a0)
    fsd     f7, 56(a0)
    fsd     f8, 64(a0)
    fsd     f9, 72(a0)
    fsd     f10, 80(a0)
    fsd     f11, 88(a0)
    fsd     f12, 96(a0)
    fsd     f13, 104(a0)
    fsd     f14, 112(a0)
    fsd     f15, 120(a0)
    fsd     f16, 128(a0)
    fsd     f17, 136(a0)
    fsd     f18, 144(a0)
    fsd     f19, 152(a0)
    fsd     f20, 160(a0)
    fsd     f21, 168(a0)
    fsd     f22, 176(a0)
    fsd     f23, 184(a0)
    fsd     f24, 192(a0)
    fsd     f25, 200(a0)
    fsd     f26, 208(a0)
    fsd     f27, 216(a0)
    fsd     f28, 224(a0)
    fsd     f29, 232(a0)
    fsd     f30, 240(a0)
    fsd     f31, 248(a0)
    frcsr   t0
    sd      t0, 256(a0)
    li      t1, {fs_dirty_bit}
    csrc    sstatus, t1
1:  ret

    .global eggos_fp_restore
    // a0: the state. Leaves FS Clean: the registers are what the state holds.
eggos_fp_restore:
    fld     f0, 0(a0)
    fld     f1, 8(a0)
    fld     f2, 16(a0)
    fld     f3, 24(a0)
    fld     f4, 32(a0)
    fld     f5, 40(a0)
    fld     f6, 48(a0)
    fld     f7, 56(a0)
    fld     f8, 64(a0)
    fld     f9, 72(a0)
    fld     f10, 80(a0)
    fld     f11, 88(a0)
    fld     f12, 96(a0)
    fld     f13, 104(a0)
    fld     f14, 112(a0)
    fld     f15, 120(a0)
    fld     f16, 128(a0)
    fld     f17, 136(a0)
    fld     f18, 144(a0)
    fld     f19, 152(a0)
    fld     f20, 160(a0)
    fld     f21, 168(a0)
    fld     f22, 176(a0)
    fld     f23, 184(a0)
    fld     f24, 192(a0)
    fld     f25, 200(a0)
    fld     f26, 208(a0)
    fld     f27, 216(a0)
    fld     f28, 224(a0)
    fld     f29, 232(a0)
    fld     f30, 240(a0)
    fld     f31, 248(a0)
    ld      t0, 256(a0)
    fscsr   t0
    li      t1, {fs_dirty_bit}
    csrc    sstatus, t1
    ret
    .option pop
    .popsection
"#,
    fs_dirty = const SSTATUS_FS,
    fs_dirty_bit = const SSTATUS_FS_DIRTY_BIT,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::Riscv64State, fcsr) == 256);

/// `sstatus.FS`: Off 0, Initial 1, Clean 2, Dirty 3 (both bits).
const SSTATUS_FS: u64 = 3 << 13;
/// The bit that tells Dirty from Clean.
const SSTATUS_FS_DIRTY_BIT: u64 = 1 << 13;
/// `sstatus.FS` Clean.
const SSTATUS_FS_CLEAN: u64 = 2 << 13;

unsafe extern "C" {
    fn eggos_fp_save(state: *mut FpState);
    fn eggos_fp_restore(state: *const FpState);
}

/// [`crate::Platform::set_user_fp`]: on is Clean (the registers are what was last saved or
/// loaded, which the caller guarantees is the next program's), off is Off.
pub(super) unsafe fn set_user(on: bool) {
    // SAFETY: `sstatus.FS` only decides whether FP instructions trap and what a save skips;
    // the kernel uses none of them.
    unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) SSTATUS_FS, options(nostack)) };
    if on {
        // SAFETY: as above.
        unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) SSTATUS_FS_CLEAN, options(nostack)) };
    }
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable, FS is not Off, and a Clean FS means
    // `*state` holds the registers; `Riscv64State` is the layout the routine stores. It
    // clobbers t0 and t1, which the calling convention lets a call clobber.
    unsafe { eggos_fp_save(state) }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; `fcsr` gets only values a program could set itself. It
    // also overwrites fs0-fs11 and `fcsr`'s rounding mode, which the calling convention says a
    // call preserves: sound only because the kernel is soft-float and holds nothing there.
    unsafe { eggos_fp_restore(state) }
}
