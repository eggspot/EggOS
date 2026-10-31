//! AArch64 SIMD and floating-point encodings for the SSE, SSE2 and x87 lowering (slice 39c of
//! D191), one pure function per form, from the encoding diagrams of Arm's Architecture
//! Reference Manual (A64). Vector registers are numbers 0 to 31; vector forms are always the
//! 128-bit ones (Q = 1) unless the function says otherwise.

use super::{Reg, Size};

fn vd(v: u8) -> u32 {
    u32::from(v & 31)
}

fn vn(v: u8) -> u32 {
    u32::from(v & 31) << 5
}

fn vm(v: u8) -> u32 {
    u32::from(v & 31) << 16
}

fn gd(r: Reg) -> u32 {
    u32::from(r.0 & 31)
}

fn gn(r: Reg) -> u32 {
    u32::from(r.0 & 31) << 5
}

/// The element size of a vector arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    /// Bytes (`.16b`, `.8b`).
    B = 0,
    /// Halfwords (`.8h`, `.4h`).
    H = 1,
    /// Words (`.4s`, `.2s`).
    S = 2,
    /// Doublewords (`.2d`).
    D = 3,
}

impl Element {
    /// The element's size in bits.
    pub const fn bits(self) -> u8 {
        8 << (self as u8)
    }

    fn size(self) -> u32 {
        (self as u32) << 22
    }

    fn imm5(self, index: u8) -> u32 {
        let shift = self as u32;
        ((u32::from(index) << (shift + 1)) | (1 << shift)) & 31
    }
}

/// The precision of a floating-point form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Float {
    /// 32-bit.
    Single,
    /// 64-bit.
    Double,
}

impl Float {
    fn bit22(self) -> u32 {
        match self {
            Float::Single => 0,
            Float::Double => 1 << 22,
        }
    }
}

/// The integer three-register vector operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreeSame {
    /// `add`.
    Add,
    /// `sub`.
    Sub,
    /// `sqadd`: signed saturating add.
    Sqadd,
    /// `uqadd`.
    Uqadd,
    /// `sqsub`.
    Sqsub,
    /// `uqsub`.
    Uqsub,
    /// `cmeq`: all ones where equal.
    Cmeq,
    /// `cmgt`: all ones where `n > m`, signed.
    Cmgt,
    /// `smax`.
    Smax,
    /// `smin`.
    Smin,
    /// `umax`.
    Umax,
    /// `umin`.
    Umin,
    /// `urhadd`: the rounded unsigned average.
    Urhadd,
    /// `uabd`: the unsigned absolute difference.
    Uabd,
    /// `mul`.
    Mul,
    /// `sshl`: shift by the signed low byte of each element of `m` (negative is right,
    /// arithmetic).
    Sshl,
    /// `ushl`: the same, logical.
    Ushl,
    /// `addp`: pairwise add of `n:m`.
    Addp,
}

/// `<op> v<d>.<T>, v<n>.<T>, v<m>.<T>` for the integer operations.
pub fn three_same(op: ThreeSame, element: Element, d: u8, n: u8, m: u8) -> u32 {
    let (unsigned, opcode) = match op {
        ThreeSame::Add => (0, 0b10000),
        ThreeSame::Sub => (1, 0b10000),
        ThreeSame::Sqadd => (0, 0b00001),
        ThreeSame::Uqadd => (1, 0b00001),
        ThreeSame::Sqsub => (0, 0b00101),
        ThreeSame::Uqsub => (1, 0b00101),
        ThreeSame::Cmeq => (1, 0b10001),
        ThreeSame::Cmgt => (0, 0b00110),
        ThreeSame::Smax => (0, 0b01100),
        ThreeSame::Smin => (0, 0b01101),
        ThreeSame::Umax => (1, 0b01100),
        ThreeSame::Umin => (1, 0b01101),
        ThreeSame::Urhadd => (1, 0b00010),
        ThreeSame::Uabd => (1, 0b01110),
        ThreeSame::Mul => (0, 0b10011),
        ThreeSame::Sshl => (0, 0b01000),
        ThreeSame::Ushl => (1, 0b01000),
        ThreeSame::Addp => (0, 0b10111),
    };
    0x4E20_0400 | (unsigned << 29) | element.size() | (opcode << 11) | vm(m) | vn(n) | vd(d)
}

