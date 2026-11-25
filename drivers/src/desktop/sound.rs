//! The bar's volume indicator, its popup, the volume keys and the on-screen volume bar (D775,
//! docs/ux/sound-v0.1.md sections 2 and 3): a speaker with zero to three waves, crossed when
//! muted, between the battery and the network, shown only while the sound server says an
//! output exists (`SET_SOUND`); its tooltip, which is also its accessible name; a click that
//! opens a popup with the mute button, a slider and `Sound Settings`; and Volume Up, Volume
//! Down and Mute in every profile, which step the volume by 5 (1 with Shift) and show the bar
//! for 1.5 s. Every change runs `sound volume <n>` or `sound mute on|off`, which the server
//! applies and saves, then tells the shell again.

use super::catalogue::Command;
use super::paint::Canvas;
use super::popups::with_shadow;
use super::shell::PanelItem;
use super::{Action, Desktop, SettingsPane, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use core::fmt::Write;

/// The volume keys' step, in percent.
pub const VOLUME_STEP: u8 = 5;
/// How long the on-screen volume bar shows after a volume key.
pub const VOLUME_BAR_MS: u32 = 1500;
/// At or above this volume the glyph has two waves.
pub const VOLUME_WAVE_1: u8 = 33;
/// At or above this volume the glyph has three waves.
pub const VOLUME_WAVE_2: u8 = 66;
/// The popup's size.
pub const SOUND_POPUP: (u32, u32) = (248, 84);
/// The on-screen bar's size.
pub const VOLUME_BAR: (u32, u32) = (320, 40);
/// The longest output name the shell keeps.
pub const SOUND_NAME_BYTES: usize = 28;

const GLYPH_SIDE: u32 = 16;
const TOOLTIP_CAPACITY: usize = 48;
const TRACK_X: u32 = 48;
const TRACK_TRAVEL: u32 = 144;
const TRACK_TOP: u32 = 22;
const TRACK_HEIGHT: u32 = 4;
const THUMB: u32 = 16;
const READOUT_END: u32 = 236;
const SEPARATOR_ROW: u32 = 48;
const SETTINGS_ROW: (u32, u32) = (50, 32);
const SETTINGS_TEXT_X: u32 = 12;
const MUTE_TARGET: (u32, u32, u32, u32) = (8, 8, 24, 32);
const BAR_TOP: u32 = 16;
const BAR_TRACK: (u32, u32, u32, u32) = (40, 18, 216, 4);
const BAR_READOUT_END: u32 = 308;
const SOUND_SETTINGS: &str = "Sound Settings";
const SOUND_PANE: &str = "sound";
const SOUND_PROGRAM: &str = "/bin/sound";

/// What the sound server last told the shell: the output's volume, mute, name and how many
/// outputs there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundShown {
    pub volume: u8,
    pub mute: bool,
    pub name: [u8; SOUND_NAME_BYTES],
    pub name_length: u8,
    pub outputs: u8,
}

impl SoundShown {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..usize::from(self.name_length)]).unwrap_or("")
    }
}

/// What a volume change asks the sound server for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCommand {
    Volume(u8),
    Mute(bool),
}

/// The indicator's state.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SoundIndicator {
    shown: Option<SoundShown>,
    popup: Option<bool>,
    bar_until: Option<u32>,
    tooltip: [u8; TOOLTIP_CAPACITY],
    tooltip_length: usize,
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

fn tooltip_of(shown: &SoundShown, out: &mut [u8; TOOLTIP_CAPACITY]) -> usize {
    let mut writer = Writer { out, length: 0 };
    let _ = write!(writer, "Volume: {}%", shown.volume);
    if shown.mute {
        let _ = write!(writer, ", muted");
    }
    if shown.outputs > 1 {
        let _ = write!(writer, ", {}", shown.name());
    }
    writer.length
}

/// The x of the popup's slider thumb's centre for `volume`, in popup coordinates.
pub fn thumb_centre(volume: u8) -> u32 {
    TRACK_X + (u32::from(volume.min(100)) * TRACK_TRAVEL + 50) / 100
}

