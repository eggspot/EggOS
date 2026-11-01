//! comctl32's status bar (section 21 of the design, D178): the [`StatusBar`] model (the parts'
//! right edges, their texts, drawing types and icons, simple mode, the program's background)
//! and its painting: the bar filled, each part's text in up to three tab-separated pieces (left,
//! centred, right), an icon before the first, and a 1 px separator after every part but the
//! last.

use super::common::{TextArena, plain_cells};
use super::{MAX_LABEL_CELLS, Palette, draw_fitted, fit_cells};
use crate::colour;
use crate::surface::{Rect, Surface};
use crate::text::{CELL_WIDTH, Cell};

/// The bar's height.
pub const HEIGHT: i32 = 24;
/// The most parts a bar has.
pub const MAX_PARTS: usize = 256;
/// The part whose text simple mode shows (`SB_SIMPLEID`).
pub const SIMPLE_PART: usize = 255;
/// `CLR_DEFAULT`: the palette's background.
pub const DEFAULT_COLOUR: u32 = 0xff00_0000;
const TEXT_UNITS: usize = 4096;
const TEXT_TOP: i32 = 4;
const TEXT_INSET: i32 = 6;
const ICON_INSET: i32 = 4;
const TEXT_AFTER_ICON: i32 = 24;
const SEPARATOR_TOP: i32 = 2;
const SEPARATOR_BOTTOM: i32 = 22;
const MAX_PIECES: usize = 3;
const TAB: u16 = 0x09;

/// `SB_SETTEXTW`'s drawing types (the high byte of `wParam`'s low word).
pub mod drawing {
    pub const NO_BORDERS: u16 = 0x100;
    pub const OWNER_DRAW: u16 = 0x1000;
    pub const NO_TAB_PARSING: u16 = 0x800;
}

/// A status bar's parts and texts.
#[derive(Debug, Clone)]
pub struct StatusBar {
    edges: [i32; MAX_PARTS],
    count: usize,
    kinds: [u16; MAX_PARTS],
    icons: [usize; MAX_PARTS],
    texts: TextArena<TEXT_UNITS, MAX_PARTS>,
    pub simple: bool,
    pub background: u32,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    /// One part as wide as the bar.
    pub const fn new() -> Self {
        let mut edges = [0; MAX_PARTS];
        edges[0] = -1;
        Self {
            edges,
            count: 1,
            kinds: [0; MAX_PARTS],
            icons: [0; MAX_PARTS],
            texts: TextArena::new(),
            simple: false,
            background: DEFAULT_COLOUR,
        }
    }

    /// The parts' right edges as `SB_SETPARTS` gave them, `-1` for the bar's width.
    pub fn edges(&self) -> &[i32] {
        &self.edges[..self.count]
    }

    /// `SB_SETPARTS`: 1 to 256 right edges; false for another count.
    pub fn set_parts(&mut self, edges: &[i32]) -> bool {
        if edges.is_empty() || edges.len() > MAX_PARTS {
            return false;
        }
        self.edges[..edges.len()].copy_from_slice(edges);
        self.count = edges.len();
        true
    }

    /// `SB_SETTEXTW` for part `part` with drawing type `kind`; false when there is no room.
    pub fn set_text(&mut self, part: usize, kind: u16, units: &[u16]) -> bool {
        if part >= MAX_PARTS {
            return false;
        }
        self.kinds[part] = kind;
        self.texts.set(part, units)
    }

    /// Part `part`'s text.
    pub fn text(&self, part: usize) -> &[u16] {
        self.texts.get(part)
    }

    /// Part `part`'s drawing type.
    pub fn kind(&self, part: usize) -> u16 {
        self.kinds.get(part).copied().unwrap_or(0)
    }

    /// `SB_SETICON`: part `part`'s icon (0 for none); the previous one.
    pub fn set_icon(&mut self, part: usize, icon: usize) -> Option<usize> {
        self.icons
            .get_mut(part)
            .map(|slot| core::mem::replace(slot, icon))
    }

    pub fn icon(&self, part: usize) -> usize {
        self.icons.get(part).copied().unwrap_or(0)
    }

    /// How many parts are shown: one in simple mode.
    pub fn shown_parts(&self) -> usize {
        match self.simple {
            true => 1,
            false => self.count,
        }
    }

    /// The part whose text and icon the shown part `index` has.
    pub fn source_part(&self, index: usize) -> usize {
        match self.simple {
            true => SIMPLE_PART,
            false => index,
        }
    }

