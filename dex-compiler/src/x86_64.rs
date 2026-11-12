//! The x86_64 back end (D471): the frame's address arrives in `rdi` and every virtual
//! register is the 64-bit slot at `rdi + 8 * register`; values pass through `eax`/`rax` and
//! `ecx`/`rcx`, so no register allocation is needed and a method calls nothing it does not
//! name. A 32-bit value is read and written as the slot's low half, a wide or object value as
//! the whole slot. The result returns in `rax`.

use crate::{CompileError, Compiled};
use alloc::vec::Vec;
use eggos_android_package::dalvik::{self, Instruction};
use eggos_android_package::dex::Code;

const EAX: u8 = 0;
const ECX: u8 = 1;
const FRAME_BASE: u8 = 7;
const WIDE: u8 = 0x48;
const SLOT_BYTES: i64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Int,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Add,
    Subtract,
    ReverseSubtract,
    Multiply,
    And,
    Or,
    Xor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
}

struct Emitter {
    code: Vec<u8>,
    fixups: Vec<(usize, usize, usize)>,
}

impl Emitter {
    fn byte(&mut self, byte: u8) {
        self.code.push(byte);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    fn wide(&mut self, width: Width) {
        if width == Width::Long {
            self.byte(WIDE);
        }
    }

    fn slot(register: u32) -> i32 {
        (i64::from(register) * SLOT_BYTES) as i32
    }

    fn load(&mut self, width: Width, into: u8, register: u32) {
        self.wide(width);
        self.bytes(&[0x8b, 0x80 | into << 3 | FRAME_BASE]);
        self.bytes(&Self::slot(register).to_le_bytes());
    }

    fn store(&mut self, width: Width, from: u8, register: u32) {
        self.wide(width);
        self.bytes(&[0x89, 0x80 | from << 3 | FRAME_BASE]);
        self.bytes(&Self::slot(register).to_le_bytes());
    }

    fn constant(&mut self, width: Width, into: u8, value: i64) {
        match width {
            Width::Int => {
                self.byte(0xb8 + into);
                self.bytes(&(value as i32).to_le_bytes());
            }
            Width::Long => {
                self.bytes(&[WIDE, 0xb8 + into]);
                self.bytes(&value.to_le_bytes());
            }
        }
    }

    fn operate(&mut self, width: Width, operation: Operation) {
        self.wide(width);
        match operation {
            Operation::Add => self.bytes(&[0x01, 0xc8]),
            Operation::Subtract | Operation::ReverseSubtract => self.bytes(&[0x29, 0xc8]),
            Operation::Multiply => self.bytes(&[0x0f, 0xaf, 0xc1]),
            Operation::And => self.bytes(&[0x21, 0xc8]),
            Operation::Or => self.bytes(&[0x09, 0xc8]),
            Operation::Xor => self.bytes(&[0x31, 0xc8]),
            Operation::ShiftLeft => self.bytes(&[0xd3, 0xe0]),
            Operation::ShiftRight => self.bytes(&[0xd3, 0xf8]),
            Operation::UnsignedShiftRight => self.bytes(&[0xd3, 0xe8]),
        }
    }

    fn jump_to(&mut self, opcode: &[u8], at: usize, target: usize) {
        self.bytes(opcode);
        self.fixups.push((self.code.len(), at, target));
        self.bytes(&[0; 4]);
    }
}

fn int_operation(index: u8) -> Option<Operation> {
    Some(match index {
        0 => Operation::Add,
        1 => Operation::Subtract,
        2 => Operation::Multiply,
        5 => Operation::And,
        6 => Operation::Or,
        7 => Operation::Xor,
        8 => Operation::ShiftLeft,
        9 => Operation::ShiftRight,
        10 => Operation::UnsignedShiftRight,
        _ => return None,
    })
}

fn literal_operation(index: u8) -> Option<Operation> {
    Some(match index {
        0 => Operation::Add,
        1 => Operation::ReverseSubtract,
        2 => Operation::Multiply,
        5 => Operation::And,
        6 => Operation::Or,
        7 => Operation::Xor,
        8 => Operation::ShiftLeft,
        9 => Operation::ShiftRight,
        10 => Operation::UnsignedShiftRight,
        _ => return None,
    })
}

const CONDITIONS: [u8; 6] = [0x84, 0x85, 0x8c, 0x8d, 0x8f, 0x8e];

fn branch_target(at: usize, offset: i64) -> Option<usize> {
    usize::try_from(i64::try_from(at).ok()?.checked_add(offset)?).ok()
}

/// Compiles `code` for x86_64.
pub fn compile(code: &Code<'_>) -> Result<Compiled, CompileError> {
    let mut emitter = Emitter {
        code: Vec::new(),
        fixups: Vec::new(),
    };
    let mut places = Vec::new();
    let mut at = 0;
    while at < code.length() {
        if let Some(length) = dalvik::payload_length(code, at) {
            at += length;
            continue;
        }
        let instruction = dalvik::decode(code, at).ok_or(CompileError::Truncated)?;
        places.push((at as u32, emitter.code.len() as u32));
        emit(&mut emitter, &instruction, at)?;
        at += instruction.length;
    }
    for &(field, from, target) in &emitter.fixups {
        let index = places
            .binary_search_by_key(&(target as u32), |&(unit, _)| unit)
            .map_err(|_| CompileError::BadBranch { at: from })?;
        let destination = places[index].1 as i64;
        let relative = (destination - (field as i64 + 4)) as i32;
        emitter.code[field..field + 4].copy_from_slice(&relative.to_le_bytes());
    }
    Ok(Compiled {
        code: emitter.code,
        places,
    })
}

fn emit(emitter: &mut Emitter, instruction: &Instruction, at: usize) -> Result<(), CompileError> {
    let unsupported = CompileError::Unsupported {
        opcode: instruction.opcode,
        at,
    };
    let bad_branch = CompileError::BadBranch { at };
    let (a, b, c) = (instruction.a, instruction.b, instruction.c);
    match instruction.opcode {
        0x00 => {}
        0x01..=0x03 => {
            emitter.load(Width::Int, EAX, b);
            emitter.store(Width::Int, EAX, a);
        }
        0x04..=0x09 => {
            emitter.load(Width::Long, EAX, b);
            emitter.store(Width::Long, EAX, a);
        }
        0x0e => emitter.byte(0xc3),
        0x0f => {
            emitter.load(Width::Int, EAX, a);
            emitter.byte(0xc3);
        }
        0x10 | 0x11 => {
            emitter.load(Width::Long, EAX, a);
            emitter.byte(0xc3);
        }
        0x12..=0x15 => {
            emitter.constant(Width::Int, EAX, instruction.literal);
            emitter.store(Width::Int, EAX, a);
        }
        0x16..=0x19 => {
            emitter.constant(Width::Long, EAX, instruction.literal);
            emitter.store(Width::Long, EAX, a);
        }
        0x28..=0x2a => {
            let target = branch_target(at, instruction.literal).ok_or(bad_branch)?;
            emitter.jump_to(&[0xe9], at, target);
        }
        0x32..=0x37 => {
            let target = branch_target(at, instruction.literal).ok_or(bad_branch)?;
            emitter.load(Width::Int, EAX, a);
            emitter.load(Width::Int, ECX, b);
            emitter.bytes(&[0x39, 0xc8]);
            let condition = CONDITIONS[usize::from(instruction.opcode - 0x32)];
            emitter.jump_to(&[0x0f, condition], at, target);
        }
        0x38..=0x3d => {
            let target = branch_target(at, instruction.literal).ok_or(bad_branch)?;
            emitter.load(Width::Int, EAX, a);
            emitter.bytes(&[0x85, 0xc0]);
            let condition = CONDITIONS[usize::from(instruction.opcode - 0x38)];
            emitter.jump_to(&[0x0f, condition], at, target);
        }
        0x7b..=0x7e => {
            let width = match instruction.opcode {
                0x7b | 0x7c => Width::Int,
                _ => Width::Long,
            };
            emitter.load(width, EAX, b);
            emitter.wide(width);
            match instruction.opcode {
                0x7b | 0x7d => emitter.bytes(&[0xf7, 0xd8]),
                _ => emitter.bytes(&[0xf7, 0xd0]),
            }
            emitter.store(width, EAX, a);
        }
        0x81 => {
            emitter.load(Width::Int, EAX, b);
            emitter.bytes(&[WIDE, 0x63, 0xc0]);
            emitter.store(Width::Long, EAX, a);
        }
        0x84 => {
            emitter.load(Width::Long, EAX, b);
            emitter.store(Width::Int, EAX, a);
        }
        0x90..=0x9a | 0x9b..=0xa5 => {
            let (width, index) = match instruction.opcode {
                0x90..=0x9a => (Width::Int, instruction.opcode - 0x90),
                _ => (Width::Long, instruction.opcode - 0x9b),
            };
            let operation = int_operation(index).ok_or(unsupported)?;
            let shift = matches!(
                operation,
                Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight
            );
            emitter.load(width, EAX, b);
            emitter.load(if shift { Width::Int } else { width }, ECX, c);
            emitter.operate(width, operation);
            emitter.store(width, EAX, a);
        }
        0xb0..=0xba | 0xbb..=0xc5 => {
            let (width, index) = match instruction.opcode {
                0xb0..=0xba => (Width::Int, instruction.opcode - 0xb0),
                _ => (Width::Long, instruction.opcode - 0xbb),
            };
            let operation = int_operation(index).ok_or(unsupported)?;
            let shift = matches!(
                operation,
                Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight
            );
            emitter.load(width, EAX, a);
            emitter.load(if shift { Width::Int } else { width }, ECX, b);
            emitter.operate(width, operation);
            emitter.store(width, EAX, a);
        }
        0xd0..=0xd7 | 0xd8..=0xe2 => {
            let index = match instruction.opcode {
                0xd0..=0xd7 => instruction.opcode - 0xd0,
                _ => instruction.opcode - 0xd8,
            };
            let operation = literal_operation(index).ok_or(unsupported)?;
            match operation {
                Operation::ReverseSubtract => {
                    emitter.constant(Width::Int, EAX, instruction.literal);
                    emitter.load(Width::Int, ECX, b);
                }
                _ => {
                    emitter.load(Width::Int, EAX, b);
                    emitter.constant(Width::Int, ECX, instruction.literal);
                }
            }
            emitter.operate(Width::Int, operation);
            emitter.store(Width::Int, EAX, a);
        }
        _ => return Err(unsupported),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
    }

    fn code(bytes: &[u8]) -> Code<'_> {
        Code {
            registers: 3,
            ins: 1,
            outs: 0,
            tries: 0,
            instructions: bytes,
        }
    }

