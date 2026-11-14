//! The overview (D591, docs/ux/desktops-overview-screenshots-v0.3.md section 4): Super+Tab,
//! F3 or Ctrl+Up in the macOS profile, the Linux profile's Super alone and the taskbar's
//! `Task view` or the bar's `Activities` cover the work area with the wallpaper, a strip of the
//! desktops at the top (a mini of each, `+` for a new one) and a grid of the shown desktop's
//! windows as thumbnails under it, most recently focused first. A click or Enter picks one; a
//! thumbnail dragged onto a desktop moves its window there; Esc closes it.

use super::paint::{self, Canvas};
use super::protocol::{EventSink, MAX_WINDOWS, Role};
use super::screenshots::outline;
use super::shortcuts::{Shortcut, shortcut};
use super::windows::{cut_title, ellipsis};
use super::{Action, Contents, Desktop, colour, contains, desktops::MAX_DESKTOPS};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use crate::input::keymap::Modifiers;

/// A desktop tile's size in the strip.
pub const TILE_SIZE: (u32, u32) = (128, 72);
const TILE_GAP: u32 = 8;
const LABEL_HEIGHT: u32 = 20;
const LABEL_GAP: u32 = 4;
const LABEL_PADDING: u32 = 8;
const FIELD_MARGIN: u32 = 16;
const STRIP_HEIGHT: u32 = 112;
const CELL_GAP: u32 = 16;
const CELL_LABEL_ROWS: u32 = 24;
const CLOSE_SIZE: u32 = 12;
const CLOSE_HIT: u32 = 20;
const DRAG_DISTANCE: u32 = 4;
const RING: u32 = 2;
const SAMPLE_RUN: usize = 64;
const EMPTY_TEXT: &str = "No windows on this desktop";
const PLUS_AT: (u32, u32) = (56, 20);

/// Something the overview shows that a pointer or a key can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverviewItem {
    /// Desktop `n` (from 0)'s tile.
    Tile(u8),
    /// Desktop `n`'s close control.
    TileClose(u8),
    /// The `+` tile.
    NewTile,
    /// The window at slot `n`'s thumbnail.
    Thumbnail(u8),
    /// Its close control.
    ThumbnailClose(u8),
}

/// What the keys act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverviewSelection {
    /// Tile `n` of the strip (the `+` tile after the desktops).
    Tile(u8),
    /// Thumbnail `n` of the grid, in its order.
    Thumbnail(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Press {
    item: OverviewItem,
    at: (u32, u32),
    offset: (u32, u32),
}

/// The open overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Overview {
    selection: OverviewSelection,
    left_grid_at: usize,
    hover: Option<OverviewItem>,
    press: Option<Press>,
    dragging: Option<(u32, u32)>,
}

struct Grid {
    slots: [u8; MAX_WINDOWS],
    length: usize,
    columns: u32,
    cell: (u32, u32),
    area: Rect,
}

impl Grid {
    fn cell_rect(&self, index: usize) -> Rect {
        let column = index as u32 % self.columns;
        let row = index as u32 / self.columns;
        Rect::new(
            self.area.x + column * (self.cell.0 + CELL_GAP),
            self.area.y + row * (self.cell.1 + CELL_GAP),
            self.cell.0,
            self.cell.1,
        )
    }
}

fn ceiling(value: u32, by: u32) -> u32 {
    value.div_ceil(by.max(1))
}

fn grown(rect: Rect, by: u32) -> Rect {
    Rect::new(
        rect.x.saturating_sub(by),
        rect.y.saturating_sub(by),
        rect.width + 2 * by,
        rect.height + 2 * by,
    )
}

fn close_circle(anchor_right: u32, top: u32) -> (Rect, Rect) {
    let circle = Rect::new(anchor_right.saturating_sub(16), top + 4, CLOSE_SIZE, CLOSE_SIZE);
    let hit = Rect::new(anchor_right.saturating_sub(CLOSE_HIT), top, CLOSE_HIT, CLOSE_HIT);
    (circle, hit)
}

