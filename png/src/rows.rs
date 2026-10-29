//! Decoding any PNG a row at a time (D148): greyscale at 1, 2, 4, 8 and 16 bits, RGB at 8 and
//! 16, palettes at 1, 2, 4 and 8, greyscale with alpha and RGBA at 8 and 16, with `PLTE` and
//! `tRNS`, the five filters and any DEFLATE block type. Each row reaches the caller as
//! straight-alpha `0xAARRGGBB` (a 16-bit sample keeps its high byte, a smaller one is scaled to
//! 0..=255). The file is read once, front to back, from a [`ByteSource`], every chunk's CRC
//! and the zlib checksum checked, and the only memory is a [`RowWorkspace`], so an image of any
//! height costs the same. Interlaced (Adam7) images and images wider than [`MAX_WIDTH`] are
//! [`DecodeError::Unsupported`].

use crate::decode::{DecodeError, inflate_error, paeth, zlib_header};
use crate::inflate::{self, ByteSource, Sink, Window};
use crate::{Adler32, SIGNATURE, crc32_update};

/// The widest image [`RowDecoder`] decodes, in pixels.
pub const MAX_WIDTH: u32 = 4096;

const MAX_ROW_BYTES: usize = MAX_WIDTH as usize * 8 + 1;
const HEADER_CHUNK_BYTES: u32 = 13;
const MAX_PALETTE: usize = 256;
const OPAQUE: u32 = 0xff00_0000;

/// What the header says about an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Its width in pixels, 1 to [`MAX_WIDTH`].
    pub width: u32,
    /// Its height in pixels, at least 1.
    pub height: u32,
    /// Bits per sample: 1, 2, 4, 8 or 16.
    pub bit_depth: u8,
    /// PNG's colour type: 0 greyscale, 2 RGB, 3 palette, 4 greyscale and alpha, 6 RGBA.
    pub colour_type: u8,
}

impl Header {
    fn channels(&self) -> u32 {
        match self.colour_type {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }

    fn bits_per_pixel(&self) -> u32 {
        self.channels() * u32::from(self.bit_depth)
    }

    fn row_bytes(&self) -> usize {
        (self.width as usize * self.bits_per_pixel() as usize).div_ceil(8) + 1
    }

    fn is_valid(&self) -> bool {
        matches!(
            (self.colour_type, self.bit_depth),
            (0, 1 | 2 | 4 | 8 | 16) | (2 | 4 | 6, 8 | 16) | (3, 1 | 2 | 4 | 8)
        )
    }
}

/// The memory [`RowDecoder::read_rows`] works in: the inflater's window, two rows of filtered
/// bytes and one row of pixels. All zero is its starting state.
pub struct RowWorkspace {
    window: Window,
    previous: [u8; MAX_ROW_BYTES],
    current: [u8; MAX_ROW_BYTES],
    pixels: [u32; MAX_WIDTH as usize],
}

impl RowWorkspace {
    /// All zero.
    pub const fn new() -> Self {
        Self {
            window: Window::new(),
            previous: [0; MAX_ROW_BYTES],
            current: [0; MAX_ROW_BYTES],
            pixels: [0; MAX_WIDTH as usize],
        }
    }
}

impl Default for RowWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for RowWorkspace {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RowWorkspace")
            .finish_non_exhaustive()
    }
}

struct Chunks<'s, S: ByteSource> {
    source: &'s mut S,
    remaining: u32,
    crc: u32,
    idat_over: bool,
    error: Option<DecodeError>,
}