    #[test]
    fn a_constant_plus_a_literal_returned_compiles_to_loads_stores_and_an_add() {
        let bytes = units(&[0x2012, 0x00d8, 0x0300, 0x000f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let expected: Vec<u8> = [
            &[0xb8, 2, 0, 0, 0][..],
            &[0x89, 0x87, 0, 0, 0, 0],
            &[0x8b, 0x87, 0, 0, 0, 0],
            &[0xb9, 3, 0, 0, 0],
            &[0x01, 0xc8],
            &[0x89, 0x87, 0, 0, 0, 0],
            &[0x8b, 0x87, 0, 0, 0, 0],
            &[0xc3],
        ]
        .concat();
        assert_eq!(compiled.code, expected);
        assert_eq!(compiled.place_of(1), Some(11));
        assert_eq!(compiled.place_of(3), Some(30));
    }

    #[test]
    fn a_loop_s_backward_branch_lands_on_its_first_instruction() {
        let bytes = units(&[0x01d8, 0xff01, 0x0139, 0xfffe, 0x010f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let branch = compiled.place_of(2).unwrap();
        let field = branch + 6 + 2 + 2;
        let relative = i32::from_le_bytes(compiled.code[field..field + 4].try_into().unwrap());
        assert_eq!(field as i32 + 4 + relative, 0);
    }

    #[test]
    fn wide_values_use_the_whole_slot() {
        let bytes = units(&[0x0016, 0x0005, 0x029b, 0x0000, 0x0010]);
        let compiled = compile(&code(&bytes)).unwrap();
        assert_eq!(&compiled.code[..2], &[0x48, 0xb8]);
        assert!(compiled.code.windows(3).any(|window| window == [0x48, 0x01, 0xc8]));
        assert_eq!(compiled.code.last(), Some(&0xc3));
    }

    #[test]
    fn division_and_invokes_are_refused_with_their_place() {
        let division = units(&[0x0093, 0x0000, 0x000e]);
        assert_eq!(
            compile(&code(&division)),
            Err(CompileError::Unsupported { opcode: 0x93, at: 0 })
        );
        let invoke = units(&[0x000e, 0x106e, 0x0000, 0x0000]);
        assert_eq!(
            compile(&code(&invoke)),
            Err(CompileError::Unsupported { opcode: 0x6e, at: 1 })
        );
        let stray = units(&[0x0228, 0x000e]);
        assert_eq!(compile(&code(&stray)), Err(CompileError::BadBranch { at: 0 }));
    }
}
