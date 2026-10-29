//! A window's pixels as drawing sees them: Win32's `RECT`, and a surface that clips every write
//! and remembers the rectangle it changed, so the layer commits only that.

/// A rectangle as Win32's `RECT`: `left` and `top` inside it, `right` and `bottom` just
/// outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The rectangle of `width` by `height` pixels whose top-left pixel is (`x`, `y`).
    pub const fn sized(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self::new(x, y, x.saturating_add(width), y.saturating_add(height))
    }

    pub const fn width(&self) -> i32 {
        self.right.saturating_sub(self.left)
    }

    pub const fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }

    /// Whether it holds no pixel.
    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    /// The pixels in both; empty when they do not overlap.
    pub fn intersection(&self, other: &Rect) -> Rect {
        Rect::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        )
    }

    /// The smallest rectangle holding both; an empty one adds nothing.
    pub fn union(&self, other: &Rect) -> Rect {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => *other,
            (_, true) => *self,
            _ => Rect::new(
                self.left.min(other.left),
                self.top.min(other.top),
                self.right.max(other.right),
                self.bottom.max(other.bottom),
            ),
        }
    }

    /// The rectangle `by` pixels smaller on each side (larger for a negative `by`).
    pub const fn inset(&self, by: i32) -> Rect {
        Rect::new(
            self.left.saturating_add(by),
            self.top.saturating_add(by),
            self.right.saturating_sub(by),
            self.bottom.saturating_sub(by),
        )
    }

    /// The same rectangle with `left <= right` and `top <= bottom`, as GDI's shapes take it.
    pub fn normalised(&self) -> Rect {
        Rect::new(
            self.left.min(self.right),
            self.top.min(self.bottom),
            self.left.max(self.right),
            self.top.max(self.bottom),
        )
    }

    /// The same rectangle moved by (`dx`, `dy`).
    pub const fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(
            self.left.saturating_add(dx),
            self.top.saturating_add(dy),
            self.right.saturating_add(dx),
            self.bottom.saturating_add(dy),
        )
    }

    /// Whether the pixel (`x`, `y`) is inside.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// A window's pixels (`0x00RRGGBB`, rows of `width` with no padding), the rectangle drawing may
/// change and the rectangle it changed, both in the coordinates drawing uses: the buffer's own,
/// moved by [`Surface::translate`] (a child window draws in its own coordinates).
#[derive(Debug)]
pub struct Surface<'a> {
    pixels: &'a mut [u32],
    width: u32,
    height: u32,
    clip: Rect,
    drawn: Rect,
    origin: (i32, i32),
}