/// The bitwise vector operations on `.16b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorLogic {
    /// `and`.
    And,
    /// `bic`: `n & !m`.
    Bic,
    /// `orr` (`mov` when `n == m`).
    Orr,
    /// `eor`.
    Eor,
    /// `bsl`: `d` selects `n` where set, `m` where clear.
    Bsl,
    /// `bif`: insert `n` where `m` is clear.
    Bif,
}

/// `<op> v<d>.16b, v<n>.16b, v<m>.16b`.
pub fn vector_logic(op: VectorLogic, d: u8, n: u8, m: u8) -> u32 {
    let (unsigned, size) = match op {
        VectorLogic::And => (0, 0),
        VectorLogic::Bic => (0, 1),
        VectorLogic::Orr => (0, 2),
        VectorLogic::Eor => (1, 0),
        VectorLogic::Bsl => (1, 1),
        VectorLogic::Bif => (1, 3),
    };
    0x4E20_1C00 | (unsigned << 29) | (size << 22) | vm(m) | vn(n) | vd(d)
}

/// `mov v<d>.16b, v<n>.16b`.
pub fn vector_move(d: u8, n: u8) -> u32 {
    vector_logic(VectorLogic::Orr, d, n, n)
}

/// The floating-point three-register vector operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatThreeSame {
    /// `fadd`.
    Fadd,
    /// `fsub`.
    Fsub,
    /// `fmul`.
    Fmul,
    /// `fdiv`.
    Fdiv,
    /// `fcmeq`: all ones where equal (false when unordered).
    Fcmeq,
    /// `fcmge`: all ones where `n >= m`.
    Fcmge,
    /// `fcmgt`: all ones where `n > m`.
    Fcmgt,
}

/// `<op> v<d>.4s|2d, v<n>, v<m>`.
pub fn float_three_same(op: FloatThreeSame, float: Float, d: u8, n: u8, m: u8) -> u32 {
    let (unsigned, high, opcode) = match op {
        FloatThreeSame::Fadd => (0, 0, 0b11010),
        FloatThreeSame::Fsub => (0, 1, 0b11010),
        FloatThreeSame::Fmul => (1, 0, 0b11011),
        FloatThreeSame::Fdiv => (1, 0, 0b11111),
        FloatThreeSame::Fcmeq => (0, 0, 0b11100),
        FloatThreeSame::Fcmge => (1, 0, 0b11100),
        FloatThreeSame::Fcmgt => (1, 1, 0b11100),
    };
    0x4E20_0400
        | (unsigned << 29)
        | (high << 23)
        | float.bit22()
        | (opcode << 11)
        | vm(m)
        | vn(n)
        | vd(d)
}

/// The floating-point two-register vector operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatUnary {
    /// `fcvtzs`: to signed integers of the same size, toward zero, saturating (NaN is 0).
    Fcvtzs,
    /// `scvtf`: from signed integers of the same size, in FPCR's rounding mode.
    Scvtf,
    /// `frintx`: round to an integral value in FPCR's rounding mode.
    Frintx,
    /// `fsqrt`.
    Fsqrt,
}

/// `<op> v<d>.4s|2d, v<n>`.
pub fn float_unary(op: FloatUnary, float: Float, d: u8, n: u8) -> u32 {
    let base = match op {
        FloatUnary::Fcvtzs => 0x4EA1_B800,
        FloatUnary::Scvtf => 0x4E21_D800,
        FloatUnary::Frintx => 0x6E21_9800,
        FloatUnary::Fsqrt => 0x6EA1_F800,
    };
    base | float.bit22() | vn(n) | vd(d)
}

/// The integer two-register vector operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    /// `not v<d>.16b`.
    Not,
    /// `fcvtl v<d>.2d, v<n>.2s`.
    Fcvtl,
    /// `fcvtn v<d>.2s, v<n>.2d` (the upper half of `d` becomes zero).
    Fcvtn,
    /// `xtn v<d>.<half>, v<n>.<T>`: the low half of each element of `n`'s element size
    /// (upper half of `d` zero).
    Xtn(Element),
    /// `sqxtn`: signed saturating narrow into the lower half (upper half zero).
    Sqxtn(Element),
    /// `sqxtn2`: the same into the upper half, the lower half kept.
    Sqxtn2(Element),
    /// `sqxtun`: signed to unsigned saturating narrow into the lower half.
    Sqxtun(Element),
    /// `sqxtun2`.
    Sqxtun2(Element),
    /// `uaddlp`: add adjacent pairs of the source's elements into elements twice as wide.
    Uaddlp(Element),
}