impl Desktop {
    /// Whether the overview is open.
    pub fn overview_open(&self) -> bool {
        self.overview.is_some()
    }

    /// Opens the overview, or closes it when it is open.
    pub(super) fn toggle_overview(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        match self.overview.is_some() {
            true => self.close_overview(None, damage, events),
            false => self.open_overview(damage, events),
        }
    }

    pub(super) fn open_overview(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        self.close_popups(damage, events);
        self.close_notice(damage);
        let selection = match self.overview_grid().length {
            0 => OverviewSelection::Tile(self.current_desktop),
            _ => OverviewSelection::Thumbnail(0),
        };
        self.overview = Some(Overview {
            selection,
            left_grid_at: 0,
            hover: None,
            press: None,
            dragging: None,
        });
        self.begin_popup(events);
        damage.add(self.screen());
    }

    /// Closes the overview, focusing window `target` when one was picked.
    pub(super) fn close_overview(
        &mut self,
        target: Option<usize>,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        if self.overview.take().is_none() {
            return;
        }
        damage.add(self.screen());
        self.end_popup(target, damage, events);
    }

    /// Where the overview draws: the work area less its margin.
    fn overview_field(&self) -> Rect {
        let work = self.work_area();
        Rect::new(
            work.x + FIELD_MARGIN,
            work.y + FIELD_MARGIN,
            work.width.saturating_sub(2 * FIELD_MARGIN),
            work.height.saturating_sub(2 * FIELD_MARGIN),
        )
    }

    fn strip_tiles(&self) -> u8 {
        self.desktops + u8::from(self.desktops < MAX_DESKTOPS)
    }

    fn tile_rect(&self, index: u8) -> Rect {
        let field = self.overview_field();
        let tiles = u32::from(self.strip_tiles());
        let group = tiles * TILE_SIZE.0 + (tiles - 1) * TILE_GAP;
        let left = field.x + field.width.saturating_sub(group) / 2;
        Rect::new(
            left + u32::from(index) * (TILE_SIZE.0 + TILE_GAP),
            field.y,
            TILE_SIZE.0,
            TILE_SIZE.1,
        )
    }

    fn tile_label_rect(&self, index: u8) -> Rect {
        let tile = self.tile_rect(index);
        let width = 9 * GLYPH_WIDTH as u32 + 2 * LABEL_PADDING;
        Rect::new(
            tile.x + (tile.width - width) / 2,
            tile.bottom() + LABEL_GAP,
            width,
            LABEL_HEIGHT,
        )
    }

    fn overview_grid_area(&self) -> Rect {
        let field = self.overview_field();
        Rect::new(
            field.x,
            field.y + STRIP_HEIGHT,
            field.width,
            field.height.saturating_sub(STRIP_HEIGHT),
        )
    }

    fn overview_grid(&self) -> Grid {
        let mut order = [0u8; MAX_WINDOWS];
        let all = self.switcher_order(&mut order);
        let mut grid = Grid {
            slots: [0; MAX_WINDOWS],
            length: 0,
            columns: 1,
            cell: (0, 0),
            area: self.overview_grid_area(),
        };
        for &slot in &order[..all] {
            let window = self.window_at(usize::from(slot));
            if window.parent.is_none() && window.desktop == self.current_desktop {
                grid.slots[grid.length] = slot;
                grid.length += 1;
            }
        }
        if grid.length == 0 {
            return grid;
        }
        let count = grid.length as u32;
        let mut columns = 1;
        while columns * columns < count {
            columns += 1;
        }
        let rows = ceiling(count, columns);
        grid.columns = columns;
        grid.cell = (
            grid.area.width.saturating_sub(CELL_GAP * (columns - 1)) / columns,
            grid.area.height.saturating_sub(CELL_GAP * (rows - 1)) / rows,
        );
        grid
    }

