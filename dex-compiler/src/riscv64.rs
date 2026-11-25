//! The riscv64 back end (D760). A compiled method is called with the address of its argument
//! words in `a0` and the runtime's entry table in `a1`, and returns its result in `a0`, as on
//! the other machines (D471). Its prologue saves `ra`, `s0` and `s1` to `s11`, keeps the table
//! in `s1`, makes its frame (the outgoing words at `sp`, then one 64-bit slot per virtual
//! register) and copies the arguments into the last registers. The ten most used Dalvik
//! registers live in `s2` to `s11` for the whole method, the others in their slots; values pass
//! through `t0` to `t2`, floats through `f0` and `f1`. An int is kept sign-extended where it is
//! compared and returned zero-extended, as the runtime reads results. Conditional branches
//! jump over a `jal`, so a branch reaches anywhere in the method; offsets past 12 bits are
//! formed in `t4`.

use crate::lower::{
    self, Condition, FloatOperation, Lowered, Narrow, Op, Operand, Operation, Width,
};
use crate::{CompileError, Compiled, helper};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use eggos_android_package::dex::{Catches, Code};

const ZERO: u8 = 0;
const LINK: u8 = 1;
const STACK: u8 = 2;
const FIRST: u8 = 5;
const SECOND: u8 = 6;
const THIRD: u8 = 7;
const TABLE: u8 = 9;
const RESULT: u8 = 10;
const ARGUMENTS: u8 = 10;
const TABLE_ARGUMENT: u8 = 11;
const CALL_TARGET: u8 = 28;
const ADDRESS: u8 = 29;
const ALLOCATABLE: [u8; 10] = [18, 19, 20, 21, 22, 23, 24, 25, 26, 27];
const SAVED: [u8; 13] = [1, 8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27];
const SLOT_BYTES: i64 = 8;
const HELPER_WORDS: i64 = 3;
const RETURN: u32 = 0x0000_8067;

const OPERATION: u32 = 0x33;
const OPERATION_WORD: u32 = 0x3b;
const IMMEDIATE: u32 = 0x13;
const IMMEDIATE_WORD: u32 = 0x1b;
const LOAD: u32 = 0x03;
const STORE: u32 = 0x23;
const FLOAT: u32 = 0x53;
const BRANCH: u32 = 0x63;
const JUMP: u32 = 0x6f;
const JUMP_REGISTER: u32 = 0x67;
const UPPER: u32 = 0x37;
const DYNAMIC_ROUNDING: u32 = 7;

const BRANCH_EQUAL: u32 = 0;
const BRANCH_NOT_EQUAL: u32 = 1;
const BRANCH_LESS: u32 = 4;
const BRANCH_GREATER_OR_EQUAL: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Machine(u8),
    Slot(i64),
}

fn register_type(funct7: u32, second: u8, first: u8, funct3: u32, to: u8, opcode: u32) -> u32 {
    funct7 << 25
        | u32::from(second) << 20
        | u32::from(first) << 15
        | funct3 << 12
        | u32::from(to) << 7
        | opcode
}

fn immediate_type(immediate: i64, first: u8, funct3: u32, to: u8, opcode: u32) -> u32 {
    ((immediate as u32) & 0xfff) << 20
        | u32::from(first) << 15
        | funct3 << 12
        | u32::from(to) << 7
        | opcode
}

fn store_type(immediate: i64, value: u8, base: u8, funct3: u32) -> u32 {
    let immediate = immediate as u32;
    (immediate >> 5 & 0x7f) << 25
        | u32::from(value) << 20
        | u32::from(base) << 15
        | funct3 << 12
        | (immediate & 0x1f) << 7
        | STORE
}

fn branch_type(offset: i64, second: u8, first: u8, funct3: u32) -> u32 {
    let offset = offset as u32;
    (offset >> 12 & 1) << 31
        | (offset >> 5 & 0x3f) << 25
        | u32::from(second) << 20
        | u32::from(first) << 15
        | funct3 << 12
        | (offset >> 1 & 0xf) << 8
        | (offset >> 11 & 1) << 7
        | BRANCH
}

fn jump_field(offset: i64) -> u32 {
    let offset = offset as u32;
    (offset >> 20 & 1) << 31 | (offset >> 1 & 0x3ff) << 21 | (offset >> 11 & 1) << 20
        | (offset >> 12 & 0xff) << 12
}

