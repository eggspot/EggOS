//! Device-independent bitmaps and image files (D178): a `BITMAPINFOHEADER` (and its V4 and V5
//! forms) with its channel masks or colour table, rows of 1, 4, 8, 16, 24 and 32 bits per
//! pixel read into the layer's 32-bit pixels and written back, `.bmp` files, packed DIBs (the
//! bitmap and icon resources), PNG images through `eggos_png`, nearest-neighbour scaling and
//! the alpha arithmetic images are drawn with. A pixel is `0xAARRGGBB`, as a 32-bit DIB holds it
//! in memory; a format without alpha leaves the top byte 0. All of it works on bytes alone, so
//! it is tested on the host.

use eggos_png::inflate::SliceSource;
use eggos_png::rows::{RowDecoder, RowWorkspace};

/// The size of a `BITMAPINFOHEADER`, the smallest header this layer reads.
pub const INFO_HEADER_SIZE: usize = 40;
/// The widest and tallest bitmap the layer makes, in pixels.
pub const LARGEST_SIDE: u32 = 16_384;
/// The mask pixel of a masked (transparent) point; an unmasked one is 0.
pub const MASKED: u32 = 0x00ff_ffff;
const LARGEST_INFO_HEADER: usize = 124;
const FILE_HEADER_SIZE: usize = 14;
const MASKS_SIZE: usize = 12;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const NATIVE_MASKS: [u32; 3] = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff];
const MASKS_555: [u32; 3] = [0x7c00, 0x03e0, 0x001f];
const ALPHA_BYTE: u32 = 0xff00_0000;
const MONOCHROME_PALETTE: [u8; 8] = [0, 0, 0, 0, 0xff, 0xff, 0xff, 0];

/// What a DIB's header says about its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub bottom_up: bool,
    pub bits: u16,
    pub compression: u32,
    pub masks: [u32; 3],
    pub alpha_mask: u32,
    pub alpha_declared: bool,
    pub colours: usize,
    pub table_offset: usize,
}

/// The little-endian `u16` at `at`, if `bytes` hold it.
pub fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The little-endian `u32` at `at`, if `bytes` hold it.
pub fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The bytes a row of `width` pixels of `bits` bits takes in a DIB: whole 32-bit words.
pub fn stride(width: u32, bits: u16) -> usize {
    (u64::from(width) * u64::from(bits)).div_ceil(32) as usize * 4
}

impl Layout {
    /// A layout of nothing, for tables that start empty.
    pub const EMPTY: Layout = Layout {
        width: 0,
        height: 0,
        bottom_up: false,
        bits: 0,
        compression: BI_RGB,
        masks: [0; 3],
        alpha_mask: 0,
        alpha_declared: false,
        colours: 0,
        table_offset: 0,
    };

    /// A top-down 32-bit layout of `width` by `height` pixels, the layer's own.
    pub const fn native(width: u32, height: u32) -> Layout {
        Layout {
            width,
            height,
            bottom_up: false,
            bits: 32,
            compression: BI_RGB,
            masks: NATIVE_MASKS,
            alpha_mask: ALPHA_BYTE,
            alpha_declared: false,
            colours: 0,
            table_offset: INFO_HEADER_SIZE,
        }
    }

