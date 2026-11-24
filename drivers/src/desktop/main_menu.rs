//! The app's main menu in the macOS profile's top bar (docs/ux/macos-gui-v0.3.md, section 5;
//! D147): the menus a client sets for its normal window (`SetMenu`), their titles in the bar
//! while that window or a dialog of it is focused, and the open menu under a title, chosen by
//! pointer or keys, which sends the window `MenuChosen`. Submenus (`S`, `s` and `E` lines,
//! docs/ux/toolkit-v0.6.md section 2.5, D704) open beside the row they come from, two levels
//! deep at most, and their entries are numbered depth-first in their title's menu.

use super::paint::Canvas;
use super::popups::with_shadow;
use super::protocol::{
    EventRecord, EventSink, MAX_MENU_ENTRIES, MAX_MENUS, MENU_POOL_CAPACITY, Role, WindowError,
    event,
};
use super::shell::PanelItem;
use super::shortcuts::Profile;
use super::{
    BAR_TEXT_Y, CHIP_X, Desktop, MENU_GAP, MENU_ITEM_HEIGHT, MENU_TEXT_X, MENU_WIDTH,
    TERMINAL_SLOT, colour, contains,
};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::{button, key};
use alloc::boxed::Box;

const MENU_TITLES_X: u32 = CHIP_X;
const MENU_TITLE_PADDING: u32 = 8;
const KEY_LABEL_GAP: u32 = 32;
const KEY_LABEL_RIGHT: u32 = 12;
const MENU_WIDTH_STEP: u32 = 8;
const SEPARATOR_HEIGHT: u32 = 9;
const SEPARATOR_LINE_Y: u32 = 4;
const SEPARATOR_INSET: u32 = 8;
const SCREEN_MARGIN: u32 = 8;
const HOVER_INSET: u32 = 2;
const MENU_START: u8 = b'M';
const ITEM_START: u8 = b'I';
const DISABLED_ITEM_START: u8 = b'D';
const SEPARATOR_START: u8 = b'-';
const SUBMENU_START: u8 = b'S';
const DISABLED_SUBMENU_START: u8 = b's';
const SUBMENU_END: u8 = b'E';
const MAX_SUBMENUS: usize = 24;
const MAX_PANELS: usize = MAX_MENUS + MAX_SUBMENUS;
pub(super) const MAX_SUBMENU_DEPTH: usize = 2;
const NO_SUBMENU: u8 = u8::MAX;
const ARROW_WIDTH: u32 = 6;
const ARROW_RIGHT: u32 = 12;
const ARROW_TOP: u32 = 11;
const ARROW_COLUMNS: [(u32, u32); 3] = [(0, 10), (2, 6), (4, 2)];
const ENTRY_END: u8 = b'\n';
const MARK_COLUMN: u32 = 20;
const MARK_X: u32 = 12;
const TICK_TOP: u32 = 12;
const TICK_ROWS: [u32; 8] = [4, 5, 6, 5, 4, 3, 2, 1];
const DOT_OFFSET: u32 = 13;
const DOT_INSETS: [u32; 6] = [2, 1, 0, 0, 1, 2];
const KEY_LABEL_START: u8 = b'\t';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Span {
    start: u8,
    end: u8,
}

/// What one entry of a menu is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    Item,
    DisabledItem,
    Separator,
    Submenu,
    DisabledSubmenu,
}

impl EntryKind {
    fn height(self) -> u32 {
        match self {
            Self::Separator => SEPARATOR_HEIGHT,
            Self::Item | Self::DisabledItem | Self::Submenu | Self::DisabledSubmenu => {
                MENU_ITEM_HEIGHT
            }
        }
    }
}

/// The mark of a check or radio item, the pool's third field (`c1`, `c0`, `r1`, `r0`;
/// docs/ux/toolkit-v0.6.md section 2.5), and whether it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryMark {
    None,
    Check(bool),
    Radio(bool),
}