fn fits(offset: i64) -> bool {
    (-2048..2048).contains(&offset)
}

struct Emitter {
    words: Vec<u32>,
    homes: Vec<Home>,
    frame: i64,
    save_at: i64,
    fixups: Vec<(usize, usize)>,
    stub_fixups: Vec<(usize, Option<u16>)>,
}

impl Emitter {
    fn put(&mut self, word: u32) {
        self.words.push(word);
    }

    fn add_immediate(&mut self, to: u8, from: u8, value: i64) {
        self.put(immediate_type(value, from, 0, to, IMMEDIATE));
    }

    fn move_register(&mut self, to: u8, from: u8) {
        if to != from {
            self.add_immediate(to, from, 0);
        }
    }

    fn sign_extend_word(&mut self, to: u8, from: u8) {
        self.put(immediate_type(0, from, 0, to, IMMEDIATE_WORD));
    }

    fn shift_immediate(&mut self, funct3: u32, arithmetic: bool, to: u8, from: u8, by: i64) {
        let field = if arithmetic { 0x400 | by } else { by };
        self.put(immediate_type(field, from, funct3, to, IMMEDIATE));
    }

    fn constant(&mut self, to: u8, value: i64) {
        if fits(value) {
            self.add_immediate(to, ZERO, value);
            return;
        }
        let low = (value << 52) >> 52;
        if value == i64::from(value as i32) {
            let high = ((value - low) >> 12) as u32 & 0xf_ffff;
            self.put(high << 12 | u32::from(to) << 7 | UPPER);
            if low != 0 {
                self.put(immediate_type(low, to, 0, to, IMMEDIATE_WORD));
            }
            return;
        }
        self.constant(to, (value - low) >> 12);
        self.shift_immediate(1, false, to, to, 12);
        if low != 0 {
            self.add_immediate(to, to, low);
        }
    }

    fn address(&mut self, base: u8, offset: i64) -> (u8, i64) {
        if fits(offset) {
            return (base, offset);
        }
        self.constant(ADDRESS, offset);
        self.put(register_type(0, ADDRESS, base, 0, ADDRESS, OPERATION));
        (ADDRESS, 0)
    }

    fn load(&mut self, wide: bool, to: u8, base: u8, offset: i64) {
        let (base, offset) = self.address(base, offset);
        let funct3 = if wide { 3 } else { 2 };
        self.put(immediate_type(offset, base, funct3, to, LOAD));
    }

    fn store(&mut self, wide: bool, value: u8, base: u8, offset: i64) {
        let (base, offset) = self.address(base, offset);
        let funct3 = if wide { 3 } else { 2 };
        self.put(store_type(offset, value, base, funct3));
    }

    fn adjust_stack(&mut self, by: i64) {
        if fits(by) {
            self.add_immediate(STACK, STACK, by);
        } else {
            self.constant(ADDRESS, by);
            self.put(register_type(0, ADDRESS, STACK, 0, STACK, OPERATION));
        }
    }

    fn read(&mut self, width: Width, register: u32, scratch: u8) -> u8 {
        match self.homes[register as usize] {
            Home::Machine(machine) => machine,
            Home::Slot(offset) => {
                self.load(width == Width::Long, scratch, STACK, offset);
                scratch
            }
        }
    }

    fn read_compared(&mut self, register: u32, scratch: u8) -> u8 {
        let value = self.read(Width::Int, register, scratch);
        self.sign_extend_word(scratch, value);
        scratch
    }

    fn operand(&mut self, width: Width, operand: Operand, scratch: u8) -> u8 {
        match operand {
            Operand::Register(register) => self.read(width, register, scratch),
            Operand::Constant(value) => {
                let value = match width {
                    Width::Int => i64::from(value as i32),
                    Width::Long => value,
                };
                self.constant(scratch, value);
                scratch
            }
        }
    }

    fn destination(&self, register: u32, scratch: u8) -> u8 {
        match self.homes[register as usize] {
            Home::Machine(machine) => machine,
            Home::Slot(_) => scratch,
        }
    }

    fn write(&mut self, width: Width, register: u32, value: u8) {
        match self.homes[register as usize] {
            Home::Machine(machine) => self.move_register(machine, value),
            Home::Slot(offset) => self.store(width == Width::Long, value, STACK, offset),
        }
    }