    fn thumbnail(&self, grid: &Grid, index: usize) -> (Rect, u32) {
        let cell = grid.cell_rect(index);
        let window = self.window_at(usize::from(grid.slots[index]));
        let (width, height) = (window.content_width, window.content_height);
        let room = (cell.width.max(1), cell.height.saturating_sub(CELL_LABEL_ROWS).max(1));
        let scale = ceiling(width, room.0)
            .max(ceiling(height, room.1))
            .max(1);
        let size = (width / scale, height / scale);
        (
            Rect::new(
                cell.x + room.0.saturating_sub(size.0) / 2,
                cell.y + room.1.saturating_sub(size.1) / 2,
                size.0,
                size.1,
            ),
            scale,
        )
    }

    fn overview_hit(&self, x: u32, y: u32) -> Option<OverviewItem> {
        for index in 0..self.strip_tiles() {
            let tile = self.tile_rect(index);
            if index >= self.desktops {
                if contains(tile, x, y) {
                    return Some(OverviewItem::NewTile);
                }
                continue;
            }
            let (_, hit) = close_circle(tile.right(), tile.y);
            if self.desktops > 1 && contains(hit, x, y) {
                return Some(OverviewItem::TileClose(index));
            }
            if contains(tile, x, y) || contains(self.tile_label_rect(index), x, y) {
                return Some(OverviewItem::Tile(index));
            }
        }
        let grid = self.overview_grid();
        for index in 0..grid.length {
            let (thumbnail, _) = self.thumbnail(&grid, index);
            let slot = grid.slots[index];
            let (_, hit) = close_circle(thumbnail.right(), thumbnail.y);
            if self.window_at(usize::from(slot)).closable && contains(hit, x, y) {
                return Some(OverviewItem::ThumbnailClose(slot));
            }
            if contains(thumbnail, x, y) {
                return Some(OverviewItem::Thumbnail(slot));
            }
        }
        None
    }

    fn set_overview(&mut self, overview: Overview, damage: &mut Damage) {
        if self.overview != Some(overview) {
            damage.add(self.work_area());
        }
        self.overview = Some(overview);
    }

    fn reset_overview_selection(&mut self, damage: &mut Damage) {
        let Some(mut overview) = self.overview else {
            return;
        };
        overview.selection = match self.overview_grid().length {
            0 => OverviewSelection::Tile(self.current_desktop),
            _ => OverviewSelection::Thumbnail(0),
        };
        overview.left_grid_at = 0;
        damage.add(self.work_area());
        self.overview = Some(overview);
    }

