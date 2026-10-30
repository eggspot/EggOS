//! A program's images as the common controls and `STATIC` pictures draw them (section 27 and 28
//! of the design, D178): a bitmap's pixels copied as given, no blending, a pixel left out when a
//! mask covers it or, in a 32-bit bitmap whose alpha is not zero everywhere, when its alpha is
//! below 128; image lists' cells cut from bitmaps with their one-bit masks; the grey of a
//! disabled toolbar button; and the nearest-neighbour stretch of `SS_REALSIZECONTROL`.

use crate::colour;
use crate::surface::{Rect, Surface};

/// The lowest alpha a pixel of a bitmap with alpha is drawn with.
pub const ALPHA_THRESHOLD: u32 = 128;
/// `CLR_DEFAULT` as `ImageList_AddMasked`'s mask colour: the bitmap's top-left pixel.
pub const DEFAULT_MASK_COLOUR: u32 = 0xff00_0000;
/// `CLR_NONE` as `ImageList_AddMasked`'s mask colour: nothing masked.
pub const NO_MASK_COLOUR: u32 = 0xffff_ffff;

/// A bitmap's pixels: `0x00RRGGBB`, a 32-bit DIB's alpha in the top byte, rows of `width`.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    pub pixels: &'a [u32],
    pub width: i32,
    pub height: i32,
}

impl<'a> Picture<'a> {
    /// The picture of `width` by `height` pixels in `pixels`; `None` when `pixels` is shorter.
    pub fn new(pixels: &'a [u32], width: i32, height: i32) -> Option<Self> {
        let needed = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        (pixels.len() >= needed).then_some(Self {
            pixels,
            width,
            height,
        })
    }

    /// The pixel at (`x`, `y`), if it is inside.
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        let inside = x >= 0 && y >= 0 && x < self.width && y < self.height;
        inside.then(|| self.pixels[y as usize * self.width as usize + x as usize])
    }

    /// Whether its alpha decides which pixels show: some pixel's alpha is not zero.
    pub fn has_alpha(&self) -> bool {
        let count = self.width.max(0) as usize * self.height.max(0) as usize;
        self.pixels[..count].iter().any(|pixel| pixel >> 24 != 0)
    }
}

/// Whether `pixel` shows when its picture's alpha decides (`alpha`).
pub fn opaque(pixel: u32, alpha: bool) -> bool {
    !alpha || pixel >> 24 >= ALPHA_THRESHOLD
}

/// The grey a disabled image's pixel becomes: `128 + (77 r + 151 g + 28 b) / 512`.
pub fn grey(pixel: u32) -> u32 {
    let (r, g, b) = ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff);
    let level = 128 + (77 * r + 151 * g + 28 * b) / 512;
    (level << 16) | (level << 8) | level
}

/// Whether a mask bitmap's pixel is a 1 bit: anything but black.
fn mask_bit(pixel: u32) -> bool {
    pixel & 0x00ff_ffff != 0
}

/// Draws `picture` with its top-left at (`x`, `y`), unscaled; with `mask`, the pixels whose
/// mask pixel is a 1 bit are left out.
pub fn draw_picture(
    surface: &mut Surface<'_>,
    picture: &Picture<'_>,
    mask: Option<&Picture<'_>>,
    x: i32,
    y: i32,
) {
    let alpha = picture.has_alpha();
    let area = Rect::sized(x, y, picture.width, picture.height).intersection(&surface.clip());
    for dy in area.top..area.bottom {
        for dx in area.left..area.right {
            let (sx, sy) = (dx - x, dy - y);
            let Some(pixel) = picture.get(sx, sy) else {
                continue;
            };
            let masked = mask.is_some_and(|mask| mask.get(sx, sy).is_some_and(mask_bit));
            if !masked && opaque(pixel, alpha) {
                surface.pixel(dx, dy, pixel & 0x00ff_ffff);
            }
        }
    }
}

/// Draws `picture` stretched over `target`, nearest neighbour: the pixel at (`dx`, `dy`) of the
/// target is the picture's (`dx * width / target width`, `dy * height / target height`).
pub fn draw_stretched(surface: &mut Surface<'_>, picture: &Picture<'_>, target: Rect) {
    let (width, height) = (target.width(), target.height());
    if width <= 0 || height <= 0 || picture.width <= 0 || picture.height <= 0 {
        return;
    }
    let alpha = picture.has_alpha();
    let area = target.intersection(&surface.clip());
    let scale = |offset: i32, source: i32, room: i32| {
        (i64::from(offset) * i64::from(source) / i64::from(room)) as i32
    };
    for y in area.top..area.bottom {
        let sy = scale(y - target.top, picture.height, height);
        for x in area.left..area.right {
            let sx = scale(x - target.left, picture.width, width);
            if let Some(pixel) = picture.get(sx, sy)
                && opaque(pixel, alpha)
            {
                surface.pixel(x, y, pixel & 0x00ff_ffff);
            }
        }
    }
}

