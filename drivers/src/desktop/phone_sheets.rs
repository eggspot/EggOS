//! The phone form's sheets, slice 2 (docs/ux/adaptive-shell-v0.5.md sections 2.5 to 2.7,
//! D305): the app switcher behind `Apps` (one column of cards, each the window scaled to 0.4,
//! with `Close`, and `Close all`), the shade behind the status bar (quick setting tiles, each
//! one `settings` key, `Settings`, `Power`, and the last four notices), and `Back` for apps
//! that take it (`HANDLES_BACK`).

use super::catalogue::AppId;
use super::paint::{self, Canvas};
use super::protocol::{EventRecord, EventSink, event};
use super::windows;
use super::{Action, Contents, Desktop, MAX_WINDOWS, SettingsPane, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use crate::input::key;
use alloc::vec::Vec;

/// How many notices the shade keeps.
pub(super) const NOTICES_KEPT: usize = 4;

const SIDE: u32 = 16;
const CARD: (u32, u32) = (358, 348);
const CARD_GAP: u32 = 16;
const CARD_TITLE: (u32, u32) = (16, 14);
const THUMBNAIL: (u32, u32, u32) = (40, 156, 300);
const CLOSE_TARGET: u32 = 44;
const CLOSE_CIRCLE: u32 = 12;
const CLOSE_ALL_HEIGHT: u32 = 44;
const WHEEL_STEP: u32 = 48;
const TILE: (u32, u32) = (171, 56);
const TILE_GAP: u32 = 16;
const TILE_LINES: (u32, u32, u32) = (16, 10, 30);
const MARK: (u32, u32) = (4, 40);
const POWER_HEIGHT: u32 = 44;
const NOTICE_ROW: (u32, u32) = (44, 48);
const CLEAR_WIDTH: u32 = 80;
const SHEET_END: u32 = 16;
const POWER_ROWS: [(&str, Action); 2] = [("Power off", Action::PowerOff), ("Restart", Action::Restart)];
const POWER_ROW_HEIGHT: u32 = 48;
const NOTICE_GLYPHS: usize = 40;
const SHADE_WIDTH: u32 = 390;

/// A shade tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tile {
    Network,
    DarkTheme,
    HighContrast,
    Rotation,
}

const TILES: [Tile; 4] = [Tile::Network, Tile::DarkTheme, Tile::HighContrast, Tile::Rotation];

impl Desktop {
    /// Keeps `text` among the shade's notices, newest first.
    pub(super) fn remember_notice(&mut self, text: &'static str) {
        self.phone.notices.copy_within(0..NOTICES_KEPT - 1, 1);
        self.phone.notices[0] = text;
        self.phone.notice_count = (self.phone.notice_count + 1).min(NOTICES_KEPT);
    }

    /// Whether the switcher, the shade or the power sheet is open.
    pub(super) fn phone_overlay_open(&self) -> bool {
        self.phone.switcher || self.phone.shade || self.phone.power_sheet
    }

    /// Closes the open sheet; whether one was open.
    pub(super) fn close_phone_overlays(&mut self) -> bool {
        let open = self.phone_overlay_open();
        self.phone.switcher = false;
        self.phone.shade = false;
        self.phone.power_sheet = false;
        open
    }

    /// Sends window `slot` the `Back` key, down and up, as any key goes to a window.
    pub(super) fn send_back_key(&mut self, slot: usize, events: &mut dyn EventSink) {
        for pressed in [1, 0] {
            let record = EventRecord::new(event::KEY, 0, i32::from(key::BACK), pressed, 0, 0);
            self.send(slot, record, events);
        }
    }

    fn phone_switcher_entries(&self) -> Vec<usize> {
        let mut order = [0u8; MAX_WINDOWS];
        let count = self.switcher_order(&mut order);
        order[..count].iter().map(|&slot| usize::from(slot)).collect()
    }

