use super::{
    CarrySense, Flags, Instruction, Logic, Lowered, Lowerer, Mnemonic, Operand, Place, Reg,
    SCRATCH, Shift, Size, T0, Width, encode, size, state, state_access,
};
use crate::codegen::aarch64::encode::{AddSub, Barrier, LoadStore};
use crate::codegen::aarch64::simd::{
    self, Element, Float, FloatThreeSame, FloatUnary, Permute, Scalar, ScalarUnary, ShiftImmediate,
    ThreeSame, Unary, VectorLoadStore, VectorLogic,
};
use crate::codegen::aarch64::{Cond, emit::Label};
use crate::codegen::registers::{ADDRESS, VECTOR_TEMPORARIES};
use crate::instruction::{Memory, Register};

const V0: u8 = VECTOR_TEMPORARIES[0];
const V1: u8 = VECTOR_TEMPORARIES[1];
const V2: u8 = VECTOR_TEMPORARIES[2];
const V3: u8 = VECTOR_TEMPORARIES[3];

const INTEGER_INDEFINITE_32: u64 = 0x8000_0000;
const INTEGER_INDEFINITE_64: u64 = 0x8000_0000_0000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Packed(Float),
    Scalar(Float),
}

impl Kind {
    fn float(self) -> Float {
        match self {
            Kind::Packed(float) | Kind::Scalar(float) => float,
        }
    }
}

fn element(float: Float) -> Element {
    match float {
        Float::Single => Element::S,
        Float::Double => Element::D,
    }
}

fn arithmetic(mnemonic: Mnemonic) -> Option<(FloatThreeSame, Kind)> {
    use Float::{Double, Single};
    use FloatThreeSame as F;
    use Kind::{Packed, Scalar};
    use Mnemonic as M;
    Some(match mnemonic {
        M::Addps => (F::Fadd, Packed(Single)),
        M::Addpd => (F::Fadd, Packed(Double)),
        M::Addss => (F::Fadd, Scalar(Single)),
        M::Addsd => (F::Fadd, Scalar(Double)),
        M::Subps => (F::Fsub, Packed(Single)),
        M::Subpd => (F::Fsub, Packed(Double)),
        M::Subss => (F::Fsub, Scalar(Single)),
        M::Subsd => (F::Fsub, Scalar(Double)),
        M::Mulps => (F::Fmul, Packed(Single)),
        M::Mulpd => (F::Fmul, Packed(Double)),
        M::Mulss => (F::Fmul, Scalar(Single)),
        M::Mulsd => (F::Fmul, Scalar(Double)),
        M::Divps => (F::Fdiv, Packed(Single)),
        M::Divpd => (F::Fdiv, Packed(Double)),
        M::Divss => (F::Fdiv, Scalar(Single)),
        M::Divsd => (F::Fdiv, Scalar(Double)),
        _ => return None,
    })
}

fn scalar_op(op: FloatThreeSame) -> Scalar {
    match op {
        FloatThreeSame::Fsub => Scalar::Fsub,
        FloatThreeSame::Fmul => Scalar::Fmul,
        FloatThreeSame::Fdiv => Scalar::Fdiv,
        _ => Scalar::Fadd,
    }
}

fn integer(mnemonic: Mnemonic) -> Option<(ThreeSame, Element)> {
    use Element::{B, D, H, S};
    use Mnemonic as M;
    use ThreeSame as T;
    Some(match mnemonic {
        M::Paddb => (T::Add, B),
        M::Paddw => (T::Add, H),
        M::Paddd => (T::Add, S),
        M::Paddq => (T::Add, D),
        M::Psubb => (T::Sub, B),
        M::Psubw => (T::Sub, H),
        M::Psubd => (T::Sub, S),
        M::Psubq => (T::Sub, D),
        M::Paddsb => (T::Sqadd, B),
        M::Paddsw => (T::Sqadd, H),
        M::Paddusb => (T::Uqadd, B),
        M::Paddusw => (T::Uqadd, H),
        M::Psubsb => (T::Sqsub, B),
        M::Psubsw => (T::Sqsub, H),
        M::Psubusb => (T::Uqsub, B),
        M::Psubusw => (T::Uqsub, H),
        M::Pcmpeqb => (T::Cmeq, B),
        M::Pcmpeqw => (T::Cmeq, H),
        M::Pcmpeqd => (T::Cmeq, S),
        M::Pcmpgtb => (T::Cmgt, B),
        M::Pcmpgtw => (T::Cmgt, H),
        M::Pcmpgtd => (T::Cmgt, S),
        M::Pminub => (T::Umin, B),
        M::Pmaxub => (T::Umax, B),
        M::Pminsw => (T::Smin, H),
        M::Pmaxsw => (T::Smax, H),
        M::Pavgb => (T::Urhadd, B),
        M::Pavgw => (T::Urhadd, H),
        M::Pmullw => (T::Mul, H),
        _ => return None,
    })
}

