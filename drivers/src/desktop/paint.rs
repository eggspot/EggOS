use crate::display::Rect;
use crate::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

pub(super) const INSET_R6: [u32; 6] = [4, 2, 1, 1, 0, 0];
pub(super) const INSET_INSIDE_R6: [u32; 5] = [3, 1, 1, 0, 0];
pub(super) const INSET_R4: [u32; 4] = [2, 1, 0, 0];
pub(super) const INSET_INSIDE_R4: [u32; 1] = [1];

const SHADOW_DARKENING: [fn(u32) -> u32; 4] = [half, quarter, quarter, eighth];

fn half(pixel: u32) -> u32 {
    (pixel >> 1) & 0x007f_7f7f
}

fn quarter(pixel: u32) -> u32 {
    pixel - ((pixel >> 2) & 0x003f_3f3f)
}

fn eighth(pixel: u32) -> u32 {
    pixel - ((pixel >> 3) & 0x001f_1f1f)
}

pub(super) fn isqrt(value: u32) -> u32 {
    let (mut low, mut high) = (0u32, value.min(65_535) + 1);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if middle * middle <= value {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

pub(super) fn egg_half_width(height: u32, row: u32) -> u32 {
    if row >= height {
        return 0;
    }
    let width = ((72 * u64::from(height) + 50) / 100) as u32;
    let equator = ((58 * u64::from(height) + 50) / 100).max(1) as u32;
    let t = if row < equator {
        (equator - row) * 256 / equator
    } else {
        (row + 1 - equator) * 256 / (height - equator).max(1)
    }
    .min(256);
    (width / 2) * isqrt(65_536 - t * t) / 256
}

pub(super) struct Canvas<'a> {
    pixels: &'a mut [u32],
    stride: usize,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    pub(super) fn new(pixels: &'a mut [u32], stride: usize, clip: Rect) -> Option<Self> {
        let fits = clip.right() as usize <= stride
            && (clip.bottom() as usize).saturating_mul(stride) <= pixels.len();
        (fits && !clip.is_empty()).then_some(Self {
            pixels,
            stride,
            clip,
        })
    }

    pub(super) fn clip(&self) -> Rect {
        self.clip
    }

    pub(super) fn row(&mut self, row: u32, left: u32, right: u32, colour: u32) {
        if row < self.clip.y || row >= self.clip.bottom() {
            return;
        }
        let left = left.max(self.clip.x);
        let right = right.min(self.clip.right());
        if left >= right {
            return;
        }
        let start = row as usize * self.stride;
        self.pixels[start + left as usize..start + right as usize].fill(colour);
    }

    pub(super) fn row_pixels(&mut self, row: u32, left: u32, right: u32) -> Option<&mut [u32]> {
        if row < self.clip.y || row >= self.clip.bottom() {
            return None;
        }
        let left = left.max(self.clip.x);
        let right = right.min(self.clip.right());
        if left >= right {
            return None;
        }
        let start = row as usize * self.stride;
        Some(&mut self.pixels[start + left as usize..start + right as usize])
    }

    pub(super) fn fill(&mut self, rect: Rect, colour: u32) {
        for row in rect.y..rect.bottom() {
            self.row(row, rect.x, rect.right(), colour);
        }
    }

    pub(super) fn pixel(&mut self, x: u32, y: u32, colour: u32) {
        self.row(y, x, x + 1, colour);
    }

    pub(super) fn rounded(
        &mut self,
        rect: Rect,
        insets: &[u32],
        top: bool,
        bottom: bool,
        colour: u32,
    ) {
        let first = self.clip.y.saturating_sub(rect.y).min(rect.height);
        let end = self.clip.bottom().saturating_sub(rect.y).min(rect.height);
        for offset in first..end {
            let from_bottom = rect.height - 1 - offset;
            let inset = match () {
                () if top && (offset as usize) < insets.len() => insets[offset as usize],
                () if bottom && (from_bottom as usize) < insets.len() => {
                    insets[from_bottom as usize]
                }
                () => 0,
            };
            self.row(
                rect.y + offset,
                rect.x + inset,
                rect.right().saturating_sub(inset),
                colour,
            );
        }
    }

    fn darken(&mut self, x: u32, y: u32, darkening: fn(u32) -> u32) {
        let clip = self.clip;
        if x >= clip.x && x < clip.right() && y >= clip.y && y < clip.bottom() {
            let at = y as usize * self.stride + x as usize;
            self.pixels[at] = darkening(self.pixels[at]);
        }
    }

    pub(super) fn shadow(&mut self, rect: Rect) {
        for (ring, darkening) in (1..).zip(SHADOW_DARKENING) {
            let top = rect.y + 2u32.saturating_sub(ring);
            let bottom = rect.bottom() - 1 + ring;
            let left = rect.x.checked_sub(ring);
            let right = rect.right() - 1 + ring;
            let clip = self.clip;
            for y in top.max(clip.y)..=bottom.min(clip.bottom() - 1) {
                if let Some(left) = left {
                    self.darken(left, y, darkening);
                }
                self.darken(right, y, darkening);
            }
            if bottom >= clip.y && bottom < clip.bottom() {
                for x in rect.x.saturating_sub(ring - 1).max(clip.x)..right.min(clip.right()) {
                    self.darken(x, bottom, darkening);
                }
            }
        }
    }

    pub(super) fn text(&mut self, x: u32, y: u32, text: &str, colour: u32) {
        for (index, character) in text.chars().enumerate() {
            self.glyph(
                x + index as u32 * GLYPH_WIDTH as u32,
                y,
                font::glyph_id(u32::from(character)),
                colour,
            );
        }
    }

    pub(super) fn circle(&mut self, centre_x: u32, centre_y: u32, radius: u32, colour: u32) {
        let diameter_squared = 4 * radius * radius;
        for row in centre_y.saturating_sub(radius)..centre_y + radius {
            let from_centre = (2 * row + 1).abs_diff(2 * centre_y);
            let squared = from_centre * from_centre;
            if squared >= diameter_squared {
                continue;
            }
            let half = isqrt(diameter_squared - squared);
            let left = (2 * centre_x).saturating_sub(half).div_ceil(2);
            let right = (2 * centre_x + half).div_ceil(2);
            self.row(row, left, right, colour);
        }
    }

    pub(super) fn glyph_doubled(&mut self, x: u32, y: u32, id: u8, colour: u32) {
        let cell = Rect::new(x, y, 2 * GLYPH_WIDTH as u32, 2 * GLYPH_HEIGHT as u32);
        if !cell.overlaps(&self.clip) || id == b' ' {
            return;
        }
        for (row, &bits) in font::rows(id).iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                if bits & (0x80 >> column) != 0 {
                    let (left, top) = (x + 2 * column as u32, y + 2 * row as u32);
                    self.row(top, left, left + 2, colour);
                    self.row(top + 1, left, left + 2, colour);
                }
            }
        }
    }

    pub(super) fn glyph_at(&mut self, x: u32, y: u32, id: u8, colour: u32) {
        self.glyph(x, y, id, colour);
    }

    fn glyph(&mut self, x: u32, y: u32, id: u8, colour: u32) {
        let cell = Rect::new(x, y, GLYPH_WIDTH as u32, GLYPH_HEIGHT as u32);
        if !cell.overlaps(&self.clip) || id == b' ' {
            return;
        }
        for (row, &bits) in font::rows(id).iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                if bits & (0x80 >> column) != 0 {
                    self.pixel(x + column as u32, y + row as u32, colour);
                }
            }
        }
    }
}
