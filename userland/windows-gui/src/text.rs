//! Text in Egg Mono 8x16, every font's face in part 1 (D129): UTF-16 decoded into cells, lines
//! wrapped greedily at spaces (`DrawTextW`'s `DT_WORDBREAK` and the message box share the rule),
//! `TextOutW` and `DrawTextW` with Win32's alignment, prefix and clipping rules.

use crate::surface::{Rect, Surface};
use eggos_font::{self as font, UNDERLINE_ROW};

/// A glyph cell's width in pixels.
pub const CELL_WIDTH: i32 = font::GLYPH_WIDTH as i32;
/// A glyph cell's height in pixels, the line height of every font.
pub const CELL_HEIGHT: i32 = font::GLYPH_HEIGHT as i32;
/// The rows of a cell above the baseline, what `GetTextMetrics` reports as every font's ascent
/// (D174, `docs/ux/windows-controls-v0.3.md` section 11.3).
pub const ASCENT: i32 = 13;
/// The rows of a cell below the baseline, every font's descent.
pub const DESCENT: i32 = CELL_HEIGHT - ASCENT;

/// One decoded character of text, as layout sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Glyph { id: u8, underline: bool },
    Space,
    Break,
}

impl Cell {
    fn drawn(self) -> (u8, bool) {
        match self {
            Cell::Glyph { id, underline } => (id, underline),
            Cell::Space | Cell::Break => (b' ', false),
        }
    }
}

/// How [`decode`] reads text: `&` marking the next character's underline (`&&` is `&`), tabs
/// to the next multiple of 8 cells (else one space), and line ends as breaks or dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    pub prefixes: bool,
    pub expand_tabs: bool,
    pub single_line: bool,
}

const TAB_CELLS: usize = 8;

/// Decodes the UTF-16 `units` into `cells` and returns how many it filled: `\r\n`, `\n` and a
/// lone `\r` are one break each, a space is a [`Cell::Space`], other control characters are
/// dropped, and a character the font lacks is its `?`. Text past `cells`' end is left out.
pub fn decode(units: &[u16], reading: Reading, cells: &mut [Cell]) -> usize {
    let capacity = cells.len();
    let mut filled = 0;
    let mut column = 0;
    let mut underline_next = false;
    let mut characters = char::decode_utf16(units.iter().copied())
        .map(|unit| unit.map_or(u32::from(font::REPLACEMENT), u32::from))
        .peekable();
    let mut push = |cell: Cell, filled: &mut usize| {
        if let Some(slot) = cells.get_mut(*filled) {
            *slot = cell;
            *filled += 1;
        }
    };
    while let Some(character) = characters.next() {
        if filled == capacity {
            break;
        }
        match character {
            0x0d | 0x0a => {
                if character == 0x0d && characters.peek() == Some(&0x0a) {
                    characters.next();
                }
                if !reading.single_line {
                    push(Cell::Break, &mut filled);
                    column = 0;
                }
            }
            0x09 if reading.expand_tabs => {
                let spaces = TAB_CELLS - column % TAB_CELLS;
                for _ in 0..spaces {
                    push(Cell::Space, &mut filled);
                }
                column += spaces;
            }
            0x09 | 0x20 => {
                push(Cell::Space, &mut filled);
                column += 1;
                underline_next = false;
            }
            0x26 if reading.prefixes && characters.peek() != Some(&0x26) => {
                underline_next = true;
            }
            0x26 if reading.prefixes => {
                characters.next();
                push(glyph_cell(0x26, underline_next), &mut filled);
                column += 1;
                underline_next = false;
            }
            0x00..=0x1f | 0x7f => {}
            _ => {
                push(glyph_cell(character, underline_next), &mut filled);
                column += 1;
                underline_next = false;
            }
        }
    }
    filled
}

fn glyph_cell(character: u32, underline: bool) -> Cell {
    Cell::Glyph {
        id: font::glyph_id(character),
        underline,
    }
}

