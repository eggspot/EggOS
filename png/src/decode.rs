use crate::inflate::{self, ByteSource, InflateError};
use crate::{Adler32, SIGNATURE, crc32_update};

/// Why a PNG could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// It does not start with the PNG signature and a header.
    NotPng,
    /// A chunk's CRC does not match its contents.
    BadChunk,
    /// A colour type, bit depth, interlacing or width this decoder does not take: [`decode`]
    /// takes 8-bit RGB without interlacing, [`crate::rows`] every non-interlaced image up to
    /// [`crate::rows::MAX_WIDTH`] pixels wide.
    Unsupported,
    /// The compressed rows, their filters or a palette index make no sense, or their checksum
    /// does not match.
    Corrupt,
    /// It ends before its last chunk or before the end of its compressed rows.
    Truncated,
    /// `inflated` or `pixels` is too small for the image.
    BufferTooSmall,
}

const HEADER_CHUNK_BYTES: usize = 13;
const CHUNK_OVERHEAD: usize = 12;

/// How many bytes of filtered rows a `width` by `height` 8-bit RGB image inflates to: what
/// [`decode`]'s `inflated` needs.
pub fn decoded_size(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(3)?
        .checked_add(1)?
        .checked_mul(height as usize)
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let bytes = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

struct Chunk<'a> {
    kind: &'a [u8],
    data: &'a [u8],
    next: usize,
}

fn chunk_at(png: &[u8], at: usize) -> Result<Chunk<'_>, DecodeError> {
    let length = word(png, at).ok_or(DecodeError::Truncated)? as usize;
    let end = at
        .checked_add(CHUNK_OVERHEAD)
        .and_then(|end| end.checked_add(length))
        .filter(|&end| end <= png.len())
        .ok_or(DecodeError::Truncated)?;
    let checked = &png[at + 4..end - 4];
    if word(png, end - 4) != Some(crc32_update(0, checked)) {
        return Err(DecodeError::BadChunk);
    }
    Ok(Chunk {
        kind: &checked[..4],
        data: &checked[4..],
        next: end,
    })
}

struct IdatBytes<'a> {
    png: &'a [u8],
    data: &'a [u8],
    next_chunk: usize,
    error: Option<DecodeError>,
}

impl IdatBytes<'_> {
    fn next(&mut self) -> Result<u8, DecodeError> {
        while self.data.is_empty() {
            let chunk = chunk_at(self.png, self.next_chunk)?;
            self.next_chunk = chunk.next;
            match chunk.kind {
                b"IDAT" => self.data = chunk.data,
                b"IEND" => return Err(DecodeError::Truncated),
                _ => {}
            }
        }
        let byte = self.data[0];
        self.data = &self.data[1..];
        Ok(byte)
    }

    fn take(&mut self) -> Result<u8, DecodeError> {
        self.next_byte()
            .ok_or_else(|| self.error.unwrap_or(DecodeError::Truncated))
    }
}

impl ByteSource for IdatBytes<'_> {
    fn next_byte(&mut self) -> Option<u8> {
        match self.next() {
            Ok(byte) => Some(byte),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }
}

pub(crate) fn zlib_header(method: u8, flags: u8) -> Result<(), DecodeError> {
    if method & 0x0f != 8 || (u16::from(method) << 8 | u16::from(flags)) % 31 != 0 {
        return Err(DecodeError::Corrupt);
    }
    if flags & 0x20 != 0 {
        return Err(DecodeError::Unsupported);
    }
    Ok(())
}

pub(crate) fn inflate_error(error: InflateError) -> DecodeError {
    match error {
        InflateError::Truncated => DecodeError::Truncated,
        InflateError::Corrupt | InflateError::TooLong | InflateError::Stopped => {
            DecodeError::Corrupt
        }
    }
}

