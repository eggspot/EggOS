//! The window server's popups (docs/ux/desktop-v0.3-gui.md, sections 5.1 and 5.2): the
//! switcher Alt+Tab opens, listing the windows most recently focused first, and the window menu
//! Alt+Space or a right-click on a title bar opens, with its keyboard move and size. While one
//! is open it takes the keys; the focused window gets `FocusOut` when it opens and `FocusIn`
//! when it closes.

use super::icons::{IconSize, draw_icon};
use super::paint::{self, Canvas};
use super::protocol::{EventRecord, EventSink, MAX_WINDOWS, PointerShape, Role, event};
use super::shortcuts::Held;
use super::windows::{ControlSide, Geometry, cut_title, ellipsis};
use super::{
    Desktop, MENU_ITEM_HEIGHT, MENU_TEXT_X, MENU_WIDTH, SHADOW_RINGS, TITLE_HEIGHT, colour,
    contains,
};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use crate::input::keymap::Modifiers;

/// The switcher's width.
pub const SWITCHER_WIDTH: u32 = 360;
/// One switcher row's height.
pub const SWITCHER_ROW_HEIGHT: u32 = 32;
/// The most rows the switcher shows; more windows scroll.
pub const SWITCHER_ROWS: usize = 12;
const SWITCHER_PADDING: u32 = 8;
const SWITCHER_TEXT_X: u32 = 48;
const SWITCHER_ICON_X: u32 = 8;
const SWITCHER_GLYPHS: usize = 40;
const FOCUS_RING_INSET: u32 = 2;
const FOCUS_RING_WIDTH: u32 = 2;
const FOCUS_RING_INNER: [u32; 1] = [1];
const WINDOW_MENU_GAP: u32 = 4;
const WINDOW_MENU_SIDE: u32 = 8;

/// The window menu's items, top to bottom.
pub const WINDOW_MENU_ITEMS: [&str; 6] =
    ["Restore", "Move", "Size", "Minimise", "Maximise", "Close"];
const RESTORE: usize = 0;
const MOVE: usize = 1;
const SIZE: usize = 2;
const MINIMISE: usize = 3;
const MAXIMISE: usize = 4;
const CLOSE: usize = 5;

/// The open switcher: the windows it lists, in order, which is selected, and the modifier
/// whose release chooses it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Switcher {
    pub entries: [u8; MAX_WINDOWS],
    pub length: usize,
    pub selected: usize,
    pub held: Held,
}

impl Switcher {
    /// Where it is, its border included: centred in `work`.
    pub fn rect(&self, work: Rect) -> Rect {
        let rows = self.length.min(SWITCHER_ROWS) as u32;
        let height = rows * SWITCHER_ROW_HEIGHT + 2 * SWITCHER_PADDING + 2;
        let width = SWITCHER_WIDTH.min(work.width);
        Rect::new(
            work.x + (work.width - width) / 2,
            work.y + work.height.saturating_sub(height) / 2,
            width,
            height.min(work.height),
        )
    }

    fn first_shown(&self) -> usize {
        (self.selected + 1).saturating_sub(SWITCHER_ROWS)
    }

    fn row(&self, rect: Rect, index: usize) -> Rect {
        let shown = (index - self.first_shown()) as u32;
        Rect::new(
            rect.x + 1,
            rect.y + 1 + SWITCHER_PADDING + shown * SWITCHER_ROW_HEIGHT,
            rect.width - 2,
            SWITCHER_ROW_HEIGHT,
        )
    }

    fn shown(&self) -> core::ops::Range<usize> {
        let first = self.first_shown();
        first..self.length.min(first + SWITCHER_ROWS)
    }

    fn step(&mut self, back: bool) {
        self.selected = match back {
            true => (self.selected + self.length - 1) % self.length,
            false => (self.selected + 1) % self.length,
        };
    }
}

/// What the window menu is doing: showing its items, or moving or sizing its window by keys
/// (with the geometry Esc puts back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuMode {
    Items,
    Move(Geometry),
    Size(Geometry),
}

/// The open window menu: whose it is, where, which item the pointer or the keys are on, which
/// items apply, and what it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WindowMenu {
    pub window: usize,
    pub origin: (u32, u32),
    pub hover: Option<usize>,
    pub enabled: [bool; WINDOW_MENU_ITEMS.len()],
    pub mode: MenuMode,
}