    /// The overview takes key `code`: moves the selection, picks, closes, or passes a typed
    /// character on to the launcher.
    pub(super) fn overview_key(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let Some(mut overview) = self.overview else {
            return Action::None;
        };
        if !pressed {
            return Action::None;
        }
        let grid = self.overview_grid();
        let held = Modifiers::from_bits(modifiers.bits() & !Modifiers::CAPS_LOCK.bits());
        let selected_slot = match overview.selection {
            OverviewSelection::Thumbnail(index) if index < grid.length => {
                Some(usize::from(grid.slots[index]))
            }
            _ => None,
        };
        if let Some(found) = shortcut(code, modifiers, self.profile, false) {
            match found {
                Shortcut::Overview => self.close_overview(None, damage, events),
                Shortcut::SwitchDesktop { next } => {
                    let wanted = match next {
                        true => self.current_desktop.checked_add(1),
                        false => self.current_desktop.checked_sub(1),
                    };
                    if let Some(wanted) = wanted
                        && self.switch_desktop(wanted, damage, events).is_ok()
                    {
                        self.reset_overview_selection(damage);
                    }
                }
                Shortcut::NewDesktop => {
                    if self.new_desktop(damage, events).is_ok() {
                        self.reset_overview_selection(damage);
                    }
                }
                Shortcut::MoveToDesktop { next } => {
                    let wanted = match next {
                        true => self.current_desktop.checked_add(1),
                        false => self.current_desktop.checked_sub(1),
                    };
                    if let (Some(slot), Some(wanted)) = (selected_slot, wanted)
                        && self
                            .move_to_desktop(slot, wanted, false, damage, events)
                            .is_ok()
                    {
                        self.reset_overview_selection(damage);
                    }
                }
                Shortcut::Switch { .. } | Shortcut::Launcher => {
                    self.close_overview(None, damage, events);
                    return self.run_shortcut(found, damage, events);
                }
                _ => {}
            }
            return Action::None;
        }
        let last = grid.length.saturating_sub(1);
        let tiles = self.strip_tiles();
        match (code, overview.selection) {
            (key::ESC, _) => {
                self.close_overview(None, damage, events);
                return Action::None;
            }
            (key::TAB, OverviewSelection::Thumbnail(index)) if grid.length > 0 => {
                let back = held.contains(Modifiers::SHIFT);
                overview.selection = OverviewSelection::Thumbnail(match back {
                    true => index.checked_sub(1).unwrap_or(last),
                    false => (index + 1) % grid.length,
                });
            }
            (key::LEFT, OverviewSelection::Thumbnail(index)) => {
                overview.selection = OverviewSelection::Thumbnail(index.saturating_sub(1));
            }
            (key::RIGHT, OverviewSelection::Thumbnail(index)) => {
                overview.selection = OverviewSelection::Thumbnail((index + 1).min(last));
            }
            (key::HOME, OverviewSelection::Thumbnail(_)) => {
                overview.selection = OverviewSelection::Thumbnail(0);
            }
            (key::END, OverviewSelection::Thumbnail(_)) => {
                overview.selection = OverviewSelection::Thumbnail(last);
            }
            (key::UP, OverviewSelection::Thumbnail(index)) => {
                let columns = grid.columns as usize;
                overview.selection = match index.checked_sub(columns) {
                    Some(above) => OverviewSelection::Thumbnail(above),
                    None => {
                        overview.left_grid_at = index;
                        OverviewSelection::Tile(self.current_desktop)
                    }
                };
            }
            (key::DOWN, OverviewSelection::Thumbnail(index)) => {
                let below = index + grid.columns as usize;
                if below <= last {
                    overview.selection = OverviewSelection::Thumbnail(below);
                } else if index / (grid.columns as usize) < last / (grid.columns as usize) {
                    overview.selection = OverviewSelection::Thumbnail(last);
                }
            }
            (key::DOWN, OverviewSelection::Tile(_)) if grid.length > 0 => {
                overview.selection = OverviewSelection::Thumbnail(overview.left_grid_at.min(last));
            }
            (key::LEFT, OverviewSelection::Tile(tile)) => {
                overview.selection = OverviewSelection::Tile(tile.saturating_sub(1));
            }
            (key::RIGHT, OverviewSelection::Tile(tile)) => {
                overview.selection = OverviewSelection::Tile((tile + 1).min(tiles - 1));
            }
            (key::ENTER | key::KEYPAD_ENTER | key::SPACE, OverviewSelection::Thumbnail(_)) => {
                if let Some(slot) = selected_slot {
                    self.close_overview(Some(slot), damage, events);
                }
                return Action::None;
            }
            (key::ENTER | key::KEYPAD_ENTER | key::SPACE, OverviewSelection::Tile(tile)) => {
                self.pick_tile(tile, damage, events);
                return Action::None;
            }
            (key::DELETE, OverviewSelection::Thumbnail(_)) => {
                if let Some(slot) = selected_slot {
                    self.request_close(slot, events);
                }
                return Action::None;
            }
            (key::DELETE, OverviewSelection::Tile(tile)) if tile < self.desktops => {
                if self.close_desktop(tile, damage, events).is_ok() {
                    self.reset_overview_selection(damage);
                }
                return Action::None;
            }
            _ => {
                let typed = self.typed;
                let plain = held.bits()
                    & (Modifiers::CTRL.bits() | Modifiers::ALT.bits() | Modifiers::SUPER.bits())
                    == 0;
                if plain && typed >= u32::from(b' ') && typed != 0x7f {
                    self.close_overview(None, damage, events);
                    self.toggle_launcher(damage, events);
                    return self.launcher_key(code, pressed, modifiers, damage, events);
                }
                return Action::None;
            }
        }
        self.set_overview(overview, damage);
        Action::None
    }

