//! The app's main menu in the macOS profile's top bar (docs/ux/macos-gui-v0.3.md, section 5;
//! D147): the menus a client sets for its normal window (`SetMenu`), their titles in the bar
//! while that window or a dialog of it is focused, and the open menu under a title, chosen by
//! pointer or keys, which sends the window `MenuChosen`.

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
const ENTRY_END: u8 = b'\n';
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
}

impl EntryKind {
    fn height(self) -> u32 {
        match self {
            Self::Separator => SEPARATOR_HEIGHT,
            Self::Item | Self::DisabledItem => MENU_ITEM_HEIGHT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    label: Span,
    key_label: Span,
}

const SEPARATOR: Entry = Entry {
    kind: EntryKind::Separator,
    label: Span { start: 0, end: 0 },
    key_label: Span { start: 0, end: 0 },
};

#[derive(Debug, Clone, Copy)]
struct Menu {
    title: Span,
    entries: [Entry; MAX_MENU_ENTRIES],
    length: u8,
}

const NO_MENU: Menu = Menu {
    title: Span { start: 0, end: 0 },
    entries: [SEPARATOR; MAX_MENU_ENTRIES],
    length: 0,
};

/// A window's main menu as `SetMenu` set it: its pool of labels and the menus and entries
/// parsed from it, as spans of the pool.
#[derive(Debug, Clone)]
pub(super) struct MainMenu {
    pool: [u8; MENU_POOL_CAPACITY],
    pool_length: u8,
    menus: [Menu; MAX_MENUS],
    length: u8,
}

impl MainMenu {
    /// The main menu `pool` describes (docs/windows.md, `SET_MENU`); `Invalid` for a pool that
    /// is empty, too long, not UTF-8, not made of whole entries, without a menu first, with a
    /// control character in a label, or with more menus or entries than the limits.
    pub fn parse(pool: &[u8]) -> Result<Box<Self>, WindowError> {
        let whole_entries = pool.last() == Some(&ENTRY_END);
        if !whole_entries || pool.len() > MENU_POOL_CAPACITY {
            return Err(WindowError::Invalid);
        }
        core::str::from_utf8(pool).map_err(|_| WindowError::Invalid)?;
        let mut parsed = Box::new(Self {
            pool: [0; MENU_POOL_CAPACITY],
            pool_length: pool.len() as u8,
            menus: [NO_MENU; MAX_MENUS],
            length: 0,
        });
        parsed.pool[..pool.len()].copy_from_slice(pool);
        let mut start = 0;
        while start < pool.len() {
            let end = pool[start..]
                .iter()
                .position(|&byte| byte == ENTRY_END)
                .map(|at| start + at)
                .ok_or(WindowError::Invalid)?;
            parsed.add_entry(start, end)?;
            start = end + 1;
        }
        Ok(parsed)
    }

    fn add_entry(&mut self, start: usize, end: usize) -> Result<(), WindowError> {
        if start >= end {
            return Err(WindowError::Invalid);
        }
        let text = start + 1;
        match self.pool[start] {
            MENU_START => {
                let title = self.label_span(text, end)?;
                let index = usize::from(self.length);
                if index == MAX_MENUS {
                    return Err(WindowError::Invalid);
                }
                self.menus[index].title = title;
                self.length += 1;
                Ok(())
            }
            kind @ (ITEM_START | DISABLED_ITEM_START) => {
                let tab = self.pool[text..end]
                    .iter()
                    .position(|&byte| byte == KEY_LABEL_START)
                    .map(|at| text + at)
                    .ok_or(WindowError::Invalid)?;
                let label = self.label_span(text, tab)?;
                let key_label = self.label_span(tab + 1, end)?;
                let kind = match kind {
                    ITEM_START => EntryKind::Item,
                    _ => EntryKind::DisabledItem,
                };
                self.push(Entry {
                    kind,
                    label,
                    key_label,
                })
            }
            SEPARATOR_START if end == text => self.push(SEPARATOR),
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

    fn push(&mut self, entry: Entry) -> Result<(), WindowError> {
        let menu = usize::from(self.length.checked_sub(1).ok_or(WindowError::Invalid)?);
        let menu = &mut self.menus[menu];
        let index = usize::from(menu.length);
        if index == MAX_MENU_ENTRIES {
            return Err(WindowError::Invalid);
        }
        menu.entries[index] = entry;
        menu.length += 1;
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

    /// The key label of entry `entry` of menu `menu`, empty when it has none.
    pub fn entry_key_label(&self, menu: usize, entry: usize) -> &str {
        self.text(self.entry(menu, entry).key_label)
    }

    /// Whether entry `entry` of menu `menu` is an item that may be chosen.
    pub fn is_enabled(&self, menu: usize, entry: usize) -> bool {
        entry < self.entry_count(menu) && self.entry_kind(menu, entry) == EntryKind::Item
    }

    /// Menu `menu`'s width, its border included: room for its widest item's label, a gap and
    /// its key label, rounded up to a glyph, and never narrower than the power menu.
    pub fn width(&self, menu: usize) -> u32 {
        let widest = (0..self.entry_count(menu))
            .filter(|&entry| self.entry_kind(menu, entry) != EntryKind::Separator)
            .map(|entry| {
                MENU_TEXT_X
                    + text_width(self.entry_label(menu, entry))
                    + KEY_LABEL_GAP
                    + text_width(self.entry_key_label(menu, entry))
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

    /// The enabled item of menu `menu` after `from` (before it when `back`), wrapping; the
    /// first (last) when there is no `from`.
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
            .find(|&entry| self.is_enabled(menu, entry))
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

/// The open menu: whose main menu, which of its menus, and the item the pointer or the keys
/// are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpenMainMenu {
    pub window: usize,
    pub menu: usize,
    pub hover: Option<usize>,
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
                Some(menu) if shown => Some(OpenMainMenu {
                    hover: open
                        .hover
                        .filter(|&entry| menu.is_enabled(open.menu, entry)),
                    ..open
                }),
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

    /// Chooses entry `entry` of menu `menu` of window `window`'s main menu for a command or an
    /// agent (`CHOOSE_MENU`), as a person's choice in the top bar does: the window's menu closes
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
        if self.person_is_active() {
            return Err(WindowError::Busy);
        }
        let (menu, entry) = (menu as usize, entry as usize);
        let enabled = self.main_menus[slot].as_deref().is_some_and(|main| {
            menu < main.menu_count()
                && main.is_enabled(menu, entry)
                && main.entry_label(menu, entry).as_bytes() == label
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
        let window = self.window_at(focus);
        let parent = window
            .parent
            .filter(|_| window.role == Role::Dialog)
            .map(usize::from)?;
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
        if let Some(rect) = self.main_menu_area() {
            damage.add(with_shadow(rect));
        }
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
        let open = OpenMainMenu {
            window: bar.window,
            menu,
            hover: None,
        };
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
        self.damage_open_main_menu(damage);
        self.open_main_menu = Some(OpenMainMenu {
            menu,
            hover,
            ..open
        });
        self.damage_open_main_menu(damage);
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
        entry: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(open) = self.open_main_menu else {
            return;
        };
        let enabled = self.main_menus[open.window]
            .as_deref()
            .is_some_and(|menu| menu.is_enabled(open.menu, entry));
        if !enabled {
            return;
        }
        self.close_main_menu(damage);
        self.send(
            open.window,
            EventRecord::new(event::MENU_CHOSEN, 0, open.menu as i32, entry as i32, 0, 0),
            events,
        );
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
        if let Some(rect) = self.main_menu_rect(open)
            && contains(rect, x, y)
        {
            let entry = self.main_menus[open.window]
                .as_deref()
                .and_then(|menu| menu.entry_at(open.menu, rect, y));
            if code == button::LEFT
                && let Some(entry) = entry
            {
                self.choose_main_menu_entry(entry, damage, events);
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
        let Some(rect) = self.main_menu_rect(open) else {
            return;
        };
        if !contains(rect, x, y) {
            return;
        }
        let hover = self.main_menus[open.window].as_deref().and_then(|menu| {
            menu.entry_at(open.menu, rect, y)
                .filter(|&entry| menu.is_enabled(open.menu, entry))
        });
        if hover != open.hover {
            self.open_main_menu = Some(OpenMainMenu { hover, ..open });
            damage.add(rect);
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
        match code {
            key::ESC => self.close_main_menu(damage),
            key::UP | key::DOWN => {
                let hover = self.main_menus[open.window]
                    .as_deref()
                    .and_then(|menu| menu.next_enabled(open.menu, open.hover, code == key::UP));
                if hover != open.hover {
                    self.open_main_menu = Some(OpenMainMenu { hover, ..open });
                    if let Some(rect) = self.main_menu_rect(open) {
                        damage.add(rect);
                    }
                }
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
            key::ENTER | key::KEYPAD_ENTER => {
                if let Some(entry) = open.hover {
                    self.choose_main_menu_entry(entry, damage, events);
                }
            }
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
            true => colour::TEXT_DIM,
            false => colour::TEXT,
        };
        self.for_each_menu_title(bar.window, |index, rect| {
            let hovered = self.panels.hover == Some(PanelItem::MenuTitle(index as u8));
            let fill = match (open == Some(index), hovered && !bar.inert) {
                (true, _) => Some(colour::BAR_PRESSED),
                (false, true) => Some(colour::BAR_HOVER),
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
        let (Some(menu), Some(rect)) = (
            self.main_menus[open.window].as_deref(),
            self.main_menu_rect(open),
        ) else {
            return;
        };
        Self::draw_popup_surface(canvas, rect, colour::MENU);
        let mut top = rect.y + 1;
        for entry in 0..menu.entry_count(open.menu) {
            let kind = menu.entry_kind(open.menu, entry);
            match kind {
                EntryKind::Separator => canvas.fill(
                    Rect::new(
                        rect.x + SEPARATOR_INSET,
                        top + SEPARATOR_LINE_Y,
                        rect.width - 2 * SEPARATOR_INSET,
                        1,
                    ),
                    colour::MENU_BORDER,
                ),
                EntryKind::Item | EntryKind::DisabledItem => {
                    if open.hover == Some(entry) {
                        canvas.fill(
                            Rect::new(
                                rect.x + 1,
                                top + HOVER_INSET,
                                rect.width - 2,
                                MENU_ITEM_HEIGHT - 2 * HOVER_INSET,
                            ),
                            colour::MENU_HOVER,
                        );
                    }
                    let label_colour = match kind {
                        EntryKind::Item => colour::TEXT,
                        _ => colour::TEXT_DIM,
                    };
                    let y = top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
                    canvas.text(
                        rect.x + MENU_TEXT_X,
                        y,
                        menu.entry_label(open.menu, entry),
                        label_colour,
                    );
                    let key_label = menu.entry_key_label(open.menu, entry);
                    canvas.text(
                        rect.right()
                            .saturating_sub(KEY_LABEL_RIGHT + text_width(key_label)),
                        y,
                        key_label,
                        colour::TEXT_DIM,
                    );
                }
            }
            top += kind.height();
        }
    }
}
