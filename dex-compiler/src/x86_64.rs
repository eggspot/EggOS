//! The x86_64 back end (D471, D472). A compiled method is called with the address of its
//! argument words in `rdi` and the runtime's entry table in `rsi`, and returns its result in
//! `rax`. Its prologue keeps the table in `r12`, makes its frame on the stack (the outgoing
//! words first, then one 64-bit slot per virtual register) with `r13` at the first register,
//! and copies the arguments into the last registers. Values pass through `eax`/`rax` and
//! `ecx`/`rcx`, so no register allocation is needed; a 32-bit value is the slot's low half, a
//! wide or object value the whole slot. A call writes its argument words at the bottom of the
//! frame and calls through the table: a runtime helper's entry or a method's
//! ([`crate::helper`], [`crate::method_entry`]); `r12` and `r13` survive it.

use crate::{CompileError, Compiled, helper, method_entry};
use alloc::vec::Vec;
use eggos_android_package::dalvik::{self, Instruction};
use eggos_android_package::dex::Code;

const EAX: u8 = 0;
const ECX: u8 = 1;
const FRAME_REGISTER: u8 = 5;
const WIDE: u8 = 0x48;
const EXTENDED_BASE: u8 = 0x41;
const WIDE_EXTENDED_BASE: u8 = 0x49;
const SLOT_BYTES: usize = 8;
const HELPER_WORDS: usize = 3;
const PROLOGUE: [u8; 7] = [0x41, 0x54, 0x41, 0x55, 0x49, 0x89, 0xf4];
const EPILOGUE_POPS: [u8; 5] = [0x41, 0x5d, 0x41, 0x5c, 0xc3];

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
    frame_bytes: u32,
}