/// The bytes of a `width` by `height` one-bit mask.
pub fn mask_bytes(width: i32, height: i32) -> usize {
    (width.max(0) as usize * height.max(0) as usize).div_ceil(8)
}

/// Where an image list's image comes from: the bitmap and what masks its pixels.
#[derive(Debug, Clone, Copy)]
pub enum MaskSource<'a> {
    /// Nothing is masked.
    None,
    /// A mask bitmap's 1 bits (`ImageList_Add`'s `hbmMask`, an icon's mask).
    Bitmap(Picture<'a>),
    /// The pixels of this `COLORREF` (`ImageList_AddMasked`; [`DEFAULT_MASK_COLOUR`] is the
    /// bitmap's top-left pixel, [`NO_MASK_COLOUR`] masks nothing).
    Colour(u32),
}

/// Cuts a `cx` by `cy` image from `source` at (`left`, 0) into `pixels` (`cx * cy`, rows of
/// `cx`) and `mask` ([`mask_bytes`], row-major bits, 1 where the pixel is not drawn): pixels
/// outside the bitmap are masked, then `masks` masks its own, then, when the bitmap has alpha,
/// those under the alpha threshold.
pub fn cut(
    source: &Picture<'_>,
    masks: MaskSource<'_>,
    left: i32,
    (cx, cy): (i32, i32),
    pixels: &mut [u32],
    mask: &mut [u8],
) {
    let alpha = source.has_alpha();
    let colour_mask = match masks {
        MaskSource::Colour(NO_MASK_COLOUR) => None,
        MaskSource::Colour(DEFAULT_MASK_COLOUR) => {
            source.get(0, 0).map(|pixel| pixel & 0x00ff_ffff)
        }
        MaskSource::Colour(colour_ref) => Some(colour::pixel(colour_ref)),
        _ => None,
    };
    mask[..mask_bytes(cx, cy)].fill(0);
    for y in 0..cy.max(0) {
        for x in 0..cx.max(0) {
            let at = (y * cx + x) as usize;
            let (sx, sy) = (left + x, y);
            let pixel = source.get(sx, sy);
            let masked = match (pixel, masks) {
                (None, _) => true,
                (Some(_), MaskSource::Bitmap(bits)) => bits.get(sx, sy).is_some_and(mask_bit),
                (Some(pixel), _) => colour_mask == Some(pixel & 0x00ff_ffff),
            };
            let shown = pixel.filter(|&pixel| !masked && opaque(pixel, alpha));
            pixels[at] = shown.map_or(0, |pixel| pixel & 0x00ff_ffff);
            if shown.is_none() {
                mask[at / 8] |= 1 << (at % 8);
            }
        }
    }
}

