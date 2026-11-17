//! Menus drawn inside a window: the context menu (docs/ux/desktop-v0.3-gui.md section 6.4 item
//! 4 and section 8.1 item 7), and the menu bar and the menu button built on the same entries
//! (docs/ux/toolkit-v0.5.md section 3). Items are 32 tall with their text at x + 12 and their
//! key right-aligned, separators 9 tall, the hovered or chosen item `menu.hover`, disabled items
//! `text.dim` and skipped by the arrows, the menu clamped inside its bounds.

use crate::canvas::{Canvas, INSET_R6};
use crate::geometry::Rect;
use crate::input::key;
use crate::text::{self, Text};
use crate::widget::{LINE_HEIGHT, Look, MAX_MENUS, MENU_KEY_GAP, MENU_TITLE_PADDING};

/// The most entries a menu holds, separators included: what `SET_MENU` takes.
pub const MAX_ENTRIES: usize = 16;
/// An item's height.
pub const ITEM_HEIGHT: i32 = 32;
/// A separator's height.
pub const SEPARATOR_HEIGHT: i32 = 9;
/// The narrowest menu.
pub const MIN_WIDTH: i32 = 160;
/// Where an item's text starts.
pub const TEXT_X: i32 = 12;
/// The most bytes of a main menu's pool for `SET_MENU`.
pub const POOL_CAPACITY: usize = 248;
const WIDTH_STEP: i32 = 8;
const CLEAR_OF_EDGE: i32 = 8;
const SEPARATOR_LINE: i32 = 4;
const UNDERLINE_ROW: i32 = 14;
const TITLE_TEXT_TOP: i32 = 6;
const MENU_BUTTON_GAP: i32 = 4;
/// The column a menu with a check or radio item keeps for the marks (docs/ux/toolkit-v0.6.md
/// section 2.2).
pub const MARK_COLUMN: i32 = 20;
const MARK_X: i32 = 12;
const TICK_TOP: i32 = 12;
const TICK_ROWS: [i32; 8] = [4, 5, 6, 5, 4, 3, 2, 1];
const DOT_OFFSET: i32 = 13;
const DOT_INSETS: [i32; 6] = [2, 1, 0, 0, 1, 2];

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// An item the program knows by `id`; `label` marks its mnemonic with `&` and `key` is the
    /// shortcut shown after it (`""` for none).
    Item {
        label: &'static str,
        key: &'static str,
        enabled: bool,
        id: u8,
    },
    /// A thin line between groups.
    Separator,
    /// An item with a tick, `checked` its starting state: the program's, which the menu shows
    /// and never flips.
    Check {
        label: &'static str,
        key: &'static str,
        enabled: bool,
        id: u8,
        checked: bool,
    },
    /// An item of a radio group (a run of radio items), with a dot when `selected`.
    Radio {
        label: &'static str,
        key: &'static str,
        enabled: bool,
        id: u8,
        selected: bool,
    },
}

/// The mark of a check or radio item, and whether it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Check(bool),
    Radio(bool),
}

/// What every chooseable entry has, whatever its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemParts {
    pub label: &'static str,
    pub key: &'static str,
    pub enabled: bool,
    pub id: u8,
    pub mark: Option<Mark>,
}

impl Entry {
    /// Its parts when it is an item of any kind; None for a separator.
    pub fn item(&self) -> Option<ItemParts> {
        let (label, key, enabled, id, mark) = match *self {
            Entry::Item {
                label,
                key,
                enabled,
                id,
            } => (label, key, enabled, id, None),
            Entry::Check {
                label,
                key,
                enabled,
                id,
                checked,
            } => (label, key, enabled, id, Some(Mark::Check(checked))),
            Entry::Radio {
                label,
                key,
                enabled,
                id,
                selected,
            } => (label, key, enabled, id, Some(Mark::Radio(selected))),
            Entry::Separator => return None,
        };
        Some(ItemParts {
            label,
            key,
            enabled,
            id,
            mark,
        })
    }

    /// The same entry with its mark shown when `on`; other kinds unchanged.
    pub fn with_mark(self, on: bool) -> Self {
        match self {
            Entry::Check {
                label,
                key,
                enabled,
                id,
                ..
            } => Entry::Check {
                label,
                key,
                enabled,
                id,
                checked: on,
            },
            Entry::Radio {
                label,
                key,
                enabled,
                id,
                ..
            } => Entry::Radio {
                label,
                key,
                enabled,
                id,
                selected: on,
            },
            other => other,
        }
    }

