use super::cursor::Cursor;
use super::prefix::Prefixes;
use super::{DecodeErrorKind, Refusal, modrm};

const VEX_THREE_BYTE: u8 = 0xC4;
const VEX_TWO_BYTE: u8 = 0xC5;
const EVEX: u8 = 0x62;
const XOP_MAP_FIRST: u8 = 8;
const VZEROUPPER_OR_VZEROALL: u8 = 0x77;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trailer {
    None,
    Byte,
    Dword,
}

pub(super) fn is_xop(byte_after_8f: u8) -> bool {
    byte_after_8f & 0x1F >= XOP_MAP_FIRST
}

pub(super) fn refuse(cursor: &mut Cursor<'_>, prefixes: &Prefixes, escape: u8) -> DecodeErrorKind {
    if prefixes.has_legacy_or_rex() {
        return DecodeErrorKind::Invalid;
    }
    let refusal = match escape {
        VEX_THREE_BYTE | VEX_TWO_BYTE => Refusal::Vex,
        EVEX => Refusal::Evex,
        _ => Refusal::Xop,
    };
    match length(cursor, escape) {
        Ok(length) => DecodeErrorKind::Unsupported { refusal, length },
        Err(kind) => kind,
    }
}

fn length(cursor: &mut Cursor<'_>, escape: u8) -> Result<u8, DecodeErrorKind> {
    let map = match escape {
        VEX_TWO_BYTE => {
            cursor.byte()?;
            1
        }
        VEX_THREE_BYTE => {
            let map = cursor.byte()? & 0x1F;
            cursor.byte()?;
            map
        }
        EVEX => {
            let map = cursor.byte()? & 0b111;
            cursor.byte()?;
            cursor.byte()?;
            map
        }
        _ => {
            let map = cursor.byte()? & 0x1F;
            cursor.byte()?;
            map
        }
    };
    let opcode = cursor.byte()?;
    let trailer = trailer(escape, map, opcode)?;
    let has_modrm = escape == EVEX || map != 1 || opcode != VZEROUPPER_OR_VZEROALL;
    if has_modrm {
        modrm::read(cursor, 0)?;
    }
    match trailer {
        Trailer::None => {}
        Trailer::Byte => {
            cursor.byte()?;
        }
        Trailer::Dword => {
            cursor.i32()?;
        }
    }
    Ok(cursor.position() as u8)
}

fn trailer(escape: u8, map: u8, opcode: u8) -> Result<Trailer, DecodeErrorKind> {
    let xop = !matches!(escape, VEX_THREE_BYTE | VEX_TWO_BYTE | EVEX);
    Ok(match (xop, map) {
        (false, 1) if matches!(opcode, 0x70..=0x73 | 0xC2 | 0xC4..=0xC6) => Trailer::Byte,
        (false, 1 | 2) => Trailer::None,
        (false, 3) => Trailer::Byte,
        (false, 5 | 6) if escape == EVEX => Trailer::None,
        (true, 8) => Trailer::Byte,
        (true, 9) => Trailer::None,
        (true, 0xA) => Trailer::Dword,
        _ => return Err(DecodeErrorKind::Invalid),
    })
}
