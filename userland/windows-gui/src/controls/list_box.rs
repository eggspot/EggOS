//! `LISTBOX`, single selection (section 5 of the design): the [`ListBox`] model (items in one
//! arena, sorted with `LBS_SORT`, the selection and the first row shown) and its painting.

use super::{MAX_LABEL_CELLS, Palette, dotted_outline, draw_fitted, framed, label_cells, ring};
use crate::surface::{Rect, Surface};
use crate::text::Cell;
use core::cmp::Ordering;

/// The most items a list holds.
pub const LIST_CAPACITY: usize = 256;
/// The UTF-16 units all of a list's items hold together.
pub const LIST_UNITS: usize = 8192;
/// A row's height.
pub const ITEM_HEIGHT: i32 = 20;
const TEXT_LEFT: i32 = 4;
const MARKED_TEXT_LEFT: i32 = 24;
const TEXT_TOP: i32 = 2;
const MARK_LEFT: i32 = 4;
const FOLDER_BACK: u32 = 0x00b8_8a24;
const FOLDER_FRONT: u32 = 0x00e2_b341;

/// `LISTBOX`'s styles this layer reads.
pub mod style {
    pub const NOTIFY: u32 = 0x1;
    pub const SORT: u32 = 0x2;
}

/// The keys a list reads, as Windows' virtual keys.
pub mod key {
    pub const PAGE_UP: u32 = 0x21;
    pub const PAGE_DOWN: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const UP: u32 = 0x26;
    pub const DOWN: u32 = 0x28;
}

#[derive(Debug, Clone, Copy, Default)]
struct Item {
    start: u16,
    length: u16,
    marked: bool,
}

/// A list's items, selection and scroll.
#[derive(Debug, Clone)]
pub struct ListBox {
    units: [u16; LIST_UNITS],
    used: usize,
    items: [Item; LIST_CAPACITY],
    count: usize,
    selected: Option<usize>,
    top: usize,
    pub marks: bool,
}

/// Compares item names as the list sorts them: ASCII letters folded, then by code unit.
pub fn compare_names(a: &[u16], b: &[u16]) -> Ordering {
    let fold = |unit: u16| match unit {
        0x41..=0x5a => unit + 0x20,
        unit => unit,
    };
    a.iter()
        .map(|unit| fold(*unit))
        .cmp(b.iter().map(|unit| fold(*unit)))
        .then_with(|| a.cmp(b))
}

impl Default for ListBox {
    fn default() -> Self {
        Self::new()
    }
}

