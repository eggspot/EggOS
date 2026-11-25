//! The aarch64 back end (D760). A compiled method is called with the address of its argument
//! words in `x0` and the runtime's entry table in `x1`, and returns its result in `x0`, as the
//! x86_64 back end's methods do (D471). Its prologue saves `x19` to `x30`, keeps the table in
//! `x19`, makes its frame (the outgoing words at `sp`, then one 64-bit slot per virtual
//! register) and copies the arguments into the last registers. Registers are allocated once
//! per method: the nine most used Dalvik registers live in `x20` to `x28` for the whole method
//! (callee-saved, so they survive every call), the others in their frame slots; values pass
//! through the scratch registers `x9` to `x11`, floats through `s0`/`d0` and `s1`/`d1`. A call
//! writes its argument words at `sp` and branches to the table's entry with `x0` the words and
//! `x1` the table; after it the pending exception is looked at, as on x86_64 (D504).

use crate::lower::{
    self, Condition, FloatOperation, Lowered, Narrow, Op, Operand, Operation, Width,
};
use crate::{CompileError, Compiled, helper};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use eggos_android_package::dex::{Catches, Code};

const TABLE: u8 = 19;
const ALLOCATABLE: [u8; 9] = [20, 21, 22, 23, 24, 25, 26, 27, 28];
const SAVED: [u8; 12] = [19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30];
const FIRST: u8 = 9;
const SECOND: u8 = 10;
const THIRD: u8 = 11;
const CALL_TARGET: u8 = 16;
const RESULT: u8 = 0;
const ARGUMENTS: u8 = 0;
const TABLE_ARGUMENT: u8 = 1;
const ZERO: u8 = 31;
const STACK: u8 = 31;
const SLOT_BYTES: u32 = 8;
const HELPER_WORDS: u32 = 3;
const LARGEST_OFFSET: u32 = 32760;
const LARGEST_IMMEDIATE: u32 = 4095;

