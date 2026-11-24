//! Windows programs' notification-area icons in the bar (D357,
//! `docs/ux/windows-tooltips-tray-v0.5.md` sections 11 to 13): the icons a client sets for its
//! windows, their run directly left of the keyboard and network indicators (28 px targets on
//! the top bar, 40 px items on the taskbar, the first added rightmost, at most four shown and
//! an overflow button past that), their drawing, their names, and the pointer's presses on
//! them, reported to the icon's window as the Windows message each stands for.

use super::paint::{self, Canvas};
use super::popups::with_shadow;
use super::protocol::{
    EventRecord, EventSink, TRAY_ICON_SIDE, TRAY_PLACE, TRAY_TIP_CAPACITY, WindowError, event,
};
use super::shell::{PanelItem, TASKBAR_ITEM, TOOLTIP_DELAY_MS};
use super::windows::{cut_title, ellipsis};
use super::{Action, Desktop, MENU_GAP, MENU_ITEM_HEIGHT, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::{button, key};

/// The most notification-area icons the bar holds.
pub const MOST_TRAY_ICONS: usize = 16;
const SHOWN: usize = 4;
const PIXELS: usize = TRAY_ICON_SIDE * TRAY_ICON_SIDE;
const TOP_BAR_ICON_AT: u32 = 6;
const TASKBAR_ICON_AT: u32 = 12;
const TASKBAR_RUN_GAP: u32 = 8;
const OPAQUE: u32 = 0x8000_0000;
const OVERFLOW_TOOLTIP: &str = "Hidden icons";
const WM_LBUTTONDOWN: i32 = 0x0201;
const WM_LBUTTONUP: i32 = 0x0202;
const WM_RBUTTONDOWN: i32 = 0x0204;
const WM_RBUTTONUP: i32 = 0x0205;
const WM_MBUTTONDOWN: i32 = 0x0207;
const WM_MBUTTONUP: i32 = 0x0208;
const WM_MOUSEMOVE: i32 = 0x0200;
const WM_CONTEXTMENU: i32 = 0x007b;
const NIN_KEYSELECT: i32 = 0x0401;
const NIN_BALLOONSHOW: i32 = 0x0402;
const NIN_BALLOONHIDE: i32 = 0x0403;
const NIN_BALLOONTIMEOUT: i32 = 0x0404;
const NIN_BALLOONUSERCLICK: i32 = 0x0405;
const NIN_POPUPOPEN: i32 = 0x0406;
const NIN_POPUPCLOSE: i32 = 0x0407;
const PLACE_HALF_BITS: u32 = 16;
const BALLOON_GLYPHS: usize = 36;
/// The overflow popup's width (docs/ux/windows-tooltips-tray-v0.5.md section 11).
pub const TRAY_OVERFLOW_WIDTH: u32 = 256;
const OVERFLOW_ICON_AT: u32 = 8;
const OVERFLOW_TEXT_X: u32 = 32;
const OVERFLOW_GLYPHS: usize = 26;
const BALLOON_BYTES: usize = BALLOON_GLYPHS * 4 + 3;

#[derive(Debug, Clone, Copy)]
struct TrayIcon {
    client: u16,
    slot: usize,
    key: u32,
    order: u32,
    hidden: bool,
    rich_popup: bool,
    reported: Option<(u32, u32, u32)>,
    tip: [u8; TRAY_TIP_CAPACITY],
    tip_length: usize,
    pixels: Option<[u32; PIXELS]>,
}

/// The bar's notification-area icons.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tray {
    icons: [Option<TrayIcon>; MOST_TRAY_ICONS],
    next_order: u32,
    overflow_open: bool,
    overflow_hover: Option<usize>,
    last_pressed: Option<u8>,
    balloon: [u8; BALLOON_BYTES],
    balloon_length: usize,
    balloon_owner: Option<(usize, u32)>,
    popup: Option<u8>,
}

impl Default for Tray {
    fn default() -> Self {
        Tray {
            icons: [None; MOST_TRAY_ICONS],
            next_order: 0,
            overflow_open: false,
            overflow_hover: None,
            last_pressed: None,
            balloon: [0; BALLOON_BYTES],
            balloon_length: 0,
            balloon_owner: None,
            popup: None,
        }
    }
}

