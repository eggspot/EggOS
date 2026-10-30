use super::form::{B, D, DQ, Form, Immediate, Q, Relative, SizeRule, Spec, V, W};
use super::modrm::{is_register_form, reg_field};
use super::prefix::{Mandatory, Prefixes};
use super::{DecodeErrorKind, Refusal, sse};
use crate::instruction::{Condition, Mnemonic, Width};

const XGETBV: u8 = 0xD0;
const RDTSCP: u8 = 0xF9;
const ENDBR64: u8 = 0xFA;
const ENDBR32: u8 = 0xFB;

pub(super) fn form(
    opcode: u8,
    modrm: Result<u8, DecodeErrorKind>,
    prefixes: &Prefixes,
) -> Result<Form, DecodeErrorKind> {
    use Mnemonic as M;
    use SizeRule::{Near64, Normal};
    use Spec::{Imm, Reg, Rm};
    let byte = SizeRule::Fixed(Width::Byte);
    let repeat = prefixes.mandatory() == Mandatory::Repeat;
    Ok(match opcode {
        0x00 | 0x02 | 0x03 => Form::refused(Refusal::System).modrm(),
        0x01 => match modrm? {
            XGETBV => Form::new(M::Xgetbv, byte).modrm(),
            RDTSCP => Form::new(M::Rdtscp, byte).modrm(),
            _ => Form::refused(Refusal::System).modrm(),
        },
        0x05 => Form::new(M::Syscall, byte),
        0x06..=0x09 | 0x30 | 0x32..=0x35 | 0x37 | 0xA0 | 0xA1 | 0xA8 | 0xA9 | 0xAA => {
            Form::refused(Refusal::System)
        }
        0x0B => Form::new(M::Undefined, byte),
        0x0D if !is_register_form(modrm?) => Form::new(M::Prefetch, byte).with(Rm(B)).memory_only(),
        0x0E => Form::refused(Refusal::Mmx),
        0x18 if !is_register_form(modrm?) => Form::new(M::Prefetch, byte).with(Rm(B)).memory_only(),
        0x1E if repeat && modrm? == ENDBR64 => Form::new(M::Endbr64, byte).modrm(),
        0x1E if repeat && modrm? == ENDBR32 => Form::new(M::Endbr32, byte).modrm(),
        0x18..=0x1F => Form::new(M::Nop, Normal).with(Rm(V)),
        0x20..=0x23 | 0x78 | 0x79 => Form::refused(Refusal::System).modrm(),
        0x10..=0x17 | 0x28..=0x2F | 0x50..=0x7F | 0xC2 | 0xC4..=0xC6 | 0xD0..=0xFE => {
            sse::form(opcode, modrm, prefixes)?
        }
        0x31 => Form::new(M::Rdtsc, byte),
        0x40..=0x4F => Form::new(M::Cmov(Condition::from_code(opcode)), Normal)
            .with(Reg(V))
            .with(Rm(V)),
        0x80..=0x8F => {
            Form::new(M::Jcc(Condition::from_code(opcode)), Near64).with(Spec::Rel(Relative::Dword))
        }
        0x90..=0x9F => Form::new(M::Set(Condition::from_code(opcode)), byte).with(Rm(B)),
        0xA2 => Form::new(M::Cpuid, byte),
        0xA3 => bit_test(M::Bt),
        0xAB => bit_test(M::Bts).lockable(),
        0xB3 => bit_test(M::Btr).lockable(),
        0xBB => bit_test(M::Btc).lockable(),
        0xA4 => double_shift(M::Shld, Imm(Immediate::UnsignedByte)),
        0xA5 => double_shift(M::Shld, Spec::Cl),
        0xAC => double_shift(M::Shrd, Imm(Immediate::UnsignedByte)),
        0xAD => double_shift(M::Shrd, Spec::Cl),
        0xAE => group15(modrm?, repeat)?,
        0xAF => Form::new(M::Imul, Normal).with(Reg(V)).with(Rm(V)),
        0xB0 => Form::new(M::Cmpxchg, byte)
            .with(Rm(B))
            .with(Reg(B))
            .lockable(),
        0xB1 => Form::new(M::Cmpxchg, Normal)
            .with(Rm(V))
            .with(Reg(V))
            .lockable(),
        0xB6 => Form::new(M::Movzx, Normal).with(Reg(V)).with(Rm(B)),
        0xB7 => Form::new(M::Movzx, Normal).with(Reg(V)).with(Rm(W)),
        0xB8 if repeat => Form::new(M::Popcnt, Normal).with(Reg(V)).with(Rm(V)),
        0xB9 | 0xFF => Form::new(M::Undefined, byte).modrm(),
        0xBA => group8(modrm?)?,
        0xBC => scan(if repeat { M::Tzcnt } else { M::Bsf }),
        0xBD => scan(if repeat { M::Lzcnt } else { M::Bsr }),
        0xBE => Form::new(M::Movsx, Normal).with(Reg(V)).with(Rm(B)),
        0xBF => Form::new(M::Movsx, Normal).with(Reg(V)).with(Rm(W)),
        0xC0 => Form::new(M::Xadd, byte).with(Rm(B)).with(Reg(B)).lockable(),
        0xC1 => Form::new(M::Xadd, Normal)
            .with(Rm(V))
            .with(Reg(V))
            .lockable(),
        0xC3 => Form::new(M::Movnti, Normal)
            .with(Rm(V))
            .with(Reg(V))
            .memory_only(),
        0xC7 => group9(modrm?, prefixes)?,
        0xC8..=0xCF => Form::new(M::Bswap, Normal).with(Spec::OpcodeReg(V)),
        _ => return Err(DecodeErrorKind::Invalid),
    })
}

