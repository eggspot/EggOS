//! AArch64 code generation (slice 39b of D191): blocks of decoded x86-64 instructions become
//! AArch64 code with the register map, the flags in NZCV and the memory order of
//! `docs/translation.md`. A [`Block`] runs from an entry to its first branch, return, call or
//! refused instruction; [`translate_block`] lowers it into a [`TranslatedBlock`] whose exits
//! the image linker patches into branches to other blocks.

pub mod aarch64;
mod block;
pub mod flags;
mod lower;

use alloc::vec::Vec;

pub use block::{Block, BlockEnd, MAX_BLOCK_INSTRUCTIONS, form_block};
pub use flags::Flags;

use crate::instruction::Mnemonic;
use aarch64::emit::EmitError;

/// What the CPU that will run the code offers; it changes the code, so it is part of the
/// cache key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CodegenMode {
    /// The kernel runs translated threads in a hardware TSO mode, so plain loads and stores
    /// keep x86's memory order.
    pub hardware_tso: bool,
    /// FEAT_LSE: single-instruction atomics (`casal`, `ldaddal`, `swpal`, ...).
    pub lse: bool,
    /// FEAT_LRCPC: `ldapr` for guest loads instead of `ldar`.
    pub lrcpc: bool,
    /// FEAT_FlagM: `cfinv` to flip the carry's sense in one instruction.
    pub flagm: bool,
}

/// The host registers of the register map in `docs/translation.md`.
pub mod registers {
    use super::aarch64::Reg;
    use crate::instruction::Gpr;

    /// The host register holding a guest general-purpose register (`rax`-`r15` in `x0`-`x15`).
    pub const fn guest(gpr: Gpr) -> Reg {
        Reg(gpr.0 & 15)
    }

    /// The guest `rsp`.
    pub const GUEST_STACK: Reg = Reg(4);
    /// Scratch: effective addresses, and the guest target of an indirect exit.
    pub const ADDRESS: Reg = Reg(16);
    /// Scratch: constants and short-lived values.
    pub const SCRATCH: Reg = Reg(17);
    /// The `gs:` base.
    pub const GS_BASE: Reg = Reg(18);
    /// The current image's indirect-branch table.
    pub const ENTRIES: Reg = Reg(19);
    /// The return shadow stack: (guest, host) pairs in a 64 KiB ring aligned to 64 KiB.
    pub const SHADOW_STACK: Reg = Reg(20);
    /// The guest state block (see [`super::state`]).
    pub const STATE: Reg = Reg(21);
    /// The translator's temporaries, `x22` to `x28`.
    pub const TEMPORARIES: [Reg; 7] = [
        Reg(22),
        Reg(23),
        Reg(24),
        Reg(25),
        Reg(26),
        Reg(27),
        Reg(28),
    ];
    /// The vector register the translator uses as a temporary.
    pub const VECTOR_TEMPORARY: u8 = 31;
}

/// Byte offsets of the guest state block that `x21` points to.
pub mod state {
    /// The `fs:` base.
    pub const FS_BASE: u32 = 0;
    /// The `gs:` base, which `x18` is reloaded from after a native call.
    pub const GS_BASE: u32 = 8;
    /// The low byte of the last result that defined PF, for a later `jp`, `setp` or `cmovp`.
    pub const PARITY: u32 = 16;
    /// `first ^ second ^ result` of the last instruction that defined AF (AF is bit 4).
    pub const ADJUST: u32 = 17;
}

/// The `brk` immediates of translated code; the layers' exception handling maps each one to
/// the guest's exception, at the guest address the instruction starts give.
pub mod trap {
    /// `int3`: a breakpoint.
    pub const BREAKPOINT: u16 = 0xE000;
    /// `ud2` or bytes that are not an x86-64 instruction: invalid opcode.
    pub const INVALID_OPCODE: u16 = 0xE001;
    /// An instruction EggOS does not translate (the image's sites table says which).
    pub const REFUSED: u16 = 0xE002;
    /// `div`/`idiv` by zero or with a quotient too large: divide error.
    pub const DIVIDE_ERROR: u16 = 0xE003;
    /// `int 0x29`: Windows' fast fail, the code in `rcx`.
    pub const FAST_FAIL: u16 = 0xE004;
    /// Any other `int n`: a general-protection fault, as in a user-mode program.
    pub const GENERAL_PROTECTION: u16 = 0xE005;
    /// An exit the image linker has not patched yet.
    pub const UNLINKED: u16 = 0xE00F;
}

/// Where an exit goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExitKind {
    /// To the translated block of this guest address.
    Jump(u64),
    /// To the image's indirect-branch dispatch, with the guest target in `x16`; the dispatch
    /// must leave NZCV unchanged.
    Indirect,
}

/// One exit of a translated block: a `brk` placeholder the image linker replaces with a `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Exit {
    /// The word index of the placeholder.
    pub word: u32,
    /// Where it goes.
    pub kind: ExitKind,
}

/// The first word of one guest instruction's code, for mapping faults back to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstructionStart {
    /// The word index.
    pub word: u32,
    /// The guest address of the instruction.
    pub guest: u64,
}

/// A block's AArch64 code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslatedBlock {
    /// The guest address of its first instruction.
    pub guest: u64,
    /// The guest address after its last instruction.
    pub guest_end: u64,
    /// The instruction words, then the literal pool; place it at an 8-byte-aligned address.
    pub code: Vec<u32>,
    /// The exits, in code order.
    pub exits: Vec<Exit>,
    /// Where each guest instruction's code starts, in order.
    pub starts: Vec<InstructionStart>,
    /// The word index of every 64-bit literal holding a guest address inside the image
    /// (RIP-relative operands), which moves by the image's load offset.
    pub guest_address_literals: Vec<u32>,
}

/// Why a block could not be lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowerErrorKind {
    /// An instruction this code generator does not lower yet.
    Unhandled(Mnemonic),
    /// Operands the instruction's lowering does not accept (an x87 or XMM register in an
    /// integer instruction).
    Operands(Mnemonic),
    /// The code could not be finished.
    Emit(EmitError),
}

/// A block that could not be lowered, and at which instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerError {
    /// The guest address of the instruction.
    pub address: u64,
    /// Why.
    pub kind: LowerErrorKind,
}

/// Lowers `block` to AArch64 code for a CPU described by `mode`. `live_out` holds the flags a
/// successor may read; [`Flags::ALL`] is always correct, a smaller set (from liveness across
/// blocks) gives shorter code. At every exit NZCV holds CF, ZF, SF and OF directly (C is CF, not
/// its inverse), and PF and AF are in the state block when live.
pub fn translate_block(
    block: &Block,
    mode: CodegenMode,
    live_out: Flags,
) -> Result<TranslatedBlock, LowerError> {
    lower::lower(block, mode, live_out)
}
