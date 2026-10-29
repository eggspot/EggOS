//! PNG images without an allocator (D141): a streaming encoder ([`Encoder`]) that takes one row
//! of `0x00RRGGBB` pixels at a time and writes an 8-bit RGB PNG to a [`PngSink`] as it goes,
//! each row filtered with PNG's Sub filter and compressed by DEFLATE with LZ77 matches over a
//! 32 KiB window and the fixed Huffman codes (RFC 1951) in one zlib stream (RFC 1950), cut into
//! IDAT chunks of [`IDAT_BYTES`]; a box downscaler for its rows ([`scale`]); and a decoder for
//! the PNGs it writes ([`decode`]), for tests and the host's checks. The encoder's memory is a
//! caller-owned [`EncoderWorkspace`], valid all zero, so it costs nothing until used. Matching
//! probes one earlier position per byte, so encoding time grows linearly with the pixels.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod checksum;
mod decode;
pub mod scale;
#[cfg(test)]
mod tests;

pub use checksum::{Adler32, crc32, crc32_update};
pub use decode::{DecodeError, decode, decoded_size};

/// The eight bytes every PNG file starts with.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The most compressed bytes one IDAT chunk holds.
pub const IDAT_BYTES: usize = 16 * 1024;

/// How far back a match may reach (DEFLATE's window).
pub const WINDOW_BYTES: usize = 32 * 1024;

/// The most bytes of filtered rows one image holds, so every position fits the hash table.
pub const MAX_IMAGE_BYTES: u64 = 0xffff_0000;

const HISTORY_BYTES: usize = 2 * WINDOW_BYTES;
const HASH_BITS: u32 = 14;
const HASH_SLOTS: usize = 1 << HASH_BITS;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const END_OF_BLOCK: u32 = 256;
const BYTES_PER_PIXEL: u64 = 3;
const SUB_FILTER: u8 = 1;

pub(crate) const LENGTH_BASES: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(crate) const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(crate) const DISTANCE_BASES: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(crate) const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Where an [`Encoder`] writes the file's bytes, in order.
pub trait PngSink {
    /// Why a write failed.
    type Error;

    /// Writes `bytes` after the ones written before.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// Why encoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError<E> {
    /// The sink refused a write.
    Sink(E),
    /// A width or height of 0, or an image past [`MAX_IMAGE_BYTES`].
    BadSize,
    /// A row of another width than the image's.
    WrongRow,
    /// More rows than the image's height.
    TooManyRows,
    /// Fewer rows than the image's height at [`Encoder::finish`].
    MissingRows,
}

/// The encoder's memory: the last 64 KiB of filtered rows, the hash table of where each
/// three-byte string last started, and the IDAT chunk being filled. All zero is its starting
/// state, so a zeroed static or stack array of it is ready.
pub struct EncoderWorkspace {
    history: [u8; HISTORY_BYTES],
    head: [u32; HASH_SLOTS],
    chunk: [u8; IDAT_BYTES],
}

impl EncoderWorkspace {
    /// All zero.
    pub const fn new() -> Self {
        Self {
            history: [0; HISTORY_BYTES],
            head: [0; HASH_SLOTS],
            chunk: [0; IDAT_BYTES],
        }
    }
}

impl Default for EncoderWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for EncoderWorkspace {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EncoderWorkspace")
            .finish_non_exhaustive()
    }
}

/// A PNG being written: [`Self::new`] writes the signature and the header, each
/// [`Self::write_row`] one row, [`Self::finish`] the rest.
pub struct Encoder<'w, S: PngSink> {
    sink: S,
    work: &'w mut EncoderWorkspace,
    width: u32,
    height: u32,
    rows: u32,
    bits: u64,
    bit_count: u32,
    chunk_length: usize,
    adler: Adler32,
    base: u64,
    filled: usize,
    cursor: usize,
    error: Option<S::Error>,
}