    fn height(&self) -> i32 {
        match self {
            Entry::Separator => SEPARATOR_HEIGHT,
            _ => ITEM_HEIGHT,
        }
    }
}

/// The ids of the radio group (the run of radio items) that holds the radio item `id` in
/// `entries`, if one does.
pub fn radio_group(entries: &[Entry], id: u8) -> Option<&[Entry]> {
    let at = entries.iter().position(|entry| {
        matches!(entry, Entry::Radio { id: found, .. } if *found == id)
    })?;
    let is_radio = |entry: &Entry| matches!(entry, Entry::Radio { .. });
    let start = entries[..at]
        .iter()
        .rposition(|entry| !is_radio(entry))
        .map_or(0, |index| index + 1);
    let end = entries[at..]
        .iter()
        .position(|entry| !is_radio(entry))
        .map_or(entries.len(), |index| at + index);
    Some(&entries[start..end])
}

/// `label` as it is drawn: each `&` that marks a mnemonic dropped, `&&` kept as one `&`.
pub fn plain_label(label: &str) -> Text<64> {
    let mut plain = Text::new();
    let mut marked = false;
    for character in label.chars() {
        if character == '&' && !marked {
            marked = true;
            continue;
        }
        marked = false;
        let mut bytes = [0; 4];
        plain.push_str(character.encode_utf8(&mut bytes));
    }
    plain
}

/// The mnemonic of `label`, the character after a single `&`, lowercased, and where it is
/// drawn: its x from the label's start and its advance.
pub fn mnemonic(label: &str) -> Option<(char, i32, i32)> {
    let mut marked = false;
    let mut x = 0;
    for character in label.chars() {
        if character == '&' && !marked {
            marked = true;
            continue;
        }
        if marked && character != '&' {
            return Some((character.to_ascii_lowercase(), x, text::advance(character)));
        }
        marked = false;
        x += text::advance(character);
    }
    None
}

fn underline(canvas: &mut Canvas<'_>, label: &str, x: i32, y: i32, colour: u32) {
    if let Some((_, at, advance)) = mnemonic(label) {
        canvas.fill(
            Rect::new(x + at, y + UNDERLINE_ROW, (advance - 1).max(1), 1),
            colour,
        );
    }
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

/// The height of a menu of `entries`.
pub fn height_of(entries: &[Entry]) -> i32 {
    2 + entries[..entries.len().min(MAX_ENTRIES)]
        .iter()
        .map(Entry::height)
        .sum::<i32>()
}

/// An open menu.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    rect: Rect,
    chosen: Option<usize>,
    mnemonics: bool,
}

impl Menu {
    /// A menu of `entries` (at most [`MAX_ENTRIES`]) with its top-left at (`x`, `y`), moved so
    /// it ends at least 8 px inside `bounds`' right and bottom edges: as wide as its widest
    /// label and widest key need, at least [`MIN_WIDTH`], in steps of 8.
    pub fn new(entries: &[Entry], x: i32, y: i32, bounds: Rect) -> Self {
        let mut held = [Entry::Separator; MAX_ENTRIES];
        let count = entries.len().min(MAX_ENTRIES);
        held[..count].copy_from_slice(&entries[..count]);
        let (widest_label, widest_key) =
            held[..count]
                .iter()
                .fold((0, 0), |(labels, keys), entry| match entry.item() {
                    Some(item) => (
                        labels.max(text::width(plain_label(item.label).as_str())),
                        keys.max(text::width(item.key)),
                    ),
                    None => (labels, keys),
                });
        let marks = match held[..count].iter().any(|entry| {
            entry.item().is_some_and(|item| item.mark.is_some())
        }) {
            true => MARK_COLUMN,
            false => 0,
        };
        let keys = match widest_key {
            0 => 0,
            widest => MENU_KEY_GAP + widest,
        };
        let widest = 2 * TEXT_X + marks + widest_label + keys;
        let width = ((widest + WIDTH_STEP - 1) / WIDTH_STEP * WIDTH_STEP).max(MIN_WIDTH);
        let height = height_of(&held[..count]);
        let x = x.min(bounds.right() - CLEAR_OF_EDGE - width).max(bounds.x);
        let y = y
            .min(bounds.bottom() - CLEAR_OF_EDGE - height)
            .max(bounds.y);
        Self {
            entries: held,
            count,
            rect: Rect::new(x, y, width, height),
            chosen: None,
            mnemonics: false,
        }
    }

