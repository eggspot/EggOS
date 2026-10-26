//! A small PNG writer for `cargo xtask screenshot` (D116): 8-bit RGB, each row filtered with
//! the PNG filter whose bytes sum smallest, compressed by DEFLATE with LZ77 matches and the
//! fixed Huffman codes (RFC 1951), in one zlib stream (RFC 1950).

use crate::qmp::Picture;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// `picture` encoded as a PNG file.
pub fn encode(picture: &Picture) -> Vec<u8> {
    let mut png = SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend(picture.width.to_be_bytes());
    header.extend(picture.height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &header);
    write_chunk(&mut png, b"IDAT", &zlib(&filtered_rows(picture)));
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(data);
    let crc = crc32(&png[start..]);
    png.extend(crc.to_be_bytes());
}

const BYTES_PER_PIXEL: usize = 3;

fn filtered_rows(picture: &Picture) -> Vec<u8> {
    let stride = picture.width as usize * BYTES_PER_PIXEL;
    let rows: Vec<Vec<u8>> = picture
        .pixels
        .chunks(picture.width.max(1) as usize)
        .map(|row| {
            row.iter()
                .flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8])
                .collect()
        })
        .collect();
    let zero_row = vec![0u8; stride];
    let mut out = Vec::with_capacity(rows.len() * (stride + 1));
    for (index, row) in rows.iter().enumerate() {
        let above = if index == 0 {
            &zero_row
        } else {
            &rows[index - 1]
        };
        let best = (0..5u8)
            .map(|filter| filter_row(filter, row, above))
            .min_by_key(|filtered| {
                filtered[1..]
                    .iter()
                    .map(|&byte| u64::from((byte as i8).unsigned_abs()))
                    .sum::<u64>()
            })
            .expect("five filters");
        out.extend(best);
    }
    out
}

fn filter_row(filter: u8, row: &[u8], above: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len() + 1);
    out.push(filter);
    for i in 0..row.len() {
        let left = if i >= BYTES_PER_PIXEL {
            row[i - BYTES_PER_PIXEL]
        } else {
            0
        };
        let up = above[i];
        let up_left = if i >= BYTES_PER_PIXEL {
            above[i - BYTES_PER_PIXEL]
        } else {
            0
        };
        let predicted = match filter {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            _ => paeth(left, up, up_left),
        };
        out.push(row[i].wrapping_sub(predicted));
    }
    out
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

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    out.extend(deflate(data));
    out.extend(adler32(data).to_be_bytes());
    out
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_buffer: u64,
    bit_count: u32,
}

impl BitWriter {
    fn bits(&mut self, value: u32, count: u32) {
        self.bit_buffer |= u64::from(value) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.bytes.push(self.bit_buffer as u8);
            self.bit_buffer >>= 8;
            self.bit_count -= 8;
        }
    }

    fn huffman(&mut self, code: u32, length: u32) {
        let reversed = code.reverse_bits() >> (32 - length);
        self.bits(reversed, length);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_count > 0 {
            self.bytes.push(self.bit_buffer as u8);
        }
        self.bytes
    }
}

const LENGTH_BASES: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASES: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const END_OF_BLOCK: u32 = 256;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const WINDOW: usize = 32 * 1024;
const HASH_BITS: u32 = 15;
const MAX_CHAIN: usize = 64;
const NO_POSITION: u32 = u32::MAX;

fn write_literal_or_length(out: &mut BitWriter, symbol: u32) {
    match symbol {
        0..=143 => out.huffman(0x30 + symbol, 8),
        144..=255 => out.huffman(0x190 + symbol - 144, 9),
        256..=279 => out.huffman(symbol - 256, 7),
        _ => out.huffman(0xc0 + symbol - 280, 8),
    }
}

fn write_match(out: &mut BitWriter, length: usize, distance: usize) {
    let code = LENGTH_BASES
        .iter()
        .rposition(|&base| usize::from(base) <= length)
        .expect("a match is at least three bytes");
    write_literal_or_length(out, 257 + code as u32);
    out.bits(
        (length - usize::from(LENGTH_BASES[code])) as u32,
        u32::from(LENGTH_EXTRA_BITS[code]),
    );
    let code = DISTANCE_BASES
        .iter()
        .rposition(|&base| usize::from(base) <= distance)
        .expect("a distance is at least one");
    out.huffman(code as u32, 5);
    out.bits(
        (distance - usize::from(DISTANCE_BASES[code])) as u32,
        u32::from(DISTANCE_EXTRA_BITS[code]),
    );
}

