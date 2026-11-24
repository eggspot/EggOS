//! The x87 operations AArch64 has no instruction for, as run-time routines every translated
//! image carries (D702): the calling contract is [`crate::codegen::Helper`]'s. The values are
//! doubles, as the translation keeps the x87 registers. Each routine reduces its argument to a
//! small interval and sums a series there whose truncation error is far below a double's
//! rounding; `fprem` and `fprem1` divide the significands as integers, so their remainders are
//! exact.

use crate::codegen::Helper;
use crate::codegen::aarch64::emit::{Assembler, Label};
use crate::codegen::aarch64::encode::{
    self, AddSub, Bitfield, Logic, MoveWide, OneSource, Pair, Select, TwoSource,
};
use crate::codegen::aarch64::simd::{self, Float, Scalar, ScalarUnary};
use crate::codegen::aarch64::{Cond, Reg, Shift, Size};

const SP: Reg = Reg(31);
const ZR: Reg = Reg(31);
const LINK: Reg = Reg(30);
const STATUS: Reg = Reg(16);
const SCRATCH: Reg = Reg(17);
const R0: Reg = Reg(22);
const R1: Reg = Reg(23);
const R2: Reg = Reg(24);
const R3: Reg = Reg(25);
const R4: Reg = Reg(26);
const R5: Reg = Reg(27);
const R6: Reg = Reg(28);
const FRAME_BYTES: u16 = 64;

const X: u8 = 16;
const Y: u8 = 17;
const A: u8 = 20;
const B: u8 = 21;
const C: u8 = 22;
const D: u8 = 23;
const E: u8 = 24;
const F: u8 = 25;
const G: u8 = 26;
const K: u8 = 27;

/// The status word's bits the routines set.
pub mod status {
    pub const INVALID: u16 = 1 << 0;
    pub const ZERO_DIVIDE: u16 = 1 << 2;
    pub const C0: u16 = 1 << 8;
    pub const C1: u16 = 1 << 9;
    pub const C2: u16 = 1 << 10;
    pub const C3: u16 = 1 << 14;
}

const SIGN: u64 = 0x8000_0000_0000_0000;
const FRACTION: u64 = 0x000F_FFFF_FFFF_FFFF;
const ALL_BUT_EXPONENT: u64 = 0x800F_FFFF_FFFF_FFFF;
const EXPONENT_OF_ONE: u64 = 0x3FF0_0000_0000_0000;
const EXPONENT_OF_HALF: u64 = 0x3FE0_0000_0000_0000;
const IMPLICIT_BIT: u64 = 1 << 52;
const FRACTION_BITS: u8 = 52;
const EXPONENT_BITS: u8 = 11;
const EXPONENT_BIAS: u16 = 1023;
const LEADING_ZEROS_OF_IMPLICIT_BIT: u16 = 11;
const INDEFINITE: u64 = 0xFFF8_0000_0000_0000;
const TWO_TO_THE_63: f64 = 9_223_372_036_854_775_808.0;
const TWO_TO_THE_64: f64 = 18_446_744_073_709_551_616.0;
const DENORMAL_SHIFT: u16 = 64;
const TWO_TO_THE_MINUS_128_BITS: u64 = (1023 - 128) << 52;
const SMALLEST_NORMAL_SCALE: u16 = 53;
const TINY_SCALE_OFFSET: u16 = 128 - 52;
const NORMAL_SCALE_OFFSET: u16 = 52;
const SCALE_LIMIT: u16 = 3000;
const SCALE_STEP: u16 = 1000;
const SCALE_STEPS: usize = 3;
const LOG_ARGUMENT_FOR_ONE_PLUS: f64 = 0.29;

/// π/2 in four parts, each of at most 33 significant bits but the last, so that `k` times
/// each of the first three is exact for `|k| < 2^20` (Cody and Waite's reduction).
pub const HALF_PI_PARTS: [u64; 4] = [
    0x3FF9_21FB_5440_0000,
    0x3DD0_B461_1A60_0000,
    0x3BA3_198A_2E00_0000,
    0x397B_839A_2520_49C1,
];

/// `1 / n!`, correctly rounded: `n!` is exact in a double up to 18.
pub fn reciprocal_factorial(n: u32) -> f64 {
    let mut factorial = 1.0f64;
    for i in 2..=n {
        factorial *= f64::from(i);
    }
    1.0 / factorial
}

/// The series of sin r after its first term, in powers of r²: `-1/3!`, `1/5!`, ..., `1/17!`.
pub fn sine_series() -> [f64; 8] {
    core::array::from_fn(|i| alternate(i) * reciprocal_factorial(2 * i as u32 + 3))
}

