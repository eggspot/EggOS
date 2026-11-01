use super::{
    Flags, Instruction, Logic, Lowered, Lowerer, Mnemonic, Operand, Reg, SCRATCH, Shift, Size, T0,
    T1, T2, T3, T4, T5, T6, Width, bits, encode, state, state_access,
};
use crate::codegen::aarch64::Cond;
use crate::codegen::aarch64::encode::{AddSub, LoadStore, OneSource, TwoSource};
use crate::codegen::aarch64::simd::{
    self, Float, FloatThreeSame, Scalar, ScalarUnary, VectorLoadStore,
};
use crate::codegen::registers::{ADDRESS, STATE, VECTOR_TEMPORARIES};
use crate::instruction::{Memory, Register};

const V0: u8 = VECTOR_TEMPORARIES[0];
const V1: u8 = VECTOR_TEMPORARIES[1];
const V2: u8 = VECTOR_TEMPORARIES[2];

const TOP: Reg = T3;
const SLOT: Reg = T4;
const OTHER_SLOT: Reg = T5;

const CONTROL_AT_START: u64 = 0x037F;
const CONTROL_ROUNDING_LOW: u8 = 10;
const FPCR_ROUNDING_LOW: u8 = 22;
const STATUS_TOP_LOW: u8 = 11;
const STATUS_KEPT_BY_FNCLEX: u64 = 0x7F00;
const CONDITION_C0: u8 = 8;
const CONDITION_C1: u8 = 9;
const CONDITION_C2: u8 = 10;
const CONDITION_C3: u8 = 14;

const DOUBLE_EXPONENT_BITS: u8 = 11;
const DOUBLE_FRACTION_BITS: u8 = 52;
const DOUBLE_INFINITY: u64 = 0x7FF0_0000_0000_0000;
const DOUBLE_QUIET_NAN: u64 = 0x7FF8_0000_0000_0000;
const EXTENDED_EXPONENT_BITS: u8 = 15;
const EXTENDED_INTEGER_BIT: u64 = 1 << 63;
const EXTENDED_MINUS_DOUBLE_BIAS: i64 = 16383 - 1023;
const EXTENDED_INTEGER_SCALE: i64 = 16383 + 63;
const EXTENDED_SPECIAL_EXPONENT: u64 = 0x7FFF;

fn pops(mnemonic: Mnemonic) -> u8 {
    use Mnemonic as M;
    match mnemonic {
        M::Fstp
        | M::Fistp
        | M::Faddp
        | M::Fsubp
        | M::Fsubrp
        | M::Fmulp
        | M::Fdivp
        | M::Fdivrp
        | M::Fcomp
        | M::Fucomp
        | M::Fcomip
        | M::Fucomip => 1,
        M::Fcompp | M::Fucompp => 2,
        _ => 0,
    }
}

fn constant(mnemonic: Mnemonic) -> f64 {
    use Mnemonic as M;
    use core::f64::consts;
    match mnemonic {
        M::Fld1 => 1.0,
        M::Fldpi => consts::PI,
        M::Fldl2e => consts::LOG2_E,
        M::Fldl2t => consts::LOG2_10,
        M::Fldlg2 => consts::LOG10_2,
        M::Fldln2 => consts::LN_2,
        _ => 0.0,
    }
}

