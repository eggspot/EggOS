//! `COMBOBOX` with `CBS_DROPDOWNLIST` (section 10 of the design, D174): the [`ComboBox`] model
//! (its items and selection in a [`ListBox`], whether its list is open, the row the list
//! highlights and the selection to go back to on a cancel), the closed box, and the list laid
//! out in the window's content and painted over it, as a menu's drop-down is.

use super::list_box::{ITEM_HEIGHT, ListBox};
use super::scroll_bar::{self, CHEVRON_DOWN, ScrollInfo, draw_chevron};
use super::{MAX_LABEL_CELLS, Palette, dotted_outline, draw_fitted, framed, label_cells, ring};
use crate::surface::{Rect, Surface};
use crate::text::Cell;

/// The closed box's height, whatever height the program gave.
pub const BOX_HEIGHT: i32 = 24;
const ARROW_AREA: i32 = 17;
const TEXT_LEFT: i32 = 4;
const TEXT_TOP: i32 = 4;
const TEXT_ROOM_LESS: i32 = 25;
const CHEVRON_FROM_RIGHT: i32 = 13;
const CHEVRON_TOP: i32 = 10;
const FOCUS_AREA_INSET: i32 = 3;
const FOCUS_AREA_HEIGHT: i32 = 18;
const ROW_TEXT_LEFT: i32 = 4;
const ROW_TEXT_TOP: i32 = 2;
const MOST_ROWS: i32 = 30;
const LIST_ALLOWANCE: i32 = 26;
const LIST_BORDERS: i32 = 2;

/// `COMBOBOX`'s styles this layer reads.
pub mod style {
    pub const TYPE: u32 = 0x3;
    pub const SIMPLE: u32 = 0x1;
    pub const DROP_DOWN: u32 = 0x2;
    pub const DROP_DOWN_LIST: u32 = 0x3;
    pub const SORT: u32 = 0x100;
}

/// The keys a combo box reads, as Windows' virtual keys.
pub mod key {
    pub const RETURN: u32 = 0x0d;
    pub const ESCAPE: u32 = 0x1b;
    pub const PAGE_UP: u32 = 0x21;
    pub const PAGE_DOWN: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const F4: u32 = 0x73;
}

/// How an open list closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// A row was chosen; whether the selection changed.
    Chosen { changed: bool },
    /// Cancelled: the selection is what it was when the list opened.
    Cancelled,
}

/// A drop-down list combo box's state.
#[derive(Debug, Clone)]
pub struct ComboBox {
    pub items: ListBox,
    pub list_height: i32,
    open: bool,
    highlighted: Option<usize>,
    at_opening: Option<usize>,
    top: usize,
}