/// The series of cos r after its first term, in powers of r²: `-1/2!`, `1/4!`, ..., `1/18!`.
pub fn cosine_series() -> [f64; 9] {
    core::array::from_fn(|i| alternate(i) * reciprocal_factorial(2 * i as u32 + 2))
}

/// The series of atan u after its first term, in powers of u²: `-1/3`, `1/5`, ..., `1/25`.
pub fn arctangent_series() -> [f64; 12] {
    core::array::from_fn(|i| alternate(i) / f64::from(2 * i as u32 + 3))
}

/// The series of atanh s after its first term, in powers of s²: `1/3`, `1/5`, ..., `1/25`.
pub fn hyperbolic_arctangent_series() -> [f64; 12] {
    core::array::from_fn(|i| 1.0 / f64::from(2 * i as u32 + 3))
}

/// The series of (e^z - 1) / z: `1`, `1/2!`, ..., `1/18!`.
pub fn exponential_series() -> [f64; 18] {
    core::array::from_fn(|i| reciprocal_factorial(i as u32 + 1))
}

fn alternate(i: usize) -> f64 {
    if i % 2 == 0 { -1.0 } else { 1.0 }
}

struct Emitter<'a> {
    asm: &'a mut Assembler,
}

impl Emitter<'_> {
    fn emit(&mut self, word: u32) {
        self.asm.emit(word);
    }

    fn float(&mut self, op: Scalar, d: u8, n: u8, m: u8) {
        self.emit(simd::scalar(op, Float::Double, d, n, m));
    }

    fn add(&mut self, d: u8, n: u8, m: u8) {
        self.float(Scalar::Fadd, d, n, m);
    }

    fn sub(&mut self, d: u8, n: u8, m: u8) {
        self.float(Scalar::Fsub, d, n, m);
    }

    fn mul(&mut self, d: u8, n: u8, m: u8) {
        self.float(Scalar::Fmul, d, n, m);
    }

    fn div(&mut self, d: u8, n: u8, m: u8) {
        self.float(Scalar::Fdiv, d, n, m);
    }

    fn unary(&mut self, op: ScalarUnary, d: u8, n: u8) {
        self.emit(simd::scalar_unary(op, Float::Double, d, n));
    }

    fn bits_into(&mut self, r: Reg, value: u64) {
        let literal = self.asm.literal(value);
        self.asm.load_literal(r, literal);
    }

    fn constant(&mut self, d: u8, value: f64) {
        self.bits_into(SCRATCH, value.to_bits());
        self.from_bits(d, SCRATCH);
    }

    fn constant_bits(&mut self, d: u8, bits: u64) {
        self.bits_into(SCRATCH, bits);
        self.from_bits(d, SCRATCH);
    }

    fn zero(&mut self, d: u8) {
        self.from_bits(d, ZR);
    }

    fn to_bits(&mut self, r: Reg, v: u8) {
        self.emit(simd::move_to_general_low(Size::X, r, v));
    }

    fn from_bits(&mut self, v: u8, r: Reg) {
        self.emit(simd::move_from_general(Size::X, v, r));
    }

    fn compare(&mut self, n: u8, m: u8) {
        self.emit(simd::fcmp(Float::Double, n, m));
    }

    fn horner(&mut self, d: u8, z: u8, coefficients: &[f64]) {
        let Some((last, rest)) = coefficients.split_last() else {
            self.zero(d);
            return;
        };
        self.constant(d, *last);
        for &coefficient in rest.iter().rev() {
            self.mul(d, d, z);
            self.constant(K, coefficient);
            self.add(d, d, K);
        }
    }

    fn move_register(&mut self, d: Reg, n: Reg) {
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            d,
            ZR,
            n,
            Shift::Lsl,
            0,
        ));
    }

    fn move_immediate(&mut self, d: Reg, value: u16) {
        self.emit(encode::move_wide(MoveWide::Movz, Size::X, d, value, 0));
    }

    fn move_negative(&mut self, d: Reg, magnitude: u16) {
        self.emit(encode::move_wide(MoveWide::Movn, Size::X, d, magnitude - 1, 0));
    }

    fn shift_left(&mut self, d: Reg, n: Reg, amount: u8) {
        self.emit(encode::bitfield(
            Bitfield::Ubfm,
            Size::X,
            d,
            n,
            (64 - amount) & 63,
            63 - amount,
        ));
    }

    fn field(&mut self, d: Reg, n: Reg, lsb: u8, width: u8) {
        self.emit(encode::bitfield(
            Bitfield::Ubfm,
            Size::X,
            d,
            n,
            lsb,
            lsb + width - 1,
        ));
    }

    fn logic(&mut self, op: Logic, d: Reg, n: Reg, value: u64) {
        let word = encode::logical_immediate(op, Size::X, d, n, value)
            .expect("the routines' masks are bitmask immediates");
        self.emit(word);
    }

    fn or_registers(&mut self, d: Reg, n: Reg, m: Reg) {
        self.emit(encode::logical_shifted(
            Logic::Orr,
            false,
            Size::X,
            d,
            n,
            m,
            Shift::Lsl,
            0,
        ));
    }

    fn arithmetic(&mut self, op: AddSub, d: Reg, n: Reg, m: Reg) {
        self.emit(encode::add_sub_shifted(op, Size::X, d, n, m, Shift::Lsl, 0));
    }

    fn immediate(&mut self, op: AddSub, d: Reg, n: Reg, value: u16) {
        self.emit(encode::add_sub_immediate(op, Size::X, d, n, value, false));
    }

    fn compare_registers(&mut self, n: Reg, m: Reg) {
        self.arithmetic(AddSub::Subs, ZR, n, m);
    }

    fn compare_immediate(&mut self, n: Reg, value: u16) {
        self.immediate(AddSub::Subs, ZR, n, value);
    }

    fn select(&mut self, d: Reg, n: Reg, m: Reg, cond: Cond) {
        self.emit(encode::select(Select::Csel, Size::X, d, n, m, cond));
    }

    fn set_status(&mut self, bits: u16) {
        self.move_immediate(STATUS, bits);
    }

    fn enter(&mut self) {
        self.immediate(AddSub::Sub, SP, SP, FRAME_BYTES);
        self.emit(encode::pair(Pair::Stp, R0, R1, SP, 0));
        self.emit(encode::pair(Pair::Stp, R2, R3, SP, 16));
        self.emit(encode::pair(Pair::Stp, R4, R5, SP, 32));
        self.emit(encode::mrs_nzcv(SCRATCH));
        self.emit(encode::pair(Pair::Stp, R6, SCRATCH, SP, 48));
    }

    fn leave(&mut self) {
        self.emit(encode::pair(Pair::Ldp, R6, SCRATCH, SP, 48));
        self.emit(encode::msr_nzcv(SCRATCH));
        self.emit(encode::pair(Pair::Ldp, R4, R5, SP, 32));
        self.emit(encode::pair(Pair::Ldp, R2, R3, SP, 16));
        self.emit(encode::pair(Pair::Ldp, R0, R1, SP, 0));
        self.immediate(AddSub::Add, SP, SP, FRAME_BYTES);
        self.emit(encode::ret(LINK));
    }

    fn label(&mut self) -> Label {
        self.asm.label()
    }

    fn bind(&mut self, label: Label) {
        self.asm.bind(label);
    }

    fn branch(&mut self, label: Label) {
        self.asm.b(label);
    }

    fn branch_if(&mut self, cond: Cond, label: Label) {
        self.asm.b_cond(cond, label);
    }

    fn copy_sign(&mut self, d: u8, magnitude: u8, sign_of: u8) {
        self.to_bits(R0, magnitude);
        self.logic(Logic::And, R0, R0, !SIGN);
        self.to_bits(R1, sign_of);
        self.logic(Logic::And, R1, R1, SIGN);
        self.or_registers(R0, R0, R1);
        self.from_bits(d, R0);
    }

    fn sine_cosine(&mut self) {
        let too_large = self.label();
        let odd = self.label();
        let signs = self.label();
        let out = self.label();
        self.enter();
        self.set_status(0);
        self.unary(ScalarUnary::Fabs, A, X);
        self.constant(B, TWO_TO_THE_63);
        self.compare(A, B);
        self.branch_if(Cond::Ge, too_large);
        self.constant(B, core::f64::consts::FRAC_2_PI);
        self.mul(A, X, B);
        self.to_bits(R0, A);
        self.logic(Logic::And, R0, R0, SIGN);
        self.logic(Logic::Orr, R0, R0, EXPONENT_OF_HALF);
        self.from_bits(B, R0);
        self.add(A, A, B);
        self.emit(simd::convert_to_integer(Size::X, Float::Double, R1, A));
        self.emit(simd::convert_to_float(Size::X, Float::Double, A, R1));
        self.unary(ScalarUnary::Fmov, C, X);
        for part in HALF_PI_PARTS {
            self.constant_bits(B, part);
            self.mul(B, A, B);
            self.sub(C, C, B);
        }
        self.mul(D, C, C);
        self.horner(E, D, &sine_series());
        self.mul(E, E, D);
        self.mul(E, E, C);
        self.add(E, E, C);
        self.horner(F, D, &cosine_series());
        self.mul(F, F, D);
        self.emit(simd::float_one(Float::Double, B));
        self.add(F, F, B);
        self.asm.tbz(R1, 0, odd, true);
        self.unary(ScalarUnary::Fmov, X, E);
        self.unary(ScalarUnary::Fmov, Y, F);
        self.branch(signs);
        self.bind(odd);
        self.unary(ScalarUnary::Fmov, X, F);
        self.unary(ScalarUnary::Fneg, Y, E);
        self.bind(signs);
        self.asm.tbz(R1, 1, out, false);
        self.unary(ScalarUnary::Fneg, X, X);
        self.unary(ScalarUnary::Fneg, Y, Y);
        self.branch(out);
        self.bind(too_large);
        self.unary(ScalarUnary::Fmov, Y, X);
        self.set_status(status::C2);
        self.bind(out);
        self.leave();
    }

    fn arctangent(&mut self) {
        let nonzero_x = self.label();
        let vertical = self.label();
        let zero_angle = self.label();
        let with_y_sign = self.label();
        let propagate = self.label();
        let no_invert = self.label();
        let no_reflect = self.label();
        let positive_x = self.label();
        let add_half_turn = self.label();
        let out = self.label();
        self.enter();
        self.set_status(0);
        self.zero(A);
        self.compare(X, A);
        self.branch_if(Cond::Ne, nonzero_x);
        self.compare(Y, Y);
        self.branch_if(Cond::Vs, propagate);
        self.compare(Y, A);
        self.branch_if(Cond::Ne, vertical);
        self.to_bits(R0, X);
        self.asm.tbz(R0, 63, zero_angle, false);
        self.constant(G, core::f64::consts::PI);
        self.branch(with_y_sign);
        self.bind(zero_angle);
        self.zero(G);
        self.branch(with_y_sign);
        self.bind(vertical);
        self.constant(G, core::f64::consts::FRAC_PI_2);
        self.bind(with_y_sign);
        self.copy_sign(X, G, Y);
        self.branch(out);
        self.bind(propagate);
        self.add(X, X, Y);
        self.branch(out);
        self.bind(nonzero_x);
        self.div(B, Y, X);
        self.unary(ScalarUnary::Fabs, C, B);
        self.emit(simd::float_one(Float::Double, D));
        self.compare(C, D);
        self.emit(encode::cset(Size::X, R2, Cond::Gt));
        self.branch_if(Cond::Le, no_invert);
        self.div(C, D, C);
        self.bind(no_invert);
        for _ in 0..2 {
            self.mul(E, C, C);
            self.add(E, E, D);
            self.unary(ScalarUnary::Fsqrt, E, E);
            self.add(E, E, D);
            self.div(C, C, E);
        }
        self.mul(E, C, C);
        self.horner(F, E, &arctangent_series());
        self.mul(F, F, E);
        self.mul(F, F, C);
        self.add(F, F, C);
        self.constant(G, 4.0);
        self.mul(F, F, G);
        self.asm.cbz(Size::X, R2, no_reflect, false);
        self.constant(G, core::f64::consts::FRAC_PI_2);
        self.sub(F, G, F);
        self.bind(no_reflect);
        self.copy_sign(F, F, B);
        self.to_bits(R0, X);
        self.asm.tbz(R0, 63, positive_x, false);
        self.constant(G, core::f64::consts::PI);
        self.to_bits(R1, Y);
        self.asm.tbz(R1, 63, add_half_turn, false);
        self.unary(ScalarUnary::Fneg, G, G);
        self.bind(add_half_turn);
        self.add(F, F, G);
        self.bind(positive_x);
        self.unary(ScalarUnary::Fmov, X, F);
        self.bind(out);
        self.leave();
    }

    fn two_to_the_minus_one(&mut self) {
        self.enter();
        self.set_status(0);
        self.constant(A, core::f64::consts::LN_2);
        self.mul(A, X, A);
        self.horner(B, A, &exponential_series());
        self.mul(X, B, A);
        self.leave();
    }

    fn logarithms(&mut self) -> (u32, u32) {
        let log_body = self.label();
        let large = self.label();
        let series = self.label();
        let propagate = self.label();
        let zero_input = self.label();
        let negative_input = self.label();
        let normal = self.label();
        let reduced = self.label();
        let out = self.label();
        let of_one_plus = self.asm.position();
        self.enter();
        self.set_status(0);
        self.unary(ScalarUnary::Fabs, A, X);
        self.constant(B, LOG_ARGUMENT_FOR_ONE_PLUS);
        self.compare(A, B);
        self.branch_if(Cond::Gt, large);
        self.constant(B, 2.0);
        self.add(B, B, X);
        self.div(C, X, B);
        self.move_immediate(R1, 0);
        self.branch(series);
        self.bind(large);
        self.emit(simd::float_one(Float::Double, B));
        self.add(X, X, B);
        self.branch(log_body);
        let of_x = self.asm.position();
        self.enter();
        self.set_status(0);
        self.bind(log_body);
        self.zero(A);
        self.compare(X, A);
        self.branch_if(Cond::Vs, propagate);
        self.branch_if(Cond::Eq, zero_input);
        self.branch_if(Cond::Lt, negative_input);
        self.constant_bits(A, f64::INFINITY.to_bits());
        self.compare(X, A);
        self.branch_if(Cond::Eq, out);
        self.to_bits(R0, X);
        self.field(R1, R0, FRACTION_BITS, EXPONENT_BITS);
        self.asm.cbz(Size::X, R1, normal, true);
        self.constant(A, TWO_TO_THE_64);
        self.mul(X, X, A);
        self.to_bits(R0, X);
        self.field(R1, R0, FRACTION_BITS, EXPONENT_BITS);
        self.immediate(AddSub::Sub, R1, R1, DENORMAL_SHIFT);
        self.bind(normal);
        self.logic(Logic::And, R0, R0, FRACTION);
        self.logic(Logic::Orr, R0, R0, EXPONENT_OF_ONE);
        self.from_bits(A, R0);
        self.immediate(AddSub::Sub, R1, R1, EXPONENT_BIAS);
        self.constant(B, core::f64::consts::SQRT_2);
        self.compare(A, B);
        self.branch_if(Cond::Le, reduced);
        self.constant(B, 0.5);
        self.mul(A, A, B);
        self.immediate(AddSub::Add, R1, R1, 1);
        self.bind(reduced);
        self.emit(simd::float_one(Float::Double, B));
        self.sub(C, A, B);
        self.add(D, A, B);
        self.div(C, C, D);
        self.bind(series);
        self.mul(D, C, C);
        self.horner(E, D, &hyperbolic_arctangent_series());
        self.mul(E, E, D);
        self.mul(E, E, C);
        self.add(E, E, C);
        self.add(E, E, E);
        self.constant(F, core::f64::consts::LOG2_E);
        self.mul(E, E, F);
        self.emit(simd::convert_to_float(Size::X, Float::Double, F, R1));
        self.add(X, F, E);
        self.branch(out);
        self.bind(propagate);
        self.add(X, X, X);
        self.branch(out);
        self.bind(zero_input);
        self.constant_bits(X, f64::NEG_INFINITY.to_bits());
        self.set_status(status::ZERO_DIVIDE);
        self.branch(out);
        self.bind(negative_input);
        self.constant_bits(X, INDEFINITE);
        self.set_status(status::INVALID);
        self.bind(out);
        self.leave();
        (of_x, of_one_plus)
    }

    fn scale(&mut self) {
        let propagate = self.label();
        let out = self.label();
        self.enter();
        self.set_status(0);
        self.compare(Y, Y);
        self.branch_if(Cond::Vs, propagate);
        self.compare(X, X);
        self.branch_if(Cond::Vs, propagate);
        self.emit(simd::convert_to_integer(Size::X, Float::Double, R0, Y));
        self.clamp(R0, SCALE_LIMIT);
        for _ in 0..SCALE_STEPS {
            self.move_register(R3, R0);
            self.clamp(R3, SCALE_STEP);
            self.arithmetic(AddSub::Sub, R0, R0, R3);
            self.immediate(AddSub::Add, R3, R3, EXPONENT_BIAS);
            self.shift_left(R3, R3, FRACTION_BITS);
            self.from_bits(A, R3);
            self.mul(X, X, A);
        }
        self.branch(out);
        self.bind(propagate);
        self.add(X, X, Y);
        self.bind(out);
        self.leave();
    }

    fn clamp(&mut self, value: Reg, limit: u16) {
        self.move_immediate(R2, limit);
        self.compare_registers(value, R2);
        self.select(value, R2, value, Cond::Gt);
        self.move_negative(R2, limit);
        self.compare_registers(value, R2);
        self.select(value, R2, value, Cond::Lt);
    }

    fn extract(&mut self) {
        let propagate = self.label();
        let zero_input = self.label();
        let infinite = self.label();
        let normal = self.label();
        let out = self.label();
        self.enter();
        self.set_status(0);
        self.compare(X, X);
        self.branch_if(Cond::Vs, propagate);
        self.zero(A);
        self.compare(X, A);
        self.branch_if(Cond::Eq, zero_input);
        self.unary(ScalarUnary::Fabs, A, X);
        self.constant_bits(B, f64::INFINITY.to_bits());
        self.compare(A, B);
        self.branch_if(Cond::Eq, infinite);
        self.to_bits(R0, X);
        self.field(R1, R0, FRACTION_BITS, EXPONENT_BITS);
        self.asm.cbz(Size::X, R1, normal, true);
        self.constant(A, TWO_TO_THE_64);
        self.mul(X, X, A);
        self.to_bits(R0, X);
        self.field(R1, R0, FRACTION_BITS, EXPONENT_BITS);
        self.immediate(AddSub::Sub, R1, R1, DENORMAL_SHIFT);
        self.bind(normal);
        self.immediate(AddSub::Sub, R1, R1, EXPONENT_BIAS);
        self.emit(simd::convert_to_float(Size::X, Float::Double, Y, R1));
        self.logic(Logic::And, R0, R0, ALL_BUT_EXPONENT);
        self.logic(Logic::Orr, R0, R0, EXPONENT_OF_ONE);
        self.from_bits(X, R0);
        self.branch(out);
        self.bind(propagate);
        self.unary(ScalarUnary::Fmov, Y, X);
        self.branch(out);
        self.bind(zero_input);
        self.constant_bits(Y, f64::NEG_INFINITY.to_bits());
        self.set_status(status::ZERO_DIVIDE);
        self.branch(out);
        self.bind(infinite);
        self.constant_bits(Y, f64::INFINITY.to_bits());
        self.bind(out);
        self.leave();
    }

    fn normalize(&mut self, significand: Reg, exponent: Reg) {
        let denormal = self.label();
        let done = self.label();
        self.asm.cbz(Size::X, exponent, denormal, false);
        self.logic(Logic::Orr, significand, significand, IMPLICIT_BIT);
        self.branch(done);
        self.bind(denormal);
        self.emit(encode::one_source(OneSource::Clz, Size::X, SCRATCH, significand));
        self.immediate(AddSub::Sub, SCRATCH, SCRATCH, LEADING_ZEROS_OF_IMPLICIT_BIT);
        self.emit(encode::two_source(
            TwoSource::Lslv,
            Size::X,
            significand,
            significand,
            SCRATCH,
        ));
        self.move_immediate(exponent, 1);
        self.arithmetic(AddSub::Sub, exponent, exponent, SCRATCH);
        self.bind(done);
    }

    fn quotient_bit(&mut self, bit: u8, status_bit: u16) {
        self.field(R2, R4, bit, 1);
        self.shift_left(R2, R2, status_bit.trailing_zeros() as u8);
        self.or_registers(STATUS, STATUS, R2);
    }

    fn remainders(&mut self) -> (u32, u32) {
        let common = self.label();
        let propagate = self.label();
        let invalid = self.label();
        let smaller = self.label();
        let divide = self.label();
        let skip = self.label();
        let last = self.label();
        let divided = self.label();
        let round_up = self.label();
        let build = self.label();
        let tiny_scale = self.label();
        let signed = self.label();
        let zero_result = self.label();
        let condition = self.label();
        let out = self.label();
        let truncated = self.asm.position();
        self.move_immediate(STATUS, 0);
        self.branch(common);
        let nearest = self.asm.position();
        self.move_immediate(STATUS, 1);
        self.bind(common);
        self.enter();
        self.move_register(R6, STATUS);
        self.set_status(0);
        self.compare(X, X);
        self.branch_if(Cond::Vs, propagate);
        self.compare(Y, Y);
        self.branch_if(Cond::Vs, propagate);
        self.constant_bits(B, f64::INFINITY.to_bits());
        self.unary(ScalarUnary::Fabs, A, X);
        self.compare(A, B);
        self.branch_if(Cond::Eq, invalid);
        self.zero(C);
        self.compare(Y, C);
        self.branch_if(Cond::Eq, invalid);
        self.unary(ScalarUnary::Fabs, A, Y);
        self.compare(A, B);
        self.branch_if(Cond::Eq, out);
        self.compare(X, C);
        self.branch_if(Cond::Eq, out);
        self.to_bits(R0, X);
        self.logic(Logic::And, R5, R0, SIGN);
        self.field(R2, R0, FRACTION_BITS, EXPONENT_BITS);
        self.logic(Logic::And, R0, R0, FRACTION);
        self.normalize(R0, R2);
        self.to_bits(R1, Y);
        self.field(R3, R1, FRACTION_BITS, EXPONENT_BITS);
        self.logic(Logic::And, R1, R1, FRACTION);
        self.normalize(R1, R3);
        self.move_immediate(R4, 0);
        self.compare_registers(R2, R3);
        self.branch_if(Cond::Lt, smaller);
        self.arithmetic(AddSub::Sub, R2, R2, R3);
        self.bind(divide);
        self.asm.cbz(Size::X, R2, last, false);
        self.compare_registers(R0, R1);
        self.branch_if(Cond::Cc, skip);
        self.arithmetic(AddSub::Sub, R0, R0, R1);
        self.immediate(AddSub::Add, R4, R4, 1);
        self.bind(skip);
        self.shift_left(R0, R0, 1);
        self.shift_left(R4, R4, 1);
        self.immediate(AddSub::Sub, R2, R2, 1);
        self.branch(divide);
        self.bind(last);
        self.compare_registers(R0, R1);
        self.branch_if(Cond::Cc, divided);
        self.arithmetic(AddSub::Sub, R0, R0, R1);
        self.immediate(AddSub::Add, R4, R4, 1);
        self.bind(divided);
        self.asm.cbz(Size::X, R6, build, false);
        self.arithmetic(AddSub::Add, R2, R0, R0);
        self.compare_registers(R2, R1);
        self.branch_if(Cond::Cc, build);
        self.branch_if(Cond::Hi, round_up);
        self.asm.tbz(R4, 0, build, false);
        self.bind(round_up);
        self.arithmetic(AddSub::Sub, R0, R1, R0);
        self.immediate(AddSub::Add, R4, R4, 1);
        self.logic(Logic::Eor, R5, R5, SIGN);
        self.bind(build);
        self.asm.cbz(Size::X, R0, zero_result, false);
        self.emit(simd::convert_unsigned_to_float(Size::X, Float::Double, A, R0));
        self.compare_immediate(R3, SMALLEST_NORMAL_SCALE);
        self.branch_if(Cond::Lt, tiny_scale);
        self.immediate(AddSub::Sub, R2, R3, NORMAL_SCALE_OFFSET);
        self.shift_left(R2, R2, FRACTION_BITS);
        self.from_bits(B, R2);
        self.mul(A, A, B);
        self.branch(signed);
        self.bind(tiny_scale);
        self.immediate(AddSub::Add, R2, R3, TINY_SCALE_OFFSET);
        self.shift_left(R2, R2, FRACTION_BITS);
        self.from_bits(B, R2);
        self.mul(A, A, B);
        self.constant_bits(B, TWO_TO_THE_MINUS_128_BITS);
        self.mul(A, A, B);
        self.bind(signed);
        self.to_bits(R2, A);
        self.or_registers(R2, R2, R5);
        self.from_bits(X, R2);
        self.branch(condition);
        self.bind(zero_result);
        self.from_bits(X, R5);
        self.bind(condition);
        self.quotient_bit(2, status::C0);
        self.quotient_bit(1, status::C3);
        self.quotient_bit(0, status::C1);
        self.branch(out);
        self.bind(smaller);
        self.asm.cbz(Size::X, R6, out, false);
        self.arithmetic(AddSub::Sub, R2, R3, R2);
        self.compare_immediate(R2, 1);
        self.branch_if(Cond::Ne, out);
        self.compare_registers(R0, R1);
        self.branch_if(Cond::Ls, out);
        self.arithmetic(AddSub::Add, R1, R1, R1);
        self.arithmetic(AddSub::Sub, R0, R1, R0);
        self.move_immediate(R4, 1);
        self.logic(Logic::Eor, R5, R5, SIGN);
        self.immediate(AddSub::Sub, R3, R3, 1);
        self.branch(build);
        self.bind(propagate);
        self.add(X, X, Y);
        self.branch(out);
        self.bind(invalid);
        self.constant_bits(X, INDEFINITE);
        self.set_status(status::INVALID);
        self.bind(out);
        self.leave();
        (truncated, nearest)
    }
}

