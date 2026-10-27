//! Variadic functions. Darwin passes the variadic arguments of an arm64 call on the stack, one
//! 8-byte slot each, and its `va_list` is a pointer to the next slot; on x86_64 it follows the
//! System V convention, registers first, with `va_list` pointing to a register save area.
//! A short assembly entry for each variadic function turns its arguments into a `va_list` and
//! calls the Rust function that takes one, so `printf` and `vprintf` share one body.

use crate::format::Arguments;
use core::ffi::c_void;

/// A `va_list` as the program passes it to `vprintf` and its kin.
pub type RawVaList = *mut c_void;

/// Reads the arguments a [`RawVaList`] points to.
#[derive(Debug)]
pub struct VaList {
    #[cfg(target_arch = "aarch64")]
    next: *const u64,
    #[cfg(target_arch = "x86_64")]
    state: *mut SystemVList,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug)]
pub struct SystemVList {
    general_offset: u32,
    vector_offset: u32,
    overflow: *const u64,
    saved: *const u8,
}

#[cfg(target_arch = "x86_64")]
const GENERAL_SAVE_END: u32 = 48;
#[cfg(target_arch = "x86_64")]
const VECTOR_SAVE_END: u32 = 176;

impl VaList {
    /// # Safety
    /// `raw` must be a `va_list` the program (or a variadic entry) set up for the arguments
    /// that are read through it, and must stay valid while they are.
    pub unsafe fn new(raw: RawVaList) -> Self {
        #[cfg(target_arch = "aarch64")]
        return Self { next: raw.cast() };
        #[cfg(target_arch = "x86_64")]
        return Self { state: raw.cast() };
    }
}

impl Arguments for VaList {
    fn integer(&mut self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: `new`'s caller promised a slot for each argument read.
            let value = unsafe { self.next.read() };
            self.next = self.next.wrapping_add(1);
            value
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: `new`'s caller promised a System V `va_list` for the arguments read.
            let state = unsafe { &mut *self.state };
            if state.general_offset < GENERAL_SAVE_END {
                // SAFETY: the offset is inside the register save area.
                let value = unsafe {
                    state
                        .saved
                        .wrapping_add(state.general_offset as usize)
                        .cast::<u64>()
                        .read_unaligned()
                };
                state.general_offset += 8;
                value
            } else {
                // SAFETY: the argument is in the caller's overflow area.
                let value = unsafe { state.overflow.read_unaligned() };
                state.overflow = state.overflow.wrapping_add(1);
                value
            }
        }
    }

    fn double(&mut self) -> f64 {
        #[cfg(target_arch = "aarch64")]
        {
            f64::from_bits(self.integer())
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: as in `integer`.
            let state = unsafe { &mut *self.state };
            if state.vector_offset < VECTOR_SAVE_END {
                // SAFETY: the offset is inside the register save area.
                let value = unsafe {
                    state
                        .saved
                        .wrapping_add(state.vector_offset as usize)
                        .cast::<f64>()
                        .read_unaligned()
                };
                state.vector_offset += 16;
                value
            } else {
                // SAFETY: the argument is in the caller's overflow area.
                let value = unsafe { state.overflow.cast::<f64>().read_unaligned() };
                state.overflow = state.overflow.wrapping_add(1);
                value
            }
        }
    }
}

macro_rules! variadic {
    ($entry:ident => $target:path, $named:literal, $arm64:literal, $x86_64:literal) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            concat!("mov ", $arm64, ", sp"),
            "b {target}",
            target = sym $target,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub rsp, 216",
            "mov [rsp], rdi",
            "mov [rsp + 8], rsi",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], rcx",
            "mov [rsp + 32], r8",
            "mov [rsp + 40], r9",
            "movaps [rsp + 48], xmm0",
            "movaps [rsp + 64], xmm1",
            "movaps [rsp + 80], xmm2",
            "movaps [rsp + 96], xmm3",
            "movaps [rsp + 112], xmm4",
            "movaps [rsp + 128], xmm5",
            "movaps [rsp + 144], xmm6",
            "movaps [rsp + 160], xmm7",
            "mov dword ptr [rsp + 176], {general}",
            "mov dword ptr [rsp + 180], 48",
            "lea rax, [rsp + 224]",
            "mov [rsp + 184], rax",
            "mov [rsp + 192], rsp",
            concat!("lea ", $x86_64, ", [rsp + 176]"),
            "call {target}",
            "add rsp, 216",
            "ret",
            general = const $named * 8,
            target = sym $target,
        );
        unsafe extern "C" {
            pub fn $entry();
        }
    };
}

pub(crate) use variadic;