impl<S: ByteSource> Chunks<'_, S> {
    fn raw(&mut self) -> Result<u8, DecodeError> {
        self.source.next_byte().ok_or(DecodeError::Truncated)
    }

    fn word(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes([
            self.raw()?,
            self.raw()?,
            self.raw()?,
            self.raw()?,
        ]))
    }

    fn start(&mut self) -> Result<[u8; 4], DecodeError> {
        let length = self.word()?;
        let kind = [self.raw()?, self.raw()?, self.raw()?, self.raw()?];
        self.crc = crc32_update(0, &kind);
        self.remaining = length;
        Ok(kind)
    }

    fn data(&mut self) -> Result<u8, DecodeError> {
        let byte = self.raw()?;
        self.crc = crc32_update(self.crc, &[byte]);
        self.remaining -= 1;
        Ok(byte)
    }

    fn end(&mut self) -> Result<(), DecodeError> {
        while self.remaining > 0 {
            self.data()?;
        }
        match self.word()? == self.crc {
            true => Ok(()),
            false => Err(DecodeError::BadChunk),
        }
    }

    fn idat(&mut self) -> Result<u8, DecodeError> {
        while self.remaining == 0 {
            if self.idat_over {
                return Err(DecodeError::Truncated);
            }
            self.end()?;
            if &self.start()? != b"IDAT" {
                self.idat_over = true;
                return Err(DecodeError::Truncated);
            }
        }
        self.data()
    }

    fn take(&mut self) -> Result<u8, DecodeError> {
        self.next_byte()
            .ok_or_else(|| self.error.unwrap_or(DecodeError::Truncated))
    }
}

impl<S: ByteSource> ByteSource for Chunks<'_, S> {
    fn next_byte(&mut self) -> Option<u8> {
        match self.idat() {
            Ok(byte) => Some(byte),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }
}

/// A PNG whose header and the chunks before its pixels are read: [`Self::open`], then
/// [`Self::header`] to size what the rows go to, then [`Self::read_rows`].
pub struct RowDecoder<'s, S: ByteSource> {
    chunks: Chunks<'s, S>,
    header: Header,
    palette: [u32; MAX_PALETTE],
    palette_length: usize,
    transparent: Option<[u16; 3]>,
}

