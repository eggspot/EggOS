//! The launcher, the app menu and the notice (docs/ux/desktop-v0.3-gui.md, sections 6.2 and
//! 6.4): a popup with a search field and one row per app, filtered as the user types by name
//! and synonym, Enter or a click launching the selected one; in the Linux profile a dash of
//! the pinned apps on its left; the titles of minimised windows as chips under the list; the
//! menu of an app (`Open`, and `Uninstall...` for an installed one); and the short notice a
//! launch that cannot happen shows.

use super::catalogue::{self, AppId, Catalogue, MAX_INSTALLED, MenuAction};
use super::icons::{IconSize, draw_icon};
use super::paint::{self, Canvas};
use super::popups::with_shadow;
use super::protocol::{EventSink, Role};
use super::shortcuts::Profile;
use super::{
    Action, Desktop, MENU_ITEM_HEIGHT, MENU_TEXT_X, MENU_WIDTH, colour, contains, draw_chip_text,
    windows,
};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use crate::input::keymap::Modifiers;

/// What launching Files says while it is not installed (D18).
pub const FILES_MISSING: &str = "Files is not installed.";
/// What launching the Text Editor says while it is not installed (D18, D149).
pub const TEXT_EDITOR_MISSING: &str = "Text Editor is not installed.";
/// What the notice says when the kernel could not start an app.
pub const LAUNCH_FAILED: &str = "The app could not start.";
/// One launcher row's height.
pub const LAUNCHER_ROW_HEIGHT: u32 = 40;
/// How long a notice stays.
pub const NOTICE_MS: u32 = 3000;

/// How long the switch key's hint shows the new layout's title (D221).
pub const LAYOUT_HINT_MS: u32 = 1000;
/// The most bytes the launcher's search holds.
pub const QUERY_CAPACITY: usize = 32;
/// How long the launcher's caret is shown, then hidden, while it blinks (D222).
pub const CARET_BLINK_MS: u32 = 500;
/// How long after the last keystroke the launcher's caret stops blinking and stays shown.
pub const CARET_STEADY_AFTER_MS: u32 = 10_000;

const PADDING: u32 = 12;
const FIELD_HEIGHT: u32 = 28;
const FIELD_TEXT_X: u32 = 8;
const LIST_GAP: u32 = 8;
const ROW_ICON_X: u32 = 8;
const ROW_ICON_Y: u32 = 4;
const ROW_TEXT_X: u32 = 48;
const ROW_SOURCE_FROM_RIGHT: u32 = 8;
const SOURCE_ROOM_GLYPHS: usize = 10;
const DASH_WIDTH: u32 = 64;
const DASH_ICON_X: u32 = 8;
const DASH_FIRST_Y: u32 = 12;
const DASH_PITCH: u32 = 56;
const DASH_TILE_FROM_ICON: u32 = 4;
const DASH_TILE: u32 = 56;
const DASH_MARK_X: u32 = 22;
const DASH_MARK_Y: u32 = 50;
const DASH_MARK: u32 = 4;
const DASH_APPS: [AppId; 3] = [AppId::Terminal, AppId::Files, AppId::Settings];
const MINIMISED_GAP: u32 = 8;
const MINIMISED_HEIGHT: u32 = 20;
const MINIMISED_TEXT_Y: u32 = 2;
const MINIMISED_LABEL: &str = "Minimised:";
const MINIMISED_MAX_ROWS: usize = 7;
const CHIP_GAP: u32 = 8;
const PLACEHOLDER: &str = "Search apps";
const MOST_LAUNCHER_ROWS: usize = 7;
const FOOTER_Y: u32 = 368;
const FOOTER_TEXT_Y: u32 = 2;
/// The launcher's last line: the keys that reach EggOS even where the host keeps others
/// (docs/ux/desktop-v0.3-gui.md 14.3).
pub const LAUNCHER_FOOTER: &str = "Super: apps   Alt+Tab: windows   Alt+F4: close";
const NO_MATCH: &str = "No apps match";
const MATCH_CAPACITY: usize = 5 + MAX_INSTALLED;
const MENU_OPEN: &str = "Open";
const MENU_UNINSTALL: &str = "Uninstall...";
const MENU_MARGIN: u32 = 8;
const NOTICE_SIZE: (u32, u32) = (320, 40);
const NOTICE_TOP: u32 = 16;
const NOTICE_TEXT_Y: u32 = 12;
const FOCUS_RING_INSET: u32 = 2;
const FOCUS_RING_WIDTH: u32 = 2;
const FOCUS_RING_INNER: [u32; 1] = [1];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Row(usize),
    Dash(usize),
    Chip(u8),
}

/// An app's menu: whose it is, where its top-left corner was asked for, its items (with an
/// installed app's own actions, D301) and the item under the pointer or the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AppMenu {
    pub app: AppId,
    pub origin: (u32, u32),
    pub hover: Option<usize>,
    items: [MenuItem; MAX_MENU_ITEMS],
    count: u8,
}