impl WindowMenu {
    /// Where it is, its border included, kept on `screen`.
    pub fn rect(&self, screen: Rect) -> Rect {
        let height = WINDOW_MENU_ITEMS.len() as u32 * MENU_ITEM_HEIGHT + 2;
        Rect::new(
            self.origin.0.min(screen.width.saturating_sub(MENU_WIDTH)),
            self.origin.1.min(screen.height.saturating_sub(height)),
            MENU_WIDTH,
            height,
        )
    }

    /// The pointer it shows: the diagonal resize shape while it sizes its window.
    pub fn pointer(&self) -> PointerShape {
        match self.mode {
            MenuMode::Size(_) => PointerShape::ResizeNorthWestSouthEast,
            MenuMode::Items | MenuMode::Move(_) => PointerShape::Arrow,
        }
    }

    fn item_at(&self, rect: Rect, x: u32, y: u32) -> Option<usize> {
        if !contains(rect, x, y) {
            return None;
        }
        let item = (y.saturating_sub(rect.y + 1) / MENU_ITEM_HEIGHT) as usize;
        (item < WINDOW_MENU_ITEMS.len()).then_some(item)
    }

    fn next_enabled(&self, back: bool) -> Option<usize> {
        let count = WINDOW_MENU_ITEMS.len();
        let start = match (self.hover, back) {
            (Some(item), false) => item + 1,
            (Some(item), true) => item + count - 1,
            (None, false) => 0,
            (None, true) => count - 1,
        };
        (0..count)
            .map(|step| match back {
                false => (start + step) % count,
                true => (start + count - step % count) % count,
            })
            .find(|&item| self.enabled[item])
    }
}

pub(super) fn with_shadow(rect: Rect) -> Rect {
    Rect::new(
        rect.x.saturating_sub(SHADOW_RINGS),
        rect.y,
        rect.width + 2 * SHADOW_RINGS,
        rect.height + SHADOW_RINGS,
    )
}

impl Desktop {
    /// Whether the switcher is open.
    pub fn switcher_open(&self) -> bool {
        self.switcher.is_some()
    }

    /// The switcher's rectangle, its border included, while it is open.
    pub fn switcher_area(&self) -> Option<Rect> {
        self.switcher
            .as_ref()
            .map(|switcher| self.switcher_rect(switcher))
    }

    /// The windows the open switcher lists, in order, and the one selected.
    pub fn switcher_entries(&self) -> Option<(alloc::vec::Vec<super::WindowId>, usize)> {
        let switcher = self.switcher.as_ref()?;
        let ids = switcher.entries[..switcher.length]
            .iter()
            .map(|&slot| self.window_at(usize::from(slot)).id)
            .collect();
        Some((ids, switcher.selected))
    }

    /// Whether the window menu is open, showing its items.
    pub fn window_menu_open(&self) -> bool {
        self.window_menu
            .is_some_and(|menu| menu.mode == MenuMode::Items)
    }

    /// The window menu's rectangle, its border included, while it shows its items.
    pub fn window_menu_area(&self) -> Option<Rect> {
        self.window_menu
            .filter(|menu| menu.mode == MenuMode::Items)
            .map(|menu| self.window_menu_rect(&menu))
    }

    pub(super) fn popup_open(&self) -> bool {
        self.switcher.is_some()
            || self.window_menu.is_some()
            || self.launcher.is_some()
            || self.icons.menu.is_some()
    }

    pub(super) fn begin_popup(&mut self, events: &mut dyn EventSink) {
        if let Some(focus) = self.focus.filter(|&slot| self.is_visible(slot)) {
            self.send(
                focus,
                EventRecord::new(event::FOCUS_OUT, 0, 0, 0, 0, 0),
                events,
            );
        }
    }

    pub(super) fn end_popup(
        &mut self,
        target: Option<usize>,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let old = self.focus.take();
        if let Some(old) = old.filter(|&slot| self.is_visible(slot)) {
            damage.add(self.window_at(old).outside());
        }
        match target {
            Some(slot) => self.activate(slot, damage, events),
            None => {
                let back = old.filter(|&slot| self.is_visible(slot));
                self.set_focus(back, damage, events);
            }
        }
    }

