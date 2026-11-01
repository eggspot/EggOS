use crate::bytes::{be16, be32, le16, le32};
use crate::lzw::{self, Flavour, Table};
use crate::{Error, Image, Limits};
use alloc::vec::Vec;
use eggos_png::inflate::{self, InflateError, SliceSource};

const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const PHOTOMETRIC: u16 = 262;
const FILL_ORDER: u16 = 266;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIGURATION: u16 = 284;
const PREDICTOR: u16 = 317;
const COLOR_MAP: u16 = 320;
const TILE_WIDTH: u16 = 322;
const EXTRA_SAMPLES: u16 = 338;
const SAMPLE_FORMAT: u16 = 339;

const TYPE_BYTE: u16 = 1;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;

const UNCOMPRESSED: u32 = 1;
const LZW: u32 = 5;
const DEFLATE: u32 = 8;
const DEFLATE_OLD: u32 = 32_946;
const PACKBITS: u32 = 32_773;

const WHITE_IS_ZERO: u32 = 0;
const BLACK_IS_ZERO: u32 = 1;
const RGB: u32 = 2;
const PALETTE: u32 = 3;
const SEPARATED: u32 = 5;

const NO_PREDICTOR: u32 = 1;
const HORIZONTAL_PREDICTOR: u32 = 2;
const ASSOCIATED_ALPHA: u32 = 1;
const UNASSOCIATED_ALPHA: u32 = 2;
const CHUNKY: u32 = 1;
const UNSIGNED: u32 = 1;
const MAX_ENTRIES: usize = 4096;
const MAX_SAMPLES: u32 = 8;
const ENTRY: usize = 12;
const LZW_LITERAL_BITS: u32 = 8;
const ZLIB_DEFLATE: u8 = 8;
const ZLIB_HEADER: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Little,
    Big,
}

#[derive(Clone, Copy)]
struct Entry {
    kind: u16,
    count: u32,
    at: usize,
}

struct File<'a> {
    bytes: &'a [u8],
    order: Order,
}

impl File<'_> {
    fn u16_at(&self, at: usize) -> Result<u16, Error> {
        match self.order {
            Order::Little => le16(self.bytes, at),
            Order::Big => be16(self.bytes, at),
        }
    }

    fn u32_at(&self, at: usize) -> Result<u32, Error> {
        match self.order {
            Order::Little => le32(self.bytes, at),
            Order::Big => be32(self.bytes, at),
        }
    }

    fn entry(&self, at: usize) -> Result<(u16, Entry), Error> {
        let tag = self.u16_at(at)?;
        let kind = self.u16_at(at + 2)?;
        let count = self.u32_at(at + 4)?;
        let size = match kind {
            TYPE_BYTE => 1,
            TYPE_SHORT => 2,
            TYPE_LONG => 4,
            _ => 0,
        };
        let length = u64::from(count) * size;
        let at = match length <= 4 {
            true => at + 8,
            false => self.u32_at(at + 8)? as usize,
        };
        if at as u64 + length > self.bytes.len() as u64 {
            return Err(Error::Truncated);
        }
        Ok((tag, Entry { kind, count, at }))
    }

    fn value(&self, entry: Entry, index: u32) -> Result<u32, Error> {
        if index >= entry.count {
            return Err(Error::Corrupt);
        }
        let index = index as usize;
        match entry.kind {
            TYPE_BYTE => Ok(u32::from(self.bytes[entry.at + index])),
            TYPE_SHORT => self.u16_at(entry.at + 2 * index).map(u32::from),
            TYPE_LONG => self.u32_at(entry.at + 4 * index),
            _ => Err(Error::Corrupt),
        }
    }
}

#[derive(Default)]
struct Tags {
    width: Option<Entry>,
    height: Option<Entry>,
    bits: Option<Entry>,
    compression: Option<Entry>,
    photometric: Option<Entry>,
    fill_order: Option<Entry>,
    offsets: Option<Entry>,
    samples: Option<Entry>,
    rows_per_strip: Option<Entry>,
    byte_counts: Option<Entry>,
    planar: Option<Entry>,
    predictor: Option<Entry>,
    colour_map: Option<Entry>,
    extra: Option<Entry>,
    sample_format: Option<Entry>,
    tiled: bool,
}