/// One row of an app's menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    Open,
    Action(u8, MenuAction),
    AllActions,
    Uninstall,
    Separator,
}

const MAX_MENU_ITEMS: usize = 1 + catalogue::MENU_ACTIONS + 1 + 2 + 1;
const ACTIONS_MENU_WIDTH: u32 = 216;
const SEPARATOR_HEIGHT: u32 = 9;
const SEPARATOR_LINE: u32 = 4;
const SEPARATOR_INSET: u32 = 8;
const MENU_ALL_ACTIONS: &str = "All actions...";

impl MenuItem {
    fn height(self) -> u32 {
        match self {
            Self::Separator => SEPARATOR_HEIGHT,
            _ => MENU_ITEM_HEIGHT,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Open => MENU_OPEN,
            Self::Action(_, action) => action.label(),
            Self::AllActions => MENU_ALL_ACTIONS,
            Self::Uninstall => MENU_UNINSTALL,
            Self::Separator => "",
        }
    }
}

impl AppMenu {
    /// The menu of `app` asked for at `origin`, `hover` on, with the actions `catalogue` knows
    /// for it: `read` and `change` ones under `Open`, `All actions...`, then the `destroy` ones
    /// with `Uninstall...` below a separator (docs/ux/app-actions-v0.5.md section 1).
    pub fn new(app: AppId, origin: (u32, u32), hover: Option<usize>, catalogue: &Catalogue) -> Self {
        let mut menu = Self {
            app,
            origin,
            hover,
            items: [MenuItem::Separator; MAX_MENU_ITEMS],
            count: 0,
        };
        let mut push = |menu: &mut Self, item: MenuItem| {
            if usize::from(menu.count) < MAX_MENU_ITEMS {
                menu.items[usize::from(menu.count)] = item;
                menu.count += 1;
            }
        };
        push(&mut menu, MenuItem::Open);
        let installed = matches!(app, AppId::Installed(_));
        if !installed {
            return menu;
        }
        if !catalogue.has_actions(app) {
            push(&mut menu, MenuItem::Uninstall);
            return menu;
        }
        push(&mut menu, MenuItem::Separator);
        for (index, action) in catalogue.menu_actions(app).enumerate() {
            if !action.destroys {
                push(&mut menu, MenuItem::Action(index as u8, *action));
            }
        }
        push(&mut menu, MenuItem::AllActions);
        push(&mut menu, MenuItem::Separator);
        for (index, action) in catalogue.menu_actions(app).enumerate() {
            if action.destroys {
                push(&mut menu, MenuItem::Action(index as u8, *action));
            }
        }
        push(&mut menu, MenuItem::Uninstall);
        if menu.hover.is_some() {
            menu.hover = Some(0);
        }
        menu
    }

    fn items(&self) -> &[MenuItem] {
        &self.items[..usize::from(self.count)]
    }

    fn width(&self) -> u32 {
        match self.items().iter().any(|item| matches!(item, MenuItem::AllActions)) {
            true => ACTIONS_MENU_WIDTH,
            false => MENU_WIDTH,
        }
    }

    fn item_top(&self, index: usize) -> u32 {
        1 + self.items()[..index].iter().map(|item| item.height()).sum::<u32>()
    }

    /// Where it is, its border included, kept inside `bounds` less 8 px.
    pub fn rect(&self, bounds: Rect) -> Rect {
        let height = self.items().iter().map(|item| item.height()).sum::<u32>() + 2;
        let width = self.width();
        let right = bounds.right().saturating_sub(MENU_MARGIN);
        let bottom = bounds.bottom().saturating_sub(MENU_MARGIN);
        Rect::new(
            self.origin
                .0
                .min(right.saturating_sub(width))
                .max(bounds.x),
            self.origin
                .1
                .min(bottom.saturating_sub(height))
                .max(bounds.y),
            width,
            height,
        )
    }

    fn item_at(&self, rect: Rect, x: u32, y: u32) -> Option<usize> {
        if !contains(rect, x, y) {
            return None;
        }
        let offset = y.saturating_sub(rect.y);
        (0..self.items().len()).find(|&index| {
            let top = self.item_top(index);
            offset >= top && offset < top + self.items()[index].height() && self.items()[index] != MenuItem::Separator
        })
    }

    fn choice(&self, item: usize) -> MenuChoice {
        match self.items().get(item) {
            Some(MenuItem::Open) => MenuChoice::Open(self.app),
            Some(MenuItem::Action(index, _)) => MenuChoice::Do(self.app, Some(*index)),
            Some(MenuItem::AllActions) => MenuChoice::Do(self.app, None),
            Some(MenuItem::Uninstall) => MenuChoice::Uninstall(self.app),
            Some(MenuItem::Separator) | None => MenuChoice::Nothing,
        }
    }

