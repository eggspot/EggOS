//! Rectangles in a window's coordinates: (0, 0) at the content's top-left, at scale 1.

/// A rectangle: its top-left corner and its size; empty when either side is zero or less.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// The rectangle at (`x`, `y`), `width` by `height`.
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The column just past its right edge.
    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    /// The row just past its bottom edge.
    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    /// Whether it holds no pixel.
    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// Whether the pixel at (`x`, `y`) is in it.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The pixels in both; empty when they do not meet.
    pub fn intersection(&self, other: &Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }

    /// The smallest rectangle holding both; the other one when one is empty.
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
        Self::new(x, y, right - x, bottom - y)
    }

    /// Shrunk by `by` on every side.
    pub const fn inset(&self, by: i32) -> Self {
        Self::new(
            self.x + by,
            self.y + by,
            self.width - 2 * by,
            self.height - 2 * by,
        )
    }

    /// Moved by (`dx`, `dy`).
    pub const fn offset(&self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Every number multiplied by `scale`: the rectangle in a buffer's pixels.
    pub const fn scaled(&self, scale: i32) -> Self {
        Self::new(
            self.x * scale,
            self.y * scale,
            self.width * scale,
            self.height * scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_union_and_containment() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersection(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.intersection(&Rect::new(20, 0, 5, 5)).is_empty());
        assert_eq!(Rect::default().union(&b), b);
        assert!(a.contains(9, 9) && !a.contains(10, 9) && !a.contains(-1, 0));
        assert_eq!(a.inset(2), Rect::new(2, 2, 6, 6));
        assert_eq!(a.offset(3, -1).scaled(2), Rect::new(6, -2, 20, 20));
    }
}