    fn card(&self, index: usize) -> Rect {
        let work = self.layout.work_area;
        Rect::new(
            SIDE,
            (work.y + CARD_GAP + (CARD.1 + CARD_GAP) * index as u32).saturating_sub(self.phone.switcher_scroll),
            CARD.0.min(work.width.saturating_sub(2 * SIDE)),
            CARD.1,
        )
    }

    fn card_close(card: Rect) -> Rect {
        Rect::new(card.right() - CLOSE_TARGET, card.y, CLOSE_TARGET, CLOSE_TARGET)
    }

    fn close_all_button(&self, count: usize) -> Rect {
        let below = self.card(count.saturating_sub(1));
        Rect::new(SIDE, below.bottom() + CARD_GAP, below.width, CLOSE_ALL_HEIGHT)
    }

    /// A press in the switcher: a card's `Close` asks its window to close, the card shows it,
    /// `Close all` asks every window, anywhere else closes the switcher.
    pub(super) fn phone_switcher_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        damage.add(self.screen());
        let entries = self.phone_switcher_entries();
        if !entries.is_empty() && contains(self.close_all_button(entries.len()), x, y) {
            for slot in entries {
                if self.window_at(slot).closable {
                    self.request_close(slot, events);
                }
            }
            self.phone.switcher = false;
            return Action::None;
        }
        for (index, &slot) in entries.iter().enumerate() {
            let card = self.card(index);
            if contains(Self::card_close(card), x, y) {
                if self.window_at(slot).closable {
                    self.request_close(slot, events);
                }
                return Action::None;
            }
            if contains(card, x, y) {
                self.phone.switcher = false;
                self.activate(slot, damage, events);
                return Action::None;
            }
        }
        self.phone.switcher = false;
        Action::None
    }

    /// A wheel turn over the open switcher scrolls its cards; whether it did.
    pub(super) fn switcher_scroll(&mut self, notches: i32, damage: &mut Damage) -> bool {
        if !self.phone.switcher {
            return false;
        }
        let count = self.phone_switcher_entries().len() as u32;
        let content = count * (CARD.1 + CARD_GAP) + CLOSE_ALL_HEIGHT + 2 * CARD_GAP;
        let most = content.saturating_sub(self.layout.work_area.height);
        let step = notches.unsigned_abs() * WHEEL_STEP;
        self.phone.switcher_scroll = match notches > 0 {
            true => self.phone.switcher_scroll.saturating_sub(step),
            false => (self.phone.switcher_scroll + step).min(most),
        };
        damage.add(self.layout.work_area);
        true
    }

    fn sheet_left(&self) -> u32 {
        match self.form {
            super::Form::Tablet => self.width.saturating_sub(SHADE_WIDTH),
            _ => 0,
        }
    }

    fn tile_rect(&self, index: usize) -> Rect {
        let work = self.layout.work_area;
        let (column, row) = (index as u32 % 2, index as u32 / 2);
        Rect::new(
            self.sheet_left() + SIDE + (TILE.0 + TILE_GAP) * column,
            work.y + TILE_GAP + (TILE.1 + TILE_GAP) * row,
            TILE.0,
            TILE.1,
        )
    }

    fn settings_button(&self) -> Rect {
        let tile = self.tile_rect(TILES.len());
        Rect::new(tile.x, tile.y, 2 * TILE.0 + TILE_GAP, TILE.1)
    }

    fn power_button_rect(&self) -> Rect {
        let above = self.settings_button();
        Rect::new(above.x, above.bottom() + TILE_GAP, above.width, POWER_HEIGHT)
    }

    fn notices_top(&self) -> u32 {
        self.power_button_rect().bottom() + 2 * SIDE
    }

    fn clear_button(&self) -> Rect {
        let power = self.power_button_rect();
        Rect::new(power.right() - CLEAR_WIDTH, self.notices_top() - 14, CLEAR_WIDTH, NOTICE_ROW.0)
    }

    fn notice_row(&self, index: usize) -> Rect {
        let power = self.power_button_rect();
        Rect::new(power.x, self.notices_top() + 36 + NOTICE_ROW.1 * index as u32, power.width, NOTICE_ROW.0)
    }

    fn shade_rect(&self) -> Rect {
        let work = self.layout.work_area;
        let last = match self.phone.notice_count {
            0 => self.notices_top() + 50,
            count => self.notice_row(count - 1).bottom(),
        };
        let width = self.width - self.sheet_left();
        Rect::new(self.sheet_left(), work.y, width, (last + SHEET_END).saturating_sub(work.y).min(work.height))
    }

    fn power_sheet_rect(&self) -> Rect {
        let work = self.layout.work_area;
        let height = POWER_ROW_HEIGHT * POWER_ROWS.len() as u32 + 1;
        Rect::new(0, work.bottom() - height, self.width, height)
    }

    fn tile_state(&self, tile: Tile) -> (&'static str, &str, bool) {
        match tile {
            Tile::Network => ("Network", self.network_indicator_name().unwrap_or("No network"), false),
            Tile::DarkTheme => {
                let dark = self.palette % 2 == 0;
                ("Dark theme", if dark { "On" } else { "Off" }, dark)
            }
            Tile::HighContrast => {
                let high = self.palette >= 2;
                ("High contrast", if high { "On" } else { "Off" }, high)
            }
            Tile::Rotation => match self.phone.rotation {
                (true, _) => ("Rotation", "Automatic", true),
                (false, _) => ("Rotation", "Locked", false),
            },
        }
    }

    /// Tells the shade `display.rotation`'s state (D330): whether it is `auto`, and the rotation
    /// in force by its value's name (`0`, `90`, `180`, `270`), which locking keeps.
    pub fn set_rotation_choice(&mut self, automatic: bool, in_force: &'static str) {
        self.phone.rotation = (automatic, in_force);
    }

    /// A press in the shade: a tile sets its setting or opens its pane, `Settings` opens
    /// Settings, `Power` its sheet, `Clear` forgets the notices; below the sheet closes it.
    pub(super) fn shade_press(&mut self, x: u32, y: u32, damage: &mut Damage) -> Action {
        damage.add(self.screen());
        if self.phone.power_sheet {
            let sheet = self.power_sheet_rect();
            self.phone.power_sheet = false;
            if contains(sheet, x, y) {
                let row = ((y - sheet.y) / POWER_ROW_HEIGHT) as usize;
                self.phone.shade = false;
                return POWER_ROWS.get(row).map_or(Action::None, |(_, action)| *action);
            }
            return Action::None;
        }
        if !contains(self.shade_rect(), x, y) {
            self.phone.shade = false;
            return Action::None;
        }
        for (index, tile) in TILES.into_iter().enumerate() {
            if !contains(self.tile_rect(index), x, y) {
                continue;
            }
            return match tile {
                Tile::Network => {
                    self.phone.shade = false;
                    Action::OpenSettingsPane(SettingsPane::Network)
                }
                Tile::DarkTheme => Action::SetSetting {
                    key: "appearance.theme",
                    value: if self.palette % 2 == 0 { "light" } else { "dark" },
                },
                Tile::HighContrast => Action::SetSetting {
                    key: "accessibility.high_contrast",
                    value: if self.palette >= 2 { "false" } else { "true" },
                },
                Tile::Rotation => Action::SetSetting {
                    key: "display.rotation",
                    value: match self.phone.rotation {
                        (true, in_force) => in_force,
                        (false, _) => "auto",
                    },
                },
            };
        }
        if contains(self.settings_button(), x, y) {
            self.phone.shade = false;
            return Action::Start(AppId::Settings);
        }
        if contains(self.power_button_rect(), x, y) {
            self.phone.power_sheet = true;
            return Action::None;
        }
        if self.phone.notice_count > 0 && contains(self.clear_button(), x, y) {
            self.phone.notice_count = 0;
        }
        Action::None
    }

    /// Draws the open sheet over the home screen or the shown window.
    pub(super) fn draw_phone_overlays(&self, canvas: &mut Canvas<'_>, contents: &dyn Contents) {
        if self.form == super::Form::Tv {
            self.draw_pointer_menu(canvas);
            return;
        }
        if !self.touch_form() {
            return;
        }
        self.draw_apps_glyph(canvas);
        if self.form == super::Form::Tablet {
            self.draw_pair(canvas);
            self.draw_tablet_menu(canvas);
        }
        if self.phone.switcher {
            self.draw_switcher_cards(canvas, contents);
        }
        if self.phone.shade {
            self.draw_shade(canvas);
        }
        if self.phone.power_sheet {
            let sheet = self.power_sheet_rect();
            canvas.dim(Rect::new(0, self.layout.work_area.y, self.width, sheet.y - self.layout.work_area.y));
            canvas.fill(sheet, colour::MENU);
            canvas.fill(Rect::new(sheet.x, sheet.y, sheet.width, 1), colour::MENU_BORDER);
            for (index, (label, _)) in POWER_ROWS.iter().enumerate() {
                let top = sheet.y + 1 + POWER_ROW_HEIGHT * index as u32;
                canvas.text(SIDE, top + 16, label, colour::TEXT);
            }
        }
    }

    fn draw_apps_glyph(&self, canvas: &mut Canvas<'_>) {
        let Some(zone) = self.navigation_zone(2) else {
            return;
        };
        let (side, x, y) = (14, zone.x + (zone.width - 14) / 2, zone.y + (zone.height - 14) / 2);
        let ink = colour::TEXT;
        canvas.fill(Rect::new(x, y, side, 1), ink);
        canvas.fill(Rect::new(x, y + side - 1, side, 1), ink);
        canvas.fill(Rect::new(x, y, 1, side), ink);
        canvas.fill(Rect::new(x + side - 1, y, 1, side), ink);
    }

    fn draw_switcher_cards(&self, canvas: &mut Canvas<'_>, contents: &dyn Contents) {
        let work = self.layout.work_area;
        canvas.fill(work, colour::WINDOW_BACKGROUND);
        let entries = self.phone_switcher_entries();
        if entries.is_empty() {
            let text = "No open apps.";
            let x = (self.width - text.len() as u32 * GLYPH_WIDTH as u32) / 2;
            canvas.text(x, work.y + work.height / 2, text, colour::TEXT_DIM);
            return;
        }
        for (index, &slot) in entries.iter().enumerate() {
            let card = self.card(index);
            if card.bottom() <= work.y || card.y >= work.bottom() {
                continue;
            }
            canvas.rounded(card, &paint::INSET_R4, true, true, colour::MENU);
            let window = self.window_at(slot);
            let room = ((card.width - CARD_TITLE.0 - CLOSE_TARGET) / GLYPH_WIDTH as u32) as usize;
            let (title, cut) = windows::cut_title(window.title(), room.saturating_sub(3));
            canvas.text(card.x + CARD_TITLE.0, card.y + CARD_TITLE.1, title, colour::TEXT);
            if cut {
                let after = card.x + CARD_TITLE.0 + title.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(after, card.y + CARD_TITLE.1, windows::ellipsis(), colour::TEXT);
            }
            let close = Self::card_close(card);
            canvas.rounded(
                Rect::new(
                    close.x + (CLOSE_TARGET - CLOSE_CIRCLE) / 2,
                    close.y + (CLOSE_TARGET - CLOSE_CIRCLE) / 2,
                    CLOSE_CIRCLE,
                    CLOSE_CIRCLE,
                ),
                &paint::INSET_R6,
                true,
                true,
                colour::BUTTON_CLOSE,
            );
            self.draw_thumbnail(canvas, contents, slot, card);
        }
        let all = self.close_all_button(entries.len());
        if all.y < work.bottom() {
            canvas.rounded(all, &paint::INSET_R4, true, true, colour::BUTTON_BORDER);
            canvas.text(all.x + 16, all.y + 14, "Close all", colour::TEXT);
        }
    }

    fn draw_thumbnail(&self, canvas: &mut Canvas<'_>, contents: &dyn Contents, slot: usize, card: Rect) {
        let window = self.window_at(slot);
        let (width, height) = (window.content_width.max(1), window.content_height.max(1));
        let area = Rect::new(card.x + (card.width - THUMBNAIL.1) / 2, card.y + THUMBNAIL.0, THUMBNAIL.1, THUMBNAIL.2);
        let clip = canvas.clip();
        if !area.overlaps(&clip) {
            return;
        }
        let mut row = alloc::vec![0u32; width as usize];
        for y in area.y.max(clip.y)..area.bottom().min(clip.bottom()) {
            let source_y = (y - area.y) * height / THUMBNAIL.2;
            contents.copy_row(window.id, 0, source_y, &mut row);
            for x in area.x.max(clip.x)..area.right().min(clip.right()) {
                let source_x = ((x - area.x) * width / THUMBNAIL.1) as usize;
                canvas.pixel(x, y, row[source_x.min(row.len() - 1)]);
            }
        }
    }

    fn draw_shade(&self, canvas: &mut Canvas<'_>) {
        let sheet = self.shade_rect();
        canvas.dim(Rect::new(0, sheet.bottom(), self.width, self.height.saturating_sub(sheet.bottom())));
        canvas.fill(sheet, colour::MENU);
        canvas.fill(Rect::new(0, sheet.bottom().saturating_sub(1), self.width, 1), colour::MENU_BORDER);
        for (index, tile) in TILES.into_iter().enumerate() {
            let rect = self.tile_rect(index);
            let (title, state, on) = self.tile_state(tile);
            let fill = if on { colour::SELECTION } else { colour::BUTTON_INACTIVE };
            canvas.rounded(rect, &paint::INSET_R4, true, true, fill);
            if on {
                canvas.fill(Rect::new(rect.x, rect.y + 8, MARK.0, MARK.1), colour::ACCENT);
            }
            canvas.text(rect.x + TILE_LINES.0, rect.y + TILE_LINES.1, title, colour::TEXT);
            let ink = if on { colour::TEXT } else { colour::TEXT_DIM };
            canvas.text(rect.x + TILE_LINES.0, rect.y + TILE_LINES.2, state, ink);
        }
        let settings = self.settings_button();
        canvas.rounded(settings, &paint::INSET_R4, true, true, colour::BUTTON_INACTIVE);
        canvas.text(settings.x + TILE_LINES.0, settings.y + 20, "Settings", colour::TEXT);
        let power = self.power_button_rect();
        canvas.rounded(power, &paint::INSET_R4, true, true, colour::BUTTON_INACTIVE);
        canvas.text(power.x + TILE_LINES.0, power.y + 14, "Power", colour::TEXT);
        let left = self.sheet_left() + SIDE;
        canvas.text(left, self.notices_top(), "Notices", colour::TEXT_DIM);
        if self.phone.notice_count == 0 {
            canvas.text(left, self.notices_top() + 50, "No notices.", colour::TEXT_DIM);
            return;
        }
        let clear = self.clear_button();
        canvas.rounded(clear, &paint::INSET_R4, true, true, colour::BUTTON_INACTIVE);
        canvas.text(clear.x + 20, clear.y + 14, "Clear", colour::TEXT);
        for index in 0..self.phone.notice_count {
            let row = self.notice_row(index);
            canvas.rounded(row, &paint::INSET_R4, true, true, colour::BAR);
            let (text, cut) = windows::cut_title(self.phone.notices[index], NOTICE_GLYPHS);
            canvas.text(row.x + 12, row.y + 14, text, colour::TEXT);
            if cut {
                let after = row.x + 12 + text.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(after, row.y + 14, windows::ellipsis(), colour::TEXT);
            }
        }
    }
}