    /// Up or Down: the item above or below, skipping separators, wrapping, or the first.
    pub fn step(&mut self, back: bool) {
        let count = self.items().len();
        let mut item = match (self.hover, back) {
            (None, _) => {
                self.hover = Some(0);
                return;
            }
            (Some(item), _) => item,
        };
        for _ in 0..count {
            item = match back {
                false => (item + 1) % count,
                true => (item + count - 1) % count,
            };
            if self.items()[item] != MenuItem::Separator {
                break;
            }
        }
        self.hover = Some(item);
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, rect: Rect) {
        Desktop::draw_popup_surface(canvas, rect, colour::MENU);
        for (index, item) in self.items().iter().enumerate() {
            let top = rect.y + self.item_top(index);
            if *item == MenuItem::Separator {
                canvas.fill(
                    Rect::new(
                        rect.x + SEPARATOR_INSET,
                        top + SEPARATOR_LINE,
                        rect.width.saturating_sub(2 * SEPARATOR_INSET),
                        1,
                    ),
                    colour::MENU_BORDER,
                );
                continue;
            }
            if self.hover == Some(index) {
                canvas.fill(
                    Rect::new(rect.x + 1, top + 2, rect.width - 2, MENU_ITEM_HEIGHT - 4),
                    colour::MENU_HOVER,
                );
            }
            canvas.text(
                rect.x + MENU_TEXT_X,
                top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                item.label(),
                colour::TEXT,
            );
        }
    }
}

/// What a choice in an app's menu asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuChoice {
    Open(AppId),
    Uninstall(AppId),
    /// One of the app's own actions, by its index, or `All actions...` (D301).
    Do(AppId, Option<u8>),
    Nothing,
    Outside,
}

impl AppMenu {
    /// A press at (`x`, `y`) on the menu drawn at `rect`.
    pub fn press(&self, rect: Rect, x: u32, y: u32) -> MenuChoice {
        match self.item_at(rect, x, y) {
            Some(item) => self.choice(item),
            None => MenuChoice::Outside,
        }
    }

    /// The hovered item chosen by Enter.
    pub fn enter(&self) -> MenuChoice {
        match self.hover {
            Some(item) => self.choice(item),
            None => MenuChoice::Nothing,
        }
    }

    /// The pointer moved to (`x`, `y`); returns whether the hovered item changed.
    pub fn hover_at(&mut self, rect: Rect, x: u32, y: u32) -> bool {
        let hover = self.item_at(rect, x, y);
        if hover.is_none() || hover == self.hover {
            return false;
        }
        self.hover = hover;
        true
    }
}

/// The open launcher: the search, the apps it matches, the selected one and the first shown,
/// what the pointer is over and the open app menu.
#[derive(Debug, Clone, Copy)]
pub(super) struct Launcher {
    query: [u8; QUERY_CAPACITY],
    query_length: usize,
    matches: [AppId; MATCH_CAPACITY],
    match_count: usize,
    selected: Option<usize>,
    first_row: usize,
    hover: Option<Spot>,
    pub menu: Option<AppMenu>,
    caret_since_ms: u32,
    caret_shown: bool,
}

impl Launcher {
    fn new(catalogue: &Catalogue) -> Self {
        let mut launcher = Self {
            query: [0; QUERY_CAPACITY],
            query_length: 0,
            matches: [AppId::Terminal; MATCH_CAPACITY],
            match_count: 0,
            selected: None,
            first_row: 0,
            hover: None,
            menu: None,
            caret_since_ms: 0,
            caret_shown: true,
        };
        launcher.filter(catalogue);
        launcher
    }

    fn query(&self) -> &str {
        core::str::from_utf8(&self.query[..self.query_length]).unwrap_or("")
    }

    fn restart_caret(&mut self, now_ms: u32) {
        self.caret_since_ms = now_ms;
        self.caret_shown = true;
    }

    /// Lists the apps the search matches again, the first selected.
    pub fn filter(&mut self, catalogue: &Catalogue) {
        self.match_count = 0;
        let query = self.query;
        let query = core::str::from_utf8(&query[..self.query_length]).unwrap_or("");
        for app in catalogue.launcher_apps() {
            if self.match_count < MATCH_CAPACITY && catalogue.matches(app, query) {
                self.matches[self.match_count] = app;
                self.match_count += 1;
            }
        }
        self.selected = (self.match_count > 0).then_some(0);
        self.first_row = 0;
        self.menu = None;
    }

    fn apps(&self) -> &[AppId] {
        &self.matches[..self.match_count]
    }
}

/// A short message under the top of the work area that closes itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Notice {
    pub text: &'static str,
    pub until_ms: u32,
}

impl Desktop {
    /// Whether the launcher is open.
    pub fn launcher_open(&self) -> bool {
        self.launcher.is_some()
    }

    /// The launcher's rectangle, its border included, while it is open.
    pub fn launcher_area(&self) -> Option<Rect> {
        self.launcher.map(|_| self.layout.launcher)
    }

    /// What the launcher's search holds, while it is open.
    pub fn launcher_query(&self) -> Option<&str> {
        self.launcher.as_ref().map(Launcher::query)
    }

