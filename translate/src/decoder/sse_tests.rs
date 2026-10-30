use super::tests::{at, check, decoded, error, general, refused, xmm};
use super::{DecodeErrorKind, Refusal};
use crate::instruction::{Gpr, Mnemonic, Operand, Width};

#[test]
fn scalar_and_packed_moves_follow_the_mandatory_prefix() {
    let at_rsp = |width| at(Gpr::RSP, 0x10, width);
    check(
        &[0xF2, 0x0F, 0x10, 0x44, 0x24, 0x10],
        Mnemonic::Movsd,
        Width::Qword,
        &[xmm(0), at_rsp(Width::Qword)],
    );
    check(
        &[0xF3, 0x0F, 0x10, 0x44, 0x24, 0x10],
        Mnemonic::Movss,
        Width::Dword,
        &[xmm(0), at_rsp(Width::Dword)],
    );
    check(
        &[0x0F, 0x10, 0xC1],
        Mnemonic::Movups,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0x10, 0xC1],
        Mnemonic::Movupd,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0xF2, 0x0F, 0x11, 0x07],
        Mnemonic::Movsd,
        Width::Qword,
        &[at(Gpr::RDI, 0, Width::Qword), xmm(0)],
    );
    check(
        &[0x0F, 0x28, 0xC1],
        Mnemonic::Movaps,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0x6F, 0x06],
        Mnemonic::Movdqa,
        Width::Dqword,
        &[xmm(0), at(Gpr::RSI, 0, Width::Dqword)],
    );
    check(
        &[0xF3, 0x0F, 0x7F, 0x07],
        Mnemonic::Movdqu,
        Width::Dqword,
        &[at(Gpr::RDI, 0, Width::Dqword), xmm(0)],
    );
    assert_eq!(decoded(&[0xF2, 0x0F, 0x10, 0xC1]).repeat, None);
}

#[test]
fn rex_reaches_xmm8_to_xmm15() {
    check(
        &[0x66, 0x44, 0x0F, 0xFE, 0xCA],
        Mnemonic::Paddd,
        Width::Dqword,
        &[xmm(9), xmm(2)],
    );
    check(
        &[0x45, 0x0F, 0x57, 0xFF],
        Mnemonic::Xorps,
        Width::Dqword,
        &[xmm(15), xmm(15)],
    );
}

#[test]
fn arithmetic_families_pick_packed_or_scalar_by_prefix() {
    let families = [
        (
            0x58,
            [
                Mnemonic::Addps,
                Mnemonic::Addpd,
                Mnemonic::Addss,
                Mnemonic::Addsd,
            ],
        ),
        (
            0x59,
            [
                Mnemonic::Mulps,
                Mnemonic::Mulpd,
                Mnemonic::Mulss,
                Mnemonic::Mulsd,
            ],
        ),
        (
            0x5C,
            [
                Mnemonic::Subps,
                Mnemonic::Subpd,
                Mnemonic::Subss,
                Mnemonic::Subsd,
            ],
        ),
        (
            0x5D,
            [
                Mnemonic::Minps,
                Mnemonic::Minpd,
                Mnemonic::Minss,
                Mnemonic::Minsd,
            ],
        ),
        (
            0x5E,
            [
                Mnemonic::Divps,
                Mnemonic::Divpd,
                Mnemonic::Divss,
                Mnemonic::Divsd,
            ],
        ),
        (
            0x5F,
            [
                Mnemonic::Maxps,
                Mnemonic::Maxpd,
                Mnemonic::Maxss,
                Mnemonic::Maxsd,
            ],
        ),
        (
            0x51,
            [
                Mnemonic::Sqrtps,
                Mnemonic::Sqrtpd,
                Mnemonic::Sqrtss,
                Mnemonic::Sqrtsd,
            ],
        ),
    ];
    for (opcode, [ps, pd, ss, sd]) in families {
        check(&[0x0F, opcode, 0xC1], ps, Width::Dqword, &[xmm(0), xmm(1)]);
        check(
            &[0x66, 0x0F, opcode, 0xC1],
            pd,
            Width::Dqword,
            &[xmm(0), xmm(1)],
        );
        check(
            &[0xF3, 0x0F, opcode, 0xC1],
            ss,
            Width::Dword,
            &[xmm(0), xmm(1)],
        );
        check(
            &[0xF2, 0x0F, opcode, 0xC1],
            sd,
            Width::Qword,
            &[xmm(0), xmm(1)],
        );
    }
    check(
        &[0x0F, 0xC2, 0xC1, 0x01],
        Mnemonic::Cmpps,
        Width::Dqword,
        &[xmm(0), xmm(1), Operand::Immediate(1)],
    );
    check(
        &[0xF2, 0x0F, 0xC2, 0xC1, 0x07],
        Mnemonic::Cmpsd,
        Width::Qword,
        &[xmm(0), xmm(1), Operand::Immediate(7)],
    );
}

