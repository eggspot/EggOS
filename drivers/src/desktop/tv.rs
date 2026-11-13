//! The TV form, slice 4 (docs/ux/adaptive-shell-tv-v0.5.md, D307): no panels; the home screen
//! is a top strip (the egg, `Settings`, `Power`, the clock, the network glyph, the `Agent`
//! chip) and rows of big tiles (`Open`, `Apps`, `Settings`) driven by a focus the remote's
//! five keys move: arrows, OK, Back, Home, Menu. Apps fill the screen without chrome.

use super::catalogue::AppId;
use super::form::Form;
use super::icons::{IconSize, draw_icon};
use super::paint::{self, Canvas};
use super::protocol::EventSink;
use super::windows;
use super::{Action, Contents, Desktop, MAX_WINDOWS, SettingsPane, colour};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use crate::input::key;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The overscan margins every TV element stays inside.
pub const TV_MARGIN_X: u32 = 48;
pub const TV_MARGIN_Y: u32 = 28;

const STRIP_BUTTONS: [(&str, u32, u32); 2] = [("Settings", 80, 96), ("Power", 184, 212)];
const STRIP_BUTTON: (u32, u32, u32) = (36, 96, 28);
const EGG: (u32, u32, u32, u32) = (48, 40, 14, 20);
const STRIP_TEXT_Y: u32 = 42;
const ROWS_TOP: u32 = 96;
const ROW_PITCH: u32 = 156;
const TILE: (u32, u32) = (144, 108);
const TILE_PITCH: u32 = 160;
const TILE_TOP: u32 = 24;
const TILE_ICON: (u32, u32) = (48, 16);
const TILE_RUNNING: (u32, u32, u32) = (70, 68, 4);
const THUMBNAIL: (u32, u32, u32, u32) = (8, 8, 128, 72);
const LABEL_GLYPHS: usize = 16;
const MENU_WIDTH: u32 = 320;
const MENU_ROW: u32 = 32;
const RING: u32 = 2;
const PANES: [(&str, &str); 12] = [
    ("appearance", "Appearance"),
    ("network", "Network"),
    ("display", "Display"),
    ("keyboard", "Keyboard"),
    ("mouse", "Mouse"),
    ("accessibility", "Accessibility"),
    ("storage", "Storage"),
    ("date-and-time", "Date & time"),
    ("agents", "Agents"),
    ("windows", "Windows"),
    ("components", "Components"),
    ("about", "About"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Focus {
    Strip(u8),
    #[default]
    Tiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Open,
    Apps,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileKind {
    Window(usize),
    App(AppId),
    Pane(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Menu {
    Tile(TileKind, usize),
    Power(usize),
}

/// The TV's own state: the focus, each row's remembered tile and scroll, the view's scroll,
/// the open menu; for apps driven by a pointer, its place, which windows are in keys mode and
/// the pointer menu's row.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TvState {
    focus: Focus,
    row: usize,
    tile: [usize; 3],
    scroll: [usize; 3],
    view: usize,
    menu: Option<Menu>,
    pointer: Option<(u32, u32)>,
    keys_mode: u64,
    pointer_menu: Option<usize>,
}

const POINTER_ROWS: usize = 5;
const POINTER_STEP: u32 = 8;
const SCROLL_NOTCHES: i32 = 3;

impl Desktop {
    /// Whether windows fill the work area without chrome: the touch forms and the TV.
    pub(super) fn fills_form(&self) -> bool {
        matches!(self.form, Form::Phone | Form::Tablet | Form::Tv)
    }

    fn tv_rows(&self) -> Vec<(RowKind, Vec<TileKind>)> {
        let mut rows = Vec::new();
        let mut order = [0u8; MAX_WINDOWS];
        let count = self.switcher_order(&mut order);
        let open: Vec<TileKind> = order[..count]
            .iter()
            .map(|&slot| TileKind::Window(usize::from(slot)))
            .collect();
        if !open.is_empty() {
            rows.push((RowKind::Open, open));
        }
        rows.push((
            RowKind::Apps,
            self.catalogue.desktop_apps().map(TileKind::App).collect(),
        ));
        rows.push((
            RowKind::Settings,
            (0..PANES.len()).map(TileKind::Pane).collect(),
        ));
        rows
    }

    fn row_title(kind: RowKind) -> &'static str {
        match kind {
            RowKind::Open => "Open",
            RowKind::Apps => "Apps",
            RowKind::Settings => "Settings",
        }
    }

    fn row_index(kind: RowKind) -> usize {
        match kind {
            RowKind::Open => 0,
            RowKind::Apps => 1,
            RowKind::Settings => 2,
        }
    }

    fn tiles_fit(&self) -> usize {
        ((self.width.saturating_sub(80)) / TILE_PITCH) as usize
    }

    fn tile_label(&self, tile: TileKind) -> String {
        match tile {
            TileKind::Window(slot) => String::from(self.window_at(slot).title()),
            TileKind::App(app) => String::from(self.catalogue.name(app)),
            TileKind::Pane(index) => String::from(PANES[index].1),
        }
    }

    /// Draws the TV's home screen while no window is shown.
    pub(super) fn draw_tv_home(&self, canvas: &mut Canvas<'_>, contents: &dyn Contents) {
        if self.phone_shown_window().is_some() {
            return;
        }
        let ink_rest = colour::TEXT_DIM;
        Self::draw_egg_mark(canvas, EGG.0, EGG.1, EGG.3, self.width);
        for (index, (label, x, text_x)) in STRIP_BUTTONS.into_iter().enumerate() {
            let rect = Rect::new(x, STRIP_BUTTON.0, STRIP_BUTTON.1, STRIP_BUTTON.2);
            let focused = self.tv.focus == Focus::Strip(index as u8) && self.tv.menu.is_none();
            self.draw_tv_panel(canvas, rect, focused, paint::INSET_R4.as_slice());
            canvas.text(
                text_x,
                STRIP_TEXT_Y,
                label,
                if focused { colour::TEXT } else { ink_rest },
            );
        }
        let clock = self.clock_text();
        let clock_x = self.width - TV_MARGIN_X - clock.len() as u32 * GLYPH_WIDTH as u32;
        canvas.text(clock_x, STRIP_TEXT_Y, clock, colour::TEXT);
        let rows = self.tv_rows();
        let view_bottom = self.height - TV_MARGIN_Y;
        for (visible, (kind, tiles)) in rows.iter().enumerate().skip(self.tv.view) {
            let row_top = ROWS_TOP + ROW_PITCH * (visible - self.tv.view) as u32;
            if row_top >= view_bottom {
                break;
            }
            let title = Self::row_title(*kind);
            let pill = Rect::new(
                TV_MARGIN_X,
                row_top - 2,
                title.len() as u32 * GLYPH_WIDTH as u32 + 8,
                20,
            );
            canvas.rounded(pill, &paint::INSET_R4, true, true, colour::BAR);
            canvas.text(TV_MARGIN_X + 4, row_top, title, colour::TEXT_DIM);
            let focused_row = self.tv.focus == Focus::Tiles && self.tv.row == visible;
            let row_slot = Self::row_index(*kind);
            if focused_row && tiles.len() > self.tiles_fit() {
                let position = format!("{} of {}", self.tv.tile[row_slot] + 1, tiles.len());
                let width = position.len() as u32 * GLYPH_WIDTH as u32 + 8;
                let x = self.width - TV_MARGIN_X - width;
                canvas.rounded(
                    Rect::new(x, row_top - 2, width, 20),
                    &paint::INSET_R4,
                    true,
                    true,
                    colour::BAR,
                );
                canvas.text(x + 4, row_top, &position, colour::TEXT_DIM);
            }
            let offset = self.tv.scroll[row_slot];
            for (index, tile) in tiles.iter().enumerate().skip(offset) {
                let x = TV_MARGIN_X + TILE_PITCH * (index - offset) as u32;
                if x >= self.width - TV_MARGIN_X {
                    break;
                }
                let rect = Rect::new(x, row_top + TILE_TOP, TILE.0, TILE.1);
                let focused =
                    focused_row && self.tv.tile[row_slot] == index && self.tv.menu.is_none();
                self.draw_tv_panel(canvas, rect, focused, paint::INSET_R6.as_slice());
                match tile {
                    TileKind::Window(slot) => self.draw_tv_thumbnail(canvas, contents, *slot, rect),
                    TileKind::App(app) => {
                        draw_icon(
                            canvas,
                            self.catalogue.icon(*app),
                            IconSize::Large,
                            x + TILE_ICON.0,
                            rect.y + TILE_ICON.1,
                        );
                        let mut windows = [0u8; MAX_WINDOWS];
                        if self.app_windows(*app, &mut windows) > 0 {
                            canvas.fill(
                                Rect::new(
                                    x + TILE_RUNNING.0,
                                    rect.y + TILE_RUNNING.1,
                                    TILE_RUNNING.2,
                                    TILE_RUNNING.2,
                                ),
                                colour::TEXT,
                            );
                        }
                    }
                    TileKind::Pane(_) => draw_icon(
                        canvas,
                        self.catalogue.icon(AppId::Settings),
                        IconSize::Large,
                        x + TILE_ICON.0,
                        rect.y + TILE_ICON.1,
                    ),
                }
                let whole =
                    rect.right() <= self.width - TV_MARGIN_X && rect.bottom() <= view_bottom;
                if whole {
                    let label = self.tile_label(*tile);
                    let (text, cut) = windows::cut_title(&label, LABEL_GLYPHS - 3);
                    let glyphs = text.chars().count() + if cut { 3 } else { 0 };
                    let label_x = x + (TILE.0 - glyphs as u32 * GLYPH_WIDTH as u32) / 2;
                    let top = rect.y
                        + if matches!(tile, TileKind::Window(_)) {
                            84
                        } else {
                            76
                        };
                    let ink = if focused { colour::TEXT } else { ink_rest };
                    canvas.text(label_x, top, text, ink);
                    if cut {
                        canvas.text(
                            label_x + text.chars().count() as u32 * GLYPH_WIDTH as u32,
                            top,
                            windows::ellipsis(),
                            ink,
                        );
                    }
                }
            }
        }
        self.draw_tv_menu(canvas);
    }

    fn draw_tv_panel(&self, canvas: &mut Canvas<'_>, rect: Rect, focused: bool, corners: &[u32]) {
        match focused {
            true => {
                canvas.rounded(rect, corners, true, true, colour::FOCUS_RING);
                canvas.rounded(
                    Rect::new(
                        rect.x + RING,
                        rect.y + RING,
                        rect.width - 2 * RING,
                        rect.height - 2 * RING,
                    ),
                    corners,
                    true,
                    true,
                    colour::SELECTION,
                );
            }
            false => {
                canvas.rounded(rect, corners, true, true, colour::MENU_BORDER);
                canvas.rounded(
                    Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
                    corners,
                    true,
                    true,
                    colour::DOCK,
                );
            }
        }
    }

    fn draw_tv_thumbnail(
        &self,
        canvas: &mut Canvas<'_>,
        contents: &dyn Contents,
        slot: usize,
        tile: Rect,
    ) {
        let window = self.window_at(slot);
        let (width, height) = (window.content_width.max(1), window.content_height.max(1));
        let area = Rect::new(
            tile.x + THUMBNAIL.0,
            tile.y + THUMBNAIL.1,
            THUMBNAIL.2,
            THUMBNAIL.3,
        );
        let clip = canvas.clip();
        if !area.overlaps(&clip) {
            return;
        }
        let mut row = alloc::vec![0u32; width as usize];
        for y in area.y.max(clip.y)..area.bottom().min(clip.bottom()) {
            contents.copy_row(window.id, 0, (y - area.y) * height / THUMBNAIL.3, &mut row);
            for x in area.x.max(clip.x)..area.right().min(clip.right()) {
                let source = ((x - area.x) * width / THUMBNAIL.2) as usize;
                canvas.pixel(x, y, row[source.min(row.len() - 1)]);
            }
        }
    }

    fn menu_rows(menu: Menu, closable: bool, uninstallable: bool) -> Vec<&'static str> {
        match menu {
            Menu::Power(_) => alloc::vec!["Power off", "Restart"],
            Menu::Tile(TileKind::Window(_), _) if closable => alloc::vec!["Open", "Close"],
            Menu::Tile(TileKind::App(_), _) if uninstallable => alloc::vec!["Open", "Uninstall..."],
            Menu::Tile(..) => alloc::vec!["Open"],
        }
    }

    fn tv_menu_rows(&self, menu: Menu) -> Vec<&'static str> {
        let (closable, uninstallable) = match menu {
            Menu::Tile(TileKind::Window(slot), _) => (self.window_at(slot).closable, false),
            Menu::Tile(TileKind::App(app), _) => (false, matches!(app, AppId::Installed(_))),
            _ => (false, false),
        };
        Self::menu_rows(menu, closable, uninstallable)
    }

    fn draw_tv_menu(&self, canvas: &mut Canvas<'_>) {
        let Some(menu) = self.tv.menu else {
            return;
        };
        let rows = self.tv_menu_rows(menu);
        let focused = match menu {
            Menu::Tile(_, row) | Menu::Power(row) => row,
        };
        let height = MENU_ROW * rows.len() as u32 + 8;
        let rect = Rect::new(
            (self.width - MENU_WIDTH) / 2,
            (self.height - height) / 2,
            MENU_WIDTH,
            height,
        );
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_R6,
            true,
            true,
            colour::MENU,
        );
        for (index, label) in rows.iter().enumerate() {
            let top = rect.y + 4 + MENU_ROW * index as u32;
            if index == focused {
                let row = Rect::new(rect.x + 1, top, rect.width - 2, MENU_ROW);
                canvas.fill(row, colour::FOCUS_RING);
                canvas.fill(
                    Rect::new(
                        row.x + RING,
                        row.y + RING,
                        row.width - 2 * RING,
                        row.height - 2 * RING,
                    ),
                    colour::SELECTION,
                );
                canvas.text(rect.x + 8, top + 8, ">", colour::TEXT);
            }
            canvas.text(rect.x + 24, top + 8, label, colour::TEXT);
        }
    }

    /// Key `code` in the TV form (section 3.1): the shell's keys on the home screen and in
    /// its menus, and Home and Back over an app. Returns the action when it was the shell's.
    pub(super) fn tv_key(
        &mut self,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if self.form != Form::Tv {
            return None;
        }
        if code == key::HOME_SCREEN {
            damage.add(self.screen());
            self.tv.menu = None;
            if let Some(slot) = self.phone_shown_window() {
                self.minimise(slot, damage, events);
            }
            self.tv.focus = Focus::Tiles;
            self.tv.row = 0;
            self.tv.view = 0;
            self.tv.tile = [0; 3];
            self.tv.scroll = [0; 3];
            return Some(Action::None);
        }
        if let Some(slot) = self.phone_shown_window() {
            let window = self.window_at(slot);
            let pointer_app = window.flags & super::protocol::flags::FOCUS_NAVIGABLE == 0
                && window.client != super::KERNEL_CLIENT;
            if pointer_app {
                return self.pointer_app_key(slot, code, damage, events);
            }
            if code == key::BACK {
                self.phone_back(damage, events);
                return Some(Action::None);
            }
            return None;
        }
        damage.add(self.screen());
        if let Some(menu) = self.tv.menu {
            return Some(self.tv_menu_key(menu, code, damage, events));
        }
        let rows = self.tv_rows();
        match code {
            key::UP => match self.tv.focus {
                Focus::Tiles if self.tv.row == 0 => self.tv.focus = Focus::Strip(0),
                Focus::Tiles => {
                    self.tv.row -= 1;
                    self.tv.view = self.tv.view.min(self.tv.row);
                }
                Focus::Strip(_) => {}
            },
            key::DOWN => match self.tv.focus {
                Focus::Strip(_) => self.tv.focus = Focus::Tiles,
                Focus::Tiles if self.tv.row + 1 < rows.len() => {
                    self.tv.row += 1;
                    if self.tv.row > self.tv.view + 1 {
                        self.tv.view = self.tv.row - 1;
                    }
                }
                Focus::Tiles => {}
            },
            key::LEFT | key::RIGHT => match self.tv.focus {
                Focus::Strip(_) => self.tv.focus = Focus::Strip(u8::from(code == key::RIGHT)),
                Focus::Tiles => {
                    let Some((kind, tiles)) = rows.get(self.tv.row) else {
                        return Some(Action::None);
                    };
                    let slot = Self::row_index(*kind);
                    let tile = &mut self.tv.tile[slot];
                    *tile = match code {
                        key::LEFT => tile.saturating_sub(1),
                        _ => (*tile + 1).min(tiles.len().saturating_sub(1)),
                    };
                    let fit = ((self.width.saturating_sub(80)) / TILE_PITCH) as usize;
                    let scroll = &mut self.tv.scroll[slot];
                    if *tile < *scroll {
                        *scroll = *tile;
                    } else if *tile >= *scroll + fit {
                        *scroll = *tile + 1 - fit;
                    }
                }
            },
            key::ENTER | key::KEYPAD_ENTER => return Some(self.tv_open_focused(damage, events)),
            key::COMPOSE => {
                if let Some(tile) = self.focused_tile(&rows) {
                    self.tv.menu = Some(Menu::Tile(tile, 0));
                }
            }
            _ => return None,
        }
        Some(Action::None)
    }

    fn keys_mode(&self, slot: usize) -> bool {
        self.tv.keys_mode & (1 << (slot % 64)) != 0
    }

    fn send_key_to(&mut self, slot: usize, code: u16, events: &mut dyn EventSink) {
        for pressed in [1, 0] {
            let record = super::protocol::EventRecord::new(
                super::protocol::event::KEY,
                0,
                i32::from(code),
                pressed,
                0,
                0,
            );
            self.send(slot, record, events);
        }
    }

    fn pointer_app_key(
        &mut self,
        slot: usize,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if let Some(row) = self.tv.pointer_menu {
            damage.add(self.screen());
            return Some(self.pointer_menu_key(slot, row, code, damage, events));
        }
        let (x, y) = self.tv.pointer.unwrap_or((self.width / 2, self.height / 2));
        let keys = self.keys_mode(slot);
        match code {
            key::BACK | key::ESC => {
                self.send_key_to(slot, key::ESC, events);
                Some(Action::None)
            }
            key::COMPOSE => {
                self.tv.pointer_menu = Some(0);
                damage.add(self.screen());
                Some(Action::None)
            }
            key::UP | key::DOWN | key::LEFT | key::RIGHT if !keys => {
                let (x, y) = match code {
                    key::UP => (x, y.saturating_sub(POINTER_STEP)),
                    key::DOWN => (x, (y + POINTER_STEP).min(self.height - 1)),
                    key::LEFT => (x.saturating_sub(POINTER_STEP), y),
                    _ => ((x + POINTER_STEP).min(self.width - 1), y),
                };
                self.tv.pointer = Some((x, y));
                self.pointer_moved(x, y, damage, events);
                Some(Action::None)
            }
            key::ENTER | key::KEYPAD_ENTER if !keys => {
                self.pointer_moved(x, y, damage, events);
                self.button(crate::input::button::LEFT, true, damage, events);
                self.button(crate::input::button::LEFT, false, damage, events);
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn pointer_menu_key(
        &mut self,
        slot: usize,
        row: usize,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        match code {
            key::UP => self.tv.pointer_menu = Some(row.saturating_sub(1)),
            key::DOWN => self.tv.pointer_menu = Some((row + 1).min(POINTER_ROWS - 1)),
            key::ESC | key::BACK | key::COMPOSE => self.tv.pointer_menu = None,
            key::ENTER | key::KEYPAD_ENTER => {
                let (x, y) = self.tv.pointer.unwrap_or((self.width / 2, self.height / 2));
                match row {
                    0 => {
                        self.tv.pointer_menu = None;
                        self.pointer_moved(x, y, damage, events);
                        self.button(crate::input::button::RIGHT, true, damage, events);
                        self.button(crate::input::button::RIGHT, false, damage, events);
                    }
                    1 | 2 => {
                        self.pointer_moved(x, y, damage, events);
                        let notches = if row == 1 {
                            SCROLL_NOTCHES
                        } else {
                            -SCROLL_NOTCHES
                        };
                        self.wheel(notches, events);
                    }
                    3 => {
                        self.tv.pointer_menu = None;
                        self.tv.keys_mode ^= 1 << (slot % 64);
                    }
                    _ => {
                        self.tv.pointer_menu = None;
                        self.request_close(slot, events);
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Draws the pointer menu over a pointer app (section 5.4).
    pub(super) fn draw_pointer_menu(&self, canvas: &mut Canvas<'_>) {
        let (Some(row), Some(slot)) = (self.tv.pointer_menu, self.phone_shown_window()) else {
            return;
        };
        let arrows = match self.keys_mode(slot) {
            true => "Move the pointer with arrows",
            false => "Send arrows to the app",
        };
        let rows = [
            "Right click",
            "Scroll up",
            "Scroll down",
            arrows,
            "Close app",
        ];
        let height = MENU_ROW * rows.len() as u32 + 8;
        let rect = Rect::new(
            (self.width - MENU_WIDTH) / 2,
            (self.height - height) / 2,
            MENU_WIDTH,
            height,
        );
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_R6,
            true,
            true,
            colour::MENU,
        );
        for (index, label) in rows.iter().enumerate() {
            let top = rect.y + 4 + MENU_ROW * index as u32;
            if index == row {
                let line = Rect::new(rect.x + 1, top, rect.width - 2, MENU_ROW);
                canvas.fill(line, colour::FOCUS_RING);
                canvas.fill(
                    Rect::new(
                        line.x + RING,
                        line.y + RING,
                        line.width - 2 * RING,
                        line.height - 2 * RING,
                    ),
                    colour::SELECTION,
                );
                canvas.text(rect.x + 8, top + 8, ">", colour::TEXT);
            }
            canvas.text(rect.x + 24, top + 8, label, colour::TEXT);
        }
    }

    fn focused_tile(&self, rows: &[(RowKind, Vec<TileKind>)]) -> Option<TileKind> {
        if self.tv.focus != Focus::Tiles {
            return None;
        }
        let (kind, tiles) = rows.get(self.tv.row)?;
        tiles.get(self.tv.tile[Self::row_index(*kind)]).copied()
    }

    fn tv_open_focused(&mut self, damage: &mut Damage, events: &mut dyn EventSink) -> Action {
        match self.tv.focus {
            Focus::Strip(0) => self.launch(AppId::Settings, damage, events),
            Focus::Strip(_) => {
                self.tv.menu = Some(Menu::Power(0));
                Action::None
            }
            Focus::Tiles => {
                let rows = self.tv_rows();
                match self.focused_tile(&rows) {
                    Some(tile) => self.tv_open(tile, damage, events),
                    None => Action::None,
                }
            }
        }
    }

    fn tv_open(
        &mut self,
        tile: TileKind,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        match tile {
            TileKind::Window(slot) => {
                self.activate(slot, damage, events);
                Action::None
            }
            TileKind::App(app) => self.phone_open_app(app, damage, events),
            TileKind::Pane(index) => Action::OpenSettingsPane(SettingsPane::Named(PANES[index].0)),
        }
    }

    fn tv_menu_key(
        &mut self,
        menu: Menu,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let rows = self.tv_menu_rows(menu);
        let focused = match menu {
            Menu::Tile(_, row) | Menu::Power(row) => row,
        };
        let moved = |row: usize| match menu {
            Menu::Tile(tile, _) => Menu::Tile(tile, row),
            Menu::Power(_) => Menu::Power(row),
        };
        match code {
            key::UP => self.tv.menu = Some(moved(focused.saturating_sub(1))),
            key::DOWN => self.tv.menu = Some(moved((focused + 1).min(rows.len() - 1))),
            key::ESC | key::BACK | key::COMPOSE => self.tv.menu = None,
            key::ENTER | key::KEYPAD_ENTER => {
                self.tv.menu = None;
                return match (menu, rows.get(focused).copied()) {
                    (Menu::Power(_), Some("Power off")) => Action::PowerOff,
                    (Menu::Power(_), Some(_)) => Action::Restart,
                    (Menu::Tile(tile, _), Some("Open")) => self.tv_open(tile, damage, events),
                    (Menu::Tile(TileKind::Window(slot), _), Some("Close")) => {
                        self.request_close(slot, events);
                        Action::None
                    }
                    (Menu::Tile(TileKind::App(app), _), Some(_)) => self.uninstall(app),
                    _ => Action::None,
                };
            }
            _ => {}
        }
        Action::None
    }
}