    /// The layout `info` (a `BITMAPINFO`: a header, then its masks or colour table) describes;
    /// `None` for a header this layer does not read (a core header, compression other than
    /// `BI_RGB` and `BI_BITFIELDS`, an empty or too large bitmap).
    pub fn parse(info: &[u8]) -> Option<Layout> {
        let size = u32_at(info, 0)? as usize;
        if !(INFO_HEADER_SIZE..=LARGEST_INFO_HEADER).contains(&size) {
            return None;
        }
        let width = u32_at(info, 4)? as i32;
        let height = u32_at(info, 8)? as i32;
        let planes = u16_at(info, 12)?;
        let bits = u16_at(info, 14)?;
        let compression = u32_at(info, 16)?;
        let used = u32_at(info, 32)? as usize;
        if planes != 1 || width <= 0 || height == 0 || height == i32::MIN {
            return None;
        }
        let (width, rows) = (width as u32, height.unsigned_abs());
        if width > LARGEST_SIDE || rows > LARGEST_SIDE {
            return None;
        }
        let in_header = size >= INFO_HEADER_SIZE + MASKS_SIZE;
        let (masks, alpha_mask, table_offset) = match (bits, compression) {
            (1 | 4 | 8 | 24, BI_RGB) => ([0; 3], 0, size),
            (16, BI_RGB) => (MASKS_555, 0, size),
            (32, BI_RGB) => (NATIVE_MASKS, ALPHA_BYTE, size),
            (16 | 32, BI_BITFIELDS) => {
                let masks = [u32_at(info, 40)?, u32_at(info, 44)?, u32_at(info, 48)?];
                match in_header {
                    true => (
                        masks,
                        u32_at(info, 52).filter(|_| size >= 56).unwrap_or(0),
                        size,
                    ),
                    false => (masks, 0, size + MASKS_SIZE),
                }
            }
            _ => return None,
        };
        let colours = match bits {
            1 | 4 | 8 if used == 0 => 1 << bits,
            1 | 4 | 8 => used.min(1 << bits),
            _ => used.min(256),
        };
        Some(Layout {
            width,
            height: rows,
            bottom_up: height > 0,
            bits,
            compression,
            masks,
            alpha_mask,
            alpha_declared: compression == BI_BITFIELDS && alpha_mask != 0,
            colours,
            table_offset,
        })
    }

    /// The whole `BITMAPINFO`'s size (header, masks and colour table) from its first
    /// [`INFO_HEADER_SIZE`] bytes, so a caller knows how much of a program's structure to read.
    pub fn info_size(header: &[u8]) -> Option<usize> {
        let size = u32_at(header, 0)? as usize;
        let bits = u16_at(header, 14)?;
        let compression = u32_at(header, 16)?;
        let used = u32_at(header, 32)? as usize;
        if !(INFO_HEADER_SIZE..=LARGEST_INFO_HEADER).contains(&size) {
            return None;
        }
        let masks = match compression == BI_BITFIELDS && size < INFO_HEADER_SIZE + MASKS_SIZE {
            true => MASKS_SIZE,
            false => 0,
        };
        let colours = match bits {
            1 | 4 | 8 if used == 0 => 1 << bits,
            1 | 4 | 8 => used.min(1 << bits),
            _ => used.min(256),
        };
        Some(size + masks + colours * 4)
    }

    /// The bytes one row takes.
    pub fn stride(&self) -> usize {
        stride(self.width, self.bits)
    }

    /// The bytes all the rows take.
    pub fn bits_size(&self) -> usize {
        self.stride() * self.height as usize
    }

    /// Where the colour table ends, from the header's start: where a packed DIB's rows start.
    pub fn palette_end(&self) -> usize {
        self.table_offset + self.colours * 4
    }

    /// Whether the rows are the layer's own pixels: 32 bits, top-down, the usual channels.
    pub fn is_native(&self) -> bool {
        self.bits == 32 && !self.bottom_up && self.masks == NATIVE_MASKS
    }

    /// The DIB row that holds image row `row` (0 at the top).
    pub fn source_row(&self, row: usize) -> usize {
        match self.bottom_up {
            true => self.height as usize - 1 - row,
            false => row,
        }
    }
}

fn channel(value: u32, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let top = u64::from(mask >> shift);
    let sample = u64::from((value & mask) >> shift);
    (sample * 255 / top) as u32
}

fn place(channel: u32, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let top = u64::from(mask >> shift);
    let sample = (u64::from(channel & 0xff) * top + 127) / 255;
    ((sample as u32) << shift) & mask
}

fn rgb(red: u32, green: u32, blue: u32) -> u32 {
    red << 16 | green << 8 | blue
}

fn palette_colour(palette: &[u8], index: usize) -> u32 {
    match palette.get(index * 4..index * 4 + 3) {
        Some(&[blue, green, red]) => rgb(u32::from(red), u32::from(green), u32::from(blue)),
        _ => 0,
    }
}

