use super::form::{Form, Immediate, SizeRule, Spec, V, W};
use super::modrm::{is_register_form, reg_field};
use super::prefix::{Mandatory, Prefixes};
use super::{DecodeErrorKind, Refusal};
use crate::instruction::{Mnemonic, Width};

const INTEGER_FIRST: u8 = 0xD1;

const INTEGER: [Option<Mnemonic>; 46] = {
    use Mnemonic as M;
    [
        Some(M::Psrlw),
        Some(M::Psrld),
        Some(M::Psrlq),
        Some(M::Paddq),
        Some(M::Pmullw),
        None,
        None,
        Some(M::Psubusb),
        Some(M::Psubusw),
        Some(M::Pminub),
        Some(M::Pand),
        Some(M::Paddusb),
        Some(M::Paddusw),
        Some(M::Pmaxub),
        Some(M::Pandn),
        Some(M::Pavgb),
        Some(M::Psraw),
        Some(M::Psrad),
        Some(M::Pavgw),
        Some(M::Pmulhuw),
        Some(M::Pmulhw),
        None,
        None,
        Some(M::Psubsb),
        Some(M::Psubsw),
        Some(M::Pminsw),
        Some(M::Por),
        Some(M::Paddsb),
        Some(M::Paddsw),
        Some(M::Pmaxsw),
        Some(M::Pxor),
        None,
        Some(M::Psllw),
        Some(M::Pslld),
        Some(M::Psllq),
        Some(M::Pmuludq),
        Some(M::Pmaddwd),
        Some(M::Psadbw),
        None,
        Some(M::Psubb),
        Some(M::Psubw),
        Some(M::Psubd),
        Some(M::Psubq),
        Some(M::Paddb),
        Some(M::Paddw),
        Some(M::Paddd),
    ]
};

const UNPACK_AND_COMPARE: [Mnemonic; 14] = {
    use Mnemonic as M;
    [
        M::Punpcklbw,
        M::Punpcklwd,
        M::Punpckldq,
        M::Packsswb,
        M::Pcmpgtb,
        M::Pcmpgtw,
        M::Pcmpgtd,
        M::Packuswb,
        M::Punpckhbw,
        M::Punpckhwd,
        M::Punpckhdq,
        M::Packssdw,
        M::Punpcklqdq,
        M::Punpckhqdq,
    ]
};

#[derive(Clone, Copy, Debug)]
struct Family {
    packed_single: Mnemonic,
    packed_double: Mnemonic,
    scalar_single: Mnemonic,
    scalar_double: Mnemonic,
}