impl ComboBox {
    /// A combo box the program gave `list_height` pixels: the closed box and its list.
    pub fn new(list_height: i32) -> ComboBox {
        ComboBox {
            items: ListBox::new(),
            list_height,
            open: false,
            highlighted: None,
            at_opening: None,
            top: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn selected(&self) -> Option<usize> {
        self.items.selected()
    }

    /// The row the open list highlights.
    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// The first item the open list shows.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The rows the list shows.
    pub fn rows(&self) -> usize {
        list_rows(self.list_height, self.items.count())
    }

    /// `CB_SETCURSEL`: `None` clears; whether the index was valid.
    pub fn select(&mut self, index: Option<usize>) -> bool {
        let valid = self.items.select(index, 1);
        if self.open {
            self.highlighted = self.items.selected();
            self.scroll_to_highlight();
        }
        valid
    }

    /// Opens the list with the selection highlighted and in view.
    pub fn open(&mut self) {
        self.open = true;
        self.at_opening = self.items.selected();
        self.highlighted = self.items.selected();
        self.top = 0;
        self.scroll_to_highlight();
    }

    /// Closes the list: `choose` selects the highlighted row, else the selection goes back to
    /// what it was when the list opened.
    pub fn close(&mut self, choose: bool) -> Closed {
        self.open = false;
        let at_opening = self.at_opening;
        match (choose, self.highlighted) {
            (true, Some(row)) => {
                self.items.select(Some(row), 1);
                Closed::Chosen {
                    changed: at_opening != Some(row),
                }
            }
            _ => {
                self.items.select(at_opening, 1);
                Closed::Cancelled
            }
        }
    }

    /// The pointer is over shown row `row` of the open list (`None`: over none); whether the
    /// highlight moved.
    pub fn point_at_row(&mut self, row: Option<usize>) -> bool {
        let Some(row) = row else {
            return false;
        };
        let index = self.top + row;
        if index >= self.items.count() || self.highlighted == Some(index) {
            return false;
        }
        self.highlighted = Some(index);
        true
    }

    /// Scrolls the open list by `rows` (up when negative); whether it moved.
    pub fn scroll_by(&mut self, rows: isize) -> bool {
        let last_top = self.items.count().saturating_sub(self.rows());
        let top = self.top.saturating_add_signed(rows).min(last_top);
        let moved = top != self.top;
        self.top = top;
        moved
    }

    fn target(&self, virtual_key: u32, from: Option<usize>) -> Option<usize> {
        let count = self.items.count();
        let last = count.checked_sub(1)?;
        let page = self.rows().max(2) - 1;
        Some(match (virtual_key, from) {
            (key::UP | key::LEFT, Some(at)) => at.saturating_sub(1),
            (key::DOWN | key::RIGHT, Some(at)) => (at + 1).min(last),
            (key::UP | key::LEFT | key::DOWN | key::RIGHT, None) => 0,
            (key::HOME, _) => 0,
            (key::END, _) => last,
            (key::PAGE_UP, at) => at.unwrap_or(0).saturating_sub(page),
            (key::PAGE_DOWN, at) => (at.unwrap_or(0) + page).min(last),
            _ => return None,
        })
    }

    /// A navigation key while the list is closed; whether the selection changed.
    pub fn key_closed(&mut self, virtual_key: u32) -> bool {
        let Some(target) = self.target(virtual_key, self.items.selected()) else {
            return false;
        };
        let changed = self.items.selected() != Some(target);
        self.items.select(Some(target), 1);
        changed
    }

    /// A navigation key while the list is open; whether the highlight moved.
    pub fn key_open(&mut self, virtual_key: u32) -> bool {
        let Some(target) = self.target(virtual_key, self.highlighted) else {
            return false;
        };
        let moved = self.highlighted != Some(target);
        self.highlighted = Some(target);
        self.scroll_to_highlight();
        moved
    }

    /// A printable key: the next item after the current one, wrapping, whose first character
    /// matches ignoring ASCII case; whether one did.
    pub fn character(&mut self, character: u32) -> bool {
        let count = self.items.count();
        let current = match self.open {
            true => self.highlighted,
            false => self.items.selected(),
        };
        let fold = |unit: u32| match unit {
            0x41..=0x5a => unit + 0x20,
            unit => unit,
        };
        let start = current.map_or(0, |at| at + 1);
        let found = (0..count)
            .map(|step| (start + step) % count)
            .find(|&index| {
                self.items
                    .text(index)
                    .and_then(|text| text.first())
                    .is_some_and(|&first| fold(u32::from(first)) == fold(character))
            });
        let Some(found) = found else {
            return false;
        };
        match self.open {
            true => {
                self.highlighted = Some(found);
                self.scroll_to_highlight();
            }
            false => {
                self.items.select(Some(found), 1);
            }
        }
        true
    }

    fn scroll_to_highlight(&mut self) {
        let rows = self.rows().max(1);
        if let Some(row) = self.highlighted {
            if row < self.top {
                self.top = row;
            }
            if row >= self.top + rows {
                self.top = row + 1 - rows;
            }
        }
    }

    /// The scroll bar's values for the open list.
    pub fn scroll_info(&self) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: self.items.count() as i32 - 1,
            page: self.rows() as u32,
            position: self.top as i32,
        }
    }
}

/// The closed box of a combo box at `rect`: its top 24 pixels.
pub fn closed_box(rect: Rect) -> Rect {
    Rect::sized(rect.left, rect.top, rect.width(), BOX_HEIGHT)
}

/// The rows a list of `count` items shows for a combo box the program gave `list_height`.
pub fn list_rows(list_height: i32, count: usize) -> usize {
    let fit = ((list_height - LIST_ALLOWANCE) / ITEM_HEIGHT).clamp(1, MOST_ROWS) as usize;
    count.min(fit)
}

/// Where the list of `rows` rows opens for the closed box `closed`, in a content `content`:
/// below the box when it fits, else above when that fits, else below, clipped; never past the
/// content's right edge.
pub fn list_rect(closed: Rect, rows: usize, content: Rect) -> Rect {
    let height = LIST_BORDERS + ITEM_HEIGHT * rows as i32;
    let width = closed.width();
    let left = closed.left.min(content.right - width).max(content.left);
    let below = Rect::sized(left, closed.bottom, width, height);
    let above = Rect::sized(left, closed.top - height, width, height);
    match () {
        () if below.bottom <= content.bottom => below,
        () if above.top >= content.top => above,
        () => below,
    }
}

/// What the point (`x`, `y`) is on in the open list at `list`: a shown row, or the scroll bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPart {
    Row(usize),
    ScrollBar(scroll_bar::Part),
}