impl<S: ByteSource> core::fmt::Debug for RowDecoder<'_, S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RowDecoder")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl<'s, S: ByteSource> RowDecoder<'s, S> {
    /// Reads the signature, the header and every chunk up to the first `IDAT` from `source`,
    /// keeping the palette and the transparency.
    pub fn open(source: &'s mut S) -> Result<Self, DecodeError> {
        let mut chunks = Chunks {
            source,
            remaining: 0,
            crc: 0,
            idat_over: false,
            error: None,
        };
        for &expected in &SIGNATURE {
            if chunks.raw().ok() != Some(expected) {
                return Err(DecodeError::NotPng);
            }
        }
        if chunks.start().ok() != Some(*b"IHDR") || chunks.remaining != HEADER_CHUNK_BYTES {
            return Err(DecodeError::NotPng);
        }
        let mut fields = [0u8; HEADER_CHUNK_BYTES as usize];
        for field in &mut fields {
            *field = chunks.data()?;
        }
        chunks.end()?;
        let header = Header {
            width: u32::from_be_bytes([fields[0], fields[1], fields[2], fields[3]]),
            height: u32::from_be_bytes([fields[4], fields[5], fields[6], fields[7]]),
            bit_depth: fields[8],
            colour_type: fields[9],
        };
        let plain = fields[10..] == [0, 0, 0];
        if header.width == 0 || header.height == 0 || !header.is_valid() {
            return Err(DecodeError::Corrupt);
        }
        if !plain || header.width > MAX_WIDTH {
            return Err(DecodeError::Unsupported);
        }
        let mut decoder = Self {
            chunks,
            header,
            palette: [0; MAX_PALETTE],
            palette_length: 0,
            transparent: None,
        };
        loop {
            match &decoder.chunks.start()? {
                b"IDAT" => break,
                b"PLTE" => decoder.read_palette()?,
                b"tRNS" => decoder.read_transparency()?,
                b"IEND" => return Err(DecodeError::Truncated),
                _ => decoder.chunks.end()?,
            }
        }
        if header.colour_type == 3 && decoder.palette_length == 0 {
            return Err(DecodeError::Corrupt);
        }
        Ok(decoder)
    }

    /// The image's header.
    pub fn header(&self) -> Header {
        self.header
    }

    fn read_palette(&mut self) -> Result<(), DecodeError> {
        let length = self.chunks.remaining as usize;
        if !length.is_multiple_of(3) || length > 3 * MAX_PALETTE {
            return Err(DecodeError::Corrupt);
        }
        for entry in 0..length / 3 {
            let rgb = [
                self.chunks.data()?,
                self.chunks.data()?,
                self.chunks.data()?,
            ];
            self.palette[entry] =
                OPAQUE | u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        }
        self.palette_length = length / 3;
        self.chunks.end()
    }

    fn read_transparency(&mut self) -> Result<(), DecodeError> {
        let length = self.chunks.remaining as usize;
        match self.header.colour_type {
            3 => {
                for entry in 0..length.min(MAX_PALETTE) {
                    let alpha = self.chunks.data()?;
                    if let Some(colour) = self.palette.get_mut(entry) {
                        *colour = u32::from(alpha) << 24 | (*colour & 0x00ff_ffff);
                    }
                }
            }
            0 if length == 2 => {
                let grey = u16::from_be_bytes([self.chunks.data()?, self.chunks.data()?]);
                self.transparent = Some([grey; 3]);
            }
            2 if length == 6 => {
                let mut samples = [0u16; 3];
                for sample in &mut samples {
                    *sample = u16::from_be_bytes([self.chunks.data()?, self.chunks.data()?]);
                }
                self.transparent = Some(samples);
            }
            _ => {}
        }
        self.chunks.end()
    }

    /// Inflates and unfilters the image, calling `each_row` with each row's index, top first,
    /// and its pixels as straight-alpha `0xAARRGGBB`.
    pub fn read_rows(
        mut self,
        work: &mut RowWorkspace,
        each_row: impl FnMut(u32, &[u32]),
    ) -> Result<(), DecodeError> {
        let header = self.header;
        let row_bytes = header.row_bytes();
        let RowWorkspace {
            window,
            previous,
            current,
            pixels,
        } = work;
        previous[..row_bytes].fill(0);
        let mut rows = Rows {
            header,
            palette: &self.palette[..self.palette_length],
            transparent: self.transparent,
            previous: &mut previous[..row_bytes],
            current: &mut current[..row_bytes],
            pixels: &mut pixels[..header.width as usize],
            filled: 0,
            row: 0,
            adler: Adler32::new(),
            failed: None,
            each_row,
        };
        let method = self.chunks.take()?;
        let flags = self.chunks.take()?;
        zlib_header(method, flags)?;
        let inflated = inflate::inflate(&mut self.chunks, window, &mut rows);
        if let Some(error) = self.chunks.error {
            return Err(error);
        }
        if let Err(error) = inflated {
            return Err(rows.failed.unwrap_or(inflate_error(error)));
        }
        if rows.row != header.height {
            return Err(DecodeError::Truncated);
        }
        let checksum = u32::from_be_bytes([
            self.chunks.take()?,
            self.chunks.take()?,
            self.chunks.take()?,
            self.chunks.take()?,
        ]);
        if checksum != rows.adler.value() {
            return Err(DecodeError::Corrupt);
        }
        self.chunks.end()
    }
}

struct Rows<'a, F: FnMut(u32, &[u32])> {
    header: Header,
    palette: &'a [u32],
    transparent: Option<[u16; 3]>,
    previous: &'a mut [u8],
    current: &'a mut [u8],
    pixels: &'a mut [u32],
    filled: usize,
    row: u32,
    adler: Adler32,
    failed: Option<DecodeError>,
    each_row: F,
}