fn distance(a: u32, b: u32) -> u32 {
    [16, 8, 0]
        .into_iter()
        .map(|shift| {
            let difference = ((a >> shift) & 0xff).abs_diff((b >> shift) & 0xff);
            difference * difference
        })
        .sum()
}

fn nearest(palette: &[u8], colours: usize, pixel: u32) -> usize {
    let pixel = pixel & 0x00ff_ffff;
    (0..colours.min(palette.len() / 4))
        .min_by_key(|&index| distance(palette_colour(palette, index), pixel))
        .unwrap_or(0)
}

/// Reads one DIB row in `layout`'s format into `out` (one pixel per column).
pub fn read_row(layout: &Layout, palette: &[u8], row: &[u8], out: &mut [u32]) {
    let width = (layout.width as usize).min(out.len());
    let out = &mut out[..width];
    match layout.bits {
        1 | 4 | 8 => {
            let bits = usize::from(layout.bits);
            let per_byte = 8 / bits;
            let top = (1usize << bits) - 1;
            for (x, pixel) in out.iter_mut().enumerate() {
                let byte = usize::from(row.get(x / per_byte).copied().unwrap_or(0));
                let shift = 8 - bits * (x % per_byte + 1);
                *pixel = palette_colour(palette, (byte >> shift) & top);
            }
        }
        16 => {
            for (x, pixel) in out.iter_mut().enumerate() {
                let value = u32::from(u16_at(row, 2 * x).unwrap_or(0));
                let [red, green, blue] = layout.masks.map(|mask| channel(value, mask));
                *pixel = rgb(red, green, blue);
            }
        }
        24 => {
            for (x, pixel) in out.iter_mut().enumerate() {
                *pixel = match row.get(3 * x..3 * x + 3) {
                    Some(&[blue, green, red]) => {
                        rgb(u32::from(red), u32::from(green), u32::from(blue))
                    }
                    _ => 0,
                };
            }
        }
        _ => {
            let native = layout.masks == NATIVE_MASKS && layout.alpha_mask == ALPHA_BYTE;
            for (x, pixel) in out.iter_mut().enumerate() {
                let value = u32_at(row, 4 * x).unwrap_or(0);
                *pixel = match native {
                    true => value,
                    false => {
                        let [red, green, blue] = layout.masks.map(|mask| channel(value, mask));
                        channel(value, layout.alpha_mask) << 24 | rgb(red, green, blue)
                    }
                };
            }
        }
    }
}