const EQUAL: u32 = 0x0;
const NOT_EQUAL: u32 = 0x1;
const MINUS: u32 = 0x4;
const OVERFLOW: u32 = 0x6;
const GREATER_OR_EQUAL: u32 = 0xa;
const LESS: u32 = 0xb;
const GREATER: u32 = 0xc;
const LESS_OR_EQUAL: u32 = 0xd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Machine(u8),
    Slot(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixup {
    Branch,
    Conditional,
}

struct Emitter {
    words: Vec<u32>,
    homes: Vec<Home>,
    frame: u32,
    save_at: u32,
    fixups: Vec<(usize, Fixup, usize)>,
    stub_fixups: Vec<(usize, Option<u16>)>,
}

fn wide(width: Width) -> bool {
    width == Width::Long
}

fn size_flag(wide: bool) -> u32 {
    if wide { 1 << 31 } else { 0 }
}

fn three(base: u32, wide: bool, to: u8, left: u8, right: u8) -> u32 {
    base | size_flag(wide) | u32::from(right) << 16 | u32::from(left) << 5 | u32::from(to)
}

fn condition_of(condition: Condition) -> u32 {
    match condition {
        Condition::Equal => EQUAL,
        Condition::NotEqual => NOT_EQUAL,
        Condition::Less => LESS,
        Condition::GreaterOrEqual => GREATER_OR_EQUAL,
        Condition::Greater => GREATER,
        Condition::LessOrEqual => LESS_OR_EQUAL,
    }
}

impl Emitter {
    fn put(&mut self, word: u32) {
        self.words.push(word);
    }

    fn move_register(&mut self, wide: bool, to: u8, from: u8) {
        if to != from {
            self.put(three(0x2a00_0000, wide, to, ZERO, from));
        }
    }

    fn compare(&mut self, wide: bool, left: u8, right: u8) {
        self.put(three(0x6b00_001f, wide, 0, left, right));
    }

    fn constant(&mut self, wide: bool, to: u8, value: i64) {
        let value = match wide {
            true => value as u64,
            false => u64::from(value as u32),
        };
        let chunks = if wide { 4 } else { 2 };
        let mut placed = false;
        for chunk in 0..chunks {
            let part = ((value >> (16 * chunk)) & 0xffff) as u32;
            if part == 0 {
                continue;
            }
            let base = if placed { 0x7280_0000 } else { 0x5280_0000 };
            self.put(base | size_flag(wide) | (chunk as u32) << 21 | part << 5 | u32::from(to));
            placed = true;
        }
        if !placed {
            self.put(0x5280_0000 | size_flag(wide) | u32::from(to));
        }
    }

    fn memory(&mut self, base: u32, wide: bool, register: u8, from: u8, offset: u32) {
        let scale = if wide { 8 } else { 4 };
        let field = offset / scale;
        let wide_bit = if wide { 1 << 30 } else { 0 };
        self.put(base | wide_bit | field << 10 | u32::from(from) << 5 | u32::from(register));
    }

    fn load(&mut self, wide: bool, register: u8, from: u8, offset: u32) {
        self.memory(0xb940_0000, wide, register, from, offset);
    }

    fn store(&mut self, wide: bool, register: u8, to: u8, offset: u32) {
        self.memory(0xb900_0000, wide, register, to, offset);
    }

    fn adjust_stack(&mut self, subtract: bool, bytes: u32) {
        let base = if subtract { 0xd100_0000 } else { 0x9100_0000 };
        let high = bytes >> 12;
        let low = bytes & 0xfff;
        if high > 0 {
            self.put(base | 1 << 22 | high << 10 | u32::from(STACK) << 5 | u32::from(STACK));
        }
        if low > 0 || high == 0 {
            self.put(base | low << 10 | u32::from(STACK) << 5 | u32::from(STACK));
        }
    }

    fn read(&mut self, width: Width, register: u32, scratch: u8) -> u8 {
        match self.homes[register as usize] {
            Home::Machine(machine) => machine,
            Home::Slot(offset) => {
                self.load(wide(width), scratch, STACK, offset);
                scratch
            }
        }
    }

    fn operand(&mut self, width: Width, operand: Operand, scratch: u8) -> u8 {
        match operand {
            Operand::Register(register) => self.read(width, register, scratch),
            Operand::Constant(value) => {
                self.constant(wide(width), scratch, value);
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
            Home::Machine(machine) => self.move_register(wide(width), machine, value),
            Home::Slot(offset) => self.store(wide(width), value, STACK, offset),
        }
    }

    fn finish(&mut self, width: Width, register: u32, value: u8) {
        if let Home::Slot(offset) = self.homes[register as usize] {
            self.store(wide(width), value, STACK, offset);
        }
    }

    fn call(&mut self, entry: usize, arguments: &[Operand]) {
        for (word, argument) in arguments.iter().enumerate() {
            let value = self.operand(Width::Long, *argument, FIRST);
            self.store(true, value, STACK, word as u32 * SLOT_BYTES);
        }
        let offset = entry as u32 * SLOT_BYTES;
        if offset <= LARGEST_OFFSET {
            self.load(true, CALL_TARGET, TABLE, offset);
        } else {
            self.constant(true, CALL_TARGET, i64::from(offset));
            self.put(three(0x0b00_0000, true, CALL_TARGET, TABLE, CALL_TARGET));
            self.load(true, CALL_TARGET, CALL_TARGET, 0);
        }
        self.put(0x9100_0000 | u32::from(STACK) << 5 | u32::from(ARGUMENTS));
        self.move_register(true, TABLE_ARGUMENT, TABLE);
        self.put(0xd63f_0000 | u32::from(CALL_TARGET) << 5);
    }

    fn epilogue(&mut self) {
        for (index, &saved) in SAVED.iter().enumerate() {
            self.load(true, saved, STACK, self.save_at + index as u32 * SLOT_BYTES);
        }
        self.adjust_stack(false, self.frame);
        self.put(0xd65f_03c0);
    }

    fn branch_to(&mut self, base: u32, target: usize) {
        self.fixups.push((self.words.len(), Fixup::Conditional, target));
        self.put(base);
    }

    fn jump_to(&mut self, target: usize) {
        self.fixups.push((self.words.len(), Fixup::Branch, target));
        self.put(0x1400_0000);
    }

    fn set_if(&mut self, condition: u32, to: u8) {
        self.put(0x1a9f_07e0 | (condition ^ 1) << 12 | u32::from(to));
    }

    fn float_in(&mut self, width: Width, float: u8, from: u8) {
        let base = if wide(width) { 0x9e67_0000 } else { 0x1e27_0000 };
        self.put(base | u32::from(from) << 5 | u32::from(float));
    }

    fn float_out(&mut self, width: Width, to: u8, float: u8) {
        let base = if wide(width) { 0x9e66_0000 } else { 0x1e26_0000 };
        self.put(base | u32::from(float) << 5 | u32::from(to));
    }
}

fn double_bit(width: Width) -> u32 {
    if wide(width) { 1 << 22 } else { 0 }
}

fn emit(emitter: &mut Emitter, op: &Op) {
    match *op {
        Op::Move { width, to, from } => {
            let value = emitter.read(width, from, FIRST);
            emitter.write(width, to, value);
        }
        Op::Constant { width, to, value } => {
            let target = emitter.destination(to, FIRST);
            emitter.constant(wide(width), target, value);
            emitter.finish(width, to, target);
        }
        Op::Binary {
            width,
            operation,
            to,
            left,
            right,
        } => {
            let right_width = match operation {
                Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight => {
                    Width::Int
                }
                _ => width,
            };
            let left = emitter.operand(width, left, FIRST);
            let right = emitter.operand(right_width, right, SECOND);
            let target = emitter.destination(to, THIRD);
            let base = match operation {
                Operation::Add => 0x0b00_0000,
                Operation::Subtract => 0x4b00_0000,
                Operation::Multiply => 0x1b00_7c00,
                Operation::And => 0x0a00_0000,
                Operation::Or => 0x2a00_0000,
                Operation::Xor => 0x4a00_0000,
                Operation::ShiftLeft => 0x1ac0_2000,
                Operation::UnsignedShiftRight => 0x1ac0_2400,
                Operation::ShiftRight => 0x1ac0_2800,
            };
            emitter.put(three(base, wide(width), target, left, right));
            emitter.finish(width, to, target);
        }
        Op::Negate { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.put(three(0x4b00_0000, wide(width), target, ZERO, from));
            emitter.finish(width, to, target);
        }
        Op::Not { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.put(three(0x2a20_0000, wide(width), target, ZERO, from));
            emitter.finish(width, to, target);
        }
        Op::FlipSign { width, to, from } => {
            let from = emitter.read(width, from, FIRST);
            let sign = match width {
                Width::Int => 1i64 << 31,
                Width::Long => i64::MIN,
            };
            emitter.constant(wide(width), SECOND, sign);
            let target = emitter.destination(to, THIRD);
            emitter.put(three(0x4a00_0000, wide(width), target, from, SECOND));
            emitter.finish(width, to, target);
        }
        Op::IntToLong { to, from } => {
            let from = emitter.read(Width::Int, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.put(0x9340_7c00 | u32::from(from) << 5 | u32::from(target));
            emitter.finish(Width::Long, to, target);
        }
        Op::LongToInt { to, from } => {
            let from = emitter.read(Width::Long, from, FIRST);
            let target = emitter.destination(to, SECOND);
            emitter.move_register(false, target, from);
            emitter.finish(Width::Int, to, target);
        }
        Op::Narrow { narrow, to, from } => {
            let from = emitter.read(Width::Int, from, FIRST);
            let target = emitter.destination(to, SECOND);
            let base = match narrow {
                Narrow::Byte => 0x1300_1c00,
                Narrow::Char => 0x5300_3c00,
                Narrow::Short => 0x1300_3c00,
            };
            emitter.put(base | u32::from(from) << 5 | u32::from(target));
            emitter.finish(Width::Int, to, target);
        }
        Op::CompareLong { to, left, right } => {
            let left = emitter.read(Width::Long, left, FIRST);
            let right = emitter.read(Width::Long, right, SECOND);
            emitter.compare(true, left, right);
            emitter.set_if(GREATER, FIRST);
            emitter.set_if(LESS, SECOND);
            let target = emitter.destination(to, THIRD);
            emitter.put(three(0x4b00_0000, false, target, FIRST, SECOND));
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
                FloatOperation::Add => 0x1e20_2800,
                FloatOperation::Subtract => 0x1e20_3800,
                FloatOperation::Multiply => 0x1e20_0800,
                FloatOperation::Divide => 0x1e20_1800,
            };
            emitter.put(base | double_bit(width) | 1 << 16);
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
            emitter.put(0x1e20_2000 | double_bit(width) | 1 << 16);
            emitter.set_if(GREATER, FIRST);
            emitter.set_if(MINUS, SECOND);
            emitter.put(three(0x4b00_0000, false, FIRST, FIRST, SECOND));
            let unordered = if below_on_nan { -1 } else { 1 };
            emitter.constant(false, SECOND, unordered);
            let target = emitter.destination(to, THIRD);
            emitter.put(
                0x1a80_0000 | u32::from(FIRST) << 16 | OVERFLOW << 12 | u32::from(SECOND) << 5
                    | u32::from(target),
            );
            emitter.finish(Width::Int, to, target);
        }
        Op::IntegerToFloat {
            from_width,
            to_width,
            to,
            from,
        } => {
            let from = emitter.read(from_width, from, FIRST);
            let base = match (from_width, to_width) {
                (Width::Int, Width::Int) => 0x1e22_0000,
                (Width::Int, Width::Long) => 0x1e62_0000,
                (Width::Long, Width::Int) => 0x9e22_0000,
                (Width::Long, Width::Long) => 0x9e62_0000,
            };
            emitter.put(base | u32::from(from) << 5);
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
            let convert = match to_width {
                Width::Long => 0x1e22_c000,
                Width::Int => 0x1e62_4000,
            };
            emitter.put(convert);
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
                emitter.write(width, to, RESULT);
            }
        }
        Op::Return { value } => {
            match value {
                Some((width, from)) => {
                    let value = emitter.read(width, from, RESULT);
                    emitter.move_register(wide(width), RESULT, value);
                }
                None => emitter.constant(true, RESULT, 0),
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
            let left = emitter.read(Width::Int, left, FIRST);
            let right = match right {
                Some(right) => emitter.read(Width::Int, right, SECOND),
                None => ZERO,
            };
            emitter.compare(false, left, right);
            emitter.branch_to(0x5400_0000 | condition_of(condition), target);
        }
        Op::BranchOnKey {
            register,
            key,
            target,
        } => {
            let value = emitter.read(Width::Int, register, FIRST);
            emitter.constant(false, SECOND, i64::from(key));
            emitter.compare(false, value, SECOND);
            emitter.branch_to(0x5400_0000 | EQUAL, target);
        }
    }
}

fn handler_stub(emitter: &mut Emitter, catches: Option<&Catches>) {
    if let Some(catches) = catches {
        for &(kind, address) in &catches.typed {
            emitter.call(helper::CATCHES, &[Operand::Constant(i64::from(kind))]);
            emitter.branch_to(0xb500_0000 | u32::from(RESULT), address as usize);
        }
        if let Some(address) = catches.catch_all {
            emitter.jump_to(address as usize);
            return;
        }
    }
    emitter.constant(true, RESULT, 0);
    emitter.epilogue();
}

fn too_large() -> CompileError {
    CompileError::Unsupported { opcode: 0, at: 0 }
}

/// Compiles `code` for aarch64.
pub fn compile(code: &Code<'_>) -> Result<Compiled, CompileError> {
    let lowered: Vec<Lowered> = lower::lower(code)?;
    let registers = u32::from(code.registers);
    let out_words = (u32::from(code.outs) + 1).max(HELPER_WORDS);
    let slots_at = out_words * SLOT_BYTES;
    let save_at = slots_at + registers * SLOT_BYTES;
    let frame = (save_at + SAVED.len() as u32 * SLOT_BYTES).next_multiple_of(16);
    if save_at + SAVED.len() as u32 * SLOT_BYTES > LARGEST_OFFSET || frame > LARGEST_IMMEDIATE << 12
    {
        return Err(too_large());
    }
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
    emitter.adjust_stack(true, frame);
    for (index, &saved) in SAVED.iter().enumerate() {
        emitter.store(true, saved, STACK, save_at + index as u32 * SLOT_BYTES);
    }
    emitter.move_register(true, TABLE, TABLE_ARGUMENT);
    let first = code.registers.saturating_sub(code.ins);
    for index in 0..u32::from(code.ins.min(code.registers)) {
        emitter.load(true, FIRST, ARGUMENTS, index * SLOT_BYTES);
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
            emitter.load(
                true,
                FIRST,
                TABLE,
                (helper::PENDING_EXCEPTION as u32) * SLOT_BYTES,
            );
            emitter
                .stub_fixups
                .push((emitter.words.len(), key));
            emitter.put(0xb500_0000 | u32::from(FIRST));
        }
    }
    let mut stubs = BTreeMap::new();
    for (key, catches) in &handlers {
        stubs.insert(*key, emitter.words.len());
        handler_stub(&mut emitter, catches.as_ref());
    }
    for &(word, key) in &emitter.stub_fixups.clone() {
        let distance = stubs[&key] as i64 - word as i64;
        emitter.words[word] |= ((distance as u32) & 0x7ffff) << 5;
    }
    for &(word, kind, target) in &emitter.fixups.clone() {
        let index = places
            .binary_search_by_key(&(target as u32), |&(unit, _)| unit)
            .map_err(|_| CompileError::BadBranch { at: target })?;
        let destination = places[index].1 as i64 / 4;
        let distance = destination - word as i64;
        emitter.words[word] |= match kind {
            Fixup::Branch => (distance as u32) & 0x03ff_ffff,
            Fixup::Conditional => ((distance as u32) & 0x7ffff) << 5,
        };
    }
    Ok(Compiled {
        code: emitter.words.iter().flat_map(|word| word.to_le_bytes()).collect(),
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
    fn a_method_saves_its_registers_keeps_the_table_and_returns() {
        let bytes = units(&[0x000e]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        assert_eq!(all[0] & 0xff00_03ff, 0xd100_03ff);
        let keep_table = three(0x2a00_0000, true, TABLE, ZERO, TABLE_ARGUMENT);
        assert_eq!(keep_table, 0xaa01_03f3);
        assert!(all.contains(&keep_table));
        assert_eq!(*all.last().unwrap(), 0xd65f_03c0);
    }

    #[test]
    fn the_most_used_register_lives_in_x20_and_a_literal_add_uses_it() {
        let bytes = units(&[0x2012, 0x00d8, 0x0300, 0x000f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        let add = compiled.place_of(1).unwrap() / 4;
        let block = &all[add..add + 4];
        let adds = block
            .iter()
            .any(|&word| word & 0xff20_0000 == 0x0b00_0000 && word & 0x1f == 20);
        assert!(adds, "{block:x?}");
    }

    #[test]
    fn a_loop_s_backward_branch_lands_on_its_first_instruction() {
        let bytes = units(&[0x01d8, 0xff01, 0x0139, 0xfffe, 0x010f]);
        let compiled = compile(&code(&bytes)).unwrap();
        let all = words(&compiled);
        let start = compiled.place_of(2).unwrap() / 4;
        let end = compiled.place_of(4).unwrap() / 4;
        let branch = (start..end)
            .find(|&index| all[index] & 0xff00_0010 == 0x5400_0000)
            .unwrap();
        let distance = ((all[branch] >> 5 & 0x7ffff) << 13) as i32 >> 13;
        assert_eq!(
            branch as i32 + distance,
            (compiled.place_of(0).unwrap() / 4) as i32
        );
    }
}