/// Emits every routine at the assembler's position: the word index of each, in
/// [`Helper::ALL`]'s order.
pub fn emit(asm: &mut Assembler) -> [u32; Helper::ALL.len()] {
    let mut at = [0; Helper::ALL.len()];
    let mut emitter = Emitter { asm };
    at[Helper::SineCosine.index()] = emitter.asm.position();
    emitter.sine_cosine();
    at[Helper::Arctangent.index()] = emitter.asm.position();
    emitter.arctangent();
    at[Helper::TwoToTheMinusOne.index()] = emitter.asm.position();
    emitter.two_to_the_minus_one();
    let (log_two, log_two_of_one_plus) = emitter.logarithms();
    at[Helper::LogTwo.index()] = log_two;
    at[Helper::LogTwoOfOnePlus.index()] = log_two_of_one_plus;
    let (truncated, nearest) = emitter.remainders();
    at[Helper::RemainderTruncated.index()] = truncated;
    at[Helper::RemainderNearest.index()] = nearest;
    at[Helper::Scale.index()] = emitter.asm.position();
    emitter.scale();
    at[Helper::Extract.index()] = emitter.asm.position();
    emitter.extract();
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polynomial(z: f64, coefficients: &[f64]) -> f64 {
        coefficients
            .iter()
            .rev()
            .fold(0.0, |sum, &coefficient| sum * z + coefficient)
    }

    fn sine_model(x: f64) -> f64 {
        let y = x * core::f64::consts::FRAC_2_PI;
        let k = (y + 0.5f64.copysign(y)) as i64;
        let mut r = x;
        for part in HALF_PI_PARTS {
            r -= k as f64 * f64::from_bits(part);
        }
        let r2 = r * r;
        let sine = r + r * r2 * polynomial(r2, &sine_series());
        let cosine = 1.0 + r2 * polynomial(r2, &cosine_series());
        match k & 3 {
            0 => sine,
            1 => cosine,
            2 => -sine,
            _ => -cosine,
        }
    }

    fn arctangent_model(t: f64) -> f64 {
        let mut u = t.abs();
        let invert = u > 1.0;
        if invert {
            u = 1.0 / u;
        }
        for _ in 0..2 {
            u /= 1.0 + (1.0 + u * u).sqrt();
        }
        let u2 = u * u;
        let mut a = 4.0 * (u + u * u2 * polynomial(u2, &arctangent_series()));
        if invert {
            a = core::f64::consts::FRAC_PI_2 - a;
        }
        a.copysign(t)
    }

    fn log_two_model(x: f64) -> f64 {
        let bits = x.to_bits();
        let mut exponent = ((bits >> 52) & 0x7FF) as i64 - 1023;
        let mut m = f64::from_bits(bits & FRACTION | EXPONENT_OF_ONE);
        if m > core::f64::consts::SQRT_2 {
            m *= 0.5;
            exponent += 1;
        }
        let s = (m - 1.0) / (m + 1.0);
        let s2 = s * s;
        let ln = 2.0 * (s + s * s2 * polynomial(s2, &hyperbolic_arctangent_series()));
        exponent as f64 + ln * core::f64::consts::LOG2_E
    }

    fn close(got: f64, wanted: f64, ulps: f64) -> bool {
        let unit = f64::from_bits(wanted.abs().to_bits() + 1) - wanted.abs();
        (got - wanted).abs() <= ulps * unit.max(f64::MIN_POSITIVE)
    }

    #[test]
    fn half_pi_parts_add_up_to_half_pi() {
        let sum: f64 = HALF_PI_PARTS.iter().map(|&part| f64::from_bits(part)).sum();
        assert_eq!(sum, core::f64::consts::FRAC_PI_2);
        for part in &HALF_PI_PARTS[..3] {
            assert_eq!(part & 0x7FFFF, 0);
        }
    }

    #[test]
    fn the_sine_reduction_and_series_match_the_library_within_three_units() {
        for i in -2000..2000 {
            let x = f64::from(i) * 0.0137;
            assert!(close(sine_model(x), x.sin(), 3.0), "{x}");
        }
    }

    #[test]
    fn the_arctangent_halvings_and_series_match_the_library_within_four_units() {
        for i in -3000..3000 {
            let t = f64::from(i) * 0.011;
            assert!(close(arctangent_model(t), t.atan(), 4.0), "{t}");
        }
    }

    #[test]
    fn the_logarithm_reduction_and_series_match_the_library_within_three_units() {
        for i in 1..4000 {
            let x = f64::from(i) * 0.37;
            assert!(close(log_two_model(x), x.log2(), 3.0), "{x}");
        }
    }

    #[test]
    fn the_exponential_series_gives_two_to_the_x_minus_one() {
        for i in -100..=100 {
            let x = f64::from(i) / 100.0;
            let z = x * core::f64::consts::LN_2;
            let got = z * polynomial(z, &exponential_series());
            assert!(close(got, z.exp_m1(), 3.0), "{x}");
        }
    }

    #[test]
    fn every_routine_assembles_and_starts_where_reported() {
        let mut asm = Assembler::new();
        let at = emit(&mut asm);
        let mut sorted = at;
        sorted.sort_unstable();
        assert!(sorted.windows(2).all(|pair| pair[0] < pair[1]));
        let words = asm.finish().unwrap().words;
        assert!(at.iter().all(|&word| (word as usize) < words.len()));
    }
}
