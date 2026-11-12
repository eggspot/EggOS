//! An outline: rows in levels, each with a disclosure glyph that opens or closes it
//! (docs/ux/windows-registry-v0.3.md section 4, D401). Like the table, the tree keeps no row
//! text: the program keeps the flattened list of shown rows (an open row's children follow it)
//! and the widget asks it for a row when it draws or describes one.

use crate::canvas::Canvas;
use crate::geometry::Rect;
use crate::text::Text;
use crate::widget::{
    LINE_HEIGHT, Look, ScrollBars, THUMB, chevron, clipped_text, focus_ring, scroll_bars,
    viewport,
};

/// A row's height.
pub const OUTLINE_ROW_HEIGHT: i32 = 24;
/// How far each level is indented.
pub const OUTLINE_INDENT: i32 = 16;
/// The width of a row's glyph cell, which toggles it.
pub const OUTLINE_GLYPH_CELL: i32 = 16;
const GLYPH_X: i32 = 8;
const GLYPH_TOP: i32 = 7;
const LEAF_X: i32 = 11;
const LEAF_TOP: i32 = 10;
const LEAF_SIZE: i32 = 4;
const TEXT_X: i32 = 24;
const TEXT_TOP: i32 = 4;
const TEXT_END: i32 = 8;
const BAR_ROOM: i32 = 8;

/// The first part number of an outline's rows under the pointer (by row index).
pub const ROW_HIT: usize = 1 << 24;
/// The first part number of an outline's glyph cells under the pointer (by row index).
pub const GLYPH_HIT: usize = 1 << 25;

/// One shown row, as the program describes it.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutlineRow {
    /// Its level: 0 for a root.
    pub depth: u8,
    /// Whether it has children to show.
    pub expandable: bool,
    /// Whether its children show now.
    pub expanded: bool,
    pub text: Text<64>,
}

/// Describes shown row `row` into `out`, from the program's `data`.
pub type RowText = fn(data: u32, row: usize, out: &mut OutlineRow);
/// Writes shown row `row`'s full path, its value for agents, into `out`.
pub type RowPath = fn(data: u32, row: usize, out: &mut Text<256>);

/// An outline's state.
#[derive(Debug, Clone, Copy)]
pub struct Outline {
    /// Its name for agents.
    pub name: &'static str,
    pub rows: usize,
    pub row: RowText,
    pub path: Option<RowPath>,
    pub data: u32,
    pub selected: usize,
    pub offset_y: i32,
    pub empty_text: &'static str,
    /// One level a page, as a phone browses (D401): no glyphs, and a press or Enter on a row
    /// asks to open it.
    pub drill_down: bool,
}

/// What a key does to an outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Select this row.
    Select(usize),
    /// Open or close this row.
    Toggle(usize),
    /// Leave the widget that way (a TV remote's Left on a root, Right on a leaf).
    Leave,
    /// Nothing: the key is the outline's but has nothing to do here.
    Stay,
    /// The key is not the outline's.
    Unused,
}

impl Outline {
    /// An outline of `rows` shown rows that `row` describes from `data`, the first selected.
    pub fn new(name: &'static str, rows: usize, row: RowText, data: u32) -> Self {
        Self {
            name,
            rows,
            row,
            path: None,
            data,
            selected: 0,
            offset_y: 0,
            empty_text: "No items",
            drill_down: false,
        }
    }

    /// Row `row` as the program describes it.
    pub fn row_of(&self, row: usize) -> OutlineRow {
        let mut out = OutlineRow::default();
        (self.row)(self.data, row, &mut out);
        out
    }

    /// Row `row`'s value for agents: its path, or its text without a path function.
    pub fn path_of(&self, row: usize) -> Text<256> {
        let mut out = Text::new();
        match self.path {
            Some(path) => path(self.data, row, &mut out),
            None => {
                out.push_str(self.row_of(row).text.as_str());
            }
        }
        out
    }

    /// Every row's height together.
    pub fn content_height(&self) -> i32 {
        (self.rows as i64 * i64::from(OUTLINE_ROW_HEIGHT)).min(i64::from(i32::MAX / 2)) as i32
    }

    /// The size its content needs.
    pub fn content_size(&self) -> (i32, i32) {
        (160, self.content_height())
    }

