use super::arith::{Binary, Output, Source};
use super::{
    Flags, Instruction, Lowered, Lowerer, Mnemonic, Place, Reg, Shift, Size, T0, T1, T4, T5, Width,
    access, encode, state, state_access,
};
use crate::codegen::aarch64::Cond;
use crate::codegen::aarch64::emit::Label;
use crate::codegen::aarch64::encode::{AddSub, Barrier, LoadStore};
use crate::codegen::registers::{ADDRESS, guest};
use crate::instruction::{Gpr, Repeat};

const RAX: Reg = guest(Gpr::RAX);
const RCX: Reg = guest(Gpr::RCX);
const RBX: Reg = guest(Gpr::RBX);
const RSI: Reg = guest(Gpr::RSI);
const RDI: Reg = guest(Gpr::RDI);
const DIRECTION_BIT: u8 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Never,
    WhenNotEqual,
    WhenEqual,
}

impl Lowerer {
    pub(super) fn string(&mut self, instruction: &Instruction) -> Lowered {
        let width = instruction.width;
        let comparing = matches!(instruction.mnemonic, Mnemonic::Cmps | Mnemonic::Scas);
        let mut need = self.defined_and_live(instruction);
        match instruction.repeat {
            None => {
                self.emit(state_access(LoadStore::Ldrb, T4, state::DIRECTION));
                self.move_immediate(Size::X, T5, u64::from(width.bytes()));
                self.emit(encode::add_sub_shifted(
                    AddSub::Sub,
                    Size::X,
                    T5,
                    T5,
                    T4,
                    Shift::Lsl,
                    width.bytes().trailing_zeros() as u8 + 1,
                ));
                self.string_step(instruction.mnemonic, width, need);
                Ok(())
            }
            Some(repeat) => {
                let stop = match (comparing, repeat) {
                    (false, _) => Stop::Never,
                    (true, Repeat::Rep) => Stop::WhenNotEqual,
                    (true, Repeat::Repne) => Stop::WhenEqual,
                };
                if comparing {
                    if self.live.any(Flags::CF) {
                        self.ensure_carry(super::CarrySense::Inverted);
                    }
                    need = need.union(Flags::ZF);
                }
                let done = self.asm.label();
                let finished = self.asm.label();
                let descending = self.asm.label();
                self.asm.cbz(Size::X, RCX, done, false);
                let storing = matches!(instruction.mnemonic, Mnemonic::Movs | Mnemonic::Stos);
                if storing && !self.mode.hardware_tso {
                    self.emit(encode::dmb(Barrier::Ish));
                }
                self.emit(state_access(LoadStore::Ldrb, T4, state::DIRECTION));
                self.asm.tbz(T4, DIRECTION_BIT, descending, true);
                let step = i16::from(width.bytes());
                self.string_loop(instruction.mnemonic, width, step, stop, need);
                self.asm.b(finished);
                self.asm.bind(descending);
                self.string_loop(instruction.mnemonic, width, -step, stop, need);
                self.asm.bind(finished);
                if !self.mode.hardware_tso {
                    let barrier = if storing {
                        Barrier::Ish
                    } else {
                        Barrier::IshLd
                    };
                    self.emit(encode::dmb(barrier));
                }
                self.asm.bind(done);
                Ok(())
            }
        }
    }

    fn string_step(&mut self, mnemonic: Mnemonic, width: Width, need: Flags) {
        match mnemonic {
            Mnemonic::Movs => {
                self.load_memory(RSI, width, T0);
                self.store_memory(RDI, width, T0);
                self.advance(RSI);
                self.advance(RDI);
            }
            Mnemonic::Stos => {
                self.store_memory(RDI, width, RAX);
                self.advance(RDI);
            }
            Mnemonic::Lods => {
                self.load_memory(RSI, width, T0);
                self.store(Place::register(Gpr::RAX, width), T0);
                self.advance(RSI);
            }
            Mnemonic::Cmps => {
                self.load_memory(RSI, width, T0);
                self.load_memory(RDI, width, T1);
                self.advance(RSI);
                self.advance(RDI);
                self.arith(Binary::Sub, width, T0, Source::Reg(T1), Output::Flags, need);
            }
            _ => {
                self.load_memory(RDI, width, T0);
                self.advance(RDI);
                let accumulator = self.load(Place::register(Gpr::RAX, width), T1);
                self.arith(
                    Binary::Sub,
                    width,
                    accumulator,
                    Source::Reg(T0),
                    Output::Flags,
                    need,
                );
            }
        }
    }

    fn advance(&mut self, pointer: Reg) {
        self.emit(encode::add_sub_shifted(
            AddSub::Add,
            Size::X,
            pointer,
            pointer,
            T5,
            Shift::Lsl,
            0,
        ));
    }

    fn post_index(&mut self, load: bool, width: Width, value: Reg, pointer: Reg, step: i16) {
        self.emit(encode::load_store_post_index(
            load,
            access(width),
            value,
            pointer,
            step,
        ));
    }

    fn string_loop(
        &mut self,
        mnemonic: Mnemonic,
        width: Width,
        step: i16,
        stop: Stop,
        need: Flags,
    ) {
        let repeat: Label = self.asm.label();
        let exit = self.asm.label();
        let accumulator = if mnemonic == Mnemonic::Scas {
            self.load(Place::register(Gpr::RAX, width), T1)
        } else {
            T1
        };
        self.asm.bind(repeat);
        match mnemonic {
            Mnemonic::Movs => {
                self.post_index(true, width, T0, RSI, step);
                self.post_index(false, width, T0, RDI, step);
            }
            Mnemonic::Stos => self.post_index(false, width, RAX, RDI, step),
            Mnemonic::Lods => {
                self.post_index(true, width, T0, RSI, step);
                self.store(Place::register(Gpr::RAX, width), T0);
            }
            Mnemonic::Cmps => {
                self.post_index(true, width, T0, RSI, step);
                self.post_index(true, width, T1, RDI, step);
            }
            _ => self.post_index(true, width, T0, RDI, step),
        }
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::X,
            RCX,
            RCX,
            1,
            false,
        ));
        match mnemonic {
            Mnemonic::Cmps => {
                self.arith(Binary::Sub, width, T0, Source::Reg(T1), Output::Flags, need);
            }
            Mnemonic::Scas => {
                self.arith(
                    Binary::Sub,
                    width,
                    accumulator,
                    Source::Reg(T0),
                    Output::Flags,
                    need,
                );
            }
            _ => {}
        }
        match stop {
            Stop::Never => {}
            Stop::WhenNotEqual => self.asm.b_cond(Cond::Ne, exit),
            Stop::WhenEqual => self.asm.b_cond(Cond::Eq, exit),
        }
        self.asm.cbz(Size::X, RCX, repeat, true);
        self.asm.bind(exit);
    }

    pub(super) fn translate_byte(&mut self) -> Lowered {
        self.ubfx(Size::W, T0, RAX, 0, 8);
        self.emit(encode::add_sub_shifted(
            AddSub::Add,
            Size::X,
            ADDRESS,
            RBX,
            T0,
            Shift::Lsl,
            0,
        ));
        self.load_memory(ADDRESS, Width::Byte, T0);
        self.bfi(RAX, T0, 0, 8);
        Ok(())
    }
}
