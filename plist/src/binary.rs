//! The binary format (`bplist00`), from its public description: an 8-byte header, the objects,
//! an offset table and a 32-byte trailer giving the table's place, its integer width, the
//! object reference width, the object count and the top object. Each object starts with a
//! marker byte whose high four bits are its kind and low four bits its size or count (15: the
//! count follows as an integer object).

use crate::Kind;
use crate::text::{Data, Text};

/// The eight bytes a binary property list starts with.
pub const MAGIC: &[u8; 8] = b"bplist00";

const TRAILER_BYTES: usize = 32;
const LONG_COUNT: u8 = 0xf;

mod marker {
    pub const SINGLETON: u8 = 0x0;
    pub const INTEGER: u8 = 0x1;
    pub const REAL: u8 = 0x2;
    pub const DATE: u8 = 0x3;
    pub const DATA: u8 = 0x4;
    pub const ASCII_STRING: u8 = 0x5;
    pub const UTF16_STRING: u8 = 0x6;
    pub const UID: u8 = 0x8;
    pub const ARRAY: u8 = 0xa;
    pub const SET: u8 = 0xc;
    pub const DICTIONARY: u8 = 0xd;
    pub const NULL: u8 = 0x00;
    pub const FALSE: u8 = 0x08;
    pub const TRUE: u8 = 0x09;
}

fn big_endian(bytes: &[u8]) -> Option<u64> {
    (bytes.len() <= 8).then(|| {
        bytes
            .iter()
            .fold(0u64, |value, &byte| value << 8 | u64::from(byte))
    })
}

/// An opened binary list: its bytes and what its trailer says.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Table<'b> {
    bytes: &'b [u8],
    offset_width: usize,
    reference_width: usize,
    pub objects: usize,
    table: usize,
    pub top: usize,
}

impl<'b> Table<'b> {
    pub fn open(bytes: &'b [u8]) -> Option<Self> {
        if !bytes.starts_with(MAGIC) || bytes.len() < MAGIC.len() + TRAILER_BYTES {
            return None;
        }
        let trailer_at = bytes.len() - TRAILER_BYTES;
        let trailer = &bytes[trailer_at..];
        let offset_width = usize::from(trailer[6]);
        let reference_width = usize::from(trailer[7]);
        let objects = usize::try_from(big_endian(&trailer[8..16])?).ok()?;
        let top = usize::try_from(big_endian(&trailer[16..24])?).ok()?;
        let table = usize::try_from(big_endian(&trailer[24..32])?).ok()?;
        let table_end = table.checked_add(objects.checked_mul(offset_width)?)?;
        let valid = (1..=8).contains(&offset_width)
            && (1..=8).contains(&reference_width)
            && top < objects
            && table >= MAGIC.len()
            && table_end <= trailer_at;
        valid.then_some(Self {
            bytes,
            offset_width,
            reference_width,
            objects,
            table,
            top,
        })
    }

    fn offset_of(&self, object: usize) -> Option<usize> {
        if object >= self.objects {
            return None;
        }
        let start = self.table + object * self.offset_width;
        let entry = self.bytes.get(start..start + self.offset_width)?;
        let offset = usize::try_from(big_endian(entry)?).ok()?;
        (MAGIC.len()..self.table).contains(&offset).then_some(offset)
    }

    fn marker(&self, object: usize) -> Option<(u8, u8, usize)> {
        let at = self.offset_of(object)?;
        let byte = *self.bytes.get(at)?;
        Some((byte >> 4, byte & 0xf, at))
    }

    fn count_at(&self, at: usize, low: u8) -> Option<(usize, usize)> {
        if low != LONG_COUNT {
            return Some((usize::from(low), at + 1));
        }
        let integer = *self.bytes.get(at + 1)?;
        if integer >> 4 != marker::INTEGER || integer & 0xf > 3 {
            return None;
        }
        let width = 1usize << (integer & 0xf);
        let start = at + 2;
        let count = big_endian(self.bytes.get(start..start + width)?)?;
        Some((usize::try_from(count).ok()?, start + width))
    }

