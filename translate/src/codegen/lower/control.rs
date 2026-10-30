use super::arith::{Binary, Output, Source};
use super::{
    AddSub, Condition, ExitKind, Flags, Instruction, Lowered, Lowerer, Mnemonic, Operand, Place,
    Reg, SCRATCH, Shift, Size, T0, T1, T2, T3, T4, Test, Width, access, encode, size,
};
use crate::codegen::aarch64::Cond;
use crate::codegen::aarch64::encode::{Atomic, Bitfield, Logic, Ordered, Pair, Select};
use crate::codegen::registers::{ADDRESS, GUEST_STACK, SHADOW_STACK};
use crate::instruction::Gpr;

const SHADOW_ENTRY: u16 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryOperand {
    Same,
    Negated,
    Inverted,
}

impl Lowerer {
    pub(super) fn conditional_move(
        &mut self,
        instruction: &Instruction,
        condition: Condition,
    ) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let from = self.place(&self.operand(instruction, 1)?)?;
        let value = self.load(from, T1);
        let current = self.load(place, T0);
        match self.condition(condition) {
            Test::Cond(cond) => match Self::direct(place) {
                Some((host, size)) => {
                    self.emit(encode::select(Select::Csel, size, host, value, host, cond));
                }
                None => {
                    self.emit(encode::select(
                        Select::Csel,
                        Size::W,
                        T2,
                        value,
                        current,
                        cond,
                    ));
                    self.store(place, T2);
                }
            },
            test @ Test::Parity { .. } => {
                let take = self.asm.label();
                let done = self.asm.label();
                self.branch_if(test, take);
                self.store(place, current);
                self.asm.b(done);
                self.asm.bind(take);
                self.store(place, value);
                self.asm.bind(done);
            }
        }
        Ok(())
    }

    pub(super) fn set(&mut self, instruction: &Instruction, condition: Condition) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        match self.condition(condition) {
            Test::Cond(cond) => self.emit(encode::cset(Size::W, T2, cond)),
            Test::Parity { set } => {
                self.parity_odd(T2);
                self.logic_immediate(Logic::And, Size::W, T2, T2, 1);
                if set {
                    self.logic_immediate(Logic::Eor, Size::W, T2, T2, 1);
                }
            }
        }
        self.store(place, T2);
        Ok(())
    }

    fn exclusive_loop(&mut self, address: Reg, width: Width, old: Reg, update: impl Fn(&mut Self)) {
        let top = self.asm.label();
        self.asm.bind(top);
        self.emit(encode::ordered(Ordered::Ldaxr, access(width), old, address));
        update(self);
        self.emit(encode::stlxr(access(width), SCRATCH, T2, address));
        self.asm.cbz(Size::W, SCRATCH, top, true);
    }

    pub(super) fn exchange(&mut self, instruction: &Instruction) -> Lowered {
        let first = self.place(&self.operand(instruction, 0)?)?;
        let second = self.place(&self.operand(instruction, 1)?)?;
        let (memory, register) = match (first, second) {
            (Place::Memory { .. }, register) => (Some(first), register),
            (register, Place::Memory { .. }) => (Some(second), register),
            _ => (None, second),
        };
        match memory {
            Some(Place::Memory { address, width }) => {
                let value = self.load(register, T1);
                if self.mode.lse {
                    self.emit(encode::atomic(
                        Atomic::Swp,
                        access(width),
                        value,
                        T0,
                        address,
                    ));
                } else {
                    if value != T2 {
                        self.copy(Size::X, T2, value);
                    }
                    self.exclusive_loop(address, width, T0, |_| {});
                }
                self.store(register, T0);
            }
            _ => {
                let a = self.load(first, T0);
                if a != T0 {
                    self.copy(Size::X, T0, a);
                }
                let b = self.load(second, T1);
                self.store(first, b);
                self.store(second, T0);
            }
        }
        Ok(())
    }

    pub(super) fn exchange_add(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let destination = self.place(&self.operand(instruction, 0)?)?;
        let source = self.place(&self.operand(instruction, 1)?)?;
        let width = destination.width();
        let addend = self.load(source, T1);
        if let Place::Memory { address, .. } = destination {
            if self.mode.lse {
                self.emit(encode::atomic(
                    Atomic::Ldadd,
                    access(width),
                    addend,
                    T0,
                    address,
                ));
            } else {
                self.exclusive_loop(address, width, T0, |this| {
                    this.plain(Binary::Add, size(width), T2, T0, addend);
                });
            }
            self.arith(
                Binary::Add,
                width,
                T0,
                Source::Reg(addend),
                Output::Flags,
                need,
            );
        } else {
            let a = self.load(destination, T0);
            if a != T0 {
                self.copy(Size::X, T0, a);
            }
            if addend != T1 {
                self.copy(Size::X, T1, addend);
            }
            self.arith(Binary::Add, width, T0, Source::Reg(T1), Output::Value, need);
            self.store(source, T0);
            self.store(destination, T2);
            return Ok(());
        }
        self.store(source, T0);
        Ok(())
    }

    pub(super) fn compare_exchange(&mut self, instruction: &Instruction) -> Lowered {
        let need = self.defined_and_live(instruction);
        let destination = self.place(&self.operand(instruction, 0)?)?;
        let source = self.place(&self.operand(instruction, 1)?)?;
        let width = destination.width();
        let accumulator = Place::register(Gpr::RAX, width);
        let expected = self.load(accumulator, T1);
        let replacement = self.load(source, T4);
        let compare = if width == Width::Qword {
            Size::X
        } else {
            Size::W
        };
        let equal = self.asm.label();
        let done = self.asm.label();
        match destination {
            Place::Memory { address, .. } if self.mode.lse => {
                self.copy(Size::X, T0, expected);
                self.emit(encode::casal(access(width), T0, replacement, address));
            }
            Place::Memory { address, .. } => {
                let top = self.asm.label();
                let differ = self.asm.label();
                let compared = self.asm.label();
                self.asm.bind(top);
                self.emit(encode::ordered(Ordered::Ldaxr, access(width), T0, address));
                self.xor(compare, SCRATCH, T0, expected);
                self.asm.cbz(compare, SCRATCH, differ, true);
                self.emit(encode::stlxr(access(width), SCRATCH, replacement, address));
                self.asm.cbz(Size::W, SCRATCH, top, true);
                self.asm.b(compared);
                self.asm.bind(differ);
                self.emit(encode::clrex());
                self.asm.bind(compared);
            }
            _ => {
                let current = self.load(destination, T0);
                if current != T0 {
                    self.copy(Size::X, T0, current);
                }
            }
        }
        self.arith(
            Binary::Sub,
            width,
            expected,
            Source::Reg(T0),
            Output::Flags,
            need,
        );
        self.xor(compare, SCRATCH, T0, expected);
        self.asm.cbz(compare, SCRATCH, equal, false);
        self.store(accumulator, T0);
        if !matches!(destination, Place::Memory { .. }) {
            self.store(destination, T0);
        }
        self.asm.b(done);
        self.asm.bind(equal);
        if !matches!(destination, Place::Memory { .. }) {
            self.store(destination, replacement);
        }
        self.asm.bind(done);
        Ok(())
    }

    fn xor(&mut self, size: Size, d: Reg, a: Reg, b: Reg) {
        self.emit(encode::logical_shifted(
            Logic::Eor,
            false,
            size,
            d,
            a,
            b,
            Shift::Lsl,
            0,
        ));
    }

    pub(super) fn locked_update(
        &mut self,
        op: Binary,
        place: Place,
        b: Source,
        need: Flags,
    ) -> Lowered {
        let Place::Memory { address, width } = place else {
            return Err(self.operands_error());
        };
        if matches!(op, Binary::Adc | Binary::Sbb) {
            return Err(super::LowerErrorKind::Unhandled(self.mnemonic));
        }
        let operand = match b {
            Source::Reg(register) if register != T1 => {
                self.copy(Size::X, T1, register);
                T1
            }
            Source::Reg(register) => register,
            Source::Imm(value) => {
                self.move_immediate(size(width), T1, value);
                T1
            }
        };
        let atomic = match op {
            Binary::Add => Some((Atomic::Ldadd, MemoryOperand::Same)),
            Binary::Sub => Some((Atomic::Ldadd, MemoryOperand::Negated)),
            Binary::And => Some((Atomic::Ldclr, MemoryOperand::Inverted)),
            Binary::Or => Some((Atomic::Ldset, MemoryOperand::Same)),
            Binary::Xor => Some((Atomic::Ldeor, MemoryOperand::Same)),
            _ => None,
        };
        match atomic {
            Some((atomic, transform)) if self.mode.lse => {
                let operand_for_memory = match transform {
                    MemoryOperand::Same => operand,
                    MemoryOperand::Negated => {
                        self.emit(encode::add_sub_shifted(
                            AddSub::Sub,
                            Size::X,
                            T3,
                            Reg::ZR,
                            operand,
                            Shift::Lsl,
                            0,
                        ));
                        T3
                    }
                    MemoryOperand::Inverted => {
                        self.emit(encode::logical_shifted(
                            Logic::Orr,
                            true,
                            Size::X,
                            T3,
                            Reg::ZR,
                            operand,
                            Shift::Lsl,
                            0,
                        ));
                        T3
                    }
                };
                self.emit(encode::atomic(
                    atomic,
                    access(width),
                    operand_for_memory,
                    T0,
                    address,
                ));
            }
            _ => {
                self.exclusive_loop(address, width, T0, |this| {
                    this.plain(op, size(width), T2, T0, operand);
                });
            }
        }
        self.arith(op, width, T0, Source::Reg(operand), Output::Flags, need);
        Ok(())
    }

    pub(super) fn push(&mut self, instruction: &Instruction) -> Lowered {
        let width = instruction.width;
        let value = match self.operand(instruction, 0)? {
            Operand::Immediate(value) => {
                self.move_immediate(Size::X, T0, value as u64);
                T0
            }
            operand => {
                let place = self.place(&operand)?;
                let value = self.load(place, T0);
                if value == GUEST_STACK {
                    self.copy(Size::X, T0, value);
                    T0
                } else {
                    value
                }
            }
        };
        let bytes = width.bytes();
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::X,
            GUEST_STACK,
            GUEST_STACK,
            u16::from(bytes),
            false,
        ));
        self.store_memory(GUEST_STACK, width, value);
        Ok(())
    }

    pub(super) fn pop(&mut self, instruction: &Instruction) -> Lowered {
        let width = instruction.width;
        self.load_memory(GUEST_STACK, width, T0);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::X,
            GUEST_STACK,
            GUEST_STACK,
            u16::from(width.bytes()),
            false,
        ));
        let place = self.place(&self.operand(instruction, 0)?)?;
        self.store(place, T0);
        Ok(())
    }

    pub(super) fn leave(&mut self, width: Width) -> Lowered {
        if width != Width::Qword {
            return Err(self.operands_error());
        }
        let rbp = Reg(Gpr::RBP.0);
        self.copy(Size::X, GUEST_STACK, rbp);
        self.load_memory(GUEST_STACK, Width::Qword, rbp);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::X,
            GUEST_STACK,
            GUEST_STACK,
            8,
            false,
        ));
        Ok(())
    }

    fn branch_target(&mut self, instruction: &Instruction) -> Lowered<Option<u64>> {
        match self.operand(instruction, 0)? {
            Operand::Target(target) => Ok(Some(target)),
            operand => {
                let place = self.place(&operand)?;
                let value = self.load(place, ADDRESS);
                if value != ADDRESS {
                    self.copy(Size::X, ADDRESS, value);
                }
                Ok(None)
            }
        }
    }

    pub(super) fn jump(&mut self, instruction: &Instruction) -> Lowered {
        match self.branch_target(instruction)? {
            Some(target) => self.exit(ExitKind::Jump(target)),
            None => self.exit(ExitKind::Indirect),
        }
        Ok(())
    }

    fn two_way(&mut self, taken: super::Label, target: u64, next: u64) {
        self.exit(ExitKind::Jump(next));
        self.asm.bind(taken);
        self.exit(ExitKind::Jump(target));
    }

    pub(super) fn conditional_jump(
        &mut self,
        instruction: &Instruction,
        condition: Condition,
    ) -> Lowered {
        let Operand::Target(target) = self.operand(instruction, 0)? else {
            return Err(self.operands_error());
        };
        let test = self.condition(condition);
        let taken = self.asm.label();
        self.branch_if(test, taken);
        self.two_way(taken, target, instruction.end());
        Ok(())
    }

    pub(super) fn count_jump(&mut self, instruction: &Instruction) -> Lowered {
        let Operand::Target(target) = self.operand(instruction, 0)? else {
            return Err(self.operands_error());
        };
        let rcx = Reg(Gpr::RCX.0);
        let taken = self.asm.label();
        let fall = self.asm.label();
        match instruction.mnemonic {
            Mnemonic::Jrcxz => self.asm.cbz(Size::X, rcx, taken, false),
            mnemonic => {
                self.emit(encode::add_sub_immediate(
                    AddSub::Sub,
                    Size::X,
                    rcx,
                    rcx,
                    1,
                    false,
                ));
                match mnemonic {
                    Mnemonic::Loop => self.asm.cbz(Size::X, rcx, taken, true),
                    _ => {
                        self.asm.cbz(Size::X, rcx, fall, false);
                        let cond = if mnemonic == Mnemonic::Loope {
                            Cond::Eq
                        } else {
                            Cond::Ne
                        };
                        self.asm.b_cond(cond, taken);
                    }
                }
            }
        }
        self.asm.bind(fall);
        self.two_way(taken, target, instruction.end());
        Ok(())
    }

    fn advance_shadow_stack(&mut self, forward: bool) {
        let op = if forward { AddSub::Add } else { AddSub::Sub };
        self.emit(encode::add_sub_immediate(
            op,
            Size::X,
            T3,
            SHADOW_STACK,
            SHADOW_ENTRY,
            false,
        ));
        self.emit(encode::bitfield(
            Bitfield::Bfm,
            Size::X,
            SHADOW_STACK,
            T3,
            0,
            15,
        ));
    }

    pub(super) fn call(&mut self, instruction: &Instruction) -> Lowered {
        let target = match self.operand(instruction, 0)? {
            Operand::Target(target) => Some(target),
            operand => {
                let place = self.place(&operand)?;
                let value = self.load(place, T4);
                if value != T4 {
                    self.copy(Size::X, T4, value);
                }
                None
            }
        };
        let return_site = self.asm.label();
        self.guest_address(T0, instruction.end());
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::X,
            GUEST_STACK,
            GUEST_STACK,
            8,
            false,
        ));
        self.store_memory(GUEST_STACK, Width::Qword, T0);
        self.asm.adr(SCRATCH, return_site);
        self.advance_shadow_stack(false);
        self.emit(encode::pair(Pair::Stp, T0, SCRATCH, SHADOW_STACK, 0));
        match target {
            Some(target) => self.exit(ExitKind::Jump(target)),
            None => {
                self.copy(Size::X, ADDRESS, T4);
                self.exit(ExitKind::Indirect);
            }
        }
        self.asm.bind(return_site);
        self.carry = super::CarrySense::Direct;
        self.exit(ExitKind::Jump(instruction.end()));
        Ok(())
    }

    pub(super) fn ret(&mut self, instruction: &Instruction) -> Lowered {
        let release = match instruction.operands() {
            [Operand::Immediate(bytes)] => *bytes,
            _ => 0,
        };
        self.load_memory(GUEST_STACK, Width::Qword, ADDRESS);
        self.add_constant(GUEST_STACK, GUEST_STACK, 8 + release);
        self.emit(encode::pair(Pair::Ldp, T1, T0, SHADOW_STACK, 0));
        self.advance_shadow_stack(true);
        self.xor(Size::X, T1, T1, ADDRESS);
        self.normalize_for_exit();
        let miss = self.asm.label();
        self.asm.cbz(Size::X, T1, miss, true);
        self.emit(encode::br(T0));
        self.asm.bind(miss);
        self.exit(ExitKind::Indirect);
        Ok(())
    }
}