impl EntryMark {
    fn parse(field: &[u8]) -> Result<Self, WindowError> {
        Ok(match field {
            b"c1" => Self::Check(true),
            b"c0" => Self::Check(false),
            b"r1" => Self::Radio(true),
            b"r0" => Self::Radio(false),
            _ => return Err(WindowError::Invalid),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    label: Span,
    key_label: Span,
    mark: EntryMark,
    submenu: u8,
    number: u8,
}

const SEPARATOR: Entry = Entry {
    kind: EntryKind::Separator,
    label: Span { start: 0, end: 0 },
    key_label: Span { start: 0, end: 0 },
    mark: EntryMark::None,
    submenu: NO_SUBMENU,
    number: 0,
};

#[derive(Debug, Clone, Copy)]
struct Menu {
    title: Span,
    entries: [Entry; MAX_MENU_ENTRIES],
    length: u8,
    owner: u8,
}

const NO_MENU: Menu = Menu {
    title: Span { start: 0, end: 0 },
    entries: [SEPARATOR; MAX_MENU_ENTRIES],
    length: 0,
    owner: 0,
};

/// Where the parser adds entries: the menus and submenus open from the title down, and the
/// next depth-first number in the title's menu.
struct Parsing {
    open: [u8; MAX_SUBMENU_DEPTH + 1],
    depth: usize,
    number: u8,
}

/// A window's main menu as `SetMenu` set it: its pool of labels and the menus, submenus and
/// entries parsed from it, as spans of the pool. Menus `0..menu_count()` are the titles'; a
/// submenu is a menu of its own after them, which its row names.
#[derive(Debug, Clone)]
pub(super) struct MainMenu {
    pool: [u8; MENU_POOL_CAPACITY],
    pool_length: u8,
    menus: [Menu; MAX_PANELS],
    length: u8,
    submenus: u8,
}

impl MainMenu {
    /// The main menu `pool` describes (docs/windows.md, `SET_MENU`); `Invalid` for a pool that
    /// is empty, too long, not UTF-8, not made of whole entries, without a menu first, with a
    /// control character in a label, with more menus, entries, submenus or levels than the
    /// limits, or with a submenu left open.
    pub fn parse(pool: &[u8]) -> Result<Box<Self>, WindowError> {
        let whole_entries = pool.last() == Some(&ENTRY_END);
        if !whole_entries || pool.len() > MENU_POOL_CAPACITY {
            return Err(WindowError::Invalid);
        }
        core::str::from_utf8(pool).map_err(|_| WindowError::Invalid)?;
        let mut parsed = Box::new(Self {
            pool: [0; MENU_POOL_CAPACITY],
            pool_length: pool.len() as u8,
            menus: [NO_MENU; MAX_PANELS],
            length: 0,
            submenus: 0,
        });
        parsed.pool[..pool.len()].copy_from_slice(pool);
        let mut parsing = Parsing {
            open: [0; MAX_SUBMENU_DEPTH + 1],
            depth: 0,
            number: 0,
        };
        let mut start = 0;
        while start < pool.len() {
            let end = pool[start..]
                .iter()
                .position(|&byte| byte == ENTRY_END)
                .map(|at| start + at)
                .ok_or(WindowError::Invalid)?;
            parsed.add_entry(start, end, &mut parsing)?;
            start = end + 1;
        }
        if parsing.depth != 0 {
            return Err(WindowError::Invalid);
        }
        Ok(parsed)
    }

    fn add_entry(
        &mut self,
        start: usize,
        end: usize,
        parsing: &mut Parsing,
    ) -> Result<(), WindowError> {
        if start >= end {
            return Err(WindowError::Invalid);
        }
        let text = start + 1;
        match self.pool[start] {
            MENU_START => {
                let title = self.label_span(text, end)?;
                let index = usize::from(self.length);
                if index == MAX_MENUS || parsing.depth != 0 {
                    return Err(WindowError::Invalid);
                }
                self.menus[index].title = title;
                self.menus[index].owner = index as u8;
                self.length += 1;
                parsing.open[0] = index as u8;
                parsing.number = 0;
                Ok(())
            }
            kind @ (SUBMENU_START | DISABLED_SUBMENU_START) => {
                let label = self.label_span(text, end)?;
                let index = MAX_MENUS + usize::from(self.submenus);
                if self.length == 0
                    || parsing.depth == MAX_SUBMENU_DEPTH
                    || usize::from(self.submenus) == MAX_SUBMENUS
                {
                    return Err(WindowError::Invalid);
                }
                let kind = match kind {
                    SUBMENU_START => EntryKind::Submenu,
                    _ => EntryKind::DisabledSubmenu,
                };
                self.push(
                    parsing,
                    Entry {
                        kind,
                        label,
                        key_label: Span { start: 0, end: 0 },
                        mark: EntryMark::None,
                        submenu: index as u8,
                        number: 0,
                    },
                )?;
                let owner = parsing.open[0];
                self.menus[index].title = label;
                self.menus[index].owner = owner;
                self.submenus += 1;
                parsing.depth += 1;
                parsing.open[parsing.depth] = index as u8;
                Ok(())
            }
            SUBMENU_END if end == text => match parsing.depth {
                0 => Err(WindowError::Invalid),
                _ => {
                    parsing.depth -= 1;
                    Ok(())
                }
            },
            kind @ (ITEM_START | DISABLED_ITEM_START) => {
                let tab = self.pool[text..end]
                    .iter()
                    .position(|&byte| byte == KEY_LABEL_START)
                    .map(|at| text + at)
                    .ok_or(WindowError::Invalid)?;
                let label = self.label_span(text, tab)?;
                let mark_tab = self.pool[tab + 1..end]
                    .iter()
                    .position(|&byte| byte == KEY_LABEL_START)
                    .map(|at| tab + 1 + at);
                let (key_label, mark) = match mark_tab {
                    Some(mark_tab) => (
                        self.label_span(tab + 1, mark_tab)?,
                        EntryMark::parse(&self.pool[mark_tab + 1..end])?,
                    ),
                    None => (self.label_span(tab + 1, end)?, EntryMark::None),
                };
                let kind = match kind {
                    ITEM_START => EntryKind::Item,
                    _ => EntryKind::DisabledItem,
                };
                self.push(
                    parsing,
                    Entry {
                        kind,
                        label,
                        key_label,
                        mark,
                        submenu: NO_SUBMENU,
                        number: 0,
                    },
                )
            }
            SEPARATOR_START if end == text => self.push(parsing, SEPARATOR),
            _ => Err(WindowError::Invalid),
        }
    }

    fn label_span(&self, start: usize, end: usize) -> Result<Span, WindowError> {
        let clean = core::str::from_utf8(&self.pool[start..end])
            .is_ok_and(|label| !label.chars().any(char::is_control));
        if !clean {
            return Err(WindowError::Invalid);
        }
        Ok(Span {
            start: start as u8,
            end: end as u8,
        })
    }

    fn push(&mut self, parsing: &mut Parsing, entry: Entry) -> Result<(), WindowError> {
        if self.length == 0 {
            return Err(WindowError::Invalid);
        }
        let menu = &mut self.menus[usize::from(parsing.open[parsing.depth])];
        let index = usize::from(menu.length);
        if index == MAX_MENU_ENTRIES {
            return Err(WindowError::Invalid);
        }
        menu.entries[index] = Entry {
            number: parsing.number,
            ..entry
        };
        menu.length += 1;
        parsing.number = parsing.number.wrapping_add(1);
        Ok(())
    }

    fn text(&self, span: Span) -> &str {
        core::str::from_utf8(&self.pool[usize::from(span.start)..usize::from(span.end)])
            .unwrap_or("")
    }

    fn entry(&self, menu: usize, entry: usize) -> Entry {
        self.menus[menu].entries[entry]
    }

    /// The pool it was parsed from, as `SetMenu` carried it.
    pub fn pool(&self) -> &[u8] {
        &self.pool[..usize::from(self.pool_length)]
    }

    /// How many menus it has.
    pub fn menu_count(&self) -> usize {
        usize::from(self.length)
    }

    /// Menu `menu`'s title.
    pub fn title(&self, menu: usize) -> &str {
        self.text(self.menus[menu].title)
    }

    /// How many entries menu `menu` has, separators included.
    pub fn entry_count(&self, menu: usize) -> usize {
        usize::from(self.menus[menu].length)
    }

    /// What entry `entry` of menu `menu` is.
    pub fn entry_kind(&self, menu: usize, entry: usize) -> EntryKind {
        self.entry(menu, entry).kind
    }

    /// The label of entry `entry` of menu `menu`.
    pub fn entry_label(&self, menu: usize, entry: usize) -> &str {
        self.text(self.entry(menu, entry).label)
    }

    /// The mark of entry `entry` of menu `menu`.
    pub fn entry_mark(&self, menu: usize, entry: usize) -> EntryMark {
        self.entry(menu, entry).mark
    }

    /// Whether menu `menu` keeps a column for marks: it holds a check or radio item.
    pub fn has_marks(&self, menu: usize) -> bool {
        (0..self.entry_count(menu)).any(|entry| self.entry_mark(menu, entry) != EntryMark::None)
    }

    /// Where menu `menu`'s labels start from its left edge.
    pub fn text_x(&self, menu: usize) -> u32 {
        match self.has_marks(menu) {
            true => MENU_TEXT_X + MARK_COLUMN,
            false => MENU_TEXT_X,
        }
    }

    /// The key label of entry `entry` of menu `menu`, empty when it has none.
    pub fn entry_key_label(&self, menu: usize, entry: usize) -> &str {
        self.text(self.entry(menu, entry).key_label)
    }

    /// Whether entry `entry` of menu `menu` is an item that may be chosen.
    pub fn is_enabled(&self, menu: usize, entry: usize) -> bool {
        entry < self.entry_count(menu) && self.entry_kind(menu, entry) == EntryKind::Item
    }

    /// The submenu entry `entry` of menu `menu` opens, if it is an enabled submenu row.
    pub fn submenu(&self, menu: usize, entry: usize) -> Option<usize> {
        (entry < self.entry_count(menu) && self.entry_kind(menu, entry) == EntryKind::Submenu)
            .then(|| usize::from(self.entry(menu, entry).submenu))
    }

    /// Whether the pointer and the keys stop on entry `entry` of menu `menu`: an enabled item
    /// or submenu row.
    pub fn is_hoverable(&self, menu: usize, entry: usize) -> bool {
        self.is_enabled(menu, entry) || self.submenu(menu, entry).is_some()
    }

    /// Whether menu `menu` holds a submenu row.
    pub fn has_submenus(&self, menu: usize) -> bool {
        (0..self.entry_count(menu)).any(|entry| {
            matches!(
                self.entry_kind(menu, entry),
                EntryKind::Submenu | EntryKind::DisabledSubmenu
            )
        })
    }

    /// The title whose menu menu `menu` belongs to: itself for a title's menu.
    pub fn owner(&self, menu: usize) -> usize {
        usize::from(self.menus[menu].owner)
    }

    /// Entry `entry` of menu `menu`'s depth-first number in its title's menu, what `MenuChosen`
    /// and `CHOOSE_MENU` carry: the entry's index when the menu has no submenu.
    pub fn number(&self, menu: usize, entry: usize) -> usize {
        usize::from(self.entry(menu, entry).number)
    }

    /// The menu and entry whose depth-first number in title `title`'s menu is `number`.
    pub fn find(&self, title: usize, number: usize) -> Option<(usize, usize)> {
        let submenus = MAX_MENUS..MAX_MENUS + usize::from(self.submenus);
        core::iter::once(title)
            .chain(submenus.filter(|&menu| self.owner(menu) == title))
            .find_map(|menu| {
                (0..self.entry_count(menu))
                    .find(|&entry| self.number(menu, entry) == number)
                    .map(|entry| (menu, entry))
            })
    }

    /// Menu `menu`'s width, its border included: room for its widest item's label, a gap and
    /// its key label, rounded up to a glyph, and never narrower than the power menu.
    pub fn width(&self, menu: usize) -> u32 {
        let arrow_room = match self.has_submenus(menu) {
            true => ARROW_WIDTH,
            false => 0,
        };
        let widest = (0..self.entry_count(menu))
            .filter(|&entry| self.entry_kind(menu, entry) != EntryKind::Separator)
            .map(|entry| {
                self.text_x(menu)
                    + text_width(self.entry_label(menu, entry))
                    + KEY_LABEL_GAP
                    + text_width(self.entry_key_label(menu, entry)).max(arrow_room)
                    + KEY_LABEL_RIGHT
            })
            .max()
            .unwrap_or(0);
        MENU_WIDTH.max(widest.next_multiple_of(MENU_WIDTH_STEP))
    }

    /// Menu `menu`'s height, its border included.
    pub fn height(&self, menu: usize) -> u32 {
        2 + (0..self.entry_count(menu))
            .map(|entry| self.entry_kind(menu, entry).height())
            .sum::<u32>()
    }

    /// The top row of entry `entry` of menu `menu` drawn in `rect`.
    pub fn entry_top(&self, menu: usize, rect: Rect, entry: usize) -> u32 {
        rect.y
            + 1
            + (0..entry)
                .map(|before| self.entry_kind(menu, before).height())
                .sum::<u32>()
    }

    /// The entry of menu `menu` drawn in `rect` at row `y`, if one is there.
    pub fn entry_at(&self, menu: usize, rect: Rect, y: u32) -> Option<usize> {
        let mut top = rect.y + 1;
        for entry in 0..self.entry_count(menu) {
            let bottom = top + self.entry_kind(menu, entry).height();
            if y >= top && y < bottom {
                return Some(entry);
            }
            top = bottom;
        }
        None
    }

    /// The enabled item or submenu row of menu `menu` after `from` (before it when `back`),
    /// wrapping; the first (last) when there is no `from`.
    pub fn next_enabled(&self, menu: usize, from: Option<usize>, back: bool) -> Option<usize> {
        let count = self.entry_count(menu);
        if count == 0 {
            return None;
        }
        let start = match (from, back) {
            (Some(entry), false) => entry + 1,
            (Some(entry), true) => entry + count - 1,
            (None, false) => 0,
            (None, true) => count - 1,
        };
        (0..count)
            .map(|step| match back {
                false => (start + step) % count,
                true => (start + count - step) % count,
            })
            .find(|&entry| self.is_hoverable(menu, entry))
    }
}

fn text_width(text: &str) -> u32 {
    text.chars().count() as u32 * GLYPH_WIDTH as u32
}

/// Whose main menu the top bar shows, and whether it is inert (a dialog of its window is
/// focused).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MenuBar {
    pub window: usize,
    pub inert: bool,
}

/// A submenu open beside its parent: the row of the parent it came from, and its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct OpenSubmenu {
    pub from: u8,
    pub menu: u8,
}

/// The open menu: whose main menu, which of its title menus, the submenus open from it (the
/// first `depth` of `submenus`), and the row the pointer or the keys are on in the deepest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpenMainMenu {
    pub window: usize,
    pub menu: usize,
    pub hover: Option<usize>,
    pub submenus: [OpenSubmenu; MAX_SUBMENU_DEPTH],
    pub depth: u8,
}

