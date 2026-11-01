use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::mem;

use super::{ImageInput, SeedOrigin};
use crate::codegen::{Block, BlockEnd, MAX_BLOCK_INSTRUCTIONS, ends_block};
use crate::instruction::{
    Address, Condition, Gpr, Instruction, Memory, Mnemonic, Operand, Register, Width,
};
use crate::{DecodeErrorKind, decode};

/// The most entries a recognised jump table may have.
pub const MAX_JUMP_TABLE_ENTRIES: u32 = 65_536;

const MAX_PLAUSIBLE_INSTRUCTIONS: usize = 64;
const SWEEP_ALIGNMENT: u64 = 16;

/// A recognised `switch` jump table: an indirect `jmp` whose targets are read from a table
/// bounded by the `cmp` and `ja` (or `jae`, `jb`, `jbe`) before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpTable {
    /// The guest address of the indirect `jmp`.
    pub jump: u64,
    /// The guest address of the table.
    pub table: u64,
    /// The size of one entry: 8 for absolute addresses, 4 for offsets from a base address.
    pub entry_bytes: u8,
    /// The case targets, in table order.
    pub targets: Vec<u64>,
}

/// The code found in an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    /// The basic blocks, sorted by their start; a block ends at a branch, call, return, trap,
    /// refused bytes, or where another block starts.
    pub blocks: Vec<Block>,
    /// The guest addresses an indirect branch, a return or a native caller may reach (seeds,
    /// call targets, return sites, jump-table targets, swept entries), sorted.
    pub entries: Vec<u64>,
    /// The recognised jump tables, sorted by the address of their `jmp`.
    pub jump_tables: Vec<JumpTable>,
    /// How many entries the linear sweep over unreached code found.
    pub swept_entries: u32,
    /// How many indirect jumps are neither a recognised jump table nor resolved otherwise;
    /// they go through the indirect dispatch at run time.
    pub unresolved_jumps: u32,
}

impl Discovery {
    /// The index in [`Discovery::blocks`] of the block starting at `address`.
    pub fn block_index(&self, address: u64) -> Option<usize> {
        self.blocks
            .binary_search_by_key(&address, |block| block.start)
            .ok()
    }

    /// The jump table of the indirect `jmp` at `jump`.
    pub fn jump_table(&self, jump: u64) -> Option<&JumpTable> {
        self.jump_tables
            .binary_search_by_key(&jump, |table| table.jump)
            .ok()
            .map(|index| &self.jump_tables[index])
    }
}

/// Finds the code of `input`: recursive descent from every seed (both sides of conditional
/// branches, call targets and the instruction after each call, jump tables, RIP-relative
/// `lea` targets in code that look like code), then a linear sweep of the executable bytes
/// nothing reached, where what looks like a function is translated as an extra entry and
/// the rest is left as data (D191, `docs/translation.md`).
pub fn discover(input: &ImageInput<'_>) -> Discovery {
    let mut explorer = Explorer::new(*input);
    for seed in input.seeds.iter().filter(|seed| seed.origin.is_certain()) {
        explorer.enter(input.address(seed.rva));
    }
    explorer.drain();
    explorer.candidates.extend(
        input
            .seeds
            .iter()
            .filter(|seed| seed.origin == SeedOrigin::Relocation)
            .map(|seed| input.address(seed.rva)),
    );
    loop {
        explorer.drain();
        if explorer.resolve_jump_tables() || explorer.sweep() {
            continue;
        }
        break;
    }
    explorer.finish()
}

#[derive(Clone, Copy, Debug)]
struct Bound {
    register: Gpr,
    count: u32,
}

struct Explorer<'a> {
    input: ImageInput<'a>,
    instructions: BTreeMap<u64, Instruction>,
    refused: BTreeMap<u64, DecodeErrorKind>,
    leaders: BTreeSet<u64>,
    entries: BTreeSet<u64>,
    pending: Vec<u64>,
    candidates: Vec<u64>,
    bounds: BTreeMap<u64, Bound>,
    indirect_jumps: Vec<(u64, u64)>,
    unresolved: Vec<(u64, u64)>,
    tables: BTreeMap<u64, JumpTable>,
    table_ranges: Vec<(u64, u64)>,
    swept: u32,
}

pub(super) fn direct_target(instruction: &Instruction) -> Option<u64> {
    instruction
        .operands()
        .iter()
        .find_map(|operand| match operand {
            Operand::Target(target) => Some(*target),
            _ => None,
        })
}

