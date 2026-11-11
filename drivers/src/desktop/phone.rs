//! The phone form's shell, slice 1 (docs/ux/adaptive-shell-v0.5.md sections 2 and 9, D303):
//! the 44 px status bar (clock, the shown window's title, the network glyph, the `Agent` chip),
//! the 48 px navigation bar (`Back`, `Home`, and the `Apps` zone, empty until slice 2), the home
//! screen that is also the launcher (a four-column grid and its search), one full window at a
//! time with no chrome, dialogs as bottom sheets, and `Back`'s rules 2, 3, 5 and 6. The pointer
//! is the finger: a press acts, as a tap will.

use super::catalogue::AppId;
use super::form::Form;
use super::icons::{IconSize, draw_icon};
use super::layout::layout_in_form;
use super::paint::{self, Canvas};
use super::protocol::{EventSink, Role};
use super::windows::{self, Geometry};
use super::{Action, Desktop, MAX_WINDOWS, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;

const QUERY_CAPACITY: usize = 32;
const TEXT_Y: u32 = 14;
const CLOCK_X: u32 = 16;
const TITLE_X: u32 = 72;
const TITLE_GAP: u32 = 8;
const RIGHT_EDGE_FROM_SCREEN: u32 = 16;
const NETWORK_SIDE: u32 = 16;
const ITEM_GAP: u32 = 8;
const AGENT_CHIP: (u32, u32) = (56, 20);
const AGENT_CHIP_Y: u32 = 12;
const AGENT_TEXT_INSET: (u32, u32) = (8, 2);
const SEARCH: (u32, u32, u32) = (16, 12, 44);
const SEARCH_TEXT: (u32, u32) = (12, 14);
const SEARCH_PLACEHOLDER: &str = "Search apps";
const GRID_TOP: u32 = 72;
const GRID_COLUMNS: u32 = 4;
const GRID_LEFT: u32 = 19;
const CELL: (u32, u32) = (88, 104);
const CELL_ICON: (u32, u32) = (20, 8);
const RUNNING_MARK: (u32, u32, u32) = (42, 58, 4);
const LABEL_TOP: u32 = 64;
const LABEL_GLYPHS: usize = 10;
const LABEL_HEIGHT: u32 = 20;
const LABEL_PADDING: u32 = 4;
const ROW_HEIGHT: u32 = 48;
const ROW_ICON: (u32, u32) = (24, 8);
const ROW_TEXT: (u32, u32) = (64, 16);
const WHEEL_STEP: u32 = 48;
const BACK_ARROW: (u32, u32, u32) = (57, 819, 16);
const HOME_MARK: (u32, u32) = (14, 20);
const NAVIGATION_ZONES: u32 = 3;

/// What the person pressed in the phone's shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    StatusBar,
    Back,
    Home,
    Apps,
}

/// The phone shell's own state: the grid's scroll, the search, the pressed zone.
#[derive(Debug, Clone, Copy)]
pub(super) struct PhoneState {
    scroll: u32,
    query: [u8; QUERY_CAPACITY],
    query_length: usize,
    search_focused: bool,
    pressed: Option<Target>,
    pub(super) switcher: bool,
    pub(super) switcher_scroll: u32,
    pub(super) shade: bool,
    pub(super) power_sheet: bool,
    pub(super) notices: [&'static str; super::phone_sheets::NOTICES_KEPT],
    pub(super) notice_count: usize,
    awaiting_back: Option<u8>,
}

impl Default for PhoneState {
    fn default() -> Self {
        Self {
            scroll: 0,
            query: [0; QUERY_CAPACITY],
            query_length: 0,
            search_focused: false,
            pressed: None,
            switcher: false,
            switcher_scroll: 0,
            shade: false,
            power_sheet: false,
            notices: [""; super::phone_sheets::NOTICES_KEPT],
            notice_count: 0,
            awaiting_back: None,
        }
    }
}

impl PhoneState {
    fn query(&self) -> &str {
        core::str::from_utf8(&self.query[..self.query_length]).unwrap_or("")
    }
}

impl Desktop {
    /// The form the shell has now (D303).
    pub fn form(&self) -> Form {
        self.form
    }