impl Lowerer {
    pub(super) fn x87(&mut self, instruction: &Instruction) -> Lowered<bool> {
        use Mnemonic as M;
        let mnemonic = instruction.mnemonic;
        match mnemonic {
            M::Fld => self.x87_load(instruction)?,
            M::Fst | M::Fstp => self.x87_store(instruction)?,
            M::Fild => self.x87_load_integer(instruction)?,
            M::Fist | M::Fistp => self.x87_store_integer(instruction)?,
            M::Fadd | M::Faddp => self.x87_arithmetic(instruction, Scalar::Fadd, false)?,
            M::Fmul | M::Fmulp => self.x87_arithmetic(instruction, Scalar::Fmul, false)?,
            M::Fsub | M::Fsubp => self.x87_arithmetic(instruction, Scalar::Fsub, false)?,
            M::Fsubr | M::Fsubrp => self.x87_arithmetic(instruction, Scalar::Fsub, true)?,
            M::Fdiv | M::Fdivp => self.x87_arithmetic(instruction, Scalar::Fdiv, false)?,
            M::Fdivr | M::Fdivrp => self.x87_arithmetic(instruction, Scalar::Fdiv, true)?,
            M::Fchs => self.x87_unary(ScalarUnary::Fneg),
            M::Fabs => self.x87_unary(ScalarUnary::Fabs),
            M::Fsqrt => self.x87_unary(ScalarUnary::Fsqrt),
            M::Frndint => self.x87_unary(ScalarUnary::Frintx),
            M::Fld1 | M::Fldz | M::Fldpi | M::Fldl2e | M::Fldl2t | M::Fldlg2 | M::Fldln2 => {
                self.x87_top();
                self.move_immediate(Size::X, SCRATCH, constant(mnemonic).to_bits());
                self.emit(simd::move_from_general(Size::X, V0, SCRATCH));
                self.x87_push(V0);
            }
            M::Fxch => self.x87_exchange(instruction)?,
            M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => {
                self.x87_compare_into_flags(instruction)?
            }
            M::Fcom | M::Fcomp | M::Fucom | M::Fucomp | M::Fcompp | M::Fucompp => {
                self.x87_compare_into_status(instruction)?
            }
            M::Fldcw => self.x87_load_control(instruction)?,
            M::Fnstcw => {
                let place = self.place(&self.operand(instruction, 0)?)?;
                self.emit(state_access(LoadStore::Ldrh, T0, state::X87_CONTROL));
                self.store(place, T0);
            }
            M::Fnstsw => {
                let place = self.place(&self.operand(instruction, 0)?)?;
                self.emit(state_access(LoadStore::Ldrh, T0, state::X87_STATUS));
                self.emit(state_access(LoadStore::Ldrb, T1, state::X87_TOP));
                self.bfi(T0, T1, STATUS_TOP_LOW, 3);
                self.store(place, T0);
            }
            M::Fnclex => {
                self.emit(state_access(LoadStore::Ldrh, T0, state::X87_STATUS));
                self.logic_immediate(Logic::And, Size::W, T0, T0, STATUS_KEPT_BY_FNCLEX);
                self.emit(state_access(LoadStore::Strh, T0, state::X87_STATUS));
            }
            M::Fninit => {
                self.move_immediate(Size::W, T0, CONTROL_AT_START);
                self.emit(state_access(LoadStore::Strh, T0, state::X87_CONTROL));
                self.emit(state_access(LoadStore::Strh, Reg::ZR, state::X87_STATUS));
                self.emit(state_access(LoadStore::Strb, Reg::ZR, state::X87_TOP));
                self.emit(simd::mrs_fpcr(SCRATCH));
                self.bfi(SCRATCH, Reg::ZR, FPCR_ROUNDING_LOW, 2);
                self.emit(simd::msr_fpcr(SCRATCH));
            }
            M::Fwait => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn x87_top(&mut self) {
        self.emit(state_access(LoadStore::Ldrb, TOP, state::X87_TOP));
    }

    fn x87_slot(&mut self, index: u8, into: Reg) {
        let physical = if index == 0 {
            TOP
        } else {
            self.emit(encode::add_sub_immediate(
                AddSub::Add,
                Size::W,
                into,
                TOP,
                u16::from(index & 7),
                false,
            ));
            self.logic_immediate(Logic::And, Size::W, into, into, 7);
            into
        };
        self.emit(encode::add_sub_shifted(
            AddSub::Add,
            Size::X,
            into,
            STATE,
            physical,
            Shift::Lsl,
            3,
        ));
    }

    fn x87_access(&mut self, op: VectorLoadStore, value: u8, slot: Reg) {
        self.emit(
            simd::vector_load_store(op, value, slot, state::X87_STACK)
                .expect("the x87 stack offset is encodable"),
        );
    }

    fn x87_push(&mut self, value: u8) {
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::W,
            TOP,
            TOP,
            1,
            false,
        ));
        self.logic_immediate(Logic::And, Size::W, TOP, TOP, 7);
        self.x87_slot(0, SLOT);
        self.x87_access(VectorLoadStore::StrD, value, SLOT);
        self.emit(state_access(LoadStore::Strb, TOP, state::X87_TOP));
    }

