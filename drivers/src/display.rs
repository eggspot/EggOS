//! What display drivers share: the framebuffer's mode, rectangles of pixels and the damage a
//! frame collects before it is flushed to the screen.
//!
//! A pixel is a `u32` `0x00RRGGBB` in the CPU's byte order, which on every little-endian
//! machine is the bytes `B, G, R, X` of virtio-gpu's `B8G8R8X8_UNORM`.

/// Bytes per pixel of a framebuffer.
pub const BYTES_PER_PIXEL: u32 = 4;

/// A display mode: the framebuffer's size in pixels, rows packed with no padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

impl Mode {
    /// `width` by `height` pixels.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Bytes from the start of one row to the next.
    pub const fn stride(self) -> usize {
        self.width as usize * BYTES_PER_PIXEL as usize
    }

    /// Bytes of the whole framebuffer.
    pub const fn bytes(self) -> usize {
        self.stride() * self.height as usize
    }

    /// The whole screen.
    pub const fn rect(self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

/// A rectangle of pixels: `width` columns from `x`, `height` rows from `y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// The rectangle at (`x`, `y`) of `width` by `height` pixels.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether it holds no pixel.
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The column after its last, saturating.
    pub const fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    /// The row after its last, saturating.
    pub const fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    /// How many pixels it holds.
    pub const fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// The smallest rectangle holding both; an empty one adds nothing.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Whether the two share a pixel.
    pub fn overlaps(&self, other: &Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The part of it inside a `width` by `height` screen; empty if none is.
    pub fn clipped(self, width: u32, height: u32) -> Rect {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Rect::new(
            x,
            y,
            self.right().min(width) - x,
            self.bottom().min(height) - y,
        )
    }
}

/// Most rectangles one [`Damage`] keeps apart (the desktop design's eight dirty rects per frame).
pub const MAX_DAMAGE_RECTS: usize = 8;

/// The parts of the screen drawn since the last flush, as at most [`MAX_DAMAGE_RECTS`]
/// rectangles that do not overlap: a rectangle that overlaps one already kept is merged into
/// it, and one more than the limit is merged into the rectangle whose union grows least.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Damage {
    rects: [Rect; MAX_DAMAGE_RECTS],
    len: usize,
}

impl Damage {
    /// No damage.
    pub const fn new() -> Self {
        Self {
            rects: [Rect::new(0, 0, 0, 0); MAX_DAMAGE_RECTS],
            len: 0,
        }
    }

    /// The damaged rectangles, none overlapping another.
    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.len]
    }

    /// Whether nothing is damaged.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Forgets all damage: what a flush does once the screen shows it.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Adds `rect`; an empty one changes nothing.
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        let mut merged = rect;
        let mut index = 0;
        while index < self.len {
            if self.rects[index].overlaps(&merged) {
                merged = merged.union(self.remove(index));
                index = 0;
            } else {
                index += 1;
            }
        }
        if self.len < MAX_DAMAGE_RECTS {
            self.rects[self.len] = merged;
            self.len += 1;
            return;
        }
        let cheapest = (0..self.len)
            .min_by_key(|&index| {
                let kept = self.rects[index];
                kept.union(merged).area() - kept.area()
            })
            .unwrap_or(0);
        let grown = self.remove(cheapest).union(merged);
        self.add(grown);
    }

    fn remove(&mut self, index: usize) -> Rect {
        let rect = self.rects[index];
        self.len -= 1;
        self.rects[index] = self.rects[self.len];
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_and_overlap_follow_the_pixels_held() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(10, 0, 5, 5);
        assert!(!a.overlaps(&b));
        assert!(a.overlaps(&Rect::new(9, 9, 1, 1)));
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 10));
        assert_eq!(a.union(Rect::default()), a);
        assert_eq!(Rect::default().union(b), b);
        assert!(!a.overlaps(&Rect::new(5, 5, 0, 3)));
    }

    #[test]
    fn clipping_keeps_only_the_screen_s_part() {
        assert_eq!(
            Rect::new(1270, 790, 20, 20).clipped(1280, 800),
            Rect::new(1270, 790, 10, 10)
        );
        assert!(Rect::new(2000, 0, 5, 5).clipped(1280, 800).is_empty());
        assert_eq!(
            Rect::new(u32::MAX - 1, 0, u32::MAX, 1).clipped(1280, 800),
            Rect::new(1280, 0, 0, 1)
        );
    }

    #[test]
    fn overlapping_damage_is_merged_and_apart_damage_kept_apart() {
        let mut damage = Damage::new();
        damage.add(Rect::new(0, 0, 4, 4));
        damage.add(Rect::new(100, 100, 4, 4));
        damage.add(Rect::new(2, 2, 4, 4));
        assert_eq!(damage.rects().len(), 2);
        assert!(damage.rects().contains(&Rect::new(0, 0, 6, 6)));
        damage.add(Rect::new(5, 5, 96, 96));
        assert_eq!(damage.rects(), &[Rect::new(0, 0, 104, 104)]);
        damage.add(Rect::new(1, 1, 0, 9));
        assert_eq!(damage.rects().len(), 1);
        damage.clear();
        assert!(damage.is_empty());
    }

    #[test]
    fn a_ninth_rect_merges_where_the_union_grows_least() {
        let mut damage = Damage::new();
        for index in 0..MAX_DAMAGE_RECTS as u32 {
            damage.add(Rect::new(index * 100, 0, 10, 10));
        }
        damage.add(Rect::new(712, 0, 4, 10));
        assert_eq!(damage.rects().len(), MAX_DAMAGE_RECTS);
        assert!(damage.rects().contains(&Rect::new(700, 0, 16, 10)));
        for (index, rect) in damage.rects().iter().enumerate() {
            for other in &damage.rects()[index + 1..] {
                assert!(!rect.overlaps(other));
            }
        }
    }

    #[test]
    fn a_merge_that_reaches_other_rects_takes_them_in() {
        let mut damage = Damage::new();
        for index in 0..MAX_DAMAGE_RECTS as u32 {
            damage.add(Rect::new(index * 10, 0, 5, 5));
        }
        damage.add(Rect::new(0, 0, 80, 1));
        assert_eq!(damage.rects(), &[Rect::new(0, 0, 80, 5)]);
    }
}
