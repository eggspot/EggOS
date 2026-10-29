//! The bitmap entries of Windows icons (the `BITMAPINFOHEADER` form an `RT_ICON` resource
//! holds, from Microsoft's public documentation): a header whose height counts the image and
//! its mask, a palette for 1, 4 and 8 bits per pixel, the colour rows bottom-up, each padded to
//! four bytes, then the 1-bit AND mask rows the same way (a set bit is transparent). A 32-bit
//! entry takes its alpha from its fourth byte, unless every alpha byte is 0 (the icons before
//! Windows XP), when the mask decides as for the other depths.

use crate::reader::Reader;

/// The largest side a bitmap entry may have.
pub const MAX_SIDE: u32 = 256;
/// The longest colour row: 256 pixels at 32 bits.
pub const MAX_LINE: usize = MAX_SIDE as usize * 4;
/// The longest mask row: 256 bits.
pub const MAX_MASK_LINE: usize = MAX_SIDE as usize / 8;

const HEADER_BYTES: usize = 40;
const MAX_PALETTE: usize = 256;
const OPAQUE: u32 = 0xff00_0000;

/// What an entry's header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    /// Its width and height.
    pub side: u32,
    /// Bits per pixel: 1, 4, 8, 24 or 32.
    pub bits: u16,
    header: u64,
    colours: usize,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn stride(side: u32, bits: u16) -> usize {
    (side as usize * usize::from(bits)).div_ceil(32) * 4
}

/// The header of the entry at `offset`: a square image, its height doubled for the mask, one
/// plane, uncompressed, at a depth read here.
pub fn info(reader: &mut impl Reader, offset: u64) -> Option<Info> {
    let mut header = [0u8; HEADER_BYTES];
    if !reader.read_exact_at(offset, &mut header) {
        return None;
    }
    let size = u32_at(&header, 0);
    let width = u32_at(&header, 4);
    let height = u32_at(&header, 8);
    let (planes, bits) = (u16_at(&header, 12), u16_at(&header, 14));
    let (compression, used) = (u32_at(&header, 16), u32_at(&header, 32));
    let fits = (1..=MAX_SIDE).contains(&width) && height == 2 * width;
    let plain = planes == 1 && compression == 0 && matches!(bits, 1 | 4 | 8 | 24 | 32);
    if !fits || !plain || (size as usize) < HEADER_BYTES {
        return None;
    }
    let colours = match bits {
        1 | 4 | 8 if used == 0 => 1usize << bits,
        1 | 4 | 8 => (used as usize).min(MAX_PALETTE),
        _ => 0,
    };
    Some(Info {
        side: width,
        bits,
        header: u64::from(size),
        colours,
    })
}

/// Decodes the entry at `offset` (`length` bytes), calling `each_row` with each row's index
/// from the top and its straight-alpha pixels; false when it cannot be read.
pub fn decode(
    reader: &mut impl Reader,
    offset: u64,
    length: u64,
    line: &mut [u8; MAX_LINE],
    mask: &mut [u8; MAX_MASK_LINE],
    mut each_row: impl FnMut(usize, &[u32]),
) -> bool {
    let Some(info) = info(reader, offset) else {
        return false;
    };
    let side = info.side as usize;
    let mut palette = [OPAQUE; MAX_PALETTE];
    let mut entry = [0u8; 4];
    for (index, colour) in palette.iter_mut().enumerate().take(info.colours) {
        if !reader.read_exact_at(offset + info.header + 4 * index as u64, &mut entry) {
            return false;
        }
        *colour =
            OPAQUE | u32::from(entry[2]) << 16 | u32::from(entry[1]) << 8 | u32::from(entry[0]);
    }
    let colour_stride = stride(info.side, info.bits);
    let mask_stride = stride(info.side, 1);
    let colour_start = offset + info.header + 4 * info.colours as u64;
    let mask_start = colour_start + (colour_stride * side) as u64;
    let has_mask = mask_start + (mask_stride * side) as u64 <= offset + length;
    if mask_start > offset + length {
        return false;
    }
    let line = &mut line[..colour_stride];
    let mask = &mut mask[..mask_stride];
    let mut own_alpha = false;
    if info.bits == 32 {
        for row in 0..side {
            if !reader.read_exact_at(colour_start + (row * colour_stride) as u64, line) {
                return false;
            }
            own_alpha |= line.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0);
        }
    }
    let mut pixels = [0u32; MAX_SIDE as usize];
    for row in 0..side {
        if !reader.read_exact_at(colour_start + (row * colour_stride) as u64, line) {
            return false;
        }
        mask.fill(0);
        if has_mask && !reader.read_exact_at(mask_start + (row * mask_stride) as u64, mask) {
            return false;
        }
        for (x, pixel) in pixels.iter_mut().enumerate().take(side) {
            let transparent = mask[x / 8] & (0x80 >> (x % 8)) != 0;
            let colour = match info.bits {
                32 => {
                    let bytes = &line[4 * x..4 * x + 4];
                    let rgb =
                        u32::from(bytes[2]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[0]);
                    match own_alpha {
                        true => u32::from(bytes[3]) << 24 | rgb,
                        false => OPAQUE | rgb,
                    }
                }
                24 => {
                    let bytes = &line[3 * x..3 * x + 3];
                    OPAQUE
                        | u32::from(bytes[2]) << 16
                        | u32::from(bytes[1]) << 8
                        | u32::from(bytes[0])
                }
                bits => {
                    let bits = usize::from(bits);
                    let per_byte = 8 / bits;
                    let shift = 8 - bits * (x % per_byte + 1);
                    let index = usize::from(line[x / per_byte] >> shift) & ((1 << bits) - 1);
                    match index < info.colours {
                        true => palette[index],
                        false => OPAQUE,
                    }
                }
            };
            *pixel = match transparent && !own_alpha {
                true => 0,
                false => colour,
            };
        }
        each_row(side - 1 - row, &pixels[..side]);
    }
    true
}

