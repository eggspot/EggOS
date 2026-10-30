use super::simd::*;
use super::{Reg, Size};

const X0: Reg = Reg(0);
const X1: Reg = Reg(1);
const X2: Reg = Reg(2);
const X3: Reg = Reg(3);
const X4: Reg = Reg(4);

#[test]
fn integer_vector_arithmetic() {
    assert_eq!(three_same(ThreeSame::Add, Element::S, 0, 1, 2), 0x4EA2_8420);
    assert_eq!(three_same(ThreeSame::Sub, Element::B, 0, 1, 2), 0x6E22_8420);
    assert_eq!(
        three_same(ThreeSame::Cmeq, Element::S, 0, 1, 2),
        0x6EA2_8C20
    );
    assert_eq!(
        three_same(ThreeSame::Cmgt, Element::H, 0, 1, 2),
        0x4E62_3420
    );
    assert_eq!(
        three_same(ThreeSame::Uqadd, Element::B, 0, 1, 2),
        0x6E22_0C20
    );
    assert_eq!(three_same(ThreeSame::Mul, Element::H, 0, 1, 2), 0x4E62_9C20);
    assert_eq!(
        three_same(ThreeSame::Ushl, Element::D, 0, 1, 2),
        0x6EE2_4420
    );
    assert_eq!(
        three_same(ThreeSame::Addp, Element::S, 0, 1, 2),
        0x4EA2_BC20
    );
}

#[test]
fn vector_logic_forms() {
    assert_eq!(vector_logic(VectorLogic::And, 0, 1, 2), 0x4E22_1C20);
    assert_eq!(vector_logic(VectorLogic::Eor, 0, 0, 0), 0x6E20_1C00);
    assert_eq!(vector_logic(VectorLogic::Bic, 0, 1, 2), 0x4E62_1C20);
    assert_eq!(vector_logic(VectorLogic::Bif, 0, 1, 2), 0x6EE2_1C20);
    assert_eq!(vector_move(0, 1), 0x4EA1_1C20);
}

#[test]
fn float_vector_forms() {
    assert_eq!(
        float_three_same(FloatThreeSame::Fadd, Float::Single, 0, 1, 2),
        0x4E22_D420
    );
    assert_eq!(
        float_three_same(FloatThreeSame::Fadd, Float::Double, 0, 1, 2),
        0x4E62_D420
    );
    assert_eq!(
        float_three_same(FloatThreeSame::Fcmgt, Float::Single, 0, 1, 2),
        0x6EA2_E420
    );
    assert_eq!(
        float_unary(FloatUnary::Fcvtzs, Float::Single, 0, 1),
        0x4EA1_B820
    );
    assert_eq!(
        float_unary(FloatUnary::Scvtf, Float::Double, 0, 1),
        0x4E61_D820
    );
    assert_eq!(unary(Unary::Fcvtl, 0, 1), 0x0E61_7820);
    assert_eq!(unary(Unary::Xtn(Element::D), 0, 1), 0x0EA1_2820);
    assert_eq!(unary(Unary::Sqxtun(Element::H), 0, 1), 0x2E21_2820);
    assert_eq!(float_one(Float::Single, 0), 0x4F03_F600);
}

#[test]
fn moves_between_elements_and_registers() {
    assert_eq!(insert_element(Element::S, 0, 1, 1, 0), 0x6E0C_0420);
    assert_eq!(insert_general(Element::D, 0, 1, X1), 0x4E18_1C20);
    assert_eq!(move_to_general(Element::H, X0, 1, 3), 0x0E0E_3C20);
    assert_eq!(move_to_general(Element::D, X0, 1, 1), 0x4E18_3C20);
    assert_eq!(dup_general(Element::B, 0, X1), 0x4E01_0C20);
    assert_eq!(zero(0), 0x6F00_E400);
    assert_eq!(move_top_byte(false, 0, 0x80), 0x4F04_6400);
    assert_eq!(permute(Permute::Zip1, Element::B, 0, 1, 2), 0x4E02_3820);
    assert_eq!(ext(0, 1, 2, 8), 0x6E02_4020);
    assert_eq!(
        widening_multiply(true, false, Element::H, 0, 1, 2),
        0x0E62_C020
    );
}

