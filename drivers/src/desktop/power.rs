//! The bar's battery indicator and the power countdown notice (D599, docs/ux/power-v0.1.md
//! sections 2 and 4.4): a 16x16 battery whose fill is the charge, a bolt on line power and an
//! `!` when low, directly left of the top bar's power button or 8 px left of the taskbar's
//! clock, shown only when a battery is reported; its tooltip, which is also its accessible
//! name; a click that opens a menu of the facts and `Power Settings`; and the notice the
//! kernel's power thread counts a power-off down in, which no key or click closes.

use super::paint::Canvas;
use super::popups::with_shadow;
use super::protocol::EventSink;
use super::shell::PanelItem;
use super::{Action, Desktop, MENU_ITEM_HEIGHT, MENU_TEXT_X, SettingsPane, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use core::fmt::Write;

/// At or under this percent, off line power, the battery is low.
pub const LOW_BATTERY_PERCENT: u8 = 15;
/// The glyph's side at scale 1.
pub const BATTERY_GLYPH_SIDE: u32 = 16;
/// The menu's width.
pub const BATTERY_MENU_WIDTH: u32 = 240;
/// The countdown notice's narrowest width.
pub const COUNTDOWN_MIN_WIDTH: u32 = 320;

const MOST_BATTERIES: usize = 4;
const INDICATOR_BEFORE_CLOCK: u32 = 8;
const TOP_BAR_ITEM: u32 = 28;
const TASKBAR_ITEM: u32 = 40;
const TOOLTIP_CAPACITY: usize = 48;
const COUNTDOWN_CAPACITY: usize = 96;
const SEPARATOR_HEIGHT: u32 = 9;
const COUNTDOWN_HEIGHT: u32 = 40;
const COUNTDOWN_TOP: u32 = 16;
const COUNTDOWN_PADDING: u32 = 16;
const FILL_CELLS: u32 = 9;
const POWER_SETTINGS: &str = "Power Settings";
const POWER_PANE: &str = "power";

/// What the battery indicator shows, as the kernel tells it (D599).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerShown {
    /// Each battery's charge in percent.
    pub percents: [u8; MOST_BATTERIES],
    /// How many batteries there are.
    pub count: u8,
    /// Whether a battery charges.
    pub charging: bool,
    /// Whether line power is online.
    pub line_power: bool,
}

impl PowerShown {
    fn mean(&self) -> u8 {
        let count = usize::from(self.count).min(MOST_BATTERIES);
        let sum: u32 = self.percents[..count].iter().map(|&p| u32::from(p)).sum();
        (sum / count.max(1) as u32) as u8
    }

    fn is_low(&self) -> bool {
        !self.line_power && self.mean() <= LOW_BATTERY_PERCENT
    }

    fn state_word(&self) -> &'static str {
        match (self.is_low(), self.charging, self.line_power) {
            (true, _, _) => "low",
            (false, true, _) => "charging",
            (false, false, true) if self.mean() >= 100 => "full",
            _ => "discharging",
        }
    }
}

/// The indicator's state: what it shows and its tooltip text.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BatteryIndicator {
    shown: Option<PowerShown>,
    tooltip: [u8; TOOLTIP_CAPACITY],
    tooltip_length: usize,
    menu: Option<bool>,
    countdown: Option<([u8; COUNTDOWN_CAPACITY], u8)>,
}

struct Writer<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let place = self.out.get_mut(self.length..end).ok_or(core::fmt::Error)?;
        place.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

fn tooltip_of(shown: &PowerShown, out: &mut [u8; TOOLTIP_CAPACITY]) -> usize {
    let mut writer = Writer { out, length: 0 };
    let state = shown.state_word();
    let _ = match shown.count {
        0 | 1 => write!(writer, "Battery: {}%, {state}", shown.percents[0]),
        _ => write!(
            writer,
            "Batteries: {}% and {}%, {state}",
            shown.percents[0], shown.percents[1]
        ),
    };
    writer.length
}

