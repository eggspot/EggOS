//! Where an `xdg_popup` goes (D172), from `xdg-shell.xml`'s `xdg_positioner`: its size, the
//! anchor rectangle in its parent's window geometry, the anchor point on it, the gravity the
//! popup extends in, an offset, and the adjustments the compositor may make so the popup stays
//! inside the area it must fit (flip, slide, resize: each axis on its own, in that order).

use crate::protocol::xdg_positioner::{
    BOTTOM, BOTTOM_LEFT, BOTTOM_RIGHT, FLIP_X, FLIP_Y, LEFT, RESIZE_X, RESIZE_Y, RIGHT, SLIDE_X,
    SLIDE_Y, TOP, TOP_LEFT, TOP_RIGHT,
};

/// A rectangle in signed coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    /// The area at (`x`, `y`) of `width` by `height`.
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
}

/// An `xdg_positioner`'s state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Positioner {
    /// The popup's size; (0, 0) until `set_size`.
    pub size: (i32, i32),
    /// The anchor rectangle; `None` until `set_anchor_rect`.
    pub anchor_rect: Option<Area>,
    pub anchor: u32,
    pub gravity: u32,
    pub adjustment: u32,
    pub offset: (i32, i32),
}

impl Positioner {
    /// Whether it says enough to place a popup: a size and an anchor rectangle.
    pub fn is_complete(&self) -> bool {
        self.size.0 > 0 && self.size.1 > 0 && self.anchor_rect.is_some()
    }