fn unpack(mnemonic: Mnemonic) -> Option<(Permute, Element)> {
    use Element::{B, D, H, S};
    use Mnemonic as M;
    use Permute::{Zip1, Zip2};
    Some(match mnemonic {
        M::Punpcklbw => (Zip1, B),
        M::Punpcklwd => (Zip1, H),
        M::Punpckldq | M::Unpcklps => (Zip1, S),
        M::Punpcklqdq | M::Unpcklpd => (Zip1, D),
        M::Punpckhbw => (Zip2, B),
        M::Punpckhwd => (Zip2, H),
        M::Punpckhdq | M::Unpckhps => (Zip2, S),
        M::Punpckhqdq | M::Unpckhpd => (Zip2, D),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VectorShift {
    Left,
    Right,
    Arithmetic,
}

fn vector_shift(mnemonic: Mnemonic) -> Option<(VectorShift, Element)> {
    use Mnemonic as M;
    use VectorShift::{Arithmetic, Left, Right};
    Some(match mnemonic {
        M::Psllw => (Left, Element::H),
        M::Pslld => (Left, Element::S),
        M::Psllq => (Left, Element::D),
        M::Psrlw => (Right, Element::H),
        M::Psrld => (Right, Element::S),
        M::Psrlq => (Right, Element::D),
        M::Psraw => (Arithmetic, Element::H),
        M::Psrad => (Arithmetic, Element::S),
        _ => return None,
    })
}

fn xmm(operand: Operand) -> Option<u8> {
    match operand {
        Operand::Register(Register::Xmm(number)) => Some(number & 15),
        _ => None,
    }
}

impl Lowerer {
    pub(super) fn sse(&mut self, instruction: &Instruction) -> Lowered<bool> {
        use Mnemonic as M;
        let mnemonic = instruction.mnemonic;
        if let Some((op, kind)) = arithmetic(mnemonic) {
            self.float_arithmetic(instruction, op, kind)?;
            return Ok(true);
        }
        if let Some((op, element)) = integer(mnemonic) {
            let d = self.xmm_destination(instruction)?;
            let s = self.vector_source(instruction, 1, V0)?;
            self.emit(simd::three_same(op, element, d, d, s));
            return Ok(true);
        }
        if let Some((op, element)) = unpack(mnemonic) {
            let d = self.xmm_destination(instruction)?;
            let s = self.vector_source(instruction, 1, V0)?;
            self.emit(simd::permute(op, element, d, d, s));
            return Ok(true);
        }
        if let Some((shift, element)) = vector_shift(mnemonic) {
            self.packed_shift(instruction, shift, element)?;
            return Ok(true);
        }
        match mnemonic {
            M::Movaps
            | M::Movapd
            | M::Movups
            | M::Movupd
            | M::Movdqa
            | M::Movdqu
            | M::Movntps
            | M::Movntpd
            | M::Movntdq => self.move_whole(instruction)?,
            M::Movss | M::Movsd => self.move_scalar(instruction)?,
            M::Movlps | M::Movlpd => self.move_half(instruction, 0)?,
            M::Movhps | M::Movhpd => self.move_half(instruction, 1)?,
            M::Movhlps | M::Movlhps => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                let (index, source) = if mnemonic == M::Movhlps {
                    (0, 1)
                } else {
                    (1, 0)
                };
                self.emit(simd::insert_element(Element::D, d, index, s, source));
            }
            M::Movd | M::Movq => self.move_integer(instruction)?,
            M::Movmskps | M::Movmskpd | M::Pmovmskb => self.move_mask(instruction)?,
            M::Andps | M::Andpd | M::Pand => self.logic(instruction, VectorLogic::And, false)?,
            M::Andnps | M::Andnpd | M::Pandn => self.logic(instruction, VectorLogic::Bic, true)?,
            M::Orps | M::Orpd | M::Por => self.logic(instruction, VectorLogic::Orr, false)?,
            M::Xorps | M::Xorpd | M::Pxor => self.logic(instruction, VectorLogic::Eor, false)?,
            M::Minps => self.min_max(instruction, Kind::Packed(Float::Single), false)?,
            M::Minpd => self.min_max(instruction, Kind::Packed(Float::Double), false)?,
            M::Minss => self.min_max(instruction, Kind::Scalar(Float::Single), false)?,
            M::Minsd => self.min_max(instruction, Kind::Scalar(Float::Double), false)?,
            M::Maxps => self.min_max(instruction, Kind::Packed(Float::Single), true)?,
            M::Maxpd => self.min_max(instruction, Kind::Packed(Float::Double), true)?,
            M::Maxss => self.min_max(instruction, Kind::Scalar(Float::Single), true)?,
            M::Maxsd => self.min_max(instruction, Kind::Scalar(Float::Double), true)?,
            M::Sqrtps => self.square_root(instruction, Kind::Packed(Float::Single))?,
            M::Sqrtpd => self.square_root(instruction, Kind::Packed(Float::Double))?,
            M::Sqrtss => self.square_root(instruction, Kind::Scalar(Float::Single))?,
            M::Sqrtsd => self.square_root(instruction, Kind::Scalar(Float::Double))?,
            M::Rcpps | M::Rcpss | M::Rsqrtps | M::Rsqrtss => self.reciprocal(instruction)?,
            M::Cmpps => self.compare_mask(instruction, Kind::Packed(Float::Single))?,
            M::Cmppd => self.compare_mask(instruction, Kind::Packed(Float::Double))?,
            M::Cmpss => self.compare_mask(instruction, Kind::Scalar(Float::Single))?,
            M::Cmpsd => self.compare_mask(instruction, Kind::Scalar(Float::Double))?,
            M::Ucomiss | M::Comiss => self.compare_flags(instruction, Float::Single)?,
            M::Ucomisd | M::Comisd => self.compare_flags(instruction, Float::Double)?,
            M::Packsswb => self.pack(instruction, Unary::Sqxtn, Unary::Sqxtn2, Element::H)?,
            M::Packssdw => self.pack(instruction, Unary::Sqxtn, Unary::Sqxtn2, Element::S)?,
            M::Packuswb => self.pack(instruction, Unary::Sqxtun, Unary::Sqxtun2, Element::H)?,
            M::Pmulhw | M::Pmulhuw => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                let signed = mnemonic == M::Pmulhw;
                self.emit(simd::widening_multiply(signed, false, Element::H, V1, d, s));
                self.emit(simd::widening_multiply(signed, true, Element::H, V2, d, s));
                self.emit(simd::permute(Permute::Uzp2, Element::H, d, V1, V2));
            }
            M::Pmuludq => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                self.emit(simd::unary(Unary::Xtn(Element::D), V1, d));
                self.emit(simd::unary(Unary::Xtn(Element::D), V2, s));
                self.emit(simd::widening_multiply(false, false, Element::S, d, V1, V2));
            }
            M::Pmaddwd => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                self.emit(simd::widening_multiply(true, false, Element::H, V1, d, s));
                self.emit(simd::widening_multiply(true, true, Element::H, V2, d, s));
                self.emit(simd::three_same(ThreeSame::Addp, Element::S, d, V1, V2));
            }
            M::Psadbw => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                self.emit(simd::three_same(ThreeSame::Uabd, Element::B, V1, d, s));
                self.emit(simd::unary(Unary::Uaddlp(Element::B), V1, V1));
                self.emit(simd::unary(Unary::Uaddlp(Element::H), V1, V1));
                self.emit(simd::unary(Unary::Uaddlp(Element::S), d, V1));
            }
            M::Psrldq | M::Pslldq => self.byte_shift(instruction)?,
            M::Pshufd => self.shuffle_words(instruction)?,
            M::Pshuflw => self.shuffle_halves(instruction, 0)?,
            M::Pshufhw => self.shuffle_halves(instruction, 4)?,
            M::Shufps | M::Shufpd => self.shuffle_two(instruction)?,
            M::Pextrw => {
                let s = self.vector_source(instruction, 1, V0)?;
                let index = self.immediate(instruction, 2)? as u8 & 7;
                self.emit(simd::move_to_general(Element::H, T0, s, index));
                let place = self.place(&self.operand(instruction, 0)?)?;
                self.store(place, T0);
            }
            M::Pinsrw => {
                let d = self.xmm_destination(instruction)?;
                let index = self.immediate(instruction, 2)? as u8 & 7;
                let value = match self.place(&self.operand(instruction, 1)?)? {
                    Place::Full(host) | Place::Low32(host) | Place::Low16(host) => host,
                    place => self.load(place, SCRATCH),
                };
                self.emit(simd::insert_general(Element::H, d, index, value));
            }
            M::Cvtsi2ss | M::Cvtsi2sd => self.convert_from_integer(instruction)?,
            M::Cvtss2sd | M::Cvtsd2ss => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                let (to, from) = if mnemonic == M::Cvtss2sd {
                    (Float::Double, Float::Single)
                } else {
                    (Float::Single, Float::Double)
                };
                self.emit(simd::convert_precision(to, from, V1, s));
                self.emit(simd::insert_element(element(to), d, 0, V1, 0));
            }
            M::Cvttss2si | M::Cvtss2si | M::Cvttsd2si | M::Cvtsd2si => {
                self.convert_to_integer(instruction)?
            }
            M::Cvtdq2ps | M::Cvtps2pd | M::Cvtpd2ps | M::Cvtdq2pd => {
                let d = self.xmm_destination(instruction)?;
                let s = self.vector_source(instruction, 1, V0)?;
                match mnemonic {
                    M::Cvtdq2ps => {
                        self.emit(simd::float_unary(FloatUnary::Scvtf, Float::Single, d, s))
                    }
                    M::Cvtps2pd => self.emit(simd::unary(Unary::Fcvtl, d, s)),
                    M::Cvtpd2ps => self.emit(simd::unary(Unary::Fcvtn, d, s)),
                    _ => {
                        self.emit(simd::shift_immediate(
                            ShiftImmediate::Sshll,
                            Element::S,
                            V1,
                            s,
                            0,
                        ));
                        self.emit(simd::float_unary(FloatUnary::Scvtf, Float::Double, d, V1));
                    }
                }
            }
            M::Cvtps2dq | M::Cvttps2dq => self.packed_to_words(instruction, Float::Single)?,
            M::Cvtpd2dq | M::Cvttpd2dq => self.packed_to_words(instruction, Float::Double)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn xmm_destination(&self, instruction: &Instruction) -> Lowered<u8> {
        xmm(self.operand(instruction, 0)?).ok_or(self.operands_error())
    }

    fn immediate(&self, instruction: &Instruction, index: usize) -> Lowered<i64> {
        match self.operand(instruction, index)? {
            Operand::Immediate(value) => Ok(value),
            _ => Err(self.operands_error()),
        }
    }

    fn vector_source(&mut self, instruction: &Instruction, index: usize, into: u8) -> Lowered<u8> {
        match self.operand(instruction, index)? {
            Operand::Register(Register::Xmm(number)) => Ok(number & 15),
            Operand::Memory(memory) => {
                self.load_vector(&memory, into)?;
                Ok(into)
            }
            _ => Err(self.operands_error()),
        }
    }

    fn load_vector(&mut self, memory: &Memory, into: u8) -> Lowered {
        let address = self.effective_address(memory, ADDRESS, memory.segment);
        match memory.width {
            Width::Dqword => {
                self.emit(
                    simd::vector_load_store(VectorLoadStore::LdrQ, into, address, 0)
                        .expect("offset 0 is encodable"),
                );
                if !self.mode.hardware_tso {
                    self.emit(encode::dmb(Barrier::IshLd));
                }
            }
            Width::Qword => {
                self.load_memory(address, Width::Qword, SCRATCH);
                self.emit(simd::move_from_general(Size::X, into, SCRATCH));
            }
            Width::Dword | Width::Word => {
                self.load_memory(address, memory.width, SCRATCH);
                self.emit(simd::move_from_general(Size::W, into, SCRATCH));
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn store_vector(&mut self, memory: &Memory, from: u8) -> Lowered {
        let address = self.effective_address(memory, ADDRESS, memory.segment);
        match memory.width {
            Width::Dqword => {
                if !self.mode.hardware_tso {
                    self.emit(encode::dmb(Barrier::Ish));
                }
                self.emit(
                    simd::vector_load_store(VectorLoadStore::StrQ, from, address, 0)
                        .expect("offset 0 is encodable"),
                );
            }
            Width::Qword => {
                self.emit(simd::move_to_general_low(Size::X, SCRATCH, from));
                self.store_memory(address, Width::Qword, SCRATCH);
            }
            Width::Dword => {
                self.emit(simd::move_to_general_low(Size::W, SCRATCH, from));
                self.store_memory(address, Width::Dword, SCRATCH);
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn float_arithmetic(
        &mut self,
        instruction: &Instruction,
        op: FloatThreeSame,
        kind: Kind,
    ) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        match kind {
            Kind::Packed(float) => self.emit(simd::float_three_same(op, float, d, d, s)),
            Kind::Scalar(float) => {
                self.emit(simd::scalar(scalar_op(op), float, V1, d, s));
                self.emit(simd::insert_element(element(float), d, 0, V1, 0));
            }
        }
        Ok(())
    }

    fn min_max(&mut self, instruction: &Instruction, kind: Kind, maximum: bool) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let float = kind.float();
        let (first, second) = if maximum { (d, s) } else { (s, d) };
        self.emit(simd::float_three_same(
            FloatThreeSame::Fcmgt,
            float,
            V1,
            first,
            second,
        ));
        match kind {
            Kind::Packed(_) => self.emit(simd::vector_logic(VectorLogic::Bif, d, s, V1)),
            Kind::Scalar(_) => {
                self.emit(simd::vector_logic(VectorLogic::Bsl, V1, d, s));
                self.emit(simd::insert_element(element(float), d, 0, V1, 0));
            }
        }
        Ok(())
    }

    fn square_root(&mut self, instruction: &Instruction, kind: Kind) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        match kind {
            Kind::Packed(float) => self.emit(simd::float_unary(FloatUnary::Fsqrt, float, d, s)),
            Kind::Scalar(float) => {
                self.emit(simd::scalar_unary(ScalarUnary::Fsqrt, float, V1, s));
                self.emit(simd::insert_element(element(float), d, 0, V1, 0));
            }
        }
        Ok(())
    }

    fn reciprocal(&mut self, instruction: &Instruction) -> Lowered {
        use Mnemonic as M;
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let mnemonic = instruction.mnemonic;
        let divisor = if matches!(mnemonic, M::Rsqrtps | M::Rsqrtss) {
            self.emit(simd::float_unary(FloatUnary::Fsqrt, Float::Single, V2, s));
            V2
        } else {
            s
        };
        self.emit(simd::float_one(Float::Single, V1));
        if matches!(mnemonic, M::Rcpps | M::Rsqrtps) {
            self.emit(simd::float_three_same(
                FloatThreeSame::Fdiv,
                Float::Single,
                d,
                V1,
                divisor,
            ));
        } else {
            self.emit(simd::float_three_same(
                FloatThreeSame::Fdiv,
                Float::Single,
                V1,
                V1,
                divisor,
            ));
            self.emit(simd::insert_element(Element::S, d, 0, V1, 0));
        }
        Ok(())
    }

    fn logic(&mut self, instruction: &Instruction, op: VectorLogic, inverted: bool) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        if inverted {
            self.emit(simd::vector_logic(op, d, s, d));
        } else {
            self.emit(simd::vector_logic(op, d, d, s));
        }
        Ok(())
    }

    fn move_whole(&mut self, instruction: &Instruction) -> Lowered {
        match (self.operand(instruction, 0)?, self.operand(instruction, 1)?) {
            (Operand::Register(Register::Xmm(d)), Operand::Register(Register::Xmm(s))) => {
                if d & 15 != s & 15 {
                    self.emit(simd::vector_move(d & 15, s & 15));
                }
            }
            (Operand::Register(Register::Xmm(d)), Operand::Memory(memory)) => {
                self.load_vector(&memory, d & 15)?
            }
            (Operand::Memory(memory), Operand::Register(Register::Xmm(s))) => {
                self.store_vector(&memory, s & 15)?
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn move_scalar(&mut self, instruction: &Instruction) -> Lowered {
        let lane = if instruction.mnemonic == Mnemonic::Movss {
            Element::S
        } else {
            Element::D
        };
        match (self.operand(instruction, 0)?, self.operand(instruction, 1)?) {
            (Operand::Register(Register::Xmm(d)), Operand::Register(Register::Xmm(s))) => {
                self.emit(simd::insert_element(lane, d & 15, 0, s & 15, 0));
            }
            (Operand::Register(Register::Xmm(d)), Operand::Memory(memory)) => {
                self.load_vector(&memory, d & 15)?
            }
            (Operand::Memory(memory), Operand::Register(Register::Xmm(s))) => {
                self.store_vector(&memory, s & 15)?
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn move_half(&mut self, instruction: &Instruction, index: u8) -> Lowered {
        match (self.operand(instruction, 0)?, self.operand(instruction, 1)?) {
            (Operand::Register(Register::Xmm(d)), Operand::Memory(memory)) => {
                let address = self.effective_address(&memory, ADDRESS, memory.segment);
                self.load_memory(address, Width::Qword, SCRATCH);
                self.emit(simd::insert_general(Element::D, d & 15, index, SCRATCH));
            }
            (Operand::Memory(memory), Operand::Register(Register::Xmm(s))) => {
                let address = self.effective_address(&memory, ADDRESS, memory.segment);
                self.emit(simd::move_to_general(Element::D, SCRATCH, s & 15, index));
                self.store_memory(address, Width::Qword, SCRATCH);
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn move_integer(&mut self, instruction: &Instruction) -> Lowered {
        match (self.operand(instruction, 0)?, self.operand(instruction, 1)?) {
            (Operand::Register(Register::Xmm(d)), Operand::Register(Register::Xmm(s))) => {
                self.emit(simd::scalar_unary(
                    ScalarUnary::Fmov,
                    Float::Double,
                    d & 15,
                    s & 15,
                ));
            }
            (Operand::Register(Register::Xmm(d)), Operand::Memory(memory)) => {
                self.load_vector(&memory, d & 15)?
            }
            (Operand::Memory(memory), Operand::Register(Register::Xmm(s))) => {
                self.store_vector(&memory, s & 15)?
            }
            (Operand::Register(Register::Xmm(d)), general) => {
                let place = self.place(&general)?;
                let value = self.load(place, SCRATCH);
                self.emit(simd::move_from_general(size(place.width()), d & 15, value));
            }
            (general, Operand::Register(Register::Xmm(s))) => {
                let place = self.place(&general)?;
                match Self::direct(place) {
                    Some((host, size)) => self.emit(simd::move_to_general_low(size, host, s & 15)),
                    None => return Err(self.operands_error()),
                }
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn move_mask(&mut self, instruction: &Instruction) -> Lowered {
        let s = self.vector_source(instruction, 1, V0)?;
        match instruction.mnemonic {
            Mnemonic::Pmovmskb => {
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Ushr,
                    Element::B,
                    V1,
                    s,
                    7,
                ));
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Usra,
                    Element::H,
                    V1,
                    V1,
                    7,
                ));
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Usra,
                    Element::S,
                    V1,
                    V1,
                    14,
                ));
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Usra,
                    Element::D,
                    V1,
                    V1,
                    28,
                ));
                self.emit(simd::move_to_general(Element::B, SCRATCH, V1, 0));
                self.emit(simd::move_to_general(Element::B, T0, V1, 8));
                self.emit(encode::logical_shifted(
                    Logic::Orr,
                    false,
                    Size::W,
                    T0,
                    SCRATCH,
                    T0,
                    Shift::Lsl,
                    8,
                ));
            }
            mnemonic => {
                let (lane, top, gap, mask) = if mnemonic == Mnemonic::Movmskps {
                    (Element::S, 31, 30, 0xF)
                } else {
                    (Element::D, 63, 31, 0x3)
                };
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Ushr,
                    lane,
                    V1,
                    s,
                    top,
                ));
                if lane == Element::S {
                    self.emit(simd::shift_immediate(
                        ShiftImmediate::Usra,
                        Element::D,
                        V1,
                        V1,
                        31,
                    ));
                }
                self.emit(simd::unary(Unary::Xtn(Element::D), V1, V1));
                self.emit(simd::move_to_general_low(Size::X, SCRATCH, V1));
                self.emit(encode::logical_shifted(
                    Logic::Orr,
                    false,
                    Size::X,
                    T0,
                    SCRATCH,
                    SCRATCH,
                    Shift::Lsr,
                    gap,
                ));
                self.logic_immediate(Logic::And, Size::W, T0, T0, mask);
            }
        }
        let place = self.place(&self.operand(instruction, 0)?)?;
        self.store(place, T0);
        Ok(())
    }

    fn compare_mask(&mut self, instruction: &Instruction, kind: Kind) -> Lowered {
        use FloatThreeSame::{Fcmeq, Fcmge, Fcmgt};
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let predicate = self.immediate(instruction, 2)? as u8 & 7;
        let float = kind.float();
        let target = match kind {
            Kind::Packed(_) => d,
            Kind::Scalar(_) => V1,
        };
        let compare = |op, n, m| simd::float_three_same(op, float, target, n, m);
        match predicate & 3 {
            0 => self.emit(compare(Fcmeq, d, s)),
            1 => self.emit(compare(Fcmgt, s, d)),
            2 => self.emit(compare(Fcmge, s, d)),
            _ => {
                self.emit(simd::float_three_same(Fcmeq, float, V2, d, d));
                self.emit(compare(Fcmeq, s, s));
                self.emit(simd::vector_logic(VectorLogic::And, target, target, V2));
            }
        }
        let negate = matches!(predicate, 3..=6);
        if negate {
            self.emit(simd::unary(Unary::Not, target, target));
        }
        if let Kind::Scalar(_) = kind {
            self.emit(simd::insert_element(element(float), d, 0, V1, 0));
        }
        Ok(())
    }

    fn compare_flags(&mut self, instruction: &Instruction, float: Float) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let live = self.defined_and_live(instruction);
        if live.is_empty() {
            return Ok(());
        }
        self.emit(simd::fcmp(float, d, s));
        if live.any(Flags::PF) {
            self.emit(encode::cset(Size::W, SCRATCH, Cond::Vc));
            self.emit(state_access(LoadStore::Strb, SCRATCH, state::PARITY));
        }
        if live.any(Flags::AF) {
            self.emit(state_access(LoadStore::Strb, Reg::ZR, state::ADJUST));
        }
        if live.any(Flags::NZCV) {
            let ordered: Label = self.asm.label();
            let done = self.asm.label();
            self.asm.b_cond(Cond::Vc, ordered);
            self.move_immediate(Size::W, SCRATCH, 0x4000_0000);
            self.emit(encode::msr_nzcv(SCRATCH));
            self.asm.b(done);
            self.asm.bind(ordered);
            if live.any(Flags::SF.union(Flags::OF)) {
                self.emit(encode::mrs_nzcv(SCRATCH));
                self.logic_immediate(Logic::And, Size::X, SCRATCH, SCRATCH, 0x6000_0000);
                self.emit(encode::msr_nzcv(SCRATCH));
            }
            self.asm.bind(done);
            self.carry = CarrySense::Inverted;
        }
        Ok(())
    }

    fn pack(
        &mut self,
        instruction: &Instruction,
        lower: fn(Element) -> Unary,
        upper: fn(Element) -> Unary,
        source: Element,
    ) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        self.emit(simd::unary(lower(source), V1, d));
        self.emit(simd::unary(upper(source), V1, s));
        self.emit(simd::vector_move(d, V1));
        Ok(())
    }

    fn packed_shift(
        &mut self,
        instruction: &Instruction,
        shift: VectorShift,
        element: Element,
    ) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let bits = element.bits();
        match self.operand(instruction, 1)? {
            Operand::Immediate(count) => {
                let count = count as u8;
                match shift {
                    _ if count == 0 => {}
                    VectorShift::Left | VectorShift::Right if count >= bits => {
                        self.emit(simd::zero(d))
                    }
                    VectorShift::Left => self.emit(simd::shift_immediate(
                        ShiftImmediate::Shl,
                        element,
                        d,
                        d,
                        count,
                    )),
                    VectorShift::Right => self.emit(simd::shift_immediate(
                        ShiftImmediate::Ushr,
                        element,
                        d,
                        d,
                        count,
                    )),
                    VectorShift::Arithmetic => self.emit(simd::shift_immediate(
                        ShiftImmediate::Sshr,
                        element,
                        d,
                        d,
                        count.min(bits),
                    )),
                }
            }
            Operand::Register(Register::Xmm(s)) => {
                self.emit(simd::move_to_general_low(Size::X, SCRATCH, s & 15));
                self.vector_shift_by(d, shift, element);
            }
            Operand::Memory(memory) => {
                let address = self.effective_address(&memory, ADDRESS, memory.segment);
                self.load_memory(address, Width::Qword, SCRATCH);
                self.vector_shift_by(d, shift, element);
            }
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn vector_shift_by(&mut self, d: u8, shift: VectorShift, element: Element) {
        let small = self.asm.label();
        self.shift_right(Size::X, T0, SCRATCH, 6);
        self.asm.cbz(Size::X, T0, small, false);
        self.move_immediate(Size::X, SCRATCH, 64);
        self.asm.bind(small);
        if shift != VectorShift::Left {
            self.emit(encode::add_sub_shifted(
                AddSub::Sub,
                Size::W,
                SCRATCH,
                Reg::ZR,
                SCRATCH,
                Shift::Lsl,
                0,
            ));
        }
        self.emit(simd::dup_general(Element::B, V1, SCRATCH));
        let op = if shift == VectorShift::Arithmetic {
            ThreeSame::Sshl
        } else {
            ThreeSame::Ushl
        };
        self.emit(simd::three_same(op, element, d, d, V1));
    }

    fn byte_shift(&mut self, instruction: &Instruction) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let count = self.immediate(instruction, 1)? as u8;
        if count == 0 {
            return Ok(());
        }
        if count >= 16 {
            self.emit(simd::zero(d));
            return Ok(());
        }
        self.emit(simd::zero(V1));
        if instruction.mnemonic == Mnemonic::Psrldq {
            self.emit(simd::ext(d, d, V1, count));
        } else {
            self.emit(simd::ext(d, V1, d, 16 - count));
        }
        Ok(())
    }

    fn shuffle_words(&mut self, instruction: &Instruction) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let control = self.immediate(instruction, 2)? as u8;
        let selects: [u8; 4] = core::array::from_fn(|lane| (control >> (2 * lane)) & 3);
        if selects.iter().all(|&select| select == selects[0]) {
            self.emit(simd::dup_element(Element::S, d, s, selects[0]));
            return Ok(());
        }
        let source = if s == d {
            self.emit(simd::vector_move(V1, s));
            V1
        } else {
            s
        };
        for (lane, &select) in selects.iter().enumerate() {
            self.emit(simd::insert_element(
                Element::S,
                d,
                lane as u8,
                source,
                select,
            ));
        }
        Ok(())
    }

    fn shuffle_halves(&mut self, instruction: &Instruction, first: u8) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let control = self.immediate(instruction, 2)? as u8;
        let source = if s == d {
            self.emit(simd::vector_move(V1, s));
            V1
        } else {
            self.emit(simd::vector_move(d, s));
            s
        };
        for lane in 0..4u8 {
            let select = first + ((control >> (2 * lane)) & 3);
            if select != first + lane {
                self.emit(simd::insert_element(
                    Element::H,
                    d,
                    first + lane,
                    source,
                    select,
                ));
            }
        }
        Ok(())
    }

    fn shuffle_two(&mut self, instruction: &Instruction) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let s = self.vector_source(instruction, 1, V0)?;
        let control = self.immediate(instruction, 2)? as u8;
        self.emit(simd::vector_move(V1, d));
        let second = if s == d { V1 } else { s };
        if instruction.mnemonic == Mnemonic::Shufps {
            for lane in 0..4u8 {
                let select = (control >> (2 * lane)) & 3;
                let from = if lane < 2 { V1 } else { second };
                if lane >= 2 || select != lane {
                    self.emit(simd::insert_element(Element::S, d, lane, from, select));
                }
            }
        } else {
            let low = control & 1;
            if low != 0 {
                self.emit(simd::insert_element(Element::D, d, 0, V1, low));
            }
            self.emit(simd::insert_element(
                Element::D,
                d,
                1,
                second,
                (control >> 1) & 1,
            ));
        }
        Ok(())
    }

    fn convert_from_integer(&mut self, instruction: &Instruction) -> Lowered {
        let d = self.xmm_destination(instruction)?;
        let float = if instruction.mnemonic == Mnemonic::Cvtsi2ss {
            Float::Single
        } else {
            Float::Double
        };
        let place = self.place(&self.operand(instruction, 1)?)?;
        let value = self.load(place, T0);
        self.emit(simd::convert_to_float(
            size(place.width()),
            float,
            V1,
            value,
        ));
        self.emit(simd::insert_element(element(float), d, 0, V1, 0));
        Ok(())
    }

    fn convert_to_integer(&mut self, instruction: &Instruction) -> Lowered {
        use Mnemonic as M;
        let float = match instruction.mnemonic {
            M::Cvttss2si | M::Cvtss2si => Float::Single,
            _ => Float::Double,
        };
        let rounded = matches!(instruction.mnemonic, M::Cvtss2si | M::Cvtsd2si);
        let place = self.place(&self.operand(instruction, 0)?)?;
        let mut s = self.vector_source(instruction, 1, V0)?;
        if rounded {
            self.emit(simd::scalar_unary(ScalarUnary::Frintx, float, V1, s));
            s = V1;
        }
        self.emit(simd::convert_to_integer(Size::X, float, T0, s));
        self.emit(simd::float_three_same(
            FloatThreeSame::Fcmeq,
            float,
            V2,
            s,
            s,
        ));
        self.emit(simd::move_to_general_low(Size::W, SCRATCH, V2));
        let indefinite = self.asm.label();
        let done = self.asm.label();
        self.asm.cbz(Size::W, SCRATCH, indefinite, false);
        let wide = place.width() == Width::Qword;
        if wide {
            self.logic_immediate(Logic::Eor, Size::X, SCRATCH, T0, i64::MAX as u64);
            self.asm.cbz(Size::X, SCRATCH, done, true);
        } else {
            self.sbfx(Size::X, SCRATCH, T0, 0, 32);
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::X,
                SCRATCH,
                SCRATCH,
                T0,
                Shift::Lsl,
                0,
            ));
            self.asm.cbz(Size::X, SCRATCH, done, false);
        }
        self.asm.bind(indefinite);
        if wide {
            self.move_immediate(Size::X, T0, INTEGER_INDEFINITE_64);
        } else {
            self.move_immediate(Size::W, T0, INTEGER_INDEFINITE_32);
        }
        self.asm.bind(done);
        self.store(place, T0);
        Ok(())
    }

    fn packed_to_words(&mut self, instruction: &Instruction, float: Float) -> Lowered {
        use Mnemonic as M;
        let d = self.xmm_destination(instruction)?;
        let mut s = self.vector_source(instruction, 1, V0)?;
        if matches!(instruction.mnemonic, M::Cvtps2dq | M::Cvtpd2dq) {
            self.emit(simd::float_unary(FloatUnary::Frintx, float, V1, s));
            s = V1;
        }
        self.emit(simd::float_three_same(
            FloatThreeSame::Fcmeq,
            float,
            V2,
            s,
            s,
        ));
        self.emit(simd::float_unary(FloatUnary::Fcvtzs, float, V3, s));
        match float {
            Float::Single => {
                self.emit(simd::move_top_byte(true, V1, 0x80));
                self.emit(simd::three_same(ThreeSame::Cmeq, Element::S, V1, V3, V1));
                self.emit(simd::vector_logic(VectorLogic::Bic, V2, V2, V1));
                self.emit(simd::move_top_byte(false, V1, 0x80));
                self.emit(simd::vector_logic(VectorLogic::Bif, V3, V1, V2));
                self.emit(simd::vector_move(d, V3));
            }
            Float::Double => {
                self.emit(simd::unary(Unary::Xtn(Element::D), V1, V3));
                self.emit(simd::shift_immediate(
                    ShiftImmediate::Sshll,
                    Element::S,
                    V1,
                    V1,
                    0,
                ));
                self.emit(simd::three_same(ThreeSame::Cmeq, Element::D, V1, V1, V3));
                self.emit(simd::vector_logic(VectorLogic::And, V2, V2, V1));
                self.emit(simd::unary(Unary::Xtn(Element::D), V3, V3));
                self.emit(simd::unary(Unary::Xtn(Element::D), V2, V2));
                self.emit(simd::move_top_byte(false, V1, 0x80));
                self.emit(simd::vector_logic(VectorLogic::Bif, V3, V1, V2));
                self.emit(simd::scalar_unary(ScalarUnary::Fmov, Float::Double, d, V3));
            }
        }
        Ok(())
    }
}