/// The lines of `cells`, as ranges of it: each break ends one (a break at the end makes an
/// empty last line, and no cells are one empty line), and with `max_cells` a longer line is
/// wrapped at the last space that fits, dropping the spaces at the wrap, or cut at `max_cells`
/// when it has none.
pub fn lines(cells: &[Cell], max_cells: Option<usize>) -> Lines<'_> {
    Lines {
        cells,
        max_cells: max_cells.map(|max| max.max(1)),
        at: 0,
        done: false,
    }
}

/// The iterator [`lines`] returns.
#[derive(Debug, Clone)]
pub struct Lines<'a> {
    cells: &'a [Cell],
    max_cells: Option<usize>,
    at: usize,
    done: bool,
}

impl Iterator for Lines<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        if self.done {
            return None;
        }
        let cells = self.cells;
        let start = self.at;
        let hard_end = cells[start..]
            .iter()
            .position(|cell| *cell == Cell::Break)
            .map_or(cells.len(), |offset| start + offset);
        let limit = match self.max_cells {
            Some(max) if hard_end - start > max => start + max,
            _ => {
                match hard_end == cells.len() {
                    true => self.done = true,
                    false => self.at = hard_end + 1,
                }
                return Some((start, hard_end));
            }
        };
        let Some(space) = (start + 1..=limit)
            .rev()
            .find(|&index| cells[index] == Cell::Space)
        else {
            self.at = limit;
            return Some((start, limit));
        };
        let mut end = space;
        while end > start && cells[end - 1] == Cell::Space {
            end -= 1;
        }
        let mut next = space;
        while next < cells.len() && cells[next] == Cell::Space {
            next += 1;
        }
        self.at = next;
        Some((start, end))
    }
}

/// Draws glyph `id` with its top-left pixel at (`x`, `y`): its ink in `foreground`, the rest of
/// its cell in `background` when there is one, and row 14 in `foreground` when `underline`.
pub fn glyph(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    id: u8,
    underline: bool,
    foreground: u32,
    background: Option<u32>,
) {
    let cell = Rect::sized(x, y, CELL_WIDTH, CELL_HEIGHT);
    if cell.intersection(&surface.clip()).is_empty() {
        return;
    }
    if let Some(background) = background {
        surface.fill(cell, background);
    }
    for (row, &bits) in font::rows(id).iter().enumerate() {
        let bits = if underline && row == UNDERLINE_ROW {
            0xff
        } else {
            bits
        };
        for column in 0..CELL_WIDTH {
            if bits & (0x80 >> column) != 0 {
                surface.pixel(
                    x.saturating_add(column),
                    y.saturating_add(row as i32),
                    foreground,
                );
            }
        }
    }
}

/// Draws glyph `id` at twice its size, a 16x32 cell whose top-left pixel is (`x`, `y`).
pub fn glyph_2x(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    id: u8,
    foreground: u32,
    background: Option<u32>,
) {
    if let Some(background) = background {
        surface.fill(
            Rect::sized(x, y, 2 * CELL_WIDTH, 2 * CELL_HEIGHT),
            background,
        );
    }
    for (row, &bits) in font::rows(id).iter().enumerate() {
        for column in 0..CELL_WIDTH {
            if bits & (0x80 >> column) != 0 {
                surface.fill(
                    Rect::sized(
                        x.saturating_add(2 * column),
                        y.saturating_add(2 * row as i32),
                        2,
                        2,
                    ),
                    foreground,
                );
            }
        }
    }
}

/// Draws `cells` on one line from (`x`, `y`).
pub fn draw_cells(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    cells: &[Cell],
    foreground: u32,
    background: Option<u32>,
) {
    for (index, cell) in cells.iter().enumerate() {
        let (id, underline) = cell.drawn();
        let at = x.saturating_add((index as i32).saturating_mul(CELL_WIDTH));
        glyph(surface, at, y, id, underline, foreground, background);
    }
}

