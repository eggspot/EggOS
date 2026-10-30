use alloc::vec::Vec;

use super::aarch64::emit::{Assembler, Label, Literal};
use super::aarch64::encode::{self, AddSub, Bitfield, LoadStore, Logic, MoveWide, Ordered};
use super::aarch64::{Access, Cond, Reg, Shift, Size};
use super::flags::{self, Flags};
use super::registers::{ADDRESS, GS_BASE, SCRATCH, STATE, TEMPORARIES, guest};
use super::{
    Block, BlockEnd, CodegenMode, Exit, ExitKind, InstructionStart, LowerError, LowerErrorKind,
    TranslatedBlock, state, trap,
};
use crate::decoder::DecodeErrorKind;
use crate::instruction::{
    Address, Condition, Instruction, Memory, Mnemonic, Operand, Register, Segment, Width,
};

mod arith;
mod bits;
mod control;
mod sse;
mod system;

#[cfg(test)]
mod tests;

const T0: Reg = TEMPORARIES[0];
const T1: Reg = TEMPORARIES[1];
const T2: Reg = TEMPORARIES[2];
const T3: Reg = TEMPORARIES[3];
const T4: Reg = TEMPORARIES[4];
const T5: Reg = TEMPORARIES[5];
const T6: Reg = TEMPORARIES[6];

const CARRY_BIT: u8 = 29;
const OVERFLOW_BIT: u8 = 28;

type Lowered<T = ()> = Result<T, LowerErrorKind>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CarrySense {
    Direct,
    Inverted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Full(Reg),
    Low32(Reg),
    Low16(Reg),
    Low8(Reg),
    High8(Reg),
    Memory { address: Reg, width: Width },
}

impl Place {
    fn width(self) -> Width {
        match self {
            Place::Full(_) => Width::Qword,
            Place::Low32(_) => Width::Dword,
            Place::Low16(_) => Width::Word,
            Place::Low8(_) | Place::High8(_) => Width::Byte,
            Place::Memory { width, .. } => width,
        }
    }