impl OpenMainMenu {
    fn new(window: usize, menu: usize, hover: Option<usize>) -> Self {
        Self {
            window,
            menu,
            hover,
            submenus: [OpenSubmenu::default(); MAX_SUBMENU_DEPTH],
            depth: 0,
        }
    }

    fn levels(&self) -> usize {
        usize::from(self.depth) + 1
    }

    /// The menu shown at `level`, 0 being the title's.
    fn menu_at(&self, level: usize) -> usize {
        match level {
            0 => self.menu,
            _ => usize::from(self.submenus[level - 1].menu),
        }
    }

    /// The deepest open menu.
    fn deepest(&self) -> usize {
        self.menu_at(usize::from(self.depth))
    }

    /// The row level `level` is drawn hovered on: the deepest level's hover, or the row the
    /// next level was opened from.
    fn hover_at(&self, level: usize) -> Option<usize> {
        match level == usize::from(self.depth) {
            true => self.hover,
            false => Some(usize::from(self.submenus[level].from)),
        }
    }

    /// Only the first `levels` levels kept open, the hover on `hover` in the deepest of them.
    fn cut(self, levels: usize, hover: Option<usize>) -> Self {
        Self {
            depth: (levels.max(1) - 1) as u8,
            hover,
            ..self
        }
    }

