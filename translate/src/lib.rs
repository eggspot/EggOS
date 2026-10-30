//! Ahead-of-time translation of x86-64 machine code for AArch64 (D17, D191): an x86_64 Windows
//! or macOS program on an Arm machine has its code translated once, when it is installed, into
//! native AArch64 code that is cached on disk, never interpreted. The design is
//! `docs/translation.md`.
//!
//! This first slice holds the x86-64 decoder ([`decode`]): legacy prefixes, REX, the one-byte,
//! `0F`, `0F 38` and `0F 3A` opcode maps, ModRM, SIB, displacements and immediates, turning the
//! bytes at one address into an [`Instruction`] with typed [`Operand`]s. It covers the subset the
//! translator handles (general integer instructions, flags, the stack, calls and returns, string
//! instructions, SSE and SSE2, and a small x87 subset); everything else is either [`Invalid`]
//! in 64-bit mode or [`Unsupported`] with its [`Refusal`] and length, so code discovery can
//! report the address and step over it. VEX, EVEX and XOP instructions are recognised and
//! refused.
//!
//! The code generator ([`codegen`]) forms blocks from decoded instructions and lowers them to
//! AArch64 code with the register map, flag handling and memory order of the design. The image
//! builder ([`image`]) finds a program's code, translates every block with flag liveness
//! across blocks, links the blocks and builds the image's tables and run-time routines; the
//! [`cache`] module reads and writes the `.tx` cache files and decides the cache directory's
//! names and evictions.
//!
//! [`Invalid`]: DecodeErrorKind::Invalid
//! [`Unsupported`]: DecodeErrorKind::Unsupported

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cache;
pub mod codegen;
mod decoder;
pub mod image;
pub mod instruction;

pub use decoder::{DecodeError, DecodeErrorKind, MAX_INSTRUCTION_LENGTH, Refusal, decode};
pub use instruction::{
    Address, Condition, Gpr, Index, Instruction, Memory, Mnemonic, Operand, Register, Repeat,
    Segment, Width,
};
