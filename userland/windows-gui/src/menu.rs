//! The menu bar and its drop-downs (section 6 of the design): their layout from the labels,
//! hit tests and painting, inside the window's content above and over its client area.

use crate::controls::button::tick;
use crate::controls::{MAX_LABEL_CELLS, Palette, label_cells, outline};
use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_WIDTH, Cell};

/// The bar's height; the client area starts below it.
pub const BAR_HEIGHT: i32 = 24;
const ENTRY_PADDING: i32 = 8;
const TEXT_TOP: i32 = 4;
const ITEM_HEIGHT: i32 = 24;
const SEPARATOR_HEIGHT: i32 = 9;
const SEPARATOR_LINE: i32 = 4;
const SEPARATOR_INSET: i32 = 8;
const DROP_DOWN_PADDING_TOP: i32 = 3;
const DROP_DOWN_PADDING: i32 = 6;
const DROP_DOWN_MINIMUM_WIDTH: i32 = 120;
const DROP_DOWN_EXTRA_WIDTH: i32 = 48;
const ITEM_TEXT_LEFT: i32 = 28;
const CHECK_LEFT: i32 = 7;
const CHECK_TOP: i32 = 5;

/// One bar entry or drop-down item as the layout sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuLabel<'a> {
    pub label: &'a [u16],
    pub separator: bool,
    pub grayed: bool,
    pub checked: bool,
}

impl MenuLabel<'_> {
    fn cells(&self, out: &mut [Cell; MAX_LABEL_CELLS]) -> usize {
        label_cells(self.label, true, out)
    }

    fn width_in_cells(&self) -> usize {
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        self.cells(&mut cells)
    }

    /// Whether choosing it does something.
    pub fn is_enabled(&self) -> bool {
        !self.separator && !self.grayed
    }
}