/// `<op> v<d>, v<n>`; the element of a narrowing form is the source's.
pub fn unary(op: Unary, d: u8, n: u8) -> u32 {
    let narrow = |element: Element| ((element as u32).saturating_sub(1)) << 22;
    let word = match op {
        Unary::Not => 0x6E20_5800,
        Unary::Fcvtl => 0x0E61_7800,
        Unary::Fcvtn => 0x0E61_6800,
        Unary::Xtn(element) => 0x0E21_2800 | narrow(element),
        Unary::Sqxtn(element) => 0x0E21_4800 | narrow(element),
        Unary::Sqxtn2(element) => 0x4E21_4800 | narrow(element),
        Unary::Sqxtun(element) => 0x2E21_2800 | narrow(element),
        Unary::Sqxtun2(element) => 0x6E21_2800 | narrow(element),
        Unary::Uaddlp(element) => 0x6E20_2800 | element.size(),
    };
    word | vn(n) | vd(d)
}

/// The permutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permute {
    /// `uzp1`: the even elements of `n:m`.
    Uzp1,
    /// `zip1`: the low halves interleaved.
    Zip1,
    /// `uzp2`: the odd elements.
    Uzp2,
    /// `zip2`: the high halves interleaved.
    Zip2,
}

/// `<op> v<d>.<T>, v<n>.<T>, v<m>.<T>`.
pub fn permute(op: Permute, element: Element, d: u8, n: u8, m: u8) -> u32 {
    let opcode = match op {
        Permute::Uzp1 => 0b001,
        Permute::Zip1 => 0b011,
        Permute::Uzp2 => 0b101,
        Permute::Zip2 => 0b111,
    };
    0x4E00_0800 | element.size() | (opcode << 12) | vm(m) | vn(n) | vd(d)
}

/// `smull`/`umull` (lower halves) or `smull2`/`umull2` (upper halves) of `n` and `m`'s
/// `element`s into elements twice as wide.
pub fn widening_multiply(signed: bool, upper: bool, element: Element, d: u8, n: u8, m: u8) -> u32 {
    0x0E20_C000
        | (u32::from(upper) << 30)
        | (u32::from(!signed) << 29)
        | element.size()
        | vm(m)
        | vn(n)
        | vd(d)
}

/// `ext v<d>.16b, v<n>.16b, v<m>.16b, #index`: bytes `index..16` of `n` then `0..index` of `m`.
pub fn ext(d: u8, n: u8, m: u8, index: u8) -> u32 {
    0x6E00_0000 | vm(m) | (u32::from(index & 15) << 11) | vn(n) | vd(d)
}

/// The shifts by an immediate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftImmediate {
    /// `shl`, 0 to element bits - 1.
    Shl,
    /// `ushr`, 1 to element bits.
    Ushr,
    /// `sshr`, 1 to element bits.
    Sshr,
    /// `usra`: unsigned shift right and accumulate, 1 to element bits.
    Usra,
    /// `sshll v<d>, v<n>.<lower half>, #shift`: sign-extend the lower half (`sxtl` with 0).
    Sshll,
}

/// `<op> v<d>.<T>, v<n>.<T>, #shift`; for `sshll` the element is the source's.
pub fn shift_immediate(op: ShiftImmediate, element: Element, d: u8, n: u8, shift: u8) -> u32 {
    let bits = u32::from(element.bits());
    let shift = u32::from(shift);
    let (base, field) = match op {
        ShiftImmediate::Shl => (0x4F00_5400, bits + shift),
        ShiftImmediate::Ushr => (0x6F00_0400, 2 * bits - shift),
        ShiftImmediate::Sshr => (0x4F00_0400, 2 * bits - shift),
        ShiftImmediate::Usra => (0x6F00_1400, 2 * bits - shift),
        ShiftImmediate::Sshll => (0x0F00_A400, bits + shift),
    };
    base | ((field & 0x7F) << 16) | vn(n) | vd(d)
}

/// `mov v<d>.<T>[index], v<n>.<T>[source]` (`ins`, element form).
pub fn insert_element(element: Element, d: u8, index: u8, n: u8, source: u8) -> u32 {
    let imm4 = (u32::from(source) << (element as u32)) & 15;
    0x6E00_0400 | (element.imm5(index) << 16) | (imm4 << 11) | vn(n) | vd(d)
}

/// `mov v<d>.<T>[index], w|x<n>` (`ins`, general form).
pub fn insert_general(element: Element, d: u8, index: u8, n: Reg) -> u32 {
    0x4E00_1C00 | (element.imm5(index) << 16) | gn(n) | vd(d)
}