/// Writes `pixels` as one DIB row in `layout`'s format into `out`; a palette format takes each
/// pixel's nearest colour in `palette`.
pub fn write_row(layout: &Layout, palette: &[u8], pixels: &[u32], out: &mut [u8]) {
    let pixels = &pixels[..pixels.len().min(layout.width as usize)];
    match layout.bits {
        1 | 4 | 8 => {
            let bits = usize::from(layout.bits);
            let per_byte = 8 / bits;
            let used = pixels.len().div_ceil(per_byte).min(out.len());
            out[..used].fill(0);
            for (x, &pixel) in pixels.iter().enumerate() {
                let index = nearest(palette, layout.colours, pixel);
                let shift = 8 - bits * (x % per_byte + 1);
                if let Some(byte) = out.get_mut(x / per_byte) {
                    *byte |= (index << shift) as u8;
                }
            }
        }
        16 => {
            for (x, &pixel) in pixels.iter().enumerate() {
                let [red, green, blue] = [pixel >> 16, pixel >> 8, pixel];
                let [red_mask, green_mask, blue_mask] = layout.masks;
                let value =
                    place(red, red_mask) | place(green, green_mask) | place(blue, blue_mask);
                if let Some(bytes) = out.get_mut(2 * x..2 * x + 2) {
                    bytes.copy_from_slice(&(value as u16).to_le_bytes());
                }
            }
        }
        24 => {
            for (x, &pixel) in pixels.iter().enumerate() {
                if let Some(bytes) = out.get_mut(3 * x..3 * x + 3) {
                    bytes.copy_from_slice(&[pixel as u8, (pixel >> 8) as u8, (pixel >> 16) as u8]);
                }
            }
        }
        _ => {
            let native = layout.masks == NATIVE_MASKS && layout.alpha_mask == ALPHA_BYTE;
            for (x, &pixel) in pixels.iter().enumerate() {
                let value = match native {
                    true => pixel,
                    false => {
                        let [red_mask, green_mask, blue_mask] = layout.masks;
                        place(pixel >> 16, red_mask)
                            | place(pixel >> 8, green_mask)
                            | place(pixel, blue_mask)
                            | place(pixel >> 24, layout.alpha_mask)
                    }
                };
                if let Some(bytes) = out.get_mut(4 * x..4 * x + 4) {
                    bytes.copy_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
}

/// Reads all of `bits` (rows in `layout`'s format) into `out`, top row first; `false` when
/// either is too short.
pub fn decode(layout: &Layout, palette: &[u8], bits: &[u8], out: &mut [u32]) -> bool {
    let (width, height, stride) = (
        layout.width as usize,
        layout.height as usize,
        layout.stride(),
    );
    if bits.len() < stride * height || out.len() < width * height {
        return false;
    }
    for row in 0..height {
        let at = layout.source_row(row) * stride;
        read_row(
            layout,
            palette,
            &bits[at..at + stride],
            &mut out[row * width..(row + 1) * width],
        );
    }
    true
}

/// Writes image rows `rows` (top row 0) of `pixels` into `bits` in `layout`'s format.
pub fn encode(
    layout: &Layout,
    palette: &[u8],
    pixels: &[u32],
    bits: &mut [u8],
    rows: core::ops::Range<usize>,
) {
    let (width, stride) = (layout.width as usize, layout.stride());
    let end = rows.end.min(layout.height as usize);
    for row in rows.start.min(end)..end {
        let at = layout.source_row(row) * stride;
        let (Some(source), Some(target)) = (
            pixels.get(row * width..(row + 1) * width),
            bits.get_mut(at..at + stride),
        ) else {
            return;
        };
        write_row(layout, palette, source, target);
    }
}

/// A DIB in memory: its layout, colour table and rows.
#[derive(Debug, Clone, Copy)]
pub struct Packed<'a> {
    pub layout: Layout,
    pub palette: &'a [u8],
    pub bits: &'a [u8],
}

impl Packed<'_> {
    /// Reads its pixels into `out`, top row first.
    pub fn decode(&self, out: &mut [u32]) -> bool {
        decode(&self.layout, self.palette, self.bits, out)
    }
}

/// A packed DIB (a header, its colour table, then its rows), as a bitmap resource holds one.
pub fn packed(dib: &[u8]) -> Option<Packed<'_>> {
    let layout = Layout::parse(dib)?;
    let start = layout.palette_end();
    Some(Packed {
        layout,
        palette: dib.get(layout.table_offset..start)?,
        bits: dib.get(start..start.checked_add(layout.bits_size())?)?,
    })
}

/// The DIB a `.bmp` file holds.
pub fn bmp_file(file: &[u8]) -> Option<Packed<'_>> {
    if file.get(..2)? != b"BM" {
        return None;
    }
    let offset = u32_at(file, 10)? as usize;
    let info = file.get(FILE_HEADER_SIZE..)?;
    let layout = Layout::parse(info)?;
    Some(Packed {
        layout,
        palette: info.get(layout.table_offset..layout.palette_end())?,
        bits: file.get(offset..offset.checked_add(layout.bits_size())?)?,
    })
}

/// An icon or cursor image in DIB form: its colour rows and its AND mask (one bit per pixel,
/// set where the image is transparent; empty when the data stops before it).
#[derive(Debug, Clone, Copy)]
pub struct IconDib<'a> {
    pub colour: Packed<'a>,
    pub mask: &'a [u8],
}