/// `TextOutW`: the UTF-16 `units` from (`x`, `y`), one cell each (a character the font lacks,
/// a control character included, is `?`); returns the text's width.
pub fn text_out(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    units: &[u16],
    foreground: u32,
    background: Option<u32>,
) -> i32 {
    let mut at = x;
    for character in char::decode_utf16(units.iter().copied()) {
        let id = character.map_or(font::REPLACEMENT, |character| {
            font::glyph_id(u32::from(character))
        });
        glyph(surface, at, y, id, false, foreground, background);
        at = at.saturating_add(CELL_WIDTH);
    }
    at.saturating_sub(x)
}

/// The width and height of the UTF-16 `units` drawn on one line.
pub fn extent(units: &[u16]) -> (i32, i32) {
    let characters = char::decode_utf16(units.iter().copied()).count();
    ((characters as i32).saturating_mul(CELL_WIDTH), CELL_HEIGHT)
}

/// `DrawTextW`'s format flags this layer reads.
pub mod format {
    pub const CENTER: u32 = 0x1;
    pub const RIGHT: u32 = 0x2;
    pub const VCENTER: u32 = 0x4;
    pub const BOTTOM: u32 = 0x8;
    pub const WORDBREAK: u32 = 0x10;
    pub const SINGLELINE: u32 = 0x20;
    pub const EXPANDTABS: u32 = 0x40;
    pub const NOCLIP: u32 = 0x100;
    pub const CALCRECT: u32 = 0x400;
    pub const NOPREFIX: u32 = 0x800;
    pub const END_ELLIPSIS: u32 = 0x8000;
}

/// How `DrawTextW` with `flags` reads its text.
pub fn reading(flags: u32) -> Reading {
    Reading {
        prefixes: flags & format::NOPREFIX == 0,
        expand_tabs: flags & format::EXPANDTABS != 0,
        single_line: flags & format::SINGLELINE != 0,
    }
}

fn wrap_cells(rect: Rect, flags: u32) -> Option<usize> {
    let wraps = flags & format::WORDBREAK != 0 && flags & format::SINGLELINE == 0;
    wraps.then(|| (rect.width().max(0) / CELL_WIDTH) as usize)
}

/// `DrawTextW` with `DT_CALCRECT`: `rect` made as wide as the widest line and as tall as the
/// lines of `cells` laid out with `flags`.
pub fn measure(cells: &[Cell], rect: Rect, flags: u32) -> Rect {
    let (mut count, mut widest) = (0i32, 0usize);
    for (start, end) in lines(cells, wrap_cells(rect, flags)) {
        count += 1;
        widest = widest.max(end - start);
    }
    Rect::new(
        rect.left,
        rect.top,
        rect.left
            .saturating_add((widest as i32).saturating_mul(CELL_WIDTH)),
        rect.top.saturating_add(count.saturating_mul(CELL_HEIGHT)),
    )
}