impl<S: PngSink> core::fmt::Debug for Encoder<'_, S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Encoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl<'w, S: PngSink> Encoder<'w, S> {
    /// Starts a `width` by `height` image in `work`, writing its signature and header to
    /// `sink`.
    pub fn new(
        width: u32,
        height: u32,
        work: &'w mut EncoderWorkspace,
        mut sink: S,
    ) -> Result<Self, EncodeError<S::Error>> {
        let filtered = (1 + u64::from(width) * BYTES_PER_PIXEL) * u64::from(height);
        if width == 0 || height == 0 || filtered > MAX_IMAGE_BYTES {
            return Err(EncodeError::BadSize);
        }
        work.head.fill(0);
        let mut header = [0u8; 13];
        header[..4].copy_from_slice(&width.to_be_bytes());
        header[4..8].copy_from_slice(&height.to_be_bytes());
        header[8..].copy_from_slice(&[8, 2, 0, 0, 0]);
        sink.write(&SIGNATURE).map_err(EncodeError::Sink)?;
        write_chunk(&mut sink, b"IHDR", &header).map_err(EncodeError::Sink)?;
        let mut encoder = Self {
            sink,
            work,
            width,
            height,
            rows: 0,
            bits: 0,
            bit_count: 0,
            chunk_length: 0,
            adler: Adler32::new(),
            base: 0,
            filled: 0,
            cursor: 0,
            error: None,
        };
        encoder.put_byte(0x78);
        encoder.put_byte(0x01);
        encoder.put_bits(1, 1);
        encoder.put_bits(1, 2);
        Ok(encoder)
    }

    /// Adds the next row, `width` pixels.
    pub fn write_row(&mut self, row: &[u32]) -> Result<(), EncodeError<S::Error>> {
        if row.len() != self.width as usize {
            return Err(EncodeError::WrongRow);
        }
        if self.rows == self.height {
            return Err(EncodeError::TooManyRows);
        }
        self.rows += 1;
        self.push(SUB_FILTER);
        let mut left = [0u8; 3];
        for &pixel in row {
            let rgb = [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8];
            for channel in 0..3 {
                self.push(rgb[channel].wrapping_sub(left[channel]));
            }
            left = rgb;
        }
        self.take_error()
    }

    /// The sink, to use between rows.
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Ends the image once every row is written: the rest of the compressed rows, the zlib
    /// checksum, the last IDAT chunk and IEND. Gives back the sink.
    pub fn finish(mut self) -> Result<S, EncodeError<S::Error>> {
        if self.rows != self.height {
            return Err(EncodeError::MissingRows);
        }
        self.compress(true);
        self.put_symbol(END_OF_BLOCK);
        if self.bit_count > 0 {
            self.put_bits(0, 8 - self.bit_count);
        }
        for byte in self.adler.value().to_be_bytes() {
            self.put_byte(byte);
        }
        if self.chunk_length > 0 {
            self.flush_chunk();
        }
        self.take_error()?;
        write_chunk(&mut self.sink, b"IEND", &[]).map_err(EncodeError::Sink)?;
        Ok(self.sink)
    }

    fn take_error(&mut self) -> Result<(), EncodeError<S::Error>> {
        match self.error.take() {
            Some(error) => Err(EncodeError::Sink(error)),
            None => Ok(()),
        }
    }

    fn push(&mut self, byte: u8) {
        if self.filled == HISTORY_BYTES {
            self.compress(false);
            self.work
                .history
                .copy_within(WINDOW_BYTES..HISTORY_BYTES, 0);
            self.base += WINDOW_BYTES as u64;
            self.filled -= WINDOW_BYTES;
            self.cursor -= WINDOW_BYTES;
        }
        self.work.history[self.filled] = byte;
        self.filled += 1;
        self.adler.update(&[byte]);
    }

    fn compress(&mut self, last: bool) {
        while self.cursor < self.filled {
            let available = self.filled - self.cursor;
            if !last && available < MAX_MATCH {
                return;
            }
            let (length, distance) = self.find_match(self.cursor, available);
            if length >= MIN_MATCH {
                self.put_match(length, distance);
                for at in self.cursor + 1..self.cursor + length {
                    self.insert(at);
                }
                self.cursor += length;
            } else {
                self.put_symbol(u32::from(self.work.history[self.cursor]));
                self.cursor += 1;
            }
        }
    }

    fn hash(&self, at: usize) -> usize {
        let bytes = &self.work.history[at..at + MIN_MATCH];
        let key = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        (key.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
    }

    fn insert(&mut self, at: usize) {
        if at + MIN_MATCH <= self.filled {
            let hash = self.hash(at);
            self.work.head[hash] = (self.base + at as u64) as u32 + 1;
        }
    }

    fn find_match(&mut self, at: usize, available: usize) -> (usize, usize) {
        if available < MIN_MATCH {
            return (0, 0);
        }
        let hash = self.hash(at);
        let position = self.base + at as u64;
        let candidate = self.work.head[hash];
        self.work.head[hash] = position as u32 + 1;
        let Some(start) = u64::from(candidate).checked_sub(1) else {
            return (0, 0);
        };
        if start < self.base || start >= position || position - start > WINDOW_BYTES as u64 {
            return (0, 0);
        }
        let from = (start - self.base) as usize;
        let limit = available.min(MAX_MATCH);
        let history = &self.work.history;
        let length = (0..limit)
            .take_while(|&offset| history[from + offset] == history[at + offset])
            .count();
        (length, (position - start) as usize)
    }

    fn put_bits(&mut self, value: u32, count: u32) {
        self.bits |= u64::from(value) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            let byte = self.bits as u8;
            self.bits >>= 8;
            self.bit_count -= 8;
            self.put_byte(byte);
        }
    }

    fn put_huffman(&mut self, code: u32, length: u32) {
        self.put_bits(code.reverse_bits() >> (32 - length), length);
    }

    fn put_symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.put_huffman(0x30 + symbol, 8),
            144..=255 => self.put_huffman(0x190 + symbol - 144, 9),
            256..=279 => self.put_huffman(symbol - 256, 7),
            _ => self.put_huffman(0xc0 + symbol - 280, 8),
        }
    }

    fn put_match(&mut self, length: usize, distance: usize) {
        let code = LENGTH_BASES
            .iter()
            .rposition(|&base| usize::from(base) <= length)
            .unwrap_or(0);
        self.put_symbol(257 + code as u32);
        self.put_bits(
            (length - usize::from(LENGTH_BASES[code])) as u32,
            u32::from(LENGTH_EXTRA_BITS[code]),
        );
        let code = DISTANCE_BASES
            .iter()
            .rposition(|&base| usize::from(base) <= distance)
            .unwrap_or(0);
        self.put_huffman(code as u32, 5);
        self.put_bits(
            (distance - usize::from(DISTANCE_BASES[code])) as u32,
            u32::from(DISTANCE_EXTRA_BITS[code]),
        );
    }

    fn put_byte(&mut self, byte: u8) {
        self.work.chunk[self.chunk_length] = byte;
        self.chunk_length += 1;
        if self.chunk_length == IDAT_BYTES {
            self.flush_chunk();
        }
    }

    fn flush_chunk(&mut self) {
        let length = core::mem::take(&mut self.chunk_length);
        if self.error.is_some() {
            return;
        }
        if let Err(error) = write_chunk(&mut self.sink, b"IDAT", &self.work.chunk[..length]) {
            self.error = Some(error);
        }
    }
}

fn write_chunk<S: PngSink>(sink: &mut S, kind: &[u8; 4], data: &[u8]) -> Result<(), S::Error> {
    let mut header = [0u8; 8];
    header[..4].copy_from_slice(&(data.len() as u32).to_be_bytes());
    header[4..].copy_from_slice(kind);
    sink.write(&header)?;
    sink.write(data)?;
    let crc = crc32_update(crc32_update(0, kind), data);
    sink.write(&crc.to_be_bytes())
}
