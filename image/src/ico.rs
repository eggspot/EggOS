use crate::bytes::{le16, le32};
use crate::{Error, Format, Image, Limits, OPAQUE};
use alloc::vec::Vec;

const DIRECTORY: usize = 6;
const ENTRY: usize = 16;
const ICON: u16 = 1;
const CURSOR: u16 = 2;
const MAX_ENTRIES: usize = 512;

/// One image an icon or cursor file's directory lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconEntry {
    /// Its width in pixels, as the directory says (1 to 256).
    pub width: u32,
    /// Its height in pixels, as the directory says (1 to 256).
    pub height: u32,
    /// Its bits per pixel, as the directory says (0 when it does not say).
    pub bit_count: u16,
    /// A cursor's hot spot, `(x, y)`; `None` for an icon.
    pub hotspot: Option<(u16, u16)>,
    data: core::ops::Range<usize>,
}

fn side(byte: u8) -> u32 {
    match byte {
        0 => 256,
        side => u32::from(side),
    }
}

pub(crate) fn kind(bytes: &[u8]) -> Option<Format> {
    let format = match le16(bytes, 2).ok()? {
        ICON => Format::Icon,
        CURSOR => Format::Cursor,
        _ => return None,
    };
    let plausible = le16(bytes, 0).ok()? == 0
        && le16(bytes, 4).ok()? > 0
        && bytes.get(DIRECTORY + 3) == Some(&0)
        && le32(bytes, DIRECTORY + 12).is_ok_and(|offset| (offset as usize) < bytes.len());
    plausible.then_some(format)
}

pub(crate) fn entries(bytes: &[u8]) -> Result<Vec<IconEntry>, Error> {
    let cursor = match kind(bytes) {
        Some(Format::Cursor) => true,
        Some(_) => false,
        None => return Err(Error::UnknownFormat),
    };
    let count = usize::from(le16(bytes, 4)?).min(MAX_ENTRIES);
    let mut found = Vec::new();
    found
        .try_reserve_exact(count)
        .map_err(|_| Error::OutOfMemory)?;
    for index in 0..count {
        let at = DIRECTORY + index * ENTRY;
        if bytes.len() < at + ENTRY {
            break;
        }
        let size = le32(bytes, at + 8)? as usize;
        let offset = le32(bytes, at + 12)? as usize;
        if offset >= bytes.len() {
            continue;
        }
        let end = offset.saturating_add(size).min(bytes.len());
        let (first, second) = (le16(bytes, at + 4)?, le16(bytes, at + 6)?);
        found.push(IconEntry {
            width: side(bytes[at]),
            height: side(bytes[at + 1]),
            bit_count: if cursor { 0 } else { second },
            hotspot: cursor.then_some((first, second)),
            data: offset..end,
        });
    }
    if found.is_empty() {
        return Err(Error::Truncated);
    }
    Ok(found)
}

pub(crate) fn decode_largest(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    let found = entries(bytes)?;
    let best = found
        .iter()
        .enumerate()
        .max_by_key(|(_, entry)| {
            let depth = match entry.bit_count {
                0 => 32,
                bits => bits,
            };
            (u64::from(entry.width) * u64::from(entry.height), depth)
        })
        .map(|(index, _)| index)
        .ok_or(Error::Corrupt)?;
    decode_image(&bytes[found[best].data.clone()], limits)
}

pub(crate) fn decode_entry(bytes: &[u8], index: usize, limits: &Limits) -> Result<Image, Error> {
    let found = entries(bytes)?;
    let entry = found.get(index).ok_or(Error::Corrupt)?;
    decode_image(&bytes[entry.data.clone()], limits)
}

fn decode_image(data: &[u8], limits: &Limits) -> Result<Image, Error> {
    if data.starts_with(&eggos_png::SIGNATURE) {
        return crate::png::decode(data, limits);
    }
    let mut image = crate::bmp::decode_dib(data, None, true, limits)?;
    let own_alpha = bit_count(data)? == 32 && image.pixels.iter().any(|pixel| pixel >> 24 != 0);
    if own_alpha {
        return Ok(image);
    }
    let mask_at = mask_offset(data)?;
    let width = image.width as usize;
    let stride = width.div_ceil(32) * 4;
    let height = image.height as usize;
    for (y, row) in image.pixels.chunks_exact_mut(width).enumerate() {
        let stored = height - 1 - y;
        let mask = mask_at
            .checked_add(stored * stride)
            .and_then(|start| data.get(start..start + stride));
        for (x, pixel) in row.iter_mut().enumerate() {
            let masked = mask.is_some_and(|bits| bits[x / 8] & (0x80 >> (x % 8)) != 0);
            *pixel = match masked {
                true => 0,
                false => *pixel | OPAQUE,
            };
        }
    }
    Ok(image)
}

fn bit_count(dib: &[u8]) -> Result<u16, Error> {
    match le32(dib, 0)? {
        12 => le16(dib, 10),
        _ => le16(dib, 14),
    }
}

fn mask_offset(dib: &[u8]) -> Result<usize, Error> {
    let size = le32(dib, 0)? as usize;
    let (width, height, bits, colours_used) = match size {
        12 => (
            u64::from(le16(dib, 4)?),
            u64::from(le16(dib, 6)?),
            le16(dib, 10)?,
            0,
        ),
        _ => (
            u64::from(le32(dib, 4)?),
            u64::from((le32(dib, 8)? as i32).unsigned_abs()),
            le16(dib, 14)?,
            le32(dib, 32)?,
        ),
    };
    let compression = match size {
        12 => 0,
        _ => le32(dib, 16)?,
    };
    let entry = if size == 12 { 3 } else { 4 };
    let palette = match bits {
        1 | 2 | 4 | 8 => match colours_used {
            0 => 1u64 << bits,
            used => u64::from(used).min(1 << bits),
        },
        _ => 0,
    };
    let masks = match (compression, size) {
        (3, 40) => 12,
        (6, 40) => 16,
        _ => 0,
    };
    let stride = (width * u64::from(bits)).div_ceil(32) * 4;
    let offset = size as u64 + masks + palette * entry + stride * (height / 2);
    usize::try_from(offset).map_err(|_| Error::Corrupt)
}
