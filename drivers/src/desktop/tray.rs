//! Windows programs' notification-area icons in the bar (D357,
//! `docs/ux/windows-tooltips-tray-v0.5.md` sections 11 to 13): the icons a client sets for its
//! windows, their run directly left of the keyboard and network indicators (28 px targets on
//! the top bar, 40 px items on the taskbar, the first added rightmost, at most four shown and
//! an overflow button past that), their drawing, their names, and the pointer's presses on
//! them, reported to the icon's window as the Windows message each stands for.

use super::paint::Canvas;
use super::protocol::{EventRecord, EventSink, TRAY_ICON_SIDE, TRAY_TIP_CAPACITY, WindowError, event};
use super::shell::{PanelItem, TASKBAR_ITEM};
use super::{Desktop, colour};
use crate::display::{Damage, Rect};

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

#[derive(Debug, Clone, Copy)]
struct TrayIcon {
    client: u16,
    slot: usize,
    key: u32,
    order: u32,
    hidden: bool,
    tip: [u8; TRAY_TIP_CAPACITY],
    tip_length: usize,
    pixels: Option<[u32; PIXELS]>,
}

/// The bar's notification-area icons.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tray {
    icons: [Option<TrayIcon>; MOST_TRAY_ICONS],
    next_order: u32,
}

impl Default for Tray {
    fn default() -> Self {
        Tray {
            icons: [None; MOST_TRAY_ICONS],
            next_order: 0,
        }
    }
}

/// Which mouse button a press on an icon was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrayButton {
    Left,
    Right,
    Middle,
}

impl Desktop {
    /// `SET_TRAY_ICON` (D357): client `client`'s icon `key` of its window `window` added or
    /// changed with `tip` (UTF-8) and its pixels (none: the generic tile), hidden or not, or
    /// removed with `remove`; the bar damaged.
    #[allow(clippy::too_many_arguments)]
    pub fn set_tray_icon(
        &mut self,
        client: u16,
        window: u32,
        key: u32,
        (hidden, remove): (bool, bool),
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
                self.tray.icons[at] = None;
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
        let order = match self.tray.icons[at] {
            Some(icon) => icon.order,
            None => {
                self.tray.next_order += 1;
                self.tray.next_order
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
            tip: stored,
            tip_length: tip.len(),
            pixels: pixels.copied(),
        });
        self.damage_tray(damage);
        Ok(())
    }

    /// Forgets the icons of the window in `slot`, which is gone.
    pub(super) fn forget_tray_icons(&mut self, slot: usize, damage: &mut Damage) {
        if self.tray.icons.iter().flatten().any(|icon| icon.slot == slot) {
            self.damage_tray(damage);
            for icon in self.tray.icons.iter_mut() {
                if icon.is_some_and(|icon| icon.slot == slot) {
                    *icon = None;
                }
            }
        }
    }

    fn damage_tray(&self, damage: &mut Damage) {
        if let Some(bar) = self.layout.taskbar.or(self.layout.top_bar) {
            damage.add(bar);
        }
    }

    /// The shown icons, rightmost first (the oldest), and whether an overflow button stands
    /// for the rest.
    fn shown_tray(&self) -> ([usize; SHOWN], usize, bool) {
        let mut order = [(u32::MAX, 0usize); MOST_TRAY_ICONS];
        let mut count = 0;
        for (at, icon) in self.tray.icons.iter().enumerate() {
            if let Some(icon) = icon.filter(|icon| !icon.hidden) {
                order[count] = (icon.order, at);
                count += 1;
            }
        }
        order[..count].sort_unstable();
        let overflow = count > SHOWN;
        let shown = if overflow { SHOWN - 1 } else { count };
        let mut indices = [0; SHOWN];
        for (index, &(_, at)) in indices.iter_mut().zip(&order[..shown]) {
            *index = at;
        }
        (indices, shown, overflow)
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
        let inset = if taskbar { TASKBAR_ICON_AT } else { TOP_BAR_ICON_AT };
        self.for_each_tray_item(|item, rect| {
            if !rect.overlaps(&canvas.clip()) {
                return;
            }
            let fill = match taskbar {
                true => self.item_fill(item, false),
                false => (self.panels.pressed == Some(item)).then_some(colour::BAR_PRESSED),
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
                _ => canvas.text(x + 4, y, "^", colour::TEXT),
            }
        });
    }

    /// A press of `button` on notification-area icon `at`: the icon's window gets the button's
    /// down and up messages at once, with the pointer's screen point.
    pub(super) fn tray_press(&mut self, at: u8, button: TrayButton, events: &mut dyn EventSink) {
        let Some(icon) = self.tray.icons.get(usize::from(at)).copied().flatten() else {
            return;
        };
        let (down, up) = match button {
            TrayButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP),
            TrayButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP),
            TrayButton::Middle => (WM_MBUTTONDOWN, WM_MBUTTONUP),
        };
        let (x, y) = self.pointer;
        for message in [down, up] {
            let record = EventRecord::new(event::TRAY, 0, message, icon.key as i32, x, y);
            self.send(icon.slot, record, events);
        }
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
    canvas.fill(Rect::new(x, y, side, side), colour::ICON_TILE_BORDER);
    canvas.fill(Rect::new(x + 1, y + 1, side - 2, side - 2), colour::ICON_TILE);
    let letter = title
        .chars()
        .find(char::is_ascii_alphanumeric)
        .map_or('?', |letter| letter.to_ascii_uppercase());
    let mut buffer = [0u8; 4];
    canvas.text(x + 4, y, letter.encode_utf8(&mut buffer), colour::TEXT);
}