struct Layout {
    width: usize,
    height: usize,
    bits: u32,
    samples: u32,
    colour_samples: u32,
    alpha: Option<bool>,
    photometric: u32,
    row_bytes: usize,
}

fn tags(file: &File<'_>) -> Result<Tags, Error> {
    let directory = file.u32_at(4)? as usize;
    let count = usize::from(file.u16_at(directory)?);
    if count > MAX_ENTRIES {
        return Err(Error::Corrupt);
    }
    let mut tags = Tags::default();
    for index in 0..count {
        let (tag, entry) = file.entry(directory + 2 + index * ENTRY)?;
        let slot = match tag {
            IMAGE_WIDTH => &mut tags.width,
            IMAGE_LENGTH => &mut tags.height,
            BITS_PER_SAMPLE => &mut tags.bits,
            COMPRESSION => &mut tags.compression,
            PHOTOMETRIC => &mut tags.photometric,
            FILL_ORDER => &mut tags.fill_order,
            STRIP_OFFSETS => &mut tags.offsets,
            SAMPLES_PER_PIXEL => &mut tags.samples,
            ROWS_PER_STRIP => &mut tags.rows_per_strip,
            STRIP_BYTE_COUNTS => &mut tags.byte_counts,
            PLANAR_CONFIGURATION => &mut tags.planar,
            PREDICTOR => &mut tags.predictor,
            COLOR_MAP => &mut tags.colour_map,
            EXTRA_SAMPLES => &mut tags.extra,
            SAMPLE_FORMAT => &mut tags.sample_format,
            TILE_WIDTH => {
                tags.tiled = true;
                continue;
            }
            _ => continue,
        };
        *slot = Some(entry);
    }
    Ok(tags)
}

fn first(file: &File<'_>, entry: Option<Entry>, absent: u32) -> Result<u32, Error> {
    match entry {
        Some(entry) => file.value(entry, 0),
        None => Ok(absent),
    }
}

