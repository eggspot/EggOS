//! Dalvik instructions decoded (D471), from the Android Open Source Project's published
//! "Dalvik bytecode" and "Dalvik executable instruction formats": each opcode's format names
//! how many 16-bit code units it takes and where its registers, literal, branch offset or pool
//! index sit. The switch and array-data payloads that live among the instructions are told by
//! their identifying `nop` and stepped over whole.

use crate::dex::Code;

/// An instruction format, named as the specification names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    F10x,
    F12x,
    F11n,
    F11x,
    F10t,
    F20t,
    F22x,
    F21t,
    F21s,
    F21h,
    F21c,
    F23x,
    F22b,
    F22t,
    F22s,
    F22c,
    F32x,
    F30t,
    F31t,
    F31i,
    F31c,
    F35c,
    F3rc,
    F45cc,
    F4rcc,
    F51l,
}

impl Format {
    /// How many code units an instruction of this format takes.
    pub const fn length(self) -> usize {
        match self {
            Self::F10x | Self::F12x | Self::F11n | Self::F11x | Self::F10t => 1,
            Self::F20t
            | Self::F22x
            | Self::F21t
            | Self::F21s
            | Self::F21h
            | Self::F21c
            | Self::F23x
            | Self::F22b
            | Self::F22t
            | Self::F22s
            | Self::F22c => 2,
            Self::F32x
            | Self::F30t
            | Self::F31t
            | Self::F31i
            | Self::F31c
            | Self::F35c
            | Self::F3rc => 3,
            Self::F45cc | Self::F4rcc => 4,
            Self::F51l => 5,
        }
    }
}

/// The format of `opcode`.
pub const fn format(opcode: u8) -> Format {
    use Format::*;
    match opcode {
        0x00 | 0x0e | 0x3e..=0x43 | 0x73 | 0x79 | 0x7a | 0xe3..=0xf9 => F10x,
        0x01 | 0x04 | 0x07 | 0x21 | 0x7b..=0x8f | 0xb0..=0xcf => F12x,
        0x02 | 0x05 | 0x08 => F22x,
        0x03 | 0x06 | 0x09 => F32x,
        0x0a..=0x11 | 0x1d | 0x1e | 0x27 => F11x,
        0x12 => F11n,
        0x13 | 0x16 => F21s,
        0x14 | 0x17 => F31i,
        0x15 | 0x19 => F21h,
        0x18 => F51l,
        0x1a | 0x1c | 0x1f | 0x22 | 0x60..=0x6d | 0xfe | 0xff => F21c,
        0x1b => F31c,
        0x20 | 0x23 | 0x52..=0x5f => F22c,
        0x24 | 0x6e..=0x72 | 0xfc => F35c,
        0x25 | 0x74..=0x78 | 0xfd => F3rc,
        0x26 | 0x2b | 0x2c => F31t,
        0x28 => F10t,
        0x29 => F20t,
        0x2a => F30t,
        0x2d..=0x31 | 0x44..=0x51 | 0x90..=0xaf => F23x,
        0x32..=0x37 => F22t,
        0x38..=0x3d => F21t,
        0xd0..=0xd7 => F22s,
        0xd8..=0xe2 => F22b,
        0xfa => F45cc,
        0xfb => F4rcc,
    }
}

/// One decoded instruction: registers `a`, `b` and `c` as its format places them, its literal
/// (a constant, or a branch offset in code units), its pool index, and an invoke's argument
/// registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Instruction {
    pub opcode: u8,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub literal: i64,
    pub index: u32,
    pub arguments: [u16; 5],
    pub argument_count: usize,
    /// Its length in code units.
    pub length: usize,
}

const PACKED_SWITCH_PAYLOAD: u16 = 0x0100;
const SPARSE_SWITCH_PAYLOAD: u16 = 0x0200;
const ARRAY_DATA_PAYLOAD: u16 = 0x0300;

fn nibble(value: u16, shift: u32) -> u32 {
    u32::from(value >> shift & 0xf)
}

fn signed_nibble(value: u16) -> i64 {
    i64::from(((value >> 12) as i8) << 4 >> 4)
}

/// The length in code units of the payload at `at`, when one starts there.
pub fn payload_length(code: &Code<'_>, at: usize) -> Option<usize> {
    let unit = code.unit(at)?;
    let size = usize::from(code.unit(at + 1)?);
    match unit {
        PACKED_SWITCH_PAYLOAD => Some(4 + size * 2),
        SPARSE_SWITCH_PAYLOAD => Some(2 + size * 4),
        ARRAY_DATA_PAYLOAD => {
            let width = size;
            let count = usize::from(code.unit(at + 2)?) | usize::from(code.unit(at + 3)?) << 16;
            Some(4 + (width * count).div_ceil(2))
        }
        _ => None,
    }
}