    /// Makes `form` the shell's: the panels and the work area follow it, the shell's popups
    /// close, and every window keeps its state. Entering the phone form shows the focused
    /// window full and minimises the others; leaving it gives maximised windows the new work
    /// area. The whole screen is drawn again.
    pub fn set_form(&mut self, form: Form, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.form == form {
            return;
        }
        self.close_popups(damage, events);
        self.close_notice(damage);
        let leaving_phone = self.form == Form::Phone;
        self.form = form;
        let notices = (self.phone.notices, self.phone.notice_count);
        self.phone = PhoneState::default();
        (self.phone.notices, self.phone.notice_count) = notices;
        self.layout = layout_in_form(self.width, self.height, 1, self.profile, form);
        self.reset_panels();
        match form {
            Form::Phone => {
                let shown = self.focused_normal();
                for slot in 0..self.windows.len() {
                    let Some(window) = self.windows[slot] else {
                        continue;
                    };
                    if !window.shown || window.minimised {
                        continue;
                    }
                    match (window.role, Some(slot) == shown) {
                        (Role::Normal, true) | (Role::Dialog, _) => self.fit_to_phone(slot, damage, events),
                        (Role::Normal, false) => self.minimise(slot, damage, events),
                        _ => {}
                    }
                }
            }
            _ if leaving_phone => self.refit_windows(damage, events),
            _ => self.refit_windows(damage, events),
        }
        damage.add(self.screen());
        self.update_pointer(damage);
    }

    fn phone_geometry(&self, slot: usize) -> Geometry {
        let work = self.layout.work_area;
        let window = self.window_at(slot);
        let (top, height) = match window.role {
            Role::Dialog => {
                let height = window.content_height.min(work.height);
                (work.bottom() - height, height)
            }
            _ => (work.y, work.height),
        };
        Geometry {
            x: work.x as i32 - 1,
            y: top.saturating_sub(windows::CHROME.1 - 1),
            content_width: work.width,
            content_height: height,
        }
    }

    /// Gives window `slot` its place in the phone form: a normal window the whole work area,
    /// the others it covered minimised; a dialog the bottom of it, full width (section 2.5).
    pub(super) fn fit_to_phone(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        let old = self.window_at(slot).with_shadow();
        let geometry = self.phone_geometry(slot);
        self.window_mut(slot).maximised = true;
        self.apply_geometry(slot, geometry, old, damage, events);
        if self.window_at(slot).role != Role::Normal {
            return;
        }
        for other in 0..self.windows.len() {
            if other == slot {
                continue;
            }
            let Some(window) = self.windows[other] else {
                continue;
            };
            if window.shown && !window.minimised && window.role == Role::Normal {
                self.minimise(other, damage, events);
            }
        }
    }

    /// The normal window the phone shows, if any: none means the home screen.
    pub fn phone_shown_window(&self) -> Option<usize> {
        self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&slot| usize::from(slot))
            .find(|&slot| self.is_visible(slot) && self.window_at(slot).role == Role::Normal)
    }

    fn phone_dialog(&self) -> Option<usize> {
        let shown = self.phone_shown_window()?;
        self.top_dialog(shown)
    }

    fn navigation_zone(&self, index: u32) -> Option<Rect> {
        let bar = self.layout.navigation_bar?;
        let width = bar.width / NAVIGATION_ZONES;
        Some(Rect::new(bar.x + width * index, bar.y, width, bar.height))
    }

    fn search_field(&self) -> Rect {
        let work = self.layout.work_area;
        Rect::new(SEARCH.0, work.y + SEARCH.1, work.width.saturating_sub(2 * SEARCH.0), SEARCH.2)
    }

    fn grid_area(&self) -> Rect {
        let work = self.layout.work_area;
        Rect::new(work.x, work.y + GRID_TOP, work.width, work.height.saturating_sub(GRID_TOP))
    }

    fn cell(&self, index: usize) -> Rect {
        let area = self.grid_area();
        let (column, row) = (index as u32 % GRID_COLUMNS, index as u32 / GRID_COLUMNS);
        Rect::new(
            GRID_LEFT + CELL.0 * column,
            (area.y + CELL.1 * row).saturating_sub(self.phone.scroll),
            CELL.0,
            CELL.1,
        )
    }