pub(super) fn form(
    opcode: u8,
    modrm: Result<u8, DecodeErrorKind>,
    prefixes: &Prefixes,
) -> Result<Form, DecodeErrorKind> {
    use Mandatory::{None as Plain, OperandSize as P66, Repeat as F3, RepeatNotEqual as F2};
    use Mnemonic as M;
    let prefix = prefixes.mandatory();
    let wide = prefixes.rex_w();
    Ok(match (opcode, prefix) {
        (0x10, _) => load(prefix, [M::Movups, M::Movupd, M::Movss, M::Movsd]),
        (0x11, _) => store(prefix, [M::Movups, M::Movupd, M::Movss, M::Movsd]),
        (0x12, Plain) if is_register_form(modrm?) => xmm(M::Movhlps, Width::Qword).register_only(),
        (0x12, Plain) => xmm(M::Movlps, Width::Qword).memory_only(),
        (0x12, P66) => xmm(M::Movlpd, Width::Qword).memory_only(),
        (0x12 | 0x16, F3) | (0x12 | 0x7C | 0x7D | 0xD0 | 0xF0, F2) | (0x7C | 0x7D | 0xD0, P66) => {
            xmm(M::Nop, Width::Dqword).refuse(Refusal::Sse3)
        }
        (0x13, Plain) => xmm_store(M::Movlps, Width::Qword).memory_only(),
        (0x13, P66) => xmm_store(M::Movlpd, Width::Qword).memory_only(),
        (0x14, Plain) => packed(M::Unpcklps),
        (0x14, P66) => packed(M::Unpcklpd),
        (0x15, Plain) => packed(M::Unpckhps),
        (0x15, P66) => packed(M::Unpckhpd),
        (0x16, Plain) if is_register_form(modrm?) => xmm(M::Movlhps, Width::Qword).register_only(),
        (0x16, Plain) => xmm(M::Movhps, Width::Qword).memory_only(),
        (0x16, P66) => xmm(M::Movhpd, Width::Qword).memory_only(),
        (0x17, Plain) => xmm_store(M::Movhps, Width::Qword).memory_only(),
        (0x17, P66) => xmm_store(M::Movhpd, Width::Qword).memory_only(),
        (0x28, Plain) => packed(M::Movaps),
        (0x28, P66) => packed(M::Movapd),
        (0x29, Plain) => xmm_store(M::Movaps, Width::Dqword),
        (0x29, P66) => xmm_store(M::Movapd, Width::Dqword),
        (0x2A, Plain | P66) => xmm(M::Nop, Width::Qword).refuse(Refusal::Mmx),
        (0x2A, F3) => from_general(M::Cvtsi2ss),
        (0x2A, F2) => from_general(M::Cvtsi2sd),
        (0x2B, Plain) => xmm_store(M::Movntps, Width::Dqword).memory_only(),
        (0x2B, P66) => xmm_store(M::Movntpd, Width::Dqword).memory_only(),
        (0x2C | 0x2D, Plain | P66) => xmm(M::Nop, Width::Qword).refuse(Refusal::Mmx),
        (0x2C, F3) => to_general(M::Cvttss2si, Width::Dword),
        (0x2C, F2) => to_general(M::Cvttsd2si, Width::Qword),
        (0x2D, F3) => to_general(M::Cvtss2si, Width::Dword),
        (0x2D, F2) => to_general(M::Cvtsd2si, Width::Qword),
        (0x2E, Plain) => xmm(M::Ucomiss, Width::Dword),
        (0x2E, P66) => xmm(M::Ucomisd, Width::Qword),
        (0x2F, Plain) => xmm(M::Comiss, Width::Dword),
        (0x2F, P66) => xmm(M::Comisd, Width::Qword),
        (0x50, Plain) => to_general(M::Movmskps, Width::Dqword).register_only(),
        (0x50, P66) => to_general(M::Movmskpd, Width::Dqword).register_only(),
        (0x51, _) => arithmetic(prefix, family(M::Sqrtps, M::Sqrtpd, M::Sqrtss, M::Sqrtsd)),
        (0x52, Plain) => packed(M::Rsqrtps),
        (0x52, F3) => xmm(M::Rsqrtss, Width::Dword),
        (0x53, Plain) => packed(M::Rcpps),
        (0x53, F3) => xmm(M::Rcpss, Width::Dword),
        (0x54, Plain) => packed(M::Andps),
        (0x54, P66) => packed(M::Andpd),
        (0x55, Plain) => packed(M::Andnps),
        (0x55, P66) => packed(M::Andnpd),
        (0x56, Plain) => packed(M::Orps),
        (0x56, P66) => packed(M::Orpd),
        (0x57, Plain) => packed(M::Xorps),
        (0x57, P66) => packed(M::Xorpd),
        (0x58, _) => arithmetic(prefix, family(M::Addps, M::Addpd, M::Addss, M::Addsd)),
        (0x59, _) => arithmetic(prefix, family(M::Mulps, M::Mulpd, M::Mulss, M::Mulsd)),
        (0x5A, Plain) => xmm(M::Cvtps2pd, Width::Qword),
        (0x5A, P66) => packed(M::Cvtpd2ps),
        (0x5A, F3) => xmm(M::Cvtss2sd, Width::Dword),
        (0x5A, F2) => xmm(M::Cvtsd2ss, Width::Qword),
        (0x5B, Plain) => packed(M::Cvtdq2ps),
        (0x5B, P66) => packed(M::Cvtps2dq),
        (0x5B, F3) => packed(M::Cvttps2dq),
        (0x5C, _) => arithmetic(prefix, family(M::Subps, M::Subpd, M::Subss, M::Subsd)),
        (0x5D, _) => arithmetic(prefix, family(M::Minps, M::Minpd, M::Minss, M::Minsd)),
        (0x5E, _) => arithmetic(prefix, family(M::Divps, M::Divpd, M::Divss, M::Divsd)),
        (0x5F, _) => arithmetic(prefix, family(M::Maxps, M::Maxpd, M::Maxss, M::Maxsd)),
        (0x60..=0x6D, P66) => packed(UNPACK_AND_COMPARE[usize::from(opcode - 0x60)]),
        (0x77, Plain) => Form::refused(Refusal::Mmx),
        (0x60..=0x6B | 0x6E | 0x6F | 0x74..=0x76 | 0x7E | 0x7F, Plain) => mmx(),
        (0x70..=0x73 | 0xC4 | 0xC5, Plain) => mmx().with(Spec::Imm(Immediate::UnsignedByte)),
        (0x6E, P66) => Form::new(if wide { M::Movq } else { M::Movd }, SizeRule::Normal)
            .with(Spec::RegXmm)
            .with(Spec::Rm(V))
            .sse(),
        (0x6F, P66) => packed(M::Movdqa),
        (0x6F, F3) => packed(M::Movdqu),
        (0x70, P66) => packed(M::Pshufd).with(Spec::Imm(Immediate::UnsignedByte)),
        (0x70, F3) => packed(M::Pshufhw).with(Spec::Imm(Immediate::UnsignedByte)),
        (0x70, F2) => packed(M::Pshuflw).with(Spec::Imm(Immediate::UnsignedByte)),
        (0x71..=0x73, P66) => shift_by_immediate(opcode, modrm?)?,
        (0x74, P66) => packed(M::Pcmpeqb),
        (0x75, P66) => packed(M::Pcmpeqw),
        (0x76, P66) => packed(M::Pcmpeqd),
        (0x7E, P66) => Form::new(if wide { M::Movq } else { M::Movd }, SizeRule::Normal)
            .with(Spec::Rm(V))
            .with(Spec::RegXmm)
            .sse(),
        (0x7E, F3) => xmm(M::Movq, Width::Qword),
        (0x7F, P66) => xmm_store(M::Movdqa, Width::Dqword),
        (0x7F, F3) => xmm_store(M::Movdqu, Width::Dqword),
        (0xC2, _) => arithmetic(prefix, family(M::Cmpps, M::Cmppd, M::Cmpss, M::Cmpsd))
            .with(Spec::Imm(Immediate::UnsignedByte)),
        (0xC4, P66) => Form::new(M::Pinsrw, SizeRule::Fixed(Width::Word))
            .with(Spec::RegXmm)
            .with(Spec::Rm(W))
            .with(Spec::Imm(Immediate::UnsignedByte))
            .sse(),
        (0xC5, P66) => to_general(M::Pextrw, Width::Dqword)
            .with(Spec::Imm(Immediate::UnsignedByte))
            .register_only(),
        (0xC6, Plain) => packed(M::Shufps).with(Spec::Imm(Immediate::UnsignedByte)),
        (0xC6, P66) => packed(M::Shufpd).with(Spec::Imm(Immediate::UnsignedByte)),
        (0xD6, P66) => xmm_store(M::Movq, Width::Qword),
        (0xD6, F3 | F2) => mmx(),
        (0xD7, P66) => to_general(M::Pmovmskb, Width::Dqword).register_only(),
        (0xE6, P66) => packed(M::Cvttpd2dq),
        (0xE6, F3) => xmm(M::Cvtdq2pd, Width::Qword),
        (0xE6, F2) => packed(M::Cvtpd2dq),
        (0xE7, P66) => xmm_store(M::Movntdq, Width::Dqword).memory_only(),
        (0xF7, P66) => packed(M::Maskmovdqu).register_only(),
        (0xD1..=0xFE, Plain) if !matches!(opcode, 0xD6 | 0xE6 | 0xF0) => mmx(),
        (0xD1..=0xFE, P66) => match INTEGER[usize::from(opcode - INTEGER_FIRST)] {
            Some(mnemonic) => packed(mnemonic),
            None => return Err(DecodeErrorKind::Invalid),
        },
        _ => return Err(DecodeErrorKind::Invalid),
    })
}

