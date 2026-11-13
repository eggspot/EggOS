//! RISC-V FP registers of user programs (D54): `f0`-`f31` and `fcsr`. The kernel is built for
//! `riscv64imac-unknown-none-elf`, soft-float, so it never touches them; only these routines
//! do, assembled with D on. `sstatus.FS` turns them on and off, for S-mode as well as U-mode,
//! and says whether a program changed them since they were last saved or loaded (Dirty), so
//! a save of unchanged registers is skipped. The V registers (D568) follow `sstatus.VS` the
//! same way, on harts whose device tree lists `v` and whose registers are at most
//! [`crate::fp::MAX_VECTOR_BYTES`] long; elsewhere `VS` stays Off and V traps.

use crate::fp::FpState;

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .option push
    .option arch, +d
    .balign 4
    .global eggos_fp_save
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

core::arch::global_asm!(
    r#"
    .pushsection .text.fp, "ax"
    .option push
    .option arch, +v
    .balign 4
    .global eggos_vector_save
eggos_vector_save:
    csrr    t0, sstatus
    li      t1, {vs_dirty}
    and     t0, t0, t1
    bne     t0, t1, 1f
    csrr    t0, vstart
    sd      t0, 0(a0)
    csrr    t0, vl
    sd      t0, 8(a0)
    csrr    t0, vtype
    sd      t0, 16(a0)
    csrr    t0, vcsr
    sd      t0, 24(a0)
    csrr    t0, vlenb
    sd      t0, 32(a0)
    slli    t1, t0, 3
    addi    a1, a0, 40
    vs8r.v  v0, (a1)
    add     a1, a1, t1
    vs8r.v  v8, (a1)
    add     a1, a1, t1
    vs8r.v  v16, (a1)
    add     a1, a1, t1
    vs8r.v  v24, (a1)
    li      t1, {vs_dirty_bit}
    csrc    sstatus, t1
1:  ret

    .global eggos_vector_restore
eggos_vector_restore:
    csrr    t0, vlenb
    slli    t1, t0, 3
    addi    a1, a0, 40
    vl8re8.v v0, (a1)
    add     a1, a1, t1
    vl8re8.v v8, (a1)
    add     a1, a1, t1
    vl8re8.v v16, (a1)
    add     a1, a1, t1
    vl8re8.v v24, (a1)
    ld      t0, 8(a0)
    ld      t1, 16(a0)
    vsetvl  x0, t0, t1
    ld      t0, 0(a0)
    csrw    vstart, t0
    ld      t0, 24(a0)
    csrw    vcsr, t0
    li      t1, {vs_dirty_bit}
    csrc    sstatus, t1
    ret
    .option pop
    .popsection
"#,
    vs_dirty = const SSTATUS_VS,
    vs_dirty_bit = const SSTATUS_VS_DIRTY_BIT,
);

const _: () = assert!(core::mem::offset_of!(crate::fp::VectorState, v) == 40);

/// `sstatus.VS`: Off 0, Initial 1, Clean 2, Dirty 3 (both bits).
const SSTATUS_VS: u64 = 3 << 9;
/// The bit that tells Dirty from Clean.
const SSTATUS_VS_DIRTY_BIT: u64 = 1 << 9;
/// `sstatus.VS` Clean.
const SSTATUS_VS_CLEAN: u64 = 2 << 9;

static VECTOR_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Whether user programs get V, and the bytes of one register: set once by the boot hart from
/// the device tree and `vlenb` (D568).
pub(super) fn vector_bytes() -> u64 {
    VECTOR_BYTES.load(core::sync::atomic::Ordering::Relaxed)
}

/// Reads `vlenb` with V on for a moment, and keeps it when it fits [`crate::fp::VectorState`].
pub(super) fn detect_vector() {
    // SAFETY: `sstatus.VS` Initial only lets V instructions run; the kernel runs none but the
    // `vlenb` read below, and VS goes back to Off at once.
    unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) SSTATUS_VS_DIRTY_BIT, options(nostack)) };
    let vlenb: u64;
    // SAFETY: reading `vlenb` has no side effects; V is on (the device tree lists it).
    unsafe { core::arch::asm!(".option push", ".option arch, +v", "csrr {}, vlenb", ".option pop", out(reg) vlenb, options(nomem, nostack)) };
    // SAFETY: as above, V off again.
    unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) SSTATUS_VS, options(nostack)) };
    if vlenb != 0 && vlenb as usize <= crate::fp::MAX_VECTOR_BYTES {
        VECTOR_BYTES.store(vlenb, core::sync::atomic::Ordering::Relaxed);
    }
}

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
    fn eggos_vector_save(state: *mut crate::fp::VectorState);
    fn eggos_vector_restore(state: *const crate::fp::VectorState);
}

/// [`crate::Platform::set_user_fp`]: on is Clean (the registers are what was last saved or
/// loaded, which the caller guarantees is the next program's), off is Off.
pub(super) unsafe fn set_user(on: bool) {
    // SAFETY: `sstatus.FS` only decides whether FP instructions trap and what a save skips;
    // the kernel uses none of them.
    unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) SSTATUS_FS | SSTATUS_VS, options(nostack)) };
    if on {
        let vector = match vector_bytes() {
            0 => 0,
            _ => SSTATUS_VS_CLEAN,
        };
        // SAFETY: as above, and `sstatus.VS` likewise for the V registers.
        unsafe {
            core::arch::asm!("csrs sstatus, {}", in(reg) SSTATUS_FS_CLEAN | vector, options(nostack))
        };
    }
}

/// [`crate::Platform::save_fp`].
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: the caller guarantees `state` is writable, FS is not Off, and a Clean FS means
    // `*state` holds the registers; `Riscv64State` is the layout the routine stores. It
    // clobbers t0 and t1, which the calling convention lets a call clobber.
    unsafe { eggos_fp_save(state) };
    if vector_bytes() != 0 {
        // SAFETY: `state` is writable (the caller's contract), so its V part is; VS is on
        // when V is (`set_user`), and the routine stores only when it is Dirty.
        unsafe {
            let vector = &raw mut (*state).arch.vector;
            (*vector).vlenb = vector_bytes();
            eggos_vector_save(vector)
        }
    }
}

/// [`crate::Platform::restore_fp`].
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: as in `save`, a read; `fcsr` gets only values a program could set itself. It
    // also overwrites fs0-fs11 and `fcsr`'s rounding mode, which the calling convention says a
    // call preserves: sound only because the kernel is soft-float and holds nothing there.
    unsafe { eggos_fp_restore(state) };
    if vector_bytes() != 0 {
        // SAFETY: as in `save`, a read of the V part; `vtype` and `vcsr` get only values a
        // program could set itself, and the kernel holds nothing in the V registers.
        unsafe { eggos_vector_restore(&raw const (*state).arch.vector) }
    }
}
