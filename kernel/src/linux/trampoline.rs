//! The return trampoline (M6, D64): the code a signal handler returns to when it has no
//! `sa_restorer`, which calls `rt_sigreturn`. RISC-V's `struct sigaction` has no restorer and
//! glibc on AArch64 passes none, so Linux returns their handlers through its vDSO; EggOS has
//! no vDSO yet, so it maps a page of its own with just this code. x86_64 has none: a handler
//! without `SA_RESTORER` cannot return, and Linux sends `SIGSEGV` instead of running it.
//!
//! The page is the process's own, read-only and executable, mapped where `mmap` would put it
//! the first time a handler needs it: a process that never runs one pays nothing, not even at
//! exec. It holds a `nop` and then the call, and handlers return to the call: unwinders and
//! debuggers recognise these exact instructions as a signal trampoline, and some look one
//! byte before the return address, which the `nop` keeps inside it. The real vDSO, shared and
//! mapped at exec, will take it over.

use super::exec::MINIMUM_ADDRESS;
use crate::process::{Placement, Process};
use eggos_hal::Platform;
use eggos_hal::paging::Attributes;

/// The trampoline's code on the architecture: `None` where handlers need `SA_RESTORER`.
const CODE: Option<&[u8]> = if cfg!(target_arch = "aarch64") {
    Some(&[
        0x1f, 0x20, 0x03, 0xd5, // nop
        0x68, 0x11, 0x80, 0xd2, // mov x8, #139 (rt_sigreturn)
        0x01, 0x00, 0x00, 0xd4, // svc #0
    ])
} else if cfg!(target_arch = "riscv64") {
    Some(&[
        0x13, 0x00, 0x00, 0x00, // nop
        0x93, 0x08, 0xb0, 0x08, // li a7, 139 (rt_sigreturn)
        0x73, 0x00, 0x00, 0x00, // ecall
    ])
} else {
    None
};

/// Where handlers return to in the page: the call, after the `nop`.
const ENTRY: u64 = 4;

/// Where `process`'s handlers without a restorer return to, its trampoline mapped first if
/// it is not yet. `None` on x86_64, or if there was no room or no memory for the page (the
/// handler cannot run then, as for a frame that cannot be written).
pub fn address<P: Platform>(process: &Process<P>) -> Option<u64> {
    let code = CODE?;
    let mapped = process.signals().lock().trampoline();
    if mapped != 0 {
        return Some(mapped + ENTRY);
    }
    let placement = Placement::Anywhere {
        hint: None,
        floor: MINIMUM_ADDRESS,
        top: super::memory::mmap_top::<P>(),
    };
    let page = process.map_filled_page(placement, Attributes::USER_TEXT, code)?;
    process.signals().lock().set_trampoline(page);
    Some(page + ENTRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trampoline_calls_rt_sigreturn_after_a_nop() {
        let Some(code) = CODE else {
            return;
        };
        let words: alloc::vec::Vec<u32> = code
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&word| u32::from_le_bytes(word))
            .collect();
        if cfg!(target_arch = "aarch64") {
            // `movz x8, #139` and `svc #0`.
            assert_eq!(
                words,
                [0xd503_201f, 0xd280_0000 | 139 << 5 | 8, 0xd400_0001]
            );
        } else {
            // `addi a7, zero, 139` and `ecall`.
            assert_eq!(words, [0x13, 139 << 20 | 17 << 7 | 0x13, 0x73]);
        }
    }
}
