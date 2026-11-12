//! Dalvik methods compiled ahead of time to the machine's own code (M15, D471; D17: apps run
//! natively, never interpreted). A method's virtual registers live in a frame, one 64-bit slot
//! each, whose address the compiled code is called with; a wide value takes its pair's first
//! slot whole. Each instruction becomes a short run of machine instructions on that frame,
//! branches are patched once every instruction's place is known, and the place of each
//! instruction is kept, so a runtime can map a code address back to the DEX instruction.
//!
//! This first part compiles moves, constants, int and long arithmetic but division,
//! comparisons, conditional and unconditional branches and returns, for x86_64; a method using
//! anything else is refused with the opcode and place, and the layer reports it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod x86_64;

use alloc::vec::Vec;
use eggos_android_package::dex::Code;

/// Why a method was not compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileError {
    /// An instruction this compiler does not compile yet, at code unit `at`.
    Unsupported { opcode: u8, at: usize },
    /// A branch at code unit `at` to a place that is no instruction's start.
    BadBranch { at: usize },
    /// The code ends inside an instruction.
    Truncated,
}

/// A compiled method: its machine code, and the code offset of each DEX instruction (by the
/// code unit it starts at), in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compiled {
    pub code: Vec<u8>,
    pub places: Vec<(u32, u32)>,
}

impl Compiled {
    /// Where the instruction at code unit `at` starts in the machine code.
    pub fn place_of(&self, at: usize) -> Option<usize> {
        let at = u32::try_from(at).ok()?;
        let index = self.places.binary_search_by_key(&at, |&(unit, _)| unit).ok()?;
        Some(self.places[index].1 as usize)
    }
}

/// Compiles `code` for x86_64.
pub fn compile(code: &Code<'_>) -> Result<Compiled, CompileError> {
    x86_64::compile(code)
}