fn refused_length(error: DecodeErrorKind) -> u64 {
    match error {
        DecodeErrorKind::Unsupported { length, .. } => u64::from(length.max(1)),
        _ => 1,
    }
}

impl<'a> Explorer<'a> {
    fn new(input: ImageInput<'a>) -> Explorer<'a> {
        Explorer {
            input,
            instructions: BTreeMap::new(),
            refused: BTreeMap::new(),
            leaders: BTreeSet::new(),
            entries: BTreeSet::new(),
            pending: Vec::new(),
            candidates: Vec::new(),
            bounds: BTreeMap::new(),
            indirect_jumps: Vec::new(),
            unresolved: Vec::new(),
            tables: BTreeMap::new(),
            table_ranges: Vec::new(),
            swept: 0,
        }
    }

    fn enter(&mut self, address: u64) {
        if self.input.is_code(address) {
            self.entries.insert(address);
            self.push(address);
        }
    }

    fn push(&mut self, address: u64) {
        if self.input.is_code(address) && self.leaders.insert(address) {
            self.pending.push(address);
        }
    }

    fn is_decoded(&self, address: u64) -> bool {
        self.instructions.contains_key(&address) || self.refused.contains_key(&address)
    }

    fn drain(&mut self) {
        loop {
            while let Some(start) = self.pending.pop() {
                self.explore(start);
            }
            let candidates = mem::take(&mut self.candidates);
            if candidates.is_empty() {
                return;
            }
            for candidate in candidates {
                if self.instructions.contains_key(&candidate)
                    || (self.claim_end(candidate).is_none() && self.plausible(candidate))
                {
                    self.enter(candidate);
                }
            }
        }
    }

    fn explore(&mut self, start: u64) {
        let mut address = start;
        let mut previous: Option<Instruction> = None;
        loop {
            if self.is_decoded(address) {
                if address != start {
                    self.leaders.insert(address);
                }
                return;
            }
            let instruction = match decode(self.input.code_from(address), address) {
                Ok(instruction) => instruction,
                Err(error) => {
                    self.refused.insert(address, error.kind);
                    return;
                }
            };
            self.instructions.insert(address, instruction);
            self.follow(&instruction, previous.as_ref(), start);
            if ends_block(instruction.mnemonic) {
                return;
            }
            previous = Some(instruction);
            address = instruction.end();
        }
    }

    fn follow(&mut self, instruction: &Instruction, previous: Option<&Instruction>, run: u64) {
        let next = instruction.end();
        let target = direct_target(instruction);
        match instruction.mnemonic {
            Mnemonic::Jmp => match target {
                Some(target) => self.push(target),
                None => self.indirect_jumps.push((run, instruction.address)),
            },
            Mnemonic::Jcc(condition) => {
                if let Some(target) = target {
                    self.push(target);
                    if let Some(compare) = previous {
                        self.record_bound(compare, condition, target, next);
                    }
                }
                self.push(next);
            }
            Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne | Mnemonic::Jrcxz => {
                if let Some(target) = target {
                    self.push(target);
                }
                self.push(next);
            }
            Mnemonic::Call => {
                if let Some(target) = target {
                    self.enter(target);
                }
                self.enter(next);
            }
            Mnemonic::Lea => {
                if let Some(Operand::Memory(Memory {
                    address: Address::Rip { target },
                    ..
                })) = instruction.operands().get(1)
                    && self.input.is_code(*target)
                {
                    self.candidates.push(*target);
                }
            }
            _ => {}
        }
    }

    fn record_bound(&mut self, compare: &Instruction, condition: Condition, taken: u64, next: u64) {
        if compare.mnemonic != Mnemonic::Cmp {
            return;
        }
        let (register, limit) = match compare.operands() {
            [
                Operand::Register(Register::General { gpr, .. }),
                Operand::Immediate(limit),
            ] => (*gpr, *limit),
            _ => return,
        };
        let Ok(limit) = u32::try_from(limit) else {
            return;
        };
        let (address, count) = match condition {
            Condition::Above => (next, limit.checked_add(1)),
            Condition::AboveOrEqual => (next, Some(limit)),
            Condition::BelowOrEqual => (taken, limit.checked_add(1)),
            Condition::Below => (taken, Some(limit)),
            _ => return,
        };
        if let Some(count) = count.filter(|count| (1..=MAX_JUMP_TABLE_ENTRIES).contains(count)) {
            self.bounds.insert(address, Bound { register, count });
        }
    }

    fn resolve_jump_tables(&mut self) -> bool {
        let mut jumps = mem::take(&mut self.indirect_jumps);
        jumps.append(&mut self.unresolved);
        let mut found = false;
        for (run, jump) in jumps {
            match self.jump_table(run, jump) {
                Some(table) => {
                    for &target in &table.targets {
                        self.enter(target);
                    }
                    let bytes = table.targets.len() as u64 * u64::from(table.entry_bytes);
                    self.table_ranges
                        .push((table.table, table.table.saturating_add(bytes)));
                    self.tables.insert(jump, table);
                    found = true;
                }
                None => self.unresolved.push((run, jump)),
            }
        }
        found
    }

    fn jump_table(&self, run: u64, jump: u64) -> Option<JumpTable> {
        let bound = self.bounds.get(&run).copied();
        let mut state = Symbolic::new();
        let mut address = run;
        let mut steps = 0;
        while address != jump {
            let instruction = self.instructions.get(&address)?;
            state.step(instruction, bound);
            steps += 1;
            if steps > MAX_BLOCK_INSTRUCTIONS {
                return None;
            }
            address = instruction.end();
        }
        let jmp = self.instructions.get(&jump)?;
        let (table, entry_bytes, signed, count, base) = match jmp.operands().first()? {
            Operand::Register(Register::General {
                gpr,
                width: Width::Qword,
            }) => {
                let relative = state.relative[slot(*gpr)]?;
                let load = relative.load;
                (
                    load.table,
                    4u8,
                    load.signed,
                    load.count?,
                    Some(relative.base),
                )
            }
            Operand::Memory(memory) => {
                let (table, index) = state.absolute_table(memory)?;
                (table, 8u8, false, state.bounded(index, bound)?, None)
            }
            _ => return None,
        };
        let mut targets = Vec::with_capacity(count as usize);
        for entry in 0..u64::from(count) {
            let at = table.checked_add(entry * u64::from(entry_bytes))?;
            let bytes = self.input.read(at, usize::from(entry_bytes))?;
            let target = match base {
                None => u64::from_le_bytes(bytes.try_into().ok()?),
                Some(base) => {
                    let raw = u32::from_le_bytes(bytes.try_into().ok()?);
                    let offset = if signed {
                        i64::from(raw as i32) as u64
                    } else {
                        u64::from(raw)
                    };
                    base.wrapping_add(offset)
                }
            };
            if !self.input.is_code(target) {
                return None;
            }
            targets.push(target);
        }
        Some(JumpTable {
            jump,
            table,
            entry_bytes,
            targets,
        })
    }

    fn claim_end(&self, address: u64) -> Option<u64> {
        if let Some((_, instruction)) = self.instructions.range(..=address).next_back()
            && instruction.end() > address
        {
            return Some(instruction.end());
        }
        if let Some((&start, &error)) = self.refused.range(..=address).next_back()
            && start.saturating_add(refused_length(error)) > address
        {
            return Some(start.saturating_add(refused_length(error)));
        }
        self.table_ranges
            .iter()
            .find(|&&(start, end)| (start..end).contains(&address))
            .map(|&(_, end)| end)
    }

    fn next_claim_start(&self, address: u64, limit: u64) -> u64 {
        let instruction = self
            .instructions
            .range(address..limit)
            .next()
            .map(|(&start, _)| start);
        let refused = self
            .refused
            .range(address..limit)
            .next()
            .map(|(&start, _)| start);
        let table = self
            .table_ranges
            .iter()
            .map(|&(start, _)| start)
            .filter(|start| (address..limit).contains(start))
            .min();
        [instruction, refused, table]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(limit)
    }

    fn plausible(&self, start: u64) -> bool {
        let mut address = start;
        for count in 0..MAX_PLAUSIBLE_INSTRUCTIONS {
            if count > 0 && self.instructions.contains_key(&address) {
                return true;
            }
            if self.claim_end(address).is_some() {
                return false;
            }
            let Ok(instruction) = decode(self.input.code_from(address), address) else {
                return false;
            };
            if matches!(
                instruction.mnemonic,
                Mnemonic::Int3 | Mnemonic::Int | Mnemonic::Undefined
            ) {
                return false;
            }
            if direct_target(&instruction).is_some_and(|target| !self.input.is_code(target)) {
                return false;
            }
            if matches!(instruction.mnemonic, Mnemonic::Ret | Mnemonic::Jmp) {
                return true;
            }
            address = instruction.end();
        }
        false
    }

    fn skip_padding(&self, mut address: u64, end: u64) -> u64 {
        while address < end {
            let bytes = self.input.code_from(address);
            match bytes.first() {
                Some(0xCC | 0x00) => address += 1,
                Some(_) => match decode(bytes, address) {
                    Ok(instruction) if instruction.mnemonic == Mnemonic::Nop => {
                        address = instruction.end();
                    }
                    _ => break,
                },
                None => break,
            }
        }
        address.min(end)
    }

    fn sweep(&mut self) -> bool {
        let mut found = false;
        let sections = self.input.sections;
        for section in sections.iter().filter(|section| section.executable) {
            let start = self.input.address(section.rva);
            let end = start.saturating_add(section.bytes.len() as u64);
            let mut address = start;
            while address < end {
                if let Some(next) = self.claim_end(address) {
                    address = next;
                    continue;
                }
                let gap_end = self.next_claim_start(address, end);
                found |= self.sweep_gap(address, gap_end);
                address = gap_end;
            }
        }
        found
    }

    fn sweep_gap(&mut self, start: u64, end: u64) -> bool {
        let mut found = false;
        let mut candidate = start;
        while candidate < end {
            if let Some(next) = self.claim_end(candidate) {
                candidate = next;
                continue;
            }
            let padded = self.skip_padding(candidate, end);
            if padded != candidate {
                candidate = padded;
                continue;
            }
            if self.plausible(candidate) {
                self.enter(candidate);
                self.drain();
                if self.claim_end(candidate).is_some() {
                    self.swept += 1;
                    found = true;
                    continue;
                }
            }
            candidate = (candidate / SWEEP_ALIGNMENT + 1) * SWEEP_ALIGNMENT;
        }
        found
    }

    fn block_at(&self, start: u64, leaders: &BTreeSet<u64>) -> Block {
        let mut instructions = Vec::new();
        let mut address = start;
        let end = loop {
            if !instructions.is_empty() && leaders.contains(&address) {
                break BlockEnd::FallThrough(address);
            }
            if instructions.len() == MAX_BLOCK_INSTRUCTIONS {
                break BlockEnd::FallThrough(address);
            }
            if let Some(instruction) = self.instructions.get(&address) {
                instructions.push(*instruction);
                if ends_block(instruction.mnemonic) {
                    break BlockEnd::Terminator;
                }
                address = instruction.end();
            } else if let Some(&error) = self.refused.get(&address) {
                break BlockEnd::Refused { address, error };
            } else {
                break BlockEnd::FallThrough(address);
            }
        };
        Block {
            start,
            instructions,
            end,
        }
    }

    fn finish(self) -> Discovery {
        let mut leaders: BTreeSet<u64> = self
            .leaders
            .iter()
            .copied()
            .filter(|&leader| self.is_decoded(leader))
            .collect();
        let mut pending: Vec<u64> = leaders.iter().copied().collect();
        let mut blocks = Vec::with_capacity(pending.len());
        while let Some(start) = pending.pop() {
            let block = self.block_at(start, &leaders);
            if let BlockEnd::FallThrough(next) = block.end
                && self.is_decoded(next)
                && leaders.insert(next)
            {
                pending.push(next);
            }
            blocks.push(block);
        }
        blocks.sort_by_key(|block| block.start);
        let entries = self
            .entries
            .iter()
            .copied()
            .filter(|entry| leaders.contains(entry))
            .collect();
        Discovery {
            blocks,
            entries,
            jump_tables: self.tables.into_values().collect(),
            swept_entries: self.swept,
            unresolved_jumps: self.unresolved.len() as u32,
        }
    }
}

fn slot(gpr: Gpr) -> usize {
    usize::from(gpr.0 & 15)
}

#[derive(Clone, Copy, Debug)]
struct TableLoad {
    table: u64,
    signed: bool,
    count: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Relative {
    load: TableLoad,
    base: u64,
}

struct Symbolic {
    constants: [Option<u64>; 16],
    origins: [Option<Gpr>; 16],
    loads: [Option<TableLoad>; 16],
    relative: [Option<Relative>; 16],
}

impl Symbolic {
    fn new() -> Symbolic {
        let mut origins = [None; 16];
        for (index, origin) in origins.iter_mut().enumerate() {
            *origin = Some(Gpr(index as u8));
        }
        Symbolic {
            constants: [None; 16],
            origins,
            loads: [None; 16],
            relative: [None; 16],
        }
    }