fn family(
    packed_single: Mnemonic,
    packed_double: Mnemonic,
    scalar_single: Mnemonic,
    scalar_double: Mnemonic,
) -> Family {
    Family {
        packed_single,
        packed_double,
        scalar_single,
        scalar_double,
    }
}

fn arithmetic(prefix: Mandatory, family: Family) -> Form {
    match prefix {
        Mandatory::None => packed(family.packed_single),
        Mandatory::OperandSize => packed(family.packed_double),
        Mandatory::Repeat => xmm(family.scalar_single, Width::Dword),
        Mandatory::RepeatNotEqual => xmm(family.scalar_double, Width::Qword),
    }
}

fn load(prefix: Mandatory, moves: [Mnemonic; 4]) -> Form {
    match prefix {
        Mandatory::None => packed(moves[0]),
        Mandatory::OperandSize => packed(moves[1]),
        Mandatory::Repeat => xmm(moves[2], Width::Dword),
        Mandatory::RepeatNotEqual => xmm(moves[3], Width::Qword),
    }
}

fn store(prefix: Mandatory, moves: [Mnemonic; 4]) -> Form {
    match prefix {
        Mandatory::None => xmm_store(moves[0], Width::Dqword),
        Mandatory::OperandSize => xmm_store(moves[1], Width::Dqword),
        Mandatory::Repeat => xmm_store(moves[2], Width::Dword),
        Mandatory::RepeatNotEqual => xmm_store(moves[3], Width::Qword),
    }
}