impl IconDib<'_> {
    /// Reads the AND mask into `out`, top row first: [`MASKED`] where a bit is set.
    pub fn decode_mask(&self, out: &mut [u32]) {
        let layout = Layout {
            bits: 1,
            colours: 2,
            ..self.colour.layout
        };
        if !decode(&layout, &MONOCHROME_PALETTE, self.mask, out) {
            out.fill(0);
        }
    }
}

/// The image of an icon resource or `.ico` entry in DIB form, whose header's height counts
/// the colour rows and the mask's.
pub fn icon_dib(dib: &[u8]) -> Option<IconDib<'_>> {
    let mut layout = Layout::parse(dib)?;
    layout.height = (layout.height / 2).max(1);
    let start = layout.palette_end();
    let colour_end = start.checked_add(layout.bits_size())?;
    let mask_size = stride(layout.width, 1) * layout.height as usize;
    Some(IconDib {
        colour: Packed {
            layout,
            palette: dib.get(layout.table_offset..start)?,
            bits: dib.get(start..colour_end)?,
        },
        mask: dib.get(colour_end..colour_end + mask_size).unwrap_or(&[]),
    })
}

/// The size a PNG's header gives, if `bytes` start with one.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if *bytes.get(..8)? != eggos_png::SIGNATURE || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let (width, height) = (
        u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
        u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
    );
    let fits = (1..=LARGEST_SIDE).contains(&width) && (1..=LARGEST_SIDE).contains(&height);
    fits.then_some((width, height))
}

/// Decodes the PNG in `bytes` into `out` (straight-alpha `0xAARRGGBB`, top row first), with
/// `work` as its memory; `false` when it is not a PNG the decoder takes or `out` is too short.
pub fn decode_png(bytes: &[u8], work: &mut RowWorkspace, out: &mut [u32]) -> bool {
    let mut source = SliceSource::new(bytes);
    let Ok(decoder) = RowDecoder::open(&mut source) else {
        return false;
    };
    let header = decoder.header();
    let width = header.width as usize;
    if out.len() < width * header.height as usize {
        return false;
    }
    decoder
        .read_rows(work, |row, pixels| {
            let at = row as usize * width;
            out[at..at + width].copy_from_slice(pixels);
        })
        .is_ok()
}

/// Scales `from` (`from_size` pixels) into `to` (`to_size` pixels), each target pixel the
/// source pixel its centre-less position falls on.
pub fn scale_nearest(from: &[u32], from_size: (u32, u32), to: &mut [u32], to_size: (u32, u32)) {
    let (from_width, from_height) = (from_size.0 as usize, from_size.1 as usize);
    let (to_width, to_height) = (to_size.0 as usize, to_size.1 as usize);
    if from_width == 0 || from_height == 0 || from.len() < from_width * from_height {
        return;
    }
    for y in 0..to_height {
        let source = y * from_height / to_height * from_width;
        for x in 0..to_width {
            if let Some(pixel) = to.get_mut(y * to_width + x) {
                *pixel = from[source + x * from_width / to_width];
            }
        }
    }
}

/// `source` (straight alpha in its top byte) drawn over `destination`, whose top byte stays.
pub fn blend(source: u32, destination: u32) -> u32 {
    let alpha = source >> 24;
    match alpha {
        0 => destination,
        255 => (destination & ALPHA_BYTE) | (source & 0x00ff_ffff),
        _ => {
            let mix = |shift: u32| {
                let (top, under) = ((source >> shift) & 0xff, (destination >> shift) & 0xff);
                ((top * alpha + under * (255 - alpha) + 127) / 255) << shift
            };
            (destination & ALPHA_BYTE) | mix(16) | mix(8) | mix(0)
        }
    }
}

/// `source` over `background`, both straight alpha, as premultiplied alpha: the pixel GDI+
/// puts in the bitmap `GdipCreateHBITMAPFromBitmap` makes.
pub fn premultiplied_over(source: u32, background: u32) -> u32 {
    let source_alpha = source >> 24;
    let weight = (background >> 24) * (255 - source_alpha);
    let alpha = source_alpha + (weight + 127) / 255;
    let mix = |shift: u32| {
        let (top, under) = ((source >> shift) & 0xff, (background >> shift) & 0xff);
        (top * source_alpha * 255 + under * weight + 32_512) / 65_025
    };
    alpha << 24 | mix(16) << 16 | mix(8) << 8 | mix(0)
}

