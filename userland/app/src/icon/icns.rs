//! Apple's `.icns` icon files, from their publicly documented layout: the `icns` magic and
//! length, then entries of a four-letter type, a big-endian length counting its own 8-byte
//! header, and data. Entries holding a PNG (`ic07` to `ic14`, `icp4` to `icp6`, and `ic04` or
//! `ic05` when they do) are candidates at the PNG's own size; `is32`, `il32` and `ih32` (16, 32
//! and 48) are red, green and blue planes, raw or packed, with their alpha in `s8mk`, `l8mk` and
//! `h8mk`; `ic04` and `ic05` starting with `ARGB` are packed alpha, red, green and blue planes.
//! Packing: a byte `b` below `0x80` is followed by `b + 1` literal bytes, one at or above it by
//! one byte repeated `b - 125` times. JPEG 2000 entries, `it32` and every other type are
//! skipped.

use super::{Candidate, Candidates, Format, png_side};
use crate::reader::Reader;

/// The largest side of a planar entry.
pub const MAX_PLANAR_SIDE: usize = 48;
/// The most bytes a planar entry's data may have.
pub const MAX_PACKED: usize = 16 * 1024;
/// The bytes four planes of the largest planar entry take.
pub const MAX_PLANES: usize = MAX_PLANAR_SIDE * MAX_PLANAR_SIDE * 4;

const MAGIC: &[u8; 4] = b"icns";
const ENTRY_HEADER: u64 = 8;
const ARGB: &[u8; 4] = b"ARGB";
const MAX_ENTRIES: usize = 256;
const RGB_TYPES: [(&[u8; 4], usize); 3] = [(b"is32", 16), (b"il32", 32), (b"ih32", 48)];
const MASK_TYPES: [(&[u8; 4], usize); 3] = [(b"s8mk", 16), (b"l8mk", 32), (b"h8mk", 48)];
const ARGB_TYPES: [(&[u8; 4], usize); 2] = [(b"ic04", 16), (b"ic05", 32)];
const RGB_RANK: u8 = 2;
const ARGB_RANK: u8 = 1;
const PNG_RANK: u8 = 0;

fn side_of(types: &[(&[u8; 4], usize)], kind: &[u8; 4]) -> Option<usize> {
    types
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|&(_, side)| side)
}

/// Adds the candidates the `.icns` `reader` holds to `list`.
pub fn candidates(reader: &mut impl Reader, list: &mut Candidates) {
    let mut header = [0u8; 8];
    if !reader.read_exact_at(0, &mut header) || &header[..4] != MAGIC {
        return;
    }
    let declared = u64::from(u32::from_be_bytes([header[4], header[5], header[6], header[7]]));
    let end = declared.min(reader.size());
    let mut masks: [Option<(u64, u64)>; 3] = [None; 3];
    let mut at = ENTRY_HEADER;
    for _ in 0..MAX_ENTRIES {
        if at + ENTRY_HEADER > end || !reader.read_exact_at(at, &mut header) {
            break;
        }
        let kind = [header[0], header[1], header[2], header[3]];
        let length = u64::from(u32::from_be_bytes([header[4], header[5], header[6], header[7]]));
        if length < ENTRY_HEADER || at + length > end {
            break;
        }
        let (data, data_length) = (at + ENTRY_HEADER, length - ENTRY_HEADER);
        let mut first = [0u8; 4];
        let peeked = reader.read_exact_at(data, &mut first);
        if let Some(index) = MASK_TYPES.iter().position(|(name, _)| **name == kind) {
            masks[index] = Some((data, data_length));
        } else if let Some(side) = png_side(reader, data) {
            list.push(Candidate {
                side,
                rank: PNG_RANK,
                offset: data,
                length: data_length,
                format: Format::Png,
            });
        } else if let Some(side) = side_of(&RGB_TYPES, &kind) {
            list.push(Candidate {
                side: side as u32,
                rank: RGB_RANK,
                offset: data,
                length: data_length,
                format: Format::Planes { mask: None },
            });
        } else if let Some(side) = side_of(&ARGB_TYPES, &kind)
            && peeked
            && &first == ARGB
        {
            list.push(Candidate {
                side: side as u32,
                rank: ARGB_RANK,
                offset: data + 4,
                length: data_length - 4,
                format: Format::Argb,
            });
        }
        at += length;
    }
    for candidate in list.iter_mut() {
        if let Format::Planes { mask } = &mut candidate.format
            && let Some(index) = MASK_TYPES
                .iter()
                .position(|&(_, side)| side as u32 == candidate.side)
        {
            *mask = masks[index];
        }
    }
}

fn unpack(packed: &[u8], out: &mut [u8]) -> bool {
    let (mut from, mut to) = (0, 0);
    while to < out.len() {
        let Some(&control) = packed.get(from) else {
            return false;
        };
        from += 1;
        if control >= 0x80 {
            let count = usize::from(control) - 125;
            let Some(&value) = packed.get(from) else {
                return false;
            };
            from += 1;
            let Some(run) = out.get_mut(to..to + count) else {
                return false;
            };
            run.fill(value);
            to += count;
        } else {
            let count = usize::from(control) + 1;
            let (Some(bytes), Some(run)) = (packed.get(from..from + count), out.get_mut(to..to + count))
            else {
                return false;
            };
            run.copy_from_slice(bytes);
            from += count;
            to += count;
        }
    }
    true
}