#[cfg(test)]
pub(crate) mod samples {
    pub fn bitmap(
        side: u32,
        bits: u16,
        palette: &[u32],
        rows: &[Vec<u8>],
        mask: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut entry = vec![0u8; 40];
        entry[..4].copy_from_slice(&40u32.to_le_bytes());
        entry[4..8].copy_from_slice(&side.to_le_bytes());
        entry[8..12].copy_from_slice(&(2 * side).to_le_bytes());
        entry[12..14].copy_from_slice(&1u16.to_le_bytes());
        entry[14..16].copy_from_slice(&bits.to_le_bytes());
        entry[32..36].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for colour in palette {
            entry.extend_from_slice(&colour.to_le_bytes());
        }
        for row in rows.iter().rev() {
            entry.extend_from_slice(row);
        }
        for row in mask.iter().rev() {
            entry.extend_from_slice(row);
        }
        entry
    }
}

#[cfg(test)]
mod tests {
    use super::samples::bitmap;
    use super::*;
    use crate::reader::Bytes;

    fn rows_of(entry: &[u8]) -> Option<Vec<Vec<u32>>> {
        let mut line = [0u8; MAX_LINE];
        let mut mask = [0u8; MAX_MASK_LINE];
        let side = info(&mut Bytes(entry), 0)?.side as usize;
        let mut rows = vec![Vec::new(); side];
        let decoded = decode(
            &mut Bytes(entry),
            0,
            entry.len() as u64,
            &mut line,
            &mut mask,
            |y, row| rows[y] = row.to_vec(),
        );
        decoded.then_some(rows)
    }

    #[test]
    fn a_32_bit_entry_keeps_its_alpha_and_ignores_its_mask() {
        let rows = vec![
            vec![0x30, 0x20, 0x10, 0xff, 0, 0, 0xff, 0x80],
            vec![0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff],
        ];
        let mask = vec![vec![0xc0, 0, 0, 0], vec![0xc0, 0, 0, 0]];
        let entry = bitmap(2, 32, &[], &rows, &mask);
        assert_eq!(
            rows_of(&entry).unwrap(),
            [[0xff10_2030, 0x80ff_0000], [0x0000_0000, 0xffff_ffff]]
        );
    }

    #[test]
    fn a_32_bit_entry_without_alpha_and_lower_depths_take_the_mask() {
        let rows = vec![vec![1, 2, 3, 0, 4, 5, 6, 0], vec![7, 8, 9, 0, 1, 1, 1, 0]];
        let mask = vec![vec![0x40, 0, 0, 0], vec![0x80, 0, 0, 0]];
        let entry = bitmap(2, 32, &[], &rows, &mask);
        assert_eq!(
            rows_of(&entry).unwrap(),
            [[0xff03_0201, 0x0000_0000], [0x0000_0000, 0xff01_0101]]
        );
        let rows = vec![
            vec![0b0110_0000, 0, 0, 0],
            vec![0b1001_0000, 0, 0, 0],
            vec![0; 4],
            vec![0; 4],
        ];
        let mask = vec![
            vec![0, 0, 0, 0],
            vec![0x80, 0, 0, 0],
            vec![0; 4],
            vec![0; 4],
        ];
        let palette = [0x0000_0000, 0x00ff_8040];
        let entry = bitmap(4, 1, &palette, &rows, &mask);
        assert_eq!(
            rows_of(&entry).unwrap()[..2],
            [
                vec![0xff00_0000, 0xffff_8040, 0xffff_8040, 0xff00_0000],
                vec![0x0000_0000, 0xff00_0000, 0xff00_0000, 0xffff_8040]
            ]
        );
        let rows = vec![vec![0x12, 0x34, 0x56, 0]];
        let entry = bitmap(1, 24, &[], &rows, &[vec![0, 0, 0, 0]]);
        assert_eq!(rows_of(&entry).unwrap(), [[0xff56_3412]]);
    }

    #[test]
    fn bitmaps_that_are_not_square_icons_are_refused() {
        let mut entry = bitmap(
            2,
            32,
            &[],
            &[vec![0; 8], vec![0; 8]],
            &[vec![0; 4], vec![0; 4]],
        );
        entry[8] = 2;
        assert!(rows_of(&entry).is_none());
        let mut entry = bitmap(
            2,
            16,
            &[],
            &[vec![0; 4], vec![0; 4]],
            &[vec![0; 4], vec![0; 4]],
        );
        assert!(rows_of(&entry).is_none());
        entry[14] = 32;
        entry.truncate(45);
        assert!(rows_of(&entry).is_none());
    }
}