    /// Shown part `index`'s rectangle in a bar `width` wide, from (0, 0).
    pub fn part_rect(&self, index: usize, width: i32) -> Option<Rect> {
        if index >= self.shown_parts() {
            return None;
        }
        if self.simple {
            return Some(Rect::new(0, 0, width, HEIGHT));
        }
        let right = |edge: i32| if edge == -1 { width } else { edge };
        let left = match index {
            0 => 0,
            index => right(self.edges[index - 1]),
        };
        Some(Rect::new(left, 0, right(self.edges[index]), HEIGHT))
    }
}

/// How a status bar looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusLook {
    pub enabled: bool,
}

fn pieces(units: &[u16], kind: u16) -> [&[u16]; MAX_PIECES] {
    let mut found: [&[u16]; MAX_PIECES] = [&[]; MAX_PIECES];
    if kind & drawing::NO_TAB_PARSING != 0 {
        found[0] = units;
        return found;
    }
    for (slot, piece) in found.iter_mut().zip(units.split(|unit| *unit == TAB)) {
        *slot = piece;
    }
    found
}

fn draw_piece(
    surface: &mut Surface<'_>,
    units: &[u16],
    place: (i32, i32, i32, i32),
    align: usize,
    colour: u32,
) {
    let (left, right, top, room) = place;
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = plain_cells(units, &mut cells);
    if count == 0 {
        return;
    }
    let (shown, ellipsis) = fit_cells(count, room);
    let width = (shown + if ellipsis { 3 } else { 0 }) as i32 * CELL_WIDTH;
    let x = match align {
        0 => left,
        1 => left + (right - left - width) / 2,
        _ => right - TEXT_INSET - width,
    };
    draw_fitted(surface, x, top, &cells[..count], room, colour);
}