    /// The submenu `menu`, opened from row `from` of level `level`, as the deepest level.
    fn opened(self, level: usize, from: usize, menu: usize, hover: Option<usize>) -> Self {
        let mut submenus = self.submenus;
        submenus[level] = OpenSubmenu {
            from: from as u8,
            menu: menu as u8,
        };
        Self {
            submenus,
            depth: (level + 1) as u8,
            hover,
            ..self
        }
    }
}

impl Desktop {
    /// Gives `client`'s normal window `id` the main menu `pool` describes, replacing the one it
    /// had; an empty pool removes it. The pool's entries each end with `\n`: `M<title>` starts
    /// a menu, `I<label>\t<key label>` is an item, `D<label>\t<key label>` a disabled one, `-`
    /// a separator (docs/windows.md, `SET_MENU`). The macOS profile's top bar shows it while
    /// the window, or a dialog of it, is focused.
    pub fn set_menu(
        &mut self,
        client: u16,
        id: u32,
        pool: &[u8],
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        if slot == TERMINAL_SLOT || self.window_at(slot).role != Role::Normal {
            return Err(WindowError::Invalid);
        }
        let menu = match pool.is_empty() {
            true => None,
            false => Some(MainMenu::parse(pool)?),
        };
        let before = self.menu_bar();
        let open = self.open_main_menu.filter(|open| open.window == slot);
        if open.is_some() {
            self.damage_open_main_menu(damage);
        }
        self.main_menus[slot] = menu;
        if let Some(open) = open {
            let menu = self.main_menus[slot].as_deref();
            let shown = open.menu < self.shown_menu_titles(slot);
            self.open_main_menu = match menu {
                Some(menu) if shown => Some(OpenMainMenu::new(
                    open.window,
                    open.menu,
                    open.hover
                        .filter(|_| open.depth == 0)
                        .filter(|&entry| menu.is_hoverable(open.menu, entry)),
                )),
                _ => None,
            };
            self.damage_open_main_menu(damage);
        }
        let after = self.menu_bar();
        if before != after || after.is_some_and(|bar| bar.window == slot) {
            self.damage_menu_titles(damage);
            self.forget_menu_title_hover();
        }
        Ok(())
    }

    /// The pool of window `window`'s main menu, any client's window, as `SetMenu` set it; empty
    /// when it has none (`GET_MENU`). [`WindowError::NoSuchWindow`] for no such window.
    pub fn menu_pool(&self, window: u32) -> Result<&[u8], WindowError> {
        let slot = self.any_window(window)?;
        Ok(self.main_menus[slot]
            .as_deref()
            .map_or(&[][..], MainMenu::pool))
    }