/// Which mouse button a press on an icon was, or which key chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrayButton {
    Left,
    Right,
    Middle,
    /// Enter or Space on the icon (`NIN_KEYSELECT`).
    KeySelect,
    /// Shift+F10 or the Menu key on the icon (`WM_CONTEXTMENU`).
    KeyMenu,
}

impl Desktop {
    /// `SET_TRAY_ICON` (D357): client `client`'s icon `key` of its window `window` added or
    /// changed with `tip` (UTF-8) and its pixels (none: the generic tile), hidden or not, with
    /// the program's own popup in place of the bar's tooltip (`rich_popup`, D719) or not, or
    /// removed with `remove`; the bar damaged.
    #[allow(clippy::too_many_arguments)]
    pub fn set_tray_icon(
        &mut self,
        client: u16,
        window: u32,
        key: u32,
        (hidden, remove, rich_popup): (bool, bool, bool),
        tip: &[u8],
        pixels: Option<&[u32; PIXELS]>,
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, window)?;
        if tip.len() > TRAY_TIP_CAPACITY || core::str::from_utf8(tip).is_err() {
            return Err(WindowError::Invalid);
        }
        self.damage_tray(damage);
        let found = self
            .tray
            .icons
            .iter()
            .position(|icon| icon.is_some_and(|icon| icon.client == client && icon.key == key));
        if remove {
            if let Some(at) = found {
                self.close_tray_overflow(damage);
                self.tray.icons[at] = None;
                if self.tray.popup == Some(at as u8) {
                    self.tray.popup = None;
                }
            }
            self.damage_tray(damage);
            return Ok(());
        }
        let at = match found {
            Some(at) => at,
            None => self
                .tray
                .icons
                .iter()
                .position(Option::is_none)
                .ok_or(WindowError::TableFull)?,
        };
        let (order, reported) = match self.tray.icons[at] {
            Some(icon) => (icon.order, icon.reported),
            None => {
                self.tray.next_order += 1;
                (self.tray.next_order, None)
            }
        };
        let mut stored = [0u8; TRAY_TIP_CAPACITY];
        stored[..tip.len()].copy_from_slice(tip);
        self.tray.icons[at] = Some(TrayIcon {
            client,
            slot,
            key,
            order,
            hidden,
            rich_popup,
            reported,
            tip: stored,
            tip_length: tip.len(),
            pixels: pixels.copied(),
        });
        self.damage_tray(damage);
        Ok(())
    }

    /// Forgets the icons of the window in `slot`, which is gone.
    pub(super) fn forget_tray_icons(&mut self, slot: usize, damage: &mut Damage) {
        if self
            .tray
            .icons
            .iter()
            .flatten()
            .any(|icon| icon.slot == slot)
        {
            self.close_tray_overflow(damage);
            self.damage_tray(damage);
            for icon in self.tray.icons.iter_mut() {
                if icon.is_some_and(|icon| icon.slot == slot) {
                    *icon = None;
                }
            }
        }
        if self.tray.popup.is_some_and(|at| self.tray.icons[usize::from(at)].is_none()) {
            self.tray.popup = None;
        }
        if self.tray.balloon_owner.is_some_and(|(owner, _)| owner == slot) {
            self.tray.balloon_owner = None;
            if self.balloon_showing() {
                self.close_notice(damage);
            }
        }
    }

    fn damage_tray(&self, damage: &mut Damage) {
        if let Some(bar) = self.layout.taskbar.or(self.layout.top_bar) {
            damage.add(bar);
        }
    }

    fn ordered_tray(&self) -> ([usize; MOST_TRAY_ICONS], usize) {
        let mut order = [(u32::MAX, 0usize); MOST_TRAY_ICONS];
        let mut count = 0;
        for (at, icon) in self.tray.icons.iter().enumerate() {
            if let Some(icon) = icon.filter(|icon| !icon.hidden) {
                order[count] = (icon.order, at);
                count += 1;
            }
        }
        order[..count].sort_unstable();
        let mut indices = [0; MOST_TRAY_ICONS];
        for (index, &(_, at)) in indices.iter_mut().zip(&order[..count]) {
            *index = at;
        }
        (indices, count)
    }

    fn shown_tray(&self) -> ([usize; SHOWN], usize, bool) {
        let (ordered, count) = self.ordered_tray();
        let overflow = count > SHOWN;
        let shown = if overflow { SHOWN - 1 } else { count };
        let mut indices = [0; SHOWN];
        indices[..shown].copy_from_slice(&ordered[..shown]);
        (indices, shown, overflow)
    }

    fn overflowed_tray(&self) -> ([usize; MOST_TRAY_ICONS], usize) {
        let (ordered, count) = self.ordered_tray();
        if count <= SHOWN {
            return ([0; MOST_TRAY_ICONS], 0);
        }
        let mut indices = [0; MOST_TRAY_ICONS];
        let rest = count - (SHOWN - 1);
        indices[..rest].copy_from_slice(&ordered[SHOWN - 1..count]);
        (indices, rest)
    }

    fn tray_cell(&self) -> Option<(u32, u32, u32)> {
        let neighbour = self
            .keyboard_layout_indicator()
            .or(self.network_indicator());
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => {
                let right = neighbour.unwrap_or(self.clock_rect()).x;
                Some((right.checked_sub(TASKBAR_RUN_GAP)?, bar.y, TASKBAR_ITEM))
            }
            (None, Some(bar)) => {
                let right = neighbour.unwrap_or(self.power_button()).x;
                Some((right, bar.y, bar.height))
            }
            (None, None) => None,
        }
    }

    /// Calls `item` with each notification-area target: the icons right to left, then the
    /// overflow button.
    pub(super) fn for_each_tray_item(&self, mut item: impl FnMut(PanelItem, Rect)) {
        let Some((mut right, y, side)) = self.tray_cell() else {
            return;
        };
        let (indices, shown, overflow) = self.shown_tray();
        for &at in &indices[..shown] {
            let Some(x) = right.checked_sub(side) else {
                return;
            };
            item(PanelItem::Tray(at as u8), Rect::new(x, y, side, side));
            right = x;
        }
        if overflow && let Some(x) = right.checked_sub(side) {
            item(PanelItem::TrayOverflow, Rect::new(x, y, side, side));
        }
    }

    /// The whole run of notification-area targets, while one shows.
    pub fn tray_run(&self) -> Option<Rect> {
        let mut run: Option<Rect> = None;
        self.for_each_tray_item(|_, rect| {
            run = Some(match run {
                Some(run) => Rect::new(rect.x, run.y, run.x + run.width - rect.x, run.height),
                None => rect,
            });
        });
        run
    }

    /// The name of notification-area target `item`: its tip, which is also its tooltip.
    /// Whether icon `at` shows its program's own popup instead of the bar's tooltip (D719).
    pub(super) fn tray_has_rich_popup(&self, at: u8) -> bool {
        self.tray
            .icons
            .get(usize::from(at))
            .copied()
            .flatten()
            .is_some_and(|icon| icon.rich_popup)
    }

    pub(super) fn tray_name(&self, item: PanelItem) -> Option<&str> {
        match item {
            PanelItem::Tray(at) => {
                let icon = self.tray.icons.get(usize::from(at))?.as_ref()?;
                let tip = core::str::from_utf8(&icon.tip[..icon.tip_length]).ok()?;
                match tip.is_empty() {
                    true => Some(self.window_at(icon.slot).title()),
                    false => Some(tip),
                }
            }
            PanelItem::TrayOverflow => Some(OVERFLOW_TOOLTIP),
            _ => None,
        }
    }

    pub(super) fn draw_tray(&self, canvas: &mut Canvas<'_>) {
        let taskbar = self.layout.taskbar.is_some();
        let inset = if taskbar {
            TASKBAR_ICON_AT
        } else {
            TOP_BAR_ICON_AT
        };
        self.for_each_tray_item(|item, rect| {
            if !rect.overlaps(&canvas.clip()) {
                return;
            }
            let open = (item == PanelItem::TrayOverflow && self.tray.overflow_open)
                || self.anchors_shell_menu(item);
            let fill = match taskbar {
                true => self.item_fill(item, open),
                false => (open || self.panels.pressed == Some(item)).then_some(colour::current().bar_pressed),
            };
            if let Some(fill) = fill {
                canvas.fill(rect, fill);
            }
            let (x, y) = (rect.x + inset, rect.y + inset);
            match item {
                PanelItem::Tray(at) => {
                    let Some(icon) = self.tray.icons[usize::from(at)] else {
                        return;
                    };
                    match icon.pixels {
                        Some(pixels) => draw_pixels(canvas, (x, y), &pixels),
                        None => {
                            let title = self.window_at(icon.slot).title();
                            draw_tile(canvas, (x, y), title);
                        }
                    }
                }
                _ => canvas.text(x + 4, y, "^", colour::current().text),
            }
        });
    }

    /// A press of `button` on notification-area icon `at`: the icon's window gets the button's
    /// down and up messages at once, with the pointer's screen point, or the key's one message;
    /// a rich popup of the icon closes first.
    pub(super) fn tray_press(&mut self, at: u8, button: TrayButton, events: &mut dyn EventSink) {
        if self.tray.icons.get(usize::from(at)).copied().flatten().is_none() {
            return;
        }
        self.tray.last_pressed = Some(at);
        if self.tray.popup == Some(at) {
            self.tray.popup = None;
            self.send_tray(at, NIN_POPUPCLOSE, events);
        }
        let messages: &[i32] = match button {
            TrayButton::Left => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
            TrayButton::Right => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
            TrayButton::Middle => &[WM_MBUTTONDOWN, WM_MBUTTONUP],
            TrayButton::KeySelect => &[NIN_KEYSELECT],
            TrayButton::KeyMenu => &[WM_CONTEXTMENU],
        };
        for &message in messages {
            self.send_tray(at, message, events);
        }
    }

    fn send_tray(&mut self, at: u8, message: i32, events: &mut dyn EventSink) {
        let Some(icon) = self.tray.icons.get(usize::from(at)).copied().flatten() else {
            return;
        };
        let (x, y) = self.pointer;
        let record = EventRecord::new(event::TRAY, 0, message, icon.key as i32, x, y);
        self.send(icon.slot, record, events);
    }

    /// The pointer moved (D719): the icon under it gets `WM_MOUSEMOVE`, and an icon whose rich
    /// popup is open gets `NIN_POPUPCLOSE` once the pointer is off it.
    pub(super) fn tray_pointer_moved(&mut self, events: &mut dyn EventSink) {
        let hover = self.panels.hover;
        if let Some(at) = self.tray.popup
            && hover != Some(PanelItem::Tray(at))
        {
            self.tray.popup = None;
            self.send_tray(at, NIN_POPUPCLOSE, events);
        }
        if let Some(PanelItem::Tray(at)) = hover {
            self.send_tray(at, WM_MOUSEMOVE, events);
        }
    }

    /// The clock moved on (D719): a balloon whose notice timed out (`balloon_timed_out`, found
    /// before the notice closed) tells its window `NIN_BALLOONTIMEOUT`, and one that went
    /// otherwise (another notice in its place, its icon removed) `NIN_BALLOONHIDE`; an icon with
    /// a rich popup the pointer rested on gets `NIN_POPUPOPEN`; and each icon whose place in the
    /// bar changed has it reported.
    pub(super) fn tick_tray(
        &mut self,
        balloon_timed_out: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.tick_tray_balloon(balloon_timed_out, damage, events);
        self.tick_tray_popup(events);
        self.report_tray_places(events);
    }

    fn tick_tray_balloon(
        &mut self,
        balloon_timed_out: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some((slot, key)) = self.tray.balloon_owner else {
            return;
        };
        let icon_there = self
            .tray
            .icons
            .iter()
            .flatten()
            .any(|icon| icon.slot == slot && icon.key == key);
        let showing = self.balloon_showing();
        let message = match (balloon_timed_out, showing && icon_there) {
            (true, _) => NIN_BALLOONTIMEOUT,
            (false, false) => NIN_BALLOONHIDE,
            (false, true) => return,
        };
        if showing {
            self.close_notice(damage);
        }
        self.tray.balloon_owner = None;
        let (x, y) = self.pointer;
        let record = EventRecord::new(event::TRAY, 0, message, key as i32, x, y);
        self.send(slot, record, events);
    }

    fn tick_tray_popup(&mut self, events: &mut dyn EventSink) {
        let Some(PanelItem::Tray(at)) = self.panels.hover else {
            return;
        };
        if self.tray.popup == Some(at) {
            return;
        }
        let rich = self
            .tray
            .icons
            .get(usize::from(at))
            .copied()
            .flatten()
            .is_some_and(|icon| icon.rich_popup);
        let rested = self.now_ms.wrapping_sub(self.panels.hover_since_ms) >= TOOLTIP_DELAY_MS;
        let blocked =
            self.panels.tooltip_spent || self.tray_overflow_open() || self.shell_menu_open();
        if !rich || !rested || blocked {
            return;
        }
        self.tray.popup = Some(at);
        self.send_tray(at, NIN_POPUPOPEN, events);
    }

    fn report_tray_places(&mut self, events: &mut dyn EventSink) {
        let mut places = [None; MOST_TRAY_ICONS];
        let mut overflow = None;
        self.for_each_tray_item(|item, rect| match item {
            PanelItem::Tray(at) => places[usize::from(at)] = Some(rect),
            PanelItem::TrayOverflow => overflow = Some(rect),
            _ => {}
        });
        for (at, place) in places.iter().enumerate() {
            let Some(icon) = self.tray.icons[at] else {
                continue;
            };
            let shown = match icon.hidden {
                true => None,
                false => place.or(overflow),
            };
            let reported = shown.map_or((0, 0, 0), |rect| (rect.x, rect.y, rect.width));
            if icon.reported == Some(reported) {
                continue;
            }
            if let Some(stored) = self.tray.icons[at].as_mut() {
                stored.reported = Some(reported);
            }
            let (x, y, side) = reported;
            let record = EventRecord::new(
                event::TRAY,
                0,
                TRAY_PLACE,
                icon.key as i32,
                x | (side << PLACE_HALF_BITS),
                y | (side << PLACE_HALF_BITS),
            );
            self.send(icon.slot, record, events);
        }
    }

    /// `SET_TRAY_ICON` with `FOCUS` (D719, `NIM_SETFOCUS`): the bar's keyboard focus goes to
    /// client `client`'s icon `key`, or to the overflow button when the icon is in its popup.
    pub fn focus_tray_icon(
        &mut self,
        client: u16,
        window: u32,
        key: u32,
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        self.owned(client, window)?;
        let at = self
            .tray
            .icons
            .iter()
            .position(|icon| icon.is_some_and(|icon| icon.client == client && icon.key == key))
            .ok_or(WindowError::Invalid)?;
        self.focus_panel_item(PanelItem::Tray(at as u8), PanelItem::TrayOverflow, damage);
        Ok(())
    }
}

