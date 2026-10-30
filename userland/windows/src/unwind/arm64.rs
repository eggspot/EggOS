//! ARM64 unwinding (D177): `.pdata` entries and the `.xdata` records or packed unwind data
//! they hold, as Microsoft's "ARM64 exception handling" documents them. Each unwind code stands
//! for one prolog or epilog instruction and undoes it; a program stopped inside the prolog
//! undoes only the instructions that ran, one inside an epilog carries out the rest of it.
//! Packed unwind data is first expanded into the canonical prolog it describes. The caller's
//! `pc` is `lr` once the frame is undone, and the establisher frame is `sp` at the function's
//! entry, which is `sp` after the step.

use super::{HandlerKinds, Image, Memory, UnwindError, Unwound};

/// The size of an ARM64 `.pdata` entry.
pub const RUNTIME_FUNCTION_SIZE: u64 = 8;
const FRAME_POINTER: u8 = 29;
const LINK_REGISTER: u8 = 30;
const FIRST_SAVED_FLOAT: u8 = 8;
const FIRST_SAVED_INTEGER: u8 = 19;
const MAX_PACKED_OPS: usize = 24;
const MAX_OPS: usize = 1024;
const FLAG_RECORD: u32 = 0;
const FLAG_PACKED: u32 = 1;
const FLAG_PACKED_FRAGMENT: u32 = 2;
const SMALL_FRAME: u64 = 512;
const ONE_ALLOCATION: u64 = 4080;

/// A thread's registers as an unwind step reads and changes them: `x0` to `x30` (`x29` is
/// the frame pointer, `x30` the link register), `sp`, `pc` and `d8` to `d15`, the floating-point
/// registers a function keeps for its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Registers {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub d: [u64; 8],
}

/// One `.pdata` entry: where it lies, the function's first instruction and length (both
/// image-relative, in bytes) and its unwind word (a packed description or an `.xdata` RVA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub entry: u64,
    pub begin: u32,
    pub length: u32,
    pub unwind: u32,
}

fn function_length(image: &Image<'_>, unwind: u32) -> Option<u32> {
    match unwind & 3 {
        FLAG_RECORD => Some((image.u32_at(u64::from(unwind))? & 0x3_ffff) * 4),
        FLAG_PACKED | FLAG_PACKED_FRAGMENT => Some((unwind >> 2 & 0x7ff) * 4),
        _ => None,
    }
}

