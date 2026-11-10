use crate::bytes::{le16, le32};
use crate::{Error, Image, Limits, OPAQUE};
use alloc::vec::Vec;

const FILE_HEADER: usize = 14;
const CORE_HEADER: u32 = 12;
const INFO_HEADER: u32 = 40;
const OS2_HEADER: u32 = 64;
const HEADER_SIZES: [u32; 6] = [CORE_HEADER, INFO_HEADER, 52, 56, 108, 124];
const MASKS_IN_HEADER: u32 = 52;
const ALPHA_IN_HEADER: u32 = 56;
const RGB: u32 = 0;
const RLE8: u32 = 1;
const RLE4: u32 = 2;
const BITFIELDS: u32 = 3;
const JPEG: u32 = 4;
const PNG: u32 = 5;
const ALPHA_BITFIELDS: u32 = 6;
const DEFAULT_16_BIT: [u32; 3] = [0x7c00, 0x03e0, 0x001f];
const DEFAULT_32_BIT: [u32; 3] = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff];

pub(crate) fn is_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM")
        && le32(bytes, FILE_HEADER)
            .is_ok_and(|size| HEADER_SIZES.contains(&size) || size == OS2_HEADER)
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    if !bytes.starts_with(b"BM") {
        return Err(Error::UnknownFormat);
    }
    let offset = le32(bytes, 10)? as usize;
    let dib = bytes.get(FILE_HEADER..).ok_or(Error::Truncated)?;
    decode_dib(dib, offset.checked_sub(FILE_HEADER), false, limits)
}

#[derive(Debug, Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    bits: u32,
}

impl Channel {
    fn new(mask: u32) -> Self {
        let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
        Self {
            mask,
            shift,
            bits: (mask >> shift).count_ones(),
        }
    }

    fn value(&self, word: u32, absent: u8) -> u8 {
        if self.bits == 0 {
            return absent;
        }
        let raw = (word & self.mask) >> self.shift;
        if self.bits >= 8 {
            return (raw >> (self.bits - 8)) as u8;
        }
        let largest = (1u32 << self.bits) - 1;
        ((raw.min(largest) * 255 + largest / 2) / largest) as u8
    }
}

#[derive(Debug, Clone, Copy)]
struct Fields {
    red: Channel,
    green: Channel,
    blue: Channel,
    alpha: Channel,
}

impl Fields {
    fn new(masks: [u32; 4]) -> Self {
        Self {
            red: Channel::new(masks[0]),
            green: Channel::new(masks[1]),
            blue: Channel::new(masks[2]),
            alpha: Channel::new(masks[3]),
        }
    }

    fn colour(&self, word: u32) -> u32 {
        crate::argb(
            self.alpha.value(word, 0xff),
            self.red.value(word, 0),
            self.green.value(word, 0),
            self.blue.value(word, 0),
        )
    }
}

struct Header {
    size: u32,
    width: u32,
    height: u32,
    top_down: bool,
    bits: u16,
    compression: u32,
    colours_used: u32,
}

fn header(dib: &[u8], icon: bool) -> Result<Header, Error> {
    let size = le32(dib, 0)?;
    if !HEADER_SIZES.contains(&size) && size != OS2_HEADER {
        return Err(Error::Corrupt);
    }
    if dib.len() < size as usize {
        return Err(Error::Truncated);
    }
    let (width, height, planes, bits, compression, colours_used) = match size {
        CORE_HEADER => (
            i64::from(le16(dib, 4)?),
            i64::from(le16(dib, 6)?),
            le16(dib, 8)?,
            le16(dib, 10)?,
            RGB,
            0,
        ),
        _ => (
            i64::from(le32(dib, 4)? as i32),
            i64::from(le32(dib, 8)? as i32),
            le16(dib, 12)?,
            le16(dib, 14)?,
            le32(dib, 16)?,
            le32(dib, 32)?,
        ),
    };
    if planes != 1 && !icon {
        return Err(Error::Corrupt);
    }
    if size == OS2_HEADER && matches!(compression, BITFIELDS | JPEG) {
        return Err(Error::Unsupported);
    }
    let top_down = height < 0;
    let mut height = height.unsigned_abs();
    if icon {
        height /= 2;
    }
    if width <= 0 || height == 0 {
        return Err(Error::Corrupt);
    }
    let width = u32::try_from(width).map_err(|_| Error::TooLarge)?;
    let height = u32::try_from(height).map_err(|_| Error::TooLarge)?;
    Ok(Header {
        size,
        width,
        height,
        top_down,
        bits,
        compression,
        colours_used,
    })
}

