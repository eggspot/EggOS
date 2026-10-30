use super::{
    ADDRESS, AddSub, Bitfield, CarrySense, Flags, Instruction, Logic, Lowered, Lowerer, Mnemonic,
    Operand, Place, Reg, SCRATCH, Shift, Size, T0, T1, T2, T3, T4, T5, Width, bits, encode, size,
    trap, truncate,
};
use crate::codegen::aarch64::Cond;
use crate::codegen::aarch64::encode::{Carry, Multiply, TwoSource};
use crate::instruction::Gpr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Binary {
    Add,
    Sub,
    Adc,
    Sbb,
    And,
    Or,
    Xor,
}

impl Binary {
    fn logic(self) -> Option<Logic> {
        match self {
            Binary::And => Some(Logic::And),
            Binary::Or => Some(Logic::Orr),
            Binary::Xor => Some(Logic::Eor),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Reg(Reg),
    Imm(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Output {
    Flags,
    Value,
    Store(Place),
}

impl Lowerer {
    pub(super) fn source(&mut self, operand: &Operand, width: Width, into: Reg) -> Lowered<Source> {
        match *operand {
            Operand::Immediate(value) => Ok(Source::Imm(truncate(value, width))),
            ref other => {
                let place = self.place(other)?;
                Ok(Source::Reg(self.load(place, into)))
            }
        }
    }

    fn source_register(&mut self, source: Source, width: Width, into: Reg) -> Reg {
        match source {
            Source::Reg(register) => register,
            Source::Imm(value) => {
                self.move_immediate(size(width), into, value);
                into
            }
        }
    }

    pub(super) fn plain(&mut self, op: Binary, size: Size, d: Reg, a: Reg, b: Reg) {
        let word = match op {
            Binary::Add | Binary::Adc => {
                encode::add_sub_shifted(AddSub::Add, size, d, a, b, Shift::Lsl, 0)
            }
            Binary::Sub | Binary::Sbb => {
                encode::add_sub_shifted(AddSub::Sub, size, d, a, b, Shift::Lsl, 0)
            }
            Binary::And => encode::logical_shifted(Logic::And, false, size, d, a, b, Shift::Lsl, 0),
            Binary::Or => encode::logical_shifted(Logic::Orr, false, size, d, a, b, Shift::Lsl, 0),
            Binary::Xor => encode::logical_shifted(Logic::Eor, false, size, d, a, b, Shift::Lsl, 0),
        };
        self.emit(word);
    }

    pub(super) fn arith(
        &mut self,
        op: Binary,
        width: Width,
        a: Reg,
        b: Source,
        output: Output,
        need: Flags,
    ) -> Reg {
        let nzcv = need.any(Flags::NZCV);
        let lazy = need.any(Flags::PF.union(Flags::AF));
        if output == Output::Flags && need.is_empty() {
            return Reg::ZR;
        }
        match op {
            Binary::Adc => self.ensure_carry(CarrySense::Direct),
            Binary::Sbb => self.ensure_carry(CarrySense::Inverted),
            _ => {}
        }
        let b = if need.any(Flags::AF) {
            Source::Reg(self.source_register(b, width, T3))
        } else {
            b
        };
        let result = if width == Width::Dword || width == Width::Qword {
            self.arith_wide(op, width, a, b, output, need)
        } else {
            self.arith_narrow(op, width, a, b, output, nzcv, lazy)
        };
        if nzcv {
            self.carry = match op {
                Binary::Sub | Binary::Sbb => CarrySense::Inverted,
                _ => CarrySense::Direct,
            };
        }
        if lazy {
            let second = match b {
                Source::Reg(register) => register,
                Source::Imm(_) => Reg::ZR,
            };
            self.save_lazy_flags(need, result, a, second);
        }
        if let Output::Store(place) = output
            && Self::direct(place).map(|(host, _)| host) != Some(result)
        {
            self.store(place, result);
        }
        result
    }

    fn arith_wide(
        &mut self,
        op: Binary,
        width: Width,
        a: Reg,
        b: Source,
        output: Output,
        need: Flags,
    ) -> Reg {
        let size = size(width);
        let nzcv = need.any(Flags::NZCV);
        let lazy = need.any(Flags::PF.union(Flags::AF));
        let result = match output {
            Output::Flags if !lazy => Reg::ZR,
            Output::Store(place) if !need.any(Flags::AF) => match Self::direct(place) {
                Some((host, _)) => host,
                None => T2,
            },
            _ => T2,
        };
        match (op, b) {
            (Binary::Add | Binary::Sub, Source::Imm(value)) if value < 0x1000 => {
                let add_sub = match (op, nzcv) {
                    (Binary::Add, false) => AddSub::Add,
                    (Binary::Add, true) => AddSub::Adds,
                    (_, false) => AddSub::Sub,
                    (_, true) => AddSub::Subs,
                };
                let destination = if result == Reg::ZR && !nzcv {
                    T2
                } else {
                    result
                };
                self.emit(encode::add_sub_immediate(
                    add_sub,
                    size,
                    destination,
                    a,
                    value as u16,
                    false,
                ));
            }
            (Binary::Add | Binary::Sub, _) => {
                let b = self.source_register(b, width, T3);
                let add_sub = match (op, nzcv) {
                    (Binary::Add, false) => AddSub::Add,
                    (Binary::Add, true) => AddSub::Adds,
                    (_, false) => AddSub::Sub,
                    (_, true) => AddSub::Subs,
                };
                self.emit(encode::add_sub_shifted(
                    add_sub,
                    size,
                    result,
                    a,
                    b,
                    Shift::Lsl,
                    0,
                ));
            }
            (Binary::Adc | Binary::Sbb, _) => {
                let b = self.source_register(b, width, T3);
                let carry = match (op, nzcv) {
                    (Binary::Adc, false) => Carry::Adc,
                    (Binary::Adc, true) => Carry::Adcs,
                    (_, false) => Carry::Sbc,
                    (_, true) => Carry::Sbcs,
                };
                self.emit(encode::carry(carry, size, result, a, b));
            }
            (Binary::And, _) if nzcv => {
                match b {
                    Source::Imm(value) => self.logic_immediate(Logic::Ands, size, result, a, value),
                    Source::Reg(b) => self.emit(encode::logical_shifted(
                        Logic::Ands,
                        false,
                        size,
                        result,
                        a,
                        b,
                        Shift::Lsl,
                        0,
                    )),
                }
                return result;
            }
            _ => {
                let logic = op.logic().unwrap_or(Logic::And);
                let destination = if result == Reg::ZR { T2 } else { result };
                match b {
                    Source::Imm(value) => self.logic_immediate(logic, size, destination, a, value),
                    Source::Reg(b) => self.emit(encode::logical_shifted(
                        logic,
                        false,
                        size,
                        destination,
                        a,
                        b,
                        Shift::Lsl,
                        0,
                    )),
                }
                if nzcv {
                    self.emit(encode::add_sub_shifted(
                        AddSub::Adds,
                        size,
                        Reg::ZR,
                        Reg::ZR,
                        destination,
                        Shift::Lsl,
                        0,
                    ));
                }
                return destination;
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn arith_narrow(
        &mut self,
        op: Binary,
        width: Width,
        a: Reg,
        b: Source,
        output: Output,
        nzcv: bool,
        lazy: bool,
    ) -> Reg {
        let shift = 32 - bits(width);
        let b = self.source_register(b, width, T3);
        if !nzcv {
            match op {
                Binary::Adc => self.emit(encode::carry(Carry::Adc, Size::W, T2, a, b)),
                Binary::Sbb => self.emit(encode::carry(Carry::Sbc, Size::W, T2, a, b)),
                _ => self.plain(op, Size::W, T2, a, b),
            }
            return T2;
        }
        match op {
            Binary::Or | Binary::Xor => {
                self.plain(op, Size::W, T2, a, b);
                self.emit(encode::add_sub_shifted(
                    AddSub::Adds,
                    Size::W,
                    Reg::ZR,
                    Reg::ZR,
                    T2,
                    Shift::Lsl,
                    shift,
                ));
                return T2;
            }
            _ => {}
        }
        self.shift_left(Size::W, T2, a, shift);
        match op {
            Binary::Add | Binary::Sub => {
                let add_sub = if op == Binary::Add {
                    AddSub::Adds
                } else {
                    AddSub::Subs
                };
                self.emit(encode::add_sub_shifted(
                    add_sub,
                    Size::W,
                    T2,
                    T2,
                    b,
                    Shift::Lsl,
                    shift,
                ));
            }
            Binary::And => self.emit(encode::logical_shifted(
                Logic::Ands,
                false,
                Size::W,
                T2,
                T2,
                b,
                Shift::Lsl,
                shift,
            )),
            _ => {
                if op == Binary::Adc {
                    let low_ones = (1u64 << shift) - 1;
                    self.logic_immediate(Logic::Orr, Size::W, T2, T2, low_ones);
                }
                self.shift_left(Size::W, SCRATCH, b, shift);
                let carry = if op == Binary::Adc {
                    Carry::Adcs
                } else {
                    Carry::Sbcs
                };
                self.emit(encode::carry(carry, Size::W, T2, T2, SCRATCH));
            }
        }
        if output != Output::Flags || lazy {
            self.shift_right(Size::W, T2, T2, shift);
        }
        T2
    }

    pub(super) fn binary(&mut self, instruction: &Instruction) -> Lowered {
        let (op, keep) = match instruction.mnemonic {
            Mnemonic::Add => (Binary::Add, true),
            Mnemonic::Sub => (Binary::Sub, true),
            Mnemonic::Adc => (Binary::Adc, true),
            Mnemonic::Sbb => (Binary::Sbb, true),
            Mnemonic::And => (Binary::And, true),
            Mnemonic::Or => (Binary::Or, true),
            Mnemonic::Xor => (Binary::Xor, true),
            Mnemonic::Cmp => (Binary::Sub, false),
            _ => (Binary::And, false),
        };
        let need = self.defined_and_live(instruction);
        let destination = self.operand(instruction, 0)?;
        let place = self.place(&destination)?;
        let width = place.width();
        if instruction.lock {
            let b = self.source(&self.operand(instruction, 1)?, width, T1)?;
            return self.locked_update(op, place, b, need);
        }
        let a = self.load(place, T0);
        let b = self.source(&self.operand(instruction, 1)?, width, T1)?;
        if !keep && need.is_empty() {
            return Ok(());
        }
        let output = if keep {
            Output::Store(place)
        } else {
            Output::Flags
        };
        self.arith(op, width, a, b, output, need);
        Ok(())
    }

    pub(super) fn step(&mut self, instruction: &Instruction) -> Lowered {
        let op = if instruction.mnemonic == Mnemonic::Inc {
            Binary::Add
        } else {
            Binary::Sub
        };
        let need = self.defined_and_live(instruction);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let keep_carry = need.any(Flags::NZCV) && self.live.any(Flags::CF);
        let sense = self.carry;
        if keep_carry {
            self.emit(encode::mrs_nzcv(T4));
        }
        if instruction.lock {
            self.locked_update(op, place, Source::Imm(1), need)?;
        } else {
            let a = self.load(place, T0);
            self.arith(
                op,
                place.width(),
                a,
                Source::Imm(1),
                Output::Store(place),
                need,
            );
        }
        if keep_carry {
            self.emit(encode::mrs_nzcv(T5));
            self.ubfx(Size::X, T4, T4, super::CARRY_BIT, 1);
            self.bfi(T5, T4, super::CARRY_BIT, 1);
            self.emit(encode::msr_nzcv(T5));
            self.carry = sense;
        }
        Ok(())
    }

    pub(super) fn negate(&mut self, instruction: &Instruction) -> Lowered {
        if instruction.lock {
            return Err(super::LowerErrorKind::Unhandled(Mnemonic::Neg));
        }
        let need = self.defined_and_live(instruction);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let value = self.load(place, T0);
        let output = Output::Store(place);
        self.arith(
            Binary::Sub,
            place.width(),
            Reg::ZR,
            Source::Reg(value),
            output,
            need,
        );
        Ok(())
    }

    pub(super) fn not(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        if instruction.lock {
            let ones = truncate(-1, width);
            return self.locked_update(Binary::Xor, place, Source::Imm(ones), Flags::NONE);
        }
        let value = self.load(place, T0);
        let (result, size) = Self::direct(place).unwrap_or((T2, Size::W));
        self.emit(encode::logical_shifted(
            Logic::Orr,
            true,
            size,
            result,
            Reg::ZR,
            value,
            Shift::Lsl,
            0,
        ));
        if Self::direct(place).is_none() {
            self.store(place, result);
        }
        Ok(())
    }

    pub(super) fn mov(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        match self.operand(instruction, 1)? {
            Operand::Immediate(value) => {
                let value = truncate(value, width);
                match Self::direct(place) {
                    Some((host, size)) => self.move_immediate(size, host, value),
                    None => {
                        let register = self.source_register(Source::Imm(value), width, T1);
                        self.store(place, register);
                    }
                }
            }
            source => {
                if let (Some((host, _)), Operand::Memory(_)) = (Self::direct(place), source) {
                    let from = self.place(&source)?;
                    self.load(from, host);
                    return Ok(());
                }
                let from = self.place(&source)?;
                let value = self.load(from, T1);
                self.store(place, value);
            }
        }
        Ok(())
    }

    pub(super) fn extend(&mut self, instruction: &Instruction, signed: bool) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let from = self.place(&self.operand(instruction, 1)?)?;
        let value = if signed {
            self.load_signed(from, T1)
        } else {
            let value = self.load(from, T1);
            if from.width() == Width::Dword {
                self.copy(Size::W, T1, value);
                T1
            } else {
                value
            }
        };
        match Self::direct(place) {
            Some((host, size)) => self.copy(size, host, value),
            None => self.store(place, value),
        }
        Ok(())
    }

    pub(super) fn lea(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let Operand::Memory(memory) = self.operand(instruction, 1)? else {
            return Err(self.operands_error());
        };
        let address = self.effective_address(&memory, ADDRESS, None);
        self.store(place, address);
        Ok(())
    }

    pub(super) fn extend_accumulator(&mut self, width: Width) -> Lowered {
        let rax = Reg(Gpr::RAX.0);
        match width {
            Width::Word => {
                self.sbfx(Size::W, T0, rax, 0, 8);
                self.bfi(rax, T0, 0, 16);
            }
            Width::Dword => self.sbfx(Size::W, rax, rax, 0, 16),
            _ => self.sbfx(Size::X, rax, rax, 0, 32),
        }
        Ok(())
    }

    pub(super) fn extend_into_rdx(&mut self, width: Width) -> Lowered {
        let rax = Reg(Gpr::RAX.0);
        let rdx = Reg(Gpr::RDX.0);
        match width {
            Width::Word => {
                self.sbfx(Size::W, T0, rax, 15, 1);
                self.bfi(rdx, T0, 0, 16);
            }
            Width::Dword => self.emit(encode::bitfield(Bitfield::Sbfm, Size::W, rdx, rax, 31, 31)),
            _ => self.emit(encode::bitfield(Bitfield::Sbfm, Size::X, rdx, rax, 63, 63)),
        }
        Ok(())
    }

    pub(super) fn multiply(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let signed = instruction.mnemonic == Mnemonic::Imul;
        let operands = instruction.operands();
        if operands.len() >= 2 {
            return self.multiply_truncated(instruction, need);
        }
        let from = self.place(&self.operand(instruction, 0)?)?;
        let width = from.width();
        let rax = Reg(Gpr::RAX.0);
        let rdx = Reg(Gpr::RDX.0);
        let accumulator = Place::register(Gpr::RAX, width);
        let (a, b) = if signed {
            let b = self.load_signed(from, T1);
            (self.load_signed(accumulator, T0), b)
        } else {
            let b = self.load(from, T1);
            (self.load(accumulator, T0), b)
        };
        match width {
            Width::Byte | Width::Word => {
                self.emit(encode::multiply(Multiply::Madd, Size::W, T2, a, b, Reg::ZR));
                self.bfi(rax, T2, 0, 16);
                if width == Width::Word {
                    self.shift_right(Size::W, T3, T2, 16);
                    self.bfi(rdx, T3, 0, 16);
                }
                if !need.is_empty() {
                    self.overflow_bit(T4, T2, bits(width), signed, Size::W);
                    self.set_flags(T2, width, Some(T4), Some(T4));
                }
            }
            Width::Dword => {
                let op = if signed {
                    Multiply::Smaddl
                } else {
                    Multiply::Umaddl
                };
                self.emit(encode::multiply(op, Size::X, T2, a, b, Reg::ZR));
                self.copy(Size::W, rax, T2);
                self.shift_right(Size::X, rdx, T2, 32);
                if !need.is_empty() {
                    self.overflow_bit(T4, T2, 32, signed, Size::X);
                    self.set_flags(rax, width, Some(T4), Some(T4));
                }
            }
            _ => {
                let high = if signed {
                    Multiply::Smulh
                } else {
                    Multiply::Umulh
                };
                self.emit(encode::multiply(high, Size::X, T3, a, b, Reg::ZR));
                self.emit(encode::multiply(Multiply::Madd, Size::X, T2, a, b, Reg::ZR));
                self.copy(Size::X, rax, T2);
                self.copy(Size::X, rdx, T3);
                if !need.is_empty() {
                    self.wide_overflow_bit(T4, T2, T3, signed);
                    self.set_flags(rax, width, Some(T4), Some(T4));
                }
            }
        }
        Ok(())
    }

    fn overflow_bit(&mut self, into: Reg, product: Reg, half: u8, signed: bool, size: Size) {
        if signed {
            self.sbfx(size, into, product, 0, half);
        } else {
            self.ubfx(size, into, product, 0, half);
        }
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            size,
            Reg::ZR,
            into,
            product,
            Shift::Lsl,
            0,
        ));
        self.emit(encode::cset(Size::W, into, Cond::Ne));
    }

    fn wide_overflow_bit(&mut self, into: Reg, low: Reg, high: Reg, signed: bool) {
        let shift = if signed { Shift::Asr } else { Shift::Lsl };
        let (n, amount) = if signed { (low, 63) } else { (Reg::ZR, 0) };
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            high,
            n,
            shift,
            amount,
        ));
        self.emit(encode::cset(Size::W, into, Cond::Ne));
    }

    fn multiply_truncated(&mut self, instruction: &Instruction, need: Flags) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        let (first, second) = if instruction.operands().len() == 3 {
            (self.operand(instruction, 1)?, self.operand(instruction, 2)?)
        } else {
            (self.operand(instruction, 0)?, self.operand(instruction, 1)?)
        };
        let first = self.place(&first)?;
        let a = self.load_signed(first, T0);
        let b = match self.source(&second, width, T1)? {
            Source::Imm(value) => {
                let value = truncate(value as i64, width);
                self.move_immediate(Size::X, T1, value);
                if width != Width::Qword {
                    self.sbfx(Size::X, T1, T1, 0, bits(width));
                }
                T1
            }
            Source::Reg(register) => {
                if width != Width::Qword {
                    self.sbfx(Size::X, T1, register, 0, bits(width));
                    T1
                } else {
                    register
                }
            }
        };
        if width == Width::Qword {
            if !need.is_empty() {
                self.emit(encode::multiply(
                    Multiply::Smulh,
                    Size::X,
                    T3,
                    a,
                    b,
                    Reg::ZR,
                ));
            }
            self.emit(encode::multiply(Multiply::Madd, Size::X, T2, a, b, Reg::ZR));
            if !need.is_empty() {
                self.wide_overflow_bit(T4, T2, T3, true);
            }
        } else {
            self.emit(encode::multiply(Multiply::Madd, Size::X, T2, a, b, Reg::ZR));
            if !need.is_empty() {
                self.overflow_bit(T4, T2, bits(width), true, Size::X);
            }
        }
        self.store(place, T2);
        if !need.is_empty() {
            self.set_flags(T2, width, Some(T4), Some(T4));
        }
        Ok(())
    }

    pub(super) fn divide(&mut self, instruction: &Instruction) -> Lowered {
        let signed = instruction.mnemonic == Mnemonic::Idiv;
        let from = self.place(&self.operand(instruction, 0)?)?;
        let width = from.width();
        let divisor = if signed {
            self.load_signed(from, T5)
        } else {
            let value = self.load(from, T5);
            if width == Width::Dword {
                self.copy(Size::W, T5, value);
                T5
            } else {
                value
            }
        };
        if divisor != T5 {
            self.copy(Size::X, T5, divisor);
        }
        let fault = self.asm.label();
        let done = self.asm.label();
        self.asm.cbz(Size::X, T5, fault, false);
        if width == Width::Qword {
            self.divide_wide(signed, fault);
        } else {
            self.divide_narrow(width, signed, fault);
        }
        self.asm.b(done);
        self.asm.bind(fault);
        self.emit(encode::brk(trap::DIVIDE_ERROR));
        self.asm.bind(done);
        Ok(())
    }

    fn divide_narrow(&mut self, width: Width, signed: bool, fault: super::Label) {
        let rax = Reg(Gpr::RAX.0);
        let rdx = Reg(Gpr::RDX.0);
        let half = bits(width);
        if width == Width::Byte {
            self.ubfx(Size::X, T0, rax, 0, 16);
        } else {
            self.ubfx(Size::X, T0, rax, 0, half);
            self.bfi(T0, rdx, half, half);
        }
        let dividend_bits = if width == Width::Byte { 16 } else { half * 2 };
        if signed {
            self.sbfx(Size::X, T0, T0, 0, dividend_bits);
        }
        let op = if signed {
            TwoSource::Sdiv
        } else {
            TwoSource::Udiv
        };
        self.emit(encode::two_source(op, Size::X, T2, T0, T5));
        self.emit(encode::multiply(Multiply::Msub, Size::X, T3, T2, T5, T0));
        if signed {
            self.sbfx(Size::X, SCRATCH, T2, 0, half);
        } else {
            self.ubfx(Size::X, SCRATCH, T2, 0, half);
        }
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            SCRATCH,
            T2,
            Shift::Lsl,
            0,
        ));
        self.asm.b_cond(Cond::Ne, fault);
        match width {
            Width::Byte => {
                self.bfi(rax, T2, 0, 8);
                self.bfi(rax, T3, 8, 8);
            }
            Width::Word => {
                self.bfi(rax, T2, 0, 16);
                self.bfi(rdx, T3, 0, 16);
            }
            _ => {
                self.copy(Size::W, rax, T2);
                self.copy(Size::W, rdx, T3);
            }
        }
    }

    fn divide_wide(&mut self, signed: bool, fault: super::Label) {
        let rax = Reg(Gpr::RAX.0);
        let rdx = Reg(Gpr::RDX.0);
        let slow = self.asm.label();
        let finish = self.asm.label();
        if signed {
            self.emit(encode::add_sub_shifted(
                AddSub::Subs,
                Size::X,
                Reg::ZR,
                rdx,
                rax,
                Shift::Asr,
                63,
            ));
        } else {
            self.emit(encode::add_sub_shifted(
                AddSub::Subs,
                Size::X,
                Reg::ZR,
                rdx,
                Reg::ZR,
                Shift::Lsl,
                0,
            ));
        }
        self.asm.b_cond(Cond::Ne, slow);
        if signed {
            let fits = self.asm.label();
            self.emit(encode::add_sub_immediate(
                AddSub::Adds,
                Size::X,
                Reg::ZR,
                T5,
                1,
                false,
            ));
            self.asm.b_cond(Cond::Ne, fits);
            self.move_immediate(Size::X, SCRATCH, 1 << 63);
            self.emit(encode::add_sub_shifted(
                AddSub::Subs,
                Size::X,
                Reg::ZR,
                rax,
                SCRATCH,
                Shift::Lsl,
                0,
            ));
            self.asm.b_cond(Cond::Eq, fault);
            self.asm.bind(fits);
        }
        let op = if signed {
            TwoSource::Sdiv
        } else {
            TwoSource::Udiv
        };
        self.emit(encode::two_source(op, Size::X, T2, rax, T5));
        self.emit(encode::multiply(Multiply::Msub, Size::X, rdx, T2, T5, rax));
        self.copy(Size::X, rax, T2);
        self.asm.b(finish);
        self.asm.bind(slow);
        self.copy(Size::X, T0, rdx);
        self.copy(Size::X, T1, rax);
        if signed {
            self.divide_signed_128(fault);
        } else {
            self.emit(encode::add_sub_shifted(
                AddSub::Subs,
                Size::X,
                Reg::ZR,
                T0,
                T5,
                Shift::Lsl,
                0,
            ));
            self.asm.b_cond(Cond::Cs, fault);
            self.divide_128();
            self.copy(Size::X, rax, T1);
            self.copy(Size::X, rdx, T0);
        }
        self.asm.bind(finish);
    }

    fn divide_128(&mut self) {
        let top = self.asm.label();
        let subtract = self.asm.label();
        let next = self.asm.label();
        self.move_immediate(Size::X, T2, 64);
        self.asm.bind(top);
        self.shift_right(Size::X, T3, T0, 63);
        self.emit(encode::extr(Size::X, T0, T0, T1, 63));
        self.shift_left(Size::X, T1, T1, 1);
        self.asm.cbz(Size::X, T3, subtract, true);
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            T0,
            T5,
            Shift::Lsl,
            0,
        ));
        self.asm.b_cond(Cond::Cc, next);
        self.asm.bind(subtract);
        self.emit(encode::add_sub_shifted(
            AddSub::Sub,
            Size::X,
            T0,
            T0,
            T5,
            Shift::Lsl,
            0,
        ));
        self.logic_immediate(Logic::Orr, Size::X, T1, T1, 1);
        self.asm.bind(next);
        self.emit(encode::add_sub_immediate(
            AddSub::Subs,
            Size::X,
            T2,
            T2,
            1,
            false,
        ));
        self.asm.b_cond(Cond::Ne, top);
    }

    fn divide_signed_128(&mut self, fault: super::Label) {
        let rax = Reg(Gpr::RAX.0);
        let rdx = Reg(Gpr::RDX.0);
        let dividend_positive = self.asm.label();
        self.shift_right(Size::X, T4, T0, 63);
        self.asm.cbz(Size::X, T4, dividend_positive, false);
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            T1,
            Reg::ZR,
            T1,
            Shift::Lsl,
            0,
        ));
        self.emit(encode::carry(Carry::Sbc, Size::X, T0, Reg::ZR, T0));
        self.asm.bind(dividend_positive);
        self.shift_right(Size::X, SCRATCH, T5, 63);
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            T5,
            Reg::ZR,
            Shift::Lsl,
            0,
        ));
        self.emit(encode::select(
            encode::Select::Csneg,
            Size::X,
            T5,
            T5,
            T5,
            Cond::Ge,
        ));
        self.emit(encode::logical_shifted(
            Logic::Eor,
            false,
            Size::X,
            SCRATCH,
            SCRATCH,
            T4,
            Shift::Lsl,
            0,
        ));
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            T0,
            T5,
            Shift::Lsl,
            0,
        ));
        self.asm.b_cond(Cond::Cs, fault);
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            rax,
            Reg::ZR,
            SCRATCH,
            Shift::Lsl,
            0,
        ));
        self.divide_128();
        let quotient_positive = self.asm.label();
        let quotient_done = self.asm.label();
        self.asm.cbz(Size::X, rax, quotient_positive, false);
        self.move_immediate(Size::X, SCRATCH, 1 << 63);
        self.emit(encode::add_sub_shifted(
            AddSub::Subs,
            Size::X,
            Reg::ZR,
            T1,
            SCRATCH,
            Shift::Lsl,
            0,
        ));
        self.asm.b_cond(Cond::Hi, fault);
        self.emit(encode::add_sub_shifted(
            AddSub::Sub,
            Size::X,
            rax,
            Reg::ZR,
            T1,
            Shift::Lsl,
            0,
        ));
        self.asm.b(quotient_done);
        self.asm.bind(quotient_positive);
        self.asm.tbz(T1, 63, fault, true);
        self.copy(Size::X, rax, T1);
        self.asm.bind(quotient_done);
        let remainder_positive = self.asm.label();
        self.copy(Size::X, rdx, T0);
        self.asm.cbz(Size::X, T4, remainder_positive, false);
        self.emit(encode::add_sub_shifted(
            AddSub::Sub,
            Size::X,
            rdx,
            Reg::ZR,
            T0,
            Shift::Lsl,
            0,
        ));
        self.asm.bind(remainder_positive);
    }
}