impl Desktop {
    /// The icon last pressed, while it is still there, and its window's slot.
    pub(super) fn tray_last_pressed(&self) -> Option<(u8, usize)> {
        let at = self.tray.last_pressed?;
        let icon = self.tray.icons.get(usize::from(at)).copied().flatten()?;
        Some((at, icon.slot))
    }

    /// Where icon `at` is in the bar, or the overflow button's place when it is hidden in it.
    pub(super) fn tray_target(&self, at: u8) -> Option<Rect> {
        let mut found = None;
        let mut overflow = None;
        self.for_each_tray_item(|item, rect| match item {
            PanelItem::Tray(shown) if shown == at => found = Some(rect),
            PanelItem::TrayOverflow => overflow = Some(rect),
            _ => {}
        });
        found.or(overflow)
    }

    /// Whether the tray's overflow popup is open.
    pub fn tray_overflow_open(&self) -> bool {
        self.tray.overflow_open
    }

    fn tray_overflow_button(&self) -> Option<Rect> {
        let mut button = None;
        self.for_each_tray_item(|item, rect| {
            if item == PanelItem::TrayOverflow {
                button = Some(rect);
            }
        });
        button
    }

    /// The overflow popup, its border included, while the overflow button shows: under the
    /// top bar with its right edge at the button's, or above the taskbar (section 11).
    pub fn tray_overflow(&self) -> Option<Rect> {
        let button = self.tray_overflow_button()?;
        let (_, rows) = self.overflowed_tray();
        let height = rows as u32 * MENU_ITEM_HEIGHT + 2;
        let y = match self.layout.taskbar {
            Some(bar) => bar.y.saturating_sub(MENU_GAP + height),
            None => button.bottom() + MENU_GAP,
        };
        Some(Rect::new(
            button.right().saturating_sub(TRAY_OVERFLOW_WIDTH),
            y,
            TRAY_OVERFLOW_WIDTH,
            height,
        ))
    }

