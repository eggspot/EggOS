//! AppKit's geometry (`NSPoint`, `NSSize`, `NSRect`, all `CGFloat`, that is `f64`) and how it
//! lands on a window's pixels (`docs/ux/macos-gui-v0.3.md` section 0): the origin is the
//! content's bottom-left and y grows upward, so a frame `(x, y, w, h)` in a content `H` tall
//! covers rows `H - y - h .. H - y - 1`, and a point `(x, y)` is the pixel `(x, H - 1 - y)`.

use eggos_toolkit::geometry::Rect;

/// `NSPoint`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// `NSSize`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// `NSRect`: its bottom-left corner and its size, laid out as the ABI passes it (four
/// doubles).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The same size at `(0, 0)`: a view's bounds.
    pub const fn at_origin(&self) -> Self {
        Self::new(0.0, 0.0, self.width, self.height)
    }

    /// Moved by `(dx, dy)`.
    pub fn offset(&self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Whether `point` is inside, as `NSPointInRect` decides for an unflipped view: left and
    /// bottom edges in, right and top edges out.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }
}

fn floor(value: f64) -> i64 {
    if value.is_nan() {
        return 0;
    }
    let truncated = value as i64;
    if (truncated as f64) > value {
        truncated - 1
    } else {
        truncated
    }
}

/// `value` rounded to the nearest whole pixel, halves up, clamped to `i32`.
pub fn round(value: f64) -> i32 {
    floor(value + 0.5).clamp(i64::from(i32::MIN / 2), i64::from(i32::MAX / 2)) as i32
}

/// The pixels `frame` covers in a content `height` pixels tall.
pub fn pixel_rect(frame: Frame, height: i32) -> Rect {
    let left = round(frame.x);
    let right = round(frame.x + frame.width);
    let top = height - round(frame.y + frame.height);
    let bottom = height - round(frame.y);
    Rect::new(left, top, right - left, bottom - top)
}

/// The point at the centre of content pixel `(x, y)`'s row, as AppKit reports a click there.
pub fn point_of_pixel(x: i32, y: i32, height: i32) -> Point {
    Point::new(f64::from(x), f64::from(height - 1 - y))
}

/// The content pixel `point` lands on.
pub fn pixel_of_point(point: Point, height: i32) -> (i32, i32) {
    (round(point.x), height - 1 - round(point.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_count_rows_from_the_bottom() {
        assert_eq!(
            pixel_rect(Frame::new(0.0, 48.0, 400.0, 252.0), 300),
            Rect::new(0, 0, 400, 252)
        );
        assert_eq!(
            pixel_rect(Frame::new(8.0, 10.0, 184.0, 28.0), 300),
            Rect::new(8, 262, 184, 28)
        );
        assert!(pixel_rect(Frame::new(5.0, 5.0, -3.0, 2.0), 100).is_empty());
    }

    #[test]
    fn points_and_pixels_round_trip_and_agree_with_frames() {
        let point = point_of_pixel(100, 100, 300);
        assert_eq!(point, Point::new(100.0, 199.0));
        assert_eq!(pixel_of_point(point, 300), (100, 100));
        let frame = Frame::new(96.0, 195.0, 8.0, 8.0);
        let rect = pixel_rect(frame, 300);
        assert!(rect.contains(100, 100));
        assert!(frame.contains(point));
        assert!(!frame.contains(Point::new(104.0, 199.0)));
    }

    #[test]
    fn rounding_takes_halves_up_and_survives_nonsense() {
        assert_eq!(round(1.5), 2);
        assert_eq!(round(-1.5), -1);
        assert_eq!(round(-1.6), -2);
        assert_eq!(round(2.49), 2);
        assert_eq!(round(f64::NAN), 0);
        assert_eq!(round(1e300), i32::MAX / 2);
    }

    #[test]
    fn the_abi_layouts_are_doubles_back_to_back() {
        assert_eq!(core::mem::size_of::<Frame>(), 32);
        assert_eq!(core::mem::size_of::<Point>(), 16);
        assert_eq!(core::mem::size_of::<Size>(), 16);
        assert_eq!(core::mem::offset_of!(Frame, height), 24);
    }
}