/// The part of the open list at `list` under (`x`, `y`), if the point is inside it.
pub fn list_part_at(list: Rect, combo: &ComboBox, x: i32, y: i32) -> Option<ListPart> {
    let inner = list.inset(1);
    if !inner.contains(x, y) {
        return None;
    }
    let info = combo.scroll_info();
    if info.scrolls() && x >= inner.right - scroll_bar::WIDTH {
        let bar = Rect::new(
            inner.right - scroll_bar::WIDTH,
            inner.top,
            inner.right,
            inner.bottom,
        );
        return scroll_bar::part_at(bar, &info, y).map(ListPart::ScrollBar);
    }
    let row = ((y - inner.top) / ITEM_HEIGHT) as usize;
    (row < combo.rows()).then_some(ListPart::Row(row))
}

/// How a combo box looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComboLook {
    pub focused: bool,
    pub enabled: bool,
}

fn selected_cells(combo: &ComboBox, cells: &mut [Cell; MAX_LABEL_CELLS]) -> usize {
    let text = combo
        .selected()
        .and_then(|index| combo.items.text(index))
        .unwrap_or(&[]);
    label_cells(text, false, cells)
}

/// Paints the closed box of the combo box at `rect` (24 pixels of it).
pub fn paint_closed(
    surface: &mut Surface<'_>,
    rect: Rect,
    combo: &ComboBox,
    look: ComboLook,
    palette: &Palette,
) {
    let rect = closed_box(rect);
    let saved = surface.clip();
    surface.clip_to(rect);
    let (fill, border, mut text) = match () {
        () if !look.enabled => (
            palette.button_disabled_fill,
            palette.button_disabled_border,
            palette.text_disabled,
        ),
        () if combo.is_open() => (
            palette.button_pressed_fill,
            palette.button_pressed_border,
            palette.text,
        ),
        () => (palette.button_fill, palette.button_border, palette.text),
    };
    framed(surface, rect, palette.rounded, border, fill);
    let chevron = text;
    let focus_area = Rect::sized(
        rect.left + FOCUS_AREA_INSET,
        rect.top + FOCUS_AREA_INSET,
        rect.width() - ARROW_AREA - FOCUS_AREA_INSET,
        FOCUS_AREA_HEIGHT,
    );
    let focused = look.focused && look.enabled && !combo.is_open();
    if focused {
        surface.fill(focus_area, palette.highlight);
        text = palette.highlight_text;
    }
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = selected_cells(combo, &mut cells);
    draw_fitted(
        surface,
        rect.left + TEXT_LEFT,
        rect.top + TEXT_TOP,
        &cells[..count],
        rect.width() - TEXT_ROOM_LESS,
        text,
    );
    draw_chevron(
        surface,
        rect.right - CHEVRON_FROM_RIGHT,
        rect.top + CHEVRON_TOP,
        &CHEVRON_DOWN,
        chevron,
    );
    if focused {
        match palette.rounded {
            true => ring(surface, rect.inset(2), palette.focus),
            false => dotted_outline(surface, focus_area, palette.highlight_text),
        }
    }
    surface.set_clip(saved);
}

