use super::DecodeErrorKind;
use super::cursor::Cursor;
use super::prefix::{REX_B, REX_R, REX_X, extension};
use crate::instruction::{Address, Gpr, Index};

const MODE_REGISTER: u8 = 0b11;
const RM_SIB: u8 = 0b100;
const RM_RIP_OR_BASELESS: u8 = 0b101;
const NO_INDEX: u8 = 0b100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RmTarget {
    Register(u8),
    Memory(Address),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ModRm {
    pub(super) raw_rm: u8,
    pub(super) reg: u8,
    pub(super) target: RmTarget,
}

pub(super) fn is_register_form(modrm: u8) -> bool {
    modrm >> 6 == MODE_REGISTER
}

pub(super) fn reg_field(modrm: u8) -> u8 {
    (modrm >> 3) & 0b111
}

pub(super) fn read(cursor: &mut Cursor<'_>, rex: u8) -> Result<ModRm, DecodeErrorKind> {
    let byte = cursor.byte()?;
    let mode = byte >> 6;
    let raw_rm = byte & 0b111;
    let target = if mode == MODE_REGISTER {
        RmTarget::Register(raw_rm | extension(rex, REX_B))
    } else {
        RmTarget::Memory(address(cursor, mode, raw_rm, rex)?)
    };
    Ok(ModRm {
        raw_rm,
        reg: reg_field(byte) | extension(rex, REX_R),
        target,
    })
}

fn address(
    cursor: &mut Cursor<'_>,
    mode: u8,
    raw_rm: u8,
    rex: u8,
) -> Result<Address, DecodeErrorKind> {
    if raw_rm == RM_SIB {
        return scaled_index(cursor, mode, rex);
    }
    if raw_rm == RM_RIP_OR_BASELESS && mode == 0 {
        let offset = cursor.i32()?;
        return Ok(Address::Rip {
            target: i64::from(offset) as u64,
        });
    }
    Ok(Address::Based {
        base: Some(Gpr(raw_rm | extension(rex, REX_B))),
        index: None,
        displacement: displacement(cursor, mode)?,
    })
}

fn scaled_index(cursor: &mut Cursor<'_>, mode: u8, rex: u8) -> Result<Address, DecodeErrorKind> {
    let sib = cursor.byte()?;
    let raw_index = (sib >> 3) & 0b111;
    let index = (raw_index != NO_INDEX || rex & REX_X != 0).then(|| Index {
        register: Gpr(raw_index | extension(rex, REX_X)),
        scale: 1 << (sib >> 6),
    });
    let raw_base = sib & 0b111;
    if raw_base == RM_RIP_OR_BASELESS && mode == 0 {
        return Ok(Address::Based {
            base: None,
            index,
            displacement: cursor.i32()?,
        });
    }
    Ok(Address::Based {
        base: Some(Gpr(raw_base | extension(rex, REX_B))),
        index,
        displacement: displacement(cursor, mode)?,
    })
}

fn displacement(cursor: &mut Cursor<'_>, mode: u8) -> Result<i32, DecodeErrorKind> {
    match mode {
        0 => Ok(0),
        1 => Ok(i32::from(cursor.i8()?)),
        _ => cursor.i32(),
    }
}