    /// Closes every popup: the power menu, the switcher (choosing nothing) and the window menu
    /// (leaving its window where it is).
    pub(super) fn close_popups(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.menu_open {
            self.close_menu(damage);
        }
        if let Some(switcher) = self.switcher.take() {
            damage.add(with_shadow(self.switcher_rect(&switcher)));
            self.end_popup(None, damage, events);
        }
        if let Some(menu) = self.window_menu.take() {
            damage.add(with_shadow(self.window_menu_rect(&menu)));
            self.end_popup(None, damage, events);
        }
        self.close_launcher(damage, events);
        self.close_icon_menu(damage, events);
    }

    /// Window `slot` is going away: the switcher stops listing it and its window menu closes.
    pub(super) fn forget_in_popups(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        if let Some(mut switcher) = self.switcher {
            damage.add(with_shadow(self.switcher_rect(&switcher)));
            let length = switcher.length;
            if let Some(at) = switcher.entries[..length]
                .iter()
                .position(|&entry| usize::from(entry) == slot)
            {
                switcher.entries[at..length].rotate_left(1);
                switcher.length -= 1;
                if switcher.selected >= switcher.length {
                    switcher.selected = switcher.length.saturating_sub(1);
                }
            }
            self.switcher = Some(switcher);
            if switcher.length == 0 {
                self.close_switcher(false, damage, events);
            }
        }
        if self.window_menu.is_some_and(|menu| menu.window == slot) {
            self.close_window_menu(damage, events);
        }
    }

    pub(super) fn open_switcher(
        &mut self,
        back: bool,
        held: Held,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.close_popups(damage, events);
        let mut switcher = Switcher {
            entries: [0; MAX_WINDOWS],
            length: 0,
            selected: 0,
            held,
        };
        switcher.length = self.switcher_order(&mut switcher.entries);
        if switcher.length == 0 {
            return;
        }
        if switcher.length > 1 {
            switcher.step(back);
        }
        damage.add(with_shadow(self.switcher_rect(&switcher)));
        self.switcher = Some(switcher);
        self.begin_popup(events);
    }

    fn close_switcher(&mut self, choose: bool, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        damage.add(with_shadow(self.switcher_rect(&switcher)));
        let chosen = choose.then(|| usize::from(switcher.entries[switcher.selected]));
        self.end_popup(chosen, damage, events);
    }