fn packed(mnemonic: Mnemonic) -> Form {
    xmm(mnemonic, Width::Dqword)
}

fn xmm(mnemonic: Mnemonic, width: Width) -> Form {
    Form::new(mnemonic, SizeRule::Fixed(width))
        .with(Spec::RegXmm)
        .with(Spec::RmXmm(width))
        .sse()
}

fn xmm_store(mnemonic: Mnemonic, width: Width) -> Form {
    Form::new(mnemonic, SizeRule::Fixed(width))
        .with(Spec::RmXmm(width))
        .with(Spec::RegXmm)
        .sse()
}

fn from_general(mnemonic: Mnemonic) -> Form {
    Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::RegXmm)
        .with(Spec::Rm(V))
        .sse()
}

fn to_general(mnemonic: Mnemonic, source: Width) -> Form {
    Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::Reg(V))
        .with(Spec::RmXmm(source))
        .sse()
}

fn mmx() -> Form {
    Form::refused(Refusal::Mmx)
        .with(Spec::RegXmm)
        .with(Spec::RmXmm(Width::Qword))
}

fn shift_by_immediate(opcode: u8, modrm: u8) -> Result<Form, DecodeErrorKind> {
    use Mnemonic as M;
    let mnemonic = match (opcode, reg_field(modrm)) {
        (0x71, 2) => M::Psrlw,
        (0x71, 4) => M::Psraw,
        (0x71, 6) => M::Psllw,
        (0x72, 2) => M::Psrld,
        (0x72, 4) => M::Psrad,
        (0x72, 6) => M::Pslld,
        (0x73, 2) => M::Psrlq,
        (0x73, 3) => M::Psrldq,
        (0x73, 6) => M::Psllq,
        (0x73, 7) => M::Pslldq,
        _ => return Err(DecodeErrorKind::Invalid),
    };
    Ok(Form::new(mnemonic, SizeRule::Fixed(Width::Dqword))
        .with(Spec::RmXmm(Width::Dqword))
        .with(Spec::Imm(Immediate::UnsignedByte))
        .register_only()
        .sse())
}