/// Bar entry `index`'s rectangle, if it fits the content's width.
pub fn entry_rect(entries: &[MenuLabel<'_>], index: usize, content_width: i32) -> Option<Rect> {
    let mut x = 0;
    for (at, entry) in entries.iter().enumerate() {
        let width = entry.width_in_cells() as i32 * CELL_WIDTH + 2 * ENTRY_PADDING;
        let rect = Rect::sized(x, 0, width, BAR_HEIGHT);
        if rect.right > content_width {
            return None;
        }
        if at == index {
            return Some(rect);
        }
        x = rect.right;
    }
    None
}

/// The bar entry at (`x`, `y`) of the content.
pub fn entry_at(entries: &[MenuLabel<'_>], content_width: i32, x: i32, y: i32) -> Option<usize> {
    (0..entries.len()).find(|&index| {
        entry_rect(entries, index, content_width).is_some_and(|rect| rect.contains(x, y))
    })
}

/// Paints the bar across a content `content_width` wide, entry `open` highlighted.
pub fn paint_bar(
    surface: &mut Surface<'_>,
    content_width: i32,
    entries: &[MenuLabel<'_>],
    open: Option<usize>,
    palette: &Palette,
) {
    surface.fill(Rect::new(0, 0, content_width, BAR_HEIGHT), palette.menu_bar);
    for (index, entry) in entries.iter().enumerate() {
        let Some(rect) = entry_rect(entries, index, content_width) else {
            break;
        };
        if open == Some(index) && entry.is_enabled() {
            surface.fill(rect, palette.menu_highlight);
        }
        let colour = match entry.grayed {
            true => palette.menu_text_disabled,
            false => palette.menu_text,
        };
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = entry.cells(&mut cells);
        text::draw_cells(
            surface,
            rect.left + ENTRY_PADDING,
            TEXT_TOP,
            &cells[..count],
            colour,
            None,
        );
    }
}

/// The drop-down of `items` under the entry at `entry_left`, in a content `content_width`
/// wide.
pub fn drop_down_rect(items: &[MenuLabel<'_>], entry_left: i32, content_width: i32) -> Rect {
    let widest = items
        .iter()
        .filter(|item| !item.separator)
        .map(|item| item.width_in_cells())
        .max()
        .unwrap_or(0) as i32;
    let width = DROP_DOWN_MINIMUM_WIDTH.max(widest * CELL_WIDTH + DROP_DOWN_EXTRA_WIDTH);
    let height = items.iter().fold(DROP_DOWN_PADDING, |height, item| {
        height
            + match item.separator {
                true => SEPARATOR_HEIGHT,
                false => ITEM_HEIGHT,
            }
    });
    let left = entry_left.min(content_width - width).max(0);
    Rect::sized(left, BAR_HEIGHT, width, height)
}

fn item_rows<'a>(
    rect: Rect,
    items: &'a [MenuLabel<'a>],
) -> impl Iterator<Item = (usize, Rect, &'a MenuLabel<'a>)> + 'a {
    let mut y = rect.top + DROP_DOWN_PADDING_TOP;
    items.iter().enumerate().map(move |(index, item)| {
        let height = match item.separator {
            true => SEPARATOR_HEIGHT,
            false => ITEM_HEIGHT,
        };
        let row = Rect::sized(rect.left + 1, y, rect.width() - 2, height);
        y += height;
        (index, row, item)
    })
}

/// The enabled item at (`x`, `y`) of the content in the drop-down at `rect`.
pub fn item_at(rect: Rect, items: &[MenuLabel<'_>], x: i32, y: i32) -> Option<usize> {
    item_rows(rect, items)
        .find(|(_, row, _)| row.contains(x, y))
        .filter(|(_, _, item)| item.is_enabled())
        .map(|(index, _, _)| index)
}

/// Paints the drop-down at `rect` with item `highlighted` highlighted.
pub fn paint_drop_down(
    surface: &mut Surface<'_>,
    rect: Rect,
    items: &[MenuLabel<'_>],
    highlighted: Option<usize>,
    palette: &Palette,
) {
    surface.fill(rect, palette.menu_fill);
    outline(surface, rect, palette.menu_border);
    for (index, row, item) in item_rows(rect, items) {
        if item.separator {
            surface.fill(
                Rect::new(
                    rect.left + SEPARATOR_INSET,
                    row.top + SEPARATOR_LINE,
                    rect.right - SEPARATOR_INSET,
                    row.top + SEPARATOR_LINE + 1,
                ),
                palette.menu_separator,
            );
            continue;
        }
        if highlighted == Some(index) && item.is_enabled() {
            surface.fill(row, palette.menu_highlight);
        }
        let colour = match item.grayed {
            true => palette.menu_text_disabled,
            false => palette.menu_text,
        };
        if item.checked {
            tick(surface, rect.left + CHECK_LEFT, row.top + CHECK_TOP, colour);
        }
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = item.cells(&mut cells);
        text::draw_cells(
            surface,
            rect.left + ITEM_TEXT_LEFT,
            row.top + TEXT_TOP,
            &cells[..count],
            colour,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    fn label(text: &[u16]) -> MenuLabel<'_> {
        MenuLabel {
            label: text,
            separator: false,
            grayed: false,
            checked: false,
        }
    }

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_worked_examples_bar_and_file_drop_down_have_the_designed_rectangles() {
        let (file, help) = (units("File"), units("&Help"));
        let entries = [label(&file), label(&help)];
        assert_eq!(
            entry_rect(&entries, 0, 400),
            Some(Rect::sized(0, 0, 48, 24))
        );
        assert_eq!(
            entry_rect(&entries, 1, 400),
            Some(Rect::sized(48, 0, 48, 24))
        );
        assert_eq!(entry_rect(&entries, 1, 90), None);
        assert_eq!(entry_at(&entries, 400, 50, 10), Some(1));
        let (open, save, exit) = (units("&Open..."), units("&Save As..."), units("E&xit"));
        let separator = MenuLabel {
            separator: true,
            ..label(&[])
        };
        let items = [label(&open), label(&save), separator, label(&exit)];
        let rect = drop_down_rect(&items, 0, 400);
        assert_eq!(rect, Rect::sized(0, 24, 128, 87));
        assert_eq!(drop_down_rect(&items, 350, 400).left, 272);
        assert_eq!(item_at(rect, &items, 10, 30), Some(0));
        assert_eq!(item_at(rect, &items, 10, 60), Some(1));
        assert_eq!(item_at(rect, &items, 10, 79), None);
        assert_eq!(item_at(rect, &items, 10, 90), Some(3));
        let mut pixels = vec![0u32; 400 * 324];
        let mut surface = Surface::new(&mut pixels, 400, 324).unwrap();
        paint_bar(&mut surface, 400, &entries, Some(0), &LIGHT);
        paint_drop_down(&mut surface, rect, &items, Some(0), &LIGHT);
        assert_eq!(surface.get(47, 23), Some(LIGHT.menu_highlight));
        assert_eq!(surface.get(60, 18), Some(LIGHT.menu_text));
        assert_eq!(surface.get(0, 24), Some(LIGHT.menu_border));
        assert_eq!(surface.get(127, 110), Some(LIGHT.menu_border));
        assert_eq!(surface.get(1, 27), Some(LIGHT.menu_highlight));
        assert_eq!(surface.get(28, 45), Some(LIGHT.menu_text));
        assert_eq!(surface.get(8, 79), Some(LIGHT.menu_separator));
        assert_eq!(surface.get(120, 79), Some(LIGHT.menu_fill));
        assert_eq!(surface.get(36, 102), Some(LIGHT.menu_text));
    }
}
