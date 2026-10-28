//! A context menu drawn inside a window (docs/ux/desktop-v0.3-gui.md section 6.4 item 4 and
//! section 8.1 item 7): items 32 tall with their text at x + 12, separators 9 tall, the hovered
//! or chosen item `menu.hover`, disabled items `text.dim` and skipped by the arrows, clamped
//! inside its bounds.

use crate::canvas::{Canvas, INSET_R6};
use crate::colour;
use crate::geometry::Rect;
use crate::input::key;
use crate::text;
use crate::widget::LINE_HEIGHT;

/// The most entries a menu holds, separators included.
pub const MAX_ENTRIES: usize = 12;
/// An item's height.
pub const ITEM_HEIGHT: i32 = 32;
/// A separator's height.
pub const SEPARATOR_HEIGHT: i32 = 9;
/// The narrowest menu.
pub const MIN_WIDTH: i32 = 160;
/// Where an item's text starts.
pub const TEXT_X: i32 = 12;
const LABEL_ROOM: i32 = 24;
const WIDTH_STEP: i32 = 8;
const CLEAR_OF_EDGE: i32 = 8;
const SEPARATOR_LINE: i32 = 4;

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// An item the program knows by `id`.
    Item {
        label: &'static str,
        enabled: bool,
        id: u8,
    },
    /// A thin line between groups.
    Separator,
}

/// What a key did to a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// It moved the choice, or nothing.
    Nothing,
    /// Enter chose the item of this id.
    Chosen(u8),
    /// Esc closed the menu.
    Closed,
}

/// An open menu.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    rect: Rect,
    chosen: Option<usize>,
}

impl Menu {
    /// A menu of `entries` (at most [`MAX_ENTRIES`]) with its top-left at (`x`, `y`), moved so
    /// it ends at least 8 px inside `bounds`' right and bottom edges.
    pub fn new(entries: &[Entry], x: i32, y: i32, bounds: Rect) -> Self {
        let mut held = [Entry::Separator; MAX_ENTRIES];
        let count = entries.len().min(MAX_ENTRIES);
        held[..count].copy_from_slice(&entries[..count]);
        let widest = held[..count]
            .iter()
            .map(|entry| match entry {
                Entry::Item { label, .. } => text::width(label) + LABEL_ROOM,
                Entry::Separator => 0,
            })
            .max()
            .unwrap_or(0);
        let width = (widest.div_ceil(WIDTH_STEP) * WIDTH_STEP).max(MIN_WIDTH);
        let height = 2 + held[..count]
            .iter()
            .map(|entry| match entry {
                Entry::Item { .. } => ITEM_HEIGHT,
                Entry::Separator => SEPARATOR_HEIGHT,
            })
            .sum::<i32>();
        let x = x.min(bounds.right() - CLEAR_OF_EDGE - width).max(bounds.x);
        let y = y.min(bounds.bottom() - CLEAR_OF_EDGE - height).max(bounds.y);
        Self {
            entries: held,
            count,
            rect: Rect::new(x, y, width, height),
            chosen: None,
        }
    }