    fn matching_apps(&self) -> impl Iterator<Item = AppId> + '_ {
        let query = self.phone.query();
        self.catalogue
            .desktop_apps()
            .filter(move |app| query.is_empty() || self.catalogue.matches(*app, query))
    }

    fn search_row(&self, index: usize) -> Rect {
        let area = self.grid_area();
        Rect::new(area.x, area.y + ROW_HEIGHT * index as u32, area.width, ROW_HEIGHT)
    }

    fn most_scroll(&self) -> u32 {
        let rows = (self.catalogue.desktop_apps().count() as u32).div_ceil(GRID_COLUMNS);
        let content = rows * CELL.1 + 16;
        content.saturating_sub(self.grid_area().height)
    }

    /// `Home`: the shown window is minimised and the home screen shows; on the home screen the
    /// grid goes back to the top and the search empties (section 2.5, `wm home`).
    pub fn phone_home(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.close_phone_overlays() {
            damage.add(self.screen());
        }
        match self.phone_shown_window() {
            Some(slot) => self.minimise(slot, damage, events),
            None => {
                self.phone.scroll = 0;
                self.phone.query_length = 0;
                self.phone.search_focused = false;
            }
        }
        damage.add(self.screen());
    }

    /// `Back` (section 2.6, rules 2, 3, 5 and 6; `wm back`): empties the search, then takes the
    /// focus from it; asks the shown window's dialog to close; minimises the shown window;
    /// on the home screen with nothing open, does nothing. Returns whether it did something.
    pub fn phone_back(&mut self, damage: &mut Damage, events: &mut dyn EventSink) -> bool {
        damage.add(self.screen());
        if self.close_phone_overlays() {
            return true;
        }
        if let Some(dialog) = self.phone_dialog() {
            self.request_close(dialog, events);
            return true;
        }
        if let Some(slot) = self.phone_shown_window() {
            let handles_back = self.window_at(slot).flags & super::protocol::flags::HANDLES_BACK != 0;
            if handles_back && self.phone.awaiting_back != Some(slot as u8) {
                self.phone.awaiting_back = Some(slot as u8);
                self.send_back_key(slot, events);
                return true;
            }
            self.phone.awaiting_back = None;
            self.minimise(slot, damage, events);
            return true;
        }
        if self.phone.query_length > 0 {
            self.phone.query_length = 0;
            return true;
        }
        if self.phone.search_focused {
            self.phone.search_focused = false;
            return true;
        }
        false
    }

    /// Whether `Back` would do something now, so its glyph is `text`, not `text.dim`.
    pub fn phone_can_go_back(&self) -> bool {
        self.phone_overlay_open()
            || self.phone_shown_window().is_some()
            || self.phone.query_length > 0
            || self.phone.search_focused
    }

    /// The shown window did not use the `Back` key it was sent (`BackUnused`, rule 4 of
    /// section 2.6): the shell goes back past it, minimising it.
    pub fn back_unused(&mut self, client: u16, window: u32, damage: &mut Damage, events: &mut dyn EventSink) {
        let Ok(id) = super::WindowId::try_from(window) else {
            return;
        };
        let Some(slot) = self.find(id).filter(|&slot| self.window_at(slot).client == client) else {
            return;
        };
        if self.form != Form::Phone || self.phone.awaiting_back != Some(slot as u8) {
            return;
        }
        self.phone.awaiting_back = None;
        self.minimise(slot, damage, events);
        damage.add(self.screen());
    }

    /// A press at (`x`, `y`) in the phone form: the shell's when it is on a bar, the home
    /// screen or the darkened rows above a sheet. `None` leaves it to the windows.
    pub(super) fn phone_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if self.layout.status_bar.is_some_and(|bar| contains(bar, x, y)) {
            self.phone.pressed = Some(Target::StatusBar);
            let open = !self.phone.shade;
            self.close_phone_overlays();
            self.phone.shade = open;
            damage.add(self.screen());
            return Some(Action::None);
        }
        let on_bar = self.layout.navigation_bar.is_some_and(|bar| contains(bar, x, y));
        if self.phone.shade && !on_bar {
            return Some(self.shade_press(x, y, damage));
        }
        if self.phone.switcher && !on_bar {
            return Some(self.switcher_press(x, y, damage, events));
        }
        if self.layout.navigation_bar.is_some_and(|bar| contains(bar, x, y)) {
            let zone = (0..NAVIGATION_ZONES).find(|&index| self.navigation_zone(index).is_some_and(|zone| contains(zone, x, y)))?;
            if let Some(rect) = self.layout.navigation_bar {
                damage.add(rect);
            }
            match zone {
                0 => {
                    self.phone.pressed = Some(Target::Back);
                    self.phone_back(damage, events);
                }
                1 => {
                    self.phone.pressed = Some(Target::Home);
                    self.phone_home(damage, events);
                }
                _ => {
                    self.phone.pressed = Some(Target::Apps);
                    let open = !self.phone.switcher;
                    self.close_phone_overlays();
                    self.phone.switcher = open;
                    self.phone.switcher_scroll = 0;
                    damage.add(self.screen());
                }
            }
            return Some(Action::None);
        }
        if let Some(dialog) = self.phone_dialog() {
            let sheet = self.window_at(dialog).content();
            if y < sheet.y {
                self.request_close(dialog, events);
                return Some(Action::None);
            }
            return None;
        }
        if self.phone_shown_window().is_some() {
            return None;
        }
        damage.add(self.layout.work_area);
        if contains(self.search_field(), x, y) {
            self.phone.search_focused = true;
            return Some(Action::None);
        }
        self.phone.search_focused = self.phone.query_length > 0;
        let chosen = match self.phone.query_length {
            0 => self
                .catalogue
                .desktop_apps()
                .enumerate()
                .find(|(index, _)| contains(self.cell(*index), x, y) && contains(self.grid_area(), x, y))
                .map(|(_, app)| app),
            _ => self
                .matching_apps()
                .enumerate()
                .find(|(index, _)| contains(self.search_row(*index), x, y))
                .map(|(_, app)| app),
        };
        let app = chosen?;
        self.phone.query_length = 0;
        self.phone.search_focused = false;
        Some(self.phone_open(app, damage, events))
    }

    fn phone_open(&mut self, app: AppId, damage: &mut Damage, events: &mut dyn EventSink) -> Action {
        let mut windows = [0u8; MAX_WINDOWS];
        match self.app_windows(app, &mut windows) {
            0 => self.launch(app, damage, events),
            _ => self.activate_app(app, damage, events),
        }
    }

    /// The left button went up: a pressed bar zone is drawn as before.
    pub(super) fn phone_release(&mut self, damage: &mut Damage) {
        if self.phone.pressed.take().is_some() {
            for bar in [self.layout.status_bar, self.layout.navigation_bar].into_iter().flatten() {
                damage.add(bar);
            }
        }
    }

    /// A wheel turn on the phone's home grid scrolls it, not the bars; returns whether it
    /// did, so the wheel goes to a window otherwise.
    pub fn home_scroll(&mut self, notches: i32, damage: &mut Damage) -> bool {
        if self.form == Form::Phone && self.switcher_scroll(notches, damage) {
            return true;
        }
        if self.form != Form::Phone || self.phone_shown_window().is_some() || self.phone.query_length > 0 {
            return false;
        }
        let step = notches.unsigned_abs() * WHEEL_STEP;
        self.phone.scroll = match notches > 0 {
            true => self.phone.scroll.saturating_sub(step),
            false => (self.phone.scroll + step).min(self.most_scroll()),
        };
        damage.add(self.grid_area());
        true
    }

    /// Key `code` on the home screen: Esc is `Back`, typing goes to the search, Backspace
    /// takes a character back, Enter opens the first match. Returns the action when it was
    /// the shell's.
    pub(super) fn phone_key(&mut self, code: u16, damage: &mut Damage, events: &mut dyn EventSink) -> Option<Action> {
        if self.phone_shown_window().is_some() {
            return None;
        }
        damage.add(self.layout.work_area);
        match code {
            key::ESC => {
                self.phone_back(damage, events);
                Some(Action::None)
            }
            key::BACKSPACE => {
                self.phone.query_length = self.phone.query_length.saturating_sub(1);
                Some(Action::None)
            }
            key::ENTER | key::KEYPAD_ENTER => {
                let app = self.matching_apps().next().filter(|_| self.phone.query_length > 0)?;
                self.phone.query_length = 0;
                self.phone.search_focused = false;
                Some(self.phone_open(app, damage, events))
            }
            _ => {
                let character = self.typed;
                if !(0x20..0x7f).contains(&character) || self.phone.query_length == QUERY_CAPACITY {
                    return None;
                }
                self.phone.query[self.phone.query_length] = character as u8;
                self.phone.query_length += 1;
                self.phone.search_focused = true;
                Some(Action::None)
            }
        }
    }

    /// Draws the home screen over the wallpaper while no window is shown.
    pub(super) fn draw_phone_home(&self, canvas: &mut Canvas<'_>) {
        if self.phone_shown_window().is_some() {
            return;
        }
        let field = self.search_field();
        canvas.rounded(field, &paint::INSET_R4, true, true, colour::BUTTON_BORDER);
        canvas.rounded(
            Rect::new(field.x + 1, field.y + 1, field.width - 2, field.height - 2),
            &paint::INSET_R4,
            true,
            true,
            colour::FIELD,
        );
        let (text, ink) = match self.phone.query_length {
            0 => (SEARCH_PLACEHOLDER, colour::TEXT_DIM),
            _ => (self.phone.query(), colour::TEXT),
        };
        canvas.text(field.x + SEARCH_TEXT.0, field.y + SEARCH_TEXT.1, text, ink);
        if self.phone.search_focused {
            let caret_x = field.x + SEARCH_TEXT.0 + self.phone.query_length as u32 * GLYPH_WIDTH as u32;
            canvas.fill(Rect::new(caret_x, field.y + SEARCH_TEXT.1, 1, GLYPH_HEIGHT as u32), colour::TEXT);
        }
        let area = self.grid_area();
        if self.phone.query_length > 0 {
            for (index, app) in self.matching_apps().enumerate() {
                let row = self.search_row(index);
                if row.bottom() > area.bottom() {
                    break;
                }
                if index == 0 {
                    canvas.fill(row, colour::SELECTION);
                }
                draw_icon(canvas, self.catalogue.icon(app), IconSize::Small, row.x + ROW_ICON.0, row.y + ROW_ICON.1);
                canvas.text(row.x + ROW_TEXT.0, row.y + ROW_TEXT.1, self.catalogue.name(app), colour::TEXT);
            }
            return;
        }
        for (index, app) in self.catalogue.desktop_apps().enumerate() {
            let cell = self.cell(index);
            if cell.y < area.y || cell.bottom() > area.bottom() {
                continue;
            }
            draw_icon(canvas, self.catalogue.icon(app), IconSize::Large, cell.x + CELL_ICON.0, cell.y + CELL_ICON.1);
            let mut windows = [0u8; MAX_WINDOWS];
            if self.app_windows(app, &mut windows) > 0 {
                canvas.fill(
                    Rect::new(cell.x + RUNNING_MARK.0, cell.y + RUNNING_MARK.1, RUNNING_MARK.2, RUNNING_MARK.2),
                    colour::TEXT,
                );
            }
            let (text, cut) = windows::cut_title(self.catalogue.name(app), LABEL_GLYPHS);
            let glyphs = text.chars().count() + if cut { windows::ellipsis().len() } else { 0 };
            let width = glyphs as u32 * GLYPH_WIDTH as u32 + 2 * LABEL_PADDING;
            let pill = Rect::new(cell.x + CELL.0.saturating_sub(width) / 2, cell.y + LABEL_TOP, width, LABEL_HEIGHT);
            canvas.rounded(pill, &paint::INSET_R4, true, true, colour::BAR);
            canvas.text(pill.x + LABEL_PADDING, pill.y + 2, text, colour::TEXT);
            if cut {
                canvas.text(
                    pill.x + LABEL_PADDING + text.chars().count() as u32 * GLYPH_WIDTH as u32,
                    pill.y + 2,
                    windows::ellipsis(),
                    colour::TEXT,
                );
            }
        }
    }

    /// Draws the phone's two bars.
    pub(super) fn draw_phone_bars(&self, canvas: &mut Canvas<'_>) {
        if let Some(bar) = self.layout.status_bar
            && bar.overlaps(&canvas.clip())
        {
            self.draw_status_bar(canvas, bar);
        }
        if let Some(bar) = self.layout.navigation_bar
            && bar.overlaps(&canvas.clip())
        {
            self.draw_navigation_bar(canvas, bar);
        }
    }

    fn draw_status_bar(&self, canvas: &mut Canvas<'_>, bar: Rect) {
        let fill = match self.phone.pressed {
            Some(Target::StatusBar) => colour::BAR_PRESSED,
            _ => colour::BAR,
        };
        canvas.fill(Rect::new(bar.x, bar.y, bar.width, bar.height - 1), fill);
        canvas.fill(Rect::new(bar.x, bar.bottom() - 1, bar.width, 1), colour::BAR_EDGE);
        let ink = match self.clock_shows_time_of_day() {
            true => colour::TEXT,
            false => colour::TEXT_DIM,
        };
        canvas.text(CLOCK_X, TEXT_Y, self.clock_text(), ink);
        let mut right = bar.width.saturating_sub(RIGHT_EDGE_FROM_SCREEN);
        if let Some(state) = self.network.state {
            let x = right - NETWORK_SIDE;
            for rect in state.glyph().into_iter().flatten() {
                canvas.fill(
                    Rect::new(
                        x + u32::from(rect.x),
                        TEXT_Y + u32::from(rect.y),
                        u32::from(rect.width),
                        u32::from(rect.height),
                    ),
                    colour::TEXT,
                );
            }
            right = x - ITEM_GAP;
        }
        if self.agent_connected() {
            let chip = Rect::new(right - AGENT_CHIP.0, AGENT_CHIP_Y, AGENT_CHIP.0, AGENT_CHIP.1);
            let (fill, text) = match self.agent_acting() {
                true => (colour::ACCENT, colour::ICON_DARK),
                false => (colour::BAR_HOVER, colour::TEXT_DIM),
            };
            canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
            canvas.text(chip.x + AGENT_TEXT_INSET.0, chip.y + AGENT_TEXT_INSET.1, super::AGENT_LABEL, text);
            right = chip.x;
        }
        if let Some(slot) = self.phone_shown_window() {
            let room = (right.saturating_sub(TITLE_GAP + TITLE_X) / GLYPH_WIDTH as u32) as usize;
            let (text, cut) = windows::cut_title(self.window_at(slot).title(), room.saturating_sub(3));
            let (text, cut) = match self.window_at(slot).title().chars().count() <= room {
                true => (self.window_at(slot).title(), false),
                false => (text, cut),
            };
            canvas.text(TITLE_X, TEXT_Y, text, colour::TEXT);
            if cut {
                canvas.text(
                    TITLE_X + text.chars().count() as u32 * GLYPH_WIDTH as u32,
                    TEXT_Y,
                    windows::ellipsis(),
                    colour::TEXT,
                );
            }
        }
    }

    fn draw_navigation_bar(&self, canvas: &mut Canvas<'_>, bar: Rect) {
        canvas.fill(Rect::new(bar.x, bar.y + 1, bar.width, bar.height - 1), colour::BAR);
        canvas.fill(Rect::new(bar.x, bar.y, bar.width, 1), colour::BAR_EDGE);
        let pressed = match self.phone.pressed {
            Some(Target::Back) => Some(0),
            Some(Target::Home) => Some(1),
            _ => None,
        };
        if let Some(zone) = pressed.and_then(|index| self.navigation_zone(index)) {
            canvas.fill(Rect::new(zone.x, zone.y + 1, zone.width, zone.height - 1), colour::BAR_PRESSED);
        }
        let ink = match self.phone_can_go_back() {
            true => colour::TEXT,
            false => colour::TEXT_DIM,
        };
        let (x, y, length) = (BACK_ARROW.0, bar.y + (BACK_ARROW.1 - 796), BACK_ARROW.2);
        canvas.fill(Rect::new(x, y, length, 2), ink);
        for step in 1..=5u32 {
            canvas.fill(Rect::new(x + step, y.saturating_sub(step), 2, 2), ink);
            canvas.fill(Rect::new(x + step, y + step, 2, 2), ink);
        }
        if let Some(home) = self.navigation_zone(1) {
            let left = home.x + (home.width - HOME_MARK.0) / 2 - 1;
            let top = home.y + (home.height - HOME_MARK.1) / 2;
            Self::draw_egg_mark(canvas, left, top, HOME_MARK.1, self.width);
        }
    }

    /// Draws window `window`'s phone look: its content only, and a sheet's top edge over the
    /// darkened rows above it.
    pub(super) fn draw_phone_window_frame(&self, canvas: &mut Canvas<'_>, window: &windows::Window) {
        if window.role != Role::Dialog {
            return;
        }
        let content = window.content();
        let work = self.layout.work_area;
        canvas.dim(Rect::new(work.x, work.y, work.width, content.y.saturating_sub(work.y)));
    }

    /// `wm move` and `wm size` in the phone form: refused with `fixed_by_form`.
    pub fn form_fixes_windows(&self) -> bool {
        self.form.fixes_windows()
    }
}
