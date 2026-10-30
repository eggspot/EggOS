//! Scroll bars (section 14 of the design, D174): 16 px wide, an arrow button at each end and
//! the thumb on the track between them, sized and placed from `SCROLLINFO`; the chevrons the
//! arrows, the combo box and submenus draw.

use super::Palette;
use crate::surface::{Rect, Surface};

/// A scroll bar's width, and its arrow buttons' size.
pub const WIDTH: i32 = 16;
const SMALLEST_THUMB: i32 = 16;
const VERTICAL_CHEVRON_AT: (i32, i32) = (4, 6);

/// A chevron, one row per byte, its leftmost pixel the byte's top bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chevron {
    pub rows: [u8; 8],
    pub height: usize,
    pub width: i32,
}

/// The 8 by 4 chevron pointing down.
pub const CHEVRON_DOWN: Chevron = Chevron {
    rows: [
        0b1100_0011,
        0b0110_0110,
        0b0011_1100,
        0b0001_1000,
        0,
        0,
        0,
        0,
    ],
    height: 4,
    width: 8,
};

/// The 8 by 4 chevron pointing up.
pub const CHEVRON_UP: Chevron = Chevron {
    rows: [
        0b0001_1000,
        0b0011_1100,
        0b0110_0110,
        0b1100_0011,
        0,
        0,
        0,
        0,
    ],
    height: 4,
    width: 8,
};

/// The 4 by 8 chevron pointing right.
pub const CHEVRON_RIGHT: Chevron = Chevron {
    rows: [
        0b1000_0000,
        0b1100_0000,
        0b0110_0000,
        0b0011_0000,
        0b0011_0000,
        0b0110_0000,
        0b1100_0000,
        0b1000_0000,
    ],
    height: 8,
    width: 4,
};

/// Draws `chevron` with its top-left at (`x`, `y`) in `colour`.
pub fn draw_chevron(surface: &mut Surface<'_>, x: i32, y: i32, chevron: &Chevron, colour: u32) {
    for (row, bits) in chevron.rows[..chevron.height].iter().enumerate() {
        for column in 0..chevron.width {
            if bits & (0x80 >> column) != 0 {
                surface.pixel(x + column, y + row as i32, colour);
            }
        }
    }
}

/// `SCROLLINFO`'s range, page and position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollInfo {
    pub minimum: i32,
    pub maximum: i32,
    pub page: u32,
    pub position: i32,
}

impl ScrollInfo {
    fn range(&self) -> i64 {
        i64::from(self.maximum) - i64::from(self.minimum) + 1
    }

    /// Whether the bar scrolls: a page that shows part of the range.
    pub fn scrolls(&self) -> bool {
        self.page > 0 && i64::from(self.page) < self.range()
    }

    /// The highest position the page can start at.
    pub fn last_position(&self) -> i32 {
        (i64::from(self.maximum) - i64::from(self.page) + 1)
            .clamp(i64::from(self.minimum), i64::from(self.maximum)) as i32
    }
}

/// Where the thumb of a bar `length` long is, from the bar's start, and how long it is; `None`
/// when the bar does not scroll or has no track.
pub fn thumb(length: i32, info: &ScrollInfo) -> Option<(i32, i32)> {
    let track = length - 2 * WIDTH;
    if track <= 0 || !info.scrolls() {
        return None;
    }
    let size = (i64::from(track) * i64::from(info.page) / info.range())
        .clamp(i64::from(SMALLEST_THUMB), i64::from(track)) as i32;
    let last = info.last_position();
    let position = info.position.clamp(info.minimum, last);
    let travelled = match i64::from(last) - i64::from(info.minimum) {
        0 => 0,
        span => {
            (i64::from(track - size) * (i64::from(position) - i64::from(info.minimum)) / span)
                as i32
        }
    };
    Some((WIDTH + travelled, size))
}

/// Which part of a vertical bar at `rect` the point `y` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    LineUp,
    PageUp,
    Thumb,
    PageDown,
    LineDown,
}

/// The part of the vertical bar at `rect` under `y`, if `y` is on it.
pub fn part_at(rect: Rect, info: &ScrollInfo, y: i32) -> Option<Part> {
    if y < rect.top || y >= rect.bottom {
        return None;
    }
    let arrow = WIDTH.min(rect.height() / 2);
    if y < rect.top + arrow {
        return Some(Part::LineUp);
    }
    if y >= rect.bottom - arrow {
        return Some(Part::LineDown);
    }
    let (start, size) = thumb(rect.height(), info)?;
    let offset = y - rect.top;
    Some(match () {
        () if offset < start => Part::PageUp,
        () if offset < start + size => Part::Thumb,
        () => Part::PageDown,
    })
}