/// Draws an image list's image (`pixels` and `mask` as [`cut`] made them, `cx` wide) with its
/// top-left at (`x`, `y`), each drawn pixel greyed when `greyed`.
pub fn draw_cell(
    surface: &mut Surface<'_>,
    pixels: &[u32],
    mask: &[u8],
    (cx, cy): (i32, i32),
    (x, y): (i32, i32),
    greyed: bool,
) {
    let area = Rect::sized(x, y, cx, cy).intersection(&surface.clip());
    for dy in area.top..area.bottom {
        for dx in area.left..area.right {
            let at = ((dy - y) * cx + (dx - x)) as usize;
            if mask[at / 8] & (1 << (at % 8)) != 0 {
                continue;
            }
            let pixel = match greyed {
                true => grey(pixels[at]),
                false => pixels[at],
            };
            surface.pixel(dx, dy, pixel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(width: u32, height: u32) -> Vec<u32> {
        vec![0x0012_3456; (width * height) as usize]
    }

    #[test]
    fn the_grey_of_red_is_166() {
        assert_eq!(grey(0x00ff_0000), 0x00a6_a6a6);
        assert_eq!(grey(0), 0x0080_8080);
        assert_eq!(grey(0x00ff_ffff), 0x00ff_ffff);
    }

    #[test]
    fn alpha_decides_only_when_it_is_not_zero_everywhere() {
        let pixels = [0x7f11_1111, 0x8022_2222, 0xff33_3333, 0x0044_4444];
        let picture = Picture::new(&pixels, 4, 1).unwrap();
        assert!(picture.has_alpha());
        let mut buffer = canvas(4, 1);
        let mut surface = Surface::new(&mut buffer, 4, 1).unwrap();
        draw_picture(&mut surface, &picture, None, 0, 0);
        assert_eq!(surface.get(0, 0), Some(0x0012_3456));
        assert_eq!(surface.get(1, 0), Some(0x0022_2222));
        assert_eq!(surface.get(2, 0), Some(0x0033_3333));
        assert_eq!(surface.get(3, 0), Some(0x0012_3456));
        let flat = [0x0011_1111, 0x0022_2222];
        let opaque_picture = Picture::new(&flat, 2, 1).unwrap();
        assert!(!opaque_picture.has_alpha());
        draw_picture(&mut surface, &opaque_picture, None, 0, 0);
        assert_eq!(surface.get(0, 0), Some(0x0011_1111));
        assert_eq!(surface.get(1, 0), Some(0x0022_2222));
    }

    #[test]
    fn add_masked_masks_the_colour_and_by_default_the_top_left_pixel() {
        let pixels = [
            0x00ff_00ff,
            0x0000_0000,
            0x00ff_00ff,
            0x0011_2233,
            0x0011_2233,
            0x00ff_00ff,
            0x0000_0000,
            0x0000_0000,
        ];
        let source = Picture::new(&pixels, 4, 2).unwrap();
        let mut cell = [0u32; 4];
        let mut mask = [0u8; 1];
        cut(
            &source,
            MaskSource::Colour(0x00ff_00ff),
            0,
            (2, 2),
            &mut cell,
            &mut mask,
        );
        assert_eq!(mask[0], 0b1001);
        assert_eq!(cell, [0, 0, 0x0011_2233, 0]);
        cut(
            &source,
            MaskSource::Colour(DEFAULT_MASK_COLOUR),
            2,
            (2, 2),
            &mut cell,
            &mut mask,
        );
        assert_eq!(mask[0], 0b0001);
        cut(
            &source,
            MaskSource::Colour(NO_MASK_COLOUR),
            2,
            (2, 2),
            &mut cell,
            &mut mask,
        );
        assert_eq!(mask[0], 0);
    }

    #[test]
    fn a_small_icon_sits_at_the_top_left_of_its_cell_and_the_rest_is_masked() {
        let icon = vec![0x00aa_bbcc; 12 * 12];
        let source = Picture::new(&icon, 12, 12).unwrap();
        let mut cell = [0u32; 16 * 16];
        let mut mask = [0u8; 32];
        cut(&source, MaskSource::None, 0, (16, 16), &mut cell, &mut mask);
        let mut buffer = canvas(16, 16);
        let mut surface = Surface::new(&mut buffer, 16, 16).unwrap();
        draw_cell(&mut surface, &cell, &mask, (16, 16), (0, 0), false);
        assert_eq!(surface.get(11, 11), Some(0x00aa_bbcc));
        assert_eq!(surface.get(12, 0), Some(0x0012_3456));
        assert_eq!(surface.get(0, 12), Some(0x0012_3456));
        draw_cell(&mut surface, &cell, &mask, (16, 16), (0, 0), true);
        assert_eq!(surface.get(0, 0), Some(grey(0x00aa_bbcc)));
    }

    #[test]
    fn a_mask_bitmaps_white_pixels_are_left_out() {
        let colours = [0x0011_1111, 0x0022_2222];
        let bits = [0x00ff_ffff, 0];
        let colour = Picture::new(&colours, 2, 1).unwrap();
        let mask = Picture::new(&bits, 2, 1).unwrap();
        let mut buffer = canvas(2, 1);
        let mut surface = Surface::new(&mut buffer, 2, 1).unwrap();
        draw_picture(&mut surface, &colour, Some(&mask), 0, 0);
        assert_eq!(surface.get(0, 0), Some(0x0012_3456));
        assert_eq!(surface.get(1, 0), Some(0x0022_2222));
    }

    #[test]
    fn stretching_twice_repeats_each_pixel_twice() {
        let pixels = [1, 2, 3, 4];
        let picture = Picture::new(&pixels, 2, 2).unwrap();
        let mut buffer = canvas(4, 4);
        let mut surface = Surface::new(&mut buffer, 4, 4).unwrap();
        draw_stretched(&mut surface, &picture, Rect::new(0, 0, 4, 4));
        let row = |surface: &Surface<'_>, y: i32| -> Vec<u32> {
            (0..4).map(|x| surface.get(x, y).unwrap()).collect()
        };
        assert_eq!(row(&surface, 0), [1, 1, 2, 2]);
        assert_eq!(row(&surface, 1), [1, 1, 2, 2]);
        assert_eq!(row(&surface, 3), [3, 3, 4, 4]);
    }
}
