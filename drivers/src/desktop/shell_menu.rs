//! A Windows program's context menu shown by the server as a shell menu (D371,
//! `docs/ux/windows-tooltips-tray-v0.5.md` section 14): the entries the layer sends when
//! `TrackPopupMenu`'s point lies outside the program's content (a tray icon's menu), drawn in
//! the power menu's chrome and anchored to the tray icon last pressed; the chosen entry, or
//! none, goes back to the window as a `SHELL_MENU` event.

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
}

impl Entry {
    const EMPTY: Self = Entry {
        flags: 0,
        label: [0; SHELL_MENU_LABEL_CAPACITY],
        length: 0,
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

/// The open shell menu: whose it is, its entries, the highlighted one and the tray icon it
/// is anchored to.
#[derive(Debug, Clone, Copy)]
pub(super) struct ShellMenu {
    slot: usize,
    entries: [Entry; SHELL_MENU_ENTRIES],
    count: usize,
    hover: Option<usize>,
    icon: Option<u8>,
    anchor: Rect,
}

fn parse(bytes: &[u8]) -> Option<([Entry; SHELL_MENU_ENTRIES], usize)> {
    let mut entries = [Entry::EMPTY; SHELL_MENU_ENTRIES];
    let mut count = 0;
    let mut at = 0;
    while at < bytes.len() {
        let flags = *bytes.get(at)?;
        let length = usize::from(*bytes.get(at + 1)?);
        let label = bytes.get(at + 2..at + 2 + length)?;
        if count == SHELL_MENU_ENTRIES || length > SHELL_MENU_LABEL_CAPACITY {
            return None;
        }
        core::str::from_utf8(label).ok()?;
        let entry = &mut entries[count];
        entry.flags = flags;
        entry.label[..length].copy_from_slice(label);
        entry.length = length;
        count += 1;
        at += 2 + length;
    }
    (count > 0).then_some((entries, count))
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

    /// The open shell menu, its border included: under the top bar with its right edge at
    /// the anchor's, or above the taskbar, clipped to the screen.
    pub fn shell_menu_rect(&self) -> Option<Rect> {
        let menu = self.shell_menu.as_ref()?;
        let entries = &menu.entries[..menu.count];
        let widest = entries
            .iter()
            .map(|entry| entry.label().chars().count() as u32)
            .max()
            .unwrap_or(0);
        let width = MINIMUM_WIDTH.max(widest * GLYPH_WIDTH as u32 + WIDTH_PADDING);
        let height = entries.iter().map(Entry::height).sum::<u32>() + 2;
        let screen = self.layout.screen;
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

    /// The rectangle of the open shell menu's entry `index`.
    pub fn shell_menu_row(&self, index: usize) -> Option<Rect> {
        let menu = self.shell_menu.as_ref()?;
        if index >= menu.count {
            return None;
        }
        let rect = self.shell_menu_rect()?;
        let above: u32 = menu.entries[..index].iter().map(Entry::height).sum();
        Some(Rect::new(
            rect.x + 1,
            rect.y + 1 + above,
            rect.width - 2,
            menu.entries[index].height(),
        ))
    }

    fn shell_menu_row_at(&self, x: u32, y: u32) -> Option<usize> {
        let menu = self.shell_menu.as_ref()?;
        (0..menu.count).find(|&index| {
            menu.entries[index].choosable()
                && self
                    .shell_menu_row(index)
                    .is_some_and(|rect| contains(rect, x, y))
        })
    }

    fn damage_shell_menu(&self, damage: &mut Damage) {
        if let Some(rect) = self.shell_menu_rect() {
            damage.add(with_shadow(rect));
        }
        if let Some(anchor) = self.shell_menu.map(|menu| menu.anchor) {
            damage.add(anchor);
        }
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
        let answer = chosen.map_or(0, |index| index as i32 + 1);
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
    /// entry chooses it, any other press closes the menu with nothing chosen; a press on the
    /// menu or its icon goes no further.
    pub(super) fn shell_menu_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let menu = self.shell_menu?;
        let row = self.shell_menu_row_at(x, y);
        let on_menu = self
            .shell_menu_rect()
            .is_some_and(|rect| contains(rect, x, y));
        let on_anchor = contains(menu.anchor, x, y);
        if on_menu && row.is_none() {
            return Some(Action::None);
        }
        self.finish_shell_menu(row, damage, events);
        (row.is_some() || on_anchor).then_some(Action::None)
    }

    /// The pointer moved to (`x`, `y`): the entry under it is highlighted.
    pub(super) fn shell_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(menu) = self.shell_menu else {
            return;
        };
        let hover = self.shell_menu_row_at(x, y);
        let on_menu = self
            .shell_menu_rect()
            .is_some_and(|rect| contains(rect, x, y));
        if (hover.is_some() || on_menu) && hover != menu.hover {
            if let Some(open) = self.shell_menu.as_mut() {
                open.hover = hover;
            }
            self.damage_shell_menu(damage);
        }
    }

    fn step_shell_menu(&mut self, down: bool) -> Option<usize> {
        let menu = self.shell_menu?;
        let count = menu.count;
        let mut index = match (menu.hover, down) {
            (Some(index), true) => index + 1,
            (Some(index), false) => index.checked_sub(1)?,
            (None, true) => 0,
            (None, false) => count.checked_sub(1)?,
        };
        while index < count {
            if menu.entries[index].choosable() {
                return Some(index);
            }
            index = match down {
                true => index + 1,
                false => index.checked_sub(1)?,
            };
        }
        None
    }

    /// Key `code` went down (`pressed`) or up while a shell menu is open: Up and Down move the
    /// highlight over the entries that can be chosen, Enter chooses, Esc closes.
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
        match code {
            key::ESC => self.dismiss_shell_menu(damage, events),
            key::UP | key::DOWN => {
                if let Some(index) = self.step_shell_menu(code == key::DOWN) {
                    if let Some(menu) = self.shell_menu.as_mut() {
                        menu.hover = Some(index);
                    }
                    self.damage_shell_menu(damage);
                }
            }
            key::ENTER | key::KEYPAD_ENTER | key::SPACE => {
                if let Some(index) = self.shell_menu.and_then(|menu| menu.hover) {
                    self.finish_shell_menu(Some(index), damage, events);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn shell_menu_overlaps(&self, clip: Rect) -> bool {
        self.shell_menu_rect()
            .is_some_and(|rect| with_shadow(rect).overlaps(&clip))
    }

    pub(super) fn draw_shell_menu(&self, canvas: &mut Canvas<'_>) {
        let (Some(menu), Some(rect)) = (self.shell_menu.as_ref(), self.shell_menu_rect()) else {
            return;
        };
        if !with_shadow(rect).overlaps(&canvas.clip()) {
            return;
        }
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::MENU,
        );
        let text_y = (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        for (index, entry) in menu.entries[..menu.count].iter().enumerate() {
            let Some(row) = self.shell_menu_row(index) else {
                continue;
            };
            if row.y >= rect.bottom() {
                break;
            }
            if entry.is(shell_menu_flags::SEPARATOR) {
                canvas.fill(
                    Rect::new(row.x, row.y + SEPARATOR_HEIGHT / 2, row.width, 1),
                    colour::MENU_BORDER,
                );
                continue;
            }
            if menu.hover == Some(index) {
                canvas.fill(row, colour::MENU_HOVER);
            }
            let text = match entry.is(shell_menu_flags::GRAYED) {
                true => colour::TEXT_DIM,
                false => colour::TEXT,
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
            canvas.text(rect.x + TEXT_X, row.y + text_y, entry.label(), text);
        }
    }

    /// The shell menu's highlighted entry, if any.
    pub fn shell_menu_highlight(&self) -> Option<usize> {
        self.shell_menu.and_then(|menu| menu.hover)
    }

    /// Whether `item` is the tray icon the open shell menu is anchored to.
    pub(super) fn anchors_shell_menu(&self, item: PanelItem) -> bool {
        matches!(item, PanelItem::Tray(at) if self.shell_menu_icon() == Some(at))
    }
}
