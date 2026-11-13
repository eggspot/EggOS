//! NSImage's and NSImageView's geometry and pixels (D374, docs/ux/macos-gui-v0.3.md section
//! 12): the image area inside a frame style, the drawn size each `imageScaling` gives and the
//! place each `imageAlignment` gives, nearest-neighbour scaling, cropping and the alpha a
//! `fraction` leaves. Pixels are premultiplied `0xAARRGGBB`; rectangles are content pixels,
//! top-left origin.

use alloc::vec::Vec;
use eggos_toolkit::geometry::Rect;

/// The longest side an image may have.
pub const MAX_SIDE: u32 = 4096;
/// The most pixels an image may have.
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;

/// `NSImageScaling`.
pub mod scaling {
    pub const PROPORTIONALLY_DOWN: usize = 0;
    pub const AXES_INDEPENDENTLY: usize = 1;
    pub const NONE: usize = 2;
    pub const PROPORTIONALLY_UP_OR_DOWN: usize = 3;
}

/// `NSImageAlignment`.
pub mod alignment {
    pub const CENTER: usize = 0;
    pub const TOP: usize = 1;
    pub const TOP_LEFT: usize = 2;
    pub const TOP_RIGHT: usize = 3;
    pub const LEFT: usize = 4;
    pub const BOTTOM: usize = 5;
    pub const BOTTOM_LEFT: usize = 6;
    pub const BOTTOM_RIGHT: usize = 7;
    pub const RIGHT: usize = 8;
}

/// `NSImageFrameStyle`.
pub mod frame_style {
    pub const NONE: usize = 0;
    pub const PHOTO: usize = 1;
    pub const GRAY_BEZEL: usize = 2;
    pub const GROOVE: usize = 3;
    pub const BUTTON: usize = 4;
}

const FRAMED_INSET: i32 = 2;

