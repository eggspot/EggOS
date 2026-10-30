use super::arith::{Binary, Source};
use super::{
    AddSub, CarrySense, Flags, Instruction, Logic, Lowered, Lowerer, Mnemonic, Operand, Place, Reg,
    SCRATCH, Shift, Size, T0, T1, T2, T3, T4, T5, Width, bits, encode, size,
};
use crate::codegen::aarch64::encode::{Atomic, Bitfield, OneSource, TwoSource};
use crate::codegen::flags;
use crate::codegen::registers::{ADDRESS, VECTOR_TEMPORARY};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Count {
    Immediate(u8),
    Cl,
}

impl Lowerer {
    fn count(instruction: &Instruction) -> Count {
        match flags::immediate_count(instruction) {
            Some(count) => Count::Immediate(count),
            None => Count::Cl,
        }
    }

    fn masked_cl(&mut self, width: Width) -> Reg {
        let mask = if width == Width::Qword { 0x3F } else { 0x1F };
        self.logic_immediate(Logic::And, Size::W, T1, Reg(1), mask);
        T1
    }

    fn msb(&mut self, into: Reg, value: Reg, width: Width) {
        self.ubfx(Size::X, into, value, bits(width) - 1, 1);
    }

    fn xor_bits(&mut self, d: Reg, a: Reg, b: Reg) {
        self.emit(encode::logical_shifted(
            Logic::Eor,
            false,
            Size::W,
            d,
            a,
            b,
            Shift::Lsl,
            0,
        ));
    }

