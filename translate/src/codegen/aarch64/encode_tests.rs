use super::encode::*;
use super::{Access, Cond, Reg, Shift, Size};

const X0: Reg = Reg(0);
const X1: Reg = Reg(1);
const X2: Reg = Reg(2);
const X3: Reg = Reg(3);
const X16: Reg = Reg(16);
const X17: Reg = Reg(17);
const X20: Reg = Reg(20);
const X21: Reg = Reg(21);
const X22: Reg = Reg(22);
const X23: Reg = Reg(23);

#[test]
fn add_and_subtract() {
    assert_eq!(
        add_sub_shifted(AddSub::Add, Size::W, X0, X0, X3, Shift::Lsl, 0),
        0x0B03_0000
    );
    assert_eq!(
        add_sub_immediate(AddSub::Adds, Size::X, X0, X1, 1, false),
        0xB100_0420
    );
    assert_eq!(
        add_sub_shifted(AddSub::Subs, Size::W, X16, X16, X17, Shift::Lsl, 24),
        0x6B11_6210
    );
    assert_eq!(
        add_sub_immediate(AddSub::Subs, Size::X, Reg::ZR, X0, 0, false),
        0xF100_001F
    );
}

#[test]
fn logical_and_moves() {
    assert_eq!(
        logical_shifted(Logic::Orr, false, Size::X, X0, Reg::ZR, X1, Shift::Lsl, 0),
        0xAA01_03E0
    );
    assert_eq!(
        move_wide(MoveWide::Movz, Size::X, X0, 0x1234, 1),
        0xD2A2_4680
    );
    assert_eq!(
        move_wide(MoveWide::Movk, Size::X, X0, 0xFFFF, 3),
        0xF2FF_FFE0
    );
    assert_eq!(
        logical_immediate(Logic::And, Size::W, X0, X1, 0xFF),
        Some(0x1200_1C20)
    );
    assert_eq!(
        logical_immediate(Logic::Orr, Size::X, X0, Reg::ZR, 0x5555_5555_5555_5555),
        Some(0xB200_F3E0)
    );
    assert_eq!(
        logical_immediate(Logic::And, Size::X, X0, X0, !1),
        Some(0x927F_F800)
    );
}

#[test]
fn values_without_a_bitmask_encoding() {
    assert_eq!(bitmask_immediate(0, Size::X), None);
    assert_eq!(bitmask_immediate(u64::MAX, Size::X), None);
    assert_eq!(bitmask_immediate(0xFFFF_FFFF, Size::W), None);
    assert_eq!(bitmask_immediate(5, Size::X), None);
}

#[test]
fn bitfields() {
    assert_eq!(
        bitfield(Bitfield::Ubfm, Size::W, X16, X0, 8, 7),
        0x5308_1C10
    );
    assert_eq!(
        bitfield(Bitfield::Ubfm, Size::W, X16, X0, 8, 15),
        0x5308_3C10
    );
    assert_eq!(bitfield(Bitfield::Bfm, Size::X, X0, X16, 0, 7), 0xB340_1E00);
    assert_eq!(
        bitfield(Bitfield::Sbfm, Size::W, X16, X0, 0, 7),
        0x1300_1C10
    );
    assert_eq!(extr(Size::W, X0, X1, X2, 8), 0x1382_2020);
    assert_eq!(adr(X17, 12), 0x1000_0071);
}

#[test]
fn carries_selects_and_data_processing() {
    assert_eq!(carry(Carry::Adc, Size::W, X0, X0, X1), 0x1A01_0000);
    assert_eq!(carry(Carry::Sbcs, Size::X, X0, X1, X2), 0xFA02_0020);
    assert_eq!(
        select(Select::Csel, Size::W, X0, X1, X0, Cond::Eq),
        0x1A80_0020
    );
    assert_eq!(cset(Size::W, X16, Cond::Ne), 0x1A9F_07F0);
    assert_eq!(
        two_source(TwoSource::Udiv, Size::X, X0, X1, X2),
        0x9AC2_0820
    );
    assert_eq!(
        two_source(TwoSource::Lslv, Size::W, X0, X1, X2),
        0x1AC2_2020
    );
    assert_eq!(one_source(OneSource::Rev, Size::W, X0, X0), 0x5AC0_0800);
    assert_eq!(one_source(OneSource::Rev, Size::X, X0, X0), 0xDAC0_0C00);
    assert_eq!(one_source(OneSource::Clz, Size::X, X0, X1), 0xDAC0_1020);
    assert_eq!(one_source(OneSource::Rbit, Size::W, X0, X1), 0x5AC0_0020);
    assert_eq!(
        multiply(Multiply::Madd, Size::X, X0, X1, X2, Reg::ZR),
        0x9B02_7C20
    );
    assert_eq!(
        multiply(Multiply::Umulh, Size::X, X0, X1, X2, Reg::ZR),
        0x9BC2_7C20
    );
    assert_eq!(
        multiply(Multiply::Smaddl, Size::X, X0, X1, X2, Reg::ZR),
        0x9B22_7C20
    );
}