impl<F: FnMut(u32, &[u32])> Sink for Rows<'_, F> {
    fn write(&mut self, mut bytes: &[u8]) -> bool {
        self.adler.update(bytes);
        while !bytes.is_empty() {
            if self.row == self.header.height {
                self.failed = Some(DecodeError::Corrupt);
                return false;
            }
            let take = (self.current.len() - self.filled).min(bytes.len());
            self.current[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == self.current.len()
                && let Err(error) = self.finish_row()
            {
                self.failed = Some(error);
                return false;
            }
        }
        true
    }
}

impl<F: FnMut(u32, &[u32])> Rows<'_, F> {
    fn finish_row(&mut self) -> Result<(), DecodeError> {
        let step = (self.header.bits_per_pixel() as usize / 8).max(1);
        unfilter_row(self.current, self.previous, step)?;
        self.convert()?;
        (self.each_row)(self.row, self.pixels);
        core::mem::swap(&mut self.previous, &mut self.current);
        self.filled = 0;
        self.row += 1;
        Ok(())
    }

    fn convert(&mut self) -> Result<(), DecodeError> {
        let line = &self.current[1..];
        let depth = self.header.bit_depth;
        let channels = self.header.channels() as usize;
        for (x, pixel) in self.pixels.iter_mut().enumerate() {
            let at = x * channels;
            let value = |channel: usize| sample(line, at + channel, depth);
            *pixel = match self.header.colour_type {
                0 => {
                    let grey = value(0);
                    let alpha = match self.transparent == Some([grey; 3]) {
                        true => 0,
                        false => 255,
                    };
                    argb(alpha, [to_byte(grey, depth); 3])
                }
                2 => {
                    let rgb = [value(0), value(1), value(2)];
                    let alpha = match self.transparent == Some(rgb) {
                        true => 0,
                        false => 255,
                    };
                    argb(alpha, rgb.map(|channel| to_byte(channel, depth)))
                }
                3 => *self
                    .palette
                    .get(usize::from(value(0)))
                    .ok_or(DecodeError::Corrupt)?,
                4 => argb(to_byte(value(1), depth), [to_byte(value(0), depth); 3]),
                _ => argb(
                    to_byte(value(3), depth),
                    [value(0), value(1), value(2)].map(|channel| to_byte(channel, depth)),
                ),
            };
        }
        Ok(())
    }
}

fn argb(alpha: u8, [red, green, blue]: [u8; 3]) -> u32 {
    u32::from(alpha) << 24 | u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}

fn sample(line: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => u16::from_be_bytes([line[2 * index], line[2 * index + 1]]),
        8 => u16::from(line[index]),
        _ => {
            let per_byte = 8 / usize::from(depth);
            let shift = 8 - usize::from(depth) * (index % per_byte + 1);
            u16::from(line[index / per_byte] >> shift) & ((1 << depth) - 1)
        }
    }
}

fn to_byte(value: u16, depth: u8) -> u8 {
    match depth {
        16 => (value >> 8) as u8,
        8 => value as u8,
        _ => (u32::from(value) * 255 / ((1u32 << depth) - 1)) as u8,
    }
}

/// Undoes one row's filter in place: `line` and `above` (the row before, unfiltered, all zero
/// for the first) start with their filter byte; `step` is the bytes per pixel, at least 1.
pub(crate) fn unfilter_row(line: &mut [u8], above: &[u8], step: usize) -> Result<(), DecodeError> {
    let (filter, line) = line.split_first_mut().ok_or(DecodeError::Corrupt)?;
    let above = above.get(1..).ok_or(DecodeError::Corrupt)?;
    for at in 0..line.len() {
        let left = if at >= step { line[at - step] } else { 0 };
        let up = above[at];
        let up_left = if at >= step { above[at - step] } else { 0 };
        let predicted = match *filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth(left, up, up_left),
            _ => return Err(DecodeError::Corrupt),
        };
        line[at] = line[at].wrapping_add(predicted);
    }
    Ok(())
}