fn masks(dib: &[u8], header: &Header) -> Result<([u32; 4], usize), Error> {
    let size = header.size as usize;
    let explicit = matches!(header.compression, BITFIELDS | ALPHA_BITFIELDS);
    if !explicit {
        let masks = match header.bits {
            16 => [DEFAULT_16_BIT[0], DEFAULT_16_BIT[1], DEFAULT_16_BIT[2], 0],
            _ => [DEFAULT_32_BIT[0], DEFAULT_32_BIT[1], DEFAULT_32_BIT[2], 0],
        };
        return Ok((masks, size));
    }
    if header.size >= MASKS_IN_HEADER {
        let alpha = match header.size >= ALPHA_IN_HEADER {
            true => le32(dib, 52)?,
            false => 0,
        };
        let masks = [le32(dib, 40)?, le32(dib, 44)?, le32(dib, 48)?, alpha];
        return Ok((masks, size));
    }
    let count = match header.compression {
        ALPHA_BITFIELDS => 4,
        _ => 3,
    };
    let mut masks = [0; 4];
    for (index, mask) in masks.iter_mut().take(count).enumerate() {
        *mask = le32(dib, size + 4 * index)?;
    }
    Ok((masks, size + 4 * count))
}

fn palette(dib: &[u8], header: &Header, at: usize) -> Result<(Vec<u32>, usize), Error> {
    if header.bits > 8 {
        return Ok((Vec::new(), at));
    }
    let largest = 1u32 << header.bits;
    let count = match header.colours_used {
        0 => largest,
        used => used.min(largest),
    } as usize;
    let entry = match header.size {
        CORE_HEADER => 3,
        _ => 4,
    };
    let end = at + count * entry;
    let table = dib.get(at..end).ok_or(Error::Truncated)?;
    let mut colours = crate::buffer(count, OPAQUE)?;
    for (colour, bytes) in colours.iter_mut().zip(table.chunks_exact(entry)) {
        *colour = crate::rgb(bytes[2], bytes[1], bytes[0]);
    }
    Ok((colours, end))
}

pub(crate) fn decode_dib(
    dib: &[u8],
    pixels_at: Option<usize>,
    icon: bool,
    limits: &Limits,
) -> Result<Image, Error> {
    let header = header(dib, icon)?;
    crate::check_size(header.width, header.height, limits)?;
    let (masks, after_masks) = masks(dib, &header)?;
    let (palette, after_palette) = palette(dib, &header, after_masks)?;
    let start = match pixels_at {
        Some(at) if at >= after_palette && at < dib.len() => at,
        _ => after_palette,
    };
    let data = dib.get(start..).ok_or(Error::Truncated)?;
    match header.compression {
        JPEG => return crate::jpeg::decode(data, limits),
        PNG => return crate::png::decode(data, limits),
        RLE8 if header.bits == 8 && !header.top_down => {
            return run_lengths(data, &header, &palette, false, limits);
        }
        RLE4 if header.bits == 4 && !header.top_down => {
            return run_lengths(data, &header, &palette, true, limits);
        }
        RGB => {}
        BITFIELDS | ALPHA_BITFIELDS if matches!(header.bits, 16 | 32) => {}
        RLE8 | RLE4 | BITFIELDS | ALPHA_BITFIELDS => return Err(Error::Corrupt),
        _ => return Err(Error::Unsupported),
    }
    if !matches!(header.bits, 1 | 2 | 4 | 8 | 16 | 24 | 32) {
        return Err(Error::Corrupt);
    }
    let mut image = crate::canvas(header.width, header.height, limits)?;
    let stride = (u64::from(header.width) * u64::from(header.bits)).div_ceil(32) * 4;
    let needed = stride * u64::from(header.height);
    if (data.len() as u64) < needed {
        return Err(Error::Truncated);
    }
    let stride = stride as usize;
    let fields = Fields::new(masks);
    let keep_alpha = icon && header.bits == 32 && header.compression == RGB;
    let width = header.width as usize;
    for (y, target) in image.pixels.chunks_exact_mut(width).enumerate() {
        let stored = match header.top_down {
            true => y,
            false => header.height as usize - 1 - y,
        };
        let row = &data[stored * stride..stored * stride + stride];
        decode_row(row, &header, &palette, &fields, keep_alpha, target);
    }
    Ok(image)
}