/// Decodes `png`, an 8-bit RGB PNG without interlacing (what [`crate::Encoder`] writes, its
/// rows compressed by any DEFLATE block type), every chunk's CRC and the zlib checksum
/// checked: its rows go to `pixels` as `0x00RRGGBB`, packed, inflated through `inflated`
/// ([`decoded_size`] bytes). Returns its width and height. [`crate::rows`] decodes every
/// other kind of PNG.
pub fn decode(
    png: &[u8],
    inflated: &mut [u8],
    pixels: &mut [u32],
) -> Result<(u32, u32), DecodeError> {
    if png.get(..SIGNATURE.len()) != Some(&SIGNATURE[..]) {
        return Err(DecodeError::NotPng);
    }
    let header = chunk_at(png, SIGNATURE.len())?;
    if header.kind != b"IHDR" || header.data.len() != HEADER_CHUNK_BYTES {
        return Err(DecodeError::NotPng);
    }
    let width = word(header.data, 0).ok_or(DecodeError::NotPng)?;
    let height = word(header.data, 4).ok_or(DecodeError::NotPng)?;
    if width == 0 || height == 0 || header.data[8..] != [8, 2, 0, 0, 0] {
        return Err(DecodeError::Unsupported);
    }
    let mut at = header.next;
    loop {
        let chunk = chunk_at(png, at)?;
        at = chunk.next;
        if chunk.kind == b"IEND" {
            break;
        }
    }
    let size = decoded_size(width, height).ok_or(DecodeError::BufferTooSmall)?;
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(DecodeError::BufferTooSmall)?;
    if inflated.len() < size || pixels.len() < count {
        return Err(DecodeError::BufferTooSmall);
    }
    let inflated = &mut inflated[..size];
    inflate_rows(png, header.next, inflated)?;
    unfilter(inflated, width as usize)?;
    let stride = width as usize * 3 + 1;
    for (row, line) in inflated.chunks_exact(stride).enumerate() {
        for (column, rgb) in line[1..].as_chunks::<3>().0.iter().enumerate() {
            pixels[row * width as usize + column] =
                u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        }
    }
    Ok((width, height))
}

fn inflate_rows(png: &[u8], first_chunk: usize, out: &mut [u8]) -> Result<(), DecodeError> {
    let mut idat = IdatBytes {
        png,
        data: &[],
        next_chunk: first_chunk,
        error: None,
    };
    let method = idat.take()?;
    let flags = idat.take()?;
    zlib_header(method, flags)?;
    let inflated = inflate::inflate_into(&mut idat, out);
    if let Some(error) = idat.error {
        return Err(error);
    }
    let length = inflated.map_err(inflate_error)?;
    let checksum = u32::from_be_bytes([idat.take()?, idat.take()?, idat.take()?, idat.take()?]);
    let mut adler = Adler32::new();
    adler.update(&out[..length]);
    if length != out.len() || adler.value() != checksum {
        return Err(DecodeError::Corrupt);
    }
    Ok(())
}

pub(crate) fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(up_left);
    let to_left = (estimate - i16::from(left)).abs();
    let to_up = (estimate - i16::from(up)).abs();
    let to_up_left = (estimate - i16::from(up_left)).abs();
    if to_left <= to_up && to_left <= to_up_left {
        left
    } else if to_up <= to_up_left {
        up
    } else {
        up_left
    }
}

fn unfilter(rows: &mut [u8], width: usize) -> Result<(), DecodeError> {
    let stride = width * 3 + 1;
    for row in 0..rows.len() / stride {
        let (done, rest) = rows.split_at_mut(row * stride);
        let above = match row {
            0 => None,
            _ => Some(&done[done.len() - stride + 1..]),
        };
        let (filter, line) = rest[..stride].split_at_mut(1);
        for at in 0..line.len() {
            let left = if at >= 3 { line[at - 3] } else { 0 };
            let up = above.map_or(0, |above| above[at]);
            let up_left = match (above, at >= 3) {
                (Some(above), true) => above[at - 3],
                _ => 0,
            };
            let predicted = match filter[0] {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                _ => return Err(DecodeError::Corrupt),
            };
            line[at] = line[at].wrapping_add(predicted);
        }
    }
    Ok(())
}