    /// The part of `rect` its rows show through: less the vertical bar when it shows.
    pub fn view(&self, rect: Rect) -> Rect {
        viewport(rect, (0, self.content_height()))
    }

    /// Its vertical scroll bar's track and thumb in an outline at `rect`.
    pub fn bars(&self, rect: Rect) -> ScrollBars {
        scroll_bars(rect, (0, self.content_height()), (0, self.offset_y))
    }

    /// Keeps the offset within what its rows and an outline at `rect` allow.
    pub fn clamp_offsets(&mut self, rect: Rect) {
        let view = self.view(rect);
        self.offset_y = self
            .offset_y
            .clamp(0, (self.content_height() - view.height).max(0));
    }

    /// Row `row`'s rectangle in an outline at `rect`.
    pub fn row_rect(&self, rect: Rect, row: usize) -> Rect {
        let view = self.view(rect);
        Rect::new(
            rect.x,
            view.y + row as i32 * OUTLINE_ROW_HEIGHT - self.offset_y,
            view.width,
            OUTLINE_ROW_HEIGHT,
        )
    }

    /// The rows that meet an outline at `rect`.
    pub fn shown_rows(&self, rect: Rect) -> core::ops::Range<usize> {
        let view = self.view(rect);
        if self.rows == 0 || view.height <= 0 {
            return 0..0;
        }
        let first = (self.offset_y / OUTLINE_ROW_HEIGHT).max(0) as usize;
        let last = ((self.offset_y + view.height - 1) / OUTLINE_ROW_HEIGHT).max(0) as usize;
        first.min(self.rows)..(last + 1).min(self.rows)
    }

    /// How many whole rows an outline at `rect` shows.
    pub fn page(&self, rect: Rect) -> usize {
        (self.view(rect).height / OUTLINE_ROW_HEIGHT).max(1) as usize
    }

    fn glyph_left(rect: Rect, depth: u8) -> i32 {
        rect.x + i32::from(depth) * OUTLINE_INDENT
    }

    /// The part at (`x`, `y`) of an outline at `rect`: the bar's thumb, a row's glyph cell
    /// (from [`GLYPH_HIT`]) or a row (from [`ROW_HIT`]).
    pub fn part_at(&self, rect: Rect, x: i32, y: i32) -> Option<usize> {
        if let (Some((track, _)), _) = self.bars(rect)
            && track.contains(x, y)
        {
            return Some(THUMB);
        }
        let view = self.view(rect);
        if !view.contains(x, y) {
            return None;
        }
        let row = ((y - view.y + self.offset_y) / OUTLINE_ROW_HEIGHT).max(0) as usize;
        if row >= self.rows {
            return None;
        }
        let shown = self.row_of(row);
        let glyph = Self::glyph_left(rect, shown.depth) + GLYPH_X - 4;
        let on_glyph = (glyph..glyph + OUTLINE_GLYPH_CELL).contains(&x);
        match !self.drill_down && shown.expandable && on_glyph {
            true => Some(GLYPH_HIT + row),
            false => Some(ROW_HIT + row),
        }
    }

    /// The offset that shows row `row` whole in an outline at `rect`.
    pub fn offset_showing(&self, rect: Rect, row: usize) -> i32 {
        let view = self.view(rect);
        let top = row as i32 * OUTLINE_ROW_HEIGHT;
        let bottom = top + OUTLINE_ROW_HEIGHT;
        match () {
            () if top < self.offset_y => top,
            () if bottom > self.offset_y + view.height => bottom - view.height,
            () => self.offset_y,
        }
    }

    /// The row above `row` one level up, if any.
    pub fn parent_of(&self, row: usize) -> Option<usize> {
        let depth = self.row_of(row).depth;
        (0..row).rev().find(|&above| self.row_of(above).depth < depth)
    }

    /// The next row after the selected one, cycling, whose text starts with `typed`, ignoring
    /// case.
    pub fn row_starting_with(&self, typed: char) -> Option<usize> {
        let wanted = typed.to_lowercase().next()?;
        (1..=self.rows)
            .map(|step| (self.selected + step) % self.rows.max(1))
            .find(|&row| {
                self.row_of(row)
                    .text
                    .as_str()
                    .chars()
                    .next()
                    .and_then(|first| first.to_lowercase().next())
                    == Some(wanted)
            })
    }