    /// The same menu with entry `chosen` shown chosen.
    pub fn with_chosen(mut self, chosen: Option<usize>) -> Self {
        self.chosen = chosen.filter(|&index| index < self.count);
        self
    }

    /// The same menu with its mnemonics underlined when `shown`.
    pub fn with_mnemonics(mut self, shown: bool) -> Self {
        self.mnemonics = shown;
        self
    }

    /// The same menu with each check or radio item's mark as `mark` says by the item's id
    /// (None leaves the entry's own): the program's state, kept apart from the entries.
    pub fn with_marks(mut self, mark: impl Fn(u8) -> Option<bool>) -> Self {
        for entry in &mut self.entries[..self.count] {
            if let Some(item) = entry.item().filter(|item| item.mark.is_some())
                && let Some(on) = mark(item.id)
            {
                *entry = entry.with_mark(on);
            }
        }
        self
    }

    /// Whether it keeps a column for marks: it holds a check or radio item.
    pub fn has_marks(&self) -> bool {
        self.entries[..self.count]
            .iter()
            .any(|entry| entry.item().is_some_and(|item| item.mark.is_some()))
    }

    /// The entry shown chosen.
    pub fn chosen(&self) -> Option<usize> {
        self.chosen
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
            y += entry.height();
        }
        let height = match self.entries[..self.count].get(index) {
            Some(entry) => entry.height(),
            None => SEPARATOR_HEIGHT,
        };
        Rect::new(self.rect.x + 1, y, self.rect.width - 2, height)
    }

    /// The id of entry `index` when it is an enabled item.
    pub fn enabled(&self, index: usize) -> Option<u8> {
        self.entries[..self.count]
            .get(index)
            .and_then(Entry::item)
            .filter(|item| item.enabled)
            .map(|item| item.id)
    }

    /// The first enabled item, by index.
    pub fn first_enabled(&self) -> Option<usize> {
        (0..self.count).find(|&index| self.enabled(index).is_some())
    }

    /// The enabled items whose mnemonic is `letter`: the first after the chosen one (cycling),
    /// and how many there are.
    pub fn with_mnemonic(&self, letter: char) -> (Option<usize>, usize) {
        let letter = letter.to_ascii_lowercase();
        let matches = |index: usize| {
            self.entries[index]
                .item()
                .filter(|item| item.enabled)
                .is_some_and(|item| {
                    mnemonic(item.label).is_some_and(|(found, _, _)| found == letter)
                })
        };
        let count = (0..self.count).filter(|&index| matches(index)).count();
        let start = self.chosen.map_or(0, |chosen| chosen + 1);
        let next = (0..self.count)
            .map(|step| (start + step) % self.count.max(1))
            .find(|&index| matches(index));
        (next, count)
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
            key::UP | key::DOWN | key::HOME | key::END => {
                let mut enabled = (0..self.count).filter(|&index| self.enabled(index).is_some());
                let next = match (code, self.chosen) {
                    (key::HOME, _) => enabled.next(),
                    (key::END, _) => enabled.next_back(),
                    (key::DOWN, None) => enabled.next(),
                    (_, None) => enabled.next_back(),
                    (key::DOWN, Some(at)) => enabled.find(|&index| index > at).or(Some(at)),
                    (_, Some(at)) => enabled.rfind(|&index| index < at).or(Some(at)),
                };
                self.chosen = next;
                MenuKey::Nothing
            }
            _ => MenuKey::Nothing,
        }
    }

    /// Draws it.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        let palette = canvas.palette();
        canvas.rounded(self.rect, &INSET_R6, palette.menu);
        canvas.outline(self.rect, &INSET_R6, 1, palette.menu_border);
        let text_x = match self.has_marks() {
            true => TEXT_X + MARK_COLUMN,
            false => TEXT_X,
        };
        for (index, entry) in self.entries[..self.count].iter().enumerate() {
            let rect = self.entry_rect(index);
            match entry.item() {
                None => canvas.fill(
                    Rect::new(
                        self.rect.x + CLEAR_OF_EDGE,
                        rect.y + SEPARATOR_LINE,
                        self.rect.width - 2 * CLEAR_OF_EDGE,
                        1,
                    ),
                    palette.menu_border,
                ),
                Some(ItemParts {
                    label,
                    key,
                    enabled,
                    mark,
                    ..
                }) => {
                    if self.chosen == Some(index) {
                        canvas.fill(rect, palette.menu_hover);
                    }
                    let ink = match enabled {
                        true => palette.text,
                        false => palette.text_dim,
                    };
                    match mark {
                        Some(Mark::Check(true)) => paint_tick(canvas, self.rect.x, rect.y, ink),
                        Some(Mark::Radio(true)) => paint_dot(canvas, self.rect.x, rect.y, ink),
                        _ => {}
                    }
                    let y = rect.y + (ITEM_HEIGHT - LINE_HEIGHT) / 2;
                    let x = self.rect.x + text_x;
                    canvas.text(x, y, plain_label(label).as_str(), ink, 1);
                    if self.mnemonics {
                        underline(canvas, label, x, y, ink);
                    }
                    if !key.is_empty() {
                        let right = self.rect.right() - TEXT_X;
                        canvas.text(right - text::width(key), y, key, palette.text_dim, 1);
                    }
                }
            }
        }
    }
}