    /// The apps the launcher lists now, in order, and the selected one's index.
    pub fn launcher_entries(&self) -> Option<(&[AppId], Option<usize>)> {
        self.launcher
            .as_ref()
            .map(|launcher| (launcher.apps(), launcher.selected))
    }

    /// The launcher's row of `app`, while it is open and shows it.
    pub fn launcher_row_of(&self, app: AppId) -> Option<Rect> {
        let launcher = self.launcher.as_ref()?;
        let index = launcher.apps().iter().position(|&listed| listed == app)?;
        self.launcher_row(launcher, index)
    }

    /// The notice shown now, its rectangle.
    pub fn notice_area(&self) -> Option<Rect> {
        self.notice.map(|_| self.notice_rect())
    }

    /// The notice shown now, its text.
    pub fn notice_text(&self) -> Option<&'static str> {
        self.notice.map(|notice| notice.text)
    }

    fn content_x(&self) -> u32 {
        let rect = self.layout.launcher;
        match self.profile {
            Profile::Linux => rect.x + DASH_WIDTH + PADDING,
            _ => rect.x + PADDING,
        }
    }

    fn content_width(&self) -> u32 {
        (self.layout.launcher.right() - PADDING).saturating_sub(self.content_x())
    }

    fn field_rect(&self) -> Rect {
        Rect::new(
            self.content_x(),
            self.layout.launcher.y + PADDING,
            self.content_width(),
            FIELD_HEIGHT,
        )
    }

    fn visible_rows(&self) -> usize {
        let room = self
            .layout
            .launcher
            .height
            .saturating_sub(PADDING + FIELD_HEIGHT + LIST_GAP + PADDING);
        ((room / LAUNCHER_ROW_HEIGHT) as usize).clamp(1, MOST_LAUNCHER_ROWS)
    }

    fn launcher_row(&self, launcher: &Launcher, index: usize) -> Option<Rect> {
        let shown = index.checked_sub(launcher.first_row)?;
        (shown < self.visible_rows()).then(|| self.row_rect(shown))
    }

    fn row_rect(&self, shown: usize) -> Rect {
        Rect::new(
            self.content_x(),
            self.layout.launcher.y
                + PADDING
                + FIELD_HEIGHT
                + LIST_GAP
                + shown as u32 * LAUNCHER_ROW_HEIGHT,
            self.content_width(),
            LAUNCHER_ROW_HEIGHT,
        )
    }

    fn dash_icon(&self, index: usize) -> Option<Rect> {
        let rect = self.layout.launcher;
        (self.profile == Profile::Linux).then(|| {
            Rect::new(
                rect.x + DASH_ICON_X,
                rect.y + DASH_FIRST_Y + index as u32 * DASH_PITCH,
                IconSize::Large.pixels(),
                IconSize::Large.pixels(),
            )
        })
    }

    fn rows_listed(&self, launcher: &Launcher) -> usize {
        launcher.match_count.clamp(1, self.visible_rows())
    }

    fn for_each_launcher_chip(&self, launcher: &Launcher, mut chip: impl FnMut(usize, Rect)) {
        let rows = self.rows_listed(launcher);
        if rows > MINIMISED_MAX_ROWS {
            return;
        }
        let line_y = self.row_rect(rows - 1).bottom() + MINIMISED_GAP;
        let label_x = self.content_x();
        let mut x = label_x + MINIMISED_LABEL.len() as u32 * GLYPH_WIDTH as u32 + CHIP_GAP;
        let end = self.layout.launcher.right() - PADDING;
        for &slot in &self.created[..self.created_length] {
            let slot = usize::from(slot);
            let window = self.window_at(slot);
            if !window.shown || !window.minimised || window.role != Role::Normal {
                continue;
            }
            let rect = super::chip_rect(x, window.title());
            let rect = Rect::new(rect.x, line_y, rect.width, MINIMISED_HEIGHT);
            if rect.right() > end {
                return;
            }
            chip(slot, rect);
            x = rect.right() + CHIP_GAP;
        }
    }

    fn spot_at(&self, launcher: &Launcher, x: u32, y: u32) -> Option<Spot> {
        let row = (launcher.first_row..launcher.match_count).find(|&index| {
            self.launcher_row(launcher, index)
                .is_some_and(|row| contains(row, x, y))
        });
        if let Some(index) = row {
            return Some(Spot::Row(index));
        }
        let dash = (0..DASH_APPS.len()).find(|&index| {
            self.dash_icon(index)
                .is_some_and(|icon| contains(icon, x, y))
        });
        if let Some(index) = dash {
            return Some(Spot::Dash(index));
        }
        let mut chip = None;
        self.for_each_launcher_chip(launcher, |slot, rect| {
            if contains(rect, x, y) {
                chip = Some(Spot::Chip(slot as u8));
            }
        });
        chip
    }

    /// Opens the launcher with every app listed and the first selected, or closes it if it is
    /// open; asks the kernel to read the catalogue again.
    pub(super) fn toggle_launcher(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.launcher.is_some() {
            self.close_launcher(damage, events);
            return;
        }
        self.close_popups(damage, events);
        self.hide_tooltip(damage);
        let mut launcher = Launcher::new(&self.catalogue);
        launcher.caret_since_ms = self.now_ms;
        self.launcher = Some(launcher);
        self.catalogue_wanted = true;
        damage.add(with_shadow(self.layout.launcher));
        self.damage_launcher_openers(damage);
        self.begin_popup(events);
    }

    fn damage_launcher_openers(&self, damage: &mut Damage) {
        for opener in [self.start_button(), self.system_item()]
            .into_iter()
            .flatten()
        {
            damage.add(opener);
        }
    }

    pub(super) fn close_launcher(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.launcher.take().is_some() {
            damage.add(with_shadow(self.layout.launcher));
            self.damage_launcher_openers(damage);
            self.end_popup(None, damage, events);
        }
    }

    /// Asks to uninstall installed app `app`: the installer's window confirms first (D134).
    pub fn uninstall(&self, app: AppId) -> Action {
        match self.catalogue.uninstall(app) {
            Some(_) => Action::Uninstall(app),
            None => Action::None,
        }
    }

    /// Asks to run one of installed app `app`'s own actions, or to list them all (`None`):
    /// `app do --window` asks what it needs and shows the result (D301).
    pub fn app_do(&self, app: AppId, action: Option<u8>) -> Action {
        match self.catalogue.app_do(app, action) {
            Some(_) => Action::AppDo(app, action),
            None => Action::None,
        }
    }

    fn launch_from_launcher(
        &mut self,
        app: AppId,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        self.close_launcher(damage, events);
        self.launch(app, damage, events)
    }

    fn set_launcher(&mut self, launcher: Launcher, damage: &mut Damage) {
        self.launcher = Some(launcher);
        damage.add(self.layout.launcher);
    }

    fn select_row(&self, launcher: &mut Launcher, index: usize) {
        launcher.selected = Some(index);
        let rows = self.visible_rows();
        if index < launcher.first_row {
            launcher.first_row = index;
        } else if index >= launcher.first_row + rows {
            launcher.first_row = index + 1 - rows;
        }
    }

    fn open_launcher_menu(&self, launcher: &mut Launcher, index: usize, at: Option<(u32, u32)>) {
        let Some(&app) = launcher.apps().get(index) else {
            return;
        };
        self.select_row(launcher, index);
        let origin = at.unwrap_or_else(|| {
            let row = self
                .launcher_row(launcher, index)
                .unwrap_or(self.row_rect(0));
            (row.x + ROW_TEXT_X, row.bottom())
        });
        launcher.menu = Some(AppMenu::new(app, origin, at.is_none().then_some(0), &self.catalogue));
    }

    /// Key `code` pressed or released with `modifiers` while the launcher is open.
    pub(super) fn launcher_key(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let Some(mut launcher) = self.launcher else {
            return Action::None;
        };
        if !pressed {
            return Action::None;
        }
        if let Some(mut menu) = launcher.menu {
            match code {
                key::ESC => launcher.menu = None,
                key::UP | key::DOWN => {
                    menu.step(code == key::UP);
                    launcher.menu = Some(menu);
                }
                key::ENTER | key::KEYPAD_ENTER => match menu.enter() {
                    MenuChoice::Open(app) => return self.launch_from_launcher(app, damage, events),
                    MenuChoice::Uninstall(app) => {
                        self.close_launcher(damage, events);
                        return self.uninstall(app);
                    }
                    MenuChoice::Do(app, action) => {
                        self.close_launcher(damage, events);
                        return self.app_do(app, action);
                    }
                    MenuChoice::Nothing | MenuChoice::Outside => {}
                },
                _ => {}
            }
            self.set_launcher(launcher, damage);
            return Action::None;
        }
        let commands = Modifiers::CTRL.bits() | Modifiers::ALT.bits() | Modifiers::SUPER.bits();
        let plain = modifiers.bits() & commands == 0;
        match code {
            key::ESC => {
                self.close_launcher(damage, events);
                return Action::None;
            }
            key::ENTER | key::KEYPAD_ENTER => {
                if let Some(&app) = launcher
                    .selected
                    .and_then(|index| launcher.apps().get(index))
                {
                    return self.launch_from_launcher(app, damage, events);
                }
                return Action::None;
            }
            key::UP | key::DOWN => {
                if let Some(selected) = launcher.selected {
                    let last = launcher.match_count - 1;
                    let next = match code {
                        key::UP => selected.saturating_sub(1),
                        _ => (selected + 1).min(last),
                    };
                    self.select_row(&mut launcher, next);
                }
            }
            key::BACKSPACE => {
                launcher.restart_caret(self.now_ms);
                if launcher.query_length > 0 {
                    launcher.query_length -= 1;
                    launcher.filter(&self.catalogue);
                }
            }
            key::F10 if modifiers.contains(Modifiers::SHIFT) => {
                if let Some(selected) = launcher.selected {
                    self.open_launcher_menu(&mut launcher, selected, None);
                }
            }
            key::COMPOSE => {
                if let Some(selected) = launcher.selected {
                    self.open_launcher_menu(&mut launcher, selected, None);
                }
            }
            _ => {
                let character = self.typed;
                let printable = (0x20..0x7f).contains(&character);
                if !plain || !printable || launcher.query_length == QUERY_CAPACITY {
                    return Action::None;
                }
                launcher.restart_caret(self.now_ms);
                launcher.query[launcher.query_length] = character as u8;
                launcher.query_length += 1;
                launcher.filter(&self.catalogue);
            }
        }
        self.set_launcher(launcher, damage);
        Action::None
    }

    /// Button `code` pressed at (`x`, `y`) while the launcher is open. Returns the action and
    /// whether the press was the launcher's; a press outside closes it and is not.
    pub(super) fn launcher_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let mut launcher = self.launcher?;
        let rect = self.layout.launcher;
        if let Some(menu) = launcher.menu {
            let menu_rect = menu.rect(rect);
            match menu.press(menu_rect, x, y) {
                MenuChoice::Open(app) => {
                    return Some(self.launch_from_launcher(app, damage, events));
                }
                MenuChoice::Uninstall(app) => {
                    self.close_launcher(damage, events);
                    return Some(self.uninstall(app));
                }
                MenuChoice::Do(app, action) => {
                    self.close_launcher(damage, events);
                    return Some(self.app_do(app, action));
                }
                MenuChoice::Nothing => return Some(Action::None),
                MenuChoice::Outside => {
                    launcher.menu = None;
                    self.set_launcher(launcher, damage);
                    if contains(rect, x, y) {
                        return Some(Action::None);
                    }
                }
            }
        }
        if !contains(rect, x, y) {
            self.close_launcher(damage, events);
            return None;
        }
        let right = code == crate::input::button::RIGHT;
        match self.spot_at(&launcher, x, y) {
            Some(Spot::Row(index)) if right => {
                self.open_launcher_menu(&mut launcher, index, Some((x, y)));
                self.set_launcher(launcher, damage);
                Some(Action::None)
            }
            Some(Spot::Row(index)) if code == crate::input::button::LEFT => {
                let app = launcher.apps()[index];
                Some(self.launch_from_launcher(app, damage, events))
            }
            Some(Spot::Dash(index)) if code == crate::input::button::LEFT => {
                Some(self.launch_from_launcher(DASH_APPS[index], damage, events))
            }
            Some(Spot::Chip(slot)) if code == crate::input::button::LEFT => {
                self.close_launcher(damage, events);
                self.activate(usize::from(slot), damage, events);
                Some(Action::None)
            }
            _ => Some(Action::None),
        }
    }

    /// The pointer moved to (`x`, `y`) while the launcher is open.
    pub(super) fn launcher_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(mut launcher) = self.launcher else {
            return;
        };
        let rect = self.layout.launcher;
        let mut changed = false;
        if let Some(mut menu) = launcher.menu {
            changed = menu.hover_at(menu.rect(rect), x, y);
            launcher.menu = Some(menu);
        }
        let hover = self.spot_at(&launcher, x, y);
        if hover != launcher.hover {
            launcher.hover = hover;
            changed = true;
        }
        if changed {
            self.set_launcher(launcher, damage);
        }
    }

    /// Whether (`x`, `y`) is on the launcher's search field.
    pub(super) fn over_launcher_field(&self, x: u32, y: u32) -> bool {
        self.launcher.is_some() && contains(self.field_rect(), x, y)
    }

    /// The catalogue changed: the launcher lists what its search matches in it now.
    pub(super) fn refilter_launcher(&mut self, damage: &mut Damage) {
        if let Some(mut launcher) = self.launcher {
            launcher.filter(&self.catalogue);
            self.set_launcher(launcher, damage);
        }
    }

    pub(super) fn launcher_overlaps(&self, clip: Rect) -> bool {
        self.launcher.is_some() && with_shadow(self.layout.launcher).overlaps(&clip)
    }

    fn draw_focus_ring(canvas: &mut Canvas<'_>, around: Rect, fill: u32) {
        let ring = Rect::new(
            around.x + FOCUS_RING_INSET,
            around.y + FOCUS_RING_INSET,
            around.width - 2 * FOCUS_RING_INSET,
            around.height - 2 * FOCUS_RING_INSET,
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
            fill,
        );
    }

    pub(super) fn draw_launcher(&self, canvas: &mut Canvas<'_>) {
        let Some(launcher) = &self.launcher else {
            return;
        };
        let rect = self.layout.launcher;
        Self::draw_popup_surface(canvas, rect, colour::DOCK);
        canvas.text(
            self.content_x(),
            rect.y + FOOTER_Y + FOOTER_TEXT_Y,
            LAUNCHER_FOOTER,
            colour::TEXT_DIM,
        );
        if self.profile == Profile::Linux {
            canvas.fill(
                Rect::new(rect.x + DASH_WIDTH, rect.y + 1, 1, rect.height - 2),
                colour::MENU_BORDER,
            );
            for (index, app) in DASH_APPS.into_iter().enumerate() {
                let Some(icon) = self.dash_icon(index) else {
                    continue;
                };
                if launcher.hover == Some(Spot::Dash(index)) {
                    canvas.rounded(
                        Rect::new(
                            icon.x - DASH_TILE_FROM_ICON,
                            icon.y - DASH_TILE_FROM_ICON,
                            DASH_TILE,
                            DASH_TILE,
                        ),
                        &paint::INSET_R6,
                        true,
                        true,
                        colour::BAR_HOVER,
                    );
                }
                draw_icon(
                    canvas,
                    self.catalogue.icon(app),
                    IconSize::Large,
                    icon.x,
                    icon.y,
                );
                let mut windows = [0u8; super::protocol::MAX_WINDOWS];
                if self.app_windows(app, &mut windows) > 0 {
                    canvas.fill(
                        Rect::new(
                            icon.x + DASH_MARK_X,
                            icon.y + DASH_MARK_Y,
                            DASH_MARK,
                            DASH_MARK,
                        ),
                        colour::TEXT,
                    );
                }
            }
        }
        let field = self.field_rect();
        canvas.rounded(field, &paint::INSET_R4, true, true, colour::BUTTON_BORDER);
        canvas.rounded(
            Rect::new(field.x + 1, field.y + 1, field.width - 2, field.height - 2),
            &paint::INSET_INSIDE_R4,
            true,
            true,
            colour::FIELD,
        );
        let text_y = field.y + (FIELD_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        let query = launcher.query();
        match query.is_empty() {
            true => canvas.text(
                field.x + FIELD_TEXT_X,
                text_y,
                PLACEHOLDER,
                colour::TEXT_DIM,
            ),
            false => canvas.text(field.x + FIELD_TEXT_X, text_y, query, colour::TEXT),
        }
        if launcher.caret_shown {
            canvas.fill(self.caret_rect(launcher), colour::TEXT);
        }
        let text_offset = (LAUNCHER_ROW_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        if launcher.match_count == 0 {
            let row = self.row_rect(0);
            canvas.text(
                row.x + ROW_TEXT_X,
                row.y + text_offset,
                NO_MATCH,
                colour::TEXT_DIM,
            );
        }
        for (index, &app) in launcher.apps().iter().enumerate() {
            let Some(row) = self.launcher_row(launcher, index) else {
                continue;
            };
            if launcher.selected == Some(index) {
                canvas.fill(row, colour::SELECTION);
                Self::draw_focus_ring(canvas, row, colour::SELECTION);
            }
            draw_icon(
                canvas,
                self.catalogue.icon(app),
                IconSize::Small,
                row.x + ROW_ICON_X,
                row.y + ROW_ICON_Y,
            );
            let name = self.catalogue.name(app);
            let room = ((row.width - ROW_TEXT_X) / GLYPH_WIDTH as u32) as usize;
            let (shown, cut) = windows::cut_title(name, room.saturating_sub(SOURCE_ROOM_GLYPHS));
            canvas.text(row.x + ROW_TEXT_X, row.y + text_offset, shown, colour::TEXT);
            if cut {
                let after = row.x + ROW_TEXT_X + shown.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(
                    after,
                    row.y + text_offset,
                    windows::ellipsis(),
                    colour::TEXT,
                );
            }
            if let Some(source) = self.catalogue.source(app) {
                let label = source.label();
                let width = label.len() as u32 * GLYPH_WIDTH as u32;
                canvas.text(
                    row.right() - ROW_SOURCE_FROM_RIGHT - width,
                    row.y + text_offset,
                    label,
                    colour::TEXT_DIM,
                );
            }
        }
        let mut label_drawn = false;
        self.for_each_launcher_chip(launcher, |slot, chip| {
            if !label_drawn {
                canvas.text(
                    self.content_x(),
                    chip.y + MINIMISED_TEXT_Y,
                    MINIMISED_LABEL,
                    colour::TEXT_DIM,
                );
                label_drawn = true;
            }
            let fill = match launcher.hover == Some(Spot::Chip(slot as u8)) {
                true => colour::BAR_PRESSED,
                false => colour::BAR_HOVER,
            };
            canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
            draw_chip_text(canvas, chip, self.window_at(slot).title());
        });
        if let Some(menu) = launcher.menu {
            menu.draw(canvas, menu.rect(rect));
        }
    }

    /// Shows `text` in the notice for [`NOTICE_MS`].
    pub fn show_notice(&mut self, text: &'static str, damage: &mut Damage) {
        self.show_notice_for(text, NOTICE_MS, damage);
    }

    /// The switch key made the layout titled `title` the one in force (D221): its title shows
    /// in the notice for [`LAYOUT_HINT_MS`], in place of any notice showing, and the Super key
    /// held for the chord no longer counts as pressed alone.
    pub fn show_layout_hint(&mut self, title: &'static str, damage: &mut Damage) {
        self.super_alone = false;
        self.show_notice_for(title, LAYOUT_HINT_MS, damage);
    }

    fn show_notice_for(&mut self, text: &'static str, milliseconds: u32, damage: &mut Damage) {
        self.close_notice(damage);
        self.notice = Some(Notice {
            text,
            until_ms: self.now_ms.wrapping_add(milliseconds),
        });
        damage.add(with_shadow(self.notice_rect()));
    }

    pub(super) fn close_notice(&mut self, damage: &mut Damage) {
        if self.notice.take().is_some() {
            damage.add(with_shadow(self.notice_rect()));
        }
    }

    /// The clock reached the notice's end: it closes.
    pub(super) fn tick_notice(&mut self, damage: &mut Damage) {
        if let Some(notice) = self.notice
            && self.now_ms.wrapping_sub(notice.until_ms) as i32 >= 0
        {
            self.close_notice(damage);
        }
    }

    fn notice_rect(&self) -> Rect {
        let work = self.layout.work_area;
        let width = NOTICE_SIZE.0.min(work.width);
        Rect::new(
            work.x + (work.width - width) / 2,
            work.y + NOTICE_TOP,
            width,
            NOTICE_SIZE.1,
        )
    }

    pub(super) fn notice_overlaps(&self, clip: Rect) -> bool {
        self.notice.is_some() && with_shadow(self.notice_rect()).overlaps(&clip)
    }

    pub(super) fn draw_notice(&self, canvas: &mut Canvas<'_>) {
        let Some(notice) = self.notice else {
            return;
        };
        let rect = self.notice_rect();
        Self::draw_popup_surface(canvas, rect, colour::MENU);
        let width = notice.text.len() as u32 * GLYPH_WIDTH as u32;
        canvas.text(
            rect.x + rect.width.saturating_sub(width) / 2,
            rect.y + NOTICE_TEXT_Y,
            notice.text,
            colour::TEXT,
        );
    }
}

/// Whether the launcher's caret is drawn at `now_ms` when the last keystroke, or the opening,
/// was at `since_ms` (D222): shown and hidden for [`CARET_BLINK_MS`] each, shown first, and
/// shown for good after [`CARET_STEADY_AFTER_MS`] or with `reduce_motion`.
pub fn caret_shown_at(since_ms: u32, now_ms: u32, reduce_motion: bool) -> bool {
    let elapsed = super::responsiveness::waited(now_ms, since_ms);
    reduce_motion
        || elapsed >= CARET_STEADY_AFTER_MS
        || (elapsed / CARET_BLINK_MS).is_multiple_of(2)
}

impl Desktop {
    /// Whether animations are left out (`reduce_motion`, docs/ux/first-run-accessibility-v0.3.md
    /// 2.5): the launcher's caret stays shown.
    pub fn set_reduce_motion(&mut self, reduce_motion: bool, damage: &mut Damage) {
        self.reduce_motion = reduce_motion;
        self.tick_caret(damage);
    }

    /// Whether the launcher's caret is drawn now, while the launcher is open.
    pub fn launcher_caret_drawn(&self) -> Option<bool> {
        self.launcher.map(|launcher| launcher.caret_shown)
    }

    /// The launcher's caret's rectangle, while the launcher is open.
    pub fn launcher_caret(&self) -> Option<Rect> {
        self.launcher.map(|launcher| self.caret_rect(&launcher))
    }

    fn caret_rect(&self, launcher: &Launcher) -> Rect {
        let field = self.field_rect();
        let text_y = field.y + (FIELD_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        let x = field.x + FIELD_TEXT_X + launcher.query_length as u32 * GLYPH_WIDTH as u32;
        Rect::new(x, text_y, 1, GLYPH_HEIGHT as u32)
    }

    /// The clock moved: the caret shows or hides, damaging only its own rectangle.
    pub(super) fn tick_caret(&mut self, damage: &mut Damage) {
        let Some(mut launcher) = self.launcher else {
            return;
        };
        let shown = caret_shown_at(launcher.caret_since_ms, self.now_ms, self.reduce_motion);
        if shown == launcher.caret_shown {
            return;
        }
        launcher.caret_shown = shown;
        damage.add(self.caret_rect(&launcher));
        self.launcher = Some(launcher);
    }

    /// How long until the caret next shows or hides; `None` while it is steady.
    pub(super) fn caret_change_after(&self) -> Option<u32> {
        let launcher = self.launcher?;
        let elapsed = super::responsiveness::waited(self.now_ms, launcher.caret_since_ms);
        if self.reduce_motion || elapsed >= CARET_STEADY_AFTER_MS {
            return None;
        }
        Some(CARET_BLINK_MS - elapsed % CARET_BLINK_MS)
    }
}