/// The volume a press at popup x `x` on the track asks for.
pub fn volume_at(x: u32) -> u8 {
    let along = x.saturating_sub(TRACK_X).min(TRACK_TRAVEL);
    ((along * 100 + TRACK_TRAVEL / 2) / TRACK_TRAVEL) as u8
}

/// The volume a volume key asks for: `Up` unmutes first, then steps; with Shift the step is 1.
pub fn stepped(volume: u8, up: bool, fine: bool) -> u8 {
    let step = if fine { 1 } else { VOLUME_STEP };
    match up {
        true => volume.saturating_add(step).min(100),
        false => volume.saturating_sub(step),
    }
}

/// How many waves the glyph draws for `volume`.
pub fn waves(volume: u8) -> u8 {
    match volume {
        0 => 0,
        1..VOLUME_WAVE_1 => 1,
        VOLUME_WAVE_1..VOLUME_WAVE_2 => 2,
        _ => 3,
    }
}

impl Desktop {
    /// Shows the sound output `shown`, `None` without one (D775): the sound server calls it on
    /// each change through `SET_SOUND`.
    pub fn set_sound(&mut self, shown: Option<SoundShown>, damage: &mut Damage) {
        if shown == self.sound.shown {
            return;
        }
        let comes_or_goes = shown.is_some() != self.sound.shown.is_some();
        if let Some(indicator) = self.sound_indicator() {
            damage.add(indicator);
        }
        self.sound.shown = shown;
        self.sound.tooltip_length =
            shown.map_or(0, |shown| tooltip_of(&shown, &mut self.sound.tooltip));
        if comes_or_goes {
            if self.panels.tooltip == Some(PanelItem::Sound) {
                self.hide_tooltip(damage);
            }
            for panel in self.layout.panels() {
                damage.add(panel);
            }
            self.panels.moved = true;
            self.refresh_panel_hover(damage);
            if shown.is_none() {
                self.close_sound_popup(damage);
            }
        } else {
            self.tooltip_text_changed(PanelItem::Sound, damage);
            if let Some(indicator) = self.sound_indicator() {
                damage.add(indicator);
            }
        }
        if let Some(popup) = self.sound_popup() {
            damage.add(with_shadow(popup));
        }
        if self.sound.bar_until.is_some() {
            damage.add(with_shadow(self.volume_bar_rect()));
        }
    }