/// `umov w|x<d>, v<n>.<T>[index]` (an `x` destination for doublewords), zero-extended.
pub fn move_to_general(element: Element, d: Reg, n: u8, index: u8) -> u32 {
    let wide = u32::from(element == Element::D) << 30;
    0x0E00_3C00 | wide | (element.imm5(index) << 16) | vn(n) | gd(d)
}

/// `dup v<d>.<T>, v<n>.<T>[index]`.
pub fn dup_element(element: Element, d: u8, n: u8, index: u8) -> u32 {
    0x4E00_0400 | (element.imm5(index) << 16) | vn(n) | vd(d)
}

/// `dup v<d>.<T>, w|x<n>`.
pub fn dup_general(element: Element, d: u8, n: Reg) -> u32 {
    0x4E00_0C00 | (element.imm5(0) << 16) | gn(n) | vd(d)
}

/// `movi v<d>.2d, #0`.
pub fn zero(d: u8) -> u32 {
    0x6F00_E400 | vd(d)
}

/// `movi v<d>.4s, #imm8, lsl #24`, or `mvni` (the inverse) when `inverted`.
pub fn move_top_byte(inverted: bool, d: u8, imm8: u8) -> u32 {
    let imm8 = u32::from(imm8);
    0x4F00_6400 | (u32::from(inverted) << 29) | ((imm8 >> 5) << 16) | ((imm8 & 31) << 5) | vd(d)
}

/// `fmov v<d>.4s|2d, #1.0`.
pub fn float_one(float: Float, d: u8) -> u32 {
    let double = u32::from(float == Float::Double) << 29;
    0x4F03_F600 | double | vd(d)
}

/// The scalar floating-point two-source operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    /// `fmul`.
    Fmul,
    /// `fdiv`.
    Fdiv,
    /// `fadd`.
    Fadd,
    /// `fsub`.
    Fsub,
}

/// `<op> s|d<d>, s|d<n>, s|d<m>`; the rest of `v<d>` becomes zero.
pub fn scalar(op: Scalar, float: Float, d: u8, n: u8, m: u8) -> u32 {
    let opcode = match op {
        Scalar::Fmul => 0,
        Scalar::Fdiv => 1,
        Scalar::Fadd => 2,
        Scalar::Fsub => 3,
    };
    0x1E20_0800 | float.bit22() | (opcode << 12) | vm(m) | vn(n) | vd(d)
}

/// The scalar floating-point one-source operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarUnary {
    /// `fmov` between vector registers (the rest of `v<d>` becomes zero).
    Fmov,
    /// `fabs`.
    Fabs,
    /// `fneg`.
    Fneg,
    /// `fsqrt`.
    Fsqrt,
    /// `frintx`: round to an integral value in FPCR's rounding mode.
    Frintx,
}

/// `<op> s|d<d>, s|d<n>`.
pub fn scalar_unary(op: ScalarUnary, float: Float, d: u8, n: u8) -> u32 {
    let opcode = match op {
        ScalarUnary::Fmov => 0,
        ScalarUnary::Fabs => 1,
        ScalarUnary::Fneg => 2,
        ScalarUnary::Fsqrt => 3,
        ScalarUnary::Frintx => 0b001110,
    };
    0x1E20_4000 | float.bit22() | (opcode << 15) | vn(n) | vd(d)
}

/// `fcvt` from `from` to `to` precision.
pub fn convert_precision(to: Float, from: Float, d: u8, n: u8) -> u32 {
    let opc = u32::from(to == Float::Double) << 15;
    0x1E22_4000 | from.bit22() | opc | vn(n) | vd(d)
}

/// `fcmp s|d<n>, s|d<m>`: equal Z C, less N, greater C, unordered C V.
pub fn fcmp(float: Float, n: u8, m: u8) -> u32 {
    0x1E20_2000 | float.bit22() | vm(m) | vn(n)
}

/// `scvtf s|d<d>, w|x<n>`.
pub fn convert_to_float(size: Size, float: Float, d: u8, n: Reg) -> u32 {
    0x1E22_0000 | sf(size) | float.bit22() | gn(n) | vd(d)
}

/// `ucvtf s|d<d>, w|x<n>`: the register read as unsigned.
pub fn convert_unsigned_to_float(size: Size, float: Float, d: u8, n: Reg) -> u32 {
    0x1E23_0000 | sf(size) | float.bit22() | gn(n) | vd(d)
}

