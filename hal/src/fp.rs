//! User programs' FP/SIMD registers, the same way on every architecture (D54).
//!
//! The kernel uses no FP/SIMD register: every kernel target is soft-float. A program gets the
//! registers the first time it uses them. Until then each CPU leaves them off for user mode, so
//! its first FP/SIMD instruction traps as an illegal instruction; the kernel then gives its
//! thread an [`FpState`] with the values Linux starts a program with, turns them on with
//! [`crate::Platform::set_user_fp`] and runs the instruction again. From then on the
//! scheduler saves the registers when the thread leaves its CPU
//! ([`crate::Platform::save_fp`]) and loads them again when it comes back, unless they are
//! still there ([`crate::Platform::restore_fp`]). A program never runs with registers another
//! left, not even with them off: before a CPU runs a program that has none, it loads
//! [`FpState::new`]'s values over another program's and then turns them off, since some CPUs
//! read registers that are off speculatively (LazyFP, CVE-2018-3665).
//!
//! What a state holds, per architecture: `V0`-`V31`, `FPCR` and `FPSR` on AArch64 (no SVE or
//! SME yet); `f0`-`f31` and `fcsr` on RISC-V (no V yet); on x86_64 the x87, MMX and SSE
//! registers and `MXCSR`, in `FXSAVE`'s 512-byte image (no AVX yet: `CR4.OSXSAVE` stays
//! clear, so `CPUID` tells programs not to use it).

/// A thread's FP/SIMD registers while they are not loaded on a CPU. Aligned for every
/// architecture's save instructions (`FXSAVE` needs 16 bytes, and `XSAVE`, when AVX comes,
/// 64).
#[derive(Clone)]
#[repr(C, align(64))]
pub struct FpState {
    #[cfg(target_arch = "aarch64")]
    pub(crate) arch: Aarch64State,
    #[cfg(target_arch = "riscv64")]
    pub(crate) arch: Riscv64State,
    #[cfg(target_arch = "x86_64")]
    pub(crate) arch: X86_64State,
}

impl FpState {
    /// What a program starts with, as on Linux: every register zero, the rounding mode round
    /// to nearest and every floating-point exception masked (x87 control word `0x037F`, `MXCSR`
    /// `0x1F80`; `FPCR` and `fcsr` zero).
    pub const fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        let arch = {
            let mut image = [0u8; 512];
            let fcw = 0x037fu16.to_le_bytes();
            let mxcsr = 0x1f80u32.to_le_bytes();
            let mut byte = 0;
            while byte < fcw.len() {
                image[X86_64State::FCW + byte] = fcw[byte];
                byte += 1;
            }
            byte = 0;
            while byte < mxcsr.len() {
                image[X86_64State::MXCSR + byte] = mxcsr[byte];
                byte += 1;
            }
            X86_64State { image }
        };
        #[cfg(target_arch = "aarch64")]
        let arch = Aarch64State {
            v: [0; 32],
            fpcr: 0,
            fpsr: 0,
        };
        #[cfg(target_arch = "riscv64")]
        let arch = Riscv64State {
            f: [0; 32],
            fcsr: 0,
        };
        Self { arch }
    }
}

impl Default for FpState {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for FpState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The registers are the program's: nothing a kernel log needs.
        f.debug_struct("FpState").finish_non_exhaustive()
    }
}

/// AArch64: the 128-bit `V` registers, then `FPCR` and `FPSR`, as the save routine stores them.
#[cfg(target_arch = "aarch64")]
#[derive(Clone)]
#[repr(C)]
pub(crate) struct Aarch64State {
    pub(crate) v: [u128; 32],
    pub(crate) fpcr: u64,
    pub(crate) fpsr: u64,
}

/// RISC-V: `f0`-`f31` (D, 64 bits each), then `fcsr`, as the save routine stores them.
#[cfg(target_arch = "riscv64")]
#[derive(Clone)]
#[repr(C)]
pub(crate) struct Riscv64State {
    pub(crate) f: [u64; 32],
    pub(crate) fcsr: u64,
}

/// x86_64: `FXSAVE64`'s image (Intel SDM Vol. 1, 10.5.1).
#[cfg(target_arch = "x86_64")]
#[derive(Clone)]
#[repr(C)]
pub(crate) struct X86_64State {
    pub(crate) image: [u8; 512],
}

#[cfg(target_arch = "x86_64")]
impl X86_64State {
    /// Offset of the x87 control word in the image.
    const FCW: usize = 0;
    /// Offset of `MXCSR` in the image.
    const MXCSR: usize = 24;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_aligned_for_every_save_instruction() {
        assert_eq!(align_of::<FpState>(), 64);
        let state = FpState::new();
        assert_eq!(&raw const state as usize % 64, 0);
    }

    #[test]
    fn a_new_state_is_what_linux_starts_a_program_with() {
        let state = FpState::new();
        #[cfg(target_arch = "x86_64")]
        {
            let image = &state.arch.image;
            assert_eq!(u16::from_le_bytes([image[0], image[1]]), 0x037f);
            assert_eq!(
                u32::from_le_bytes(image[24..28].try_into().unwrap()),
                0x1f80
            );
            // Abridged tag word 0: every x87 register empty. Everything else zero.
            assert!(
                image
                    .iter()
                    .enumerate()
                    .all(|(offset, &byte)| { matches!(offset, 0 | 1 | 24 | 25) || byte == 0 })
            );
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert!(state.arch.v.iter().all(|&v| v == 0));
            assert_eq!((state.arch.fpcr, state.arch.fpsr), (0, 0));
        }
    }
}