/// Decodes the planar entry `candidate`, calling `each_row` with each row's index and its
/// straight-alpha pixels; false when it cannot be read.
pub fn decode(
    reader: &mut impl Reader,
    candidate: &Candidate,
    packed: &mut [u8; MAX_PACKED],
    planes: &mut [u8; MAX_PLANES],
    mut each_row: impl FnMut(usize, &[u32]),
) -> bool {
    let side = candidate.side as usize;
    let count = side * side;
    if side > MAX_PLANAR_SIDE || candidate.length > MAX_PACKED as u64 {
        return false;
    }
    let packed = &mut packed[..candidate.length as usize];
    if !reader.read_exact_at(candidate.offset, packed) {
        return false;
    }
    let argb = matches!(candidate.format, Format::Argb);
    let colour_planes = &mut planes[..count * 4];
    let unpacked = match (argb, packed.len() == 3 * count) {
        (false, true) => {
            colour_planes[..3 * count].copy_from_slice(packed);
            true
        }
        (false, false) => unpack(packed, &mut colour_planes[..3 * count]),
        (true, _) => unpack(packed, colour_planes),
    };
    if !unpacked {
        return false;
    }
    if let Format::Planes { mask } = candidate.format {
        let alpha = &mut colour_planes[3 * count..];
        match mask {
            Some((offset, length)) if length == count as u64 => {
                if !reader.read_exact_at(offset, alpha) {
                    return false;
                }
            }
            _ => alpha.fill(0xff),
        }
    }
    let plane = |index: usize, at: usize| u32::from(colour_planes[index * count + at]);
    let order: [usize; 4] = match argb {
        true => [0, 1, 2, 3],
        false => [3, 0, 1, 2],
    };
    let mut pixels = [0u32; MAX_PLANAR_SIDE];
    for y in 0..side {
        for (x, pixel) in pixels.iter_mut().enumerate().take(side) {
            let at = y * side + x;
            *pixel = plane(order[0], at) << 24
                | plane(order[1], at) << 16
                | plane(order[2], at) << 8
                | plane(order[3], at);
        }
        each_row(y, &pixels[..side]);
    }
    true
}

#[cfg(test)]
pub(crate) mod samples {
    pub fn icns(entries: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (kind, data) in entries {
            body.extend_from_slice(*kind);
            body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut file = b"icns".to_vec();
        file.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
        file.extend_from_slice(&body);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::samples::icns;
    use super::*;
    use crate::reader::Bytes;

    #[test]
    fn packed_planes_unpack_runs_and_literals() {
        let mut out = [0u8; 8];
        assert!(unpack(&[0x80, 9, 0x01, 1, 2, 0x80, 7], &mut out));
        assert_eq!(out, [9, 9, 9, 1, 2, 7, 7, 7]);
        let mut short = [0u8; 8];
        assert!(!unpack(&[0x80, 9], &mut short));
        let mut tight = [0u8; 2];
        assert!(!unpack(&[0x80, 9], &mut tight));
        assert!(!unpack(&[0x05, 1], &mut tight));
    }

    #[test]
    fn a_16_pixel_rgb_entry_takes_its_mask_and_an_argb_entry_its_own_alpha() {
        let count = 16 * 16;
        let mut raw = vec![0x10; count];
        raw.extend(vec![0x20; count]);
        raw.extend(vec![0x30; count]);
        let mut mask = vec![0xff; count];
        mask[0] = 0;
        let mut argb = b"ARGB".to_vec();
        for value in [0x80u8, 1, 2, 3] {
            let mut left = 32 * 32;
            while left > 0 {
                let run = left.min(130);
                argb.extend_from_slice(&[(run + 125) as u8, value]);
                left -= run;
            }
        }
        let file = icns(&[
            (b"TOC ", vec![0; 8]),
            (b"is32", raw),
            (b"s8mk", mask),
            (b"ic05", argb),
            (b"ic08", vec![0, 0, 0, 0x0c, b'j', b'P', b' ', b' ']),
        ]);
        let mut list = Candidates::new();
        candidates(&mut Bytes(&file), &mut list);
        assert_eq!(list.len(), 2);
        let mut packed = [0u8; MAX_PACKED];
        let mut planes = [0u8; MAX_PLANES];
        let mut rows = Vec::new();
        let rgb = list.iter().find(|candidate| candidate.side == 16).unwrap();
        assert_eq!(rgb.rank, RGB_RANK);
        assert!(decode(&mut Bytes(&file), rgb, &mut packed, &mut planes, |_, row| {
            rows.push(row.to_vec())
        }));
        assert_eq!(rows[0][0], 0x0010_2030);
        assert_eq!(rows[0][1], 0xff10_2030);
        assert_eq!(rows.len(), 16);
        let argb = *list.iter().find(|candidate| candidate.side == 32).unwrap();
        assert_eq!(argb.rank, ARGB_RANK);
        let mut first = None;
        assert!(decode(&mut Bytes(&file), &argb, &mut packed, &mut planes, |y, row| {
            if y == 0 {
                first = Some(row[0]);
            }
        }));
        assert_eq!(first, Some(0x8001_0203));
    }

    #[test]
    fn a_file_that_is_not_icns_or_is_cut_gives_what_it_can() {
        let mut list = Candidates::new();
        candidates(&mut Bytes(b"not an icns file"), &mut list);
        assert_eq!(list.len(), 0);
        let file = icns(&[(b"is32", vec![0x10; 3 * 256]), (b"il32", vec![0; 3 * 1024])]);
        candidates(&mut Bytes(&file[..file.len() - 1]), &mut list);
        assert_eq!(list.len(), 1);
    }
}