    pub(super) fn shift(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        let need = self.defined_and_live(instruction);
        let mnemonic = instruction.mnemonic;
        let count = Self::count(instruction);
        if count == Count::Immediate(0) {
            return Ok(());
        }
        let done = self.asm.label();
        let count_register = match count {
            Count::Immediate(_) => Reg::ZR,
            Count::Cl => {
                if !need.is_empty() {
                    self.ensure_carry(CarrySense::Direct);
                }
                let count = self.masked_cl(width);
                self.asm.cbz(Size::W, count, done, false);
                count
            }
        };
        let a = if mnemonic == Mnemonic::Sar {
            self.load_signed(place, T0)
        } else {
            self.load(place, T0)
        };
        let operation = if width == Width::Qword {
            Size::X
        } else {
            Size::W
        };
        let (result, direct) = match Self::direct(place) {
            Some((host, _)) => (host, true),
            None => (T2, false),
        };
        let result = if need.is_empty() || !direct || result != a {
            result
        } else {
            T2
        };
        match (mnemonic, count) {
            (Mnemonic::Shl, Count::Immediate(c)) => self.shift_left(operation, result, a, c),
            (Mnemonic::Shr, Count::Immediate(c)) => self.shift_right(operation, result, a, c),
            (_, Count::Immediate(c)) => {
                let total = operation.bits() as u8;
                self.emit(encode::bitfield(
                    Bitfield::Sbfm,
                    operation,
                    result,
                    a,
                    c,
                    total - 1,
                ));
            }
            (_, Count::Cl) => {
                let op = match mnemonic {
                    Mnemonic::Shl => TwoSource::Lslv,
                    Mnemonic::Shr => TwoSource::Lsrv,
                    _ => TwoSource::Asrv,
                };
                self.emit(encode::two_source(op, operation, result, a, count_register));
            }
        }
        if !need.is_empty() {
            self.shift_flags(mnemonic, width, a, result, count, count_register, need);
        }
        if !(direct && Self::direct(place).map(|(host, _)| host) == Some(result)) {
            self.store(place, result);
        }
        self.asm.bind(done);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn shift_flags(
        &mut self,
        mnemonic: Mnemonic,
        width: Width,
        a: Reg,
        result: Reg,
        count: Count,
        count_register: Reg,
        need: Flags,
    ) {
        let total = bits(width);
        let signed = mnemonic == Mnemonic::Sar;
        match (mnemonic, count) {
            (Mnemonic::Shl, Count::Immediate(c)) if c <= total => {
                self.ubfx(Size::X, T4, a, total - c, 1);
            }
            (Mnemonic::Shl, Count::Immediate(_)) => {
                self.copy(Size::X, T4, Reg::ZR);
            }
            (_, Count::Immediate(c)) => self.ubfx(Size::X, T4, a, c - 1, 1),
            (Mnemonic::Shl, Count::Cl) => {
                self.move_immediate(Size::W, T5, u64::from(total));
                self.emit(encode::add_sub_shifted(
                    AddSub::Sub,
                    Size::W,
                    T5,
                    T5,
                    count_register,
                    Shift::Lsl,
                    0,
                ));
                self.emit(encode::two_source(TwoSource::Lsrv, Size::X, T4, a, T5));
                self.logic_immediate(Logic::And, Size::X, T4, T4, 1);
            }
            (_, Count::Cl) => {
                self.emit(encode::add_sub_immediate(
                    AddSub::Sub,
                    Size::W,
                    T5,
                    count_register,
                    1,
                    false,
                ));
                let op = if signed {
                    TwoSource::Asrv
                } else {
                    TwoSource::Lsrv
                };
                self.emit(encode::two_source(op, Size::X, T4, a, T5));
                self.logic_immediate(Logic::And, Size::X, T4, T4, 1);
            }
        }
        let overflow = match (mnemonic, count) {
            (_, Count::Immediate(c)) if c != 1 => None,
            (Mnemonic::Shl, _) => {
                self.msb(T5, result, width);
                self.xor_bits(T5, T5, T4);
                Some(T5)
            }
            (Mnemonic::Shr, _) => {
                self.msb(T5, a, width);
                Some(T5)
            }
            _ => None,
        };
        self.set_flags(result, width, Some(T4), overflow);
        if need.any(Flags::PF) {
            self.save_lazy_flags(Flags::PF, result, Reg::ZR, Reg::ZR);
        }
    }

    pub(super) fn rotate(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        let need = self.defined_and_live(instruction);
        let left = instruction.mnemonic == Mnemonic::Rol;
        let count = Self::count(instruction);
        if count == Count::Immediate(0) {
            return Ok(());
        }
        let done = self.asm.label();
        let count_register = match count {
            Count::Immediate(_) => Reg::ZR,
            Count::Cl => {
                if !need.is_empty() {
                    self.ensure_carry(CarrySense::Direct);
                }
                let count = self.masked_cl(width);
                self.asm.cbz(Size::W, count, done, false);
                count
            }
        };
        let a = self.load(place, T0);
        let (operand, operation) = match width {
            Width::Byte => {
                self.emit(encode::logical_shifted(
                    Logic::Orr,
                    false,
                    Size::W,
                    T3,
                    a,
                    a,
                    Shift::Lsl,
                    8,
                ));
                self.emit(encode::logical_shifted(
                    Logic::Orr,
                    false,
                    Size::W,
                    T3,
                    T3,
                    T3,
                    Shift::Lsl,
                    16,
                ));
                (T3, Size::W)
            }
            Width::Word => {
                self.emit(encode::logical_shifted(
                    Logic::Orr,
                    false,
                    Size::W,
                    T3,
                    a,
                    a,
                    Shift::Lsl,
                    16,
                ));
                (T3, Size::W)
            }
            _ => (a, size(width)),
        };
        let total = operation.bits() as u8;
        match count {
            Count::Immediate(c) => {
                let c = c % total;
                let amount = if left { (total - c) % total } else { c };
                self.emit(encode::extr(operation, T2, operand, operand, amount));
            }
            Count::Cl => {
                let amount = if left {
                    self.emit(encode::add_sub_shifted(
                        AddSub::Sub,
                        Size::W,
                        T5,
                        Reg::ZR,
                        count_register,
                        Shift::Lsl,
                        0,
                    ));
                    T5
                } else {
                    count_register
                };
                self.emit(encode::two_source(
                    TwoSource::Rorv,
                    operation,
                    T2,
                    operand,
                    amount,
                ));
            }
        }
        self.store(place, T2);
        if need.any(Flags::CF.union(Flags::OF)) {
            if left {
                self.ubfx(Size::X, T4, T2, 0, 1);
                self.msb(T5, T2, width);
                self.xor_bits(T5, T5, T4);
            } else {
                self.msb(T4, T2, width);
                self.ubfx(Size::X, T5, T2, bits(width) - 2, 1);
                self.xor_bits(T5, T5, T4);
            }
            self.insert_carry_overflow(Some(T4), Some(T5));
        }
        self.asm.bind(done);
        Ok(())
    }

    pub(super) fn double_shift(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let width = place.width();
        if width == Width::Word {
            return Err(self.operands_error());
        }
        let left = instruction.mnemonic == Mnemonic::Shld;
        let need = self.defined_and_live(instruction);
        let count = Self::count(instruction);
        if count == Count::Immediate(0) {
            return Ok(());
        }
        let done = self.asm.label();
        let count_register = match count {
            Count::Immediate(_) => Reg::ZR,
            Count::Cl => {
                if !need.is_empty() {
                    self.ensure_carry(CarrySense::Direct);
                }
                let count = self.masked_cl(width);
                self.asm.cbz(Size::W, count, done, false);
                count
            }
        };
        let a = self.load(place, T0);
        let source = self.place(&self.operand(instruction, 1)?)?;
        let fill = self.load(source, T3);
        let operation = size(width);
        let total = bits(width);
        match count {
            Count::Immediate(c) => {
                if left {
                    self.emit(encode::extr(operation, T2, a, fill, total - c));
                } else {
                    self.emit(encode::extr(operation, T2, fill, a, c));
                }
            }
            Count::Cl => {
                let (first, second) = if left {
                    (TwoSource::Lslv, TwoSource::Lsrv)
                } else {
                    (TwoSource::Lsrv, TwoSource::Lslv)
                };
                self.emit(encode::two_source(first, operation, T2, a, count_register));
                self.emit(encode::add_sub_shifted(
                    AddSub::Sub,
                    Size::W,
                    T5,
                    Reg::ZR,
                    count_register,
                    Shift::Lsl,
                    0,
                ));
                self.emit(encode::two_source(second, operation, T5, fill, T5));
                self.plain(Binary::Or, operation, T2, T2, T5);
            }
        }
        if !need.is_empty() {
            let shift = if left { Mnemonic::Shl } else { Mnemonic::Shr };
            self.shift_flags(shift, width, a, T2, count, count_register, need);
            if count == Count::Immediate(1) || count == Count::Cl {
                self.msb(T5, T2, width);
                self.msb(SCRATCH, a, width);
                self.xor_bits(T5, T5, SCRATCH);
                self.insert_carry_overflow(None, Some(T5));
            }
        }
        self.store(place, T2);
        self.asm.bind(done);
        Ok(())
    }

    pub(super) fn bit_test(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let destination = self.operand(instruction, 0)?;
        let width = instruction.width;
        let total = bits(width);
        let operation = size(width);
        let place = match (destination, self.operand(instruction, 1)?) {
            (Operand::Memory(_), Operand::Register(_)) => {
                let index = self.place(&self.operand(instruction, 1)?)?;
                let offset = self.load_signed(index, T1);
                let memory = self.place(&destination)?;
                let Place::Memory { address, .. } = memory else {
                    return Err(self.operands_error());
                };
                self.emit(encode::bitfield(
                    Bitfield::Sbfm,
                    Size::X,
                    SCRATCH,
                    offset,
                    total.trailing_zeros() as u8,
                    63,
                ));
                self.emit(encode::add_sub_shifted(
                    AddSub::Add,
                    Size::X,
                    ADDRESS,
                    address,
                    SCRATCH,
                    Shift::Lsl,
                    width.bytes().trailing_zeros() as u8,
                ));
                Place::Memory {
                    address: ADDRESS,
                    width,
                }
            }
            _ => self.place(&destination)?,
        };
        match self.source(&self.operand(instruction, 1)?, width, T1)? {
            Source::Imm(index) => {
                self.move_immediate(Size::W, T1, index % u64::from(total));
            }
            Source::Reg(index) => {
                self.logic_immediate(Logic::And, Size::W, T1, index, u64::from(total - 1));
            }
        }
        let modify = match instruction.mnemonic {
            Mnemonic::Bts => Some((Logic::Orr, false, Atomic::Ldset)),
            Mnemonic::Btr => Some((Logic::And, true, Atomic::Ldclr)),
            Mnemonic::Btc => Some((Logic::Eor, false, Atomic::Ldeor)),
            _ => None,
        };
        if let Some((logic, negate, atomic)) = modify {
            self.move_immediate(Size::X, T3, 1);
            self.emit(encode::two_source(TwoSource::Lslv, operation, T3, T3, T1));
            if instruction.lock
                && let Place::Memory { address, width } = place
            {
                if self.mode.lse {
                    self.emit(encode::atomic(
                        atomic,
                        super::access(width),
                        T3,
                        T0,
                        address,
                    ));
                } else {
                    let top = self.asm.label();
                    self.asm.bind(top);
                    self.emit(encode::ordered(
                        encode::Ordered::Ldaxr,
                        super::access(width),
                        T0,
                        address,
                    ));
                    self.emit(encode::logical_shifted(
                        logic,
                        negate,
                        operation,
                        T2,
                        T0,
                        T3,
                        Shift::Lsl,
                        0,
                    ));
                    self.emit(encode::stlxr(super::access(width), SCRATCH, T2, address));
                    self.asm.cbz(Size::W, SCRATCH, top, true);
                }
            } else {
                let value = self.load(place, T0);
                if value != T0 {
                    self.copy(Size::X, T0, value);
                }
                self.emit(encode::logical_shifted(
                    logic,
                    negate,
                    operation,
                    T2,
                    T0,
                    T3,
                    Shift::Lsl,
                    0,
                ));
                self.store(place, T2);
            }
        } else {
            let value = self.load(place, T0);
            if value != T0 {
                self.copy(Size::X, T0, value);
            }
        }
        if need.any(Flags::CF) {
            self.emit(encode::two_source(TwoSource::Lsrv, operation, T4, T0, T1));
            self.insert_carry_overflow(Some(T4), None);
        }
        Ok(())
    }

    pub(super) fn bit_scan(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let source = self.place(&self.operand(instruction, 1)?)?;
        let width = place.width();
        let operation = size(width);
        let value = self.load(source, T1);
        if need.any(Flags::ZF) {
            self.set_flags(value, width, None, None);
        }
        let zero = self.asm.label();
        self.asm.cbz(operation, value, zero, false);
        if instruction.mnemonic == Mnemonic::Bsr {
            self.emit(encode::one_source(OneSource::Clz, operation, T2, value));
            let top = u64::from(operation.bits() - 1);
            self.logic_immediate(Logic::Eor, operation, T2, T2, top);
        } else {
            self.emit(encode::one_source(OneSource::Rbit, operation, T2, value));
            self.emit(encode::one_source(OneSource::Clz, operation, T2, T2));
        }
        self.store(place, T2);
        self.asm.bind(zero);
        Ok(())
    }

    pub(super) fn leading_zeros(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let source = self.place(&self.operand(instruction, 1)?)?;
        let width = place.width();
        let operation = size(width);
        let value = self.load(source, T1);
        self.emit(encode::one_source(OneSource::Clz, operation, T2, value));
        if width == Width::Word {
            self.emit(encode::add_sub_immediate(
                AddSub::Sub,
                Size::W,
                T2,
                T2,
                16,
                false,
            ));
        }
        if !need.is_empty() {
            self.emit(encode::add_sub_shifted(
                AddSub::Subs,
                operation,
                Reg::ZR,
                value,
                Reg::ZR,
                Shift::Lsl,
                0,
            ));
            self.emit(encode::cset(Size::W, T4, super::Cond::Eq));
            self.set_flags(T2, width, Some(T4), None);
        }
        self.store(place, T2);
        Ok(())
    }

    pub(super) fn population_count(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let source = self.place(&self.operand(instruction, 1)?)?;
        let width = place.width();
        let value = self.load(source, T1);
        let value = if width == Width::Dword {
            self.copy(Size::W, T1, value);
            T1
        } else {
            value
        };
        self.emit(encode::fmov_d_from_x(VECTOR_TEMPORARY, value));
        self.emit(encode::cnt_8b(VECTOR_TEMPORARY, VECTOR_TEMPORARY));
        self.emit(encode::addv_8b(VECTOR_TEMPORARY, VECTOR_TEMPORARY));
        self.emit(encode::fmov_w_from_s(T2, VECTOR_TEMPORARY));
        if !need.is_empty() {
            self.set_flags(T2, width, None, None);
            if need.any(Flags::PF) {
                self.move_immediate(Size::W, SCRATCH, 1);
                self.save_lazy_flags(Flags::PF, SCRATCH, Reg::ZR, Reg::ZR);
            }
        }
        self.store(place, T2);
        Ok(())
    }

    pub(super) fn byte_swap(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        match place {
            Place::Full(host) => self.emit(encode::one_source(OneSource::Rev, Size::X, host, host)),
            Place::Low32(host) => {
                self.emit(encode::one_source(OneSource::Rev, Size::W, host, host))
            }
            Place::Low16(host) => self.bfi(host, Reg::ZR, 0, 16),
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }
}