fn decode_row(
    row: &[u8],
    header: &Header,
    palette: &[u32],
    fields: &Fields,
    keep_alpha: bool,
    target: &mut [u32],
) {
    let indexed = |index: usize| palette.get(index).copied().unwrap_or(OPAQUE);
    match header.bits {
        1 | 2 | 4 | 8 => {
            let bits = usize::from(header.bits);
            let per_byte = 8 / bits;
            let mask = ((1u16 << bits) - 1) as u8;
            for (x, pixel) in target.iter_mut().enumerate() {
                let byte = row[x / per_byte];
                let shift = 8 - bits * (x % per_byte + 1);
                *pixel = indexed(usize::from(byte >> shift & mask));
            }
        }
        16 => {
            for (pixel, bytes) in target.iter_mut().zip(row.as_chunks::<2>().0) {
                *pixel = fields.colour(u32::from(u16::from_le_bytes(*bytes)));
            }
        }
        24 => {
            for (pixel, bytes) in target.iter_mut().zip(row.as_chunks::<3>().0) {
                *pixel = crate::rgb(bytes[2], bytes[1], bytes[0]);
            }
        }
        _ => {
            for (pixel, bytes) in target.iter_mut().zip(row.as_chunks::<4>().0) {
                let word = u32::from_le_bytes(*bytes);
                *pixel = match keep_alpha {
                    true => word,
                    false => fields.colour(word),
                };
            }
        }
    }
}

fn run_lengths(
    data: &[u8],
    header: &Header,
    palette: &[u32],
    nibbles: bool,
    limits: &Limits,
) -> Result<Image, Error> {
    let mut image = crate::canvas(header.width, header.height, limits)?;
    let (width, height) = (header.width as usize, header.height as usize);
    let colour = |index: u8| palette.get(usize::from(index)).copied().unwrap_or(OPAQUE);
    let mut put = |x: usize, line: usize, index: u8| {
        if x < width && line < height {
            image.pixels[(height - 1 - line) * width + x] = colour(index);
        }
    };
    let (mut x, mut line, mut at) = (0usize, 0usize, 0usize);
    while line < height {
        let (Some(&count), Some(&value)) = (data.get(at), data.get(at + 1)) else {
            break;
        };
        at += 2;
        if count > 0 {
            for step in 0..usize::from(count) {
                let index = match (nibbles, step % 2) {
                    (true, 0) => value >> 4,
                    (true, _) => value & 0x0f,
                    (false, _) => value,
                };
                put(x, line, index);
                x = x.saturating_add(1);
            }
            continue;
        }
        match value {
            0 => {
                x = 0;
                line += 1;
            }
            1 => break,
            2 => {
                let (Some(&right), Some(&up)) = (data.get(at), data.get(at + 1)) else {
                    break;
                };
                at += 2;
                x = x.saturating_add(usize::from(right));
                line += usize::from(up);
            }
            count => {
                let count = usize::from(count);
                let stored = match nibbles {
                    true => count.div_ceil(2),
                    false => count,
                };
                let Some(literal) = data.get(at..at + stored) else {
                    break;
                };
                for step in 0..count {
                    let index = match nibbles {
                        true => match step % 2 {
                            0 => literal[step / 2] >> 4,
                            _ => literal[step / 2] & 0x0f,
                        },
                        false => literal[step],
                    };
                    put(x, line, index);
                    x = x.saturating_add(1);
                }
                at += stored + stored % 2;
            }
        }
    }
    Ok(image)
}