/// `fcvtzs w|x<d>, s|d<n>`: toward zero, saturating, NaN is 0.
pub fn convert_to_integer(size: Size, float: Float, d: Reg, n: u8) -> u32 {
    0x1E38_0000 | sf(size) | float.bit22() | vn(n) | gd(d)
}

/// `fmov s<d>, w<n>` or `fmov d<d>, x<n>`: the rest of `v<d>` becomes zero.
pub fn move_from_general(size: Size, d: u8, n: Reg) -> u32 {
    match size {
        Size::W => 0x1E27_0000 | gn(n) | vd(d),
        Size::X => 0x9E67_0000 | gn(n) | vd(d),
    }
}

/// `fmov w<d>, s<n>` or `fmov x<d>, d<n>`.
pub fn move_to_general_low(size: Size, d: Reg, n: u8) -> u32 {
    match size {
        Size::W => 0x1E26_0000 | vn(n) | gd(d),
        Size::X => 0x9E66_0000 | vn(n) | gd(d),
    }
}

fn sf(size: Size) -> u32 {
    match size {
        Size::W => 0,
        Size::X => 1 << 31,
    }
}

/// The SIMD and floating-point loads and stores with an unsigned scaled offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorLoadStore {
    /// `ldr s`.
    LdrS,
    /// `ldr d`.
    LdrD,
    /// `ldr q`.
    LdrQ,
    /// `str s`.
    StrS,
    /// `str d`.
    StrD,
    /// `str q`.
    StrQ,
}

/// `ldr|str s|d|q<t>, [x<n>, #offset]`, or `None` when the offset is not a multiple of the
/// access size below 4096 accesses.
pub fn vector_load_store(op: VectorLoadStore, t: u8, n: Reg, offset: u32) -> Option<u32> {
    let (base, scale) = match op {
        VectorLoadStore::LdrS => (0xBD40_0000, 4),
        VectorLoadStore::LdrD => (0xFD40_0000, 8),
        VectorLoadStore::LdrQ => (0x3DC0_0000, 16),
        VectorLoadStore::StrS => (0xBD00_0000, 4),
        VectorLoadStore::StrD => (0xFD00_0000, 8),
        VectorLoadStore::StrQ => (0x3D80_0000, 16),
    };
    if !offset.is_multiple_of(scale) || offset / scale >= 4096 {
        return None;
    }
    Some(base | ((offset / scale) << 10) | gn(n) | vd(t))
}

/// `mrs x<t>, fpcr`.
pub fn mrs_fpcr(t: Reg) -> u32 {
    0xD53B_4400 | gd(t)
}

/// `msr fpcr, x<t>`.
pub fn msr_fpcr(t: Reg) -> u32 {
    0xD51B_4400 | gd(t)
}

/// `mrs x<t>, cntvct_el0`: the virtual counter.
pub fn mrs_cntvct(t: Reg) -> u32 {
    0xD53B_E040 | gd(t)
}

/// `mrs x<t>, cntfrq_el0`: the counter's frequency in Hz.
pub fn mrs_cntfrq(t: Reg) -> u32 {
    0xD53B_E000 | gd(t)
}

/// `dc civac, x<t>`: clean and invalidate the data cache line to the point of coherency.
pub fn dc_civac(t: Reg) -> u32 {
    0xD50B_7E20 | gd(t)
}

/// `caspal x<s>, x<s+1>, x<t>, x<t+1>, [x<n>]` (FEAT_LSE; `s` and `t` even): compares the
/// pair at `n` with `s:s+1`, stores `t:t+1` when equal, and returns the old pair in `s:s+1`.
pub fn caspal(s: Reg, t: Reg, n: Reg) -> u32 {
    0x4860_FC00 | (u32::from(s.0 & 31) << 16) | gn(n) | gd(t)
}

/// `ldaxp x<t1>, x<t2>, [x<n>]`.
pub fn ldaxp(t1: Reg, t2: Reg, n: Reg) -> u32 {
    0xC87F_8000 | (u32::from(t2.0 & 31) << 10) | gn(n) | gd(t1)
}

/// `stlxp w<s>, x<t1>, x<t2>, [x<n>]`: `s` is 0 when the store happened.
pub fn stlxp(s: Reg, t1: Reg, t2: Reg, n: Reg) -> u32 {
    0xC820_8000 | (u32::from(s.0 & 31) << 16) | (u32::from(t2.0 & 31) << 10) | gn(n) | gd(t1)
}