    fn unknown() -> Symbolic {
        Symbolic {
            constants: [None; 16],
            origins: [None; 16],
            loads: [None; 16],
            relative: [None; 16],
        }
    }

    fn clear(&mut self, index: usize) {
        self.constants[index] = None;
        self.origins[index] = None;
        self.loads[index] = None;
        self.relative[index] = None;
    }

    fn bounded(&self, index: Gpr, bound: Option<Bound>) -> Option<u32> {
        let bound = bound?;
        (self.origins[slot(index)] == Some(bound.register)).then_some(bound.count)
    }

    fn step(&mut self, instruction: &Instruction, bound: Option<Bound>) {
        use Mnemonic as M;
        match instruction.mnemonic {
            M::Cmp | M::Test | M::Nop => {}
            M::Mov
            | M::Movsxd
            | M::Movzx
            | M::Movsx
            | M::Lea
            | M::Add
            | M::Sub
            | M::And
            | M::Or
            | M::Xor
            | M::Shl
            | M::Shr
            | M::Sar
            | M::Inc
            | M::Dec
            | M::Neg
            | M::Not => self.write(instruction, bound),
            _ => *self = Symbolic::unknown(),
        }
    }

    fn write(&mut self, instruction: &Instruction, bound: Option<Bound>) {
        let operands = instruction.operands();
        let (destination, width) = match operands.first() {
            Some(Operand::Register(Register::General { gpr, width })) => (slot(*gpr), *width),
            Some(Operand::Register(Register::HighByte(gpr))) => {
                self.clear(slot(*gpr));
                return;
            }
            _ => return,
        };
        let mnemonic = instruction.mnemonic;
        match (mnemonic, operands.get(1)) {
            (
                Mnemonic::Lea,
                Some(Operand::Memory(Memory {
                    address: Address::Rip { target },
                    ..
                })),
            ) if width == Width::Qword => {
                let target = *target;
                self.clear(destination);
                self.constants[destination] = Some(target);
            }
            (
                Mnemonic::Mov | Mnemonic::Movsxd | Mnemonic::Movzx,
                Some(Operand::Register(Register::General { gpr, .. })),
            ) => {
                let source = slot(*gpr);
                let origin = self.origins[source];
                let constant = if mnemonic == Mnemonic::Mov && width == Width::Qword {
                    self.constants[source]
                } else {
                    None
                };
                self.clear(destination);
                self.origins[destination] = origin;
                self.constants[destination] = constant;
            }
            (Mnemonic::Mov | Mnemonic::Movsxd, Some(Operand::Memory(memory)))
                if memory.width == Width::Dword && memory.segment.is_none() =>
            {
                let load = self.table_load(memory, mnemonic == Mnemonic::Movsxd, bound);
                self.clear(destination);
                self.loads[destination] = load;
            }
            (Mnemonic::Add, Some(Operand::Register(Register::General { gpr, width: other })))
                if width == Width::Qword && *other == Width::Qword =>
            {
                let source = slot(*gpr);
                let relative = match (
                    self.loads[destination],
                    self.constants[source],
                    self.loads[source],
                    self.constants[destination],
                ) {
                    (Some(load), Some(base), _, _) | (_, _, Some(load), Some(base)) => {
                        Some(Relative { load, base })
                    }
                    _ => None,
                };
                self.clear(destination);
                self.relative[destination] = relative;
            }
            _ => self.clear(destination),
        }
    }

    fn table_load(&self, memory: &Memory, signed: bool, bound: Option<Bound>) -> Option<TableLoad> {
        let Address::Based {
            base: Some(base),
            index: Some(index),
            displacement,
        } = memory.address
        else {
            return None;
        };
        if index.scale != 4 {
            return None;
        }
        let table = self.constants[slot(base)]?.wrapping_add(i64::from(displacement) as u64);
        Some(TableLoad {
            table,
            signed,
            count: self.bounded(index.register, bound),
        })
    }

    fn absolute_table(&self, memory: &Memory) -> Option<(u64, Gpr)> {
        if memory.segment.is_some() || memory.width != Width::Qword {
            return None;
        }
        match memory.address {
            Address::Based {
                base,
                index: Some(index),
                displacement,
            } if index.scale == 8 => {
                let base = match base {
                    None => 0,
                    Some(base) => self.constants[slot(base)]?,
                };
                Some((
                    base.wrapping_add(i64::from(displacement) as u64),
                    index.register,
                ))
            }
            _ => None,
        }
    }
}
