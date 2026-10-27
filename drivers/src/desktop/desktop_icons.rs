//! The desktop icons on the wallpaper (docs/ux/desktop-v0.3-gui.md, sections 6.3 and 6.4.7):
//! a grid of 96x96 cells from the work area's left edge (right in the macOS profile), filled
//! top to bottom, one per app the catalogue puts on the desktop; a click selects, a
//! double-click or Enter launches, a right-click or Shift+F10 opens the app's menu; the arrows
//! move the selection while the wallpaper has the keyboard.

use super::catalogue::AppId;
use super::icons::{IconSize, draw_icon};
use super::launcher::{AppMenu, MenuChoice};
use super::layout::IconSide;
use super::paint::{self, Canvas};
use super::popups::with_shadow;
use super::protocol::EventSink;
use super::{Action, Desktop, colour, contains, is_double_click, windows};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use crate::input::key;
use crate::input::keymap::Modifiers;

/// Side of a desktop icon's cell.
pub const ICON_CELL: u32 = 96;
/// The room between the work area's edges and the grid.
pub const ICON_GRID_MARGIN: u32 = 16;

const ICON_X: u32 = 24;
const ICON_Y: u32 = 8;
const LABEL_TOP: u32 = 64;
const LABEL_GLYPHS: usize = ((ICON_CELL - 2 * LABEL_PADDING) / GLYPH_WIDTH as u32) as usize;
const LABEL_PADDING: u32 = 4;
const LABEL_HEIGHT: u32 = 20;
const LABEL_TEXT_Y: u32 = 2;
const MENU_FROM_CELL: (u32, u32) = (24, 64);
const FOCUS_RING_INSET: u32 = 2;
const FOCUS_RING_WIDTH: u32 = 2;
const FOCUS_RING_INNER: [u32; 3] = [3, 1, 1];

/// The icons' state: the selected one, whether the wallpaper has the keyboard, the last
/// press for double-clicks and the open menu.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DesktopIcons {
    pub selected: Option<usize>,
    pub focused: bool,
    pub last_press: Option<(usize, u32, u32, u32)>,
    pub menu: Option<AppMenu>,
}

impl Desktop {
    fn icon_rows(&self) -> u32 {
        let work = self.layout.work_area;
        (work.height.saturating_sub(2 * ICON_GRID_MARGIN) / ICON_CELL).max(1)
    }

    fn icon_cell(&self, index: usize) -> Option<Rect> {
        let work = self.layout.work_area;
        let rows = self.icon_rows();
        let (column, row) = (index as u32 / rows, index as u32 % rows);
        let offset = ICON_GRID_MARGIN + column * ICON_CELL;
        let x = match self.layout.icon_side {
            IconSide::Left => work.x + offset,
            IconSide::Right => work.right().checked_sub(offset + ICON_CELL)?,
        };
        let cell = Rect::new(
            x,
            work.y + ICON_GRID_MARGIN + row * ICON_CELL,
            ICON_CELL,
            ICON_CELL,
        );
        let fits = cell.x >= work.x && cell.right() <= work.right();
        fits.then_some(cell)
    }

    fn desktop_app(&self, index: usize) -> Option<AppId> {
        self.catalogue.desktop_apps().nth(index)
    }

    /// Where `app`'s desktop icon's cell is, if it has one.
    pub fn desktop_icon(&self, app: AppId) -> Option<Rect> {
        let index = self
            .catalogue
            .desktop_apps()
            .position(|shown| shown == app)?;
        self.icon_cell(index)
    }

    /// The selected desktop icon's app.
    pub fn selected_desktop_icon(&self) -> Option<AppId> {
        self.icons
            .selected
            .and_then(|index| self.desktop_app(index))
    }

    /// Whether the wallpaper, with its icons, has the keyboard.
    pub fn desktop_focused(&self) -> bool {
        self.icons.focused && self.focus.is_none()
    }

    fn icon_at(&self, x: u32, y: u32) -> Option<usize> {
        (0..self.catalogue.desktop_apps().count()).find(|&index| {
            self.icon_cell(index)
                .is_some_and(|cell| contains(cell, x, y))
        })
    }

    fn damage_icon(&self, index: Option<usize>, damage: &mut Damage) {
        if let Some(cell) = index.and_then(|index| self.icon_cell(index)) {
            damage.add(cell);
        }
    }

    fn select_icon(&mut self, index: Option<usize>, damage: &mut Damage) {
        self.damage_icon(self.icons.selected, damage);
        self.icons.selected = index;
        self.damage_icon(index, damage);
    }