/// Paints `bar` over the whole of `rect` (its top-left at (0, 0)); `draw_icon` draws icon
/// `handle` with its top-left at (`x`, `y`).
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    bar: &StatusBar,
    look: StatusLook,
    palette: &Palette,
    draw_icon: &mut dyn FnMut(&mut Surface<'_>, usize, i32, i32),
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    let fill = match bar.background {
        DEFAULT_COLOUR => palette.surface,
        colour_ref => colour::pixel(colour_ref),
    };
    surface.fill(rect, fill);
    let colour = match look.enabled {
        true => palette.text,
        false => palette.text_disabled,
    };
    let width = rect.width();
    let shown = bar.shown_parts();
    for index in 0..shown {
        let Some(part) = bar.part_rect(index, width) else {
            break;
        };
        if part.left >= width {
            break;
        }
        let source = bar.source_part(index);
        let part = part.offset(rect.left, rect.top);
        let part_width = part.width();
        let icon = bar.icon(source);
        let first_left = match icon {
            0 => part.left + TEXT_INSET,
            icon => {
                draw_icon(surface, icon, part.left + ICON_INSET, part.top + ICON_INSET);
                part.left + TEXT_AFTER_ICON
            }
        };
        let first_room = match icon {
            0 => part_width - 2 * TEXT_INSET,
            _ => part_width - TEXT_AFTER_ICON - TEXT_INSET,
        };
        let kind = bar.kind(source);
        let texts = match kind & drawing::OWNER_DRAW {
            0 => bar.text(source),
            _ => &[][..],
        };
        let top = part.top + TEXT_TOP;
        for (align, piece) in pieces(texts, kind).iter().enumerate() {
            let room = match align {
                0 => first_room,
                _ => part_width - 2 * TEXT_INSET,
            };
            let left = match align {
                0 => first_left,
                _ => part.left,
            };
            draw_piece(surface, piece, (left, part.right, top, room), align, colour);
        }
        let last = index + 1 == shown;
        if !last && kind & drawing::NO_BORDERS == 0 {
            surface.fill(
                Rect::new(
                    part.right - 1,
                    part.top + SEPARATOR_TOP,
                    part.right,
                    part.top + SEPARATOR_BOTTOM,
                ),
                palette.menu_separator,
            );
        }
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn render(bar: &StatusBar, width: u32) -> Vec<u32> {
        let mut pixels = vec![0u32; width as usize * HEIGHT as usize];
        let mut surface = Surface::new(&mut pixels, width, HEIGHT as u32).unwrap();
        let look = StatusLook { enabled: true };
        paint(
            &mut surface,
            Rect::new(0, 0, width as i32, HEIGHT),
            bar,
            look,
            &LIGHT,
            &mut |_, _, _, _| {},
        );
        pixels
    }

    fn reference(width: u32, texts: &[(i32, &str)], separators: &[i32]) -> Vec<u32> {
        let mut pixels = vec![0x00f0_f0f0u32; width as usize * HEIGHT as usize];
        let mut surface = Surface::new(&mut pixels, width, HEIGHT as u32).unwrap();
        for &(x, text) in texts {
            let mut cells = [Cell::Space; MAX_LABEL_CELLS];
            let count = plain_cells(&units(text), &mut cells);
            crate::text::draw_cells(&mut surface, x, 4, &cells[..count], 0, None);
        }
        for &x in separators {
            surface.fill(Rect::new(x, 2, x + 1, 22), 0x00d7_d7d7);
        }
        pixels
    }

    #[test]
    fn parts_follow_their_edges_and_minus_one_is_the_width() {
        let mut bar = StatusBar::new();
        assert!(bar.set_parts(&[240, -1]));
        assert_eq!(bar.part_rect(0, 480), Some(Rect::new(0, 0, 240, 24)));
        assert_eq!(bar.part_rect(1, 480), Some(Rect::new(240, 0, 480, 24)));
        assert_eq!(bar.part_rect(1, 150), Some(Rect::new(240, 0, 150, 24)));
        assert!(bar.set_parts(&[100, 200, -1]));
        assert_eq!(bar.part_rect(2, 150), Some(Rect::new(200, 0, 150, 24)));
        assert_eq!(bar.part_rect(3, 150), None);
        assert!(!bar.set_parts(&[]));
        bar.simple = true;
        assert_eq!(bar.part_rect(0, 480), Some(Rect::new(0, 0, 480, 24)));
        assert_eq!(bar.source_part(0), SIMPLE_PART);
    }

    #[test]
    fn the_examples_texts_and_separator_sit_where_the_design_says() {
        let mut bar = StatusBar::new();
        bar.set_parts(&[240, -1]);
        bar.set_text(0, 0, &units("Ready"));
        bar.set_text(1, 0, &units("5 items"));
        let expected = reference(480, &[(6, "Ready"), (246, "5 items")], &[239]);
        assert!(render(&bar, 480) == expected);
        bar.set_text(0, 0, &units("Cherries 7"));
        let expected = reference(480, &[(6, "Cherries 7"), (246, "5 items")], &[239]);
        assert!(render(&bar, 480) == expected);
    }

    #[test]
    fn tab_pieces_are_left_centred_and_right_aligned() {
        let mut bar = StatusBar::new();
        bar.set_text(0, 0, &units("a\tbb\tc"));
        let expected = reference(200, &[(6, "a"), (92, "bb"), (186, "c")], &[]);
        assert!(render(&bar, 200) == expected);
        bar.set_text(0, drawing::NO_TAB_PARSING, &units("a\tb"));
        let expected = reference(200, &[(6, "a b")], &[]);
        assert!(render(&bar, 200) == expected);
    }

    #[test]
    fn a_part_past_the_width_is_not_drawn_and_simple_mode_shows_its_own_text() {
        let mut bar = StatusBar::new();
        bar.set_parts(&[100, 200, -1]);
        bar.set_text(0, 0, &units("one"));
        bar.set_text(1, 0, &units("two"));
        bar.set_text(2, 0, &units("three"));
        let expected = reference(150, &[(6, "one"), (106, "two")], &[99]);
        assert!(render(&bar, 150) == expected);
        bar.set_text(SIMPLE_PART, 0, &units("simple"));
        bar.simple = true;
        assert!(render(&bar, 150) == reference(150, &[(6, "simple")], &[]));
        bar.simple = false;
        assert_eq!(bar.text(1), units("two"));
    }

    #[test]
    fn no_borders_drops_the_separator_and_an_icon_moves_the_first_piece() {
        let mut bar = StatusBar::new();
        bar.set_parts(&[100, -1]);
        bar.set_text(0, drawing::NO_BORDERS, &units("x"));
        bar.set_icon(0, 7);
        let mut pixels = vec![0u32; 200 * 24];
        let mut surface = Surface::new(&mut pixels, 200, 24).unwrap();
        let mut drawn = Vec::new();
        paint(
            &mut surface,
            Rect::new(0, 0, 200, 24),
            &bar,
            StatusLook { enabled: true },
            &LIGHT,
            &mut |surface, icon, x, y| {
                drawn.push((icon, x, y));
                surface.pixel(x, y, 0x0012_3456);
            },
        );
        let mut expected = reference(200, &[(24, "x")], &[]);
        expected[4 * 200 + 4] = 0x0012_3456;
        assert!(pixels == expected);
        assert_eq!(drawn, [(7, 4, 4)]);
    }
}