impl<'a> Surface<'a> {
    /// The surface of `width` by `height` pixels in `pixels`, clipped to itself; `None` when
    /// `pixels` is shorter.
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32) -> Option<Self> {
        let needed = (width as usize).checked_mul(height as usize)?;
        let fits = pixels.len() >= needed && width <= i32::MAX as u32 && height <= i32::MAX as u32;
        fits.then_some(Self {
            pixels,
            width,
            height,
            clip: Rect::new(0, 0, width as i32, height as i32),
            drawn: Rect::default(),
            origin: (0, 0),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Moves the origin to (`dx`, `dy`) of the current coordinates: after it, that point is
    /// (0, 0), and the clip and the rectangle drawn so far follow.
    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.origin = (
            self.origin.0.saturating_add(dx),
            self.origin.1.saturating_add(dy),
        );
        self.clip = self.clip.offset(-dx, -dy);
        if !self.drawn.is_empty() {
            self.drawn = self.drawn.offset(-dx, -dy);
        }
    }

    /// Where (0, 0) is in the buffer.
    pub fn origin(&self) -> (i32, i32) {
        self.origin
    }

    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
            .offset(-self.origin.0, -self.origin.1)
    }

    /// Limits drawing to `rect` as well.
    pub fn clip_to(&mut self, rect: Rect) {
        self.clip = self.clip.intersection(&rect);
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Limits drawing to `rect` only, within the surface.
    pub fn set_clip(&mut self, rect: Rect) {
        self.clip = self.bounds().intersection(&rect);
    }

    /// The smallest rectangle holding every pixel drawn so far.
    pub fn drawn(&self) -> Rect {
        self.drawn
    }

    /// The pixel at (`x`, `y`), if it is on the surface.
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        let (x, y) = (x.checked_add(self.origin.0)?, y.checked_add(self.origin.1)?);
        let inside = x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height;
        inside.then(|| self.pixels[y as usize * self.width as usize + x as usize])
    }

    /// Sets pixels `left..right` of row `y` to `colour`, where the clip allows.
    pub fn span(&mut self, y: i32, left: i32, right: i32, colour: u32) {
        if y < self.clip.top || y >= self.clip.bottom {
            return;
        }
        let left = left.max(self.clip.left);
        let right = right.min(self.clip.right);
        if left >= right {
            return;
        }
        let start = (y + self.origin.1) as usize * self.width as usize;
        let (from, to) = (left + self.origin.0, right + self.origin.0);
        self.pixels[start + from as usize..start + to as usize].fill(colour);
        self.drawn = self.drawn.union(&Rect::new(left, y, right, y + 1));
    }

    pub fn pixel(&mut self, x: i32, y: i32, colour: u32) {
        self.span(y, x, x.saturating_add(1), colour);
    }

    pub fn fill(&mut self, rect: Rect, colour: u32) {
        let rows = rect.intersection(&self.clip);
        for y in rows.top..rows.bottom {
            self.span(y, rect.left, rect.right, colour);
        }
    }

    /// Fills `rect` with its four corners cut by `insets` (row `i` from the top and from the
    /// bottom starts `insets[i]` pixels in on each side), as the desktop's rounded shapes are.
    pub fn rounded(&mut self, rect: Rect, insets: &[i32], colour: u32) {
        let height = rect.height();
        for offset in 0..height.max(0) {
            let from_bottom = height - 1 - offset;
            let inset = match () {
                () if (offset as usize) < insets.len() => insets[offset as usize],
                () if (from_bottom as usize) < insets.len() => insets[from_bottom as usize],
                () => 0,
            };
            self.span(
                rect.top + offset,
                rect.left + inset,
                rect.right - inset,
                colour,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_intersect_and_unite_with_empty_ones_adding_nothing() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 20, 8);
        assert_eq!(a.intersection(&b), Rect::new(5, 5, 10, 8));
        assert_eq!(a.union(&b), Rect::new(0, 0, 20, 10));
        assert!(Rect::new(3, 3, 3, 9).is_empty());
        assert_eq!(Rect::default().union(&b), b);
        assert_eq!(b.union(&Rect::new(50, 50, 40, 60)), b);
        assert_eq!(Rect::new(9, 8, 1, 2).normalised(), Rect::new(1, 2, 9, 8));
        assert_eq!(Rect::sized(2, 3, 4, 5), Rect::new(2, 3, 6, 8));
    }

    #[test]
    fn drawing_is_clipped_and_records_what_it_changed() {
        let mut pixels = [0u32; 8 * 4];
        let mut surface = Surface::new(&mut pixels, 8, 4).unwrap();
        surface.clip_to(Rect::new(2, 1, 6, 3));
        surface.fill(Rect::new(-5, -5, 50, 50), 7);
        surface.pixel(0, 0, 9);
        assert_eq!(surface.drawn(), Rect::new(2, 1, 6, 3));
        assert_eq!(surface.get(1, 1), Some(0));
        assert_eq!(surface.get(2, 1), Some(7));
        assert_eq!(surface.get(5, 2), Some(7));
        assert_eq!(surface.get(6, 2), Some(0));
        assert_eq!(surface.get(8, 0), None);
        assert!(Surface::new(&mut [0u32; 3], 2, 2).is_none());
    }

    #[test]
    fn a_translated_surface_draws_in_its_own_coordinates_inside_the_buffer() {
        let mut pixels = [0u32; 8 * 6];
        let mut surface = Surface::new(&mut pixels, 8, 6).unwrap();
        surface.clip_to(Rect::new(2, 1, 7, 5));
        surface.translate(3, 2);
        assert_eq!(surface.clip(), Rect::new(-1, -1, 4, 3));
        assert_eq!(surface.origin(), (3, 2));
        surface.fill(Rect::new(0, 0, 10, 10), 5);
        assert_eq!(surface.drawn(), Rect::new(0, 0, 4, 3));
        assert_eq!(surface.get(0, 0), Some(5));
        assert_eq!(surface.get(-3, -2), Some(0));
        surface.set_clip(Rect::new(-10, -10, 100, 100));
        assert_eq!(surface.clip(), Rect::new(-3, -2, 5, 4));
        surface.translate(-3, -2);
        assert_eq!(surface.drawn(), Rect::new(3, 2, 7, 5));
        assert_eq!(surface.get(6, 4), Some(5));
        assert_eq!(surface.get(7, 4), Some(0));
        assert_eq!(surface.get(2, 2), Some(0));
    }

    #[test]
    fn rounded_corners_cut_the_same_rows_at_the_top_and_bottom() {
        let mut pixels = [0u32; 6 * 6];
        let mut surface = Surface::new(&mut pixels, 6, 6).unwrap();
        surface.rounded(Rect::new(0, 0, 6, 6), &[2, 1], 1);
        let row =
            |y: i32| -> [u32; 6] { core::array::from_fn(|x| surface.get(x as i32, y).unwrap()) };
        assert_eq!(row(0), [0, 0, 1, 1, 0, 0]);
        assert_eq!(row(1), [0, 1, 1, 1, 1, 0]);
        assert_eq!(row(2), [1; 6]);
        assert_eq!(row(4), [0, 1, 1, 1, 1, 0]);
        assert_eq!(row(5), [0, 0, 1, 1, 0, 0]);
    }
}