fn centred(start: i32, room: i32, size: i32) -> i32 {
    let at = i64::from(start) + (i64::from(room) - i64::from(size)) / 2;
    at.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

const ELLIPSIS: [Cell; 3] = [Cell::Glyph {
    id: b'.',
    underline: false,
}; 3];

/// `DrawTextW`: `cells` laid out in `rect` with `flags` (left, centred or right; with
/// `DT_SINGLELINE` top, centred or bottom; wrapped with `DT_WORDBREAK`; a line too wide ended
/// with `...` under `DT_END_ELLIPSIS`), clipped to `rect` unless `DT_NOCLIP`; returns the
/// text's height, or with `DT_VCENTER` or `DT_BOTTOM` the offset from `rect`'s top to the
/// text's bottom.
pub fn draw_text(
    surface: &mut Surface<'_>,
    cells: &[Cell],
    rect: Rect,
    flags: u32,
    foreground: u32,
    background: Option<u32>,
) -> i32 {
    let single_line = flags & format::SINGLELINE != 0;
    let top = match () {
        () if single_line && flags & format::VCENTER != 0 => {
            centred(rect.top, rect.height(), CELL_HEIGHT)
        }
        () if single_line && flags & format::BOTTOM != 0 => rect.bottom.saturating_sub(CELL_HEIGHT),
        () => rect.top,
    };
    let saved = surface.clip();
    if flags & format::NOCLIP == 0 {
        surface.clip_to(rect);
    }
    let fit = (rect.width().max(0) / CELL_WIDTH) as usize;
    let mut y = top;
    for (start, end) in lines(cells, wrap_cells(rect, flags)) {
        let line = &cells[start..end];
        let cut = flags & format::END_ELLIPSIS != 0 && line.len() > fit;
        let shown = match cut {
            true => &line[..fit.saturating_sub(ELLIPSIS.len())],
            false => line,
        };
        let cells_wide = shown.len() + if cut { ELLIPSIS.len() } else { 0 };
        let width = (cells_wide as i32).saturating_mul(CELL_WIDTH);
        let x = match () {
            () if flags & format::CENTER != 0 => centred(rect.left, rect.width(), width),
            () if flags & format::RIGHT != 0 => rect.right.saturating_sub(width),
            () => rect.left,
        };
        draw_cells(surface, x, y, shown, foreground, background);
        if cut {
            let after = x.saturating_add((shown.len() as i32).saturating_mul(CELL_WIDTH));
            draw_cells(surface, after, y, &ELLIPSIS, foreground, background);
        }
        y = y.saturating_add(CELL_HEIGHT);
    }
    surface.set_clip(saved);
    match single_line && flags & (format::VCENTER | format::BOTTOM) != 0 {
        true => top.saturating_sub(rect.top).saturating_add(CELL_HEIGHT),
        false => y.saturating_sub(top),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn decoded(text: &str, reading: Reading) -> Vec<Cell> {
        let mut cells = vec![Cell::Space; 256];
        let filled = decode(&wide(text), reading, &mut cells);
        cells.truncate(filled);
        cells
    }

    fn shown(cells: &[Cell]) -> String {
        cells
            .iter()
            .map(|cell| match *cell {
                Cell::Glyph {
                    id,
                    underline: true,
                } => format!("_{}", id as char),
                Cell::Glyph { id, .. } => (id as char).to_string(),
                Cell::Space => " ".into(),
                Cell::Break => "|".into(),
            })
            .collect()
    }

    fn wrapped(text: &str, max: Option<usize>) -> Vec<String> {
        let cells = decoded(text, Reading::default());
        lines(&cells, max)
            .map(|(start, end)| shown(&cells[start..end]))
            .collect()
    }

    #[test]
    fn decoding_breaks_lines_drops_controls_and_marks_prefixes() {
        let plain = Reading::default();
        assert_eq!(shown(&decoded("a\r\nb\nc\rd", plain)), "a|b|c|d");
        assert_eq!(shown(&decoded("a\u{1}b\tc", plain)), "ab c");
        assert_eq!(shown(&decoded("é€😀", plain)), "???");
        let prefixed = Reading {
            prefixes: true,
            ..plain
        };
        assert_eq!(shown(&decoded("&Yes && no&", prefixed)), "_Yes & no");
        assert_eq!(shown(&decoded("&Yes", plain)), "&Yes");
        let tabs = Reading {
            expand_tabs: true,
            single_line: true,
            ..plain
        };
        assert_eq!(shown(&decoded("ab\tc\r\nd", tabs)), "ab      cd");
        let mut few = [Cell::Space; 2];
        assert_eq!(decode(&wide("abc"), plain, &mut few), 2);
    }

    #[test]
    fn lines_wrap_at_the_last_space_that_fits_and_cut_long_words() {
        assert_eq!(wrapped("", None), [""]);
        assert_eq!(wrapped("a\n", None), ["a", ""]);
        assert_eq!(wrapped("one two three", Some(7)), ["one two", "three"]);
        assert_eq!(wrapped("one two   three", Some(8)), ["one two", "three"]);
        assert_eq!(wrapped("abcdefghij", Some(4)), ["abcd", "efgh", "ij"]);
        assert_eq!(wrapped("ab cdefghij", Some(4)), ["ab", "cdef", "ghij"]);
        assert_eq!(wrapped("abcd efgh", Some(4)), ["abcd", "efgh"]);
        assert_eq!(wrapped("ab\ncd ef", Some(3)), ["ab", "cd", "ef"]);
    }

    fn picture(width: u32, height: u32, draw: impl FnOnce(&mut Surface<'_>)) -> Vec<u32> {
        let mut pixels = vec![0u32; (width * height) as usize];
        let mut surface = Surface::new(&mut pixels, width, height).unwrap();
        draw(&mut surface);
        pixels
    }

    #[test]
    fn glyphs_draw_the_fonts_rows_with_an_optional_background_and_underline() {
        let pixels = picture(8, 16, |s| glyph(s, 0, 0, b'i', true, 1, Some(2)));
        for (row, &bits) in font::rows(b'i').iter().enumerate() {
            for column in 0..8 {
                let ink = bits & (0x80 >> column) != 0 || row == UNDERLINE_ROW;
                assert_eq!(pixels[row * 8 + column], if ink { 1 } else { 2 });
            }
        }
        let pixels = picture(16, 32, |s| glyph_2x(s, 0, 0, b'i', 1, None));
        for (row, &bits) in font::rows(b'i').iter().enumerate() {
            for column in 0..8 {
                let ink = u32::from(bits & (0x80 >> column) != 0);
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    assert_eq!(pixels[(2 * row + dy) * 16 + 2 * column + dx], ink);
                }
            }
        }
    }

    #[test]
    fn text_out_draws_one_cell_per_character_and_extent_counts_them() {
        let text = wide("Hi😀");
        assert_eq!(extent(&text), (24, 16));
        let mut width = 0;
        let pixels = picture(40, 16, |s| width = text_out(s, 4, 0, &text, 1, Some(2)));
        assert_eq!(width, 24);
        assert_eq!(pixels[3], 0);
        assert_eq!(pixels[4], 2);
        assert_eq!(pixels[28], 0);
        let expected = picture(40, 16, |s| glyph(s, 20, 0, b'?', false, 1, Some(2)));
        assert_eq!(pixels[20..28], expected[20..28]);
    }

    #[test]
    fn draw_text_aligns_centres_wraps_clips_and_measures() {
        let cells = decoded("ab", Reading::default());
        let rect = Rect::new(0, 0, 40, 40);
        let flags = format::CENTER | format::VCENTER | format::SINGLELINE;
        let mut offset = 0;
        let pixels = picture(40, 40, |s| {
            offset = draw_text(s, &cells, rect, flags, 1, Some(2))
        });
        assert_eq!(offset, 12 + 16);
        let expected = picture(40, 40, |s| draw_cells(s, 12, 12, &cells, 1, Some(2)));
        assert_eq!(pixels, expected);

        let cells = decoded("one two", Reading::default());
        let rect = Rect::new(0, 0, 32, 40);
        assert_eq!(
            measure(&cells, rect, format::WORDBREAK),
            Rect::new(0, 0, 24, 32)
        );
        assert_eq!(measure(&cells, rect, 0), Rect::new(0, 0, 56, 16));
        let mut height = 0;
        let pixels = picture(40, 40, |s| {
            height = draw_text(
                s,
                &cells,
                Rect::new(0, 0, 20, 40),
                format::RIGHT,
                1,
                Some(2),
            )
        });
        assert_eq!(height, 16);
        assert!(
            pixels
                .chunks(40)
                .all(|row| row[20..].iter().all(|&p| p == 0))
        );

        let cells = decoded("abcdefgh", Reading::default());
        let pixels = picture(40, 16, |s| {
            draw_text(
                s,
                &cells,
                Rect::new(0, 0, 40, 16),
                format::END_ELLIPSIS,
                1,
                Some(2),
            );
        });
        let expected = picture(40, 16, |s| {
            draw_cells(s, 0, 0, &cells[..2], 1, Some(2));
            draw_cells(s, 16, 0, &ELLIPSIS, 1, Some(2));
        });
        assert_eq!(pixels, expected);
    }
}
