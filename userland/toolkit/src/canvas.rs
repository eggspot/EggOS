//! Drawing into a window's pixels: rectangles, the design's rounded rectangles and outlines
//! (insets per row, docs/ux/desktop-v0.3.md section 2), and text in Egg Sans 16. Every
//! coordinate is at scale 1; the canvas multiplies it by the window's scale, so at 2x every
//! number and stroke doubles and glyphs are pixel-doubled. Nothing is drawn outside the clip.

use crate::geometry::Rect;
use eggos_font::egg_sans::{self, HEIGHT as GLYPH_HEIGHT};

/// The corner of radius 6: how far each row from the outside is shortened at both ends.
pub const INSET_R6: [i32; 6] = [4, 2, 1, 1, 0, 0];
/// The corner of radius 4.
pub const INSET_R4: [i32; 4] = [2, 1, 0, 0];
/// The circle of diameter 16 (the radio button), top half.
pub const INSET_R8: [i32; 8] = [5, 3, 2, 1, 1, 0, 0, 0];
/// The circle of diameter 6 (the radio button's dot), top half.
pub const INSET_R3: [i32; 3] = [2, 1, 0];
/// Square corners.
pub const SQUARE: [i32; 0] = [];

const LONGEST_INSETS: usize = 12;

/// A rectangle with corners cut by insets per row, mirrored at the bottom.
#[derive(Debug, Clone, Copy)]
pub struct Shape<'a> {
    pub rect: Rect,
    pub insets: &'a [i32],
}

impl Shape<'_> {
    fn inset_at(&self, row: i32) -> i32 {
        if row < 0 || row >= self.rect.height {
            return i32::MAX / 4;
        }
        let from_bottom = self.rect.height - 1 - row;
        let nearest = row.min(from_bottom) as usize;
        self.insets.get(nearest).copied().unwrap_or(0)
    }

    fn span(&self, row: i32) -> Option<(i32, i32)> {
        let inset = self.inset_at(row - self.rect.y);
        let (left, right) = (self.rect.x + inset, self.rect.right() - inset);
        (row >= self.rect.y && row < self.rect.bottom() && left < right).then_some((left, right))
    }
}

fn shrunk_by_one_pixel(insets: &[i32], height: i32, into: &mut [i32; LONGEST_INSETS]) -> usize {
    let at = |row: i32| -> i32 {
        if row < 0 {
            i32::MAX / 4
        } else {
            let from_bottom = height - 1 - row;
            insets
                .get(row.min(from_bottom) as usize)
                .copied()
                .unwrap_or(0)
        }
    };
    let count = insets.len().saturating_sub(1).min(LONGEST_INSETS);
    for (row, inset) in into.iter_mut().enumerate().take(count) {
        let row = row as i32;
        *inset = at(row + 1).max(at(row) - 1).max(at(row + 2) - 1).max(0);
    }
    let mut trimmed = count;
    while trimmed > 0 && into[trimmed - 1] == 0 {
        trimmed -= 1;
    }
    trimmed
}

