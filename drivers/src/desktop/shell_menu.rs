//! A Windows program's context menu shown by the server as a shell menu (D371,
//! `docs/ux/windows-tooltips-tray-v0.5.md` section 14): the entries the layer sends when
//! `TrackPopupMenu`'s point lies outside the program's content (a tray icon's menu), drawn in
//! the power menu's chrome and anchored to the tray icon last pressed, each submenu in a panel
//! of its own beside its entry (D720); the chosen entry, or none, goes back to the window as a
//! `SHELL_MENU` event.

use super::paint::{self, Canvas};
use super::popups::with_shadow;
use super::protocol::{
    EventRecord, EventSink, SHELL_MENU_ENTRIES, SHELL_MENU_LABEL_CAPACITY, WindowError, event,
    shell_menu_flags,
};
use super::shell::PanelItem;
use super::{Action, Desktop, MENU_GAP, MENU_ITEM_HEIGHT, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;

const MINIMUM_WIDTH: u32 = 160;
const WIDTH_PADDING: u32 = 48;
const TEXT_X: u32 = 24;
const SEPARATOR_HEIGHT: u32 = 8;
const TICK_AT: (u32, u32) = (7, 9);
const DOT_AT: (u32, u32) = (11, 13);
const DOT_SIDE: u32 = 6;
const DOT_INSET: [u32; 3] = [2, 1, 0];
const MOST_LEVELS: usize = 4;
const TOP_LEVEL: u8 = u8::MAX;
const ARROW_FROM_RIGHT: u32 = 14;
const ARROW_TOP: u32 = 9;
const ARROW: [(u32, u32); 7] = [(0, 0), (1, 1), (2, 2), (3, 3), (2, 4), (1, 5), (0, 6)];
const TICK: [(u32, u32); 10] = [
    (0, 3),
    (1, 4),
    (2, 5),
    (3, 6),
    (4, 5),
    (5, 4),
    (6, 3),
    (7, 2),
    (8, 1),
    (9, 0),
];

#[derive(Debug, Clone, Copy)]
struct Entry {
    flags: u8,
    label: [u8; SHELL_MENU_LABEL_CAPACITY],
    length: usize,
    parent: u8,
    wire: u8,
}

impl Entry {
    const EMPTY: Self = Entry {
        flags: 0,
        label: [0; SHELL_MENU_LABEL_CAPACITY],
        length: 0,
        parent: TOP_LEVEL,
        wire: 0,
    };

    fn is(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    fn label(&self) -> &str {
        core::str::from_utf8(&self.label[..self.length]).unwrap_or("")
    }

    fn height(&self) -> u32 {
        match self.is(shell_menu_flags::SEPARATOR) {
            true => SEPARATOR_HEIGHT,
            false => MENU_ITEM_HEIGHT,
        }
    }

    fn choosable(&self) -> bool {
        !self.is(shell_menu_flags::SEPARATOR) && !self.is(shell_menu_flags::GRAYED)
    }
}

/// The open shell menu: whose it is, its entries (each with the entry whose submenu holds it),
/// the highlighted one, the submenus open from the top level down, and the tray icon it is
/// anchored to.
#[derive(Debug, Clone, Copy)]
pub(super) struct ShellMenu {
    slot: usize,
    entries: [Entry; SHELL_MENU_ENTRIES],
    count: usize,
    hover: Option<usize>,
    open: [u8; MOST_LEVELS],
    depth: usize,
    icon: Option<u8>,
    anchor: Rect,
}

impl ShellMenu {
    fn level_of(&self, index: usize) -> usize {
        let mut level = 0;
        let mut parent = self.entries[index].parent;
        while parent != TOP_LEVEL && level < MOST_LEVELS {
            level += 1;
            parent = self.entries[usize::from(parent)].parent;
        }
        level
    }

    fn parent_at(&self, level: usize) -> Option<u8> {
        match level {
            0 => Some(TOP_LEVEL),
            _ if level <= self.depth => Some(self.open[level - 1]),
            _ => None,
        }
    }

    fn children(&self, parent: u8) -> impl Iterator<Item = usize> + '_ {
        (0..self.count).filter(move |&index| self.entries[index].parent == parent)
    }

    fn shows(&self, index: usize) -> bool {
        self.parent_at(self.level_of(index)) == Some(self.entries[index].parent)
    }

    fn opens(&self, index: usize) -> bool {
        self.entries[index].is(shell_menu_flags::SUBMENU)
            && self.children(index as u8).next().is_some()
    }

    fn can_choose(&self, index: usize) -> bool {
        let entry = &self.entries[index];
        entry.choosable() && (!entry.is(shell_menu_flags::SUBMENU) || self.opens(index))
    }

    fn close_below(&mut self, level: usize) {
        self.depth = self.depth.min(level);
    }

    fn open_from(&mut self, index: usize) {
        let level = self.level_of(index);
        self.depth = level;
        if self.opens(index) && level + 1 < MOST_LEVELS {
            self.open[level] = index as u8;
            self.depth = level + 1;
        }
    }

    fn first_choosable(&self, parent: u8) -> Option<usize> {
        self.children(parent).find(|&index| self.can_choose(index))
    }
}

fn parse(bytes: &[u8]) -> Option<([Entry; SHELL_MENU_ENTRIES], usize)> {
    let mut entries = [Entry::EMPTY; SHELL_MENU_ENTRIES];
    let mut parents = [TOP_LEVEL; MOST_LEVELS];
    let mut level = 0;
    let mut count = 0;
    let mut wire = 0;
    let mut at = 0;
    while at < bytes.len() {
        let flags = *bytes.get(at)?;
        let length = usize::from(*bytes.get(at + 1)?);
        let label = bytes.get(at + 2..at + 2 + length)?;
        if wire == SHELL_MENU_ENTRIES || length > SHELL_MENU_LABEL_CAPACITY {
            return None;
        }
        core::str::from_utf8(label).ok()?;
        at += 2 + length;
        let entry_wire = wire as u8;
        wire += 1;
        if flags & shell_menu_flags::END != 0 {
            level = level.checked_sub(1)?;
            continue;
        }
        let entry = &mut entries[count];
        entry.flags = flags;
        entry.label[..length].copy_from_slice(label);
        entry.length = length;
        entry.parent = parents[level];
        entry.wire = entry_wire;
        if flags & shell_menu_flags::SUBMENU != 0 {
            if level + 1 == MOST_LEVELS {
                return None;
            }
            level += 1;
            parents[level] = count as u8;
        }
        count += 1;
    }
    (count > 0).then_some((entries, count))
}

fn panel_size(menu: &ShellMenu, parent: u8) -> (u32, u32) {
    let widest = menu
        .children(parent)
        .map(|index| menu.entries[index].label().chars().count() as u32)
        .max()
        .unwrap_or(0);
    let width = MINIMUM_WIDTH.max(widest * GLYPH_WIDTH as u32 + WIDTH_PADDING);
    let height = menu
        .children(parent)
        .map(|index| menu.entries[index].height())
        .sum::<u32>()
        + 2;
    (width, height)
}

impl Desktop {
    /// `SHOW_SHELL_MENU` (D371): client `client`'s window `window` shows `entries` as a shell
    /// menu, anchored to its tray icon last pressed (or under the pointer); an open one is
    /// dismissed first.
    pub fn show_shell_menu(
        &mut self,
        client: u16,
        window: u32,
        entries: &[u8],
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, window)?;
        let (parsed, count) = parse(entries).ok_or(WindowError::Invalid)?;
        self.dismiss_shell_menu(damage, events);
        let icon = self
            .tray_last_pressed()
            .filter(|&(_, owner)| self.window_at(owner).client == client)
            .map(|(at, _)| at);
        let anchor = icon.and_then(|at| self.tray_target(at)).unwrap_or_else(|| {
            let (x, y) = self.pointer;
            Rect::new(x, y, 1, 1)
        });
        self.shell_menu = Some(ShellMenu {
            slot,
            entries: parsed,
            count,
            hover: None,
            open: [TOP_LEVEL; MOST_LEVELS],
            depth: 0,
            icon,
            anchor,
        });
        self.damage_shell_menu(damage);
        Ok(())
    }

    /// Whether a shell menu is open.
    pub fn shell_menu_open(&self) -> bool {
        self.shell_menu.is_some()
    }

    /// The tray icon the open shell menu is anchored to, drawn pressed.
    pub(super) fn shell_menu_icon(&self) -> Option<u8> {
        self.shell_menu.and_then(|menu| menu.icon)
    }

    /// The open shell menu's top level, its border included: under the top bar with its right
    /// edge at the anchor's, or above the taskbar, clipped to the screen.
    pub fn shell_menu_rect(&self) -> Option<Rect> {
        self.shell_menu_panel(0)
    }

    /// The open shell menu's panel at `level` (0 the top level, then each open submenu), its
    /// border included: a submenu right of its parent's panel, or left of it where the screen
    /// ends, its first entry level with the entry that opened it.
    pub fn shell_menu_panel(&self, level: usize) -> Option<Rect> {
        let menu = self.shell_menu.as_ref()?;
        let parent = menu.parent_at(level)?;
        let (width, height) = panel_size(menu, parent);
        let screen = self.layout.screen;
        if level > 0 {
            let outer = self.shell_menu_panel(level - 1)?;
            let opener = self.shell_menu_row(usize::from(parent))?;
            let x = match outer.right() + width <= screen.right() {
                true => outer.right(),
                false => outer.x.saturating_sub(width),
            };
            let y = opener
                .y
                .saturating_sub(1)
                .min(screen.bottom().saturating_sub(height));
            return Some(Rect::new(x, y, width, height.min(screen.height)));
        }
        let x = menu
            .anchor
            .right()
            .saturating_sub(width)
            .min(screen.right().saturating_sub(width));
        let (y, height) = match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => {
                let height = height.min(bar.y.saturating_sub(MENU_GAP));
                (bar.y.saturating_sub(MENU_GAP + height), height)
            }
            (None, Some(bar)) => {
                let top = bar.bottom() + MENU_GAP;
                (top, height.min(screen.bottom().saturating_sub(top)))
            }
            (None, None) => {
                let top = menu
                    .anchor
                    .bottom()
                    .min(screen.bottom().saturating_sub(height));
                (top, height.min(screen.height))
            }
        };
        Some(Rect::new(x, y, width, height))
    }

    /// The rectangle of the open shell menu's entry `index`, while its panel is open.
    pub fn shell_menu_row(&self, index: usize) -> Option<Rect> {
        let menu = self.shell_menu.as_ref()?;
        if index >= menu.count || !menu.shows(index) {
            return None;
        }
        let rect = self.shell_menu_panel(menu.level_of(index))?;
        let parent = menu.entries[index].parent;
        let above: u32 = menu
            .children(parent)
            .take_while(|&sibling| sibling < index)
            .map(|sibling| menu.entries[sibling].height())
            .sum();
        Some(Rect::new(
            rect.x + 1,
            rect.y + 1 + above,
            rect.width - 2,
            menu.entries[index].height(),
        ))
    }

    fn shell_menu_row_at(&self, x: u32, y: u32) -> Option<usize> {
        let menu = self.shell_menu.as_ref()?;
        (0..menu.count)
            .filter(|&index| {
                menu.can_choose(index)
                    && self
                        .shell_menu_row(index)
                        .is_some_and(|rect| contains(rect, x, y))
            })
            .max_by_key(|&index| menu.level_of(index))
    }

    fn on_shell_menu(&self, x: u32, y: u32) -> bool {
        let depth = self.shell_menu.map_or(0, |menu| menu.depth);
        (0..=depth).any(|level| {
            self.shell_menu_panel(level)
                .is_some_and(|rect| contains(rect, x, y))
        })
    }

    fn damage_shell_menu(&self, damage: &mut Damage) {
        let Some(menu) = self.shell_menu else {
            return;
        };
        for level in 0..=menu.depth {
            if let Some(rect) = self.shell_menu_panel(level) {
                damage.add(with_shadow(rect));
            }
        }
        damage.add(menu.anchor);
    }

    fn change_shell_menu(&mut self, damage: &mut Damage, change: impl FnOnce(&mut ShellMenu)) {
        self.damage_shell_menu(damage);
        if let Some(menu) = self.shell_menu.as_mut() {
            change(menu);
        }
        self.damage_shell_menu(damage);
    }

    fn finish_shell_menu(
        &mut self,
        chosen: Option<usize>,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.damage_shell_menu(damage);
        let Some(menu) = self.shell_menu.take() else {
            return;
        };
        let answer = chosen.map_or(0, |index| i32::from(menu.entries[index].wire) + 1);
        let (x, y) = self.pointer;
        let record = EventRecord::new(event::SHELL_MENU, 0, answer, 0, x, y);
        self.send(menu.slot, record, events);
    }

    /// Closes the open shell menu with nothing chosen.
    pub fn dismiss_shell_menu(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        self.finish_shell_menu(None, damage, events);
    }

    /// Forgets the shell menu of the window in `slot`, which is gone.
    pub(super) fn forget_shell_menu(&mut self, slot: usize, damage: &mut Damage) {
        if self.shell_menu.is_some_and(|menu| menu.slot == slot) {
            self.damage_shell_menu(damage);
            self.shell_menu = None;
        }
    }

    /// A button press at (`x`, `y`) while a shell menu is open: a left or right press on an
    /// entry chooses it, or opens its submenu, any other press closes the menu with nothing
    /// chosen; a press on the menu or its icon goes no further.
    pub(super) fn shell_menu_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let menu = self.shell_menu?;
        let row = self.shell_menu_row_at(x, y);
        let on_menu = self.on_shell_menu(x, y);
        let on_anchor = contains(menu.anchor, x, y);
        if let Some(index) = row.filter(|&index| menu.opens(index)) {
            self.change_shell_menu(damage, |menu| {
                menu.hover = Some(index);
                menu.open_from(index);
            });
            return Some(Action::None);
        }
        if on_menu && row.is_none() {
            return Some(Action::None);
        }
        self.finish_shell_menu(row, damage, events);
        (row.is_some() || on_anchor).then_some(Action::None)
    }

    /// The pointer moved to (`x`, `y`): the entry under it is highlighted, and its submenu
    /// opens in place of any deeper one.
    pub(super) fn shell_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(menu) = self.shell_menu else {
            return;
        };
        let hover = self.shell_menu_row_at(x, y);
        let on_menu = self.on_shell_menu(x, y);
        if (hover.is_some() || on_menu) && hover != menu.hover {
            self.change_shell_menu(damage, |menu| {
                menu.hover = hover;
                if let Some(index) = hover {
                    menu.open_from(index);
                }
            });
        }
    }

    fn step_shell_menu(&self, down: bool) -> Option<usize> {
        let menu = self.shell_menu?;
        let parent = menu
            .hover
            .map_or(TOP_LEVEL, |index| menu.entries[index].parent);
        let mut siblings = [0usize; SHELL_MENU_ENTRIES];
        let mut count = 0;
        for index in menu.children(parent).filter(|&index| menu.can_choose(index)) {
            siblings[count] = index;
            count += 1;
        }
        let siblings = &siblings[..count];
        let at = menu
            .hover
            .and_then(|hover| siblings.iter().position(|&index| index == hover));
        let next = match (at, down) {
            (Some(at), true) => at + 1,
            (Some(at), false) => at.checked_sub(1)?,
            (None, true) => 0,
            (None, false) => count.checked_sub(1)?,
        };
        siblings.get(next).copied()
    }

    /// Key `code` went down (`pressed`) or up while a shell menu is open: Up and Down move the
    /// highlight over the entries of its panel that can be chosen, Right, Enter or Space on an
    /// entry with a submenu opens it, Left closes the submenu the highlight is in, Esc closes the
    /// deepest open submenu or, with none, the menu, and Enter or Space chooses.
    pub(super) fn shell_menu_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        if !pressed {
            return Action::None;
        }
        let Some(menu) = self.shell_menu else {
            return Action::None;
        };
        let hover = menu.hover;
        let opener = hover.filter(|&index| menu.opens(index));
        let inner = hover
            .map(|index| menu.entries[index].parent)
            .filter(|&parent| parent != TOP_LEVEL);
        match code {
            key::UP | key::DOWN => {
                if let Some(index) = self.step_shell_menu(code == key::DOWN) {
                    self.change_shell_menu(damage, |menu| {
                        menu.hover = Some(index);
                        menu.close_below(menu.level_of(index));
                    });
                }
            }
            key::RIGHT | key::ENTER | key::KEYPAD_ENTER | key::SPACE if opener.is_some() => {
                if let Some(index) = opener {
                    self.change_shell_menu(damage, |menu| {
                        menu.open_from(index);
                        menu.hover = menu.first_choosable(index as u8).or(Some(index));
                    });
                }
            }
            key::LEFT if inner.is_some() => {
                if let Some(parent) = inner {
                    self.change_shell_menu(damage, |menu| {
                        let index = usize::from(parent);
                        menu.hover = Some(index);
                        menu.close_below(menu.level_of(index));
                    });
                }
            }
            key::ESC if menu.depth > 0 => {
                self.change_shell_menu(damage, |menu| {
                    menu.depth -= 1;
                    menu.hover = Some(usize::from(menu.open[menu.depth]));
                });
            }
            key::ESC => self.dismiss_shell_menu(damage, events),
            key::ENTER | key::KEYPAD_ENTER | key::SPACE => {
                if let Some(index) = hover {
                    self.finish_shell_menu(Some(index), damage, events);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn shell_menu_overlaps(&self, clip: Rect) -> bool {
        let depth = self.shell_menu.map_or(0, |menu| menu.depth);
        (0..=depth).any(|level| {
            self.shell_menu_panel(level)
                .is_some_and(|rect| with_shadow(rect).overlaps(&clip))
        })
    }

    pub(super) fn draw_shell_menu(&self, canvas: &mut Canvas<'_>) {
        let Some(menu) = self.shell_menu.as_ref() else {
            return;
        };
        for level in 0..=menu.depth {
            let rect = self.shell_menu_panel(level);
            if let (Some(rect), Some(parent)) = (rect, menu.parent_at(level)) {
                self.draw_shell_menu_panel(canvas, menu, level, parent, rect);
            }
        }
    }

    fn draw_shell_menu_panel(
        &self,
        canvas: &mut Canvas<'_>,
        menu: &ShellMenu,
        level: usize,
        parent: u8,
        rect: Rect,
    ) {
        if !with_shadow(rect).overlaps(&canvas.clip()) {
            return;
        }
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::current().menu_border);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::current().menu,
        );
        let text_y = (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        for index in menu.children(parent) {
            let entry = &menu.entries[index];
            let Some(row) = self.shell_menu_row(index) else {
                continue;
            };
            if row.y >= rect.bottom() {
                break;
            }
            if entry.is(shell_menu_flags::SEPARATOR) {
                canvas.fill(
                    Rect::new(row.x, row.y + SEPARATOR_HEIGHT / 2, row.width, 1),
                    colour::current().menu_border,
                );
                continue;
            }
            let on_open_path = level < menu.depth && usize::from(menu.open[level]) == index;
            if menu.hover == Some(index) || on_open_path {
                canvas.fill(row, colour::current().menu_hover);
            }
            let text = match menu.can_choose(index) {
                true => colour::current().text,
                false => colour::current().text_dim,
            };
            if entry.is(shell_menu_flags::CHECKED) {
                match entry.is(shell_menu_flags::RADIO) {
                    true => canvas.rounded(
                        Rect::new(rect.x + DOT_AT.0, row.y + DOT_AT.1, DOT_SIDE, DOT_SIDE),
                        &DOT_INSET,
                        true,
                        true,
                        text,
                    ),
                    false => {
                        let (x, y) = (rect.x + TICK_AT.0, row.y + TICK_AT.1);
                        for (dx, dy) in TICK {
                            canvas.fill(Rect::new(x + dx, y + dy, 1, 2), text);
                        }
                    }
                }
            }
            if entry.is(shell_menu_flags::SUBMENU) {
                let (x, y) = (rect.right() - ARROW_FROM_RIGHT, row.y + ARROW_TOP);
                for (dx, dy) in ARROW {
                    canvas.fill(Rect::new(x + dx, y + dy, 2, 1), text);
                }
            }
            canvas.text(rect.x + TEXT_X, row.y + text_y, entry.label(), text);
        }
    }

    /// The shell menu's highlighted entry, if any, numbered as the entries were sent.
    pub fn shell_menu_highlight(&self) -> Option<usize> {
        let menu = self.shell_menu?;
        menu.hover.map(|index| usize::from(menu.entries[index].wire))
    }

    /// Whether `item` is the tray icon the open shell menu is anchored to.
    pub(super) fn anchors_shell_menu(&self, item: PanelItem) -> bool {
        matches!(item, PanelItem::Tray(at) if self.shell_menu_icon() == Some(at))
    }
}