#[test]
fn loads_and_stores() {
    assert_eq!(load_store(LoadStore::LdrX, X0, X1, 0), Some(0xF940_0020));
    assert_eq!(load_store(LoadStore::Strb, X0, X16, 0), Some(0x3900_0200));
    assert_eq!(load_store(LoadStore::LdrW, X0, X21, 8), Some(0xB940_0AA0));
    assert_eq!(load_store(LoadStore::LdrW, X0, X21, 6), None);
    assert_eq!(load_store(LoadStore::LdrX, X0, X21, 8 * 4096), None);
    assert_eq!(load_literal(X16, 4), 0x5800_0090);
    assert_eq!(pair(Pair::Stp, X16, X17, X20, 0), 0xA900_4690);
    assert_eq!(pair(Pair::Ldp, X17, X22, X20, 0), 0xA940_5A91);
}

#[test]
fn ordered_exclusive_and_atomic_accesses() {
    assert_eq!(ordered(Ordered::Ldar, Access::X, X0, X16), 0xC8DF_FE00);
    assert_eq!(ordered(Ordered::Stlr, Access::W, X0, X16), 0x889F_FE00);
    assert_eq!(ordered(Ordered::Ldapr, Access::X, X0, X16), 0xF8BF_C200);
    assert_eq!(ordered(Ordered::Ldaxr, Access::W, X22, X16), 0x885F_FE16);
    assert_eq!(stlxr(Access::W, X17, X23, X16), 0x8811_FE17);
    assert_eq!(atomic(Atomic::Ldadd, Access::X, X1, X0, X16), 0xF8E1_0200);
    assert_eq!(atomic(Atomic::Swp, Access::W, X1, X0, X16), 0xB8E1_8200);
    assert_eq!(casal(Access::W, X0, X1, X16), 0x88E0_FE01);
}

#[test]
fn branches_and_system() {
    assert_eq!(b(2), 0x1400_0002);
    assert_eq!(b_cond(Cond::Ne, -1), 0x54FF_FFE1);
    assert_eq!(cbz(Size::W, X0, 2, false), 0x3400_0040);
    assert_eq!(tbz(X16, 0, 3, true), 0x3700_0070);
    assert_eq!(br(X22), 0xD61F_02C0);
    assert_eq!(brk(1), 0xD420_0020);
    assert_eq!(dmb(Barrier::Ish), 0xD503_3BBF);
    assert_eq!(dmb(Barrier::IshSt), 0xD503_3ABF);
    assert_eq!(dmb(Barrier::IshLd), 0xD503_39BF);
    assert_eq!(mrs_nzcv(X16), 0xD53B_4210);
    assert_eq!(msr_nzcv(X16), 0xD51B_4210);
}

#[test]
fn population_count_sequence() {
    assert_eq!(fmov_d_from_x(31, X16), 0x9E67_021F);
    assert_eq!(cnt_8b(31, 31), 0x0E20_5BFF);
    assert_eq!(addv_8b(31, 31), 0x0E31_BBFF);
    assert_eq!(fmov_w_from_s(X16, 31), 0x1E26_03F0);
}

#[test]
fn post_index_loads_and_stores() {
    assert_eq!(
        load_store_post_index(true, Access::B, X0, X1, 1),
        0x3840_1420
    );
    assert_eq!(
        load_store_post_index(false, Access::B, X0, X1, -1),
        0x381F_F420
    );
    assert_eq!(
        load_store_post_index(true, Access::X, X2, X3, 8),
        0xF840_8462
    );
    assert_eq!(
        load_store_post_index(false, Access::W, X0, X1, -4),
        0xB81F_C420
    );
}
