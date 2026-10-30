mod cursor;
mod form;
mod modrm;
mod one_byte;
mod operands;
mod prefix;
mod sse;
#[cfg(test)]
mod sse_tests;
#[cfg(test)]
mod tests;
mod two_byte;
mod vex;
mod x87;

use core::fmt;

use cursor::Cursor;
pub use cursor::MAX_INSTRUCTION_LENGTH;
use form::{Form, Immediate, Spec};
use prefix::Prefixes;

use crate::instruction::Instruction;

/// Why an instruction the decoder recognises is outside what the translator handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// A VEX-encoded instruction: AVX, AVX2, FMA, BMI2, ...
    Vex,
    /// An EVEX-encoded instruction: AVX-512 and AVX10.
    Evex,
    /// AMD's XOP.
    Xop,
    /// MMX or 3DNow!, on the `mm` registers.
    Mmx,
    /// SSE3 (`movddup`, `haddps`, `lddqu`, `fisttp`, ...).
    Sse3,
    /// The `0F 38` and `0F 3A` maps: SSSE3, SSE4, AES-NI, SHA, `movbe`, `crc32`, ...
    ThreeByteMap,
    /// An x87 instruction outside the translated subset.
    X87,
    /// The address-size prefix on an instruction that addresses memory.
    AddressSize,
    /// The operand-size prefix on a near branch, whose meaning differs between Intel and AMD.
    Branch16,
    /// `fxsave`, `xsave` and the other state-saving instructions.
    StateSave,
    /// A system, I/O, segment, far-transfer or transactional-memory instruction.
    System,
}

impl Refusal {
    /// What was refused, for the install report.
    pub const fn description(self) -> &'static str {
        match self {
            Refusal::Vex => "an AVX (VEX-encoded) instruction",
            Refusal::Evex => "an AVX-512 (EVEX-encoded) instruction",
            Refusal::Xop => "an AMD XOP instruction",
            Refusal::Mmx => "an MMX or 3DNow! instruction",
            Refusal::Sse3 => "an SSE3 instruction",
            Refusal::ThreeByteMap => "an SSSE3, SSE4 or later instruction (0F 38 or 0F 3A map)",
            Refusal::X87 => "an x87 instruction outside the translated subset",
            Refusal::AddressSize => "32-bit addressing (an address-size prefix)",
            Refusal::Branch16 => "a near branch with an operand-size prefix",
            Refusal::StateSave => "an FXSAVE or XSAVE state instruction",
            Refusal::System => "a system, I/O, segment or far-transfer instruction",
        }
    }
}

/// Why the bytes at an address are not a translatable instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecodeErrorKind {
    /// The bytes end before the instruction does.
    Truncated,
    /// The instruction would be longer than [`MAX_INSTRUCTION_LENGTH`] bytes.
    TooLong,
    /// Not an instruction in 64-bit mode: the CPU raises the invalid-opcode exception.
    Invalid,
    /// A valid instruction the translator does not handle.
    Unsupported {
        /// Why.
        refusal: Refusal,
        /// Its length in bytes, so code discovery can step over it.
        length: u8,
    },
}

/// The bytes at `address` could not be decoded into a translatable instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecodeError {
    /// Where the instruction starts.
    pub address: u64,
    /// Why it was not decoded.
    pub kind: DecodeErrorKind,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            DecodeErrorKind::Truncated => {
                write!(
                    formatter,
                    "{:#x}: the code ends inside an instruction",
                    self.address
                )
            }
            DecodeErrorKind::TooLong => write!(
                formatter,
                "{:#x}: an instruction longer than {MAX_INSTRUCTION_LENGTH} bytes",
                self.address
            ),
            DecodeErrorKind::Invalid => {
                write!(formatter, "{:#x}: not an x86-64 instruction", self.address)
            }
            DecodeErrorKind::Unsupported { refusal, .. } => write!(
                formatter,
                "{:#x}: {}, which EggOS does not translate",
                self.address,
                refusal.description()
            ),
        }
    }
}

/// Decodes the instruction at the start of `code`, which the program has at `address`.
pub fn decode(code: &[u8], address: u64) -> Result<Instruction, DecodeError> {
    decode_at(code, address).map_err(|kind| DecodeError { address, kind })
}

fn decode_at(code: &[u8], address: u64) -> Result<Instruction, DecodeErrorKind> {
    let mut cursor = Cursor::new(code);
    let prefixes = prefix::read(&mut cursor)?;
    let opcode = cursor.byte()?;
    let form = match opcode {
        0xC4 | 0xC5 | 0x62 => return Err(vex::refuse(&mut cursor, &prefixes, opcode)),
        0x8F if vex::is_xop(cursor.peek()?) => {
            return Err(vex::refuse(&mut cursor, &prefixes, opcode));
        }
        0x0F => return escape(&mut cursor, &prefixes, address),
        0xD8..=0xDF => x87::form(opcode, cursor.peek())?,
        _ => one_byte::form(opcode, cursor.peek(), &prefixes)?,
    };
    operands::build(&mut cursor, &prefixes, opcode, form, address)
}

fn escape(
    cursor: &mut Cursor<'_>,
    prefixes: &Prefixes,
    address: u64,
) -> Result<Instruction, DecodeErrorKind> {
    let opcode = cursor.byte()?;
    let form = match opcode {
        0x38 => {
            cursor.byte()?;
            Form::refused(Refusal::ThreeByteMap).modrm()
        }
        0x3A => {
            cursor.byte()?;
            Form::refused(Refusal::ThreeByteMap)
                .modrm()
                .with(Spec::Imm(Immediate::UnsignedByte))
        }
        _ => two_byte::form(opcode, cursor.peek(), prefixes)?,
    };
    operands::build(cursor, prefixes, opcode, form, address)
}