#[test]
fn conversions_use_general_registers_at_their_operand_size() {
    check(
        &[0xF2, 0x48, 0x0F, 0x2A, 0xC8],
        Mnemonic::Cvtsi2sd,
        Width::Qword,
        &[xmm(1), general(Gpr::RAX, Width::Qword)],
    );
    check(
        &[0xF3, 0x0F, 0x2A, 0xC8],
        Mnemonic::Cvtsi2ss,
        Width::Dword,
        &[xmm(1), general(Gpr::RAX, Width::Dword)],
    );
    check(
        &[0xF2, 0x48, 0x0F, 0x2C, 0xC1],
        Mnemonic::Cvttsd2si,
        Width::Qword,
        &[general(Gpr::RAX, Width::Qword), xmm(1)],
    );
    check(
        &[0xF3, 0x0F, 0x2D, 0x06],
        Mnemonic::Cvtss2si,
        Width::Dword,
        &[
            general(Gpr::RAX, Width::Dword),
            at(Gpr::RSI, 0, Width::Dword),
        ],
    );
    check(
        &[0xF3, 0x0F, 0x5A, 0xC1],
        Mnemonic::Cvtss2sd,
        Width::Dword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x0F, 0x5B, 0xC1],
        Mnemonic::Cvtdq2ps,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0xF3, 0x0F, 0x5B, 0xC1],
        Mnemonic::Cvttps2dq,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0xF3, 0x0F, 0xE6, 0xC1],
        Mnemonic::Cvtdq2pd,
        Width::Qword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x0F, 0x2E, 0xC1],
        Mnemonic::Ucomiss,
        Width::Dword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0x2F, 0xC1],
        Mnemonic::Comisd,
        Width::Qword,
        &[xmm(0), xmm(1)],
    );
}

#[test]
fn movd_and_movq_move_between_general_and_xmm_registers() {
    let eax = general(Gpr::RAX, Width::Dword);
    let rax = general(Gpr::RAX, Width::Qword);
    check(
        &[0x66, 0x0F, 0x6E, 0xC0],
        Mnemonic::Movd,
        Width::Dword,
        &[xmm(0), eax],
    );
    check(
        &[0x66, 0x48, 0x0F, 0x6E, 0xC0],
        Mnemonic::Movq,
        Width::Qword,
        &[xmm(0), rax],
    );
    check(
        &[0x66, 0x0F, 0x7E, 0xC0],
        Mnemonic::Movd,
        Width::Dword,
        &[eax, xmm(0)],
    );
    check(
        &[0x66, 0x48, 0x0F, 0x7E, 0xC0],
        Mnemonic::Movq,
        Width::Qword,
        &[rax, xmm(0)],
    );
    check(
        &[0xF3, 0x0F, 0x7E, 0xC1],
        Mnemonic::Movq,
        Width::Qword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0xD6, 0x07],
        Mnemonic::Movq,
        Width::Qword,
        &[at(Gpr::RDI, 0, Width::Qword), xmm(0)],
    );
}

#[test]
fn half_register_moves_split_on_the_modrm_mode() {
    check(
        &[0x0F, 0x12, 0xC1],
        Mnemonic::Movhlps,
        Width::Qword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x0F, 0x12, 0x00],
        Mnemonic::Movlps,
        Width::Qword,
        &[xmm(0), at(Gpr::RAX, 0, Width::Qword)],
    );
    check(
        &[0x0F, 0x16, 0xC1],
        Mnemonic::Movlhps,
        Width::Qword,
        &[xmm(0), xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0x17, 0x00],
        Mnemonic::Movhpd,
        Width::Qword,
        &[at(Gpr::RAX, 0, Width::Qword), xmm(0)],
    );
    assert_eq!(error(&[0x66, 0x0F, 0x12, 0xC1]), DecodeErrorKind::Invalid);
}

#[test]
fn shuffles_shifts_and_extracts_take_an_immediate() {
    check(
        &[0x66, 0x0F, 0x70, 0xC1, 0x1B],
        Mnemonic::Pshufd,
        Width::Dqword,
        &[xmm(0), xmm(1), Operand::Immediate(0x1B)],
    );
    check(
        &[0x66, 0x0F, 0x73, 0xDA, 0x08],
        Mnemonic::Psrldq,
        Width::Dqword,
        &[xmm(2), Operand::Immediate(8)],
    );
    check(
        &[0x66, 0x0F, 0x72, 0xE1, 0x1F],
        Mnemonic::Psrad,
        Width::Dqword,
        &[xmm(1), Operand::Immediate(0x1F)],
    );
    check(
        &[0x66, 0x0F, 0xC5, 0xC1, 0x03],
        Mnemonic::Pextrw,
        Width::Dword,
        &[
            general(Gpr::RAX, Width::Dword),
            xmm(1),
            Operand::Immediate(3),
        ],
    );
    check(
        &[0x66, 0x0F, 0xC4, 0xC0, 0x02],
        Mnemonic::Pinsrw,
        Width::Word,
        &[
            xmm(0),
            general(Gpr::RAX, Width::Word),
            Operand::Immediate(2),
        ],
    );
    check(
        &[0x0F, 0xC6, 0xC1, 0x44],
        Mnemonic::Shufps,
        Width::Dqword,
        &[xmm(0), xmm(1), Operand::Immediate(0x44)],
    );
    check(
        &[0x66, 0x0F, 0xD2, 0xC1],
        Mnemonic::Psrld,
        Width::Dqword,
        &[xmm(0), xmm(1)],
    );
    assert_eq!(
        error(&[0x66, 0x0F, 0x73, 0xC1, 0x08]),
        DecodeErrorKind::Invalid
    );
    assert_eq!(
        error(&[0x66, 0x0F, 0x73, 0x10, 0x08]),
        DecodeErrorKind::Invalid
    );
}