/// Pixels to draw into: `width` by `height`, rows `width` apart, at `scale`.
#[derive(Debug)]
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: i32,
    scale: i32,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`, `width` by `height` buffer pixels at `scale` (1 or 2), clipped
    /// to the whole buffer; `None` when `pixels` is too short for that size.
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32, scale: u32) -> Option<Self> {
        let fits = (width as usize)
            .checked_mul(height as usize)
            .is_some_and(|needed| needed <= pixels.len());
        (fits && scale >= 1).then_some(Self {
            pixels,
            width: width as i32,
            scale: scale as i32,
            clip: Rect::new(0, 0, width as i32, height as i32),
        })
    }

    /// The scale every coordinate is multiplied by.
    pub fn scale(&self) -> i32 {
        self.scale
    }

    /// The clip, in scale-1 coordinates (rounded outwards).
    pub fn clip(&self) -> Rect {
        let scale = self.scale;
        let x = self.clip.x.div_euclid(scale);
        let y = self.clip.y.div_euclid(scale);
        let right = (self.clip.right() + scale - 1).div_euclid(scale);
        let bottom = (self.clip.bottom() + scale - 1).div_euclid(scale);
        Rect::new(x, y, right - x, bottom - y)
    }

    /// Narrows the clip to `rect` (scale 1) and returns the clip it had, for [`Self::restore`].
    pub fn narrow(&mut self, rect: Rect) -> Rect {
        let previous = self.clip;
        self.clip = self.clip.intersection(&rect.scaled(self.scale));
        previous
    }

    /// Puts back a clip [`Self::narrow`] returned.
    pub fn restore(&mut self, clip: Rect) {
        self.clip = clip;
    }

    fn device_row(&mut self, row: i32, left: i32, right: i32, colour: u32) {
        if row < self.clip.y || row >= self.clip.bottom() {
            return;
        }
        let left = left.max(self.clip.x);
        let right = right.min(self.clip.right());
        if left >= right {
            return;
        }
        let start = row as usize * self.width as usize;
        self.pixels[start + left as usize..start + right as usize].fill(colour);
    }

    fn row(&mut self, row: i32, left: i32, right: i32, colour: u32) {
        let scale = self.scale;
        for line in 0..scale {
            self.device_row(row * scale + line, left * scale, right * scale, colour);
        }
    }

    /// Fills `rect`.
    pub fn fill(&mut self, rect: Rect, colour: u32) {
        let clip = self.clip();
        let rect = rect.intersection(&clip);
        for row in rect.y..rect.bottom() {
            self.row(row, rect.x, rect.right(), colour);
        }
    }

    /// Fills one pixel.
    pub fn pixel(&mut self, x: i32, y: i32, colour: u32) {
        self.row(y, x, x + 1, colour);
    }

    /// Fills `rect` with its corners cut by `insets`.
    pub fn rounded(&mut self, rect: Rect, insets: &[i32], colour: u32) {
        let shape = Shape { rect, insets };
        let clip = self.clip();
        for row in rect.y.max(clip.y)..rect.bottom().min(clip.bottom()) {
            if let Some((left, right)) = shape.span(row) {
                self.row(row, left, right, colour);
            }
        }
    }

    /// Draws the outline `width` pixels wide along the inside of `rect` with its corners cut
    /// by `insets`, leaving the inside as it is.
    pub fn outline(&mut self, rect: Rect, insets: &[i32], width: i32, colour: u32) {
        let mut inner_insets = [0i32; LONGEST_INSETS];
        let mut outer_insets = [0i32; LONGEST_INSETS];
        let outer_count = insets.len().min(LONGEST_INSETS);
        outer_insets[..outer_count].copy_from_slice(&insets[..outer_count]);
        let mut inner_count = outer_count;
        inner_insets[..inner_count].copy_from_slice(&outer_insets[..inner_count]);
        let mut inner_rect = rect;
        for _ in 0..width {
            let mut next = [0i32; LONGEST_INSETS];
            inner_count =
                shrunk_by_one_pixel(&inner_insets[..inner_count], inner_rect.height, &mut next);
            inner_insets = next;
            inner_rect = inner_rect.inset(1);
        }
        let outer = Shape {
            rect,
            insets: &outer_insets[..outer_count],
        };
        let inner = Shape {
            rect: inner_rect,
            insets: &inner_insets[..inner_count],
        };
        let clip = self.clip();
        for row in rect.y.max(clip.y)..rect.bottom().min(clip.bottom()) {
            let Some((left, right)) = outer.span(row) else {
                continue;
            };
            match inner.span(row) {
                Some((inner_left, inner_right)) => {
                    self.row(row, left, inner_left, colour);
                    self.row(row, inner_right, right, colour);
                }
                None => self.row(row, left, right, colour),
            }
        }
    }

    /// Draws `text` in Egg Sans with its cell's top-left at (`x`, `y`), every glyph pixel a
    /// `size` by `size` square (1 for body text, 2 for headings); returns the x after it.
    pub fn text(&mut self, x: i32, y: i32, text: &str, colour: u32, size: i32) -> i32 {
        let clip = self.clip();
        let mut pen = x;
        for character in text.chars() {
            let glyph = egg_sans::glyph(character);
            let advance = i32::from(glyph.advance) * size;
            let cell = Rect::new(pen, y, advance, GLYPH_HEIGHT as i32 * size);
            if !cell.intersection(&clip).is_empty() {
                self.glyph(pen, y, glyph, colour, size);
            }
            pen += advance;
        }
        pen
    }

    /// Draws glyph `id` of Egg Mono 8x16 (`eggos_font::glyph_id`) with its cell's top-left at
    /// (`x`, `y`): only its ink, over what is there.
    pub fn mono_glyph(&mut self, x: i32, y: i32, id: u8, colour: u32) {
        for (row, &bits) in eggos_font::rows(id).iter().enumerate() {
            let mut column = 0;
            while column < eggos_font::GLYPH_WIDTH {
                if bits & (0x80u8 >> column) == 0 {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < eggos_font::GLYPH_WIDTH && bits & (0x80u8 >> column) != 0 {
                    column += 1;
                }
                self.row(y + row as i32, x + start as i32, x + column as i32, colour);
            }
        }
    }

    fn glyph(&mut self, x: i32, y: i32, glyph: &egg_sans::Glyph, colour: u32, size: i32) {
        for row in 0..GLYPH_HEIGHT {
            let bits = glyph.rows[row];
            if bits == 0 {
                continue;
            }
            let mut column = 0;
            while column < usize::from(glyph.advance) {
                if !glyph.is_ink(column, row) {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < usize::from(glyph.advance) && glyph.is_ink(column, row) {
                    column += 1;
                }
                let top = y + row as i32 * size;
                for line in 0..size {
                    self.row(
                        top + line,
                        x + start as i32 * size,
                        x + column as i32 * size,
                        colour,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(pixels: &[u32], width: usize, colour: u32) -> Vec<String> {
        pixels
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|&pixel| if pixel == colour { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_rounded_rectangle_cuts_its_corners_by_the_insets() {
        let mut pixels = vec![0u32; 10 * 6];
        let mut canvas = Canvas::new(&mut pixels, 10, 6, 1).unwrap();
        canvas.rounded(Rect::new(0, 0, 10, 6), &INSET_R4, 1);
        assert_eq!(
            picture(&pixels, 10, 1),
            [
                "..######..",
                ".########.",
                "##########",
                "##########",
                ".########.",
                "..######..",
            ]
        );
    }

    #[test]
    fn a_mono_glyph_draws_its_ink_only_and_doubles_at_scale_2() {
        let mut pixels = vec![7u32; 8 * 16];
        let mut canvas = Canvas::new(&mut pixels, 8, 16, 1).unwrap();
        canvas.mono_glyph(0, 0, b'|', 1);
        for (row, &bits) in eggos_font::rows(b'|').iter().enumerate() {
            for column in 0..8 {
                let ink = bits & (0x80u8 >> column) != 0;
                let expected = if ink { 1 } else { 7 };
                assert_eq!(pixels[row * 8 + column], expected, "{row} {column}");
            }
        }
        let mut doubled = vec![0u32; 16 * 32];
        let mut canvas = Canvas::new(&mut doubled, 16, 32, 2).unwrap();
        canvas.mono_glyph(0, 0, b'|', 1);
        let ink = |pixels: &[u32]| pixels.iter().filter(|&&pixel| pixel == 1).count();
        assert_eq!(ink(&doubled), 4 * ink(&pixels));
    }

    #[test]
    fn an_outline_follows_the_insets_and_leaves_the_inside() {
        let mut pixels = vec![0u32; 12 * 12];
        let mut canvas = Canvas::new(&mut pixels, 12, 12, 1).unwrap();
        canvas.outline(Rect::new(0, 0, 12, 12), &INSET_R6, 1, 1);
        let rows = picture(&pixels, 12, 1);
        assert_eq!(rows[0], "....####....");
        assert_eq!(rows[1], "..##....##..");
        assert_eq!(rows[2], ".#........#.");
        assert_eq!(rows[5], "#..........#");
        assert_eq!(rows[11], "....####....");
    }

    #[test]
    fn the_eroded_insets_of_radius_6_are_the_desktop_s() {
        let mut inner = [0; LONGEST_INSETS];
        let count = shrunk_by_one_pixel(&INSET_R6, 40, &mut inner);
        assert_eq!(&inner[..count], &[3, 1, 1]);
    }

    #[test]
    fn at_scale_2_every_pixel_is_doubled_and_the_clip_holds() {
        let mut pixels = vec![0u32; 8 * 4];
        let mut canvas = Canvas::new(&mut pixels, 8, 4, 2).unwrap();
        let previous = canvas.narrow(Rect::new(0, 0, 3, 2));
        canvas.fill(Rect::new(1, 0, 10, 10), 5);
        canvas.restore(previous);
        assert_eq!(
            picture(&pixels, 8, 5),
            ["..####..", "..####..", "..####..", "..####.."]
        );
    }

    #[test]
    fn text_is_set_glyph_after_glyph_by_their_advances() {
        let mut pixels = vec![0u32; 20 * 16];
        let mut canvas = Canvas::new(&mut pixels, 20, 16, 1).unwrap();
        let end = canvas.text(1, 0, "Ai", 7, 1);
        assert_eq!(end, 1 + 6 + 2);
        let rows = picture(&pixels, 20, 7);
        assert_eq!(&rows[3][..9], "...#...#.");
        assert_eq!(&rows[8][..7], ".#####.");
        let mut doubled = vec![0u32; 40 * 32];
        let mut canvas = Canvas::new(&mut doubled, 40, 32, 1).unwrap();
        canvas.text(0, 0, "A", 7, 2);
        let rows = picture(&doubled, 40, 7);
        assert_eq!(&rows[6][..12], "....##......");
        assert_eq!(rows[6], rows[7]);
    }
}