#[test]
fn shifts_by_immediate() {
    assert_eq!(
        shift_immediate(ShiftImmediate::Ushr, Element::B, 0, 1, 7),
        0x6F09_0420
    );
    assert_eq!(
        shift_immediate(ShiftImmediate::Shl, Element::S, 0, 1, 3),
        0x4F23_5420
    );
    assert_eq!(
        shift_immediate(ShiftImmediate::Sshr, Element::H, 0, 1, 16),
        0x4F10_0420
    );
    assert_eq!(
        shift_immediate(ShiftImmediate::Sshll, Element::S, 0, 1, 0),
        0x0F20_A420
    );
}

#[test]
fn scalar_floating_point() {
    assert_eq!(scalar(Scalar::Fadd, Float::Single, 0, 1, 2), 0x1E22_2820);
    assert_eq!(scalar(Scalar::Fadd, Float::Double, 0, 1, 2), 0x1E62_2820);
    assert_eq!(scalar(Scalar::Fmul, Float::Single, 0, 1, 2), 0x1E22_0820);
    assert_eq!(
        scalar_unary(ScalarUnary::Fsqrt, Float::Single, 0, 1),
        0x1E21_C020
    );
    assert_eq!(
        scalar_unary(ScalarUnary::Frintx, Float::Single, 0, 1),
        0x1E27_4020
    );
    assert_eq!(
        convert_precision(Float::Double, Float::Single, 0, 1),
        0x1E22_C020
    );
    assert_eq!(
        convert_precision(Float::Single, Float::Double, 0, 1),
        0x1E62_4020
    );
    assert_eq!(fcmp(Float::Single, 0, 1), 0x1E21_2000);
    assert_eq!(fcmp(Float::Double, 0, 1), 0x1E61_2000);
    assert_eq!(convert_to_float(Size::X, Float::Double, 0, X1), 0x9E62_0020);
    assert_eq!(
        convert_to_integer(Size::W, Float::Single, X0, 1),
        0x1E38_0020
    );
    assert_eq!(
        convert_to_integer(Size::X, Float::Double, X0, 1),
        0x9E78_0020
    );
    assert_eq!(move_from_general(Size::X, 0, X1), 0x9E67_0020);
    assert_eq!(move_to_general_low(Size::W, X0, 1), 0x1E26_0020);
}

#[test]
fn vector_loads_and_stores() {
    assert_eq!(
        vector_load_store(VectorLoadStore::LdrQ, 0, X0, 0),
        Some(0x3DC0_0000)
    );
    assert_eq!(
        vector_load_store(VectorLoadStore::StrQ, 0, X0, 0),
        Some(0x3D80_0000)
    );
    assert_eq!(
        vector_load_store(VectorLoadStore::LdrS, 0, X1, 4),
        Some(0xBD40_0420)
    );
    assert_eq!(vector_load_store(VectorLoadStore::LdrD, 0, X1, 4), None);
}

#[test]
fn system_and_pair_forms() {
    assert_eq!(mrs_fpcr(X0), 0xD53B_4400);
    assert_eq!(msr_fpcr(X0), 0xD51B_4400);
    assert_eq!(mrs_cntvct(X0), 0xD53B_E040);
    assert_eq!(mrs_cntfrq(X0), 0xD53B_E000);
    assert_eq!(dc_civac(X0), 0xD50B_7E20);
    assert_eq!(caspal(X0, X2, X4), 0x4860_FC82);
    assert_eq!(ldaxp(X0, X1, X2), 0xC87F_8440);
    assert_eq!(stlxp(X3, X0, X1, X2), 0xC823_8440);
}