fn slot(register: u32) -> i32 {
    (register as usize * SLOT_BYTES) as i32
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

    fn frame_access(&mut self, width: Width, opcode: u8, register_field: u8, register: u32) {
        let prefix = match width {
            Width::Int => EXTENDED_BASE,
            Width::Long => WIDE_EXTENDED_BASE,
        };
        self.bytes(&[prefix, opcode, 0x80 | register_field << 3 | FRAME_REGISTER]);
        self.bytes(&slot(register).to_le_bytes());
    }

    fn load(&mut self, width: Width, into: u8, register: u32) {
        self.frame_access(width, 0x8b, into, register);
    }

    fn store(&mut self, width: Width, from: u8, register: u32) {
        self.frame_access(width, 0x89, from, register);
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

    fn prologue(&mut self, registers: u16, ins: u16, outs: u16) {
        let out_words = usize::from(outs).max(HELPER_WORDS);
        let bytes = (out_words + usize::from(registers)) * SLOT_BYTES;
        self.frame_bytes = (bytes.next_multiple_of(16) + 8) as u32;
        self.bytes(&PROLOGUE);
        self.bytes(&[WIDE, 0x81, 0xec]);
        self.bytes(&self.frame_bytes.to_le_bytes());
        self.bytes(&[0x4c, 0x8d, 0xac, 0x24]);
        self.bytes(&((out_words * SLOT_BYTES) as u32).to_le_bytes());
        let first = registers.saturating_sub(ins);
        for index in 0..u32::from(ins.min(registers)) {
            self.bytes(&[WIDE, 0x8b, 0x87]);
            self.bytes(&((index as usize * SLOT_BYTES) as u32).to_le_bytes());
            self.store(Width::Long, EAX, u32::from(first) + index);
        }
    }

    fn epilogue(&mut self) {
        self.bytes(&[WIDE, 0x81, 0xc4]);
        self.bytes(&self.frame_bytes.to_le_bytes());
        self.bytes(&EPILOGUE_POPS);
    }

    fn argument_from(&mut self, word: usize, register: u32) {
        self.load(Width::Long, EAX, register);
        self.bytes(&[WIDE, 0x89, 0x84, 0x24]);
        self.bytes(&((word * SLOT_BYTES) as u32).to_le_bytes());
    }

    fn argument_constant(&mut self, word: usize, value: i64) {
        self.bytes(&[WIDE, 0xc7, 0x84, 0x24]);
        self.bytes(&((word * SLOT_BYTES) as u32).to_le_bytes());
        self.bytes(&(value as i32).to_le_bytes());
    }

    fn call_entry(&mut self, entry: usize) {
        self.bytes(&[WIDE, 0x89, 0xe7, 0x4c, 0x89, 0xe6, 0x41, 0xff, 0x94, 0x24]);
        self.bytes(&((entry * SLOT_BYTES) as u32).to_le_bytes());
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
    match index {
        1 => Some(Operation::ReverseSubtract),
        index => int_operation(index),
    }
}

fn division_helper(width: Width, index: u8) -> Option<usize> {
    Some(match (width, index) {
        (Width::Int, 3) => helper::DIVIDE_INT,
        (Width::Int, 4) => helper::REMAINDER_INT,
        (Width::Long, 3) => helper::DIVIDE_LONG,
        (Width::Long, 4) => helper::REMAINDER_LONG,
        _ => return None,
    })
}

fn is_shift(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight
    )
}

fn field_width(opcode: u8) -> Width {
    match opcode {
        0x53 | 0x54 | 0x5a | 0x5b => Width::Long,
        _ => Width::Int,
    }
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
        frame_bytes: 0,
    };
    emitter.prologue(code.registers, code.ins, code.outs);
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

fn arithmetic(
    emitter: &mut Emitter,
    width: Width,
    operation: Operation,
    target: u32,
    left: u32,
    right: u32,
) {
    emitter.load(width, EAX, left);
    let right_width = if is_shift(operation) { Width::Int } else { width };
    emitter.load(right_width, ECX, right);
    emitter.operate(width, operation);
    emitter.store(width, EAX, target);
}

fn divide(
    emitter: &mut Emitter,
    width: Width,
    entry: usize,
    target: u32,
    left: u32,
    right: Result<u32, i64>,
) {
    emitter.argument_from(0, left);
    match right {
        Ok(register) => emitter.argument_from(1, register),
        Err(value) => emitter.argument_constant(1, value),
    }
    emitter.call_entry(entry);
    emitter.store(width, EAX, target);
}

fn emit(emitter: &mut Emitter, instruction: &Instruction, at: usize) -> Result<(), CompileError> {
    let opcode = instruction.opcode;
    let unsupported = CompileError::Unsupported { opcode, at };
    let bad_branch = CompileError::BadBranch { at };
    let (a, b, c) = (instruction.a, instruction.b, instruction.c);
    match opcode {
        0x00 => {}
        0x01..=0x03 => {
            emitter.load(Width::Int, EAX, b);
            emitter.store(Width::Int, EAX, a);
        }
        0x04..=0x09 => {
            emitter.load(Width::Long, EAX, b);
            emitter.store(Width::Long, EAX, a);
        }
        0x0a => emitter.store(Width::Int, EAX, a),
        0x0b | 0x0c => emitter.store(Width::Long, EAX, a),
        0x0e => {
            emitter.bytes(&[0x31, 0xc0]);
            emitter.epilogue();
        }
        0x0f => {
            emitter.load(Width::Int, EAX, a);
            emitter.epilogue();
        }
        0x10 | 0x11 => {
            emitter.load(Width::Long, EAX, a);
            emitter.epilogue();
        }
        0x12..=0x15 => {
            emitter.constant(Width::Int, EAX, instruction.literal);
            emitter.store(Width::Int, EAX, a);
        }
        0x16..=0x19 => {
            emitter.constant(Width::Long, EAX, instruction.literal);
            emitter.store(Width::Long, EAX, a);
        }
        0x1a | 0x1b | 0x22 => {
            let entry = match opcode {
                0x22 => helper::NEW_INSTANCE,
                _ => helper::STRING,
            };
            emitter.argument_constant(0, i64::from(instruction.index));
            emitter.call_entry(entry);
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
            let condition = CONDITIONS[usize::from(opcode - 0x32)];
            emitter.jump_to(&[0x0f, condition], at, target);
        }
        0x38..=0x3d => {
            let target = branch_target(at, instruction.literal).ok_or(bad_branch)?;
            emitter.load(Width::Int, EAX, a);
            emitter.bytes(&[0x85, 0xc0]);
            let condition = CONDITIONS[usize::from(opcode - 0x38)];
            emitter.jump_to(&[0x0f, condition], at, target);
        }
        0x52..=0x58 => {
            emitter.argument_from(0, b);
            emitter.argument_constant(1, i64::from(instruction.index));
            emitter.call_entry(helper::FIELD_GET);
            emitter.store(field_width(opcode), EAX, a);
        }
        0x59..=0x5f => {
            emitter.argument_from(0, b);
            emitter.argument_constant(1, i64::from(instruction.index));
            emitter.argument_from(2, a);
            emitter.call_entry(helper::FIELD_PUT);
        }
        0x6e..=0x72 => {
            let count = instruction.argument_count;
            for (word, &register) in instruction.arguments[..count].iter().enumerate() {
                emitter.argument_from(word, u32::from(register));
            }
            emitter.call_entry(method_entry(instruction.index));
        }
        0x74..=0x78 => {
            for word in 0..instruction.argument_count {
                emitter.argument_from(word, b + word as u32);
            }
            emitter.call_entry(method_entry(instruction.index));
        }
        0x7b..=0x7e => {
            let width = match opcode {
                0x7b | 0x7c => Width::Int,
                _ => Width::Long,
            };
            emitter.load(width, EAX, b);
            emitter.wide(width);
            match opcode {
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
        0x90..=0xa5 => {
            let (width, index) = match opcode {
                0x90..=0x9a => (Width::Int, opcode - 0x90),
                _ => (Width::Long, opcode - 0x9b),
            };
            match (int_operation(index), division_helper(width, index)) {
                (Some(operation), _) => arithmetic(emitter, width, operation, a, b, c),
                (None, Some(entry)) => divide(emitter, width, entry, a, b, Ok(c)),
                (None, None) => return Err(unsupported),
            }
        }
        0xb0..=0xc5 => {
            let (width, index) = match opcode {
                0xb0..=0xba => (Width::Int, opcode - 0xb0),
                _ => (Width::Long, opcode - 0xbb),
            };
            match (int_operation(index), division_helper(width, index)) {
                (Some(operation), _) => arithmetic(emitter, width, operation, a, a, b),
                (None, Some(entry)) => divide(emitter, width, entry, a, a, Ok(b)),
                (None, None) => return Err(unsupported),
            }
        }
        0xd0..=0xe2 => {
            let index = match opcode {
                0xd0..=0xd7 => opcode - 0xd0,
                _ => opcode - 0xd8,
            };
            if let Some(entry) = division_helper(Width::Int, index) {
                divide(emitter, Width::Int, entry, a, b, Err(instruction.literal));
                return Ok(());
            }
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
            outs: 1,
            tries: 0,
            instructions: bytes,
        }
    }

    fn contains(code: &[u8], wanted: &[u8]) -> bool {
        code.windows(wanted.len()).any(|window| window == wanted)
    }

    #[test]
    fn a_method_keeps_the_table_makes_its_frame_and_copies_its_argument_into_the_last_register() {
        let bytes = units(&[0x000e]);
        let compiled = compile(&code(&bytes)).unwrap();
        assert_eq!(compiled.code[..7], PROLOGUE);
        let frame = (6usize * 8).next_multiple_of(16) as u32 + 8;
        let reserve = [[WIDE, 0x81, 0xec].as_slice(), &frame.to_le_bytes()].concat();
        assert!(contains(&compiled.code, &reserve));
        assert!(contains(&compiled.code, &[0x4c, 0x8d, 0xac, 0x24, 24, 0, 0, 0]));
        let copy = [WIDE, 0x8b, 0x87, 0, 0, 0, 0, 0x49, 0x89, 0x85, 16, 0, 0, 0];
        assert!(contains(&compiled.code, &copy));
        assert!(compiled.code.ends_with(&EPILOGUE_POPS));
    }

    #[test]
    fn a_constant_plus_a_literal_returned_loads_adds_and_stores_on_the_frame() {
        let bytes = units(&[0x2012, 0x00d8, 0x0300, 0x000f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let add = compiled.place_of(1).unwrap();
        assert_eq!(
            compiled.code[add..add + 20],
            [
                0x41, 0x8b, 0x85, 0, 0, 0, 0, 0xb9, 3, 0, 0, 0, 0x01, 0xc8, 0x41, 0x89, 0x85, 0, 0,
                0
            ]
        );
    }

    #[test]
    fn a_loop_s_backward_branch_lands_on_its_first_instruction() {
        let bytes = units(&[0x01d8, 0xff01, 0x0139, 0xfffe, 0x010f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let branch = compiled.place_of(2).unwrap();
        let field = branch + 7 + 2 + 2;
        let relative = i32::from_le_bytes(compiled.code[field..field + 4].try_into().unwrap());
        assert_eq!(field as i32 + 4 + relative, compiled.place_of(0).unwrap() as i32);
    }

    #[test]
    fn an_invoke_writes_its_arguments_and_calls_the_method_s_entry() {
        let bytes = units(&[0x206e, 0x0004, 0x0021, 0x000a, 0x000f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let entry = ((helper::COUNT + 4) * 8) as u32;
        let call_through = [WIDE, 0x89, 0xe7, 0x4c, 0x89, 0xe6, 0x41, 0xff, 0x94, 0x24];
        let call = [&call_through[..], &entry.to_le_bytes()].concat();
        assert!(contains(&compiled.code, &call));
        assert!(contains(&compiled.code, &[WIDE, 0x89, 0x84, 0x24, 8, 0, 0, 0]));
        let result = compiled.place_of(3).unwrap();
        assert_eq!(compiled.code[result..result + 3], [0x41, 0x89, 0x85]);
    }

    #[test]
    fn division_strings_and_objects_go_through_the_runtime_s_helpers() {
        let bytes = units(&[0x0093, 0x0201, 0x001a, 0x0005, 0x0122, 0x0002, 0x000e]);
        let compiled = compile(&code(&bytes)).unwrap();
        for entry in [helper::DIVIDE_INT, helper::STRING, helper::NEW_INSTANCE] {
            let target = ((entry * 8) as u32).to_le_bytes();
            let call = [[0x41, 0xff, 0x94, 0x24].as_slice(), &target].concat();
            assert!(contains(&compiled.code, &call));
        }
    }

    #[test]
    fn what_needs_exceptions_or_arrays_is_refused_with_its_place() {
        let throw = units(&[0x000e, 0x0027]);
        assert_eq!(
            compile(&code(&throw)),
            Err(CompileError::Unsupported { opcode: 0x27, at: 1 })
        );
        let stray = units(&[0x0228, 0x000e]);
        assert_eq!(compile(&code(&stray)), Err(CompileError::BadBranch { at: 0 }));
    }
}