    fn payload(&self, object: usize, kind: u8, unit: usize) -> Option<&'b [u8]> {
        let (high, low, at) = self.marker(object)?;
        if high != kind {
            return None;
        }
        let (count, start) = self.count_at(at, low)?;
        self.bytes
            .get(start..start.checked_add(count.checked_mul(unit)?)?)
    }

    fn sized(&self, object: usize, kind: u8) -> Option<&'b [u8]> {
        let (high, low, at) = self.marker(object)?;
        if high != kind || low > 4 {
            return None;
        }
        self.bytes.get(at + 1..at + 1 + (1usize << low))
    }

    pub fn reference_at(&self, refs: usize, index: usize) -> Option<usize> {
        let at = refs.checked_add(index.checked_mul(self.reference_width)?)?;
        let entry = self.bytes.get(at..at.checked_add(self.reference_width)?)?;
        let reference = usize::try_from(big_endian(entry)?).ok()?;
        (reference < self.objects).then_some(reference)
    }

    pub fn kind(&self, object: usize) -> Option<Kind> {
        let (high, low, _) = self.marker(object)?;
        Some(match (high, low) {
            (marker::SINGLETON, marker::NULL) => Kind::Null,
            (marker::SINGLETON, marker::FALSE | marker::TRUE) => Kind::Boolean,
            (marker::INTEGER, _) => Kind::Integer,
            (marker::REAL, _) => Kind::Real,
            (marker::DATE, _) => Kind::Date,
            (marker::DATA, _) => Kind::Data,
            (marker::ASCII_STRING | marker::UTF16_STRING, _) => Kind::String,
            (marker::UID, _) => Kind::Uid,
            (marker::ARRAY, _) => Kind::Array,
            (marker::SET, _) => Kind::Set,
            (marker::DICTIONARY, _) => Kind::Dictionary,
            _ => return None,
        })
    }

    pub fn boolean(&self, object: usize) -> Option<bool> {
        match self.marker(object)? {
            (marker::SINGLETON, marker::TRUE, _) => Some(true),
            (marker::SINGLETON, marker::FALSE, _) => Some(false),
            _ => None,
        }
    }

    pub fn integer(&self, object: usize) -> Option<i128> {
        let bytes = self.sized(object, marker::INTEGER)?;
        Some(match bytes.len() {
            1 | 2 | 4 => i128::from(big_endian(bytes)?),
            8 => i128::from(big_endian(bytes)? as i64),
            _ => i128::from_be_bytes(bytes.try_into().ok()?),
        })
    }

    fn float(&self, object: usize, kind: u8) -> Option<f64> {
        let bytes = self.sized(object, kind)?;
        match bytes.len() {
            4 => Some(f64::from(f32::from_be_bytes(bytes.try_into().ok()?))),
            8 => Some(f64::from_be_bytes(bytes.try_into().ok()?)),
            _ => None,
        }
    }

    pub fn real(&self, object: usize) -> Option<f64> {
        self.float(object, marker::REAL)
    }

    pub fn date(&self, object: usize) -> Option<f64> {
        self.float(object, marker::DATE)
    }

    pub fn uid(&self, object: usize) -> Option<u64> {
        let (high, low, at) = self.marker(object)?;
        if high != marker::UID || low > 7 {
            return None;
        }
        big_endian(self.bytes.get(at + 1..at + 2 + usize::from(low))?)
    }

    pub fn text(&self, object: usize) -> Option<Text<'b>> {
        match self.marker(object)?.0 {
            marker::ASCII_STRING => Some(Text::latin1(self.payload(
                object,
                marker::ASCII_STRING,
                1,
            )?)),
            marker::UTF16_STRING => Some(Text::utf16_big_endian(self.payload(
                object,
                marker::UTF16_STRING,
                2,
            )?)),
            _ => None,
        }
    }

    pub fn data(&self, object: usize) -> Option<Data<'b>> {
        Some(Data::raw(self.payload(object, marker::DATA, 1)?))
    }

    pub fn members(&self, object: usize) -> Option<(usize, usize)> {
        let (high, low, at) = self.marker(object)?;
        if high != marker::ARRAY && high != marker::SET {
            return None;
        }
        let (count, refs) = self.count_at(at, low)?;
        let end = refs.checked_add(count.checked_mul(self.reference_width)?)?;
        (end <= self.table).then_some((refs, count))
    }

    pub fn entries(&self, object: usize) -> Option<(usize, usize)> {
        let (high, low, at) = self.marker(object)?;
        if high != marker::DICTIONARY {
            return None;
        }
        let (count, keys) = self.count_at(at, low)?;
        let end = keys.checked_add(count.checked_mul(2)?.checked_mul(self.reference_width)?)?;
        (end <= self.table).then_some((keys, count))
    }
}