/// Paints the open list of `combo` at `list`.
pub fn paint_list(surface: &mut Surface<'_>, list: Rect, combo: &ComboBox, palette: &Palette) {
    let saved = surface.clip();
    surface.clip_to(list);
    framed(
        surface,
        list,
        palette.rounded,
        palette.field_border,
        palette.field_fill,
    );
    let inner = list.inset(1);
    let info = combo.scroll_info();
    let rows_right = match info.scrolls() {
        true => inner.right - scroll_bar::WIDTH,
        false => inner.right,
    };
    surface.clip_to(inner);
    for row in 0..combo.rows() {
        let index = combo.top() + row;
        let Some(text) = combo.items.text(index) else {
            break;
        };
        let top = inner.top + ITEM_HEIGHT * row as i32;
        let rect = Rect::new(inner.left, top, rows_right, top + ITEM_HEIGHT);
        let colour = match combo.highlighted() == Some(index) {
            true => {
                surface.fill(rect, palette.highlight);
                palette.highlight_text
            }
            false => palette.text,
        };
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = label_cells(text, false, &mut cells);
        let x = rect.left + ROW_TEXT_LEFT;
        draw_fitted(
            surface,
            x,
            top + ROW_TEXT_TOP,
            &cells[..count],
            rect.right - x,
            colour,
        );
    }
    if info.scrolls() {
        let bar = Rect::new(rows_right, inner.top, inner.right, inner.bottom);
        scroll_bar::paint_vertical(surface, bar, &info, true, palette);
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

    fn pegs_board_types() -> ComboBox {
        let mut combo = ComboBox::new(328);
        for name in ["Cross", "Octagon", "Random"] {
            combo.items.add(&units(name), false);
        }
        combo.select(Some(0));
        combo
    }

    #[test]
    fn the_list_shows_at_most_what_the_given_height_and_thirty_rows_allow() {
        assert_eq!(list_rows(328, 3), 3);
        assert_eq!(list_rows(328, 40), 15);
        assert_eq!(list_rows(26, 40), 1);
        assert_eq!(list_rows(1000, 40), 30);
        assert_eq!(list_rows(1000, 0), 0);
    }

    #[test]
    fn the_list_opens_below_else_above_else_below_clipped() {
        let content = Rect::new(0, 0, 368, 148);
        let closed = Rect::sized(112, 72, 240, 24);
        assert_eq!(list_rect(closed, 3, content), Rect::sized(112, 10, 240, 62));
        let high = Rect::sized(112, 8, 240, 24);
        assert_eq!(list_rect(high, 3, content), Rect::sized(112, 32, 240, 62));
        let tall = Rect::new(0, 0, 368, 80);
        assert_eq!(list_rect(closed, 3, tall), Rect::sized(112, 10, 240, 62));
        assert_eq!(list_rect(closed, 5, tall), Rect::sized(112, 96, 240, 102));
        let narrow = Rect::new(0, 0, 300, 400);
        assert_eq!(list_rect(closed, 3, narrow).left, 60);
    }

    #[test]
    fn choosing_a_row_selects_it_and_a_cancel_restores_the_selection() {
        let mut combo = pegs_board_types();
        combo.open();
        assert_eq!(combo.highlighted(), Some(0));
        assert!(combo.point_at_row(Some(2)));
        assert!(!combo.point_at_row(Some(2)));
        assert_eq!(combo.selected(), Some(0));
        assert_eq!(combo.close(true), Closed::Chosen { changed: true });
        assert_eq!(combo.selected(), Some(2));
        combo.open();
        assert!(combo.key_open(key::UP));
        assert_eq!(combo.close(false), Closed::Cancelled);
        assert_eq!(combo.selected(), Some(2));
        combo.open();
        assert_eq!(combo.close(true), Closed::Chosen { changed: false });
    }

    #[test]
    fn closed_keys_and_letters_move_the_selection() {
        let mut combo = pegs_board_types();
        assert!(combo.key_closed(key::DOWN));
        assert_eq!(combo.selected(), Some(1));
        assert!(combo.key_closed(key::END));
        assert!(!combo.key_closed(key::DOWN));
        assert!(combo.key_closed(key::HOME));
        assert!(combo.character(u32::from(b'r')));
        assert_eq!(combo.selected(), Some(2));
        assert!(!combo.character(u32::from(b'z')));
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_highlight_in_view() {
        let mut combo = ComboBox::new(86);
        for index in 0..10 {
            combo.items.add(&units(&format!("item {index}")), false);
        }
        assert_eq!(combo.rows(), 3);
        combo.open();
        for _ in 0..5 {
            combo.key_open(key::DOWN);
        }
        assert_eq!((combo.highlighted(), combo.top()), (Some(4), 2));
        assert!(combo.scroll_by(10));
        assert_eq!(combo.top(), 7);
        let list = Rect::sized(0, 0, 100, 62);
        assert_eq!(list_part_at(list, &combo, 10, 25), Some(ListPart::Row(1)));
        assert_eq!(
            list_part_at(list, &combo, 95, 3),
            Some(ListPart::ScrollBar(scroll_bar::Part::LineUp))
        );
        assert_eq!(list_part_at(list, &combo, 10, 70), None);
    }

    #[test]
    fn the_closed_box_and_the_open_list_have_the_designed_pixels() {
        let mut combo = pegs_board_types();
        let mut pixels = vec![0u32; 368 * 148];
        let mut surface = Surface::new(&mut pixels, 368, 148).unwrap();
        let rect = Rect::sized(112, 72, 240, 328);
        let look = ComboLook {
            focused: false,
            enabled: true,
        };
        paint_closed(&mut surface, rect, &combo, look, &LIGHT);
        assert_eq!(surface.get(112, 72), Some(LIGHT.button_border));
        assert_eq!(surface.get(113, 73), Some(LIGHT.button_fill));
        assert_eq!(surface.get(339, 82), Some(LIGHT.text));
        assert_eq!(surface.get(112, 96), Some(0));
        let focused = ComboLook {
            focused: true,
            ..look
        };
        paint_closed(&mut surface, rect, &combo, focused, &LIGHT);
        assert_eq!(surface.get(116, 76), Some(LIGHT.highlight));
        combo.open();
        let list = list_rect(closed_box(rect), combo.rows(), Rect::new(0, 0, 368, 148));
        paint_list(&mut surface, list, &combo, &LIGHT);
        assert_eq!(surface.get(112, 10), Some(LIGHT.field_border));
        assert_eq!(surface.get(113, 11), Some(LIGHT.highlight));
        assert_eq!(surface.get(113, 31), Some(LIGHT.field_fill));
        assert_eq!(surface.get(351, 71), Some(LIGHT.field_border));
    }
}