    fn pick_tile(&mut self, tile: u8, damage: &mut Damage, events: &mut dyn EventSink) {
        match tile < self.desktops {
            true => {
                let _ = self.switch_desktop(tile, damage, events);
            }
            false => {
                let _ = self.new_desktop(damage, events);
            }
        }
        self.close_overview(None, damage, events);
        self.focus_top(damage, events);
    }

    /// A button press at (`x`, `y`) while the overview is open: `Some` when the overview took
    /// it, `None` when it closed for a press on a panel, which then goes on as usual.
    pub(super) fn overview_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let mut overview = self.overview?;
        let on_panel = self.layout.panels().any(|panel| contains(panel, x, y));
        let toggles = match self.panel_item_at(x, y) {
            Some(super::shell::PanelItem::TaskView) => true,
            Some(super::shell::PanelItem::System) => self.profile == super::Profile::Linux,
            _ => false,
        };
        if toggles {
            self.close_overview(None, damage, events);
            return Some(Action::None);
        }
        let hit = self.overview_hit(x, y);
        if code != crate::input::button::LEFT {
            return Some(Action::None);
        }
        match hit {
            None => {
                self.close_overview(None, damage, events);
                return match on_panel {
                    true => None,
                    false => Some(Action::None),
                };
            }
            Some(OverviewItem::TileClose(desktop)) => {
                if self.close_desktop(desktop, damage, events).is_ok() {
                    self.reset_overview_selection(damage);
                }
                return Some(Action::None);
            }
            Some(OverviewItem::ThumbnailClose(slot)) => {
                self.request_close(usize::from(slot), events);
                return Some(Action::None);
            }
            Some(item) => {
                let origin = match item {
                    OverviewItem::Thumbnail(slot) => {
                        let grid = self.overview_grid();
                        let index = grid.slots[..grid.length]
                            .iter()
                            .position(|&known| known == slot)
                            .unwrap_or(0);
                        overview.selection = OverviewSelection::Thumbnail(index);
                        let (thumbnail, _) = self.thumbnail(&grid, index);
                        (thumbnail.x, thumbnail.y)
                    }
                    _ => (x, y),
                };
                overview.press = Some(Press {
                    item,
                    at: (x, y),
                    offset: (x.saturating_sub(origin.0), y.saturating_sub(origin.1)),
                });
            }
        }
        self.set_overview(overview, damage);
        Some(Action::None)
    }

    /// The pointer moved to (`x`, `y`) while the overview is open: hover, and a thumbnail's
    /// drag once it went past 4 pixels.
    pub(super) fn overview_motion(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let Some(mut overview) = self.overview else {
            return;
        };
        overview.hover = self.overview_hit(x, y);
        if let Some(press) = overview.press
            && matches!(press.item, OverviewItem::Thumbnail(_))
            && (overview.dragging.is_some()
                || x.abs_diff(press.at.0) > DRAG_DISTANCE
                || y.abs_diff(press.at.1) > DRAG_DISTANCE)
        {
            overview.dragging = Some((x, y));
        }
        self.set_overview(overview, damage);
    }

    /// The left button went up at (`x`, `y`) while the overview is open: a click picks what it
    /// was pressed on, a drag drops the window on the desktop under the pointer.
    pub(super) fn overview_release(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let Some(mut overview) = self.overview else {
            return;
        };
        let Some(press) = overview.press.take() else {
            return;
        };
        let dragged = overview.dragging.take();
        self.set_overview(overview, damage);
        damage.add(self.work_area());
        match (press.item, dragged) {
            (OverviewItem::Thumbnail(slot), Some(_)) => {
                let slot = usize::from(slot);
                match self.overview_hit(x, y) {
                    Some(OverviewItem::Tile(desktop) | OverviewItem::TileClose(desktop)) => {
                        let _ = self.move_to_desktop(slot, desktop, false, damage, events);
                    }
                    Some(OverviewItem::NewTile) => {
                        let shown = self.current_desktop;
                        if let Ok(made) = self.new_desktop(damage, events) {
                            let _ = self.switch_desktop(shown, damage, events);
                            let _ = self.move_to_desktop(slot, made, false, damage, events);
                        }
                    }
                    _ => {}
                }
                self.reset_overview_selection(damage);
            }
            (item, None) if self.overview_hit(x, y) == Some(item) => match item {
                OverviewItem::Thumbnail(slot) => {
                    self.close_overview(Some(usize::from(slot)), damage, events);
                }
                OverviewItem::Tile(desktop) => self.pick_tile(desktop, damage, events),
                OverviewItem::NewTile => self.pick_tile(self.desktops, damage, events),
                _ => {}
            },
            _ => {}
        }
    }

    /// A window committed while the overview is open: its thumbnail is somewhere in the grid.
    pub(super) fn overview_commit(&self, damage: &mut Damage) {
        damage.add(self.overview_grid_area());
    }

    pub(super) fn draw_overview(&self, canvas: &mut Canvas<'_>, contents: &dyn Contents) {
        let Some(overview) = self.overview else {
            return;
        };
        let hover = overview.hover;
        let work = self.work_area();
        for index in 0..self.strip_tiles() {
            let tile = self.tile_rect(index);
            let selected = overview.selection == OverviewSelection::Tile(index);
            if index >= self.desktops {
                let fill = match hover == Some(OverviewItem::NewTile) {
                    true => colour::BAR_PRESSED,
                    false => colour::BAR_HOVER,
                };
                canvas.rounded(tile, &paint::INSET_R4, true, true, fill);
                canvas.character_doubled(tile.x + PLUS_AT.0, tile.y + PLUS_AT.1, '+', colour::TEXT);
            } else {
                self.draw_desktop_tile(canvas, tile, index, work, hover);
                let label = self.tile_label_rect(index);
                canvas.rounded(label, &paint::INSET_R4, true, true, colour::BAR);
                let mut text = *b"Desktop 0";
                text[8] = b'1' + index;
                let ink = match index == self.current_desktop {
                    true => colour::TEXT,
                    false => colour::TEXT_DIM,
                };
                canvas.text(
                    label.x + LABEL_PADDING,
                    label.y + (LABEL_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                    core::str::from_utf8(&text).unwrap_or(""),
                    ink,
                );
            }
            if let Some((x, y)) = overview.dragging
                && contains(tile, x, y)
            {
                outline(canvas, tile, RING, colour::FOCUS_RING);
            }
            if selected {
                outline(canvas, grown(tile, RING), RING, colour::FOCUS_RING);
            }
        }
        let grid = self.overview_grid();
        if grid.length == 0 {
            let width = EMPTY_TEXT.len() as u32 * GLYPH_WIDTH as u32;
            canvas.text(
                grid.area.x + grid.area.width.saturating_sub(width) / 2,
                grid.area.y + grid.area.height.saturating_sub(GLYPH_HEIGHT as u32) / 2,
                EMPTY_TEXT,
                colour::TEXT_DIM,
            );
        }
        for index in 0..grid.length {
            let slot = usize::from(grid.slots[index]);
            let (thumbnail, scale) = self.thumbnail(&grid, index);
            self.draw_thumbnail(canvas, slot, thumbnail, scale, contents);
            let ring = match (
                overview.selection == OverviewSelection::Thumbnail(index),
                self.focus == Some(slot),
            ) {
                (true, _) => (RING, colour::FOCUS_RING),
                (false, true) => (1, colour::BORDER_ACTIVE),
                (false, false) => (1, colour::BORDER_INACTIVE),
            };
            outline(canvas, grown(thumbnail, ring.0), ring.0, ring.1);
            self.draw_thumbnail_label(canvas, &grid, index, thumbnail);
            let hovered = matches!(
                hover,
                Some(OverviewItem::Thumbnail(at) | OverviewItem::ThumbnailClose(at))
                    if usize::from(at) == slot
            );
            if hovered && self.window_at(slot).closable {
                let (circle, _) = close_circle(thumbnail.right(), thumbnail.y);
                let pressed = hover == Some(OverviewItem::ThumbnailClose(grid.slots[index]));
                draw_close(canvas, circle, pressed);
            }
        }
        if let (Some((x, y)), Some(press)) = (overview.dragging, overview.press)
            && let OverviewItem::Thumbnail(slot) = press.item
            && let Some(index) = grid.slots[..grid.length].iter().position(|&known| known == slot)
        {
            let (thumbnail, _) = self.thumbnail(&grid, index);
            outline(
                canvas,
                Rect::new(
                    x.saturating_sub(press.offset.0),
                    y.saturating_sub(press.offset.1),
                    thumbnail.width,
                    thumbnail.height,
                ),
                1,
                colour::ACCENT,
            );
        }
    }

    fn draw_desktop_tile(
        &self,
        canvas: &mut Canvas<'_>,
        tile: Rect,
        desktop: u8,
        work: Rect,
        hover: Option<OverviewItem>,
    ) {
        for row in 0..tile.height {
            let source = work.y + row * work.height / tile.height;
            let gradient = self
                .gradient
                .get(source as usize)
                .copied()
                .unwrap_or(colour::WALLPAPER_TOP);
            let inset = paint::INSET_R4
                .get(row as usize)
                .or_else(|| paint::INSET_R4.get((tile.height - 1 - row) as usize))
                .copied()
                .unwrap_or(0);
            canvas.row(tile.y + row, tile.x + inset, tile.right() - inset, gradient);
        }
        for &slot in &self.stack[..self.stack_length] {
            let window = self.window_at(usize::from(slot));
            if window.role != Role::Normal
                || window.parent.is_some()
                || window.minimised
                || !window.shown
                || window.desktop != desktop
            {
                continue;
            }
            let outside = window.outside();
            let x = outside.x.saturating_sub(work.x) * tile.width / work.width.max(1);
            let y = outside.y.saturating_sub(work.y) * tile.height / work.height.max(1);
            let width = (outside.width * tile.width / work.width.max(1)).max(2);
            let height = (outside.height * tile.height / work.height.max(1)).max(2);
            let mini = Rect::new(
                tile.x + x.min(tile.width - 1),
                tile.y + y.min(tile.height - 1),
                width.min(tile.width - x.min(tile.width - 1)),
                height.min(tile.height - y.min(tile.height - 1)),
            );
            canvas.fill(mini, colour::TITLE_ACTIVE);
            outline(canvas, mini, 1, colour::BORDER_ACTIVE);
        }
        let hovered = matches!(
            hover,
            Some(OverviewItem::Tile(at) | OverviewItem::TileClose(at)) if at == desktop
        );
        let (thickness, ring) = match (desktop == self.current_desktop, hovered) {
            (true, _) => (2, colour::ACCENT),
            (false, true) => (1, colour::TEXT_DIM),
            (false, false) => (1, colour::MENU_BORDER),
        };
        outline(canvas, tile, thickness, ring);
        if hovered && self.desktops > 1 {
            let (circle, _) = close_circle(tile.right(), tile.y);
            draw_close(
                canvas,
                circle,
                hover == Some(OverviewItem::TileClose(desktop)),
            );
        }
    }

    fn draw_thumbnail(
        &self,
        canvas: &mut Canvas<'_>,
        slot: usize,
        thumbnail: Rect,
        scale: u32,
        contents: &dyn Contents,
    ) {
        let window = self.window_at(slot);
        let clip = canvas.clip();
        let top = thumbnail.y.max(clip.y);
        let bottom = thumbnail.bottom().min(clip.bottom());
        let left = thumbnail.x.max(clip.x);
        let right = thumbnail.right().min(clip.right());
        if left >= right {
            return;
        }
        let buffer = window.buffer;
        let mut run = [0u32; SAMPLE_RUN];
        for row in top..bottom {
            let source_y = (row - thumbnail.y) * scale;
            let in_rows = window.ready && source_y >= buffer.y && source_y < buffer.bottom();
            let Some(out) = canvas.row_pixels(row, left, right) else {
                continue;
            };
            let mut column = left;
            while column < right {
                let source_x = (column - thumbnail.x) * scale;
                if !(in_rows && source_x >= buffer.x && source_x < buffer.right()) {
                    out[(column - left) as usize] = colour::WINDOW_BACKGROUND;
                    column += 1;
                    continue;
                }
                let at = source_x - buffer.x;
                let span = (SAMPLE_RUN as u32).min(buffer.width - at);
                contents.copy_row(window.id, at, source_y - buffer.y, &mut run[..span as usize]);
                let mut offset = 0;
                while offset < span && column < right {
                    out[(column - left) as usize] = run[offset as usize];
                    offset += scale;
                    column += 1;
                }
            }
        }
    }

    fn draw_thumbnail_label(&self, canvas: &mut Canvas<'_>, grid: &Grid, index: usize, thumbnail: Rect) {
        let window = self.window_at(usize::from(grid.slots[index]));
        let room = (grid.cell.0.saturating_sub(16) / GLYPH_WIDTH as u32) as usize;
        let (text, cut) = cut_title(window.title(), room.saturating_sub(3));
        if text.is_empty() {
            return;
        }
        let tail = if cut { ellipsis() } else { "" };
        let glyphs = (text.chars().count() + tail.len()) as u32;
        let width = glyphs * GLYPH_WIDTH as u32 + 2 * LABEL_PADDING;
        let cell = grid.cell_rect(index);
        let label = Rect::new(
            cell.x + cell.width.saturating_sub(width) / 2,
            thumbnail.bottom() + LABEL_GAP,
            width,
            LABEL_HEIGHT,
        );
        canvas.rounded(label, &paint::INSET_R4, true, true, colour::BAR);
        let ink = match window.minimised {
            true => colour::TEXT_DIM,
            false => colour::TEXT,
        };
        let y = label.y + (LABEL_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        canvas.text(label.x + LABEL_PADDING, y, text, ink);
        let after = label.x + LABEL_PADDING + text.chars().count() as u32 * GLYPH_WIDTH as u32;
        canvas.text(after, y, tail, ink);
    }

    /// Whether the overview covers `clip`, so no window's content can stand for it.
    pub(super) fn overview_covers(&self, clip: Rect) -> bool {
        self.overview.is_some() && self.work_area().overlaps(&clip)
    }
}

fn draw_close(canvas: &mut Canvas<'_>, circle: Rect, pressed: bool) {
    let fill = match pressed {
        true => paint::quarter(colour::BUTTON_CLOSE),
        false => colour::BUTTON_CLOSE,
    };
    canvas.rounded(circle, &paint::INSET_R6, true, true, fill);
    for (row, bits) in (0..).zip(&super::CLOSE_GLYPH) {
        for column in 0..super::CONTROL_GLYPH_SIZE {
            if bits & (1 << (super::CONTROL_GLYPH_SIZE - 1 - column)) != 0 {
                canvas.pixel(
                    circle.x + super::CONTROL_GLYPH_OFFSET + column,
                    circle.y + super::CONTROL_GLYPH_OFFSET + row,
                    colour::GLYPH,
                );
            }
        }
    }
}