fn paint_tick(canvas: &mut Canvas<'_>, menu_x: i32, row_y: i32, ink: u32) {
    let (x, y) = (menu_x + MARK_X, row_y + TICK_TOP);
    for (column, row) in TICK_ROWS.iter().enumerate() {
        canvas.fill(Rect::new(x + column as i32, y + row, 1, 2), ink);
    }
}

fn paint_dot(canvas: &mut Canvas<'_>, menu_x: i32, row_y: i32, ink: u32) {
    let (x, y) = (menu_x + DOT_OFFSET, row_y + DOT_OFFSET);
    let size = DOT_INSETS.len() as i32;
    for (row, inset) in DOT_INSETS.iter().enumerate() {
        canvas.fill(Rect::new(x + inset, y + row as i32, size - 2 * inset, 1), ink);
    }
}

/// A window's menu bar: up to [`MAX_MENUS`] titles, each with its entries
/// (docs/ux/toolkit-v0.5.md section 3). While a menu is open, `highlighted` is its chosen
/// entry; while the bar is armed with none open, it is the highlighted title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    pub titles: [&'static str; MAX_MENUS],
    pub menus: [&'static [Entry]; MAX_MENUS],
    pub count: usize,
    pub open: Option<usize>,
    pub highlighted: Option<usize>,
    /// Whether the mnemonics are underlined: from F10 or the first Alt until the bar closes.
    pub mnemonics: bool,
}