fn all_equal(file: &File<'_>, entry: Option<Entry>, expected: u32) -> Result<bool, Error> {
    let Some(entry) = entry else {
        return Ok(true);
    };
    for index in 0..entry.count.min(MAX_SAMPLES) {
        if file.value(entry, index)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn layout(file: &File<'_>, tags: &Tags, limits: &Limits) -> Result<Layout, Error> {
    let width = first(file, tags.width, 0)?;
    let height = first(file, tags.height, 0)?;
    crate::check_size(width, height, limits)?;
    let bits = first(file, tags.bits, 1)?;
    let samples = first(file, tags.samples, 1)?;
    let photometric = first(file, tags.photometric, BLACK_IS_ZERO)?;
    let plain = !tags.tiled
        && first(file, tags.planar, CHUNKY)? == CHUNKY
        && first(file, tags.fill_order, 1)? == 1
        && all_equal(file, tags.sample_format, UNSIGNED)?
        && all_equal(file, tags.bits, bits)?;
    if !plain || !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Err(Error::Unsupported);
    }
    let colour_samples = match photometric {
        WHITE_IS_ZERO | BLACK_IS_ZERO => 1,
        PALETTE if bits <= 8 => 1,
        RGB if bits >= 8 => 3,
        SEPARATED if bits >= 8 => 4,
        _ => return Err(Error::Unsupported),
    };
    if samples < colour_samples || samples > MAX_SAMPLES {
        return Err(Error::Corrupt);
    }
    let alpha = match (samples > colour_samples, tags.extra) {
        (true, Some(extra)) => match file.value(extra, 0)? {
            ASSOCIATED_ALPHA => Some(true),
            UNASSOCIATED_ALPHA => Some(false),
            _ => None,
        },
        _ => None,
    };
    let row_bits = u64::from(width) * u64::from(samples) * u64::from(bits);
    let row_bytes = usize::try_from(row_bits.div_ceil(8)).map_err(|_| Error::TooLarge)?;
    Ok(Layout {
        width: width as usize,
        height: height as usize,
        bits,
        samples,
        colour_samples,
        alpha,
        photometric,
        row_bytes,
    })
}

fn unpack_bits(source: &[u8], out: &mut [u8]) {
    let (mut at, mut written) = (0, 0);
    while written < out.len() {
        let Some(&header) = source.get(at) else {
            break;
        };
        at += 1;
        let header = header as i8;
        if header >= 0 {
            let count = header as usize + 1;
            let end = (written + count).min(out.len());
            let Some(literal) = source.get(at..at + (end - written)) else {
                break;
            };
            out[written..end].copy_from_slice(literal);
            at += count;
            written = end;
        } else if header != i8::MIN {
            let Some(&value) = source.get(at) else {
                break;
            };
            at += 1;
            let end = (written + 1 + header.unsigned_abs() as usize).min(out.len());
            out[written..end].fill(value);
            written = end;
        }
    }
}

fn inflate_strip(source: &[u8], out: &mut [u8]) -> Result<(), Error> {
    let header = source.get(..ZLIB_HEADER).ok_or(Error::Truncated)?;
    let checked = (u16::from(header[0]) << 8 | u16::from(header[1])).is_multiple_of(31);
    if header[0] & 0x0f != ZLIB_DEFLATE || !checked {
        return Err(Error::Corrupt);
    }
    let mut stream = SliceSource::new(&source[ZLIB_HEADER..]);
    match inflate::inflate_into(&mut stream, out) {
        Ok(_) | Err(InflateError::TooLong) => Ok(()),
        Err(InflateError::Truncated) => Err(Error::Truncated),
        Err(InflateError::Corrupt | InflateError::Stopped) => Err(Error::Corrupt),
    }
}

fn strips(file: &File<'_>, tags: &Tags, layout: &Layout) -> Result<Vec<u8>, Error> {
    let compression = first(file, tags.compression, UNCOMPRESSED)?;
    if !matches!(compression, UNCOMPRESSED | LZW | DEFLATE | DEFLATE_OLD | PACKBITS) {
        return Err(Error::Unsupported);
    }
    let total = layout
        .row_bytes
        .checked_mul(layout.height)
        .ok_or(Error::TooLarge)?;
    let mut raw = crate::buffer(total, 0u8)?;
    let offsets = tags.offsets.ok_or(Error::Corrupt)?;
    let rows_per_strip = (first(file, tags.rows_per_strip, u32::MAX)? as usize).max(1);
    let strip_bytes = rows_per_strip.saturating_mul(layout.row_bytes);
    let mut table = match compression {
        LZW => Some(Table::new()?),
        _ => None,
    };
    for (index, out) in raw.chunks_mut(strip_bytes).enumerate() {
        let Ok(index) = u32::try_from(index) else {
            break;
        };
        if index >= offsets.count {
            break;
        }
        let start = file.value(offsets, index)? as usize;
        let length = match tags.byte_counts {
            Some(counts) => file.value(counts, index)? as usize,
            None if compression == UNCOMPRESSED => out.len(),
            None => return Err(Error::Corrupt),
        };
        let end = start.saturating_add(length).min(file.bytes.len());
        let source = file.bytes.get(start..end).ok_or(Error::Truncated)?;
        match (compression, table.as_mut()) {
            (UNCOMPRESSED, _) => {
                let count = source.len().min(out.len());
                out[..count].copy_from_slice(&source[..count]);
            }
            (PACKBITS, _) => unpack_bits(source, out),
            (LZW, Some(table)) => {
                lzw::decode(source, LZW_LITERAL_BITS, Flavour::Tiff, table, out)?;
            }
            _ => inflate_strip(source, out)?,
        }
    }
    match first(file, tags.predictor, NO_PREDICTOR)? {
        NO_PREDICTOR => {}
        HORIZONTAL_PREDICTOR => undo_prediction(file.order, layout, &mut raw)?,
        _ => return Err(Error::Unsupported),
    }
    Ok(raw)
}

fn undo_prediction(order: Order, layout: &Layout, raw: &mut [u8]) -> Result<(), Error> {
    let samples = layout.samples as usize;
    match layout.bits {
        8 => {
            for row in raw.chunks_exact_mut(layout.row_bytes) {
                for at in samples..row.len() {
                    row[at] = row[at].wrapping_add(row[at - samples]);
                }
            }
        }
        16 => {
            let read = |pair: &[u8]| match order {
                Order::Little => u16::from_le_bytes([pair[0], pair[1]]),
                Order::Big => u16::from_be_bytes([pair[0], pair[1]]),
            };
            for row in raw.chunks_exact_mut(layout.row_bytes) {
                for at in (2 * samples..row.len() - row.len() % 2).step_by(2) {
                    let sum = read(&row[at..at + 2]).wrapping_add(read(&row[at - 2 * samples..]));
                    let bytes = match order {
                        Order::Little => sum.to_le_bytes(),
                        Order::Big => sum.to_be_bytes(),
                    };
                    row[at..at + 2].copy_from_slice(&bytes);
                }
            }
        }
        _ => return Err(Error::Unsupported),
    }
    Ok(())
}

fn sample_at(row: &[u8], layout: &Layout, index: usize, order: Order) -> u32 {
    match layout.bits {
        8 => u32::from(row[index]),
        16 => {
            let pair = [row[2 * index], row[2 * index + 1]];
            u32::from(match order {
                Order::Little => u16::from_le_bytes(pair),
                Order::Big => u16::from_be_bytes(pair),
            })
        }
        bits => {
            let bit = index * bits as usize;
            let shift = 8 - bits as usize - bit % 8;
            u32::from(row[bit / 8] >> shift) & ((1 << bits) - 1)
        }
    }
}

fn eight_bits(value: u32, bits: u32) -> u8 {
    match bits {
        16 => (value >> 8) as u8,
        8 => value as u8,
        bits => (value * 255 / ((1 << bits) - 1)) as u8,
    }
}

fn colour_map(file: &File<'_>, tags: &Tags, bits: u32) -> Result<Vec<u32>, Error> {
    let entry = tags.colour_map.ok_or(Error::Corrupt)?;
    let size = 1u32 << bits;
    if entry.count < 3 * size {
        return Err(Error::Corrupt);
    }
    let mut colours = crate::buffer(size as usize, 0u32)?;
    for (index, colour) in (0..size).zip(colours.iter_mut()) {
        let channel = |part: u32| file.value(entry, part * size + index).map(|v| (v >> 8) as u8);
        *colour = crate::rgb(channel(0)?, channel(1)?, channel(2)?);
    }
    Ok(colours)
}

fn straight(channel: u8, alpha: u8) -> u8 {
    match alpha {
        0 => 0,
        alpha => {
            let value = (u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
            value.min(255) as u8
        }
    }
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    let order = match bytes.get(..4) {
        Some(b"II*\0") => Order::Little,
        Some(b"MM\0*") => Order::Big,
        _ => return Err(Error::UnknownFormat),
    };
    let file = File { bytes, order };
    let tags = tags(&file)?;
    let layout = layout(&file, &tags, limits)?;
    let palette = match layout.photometric {
        PALETTE => colour_map(&file, &tags, layout.bits)?,
        _ => Vec::new(),
    };
    let raw = strips(&file, &tags, &layout)?;
    let mut image = crate::canvas(layout.width as u32, layout.height as u32, limits)?;
    let samples = layout.samples as usize;
    let bits = layout.bits;
    for (target, row) in image
        .pixels
        .chunks_exact_mut(layout.width)
        .zip(raw.chunks_exact(layout.row_bytes))
    {
        for (x, pixel) in target.iter_mut().enumerate() {
            let at = x * samples;
            let value = |index: usize| sample_at(row, &layout, at + index, order);
            let channel = |index: usize| eight_bits(value(index), bits);
            let alpha = match layout.alpha {
                Some(_) => channel(layout.colour_samples as usize),
                None => 0xff,
            };
            let (red, green, blue) = match layout.photometric {
                WHITE_IS_ZERO => {
                    let grey = 255 - channel(0);
                    (grey, grey, grey)
                }
                PALETTE => {
                    let colour = palette.get(value(0) as usize).copied().unwrap_or(0);
                    ((colour >> 16) as u8, (colour >> 8) as u8, colour as u8)
                }
                RGB => (channel(0), channel(1), channel(2)),
                SEPARATED => {
                    let key = 255 - u32::from(channel(3));
                    let ink = |index: usize| ((255 - u32::from(channel(index))) * key / 255) as u8;
                    (ink(0), ink(1), ink(2))
                }
                _ => {
                    let grey = channel(0);
                    (grey, grey, grey)
                }
            };
            *pixel = match layout.alpha {
                Some(true) => crate::argb(
                    alpha,
                    straight(red, alpha),
                    straight(green, alpha),
                    straight(blue, alpha),
                ),
                _ => crate::argb(alpha, red, green, blue),
            };
        }
    }
    Ok(image)
}
