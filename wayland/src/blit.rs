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
        Self::new(x, y, (right - u64::from(x)) as u32, (bottom - u64::from(y)) as u32)
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
