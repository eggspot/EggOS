use super::DecodeErrorKind;
use super::cursor::Cursor;
use crate::instruction::Segment;

pub(super) const REX_W: u8 = 0b1000;
pub(super) const REX_R: u8 = 0b0100;
pub(super) const REX_X: u8 = 0b0010;
pub(super) const REX_B: u8 = 0b0001;

const REPEAT: u8 = 0xF3;
const REPEAT_NOT_EQUAL: u8 = 0xF2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mandatory {
    None,
    OperandSize,
    Repeat,
    RepeatNotEqual,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Prefixes {
    pub(super) lock: bool,
    pub(super) repeat: Option<u8>,
    pub(super) operand_size: bool,
    pub(super) address_size: bool,
    pub(super) segment: Option<Segment>,
    pub(super) rex: Option<u8>,
}

impl Prefixes {
    pub(super) fn rex_bits(&self) -> u8 {
        self.rex.map_or(0, |rex| rex & 0xF)
    }

    pub(super) fn rex_w(&self) -> bool {
        self.rex_bits() & REX_W != 0
    }

    pub(super) fn rex_b(&self) -> u8 {
        extension(self.rex_bits(), REX_B)
    }

    pub(super) fn mandatory(&self) -> Mandatory {
        match self.repeat {
            Some(REPEAT) => Mandatory::Repeat,
            Some(_) => Mandatory::RepeatNotEqual,
            None if self.operand_size => Mandatory::OperandSize,
            None => Mandatory::None,
        }
    }

    pub(super) fn has_legacy_or_rex(&self) -> bool {
        self.lock || self.repeat.is_some() || self.operand_size || self.rex.is_some()
    }
}

pub(super) fn extension(rex: u8, bit: u8) -> u8 {
    if rex & bit != 0 { 8 } else { 0 }
}

pub(super) fn read(cursor: &mut Cursor<'_>) -> Result<Prefixes, DecodeErrorKind> {
    let mut prefixes = Prefixes::default();
    loop {
        let byte = cursor.peek()?;
        match byte {
            0xF0 => prefixes.lock = true,
            REPEAT | REPEAT_NOT_EQUAL => prefixes.repeat = Some(byte),
            0x66 => prefixes.operand_size = true,
            0x67 => prefixes.address_size = true,
            0x64 => prefixes.segment = Some(Segment::Fs),
            0x65 => prefixes.segment = Some(Segment::Gs),
            0x26 | 0x2E | 0x36 | 0x3E => prefixes.segment = None,
            0x40..=0x4F => {
                prefixes.rex = Some(byte);
                cursor.byte()?;
                continue;
            }
            _ => return Ok(prefixes),
        }
        prefixes.rex = None;
        cursor.byte()?;
    }
}
