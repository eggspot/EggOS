//! DEFLATE decompression (RFC 1951), our own (D148): stored, fixed-Huffman and
//! dynamic-Huffman blocks, read a byte at a time from a [`ByteSource`] and written either into
//! one slice that holds the whole output ([`inflate_into`]) or through a 32 KiB [`Window`] to a
//! [`Sink`] in pieces ([`inflate`]), so a stream of any length needs no more memory than the
//! window. Codes are decoded bit by bit through their canonical counts, so a code table is 608
//! bytes and nothing allocates. Malformed input is an error, never a panic. The source is left
//! just after the last byte that holds bits of the stream, so a container's trailer (zlib's
//! checksum) follows at once.

use crate::{DISTANCE_BASES, DISTANCE_EXTRA_BITS, LENGTH_BASES, LENGTH_EXTRA_BITS, WINDOW_BYTES};

const MAX_BITS: usize = 15;
const LITERAL_SYMBOLS: usize = 288;
const DISTANCE_SYMBOLS: usize = 32;
const CODE_LENGTH_SYMBOLS: usize = 19;
const MAX_LITERAL_LENGTHS: usize = 286;
const MAX_DISTANCE_LENGTHS: usize = 30;
const END_OF_BLOCK: u16 = 256;
const WINDOW_MASK: usize = WINDOW_BYTES - 1;
const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_SYMBOLS] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Where compressed bytes come from, one at a time.
pub trait ByteSource {
    /// The next byte; `None` at the end, or when the source failed (it keeps its own error).
    fn next_byte(&mut self) -> Option<u8>;
}

/// Bytes in memory, from the first.
#[derive(Debug, Clone)]
pub struct SliceSource<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> SliceSource<'a> {
    /// A source of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// How many bytes were taken.
    pub fn consumed(&self) -> usize {
        self.at
    }
}

impl ByteSource for SliceSource<'_> {
    fn next_byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }
}

/// Where [`inflate`] writes what it decompressed, in order, in pieces of at most 32 KiB.
pub trait Sink {
    /// Takes the next `bytes`; `false` stops the inflate with [`InflateError::Stopped`].
    fn write(&mut self, bytes: &[u8]) -> bool;
}

/// Why inflating failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The source ended before the last block did.
    Truncated,
    /// A block type, a code, a length or a distance that DEFLATE does not allow.
    Corrupt,
    /// The output passed the end of [`inflate_into`]'s slice.
    TooLong,
    /// The sink refused a write.
    Stopped,
}

/// The last 32 KiB written, which matches copy from. All zero is its starting state.
pub struct Window {
    bytes: [u8; WINDOW_BYTES],
}

impl Window {
    /// All zero.
    pub const fn new() -> Self {
        Self {
            bytes: [0; WINDOW_BYTES],
        }
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Window {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Window").finish_non_exhaustive()
    }
}

/// Inflates the raw DEFLATE stream `source` holds into `out`; returns how many bytes it wrote.
pub fn inflate_into<S: ByteSource>(source: &mut S, out: &mut [u8]) -> Result<usize, InflateError> {
    let mut output = SliceOutput { out, length: 0 };
    run(source, &mut output)?;
    Ok(output.length)
}

/// Inflates the raw DEFLATE stream `source` holds through `window` to `sink`; returns how many
/// bytes it wrote.
pub fn inflate<S: ByteSource, K: Sink>(
    source: &mut S,
    window: &mut Window,
    sink: &mut K,
) -> Result<u64, InflateError> {
    let mut output = WindowOutput {
        window,
        sink,
        position: 0,
        flushed: 0,
    };
    run(source, &mut output)?;
    Ok(output.position)
}

trait Output {
    fn literal(&mut self, byte: u8) -> Result<(), InflateError>;
    fn copy(&mut self, distance: usize, length: usize) -> Result<(), InflateError>;
    fn finish(&mut self) -> Result<(), InflateError>;
}

struct SliceOutput<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Output for SliceOutput<'_> {
    fn literal(&mut self, byte: u8) -> Result<(), InflateError> {
        *self.out.get_mut(self.length).ok_or(InflateError::TooLong)? = byte;
        self.length += 1;
        Ok(())
    }