/// Paints a vertical scroll bar over `rect` (16 px wide); a disabled one, or one that does
/// not scroll, has no thumb and greyed arrows.
pub fn paint_vertical(
    surface: &mut Surface<'_>,
    rect: Rect,
    info: &ScrollInfo,
    enabled: bool,
    palette: &Palette,
) {
    surface.fill(rect, palette.scroll_track);
    let placed = thumb(rect.height(), info).filter(|_| enabled);
    let arrow_colour = match placed {
        Some(_) => palette.scroll_arrow,
        None => palette.scroll_arrow_disabled,
    };
    let arrow = WIDTH.min(rect.height() / 2);
    let (dx, dy) = VERTICAL_CHEVRON_AT;
    if arrow >= dy + CHEVRON_UP.height as i32 {
        draw_chevron(
            surface,
            rect.left + dx,
            rect.top + dy,
            &CHEVRON_UP,
            arrow_colour,
        );
        let bottom_button = rect.bottom - arrow;
        draw_chevron(
            surface,
            rect.left + dx,
            bottom_button + dy,
            &CHEVRON_DOWN,
            arrow_colour,
        );
    }
    if let Some((start, size)) = placed {
        surface.fill(
            Rect::sized(rect.left, rect.top + start, rect.width(), size),
            palette.scroll_thumb,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    fn info(position: i32) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: 99,
            page: 10,
            position,
        }
    }

    #[test]
    fn the_thumb_is_sized_by_the_page_and_placed_by_the_position() {
        assert_eq!(thumb(200, &info(0)), Some((16, 16)));
        assert_eq!(thumb(200, &info(45)), Some((92, 16)));
        assert_eq!(thumb(200, &info(90)), Some((168, 16)));
        assert_eq!(thumb(200, &info(500)), Some((168, 16)));
        let half = ScrollInfo {
            page: 50,
            ..info(0)
        };
        assert_eq!(thumb(232, &half), Some((16, 100)));
        let whole = ScrollInfo {
            page: 100,
            ..info(0)
        };
        assert_eq!(thumb(200, &whole), None);
        assert_eq!(thumb(30, &info(0)), None);
    }

    #[test]
    fn a_point_falls_on_an_arrow_the_track_or_the_thumb() {
        let rect = Rect::sized(0, 0, 16, 200);
        assert_eq!(part_at(rect, &info(45), 3), Some(Part::LineUp));
        assert_eq!(part_at(rect, &info(45), 50), Some(Part::PageUp));
        assert_eq!(part_at(rect, &info(45), 100), Some(Part::Thumb));
        assert_eq!(part_at(rect, &info(45), 150), Some(Part::PageDown));
        assert_eq!(part_at(rect, &info(45), 199), Some(Part::LineDown));
        assert_eq!(part_at(rect, &info(45), 200), None);
    }

    #[test]
    fn a_bar_that_scrolls_shows_its_thumb_and_dark_arrows() {
        let mut pixels = vec![0u32; 16 * 200];
        let mut surface = Surface::new(&mut pixels, 16, 200).unwrap();
        let rect = Rect::sized(0, 0, 16, 200);
        paint_vertical(&mut surface, rect, &info(0), true, &LIGHT);
        assert_eq!(surface.get(0, 0), Some(LIGHT.scroll_track));
        assert_eq!(surface.get(7, 6), Some(LIGHT.scroll_arrow));
        assert_eq!(surface.get(0, 16), Some(LIGHT.scroll_thumb));
        assert_eq!(surface.get(15, 31), Some(LIGHT.scroll_thumb));
        assert_eq!(surface.get(0, 32), Some(LIGHT.scroll_track));
        assert_eq!(surface.get(4, 190), Some(LIGHT.scroll_arrow));
        paint_vertical(&mut surface, rect, &info(0), false, &LIGHT);
        assert_eq!(surface.get(0, 16), Some(LIGHT.scroll_track));
        assert_eq!(surface.get(7, 6), Some(LIGHT.scroll_arrow_disabled));
    }
}