    /// Chooses entry `entry` (its depth-first number, which counts the rows of submenus) of menu
    /// `menu` of window `window`'s main menu for a command or an agent (`CHOOSE_MENU`), as a
    /// person's choice in the top bar does: the window's menu closes
    /// if it is open, and the window gets `MenuChosen`. It works on a covered, minimised or
    /// unfocused window, in every profile, and moves neither focus nor the pointer. `label` is
    /// the entry's label as the caller read it (`GET_MENU`): a menu set again since then with
    /// another item there is not chosen by mistake. [`WindowError::NoSuchWindow`] for no such
    /// window, [`WindowError::Busy`] while [`Self::person_is_active`], [`WindowError::Invalid`]
    /// when the window has no such enabled item, its label is not `label`, or the window's
    /// client has a dialog open (as a macOS app's menus are while it shows an alert),
    /// [`WindowError::QueueFull`] when its client's queue has no room (nothing is sent).
    pub fn choose_menu(
        &mut self,
        window: u32,
        menu: u32,
        entry: u32,
        label: &[u8],
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.any_window(window)?;
        if self.window_at(slot).takes_person_input_only() {
            return Err(WindowError::NotAllowed);
        }
        if self.person_is_active() {
            return Err(WindowError::Busy);
        }
        let (menu, entry) = (menu as usize, entry as usize);
        let enabled = self.main_menus[slot].as_deref().is_some_and(|main| {
            menu < main.menu_count()
                && main.find(menu, entry).is_some_and(|(panel, row)| {
                    main.is_enabled(panel, row) && main.entry_label(panel, row).as_bytes() == label
                })
        });
        if !enabled || self.has_dialog(self.window_at(slot).client) {
            return Err(WindowError::Invalid);
        }
        if events.room(self.window_at(slot).client) == 0 {
            return Err(WindowError::QueueFull);
        }
        if self.open_main_menu.is_some_and(|open| open.window == slot) {
            self.close_main_menu(damage);
        }
        self.send(
            slot,
            EventRecord::new(event::MENU_CHOSEN, 0, menu as i32, entry as i32, 0, 0),
            events,
        );
        Ok(())
    }

    /// The hit target of title `menu` of the main menu the top bar shows, if it shows it.
    pub fn menu_title(&self, menu: usize) -> Option<Rect> {
        self.menu_bar()
            .and_then(|bar| self.menu_title_rect(bar.window, menu))
    }

    /// The open menu's rectangle, its border included, while one is open.
    pub fn main_menu_area(&self) -> Option<Rect> {
        self.open_main_menu
            .and_then(|open| self.main_menu_rect(open))
    }

    /// The rectangle of the submenu open at level `level` (1 or 2), while it is.
    pub fn main_submenu_area(&self, level: usize) -> Option<Rect> {
        let open = self.open_main_menu?;
        (level >= 1 && level < open.levels())
            .then(|| self.level_rect(open, level))
            .flatten()
    }

    /// Whether (`x`, `y`) is on the open menu or one of its open submenus.
    pub(super) fn main_menu_covers(&self, x: u32, y: u32) -> bool {
        self.open_main_menu.is_some_and(|open| {
            (0..open.levels()).any(|level| {
                self.level_rect(open, level)
                    .is_some_and(|rect| contains(rect, x, y))
            })
        })
    }

    /// Whether the open menu or one of its submenus, their shadows included, overlaps `clip`.
    pub(super) fn main_menu_overlaps(&self, clip: Rect) -> bool {
        self.open_main_menu.is_some_and(|open| {
            (0..open.levels()).any(|level| {
                self.level_rect(open, level)
                    .is_some_and(|rect| with_shadow(rect).overlaps(&clip))
            })
        })
    }

    /// The rectangle of entry `entry` of the open menu, while one is open and has it.
    pub fn main_menu_entry(&self, entry: usize) -> Option<Rect> {
        let open = self.open_main_menu?;
        let menu = self.main_menus[open.window].as_deref()?;
        let rect = self.main_menu_rect(open)?;
        (entry < menu.entry_count(open.menu)).then(|| {
            Rect::new(
                rect.x,
                menu.entry_top(open.menu, rect, entry),
                rect.width,
                menu.entry_kind(open.menu, entry).height(),
            )
        })
    }

    fn has_dialog(&self, client: u16) -> bool {
        self.windows
            .iter()
            .flatten()
            .any(|window| window.role == Role::Dialog && window.client == client)
    }

    /// Whose main menu the top bar shows now: the focused window's, or its parent's when the
    /// focused window is a dialog without one (then inert); none outside the macOS profile.
    pub(super) fn menu_bar(&self) -> Option<MenuBar> {
        if self.profile != Profile::Macos || self.layout.top_bar.is_none() {
            return None;
        }
        let focus = self.focus.filter(|&slot| self.is_visible(slot))?;
        if self.main_menus[focus].is_some() {
            return Some(MenuBar {
                window: focus,
                inert: false,
            });
        }
        if self.window_at(focus).role != Role::Dialog {
            return None;
        }
        let parent = self.owner_window(focus);
        self.main_menus[parent].is_some().then_some(MenuBar {
            window: parent,
            inert: true,
        })
    }

    /// Calls `title` with each title of window `window`'s main menu that fits the top bar and
    /// its hit target, left to right.
    pub(super) fn for_each_menu_title(&self, window: usize, mut title: impl FnMut(usize, Rect)) {
        let (Some(bar), Some(menu)) = (self.layout.top_bar, self.main_menus[window].as_deref())
        else {
            return;
        };
        if self.profile != Profile::Macos {
            return;
        }
        let end = self.chips_area().right();
        let mut x = MENU_TITLES_X;
        for index in 0..menu.menu_count() {
            let width = text_width(menu.title(index)) + 2 * MENU_TITLE_PADDING;
            if x + width > end {
                return;
            }
            title(index, Rect::new(x, bar.y, width, bar.height));
            x += width;
        }
    }