    /// The indicator's hit target while an output exists: directly left of the battery
    /// indicator, else of the top bar's power button, else 8 px left of the taskbar's clock.
    pub fn sound_indicator(&self) -> Option<Rect> {
        self.sound.shown?;
        if let Some(battery) = self.battery_indicator() {
            return Some(Rect::new(
                battery.x.checked_sub(battery.width)?,
                battery.y,
                battery.width,
                battery.height,
            ));
        }
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                self.clock_rect()
                    .x
                    .checked_sub(8 * self.bar_scale() + bar.height)?,
                bar.y,
                bar.height,
                bar.height,
            )),
            (None, Some(bar)) => Some(Rect::new(
                self.power_button().x.checked_sub(bar.height)?,
                bar.y,
                bar.height,
                bar.height,
            )),
            (None, None) => None,
        }
    }

    /// The indicator's tooltip, which is also its accessible name, while it shows.
    pub fn sound_indicator_name(&self) -> Option<&str> {
        self.sound.shown?;
        core::str::from_utf8(&self.sound.tooltip[..self.sound.tooltip_length]).ok()
    }

    pub(super) fn draw_sound_item(&self, canvas: &mut Canvas<'_>) {
        let (Some(shown), Some(indicator)) = (self.sound.shown, self.sound_indicator()) else {
            return;
        };
        if !indicator.overlaps(&canvas.clip()) {
            return;
        }
        let open = self.sound.popup.is_some();
        if let Some(fill) = self.item_fill(PanelItem::Sound, open) {
            canvas.fill(
                Rect::new(indicator.x, indicator.y + 1, indicator.width, indicator.height - 1),
                fill,
            );
        }
        let x = indicator.x + (indicator.width - GLYPH_SIDE) / 2;
        let y = indicator.y + (indicator.height - GLYPH_SIDE) / 2;
        draw_speaker(canvas, x, y, shown.volume, shown.mute);
    }

    /// A press on the indicator: the popup opens, or closes when open.
    pub(super) fn sound_indicator_press(&mut self, damage: &mut Damage) {
        match self.sound.popup.is_some() {
            true => self.close_sound_popup(damage),
            false => {
                self.sound.popup = Some(false);
                if let Some(popup) = self.sound_popup() {
                    damage.add(with_shadow(popup));
                }
            }
        }
    }

    pub(super) fn close_sound_popup(&mut self, damage: &mut Damage) {
        if let Some(popup) = self.sound_popup() {
            damage.add(with_shadow(popup));
        }
        if let Some(indicator) = self.sound_indicator() {
            damage.add(indicator);
        }
        self.sound.popup = None;
    }

    /// Whether the popup is open.
    pub fn sound_popup_open(&self) -> bool {
        self.sound.popup.is_some()
    }

    /// The popup's rectangle while it is open: under the top bar or above the taskbar, its
    /// right edge on the indicator's.
    pub fn sound_popup(&self) -> Option<Rect> {
        self.sound.popup?;
        let indicator = self.sound_indicator()?;
        let (width, height) = SOUND_POPUP;
        let x = indicator.right().saturating_sub(width);
        let y = match self.layout.taskbar {
            Some(bar) => bar.y.saturating_sub(height + 4),
            None => indicator.bottom() + 4,
        };
        Some(Rect::new(x, y, width, height))
    }

    pub(super) fn sound_popup_overlaps(&self, clip: Rect) -> bool {
        self.sound_popup()
            .is_some_and(|popup| with_shadow(popup).overlaps(&clip))
    }

    fn change_sound(&mut self, command: SoundCommand, damage: &mut Damage) -> Action {
        if let Some(shown) = self.sound.shown.as_mut() {
            match command {
                SoundCommand::Volume(volume) => shown.volume = volume,
                SoundCommand::Mute(mute) => shown.mute = mute,
            }
            let shown = *shown;
            self.sound.tooltip_length = tooltip_of(&shown, &mut self.sound.tooltip);
        }
        if let Some(indicator) = self.sound_indicator() {
            damage.add(indicator);
        }
        if let Some(popup) = self.sound_popup() {
            damage.add(popup);
        }
        Action::Sound(command)
    }

    /// A button press at (`x`, `y`) while the popup is open: the mute button toggles mute, the
    /// track sets the volume, `Sound Settings` opens the pane, the popup or the indicator keep
    /// it, anything else closes it.
    pub(super) fn sound_popup_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn super::protocol::EventSink,
    ) -> Option<Action> {
        self.sound.popup?;
        let popup = self.sound_popup()?;
        let shown = self.sound.shown?;
        let on_indicator = self
            .sound_indicator()
            .is_some_and(|indicator| contains(indicator, x, y));
        if !contains(popup, x, y) {
            self.close_sound_popup(damage);
            return on_indicator.then_some(Action::None);
        }
        if code != crate::input::button::LEFT {
            return Some(Action::None);
        }
        let (local_x, local_y) = (x - popup.x, y - popup.y);
        let (mx, my, mw, mh) = MUTE_TARGET;
        if (mx..mx + mw).contains(&local_x) && (my..my + mh).contains(&local_y) {
            return Some(self.change_sound(SoundCommand::Mute(!shown.mute), damage));
        }
        if local_y < SEPARATOR_ROW && (TRACK_X - THUMB / 2..TRACK_X + TRACK_TRAVEL + THUMB / 2)
            .contains(&local_x)
        {
            return Some(self.change_sound(SoundCommand::Volume(volume_at(local_x)), damage));
        }
        if local_y >= SETTINGS_ROW.0 {
            self.close_sound_popup(damage);
            return Some(self.open_settings_pane(SettingsPane::Named(SOUND_PANE), damage, events));
        }
        Some(Action::None)
    }

    /// Key `code` while the popup is open: Left and Right step 1 (5 with Shift), Page Up and
    /// Page Down 10, Home 0, End 100, `M` mutes, Down selects `Sound Settings` and Enter opens
    /// it, Esc closes the popup.
    pub(super) fn sound_popup_key(
        &mut self,
        code: u16,
        pressed: bool,
        shift: bool,
        damage: &mut Damage,
        events: &mut dyn super::protocol::EventSink,
    ) -> Action {
        let Some(shown) = self.sound.shown.filter(|_| pressed) else {
            return Action::None;
        };
        let step = if shift { VOLUME_STEP } else { 1 };
        let volume = match code {
            key::LEFT => Some(shown.volume.saturating_sub(step)),
            key::RIGHT => Some(shown.volume.saturating_add(step).min(100)),
            key::PAGE_DOWN => Some(shown.volume.saturating_sub(10)),
            key::PAGE_UP => Some(shown.volume.saturating_add(10).min(100)),
            key::HOME => Some(0),
            key::END => Some(100),
            _ => None,
        };
        if let Some(volume) = volume {
            return self.change_sound(SoundCommand::Volume(volume), damage);
        }
        match code {
            key::M => return self.change_sound(SoundCommand::Mute(!shown.mute), damage),
            key::ESC => self.close_sound_popup(damage),
            key::DOWN | key::UP => {
                self.sound.popup = Some(code == key::DOWN);
                if let Some(popup) = self.sound_popup() {
                    damage.add(popup);
                }
            }
            key::ENTER | key::KEYPAD_ENTER if self.sound.popup == Some(true) => {
                self.close_sound_popup(damage);
                return self.open_settings_pane(SettingsPane::Named(SOUND_PANE), damage, events);
            }
            _ => {}
        }
        Action::None
    }

    /// Volume Up, Volume Down or Mute pressed, in any profile and before any client sees it:
    /// the change it asks for, with the on-screen bar shown; `None` for another key, or with no
    /// output.
    pub(super) fn volume_key(
        &mut self,
        code: u16,
        shift: bool,
        damage: &mut Damage,
    ) -> Option<Action> {
        if !matches!(code, key::VOLUME_UP | key::VOLUME_DOWN | key::MUTE) {
            return None;
        }
        let shown = self.sound.shown?;
        self.close_notice(damage);
        damage.add(with_shadow(self.volume_bar_rect()));
        self.sound.bar_until = Some(self.now_ms.wrapping_add(VOLUME_BAR_MS));
        let action = match code {
            key::MUTE => self.change_sound(SoundCommand::Mute(!shown.mute), damage),
            key::VOLUME_UP if shown.mute => self.change_sound(SoundCommand::Mute(false), damage),
            _ => {
                let up = code == key::VOLUME_UP;
                let volume = stepped(shown.volume, up, shift);
                self.change_sound(SoundCommand::Volume(volume), damage)
            }
        };
        Some(action)
    }

    /// The clock reached the on-screen bar's end: it closes.
    pub(super) fn tick_volume_bar(&mut self, damage: &mut Damage) {
        if let Some(until) = self.sound.bar_until
            && self.now_ms.wrapping_sub(until) as i32 >= 0
        {
            self.sound.bar_until = None;
            damage.add(with_shadow(self.volume_bar_rect()));
        }
    }

    fn volume_bar_rect(&self) -> Rect {
        let work = self.layout.work_area;
        let width = VOLUME_BAR.0.min(work.width);
        Rect::new(
            work.x + (work.width - width) / 2,
            work.y + BAR_TOP,
            width,
            VOLUME_BAR.1,
        )
    }

    pub(super) fn volume_bar_overlaps(&self, clip: Rect) -> bool {
        self.sound.bar_until.is_some() && with_shadow(self.volume_bar_rect()).overlaps(&clip)
    }

    /// Whether the on-screen volume bar shows.
    pub fn volume_bar_shown(&self) -> bool {
        self.sound.bar_until.is_some()
    }

    pub(super) fn draw_volume_bar(&self, canvas: &mut Canvas<'_>) {
        let (Some(_), Some(shown)) = (self.sound.bar_until, self.sound.shown) else {
            return;
        };
        let rect = self.volume_bar_rect();
        canvas.shadow(rect);
        Self::draw_popup_surface(canvas, rect, colour::current().menu);
        draw_speaker(canvas, rect.x + 12, rect.y + 12, shown.volume, shown.mute);
        let (tx, ty, tw, th) = BAR_TRACK;
        canvas.fill(
            Rect::new(rect.x + tx, rect.y + ty, tw, th),
            colour::current().progress_track,
        );
        if !shown.mute {
            let done = (u32::from(shown.volume) * tw + 50) / 100;
            canvas.fill(
                Rect::new(rect.x + tx, rect.y + ty, done, th),
                colour::current().accent,
            );
        }
        let mut readout = [0u8; 8];
        let text = readout_text(&shown, &mut readout);
        let width = text.len() as u32 * GLYPH_WIDTH as u32;
        canvas.text(
            rect.x + BAR_READOUT_END - width,
            rect.y + 12,
            text,
            colour::current().text,
        );
    }

    pub(super) fn draw_sound_popup(&self, canvas: &mut Canvas<'_>) {
        let (Some(popup), Some(shown), Some(selected)) =
            (self.sound_popup(), self.sound.shown, self.sound.popup)
        else {
            return;
        };
        canvas.shadow(popup);
        Self::draw_popup_surface(canvas, popup, colour::current().menu);
        let palette = colour::current();
        draw_speaker(canvas, popup.x + 12, popup.y + 16, shown.volume, shown.mute);
        canvas.fill(
            Rect::new(popup.x + TRACK_X, popup.y + TRACK_TOP, TRACK_TRAVEL, TRACK_HEIGHT),
            palette.progress_track,
        );
        let thumb = thumb_centre(shown.volume);
        if !shown.mute {
            canvas.fill(
                Rect::new(popup.x + TRACK_X, popup.y + TRACK_TOP, thumb - TRACK_X, TRACK_HEIGHT),
                palette.accent,
            );
        }
        let thumb_rect = Rect::new(popup.x + thumb - THUMB / 2, popup.y + 16, THUMB, THUMB);
        canvas.fill(thumb_rect, palette.menu_border);
        canvas.fill(
            Rect::new(thumb_rect.x + 1, thumb_rect.y + 1, THUMB - 2, THUMB - 2),
            palette.text,
        );
        let mut readout = [0u8; 8];
        let text = readout_text(&shown, &mut readout);
        let width = text.len() as u32 * GLYPH_WIDTH as u32;
        let ink = if shown.mute { palette.text_dim } else { palette.text };
        canvas.text(popup.x + READOUT_END - width, popup.y + 16, text, ink);
        canvas.fill(
            Rect::new(popup.x + 8, popup.y + SEPARATOR_ROW, popup.width - 16, 1),
            palette.menu_border,
        );
        let row = Rect::new(popup.x, popup.y + SETTINGS_ROW.0, popup.width, SETTINGS_ROW.1);
        if selected {
            canvas.fill(row, palette.menu_hover);
        }
        canvas.text(
            row.x + SETTINGS_TEXT_X,
            row.y + (row.height - GLYPH_HEIGHT as u32) / 2,
            SOUND_SETTINGS,
            palette.text,
        );
    }

    /// What [`Action::Sound`] starts: `sound volume <n>` or `sound mute on|off`.
    pub fn sound_command(&self, command: SoundCommand) -> Option<Command> {
        let mut number = [0u8; 3];
        let words: [&[u8]; 2] = match command {
            SoundCommand::Volume(volume) => {
                let digits = digits_of(volume, &mut number);
                [b"volume", digits]
            }
            SoundCommand::Mute(true) => [b"mute", b"on"],
            SoundCommand::Mute(false) => [b"mute", b"off"],
        };
        Command::program(SOUND_PROGRAM, &words)
    }
}