impl Desktop {
    /// Shows the batteries `shown` on the bar, `None` for a machine without one (D599). The
    /// kernel calls it on each change; the indicator appearing or going re-lays the bar's
    /// right-hand items.
    pub fn set_power(&mut self, shown: Option<PowerShown>, damage: &mut Damage) {
        let shown = shown.filter(|shown| shown.count > 0);
        if shown == self.battery.shown {
            return;
        }
        let comes_or_goes = shown.is_some() != self.battery.shown.is_some();
        if let Some(indicator) = self.battery_indicator() {
            damage.add(indicator);
        }
        self.battery.shown = shown;
        self.battery.tooltip_length = shown.map_or(0, |shown| tooltip_of(&shown, &mut self.battery.tooltip));
        if comes_or_goes {
            if self.panels.tooltip == Some(PanelItem::Battery) {
                self.hide_tooltip(damage);
            }
            for panel in self.layout.panels() {
                damage.add(panel);
            }
            self.panels.moved = true;
            self.refresh_panel_hover(damage);
        } else {
            self.tooltip_text_changed(PanelItem::Battery, damage);
        }
        if self.battery.menu.is_some() {
            if let Some(menu) = self.battery_menu() {
                damage.add(with_shadow(menu));
            }
        }
    }

    /// The indicator's hit target while a battery is reported: 28 px square directly left of
    /// the top bar's power button, or a 40 px taskbar item 8 px left of the clock.
    pub fn battery_indicator(&self) -> Option<Rect> {
        self.battery.shown?;
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                self.clock_rect()
                    .x
                    .checked_sub(INDICATOR_BEFORE_CLOCK + TASKBAR_ITEM)?,
                bar.y,
                TASKBAR_ITEM,
                TASKBAR_ITEM,
            )),
            (None, Some(bar)) => Some(Rect::new(
                self.power_button().x.checked_sub(TOP_BAR_ITEM)?,
                bar.y,
                TOP_BAR_ITEM,
                TOP_BAR_ITEM,
            )),
            (None, None) => None,
        }
    }

    /// The indicator's tooltip, which is also its accessible name, while it shows.
    pub fn battery_indicator_name(&self) -> Option<&str> {
        self.battery.shown?;
        core::str::from_utf8(&self.battery.tooltip[..self.battery.tooltip_length]).ok()
    }

    pub(super) fn draw_battery_item(&self, canvas: &mut Canvas<'_>) {
        let (Some(shown), Some(indicator)) = (self.battery.shown, self.battery_indicator()) else {
            return;
        };
        if !indicator.overlaps(&canvas.clip()) {
            return;
        }
        let open = self.battery.menu.is_some();
        if let Some(fill) = self.item_fill(PanelItem::Battery, open) {
            canvas.fill(
                Rect::new(indicator.x, indicator.y + 1, indicator.width, indicator.height - 1),
                fill,
            );
        }
        let x = indicator.x + (indicator.width - BATTERY_GLYPH_SIDE) / 2;
        let y = indicator.y + (indicator.height - BATTERY_GLYPH_SIDE) / 2;
        draw_battery_glyph(canvas, x, y, &shown);
    }

    pub(super) fn open_battery_menu(&mut self, damage: &mut Damage) {
        if self.battery.shown.is_none() {
            return;
        }
        self.battery.menu = Some(false);
        if let Some(menu) = self.battery_menu() {
            damage.add(with_shadow(menu));
        }
    }

    pub(super) fn close_battery_menu(&mut self, damage: &mut Damage) {
        if let Some(menu) = self.battery_menu() {
            damage.add(with_shadow(menu));
        }
        if let Some(indicator) = self.battery_indicator() {
            damage.add(indicator);
        }
        self.battery.menu = None;
    }

    /// Whether the battery menu is open.
    pub fn battery_menu_open(&self) -> bool {
        self.battery.menu.is_some()
    }

    fn battery_menu_rows(&self) -> u32 {
        u32::from(self.battery.shown.map_or(1, |shown| shown.count.max(1))) + 2
    }

    /// The battery menu's rectangle while it is open: under the top bar or above the taskbar,
    /// its right edge on the indicator's.
    pub fn battery_menu(&self) -> Option<Rect> {
        self.battery.menu?;
        let indicator = self.battery_indicator()?;
        let height = 2 + MENU_ITEM_HEIGHT * self.battery_menu_rows() + SEPARATOR_HEIGHT;
        let x = indicator.right().saturating_sub(BATTERY_MENU_WIDTH);
        let y = match self.layout.taskbar {
            Some(bar) => bar.y.saturating_sub(height + 4),
            None => indicator.bottom() + 4,
        };
        Some(Rect::new(x, y, BATTERY_MENU_WIDTH, height))
    }

    fn power_settings_row(&self) -> Option<Rect> {
        let menu = self.battery_menu()?;
        let rows = self.battery_menu_rows() - 1;
        Some(Rect::new(
            menu.x + 1,
            menu.y + 1 + MENU_ITEM_HEIGHT * rows + SEPARATOR_HEIGHT,
            menu.width - 2,
            MENU_ITEM_HEIGHT,
        ))
    }

    pub(super) fn battery_menu_overlaps(&self, clip: Rect) -> bool {
        self.battery_menu()
            .is_some_and(|menu| with_shadow(menu).overlaps(&clip))
    }

    /// A button press at (`x`, `y`) while the battery menu is open: `Power Settings` opens the
    /// Power pane, a press on the menu or the indicator is the menu's, anything else closes it.
    pub(super) fn battery_menu_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        self.battery.menu?;
        let on_settings = self.power_settings_row().is_some_and(|row| contains(row, x, y));
        let on_menu = self.battery_menu().is_some_and(|menu| contains(menu, x, y));
        let on_indicator = self
            .battery_indicator()
            .is_some_and(|indicator| contains(indicator, x, y));
        self.close_battery_menu(damage);
        match () {
            () if on_settings && code == crate::input::button::LEFT => Some(
                self.open_settings_pane(SettingsPane::Named(POWER_PANE), damage, events),
            ),
            () if on_menu || on_indicator => Some(Action::None),
            () => None,
        }
    }

    /// Key `code` while the battery menu is open: Up and Down select `Power Settings`, the one
    /// item; Enter chooses it; Esc closes the menu.
    pub(super) fn battery_menu_key(
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
            key::ESC => self.close_battery_menu(damage),
            key::UP | key::DOWN => {
                self.battery.menu = Some(true);
                if let Some(menu) = self.battery_menu() {
                    damage.add(menu);
                }
            }
            key::ENTER | key::KEYPAD_ENTER if self.battery.menu == Some(true) => {
                self.close_battery_menu(damage);
                return self.open_settings_pane(SettingsPane::Named(POWER_PANE), damage, events);
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn draw_battery_menu(&self, canvas: &mut Canvas<'_>) {
        let (Some(menu), Some(shown), Some(selected)) =
            (self.battery_menu(), self.battery.shown, self.battery.menu)
        else {
            return;
        };
        canvas.shadow(menu);
        Self::draw_popup_surface(canvas, menu, colour::MENU);
        let text_y = (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        let mut row_y = menu.y + 1;
        let mut line = [0u8; TOOLTIP_CAPACITY];
        for index in 0..usize::from(shown.count.max(1)) {
            let mut writer = Writer {
                out: &mut line,
                length: 0,
            };
            let percent = shown.percents[index.min(MOST_BATTERIES - 1)];
            let _ = match shown.count {
                0 | 1 => write!(writer, "Battery: {percent}%"),
                _ => write!(writer, "Battery {}: {percent}%", index + 1),
            };
            let length = writer.length;
            let text = core::str::from_utf8(&line[..length]).unwrap_or("");
            canvas.text(menu.x + MENU_TEXT_X, row_y + text_y, text, colour::TEXT_DIM);
            row_y += MENU_ITEM_HEIGHT;
        }
        let state = match shown.state_word() {
            "low" => "Low: plug in soon",
            "charging" => "Charging",
            "full" => "Plugged in, full",
            _ => "Discharging",
        };
        canvas.text(menu.x + MENU_TEXT_X, row_y + text_y, state, colour::TEXT_DIM);
        row_y += MENU_ITEM_HEIGHT;
        canvas.fill(
            Rect::new(menu.x + 1, row_y + SEPARATOR_HEIGHT / 2, menu.width - 2, 1),
            colour::MENU_BORDER,
        );
        row_y += SEPARATOR_HEIGHT;
        if selected {
            canvas.fill(
                Rect::new(menu.x + 1, row_y, menu.width - 2, MENU_ITEM_HEIGHT),
                colour::MENU_HOVER,
            );
        }
        canvas.text(menu.x + MENU_TEXT_X, row_y + text_y, POWER_SETTINGS, colour::TEXT);
    }

    /// Shows the power thread's countdown `text` (`None` ends it), redrawn each time the
    /// kernel calls with the seconds left (D599). It closes on no key or click.
    pub fn set_countdown(&mut self, text: Option<&[u8]>, damage: &mut Damage) {
        if let Some(rect) = self.countdown_rect() {
            damage.add(with_shadow(rect));
        }
        self.battery.countdown = text.map(|text| {
            let mut copied = [0u8; COUNTDOWN_CAPACITY];
            let length = text.len().min(COUNTDOWN_CAPACITY);
            copied[..length].copy_from_slice(&text[..length]);
            (copied, length as u8)
        });
        if let Some(rect) = self.countdown_rect() {
            damage.add(with_shadow(rect));
        }
    }

    fn countdown_text(&self) -> Option<&str> {
        let (text, length) = self.battery.countdown.as_ref()?;
        core::str::from_utf8(&text[..usize::from(*length)]).ok()
    }

    fn countdown_rect(&self) -> Option<Rect> {
        let text = self.countdown_text()?;
        let work = self.layout.work_area;
        let width = (text.chars().count() as u32 * GLYPH_WIDTH as u32 + 2 * COUNTDOWN_PADDING)
            .max(COUNTDOWN_MIN_WIDTH)
            .min(work.width.saturating_sub(2 * COUNTDOWN_PADDING));
        Some(Rect::new(
            work.x + (work.width - width) / 2,
            work.y + COUNTDOWN_TOP,
            width,
            COUNTDOWN_HEIGHT,
        ))
    }

    pub(super) fn countdown_overlaps(&self, clip: Rect) -> bool {
        self.countdown_rect()
            .is_some_and(|rect| with_shadow(rect).overlaps(&clip))
    }

    pub(super) fn draw_countdown(&self, canvas: &mut Canvas<'_>) {
        let (Some(text), Some(rect)) = (self.countdown_text(), self.countdown_rect()) else {
            return;
        };
        canvas.shadow(rect);
        Self::draw_popup_surface(canvas, rect, colour::MENU);
        let width = text.chars().count() as u32 * GLYPH_WIDTH as u32;
        canvas.text(
            rect.x + rect.width.saturating_sub(width) / 2,
            rect.y + (COUNTDOWN_HEIGHT - GLYPH_HEIGHT as u32) / 2,
            text,
            colour::TEXT,
        );
    }
}

fn draw_battery_glyph(canvas: &mut Canvas<'_>, x: u32, y: u32, shown: &PowerShown) {
    let at = |dx: u32, dy: u32, width: u32, height: u32| Rect::new(x + dx, y + dy, width, height);
    let text = colour::TEXT;
    let accent = colour::ACCENT;
    canvas.fill(at(0, 4, 13, 1), text);
    canvas.fill(at(0, 11, 13, 1), text);
    canvas.fill(at(0, 4, 1, 8), text);
    canvas.fill(at(12, 4, 1, 8), text);
    canvas.fill(at(12, 6, 3, 1), text);
    canvas.fill(at(12, 9, 3, 1), text);
    canvas.fill(at(14, 7, 1, 2), text);
    let filled = (u32::from(shown.mean()) * FILL_CELLS).div_ceil(100).min(FILL_CELLS);
    let low = shown.is_low();
    let fill = if low { accent } else { text };
    if filled > 0 {
        canvas.fill(at(2, 6, filled, 4), fill);
    }
    if shown.line_power {
        canvas.fill(at(8, 0, 2, 1), accent);
        canvas.fill(at(7, 1, 2, 1), accent);
        canvas.fill(at(6, 2, 4, 1), accent);
        canvas.fill(at(7, 3, 1, 1), accent);
    }
    if low {
        canvas.fill(at(6, 6, 1, 2), accent);
        canvas.fill(at(6, 9, 1, 1), accent);
    }
}