/// Whether an image `width` by `height` is within the limits.
pub fn fits(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

/// Where an image goes in `frame` drawn with `style`: the frame, or inset by 2 inside a line.
pub fn image_area(frame: Rect, style: usize) -> Rect {
    match style {
        frame_style::NONE => frame,
        _ => frame.inset(FRAMED_INSET),
    }
}

/// Whether `style` fills its frame with the button colour inside its line.
pub fn is_filled(style: usize) -> bool {
    matches!(style, frame_style::GRAY_BEZEL | frame_style::BUTTON)
}

fn proportional((width, height): (i32, i32), (area_width, area_height): (i32, i32)) -> (i32, i32) {
    let (iw, ih) = (i64::from(width), i64::from(height));
    let (aw, ah) = (i64::from(area_width), i64::from(area_height));
    if iw * ah <= ih * aw {
        ((iw * ah / ih.max(1)).max(1) as i32, area_height)
    } else {
        (area_width, (ih * aw / iw.max(1)).max(1) as i32)
    }
}

/// The size an image `image` drawn into an area `area` takes under `scaling`.
pub fn drawn_size(image: (i32, i32), area: (i32, i32), scaling: usize) -> (i32, i32) {
    match scaling {
        scaling::AXES_INDEPENDENTLY => area,
        scaling::NONE => image,
        scaling::PROPORTIONALLY_UP_OR_DOWN => proportional(image, area),
        _ if image.0 <= area.0 && image.1 <= area.1 => image,
        _ => proportional(image, area),
    }
}

/// The top-left of something `size` placed in `area` by `alignment`.
pub fn place(area: Rect, size: (i32, i32), alignment: usize) -> (i32, i32) {
    let centre_x = area.x + (area.width - size.0).div_euclid(2);
    let centre_y = area.y + (area.height - size.1).div_euclid(2);
    let (left, right) = (area.x, area.right() - size.0);
    let (top, bottom) = (area.y, area.bottom() - size.1);
    match alignment {
        alignment::TOP => (centre_x, top),
        alignment::TOP_LEFT => (left, top),
        alignment::TOP_RIGHT => (right, top),
        alignment::LEFT => (left, centre_y),
        alignment::BOTTOM => (centre_x, bottom),
        alignment::BOTTOM_LEFT => (left, bottom),
        alignment::BOTTOM_RIGHT => (right, bottom),
        alignment::RIGHT => (right, centre_y),
        _ => (centre_x, centre_y),
    }
}

/// The part `crop` of `pixels` (rows `width` apart) scaled to `size` by nearest neighbour.
pub fn scaled(pixels: &[u32], width: usize, crop: Rect, size: (i32, i32)) -> Vec<u32> {
    let (out_width, out_height) = (size.0.max(0) as usize, size.1.max(0) as usize);
    let mut out = Vec::new();
    if out.try_reserve_exact(out_width * out_height).is_err() || crop.is_empty() {
        return out;
    }
    let (crop_width, crop_height) = (crop.width as usize, crop.height as usize);
    for row in 0..out_height {
        let source_row = crop.y as usize + row * crop_height / out_height;
        for column in 0..out_width {
            let source_column = crop.x as usize + column * crop_width / out_width;
            out.push(
                pixels
                    .get(source_row * width + source_column)
                    .copied()
                    .unwrap_or(0),
            );
        }
    }
    out
}

/// `pixel` with its alpha, and so its premultiplied colour, scaled by `fraction` (0 to 1).
pub fn faded(pixel: u32, fraction: f64) -> u32 {
    if fraction >= 1.0 {
        return pixel;
    }
    let factor = (fraction.max(0.0) * 256.0) as u32;
    [24, 16, 8, 0].into_iter().fold(0, |mixed, shift| {
        mixed | ((pixel >> shift & 0xff) * factor / 256) << shift
    })
}

/// A straight-alpha `0xAARRGGBB` pixel premultiplied.
pub fn premultiplied(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    match alpha {
        0xff => pixel,
        0 => 0,
        _ => [16, 8, 0].into_iter().fold(alpha << 24, |mixed, shift| {
            mixed | (((pixel >> shift & 0xff) * alpha + 127) / 255) << shift
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const FRAME: Rect = Rect::new(0, 0, 96, 96);

    fn placed(image: (i32, i32), scaling: usize) -> ((i32, i32), (i32, i32)) {
        let size = drawn_size(image, (96, 96), scaling);
        (size, place(FRAME, size, alignment::CENTER))
    }

    #[test]
    fn a_small_image_under_each_scaling() {
        assert_eq!(
            placed((48, 48), scaling::PROPORTIONALLY_DOWN),
            ((48, 48), (24, 24))
        );
        assert_eq!(
            placed((48, 48), scaling::PROPORTIONALLY_UP_OR_DOWN),
            ((96, 96), (0, 0))
        );
        assert_eq!(
            placed((48, 48), scaling::AXES_INDEPENDENTLY),
            ((96, 96), (0, 0))
        );
    }

    #[test]
    fn a_wide_image_fits_or_overflows_evenly() {
        assert_eq!(
            placed((200, 100), scaling::PROPORTIONALLY_DOWN),
            ((96, 48), (0, 24))
        );
        assert_eq!(placed((200, 100), scaling::NONE), ((200, 100), (-52, -2)));
    }

    #[test]
    fn a_framed_style_insets_the_area_by_two() {
        assert_eq!(
            image_area(FRAME, frame_style::PHOTO),
            Rect::new(2, 2, 92, 92)
        );
        assert_eq!(image_area(FRAME, frame_style::NONE), FRAME);
    }

    #[test]
    fn alignments_are_flush_on_their_sides() {
        assert_eq!(place(FRAME, (48, 48), alignment::TOP_RIGHT), (48, 0));
        assert_eq!(place(FRAME, (48, 48), alignment::BOTTOM), (24, 48));
    }

    #[test]
    fn nearest_neighbour_scales_a_checker_into_cells() {
        let checker = [0xff00_0000, 0xffff_ffff, 0xffff_ffff, 0xff00_0000];
        let out = scaled(&checker, 2, Rect::new(0, 0, 2, 2), (96, 96));
        assert_eq!(out[0], 0xff00_0000);
        assert_eq!(out[47], 0xff00_0000);
        assert_eq!(out[48], 0xffff_ffff);
        assert_eq!(out[48 * 96], 0xffff_ffff);
        assert_eq!(out[95 * 96 + 95], 0xff00_0000);
    }

    #[test]
    fn half_a_fraction_halves_alpha_and_colour() {
        assert_eq!(faded(0xff80_4020, 0.5), 0x7f40_2010);
    }

    #[test]
    fn limits_refuse_long_or_huge_images() {
        assert!(fits(4096, 4096));
        assert!(!fits(5000, 10));
        assert!(!fits(0, 10));
    }

    #[test]
    fn premultiplying_scales_the_colour_by_alpha() {
        assert_eq!(premultiplied(0x80ff_0000), 0x8080_0000);
        assert_eq!(premultiplied(0x00ff_ffff), 0);
        let pixels = [premultiplied(0xffff_0000)];
        assert_eq!(pixels[0], 0xffff_0000);
    }
}