    fn x87_pop(&mut self, count: u8) {
        if count == 0 {
            return;
        }
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::W,
            TOP,
            TOP,
            u16::from(count),
            false,
        ));
        self.logic_immediate(Logic::And, Size::W, TOP, TOP, 7);
        self.emit(state_access(LoadStore::Strb, TOP, state::X87_TOP));
    }

    fn x87_load_st0(&mut self, into: u8) {
        self.x87_slot(0, SLOT);
        self.x87_access(VectorLoadStore::LdrD, into, SLOT);
    }

    fn x87_load_memory(&mut self, memory: &Memory, into: u8) -> Lowered {
        let address = self.effective_address(memory, ADDRESS, memory.segment);
        match memory.width {
            Width::Dword => {
                self.load_memory(address, Width::Dword, SCRATCH);
                self.emit(simd::move_from_general(Size::W, into, SCRATCH));
                self.emit(simd::convert_precision(
                    Float::Double,
                    Float::Single,
                    into,
                    into,
                ));
            }
            Width::Qword => {
                self.load_memory(address, Width::Qword, SCRATCH);
                self.emit(simd::move_from_general(Size::X, into, SCRATCH));
            }
            Width::Tbyte => self.extended_to_double(address, into),
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn x87_store_memory(&mut self, memory: &Memory, value: u8) -> Lowered {
        let address = self.effective_address(memory, ADDRESS, memory.segment);
        match memory.width {
            Width::Dword => {
                self.emit(simd::convert_precision(
                    Float::Single,
                    Float::Double,
                    V2,
                    value,
                ));
                self.emit(simd::move_to_general_low(Size::W, SCRATCH, V2));
                self.store_memory(address, Width::Dword, SCRATCH);
            }
            Width::Qword => {
                self.emit(simd::move_to_general_low(Size::X, SCRATCH, value));
                self.store_memory(address, Width::Qword, SCRATCH);
            }
            Width::Tbyte => self.double_to_extended(value, address),
            _ => return Err(self.operands_error()),
        }
        Ok(())
    }

    fn extended_to_double(&mut self, address: Reg, into: u8) {
        let special = self.asm.label();
        let zero = self.asm.label();
        let infinity = self.asm.label();
        let sign = self.asm.label();
        self.load_memory(address, Width::Qword, T0);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::X,
            T5,
            address,
            8,
            false,
        ));
        self.load_memory(T5, Width::Word, T1);
        self.ubfx(Size::W, T2, T1, 0, EXTENDED_EXPONENT_BITS);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::W,
            T5,
            T2,
            1,
            false,
        ));
        self.asm.tbz(T5, EXTENDED_EXPONENT_BITS, special, true);
        self.emit(simd::convert_unsigned_to_float(
            Size::X,
            Float::Double,
            into,
            T0,
        ));
        self.emit(simd::move_to_general_low(Size::X, T5, into));
        self.asm.cbz(Size::X, T5, sign, false);
        self.ubfx(Size::X, T6, T5, DOUBLE_FRACTION_BITS, DOUBLE_EXPONENT_BITS);
        self.emit(encode::add_sub_shifted(
            AddSub::Add,
            Size::X,
            T6,
            T6,
            T2,
            Shift::Lsl,
            0,
        ));
        self.add_constant(T6, T6, -EXTENDED_INTEGER_SCALE);
        self.asm.tbz(T6, 63, zero, true);
        self.asm.cbz(Size::X, T6, zero, false);
        self.emit(encode::add_sub_immediate(
            AddSub::Sub,
            Size::X,
            T2,
            T6,
            0x7FF,
            false,
        ));
        self.asm.tbz(T2, 63, infinity, false);
        self.bfi(T5, T6, DOUBLE_FRACTION_BITS, DOUBLE_EXPONENT_BITS);
        self.asm.b(sign);
        self.asm.bind(special);
        self.shift_left(Size::X, T5, T0, 1);
        self.asm.cbz(Size::X, T5, infinity, false);
        self.shift_right(Size::X, T5, T5, 12);
        self.logic_immediate(Logic::Orr, Size::X, T5, T5, DOUBLE_QUIET_NAN);
        self.asm.b(sign);
        self.asm.bind(infinity);
        self.move_immediate(Size::X, T5, DOUBLE_INFINITY);
        self.asm.b(sign);
        self.asm.bind(zero);
        self.move_immediate(Size::X, T5, 0);
        self.asm.bind(sign);
        self.ubfx(Size::W, T2, T1, EXTENDED_EXPONENT_BITS, 1);
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            T5,
            T5,
            T2,
            Shift::Lsl,
            63,
        ));
        self.emit(simd::move_from_general(Size::X, into, T5));
    }

    fn double_to_extended(&mut self, value: u8, address: Reg) {
        let special = self.asm.label();
        let normal = self.asm.label();
        let store = self.asm.label();
        self.emit(simd::move_to_general_low(Size::X, T0, value));
        self.ubfx(Size::X, T1, T0, DOUBLE_FRACTION_BITS, DOUBLE_EXPONENT_BITS);
        self.shift_left(Size::X, T2, T0, DOUBLE_EXPONENT_BITS);
        self.logic_immediate(Logic::Orr, Size::X, T2, T2, EXTENDED_INTEGER_BIT);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::W,
            T5,
            T1,
            1,
            false,
        ));
        self.asm.tbz(T5, DOUBLE_EXPONENT_BITS, special, true);
        self.asm.cbz(Size::X, T1, normal, true);
        self.shift_left(Size::X, T2, T0, DOUBLE_EXPONENT_BITS + 1);
        self.asm.cbz(Size::X, T2, store, false);
        self.emit(encode::one_source(OneSource::Clz, Size::X, T5, T2));
        self.emit(encode::two_source(TwoSource::Lslv, Size::X, T2, T2, T5));
        self.move_immediate(Size::X, T1, EXTENDED_MINUS_DOUBLE_BIAS as u64);
        self.emit(encode::add_sub_shifted(
            AddSub::Sub,
            Size::X,
            T1,
            T1,
            T5,
            Shift::Lsl,
            0,
        ));
        self.asm.b(store);
        self.asm.bind(special);
        self.move_immediate(Size::X, T1, EXTENDED_SPECIAL_EXPONENT);
        self.asm.b(store);
        self.asm.bind(normal);
        self.add_constant(T1, T1, EXTENDED_MINUS_DOUBLE_BIAS);
        self.asm.bind(store);
        self.shift_right(Size::X, T5, T0, 63);
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            T1,
            T1,
            T5,
            Shift::Lsl,
            EXTENDED_EXPONENT_BITS,
        ));
        self.store_memory(address, Width::Qword, T2);
        self.emit(encode::add_sub_immediate(
            AddSub::Add,
            Size::X,
            T5,
            address,
            8,
            false,
        ));
        self.store_memory(T5, Width::Word, T1);
    }

    fn x87_load(&mut self, instruction: &Instruction) -> Lowered {
        match self.operand(instruction, 0)? {
            Operand::Register(Register::St(index)) => {
                self.x87_top();
                self.x87_slot(index, OTHER_SLOT);
                self.x87_access(VectorLoadStore::LdrD, V0, OTHER_SLOT);
            }
            Operand::Memory(memory) => {
                self.x87_load_memory(&memory, V0)?;
                self.x87_top();
            }
            _ => return Err(self.operands_error()),
        }
        self.x87_push(V0);
        Ok(())
    }

    fn x87_store(&mut self, instruction: &Instruction) -> Lowered {
        let operand = self.operand(instruction, 0)?;
        self.x87_top();
        self.x87_load_st0(V1);
        match operand {
            Operand::Register(Register::St(index)) => {
                self.x87_slot(index, OTHER_SLOT);
                self.x87_access(VectorLoadStore::StrD, V1, OTHER_SLOT);
            }
            Operand::Memory(memory) => self.x87_store_memory(&memory, V1)?,
            _ => return Err(self.operands_error()),
        }
        self.x87_pop(pops(instruction.mnemonic));
        Ok(())
    }

    fn x87_load_integer(&mut self, instruction: &Instruction) -> Lowered {
        let Operand::Memory(memory) = self.operand(instruction, 0)? else {
            return Err(self.operands_error());
        };
        if !matches!(memory.width, Width::Word | Width::Dword | Width::Qword) {
            return Err(self.operands_error());
        }
        let address = self.effective_address(&memory, ADDRESS, memory.segment);
        self.load_memory(address, memory.width, T0);
        if memory.width != Width::Qword {
            self.sbfx(Size::X, T0, T0, 0, bits(memory.width));
        }
        self.emit(simd::convert_to_float(Size::X, Float::Double, V0, T0));
        self.x87_top();
        self.x87_push(V0);
        Ok(())
    }

    fn x87_store_integer(&mut self, instruction: &Instruction) -> Lowered {
        let Operand::Memory(memory) = self.operand(instruction, 0)? else {
            return Err(self.operands_error());
        };
        if !matches!(memory.width, Width::Word | Width::Dword | Width::Qword) {
            return Err(self.operands_error());
        }
        self.x87_top();
        self.x87_load_st0(V1);
        self.emit(simd::scalar_unary(
            ScalarUnary::Frintx,
            Float::Double,
            V2,
            V1,
        ));
        self.emit(simd::convert_to_integer(Size::X, Float::Double, T0, V2));
        self.emit(simd::float_three_same(
            FloatThreeSame::Fcmeq,
            Float::Double,
            V0,
            V2,
            V2,
        ));
        self.emit(simd::move_to_general_low(Size::W, T1, V0));
        let indefinite = self.asm.label();
        let done = self.asm.label();
        self.asm.cbz(Size::W, T1, indefinite, false);
        if memory.width == Width::Qword {
            self.logic_immediate(Logic::Eor, Size::X, T1, T0, i64::MAX as u64);
            self.asm.cbz(Size::X, T1, done, true);
        } else {
            self.sbfx(Size::X, T1, T0, 0, bits(memory.width));
            self.emit(encode::logical_shifted(
                Logic::Eor,
                false,
                Size::X,
                T1,
                T1,
                T0,
                Shift::Lsl,
                0,
            ));
            self.asm.cbz(Size::X, T1, done, false);
        }
        self.asm.bind(indefinite);
        self.move_immediate(Size::X, T0, 1u64 << (bits(memory.width) - 1));
        self.asm.bind(done);
        let address = self.effective_address(&memory, ADDRESS, memory.segment);
        self.store_memory(address, memory.width, T0);
        self.x87_pop(pops(instruction.mnemonic));
        Ok(())
    }

    fn x87_source(&mut self, instruction: &Instruction) -> Lowered<u8> {
        match *instruction.operands() {
            [] => {
                self.x87_top();
                self.x87_slot(1, OTHER_SLOT);
                self.x87_access(VectorLoadStore::LdrD, V0, OTHER_SLOT);
                Ok(0)
            }
            [Operand::Memory(memory)] => {
                self.x87_load_memory(&memory, V0)?;
                self.x87_top();
                Ok(0)
            }
            [
                Operand::Register(Register::St(destination)),
                Operand::Register(Register::St(source)),
            ] => {
                self.x87_top();
                self.x87_slot(source, OTHER_SLOT);
                self.x87_access(VectorLoadStore::LdrD, V0, OTHER_SLOT);
                Ok(destination)
            }
            _ => Err(self.operands_error()),
        }
    }

    fn x87_arithmetic(&mut self, instruction: &Instruction, op: Scalar, reversed: bool) -> Lowered {
        let destination = self.x87_source(instruction)?;
        self.x87_slot(destination, SLOT);
        self.x87_access(VectorLoadStore::LdrD, V1, SLOT);
        let (first, second) = if reversed { (V0, V1) } else { (V1, V0) };
        self.emit(simd::scalar(op, Float::Double, V1, first, second));
        self.x87_access(VectorLoadStore::StrD, V1, SLOT);
        self.x87_pop(pops(instruction.mnemonic));
        Ok(())
    }

    fn x87_unary(&mut self, op: ScalarUnary) {
        self.x87_top();
        self.x87_load_st0(V1);
        self.emit(simd::scalar_unary(op, Float::Double, V1, V1));
        self.x87_access(VectorLoadStore::StrD, V1, SLOT);
    }

    fn x87_exchange(&mut self, instruction: &Instruction) -> Lowered {
        let Operand::Register(Register::St(index)) = self.operand(instruction, 0)? else {
            return Err(self.operands_error());
        };
        self.x87_top();
        self.x87_load_st0(V0);
        self.x87_slot(index, OTHER_SLOT);
        self.x87_access(VectorLoadStore::LdrD, V1, OTHER_SLOT);
        self.x87_access(VectorLoadStore::StrD, V1, SLOT);
        self.x87_access(VectorLoadStore::StrD, V0, OTHER_SLOT);
        Ok(())
    }

    fn x87_compare_into_flags(&mut self, instruction: &Instruction) -> Lowered {
        self.x87_source(instruction)?;
        self.x87_load_st0(V1);
        let live = self.defined_and_live(instruction);
        self.float_compare_flags(live, Float::Double, V1, V0);
        self.x87_pop(pops(instruction.mnemonic));
        Ok(())
    }

    fn x87_compare_into_status(&mut self, instruction: &Instruction) -> Lowered {
        self.x87_source(instruction)?;
        self.x87_load_st0(V1);
        let keep = self.live.any(Flags::NZCV);
        if keep {
            self.emit(encode::mrs_nzcv(T6));
        }
        self.emit(simd::fcmp(Float::Double, V1, V0));
        self.emit(encode::cset(Size::W, T0, Cond::Lt));
        self.emit(encode::cset(Size::W, T1, Cond::Eq));
        self.emit(encode::cset(Size::W, T2, Cond::Vs));
        if keep {
            self.emit(encode::msr_nzcv(T6));
        }
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::W,
            T1,
            T1,
            T2,
            Shift::Lsl,
            0,
        ));
        self.emit(state_access(LoadStore::Ldrh, T5, state::X87_STATUS));
        self.bfi(T5, T0, CONDITION_C0, 1);
        self.bfi(T5, Reg::ZR, CONDITION_C1, 1);
        self.bfi(T5, T2, CONDITION_C2, 1);
        self.bfi(T5, T1, CONDITION_C3, 1);
        self.emit(state_access(LoadStore::Strh, T5, state::X87_STATUS));
        self.x87_pop(pops(instruction.mnemonic));
        Ok(())
    }

    fn x87_load_control(&mut self, instruction: &Instruction) -> Lowered {
        let place = self.place(&self.operand(instruction, 0)?)?;
        let value = self.load(place, T0);
        self.emit(state_access(LoadStore::Strh, value, state::X87_CONTROL));
        self.emit(simd::mrs_fpcr(SCRATCH));
        self.rounding_into_fpcr(SCRATCH, value, CONTROL_ROUNDING_LOW);
        self.emit(simd::msr_fpcr(SCRATCH));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Mnemonic, constant};

    #[test]
    fn the_constants_are_the_nearest_doubles() {
        assert_eq!(constant(Mnemonic::Fldpi).to_bits(), 0x4009_21FB_5444_2D18);
        assert_eq!(constant(Mnemonic::Fldln2).to_bits(), 0x3FE6_2E42_FEFA_39EF);
        assert_eq!(constant(Mnemonic::Fldl2e).to_bits(), 0x3FF7_1547_652B_82FE);
        assert_eq!(constant(Mnemonic::Fld1).to_bits(), 0x3FF0_0000_0000_0000);
        assert_eq!(constant(Mnemonic::Fldz).to_bits(), 0);
    }
}
