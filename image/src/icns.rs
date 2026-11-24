//! Apple icon files (`.icns`, D727): a `icns` header with the file's length, then elements, each
//! a four-letter type, its length (header included) and its data. The modern elements hold a
//! whole PNG (or a JPEG 2000) image of one size; the largest PNG within the limits is the one
//! decoded. Files holding only the old run-length elements or JPEG 2000 are unsupported.

use crate::{Error, Image, Limits};

const MAGIC: &[u8; 4] = b"icns";
const HEADER: usize = 8;
const PNG_WIDTH_AT: usize = 16;

fn big_endian(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// Whether `bytes` starts as an Apple icon file.
pub(crate) fn is_icns(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// The data of the widest PNG element no wider or taller than `max_side`.
pub(crate) fn largest_png(bytes: &[u8], max_side: u32) -> Result<&[u8], Error> {
    if !is_icns(bytes) {
        return Err(Error::Corrupt);
    }
    let declared = big_endian(&bytes[MAGIC.len()..]).ok_or(Error::Truncated)? as usize;
    let end = declared.min(bytes.len());
    let mut at = HEADER;
    let mut best: Option<(u32, &[u8])> = None;
    while at + HEADER <= end {
        let length = big_endian(&bytes[at + 4..]).ok_or(Error::Truncated)? as usize;
        if length < HEADER || at + length > end {
            return Err(Error::Corrupt);
        }
        let data = &bytes[at + HEADER..at + length];
        if data.starts_with(&eggos_png::SIGNATURE)
            && let Some(width) = data.get(PNG_WIDTH_AT..).and_then(big_endian)
            && width <= max_side
            && best.is_none_or(|(widest, _)| width > widest)
        {
            best = Some((width, data));
        }
        at += length;
    }
    best.map(|(_, data)| data).ok_or(Error::Unsupported)
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    crate::png::decode(largest_png(bytes, limits.max_side)?, limits)
}