impl ListBox {
    pub const fn new() -> ListBox {
        ListBox {
            units: [0; LIST_UNITS],
            used: 0,
            items: [Item {
                start: 0,
                length: 0,
                marked: false,
            }; LIST_CAPACITY],
            count: 0,
            selected: None,
            top: 0,
            marks: false,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn top(&self) -> usize {
        self.top
    }

    /// Item `index`'s text.
    pub fn text(&self, index: usize) -> Option<&[u16]> {
        let item = self.items[..self.count].get(index)?;
        let start = usize::from(item.start);
        Some(&self.units[start..start + usize::from(item.length)])
    }

    /// Whether item `index` carries the folder mark.
    pub fn is_marked(&self, index: usize) -> bool {
        self.items[..self.count]
            .get(index)
            .is_some_and(|item| item.marked)
    }

    /// `LB_ADDSTRING`: at the end, or in its sorted place with `sorted`; its index, `None`
    /// when the list is full.
    pub fn add(&mut self, units: &[u16], sorted: bool) -> Option<usize> {
        let index = match sorted {
            true => (0..self.count)
                .find(|&index| {
                    self.text(index)
                        .is_some_and(|text| compare_names(units, text) == Ordering::Less)
                })
                .unwrap_or(self.count),
            false => self.count,
        };
        self.insert(index, units)
    }

    /// `LB_INSERTSTRING` at `index` (the end when past it); its index.
    pub fn insert(&mut self, index: usize, units: &[u16]) -> Option<usize> {
        let length = units.len().min(u16::MAX as usize);
        if self.count == LIST_CAPACITY || self.used + length > LIST_UNITS {
            return None;
        }
        let index = index.min(self.count);
        self.units[self.used..self.used + length].copy_from_slice(&units[..length]);
        self.items.copy_within(index..self.count, index + 1);
        self.items[index] = Item {
            start: self.used as u16,
            length: length as u16,
            marked: false,
        };
        self.used += length;
        self.count += 1;
        if let Some(selected) = self.selected
            && selected >= index
        {
            self.selected = Some(selected + 1);
        }
        Some(index)
    }

    /// Gives item `index` the folder mark.
    pub fn mark(&mut self, index: usize) {
        if let Some(item) = self.items[..self.count].get_mut(index) {
            item.marked = true;
        }
    }

    /// `LB_DELETESTRING`; the count left, `None` when there is no such item.
    pub fn delete(&mut self, index: usize) -> Option<usize> {
        if index >= self.count {
            return None;
        }
        let removed = self.items[index];
        let (start, length) = (usize::from(removed.start), usize::from(removed.length));
        self.units.copy_within(start + length..self.used, start);
        self.used -= length;
        self.items.copy_within(index + 1..self.count, index);
        self.count -= 1;
        for item in &mut self.items[..self.count] {
            if usize::from(item.start) > start {
                item.start -= length as u16;
            }
        }
        self.selected = match self.selected {
            Some(selected) if selected == index => None,
            Some(selected) if selected > index => Some(selected - 1),
            other => other,
        };
        self.top = self.top.min(self.count.saturating_sub(1));
        Some(self.count)
    }

    /// `LB_RESETCONTENT`.
    pub fn clear(&mut self) {
        self.used = 0;
        self.count = 0;
        self.selected = None;
        self.top = 0;
    }

    /// `LB_SETCURSEL`: `None` clears; whether the index was valid.
    pub fn select(&mut self, index: Option<usize>, visible: usize) -> bool {
        match index {
            Some(index) if index >= self.count => {
                self.selected = None;
                false
            }
            index => {
                self.selected = index;
                self.scroll(visible);
                true
            }
        }
    }

    /// A key press; whether the selection changed.
    pub fn key(&mut self, virtual_key: u32, visible: usize) -> bool {
        if self.count == 0 {
            return false;
        }
        let last = self.count - 1;
        let page = visible.max(2) - 1;
        let target = match (virtual_key, self.selected) {
            (key::UP, Some(selected)) => selected.saturating_sub(1),
            (key::UP, None) => 0,
            (key::DOWN, Some(selected)) => (selected + 1).min(last),
            (key::DOWN, None) => 0,
            (key::HOME, _) => 0,
            (key::END, _) => last,
            (key::PAGE_UP, selected) => selected.unwrap_or(0).saturating_sub(page),
            (key::PAGE_DOWN, selected) => (selected.unwrap_or(0) + page).min(last),
            _ => return false,
        };
        let changed = self.selected != Some(target);
        self.selected = Some(target);
        self.scroll(visible);
        changed
    }

    /// A click on row `row` of those shown; whether the selection changed.
    pub fn click_row(&mut self, row: usize, visible: usize) -> bool {
        let index = self.top + row;
        if index >= self.count {
            return false;
        }
        let changed = self.selected != Some(index);
        self.selected = Some(index);
        self.scroll(visible);
        changed
    }

    fn scroll(&mut self, visible: usize) {
        let visible = visible.max(1);
        if let Some(selected) = self.selected {
            if selected < self.top {
                self.top = selected;
            }
            if selected >= self.top + visible {
                self.top = selected + 1 - visible;
            }
        }
    }
}

/// The whole rows a list `height` tall shows.
pub fn visible_rows(height: i32) -> usize {
    (((height - 2) / ITEM_HEIGHT).max(1)) as usize
}

/// The row of those shown at `y` pixels from the list's top, partial rows included.
pub fn row_at(y: i32, height: i32) -> Option<usize> {
    let inner = y - 1;
    (inner >= 0 && y < height - 1).then_some((inner / ITEM_HEIGHT) as usize)
}

/// How a list looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLook {
    pub focused: bool,
    pub enabled: bool,
}

/// Paints `list` over the whole of `rect`.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    list: &ListBox,
    look: ListLook,
    palette: &Palette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    framed(
        surface,
        rect,
        palette.rounded,
        palette.field_border,
        palette.field_fill,
    );
    let inner = rect.inset(1);
    surface.clip_to(inner);
    let caret = list.selected.unwrap_or(0);
    let text_left = match list.marks {
        true => MARKED_TEXT_LEFT,
        false => TEXT_LEFT,
    };
    let mut index = list.top;
    let mut y = inner.top;
    while index < list.count && y < inner.bottom {
        let row = Rect::new(inner.left, y, inner.right, y + ITEM_HEIGHT);
        let selected = list.selected == Some(index);
        let colour = match () {
            () if !look.enabled => palette.text_disabled,
            () if selected => palette.highlight_text,
            () => palette.text,
        };
        if selected {
            surface.fill(row, palette.highlight);
        }
        if list.is_marked(index) {
            let x = rect.left + MARK_LEFT;
            surface.fill(Rect::sized(x, y + 5, 12, 8), FOLDER_BACK);
            surface.fill(Rect::sized(x, y + 7, 12, 6), FOLDER_FRONT);
        }
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = label_cells(list.text(index).unwrap_or(&[]), false, &mut cells);
        let x = rect.left + text_left;
        draw_fitted(
            surface,
            x,
            y + TEXT_TOP,
            &cells[..count],
            row.right - x,
            colour,
        );
        if look.focused && index == caret {
            match palette.rounded {
                true => ring(surface, row, palette.focus),
                false => {
                    let dots = match selected {
                        true => palette.highlight_text,
                        false => palette.focus,
                    };
                    dotted_outline(surface, row, dots);
                }
            }
        }
        index += 1;
        y += ITEM_HEIGHT;
    }
    if look.focused && list.count == 0 && !palette.rounded {
        dotted_outline(
            surface,
            Rect::new(inner.left, inner.top, inner.right, inner.top + ITEM_HEIGHT),
            palette.focus,
        );
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

    fn texts(list: &ListBox) -> Vec<String> {
        (0..list.count())
            .map(|index| String::from_utf16(list.text(index).unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn sorted_lists_order_names_ignoring_ascii_case() {
        let mut list = ListBox::new();
        for name in ["egg", "Alpha", "beta", "alpha"] {
            list.add(&units(name), true);
        }
        assert_eq!(texts(&list), ["Alpha", "alpha", "beta", "egg"]);
        list.delete(1);
        assert_eq!(texts(&list), ["Alpha", "beta", "egg"]);
        list.insert(0, &units("z"));
        assert_eq!(texts(&list), ["z", "Alpha", "beta", "egg"]);
        list.clear();
        assert_eq!(list.count(), 0);
    }

    #[test]
    fn keys_move_the_selection_and_the_list_scrolls_to_it() {
        let mut list = ListBox::new();
        for index in 0..30 {
            list.add(&units(&format!("item {index}")), false);
        }
        assert!(list.key(key::DOWN, 7));
        assert_eq!(list.selected(), Some(0));
        for _ in 0..7 {
            list.key(key::DOWN, 7);
        }
        assert_eq!((list.selected(), list.top()), (Some(7), 1));
        list.key(key::PAGE_DOWN, 7);
        assert_eq!((list.selected(), list.top()), (Some(13), 7));
        list.key(key::END, 7);
        assert_eq!((list.selected(), list.top()), (Some(29), 23));
        list.key(key::HOME, 7);
        assert_eq!((list.selected(), list.top()), (Some(0), 0));
        assert!(list.click_row(7, 7));
        assert_eq!((list.selected(), list.top()), (Some(7), 1));
        assert!(!list.key(key::HOME, 7) || list.selected() == Some(0));
    }

    #[test]
    fn rows_are_found_by_height_partial_ones_included() {
        assert_eq!(visible_rows(160), 7);
        assert_eq!(row_at(0, 160), None);
        assert_eq!(row_at(1, 160), Some(0));
        assert_eq!(row_at(21, 160), Some(1));
        assert_eq!(row_at(158, 160), Some(7));
        assert_eq!(row_at(159, 160), None);
    }

    #[test]
    fn the_selected_row_is_highlighted_with_white_text() {
        let mut list = ListBox::new();
        list.add(&units("alpha"), true);
        list.add(&units("egg"), true);
        list.select(Some(1), 7);
        let mut pixels = vec![0u32; 200 * 160];
        let mut surface = Surface::new(&mut pixels, 200, 160).unwrap();
        let look = ListLook {
            focused: false,
            enabled: true,
        };
        paint(&mut surface, Rect::new(0, 0, 200, 160), &list, look, &LIGHT);
        assert_eq!(surface.get(0, 0), Some(0x007a_7a7a));
        assert_eq!(surface.get(1, 1), Some(0x00ff_ffff));
        assert_eq!(surface.get(1, 21), Some(0x0000_78d7));
        assert_eq!(surface.get(198, 40), Some(0x0000_78d7));
        assert!((4..28).any(|x| (23..39).any(|y| surface.get(x, y) == Some(0x00ff_ffff))));
    }
}