/// The instruction at code unit `at`; `None` past the end or for a truncated one.
pub fn decode(code: &Code<'_>, at: usize) -> Option<Instruction> {
    let first = code.unit(at)?;
    let opcode = (first & 0xff) as u8;
    let format = format(opcode);
    let unit = |offset: usize| code.unit(at + offset);
    let high = u32::from(first >> 8);
    let mut instruction = Instruction {
        opcode,
        length: format.length(),
        ..Instruction::default()
    };
    if at + instruction.length > code.length() {
        return None;
    }
    match format {
        Format::F10x => {}
        Format::F12x => {
            instruction.a = nibble(first, 8);
            instruction.b = nibble(first, 12);
        }
        Format::F11n => {
            instruction.a = nibble(first, 8);
            instruction.literal = signed_nibble(first);
        }
        Format::F11x => instruction.a = high,
        Format::F10t => instruction.literal = i64::from((first >> 8) as u8 as i8),
        Format::F20t => instruction.literal = i64::from(unit(1)? as i16),
        Format::F22x => {
            instruction.a = high;
            instruction.b = u32::from(unit(1)?);
        }
        Format::F21t | Format::F21s => {
            instruction.a = high;
            instruction.literal = i64::from(unit(1)? as i16);
        }
        Format::F21h => {
            instruction.a = high;
            let shift = if opcode == 0x19 { 48 } else { 16 };
            instruction.literal = i64::from(unit(1)? as i16) << shift;
        }
        Format::F21c => {
            instruction.a = high;
            instruction.index = u32::from(unit(1)?);
        }
        Format::F23x => {
            let second = unit(1)?;
            instruction.a = high;
            instruction.b = u32::from(second & 0xff);
            instruction.c = u32::from(second >> 8);
        }
        Format::F22b => {
            let second = unit(1)?;
            instruction.a = high;
            instruction.b = u32::from(second & 0xff);
            instruction.literal = i64::from((second >> 8) as u8 as i8);
        }
        Format::F22t | Format::F22s | Format::F22c => {
            instruction.a = nibble(first, 8);
            instruction.b = nibble(first, 12);
            let second = unit(1)?;
            match format {
                Format::F22c => instruction.index = u32::from(second),
                _ => instruction.literal = i64::from(second as i16),
            }
        }
        Format::F32x => {
            instruction.a = u32::from(unit(1)?);
            instruction.b = u32::from(unit(2)?);
        }
        Format::F30t => {
            let value = u32::from(unit(1)?) | u32::from(unit(2)?) << 16;
            instruction.literal = i64::from(value as i32);
        }
        Format::F31t | Format::F31i | Format::F31c => {
            instruction.a = high;
            let value = u32::from(unit(1)?) | u32::from(unit(2)?) << 16;
            match format {
                Format::F31c => instruction.index = value,
                _ => instruction.literal = i64::from(value as i32),
            }
        }
        Format::F35c | Format::F45cc => {
            let count = nibble(first, 12) as usize;
            let registers = unit(2)?;
            instruction.index = u32::from(unit(1)?);
            instruction.argument_count = count.min(5);
            instruction.arguments = [
                registers & 0xf,
                registers >> 4 & 0xf,
                registers >> 8 & 0xf,
                registers >> 12,
                first >> 8 & 0xf,
            ];
            if format == Format::F45cc {
                instruction.c = u32::from(unit(3)?);
            }
        }
        Format::F3rc | Format::F4rcc => {
            instruction.argument_count = high as usize;
            instruction.index = u32::from(unit(1)?);
            instruction.b = u32::from(unit(2)?);
            if format == Format::F4rcc {
                instruction.c = u32::from(unit(3)?);
            }
        }
        Format::F51l => {
            instruction.a = high;
            let mut value = 0u64;
            for index in (1..5).rev() {
                value = value << 16 | u64::from(unit(index)?);
            }
            instruction.literal = value as i64;
        }
    }
    Some(instruction)
}

/// Each instruction of `code` with the code unit it starts at, payloads stepped over; it stops
/// at the first one that does not decode.
pub fn instructions<'a>(code: &'a Code<'a>) -> impl Iterator<Item = (usize, Instruction)> + 'a {
    let mut at = 0;
    core::iter::from_fn(move || {
        while let Some(length) = payload_length(code, at) {
            at += length;
        }
        let instruction = decode(code, at)?;
        let start = at;
        at += instruction.length;
        Some((start, instruction))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
    }

    fn of(bytes: &[u8]) -> Code<'_> {
        Code {
            registers: 4,
            ins: 0,
            outs: 0,
            tries: 0,
            instructions: bytes,
        }
    }

    #[test]
    fn constants_carry_their_sign_and_width() {
        let bytes = code(&[0xf012, 0xff13, 0x8000, 0x0019, 0x4024]);
        let code = of(&bytes);
        let first = decode(&code, 0).unwrap();
        assert_eq!((first.a, first.literal, first.length), (0, -1, 1));
        let second = decode(&code, 1).unwrap();
        assert_eq!((second.a, second.literal), (0xff, -32768));
        let wide = decode(&code, 3).unwrap();
        assert_eq!(wide.literal, 0x4024_i64 << 48);
    }

    #[test]
    fn binary_operations_branches_and_invokes_name_their_registers() {
        let bytes = code(&[0x0290, 0x0100, 0x1032, 0xfffe, 0x206e, 0x0007, 0x0021, 0x000e]);
        let code = of(&bytes);
        let listed: Vec<(usize, Instruction)> = instructions(&code).collect();
        let (_, add) = listed[0];
        assert_eq!((add.opcode, add.a, add.b, add.c), (0x90, 2, 0, 1));
        let (at, branch) = listed[1];
        assert_eq!((at, branch.a, branch.b, branch.literal), (2, 0, 1, -2));
        let (_, invoke) = listed[2];
        assert_eq!((invoke.index, invoke.argument_count), (7, 2));
        assert_eq!(invoke.arguments[..2], [1, 2]);
        assert_eq!(listed[3].1.opcode, 0x0e);
    }

    #[test]
    fn a_packed_switch_payload_is_stepped_over() {
        let bytes = code(&[0x0100, 2, 0, 0, 3, 0, 5, 0, 0x000e]);
        let code = of(&bytes);
        let listed: Vec<(usize, Instruction)> = instructions(&code).collect();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, 8);
    }
}