    fn menu_title_rect(&self, window: usize, menu: usize) -> Option<Rect> {
        let mut found = None;
        self.for_each_menu_title(window, |index, rect| {
            if index == menu {
                found = Some(rect);
            }
        });
        found
    }

    fn menu_title_at(&self, window: usize, x: u32, y: u32) -> Option<usize> {
        let mut found = None;
        self.for_each_menu_title(window, |index, rect| {
            if contains(rect, x, y) {
                found = Some(index);
            }
        });
        found
    }

    fn shown_menu_titles(&self, window: usize) -> usize {
        let mut count = 0;
        self.for_each_menu_title(window, |_, _| count += 1);
        count
    }

    /// Where level `level` of `open` is drawn: the title's menu under its title, a submenu
    /// beside the row it came from, its left edge on the parent's right border and its first
    /// row level with that row, or to the left of the parent when it would pass the screen's
    /// right margin, and moved up when it would pass the bottom one.
    fn level_rect(&self, open: OpenMainMenu, level: usize) -> Option<Rect> {
        if level == 0 {
            return self.main_menu_rect(open);
        }
        let parent = self.level_rect(open, level - 1)?;
        let menu = self.main_menus[open.window].as_deref()?;
        let shown = open.menu_at(level);
        let (width, height) = (menu.width(shown), menu.height(shown));
        let from = usize::from(open.submenus[level - 1].from);
        let right_bound = self.width.saturating_sub(SCREEN_MARGIN);
        let beside = parent.right().saturating_sub(1);
        let x = match beside + width <= right_bound {
            true => beside,
            false => match (parent.x + 1).checked_sub(width) {
                Some(left) if left >= SCREEN_MARGIN => left,
                _ => right_bound.saturating_sub(width),
            },
        };
        let row = menu.entry_top(open.menu_at(level - 1), parent, from);
        let y = row
            .saturating_sub(1)
            .min(self.height.saturating_sub(SCREEN_MARGIN + height));
        Some(Rect::new(x, y, width, height))
    }

    fn main_menu_rect(&self, open: OpenMainMenu) -> Option<Rect> {
        let menu = self.main_menus[open.window].as_deref()?;
        let title = self.menu_title_rect(open.window, open.menu)?;
        let bar = self.layout.top_bar?;
        let width = menu.width(open.menu);
        Some(Rect::new(
            title
                .x
                .min(self.width.saturating_sub(SCREEN_MARGIN + width)),
            bar.bottom() + MENU_GAP,
            width,
            menu.height(open.menu),
        ))
    }

    fn damage_menu_titles(&self, damage: &mut Damage) {
        if self.profile != Profile::Macos {
            return;
        }
        if let Some(bar) = self.layout.top_bar {
            let end = self.chips_area().right();
            damage.add(Rect::new(
                MENU_TITLES_X,
                bar.y,
                end.saturating_sub(MENU_TITLES_X),
                bar.height,
            ));
        }
    }

    fn damage_open_main_menu(&self, damage: &mut Damage) {
        if let Some(open) = self.open_main_menu {
            for level in 0..open.levels() {
                if let Some(rect) = self.level_rect(open, level) {
                    damage.add(with_shadow(rect));
                }
            }
        }
    }

    fn set_open_main_menu(&mut self, open: OpenMainMenu, damage: &mut Damage) {
        self.damage_open_main_menu(damage);
        self.open_main_menu = Some(open);
        self.damage_open_main_menu(damage);
    }

    fn forget_menu_title_hover(&mut self) {
        if matches!(self.panels.hover, Some(PanelItem::MenuTitle(_))) {
            self.panels.hover = None;
        }
        self.panels.moved = true;
    }

    /// The focus moved, and the top bar showed `before`'s main menu: if it shows another now,
    /// the open menu closes and the titles are drawn again.
    pub(super) fn menu_bar_moved(&mut self, before: Option<MenuBar>, damage: &mut Damage) {
        if self.menu_bar() == before {
            return;
        }
        self.close_main_menu(damage);
        self.damage_menu_titles(damage);
        self.forget_menu_title_hover();
    }

    /// Window `slot` is going away: its main menu goes with it, and the top bar is drawn again
    /// if it showed it.
    pub(super) fn forget_main_menu(&mut self, slot: usize, damage: &mut Damage) {
        let bar = self.menu_bar();
        let shown = bar.is_some_and(|bar| bar.window == slot || self.focus == Some(slot));
        if shown || self.open_main_menu.is_some_and(|open| open.window == slot) {
            self.close_main_menu(damage);
            self.damage_menu_titles(damage);
            self.forget_menu_title_hover();
        }
        self.main_menus[slot] = None;
    }

    /// A press on title `menu` in the top bar: opens its menu, or closes it if it is open.
    pub(super) fn press_menu_title(
        &mut self,
        menu: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(bar) = self.menu_bar().filter(|bar| !bar.inert) else {
            return;
        };
        let open = OpenMainMenu::new(bar.window, menu, None);
        if self
            .open_main_menu
            .is_some_and(|already| already.window == open.window && already.menu == menu)
        {
            self.close_main_menu(damage);
            return;
        }
        if self.main_menu_rect(open).is_none() {
            return;
        }
        self.close_popups(damage, events);
        self.hide_tooltip(damage);
        self.open_main_menu = Some(open);
        self.damage_open_main_menu(damage);
        self.damage_menu_titles(damage);
    }

