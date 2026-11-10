//! Scroll bars (section 14 of the design, D174): 16 px wide, an arrow button at each end and
//! the thumb on the track between them, sized and placed from `SCROLLINFO`; the chevrons the
//! arrows, the combo box and submenus draw.

use super::list_view::{CHEVRON_LEFT, horizontal_part_at, thumb_position};
use super::{Palette, dotted_outline, ring};
use crate::surface::{Rect, Surface};

/// A scroll bar's width, and its arrow buttons' size.
pub const WIDTH: i32 = 16;
/// How long an arrow or the track is held before its request repeats.
pub const REPEAT_DELAY_MS: u32 = 250;
/// How often a held arrow or track repeats its request after the first delay.
pub const REPEAT_INTERVAL_MS: u32 = 50;
const SMALLEST_THUMB: i32 = 16;
const VERTICAL_CHEVRON_AT: (i32, i32) = (4, 6);
const HORIZONTAL_CHEVRON_AT: (i32, i32) = (6, 4);

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

    /// Whether the bar scrolls: a range of more than one position that the page shows only
    /// part of, as Windows has it (a page of 0 scrolls whenever the maximum is above the
    /// minimum, with a thumb of the arrows' size).
    pub fn scrolls(&self) -> bool {
        self.maximum > self.minimum && i64::from(self.page) < self.range()
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
    let size = match info.page {
        0 => WIDTH.min(track),
        page => (i64::from(track) * i64::from(page) / info.range())
            .clamp(i64::from(SMALLEST_THUMB.min(track)), i64::from(track)) as i32,
    };
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

/// `SCROLLBAR`'s styles this layer reads.
pub mod style {
    pub const VERTICAL: u32 = 0x0001;
}

/// The `SB_*` request codes a scroll bar sends in `WM_VSCROLL` and `WM_HSCROLL`.
pub mod request {
    pub const LINE_UP: u32 = 0;
    pub const LINE_DOWN: u32 = 1;
    pub const PAGE_UP: u32 = 2;
    pub const PAGE_DOWN: u32 = 3;
    pub const THUMB_POSITION: u32 = 4;
    pub const THUMB_TRACK: u32 = 5;
    pub const TOP: u32 = 6;
    pub const BOTTOM: u32 = 7;
    pub const END_SCROLL: u32 = 8;
}

/// `SCROLLINFO`'s `fMask` bits.
pub mod mask {
    pub const RANGE: u32 = 0x0001;
    pub const PAGE: u32 = 0x0002;
    pub const POSITION: u32 = 0x0004;
    pub const DISABLE_NO_SCROLL: u32 = 0x0008;
    pub const TRACK_POSITION: u32 = 0x0010;
}

/// The keys a focused scroll bar reads, as Windows' virtual keys.
pub mod key {
    pub const PRIOR: u32 = 0x21;
    pub const NEXT: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
}

/// One `WM_VSCROLL` or `WM_HSCROLL` a scroll bar sends: an `SB_*` code and, for the thumb's
/// codes, the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub code: u32,
    pub position: i32,
}

