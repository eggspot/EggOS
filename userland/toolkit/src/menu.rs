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
const ARROW_RIGHT: i32 = 12;
const ARROW_TOP: i32 = 11;
const ARROW_ROOM: i32 = 6;
const ARROW_SQUARES: [(i32, i32); 9] = [
    (0, 0),
    (0, 2),
    (0, 4),
    (0, 6),
    (0, 8),
    (2, 2),
    (2, 4),
    (2, 6),
    (4, 4),
];
/// The most levels open at once: a title's menu, a submenu and a submenu of that.
pub const DEPTH: usize = 3;
/// The overflow title's width at a bar's right end, its text and its name for agents
/// (docs/ux/toolkit-v0.6.md section 3).
pub const OVERFLOW_WIDTH: i32 = 33;
pub const OVERFLOW_TITLE: &str = "...";
pub const OVERFLOW_NAME: &str = "More";
/// How long the pointer rests on a submenu's row before it opens, or on another row before an
/// open submenu closes.
pub const SUBMENU_OPEN_MS: u32 = 150;

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
    /// A row that opens `entries` to its side; it has no id and is never chosen itself.
    Submenu {
        label: &'static str,
        enabled: bool,
        entries: &'static [Entry],
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
            Entry::Separator | Entry::Submenu { .. } => return None,
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

    /// Its label and whether it is enabled, for an item of any kind or a submenu's row.
    pub fn row(&self) -> Option<(&'static str, bool)> {
        match *self {
            Entry::Submenu { label, enabled, .. } => Some((label, enabled)),
            _ => self.item().map(|item| (item.label, item.enabled)),
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
    level: usize,
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
                .fold((0, 0), |(labels, keys), entry| match (entry.item(), entry) {
                    (Some(item), _) => (
                        labels.max(text::width(plain_label(item.label).as_str())),
                        keys.max(text::width(item.key)),
                    ),
                    (None, Entry::Submenu { label, .. }) => (
                        labels.max(text::width(plain_label(label).as_str())),
                        keys.max(ARROW_ROOM),
                    ),
                    (None, _) => (labels, keys),
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
            level: 0,
        }
    }

    /// The submenu of `entries` that row `row` of `parent` opens, inside `bounds`: its left
    /// edge on the parent's right border and its first row level with `row`, or opening to the
    /// left where the right has no room (docs/ux/toolkit-v0.6.md section 2.2).
    pub fn beside(parent: &Menu, row: usize, entries: &[Entry], bounds: Rect) -> Self {
        let row_rect = parent.entry_rect(row);
        let x = parent.rect.right() - 1;
        let y = row_rect.y - 1;
        let mut menu = Self::new(entries, x, y, bounds);
        if menu.rect.x < x {
            let left = parent.rect.x + 1 - menu.rect.width;
            menu = Self::new(entries, left.max(bounds.x), y, bounds);
        }
        menu.level = parent.level + 1;
        menu
    }

    /// Which level it is: 0 for a title's or a button's menu, 1 for its submenus, and so on.
    pub fn level(&self) -> usize {
        self.level
    }

    /// The entries row `index` opens when it is an enabled submenu's row this level may open.
    pub fn submenu(&self, index: usize) -> Option<&'static [Entry]> {
        match self.entries[..self.count].get(index) {
            Some(Entry::Submenu {
                enabled: true,
                entries,
                ..
            }) if self.level + 1 < DEPTH => Some(entries),
            _ => None,
        }
    }

    /// Whether row `index` may be highlighted: an enabled item or a submenu that opens.
    pub fn selectable(&self, index: usize) -> bool {
        self.enabled(index).is_some() || self.submenu(index).is_some()
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

    /// The first row that may be highlighted, by index.
    pub fn first_enabled(&self) -> Option<usize> {
        (0..self.count).find(|&index| self.selectable(index))
    }

    /// The enabled items whose mnemonic is `letter`: the first after the chosen one (cycling),
    /// and how many there are.
    pub fn with_mnemonic(&self, letter: char) -> (Option<usize>, usize) {
        let letter = letter.to_ascii_lowercase();
        let matches = |index: usize| {
            self.selectable(index)
                && self.entries[index].row().is_some_and(|(label, _)| {
                    mnemonic(label).is_some_and(|(found, _, _)| found == letter)
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
            .filter(|&index| self.selectable(index))
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
                let mut enabled = (0..self.count).filter(|&index| self.selectable(index));
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
            if let Entry::Submenu { label, .. } = entry {
                self.paint_submenu_row(canvas, index, label, text_x);
                continue;
            }
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

impl Menu {
    fn paint_submenu_row(&self, canvas: &mut Canvas<'_>, index: usize, label: &str, text_x: i32) {
        let palette = canvas.palette();
        let rect = self.entry_rect(index);
        if self.chosen == Some(index) {
            canvas.fill(rect, palette.menu_hover);
        }
        let ink = match self.submenu(index).is_some() {
            true => palette.text,
            false => palette.text_dim,
        };
        let y = rect.y + (ITEM_HEIGHT - LINE_HEIGHT) / 2;
        let x = self.rect.x + text_x;
        canvas.text(x, y, plain_label(label).as_str(), ink, 1);
        if self.mnemonics {
            underline(canvas, label, x, y, ink);
        }
        let left = self.rect.right() - ARROW_RIGHT - ARROW_ROOM;
        for (column, row) in ARROW_SQUARES {
            canvas.fill(Rect::new(left + column, rect.y + ARROW_TOP + row, 2, 2), ink);
        }
    }
}

/// The open levels of a menu (docs/ux/toolkit-v0.6.md section 2.1): its first level and the
/// submenus open from it, each opened from its parent's row on the path.
#[derive(Debug, Clone, Copy)]
pub struct Stack {
    levels: [Menu; DEPTH],
    depth: usize,
}

impl Stack {
    /// The levels of `first` with the submenus of `path` open inside `bounds`, each level's
    /// row on the path shown highlighted, the innermost's `chosen`, and each submenu's marks as
    /// `mark` says by item id.
    pub fn build(
        first: Menu,
        path: &[usize],
        chosen: Option<usize>,
        bounds: Rect,
        mark: &dyn Fn(u8) -> Option<bool>,
    ) -> Self {
        let mut levels = [first; DEPTH];
        let mut depth = 0;
        for &row in path.iter().take(DEPTH - 1) {
            let parent = levels[depth];
            let Some(entries) = parent.submenu(row) else {
                break;
            };
            levels[depth] = parent.with_chosen(Some(row));
            depth += 1;
            levels[depth] = Menu::beside(&parent, row, entries, bounds)
                .with_mnemonics(parent.mnemonics)
                .with_marks(mark);
        }
        if depth > 0 {
            levels[depth] = levels[depth].with_chosen(chosen);
        }
        Self { levels, depth }
    }

    /// How many submenus are open.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Level `level`, 0 the first, while it is open.
    pub fn level(&self, level: usize) -> Option<&Menu> {
        self.levels[..=self.depth].get(level)
    }

    /// The innermost open level.
    pub fn innermost(&self) -> &Menu {
        &self.levels[self.depth]
    }

    /// The innermost level whose rectangle holds (`x`, `y`).
    pub fn level_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..=self.depth)
            .rev()
            .find(|&level| self.levels[level].rect().contains(x, y))
    }

    /// Draws every open level, the first beneath.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        for menu in &self.levels[..=self.depth] {
            menu.paint(canvas);
        }
    }
}

/// A program's own context menu with its submenus (Files, the Text Editor; docs/ux/toolkit-v0.6.md
/// section 2.1, D706): a [`Stack`] driven as a single [`Menu`] is, by `hover`, `leave`, `key`,
/// `id_at` and `paint`. Pointing at a submenu's row opens it; Right or Enter opens the chosen
/// one with its first row chosen, Left or Esc closes the innermost.
#[derive(Debug, Clone, Copy)]
pub struct ContextMenu {
    first: Menu,
    path: [usize; DEPTH - 1],
    depth: usize,
    chosen: Option<usize>,
    bounds: Rect,
}

impl ContextMenu {
    /// `first` with no submenu open, its submenus placed inside `bounds`.
    pub fn new(first: Menu, bounds: Rect) -> Self {
        Self {
            chosen: first.chosen(),
            first,
            path: [0; DEPTH - 1],
            depth: 0,
            bounds,
        }
    }

    /// The open levels as they are drawn.
    pub fn stack(&self) -> Stack {
        let first = match self.depth {
            0 => self.first.with_chosen(self.chosen),
            _ => self.first,
        };
        Stack::build(
            first,
            &self.path[..self.depth],
            self.chosen,
            self.bounds,
            &|_| None,
        )
    }

    /// The smallest rectangle holding every open level, for damage.
    pub fn rect(&self) -> Rect {
        let stack = self.stack();
        (1..=stack.depth())
            .filter_map(|level| stack.level(level))
            .fold(self.first.rect(), |area, menu| area.union(&menu.rect()))
    }

    /// Whether (`x`, `y`) is on one of its open levels.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.stack().level_at(x, y).is_some()
    }

    /// The rectangle of entry `index` of the first level.
    pub fn entry_rect(&self, index: usize) -> Rect {
        self.first.entry_rect(index)
    }

    /// The id of the enabled item at (`x`, `y`), on any open level.
    pub fn id_at(&self, x: i32, y: i32) -> Option<u8> {
        let stack = self.stack();
        let level = stack.level_at(x, y)?;
        stack.level(level)?.id_at(x, y)
    }

    /// The pointer is at (`x`, `y`): the row under it is chosen, a submenu's row opens its
    /// submenu, and the levels below the pointed one close; whether anything changed.
    pub fn hover(&mut self, x: i32, y: i32) -> bool {
        let stack = self.stack();
        let Some(level) = stack.level_at(x, y) else {
            return false;
        };
        let Some(menu) = stack.level(level).copied() else {
            return false;
        };
        let row = menu.item_at(x, y);
        let before = (self.path, self.depth, self.chosen);
        match row {
            Some(row) if menu.submenu(row).is_some() && level < DEPTH - 1 => {
                if !(self.depth > level && self.path[level] == row) {
                    self.path[level] = row;
                    self.depth = level + 1;
                    self.chosen = None;
                }
            }
            _ => {
                self.depth = level;
                self.chosen = row;
            }
        }
        before != (self.path, self.depth, self.chosen)
    }

    /// The pointer left: nothing is chosen on the first level; open submenus stay.
    pub fn leave(&mut self) -> bool {
        self.depth == 0 && self.chosen.take().is_some()
    }

    /// Key `code` was pressed.
    pub fn key(&mut self, code: u16) -> MenuKey {
        let stack = self.stack();
        let innermost = *stack.innermost();
        let opens = self
            .chosen
            .filter(|&row| innermost.submenu(row).is_some() && self.depth < DEPTH - 1);
        match code {
            key::ESC | key::LEFT if self.depth > 0 => {
                self.depth -= 1;
                self.chosen = Some(self.path[self.depth]);
                MenuKey::Nothing
            }
            key::LEFT => MenuKey::Nothing,
            key::RIGHT | key::ENTER if opens.is_some() => {
                let row = opens.unwrap_or(0);
                self.path[self.depth] = row;
                self.depth += 1;
                self.chosen = self.stack().innermost().first_enabled();
                MenuKey::Nothing
            }
            key::RIGHT => MenuKey::Nothing,
            _ => {
                let mut menu = innermost.with_chosen(self.chosen);
                let answer = menu.key(code);
                self.chosen = menu.chosen();
                answer
            }
        }
    }

    /// Draws every open level.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        self.stack().paint(canvas);
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

    /// How many titles a bar at `rect` shows from the left: all when they fit its width, else
    /// those that end before the overflow title.
    pub fn shown(&self, rect: Rect) -> usize {
        if self.content_width() <= rect.width {
            return self.count;
        }
        let room = rect.width - OVERFLOW_WIDTH;
        let mut end = 0;
        self.titles[..self.count]
            .iter()
            .take_while(|title| {
                end += Self::title_width(title);
                end <= room
            })
            .count()
    }

    /// Whether a bar at `rect` hides titles behind its overflow title.
    pub fn overflows(&self, rect: Rect) -> bool {
        self.shown(rect) < self.count
    }

    /// The overflow title's index, after the last title: what `open` and `highlighted` hold
    /// for it.
    pub fn overflow_index(&self) -> usize {
        self.count
    }

    /// How many titles the arrows move between in a bar at `rect`: the shown ones and the
    /// overflow title when there is one.
    pub fn stops(&self, rect: Rect) -> usize {
        self.shown(rect) + usize::from(self.overflows(rect))
    }

    fn overflow_rect(rect: Rect) -> Rect {
        Rect::new(
            rect.right() - OVERFLOW_WIDTH,
            rect.y,
            OVERFLOW_WIDTH,
            rect.height,
        )
    }

    /// Where title `index` is in a bar at `rect`: its hit rectangle, the bar's full height; a
    /// hidden title and the overflow title are at the overflow title's place.
    pub fn title_rect(&self, rect: Rect, index: usize) -> Rect {
        if index >= self.shown(rect) && (index == self.count || self.overflows(rect)) {
            return Self::overflow_rect(rect);
        }
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
        let shown = (0..self.shown(rect)).find(|&index| self.title_rect(rect, index).contains(x, y));
        match shown {
            Some(index) => Some(index),
            None if self.overflows(rect) && Self::overflow_rect(rect).contains(x, y) => {
                Some(self.count)
            }
            None => None,
        }
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
        let y = rect.bottom() + 1;
        if open < self.shown(rect) {
            let title = self.title_rect(rect, open);
            return Some(
                Menu::new(self.menus[open], title.x, y, window)
                    .with_chosen(self.highlighted)
                    .with_mnemonics(self.mnemonics),
            );
        }
        let mut overflow = [Entry::Separator; MAX_MENUS];
        let entries: &[Entry] = match open == self.count {
            true => {
                let shown = self.shown(rect);
                for (place, index) in overflow.iter_mut().zip(shown..self.count) {
                    *place = Entry::Submenu {
                        label: self.titles[index],
                        enabled: true,
                        entries: self.menus[index],
                    };
                }
                &overflow[..self.count - shown]
            }
            false => self.menus.get(open).copied().unwrap_or(&[]),
        };
        let width = Menu::new(entries, 0, y, window).rect().width;
        Some(
            Menu::new(entries, rect.right() - width, y, window)
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
    /// radio; a submenu is an `S<label>` line (`s` when disabled), its entries, and an `E` line,
    /// two levels deep at most, a deeper one a disabled item (docs/ux/toolkit-v0.6.md section
    /// 2.5). A cut leaves no submenu open: room for the `E` lines it needs is kept.
    pub fn pool_with_marks(
        &self,
        out: &mut [u8; POOL_CAPACITY],
        mark: impl Fn(u8) -> Option<bool>,
    ) -> usize {
        let mut length = 0;
        for index in 0..self.count {
            let title = plain_label(self.titles[index]);
            if !put_in_pool(out, &mut length, &[b"M", title.as_str().as_bytes(), b"\n"], 0)
                || !pool_entries(self.menus[index], 0, &mark, out, &mut length)
            {
                return length;
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
        let shown = self.shown(rect);
        if shown < self.count {
            let title = Self::overflow_rect(rect);
            let open_behind = self.open.is_some_and(|open| open >= shown);
            let lit = open_behind
                || armed_title == Some(self.count)
                || (self.open.is_none() && look.enabled && look.hovered == Some(self.count));
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
            canvas.text(
                title.x + MENU_TITLE_PADDING,
                rect.y + TITLE_TEXT_TOP,
                OVERFLOW_TITLE,
                ink,
                1,
            );
        }
        for index in 0..shown {
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

const POOL_SUBMENU_DEPTH: usize = 2;
const POOL_SUBMENU_END: &[u8] = b"E\n";

/// The entry of `entries` whose depth-first number in the pool is `number`, as the top bar's
/// `MenuChosen` names it: every entry counts one, a submenu's row and then its entries, two
/// levels deep (docs/ux/toolkit-v0.6.md section 2.5).
pub fn entry_by_number(entries: &'static [Entry], number: usize) -> Option<&'static Entry> {
    let mut counted = 0;
    find_by_number(entries, 0, number, &mut counted)
}

fn find_by_number(
    entries: &'static [Entry],
    depth: usize,
    number: usize,
    counted: &mut usize,
) -> Option<&'static Entry> {
    for entry in entries.iter().take(MAX_ENTRIES) {
        if *counted == number {
            return Some(entry);
        }
        *counted += 1;
        if let Entry::Submenu {
            entries: nested, ..
        } = entry
            && depth < POOL_SUBMENU_DEPTH
            && let Some(found) = find_by_number(nested, depth + 1, number, counted)
        {
            return Some(found);
        }
    }
    None
}

fn put_in_pool(
    out: &mut [u8; POOL_CAPACITY],
    length: &mut usize,
    parts: &[&[u8]],
    reserved: usize,
) -> bool {
    let needed: usize = parts.iter().map(|part| part.len()).sum();
    if *length + needed + reserved > POOL_CAPACITY {
        return false;
    }
    for part in parts {
        out[*length..*length + part.len()].copy_from_slice(part);
        *length += part.len();
    }
    true
}

/// Writes `entries`, `depth` submenus deep, into the pool; whether all of them fitted.
fn pool_entries(
    entries: &[Entry],
    depth: usize,
    mark: &impl Fn(u8) -> Option<bool>,
    out: &mut [u8; POOL_CAPACITY],
    length: &mut usize,
) -> bool {
    let reserved = depth * POOL_SUBMENU_END.len();
    for entry in entries.iter().take(MAX_ENTRIES) {
        let fits = match (entry.item(), entry) {
            (
                None,
                Entry::Submenu {
                    label,
                    enabled,
                    entries: nested,
                },
            ) if depth < POOL_SUBMENU_DEPTH => {
                let label = plain_label(label);
                let start: &[u8] = if *enabled { b"S" } else { b"s" };
                let own_end = reserved + POOL_SUBMENU_END.len();
                if !put_in_pool(out, length, &[start, label.as_str().as_bytes(), b"\n"], own_end) {
                    return false;
                }
                let whole = pool_entries(nested, depth + 1, mark, out, length);
                put_in_pool(out, length, &[POOL_SUBMENU_END], reserved);
                whole
            }
            (None, Entry::Submenu { label, .. }) => {
                let label = plain_label(label);
                put_in_pool(out, length, &[b"D", label.as_str().as_bytes(), b"\t\n"], reserved)
            }
            (None, _) => put_in_pool(out, length, &[b"-\n"], reserved),
            (Some(item), _) => {
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
                put_in_pool(
                    out,
                    length,
                    &[
                        kind,
                        label.as_str().as_bytes(),
                        b"\t",
                        item.key.as_bytes(),
                        marked,
                        b"\n",
                    ],
                    reserved,
                )
            }
        };
        if !fits {
            return false;
        }
    }
    true
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

    const ZOOM: [Entry; 2] = [
        Entry::Item {
            label: "&In",
            key: "",
            enabled: true,
            id: 7,
        },
        Entry::Item {
            label: "&Out",
            key: "",
            enabled: true,
            id: 8,
        },
    ];

    const WITH_SUBMENU: [Entry; 2] = [
        Entry::Item {
            label: "&Wrap",
            key: "",
            enabled: true,
            id: 1,
        },
        Entry::Submenu {
            label: "&Zoom",
            enabled: true,
            entries: &ZOOM,
        },
    ];

    #[test]
    fn the_pool_writes_a_submenu_between_s_and_e_lines_and_never_cuts_one_open() {
        let menus: [(&'static str, &'static [Entry]); 1] = [("&View", &WITH_SUBMENU)];
        let bar = Bar::new(&menus);
        let mut out = [0u8; POOL_CAPACITY];
        let length = bar.pool(&mut out);
        assert_eq!(
            &out[..length],
            b"MView\nIWrap\t\nSZoom\nIIn\t\nIOut\t\nE\n"
        );
        let mut whole = 0;
        let fits = pool_entries(&WITH_SUBMENU, 0, &|_| None, &mut out, &mut whole);
        assert!(fits);
        let mut cut = POOL_CAPACITY - 12;
        assert!(!pool_entries(&WITH_SUBMENU, 0, &|_| None, &mut out, &mut cut));
        let written = &out[POOL_CAPACITY - 12..cut];
        let opened = written.iter().filter(|&&byte| byte == b'S').count();
        let closed = written.split(|&byte| byte == b'\n').filter(|line| *line == b"E").count();
        assert_eq!(opened, closed);
    }

    #[test]
    fn depth_first_numbers_reach_into_submenus() {
        let number = |n| entry_by_number(&WITH_SUBMENU, n).and_then(Entry::item).map(|item| item.id);
        assert_eq!(number(0), Some(1));
        assert_eq!(entry_by_number(&WITH_SUBMENU, 1).and_then(Entry::item), None);
        assert_eq!(number(2), Some(7));
        assert_eq!(number(3), Some(8));
        assert!(entry_by_number(&WITH_SUBMENU, 4).is_none());
    }

    #[test]
    fn a_context_menu_opens_a_submenu_by_pointing_and_by_keys() {
        let bounds = Rect::new(0, 0, 720, 480);
        let mut menu = ContextMenu::new(Menu::new(&WITH_SUBMENU, 10, 10, bounds), bounds);
        let zoom = menu.entry_rect(1);
        assert!(menu.hover(zoom.x + 4, zoom.y + 4));
        assert_eq!(menu.stack().depth(), 1);
        let inner = menu.stack().innermost().rect();
        assert!(menu.contains(inner.x + 4, inner.y + 20));
        assert_eq!(menu.id_at(inner.x + 4, inner.y + 1 + 32 + 16), Some(8));
        assert_eq!(menu.key(key::LEFT), MenuKey::Nothing);
        assert_eq!(menu.stack().depth(), 0);
        assert_eq!(menu.key(key::RIGHT), MenuKey::Nothing);
        assert_eq!(menu.stack().depth(), 1);
        assert_eq!(menu.key(key::DOWN), MenuKey::Nothing);
        assert_eq!(menu.key(key::ENTER), MenuKey::Chosen(8));
        let mut plain = ContextMenu::new(Menu::new(&ZOOM, 10, 10, bounds), bounds);
        assert_eq!(plain.key(key::DOWN), MenuKey::Nothing);
        assert_eq!(plain.key(key::ENTER), MenuKey::Chosen(7));
        assert_eq!(plain.key(key::ESC), MenuKey::Closed);
    }

    #[test]
    fn a_submenu_opens_beside_its_row_and_to_the_left_without_room() {
        let bounds = Rect::new(0, 0, 720, 480);
        let first = Menu::new(&WITH_SUBMENU, 10, 10, bounds);
        assert!(first.selectable(1) && first.enabled(1).is_none());
        let stack = Stack::build(first, &[1], Some(0), bounds, &|_| None);
        assert_eq!(stack.depth(), 1);
        let submenu = stack.innermost();
        assert_eq!(submenu.rect().x, first.rect().right() - 1);
        assert_eq!(submenu.rect().y, first.entry_rect(1).y - 1);
        assert_eq!(submenu.chosen(), Some(0));
        assert_eq!(stack.level(0).unwrap().chosen(), Some(1));
        let narrow = Rect::new(0, 0, first.rect().right() + 40, 480);
        let crowded = Stack::build(first, &[1], None, narrow, &|_| None);
        assert!(crowded.innermost().rect().right() <= first.rect().x + 1);
        let deepest = Menu::new(&WITH_SUBMENU, 0, 0, bounds);
        let third = Stack::build(deepest, &[1, 0], None, bounds, &|_| None);
        assert_eq!(third.depth(), 1);
    }

    #[test]
    fn a_narrow_bar_hides_the_titles_that_do_not_fit_behind_its_overflow_title() {
        let menus: [(&'static str, &'static [Entry]); 3] = [
            ("&File", &ENTRIES),
            ("&Edit", &ENTRIES),
            ("&Tools", &ZOOM),
        ];
        let mut bar = Bar::new(&menus);
        let wide = Rect::new(0, 0, bar.content_width(), 28);
        assert_eq!(bar.shown(wide), 3);
        assert!(!bar.overflows(wide));
        let first_two = Bar::title_width("&File") + Bar::title_width("&Edit");
        let narrow = Rect::new(0, 0, first_two + OVERFLOW_WIDTH, 28);
        assert_eq!(bar.shown(narrow), 2);
        assert_eq!(bar.stops(narrow), 3);
        assert_eq!(bar.title_at(narrow, narrow.right() - 4, 4), Some(3));
        assert_eq!(bar.title_rect(narrow, 2), bar.title_rect(narrow, 3));
        bar.open = Some(bar.overflow_index());
        let window = Rect::new(0, 0, 720, 480);
        let menu = bar.menu(narrow, window).unwrap();
        assert_eq!(menu.entries().len(), 1);
        assert_eq!(menu.submenu(0), Some(&ZOOM[..]));
        bar.open = Some(2);
        assert_eq!(bar.menu(narrow, window).unwrap().entries().len(), ZOOM.len());
    }
}
