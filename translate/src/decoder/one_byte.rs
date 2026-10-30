use super::form::{B, D, Form, Immediate, Relative, SizeRule, Spec, V};
use super::modrm::reg_field;
use super::prefix::{Mandatory, Prefixes};
use super::{DecodeErrorKind, Refusal};
use crate::instruction::{Condition, Mnemonic, Width};

const ARITHMETIC: [Mnemonic; 8] = [
    Mnemonic::Add,
    Mnemonic::Or,
    Mnemonic::Adc,
    Mnemonic::Sbb,
    Mnemonic::And,
    Mnemonic::Sub,
    Mnemonic::Xor,
    Mnemonic::Cmp,
];

const SHIFTS: [Mnemonic; 8] = [
    Mnemonic::Rol,
    Mnemonic::Ror,
    Mnemonic::Rcl,
    Mnemonic::Rcr,
    Mnemonic::Shl,
    Mnemonic::Shr,
    Mnemonic::Shl,
    Mnemonic::Sar,
];

const XBEGIN_OR_XABORT: u8 = 0xF8;

pub(super) fn form(
    opcode: u8,
    modrm: Result<u8, DecodeErrorKind>,
    prefixes: &Prefixes,
) -> Result<Form, DecodeErrorKind> {
    use Mnemonic as M;
    use SizeRule::{Default64, Near64, Normal};
    use Spec::{Accumulator, Imm, OpcodeReg, Reg, Rel, Rm};
    let byte = SizeRule::Fixed(Width::Byte);
    Ok(match opcode {
        0x00..=0x3F if opcode & 0b111 < 6 => arithmetic(opcode),
        0x50..=0x57 => Form::new(M::Push, Default64).with(OpcodeReg(V)),
        0x58..=0x5F => Form::new(M::Pop, Default64).with(OpcodeReg(V)),
        0x63 => Form::new(M::Movsxd, Normal).with(Reg(V)).with(Rm(D)),
        0x68 => Form::new(M::Push, Default64).with(Imm(Immediate::OperandSized32)),
        0x69 => Form::new(M::Imul, Normal)
            .with(Reg(V))
            .with(Rm(V))
            .with(Imm(Immediate::OperandSized32)),
        0x6A => Form::new(M::Push, Default64).with(Imm(Immediate::SignedByte)),
        0x6B => Form::new(M::Imul, Normal)
            .with(Reg(V))
            .with(Rm(V))
            .with(Imm(Immediate::SignedByte)),
        0x6C..=0x6F | 0xEC..=0xEF => Form::refused(Refusal::System),
        0xE4..=0xE7 => Form::refused(Refusal::System).with(Imm(Immediate::UnsignedByte)),
        0x70..=0x7F => {
            Form::new(M::Jcc(Condition::from_code(opcode)), Near64).with(Rel(Relative::Byte))
        }
        0x80 => group1(modrm?, byte, Rm(B), Immediate::SignedByte),
        0x81 => group1(modrm?, Normal, Rm(V), Immediate::OperandSized32),
        0x83 => group1(modrm?, Normal, Rm(V), Immediate::SignedByte),
        0x84 => Form::new(M::Test, byte).with(Rm(B)).with(Reg(B)),
        0x85 => Form::new(M::Test, Normal).with(Rm(V)).with(Reg(V)),
        0x86 => Form::new(M::Xchg, byte).with(Rm(B)).with(Reg(B)).lockable(),
        0x87 => Form::new(M::Xchg, Normal)
            .with(Rm(V))
            .with(Reg(V))
            .lockable(),
        0x88 => Form::new(M::Mov, byte).with(Rm(B)).with(Reg(B)),
        0x89 => Form::new(M::Mov, Normal).with(Rm(V)).with(Reg(V)),
        0x8A => Form::new(M::Mov, byte).with(Reg(B)).with(Rm(B)),
        0x8B => Form::new(M::Mov, Normal).with(Reg(V)).with(Rm(V)),
        0x8C | 0x8E => Form::refused(Refusal::System).modrm(),
        0x8D => Form::new(M::Lea, Normal)
            .with(Reg(V))
            .with(Rm(V))
            .memory_only(),
        0x8F if reg_field(modrm?) == 0 => Form::new(M::Pop, Default64).with(Rm(V)),
        0x90 if prefixes.rex_b() != 0 => xchg_with_accumulator(),
        0x90 if prefixes.mandatory() == Mandatory::Repeat => Form::new(M::Pause, byte),
        0x90 => Form::new(M::Nop, byte),
        0x91..=0x97 => xchg_with_accumulator(),
        0x98 => Form::new(M::ExtendAccumulator, Normal),
        0x99 => Form::new(M::ExtendIntoRdx, Normal),
        0x9B => Form::new(M::Fwait, byte),
        0x9C => Form::new(M::Pushf, Default64),
        0x9D => Form::new(M::Popf, Default64),
        0x9E => Form::new(M::Sahf, byte),
        0x9F => Form::new(M::Lahf, byte),
        0xA0 => Form::new(M::Mov, byte)
            .with(Accumulator(B))
            .with(Spec::Moffs(B)),
        0xA1 => Form::new(M::Mov, Normal)
            .with(Accumulator(V))
            .with(Spec::Moffs(V)),
        0xA2 => Form::new(M::Mov, byte)
            .with(Spec::Moffs(B))
            .with(Accumulator(B)),
        0xA3 => Form::new(M::Mov, Normal)
            .with(Spec::Moffs(V))
            .with(Accumulator(V)),
        0xA4..=0xA7 | 0xAA..=0xAF => string(opcode),
        0xA8 => Form::new(M::Test, byte)
            .with(Accumulator(B))
            .with(Imm(Immediate::SignedByte)),
        0xA9 => Form::new(M::Test, Normal)
            .with(Accumulator(V))
            .with(Imm(Immediate::OperandSized32)),
        0xB0..=0xB7 => Form::new(M::Mov, byte)
            .with(OpcodeReg(B))
            .with(Imm(Immediate::SignedByte)),
        0xB8..=0xBF => Form::new(M::Mov, Normal)
            .with(OpcodeReg(V))
            .with(Imm(Immediate::OperandSized64)),
        0xC0 => group2(modrm?, byte, Rm(B), Imm(Immediate::UnsignedByte)),
        0xC1 => group2(modrm?, Normal, Rm(V), Imm(Immediate::UnsignedByte)),
        0xC2 => Form::new(M::Ret, Near64).with(Imm(Immediate::UnsignedWord)),
        0xC3 => Form::new(M::Ret, Near64),
        0xC6 if modrm? == XBEGIN_OR_XABORT => Form::refused(Refusal::System)
            .modrm()
            .with(Imm(Immediate::UnsignedByte)),
        0xC7 if modrm? == XBEGIN_OR_XABORT => Form::refused(Refusal::System)
            .modrm()
            .with(Rel(Relative::Dword)),
        0xC6 if reg_field(modrm?) == 0 => Form::new(M::Mov, byte)
            .with(Rm(B))
            .with(Imm(Immediate::SignedByte)),
        0xC7 if reg_field(modrm?) == 0 => Form::new(M::Mov, Normal)
            .with(Rm(V))
            .with(Imm(Immediate::OperandSized32)),
        0xC8 => Form::new(M::Enter, Default64)
            .with(Imm(Immediate::UnsignedWord))
            .with(Imm(Immediate::UnsignedByte)),
        0xC9 => Form::new(M::Leave, Default64),
        0xCA => Form::refused(Refusal::System).with(Imm(Immediate::UnsignedWord)),
        0xCB | 0xCF | 0xF1 | 0xF4 | 0xFA | 0xFB => Form::refused(Refusal::System),
        0xCC => Form::new(M::Int3, byte),
        0xCD => Form::new(M::Int, byte).with(Imm(Immediate::UnsignedByte)),
        0xD0 => group2(modrm?, byte, Rm(B), Spec::One),
        0xD1 => group2(modrm?, Normal, Rm(V), Spec::One),
        0xD2 => group2(modrm?, byte, Rm(B), Spec::Cl),
        0xD3 => group2(modrm?, Normal, Rm(V), Spec::Cl),
        0xD7 => Form::new(M::Xlat, byte),
        0xE0 => Form::new(M::Loopne, Near64).with(Rel(Relative::Byte)),
        0xE1 => Form::new(M::Loope, Near64).with(Rel(Relative::Byte)),
        0xE2 => Form::new(M::Loop, Near64).with(Rel(Relative::Byte)),
        0xE3 => Form::new(M::Jrcxz, Near64).with(Rel(Relative::Byte)),
        0xE8 => Form::new(M::Call, Near64).with(Rel(Relative::Dword)),
        0xE9 => Form::new(M::Jmp, Near64).with(Rel(Relative::Dword)),
        0xEB => Form::new(M::Jmp, Near64).with(Rel(Relative::Byte)),
        0xF5 => Form::new(M::Cmc, byte),
        0xF6 => group3(modrm?, byte, Rm(B), Immediate::SignedByte),
        0xF7 => group3(modrm?, Normal, Rm(V), Immediate::OperandSized32),
        0xF8 => Form::new(M::Clc, byte),
        0xF9 => Form::new(M::Stc, byte),
        0xFC => Form::new(M::Cld, byte),
        0xFD => Form::new(M::Std, byte),
        0xFE => group4(modrm?)?,
        0xFF => group5(modrm?)?,
        _ => return Err(DecodeErrorKind::Invalid),
    })
}