    /// The popup's place and size in its parent's window geometry, kept inside `bounds` (in
    /// the same coordinates) as far as its adjustments allow.
    pub fn place(&self, bounds: Area) -> Area {
        let anchor_rect = self.anchor_rect.unwrap_or_default();
        let (width, height) = (self.size.0.max(1), self.size.1.max(1));
        let horizontal = Axis {
            start: anchor_rect.x,
            length: anchor_rect.width,
            size: width,
            offset: self.offset.0,
            anchor: side_x(self.anchor),
            gravity: side_x(self.gravity),
        };
        let vertical = Axis {
            start: anchor_rect.y,
            length: anchor_rect.height,
            size: height,
            offset: self.offset.1,
            anchor: side_y(self.anchor),
            gravity: side_y(self.gravity),
        };
        let (x, width) = horizontal.fit(
            bounds.x,
            bounds.right(),
            self.adjustment & FLIP_X != 0,
            self.adjustment & SLIDE_X != 0,
            self.adjustment & RESIZE_X != 0,
        );
        let (y, height) = vertical.fit(
            bounds.y,
            bounds.bottom(),
            self.adjustment & FLIP_Y != 0,
            self.adjustment & SLIDE_Y != 0,
            self.adjustment & RESIZE_Y != 0,
        );
        Area::new(x, y, width, height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Start,
    Middle,
    End,
}

impl Side {
    fn flipped(self) -> Self {
        match self {
            Self::Start => Self::End,
            Self::Middle => Self::Middle,
            Self::End => Self::Start,
        }
    }
}

fn side_x(edge: u32) -> Side {
    match edge {
        LEFT | TOP_LEFT | BOTTOM_LEFT => Side::Start,
        RIGHT | TOP_RIGHT | BOTTOM_RIGHT => Side::End,
        _ => Side::Middle,
    }
}

fn side_y(edge: u32) -> Side {
    match edge {
        TOP | TOP_LEFT | TOP_RIGHT => Side::Start,
        BOTTOM | BOTTOM_LEFT | BOTTOM_RIGHT => Side::End,
        _ => Side::Middle,
    }
}

#[derive(Debug, Clone, Copy)]
struct Axis {
    start: i32,
    length: i32,
    size: i32,
    offset: i32,
    anchor: Side,
    gravity: Side,
}

impl Axis {
    fn position(&self, anchor: Side, gravity: Side, offset: i32) -> i64 {
        let point = i64::from(self.start)
            + match anchor {
                Side::Start => 0,
                Side::Middle => i64::from(self.length) / 2,
                Side::End => i64::from(self.length),
            };
        let size = i64::from(self.size);
        point
            + i64::from(offset)
            + match gravity {
                Side::Start => -size,
                Side::Middle => -size / 2,
                Side::End => 0,
            }
    }

    fn fits(position: i64, size: i64, low: i64, high: i64) -> bool {
        position >= low && position + size <= high
    }

    fn fit(&self, low: i32, high: i64, flip: bool, slide: bool, resize: bool) -> (i32, i32) {
        let low = i64::from(low);
        let size = i64::from(self.size);
        let mut position = self.position(self.anchor, self.gravity, self.offset);
        if Self::fits(position, size, low, high) {
            return (clamp(position), self.size);
        }
        if flip {
            let flipped = self.position(
                self.anchor.flipped(),
                self.gravity.flipped(),
                self.offset.saturating_neg(),
            );
            if Self::fits(flipped, size, low, high) {
                return (clamp(flipped), self.size);
            }
        }
        if slide {
            if position + size > high {
                position = high - size;
            }
            if position < low {
                position = low;
            }
            if Self::fits(position, size, low, high) || !resize {
                return (clamp(position), self.size);
            }
        }
        if resize {
            let start = position.max(low);
            let end = (position + size).min(high);
            if end > start {
                return (clamp(start), clamp(end - start));
            }
        }
        (clamp(position), self.size)
    }
}

fn clamp(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::xdg_positioner::NONE;

    fn menu(anchor: u32, gravity: u32, adjustment: u32) -> Positioner {
        Positioner {
            size: (100, 80),
            anchor_rect: Some(Area::new(10, 20, 40, 10)),
            anchor,
            gravity,
            adjustment,
            offset: (0, 0),
        }
    }

    #[test]
    fn a_menu_hangs_below_its_anchor_from_the_anchor_s_corner() {
        let bounds = Area::new(0, 0, 400, 300);
        assert_eq!(
            menu(BOTTOM_LEFT, BOTTOM_RIGHT, 0).place(bounds),
            Area::new(10, 30, 100, 80)
        );
        assert_eq!(
            menu(NONE, NONE, 0).place(bounds),
            Area::new(-20, -15, 100, 80)
        );
        let mut offset = menu(BOTTOM_LEFT, BOTTOM_RIGHT, 0);
        offset.offset = (5, -2);
        assert_eq!(offset.place(bounds), Area::new(15, 28, 100, 80));
    }

    #[test]
    fn a_popup_past_the_edge_flips_then_slides_then_shrinks() {
        let bounds = Area::new(0, 0, 120, 100);
        let flipping = menu(BOTTOM_LEFT, BOTTOM_RIGHT, FLIP_Y);
        assert_eq!(flipping.place(bounds).y, 30);
        let tall = Positioner {
            size: (100, 90),
            ..flipping
        };
        assert_eq!(tall.place(bounds).y, 30);
        let flips_up = Positioner {
            anchor_rect: Some(Area::new(10, 90, 40, 10)),
            ..flipping
        };
        assert_eq!(flips_up.place(bounds), Area::new(10, 10, 100, 80));
        let slides = Positioner {
            anchor_rect: Some(Area::new(60, 20, 40, 10)),
            ..menu(BOTTOM_LEFT, BOTTOM_RIGHT, SLIDE_X)
        };
        assert_eq!(slides.place(bounds).x, 20);
        let shrinks = Positioner {
            anchor_rect: Some(Area::new(60, 20, 40, 10)),
            ..menu(BOTTOM_LEFT, BOTTOM_RIGHT, RESIZE_X)
        };
        assert_eq!(shrinks.place(bounds), Area::new(60, 30, 60, 80));
    }

    #[test]
    fn a_positioner_needs_a_size_and_an_anchor_rectangle() {
        assert!(!Positioner::default().is_complete());
        assert!(menu(NONE, NONE, 0).is_complete());
        let no_rect = Positioner {
            anchor_rect: None,
            ..menu(NONE, NONE, 0)
        };
        assert!(!no_rect.is_complete());
    }
}
