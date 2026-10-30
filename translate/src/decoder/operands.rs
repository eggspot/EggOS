use super::cursor::Cursor;
use super::form::{Form, Immediate, ModRule, Outcome, Relative, Size, SizeRule, Spec};
use super::modrm::{self, ModRm, RmTarget};
use super::prefix::Prefixes;
use super::{DecodeErrorKind, Refusal};
use crate::instruction::{
    Address, Gpr, Instruction, Memory, Mnemonic, Operand, Register, Repeat, Width,
};

const REPEAT_PREFIX: u8 = 0xF3;
const HIGH_BYTE_FIRST: u8 = 4;
const HIGH_BYTE_END: u8 = 8;

struct Context<'p> {
    prefixes: &'p Prefixes,
    opcode: u8,
    width: Width,
    modrm: Option<ModRm>,
    address: u64,
}

pub(super) fn build(
    cursor: &mut Cursor<'_>,
    prefixes: &Prefixes,
    opcode: u8,
    form: Form,
    address: u64,
) -> Result<Instruction, DecodeErrorKind> {
    let (width, width_refusal) = operation_width(&form, prefixes);
    let modrm = if form.needs_modrm() {
        Some(modrm::read(cursor, prefixes.rex_bits())?)
    } else {
        None
    };
    if let Some(modrm) = modrm {
        let is_register = matches!(modrm.target, RmTarget::Register(_));
        match form.mod_rule {
            ModRule::RegisterOnly if !is_register => return Err(DecodeErrorKind::Invalid),
            ModRule::MemoryOnly if is_register => return Err(DecodeErrorKind::Invalid),
            _ => {}
        }
    }
    let mnemonic = match form.outcome {
        Outcome::Decoded(mnemonic) => mnemonic,
        Outcome::Refused(_) => Mnemonic::Nop,
    };
    let context = Context {
        prefixes,
        opcode,
        width,
        modrm,
        address,
    };
    let mut instruction = Instruction::new(address, mnemonic, width);
    for spec in form.specs() {
        instruction.push(operand(spec, &context, cursor)?);
    }
    let length = cursor.position() as u8;
    let refusal = match form.outcome {
        Outcome::Refused(refusal) => Some(refusal),
        Outcome::Decoded(_) => width_refusal,
    };
    if let Some(refusal) = refusal {
        return Err(DecodeErrorKind::Unsupported { refusal, length });
    }
    let has_memory = instruction
        .operands()
        .iter()
        .any(|operand| matches!(operand, Operand::Memory(_)));
    if prefixes.lock {
        let memory_destination = matches!(instruction.operands().first(), Some(Operand::Memory(_)));
        if !form.lockable || !memory_destination {
            return Err(DecodeErrorKind::Invalid);
        }
    }
    if prefixes.address_size && (has_memory || mnemonic.has_implicit_address()) {
        return Err(DecodeErrorKind::Unsupported {
            refusal: Refusal::AddressSize,
            length,
        });
    }
    instruction.length = length;
    instruction.lock = prefixes.lock;
    if mnemonic.is_string() {
        instruction.repeat = prefixes.repeat.map(|byte| {
            if byte == REPEAT_PREFIX {
                Repeat::Rep
            } else {
                Repeat::Repne
            }
        });
    }
    let end = instruction.end();
    resolve_rip_relative(&mut instruction, end);
    Ok(instruction)
}

fn operation_width(form: &Form, prefixes: &Prefixes) -> (Width, Option<Refusal>) {
    let operand_size = prefixes.operand_size && !form.mandatory_prefix;
    match form.size {
        SizeRule::Normal if prefixes.rex_w() => (Width::Qword, None),
        SizeRule::Normal if operand_size => (Width::Word, None),
        SizeRule::Normal => (Width::Dword, None),
        SizeRule::Default64 if operand_size => (Width::Word, None),
        SizeRule::Default64 => (Width::Qword, None),
        SizeRule::Near64 if operand_size => (Width::Qword, Some(Refusal::Branch16)),
        SizeRule::Near64 => (Width::Qword, None),
        SizeRule::Fixed(width) => (width, None),
    }
}