    fn register(gpr: crate::instruction::Gpr, width: Width) -> Place {
        let host = guest(gpr);
        match width {
            Width::Qword => Place::Full(host),
            Width::Dword => Place::Low32(host),
            Width::Word => Place::Low16(host),
            _ => Place::Low8(host),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Test {
    Cond(Cond),
    Parity { set: bool },
}

fn size(width: Width) -> Size {
    if width == Width::Qword {
        Size::X
    } else {
        Size::W
    }
}

fn bits(width: Width) -> u8 {
    width.bytes() * 8
}

fn access(width: Width) -> Access {
    match width {
        Width::Byte => Access::B,
        Width::Word => Access::H,
        Width::Dword => Access::W,
        _ => Access::X,
    }
}

fn truncate(value: i64, width: Width) -> u64 {
    match width {
        Width::Byte => value as u64 & 0xFF,
        Width::Word => value as u64 & 0xFFFF,
        Width::Dword => value as u64 & 0xFFFF_FFFF,
        _ => value as u64,
    }
}

fn state_access(op: LoadStore, t: Reg, offset: u32) -> u32 {
    encode::load_store(op, t, STATE, offset).expect("state block offsets are encodable")
}

struct Lowerer {
    asm: Assembler,
    mode: CodegenMode,
    carry: CarrySense,
    live_out: Flags,
    live: Flags,
    mnemonic: Mnemonic,
    exits: Vec<(u32, ExitKind)>,
    starts: Vec<InstructionStart>,
    guest_literals: Vec<Literal>,
}

pub(super) fn lower(
    block: &Block,
    mode: CodegenMode,
    live_out: Flags,
) -> Result<TranslatedBlock, LowerError> {
    let live = flags::live_after(&block.instructions, live_out);
    let mut lowerer = Lowerer {
        asm: Assembler::new(),
        mode,
        carry: CarrySense::Direct,
        live_out,
        live: live_out,
        mnemonic: Mnemonic::Nop,
        exits: Vec::new(),
        starts: Vec::new(),
        guest_literals: Vec::new(),
    };
    for (instruction, live) in block.instructions.iter().zip(live) {
        lowerer.starts.push(InstructionStart {
            word: lowerer.asm.position(),
            guest: instruction.address,
        });
        lowerer.live = live;
        lowerer.mnemonic = instruction.mnemonic;
        lowerer
            .instruction(instruction)
            .map_err(|kind| LowerError {
                address: instruction.address,
                kind,
            })?;
    }
    let guest_end = block
        .instructions
        .last()
        .map_or(block.start, Instruction::end);
    match block.end {
        BlockEnd::Terminator => {}
        BlockEnd::FallThrough(next) => lowerer.exit(ExitKind::Jump(next)),
        BlockEnd::Refused { address, error } => {
            lowerer.starts.push(InstructionStart {
                word: lowerer.asm.position(),
                guest: address,
            });
            let code = match error {
                DecodeErrorKind::Unsupported { .. } => {
                    lowerer.guest_address(ADDRESS, address);
                    trap::REFUSED
                }
                _ => trap::INVALID_OPCODE,
            };
            lowerer.asm.emit(encode::brk(code));
        }
    }
    let assembled = lowerer.asm.finish().map_err(|error| LowerError {
        address: guest_end,
        kind: LowerErrorKind::Emit(error),
    })?;
    Ok(TranslatedBlock {
        guest: block.start,
        guest_end,
        code: assembled.words,
        exits: lowerer
            .exits
            .into_iter()
            .map(|(word, kind)| Exit { word, kind })
            .collect(),
        starts: lowerer.starts,
        guest_address_literals: lowerer
            .guest_literals
            .iter()
            .map(|literal| assembled.literals[literal.index()])
            .collect(),
    })
}

impl Lowerer {
    fn instruction(&mut self, instruction: &Instruction) -> Lowered {
        use Mnemonic as M;
        match instruction.mnemonic {
            M::Mov => self.mov(instruction),
            M::Movzx => self.extend(instruction, false),
            M::Movsx | M::Movsxd => self.extend(instruction, true),
            M::Lea => self.lea(instruction),
            M::Add | M::Sub | M::Adc | M::Sbb | M::And | M::Or | M::Xor | M::Cmp | M::Test => {
                self.binary(instruction)
            }
            M::Inc | M::Dec => self.step(instruction),
            M::Neg => self.negate(instruction),
            M::Not => self.not(instruction),
            M::Mul | M::Imul => self.multiply(instruction),
            M::Div | M::Idiv => self.divide(instruction),
            M::ExtendAccumulator => self.extend_accumulator(instruction.width),
            M::ExtendIntoRdx => self.extend_into_rdx(instruction.width),
            M::Shl | M::Shr | M::Sar => self.shift(instruction),
            M::Rol | M::Ror => self.rotate(instruction),
            M::Shld | M::Shrd => self.double_shift(instruction),
            M::Bt | M::Bts | M::Btr | M::Btc => self.bit_test(instruction),
            M::Bsf | M::Tzcnt | M::Bsr => self.bit_scan(instruction),
            M::Lzcnt => self.leading_zeros(instruction),
            M::Popcnt => self.population_count(instruction),
            M::Bswap => self.byte_swap(instruction),
            M::Cmov(condition) => self.conditional_move(instruction, condition),
            M::Set(condition) => self.set(instruction, condition),
            M::Xchg => self.exchange(instruction),
            M::Xadd => self.exchange_add(instruction),
            M::Cmpxchg => self.compare_exchange(instruction),
            M::Push => self.push(instruction),
            M::Pop => self.pop(instruction),
            M::Leave => self.leave(instruction.width),
            M::Jmp => self.jump(instruction),
            M::Jcc(condition) => self.conditional_jump(instruction, condition),
            M::Loop | M::Loope | M::Loopne | M::Jrcxz => self.count_jump(instruction),
            M::Call => self.call(instruction),
            M::Ret => self.ret(instruction),
            M::Nop | M::Endbr64 | M::Endbr32 | M::Prefetch => Ok(()),
            M::Pause => {
                self.asm.emit(encode::yield_hint());
                Ok(())
            }
            M::Mfence => self.barrier(encode::Barrier::Ish),
            M::Sfence => self.barrier(encode::Barrier::IshSt),
            M::Lfence => self.barrier(encode::Barrier::IshLd),
            M::Clc | M::Stc => self.set_carry_constant(instruction.mnemonic == M::Stc),
            M::Cmc => {
                self.carry = match self.carry {
                    CarrySense::Direct => CarrySense::Inverted,
                    CarrySense::Inverted => CarrySense::Direct,
                };
                Ok(())
            }
            M::Int3 => self.trap(trap::BREAKPOINT),
            M::Undefined => self.trap(trap::INVALID_OPCODE),
            M::Int => match instruction.operands() {
                [Operand::Immediate(0x29)] => self.trap(trap::FAST_FAIL),
                [Operand::Immediate(3)] => self.trap(trap::BREAKPOINT),
                _ => self.trap(trap::GENERAL_PROTECTION),
            },
            other => {
                if self.sse(instruction)? || self.system(instruction)? {
                    Ok(())
                } else {
                    Err(LowerErrorKind::Unhandled(other))
                }
            }
        }
    }

    fn operands_error(&self) -> LowerErrorKind {
        LowerErrorKind::Operands(self.mnemonic)
    }

    fn defined_and_live(&self, instruction: &Instruction) -> Flags {
        flags::effect(instruction).defines.intersection(self.live)
    }

    fn emit(&mut self, word: u32) {
        self.asm.emit(word);
    }

    fn trap(&mut self, code: u16) -> Lowered {
        self.emit(encode::brk(code));
        Ok(())
    }

    fn barrier(&mut self, barrier: encode::Barrier) -> Lowered {
        self.emit(encode::dmb(barrier));
        Ok(())
    }

    fn copy(&mut self, size: Size, d: Reg, n: Reg) {
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            size,
            d,
            Reg::ZR,
            n,
            Shift::Lsl,
            0,
        ));
    }

    fn ubfx(&mut self, size: Size, d: Reg, n: Reg, lsb: u8, width: u8) {
        self.emit(encode::bitfield(
            Bitfield::Ubfm,
            size,
            d,
            n,
            lsb,
            lsb + width - 1,
        ));
    }

    fn sbfx(&mut self, size: Size, d: Reg, n: Reg, lsb: u8, width: u8) {
        self.emit(encode::bitfield(
            Bitfield::Sbfm,
            size,
            d,
            n,
            lsb,
            lsb + width - 1,
        ));
    }

    fn bfi(&mut self, d: Reg, n: Reg, lsb: u8, width: u8) {
        let immr = (64 - lsb) % 64;
        self.emit(encode::bitfield(
            Bitfield::Bfm,
            Size::X,
            d,
            n,
            immr,
            width - 1,
        ));
    }

    fn shift_left(&mut self, size: Size, d: Reg, n: Reg, amount: u8) {
        let total = size.bits() as u8;
        let immr = (total - amount) % total;
        self.emit(encode::bitfield(
            Bitfield::Ubfm,
            size,
            d,
            n,
            immr,
            total - 1 - amount,
        ));
    }

    fn shift_right(&mut self, size: Size, d: Reg, n: Reg, amount: u8) {
        let total = size.bits() as u8;
        self.emit(encode::bitfield(
            Bitfield::Ubfm,
            size,
            d,
            n,
            amount,
            total - 1,
        ));
    }

    fn logic_immediate(&mut self, op: Logic, size: Size, d: Reg, n: Reg, value: u64) {
        match encode::logical_immediate(op, size, d, n, value) {
            Some(word) => self.emit(word),
            None => {
                self.move_immediate(size, SCRATCH, value);
                self.emit(encode::logical_shifted(
                    op,
                    false,
                    size,
                    d,
                    n,
                    SCRATCH,
                    Shift::Lsl,
                    0,
                ));
            }
        }
    }

    fn move_immediate(&mut self, size: Size, d: Reg, value: u64) {
        let value = if size == Size::W {
            value & 0xFFFF_FFFF
        } else {
            value
        };
        if let Some(word) = encode::logical_immediate(Logic::Orr, size, d, Reg::ZR, value) {
            self.emit(word);
            return;
        }
        let chunks = if size == Size::W { 2 } else { 4 };
        let halves: [u16; 4] = core::array::from_fn(|index| (value >> (16 * index)) as u16);
        let zeros = halves[..chunks].iter().filter(|&&half| half == 0).count();
        let ones = halves[..chunks]
            .iter()
            .filter(|&&half| half == 0xFFFF)
            .count();
        let (first_op, skip) = if ones > zeros {
            (MoveWide::Movn, 0xFFFF)
        } else {
            (MoveWide::Movz, 0)
        };
        let mut first = true;
        for (hw, &half) in halves[..chunks].iter().enumerate() {
            if half == skip {
                continue;
            }
            if first {
                let imm = if first_op == MoveWide::Movn {
                    !half
                } else {
                    half
                };
                self.emit(encode::move_wide(first_op, size, d, imm, hw as u8));
                first = false;
            } else {
                self.emit(encode::move_wide(MoveWide::Movk, size, d, half, hw as u8));
            }
        }
        if first {
            self.emit(encode::move_wide(first_op, size, d, 0, 0));
        }
    }

    fn add_constant(&mut self, d: Reg, n: Reg, value: i64) {
        let op = if value < 0 { AddSub::Sub } else { AddSub::Add };
        let magnitude = value.unsigned_abs();
        if magnitude == 0 {
            if d != n {
                self.copy(Size::X, d, n);
            }
        } else if magnitude < 0x1000 {
            self.emit(encode::add_sub_immediate(
                op,
                Size::X,
                d,
                n,
                magnitude as u16,
                false,
            ));
        } else if magnitude < 0x100_0000 {
            let high = (magnitude >> 12) as u16;
            self.emit(encode::add_sub_immediate(op, Size::X, d, n, high, true));
            let low = (magnitude & 0xFFF) as u16;
            if low != 0 {
                self.emit(encode::add_sub_immediate(op, Size::X, d, d, low, false));
            }
        } else {
            self.move_immediate(Size::X, SCRATCH, value as u64);
            self.emit(encode::add_sub_shifted(
                AddSub::Add,
                Size::X,
                d,
                n,
                SCRATCH,
                Shift::Lsl,
                0,
            ));
        }
    }

    fn guest_address(&mut self, d: Reg, address: u64) {
        let literal = self.asm.literal(address);
        self.guest_literals.push(literal);
        self.asm.load_literal(d, literal);
    }

    fn effective_address(&mut self, memory: &Memory, target: Reg, segment: Option<Segment>) -> Reg {
        let address = match memory.address {
            Address::Based {
                base: Some(base),
                index: None,
                displacement: 0,
            } if segment.is_none() => return guest(base),
            Address::Based {
                base,
                index,
                displacement,
            } => {
                let start = match (base, index) {
                    (Some(base), Some(index)) => {
                        let amount = index.scale.trailing_zeros() as u8;
                        self.emit(encode::add_sub_shifted(
                            AddSub::Add,
                            Size::X,
                            target,
                            guest(base),
                            guest(index.register),
                            Shift::Lsl,
                            amount,
                        ));
                        target
                    }
                    (Some(base), None) => guest(base),
                    (None, Some(index)) => {
                        let amount = index.scale.trailing_zeros() as u8;
                        self.emit(encode::add_sub_shifted(
                            AddSub::Add,
                            Size::X,
                            target,
                            Reg::ZR,
                            guest(index.register),
                            Shift::Lsl,
                            amount,
                        ));
                        target
                    }
                    (None, None) => {
                        self.move_immediate(Size::X, target, i64::from(displacement) as u64);
                        target
                    }
                };
                if base.is_none() && index.is_none() {
                    target
                } else {
                    self.add_constant(target, start, i64::from(displacement));
                    target
                }
            }
            Address::Rip { target: address } => {
                self.guest_address(target, address);
                target
            }
            Address::Absolute(address) => {
                self.move_immediate(Size::X, target, address);
                target
            }
        };
        match segment {
            Some(Segment::Gs) => {
                self.emit(encode::add_sub_shifted(
                    AddSub::Add,
                    Size::X,
                    target,
                    address,
                    GS_BASE,
                    Shift::Lsl,
                    0,
                ));
                target
            }
            Some(Segment::Fs) => {
                self.emit(state_access(LoadStore::LdrX, SCRATCH, state::FS_BASE));
                self.emit(encode::add_sub_shifted(
                    AddSub::Add,
                    Size::X,
                    target,
                    address,
                    SCRATCH,
                    Shift::Lsl,
                    0,
                ));
                target
            }
            None => address,
        }
    }

    fn place(&mut self, operand: &Operand) -> Lowered<Place> {
        match *operand {
            Operand::Register(Register::General { gpr, width }) => Ok(Place::register(gpr, width)),
            Operand::Register(Register::HighByte(gpr)) => Ok(Place::High8(guest(gpr))),
            Operand::Memory(memory)
                if matches!(
                    memory.width,
                    Width::Byte | Width::Word | Width::Dword | Width::Qword
                ) =>
            {
                let address = self.effective_address(&memory, ADDRESS, memory.segment);
                Ok(Place::Memory {
                    address,
                    width: memory.width,
                })
            }
            _ => Err(self.operands_error()),
        }
    }

    fn operand(&self, instruction: &Instruction, index: usize) -> Lowered<Operand> {
        instruction
            .operands()
            .get(index)
            .copied()
            .ok_or(self.operands_error())
    }

    fn load_memory(&mut self, address: Reg, width: Width, into: Reg) {
        let word = if self.mode.hardware_tso {
            let op = match width {
                Width::Byte => LoadStore::Ldrb,
                Width::Word => LoadStore::Ldrh,
                Width::Dword => LoadStore::LdrW,
                _ => LoadStore::LdrX,
            };
            encode::load_store(op, into, address, 0).expect("offset 0 is encodable")
        } else {
            let op = if self.mode.lrcpc {
                Ordered::Ldapr
            } else {
                Ordered::Ldar
            };
            encode::ordered(op, access(width), into, address)
        };
        self.emit(word);
    }

    fn store_memory(&mut self, address: Reg, width: Width, value: Reg) {
        let word = if self.mode.hardware_tso {
            let op = match width {
                Width::Byte => LoadStore::Strb,
                Width::Word => LoadStore::Strh,
                Width::Dword => LoadStore::StrW,
                _ => LoadStore::StrX,
            };
            encode::load_store(op, value, address, 0).expect("offset 0 is encodable")
        } else {
            encode::ordered(Ordered::Stlr, access(width), value, address)
        };
        self.emit(word);
    }

    fn load(&mut self, place: Place, into: Reg) -> Reg {
        match place {
            Place::Full(host) | Place::Low32(host) => host,
            Place::Low16(host) => {
                self.ubfx(Size::W, into, host, 0, 16);
                into
            }
            Place::Low8(host) => {
                self.ubfx(Size::W, into, host, 0, 8);
                into
            }
            Place::High8(host) => {
                self.ubfx(Size::W, into, host, 8, 8);
                into
            }
            Place::Memory { address, width } => {
                self.load_memory(address, width, into);
                into
            }
        }
    }

    fn load_signed(&mut self, place: Place, into: Reg) -> Reg {
        let value = self.load(place, into);
        let width = place.width();
        if width != Width::Qword {
            self.sbfx(Size::X, into, value, 0, bits(width));
            into
        } else {
            value
        }
    }

    fn store(&mut self, place: Place, value: Reg) {
        match place {
            Place::Full(host) => {
                if host != value {
                    self.copy(Size::X, host, value);
                }
            }
            Place::Low32(host) => self.copy(Size::W, host, value),
            Place::Low16(host) => self.bfi(host, value, 0, 16),
            Place::Low8(host) => self.bfi(host, value, 0, 8),
            Place::High8(host) => self.bfi(host, value, 8, 8),
            Place::Memory { address, width } => self.store_memory(address, width, value),
        }
    }

    fn direct(place: Place) -> Option<(Reg, Size)> {
        match place {
            Place::Full(host) => Some((host, Size::X)),
            Place::Low32(host) => Some((host, Size::W)),
            _ => None,
        }
    }

    fn flip_carry(&mut self) {
        if self.mode.flagm {
            self.emit(encode::cfinv());
        } else {
            self.emit(encode::mrs_nzcv(SCRATCH));
            self.logic_immediate(Logic::Eor, Size::X, SCRATCH, SCRATCH, 1 << CARRY_BIT);
            self.emit(encode::msr_nzcv(SCRATCH));
        }
        self.carry = match self.carry {
            CarrySense::Direct => CarrySense::Inverted,
            CarrySense::Inverted => CarrySense::Direct,
        };
    }

    fn ensure_carry(&mut self, sense: CarrySense) {
        if self.carry != sense {
            self.flip_carry();
        }
    }

    fn set_carry_constant(&mut self, set: bool) -> Lowered {
        if self.live.any(Flags::CF) {
            self.emit(encode::mrs_nzcv(SCRATCH));
            let op = if set { Logic::Orr } else { Logic::And };
            let mask = if set {
                1u64 << CARRY_BIT
            } else {
                !(1u64 << CARRY_BIT)
            };
            self.logic_immediate(op, Size::X, SCRATCH, SCRATCH, mask);
            self.emit(encode::msr_nzcv(SCRATCH));
            self.carry = CarrySense::Direct;
        }
        Ok(())
    }

    fn set_flags(&mut self, result: Reg, width: Width, carry: Option<Reg>, overflow: Option<Reg>) {
        let shift = 32u8.saturating_sub(bits(width));
        self.emit(encode::add_sub_shifted(
            AddSub::Adds,
            size(width),
            Reg::ZR,
            Reg::ZR,
            result,
            Shift::Lsl,
            shift,
        ));
        self.carry = CarrySense::Direct;
        if carry.is_some() || overflow.is_some() {
            self.insert_carry_overflow(carry, overflow);
        }
    }

    fn insert_carry_overflow(&mut self, carry: Option<Reg>, overflow: Option<Reg>) {
        self.emit(encode::mrs_nzcv(T6));
        if let Some(carry) = carry {
            self.bfi(T6, carry, CARRY_BIT, 1);
        }
        if let Some(overflow) = overflow {
            self.bfi(T6, overflow, OVERFLOW_BIT, 1);
        }
        self.emit(encode::msr_nzcv(T6));
        if carry.is_some() {
            self.carry = CarrySense::Direct;
        }
    }

    fn save_lazy_flags(&mut self, need: Flags, result: Reg, first: Reg, second: Reg) {
        if need.any(Flags::PF) {
            self.emit(state_access(LoadStore::Strb, result, state::PARITY));
        }
        if need.any(Flags::AF) {
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::W,
                SCRATCH,
                first,
                second,
                Shift::Lsl,
                0,
            ));
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::W,
                SCRATCH,
                SCRATCH,
                result,
                Shift::Lsl,
                0,
            ));
            self.emit(state_access(LoadStore::Strb, SCRATCH, state::ADJUST));
        }
    }

    fn parity_odd(&mut self, into: Reg) {
        self.emit(state_access(LoadStore::Ldrb, into, state::PARITY));
        for amount in [4, 2, 1] {
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::W,
                into,
                into,
                into,
                Shift::Lsr,
                amount,
            ));
        }
    }

    fn condition(&mut self, condition: Condition) -> Test {
        let direct = self.carry == CarrySense::Direct;
        let cond = match condition {
            Condition::Overflow => Cond::Vs,
            Condition::NoOverflow => Cond::Vc,
            Condition::Below if direct => Cond::Cs,
            Condition::Below => Cond::Cc,
            Condition::AboveOrEqual if direct => Cond::Cc,
            Condition::AboveOrEqual => Cond::Cs,
            Condition::Equal => Cond::Eq,
            Condition::NotEqual => Cond::Ne,
            Condition::BelowOrEqual => {
                self.ensure_carry(CarrySense::Inverted);
                Cond::Ls
            }
            Condition::Above => {
                self.ensure_carry(CarrySense::Inverted);
                Cond::Hi
            }
            Condition::Sign => Cond::Mi,
            Condition::NoSign => Cond::Pl,
            Condition::Parity => return Test::Parity { set: true },
            Condition::NoParity => return Test::Parity { set: false },
            Condition::Less => Cond::Lt,
            Condition::GreaterOrEqual => Cond::Ge,
            Condition::LessOrEqual => Cond::Le,
            Condition::Greater => Cond::Gt,
        };
        Test::Cond(cond)
    }

    fn branch_if(&mut self, test: Test, label: Label) {
        match test {
            Test::Cond(cond) => self.asm.b_cond(cond, label),
            Test::Parity { set } => {
                self.parity_odd(SCRATCH);
                self.asm.tbz(SCRATCH, 0, label, !set);
            }
        }
    }

    fn exit(&mut self, kind: ExitKind) {
        let sense = self.carry;
        self.normalize_for_exit();
        self.exits.push((self.asm.position(), kind));
        self.emit(encode::brk(trap::UNLINKED));
        self.carry = sense;
    }

    fn normalize_for_exit(&mut self) {
        if self.live_out.any(Flags::CF) {
            self.ensure_carry(CarrySense::Direct);
        }
    }
}