impl Bar {
    /// A bar of `menus` (at most [`MAX_MENUS`]), closed.
    pub fn new(menus: &[(&'static str, &'static [Entry])]) -> Self {
        let count = menus.len().min(MAX_MENUS);
        let mut titles = [""; MAX_MENUS];
        let mut held: [&'static [Entry]; MAX_MENUS] = [&[]; MAX_MENUS];
        for (index, (title, entries)) in menus[..count].iter().enumerate() {
            titles[index] = title;
            held[index] = entries;
        }
        Self {
            titles,
            menus: held,
            count,
            open: None,
            highlighted: None,
            mnemonics: false,
        }
    }

    /// Whether the keyboard goes to it: a menu is open, or it is armed.
    pub fn has_keyboard(&self) -> bool {
        self.open.is_some() || self.mnemonics
    }

    fn title_width(title: &str) -> i32 {
        text::width(plain_label(title).as_str()) + 2 * MENU_TITLE_PADDING
    }

    /// Every title's width together.
    pub fn content_width(&self) -> i32 {
        self.titles[..self.count]
            .iter()
            .map(|title| Self::title_width(title))
            .sum()
    }

    /// Where title `index` is in a bar at `rect`: its hit rectangle, the bar's full height.
    pub fn title_rect(&self, rect: Rect, index: usize) -> Rect {
        let x = self.titles[..index.min(self.count)]
            .iter()
            .map(|title| Self::title_width(title))
            .sum::<i32>();
        Rect::new(
            rect.x + x,
            rect.y,
            Self::title_width(self.titles.get(index).copied().unwrap_or("")),
            rect.height,
        )
    }

    /// The title at (`x`, `y`) in a bar at `rect`.
    pub fn title_at(&self, rect: Rect, x: i32, y: i32) -> Option<usize> {
        (0..self.count).find(|&index| self.title_rect(rect, index).contains(x, y))
    }

    /// The title whose mnemonic is `letter`.
    pub fn title_with_mnemonic(&self, letter: char) -> Option<usize> {
        let letter = letter.to_ascii_lowercase();
        (0..self.count).find(|&index| {
            mnemonic(self.titles[index]).is_some_and(|(found, _, _)| found == letter)
        })
    }

    /// Its open menu, for a bar at `rect` in a window whose content is `window`.
    pub fn menu(&self, rect: Rect, window: Rect) -> Option<Menu> {
        let open = self.open?;
        let title = self.title_rect(rect, open);
        Some(
            Menu::new(self.menus[open], title.x, rect.bottom() + 1, window)
                .with_chosen(self.highlighted)
                .with_mnemonics(self.mnemonics),
        )
    }

    /// Writes its menus as `SET_MENU`'s pool into `out` (`M<title>`, `I<label>\t<key>`,
    /// `D<label>\t<key>`, `-`, each ending with a line break, `&` dropped); the pool's length,
    /// cut at the last whole entry that fits [`POOL_CAPACITY`].
    pub fn pool(&self, out: &mut [u8; POOL_CAPACITY]) -> usize {
        self.pool_with_marks(out, |_| None)
    }

    /// [`Self::pool`] with each check or radio item's mark as `mark` says by its id (None
    /// leaves the entry's own), as a third field: `c1` or `c0` for a check, `r1` or `r0` for a
    /// radio (docs/ux/toolkit-v0.6.md section 2.5).
    pub fn pool_with_marks(
        &self,
        out: &mut [u8; POOL_CAPACITY],
        mark: impl Fn(u8) -> Option<bool>,
    ) -> usize {
        let mut length = 0;
        let mut put = |parts: &[&[u8]], length: &mut usize| -> bool {
            let needed: usize = parts.iter().map(|part| part.len()).sum();
            if *length + needed > POOL_CAPACITY {
                return false;
            }
            for part in parts {
                out[*length..*length + part.len()].copy_from_slice(part);
                *length += part.len();
            }
            true
        };
        for index in 0..self.count {
            let title = plain_label(self.titles[index]);
            if !put(&[b"M", title.as_str().as_bytes(), b"\n"], &mut length) {
                return length;
            }
            for entry in self.menus[index].iter().take(MAX_ENTRIES) {
                let fits = match entry.item() {
                    None => put(&[b"-\n"], &mut length),
                    Some(item) => {
                        let label = plain_label(item.label);
                        let kind: &[u8] = if item.enabled { b"I" } else { b"D" };
                        let shown = |own: bool| mark(item.id).unwrap_or(own);
                        let marked: &[u8] = match item.mark {
                            None => b"",
                            Some(Mark::Check(own)) if shown(own) => b"\tc1",
                            Some(Mark::Check(_)) => b"\tc0",
                            Some(Mark::Radio(own)) if shown(own) => b"\tr1",
                            Some(Mark::Radio(_)) => b"\tr0",
                        };
                        put(
                            &[
                                kind,
                                label.as_str().as_bytes(),
                                b"\t",
                                item.key.as_bytes(),
                                marked,
                                b"\n",
                            ],
                            &mut length,
                        )
                    }
                };
                if !fits {
                    return length;
                }
            }
        }
        length
    }

    /// Draws it at `rect`, the title under the pointer as `look.hovered` says.
    pub fn paint(&self, canvas: &mut Canvas<'_>, rect: Rect, look: Look) {
        if rect.height == 0 {
            return;
        }
        let palette = canvas.palette();
        canvas.fill(rect, palette.menu);
        canvas.fill(
            Rect::new(rect.x, rect.bottom() - 1, rect.width, 1),
            palette.menu_border,
        );
        let armed_title = match self.open {
            Some(_) => None,
            None if self.mnemonics => self.highlighted,
            None => None,
        };
        for index in 0..self.count {
            let title = self.title_rect(rect, index);
            let lit = self.open == Some(index)
                || armed_title == Some(index)
                || (self.open.is_none() && look.enabled && look.hovered == Some(index));
            if lit {
                canvas.fill(
                    Rect::new(title.x, title.y, title.width, title.height - 1),
                    palette.menu_hover,
                );
            }
            let ink = match look.enabled {
                true => palette.text,
                false => palette.text_dim,
            };
            let (x, y) = (title.x + MENU_TITLE_PADDING, rect.y + TITLE_TEXT_TOP);
            canvas.text(x, y, plain_label(self.titles[index]).as_str(), ink, 1);
            if self.mnemonics {
                underline(canvas, self.titles[index], x, y, ink);
            }
        }
    }
}

/// A button that opens a short menu of `entries` (docs/ux/toolkit-v0.5.md section 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub text: Text<32>,
    pub entries: &'static [Entry],
    pub open: bool,
    pub highlighted: Option<usize>,
}