    fn finish(&mut self, width: Width, register: u32, value: u8) {
        if let Home::Slot(offset) = self.homes[register as usize] {
            self.store(width == Width::Long, value, STACK, offset);
        }
    }

    fn call(&mut self, entry: usize, arguments: &[Operand]) {
        for (word, argument) in arguments.iter().enumerate() {
            let value = self.operand(Width::Long, *argument, FIRST);
            self.store(true, value, STACK, word as i64 * SLOT_BYTES);
        }
        self.load(true, CALL_TARGET, TABLE, entry as i64 * SLOT_BYTES);
        self.move_register(ARGUMENTS, STACK);
        self.move_register(TABLE_ARGUMENT, TABLE);
        self.put(immediate_type(0, CALL_TARGET, 0, LINK, JUMP_REGISTER));
    }

    fn epilogue(&mut self) {
        for (index, &saved) in SAVED.iter().enumerate() {
            self.load(true, saved, STACK, self.save_at + index as i64 * SLOT_BYTES);
        }
        self.adjust_stack(self.frame);
        self.put(RETURN);
    }

    fn jump_to(&mut self, target: usize) {
        self.fixups.push((self.words.len(), target));
        self.put(JUMP);
    }

    fn branch_over_jump(&mut self, funct3: u32, first: u8, second: u8, target: usize) {
        self.put(branch_type(8, second, first, funct3));
        self.jump_to(target);
    }

    fn float_in(&mut self, width: Width, float: u8, from: u8) {
        let funct7 = if width == Width::Long { 0x79 } else { 0x78 };
        self.put(register_type(funct7, 0, from, 0, float, FLOAT));
    }

    fn float_out(&mut self, width: Width, to: u8, float: u8) {
        let funct7 = if width == Width::Long { 0x71 } else { 0x70 };
        self.put(register_type(funct7, 0, float, 0, to, FLOAT));
    }
}

fn double(width: Width) -> u32 {
    u32::from(width == Width::Long)
}

