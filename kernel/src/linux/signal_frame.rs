//! Linux's signal frames (M6, D64): what the kernel writes on a program's stack before one of
//! its signal handlers runs, and reads back when the handler returns with `rt_sigreturn`.
//!
//! A frame is Linux's `struct rt_sigframe` on the architecture: the signal's `siginfo_t` and a
//! `ucontext_t` holding the interrupted registers, blocked mask and alternate stack. Every
//! offset below is the architecture's Linux ABI (its `asm/sigcontext.h` and `asm/ucontext.h`),
//! so C libraries, unwinders and debuggers that read a frame find what they expect. The
//! handler starts with the signal number, the `siginfo_t` and the `ucontext_t` as its three
//! arguments, and returns to its restorer: the code at `sa_restorer` that calls
//! `rt_sigreturn` (x86_64 and AArch64), or the kernel's own return trampoline
//! ([`super::trampoline`]).
//!
//! Every frame has its FP/SIMD part in Linux's shape (D64): x86_64's `FXSAVE` image above the
//! frame, which `fpstate` points at, AArch64's `fpsimd_context` record and RISC-V's
//! D-extension state. It holds the thread's registers as the interrupted code left them, or
//! the architecture's initial state for a thread that has none (D82). `rt_sigreturn` checks
//! the records' shape and loads the registers from them, so a handler that changes them in the
//! frame changes them for the interrupted code, as on Linux.
//!
//! A fault's frame (D65) also reports the exception ([`Trap`]): x86_64's `err`, `trapno` and
//! `cr2` in the `sigcontext`, AArch64's `fault_address` and an `esr_context` record after the
//! FP/SIMD one (aborts and alignment faults only, D67). Other signals' frames hold zeros
//! there and no `esr_context`.

use super::fault::{Fault, Trap};
use super::signals::info_offset;
use crate::process::Process;
use eggos_hal::Platform;
use eggos_hal::fp::FpState;
use eggos_hal::user::UserRegisters;

use crate::linux::abi::signal_frame as arch;
pub use arch::FpPart;

/// The size of a `siginfo_t`.
pub const INFO_SIZE: usize = 128;

/// The size of a `stack_t`: the base, the flags (an `int`, padded to a word) and the size,
/// each a word of the program's width (D487).
pub const STACK_SIZE: usize = 3 * crate::linux::abi::WORD_BYTES;

/// The `siginfo_t` of `fault`: `si_signo`, `si_errno` 0, `si_code` and `si_addr`, the
/// sender's fields' place.
pub fn fault_info(fault: &Fault) -> [u8; INFO_SIZE] {
    let mut info = [0; INFO_SIZE];
    info[..4].copy_from_slice(&(fault.signal as i32).to_ne_bytes());
    info[info_offset::CODE..][..4].copy_from_slice(&fault.code.to_ne_bytes());
    crate::linux::abi::put_word(&mut info[info_offset::ADDRESS..], fault.address);
    crate::linux::abi::reorder_siginfo(&mut info);
    info
}

/// What a frame saves of the interrupted thread and `rt_sigreturn` restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    pub registers: UserRegisters,
    /// The signals it blocked.
    pub blocked: u64,
    /// Its alternate stack, a `stack_t`.
    pub alternate_stack: [u8; STACK_SIZE],
    /// Its FP/SIMD registers, as the frame holds them ([`fp_part`]).
    pub fp: FpPart,
}

/// The FP/SIMD part of a frame holding `state`'s registers.
pub fn fp_part(state: &FpState) -> FpPart {
    arch::fp_part(state)
}

/// The FP/SIMD part of a frame for a thread that has no FP/SIMD registers: the initial state,
/// what it would start with.
pub fn initial_fp_part() -> FpPart {
    arch::fp_part(&FpState::new())
}

/// Sets `state`'s registers to those `part` holds. [`BadFrame`], changing nothing, if a load
/// would fault on them (x86_64's reserved `MXCSR` bits), as Linux's `rt_sigreturn` refuses them.
pub fn load_fp_part(part: &FpPart, state: &mut FpState) -> Result<(), BadFrame> {
    arch::load_fp_part(part, state)
}

/// Whether a handler starts with the initial FP/SIMD state rather than the interrupted code's,
/// as on Linux: x86_64 only.
pub const HANDLER_STARTS_WITH_INITIAL_FP: bool = arch::HANDLER_STARTS_WITH_INITIAL_FP;

/// Why a frame could not be written or read: its memory is not the program's (or, for a
/// handler with no way to return, there is none). Linux then sends `SIGSEGV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadFrame;