/// The function whose code holds `pc_rva`, from the `.pdata` table at `table_rva`
/// (`table_size` bytes, sorted by start).
pub fn lookup(image: &Image<'_>, table_rva: u32, table_size: u32, pc_rva: u64) -> Option<Function> {
    let count = u64::from(table_size) / RUNTIME_FUNCTION_SIZE;
    let begin_of = |index: u64| image.u32_at(u64::from(table_rva) + index * RUNTIME_FUNCTION_SIZE);
    let (mut low, mut high) = (0u64, count);
    while low < high {
        let middle = low + (high - low) / 2;
        if u64::from(begin_of(middle)?) <= pc_rva {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let entry = u64::from(table_rva) + low.checked_sub(1)? * RUNTIME_FUNCTION_SIZE;
    let begin = image.u32_at(entry)?;
    let unwind = image.u32_at(entry + 4)?;
    let length = function_length(image, unwind)?;
    (pc_rva < u64::from(begin) + u64::from(length)).then_some(Function {
        entry,
        begin,
        length,
        unwind,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Integer,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Allocate(u64),
    Save {
        kind: Kind,
        registers: [u8; 2],
        count: u8,
        offset: u64,
        increment: u64,
    },
    SaveNext,
    SetFp,
    AddFp(u64),
    Nop,
    End,
}

fn pair(kind: Kind, first: u8, offset: u64, increment: u64) -> Op {
    Op::Save {
        kind,
        registers: [first, first + 1],
        count: 2,
        offset,
        increment,
    }
}

fn single(kind: Kind, register: u8, offset: u64, increment: u64) -> Op {
    Op::Save {
        kind,
        registers: [register, 0],
        count: 1,
        offset,
        increment,
    }
}

fn decode(codes: &[u8], at: usize) -> Result<(Op, usize), UnwindError> {
    let byte = |index: usize| {
        codes
            .get(at + index)
            .map(|byte| u64::from(*byte))
            .ok_or(UnwindError::Truncated)
    };
    let first = byte(0)?;
    let integer = Kind::Integer;
    let float = Kind::Float;
    let op = match first {
        0x00..=0x1f => return Ok((Op::Allocate((first & 0x1f) * 16), 1)),
        0x20..=0x3f => return Ok((pair(integer, 19, 0, (first & 0x1f) * 8), 1)),
        0x40..=0x7f => return Ok((pair(integer, 29, (first & 0x3f) * 8, 0), 1)),
        0x80..=0xbf => return Ok((pair(integer, 29, 0, ((first & 0x3f) + 1) * 8), 1)),
        0xe1 => return Ok((Op::SetFp, 1)),
        0xe3 | 0xfc => return Ok((Op::Nop, 1)),
        0xe4 | 0xe5 => return Ok((Op::End, 1)),
        0xe6 => return Ok((Op::SaveNext, 1)),
        0xe0 => {
            let size = byte(1)? << 16 | byte(2)? << 8 | byte(3)?;
            return Ok((Op::Allocate(size * 16), 4));
        }
        _ => {
            let second = byte(1)?;
            let x4 = ((first & 3) << 2 | second >> 6) as u8;
            let x3 = ((first & 1) << 2 | second >> 6) as u8;
            let z6 = second & 0x3f;
            match first {
                0xc0..=0xc7 => Op::Allocate(((first & 7) << 8 | second) * 16),
                0xc8..=0xcb => pair(integer, 19 + x4, z6 * 8, 0),
                0xcc..=0xcf => pair(integer, 19 + x4, 0, (z6 + 1) * 8),
                0xd0..=0xd3 => single(integer, 19 + x4, z6 * 8, 0),
                0xd4 | 0xd5 => {
                    let register = ((first & 1) << 3 | second >> 5) as u8;
                    single(integer, 19 + register, 0, ((second & 0x1f) + 1) * 8)
                }
                0xd6 | 0xd7 => Op::Save {
                    kind: integer,
                    registers: [19 + 2 * x3, LINK_REGISTER],
                    count: 2,
                    offset: z6 * 8,
                    increment: 0,
                },
                0xd8 | 0xd9 => pair(float, 8 + x3, z6 * 8, 0),
                0xda | 0xdb => pair(float, 8 + x3, 0, (z6 + 1) * 8),
                0xdc | 0xdd => single(float, 8 + x3, z6 * 8, 0),
                0xde => single(float, 8 + (second >> 5) as u8, 0, ((second & 0x1f) + 1) * 8),
                0xe2 => Op::AddFp(second * 8),
                _ => return Err(UnwindError::UnknownCode),
            }
        }
    };
    Ok((op, 2))
}

fn read(memory: &impl Memory, address: u64) -> Result<u64, UnwindError> {
    memory.u64_at(address).ok_or(UnwindError::UnreadableStack)
}

fn restore(registers: &mut Registers, kind: Kind, register: u8, value: u64) {
    match kind {
        Kind::Integer if register <= LINK_REGISTER => registers.x[usize::from(register)] = value,
        Kind::Float if (FIRST_SAVED_FLOAT..FIRST_SAVED_FLOAT + 8).contains(&register) => {
            registers.d[usize::from(register - FIRST_SAVED_FLOAT)] = value
        }
        _ => {}
    }
}

fn apply(op: Op, registers: &mut Registers, memory: &impl Memory) -> Result<(), UnwindError> {
    match op {
        Op::Allocate(size) => registers.sp = registers.sp.wrapping_add(size),
        Op::Save {
            kind,
            registers: saved,
            count,
            offset,
            increment,
        } => {
            let at = registers.sp.wrapping_add(offset);
            for (index, register) in saved.iter().take(usize::from(count)).enumerate() {
                let value = read(memory, at.wrapping_add(index as u64 * 8))?;
                restore(registers, kind, *register, value);
            }
            registers.sp = registers.sp.wrapping_add(increment);
        }
        Op::SetFp => registers.sp = registers.x[usize::from(FRAME_POINTER)],
        Op::AddFp(offset) => {
            registers.sp = registers.x[usize::from(FRAME_POINTER)].wrapping_sub(offset)
        }
        Op::SaveNext | Op::Nop | Op::End => {}
    }
    Ok(())
}

/// The concrete save a `save_next` at `at` stands for: the pair after the one the first
/// non-`save_next` code that follows saves, 16 bytes further, once per `save_next` between.
fn resolve_save_next(codes: &[u8], mut at: usize) -> Result<Op, UnwindError> {
    let mut steps = 0u8;
    loop {
        let (op, length) = decode(codes, at)?;
        match op {
            Op::SaveNext => {
                steps += 1;
                at += length;
            }
            Op::Save {
                kind,
                registers,
                count: 2,
                offset,
                increment,
            } if registers[1] == registers[0] + 1 => {
                let base_offset = if increment != 0 { 0 } else { offset };
                return Ok(pair(
                    kind,
                    registers[0] + 2 * steps,
                    base_offset + 16 * u64::from(steps),
                    0,
                ));
            }
            _ => return Err(UnwindError::UnknownCode),
        }
    }
}

/// Undoes the codes from byte `start` on, after skipping `skip` of them, until `end`.
fn undo_codes(
    codes: &[u8],
    start: usize,
    skip: usize,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<(), UnwindError> {
    let mut at = start;
    let mut index = 0;
    while at < codes.len() && index < MAX_OPS {
        let (op, length) = decode(codes, at)?;
        if op == Op::End {
            break;
        }
        if index >= skip {
            let op = match op {
                Op::SaveNext => resolve_save_next(codes, at)?,
                other => other,
            };
            apply(op, registers, memory)?;
        }
        at += length;
        index += 1;
    }
    Ok(())
}

/// How many codes there are from byte `start` up to `end`, without it (`include_end` false,
/// the prolog's instructions) or with it (an epilog's, whose `end` stands for its `ret`).
fn code_count(codes: &[u8], start: usize, include_end: bool) -> Result<usize, UnwindError> {
    let mut at = start;
    let mut count = 0;
    while at < codes.len() && count < MAX_OPS {
        let (op, length) = decode(codes, at)?;
        if op == Op::End {
            return Ok(count + usize::from(include_end));
        }
        at += length;
        count += 1;
    }
    Ok(count)
}

struct Record<'a> {
    codes: &'a [u8],
    epilogs: Epilogs,
    handler: Option<u64>,
}

enum Epilogs {
    Scopes { rva: u64, count: u64 },
    AtEnd { start: usize },
}

fn record<'a>(image: &Image<'a>, rva: u64) -> Result<Record<'a>, UnwindError> {
    let header = image.u32_at(rva).ok_or(UnwindError::Truncated)?;
    let has_handler = header >> 20 & 1 != 0;
    let single_epilog = header >> 21 & 1 != 0;
    let mut epilog_field = u64::from(header >> 22 & 0x1f);
    let mut code_words = u64::from(header >> 27 & 0x1f);
    let mut at = rva + 4;
    if epilog_field == 0 && code_words == 0 {
        let extension = image.u32_at(at).ok_or(UnwindError::Truncated)?;
        epilog_field = u64::from(extension & 0xffff);
        code_words = u64::from(extension >> 16 & 0xff);
        at += 4;
    }
    let epilogs = match single_epilog {
        true => Epilogs::AtEnd {
            start: epilog_field as usize,
        },
        false => {
            let scopes = Epilogs::Scopes {
                rva: at,
                count: epilog_field,
            };
            at += epilog_field * 4;
            scopes
        }
    };
    let start = at as usize;
    let codes = image
        .bytes
        .get(start..start + code_words as usize * 4)
        .ok_or(UnwindError::Truncated)?;
    let handler = match has_handler {
        true => Some(at + code_words * 4),
        false => None,
    };
    Ok(Record {
        codes,
        epilogs,
        handler,
    })
}

/// Undoes the rest of an epilog of `record`'s function if the program stopped in one, `offset`
/// bytes into a function `length` bytes long.
fn undo_epilog(
    image: &Image<'_>,
    record: &Record<'_>,
    offset: u64,
    length: u64,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<bool, UnwindError> {
    let mut in_scope = |start_offset: u64, start: usize| -> Result<bool, UnwindError> {
        let count = code_count(record.codes, start, true)? as u64;
        if offset < start_offset || offset >= start_offset + count * 4 {
            return Ok(false);
        }
        let done = ((offset - start_offset) / 4) as usize;
        undo_codes(record.codes, start, done, registers, memory)?;
        Ok(true)
    };
    match record.epilogs {
        Epilogs::AtEnd { start } => {
            let count = code_count(record.codes, start, true)? as u64;
            in_scope(length.saturating_sub(count * 4), start)
        }
        Epilogs::Scopes { rva, count } => {
            for scope in 0..count {
                let word = image
                    .u32_at(rva + scope * 4)
                    .ok_or(UnwindError::Truncated)?;
                let start_offset = u64::from(word & 0x3_ffff) * 4;
                if in_scope(start_offset, (word >> 22) as usize)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

struct Packed {
    ops: [Op; MAX_PACKED_OPS],
    in_epilog: [bool; MAX_PACKED_OPS],
    count: usize,
}

impl Packed {
    fn push(&mut self, op: Op, in_epilog: bool) {
        if self.count < MAX_PACKED_OPS {
            self.ops[self.count] = op;
            self.in_epilog[self.count] = in_epilog;
            self.count += 1;
        }
    }

    /// The canonical prolog packed unwind data `word` describes, one op per instruction in
    /// the order they run.
    fn expand(word: u32) -> Self {
        let mut packed = Self {
            ops: [Op::Nop; MAX_PACKED_OPS],
            in_epilog: [false; MAX_PACKED_OPS],
            count: 0,
        };
        let float_registers = u64::from(word >> 13 & 7);
        let integer_registers = (word >> 16 & 0xf) as u8;
        let home_arguments = word >> 20 & 1 != 0;
        let chain = word >> 21 & 3;
        let frame_size = u64::from(word >> 23 & 0x1ff) * 16;
        let lr_saved = chain == 1;
        let integer_size = u64::from(integer_registers) * 8 + if lr_saved { 8 } else { 0 };
        let float_size = if float_registers > 0 {
            (float_registers + 1) * 8
        } else {
            0
        };
        let home_size = if home_arguments { 64 } else { 0 };
        let save_size = (integer_size + float_size + home_size).next_multiple_of(16);
        let local_size = frame_size.saturating_sub(save_size);
        if chain == 2 {
            packed.push(Op::Nop, true);
        }
        let mut first = true;
        let mut place = |offset: u64| -> (u64, u64) {
            match core::mem::replace(&mut first, false) {
                true => (0, save_size),
                false => (offset, 0),
            }
        };
        let mut integers = [0u8; 12];
        let mut integer_count = 0;
        for register in 0..integer_registers.min(10) {
            integers[integer_count] = FIRST_SAVED_INTEGER + register;
            integer_count += 1;
        }
        if lr_saved {
            integers[integer_count] = LINK_REGISTER;
            integer_count += 1;
        }
        for (index, chunk) in integers[..integer_count].chunks(2).enumerate() {
            let (offset, increment) = place(index as u64 * 16);
            let op = match *chunk {
                [a, b] => Op::Save {
                    kind: Kind::Integer,
                    registers: [a, b],
                    count: 2,
                    offset,
                    increment,
                },
                [a] => single(Kind::Integer, a, offset, increment),
                _ => Op::Nop,
            };
            packed.push(op, true);
        }
        if float_registers > 0 {
            let count = float_registers as u8 + 1;
            for index in (0..count).step_by(2) {
                let (offset, increment) = place(integer_size + u64::from(index) * 8);
                let register = FIRST_SAVED_FLOAT + index;
                let op = match count - index {
                    1 => single(Kind::Float, register, offset, increment),
                    _ => pair(Kind::Float, register, offset, increment),
                };
                packed.push(op, true);
            }
        }
        if home_arguments {
            for index in 0..4u64 {
                let (_, increment) = place(integer_size + float_size + index * 16);
                match increment {
                    0 => packed.push(Op::Nop, false),
                    _ => packed.push(Op::Allocate(increment), true),
                }
            }
        }
        let frame_pair = |offset: u64, increment: u64| Op::Save {
            kind: Kind::Integer,
            registers: [FRAME_POINTER, LINK_REGISTER],
            count: 2,
            offset,
            increment,
        };
        match chain {
            2 | 3 if local_size <= SMALL_FRAME => {
                packed.push(frame_pair(0, local_size), true);
                packed.push(Op::SetFp, false);
            }
            2 | 3 => {
                let first_part = local_size.min(ONE_ALLOCATION);
                packed.push(Op::Allocate(first_part), true);
                packed.push(frame_pair(0, 0), true);
                packed.push(Op::SetFp, false);
                if local_size > ONE_ALLOCATION {
                    packed.push(Op::Allocate(local_size - ONE_ALLOCATION), true);
                }
            }
            _ if local_size > ONE_ALLOCATION => {
                packed.push(Op::Allocate(ONE_ALLOCATION), true);
                packed.push(Op::Allocate(local_size - ONE_ALLOCATION), true);
            }
            _ if local_size > 0 => packed.push(Op::Allocate(local_size), true),
            _ => {}
        }
        packed
    }

    fn epilog_count(&self) -> usize {
        self.in_epilog[..self.count]
            .iter()
            .filter(|kept| **kept)
            .count()
    }

    fn undo(
        &self,
        ran: usize,
        registers: &mut Registers,
        memory: &impl Memory,
    ) -> Result<(), UnwindError> {
        for op in self.ops[..ran.min(self.count)].iter().rev() {
            apply(*op, registers, memory)?;
        }
        Ok(())
    }

    fn undo_epilog_from(
        &self,
        done: usize,
        registers: &mut Registers,
        memory: &impl Memory,
    ) -> Result<(), UnwindError> {
        let epilog = self.ops[..self.count]
            .iter()
            .zip(&self.in_epilog[..self.count])
            .rev()
            .filter(|(_, kept)| **kept)
            .map(|(op, _)| *op);
        for op in epilog.skip(done) {
            apply(op, registers, memory)?;
        }
        Ok(())
    }
}

/// Undoes one frame of `function`, whose code holds `registers.pc`: on success `registers`
/// hold the caller's registers (`pc` the return address) and the result names the frame's
/// establisher and, if `kinds` asks for any handler and the function has one and the program
/// was in its body, its language handler and the handler's data.
pub fn virtual_unwind(
    image: &Image<'_>,
    function: Function,
    kinds: HandlerKinds,
    registers: &mut Registers,
    memory: &impl Memory,
) -> Result<Unwound, UnwindError> {
    let offset = image
        .rva_of(registers.pc)
        .and_then(|rva| rva.checked_sub(u64::from(function.begin)))
        .unwrap_or(0);
    let length = u64::from(function.length);
    let mut handler = None;
    match function.unwind & 3 {
        FLAG_RECORD => {
            let record = record(image, u64::from(function.unwind))?;
            let prolog = code_count(record.codes, 0, false)? as u64;
            let in_epilog = undo_epilog(image, &record, offset, length, registers, memory)?;
            if in_epilog {
                handler = None;
            } else if offset < prolog * 4 {
                let ran = (offset / 4) as usize;
                undo_codes(record.codes, 0, prolog as usize - ran, registers, memory)?;
            } else {
                undo_codes(record.codes, 0, 0, registers, memory)?;
                if kinds.exception || kinds.termination {
                    handler = record.handler;
                }
            }
        }
        FLAG_PACKED => {
            let packed = Packed::expand(function.unwind);
            let epilog = packed.epilog_count() as u64 + 1;
            let epilog_start = length.saturating_sub(epilog * 4);
            if offset < packed.count as u64 * 4 {
                packed.undo((offset / 4) as usize, registers, memory)?;
            } else if offset >= epilog_start {
                let done = ((offset - epilog_start) / 4) as usize;
                packed.undo_epilog_from(done, registers, memory)?;
            } else {
                packed.undo(packed.count, registers, memory)?;
            }
        }
        FLAG_PACKED_FRAGMENT => {
            let packed = Packed::expand(function.unwind);
            packed.undo(packed.count, registers, memory)?;
        }
        _ => return Err(UnwindError::UnknownCode),
    }
    registers.pc = registers.x[usize::from(LINK_REGISTER)];
    let handler = match handler {
        Some(at) => {
            let rva = image.u32_at(at).ok_or(UnwindError::Truncated)?;
            Some((image.base + u64::from(rva), image.base + at + 4))
        }
        None => None,
    };
    Ok(Unwound {
        handler: handler.map(|(address, _)| address),
        handler_data: handler.map_or(0, |(_, data)| data),
        establisher: registers.sp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unwind::test_memory::Stack;

    const BASE: u64 = 0x1_4000_0000;
    const XDATA: u64 = 0x2000;
    const PDATA: u64 = 0x3000;
    const STACK: u64 = 0x7000_0000;
    const BOTH: HandlerKinds = HandlerKinds {
        exception: true,
        termination: true,
    };

    fn image_with(entries: &[(u32, u32)], xdata: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x4000];
        bytes[XDATA as usize..XDATA as usize + xdata.len()].copy_from_slice(xdata);
        for (index, (begin, unwind)) in entries.iter().enumerate() {
            let at = PDATA as usize + index * 8;
            bytes[at..at + 4].copy_from_slice(&begin.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&unwind.to_le_bytes());
        }
        bytes
    }

    fn header(length: u32, handler: bool, single: bool, epilogs: u32, words: u32) -> [u8; 4] {
        ((length / 4)
            | u32::from(handler) << 20
            | u32::from(single) << 21
            | epilogs << 22
            | words << 27)
            .to_le_bytes()
    }

    fn registers_at(pc: u64) -> Registers {
        Registers {
            pc: BASE + pc,
            sp: STACK,
            ..Registers::default()
        }
    }

    fn stack(words: &[u64]) -> Stack {
        Stack {
            base: STACK,
            words: words.to_vec(),
        }
    }

    fn standard_record() -> Vec<u8> {
        let mut xdata = Vec::new();
        xdata.extend(header(0x40, true, false, 1, 2));
        xdata.extend(((0x34u32 / 4) | 4 << 22).to_le_bytes());
        xdata.extend([0xe1, 0x81, 0x22, 0xe4, 0x81, 0x22, 0xe4, 0xe4]);
        xdata.extend(0x1800u32.to_le_bytes());
        xdata.extend(0x2468u32.to_le_bytes());
        xdata
    }

    #[test]
    fn functions_are_found_by_start_and_length() {
        let xdata = standard_record();
        let bytes = image_with(&[(0x1000, XDATA as u32), (0x1100, 0x20 << 2 | 1)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let found = lookup(&image, PDATA as u32, 16, 0x1010).unwrap();
        assert_eq!((found.begin, found.length), (0x1000, 0x40));
        assert_eq!(lookup(&image, PDATA as u32, 16, 0x1040), None);
        let packed = lookup(&image, PDATA as u32, 16, 0x117c).unwrap();
        assert_eq!(
            (packed.begin, packed.length, packed.entry),
            (0x1100, 0x80, PDATA + 8)
        );
        assert_eq!(lookup(&image, PDATA as u32, 16, 0x0fff), None);
    }

    #[test]
    fn the_body_undoes_every_code_and_returns_the_handler() {
        let xdata = standard_record();
        let bytes = image_with(&[(0x1000, XDATA as u32)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1020).unwrap();
        let mut registers = registers_at(0x1020);
        registers.x[29] = STACK + 16;
        let memory = stack(&[0, 0, STACK + 0x100, BASE + 0x1234, 0x1919, 0x2020]);
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.x[29], STACK + 0x100);
        assert_eq!(registers.x[30], BASE + 0x1234);
        assert_eq!(registers.pc, BASE + 0x1234);
        assert_eq!((registers.x[19], registers.x[20]), (0x1919, 0x2020));
        assert_eq!(registers.sp, STACK + 48);
        assert_eq!(unwound.establisher, STACK + 48);
        assert_eq!(unwound.handler, Some(BASE + 0x1800));
        assert_eq!(unwound.handler_data, BASE + XDATA + 20);
    }

    #[test]
    fn inside_the_prolog_or_an_epilog_only_part_is_undone_and_no_handler_is_called() {
        let xdata = standard_record();
        let bytes = image_with(&[(0x1000, XDATA as u32)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1004).unwrap();
        let memory = stack(&[0x1919, 0x2020, 0, 0]);
        let mut registers = registers_at(0x1004);
        registers.x[30] = BASE + 0x5555;
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!((registers.x[19], registers.x[20]), (0x1919, 0x2020));
        assert_eq!(registers.sp, STACK + 16);
        assert_eq!(registers.pc, BASE + 0x5555);
        assert_eq!(unwound.handler, None);
        let mut entry = registers_at(0x1000);
        entry.x[30] = BASE + 0x6666;
        virtual_unwind(&image, function, BOTH, &mut entry, &memory).unwrap();
        assert_eq!((entry.sp, entry.pc), (STACK, BASE + 0x6666));
        let words = [STACK + 0x200, BASE + 0x7777, 0x1919, 0x2020];
        let memory = stack(&words);
        let mut in_epilog = registers_at(0x1034);
        let unwound = virtual_unwind(&image, function, BOTH, &mut in_epilog, &memory).unwrap();
        assert_eq!(unwound.handler, None);
        assert_eq!(in_epilog.x[29], STACK + 0x200);
        assert_eq!(in_epilog.pc, BASE + 0x7777);
        assert_eq!(in_epilog.sp, STACK + 32);
        let mut at_ret = registers_at(0x1040 - 4);
        at_ret.x[30] = BASE + 0x8888;
        virtual_unwind(&image, function, BOTH, &mut at_ret, &memory).unwrap();
        assert_eq!((at_ret.sp, at_ret.pc), (STACK, BASE + 0x8888));
    }

    #[test]
    fn save_next_continues_the_pair_before_it_and_float_registers_are_kept() {
        let mut xdata = Vec::new();
        xdata.extend(header(0x80, false, true, 0, 1));
        xdata.extend([0xe6, 0xe6, 0x26, 0xe4]);
        let bytes = image_with(&[(0x1000, XDATA as u32)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1040).unwrap();
        let words = [
            19, 20, 21, 22, 23, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x8, 0x9,
        ];
        let memory = stack(&words);
        let mut registers = registers_at(0x1040);
        registers.x[30] = BASE + 0x4444;
        virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(&registers.x[19..25], &[19, 20, 21, 22, 23, 24]);
        assert_eq!(registers.sp, STACK + 48);
        let mut deeper = Vec::new();
        deeper.extend(header(0x80, false, true, 0, 1));
        deeper.extend([0xd8, 0x02, 0x26, 0xe4]);
        let bytes = image_with(&[(0x1000, XDATA as u32)], &deeper);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let mut registers = registers_at(0x1040);
        let memory = stack(&[19, 20, 0x88, 0x99, 0]);
        let function = lookup(&image, PDATA as u32, 8, 0x1040).unwrap();
        virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!((registers.d[0], registers.d[1]), (0x88, 0x99));
        assert_eq!(registers.sp, STACK + 48);
    }

    #[test]
    fn packed_data_is_expanded_into_its_canonical_prolog() {
        let length = 0x40u32 / 4;
        let word = 1 | length << 2 | 2 << 16 | 3 << 21 | 4 << 23;
        let bytes = image_with(&[(0x1000, word)], &[]);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1020).unwrap();
        let memory = stack(&[STACK + 0x300, BASE + 0x9999, 0, 0, 0, 0, 0x1919, 0x2020]);
        let mut registers = registers_at(0x1020);
        registers.x[29] = STACK;
        let unwound = virtual_unwind(&image, function, BOTH, &mut registers, &memory).unwrap();
        assert_eq!(registers.pc, BASE + 0x9999);
        assert_eq!(registers.x[29], STACK + 0x300);
        assert_eq!((registers.x[19], registers.x[20]), (0x1919, 0x2020));
        assert_eq!(registers.sp, STACK + 64);
        assert_eq!(unwound.establisher, STACK + 64);
        assert_eq!(unwound.handler, None);
        let mut first_instruction_done = registers_at(0x1004);
        first_instruction_done.sp = STACK + 48;
        first_instruction_done.x[30] = BASE + 0xabcd;
        virtual_unwind(&image, function, BOTH, &mut first_instruction_done, &memory).unwrap();
        assert_eq!(first_instruction_done.sp, STACK + 64);
        assert_eq!(first_instruction_done.x[19], 0x1919);
        assert_eq!(first_instruction_done.pc, BASE + 0xabcd);
        let mut before_ret = registers_at(0x1040 - 4);
        before_ret.sp = STACK + 64;
        before_ret.x[30] = BASE + 0x1111;
        virtual_unwind(&image, function, BOTH, &mut before_ret, &memory).unwrap();
        assert_eq!((before_ret.sp, before_ret.pc), (STACK + 64, BASE + 0x1111));
    }

    #[test]
    fn an_unknown_code_or_an_unreadable_stack_is_an_error() {
        let mut xdata = Vec::new();
        xdata.extend(header(0x40, false, true, 1, 1));
        xdata.extend([0xe7, 0x00, 0x00, 0xe4]);
        let bytes = image_with(&[(0x1000, XDATA as u32)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1020).unwrap();
        let memory = stack(&[]);
        let mut registers = registers_at(0x1020);
        assert_eq!(
            virtual_unwind(&image, function, BOTH, &mut registers, &memory),
            Err(UnwindError::UnknownCode)
        );
        let xdata = standard_record();
        let bytes = image_with(&[(0x1000, XDATA as u32)], &xdata);
        let image = Image {
            base: BASE,
            bytes: &bytes,
        };
        let function = lookup(&image, PDATA as u32, 8, 0x1020).unwrap();
        let mut registers = registers_at(0x1020);
        registers.x[29] = STACK;
        assert_eq!(
            virtual_unwind(&image, function, BOTH, &mut registers, &memory),
            Err(UnwindError::UnreadableStack)
        );
    }
}