fn emit(emitter: &mut Emitter, op: &Op) {
    match *op {
        Op::Move { width, to, from } => {
            let value = emitter.read(width, from, FIRST);
            emitter.write(width, to, value);
        }
        Op::Constant { width, to, value } => {
            let target = emitter.destination(to, FIRST);
            let value = match width {
                Width::Int => i64::from(value as i32),
                Width::Long => value,
            };
            emitter.constant(target, value);
            emitter.finish(width, to, target);
        }
        Op::Binary {
            width,
            operation,
            to,
            left,
            right,
        } => {
            let left = emitter.operand(width, left, FIRST);
            let right = emitter.operand(width, right, SECOND);
            let target = emitter.destination(to, THIRD);
            let word = width == Width::Int;
            let (funct7, funct3, uses_word) = match operation {
                Operation::Add => (0x00, 0, true),
                Operation::Subtract => (0x20, 0, true),
                Operation::Multiply => (0x01, 0, true),
                Operation::ShiftLeft => (0x00, 1, true),
                Operation::UnsignedShiftRight => (0x00, 5, true),
                Operation::ShiftRight => (0x20, 5, true),
                Operation::Xor => (0x00, 4, false),
                Operation::Or => (0x00, 6, false),
                Operation::And => (0x00, 7, false),
            };
            let opcode = if word && uses_word {
                OPERATION_WORD
            } else {
                OPERATION
            };
            emitter.put(register_type(funct7, right, left, funct3, target, opcode));
            emitter.finish(width, to, target);
        }
        Op::Negate { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let target = emitter.destination(to, SECOND);
            let opcode = if width == Width::Int {
                OPERATION_WORD
            } else {
                OPERATION
            };
            emitter.put(register_type(0x20, from, ZERO, 0, target, opcode));
            emitter.finish(width, to, target);
        }
        Op::Not { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.put(immediate_type(-1, from, 4, target, IMMEDIATE));
            emitter.finish(width, to, target);
        }
        Op::FlipSign { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let sign = match width {
                Width::Int => i64::from(i32::MIN),
                Width::Long => i64::MIN,
            };
            emitter.constant(SECOND, sign);
            let target = emitter.destination(to, THIRD);
            emitter.put(register_type(0, SECOND, from, 4, target, OPERATION));
            emitter.finish(width, to, target);
        }
        Op::IntToLong { to, from } | Op::LongToInt { to, from } => {
            let width = match op {
                Op::IntToLong { .. } => Width::Long,
                _ => Width::Int,
            };
            let from = emitter.read(Width::Long, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.sign_extend_word(target, from);
            emitter.finish(width, to, target);
        }
        Op::Narrow { narrow, to, from } => {
            let from = emitter.read(Width::Int, from, FIRST);
            let target = emitter.destination(to, SECOND);
            let (by, arithmetic) = match narrow {
                Narrow::Byte => (56, true),
                Narrow::Char => (48, false),
                Narrow::Short => (48, true),
            };
            emitter.shift_immediate(1, false, target, from, by);
            emitter.shift_immediate(5, arithmetic, target, target, by);
            emitter.finish(Width::Int, to, target);
        }
        Op::CompareLong { to, left, right } => {
            let left = emitter.read(Width::Long, left, FIRST);
            let right = emitter.read(Width::Long, right, SECOND);
            emitter.put(register_type(0, right, left, 2, THIRD, OPERATION));
            emitter.put(register_type(0, left, right, 2, FIRST, OPERATION));
            let target = emitter.destination(to, SECOND);
            emitter.put(register_type(0x20, THIRD, FIRST, 0, target, OPERATION));
            emitter.finish(Width::Int, to, target);
        }
        Op::Float {
            width,
            operation,
            to,
            left,
            right,
        } => {
            let left = emitter.read(width, left, FIRST);
            let right = emitter.read(width, right, SECOND);
            emitter.float_in(width, 0, left);
            emitter.float_in(width, 1, right);
            let base = match operation {
                FloatOperation::Add => 0x00,
                FloatOperation::Subtract => 0x04,
                FloatOperation::Multiply => 0x08,
                FloatOperation::Divide => 0x0c,
            };
            emitter.put(register_type(base | double(width), 1, 0, DYNAMIC_ROUNDING, 0, FLOAT));
            let target = emitter.destination(to, THIRD);
            emitter.float_out(width, target, 0);
            emitter.finish(width, to, target);
        }
        Op::CompareFloat {
            width,
            below_on_nan,
            to,
            left,
            right,
        } => {
            let left = emitter.read(width, left, FIRST);
            let right = emitter.read(width, right, SECOND);
            emitter.float_in(width, 0, left);
            emitter.float_in(width, 1, right);
            let compare = 0x50 | double(width);
            emitter.put(register_type(compare, 0, 1, 1, FIRST, FLOAT));
            emitter.put(register_type(compare, 1, 0, 1, SECOND, FLOAT));
            emitter.put(register_type(0x20, SECOND, FIRST, 0, FIRST, OPERATION));
            emitter.put(register_type(compare, 0, 0, 2, SECOND, FLOAT));
            emitter.put(register_type(compare, 1, 1, 2, THIRD, FLOAT));
            emitter.put(register_type(0, THIRD, SECOND, 7, SECOND, OPERATION));
            emitter.put(branch_type(8, ZERO, SECOND, BRANCH_NOT_EQUAL));
            emitter.add_immediate(FIRST, ZERO, if below_on_nan { -1 } else { 1 });
            emitter.write(Width::Int, to, FIRST);
        }
        Op::IntegerToFloat {
            from_width,
            to_width,
            to,
            from,
        } => {
            let from = emitter.read(from_width, from, FIRST);
            let funct7 = 0x68 | double(to_width);
            let source = if from_width == Width::Long { 2 } else { 0 };
            emitter.put(register_type(funct7, source, from, DYNAMIC_ROUNDING, 0, FLOAT));
            let target = emitter.destination(to, SECOND);
            emitter.float_out(to_width, target, 0);
            emitter.finish(to_width, to, target);
        }
        Op::FloatToFloat { to_width, to, from } => {
            let from_width = match to_width {
                Width::Int => Width::Long,
                Width::Long => Width::Int,
            };
            let from = emitter.read(from_width, from, FIRST);
            emitter.float_in(from_width, 0, from);
            let (funct7, source) = match to_width {
                Width::Long => (0x21, 0),
                Width::Int => (0x20, 1),
            };
            emitter.put(register_type(funct7, source, 0, DYNAMIC_ROUNDING, 0, FLOAT));
            let target = emitter.destination(to, SECOND);
            emitter.float_out(to_width, target, 0);
            emitter.finish(to_width, to, target);
        }
        Op::Call {
            entry,
            ref arguments,
            result,
        } => {
            emitter.call(entry, arguments);
            if let Some((width, to)) = result {
                if width == Width::Int {
                    emitter.sign_extend_word(RESULT, RESULT);
                }
                emitter.write(width, to, RESULT);
            }
        }
        Op::Return { value } => {
            match value {
                Some((width, from)) => {
                    let value = emitter.read(width, from, RESULT);
                    emitter.move_register(RESULT, value);
                    if width == Width::Int {
                        emitter.shift_immediate(1, false, RESULT, RESULT, 32);
                        emitter.shift_immediate(5, false, RESULT, RESULT, 32);
                    }
                }
                None => emitter.add_immediate(RESULT, ZERO, 0),
            }
            emitter.epilogue();
        }
        Op::Jump { target } => emitter.jump_to(target),
        Op::Branch {
            condition,
            left,
            right,
            target,
        } => {
            let left = emitter.read_compared(left, FIRST);
            let right = match right {
                Some(right) => emitter.read_compared(right, SECOND),
                None => ZERO,
            };
            let (funct3, first, second) = match condition {
                Condition::Equal => (BRANCH_NOT_EQUAL, left, right),
                Condition::NotEqual => (BRANCH_EQUAL, left, right),
                Condition::Less => (BRANCH_GREATER_OR_EQUAL, left, right),
                Condition::GreaterOrEqual => (BRANCH_LESS, left, right),
                Condition::Greater => (BRANCH_GREATER_OR_EQUAL, right, left),
                Condition::LessOrEqual => (BRANCH_LESS, right, left),
            };
            emitter.branch_over_jump(funct3, first, second, target);
        }
        Op::BranchOnKey {
            register,
            key,
            target,
        } => {
            let value = emitter.read_compared(register, FIRST);
            emitter.constant(SECOND, i64::from(key));
            emitter.branch_over_jump(BRANCH_NOT_EQUAL, value, SECOND, target);
        }
    }
}

fn handler_stub(emitter: &mut Emitter, catches: Option<&Catches>) {
    if let Some(catches) = catches {
        for &(kind, address) in &catches.typed {
            emitter.call(helper::CATCHES, &[Operand::Constant(i64::from(kind))]);
            emitter.branch_over_jump(BRANCH_EQUAL, RESULT, ZERO, address as usize);
        }
        if let Some(address) = catches.catch_all {
            emitter.jump_to(address as usize);
            return;
        }
    }
    emitter.add_immediate(RESULT, ZERO, 0);
    emitter.epilogue();
}

/// Compiles `code` for riscv64.
pub fn compile(code: &Code<'_>) -> Result<Compiled, CompileError> {
    let lowered: Vec<Lowered> = lower::lower(code)?;
    let registers = i64::from(code.registers);
    let out_words = (i64::from(code.outs) + 1).max(HELPER_WORDS);
    let slots_at = out_words * SLOT_BYTES;
    let save_at = slots_at + registers * SLOT_BYTES;
    let frame = (save_at + SAVED.len() as i64 * SLOT_BYTES).next_multiple_of(16);
    let mut homes: Vec<Home> = (0..registers)
        .map(|register| Home::Slot(slots_at + register * SLOT_BYTES))
        .collect();
    let counts = lower::uses(&lowered, code.registers);
    for (machine, register) in ALLOCATABLE
        .iter()
        .zip(lower::most_used(&counts, ALLOCATABLE.len()))
    {
        homes[register as usize] = Home::Machine(*machine);
    }
    let mut emitter = Emitter {
        words: Vec::new(),
        homes,
        frame,
        save_at,
        fixups: Vec::new(),
        stub_fixups: Vec::new(),
    };
    emitter.adjust_stack(-frame);
    for (index, &saved) in SAVED.iter().enumerate() {
        emitter.store(true, saved, STACK, save_at + index as i64 * SLOT_BYTES);
    }
    emitter.move_register(TABLE, TABLE_ARGUMENT);
    let first = code.registers.saturating_sub(code.ins);
    for index in 0..u32::from(code.ins.min(code.registers)) {
        emitter.load(true, FIRST, ARGUMENTS, i64::from(index) * SLOT_BYTES);
        emitter.write(Width::Long, u32::from(first) + index, FIRST);
    }
    let mut places = Vec::new();
    let mut handlers: BTreeMap<Option<u16>, Option<Catches>> = BTreeMap::new();
    for instruction in &lowered {
        places.push((instruction.at as u32, (emitter.words.len() * 4) as u32));
        for op in &instruction.ops {
            emit(&mut emitter, op);
        }
        if instruction.calls() {
            let key = instruction
                .catches
                .as_ref()
                .map(|catches| catches.handler_offset);
            handlers
                .entry(key)
                .or_insert_with(|| instruction.catches.clone());
            let pending = helper::PENDING_EXCEPTION as i64 * SLOT_BYTES;
            emitter.load(true, FIRST, TABLE, pending);
            emitter.put(branch_type(8, ZERO, FIRST, BRANCH_EQUAL));
            emitter.stub_fixups.push((emitter.words.len(), key));
            emitter.put(JUMP);
        }
    }
    let mut stubs = BTreeMap::new();
    for (key, catches) in &handlers {
        stubs.insert(*key, emitter.words.len());
        handler_stub(&mut emitter, catches.as_ref());
    }
    for &(word, key) in &emitter.stub_fixups.clone() {
        let distance = (stubs[&key] as i64 - word as i64) * 4;
        emitter.words[word] |= jump_field(distance);
    }
    for &(word, target) in &emitter.fixups.clone() {
        let index = places
            .binary_search_by_key(&(target as u32), |&(unit, _)| unit)
            .map_err(|_| CompileError::BadBranch { at: target })?;
        let distance = i64::from(places[index].1) - word as i64 * 4;
        emitter.words[word] |= jump_field(distance);
    }
    Ok(Compiled {
        code: emitter
            .words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect(),
        places,
    })
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
            try_blocks: &[],
        }
    }

    fn words(compiled: &Compiled) -> Vec<u32> {
        compiled
            .code
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect()
    }

    #[test]
    fn a_method_reserves_its_frame_keeps_the_table_and_returns() {
        let bytes = units(&[0x000e]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        assert_eq!(all[0] & 0x000f_ffff, immediate_type(0, STACK, 0, STACK, IMMEDIATE));
        assert!(all.contains(&immediate_type(0, TABLE_ARGUMENT, 0, TABLE, IMMEDIATE)));
        assert_eq!(*all.last().unwrap(), RETURN);
    }

    #[test]
    fn an_int_add_of_a_literal_is_an_addw_into_s2() {
        let bytes = units(&[0x2012, 0x00d8, 0x0300, 0x000f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        let add = compiled.place_of(1).unwrap() / 4;
        assert!(all[add..add + 3].iter().any(|&word| {
            word & 0x7f == OPERATION_WORD && (word >> 7 & 0x1f) == 18 && word >> 25 == 0
        }));
    }

    #[test]
    fn constants_of_every_size_are_built() {
        let mut emitter = Emitter {
            words: Vec::new(),
            homes: Vec::new(),
            frame: 0,
            save_at: 0,
            fixups: Vec::new(),
            stub_fixups: Vec::new(),
        };
        emitter.constant(FIRST, 5);
        assert_eq!(emitter.words.len(), 1);
        emitter.words.clear();
        emitter.constant(FIRST, 0x1234_5678);
        assert_eq!(emitter.words.len(), 2);
        emitter.words.clear();
        emitter.constant(FIRST, 0x1234_5678_9abc_def0);
        assert!(emitter.words.len() > 2);
    }

    #[test]
    fn a_backward_branch_jumps_to_the_loop_s_first_instruction() {
        let bytes = units(&[0x01d8, 0xff01, 0x0139, 0xfffe, 0x010f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        let start = compiled.place_of(2).unwrap() / 4;
        let end = compiled.place_of(4).unwrap() / 4;
        let jump = (start..end).find(|&index| all[index] & 0x7f == JUMP).unwrap();
        let word = all[jump];
        let offset = (((word >> 31) & 1) << 20
            | ((word >> 21) & 0x3ff) << 1
            | ((word >> 20) & 1) << 11
            | ((word >> 12) & 0xff) << 12) as i32;
        let offset = (offset << 11) >> 11;
        assert_eq!(
            jump as i32 * 4 + offset,
            compiled.place_of(0).unwrap() as i32
        );
    }
}
