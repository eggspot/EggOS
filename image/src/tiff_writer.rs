//! Writing an uncompressed TIFF (D727), as macOS's `TIFFRepresentation` gives one: little
//! endian, one strip of 8-bit RGBA, the alpha marked associated (premultiplied) or not.

use crate::Error;
use alloc::vec::Vec;

const ENTRIES: u16 = 11;
const ENTRY_BYTES: usize = 12;
const IFD_AT: u32 = 8;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const ASSOCIATED_ALPHA: u16 = 1;
const UNASSOCIATED_ALPHA: u16 = 2;
const RGB: u16 = 2;
const NO_COMPRESSION: u16 = 1;
const CHUNKY: u16 = 1;

mod tag {
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const PLANAR_CONFIGURATION: u16 = 284;
    pub const EXTRA_SAMPLES: u16 = 338;
}

fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    match (kind, count) {
        (SHORT, 1) => {
            out.extend_from_slice(&(value as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        _ => out.extend_from_slice(&value.to_le_bytes()),
    }
}

/// `width * height` `0xAARRGGBB` pixels, top row first, as an uncompressed TIFF; `premultiplied`
/// says whether their colour is already multiplied by their alpha.
pub fn encode_tiff(
    width: u32,
    height: u32,
    pixels: &[u32],
    premultiplied: bool,
) -> Result<Vec<u8>, Error> {
    let count = width as usize * height as usize;
    if width == 0 || height == 0 || pixels.len() < count {
        return Err(Error::Corrupt);
    }
    let data_bytes = u32::try_from(count * 4).map_err(|_| Error::TooLarge)?;
    let directory_end = IFD_AT as usize + 2 + usize::from(ENTRIES) * ENTRY_BYTES + 4;
    let bits_at = directory_end as u32;
    let data_at = bits_at + 8;
    let total = data_at as usize + data_bytes as usize;
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| Error::OutOfMemory)?;
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&IFD_AT.to_le_bytes());
    out.extend_from_slice(&ENTRIES.to_le_bytes());
    let alpha = match premultiplied {
        true => ASSOCIATED_ALPHA,
        false => UNASSOCIATED_ALPHA,
    };
    entry(&mut out, tag::IMAGE_WIDTH, LONG, 1, width);
    entry(&mut out, tag::IMAGE_LENGTH, LONG, 1, height);
    entry(&mut out, tag::BITS_PER_SAMPLE, SHORT, 4, bits_at);
    entry(&mut out, tag::COMPRESSION, SHORT, 1, u32::from(NO_COMPRESSION));
    entry(&mut out, tag::PHOTOMETRIC, SHORT, 1, u32::from(RGB));
    entry(&mut out, tag::STRIP_OFFSETS, LONG, 1, data_at);
    entry(&mut out, tag::SAMPLES_PER_PIXEL, SHORT, 1, 4);
    entry(&mut out, tag::ROWS_PER_STRIP, LONG, 1, height);
    entry(&mut out, tag::STRIP_BYTE_COUNTS, LONG, 1, data_bytes);
    entry(&mut out, tag::PLANAR_CONFIGURATION, SHORT, 1, u32::from(CHUNKY));
    entry(&mut out, tag::EXTRA_SAMPLES, SHORT, 1, u32::from(alpha));
    out.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..4 {
        out.extend_from_slice(&8u16.to_le_bytes());
    }
    for &pixel in &pixels[..count] {
        let [alpha, red, green, blue] = pixel.to_be_bytes();
        out.extend_from_slice(&[red, green, blue, alpha]);
    }
    Ok(out)
}