    /// The rectangle of the overflow popup's row `index`.
    pub fn tray_overflow_row(&self, index: usize) -> Option<Rect> {
        let popup = self.tray_overflow()?;
        let (_, rows) = self.overflowed_tray();
        if index >= rows {
            return None;
        }
        Some(Rect::new(
            popup.x + 1,
            popup.y + 1 + index as u32 * MENU_ITEM_HEIGHT,
            popup.width - 2,
            MENU_ITEM_HEIGHT,
        ))
    }

    fn tray_overflow_row_at(&self, x: u32, y: u32) -> Option<usize> {
        let (_, rows) = self.overflowed_tray();
        (0..rows).find(|&index| {
            self.tray_overflow_row(index)
                .is_some_and(|rect| contains(rect, x, y))
        })
    }

    fn damage_tray_overflow(&self, damage: &mut Damage) {
        if self.tray.overflow_open
            && let Some(popup) = self.tray_overflow()
        {
            damage.add(with_shadow(popup));
        }
        if let Some(button) = self.tray_overflow_button() {
            damage.add(button);
        }
    }

    /// Opens the overflow popup, its row under the pointer highlighted.
    pub(super) fn open_tray_overflow(&mut self, damage: &mut Damage) {
        if self.tray_overflow_button().is_none() {
            return;
        }
        let (x, y) = self.pointer;
        self.tray.overflow_open = true;
        self.tray.overflow_hover = self.tray_overflow_row_at(x, y);
        self.damage_tray_overflow(damage);
    }