    /// Where it is.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Its entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries[..self.count]
    }

    /// The rectangle of entry `index`.
    pub fn entry_rect(&self, index: usize) -> Rect {
        let mut y = self.rect.y + 1;
        for entry in &self.entries[..index.min(self.count)] {
            y += match entry {
                Entry::Item { .. } => ITEM_HEIGHT,
                Entry::Separator => SEPARATOR_HEIGHT,
            };
        }
        let height = match self.entries.get(index) {
            Some(Entry::Item { .. }) => ITEM_HEIGHT,
            _ => SEPARATOR_HEIGHT,
        };
        Rect::new(self.rect.x + 1, y, self.rect.width - 2, height)
    }

    fn enabled(&self, index: usize) -> Option<u8> {
        match self.entries[..self.count].get(index) {
            Some(Entry::Item {
                enabled: true, id, ..
            }) => Some(*id),
            _ => None,
        }
    }

    /// The enabled item at (`x`, `y`), by index.
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.count)
            .find(|&index| self.entry_rect(index).contains(x, y))
            .filter(|&index| self.enabled(index).is_some())
    }

    /// The id of the enabled item at (`x`, `y`).
    pub fn id_at(&self, x: i32, y: i32) -> Option<u8> {
        self.item_at(x, y).and_then(|index| self.enabled(index))
    }

    /// The pointer is at (`x`, `y`): the item under it is shown chosen; whether that changed.
    pub fn hover(&mut self, x: i32, y: i32) -> bool {
        let chosen = self.item_at(x, y);
        let changed = chosen != self.chosen;
        self.chosen = chosen;
        changed
    }

    /// The pointer left: nothing is shown chosen.
    pub fn leave(&mut self) -> bool {
        self.chosen.take().is_some()
    }

    /// Key `code` was pressed.
    pub fn key(&mut self, code: u16) -> MenuKey {
        match code {
            key::ESC => MenuKey::Closed,
            key::ENTER => match self.chosen.and_then(|index| self.enabled(index)) {
                Some(id) => MenuKey::Chosen(id),
                None => MenuKey::Nothing,
            },
            key::UP | key::DOWN => {
                let mut enabled = (0..self.count).filter(|&index| self.enabled(index).is_some());
                let next = match (code, self.chosen) {
                    (key::DOWN, None) => enabled.next(),
                    (key::UP, None) => enabled.last(),
                    (key::DOWN, Some(at)) => enabled.find(|&index| index > at).or(Some(at)),
                    (_, Some(at)) => enabled.filter(|&index| index < at).last().or(Some(at)),
                };
                self.chosen = next;
                MenuKey::Nothing
            }
            _ => MenuKey::Nothing,
        }
    }

    /// Draws it.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        canvas.rounded(self.rect, &INSET_R6, colour::MENU);
        canvas.outline(self.rect, &INSET_R6, 1, colour::MENU_BORDER);
        for (index, entry) in self.entries[..self.count].iter().enumerate() {
            let rect = self.entry_rect(index);
            match entry {
                Entry::Separator => canvas.fill(
                    Rect::new(
                        self.rect.x + CLEAR_OF_EDGE,
                        rect.y + SEPARATOR_LINE,
                        self.rect.width - 2 * CLEAR_OF_EDGE,
                        1,
                    ),
                    colour::MENU_BORDER,
                ),
                Entry::Item { label, enabled, .. } => {
                    if self.chosen == Some(index) {
                        canvas.fill(rect, colour::MENU_HOVER);
                    }
                    let ink = match enabled {
                        true => colour::TEXT,
                        false => colour::TEXT_DIM,
                    };
                    canvas.text(
                        self.rect.x + TEXT_X,
                        rect.y + (ITEM_HEIGHT - LINE_HEIGHT) / 2,
                        label,
                        ink,
                        1,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRIES: [Entry; 4] = [
        Entry::Item {
            label: "Open",
            enabled: true,
            id: 1,
        },
        Entry::Separator,
        Entry::Item {
            label: "Paste",
            enabled: false,
            id: 2,
        },
        Entry::Item {
            label: "Delete permanently",
            enabled: true,
            id: 3,
        },
    ];

    #[test]
    fn a_menu_is_as_wide_as_its_widest_label_and_stays_inside_its_bounds() {
        let menu = Menu::new(&ENTRIES, 700, 400, Rect::new(0, 0, 720, 480));
        let rect = menu.rect();
        assert_eq!(rect.height, 2 + 3 * ITEM_HEIGHT + SEPARATOR_HEIGHT);
        assert_eq!(rect.width % WIDTH_STEP, 0);
        assert!(rect.width >= MIN_WIDTH);
        assert_eq!(rect.right(), 720 - 8);
        assert_eq!(rect.bottom(), 480 - 8);
        assert_eq!(menu.entry_rect(2).y, rect.y + 1 + ITEM_HEIGHT + SEPARATOR_HEIGHT);
    }

    #[test]
    fn the_arrows_skip_disabled_items_and_enter_chooses() {
        let mut menu = Menu::new(&ENTRIES, 0, 0, Rect::new(0, 0, 720, 480));
        assert_eq!(menu.key(key::ENTER), MenuKey::Nothing);
        menu.key(key::DOWN);
        assert_eq!(menu.key(key::ENTER), MenuKey::Chosen(1));
        menu.key(key::DOWN);
        assert_eq!(menu.key(key::ENTER), MenuKey::Chosen(3));
        menu.key(key::DOWN);
        assert_eq!(menu.key(key::ENTER), MenuKey::Chosen(3));
        menu.key(key::UP);
        assert_eq!(menu.key(key::ENTER), MenuKey::Chosen(1));
        assert_eq!(menu.key(key::ESC), MenuKey::Closed);
    }

    #[test]
    fn the_pointer_chooses_enabled_items_only() {
        let mut menu = Menu::new(&ENTRIES, 10, 10, Rect::new(0, 0, 720, 480));
        let paste = menu.entry_rect(2);
        assert_eq!(menu.id_at(paste.x + 4, paste.y + 4), None);
        let delete = menu.entry_rect(3);
        assert_eq!(menu.id_at(delete.x + 4, delete.y + 4), Some(3));
        assert!(menu.hover(delete.x + 4, delete.y + 4));
        assert!(!menu.hover(delete.x + 5, delete.y + 5));
        assert!(menu.leave());
    }
}
