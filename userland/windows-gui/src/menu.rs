//! The menu bar and its drop-downs (section 6 of the design): their layout from the labels,
//! hit tests and painting, inside the window's content above and over its client area. A bar
//! title selected by the keyboard without a drop-down carries the dotted focus outline, and the
//! mnemonic underlines show only while the window's keyboard cues are on
//! (`docs/ux/windows-menu-bar-keys-v0.1.md`, D733).

use crate::controls::button::{dot, tick};
use crate::controls::{MAX_LABEL_CELLS, Palette, dotted_outline, label_cells, outline};
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
const ARROW_RIGHT: i32 = 10;
const ARROW_TOP: i32 = 8;
const ARROW_HEIGHT: i32 = 7;
const CASCADE_OVERLAP: i32 = 2;
const SELECTED_OUTLINE_INSET: i32 = 2;

/// Which bar title is lit: none, the one whose drop-down is open, or the one the keyboard
/// selected with no drop-down open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarLight {
    None,
    Open(usize),
    Selected(usize),
}

fn hide_underlines(cells: &mut [Cell]) {
    for cell in cells {
        if let Cell::Glyph { underline, .. } = cell {
            *underline = false;
        }
    }
}

/// One bar entry or drop-down item as the layout sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuLabel<'a> {
    pub label: &'a [u16],
    pub separator: bool,
    pub grayed: bool,
    pub checked: bool,
    /// `MFT_RADIOCHECK`: the check is a dot (D334).
    pub radio: bool,
    /// The item opens a submenu, drawn with an arrow at its right (D337).
    pub submenu: bool,
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

/// Paints the bar across a content `content_width` wide with `light` lit, the mnemonics
/// underlined only when `cues` is on.
pub fn paint_bar(
    surface: &mut Surface<'_>,
    content_width: i32,
    entries: &[MenuLabel<'_>],
    light: BarLight,
    cues: bool,
    palette: &Palette,
) {
    surface.fill(Rect::new(0, 0, content_width, BAR_HEIGHT), palette.menu_bar);
    for (index, entry) in entries.iter().enumerate() {
        let Some(rect) = entry_rect(entries, index, content_width) else {
            break;
        };
        let lit = matches!(light, BarLight::Open(at) | BarLight::Selected(at) if at == index);
        if lit && entry.is_enabled() {
            surface.fill(rect, palette.menu_highlight);
        }
        if light == BarLight::Selected(index) && entry.is_enabled() {
            let inset = SELECTED_OUTLINE_INSET;
            let ring = Rect::new(rect.left + inset, inset, rect.right - inset, BAR_HEIGHT - inset);
            dotted_outline(surface, ring, palette.focus);
        }
        let colour = match entry.grayed {
            true => palette.menu_text_disabled,
            false => palette.menu_text,
        };
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = entry.cells(&mut cells);
        if !cues {
            hide_underlines(&mut cells[..count]);
        }
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

/// The top of item `index`'s row in the drop-down at `rect`.
pub fn item_top(rect: Rect, items: &[MenuLabel<'_>], index: usize) -> Option<i32> {
    item_rows(rect, items)
        .find(|(at, _, _)| *at == index)
        .map(|(_, row, _)| row.top)
}

/// The submenu of `items` that the item whose row starts at `row_top` of the drop-down at
/// `parent` opens (D337): at the parent's right edge, its first item level with that row,
/// flipped to the parent's left when it would leave a content `content_width` wide.
pub fn cascade_rect(
    items: &[MenuLabel<'_>],
    parent: Rect,
    row_top: i32,
    content_width: i32,
) -> Rect {
    let sized = drop_down_rect(items, 0, content_width);
    let width = sized.right - sized.left;
    let height = sized.bottom - sized.top;
    let right_side = parent.right - CASCADE_OVERLAP;
    let left = match right_side + width > content_width {
        true => (parent.left + CASCADE_OVERLAP - width).max(0),
        false => right_side,
    };
    Rect::sized(left, row_top - DROP_DOWN_PADDING_TOP, width, height)
}

/// The enabled item at (`x`, `y`) of the content in the drop-down at `rect`.
pub fn item_at(rect: Rect, items: &[MenuLabel<'_>], x: i32, y: i32) -> Option<usize> {
    item_rows(rect, items)
        .find(|(_, row, _)| row.contains(x, y))
        .filter(|(_, _, item)| item.is_enabled())
        .map(|(index, _, _)| index)
}

/// Paints the drop-down at `rect` with item `highlighted` highlighted, the mnemonics
/// underlined only when `cues` is on.
pub fn paint_drop_down(
    surface: &mut Surface<'_>,
    rect: Rect,
    items: &[MenuLabel<'_>],
    highlighted: Option<usize>,
    cues: bool,
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
        match (item.checked, item.radio) {
            (true, true) => dot(surface, rect.left + CHECK_LEFT, row.top + CHECK_TOP, colour),
            (true, false) => tick(surface, rect.left + CHECK_LEFT, row.top + CHECK_TOP, colour),
            _ => {}
        }
        if item.submenu {
            let tip = row.right - ARROW_RIGHT;
            for step in 0..=ARROW_HEIGHT / 2 {
                let length = ARROW_HEIGHT - 2 * step;
                surface.fill(
                    Rect::sized(
                        tip - ARROW_HEIGHT / 2 + step,
                        row.top + ARROW_TOP + step,
                        1,
                        length,
                    ),
                    colour,
                );
            }
        }
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = item.cells(&mut cells);
        if !cues {
            hide_underlines(&mut cells[..count]);
        }
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
            radio: false,
            submenu: false,
        }
    }

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_selected_title_has_the_dotted_outline_and_underlines_follow_the_cues() {
        let (file, edit, help) = (units("&File"), units("&Edit"), units("&Help"));
        let entries = [label(&file), label(&edit), label(&help)];
        let mut pixels = vec![0u32; 400 * 24];
        let mut surface = Surface::new(&mut pixels, 400, 24).unwrap();
        paint_bar(&mut surface, 400, &entries, BarLight::Selected(0), true, &LIGHT);
        assert_eq!(surface.get(2, 2), Some(LIGHT.focus));
        assert_eq!(surface.get(45, 21), Some(LIGHT.focus));
        assert_eq!(surface.get(3, 2), Some(LIGHT.menu_highlight));
        assert_eq!(surface.get(8, 18), Some(LIGHT.menu_text));
        assert_eq!(surface.get(56, 18), Some(LIGHT.menu_text));
        paint_bar(&mut surface, 400, &entries, BarLight::None, false, &LIGHT);
        assert_eq!(surface.get(8, 18), Some(LIGHT.menu_bar));
        assert_eq!(surface.get(2, 2), Some(LIGHT.menu_bar));
        paint_bar(&mut surface, 400, &entries, BarLight::Open(1), false, &LIGHT);
        assert_eq!(surface.get(50, 2), Some(LIGHT.menu_highlight));
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
        paint_bar(&mut surface, 400, &entries, BarLight::Open(0), true, &LIGHT);
        paint_drop_down(&mut surface, rect, &items, Some(0), true, &LIGHT);
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