impl Button {
    /// Its menu, for a button at `rect` in a window whose content is `window`: below it, or
    /// above when below does not fit.
    pub fn menu(&self, rect: Rect, window: Rect) -> Menu {
        let height = height_of(self.entries);
        let below = rect.bottom() + MENU_BUTTON_GAP;
        let above = rect.y - MENU_BUTTON_GAP - height;
        let y = match below + height > window.bottom() && above >= window.y {
            true => above,
            false => below,
        };
        Menu::new(self.entries, rect.x, y, window).with_chosen(self.highlighted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRIES: [Entry; 4] = [
        Entry::Item {
            label: "Open",
            key: "",
            enabled: true,
            id: 1,
        },
        Entry::Separator,
        Entry::Item {
            label: "Paste",
            key: "",
            enabled: false,
            id: 2,
        },
        Entry::Item {
            label: "Delete permanently",
            key: "",
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
        assert_eq!(
            menu.entry_rect(2).y,
            rect.y + 1 + ITEM_HEIGHT + SEPARATOR_HEIGHT
        );
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

    const VIEW: [Entry; 5] = [
        Entry::Check {
            label: "&Wrap",
            key: "",
            enabled: true,
            id: 1,
            checked: false,
        },
        Entry::Separator,
        Entry::Radio {
            label: "&Small",
            key: "",
            enabled: true,
            id: 2,
            selected: true,
        },
        Entry::Radio {
            label: "&Large",
            key: "",
            enabled: true,
            id: 3,
            selected: false,
        },
        Entry::Item {
            label: "&Zoom",
            key: "",
            enabled: true,
            id: 4,
        },
    ];

    #[test]
    fn marks_come_from_the_program_and_a_radio_group_is_a_run() {
        let group = radio_group(&VIEW, 3).unwrap();
        assert_eq!(group.len(), 2);
        assert!(radio_group(&VIEW, 1).is_none());
        let menu = Menu::new(&VIEW, 0, 0, Rect::new(0, 0, 720, 480))
            .with_marks(|id| (id == 1 || id == 3).then_some(true));
        assert!(menu.has_marks());
        assert_eq!(menu.entries()[0].item().unwrap().mark, Some(Mark::Check(true)));
        assert_eq!(menu.entries()[3].item().unwrap().mark, Some(Mark::Radio(true)));
        assert_eq!(menu.enabled(0), Some(1));
        let plain = Menu::new(&ENTRIES, 0, 0, Rect::new(0, 0, 720, 480));
        assert!(!plain.has_marks());
    }

    #[test]
    fn the_pool_carries_each_mark_as_a_third_field() {
        let menus: [(&'static str, &'static [Entry]); 1] = [("&View", &VIEW)];
        let bar = Bar::new(&menus);
        let mut out = [0u8; POOL_CAPACITY];
        let length = bar.pool_with_marks(&mut out, |id| (id == 1).then_some(true));
        assert_eq!(
            &out[..length],
            b"MView\nIWrap\t\tc1\n-\nISmall\t\tr1\nILarge\t\tr0\nIZoom\t\n"
        );
    }
}