    /// The wallpaper takes the keyboard (or gives it up): its selected icon's ring shows.
    pub(super) fn set_desktop_focused(&mut self, focused: bool, damage: &mut Damage) {
        if self.icons.focused != focused {
            self.icons.focused = focused;
            self.damage_icon(self.icons.selected, damage);
        }
    }

    /// The catalogue changed: a selection past its icons is dropped, and every icon is drawn
    /// again.
    pub(super) fn icons_changed(&mut self, damage: &mut Damage) {
        let count = self.catalogue.desktop_apps().count();
        if self.icons.selected.is_some_and(|index| index >= count) {
            self.icons.selected = None;
        }
        self.icons.last_press = None;
        damage.add(self.layout.work_area);
    }

    /// Button `code` pressed at (`x`, `y`) on the wallpaper, no window there: selects the icon
    /// there (a double-click launches it, a right-click opens its menu) or clears the selection,
    /// and the wallpaper takes the keyboard.
    pub(super) fn wallpaper_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        self.set_focus(None, damage, events);
        self.set_desktop_focused(true, damage);
        let Some(index) = self.icon_at(x, y) else {
            self.select_icon(None, damage);
            self.icons.last_press = None;
            return Action::None;
        };
        self.select_icon(Some(index), damage);
        if code == crate::input::button::RIGHT {
            self.open_icon_menu(index, Some((x, y)), damage, events);
            return Action::None;
        }
        let press = (x, y, self.now_ms);
        let double = self.icons.last_press.is_some_and(|(previous, px, py, at)| {
            previous == index && is_double_click((px, py, at), press)
        });
        if double {
            self.icons.last_press = None;
            return self.launch_icon(index, damage, events);
        }
        self.icons.last_press = Some((index, x, y, self.now_ms));
        Action::None
    }

    fn launch_icon(
        &mut self,
        index: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        match self.desktop_app(index) {
            Some(app) => self.launch(app, damage, events),
            None => Action::None,
        }
    }

    fn open_icon_menu(
        &mut self,
        index: usize,
        at: Option<(u32, u32)>,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let (Some(app), Some(cell)) = (self.desktop_app(index), self.icon_cell(index)) else {
            return;
        };
        self.close_popups(damage, events);
        let menu = AppMenu {
            app,
            origin: at.unwrap_or((cell.x + MENU_FROM_CELL.0, cell.y + MENU_FROM_CELL.1)),
            hover: at.is_none().then_some(0),
        };
        self.icons.menu = Some(menu);
        damage.add(with_shadow(self.icon_menu_rect(&menu)));
        self.begin_popup(events);
    }

    fn icon_menu_rect(&self, menu: &AppMenu) -> Rect {
        let work = self.layout.work_area;
        menu.rect(Rect::new(0, 0, self.width, work.bottom()))
    }

    /// The desktop icon menu's rectangle while it is open.
    pub fn icon_menu_area(&self) -> Option<Rect> {
        self.icons.menu.map(|menu| self.icon_menu_rect(&menu))
    }

    pub(super) fn close_icon_menu(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if let Some(menu) = self.icons.menu.take() {
            damage.add(with_shadow(self.icon_menu_rect(&menu)));
            self.end_popup(None, damage, events);
        }
    }

    /// A button press at (`x`, `y`) while the icon menu is open: an item chooses it; anywhere
    /// else closes the menu. Returns the action and whether the press was the menu's.
    pub(super) fn icon_menu_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let menu = self.icons.menu?;
        let rect = self.icon_menu_rect(&menu);
        match menu.press(rect, x, y) {
            MenuChoice::Open(app) => {
                self.close_icon_menu(damage, events);
                Some(self.launch(app, damage, events))
            }
            MenuChoice::Nothing => Some(Action::None),
            MenuChoice::Outside => {
                self.close_icon_menu(damage, events);
                None
            }
        }
    }

    pub(super) fn icon_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        if let Some(mut menu) = self.icons.menu {
            let rect = self.icon_menu_rect(&menu);
            if menu.hover_at(rect, x, y) {
                self.icons.menu = Some(menu);
                damage.add(rect);
            }
        }
    }

    pub(super) fn icon_menu_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let Some(mut menu) = self.icons.menu else {
            return Action::None;
        };
        if !pressed {
            return Action::None;
        }
        match code {
            key::ESC => self.close_icon_menu(damage, events),
            key::UP | key::DOWN => {
                menu.step();
                self.icons.menu = Some(menu);
                damage.add(self.icon_menu_rect(&menu));
            }
            key::ENTER | key::KEYPAD_ENTER => {
                if let MenuChoice::Open(app) = menu.enter() {
                    self.close_icon_menu(damage, events);
                    return self.launch(app, damage, events);
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Key `code` pressed while the wallpaper has the keyboard.
    pub(super) fn desktop_key(
        &mut self,
        code: u16,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let count = (0..self.catalogue.desktop_apps().count())
            .take_while(|&index| self.icon_cell(index).is_some())
            .count();
        if count == 0 {
            return Action::None;
        }
        let rows = self.icon_rows() as usize;
        let last = count - 1;
        let moved = match (code, self.icons.selected) {
            (key::UP | key::DOWN | key::LEFT | key::RIGHT, None) => Some(0),
            (key::UP, Some(at)) if at % rows > 0 => Some(at - 1),
            (key::DOWN, Some(at)) if at % rows + 1 < rows && at < last => Some(at + 1),
            (key::LEFT | key::RIGHT, Some(at)) => {
                let row = at % rows;
                let column = at / rows;
                let target = match (code, self.layout.icon_side) {
                    (key::LEFT, IconSide::Left) | (key::RIGHT, IconSide::Right) => {
                        column.checked_sub(1)
                    }
                    _ => Some(column + 1),
                };
                target
                    .filter(|&column| column * rows <= last)
                    .map(|column| (column * rows + row).min(last))
            }
            (key::HOME, _) => Some(0),
            (key::END, _) => Some(last),
            _ => None,
        };
        if let Some(index) = moved {
            self.select_icon(Some(index), damage);
            return Action::None;
        }
        let menu_key =
            code == key::COMPOSE || (code == key::F10 && modifiers.contains(Modifiers::SHIFT));
        match (code, self.icons.selected) {
            (key::ENTER | key::KEYPAD_ENTER, Some(index)) => {
                self.launch_icon(index, damage, events)
            }
            (key::ESC, Some(_)) => {
                self.select_icon(None, damage);
                Action::None
            }
            (_, Some(index)) if menu_key => {
                self.open_icon_menu(index, None, damage, events);
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(super) fn icon_menu_overlaps(&self, clip: Rect) -> bool {
        self.icons
            .menu
            .is_some_and(|menu| with_shadow(self.icon_menu_rect(&menu)).overlaps(&clip))
    }

    pub(super) fn draw_icon_menu(&self, canvas: &mut Canvas<'_>) {
        if let Some(menu) = self.icons.menu {
            menu.draw(canvas, self.icon_menu_rect(&menu));
        }
    }

    pub(super) fn draw_desktop_icons(&self, canvas: &mut Canvas<'_>) {
        let clip = canvas.clip();
        if !self.layout.work_area.overlaps(&clip) {
            return;
        }
        for (index, app) in self.catalogue.desktop_apps().enumerate() {
            let Some(cell) = self.icon_cell(index) else {
                break;
            };
            if !cell.overlaps(&clip) {
                continue;
            }
            if self.icons.selected == Some(index) {
                canvas.rounded(cell, &paint::INSET_R6, true, true, colour::SELECTION);
                if self.desktop_focused() {
                    let ring = Rect::new(
                        cell.x + FOCUS_RING_INSET,
                        cell.y + FOCUS_RING_INSET,
                        cell.width - 2 * FOCUS_RING_INSET,
                        cell.height - 2 * FOCUS_RING_INSET,
                    );
                    canvas.rounded(ring, &paint::INSET_R6, true, true, colour::FOCUS_RING);
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
                        colour::SELECTION,
                    );
                }
            }
            draw_icon(
                canvas,
                self.catalogue.icon(app),
                IconSize::Large,
                cell.x + ICON_X,
                cell.y + ICON_Y,
            );
            let (text, cut) = windows::cut_title(self.catalogue.name(app), LABEL_GLYPHS);
            let glyphs = text.chars().count() + if cut { windows::ellipsis().len() } else { 0 };
            let width = glyphs as u32 * GLYPH_WIDTH as u32 + 2 * LABEL_PADDING;
            let pill = Rect::new(
                cell.x + (ICON_CELL - width.min(ICON_CELL)) / 2,
                cell.y + LABEL_TOP,
                width,
                LABEL_HEIGHT,
            );
            canvas.rounded(pill, &paint::INSET_R4, true, true, colour::BAR);
            let text_x = pill.x + LABEL_PADDING;
            let text_y = pill.y + LABEL_TEXT_Y;
            canvas.text(text_x, text_y, text, colour::TEXT);
            if cut {
                let after = text_x + text.chars().count() as u32 * GLYPH_WIDTH as u32;
                canvas.text(after, text_y, windows::ellipsis(), colour::TEXT);
            }
        }
    }
}