fn resolve_rip_relative(instruction: &mut Instruction, end: u64) {
    for operand in instruction.operands_mut() {
        if let Operand::Memory(Memory {
            address: Address::Rip { target },
            ..
        }) = operand
        {
            *target = end.wrapping_add(*target);
        }
    }
}

fn operand(
    spec: Spec,
    context: &Context<'_>,
    cursor: &mut Cursor<'_>,
) -> Result<Operand, DecodeErrorKind> {
    let modrm = || context.modrm.ok_or(DecodeErrorKind::Invalid);
    Ok(match spec {
        Spec::Rm(size) => match modrm()?.target {
            RmTarget::Register(number) => general(number, resolve(size, context), context),
            RmTarget::Memory(address) => memory(address, resolve(size, context), context),
        },
        Spec::Reg(size) => general(modrm()?.reg, resolve(size, context), context),
        Spec::RmXmm(width) => match modrm()?.target {
            RmTarget::Register(number) => Operand::Register(Register::Xmm(number)),
            RmTarget::Memory(address) => memory(address, width, context),
        },
        Spec::RegXmm => Operand::Register(Register::Xmm(modrm()?.reg)),
        Spec::OpcodeReg(size) => {
            let number = (context.opcode & 0b111) | context.prefixes.rex_b();
            general(number, resolve(size, context), context)
        }
        Spec::Accumulator(size) => general(Gpr::RAX.0, resolve(size, context), context),
        Spec::Cl => Operand::Register(Register::General {
            gpr: Gpr::RCX,
            width: Width::Byte,
        }),
        Spec::One => Operand::Immediate(1),
        Spec::Imm(kind) => Operand::Immediate(immediate(kind, context.width, cursor)?),
        Spec::Rel(kind) => {
            let offset = match kind {
                Relative::Byte => i64::from(cursor.i8()?),
                Relative::Dword => i64::from(cursor.i32()?),
            };
            let end = context.address.wrapping_add(cursor.position() as u64);
            Operand::Target(end.wrapping_add(offset as u64))
        }
        Spec::St0 => Operand::Register(Register::St(0)),
        Spec::StRm => Operand::Register(Register::St(modrm()?.raw_rm)),
        Spec::Moffs(size) => {
            let absolute = cursor.i64()? as u64;
            memory(Address::Absolute(absolute), resolve(size, context), context)
        }
    })
}

fn resolve(size: Size, context: &Context<'_>) -> Width {
    match size {
        Size::Fixed(width) => width,
        Size::Operand => context.width,
    }
}

fn general(number: u8, width: Width, context: &Context<'_>) -> Operand {
    let high_byte = width == Width::Byte
        && context.prefixes.rex.is_none()
        && (HIGH_BYTE_FIRST..HIGH_BYTE_END).contains(&number);
    Operand::Register(if high_byte {
        Register::HighByte(Gpr(number - HIGH_BYTE_FIRST))
    } else {
        Register::General {
            gpr: Gpr(number),
            width,
        }
    })
}

fn memory(address: Address, width: Width, context: &Context<'_>) -> Operand {
    Operand::Memory(Memory {
        segment: context.prefixes.segment,
        address,
        width,
    })
}

fn immediate(
    kind: Immediate,
    width: Width,
    cursor: &mut Cursor<'_>,
) -> Result<i64, DecodeErrorKind> {
    Ok(match kind {
        Immediate::SignedByte => i64::from(cursor.i8()?),
        Immediate::UnsignedByte => i64::from(cursor.i8()? as u8),
        Immediate::UnsignedWord => i64::from(cursor.i16()? as u16),
        Immediate::OperandSized32 if width == Width::Word => i64::from(cursor.i16()?),
        Immediate::OperandSized32 => i64::from(cursor.i32()?),
        Immediate::OperandSized64 => match width {
            Width::Word => i64::from(cursor.i16()?),
            Width::Qword => cursor.i64()?,
            _ => i64::from(cursor.i32()?),
        },
    })
}
