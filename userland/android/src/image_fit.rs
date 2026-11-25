//! How an `ImageView` fits its picture in its box (D763): `ImageView.ScaleType`'s rules for
//! the picture's drawn size and place, and the picture resampled to a new size (an area
//! average when it shrinks, the nearest pixel when it grows), as premultiplied pixels.

use alloc::vec::Vec;

/// `ImageView.ScaleType`'s ordinals.
pub mod scale_type {
    pub const MATRIX: u32 = 0;
    pub const FIT_XY: u32 = 1;
    pub const FIT_START: u32 = 2;
    pub const FIT_CENTER: u32 = 3;
    pub const FIT_END: u32 = 4;
    pub const CENTER: u32 = 5;
    pub const CENTER_CROP: u32 = 6;
    pub const CENTER_INSIDE: u32 = 7;
}

/// Where a picture is drawn in its box: its drawn size and its top-left corner, relative to
/// the box (negative when it is cropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

fn scaled(size: (i32, i32), numerator: i64, denominator: i64) -> (i32, i32) {
    let scale = |length: i32| (i64::from(length) * numerator / denominator.max(1)) as i32;
    (scale(size.0).max(1), scale(size.1).max(1))
}

/// Where a `picture`-sized picture goes in a `area`-sized box under scale type `kind`.
pub fn fit(kind: u32, picture: (i32, i32), area: (i32, i32)) -> Placement {
    let (pw, ph) = (picture.0.max(1), picture.1.max(1));
    let (bw, bh) = area;
    let wider = i64::from(bw) * i64::from(ph) > i64::from(bh) * i64::from(pw);
    let inside = || match wider {
        true => scaled((pw, ph), i64::from(bh), i64::from(ph)),
        false => scaled((pw, ph), i64::from(bw), i64::from(pw)),
    };
    let (width, height) = match kind {
        scale_type::FIT_XY => (bw, bh),
        scale_type::MATRIX | scale_type::CENTER => (pw, ph),
        scale_type::CENTER_CROP => match wider {
            true => scaled((pw, ph), i64::from(bw), i64::from(pw)),
            false => scaled((pw, ph), i64::from(bh), i64::from(ph)),
        },
        scale_type::CENTER_INSIDE if pw <= bw && ph <= bh => (pw, ph),
        _ => inside(),
    };
    let (x, y) = match kind {
        scale_type::MATRIX | scale_type::FIT_START => (0, 0),
        scale_type::FIT_END => (bw - width, bh - height),
        _ => ((bw - width) / 2, (bh - height) / 2),
    };
    Placement {
        x,
        y,
        width,
        height,
    }
}

fn channels(pixel: u32) -> [u64; 4] {
    [
        u64::from(pixel >> 24),
        u64::from(pixel >> 16 & 0xff),
        u64::from(pixel >> 8 & 0xff),
        u64::from(pixel & 0xff),
    ]
}

/// `pixels` (`width` wide) resampled to `new_width` by `new_height`.
pub fn resample(pixels: &[u32], width: usize, new_width: usize, new_height: usize) -> Vec<u32> {
    let height = pixels.len() / width.max(1);
    if width == 0 || height == 0 || new_width == 0 || new_height == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(new_width * new_height);
    for row in 0..new_height {
        let top = row * height / new_height;
        let bottom = ((row + 1) * height / new_height).max(top + 1).min(height);
        for column in 0..new_width {
            let left = column * width / new_width;
            let right = ((column + 1) * width / new_width).max(left + 1).min(width);
            let mut sum = [0u64; 4];
            for source_row in top..bottom {
                for &pixel in &pixels[source_row * width + left..source_row * width + right] {
                    for (total, part) in sum.iter_mut().zip(channels(pixel)) {
                        *total += part;
                    }
                }
            }
            let count = ((bottom - top) * (right - left)) as u64;
            let [a, r, g, b] = sum.map(|total| (total / count) as u32);
            out.push(a << 24 | r << 16 | g << 8 | b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_scale_type_places_a_wide_picture_in_a_square_box() {
        let picture = (200, 100);
        let area = (100, 100);
        assert_eq!(
            fit(scale_type::FIT_CENTER, picture, area),
            Placement { x: 0, y: 25, width: 100, height: 50 }
        );
        assert_eq!(
            fit(scale_type::CENTER_CROP, picture, area),
            Placement { x: -50, y: 0, width: 200, height: 100 }
        );
        assert_eq!(
            fit(scale_type::FIT_XY, picture, area),
            Placement { x: 0, y: 0, width: 100, height: 100 }
        );
        assert_eq!(
            fit(scale_type::FIT_END, picture, area),
            Placement { x: 0, y: 50, width: 100, height: 50 }
        );
        assert_eq!(
            fit(scale_type::CENTER, picture, area),
            Placement { x: -50, y: -25, width: 200, height: 100 }
        );
        assert_eq!(
            fit(scale_type::CENTER_INSIDE, (40, 20), area),
            Placement { x: 30, y: 40, width: 40, height: 20 }
        );
    }

    #[test]
    fn shrinking_averages_and_growing_repeats() {
        let pixels = [0xff00_0000, 0xff00_00fe, 0xff00_0000, 0xff00_00fe];
        assert_eq!(resample(&pixels, 2, 1, 1), vec![0xff00_007f]);
        let grown = resample(&[0xff11_2233], 1, 2, 2);
        assert_eq!(grown, vec![0xff11_2233; 4]);
    }
}
