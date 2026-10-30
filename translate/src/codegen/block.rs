use alloc::vec::Vec;

use crate::decoder::{DecodeErrorKind, decode};
use crate::instruction::{Instruction, Mnemonic};

/// The most instructions one block holds; a longer straight run continues in the next block.
pub const MAX_BLOCK_INSTRUCTIONS: usize = 256;

/// How a block ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockEnd {
    /// Its last instruction transfers control (a branch, call, return, loop or trap).
    Terminator,
    /// It reached [`MAX_BLOCK_INSTRUCTIONS`] and continues at this address.
    FallThrough(u64),
    /// The bytes at `address` are not a translatable instruction; reaching them traps.
    Refused {
        /// Where.
        address: u64,
        /// Why.
        error: DecodeErrorKind,
    },
}

/// A straight run of decoded instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The guest address of the first instruction (or of the refused bytes when there is
    /// none).
    pub start: u64,
    /// The instructions.
    pub instructions: Vec<Instruction>,
    /// How it ends.
    pub end: BlockEnd,
}

pub(crate) fn ends_block(mnemonic: Mnemonic) -> bool {
    matches!(
        mnemonic,
        Mnemonic::Jmp
            | Mnemonic::Jcc(_)
            | Mnemonic::Call
            | Mnemonic::Ret
            | Mnemonic::Loop
            | Mnemonic::Loope
            | Mnemonic::Loopne
            | Mnemonic::Jrcxz
            | Mnemonic::Undefined
            | Mnemonic::Int3
            | Mnemonic::Int
    )
}

/// Decodes the block starting at `start` from `code`, the bytes mapped at `code_address`.
pub fn form_block(code: &[u8], code_address: u64, start: u64) -> Block {
    let mut instructions = Vec::new();
    let mut address = start;
    let end = loop {
        if instructions.len() == MAX_BLOCK_INSTRUCTIONS {
            break BlockEnd::FallThrough(address);
        }
        let offset = address.wrapping_sub(code_address);
        let bytes = usize::try_from(offset)
            .ok()
            .and_then(|offset| code.get(offset..))
            .unwrap_or(&[]);
        match decode(bytes, address) {
            Ok(instruction) => {
                address = instruction.end();
                let last = ends_block(instruction.mnemonic);
                instructions.push(instruction);
                if last {
                    break BlockEnd::Terminator;
                }
            }
            Err(error) => {
                break BlockEnd::Refused {
                    address,
                    error: error.kind,
                };
            }
        }
    };
    Block {
        start,
        instructions,
        end,
    }
}