fn arithmetic(opcode: u8) -> Form {
    use Spec::{Accumulator, Imm, Reg, Rm};
    let mnemonic = ARITHMETIC[usize::from(opcode >> 3)];
    let byte = SizeRule::Fixed(Width::Byte);
    let form = match opcode & 0b111 {
        0 => Form::new(mnemonic, byte).with(Rm(B)).with(Reg(B)),
        1 => Form::new(mnemonic, SizeRule::Normal)
            .with(Rm(V))
            .with(Reg(V)),
        2 => Form::new(mnemonic, byte).with(Reg(B)).with(Rm(B)),
        3 => Form::new(mnemonic, SizeRule::Normal)
            .with(Reg(V))
            .with(Rm(V)),
        4 => Form::new(mnemonic, byte)
            .with(Accumulator(B))
            .with(Imm(Immediate::SignedByte)),
        _ => Form::new(mnemonic, SizeRule::Normal)
            .with(Accumulator(V))
            .with(Imm(Immediate::OperandSized32)),
    };
    if mnemonic == Mnemonic::Cmp {
        form
    } else {
        form.lockable()
    }
}

fn xchg_with_accumulator() -> Form {
    Form::new(Mnemonic::Xchg, SizeRule::Normal)
        .with(Spec::OpcodeReg(V))
        .with(Spec::Accumulator(V))
}

