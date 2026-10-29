use crate::{
    Adler32, DISTANCE_BASES, DISTANCE_EXTRA_BITS, LENGTH_BASES, LENGTH_EXTRA_BITS, SIGNATURE,
    crc32_update,
};

/// Why a PNG could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// It does not start with the PNG signature and a header.
    NotPng,
    /// A chunk's CRC does not match its contents.
    BadChunk,
    /// Not an 8-bit RGB image without interlacing, or a compressed block with Huffman codes of
    /// its own: what [`crate::Encoder`] never writes.
    Unsupported,
    /// The compressed rows or their filters make no sense, or their checksum does not match.
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
}

struct Bits<'a> {
    bytes: IdatBytes<'a>,
    buffer: u32,
    count: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32, DecodeError> {
        if self.count == 0 {
            self.buffer = u32::from(self.bytes.next()?);
            self.count = 8;
        }
        let bit = self.buffer & 1;
        self.buffer >>= 1;
        self.count -= 1;
        Ok(bit)
    }

    fn bits(&mut self, count: u32) -> Result<u32, DecodeError> {
        let mut value = 0;
        for shift in 0..count {
            value |= self.bit()? << shift;
        }
        Ok(value)
    }

    fn huffman(&mut self, length: u32) -> Result<u32, DecodeError> {
        let mut code = 0;
        for _ in 0..length {
            code = code << 1 | self.bit()?;
        }
        Ok(code)
    }

    fn align(&mut self) {
        self.count = 0;
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        self.bytes.next()
    }

    fn literal_or_length(&mut self) -> Result<u32, DecodeError> {
        let code = self.huffman(7)?;
        if code <= 0x17 {
            return Ok(code + 256);
        }
        let code = code << 1 | self.bit()?;
        Ok(match code {
            0x30..=0xbf => code - 0x30,
            0xc0..=0xc7 => code - 0xc0 + 280,
            _ => (code << 1 | self.bit()?) - 0x190 + 144,
        })
    }
}

/// Decodes `png`, an 8-bit RGB PNG with stored or fixed-Huffman blocks as [`crate::Encoder`]
/// writes, every chunk's CRC and the zlib checksum checked: its rows go to `pixels` as
/// `0x00RRGGBB`, packed, inflated through `inflated` ([`decoded_size`] bytes). Returns its
/// width and height.
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
    inflate(png, header.next, inflated)?;
    unfilter(inflated, width as usize)?;
    let stride = width as usize * 3 + 1;
    for (row, line) in inflated.chunks_exact(stride).enumerate() {
        for (column, rgb) in line[1..].chunks_exact(3).enumerate() {
            pixels[row * width as usize + column] =
                u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        }
    }
    Ok((width, height))
}

fn inflate(png: &[u8], first_chunk: usize, out: &mut [u8]) -> Result<(), DecodeError> {
    let mut bits = Bits {
        bytes: IdatBytes {
            png,
            data: &[],
            next_chunk: first_chunk,
        },
        buffer: 0,
        count: 0,
    };
    let method = bits.byte()?;
    let flags = bits.byte()?;
    if method & 0x0f != 8 || (u16::from(method) << 8 | u16::from(flags)) % 31 != 0 {
        return Err(DecodeError::Corrupt);
    }
    if flags & 0x20 != 0 {
        return Err(DecodeError::Unsupported);
    }
    let mut length = 0;
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            0 => {
                bits.align();
                let low = [bits.byte()?, bits.byte()?, bits.byte()?, bits.byte()?];
                let stored = u16::from_le_bytes([low[0], low[1]]);
                if stored != !u16::from_le_bytes([low[2], low[3]]) {
                    return Err(DecodeError::Corrupt);
                }
                for _ in 0..stored {
                    let byte = bits.byte()?;
                    *out.get_mut(length).ok_or(DecodeError::Corrupt)? = byte;
                    length += 1;
                }
            }
            1 => length = inflate_fixed(&mut bits, out, length)?,
            _ => return Err(DecodeError::Unsupported),
        }
        if last {
            break;
        }
    }
    bits.align();
    let checksum = u32::from_be_bytes([bits.byte()?, bits.byte()?, bits.byte()?, bits.byte()?]);
    let mut adler = Adler32::new();
    adler.update(&out[..length]);
    if length != out.len() || adler.value() != checksum {
        return Err(DecodeError::Corrupt);
    }
    Ok(())
}

fn inflate_fixed(bits: &mut Bits<'_>, out: &mut [u8], mut length: usize) -> Result<usize, DecodeError> {
    loop {
        let symbol = bits.literal_or_length()?;
        match symbol {
            0..=255 => {
                *out.get_mut(length).ok_or(DecodeError::Corrupt)? = symbol as u8;
                length += 1;
            }
            256 => return Ok(length),
            257..=285 => {
                let code = (symbol - 257) as usize;
                let run = usize::from(LENGTH_BASES[code])
                    + bits.bits(u32::from(LENGTH_EXTRA_BITS[code]))? as usize;
                let code = bits.huffman(5)? as usize;
                let distance = usize::from(*DISTANCE_BASES.get(code).ok_or(DecodeError::Corrupt)?)
                    + bits.bits(u32::from(DISTANCE_EXTRA_BITS[code]))? as usize;
                let from = length.checked_sub(distance).ok_or(DecodeError::Corrupt)?;
                if length + run > out.len() {
                    return Err(DecodeError::Corrupt);
                }
                for offset in 0..run {
                    out[length + offset] = out[from + offset];
                }
                length += run;
            }
            _ => return Err(DecodeError::Corrupt),
        }
    }
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
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