impl Scroll {
    const fn plain(code: u32) -> Scroll {
        Scroll { code, position: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Track {
    grab: i32,
    start: i32,
    position: i32,
}

/// A `SCROLLBAR` control (section 14): its `SCROLLINFO`, the part held down and the thumb
/// while it is dragged. Offsets are along the bar from its start, lengths its long side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollBar {
    pub info: ScrollInfo,
    pub vertical: bool,
    pressed: Option<Part>,
    hovered: Option<Part>,
    track: Option<Track>,
    pointer: i32,
}

impl ScrollBar {
    pub const fn new(vertical: bool) -> ScrollBar {
        ScrollBar {
            info: ScrollInfo {
                minimum: 0,
                maximum: 0,
                page: 0,
                position: 0,
            },
            vertical,
            pressed: None,
            hovered: None,
            track: None,
            pointer: 0,
        }
    }

    /// The part under the pointer, if any.
    pub fn hovered(&self) -> Option<Part> {
        self.hovered
    }

    /// The pointer is `offset` along a bar `length` long, or off it (`None`); whether the part
    /// under it changed.
    pub fn hover(&mut self, offset: Option<i32>, length: i32) -> bool {
        let part = offset.and_then(|offset| self.part_at(offset, length));
        let changed = part != self.hovered;
        self.hovered = part;
        changed
    }

    /// The part held down, if any.
    pub fn pressed(&self) -> Option<Part> {
        self.pressed
    }

    /// The position the dragged thumb shows (`SIF_TRACKPOS`), `nPos` when not dragging.
    pub fn track_position(&self) -> i32 {
        self.track
            .map_or(self.info.position, |track| track.position)
    }

    /// `SetScrollInfo`: the parts of `given` that `fields` names replace the bar's; the
    /// position is clamped to the range the page leaves. Returns the new position.
    pub fn set_info(&mut self, fields: u32, given: &ScrollInfo) -> i32 {
        if fields & mask::RANGE != 0 {
            self.info.minimum = given.minimum;
            self.info.maximum = given.maximum.max(given.minimum);
        }
        if fields & mask::PAGE != 0 {
            self.info.page = given.page;
        }
        if fields & mask::POSITION != 0 {
            self.info.position = given.position;
        }
        let last = match self.info.page {
            0 => self.info.maximum,
            _ => self.info.last_position(),
        };
        self.info.position = self.info.position.clamp(self.info.minimum, last);
        self.info.position
    }

    fn part_at(&self, offset: i32, length: i32) -> Option<Part> {
        match self.vertical {
            true => part_at(Rect::sized(0, 0, WIDTH, length), &self.info, offset),
            false => horizontal_part_at(Rect::sized(0, 0, length, WIDTH), &self.info, offset),
        }
    }

    /// A left button-down `offset` along a bar `length` long: an arrow scrolls a line, the
    /// track a page, the thumb starts a drag (no request until it moves).
    pub fn button_down(&mut self, offset: i32, length: i32) -> Option<Scroll> {
        self.pointer = offset;
        let part = self.part_at(offset, length)?;
        self.pressed = Some(part);
        let code = match part {
            Part::LineUp => request::LINE_UP,
            Part::LineDown => request::LINE_DOWN,
            Part::PageUp => request::PAGE_UP,
            Part::PageDown => request::PAGE_DOWN,
            Part::Thumb => {
                let (start, _) = thumb(length, &self.info)?;
                self.track = Some(Track {
                    grab: offset - start,
                    start,
                    position: self.info.position,
                });
                return None;
            }
        };
        Some(Scroll::plain(code))
    }

    /// The pointer moved to `offset` while the button is held: a dragged thumb follows it,
    /// keeping the grab, and reports each new position as `SB_THUMBTRACK`.
    pub fn pointer_moved(&mut self, offset: i32, length: i32) -> Option<Scroll> {
        self.pointer = offset;
        let info = self.info;
        let track = self.track.as_mut()?;
        let (_, size) = thumb(length, &info)?;
        let start = (offset - track.grab).clamp(WIDTH, (length - WIDTH - size).max(WIDTH));
        track.start = start;
        let position = thumb_position(length, &info, start);
        if position == track.position {
            return None;
        }
        track.position = position;
        Some(Scroll {
            code: request::THUMB_TRACK,
            position,
        })
    }

    /// The request a held arrow or track repeats now: the arrow's line while the pointer is on
    /// it, the track's page while the pointer is still on that side of the thumb (so paging
    /// stops when the thumb reaches it); `None` otherwise.
    pub fn repeat(&self, length: i32) -> Option<Scroll> {
        let pressed = self.pressed?;
        let code = match pressed {
            Part::LineUp => request::LINE_UP,
            Part::LineDown => request::LINE_DOWN,
            Part::PageUp => request::PAGE_UP,
            Part::PageDown => request::PAGE_DOWN,
            Part::Thumb => return None,
        };
        (self.part_at(self.pointer, length) == Some(pressed)).then_some(Scroll::plain(code))
    }

    /// The left button released: a drag ends with `SB_THUMBPOSITION` and `SB_ENDSCROLL`, any
    /// other press with `SB_ENDSCROLL`; the thumb goes back to `nPos`.
    pub fn button_up(&mut self) -> [Option<Scroll>; 2] {
        let pressed = self.pressed.take();
        match self.track.take() {
            Some(track) => [
                Some(Scroll {
                    code: request::THUMB_POSITION,
                    position: track.position,
                }),
                Some(Scroll::plain(request::END_SCROLL)),
            ],
            None => [pressed.map(|_| Scroll::plain(request::END_SCROLL)), None],
        }
    }

    /// A key on a focused scroll bar control.
    pub fn key(&self, virtual_key: u32) -> Option<Scroll> {
        let code = match virtual_key {
            key::UP | key::LEFT => request::LINE_UP,
            key::DOWN | key::RIGHT => request::LINE_DOWN,
            key::PRIOR => request::PAGE_UP,
            key::NEXT => request::PAGE_DOWN,
            key::HOME => request::TOP,
            key::END => request::BOTTOM,
            _ => return None,
        };
        Some(Scroll::plain(code))
    }

    fn shown_thumb(&self, length: i32) -> Option<(i32, i32)> {
        let (start, size) = thumb(length, &self.info)?;
        Some((self.track.map_or(start, |track| track.start), size))
    }
}

/// How a scroll bar control looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarLook {
    pub enabled: bool,
    pub focused: bool,
    pub hovered: bool,
}

/// Paints scroll bar control `bar` over the whole of `rect`: track, arrows (the held one
/// pressed), the thumb (pressed while dragged) and, when focused, the focus mark on it.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    bar: &ScrollBar,
    look: BarLook,
    palette: &Palette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, palette.scroll_track);
    let length = match bar.vertical {
        true => rect.height(),
        false => rect.width(),
    };
    let along = |start: i32, size: i32| match bar.vertical {
        true => Rect::sized(rect.left, rect.top + start, rect.width(), size),
        false => Rect::sized(rect.left + start, rect.top, size, rect.height()),
    };
    let placed = bar.shown_thumb(length).filter(|_| look.enabled);
    let arrow = WIDTH.min(length / 2);
    let (first, second, at) = match bar.vertical {
        true => (&CHEVRON_UP, &CHEVRON_DOWN, VERTICAL_CHEVRON_AT),
        false => (&CHEVRON_LEFT, &CHEVRON_RIGHT, HORIZONTAL_CHEVRON_AT),
    };
    for (part, start, chevron) in [
        (Part::LineUp, 0, first),
        (Part::LineDown, length - arrow, second),
    ] {
        let button = along(start, arrow);
        let pressed = placed.is_some() && bar.pressed == Some(part);
        let hovered = placed.is_some() && look.hovered && bar.hovered == Some(part);
        let colour = match (placed, pressed) {
            (None, _) => palette.scroll_arrow_disabled,
            (Some(_), true) => palette.scroll_arrow_pressed_glyph,
            (Some(_), false) => palette.scroll_arrow,
        };
        match (pressed, hovered) {
            (true, _) => surface.fill(button, palette.scroll_arrow_pressed_fill),
            (false, true) => surface.fill(button, palette.scroll_arrow_hover_fill),
            (false, false) => {}
        }
        if arrow >= WIDTH {
            draw_chevron(
                surface,
                button.left + at.0,
                button.top + at.1,
                chevron,
                colour,
            );
        }
    }
    if let Some((start, size)) = placed {
        let thumb_rect = along(start, size);
        let fill = match (
            bar.pressed,
            look.hovered && bar.hovered == Some(Part::Thumb),
        ) {
            (Some(Part::Thumb), _) => palette.scroll_thumb_pressed,
            (_, true) => palette.scroll_thumb_hover,
            _ => palette.scroll_thumb,
        };
        surface.fill(thumb_rect, fill);
        match (look.focused, palette.rounded) {
            (false, _) => {}
            (true, false) => dotted_outline(surface, thumb_rect, palette.focus),
            (true, true) => ring(surface, thumb_rect, palette.focus),
        }
    }
    surface.set_clip(saved);
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
    fn a_bar_without_a_page_scrolls_with_an_arrow_sized_thumb_when_its_range_is_wider_than_one() {
        let pageless = ScrollInfo { page: 0, ..info(0) };
        assert!(pageless.scrolls());
        assert_eq!(pageless.last_position(), 99);
        assert_eq!(thumb(200, &pageless), Some((16, WIDTH)));
        assert_eq!(
            thumb(
                200,
                &ScrollInfo {
                    position: 99,
                    ..pageless
                }
            ),
            Some((168, WIDTH))
        );
        let one_position = ScrollInfo {
            maximum: 0,
            ..pageless
        };
        assert!(!one_position.scrolls());
        assert_eq!(thumb(200, &one_position), None);
        assert_eq!(thumb(40, &info(0)), Some((16, 8)));
        let mut bar = ScrollBar::new(true);
        bar.set_info(mask::RANGE | mask::PAGE, &pageless);
        assert_eq!(
            bar.button_down(100, 200),
            Some(Scroll::plain(request::PAGE_DOWN))
        );
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

    fn bar(vertical: bool) -> ScrollBar {
        let mut bar = ScrollBar::new(vertical);
        bar.set_info(mask::RANGE | mask::PAGE | mask::POSITION, &info(0));
        bar
    }

    #[test]
    fn set_info_takes_the_named_fields_and_clamps_the_position() {
        let mut bar = ScrollBar::new(true);
        assert_eq!(bar.set_info(mask::RANGE, &info(0)), 0);
        assert_eq!(bar.set_info(mask::POSITION, &info(500)), 99);
        assert_eq!(bar.set_info(mask::PAGE, &info(0)), 90);
        assert_eq!(bar.set_info(mask::POSITION, &info(-4)), 0);
        assert_eq!(bar.info.page, 10);
    }

    #[test]
    fn arrows_and_the_track_ask_for_lines_and_pages_then_end_scroll() {
        let mut bar = bar(true);
        assert_eq!(
            bar.button_down(3, 200),
            Some(Scroll {
                code: request::LINE_UP,
                position: 0
            })
        );
        assert_eq!(bar.pressed(), Some(Part::LineUp));
        assert_eq!(
            bar.button_up(),
            [
                Some(Scroll {
                    code: request::END_SCROLL,
                    position: 0
                }),
                None
            ]
        );
        assert_eq!(
            bar.button_down(150, 200).map(|s| s.code),
            Some(request::PAGE_DOWN)
        );
        bar.button_up();
        assert_eq!(
            bar.button_down(199, 200).map(|s| s.code),
            Some(request::LINE_DOWN)
        );
        bar.button_up();
        assert_eq!(bar.pressed(), None);
    }

    #[test]
    fn a_dragged_thumb_tracks_then_reports_its_position_and_snaps_back() {
        let mut bar = bar(true);
        assert_eq!(bar.button_down(20, 200), None);
        assert_eq!(bar.pressed(), Some(Part::Thumb));
        let moved = bar.pointer_moved(96, 200).unwrap();
        assert_eq!(moved.code, request::THUMB_TRACK);
        assert_eq!(moved.position, 45);
        assert_eq!(bar.track_position(), 45);
        assert_eq!(bar.pointer_moved(96, 200), None);
        assert_eq!(bar.pointer_moved(1000, 200).map(|s| s.position), Some(90));
        let [first, second] = bar.button_up();
        assert_eq!(
            first,
            Some(Scroll {
                code: request::THUMB_POSITION,
                position: 90
            })
        );
        assert_eq!(second.map(|s| s.code), Some(request::END_SCROLL));
        assert_eq!(bar.track_position(), 0);
    }

    #[test]
    fn keys_on_a_focused_bar_map_to_requests() {
        let bar = bar(false);
        assert_eq!(bar.key(key::LEFT).map(|s| s.code), Some(request::LINE_UP));
        assert_eq!(bar.key(key::NEXT).map(|s| s.code), Some(request::PAGE_DOWN));
        assert_eq!(bar.key(key::HOME).map(|s| s.code), Some(request::TOP));
        assert_eq!(bar.key(key::END).map(|s| s.code), Some(request::BOTTOM));
        assert_eq!(bar.key(0x41), None);
    }

    #[test]
    fn a_horizontal_control_draws_its_pressed_arrow_and_focused_thumb() {
        let mut pixels = vec![0u32; 200 * 16];
        let mut surface = Surface::new(&mut pixels, 200, 16).unwrap();
        let mut bar = bar(false);
        bar.button_down(3, 200);
        let look = BarLook {
            enabled: true,
            focused: true,
            hovered: false,
        };
        paint(&mut surface, Rect::sized(0, 0, 200, 16), &bar, look, &LIGHT);
        assert_eq!(surface.get(0, 0), Some(LIGHT.scroll_arrow_pressed_fill));
        assert_eq!(surface.get(6, 7), Some(LIGHT.scroll_arrow_pressed_glyph));
        assert_eq!(surface.get(17, 1), Some(LIGHT.scroll_thumb));
        assert_eq!(surface.get(16, 0), Some(LIGHT.focus));
        assert_eq!(surface.get(17, 0), Some(LIGHT.scroll_thumb));
        assert_eq!(surface.get(40, 8), Some(LIGHT.scroll_track));
        let disabled = BarLook {
            enabled: false,
            focused: false,
            hovered: false,
        };
        paint(
            &mut surface,
            Rect::sized(0, 0, 200, 16),
            &bar,
            disabled,
            &LIGHT,
        );
        assert_eq!(surface.get(17, 1), Some(LIGHT.scroll_track));
        assert_eq!(surface.get(0, 0), Some(LIGHT.scroll_track));
    }

    #[test]
    fn the_part_under_the_pointer_takes_the_hover_colours_while_the_bar_is_hovered() {
        let mut bar = ScrollBar::new(false);
        bar.set_info(
            mask::RANGE | mask::PAGE,
            &ScrollInfo {
                minimum: 0,
                maximum: 99,
                page: 10,
                position: 0,
            },
        );
        assert!(bar.hover(Some(195), 200));
        assert_eq!(bar.hovered(), Some(Part::LineDown));
        assert!(!bar.hover(Some(190), 200));
        let mut pixels = vec![0u32; 200 * 16];
        let mut surface = Surface::new(&mut pixels, 200, 16).unwrap();
        let look = BarLook {
            enabled: true,
            focused: false,
            hovered: true,
        };
        paint(&mut surface, Rect::sized(0, 0, 200, 16), &bar, look, &LIGHT);
        assert_eq!(surface.get(199, 0), Some(LIGHT.scroll_arrow_hover_fill));
        assert_eq!(surface.get(0, 0), Some(LIGHT.scroll_track));
        assert!(bar.hover(Some(20), 200));
        paint(&mut surface, Rect::sized(0, 0, 200, 16), &bar, look, &LIGHT);
        assert_eq!(surface.get(20, 8), Some(LIGHT.scroll_thumb_hover));
        assert_eq!(surface.get(199, 0), Some(LIGHT.scroll_track));
        assert!(bar.hover(None, 200));
        assert_eq!(bar.hovered(), None);
    }

    #[test]
    fn a_held_arrow_repeats_while_the_pointer_stays_on_it_and_paging_stops_at_the_thumb() {
        let mut bar = ScrollBar::new(true);
        bar.set_info(
            mask::RANGE | mask::PAGE,
            &ScrollInfo {
                minimum: 0,
                maximum: 99,
                page: 10,
                position: 0,
            },
        );
        assert_eq!(bar.repeat(200), None);
        bar.button_down(190, 200);
        assert_eq!(bar.repeat(200), Some(Scroll::plain(request::LINE_DOWN)));
        bar.pointer_moved(100, 200);
        assert_eq!(bar.repeat(200), None);
        bar.pointer_moved(195, 200);
        assert_eq!(bar.repeat(200), Some(Scroll::plain(request::LINE_DOWN)));
        bar.button_up();
        assert_eq!(bar.repeat(200), None);
        bar.button_down(100, 200);
        assert_eq!(bar.repeat(200), Some(Scroll::plain(request::PAGE_DOWN)));
        bar.set_info(
            mask::POSITION,
            &ScrollInfo {
                minimum: 0,
                maximum: 0,
                page: 0,
                position: 50,
            },
        );
        assert_eq!(bar.repeat(200), None);
        bar.button_up();
        bar.button_down(20, 200);
        assert_eq!(bar.repeat(200), Some(Scroll::plain(request::PAGE_UP)));
        bar.button_up();
        bar.button_down(105, 200);
        assert_eq!(bar.repeat(200), None);
    }
}