fn bit_test(mnemonic: Mnemonic) -> Form {
    Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::Rm(V))
        .with(Spec::Reg(V))
}

fn double_shift(mnemonic: Mnemonic, count: Spec) -> Form {
    Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::Rm(V))
        .with(Spec::Reg(V))
        .with(count)
}

fn scan(mnemonic: Mnemonic) -> Form {
    Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::Reg(V))
        .with(Spec::Rm(V))
}

fn group8(modrm: u8) -> Result<Form, DecodeErrorKind> {
    let mnemonic = match reg_field(modrm) {
        4 => Mnemonic::Bt,
        5 => Mnemonic::Bts,
        6 => Mnemonic::Btr,
        7 => Mnemonic::Btc,
        _ => return Err(DecodeErrorKind::Invalid),
    };
    let form = Form::new(mnemonic, SizeRule::Normal)
        .with(Spec::Rm(V))
        .with(Spec::Imm(Immediate::UnsignedByte));
    Ok(if mnemonic == Mnemonic::Bt {
        form
    } else {
        form.lockable()
    })
}

fn group9(modrm: u8, prefixes: &Prefixes) -> Result<Form, DecodeErrorKind> {
    let plain = prefixes.repeat.is_none();
    Ok(match (is_register_form(modrm), reg_field(modrm)) {
        (false, 1) if prefixes.rex_w() => {
            Form::new(Mnemonic::Cmpxchg16b, SizeRule::Fixed(Width::Dqword))
                .with(Spec::Rm(DQ))
                .lockable()
        }
        (false, 1) => Form::new(Mnemonic::Cmpxchg8b, SizeRule::Fixed(Width::Qword))
            .with(Spec::Rm(Q))
            .lockable(),
        (false, 3..=5) => Form::refused(Refusal::StateSave).modrm(),
        (false, 6 | 7) => Form::refused(Refusal::System).modrm(),
        (true, 6) if plain => Form::new(Mnemonic::Rdrand, SizeRule::Normal).with(Spec::Rm(V)),
        (true, 7) if plain => Form::new(Mnemonic::Rdseed, SizeRule::Normal).with(Spec::Rm(V)),
        (true, _) => Form::refused(Refusal::System).modrm(),
        _ => return Err(DecodeErrorKind::Invalid),
    })
}

fn group15(modrm: u8, repeat: bool) -> Result<Form, DecodeErrorKind> {
    let none = SizeRule::Fixed(Width::Byte);
    let control = SizeRule::Fixed(Width::Dword);
    Ok(match (is_register_form(modrm), reg_field(modrm), repeat) {
        (false, 0 | 1 | 4..=6, false) => Form::refused(Refusal::StateSave).modrm(),
        (false, 2, false) => Form::new(Mnemonic::Ldmxcsr, control)
            .with(Spec::Rm(D))
            .memory_only(),
        (false, 3, false) => Form::new(Mnemonic::Stmxcsr, control)
            .with(Spec::Rm(D))
            .memory_only(),
        (false, 7, false) => Form::new(Mnemonic::Clflush, none)
            .with(Spec::Rm(B))
            .memory_only(),
        (true, 0..=3, true) => Form::refused(Refusal::System).modrm(),
        (true, 5, false) => Form::new(Mnemonic::Lfence, none).modrm(),
        (true, 6, false) => Form::new(Mnemonic::Mfence, none).modrm(),
        (true, 7, false) => Form::new(Mnemonic::Sfence, none).modrm(),
        _ => return Err(DecodeErrorKind::Invalid),
    })
}