fn string(opcode: u8) -> Form {
    let mnemonic = match opcode & !1 {
        0xA4 => Mnemonic::Movs,
        0xA6 => Mnemonic::Cmps,
        0xAA => Mnemonic::Stos,
        0xAC => Mnemonic::Lods,
        _ => Mnemonic::Scas,
    };
    let size = if opcode & 1 == 0 {
        SizeRule::Fixed(Width::Byte)
    } else {
        SizeRule::Normal
    };
    Form::new(mnemonic, size)
}

fn group1(modrm: u8, size: SizeRule, target: Spec, immediate: Immediate) -> Form {
    let mnemonic = ARITHMETIC[usize::from(reg_field(modrm))];
    let form = Form::new(mnemonic, size)
        .with(target)
        .with(Spec::Imm(immediate));
    if mnemonic == Mnemonic::Cmp {
        form
    } else {
        form.lockable()
    }
}

fn group2(modrm: u8, size: SizeRule, target: Spec, count: Spec) -> Form {
    Form::new(SHIFTS[usize::from(reg_field(modrm))], size)
        .with(target)
        .with(count)
}

fn group3(modrm: u8, size: SizeRule, target: Spec, immediate: Immediate) -> Form {
    let unary = |mnemonic| Form::new(mnemonic, size).with(target);
    match reg_field(modrm) {
        0 | 1 => Form::new(Mnemonic::Test, size)
            .with(target)
            .with(Spec::Imm(immediate)),
        2 => unary(Mnemonic::Not).lockable(),
        3 => unary(Mnemonic::Neg).lockable(),
        4 => unary(Mnemonic::Mul),
        5 => unary(Mnemonic::Imul),
        6 => unary(Mnemonic::Div),
        _ => unary(Mnemonic::Idiv),
    }
}

fn group4(modrm: u8) -> Result<Form, DecodeErrorKind> {
    let byte = SizeRule::Fixed(Width::Byte);
    Ok(match reg_field(modrm) {
        0 => Form::new(Mnemonic::Inc, byte).with(Spec::Rm(B)).lockable(),
        1 => Form::new(Mnemonic::Dec, byte).with(Spec::Rm(B)).lockable(),
        _ => return Err(DecodeErrorKind::Invalid),
    })
}

fn group5(modrm: u8) -> Result<Form, DecodeErrorKind> {
    let target = Spec::Rm(V);
    Ok(match reg_field(modrm) {
        0 => Form::new(Mnemonic::Inc, SizeRule::Normal)
            .with(target)
            .lockable(),
        1 => Form::new(Mnemonic::Dec, SizeRule::Normal)
            .with(target)
            .lockable(),
        2 => Form::new(Mnemonic::Call, SizeRule::Near64).with(target),
        4 => Form::new(Mnemonic::Jmp, SizeRule::Near64).with(target),
        6 => Form::new(Mnemonic::Push, SizeRule::Default64).with(target),
        3 | 5 => Form::refused(Refusal::System).modrm(),
        _ => return Err(DecodeErrorKind::Invalid),
    })
}