const ENTER_NESTING_MASK: i64 = 31;

impl Lowerer {
    pub(super) fn enter(&mut self, instruction: &Instruction) -> Lowered {
        if instruction.width != Width::Qword {
            return Err(self.operands_error());
        }
        let (frame_bytes, level) = match *instruction.operands() {
            [Operand::Immediate(frame_bytes), Operand::Immediate(level)] => {
                (frame_bytes & 0xFFFF, (level & ENTER_NESTING_MASK) as u8)
            }
            _ => return Err(self.operands_error()),
        };
        let frame_pointer = super::guest(Gpr::RBP);
        self.push_quadword(frame_pointer);
        self.copy(Size::X, T0, GUEST_STACK);
        if level > 0 {
            self.copy(Size::X, T1, frame_pointer);
            for _ in 1..level {
                self.emit(encode::add_sub_immediate(
                    AddSub::Sub,
                    Size::X,
                    T1,
                    T1,
                    8,
                    false,
                ));
                self.load_memory(T1, Width::Qword, T2);
                self.push_quadword(T2);
            }
            self.push_quadword(T0);
        }
        self.copy(Size::X, frame_pointer, T0);
        self.add_constant(GUEST_STACK, GUEST_STACK, -frame_bytes);
        Ok(())
    }

    fn push_quadword(&mut self, value: Reg) {
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::X,
            GUEST_STACK,
            GUEST_STACK,
            8,
            false,
        ));
        self.store_memory(GUEST_STACK, Width::Qword, value);
    }
}
