//! The pixel layouts a `CGImage` is made from (D739): 8-bit red, green and blue with or
//! without alpha, alpha first or last, premultiplied or not, in big- or little-endian words,
//! read into the premultiplied pixels the layer paints.

use alloc::vec::Vec;

const ALPHA_INFO_MASK: u32 = 0x1f;
const BYTE_ORDER_MASK: u32 = 0x7000;
const BYTE_ORDER_16_LITTLE: u32 = 1 << 12;
const BYTE_ORDER_32_LITTLE: u32 = 2 << 12;

/// Where alpha sits in each pixel, from a `CGBitmapInfo`'s alpha bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alpha {
    None,
    PremultipliedLast,
    PremultipliedFirst,
    Last,
    First,
    SkipLast,
    SkipFirst,
    Only,
}

impl Alpha {
    pub fn from_info(info: u32) -> Option<Self> {
        Some(match info & ALPHA_INFO_MASK {
            0 => Alpha::None,
            1 => Alpha::PremultipliedLast,
            2 => Alpha::PremultipliedFirst,
            3 => Alpha::Last,
            4 => Alpha::First,
            5 => Alpha::SkipLast,
            6 => Alpha::SkipFirst,
            7 => Alpha::Only,
            _ => return None,
        })
    }

    fn first(self) -> bool {
        matches!(self, Alpha::PremultipliedFirst | Alpha::First | Alpha::SkipFirst)
    }

    fn premultiplied(self) -> bool {
        matches!(self, Alpha::PremultipliedLast | Alpha::PremultipliedFirst)
    }

    fn carried(self) -> bool {
        !matches!(self, Alpha::None | Alpha::SkipLast | Alpha::SkipFirst)
    }
}

/// How an image's bytes are laid out: `CGImageCreate`'s arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub bits_per_component: usize,
    pub bits_per_pixel: usize,
    pub bytes_per_row: usize,
    pub info: u32,
}

/// The image's premultiplied pixels, top row first, or `None` for a layout the layer does not
/// read or bytes too short for it.
pub fn premultiplied_pixels(bytes: &[u8], layout: Layout) -> Option<Vec<u32>> {
    let alpha = Alpha::from_info(layout.info)?;
    let size = layout.bits_per_pixel / 8;
    let usable = layout.bits_per_component == 8
        && match alpha {
            Alpha::Only => size == 1,
            Alpha::None => size == 3 || size == 4,
            _ => size == 4,
        };
    let last_row = layout.height.checked_sub(1)?;
    let needed = last_row * layout.bytes_per_row + layout.width * size;
    if !usable || layout.width == 0 || bytes.len() < needed {
        return None;
    }
    let little = matches!(
        layout.info & BYTE_ORDER_MASK,
        BYTE_ORDER_32_LITTLE | BYTE_ORDER_16_LITTLE
    );
    let mut pixels = Vec::with_capacity(layout.width * layout.height);
    for row in 0..layout.height {
        let start = row * layout.bytes_per_row;
        for column in 0..layout.width {
            let at = start + column * size;
            let mut each = [0u8; 4];
            each[..size].copy_from_slice(&bytes[at..at + size]);
            if little && size == 4 {
                each.reverse();
            }
            pixels.push(pixel_of(&each[..size], alpha));
        }
    }
    Some(pixels)
}

fn pixel_of(bytes: &[u8], alpha: Alpha) -> u32 {
    if alpha == Alpha::Only {
        return u32::from(bytes[0]) << 24;
    }
    let (opacity, red, green, blue) = match (bytes.len(), alpha.first()) {
        (3, _) => (255, bytes[0], bytes[1], bytes[2]),
        (_, true) => (bytes[0], bytes[1], bytes[2], bytes[3]),
        (_, false) => (bytes[3], bytes[0], bytes[1], bytes[2]),
    };
    let opacity = match alpha.carried() {
        true => u32::from(opacity),
        false => 255,
    };
    let channel = |value: u8| match alpha.premultiplied() {
        true => u32::from(value).min(opacity),
        false => (u32::from(value) * opacity + 127) / 255,
    };
    (opacity << 24) | (channel(red) << 16) | (channel(green) << 8) | channel(blue)
}

/// The part of a `width`-wide image inside (`x`, `y`, `width`, `height`) from its top left,
/// cut to the image, with its size.
pub fn crop(
    pixels: &[u32],
    width: usize,
    (x, y, part_width, part_height): (usize, usize, usize, usize),
) -> (Vec<u32>, usize, usize) {
    let height = pixels.len() / width.max(1);
    let right = (x + part_width).min(width);
    let bottom = (y + part_height).min(height);
    if x >= right || y >= bottom {
        return (Vec::new(), 0, 0);
    }
    let mut kept = Vec::with_capacity((right - x) * (bottom - y));
    for row in y..bottom {
        kept.extend_from_slice(&pixels[row * width + x..row * width + right]);
    }
    (kept, right - x, bottom - y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(info: u32, bits_per_pixel: usize) -> Layout {
        Layout {
            width: 1,
            height: 1,
            bits_per_component: 8,
            bits_per_pixel,
            bytes_per_row: bits_per_pixel / 8,
            info,
        }
    }

    #[test]
    fn rgba_with_straight_alpha_is_premultiplied() {
        let pixels = premultiplied_pixels(&[255, 0, 0, 128], layout(3, 32)).unwrap();
        assert_eq!(pixels, [0x8080_0000]);
    }

    #[test]
    fn little_endian_premultiplied_first_reads_bgra_bytes() {
        let pixels = premultiplied_pixels(&[0x10, 0x20, 0x30, 0xff], layout(2 | 0x2000, 32));
        assert_eq!(pixels.unwrap(), [0xff30_2010]);
    }

    #[test]
    fn rgb_without_alpha_is_opaque() {
        let pixels = premultiplied_pixels(&[1, 2, 3], layout(0, 24)).unwrap();
        assert_eq!(pixels, [0xff01_0203]);
    }

    #[test]
    fn short_bytes_and_odd_layouts_are_refused() {
        assert_eq!(premultiplied_pixels(&[1, 2], layout(0, 24)), None);
        assert_eq!(premultiplied_pixels(&[1, 2, 3, 4], layout(3, 16)), None);
    }

    #[test]
    fn crops_stay_inside_the_image() {
        let pixels = [1, 2, 3, 4, 5, 6];
        assert_eq!(crop(&pixels, 3, (1, 0, 5, 1)), (alloc::vec![2, 3], 2, 1));
        assert_eq!(crop(&pixels, 3, (4, 0, 1, 1)), (Vec::new(), 0, 0));
    }
}