    pub(super) fn switcher_key(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut switcher) = self.switcher else {
            return;
        };
        match (code, pressed) {
            (key::TAB, true) => {
                switcher.step(modifiers.contains(Modifiers::SHIFT));
                damage.add(self.switcher_rect(&switcher));
                self.switcher = Some(switcher);
            }
            (key::ESC, true) => self.close_switcher(false, damage, events),
            (code, false) if switcher.held.is(code) => self.close_switcher(true, damage, events),
            _ => {}
        }
    }

    /// A button press at (`x`, `y`) while the switcher is open: a row chooses its window,
    /// anywhere else closes it. Returns whether the press was the switcher's.
    pub(super) fn switcher_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> bool {
        let Some(mut switcher) = self.switcher else {
            return false;
        };
        let rect = self.switcher_rect(&switcher);
        let row = switcher
            .shown()
            .find(|&index| contains(switcher.row(rect, index), x, y));
        match row {
            Some(index) => {
                switcher.selected = index;
                self.switcher = Some(switcher);
                self.close_switcher(true, damage, events);
                true
            }
            None => {
                self.close_switcher(false, damage, events);
                contains(rect, x, y)
            }
        }
    }

    /// Opens window `slot`'s window menu: at (`x`, `y`) for a right-click, else under its
    /// title bar on the controls' side, with the first item that applies selected.
    pub(super) fn open_window_menu(
        &mut self,
        slot: usize,
        at: Option<(u32, u32)>,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.close_popups(damage, events);
        let window = *self.window_at(slot);
        let normal = window.role == Role::Normal;
        let mut enabled = [false; WINDOW_MENU_ITEMS.len()];
        enabled[RESTORE] = window.maximised;
        enabled[MOVE] = !window.maximised;
        enabled[SIZE] = window.is_resizable();
        enabled[MINIMISE] = normal;
        enabled[MAXIMISE] = window.can_maximise() && !window.maximised;
        enabled[CLOSE] = window.closable;
        let outside = window.outside();
        let top = outside.y + TITLE_HEIGHT + 1 + WINDOW_MENU_GAP;
        let origin = at.unwrap_or(match self.profile.control_side() {
            ControlSide::Left => (outside.x + WINDOW_MENU_SIDE, top),
            ControlSide::Right => (
                outside
                    .right()
                    .saturating_sub(WINDOW_MENU_SIDE + MENU_WIDTH),
                top,
            ),
        });
        let mut menu = WindowMenu {
            window: slot,
            origin,
            hover: None,
            enabled,
            mode: MenuMode::Items,
        };
        if at.is_none() {
            menu.hover = menu.next_enabled(false);
        }
        damage.add(with_shadow(self.window_menu_rect(&menu)));
        self.window_menu = Some(menu);
        self.begin_popup(events);
    }

    fn close_window_menu(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if let Some(menu) = self.window_menu.take() {
            if menu.mode == MenuMode::Items {
                damage.add(with_shadow(self.window_menu_rect(&menu)));
            }
            self.end_popup(None, damage, events);
        }
    }

    fn choose_window_menu_item(
        &mut self,
        item: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut menu) = self.window_menu else {
            return;
        };
        if !menu.enabled[item] {
            return;
        }
        let slot = menu.window;
        let geometry = self.window_at(slot).geometry();
        match item {
            MOVE | SIZE => {
                damage.add(with_shadow(self.window_menu_rect(&menu)));
                menu.mode = match item {
                    MOVE => MenuMode::Move(geometry),
                    _ => MenuMode::Size(geometry),
                };
                self.window_menu = Some(menu);
                return;
            }
            _ => self.close_window_menu(damage, events),
        }
        match item {
            RESTORE => self.unmaximise(slot, damage, events),
            MINIMISE => self.minimise(slot, damage, events),
            MAXIMISE => self.maximise(slot, damage, events),
            _ => self.request_close(slot, events),
        }
    }

    pub(super) fn window_menu_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut menu) = self.window_menu else {
            return;
        };
        if !pressed {
            return;
        }
        match (menu.mode, code) {
            (MenuMode::Items, key::UP | key::DOWN) => {
                menu.hover = menu.next_enabled(code == key::UP);
                damage.add(self.window_menu_rect(&menu));
                self.window_menu = Some(menu);
            }
            (MenuMode::Items, key::ENTER | key::KEYPAD_ENTER) => {
                if let Some(item) = menu.hover {
                    self.choose_window_menu_item(item, damage, events);
                }
            }
            (MenuMode::Move(_), key::LEFT | key::RIGHT | key::UP | key::DOWN) => {
                self.keyboard_step(menu.window, false, code, damage, events)
            }
            (MenuMode::Size(_), key::LEFT | key::RIGHT | key::UP | key::DOWN) => {
                self.keyboard_step(menu.window, true, code, damage, events)
            }
            (MenuMode::Move(before) | MenuMode::Size(before), key::ESC) => {
                self.put_back(menu.window, before, damage, events);
                self.close_window_menu(damage, events);
            }
            (_, key::ESC)
            | (MenuMode::Move(_) | MenuMode::Size(_), key::ENTER | key::KEYPAD_ENTER) => {
                self.close_window_menu(damage, events)
            }
            _ => {}
        }
    }

    /// A button press at (`x`, `y`) while the window menu is open: an item chooses it;
    /// anywhere else closes the menu (ending a keyboard move or size where it is). Returns
    /// whether the press was the menu's.
    pub(super) fn window_menu_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> bool {
        let Some(menu) = self.window_menu else {
            return false;
        };
        let rect = self.window_menu_rect(&menu);
        if menu.mode == MenuMode::Items
            && let Some(item) = menu.item_at(rect, x, y)
        {
            self.choose_window_menu_item(item, damage, events);
            return true;
        }
        self.close_window_menu(damage, events);
        menu.mode == MenuMode::Items && contains(rect, x, y)
    }

    pub(super) fn window_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(mut menu) = self.window_menu.filter(|menu| menu.mode == MenuMode::Items) else {
            return;
        };
        let rect = self.window_menu_rect(&menu);
        let Some(item) = menu.item_at(rect, x, y) else {
            return;
        };
        if menu.hover != Some(item) {
            menu.hover = Some(item);
            damage.add(rect);
            self.window_menu = Some(menu);
        }
    }

    pub(super) fn draw_popups(&self, canvas: &mut Canvas<'_>) {
        if let Some(switcher) = &self.switcher {
            self.draw_switcher(canvas, switcher);
        }
        if let Some(menu) = self.window_menu.filter(|menu| menu.mode == MenuMode::Items) {
            self.draw_window_menu(canvas, &menu);
        }
        self.draw_launcher(canvas);
        self.draw_icon_menu(canvas);
        self.draw_notice(canvas);
    }

    pub(super) fn popups_overlap(&self, clip: Rect) -> bool {
        self.switcher
            .as_ref()
            .is_some_and(|switcher| with_shadow(self.switcher_rect(switcher)).overlaps(&clip))
            || self
                .window_menu
                .filter(|menu| menu.mode == MenuMode::Items)
                .is_some_and(|menu| with_shadow(self.window_menu_rect(&menu)).overlaps(&clip))
            || self.launcher_overlaps(clip)
            || self.icon_menu_overlaps(clip)
            || self.notice_overlaps(clip)
    }

    pub(super) fn draw_popup_surface(canvas: &mut Canvas<'_>, rect: Rect, surface: u32) {
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            surface,
        );
    }

    fn draw_switcher(&self, canvas: &mut Canvas<'_>, switcher: &Switcher) {
        let rect = self.switcher_rect(switcher);
        Self::draw_popup_surface(canvas, rect, colour::DOCK);
        for index in switcher.shown() {
            let row = switcher.row(rect, index);
            if index == switcher.selected {
                canvas.fill(row, colour::BAR_PRESSED);
                let ring = Rect::new(
                    row.x + FOCUS_RING_INSET,
                    row.y + FOCUS_RING_INSET,
                    row.width - 2 * FOCUS_RING_INSET,
                    row.height - 2 * FOCUS_RING_INSET,
                );
                canvas.rounded(ring, &paint::INSET_R4, true, true, colour::FOCUS_RING);
                canvas.rounded(
                    Rect::new(
                        ring.x + FOCUS_RING_WIDTH,
                        ring.y + FOCUS_RING_WIDTH,
                        ring.width - 2 * FOCUS_RING_WIDTH,
                        ring.height - 2 * FOCUS_RING_WIDTH,
                    ),
                    &FOCUS_RING_INNER,
                    true,
                    true,
                    colour::BAR_PRESSED,
                );
            }
            let slot = usize::from(switcher.entries[index]);
            draw_icon(
                canvas,
                self.window_icon(slot),
                IconSize::Small,
                row.x - 1 + SWITCHER_ICON_X,
                row.y,
            );
            let window = self.window_at(slot);
            let text_colour = match window.minimised {
                true => colour::TEXT_DIM,
                false => colour::TEXT,
            };
            let (text, cut) = cut_title(window.title(), SWITCHER_GLYPHS);
            let x = row.x - 1 + SWITCHER_TEXT_X;
            let y = row.y + (SWITCHER_ROW_HEIGHT - GLYPH_HEIGHT as u32) / 2;
            canvas.text(x, y, text, text_colour);
            if cut {
                let after = x + text.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(after, y, ellipsis(), text_colour);
            }
        }
    }

    fn draw_window_menu(&self, canvas: &mut Canvas<'_>, menu: &WindowMenu) {
        let rect = self.window_menu_rect(menu);
        Self::draw_popup_surface(canvas, rect, colour::MENU);
        for (index, label) in WINDOW_MENU_ITEMS.iter().enumerate() {
            let top = rect.y + 1 + index as u32 * MENU_ITEM_HEIGHT;
            if menu.hover == Some(index) {
                canvas.fill(
                    Rect::new(rect.x + 1, top + 2, rect.width - 2, MENU_ITEM_HEIGHT - 4),
                    colour::MENU_HOVER,
                );
            }
            let text_colour = match menu.enabled[index] {
                true => colour::TEXT,
                false => colour::TEXT_DIM,
            };
            canvas.text(
                rect.x + MENU_TEXT_X,
                top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                label,
                text_colour,
            );
        }
    }
}