    /// What `code` does, the outline having `page` whole rows in view; `tv` when a TV remote
    /// drives it.
    pub fn step_for(&self, code: u16, page: usize, tv: bool) -> Step {
        use crate::input::key;
        if self.rows == 0 {
            return Step::Unused;
        }
        let here = self.selected.min(self.rows - 1);
        let last = self.rows - 1;
        let shown = self.row_of(here);
        if self.drill_down {
            return match code {
                key::UP => Step::Select(here.saturating_sub(1)),
                key::DOWN => Step::Select((here + 1).min(last)),
                key::HOME => Step::Select(0),
                key::END => Step::Select(last),
                key::ENTER => Step::Toggle(here),
                _ => Step::Unused,
            };
        }
        match code {
            key::UP => Step::Select(here.saturating_sub(1)),
            key::DOWN => Step::Select((here + 1).min(last)),
            key::HOME => Step::Select(0),
            key::END => Step::Select(last),
            key::PAGE_UP => Step::Select(here.saturating_sub(page.saturating_sub(1).max(1))),
            key::PAGE_DOWN => Step::Select((here + page.saturating_sub(1).max(1)).min(last)),
            key::RIGHT => match (shown.expandable, shown.expanded) {
                (true, false) => Step::Toggle(here),
                (true, true) if here < last => Step::Select(here + 1),
                _ if tv => Step::Leave,
                _ => Step::Stay,
            },
            key::LEFT => match (shown.expanded, self.parent_of(here)) {
                (true, _) => Step::Toggle(here),
                (false, Some(parent)) => Step::Select(parent),
                (false, None) if tv => Step::Leave,
                (false, None) => Step::Stay,
            },
            key::BACKSPACE => self.parent_of(here).map_or(Step::Stay, Step::Select),
            key::ENTER if shown.expandable => Step::Toggle(here),
            _ => Step::Unused,
        }
    }

    /// Draws it at `rect` as `look` says.
    pub fn paint(&self, canvas: &mut Canvas<'_>, rect: Rect, look: Look) {
        let palette = canvas.palette();
        canvas.fill(rect, palette.menu);
        let view = self.view(rect);
        let previous = canvas.narrow(view);
        let text_top = (OUTLINE_ROW_HEIGHT - LINE_HEIGHT) / 2;
        if self.rows == 0 {
            canvas.text(
                rect.x + TEXT_X,
                view.y + text_top,
                self.empty_text,
                palette.text_dim,
                1,
            );
        }
        let clip = canvas.clip();
        let end = view.right() - TEXT_END;
        for row in self.shown_rows(rect) {
            let row_rect = self.row_rect(rect, row);
            if row_rect.intersection(&clip).is_empty() {
                continue;
            }
            let chosen = row == self.selected;
            if chosen {
                canvas.fill(row_rect, palette.selection);
            } else if [ROW_HIT + row, GLYPH_HIT + row].map(Some).contains(&look.hovered) {
                canvas.fill(row_rect, palette.menu_hover);
            }
            let shown = self.row_of(row);
            let left = Self::glyph_left(rect, shown.depth);
            let glyph_ink = match (look.enabled, chosen) {
                (true, true) => palette.accent,
                _ => palette.text_dim,
            };
            let glyph_top = row_rect.y + GLYPH_TOP;
            match (shown.expandable, shown.expanded) {
                _ if self.drill_down => {}
                (true, true) => chevron(canvas, left + GLYPH_X - 2, glyph_top + 2, true, glyph_ink),
                (true, false) => side_chevron(canvas, left + GLYPH_X, glyph_top, glyph_ink),
                (false, _) => canvas.fill(
                    Rect::new(left + LEAF_X, row_rect.y + LEAF_TOP, LEAF_SIZE, LEAF_SIZE),
                    glyph_ink,
                ),
            }
            let ink = match look.enabled {
                true => palette.text,
                false => palette.text_dim,
            };
            let x = left + TEXT_X;
            let room = match self.bars(rect).0 {
                Some(_) => end - x - BAR_ROOM,
                None => end - x,
            };
            if room > 0 {
                let words = shown.text.as_str();
                clipped_text(canvas, x, row_rect.y + TEXT_TOP, room, words, ink);
            }
            if chosen && look.focused {
                focus_ring(canvas, row_rect, 0);
            }
        }
        canvas.restore(previous);
        if let (Some((track, thumb)), _) = self.bars(rect) {
            canvas.fill(track, palette.progress_track);
            let colour = match look.hovered == Some(THUMB) || look.pressed == Some(THUMB) {
                true => palette.button_inactive,
                false => palette.bar_pressed,
            };
            canvas.rounded(thumb, &crate::canvas::INSET_R4, colour);
        }
    }
}

