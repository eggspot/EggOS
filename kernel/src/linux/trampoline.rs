//! The return trampoline (M6, D64): the code a signal handler returns to when it has no
//! `sa_restorer`, which calls `rt_sigreturn`. RISC-V's `struct sigaction` has no restorer and
//! glibc on AArch64 passes none, so Linux returns their handlers through its vDSO; EggOS has
//! no vDSO yet, so it maps a page of its own with just this code. x86_64 has none: a handler
//! without `SA_RESTORER` cannot return, and Linux sends `SIGSEGV` instead of running it.
//!
//! The page is the process's own, read-only and executable, mapped the first time a handler
//! needs it: a process that never runs one pays nothing, not even at exec. It goes at the
//! bottom of the guard gap kept below the stack, where `mmap` never places anything, so it
//! never lands in a hole the program made and may still touch (a range it unmapped, or one a
//! `fork` left out for `MADV_DONTFORK`), as Linux's vDSO, mapped at exec, never does. If that
//! page is taken (the program mapped something there itself, or a thread racing for its first
//! handler got there first), it takes the next free one of the gap's lowest
//! [`GAP_PAGES_TRIED`] pages, never one below the gap. Threads racing for it each map a page,
//! the first to record its page wins, and the others unmap theirs, so the address space keeps
//! one. It holds a `nop` and then the call, and handlers return to the call: unwinders and
//! debuggers recognise these exact instructions as a signal trampoline, and some look one
//! byte before the return address, which the `nop` keeps inside it. The real vDSO, shared and
//! mapped at exec, will take it over.

use super::exec::STACK_GUARD_GAP;
use crate::process::{Placement, Process};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;

/// The trampoline's code on the architecture: `None` where handlers need `SA_RESTORER`.
pub const CODE: Option<&[u8]> = crate::linux::abi::SIGRETURN_TRAMPOLINE;

/// Where handlers return to in the page: the call, after the `nop`.
pub const ENTRY: u64 = 4;

/// How many pages at the bottom of the stack's guard gap the trampoline may take, lowest
/// first: enough for threads racing for their first handler, and a small part of the gap, so
/// the rest of it stays unmapped above the page.
pub const GAP_PAGES_TRIED: u64 = 16;

const _: () = assert!(GAP_PAGES_TRIED * FRAME_SIZE * 8 <= STACK_GUARD_GAP);

/// Where `process`'s handlers without a restorer return to, its trampoline mapped first if
/// it is not yet. `None` on x86_64, or if there was no room or no memory for the page (the
/// handler cannot run then, as for a frame that cannot be written).
pub fn address<P: Platform>(process: &Process<P>) -> Option<u64> {
    let code = CODE?;
    let mapped = process.signals().lock().trampoline();
    if mapped != 0 {
        return Some(mapped + ENTRY);
    }
    let stack_gap_bottom = super::memory::mmap_top::<P>();
    let page = (0..GAP_PAGES_TRIED)
        .map(|index| stack_gap_bottom + index * FRAME_SIZE)
        .find_map(|at| {
            process.map_filled_page(Placement::Exactly(at), Attributes::USER_TEXT, code)
        })?;
    let recorded = {
        let mut signals = process.signals().lock();
        let recorded = signals.trampoline();
        if recorded == 0 {
            signals.set_trampoline(page);
        }
        recorded
    };
    if recorded == 0 {
        return Some(page + ENTRY);
    }
    let _ = process.unmap_range(page, page + FRAME_SIZE);
    Some(recorded + ENTRY)
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
        let aarch64_movz_x8_139_svc_0 = [0xd503_201f, 0xd280_0000 | 139 << 5 | 8, 0xd400_0001];
        let riscv_addi_a7_zero_139_ecall = [0x13, 139 << 20 | 17 << 7 | 0x13, 0x73];
        let expected = match crate::linux::abi::MACHINE {
            "aarch64" => aarch64_movz_x8_139_svc_0,
            _ => riscv_addi_a7_zero_139_ecall,
        };
        assert_eq!(words, expected);
    }
}