fn digits_of(value: u8, out: &mut [u8; 3]) -> &[u8] {
    let mut length = 0;
    let mut writer = Writer {
        out: &mut out[..],
        length: 0,
    };
    if write!(writer, "{value}").is_ok() {
        length = writer.length;
    }
    &out[..length]
}

fn readout_text<'a>(shown: &SoundShown, out: &'a mut [u8; 8]) -> &'a str {
    if shown.mute {
        return "Muted";
    }
    let mut writer = Writer {
        out: &mut out[..],
        length: 0,
    };
    let _ = write!(writer, "{}%", shown.volume);
    let length = writer.length;
    core::str::from_utf8(&out[..length]).unwrap_or("")
}

fn draw_speaker(canvas: &mut Canvas<'_>, x: u32, y: u32, volume: u8, mute: bool) {
    let ink = colour::current().text;
    let at = |dx: u32, dy: u32, width: u32, height: u32| Rect::new(x + dx, y + dy, width, height);
    for (dx, dy, width, height) in [
        (0, 5, 4, 6),
        (4, 5, 1, 6),
        (5, 4, 1, 8),
        (6, 3, 1, 10),
        (7, 2, 1, 12),
    ] {
        canvas.fill(at(dx, dy, width, height), ink);
    }
    if mute {
        for step in 0..5 {
            canvas.fill(at(9 + step, 5 + step, 1, 1), ink);
            canvas.fill(at(13 - step, 5 + step, 1, 1), ink);
        }
        return;
    }
    let arcs = [
        ((9, 6, 1, 4), (8, 5), (8, 10)),
        ((11, 4, 1, 8), (10, 3), (10, 12)),
        ((13, 2, 1, 12), (12, 1), (12, 14)),
    ];
    for (rect, top, bottom) in arcs.iter().take(usize::from(waves(volume))) {
        canvas.fill(at(rect.0, rect.1, rect.2, rect.3), ink);
        canvas.fill(at(top.0, top.1, 1, 1), ink);
        canvas.fill(at(bottom.0, bottom.1, 1, 1), ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_keys_and_the_slider_follow_the_design() {
        assert_eq!([0, 1, 33, 66, 100].map(waves), [0, 1, 2, 3, 3]);
        assert_eq!(waves(32), 1);
        assert_eq!(waves(65), 2);
        assert_eq!(stepped(50, true, false), 55);
        assert_eq!(stepped(50, false, true), 49);
        assert_eq!(stepped(98, true, false), 100);
        assert_eq!(thumb_centre(50), TRACK_X + 72);
        assert_eq!(volume_at(TRACK_X + 72), 50);
        assert_eq!(volume_at(0), 0);
        assert_eq!(volume_at(400), 100);
        let mut name = [0u8; SOUND_NAME_BYTES];
        name[..10].copy_from_slice(b"Headphones");
        let shown = SoundShown {
            volume: 50,
            mute: false,
            name,
            name_length: 10,
            outputs: 2,
        };
        let mut tooltip = [0u8; TOOLTIP_CAPACITY];
        let length = tooltip_of(&shown, &mut tooltip);
        assert_eq!(&tooltip[..length], b"Volume: 50%, Headphones");
        let muted = SoundShown {
            mute: true,
            outputs: 1,
            ..shown
        };
        let length = tooltip_of(&muted, &mut tooltip);
        assert_eq!(&tooltip[..length], b"Volume: 50%, muted");
        let mut digits = [0u8; 3];
        assert_eq!(digits_of(55, &mut digits), b"55");
    }
}