fn side_chevron(canvas: &mut Canvas<'_>, left: i32, top: i32, colour: u32) {
    for index in 0..9 {
        let from_tip = 4 - (index - 4i32).abs();
        canvas.fill(Rect::new(left + from_tip, top + index, 2, 2), colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::key;

    const SHAPE: [(u8, bool, bool, &str); 5] = [
        (0, true, true, "HKEY_CURRENT_USER"),
        (1, true, false, "Software"),
        (1, false, false, "Environment"),
        (0, true, false, "HKEY_LOCAL_MACHINE"),
        (0, false, false, "HKEY_CLASSES_ROOT"),
    ];

    fn describe(_: u32, row: usize, out: &mut OutlineRow) {
        let (depth, expandable, expanded, text) = SHAPE[row];
        out.depth = depth;
        out.expandable = expandable;
        out.expanded = expanded;
        out.text.push_str(text);
    }

    fn outline(selected: usize) -> Outline {
        Outline {
            selected,
            ..Outline::new("Keys", SHAPE.len(), describe, 0)
        }
    }

    #[test]
    fn right_opens_a_closed_row_and_enters_an_open_one() {
        assert_eq!(outline(1).step_for(key::RIGHT, 5, false), Step::Toggle(1));
        assert_eq!(outline(0).step_for(key::RIGHT, 5, false), Step::Select(1));
        assert_eq!(outline(2).step_for(key::RIGHT, 5, false), Step::Stay);
        assert_eq!(outline(2).step_for(key::RIGHT, 5, true), Step::Leave);
    }

    #[test]
    fn left_closes_an_open_row_or_goes_to_the_parent() {
        assert_eq!(outline(0).step_for(key::LEFT, 5, false), Step::Toggle(0));
        assert_eq!(outline(2).step_for(key::LEFT, 5, false), Step::Select(0));
        assert_eq!(outline(3).step_for(key::LEFT, 5, false), Step::Stay);
        assert_eq!(outline(3).step_for(key::LEFT, 5, true), Step::Leave);
        assert_eq!(outline(2).step_for(key::BACKSPACE, 5, false), Step::Select(0));
    }

    #[test]
    fn typing_finds_the_next_row_by_its_first_letter() {
        assert_eq!(outline(0).row_starting_with('h'), Some(3));
        assert_eq!(outline(3).row_starting_with('H'), Some(4));
        assert_eq!(outline(4).row_starting_with('h'), Some(0));
        assert_eq!(outline(0).row_starting_with('z'), None);
    }

    #[test]
    fn a_press_on_the_glyph_is_the_glyph_and_elsewhere_the_row() {
        let rect = Rect::new(0, 0, 200, 120);
        let held = outline(0);
        assert_eq!(held.part_at(rect, 24, 30), Some(GLYPH_HIT + 1));
        assert_eq!(held.part_at(rect, 100, 30), Some(ROW_HIT + 1));
        assert_eq!(held.part_at(rect, 24, 54), Some(ROW_HIT + 2));
        assert_eq!(held.part_at(rect, 8, 6), Some(GLYPH_HIT));
        assert_eq!(held.part_at(rect, 100, 119), Some(ROW_HIT + 4));
    }

    #[test]
    fn the_offset_keeps_the_selected_row_in_view() {
        let rect = Rect::new(0, 0, 200, 48);
        let held = outline(4);
        assert_eq!(held.offset_showing(rect, 4), 72);
        assert_eq!(held.parent_of(2), Some(0));
        assert_eq!(held.parent_of(0), None);
    }
}