/// A premultiplied pixel as straight alpha.
pub fn unpremultiplied(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    if alpha == 0 || alpha == 255 {
        return pixel;
    }
    let undo = |shift: u32| (((pixel >> shift) & 0xff) * 255 / alpha).min(255) << shift;
    alpha << 24 | undo(16) | undo(8) | undo(0)
}

/// Whether any pixel has a non-zero top byte: a 32-bit image whose alpha is all zero is
/// opaque, as Win32 draws it.
pub fn has_alpha(pixels: &[u32]) -> bool {
    pixels.iter().any(|pixel| pixel >> 24 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: i32, height: i32, bits: u16, compression: u32, used: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; INFO_HEADER_SIZE];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..12].copy_from_slice(&height.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&bits.to_le_bytes());
        bytes[16..20].copy_from_slice(&compression.to_le_bytes());
        bytes[32..36].copy_from_slice(&used.to_le_bytes());
        bytes
    }

    #[test]
    fn headers_give_the_rows_strides_and_tables() {
        let layout = Layout::parse(&header(3, 2, 24, BI_RGB, 0)).unwrap();
        assert_eq!(
            (layout.width, layout.height, layout.bottom_up),
            (3, 2, true)
        );
        assert_eq!(layout.stride(), 12);
        assert_eq!(layout.palette_end(), 40);
        let top_down = Layout::parse(&header(5, -4, 32, BI_RGB, 0)).unwrap();
        assert!(top_down.is_native());
        assert_eq!(top_down.source_row(1), 1);
        let mono = Layout::parse(&header(33, 1, 1, BI_RGB, 0)).unwrap();
        assert_eq!(
            (mono.stride(), mono.colours, mono.palette_end()),
            (8, 2, 48)
        );
        let eight = Layout::parse(&header(1, 1, 8, BI_RGB, 16)).unwrap();
        assert_eq!(eight.colours, 16);
        let mut fields = header(2, 2, 16, BI_BITFIELDS, 0);
        for mask in [0xf800u32, 0x07e0, 0x001f] {
            fields.extend_from_slice(&mask.to_le_bytes());
        }
        let fields_layout = Layout::parse(&fields).unwrap();
        assert_eq!(fields_layout.masks, [0xf800, 0x07e0, 0x001f]);
        assert_eq!(fields_layout.palette_end(), 52);
        assert_eq!(Layout::info_size(&fields), Some(52));
        assert_eq!(Layout::info_size(&header(1, 1, 4, BI_RGB, 0)), Some(104));
        assert_eq!(Layout::parse(&header(0, 1, 24, BI_RGB, 0)), None);
        assert_eq!(Layout::parse(&header(1, 1, 24, 1, 0)), None);
        assert_eq!(Layout::parse(&header(1, 1, 7, BI_RGB, 0)), None);
    }

    #[test]
    fn rows_of_every_depth_read_and_write_back() {
        let palette = [0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0xff, 0, 0, 0, 0, 0xff, 0];
        let four = Layout::parse(&header(3, 1, 4, BI_RGB, 4)).unwrap();
        let mut out = [0u32; 3];
        read_row(&four, &palette, &[0x12, 0x30, 0, 0], &mut out);
        assert_eq!(out, [0x0000_00ff, 0x0000_ff00, 0x00ff_0000]);
        let mut row = [0xffu8; 4];
        write_row(&four, &palette, &out, &mut row);
        assert_eq!(row[..2], [0x12, 0x30]);
        let sixteen = Layout::parse(&header(1, 1, 16, BI_RGB, 0)).unwrap();
        read_row(&sixteen, &[], &0x7c1fu16.to_le_bytes(), &mut out[..1]);
        assert_eq!(out[0], 0x00ff_00ff);
        let mut bytes = [0u8; 4];
        write_row(&sixteen, &[], &[0x00ff_00ff], &mut bytes);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x7c1f);
        let twenty_four = Layout::parse(&header(1, 1, 24, BI_RGB, 0)).unwrap();
        read_row(&twenty_four, &[], &[0x33, 0x22, 0x11, 0], &mut out[..1]);
        assert_eq!(out[0], 0x0011_2233);
        write_row(&twenty_four, &[], &[0x0044_5566], &mut bytes);
        assert_eq!(bytes[..3], [0x66, 0x55, 0x44]);
        let native = Layout::native(1, 1);
        read_row(&native, &[], &0x8012_3456u32.to_le_bytes(), &mut out[..1]);
        assert_eq!(out[0], 0x8012_3456);
    }

    #[test]
    fn bottom_up_rows_come_out_top_first_and_encode_back() {
        let layout = Layout::parse(&header(1, 2, 32, BI_RGB, 0)).unwrap();
        let mut bits = Vec::new();
        bits.extend_from_slice(&0x0000_0001u32.to_le_bytes());
        bits.extend_from_slice(&0x0000_0002u32.to_le_bytes());
        let mut pixels = [0u32; 2];
        assert!(decode(&layout, &[], &bits, &mut pixels));
        assert_eq!(pixels, [2, 1]);
        pixels[0] = 7;
        encode(&layout, &[], &pixels, &mut bits, 0..1);
        assert_eq!(bits[4..8], 7u32.to_le_bytes());
        assert!(!decode(&layout, &[], &bits[..4], &mut pixels));
    }

    #[test]
    fn a_bmp_file_and_an_icon_dib_are_found() {
        let mut file = b"BM".to_vec();
        file.extend_from_slice(&[0; 8]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&header(1, 1, 24, BI_RGB, 0));
        file.extend_from_slice(&[0x10, 0x20, 0x30, 0]);
        let bmp = bmp_file(&file).unwrap();
        let mut pixel = [0u32; 1];
        assert!(bmp.decode(&mut pixel));
        assert_eq!(pixel[0], 0x0030_2010);
        assert!(bmp_file(&file[..56]).is_none());
        let mut icon = header(2, 4, 32, BI_RGB, 0);
        for value in [0xff00_0001u32, 0xff00_0002, 0x0000_0003, 0x0000_0004] {
            icon.extend_from_slice(&value.to_le_bytes());
        }
        icon.extend_from_slice(&[0x40, 0, 0, 0, 0x80, 0, 0, 0]);
        let image = icon_dib(&icon).unwrap();
        let mut colour = [0u32; 4];
        assert!(image.colour.decode(&mut colour));
        assert_eq!(colour, [3, 4, 0xff00_0001, 0xff00_0002]);
        let mut mask = [0u32; 4];
        image.decode_mask(&mut mask);
        assert_eq!(mask, [MASKED, 0, 0, MASKED]);
    }

    #[test]
    fn scaling_and_alpha_arithmetic() {
        let mut to = [0u32; 4];
        scale_nearest(&[1, 2, 3, 4], (2, 2), &mut to, (4, 1));
        assert_eq!(to, [1, 1, 2, 2]);
        assert_eq!(blend(0xff12_3456, 0x0000_0000), 0x0012_3456);
        assert_eq!(blend(0x0012_3456, 0x0065_4321), 0x0065_4321);
        assert_eq!(blend(0x80ff_ffff, 0x0000_0000), 0x0080_8080);
        assert_eq!(premultiplied_over(0xff11_2233, 0), 0xff11_2233);
        assert_eq!(premultiplied_over(0x80ff_ffff, 0), 0x8080_8080);
        assert_eq!(premultiplied_over(0x0000_0000, 0xff00_00ff), 0xff00_00ff);
        assert_eq!(unpremultiplied(0x8080_8080), 0x80ff_ffff);
        assert!(has_alpha(&[0, 0x0100_0000]));
        assert!(!has_alpha(&[0x00ff_ffff]));
        assert_eq!(png_size(&[0; 4]), None);
    }
}