    fn copy(&mut self, distance: usize, length: usize) -> Result<(), InflateError> {
        let from = self
            .length
            .checked_sub(distance)
            .ok_or(InflateError::Corrupt)?;
        let end = self.length + length;
        if end > self.out.len() {
            return Err(InflateError::TooLong);
        }
        for offset in 0..length {
            self.out[self.length + offset] = self.out[from + offset];
        }
        self.length = end;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), InflateError> {
        Ok(())
    }
}

struct WindowOutput<'a, K: Sink> {
    window: &'a mut Window,
    sink: &'a mut K,
    position: u64,
    flushed: u64,
}

impl<K: Sink> WindowOutput<'_, K> {
    fn push(&mut self, byte: u8) -> Result<(), InflateError> {
        self.window.bytes[self.position as usize & WINDOW_MASK] = byte;
        self.position += 1;
        if self.position - self.flushed == WINDOW_BYTES as u64 {
            if !self.sink.write(&self.window.bytes) {
                return Err(InflateError::Stopped);
            }
            self.flushed = self.position;
        }
        Ok(())
    }
}

impl<K: Sink> Output for WindowOutput<'_, K> {
    fn literal(&mut self, byte: u8) -> Result<(), InflateError> {
        self.push(byte)
    }

    fn copy(&mut self, distance: usize, length: usize) -> Result<(), InflateError> {
        if distance as u64 > self.position || distance > WINDOW_BYTES {
            return Err(InflateError::Corrupt);
        }
        for _ in 0..length {
            let from = (self.position as usize).wrapping_sub(distance) & WINDOW_MASK;
            let byte = self.window.bytes[from];
            self.push(byte)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), InflateError> {
        let pending = (self.position - self.flushed) as usize;
        if pending > 0 && !self.sink.write(&self.window.bytes[..pending]) {
            return Err(InflateError::Stopped);
        }
        self.flushed = self.position;
        Ok(())
    }
}

struct Bits<'s, S: ByteSource> {
    source: &'s mut S,
    buffer: u32,
    count: u32,
}

impl<S: ByteSource> Bits<'_, S> {
    fn bit(&mut self) -> Result<u32, InflateError> {
        if self.count == 0 {
            self.buffer = u32::from(self.source.next_byte().ok_or(InflateError::Truncated)?);
            self.count = 8;
        }
        let bit = self.buffer & 1;
        self.buffer >>= 1;
        self.count -= 1;
        Ok(bit)
    }

    fn bits(&mut self, count: u32) -> Result<u32, InflateError> {
        let mut value = 0;
        for shift in 0..count {
            value |= self.bit()? << shift;
        }
        Ok(value)
    }

    fn align(&mut self) {
        self.count = 0;
    }

    fn byte(&mut self) -> Result<u8, InflateError> {
        self.source.next_byte().ok_or(InflateError::Truncated)
    }
}

struct Huffman<const N: usize> {
    counts: [u16; MAX_BITS + 1],
    symbols: [u16; N],
}