    /// Closes the overflow popup if it is open.
    pub fn close_tray_overflow(&mut self, damage: &mut Damage) {
        if !self.tray.overflow_open {
            return;
        }
        self.damage_tray_overflow(damage);
        self.tray.overflow_open = false;
        self.tray.overflow_hover = None;
    }

    fn press_tray_overflow_row(
        &mut self,
        index: usize,
        button: TrayButton,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let (overflowed, rows) = self.overflowed_tray();
        self.close_tray_overflow(damage);
        if index < rows {
            self.tray_press(overflowed[index] as u8, button, events);
        }
    }

    /// A button press at (`x`, `y`) while the overflow popup is open: the popup closes first,
    /// and a press on a row is the same press on that icon in the bar (section 11). `None`
    /// when the press was not the popup's, so it goes on to whatever is under it.
    pub(super) fn tray_overflow_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if !self.tray.overflow_open {
            return None;
        }
        let row = self.tray_overflow_row_at(x, y);
        let on_popup = self
            .tray_overflow()
            .is_some_and(|popup| contains(popup, x, y));
        let on_button = self
            .tray_overflow_button()
            .is_some_and(|rect| contains(rect, x, y));
        let pressed = match code {
            button::LEFT => TrayButton::Left,
            button::RIGHT => TrayButton::Right,
            _ => TrayButton::Middle,
        };
        match row {
            Some(index) => {
                self.press_tray_overflow_row(index, pressed, damage, events);
                Some(Action::None)
            }
            None => {
                self.close_tray_overflow(damage);
                (on_popup || on_button).then_some(Action::None)
            }
        }
    }

    /// The pointer moved to (`x`, `y`): the popup's row under it is drawn hovered.
    pub(super) fn tray_overflow_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        if !self.tray.overflow_open {
            return;
        }
        let hover = self.tray_overflow_row_at(x, y);
        if hover != self.tray.overflow_hover {
            self.tray.overflow_hover = hover;
            self.damage_tray_overflow(damage);
        }
    }

    /// Key `code` went down (`pressed`) or up while the overflow popup is open: Up and Down
    /// move the highlight, Enter and Space are a left click on its icon, Shift+F10 and the
    /// Menu key a right click, Esc closes.
    pub(super) fn tray_overflow_key(
        &mut self,
        code: u16,
        pressed: bool,
        shifted: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        if !pressed {
            return Action::None;
        }
        let (_, rows) = self.overflowed_tray();
        if rows == 0 {
            self.close_tray_overflow(damage);
            return Action::None;
        }
        let hover = self.tray.overflow_hover;
        match code {
            key::ESC => self.close_tray_overflow(damage),
            key::UP => {
                self.tray.overflow_hover =
                    Some(hover.map_or(rows - 1, |index| index.saturating_sub(1)));
                self.damage_tray_overflow(damage);
            }
            key::DOWN => {
                self.tray.overflow_hover = Some(hover.map_or(0, |index| (index + 1).min(rows - 1)));
                self.damage_tray_overflow(damage);
            }
            key::ENTER | key::KEYPAD_ENTER | key::SPACE => {
                if let Some(index) = hover {
                    self.press_tray_overflow_row(index, TrayButton::KeySelect, damage, events);
                }
            }
            key::F10 if shifted => {
                if let Some(index) = hover {
                    self.press_tray_overflow_row(index, TrayButton::KeyMenu, damage, events);
                }
            }
            key::COMPOSE => {
                if let Some(index) = hover {
                    self.press_tray_overflow_row(index, TrayButton::KeyMenu, damage, events);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn tray_overflow_overlaps(&self, clip: Rect) -> bool {
        self.tray.overflow_open
            && self
                .tray_overflow()
                .is_some_and(|popup| with_shadow(popup).overlaps(&clip))
    }

    pub(super) fn draw_tray_overflow(&self, canvas: &mut Canvas<'_>) {
        if !self.tray.overflow_open {
            return;
        }
        let Some(popup) = self.tray_overflow() else {
            return;
        };
        if !with_shadow(popup).overlaps(&canvas.clip()) {
            return;
        }
        canvas.shadow(popup);
        canvas.rounded(popup, &paint::INSET_R6, true, true, colour::current().menu_border);
        canvas.rounded(
            Rect::new(popup.x + 1, popup.y + 1, popup.width - 2, popup.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::current().menu,
        );
        let (overflowed, rows) = self.overflowed_tray();
        let text_y = (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        for (index, &at) in overflowed[..rows].iter().enumerate() {
            let Some(row) = self.tray_overflow_row(index) else {
                continue;
            };
            if self.tray.overflow_hover == Some(index) {
                canvas.fill(row, colour::current().menu_hover);
            }
            let Some(icon) = self.tray.icons[at] else {
                continue;
            };
            let icon_at = (popup.x + OVERFLOW_ICON_AT, row.y + OVERFLOW_ICON_AT);
            let name = self.tray_name(PanelItem::Tray(at as u8)).unwrap_or("");
            match icon.pixels {
                Some(pixels) => draw_pixels(canvas, icon_at, &pixels),
                None => draw_tile(canvas, icon_at, self.window_at(icon.slot).title()),
            }
            let (text, cut) = cut_title(name, OVERFLOW_GLYPHS);
            let x = popup.x + OVERFLOW_TEXT_X;
            canvas.text(x, row.y + text_y, text, colour::current().text);
            if cut {
                let after = x + text.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(after, row.y + text_y, ellipsis(), colour::current().text);
            }
        }
    }

    /// The overflow popup's row drawn highlighted, if any.
    pub fn tray_overflow_highlight(&self) -> Option<usize> {
        self.tray.overflow_hover
    }
}

fn draw_pixels(canvas: &mut Canvas<'_>, (x, y): (u32, u32), pixels: &[u32; PIXELS]) {
    for (row, line) in pixels.chunks(TRAY_ICON_SIDE).enumerate() {
        for (column, &pixel) in line.iter().enumerate() {
            if pixel & OPAQUE != 0 {
                canvas.pixel(x + column as u32, y + row as u32, pixel & 0x00ff_ffff);
            }
        }
    }
}

fn draw_tile(canvas: &mut Canvas<'_>, (x, y): (u32, u32), title: &str) {
    let side = TRAY_ICON_SIDE as u32;
    canvas.fill(Rect::new(x, y, side, side), colour::current().icon_tile_border);
    canvas.fill(
        Rect::new(x + 1, y + 1, side - 2, side - 2),
        colour::current().icon_tile,
    );
    let letter = title
        .chars()
        .find(char::is_ascii_alphanumeric)
        .map_or('?', |letter| letter.to_ascii_uppercase());
    let mut buffer = [0u8; 4];
    canvas.text(x + 4, y, letter.encode_utf8(&mut buffer), colour::current().text);
}

impl Desktop {
    /// A balloon of client `client`'s icon `key` (D364): `text` (UTF-8, its title and text
    /// already joined) shows in the shell's notice, cut at 36 glyphs with `...`, and the icon's
    /// window gets `NIN_BALLOONSHOW`; an empty text closes the icon's balloon if it shows.
    pub fn show_tray_balloon(
        &mut self,
        client: u16,
        window: u32,
        key: u32,
        text: &[u8],
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, window)?;
        let text = core::str::from_utf8(text).map_err(|_| WindowError::Invalid)?;
        let known = self
            .tray
            .icons
            .iter()
            .flatten()
            .any(|icon| icon.client == client && icon.key == key);
        if !known {
            return Err(WindowError::Invalid);
        }
        let (x, y) = self.pointer;
        if text.is_empty() {
            if self.tray.balloon_owner == Some((slot, key)) {
                self.close_notice(damage);
                self.tray.balloon_owner = None;
                let record = EventRecord::new(event::TRAY, 0, NIN_BALLOONHIDE, key as i32, x, y);
                self.send(slot, record, events);
            }
            return Ok(());
        }
        if let Some((owner, owner_key)) = self.tray.balloon_owner
            && (owner, owner_key) != (slot, key)
        {
            let record = EventRecord::new(event::TRAY, 0, NIN_BALLOONHIDE, owner_key as i32, x, y);
            self.send(owner, record, events);
        }
        let mut length = 0;
        for (glyphs, character) in text.chars().enumerate() {
            if glyphs == BALLOON_GLYPHS {
                for _ in 0..3 {
                    self.tray.balloon[length] = b'.';
                    length += 1;
                }
                break;
            }
            let shown = if character == '\n' { ' ' } else { character };
            let mut bytes = [0u8; 4];
            let encoded = shown.encode_utf8(&mut bytes).as_bytes();
            self.tray.balloon[length..length + encoded.len()].copy_from_slice(encoded);
            length += encoded.len();
        }
        self.tray.balloon_length = length;
        self.tray.balloon_owner = Some((slot, key));
        self.show_balloon_notice(damage);
        let record = EventRecord::new(event::TRAY, 0, NIN_BALLOONSHOW, key as i32, x, y);
        self.send(slot, record, events);
        Ok(())
    }

    /// The balloon's text, for the notice that shows it.
    pub(super) fn balloon_text(&self) -> &str {
        core::str::from_utf8(&self.tray.balloon[..self.tray.balloon_length]).unwrap_or("")
    }

    /// A press on the notice while it shows a balloon (D364): the balloon's window gets
    /// `NIN_BALLOONUSERCLICK`; whether the press was one.
    pub(super) fn balloon_pressed(&mut self, events: &mut dyn EventSink) -> bool {
        let (x, y) = self.pointer;
        if !self.balloon_at(x, y) {
            return false;
        }
        if let Some((slot, key)) = self.tray.balloon_owner.take() {
            let record = EventRecord::new(event::TRAY, 0, NIN_BALLOONUSERCLICK, key as i32, x, y);
            self.send(slot, record, events);
        }
        true
    }
}