    fn switch_main_menu(&mut self, menu: usize, by_keys: bool, damage: &mut Damage) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        if open.menu == menu {
            return;
        }
        let hover = match by_keys {
            true => self.main_menus[open.window]
                .as_deref()
                .and_then(|main_menu| main_menu.next_enabled(menu, None, false)),
            false => None,
        };
        self.set_open_main_menu(OpenMainMenu::new(open.window, menu, hover), damage);
        self.damage_menu_titles(damage);
    }

    /// Closes the open menu, if one is.
    pub(super) fn close_main_menu(&mut self, damage: &mut Damage) {
        if self.open_main_menu.is_some() {
            self.damage_open_main_menu(damage);
            self.open_main_menu = None;
            self.damage_menu_titles(damage);
        }
    }

    fn choose_main_menu_entry(
        &mut self,
        level: usize,
        entry: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        let shown = open.menu_at(level);
        let number = self.main_menus[open.window]
            .as_deref()
            .filter(|menu| menu.is_enabled(shown, entry))
            .map(|menu| menu.number(shown, entry));
        let Some(number) = number else {
            return;
        };
        self.close_main_menu(damage);
        self.send(
            open.window,
            EventRecord::new(event::MENU_CHOSEN, 0, open.menu as i32, number as i32, 0, 0),
            events,
        );
    }

    /// Opens the submenu of row `entry` of level `level`, closing any deeper one, the keys'
    /// hover on its first row when `by_keys`; nothing for a row that opens none.
    fn open_main_submenu(&mut self, level: usize, entry: usize, by_keys: bool, damage: &mut Damage) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        let Some(main) = self.main_menus[open.window].as_deref() else {
            return;
        };
        let Some(submenu) = main.submenu(open.menu_at(level), entry) else {
            return;
        };
        if level >= MAX_SUBMENU_DEPTH {
            return;
        }
        let already = usize::from(open.depth) > level
            && usize::from(open.submenus[level].menu) == submenu
            && usize::from(open.submenus[level].from) == entry;
        let hover = by_keys
            .then(|| main.next_enabled(submenu, None, false))
            .flatten();
        let next = match already {
            true => open.cut(level + 2, hover.or(open.hover_at(level + 1))),
            false => open.opened(level, entry, submenu, hover),
        };
        if Some(next) != self.open_main_menu {
            self.set_open_main_menu(next, damage);
        }
    }

    /// The level and row under (`x`, `y`) in the open menu or its submenus, the deepest first.
    fn main_menu_row_at(&self, open: OpenMainMenu, x: u32, y: u32) -> Option<(usize, Option<usize>)> {
        let main = self.main_menus[open.window].as_deref()?;
        (0..open.levels()).rev().find_map(|level| {
            let rect = self.level_rect(open, level)?;
            contains(rect, x, y).then(|| (level, main.entry_at(open.menu_at(level), rect, y)))
        })
    }

    /// Button `code` went down at (`x`, `y`) while a menu may be open: a left press on an item
    /// chooses it, a press elsewhere on the menu does nothing, one on another title opens that
    /// menu, one on the open title closes it; anywhere else the menu closes. Returns whether
    /// the press was the menu's.
    pub(super) fn main_menu_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> bool {
        let Some(open) = self.open_main_menu else {
            return false;
        };
        if let Some((level, entry)) = self.main_menu_row_at(open, x, y) {
            if code == button::LEFT
                && let Some(entry) = entry
            {
                let opens = self.main_menus[open.window]
                    .as_deref()
                    .and_then(|menu| menu.submenu(open.menu_at(level), entry));
                match opens {
                    Some(_) => self.open_main_submenu(level, entry, false, damage),
                    None => self.choose_main_menu_entry(level, entry, damage, events),
                }
            }
            return true;
        }
        match self.menu_title_at(open.window, x, y) {
            Some(menu) if menu != open.menu => self.switch_main_menu(menu, false, damage),
            Some(_) => self.close_main_menu(damage),
            None => {
                self.close_main_menu(damage);
                return false;
            }
        }
        true
    }

    /// The pointer moved to (`x`, `y`) while a menu may be open: over another title it opens
    /// that menu, over the menu the enabled item under it is hovered.
    pub(super) fn main_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        if let Some(menu) = self.menu_title_at(open.window, x, y) {
            self.switch_main_menu(menu, false, damage);
            return;
        }
        let Some((level, entry)) = self.main_menu_row_at(open, x, y) else {
            return;
        };
        let Some(main) = self.main_menus[open.window].as_deref() else {
            return;
        };
        let shown = open.menu_at(level);
        let hover = entry.filter(|&entry| main.is_hoverable(shown, entry));
        if let Some(entry) = hover
            && main.submenu(shown, entry).is_some()
        {
            self.open_main_submenu(level, entry, false, damage);
            return;
        }
        let next = open.cut(level + 1, hover);
        if next != open {
            self.set_open_main_menu(next, damage);
        }
    }

    /// Key `code` went down (`pressed`) or up while a menu is open: Esc closes it, Up and Down
    /// move the hover over the enabled items, Left and Right open the menu beside it, Enter
    /// chooses the hovered item.
    pub(super) fn main_menu_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        if !pressed {
            return;
        }
        let deepest = usize::from(open.depth);
        let opens = open.hover.and_then(|entry| {
            self.main_menus[open.window]
                .as_deref()
                .and_then(|menu| menu.submenu(open.deepest(), entry))
        });
        match code {
            key::ESC if deepest > 0 => {
                let back = open.hover_at(deepest - 1);
                self.set_open_main_menu(open.cut(deepest, back), damage);
            }
            key::ESC => self.close_main_menu(damage),
            key::UP | key::DOWN => {
                let hover = self.main_menus[open.window].as_deref().and_then(|menu| {
                    menu.next_enabled(open.deepest(), open.hover, code == key::UP)
                });
                if hover != open.hover {
                    self.set_open_main_menu(OpenMainMenu { hover, ..open }, damage);
                }
            }
            key::RIGHT if opens.is_some() => {
                if let Some(entry) = open.hover {
                    self.open_main_submenu(deepest, entry, true, damage);
                }
            }
            key::LEFT if deepest > 0 => {
                let back = open.hover_at(deepest - 1);
                self.set_open_main_menu(open.cut(deepest, back), damage);
            }
            key::LEFT | key::RIGHT => {
                let count = self.shown_menu_titles(open.window);
                if count > 1 {
                    let next = match code {
                        key::LEFT => (open.menu + count - 1) % count,
                        _ => (open.menu + 1) % count,
                    };
                    self.switch_main_menu(next, true, damage);
                }
            }
            key::ENTER | key::KEYPAD_ENTER => match (open.hover, opens) {
                (Some(entry), Some(_)) => self.open_main_submenu(deepest, entry, true, damage),
                (Some(entry), None) => self.choose_main_menu_entry(deepest, entry, damage, events),
                (None, _) => {}
            },
            _ => {}
        }
    }

    pub(super) fn draw_menu_titles(&self, canvas: &mut Canvas<'_>) {
        let Some(bar) = self.menu_bar() else {
            return;
        };
        let Some(menu) = self.main_menus[bar.window].as_deref() else {
            return;
        };
        let open = self
            .open_main_menu
            .filter(|open| open.window == bar.window)
            .map(|open| open.menu);
        let text_colour = match bar.inert {
            true => colour::current().text_dim,
            false => colour::current().text,
        };
        self.for_each_menu_title(bar.window, |index, rect| {
            let hovered = self.panels.hover == Some(PanelItem::MenuTitle(index as u8));
            let fill = match (open == Some(index), hovered && !bar.inert) {
                (true, _) => Some(colour::current().bar_pressed),
                (false, true) => Some(colour::current().bar_hover),
                (false, false) => None,
            };
            if let Some(fill) = fill {
                canvas.fill(Rect::new(rect.x, rect.y, rect.width, rect.height - 1), fill);
            }
            canvas.text(
                rect.x + MENU_TITLE_PADDING,
                rect.y + BAR_TEXT_Y,
                menu.title(index),
                text_colour,
            );
        });
    }

    pub(super) fn draw_main_menu(&self, canvas: &mut Canvas<'_>) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        let Some(menu) = self.main_menus[open.window].as_deref() else {
            return;
        };
        for level in 0..open.levels() {
            if let Some(rect) = self.level_rect(open, level) {
                Self::draw_main_menu_level(canvas, menu, open.menu_at(level), rect, open.hover_at(level));
            }
        }
    }

    fn draw_main_menu_level(
        canvas: &mut Canvas<'_>,
        menu: &MainMenu,
        shown: usize,
        rect: Rect,
        hover: Option<usize>,
    ) {
        Self::draw_popup_surface(canvas, rect, colour::current().menu);
        let mut top = rect.y + 1;
        for entry in 0..menu.entry_count(shown) {
            let kind = menu.entry_kind(shown, entry);
            if kind == EntryKind::Separator {
                canvas.fill(
                    Rect::new(
                        rect.x + SEPARATOR_INSET,
                        top + SEPARATOR_LINE_Y,
                        rect.width - 2 * SEPARATOR_INSET,
                        1,
                    ),
                    colour::current().menu_border,
                );
                top += kind.height();
                continue;
            }
            if hover == Some(entry) {
                canvas.fill(
                    Rect::new(
                        rect.x + 1,
                        top + HOVER_INSET,
                        rect.width - 2,
                        MENU_ITEM_HEIGHT - 2 * HOVER_INSET,
                    ),
                    colour::current().menu_hover,
                );
            }
            let label_colour = match kind {
                EntryKind::Item | EntryKind::Submenu => colour::current().text,
                _ => colour::current().text_dim,
            };
            Self::draw_mark(canvas, menu.entry_mark(shown, entry), rect.x, top, label_colour);
            let y = top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
            canvas.text(
                rect.x + menu.text_x(shown),
                y,
                menu.entry_label(shown, entry),
                label_colour,
            );
            match kind {
                EntryKind::Submenu | EntryKind::DisabledSubmenu => {
                    Self::draw_submenu_arrow(canvas, rect.right(), top, label_colour);
                }
                _ => {
                    let key_label = menu.entry_key_label(shown, entry);
                    canvas.text(
                        rect.right()
                            .saturating_sub(KEY_LABEL_RIGHT + text_width(key_label)),
                        y,
                        key_label,
                        colour::current().text_dim,
                    );
                }
            }
            top += kind.height();
        }
    }

    fn draw_mark(canvas: &mut Canvas<'_>, mark: EntryMark, x: u32, top: u32, colour: u32) {
        match mark {
            EntryMark::Check(true) => {
                for (column, row) in TICK_ROWS.iter().enumerate() {
                    canvas.fill(
                        Rect::new(x + MARK_X + column as u32, top + TICK_TOP + row, 1, 2),
                        colour,
                    );
                }
            }
            EntryMark::Radio(true) => {
                let size = DOT_INSETS.len() as u32;
                for (row, inset) in DOT_INSETS.iter().enumerate() {
                    canvas.fill(
                        Rect::new(
                            x + DOT_OFFSET + inset,
                            top + DOT_OFFSET + row as u32,
                            size - 2 * inset,
                            1,
                        ),
                        colour,
                    );
                }
            }
            _ => {}
        }
    }

    fn draw_submenu_arrow(canvas: &mut Canvas<'_>, right: u32, top: u32, colour: u32) {
        let left = right.saturating_sub(ARROW_RIGHT + ARROW_WIDTH);
        for (column, (first_row, rows)) in ARROW_COLUMNS.iter().enumerate() {
            canvas.fill(
                Rect::new(left + 2 * column as u32, top + ARROW_TOP + first_row, 2, *rows),
                colour,
            );
        }
    }
}