impl<const N: usize> Huffman<N> {
    fn new(lengths: &[u8]) -> Result<Self, InflateError> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &length in lengths {
            *counts
                .get_mut(usize::from(length))
                .ok_or(InflateError::Corrupt)? += 1;
        }
        let mut left: i32 = 1;
        for &count in &counts[1..] {
            left = (left << 1) - i32::from(count);
            if left < 0 {
                return Err(InflateError::Corrupt);
            }
        }
        let mut offsets = [0u16; MAX_BITS + 1];
        for length in 1..MAX_BITS {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = [0u16; N];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let at = &mut offsets[usize::from(length)];
                *symbols
                    .get_mut(usize::from(*at))
                    .ok_or(InflateError::Corrupt)? = symbol as u16;
                *at += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode<S: ByteSource>(&self, bits: &mut Bits<'_, S>) -> Result<u16, InflateError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for &count in &self.counts[1..] {
            code |= bits.bit()? as i32;
            let count = i32::from(count);
            if code - count < first {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or(InflateError::Corrupt);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::Corrupt)
    }
}

type Tables = (Huffman<LITERAL_SYMBOLS>, Huffman<DISTANCE_SYMBOLS>);

fn fixed_tables() -> Result<Tables, InflateError> {
    let mut lengths = [8u8; LITERAL_SYMBOLS];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    Ok((
        Huffman::new(&lengths)?,
        Huffman::new(&[5u8; MAX_DISTANCE_LENGTHS])?,
    ))
}

fn dynamic_tables<S: ByteSource>(bits: &mut Bits<'_, S>) -> Result<Tables, InflateError> {
    let literal_count = bits.bits(5)? as usize + 257;
    let distance_count = bits.bits(5)? as usize + 1;
    let code_length_count = bits.bits(4)? as usize + 4;
    if literal_count > MAX_LITERAL_LENGTHS || distance_count > MAX_DISTANCE_LENGTHS {
        return Err(InflateError::Corrupt);
    }
    let mut code_lengths = [0u8; CODE_LENGTH_SYMBOLS];
    for &symbol in &CODE_LENGTH_ORDER[..code_length_count] {
        code_lengths[symbol] = bits.bits(3)? as u8;
    }
    let code_length_code = Huffman::<CODE_LENGTH_SYMBOLS>::new(&code_lengths)?;
    let mut lengths = [0u8; MAX_LITERAL_LENGTHS + MAX_DISTANCE_LENGTHS];
    let total = literal_count + distance_count;
    let mut at = 0;
    while at < total {
        let symbol = code_length_code.decode(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 if at > 0 => (lengths[at - 1], 3 + bits.bits(2)?),
            17 => (0, 3 + bits.bits(3)?),
            18 => (0, 11 + bits.bits(7)?),
            _ => return Err(InflateError::Corrupt),
        };
        let end = at + repeat as usize;
        if end > total {
            return Err(InflateError::Corrupt);
        }
        lengths[at..end].fill(value);
        at = end;
    }
    if lengths[usize::from(END_OF_BLOCK)] == 0 {
        return Err(InflateError::Corrupt);
    }
    Ok((
        Huffman::new(&lengths[..literal_count])?,
        Huffman::new(&lengths[literal_count..total])?,
    ))
}

fn stored<S: ByteSource, O: Output>(
    bits: &mut Bits<'_, S>,
    output: &mut O,
) -> Result<(), InflateError> {
    bits.align();
    let header = [bits.byte()?, bits.byte()?, bits.byte()?, bits.byte()?];
    let length = u16::from_le_bytes([header[0], header[1]]);
    if length != !u16::from_le_bytes([header[2], header[3]]) {
        return Err(InflateError::Corrupt);
    }
    for _ in 0..length {
        output.literal(bits.byte()?)?;
    }
    Ok(())
}

fn codes<S: ByteSource, O: Output>(
    bits: &mut Bits<'_, S>,
    output: &mut O,
    (literals, distances): &Tables,
) -> Result<(), InflateError> {
    loop {
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => output.literal(symbol as u8)?,
            END_OF_BLOCK => return Ok(()),
            257..=285 => {
                let code = usize::from(symbol - 257);
                let length = usize::from(LENGTH_BASES[code])
                    + bits.bits(u32::from(LENGTH_EXTRA_BITS[code]))? as usize;
                let code = usize::from(distances.decode(bits)?);
                let base = *DISTANCE_BASES.get(code).ok_or(InflateError::Corrupt)?;
                let distance = usize::from(base)
                    + bits.bits(u32::from(DISTANCE_EXTRA_BITS[code]))? as usize;
                output.copy(distance, length)?;
            }
            _ => return Err(InflateError::Corrupt),
        }
    }
}

fn run<S: ByteSource, O: Output>(source: &mut S, output: &mut O) -> Result<(), InflateError> {
    let mut bits = Bits {
        source,
        buffer: 0,
        count: 0,
    };
    loop {
        let last = bits.bit()? == 1;
        match bits.bits(2)? {
            0 => stored(&mut bits, output)?,
            1 => codes(&mut bits, output, &fixed_tables()?)?,
            2 => {
                let tables = dynamic_tables(&mut bits)?;
                codes(&mut bits, output, &tables)?;
            }
            _ => return Err(InflateError::Corrupt),
        }
        if last {
            return output.finish();
        }
    }
}