#[test]
fn masks_go_to_general_registers() {
    let eax = general(Gpr::RAX, Width::Dword);
    check(
        &[0x0F, 0x50, 0xC1],
        Mnemonic::Movmskps,
        Width::Dword,
        &[eax, xmm(1)],
    );
    check(
        &[0x66, 0x0F, 0xD7, 0xC1],
        Mnemonic::Pmovmskb,
        Width::Dword,
        &[eax, xmm(1)],
    );
    assert_eq!(error(&[0x0F, 0x50, 0x01]), DecodeErrorKind::Invalid);
}

#[test]
fn the_integer_block_decodes_with_the_66_prefix() {
    let ops = [
        (0xD4, Mnemonic::Paddq),
        (0xDB, Mnemonic::Pand),
        (0xDF, Mnemonic::Pandn),
        (0xEB, Mnemonic::Por),
        (0xEF, Mnemonic::Pxor),
        (0xF4, Mnemonic::Pmuludq),
        (0xFA, Mnemonic::Psubd),
        (0xFC, Mnemonic::Paddb),
        (0x60, Mnemonic::Punpcklbw),
        (0x6C, Mnemonic::Punpcklqdq),
        (0x74, Mnemonic::Pcmpeqb),
        (0x76, Mnemonic::Pcmpeqd),
    ];
    for (opcode, mnemonic) in ops {
        check(
            &[0x66, 0x0F, opcode, 0xC1],
            mnemonic,
            Width::Dqword,
            &[xmm(0), xmm(1)],
        );
    }
    assert_eq!(error(&[0x66, 0x0F, 0xF0, 0xC1]), DecodeErrorKind::Invalid);
}

#[test]
fn control_fences_and_non_temporal_stores_decode() {
    check(
        &[0x0F, 0xAE, 0x10],
        Mnemonic::Ldmxcsr,
        Width::Dword,
        &[at(Gpr::RAX, 0, Width::Dword)],
    );
    check(
        &[0x0F, 0xAE, 0x18],
        Mnemonic::Stmxcsr,
        Width::Dword,
        &[at(Gpr::RAX, 0, Width::Dword)],
    );
    check(&[0x0F, 0xAE, 0xF0], Mnemonic::Mfence, Width::Byte, &[]);
    check(&[0x0F, 0xAE, 0xE8], Mnemonic::Lfence, Width::Byte, &[]);
    check(&[0x0F, 0xAE, 0xF8], Mnemonic::Sfence, Width::Byte, &[]);
    check(
        &[0x0F, 0xAE, 0x38],
        Mnemonic::Clflush,
        Width::Byte,
        &[at(Gpr::RAX, 0, Width::Byte)],
    );
    check(
        &[0x66, 0x0F, 0xE7, 0x07],
        Mnemonic::Movntdq,
        Width::Dqword,
        &[at(Gpr::RDI, 0, Width::Dqword), xmm(0)],
    );
    check(
        &[0x0F, 0xC3, 0x07],
        Mnemonic::Movnti,
        Width::Dword,
        &[
            at(Gpr::RDI, 0, Width::Dword),
            general(Gpr::RAX, Width::Dword),
        ],
    );
}

#[test]
fn mmx_and_sse3_forms_are_refused_with_their_length() {
    assert_eq!(refused(&[0x0F, 0xFC, 0xC1]), Refusal::Mmx);
    assert_eq!(refused(&[0x0F, 0x6F, 0x06]), Refusal::Mmx);
    assert_eq!(refused(&[0x0F, 0x70, 0xC1, 0x1B]), Refusal::Mmx);
    assert_eq!(refused(&[0x0F, 0x77]), Refusal::Mmx);
    assert_eq!(refused(&[0x0F, 0x2A, 0xC1]), Refusal::Mmx);
    assert_eq!(refused(&[0xF2, 0x0F, 0x12, 0xC1]), Refusal::Sse3);
    assert_eq!(refused(&[0x66, 0x0F, 0x7C, 0xC1]), Refusal::Sse3);
    assert_eq!(refused(&[0xF2, 0x0F, 0xF0, 0x06]), Refusal::Sse3);
}
