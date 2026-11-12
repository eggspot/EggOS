//! 32-bit Arm's FP and NEON registers of user programs (D482): `d0`-`d31` and `FPSCR`. The
//! kernel is soft-float and never touches them; `FPEXC.EN` turns them on for a program that
//! uses them, and off otherwise, so its first use traps as an undefined instruction (D54).

use crate::fp::FpState;

const ENABLED: u32 = 1 << 30;

/// Turns the registers on or off for user mode.
///
/// # Safety
/// The calling thread's registers must be loaded when turning them on.
pub(super) unsafe fn set_user(on: bool) {
    let value = if on { ENABLED } else { 0 };
    // SAFETY: writes `FPEXC`, which only gates the FP and NEON instructions.
    unsafe {
        core::arch::asm!(
            ".fpu vfpv3-d32",
            "vmsr fpexc, {}",
            in(reg) value,
        )
    };
}

/// Saves the registers into `state`.
///
/// # Safety
/// `state` must be writable and the registers on.
pub(super) unsafe fn save(state: *mut FpState) {
    // SAFETY: stores 32 doubles and `FPSCR` into the state the caller owns.
    unsafe {
        core::arch::asm!(
            ".fpu vfpv3-d32",
            "vmrs {enable}, fpexc",
            "orr {scratch}, {enable}, #0x40000000",
            "vmsr fpexc, {scratch}",
            "vstmia {at}!, {{d0-d15}}",
            "vstmia {at}!, {{d16-d31}}",
            "vmrs {scratch}, fpscr",
            "str {scratch}, [{at}]",
            "vmsr fpexc, {enable}",
            at = inout(reg) state => _,
            enable = out(reg) _,
            scratch = out(reg) _,
        )
    };
}

/// Loads the registers from `state`.
///
/// # Safety
/// `state` must be readable.
pub(super) unsafe fn restore(state: *const FpState) {
    // SAFETY: loads 32 doubles and `FPSCR` from the state the caller owns.
    unsafe {
        core::arch::asm!(
            ".fpu vfpv3-d32",
            "vmrs {enable}, fpexc",
            "orr {scratch}, {enable}, #0x40000000",
            "vmsr fpexc, {scratch}",
            "vldmia {at}!, {{d0-d15}}",
            "vldmia {at}!, {{d16-d31}}",
            "ldr {scratch}, [{at}]",
            "vmsr fpscr, {scratch}",
            "vmsr fpexc, {enable}",
            at = inout(reg) state => _,
            enable = out(reg) _,
            scratch = out(reg) _,
        )
    };
}