fn hash_at(data: &[u8], at: usize) -> usize {
    let key = u32::from(data[at]) << 16 | u32::from(data[at + 1]) << 8 | u32::from(data[at + 2]);
    (key.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut out = BitWriter::default();
    out.bits(1, 1);
    out.bits(1, 2);
    let mut head = vec![NO_POSITION; 1 << HASH_BITS];
    let mut previous = vec![NO_POSITION; WINDOW];
    let insert = |head: &mut Vec<u32>, previous: &mut Vec<u32>, at: usize| {
        if at + MIN_MATCH <= data.len() {
            let hash = hash_at(data, at);
            previous[at % WINDOW] = head[hash];
            head[hash] = at as u32;
        }
    };
    let mut at = 0;
    while at < data.len() {
        let (length, distance) = longest_match(data, at, &head, &previous);
        if length >= MIN_MATCH {
            write_match(&mut out, length, distance);
            for position in at..at + length {
                insert(&mut head, &mut previous, position);
            }
            at += length;
        } else {
            write_literal_or_length(&mut out, u32::from(data[at]));
            insert(&mut head, &mut previous, at);
            at += 1;
        }
    }
    write_literal_or_length(&mut out, END_OF_BLOCK);
    out.finish()
}

fn longest_match(data: &[u8], at: usize, head: &[u32], previous: &[u32]) -> (usize, usize) {
    if at + MIN_MATCH > data.len() {
        return (0, 0);
    }
    let limit = (data.len() - at).min(MAX_MATCH);
    let mut best = (0, 0);
    let mut candidate = head[hash_at(data, at)];
    for _ in 0..MAX_CHAIN {
        if candidate == NO_POSITION {
            break;
        }
        let start = candidate as usize;
        if start >= at || at - start > WINDOW {
            break;
        }
        let length = data[start..]
            .iter()
            .zip(&data[at..at + limit])
            .take_while(|(a, b)| a == b)
            .count();
        if length > best.0 {
            best = (length, at - start);
            if length == limit {
                break;
            }
        }
        let next = previous[start % WINDOW];
        if next != NO_POSITION && next as usize >= start {
            break;
        }
        candidate = next;
    }
    best
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 == 1 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *entry = value;
    }
    !data.iter().fold(u32::MAX, |crc, &byte| {
        table[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

fn adler32(data: &[u8]) -> u32 {
    const MODULUS: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    b << 16 | a
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BitReader<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl BitReader<'_> {
        fn bit(&mut self) -> u32 {
            let bit = (self.bytes[self.at / 8] >> (self.at % 8)) & 1;
            self.at += 1;
            u32::from(bit)
        }

        fn bits(&mut self, count: u8) -> usize {
            (0..count).fold(0, |value, shift| value | (self.bit() as usize) << shift)
        }

        fn huffman(&mut self, length: u32) -> u32 {
            (0..length).fold(0, |code, _| code << 1 | self.bit())
        }

        fn literal_or_length(&mut self) -> u32 {
            let code = self.huffman(7);
            if code <= 0x17 {
                return code + 256;
            }
            let code = code << 1 | self.bit();
            match code {
                0x30..=0xbf => code - 0x30,
                0xc0..=0xc7 => code - 0xc0 + 280,
                _ => (code << 1 | self.bit()) - 0x190 + 144,
            }
        }
    }

    fn inflate_fixed(stream: &[u8]) -> Vec<u8> {
        let mut reader = BitReader {
            bytes: stream,
            at: 0,
        };
        assert_eq!(reader.bits(1), 1, "the last block");
        assert_eq!(reader.bits(2), 1, "fixed Huffman codes");
        let mut out: Vec<u8> = Vec::new();
        loop {
            let symbol = reader.literal_or_length();
            match symbol {
                0..=255 => out.push(symbol as u8),
                END_OF_BLOCK => return out,
                _ => {
                    let code = (symbol - 257) as usize;
                    let length =
                        usize::from(LENGTH_BASES[code]) + reader.bits(LENGTH_EXTRA_BITS[code]);
                    let code = reader.huffman(5) as usize;
                    let distance =
                        usize::from(DISTANCE_BASES[code]) + reader.bits(DISTANCE_EXTRA_BITS[code]);
                    for _ in 0..length {
                        out.push(out[out.len() - distance]);
                    }
                }
            }
        }
    }

    #[test]
    fn deflate_round_trips_through_a_fixed_huffman_inflater() {
        let mut varied: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        varied.extend(vec![0u8; 40_000]);
        varied.extend(b"abcabcabcabd".repeat(300));
        for data in [&b""[..], b"a", b"aaaaaaaaaaaaaaaaaaaaaaaa", &varied] {
            assert_eq!(inflate_fixed(&deflate(data)), data);
        }
        assert!(deflate(&vec![0u8; 100_000]).len() < 1_000);
    }

    #[test]
    fn checksums_match_their_reference_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(&vec![0xff; 100_000]), {
            let (mut a, mut b) = (1u64, 0u64);
            for _ in 0..100_000 {
                a = (a + 0xff) % 65521;
                b = (b + a) % 65521;
            }
            (b << 16 | a) as u32
        });
    }

    #[test]
    fn every_filter_is_undone_by_its_predictor() {
        let row = [10u8, 200, 30, 40, 250, 60, 7, 8, 9];
        let above = [1u8, 2, 3, 255, 128, 0, 70, 80, 90];
        for filter in 0..5 {
            let filtered = filter_row(filter, &row, &above);
            let mut decoded = Vec::new();
            for i in 0..row.len() {
                let left = if i >= 3 { decoded[i - 3] } else { 0 };
                let up_left = if i >= 3 { above[i - 3] } else { 0 };
                let predicted = match filter {
                    0 => 0,
                    1 => left,
                    2 => above[i],
                    3 => ((u16::from(left) + u16::from(above[i])) / 2) as u8,
                    _ => paeth(left, above[i], up_left),
                };
                decoded.push(filtered[i + 1].wrapping_add(predicted));
            }
            assert_eq!(decoded, row, "filter {filter}");
        }
    }

    #[test]
    fn a_png_has_its_signature_header_and_checked_chunks() {
        let picture = Picture {
            width: 2,
            height: 1,
            pixels: vec![0x00ff_0000, 0x0000_ff00],
        };
        let png = encode(&picture);
        assert_eq!(png[..8], SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(crc32(&png[12..29]).to_be_bytes(), png[29..33]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        let idat_length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let zlib = &png[41..41 + idat_length];
        let rows = inflate_fixed(&zlib[2..zlib.len() - 4]);
        assert_eq!(rows.len(), 1 + 2 * 3);
        assert_eq!(
            adler32(&rows).to_be_bytes(),
            zlib[zlib.len() - 4..],
            "the zlib trailer"
        );
    }
}