/// A handler about to run: what to write, where, and how it starts.
#[derive(Debug, Clone, Copy)]
pub struct Handler {
    /// The handler's address.
    pub entry: u64,
    /// Where it returns to: its `sa_restorer`, or the kernel's trampoline.
    pub restorer: u64,
    pub signal: u32,
}

/// Writes the frame for `handler` below stack pointer `top` (already on the alternate stack if
/// the handler runs there) and sets `registers` to start the handler with it: on success the
/// program's next instruction is the handler's first. `saved` is what the frame keeps, `trap`
/// the exception it reports for a fault's signal. Its padding and unused fields are written as
/// zeros (but AArch64's `__reserved` after the terminating record, which Linux leaves as it
/// was too).
pub fn write<P: Platform>(
    process: &Process<P>,
    top: u64,
    saved: &Saved,
    info: &[u8; INFO_SIZE],
    trap: Option<&Trap>,
    handler: &Handler,
    registers: &mut UserRegisters,
) -> Result<(), BadFrame> {
    let (address, bytes) = arch::build(top, saved, info, trap, handler.restorer);
    process
        .write_user_memory(address, &bytes)
        .map_err(|_| BadFrame)?;
    arch::write_extra(process, top, address, saved)?;
    arch::enter(registers, address, handler);
    Ok(())
}

/// How far below the interrupted stack pointer a frame starts, unless it goes on the
/// alternate stack: the ABI's red zone (x86_64's 128 bytes), which the interrupted code may be
/// using.
pub const RED_ZONE: u64 = arch::RED_ZONE;

/// The lowest address of the frame [`write()`] puts below `top`: where the stack pointer must
/// stay above, on an alternate stack.
pub const fn lowest_address(top: u64) -> u64 {
    arch::place(top)
}

/// The most stack one frame takes below the top of an alternate stack, whatever that top's
/// alignment: the frame, its FP/SIMD part and the padding that aligns them.
pub const LARGEST_FRAME_SPACE: u64 = largest_frame_space();

const fn largest_frame_space() -> u64 {
    const ALIGNED_TOP: u64 = 1 << 20;
    const LARGEST_ALIGNMENT: u64 = 64;
    let mut largest = 0;
    let mut misalignment = 0;
    while misalignment < LARGEST_ALIGNMENT {
        let top = ALIGNED_TOP + misalignment;
        let space = top - lowest_address(top);
        if space > largest {
            largest = space;
        }
        misalignment += 1;
    }
    largest
}

/// `rt_sigreturn`'s side: reads the frame of the handler that returned, whose registers are
/// `registers` (its stack pointer where the frame is), and returns what it saved.
pub fn read<P: Platform>(
    process: &Process<P>,
    registers: &UserRegisters,
) -> Result<Saved, BadFrame> {
    let address = arch::returned_frame(registers).ok_or(BadFrame)?;
    let mut bytes = [0; arch::READ];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| BadFrame)?;
    let mut saved = arch::parse(&bytes, registers).ok_or(BadFrame)?;
    arch::read_extra(process, &bytes, &mut saved)?;
    Ok(saved)
}

/// The value in the system-call result register of `registers`, which the dispatcher writes
/// back after `rt_sigreturn` returns it.
pub fn result(registers: &UserRegisters) -> u64 {
    arch::result(registers)
}

/// The frame's layout, for tests.
#[cfg(test)]
pub use arch::layout;

/// Reads the `u64` at `offset`.
pub fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Writes `value` at `offset`.
pub fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

/// The `ucontext_t` fields every architecture puts first: `uc_flags` at 0 (only x86_64 sets
/// any), `uc_link` (null) and `uc_stack`, the first two a word each (D490).
pub mod ucontext {
    pub const STACK: usize = 2 * crate::linux::abi::WORD_BYTES;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_frame_space_holds_every_frame_whatever_the_top_s_alignment() {
        assert_eq!(LARGEST_FRAME_SPACE, arch::LARGEST_FRAME_SPACE_EXPECTED);
        for top in (1 << 20)..(1 << 20) + 256 {
            assert!(top - lowest_address(top) <= LARGEST_FRAME_SPACE);
        }
    }

    #[test]
    fn a_frame_s_fp_simd_part_loads_back_into_the_same_registers() {
        let part = fp_part(&arch::state_with_values());
        assert_ne!(part, initial_fp_part());
        let mut loaded = FpState::new();
        load_fp_part(&part, &mut loaded).unwrap();
        assert_eq!(fp_part(&loaded), part);
    }
}
