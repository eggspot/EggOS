//! Copying a client's pixels into a window (D140): rows of 32-bit pixels, the alpha byte or
//! the unused byte of `ARGB8888` and `XRGB8888` cleared, since a window's pixels are
//! `0x00RRGGBB` and EggOS shows Wayland surfaces opaque.

/// What keeps a pixel's colour and clears its top byte.
pub const OPAQUE_MASK: u32 = 0x00ff_ffff;

/// A rectangle of pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// The rectangle at (`x`, `y`) of `width` by `height`.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle a client names with signed numbers, clipped to the positive quarter and
    /// to `u32`; empty when it has no area there.
    pub fn from_signed(x: i32, y: i32, width: i32, height: i32) -> Self {
        let right = i64::from(x).saturating_add(i64::from(width));
        let bottom = i64::from(y).saturating_add(i64::from(height));
        let (left, top) = (i64::from(x).max(0), i64::from(y).max(0));
        if right <= left || bottom <= top {
            return Self::default();
        }
        let clamp = |value: i64| value.min(i64::from(u32::MAX)) as u32;
        Self::new(
            clamp(left),
            clamp(top),
            clamp(right - left),
            clamp(bottom - top),
        )
    }

    /// Whether it has no pixels.
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Its right edge, one past its last column.
    pub const fn right(&self) -> u64 {
        self.x as u64 + self.width as u64
    }

    /// Its bottom edge, one past its last row.
    pub const fn bottom(&self) -> u64 {
        self.y as u64 + self.height as u64
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(
            x,
            y,
            (right - u64::from(x)) as u32,
            (bottom - u64::from(y)) as u32,
        )
    }

    /// The part of it inside a `width` by `height` area at the origin.
    pub fn clipped(&self, width: u32, height: u32) -> Self {
        let right = self.right().min(u64::from(width));
        let bottom = self.bottom().min(u64::from(height));
        if right <= u64::from(self.x) || bottom <= u64::from(self.y) {
            return Self::default();
        }
        Self::new(
            self.x,
            self.y,
            (right - u64::from(self.x)) as u32,
            (bottom - u64::from(self.y)) as u32,
        )
    }
}

/// Copies `rect` from `source`, rows of `source_stride` pixels, to the same place of
/// `destination`, rows of `destination_width` pixels, clearing each pixel's top byte; the parts
/// of `rect` outside either are left out. The same work per pixel as the server's copy from a
/// client's memory: one load, one mask, one store.
pub fn copy_opaque_rows(
    source: &[u32],
    source_stride: usize,
    destination: &mut [u32],
    destination_width: usize,
    rect: Rect,
) {
    let (x, width) = (rect.x as usize, rect.width as usize);
    for row in rect.y as usize..rect.bottom() as usize {
        let from = row.saturating_mul(source_stride).saturating_add(x);
        let to = row.saturating_mul(destination_width).saturating_add(x);
        let source_row = source.get(from..).unwrap_or(&[]);
        let destination_row = destination.get_mut(to..).unwrap_or(&mut []);
        let fits = width
            .min(source_stride.saturating_sub(x))
            .min(destination_width.saturating_sub(x));
        for (out, pixel) in destination_row.iter_mut().zip(source_row).take(fits) {
            *out = pixel & OPAQUE_MASK;
        }
    }
}

/// Where a surface's pixels land in a window: `source`, a rectangle of its buffer, scaled to
/// `size` with its top-left at `at` in the window (negative when it starts left of or above
/// the window), only `clip` of the window written; `blend` draws `ARGB8888` pixels over what
/// the window holds, by their premultiplied alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    pub source: Rect,
    pub at: (i32, i32),
    pub size: (u32, u32),
    pub clip: Rect,
    pub blend: bool,
}

impl Placement {
    /// The window's pixels it writes: its rectangle inside `clip`; empty when it writes none.
    pub fn written(&self) -> Rect {
        let left = i64::from(self.at.0).max(i64::from(self.clip.x));
        let top = i64::from(self.at.1).max(i64::from(self.clip.y));
        let right = (i64::from(self.at.0) + i64::from(self.size.0)).min(self.clip.right() as i64);
        let bottom = (i64::from(self.at.1) + i64::from(self.size.1)).min(self.clip.bottom() as i64);
        if right <= left || bottom <= top || self.source.is_empty() {
            return Rect::default();
        }
        Rect::new(
            left as u32,
            top as u32,
            (right - left) as u32,
            (bottom - top) as u32,
        )
    }

    /// The buffer pixel the window's pixel (`x`, `y`) inside [`Self::written`] shows: the
    /// nearest one, so a scale of 1 copies pixel for pixel.
    pub fn source_of(&self, x: u32, y: u32) -> (u32, u32) {
        let along = |at: i32, value: u32, start: u32, length: u32, size: u32| -> u32 {
            let offset = (i64::from(value) - i64::from(at)).max(0) as u64;
            let scaled = offset * u64::from(length) / u64::from(size.max(1));
            start + scaled.min(u64::from(length.saturating_sub(1))) as u32
        };
        (
            along(self.at.0, x, self.source.x, self.source.width, self.size.0),
            along(self.at.1, y, self.source.y, self.source.height, self.size.1),
        )
    }

    /// Writes every pixel of [`Self::written`] into `window`: `read` gives a buffer pixel
    /// (`None` outside the client's memory, which leaves that window pixel alone).
    pub fn draw(
        &self,
        mut read: impl FnMut(u32, u32) -> Option<u32>,
        window: &mut impl WindowPixels,
    ) {
        let written = self.written();
        let unscaled = self.size == (self.source.width, self.source.height);
        let shift = (
            i64::from(self.source.x) - i64::from(self.at.0),
            i64::from(self.source.y) - i64::from(self.at.1),
        );
        for y in written.y..written.bottom() as u32 {
            for x in written.x..written.right() as u32 {
                let (column, row) = match unscaled {
                    true => (
                        (i64::from(x) + shift.0) as u32,
                        (i64::from(y) + shift.1) as u32,
                    ),
                    false => self.source_of(x, y),
                };
                let Some(pixel) = read(column, row) else {
                    continue;
                };
                let value = match self.blend {
                    true => blend_over(pixel, window.get(x, y)),
                    false => pixel & OPAQUE_MASK,
                };
                window.set(x, y, value);
            }
        }
    }
}

/// A window's pixels, as [`Placement::draw`] reads and writes them.
pub trait WindowPixels {
    /// The pixel at (`x`, `y`), inside the window.
    fn get(&mut self, x: u32, y: u32) -> u32;
    /// Sets the pixel at (`x`, `y`), inside the window.
    fn set(&mut self, x: u32, y: u32, pixel: u32);
}

/// `source`, an `ARGB8888` pixel with premultiplied alpha, drawn over the opaque
/// `destination`; the result is opaque.
pub fn blend_over(source: u32, destination: u32) -> u32 {
    let alpha = source >> 24;
    if alpha == 0xff {
        return source & OPAQUE_MASK;
    }
    let keep = 255 - alpha;
    let channel = |shift: u32| -> u32 {
        let top = (source >> shift) & 0xff;
        let under = (destination >> shift) & 0xff;
        (top + (under * keep + 127) / 255).min(255) << shift
    };
    channel(16) | channel(8) | channel(0)
}
