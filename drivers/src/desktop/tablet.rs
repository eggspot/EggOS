//! The tablet form, slice 3 (docs/ux/adaptive-shell-tablet-v0.5.md, D306): the phone's status
//! bar and home screen at tablet numbers, a 64 px dock (`Back`, `Home`, `Apps`, then the
//! pinned apps and the apps with windows, centred), title buttons that open the window menu,
//! and windows full or split at a third, a half or two thirds with a divider; the side waiting
//! for a second app shows the apps to choose from.

use super::catalogue::AppId;
use super::form::Form;
use super::icons::{IconSize, draw_icon};
use super::paint::{self, Canvas};
use super::protocol::{EventSink, Role};
use super::windows::{self, Geometry};
use super::{Action, Desktop, MAX_WINDOWS, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use alloc::vec::Vec;

/// The left side's share of a pair (section 5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Share {
    Third,
    Half,
    TwoThirds,
}

/// Two windows side by side; a side is `None` while the chooser waits for its app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pair {
    pub(super) left: Option<u8>,
    pub(super) right: Option<u8>,
    pub(super) share: Share,
    pub(super) right_focused: bool,
}

/// The window menu of a title button (section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WindowMenu {
    slot: u8,
    hover: Option<usize>,
}

/// The tablet's own state: the pair, the open window menu, a divider being dragged.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TabletState {
    pub(super) pair: Option<Pair>,
    menu: Option<WindowMenu>,
    dragging: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    FullScreen,
    SplitLeft,
    SplitRight,
    MakeWider,
    MakeNarrower,
    SwapSides,
    Close,
}

const DIVIDER: u32 = 8;
const NARROWEST_SIDE: u32 = 320;
const DOCK_ZONE: u32 = 64;
const DOCK_ZONES: u32 = 3;
const DOCK_ICON: (u32, u32) = (8, 6);
const DOCK_RUNNING: (u32, u32, u32) = (30, 56, 4);
const DOCK_FOCUS: (u32, u32, u32, u32) = (24, 60, 16, 2);
const DOCK_SEPARATOR: u32 = 17;
const DOCK_CLEAR: u32 = 16;
const PINNED: [AppId; 3] = [AppId::Terminal, AppId::Files, AppId::Settings];
const TITLE_MIN: u32 = 96;
const TITLE_PADDING: u32 = 32;
const TITLE_LEFT_LIMIT: u32 = 72;
const TITLE_TEXT_Y: u32 = 14;
const CARET: [u32; 4] = [7, 5, 3, 1];
const CARET_TOP: u32 = 20;
const CARET_GAP: u32 = 6;
const TITLE_LINE: (u32, u32) = (34, 2);
const TITLE_GLYPHS: usize = 40;
const MENU_WIDTH: u32 = 200;
const MENU_ROW: u32 = 44;
const MENU_TOP: u32 = 48;
const MENU_PAD: u32 = 4;
const MENU_TEXT: (u32, u32) = (16, 14);
const GRIP: (u32, u32) = (4, 48);
const SNAP_EDGE: u32 = 160;
const CHOOSER_HEADING: (u32, u32) = (16, 16);
const CHOOSER_TOP: u32 = 48;
const CELL: (u32, u32) = (112, 104);
const CELL_ICON: (u32, u32) = (32, 8);

impl Desktop {
    /// Whether the shell has one of the touch forms, whose windows have no chrome.
    pub(super) fn touch_form(&self) -> bool {
        matches!(self.form, Form::Phone | Form::Tablet)
    }

    fn third(&self) -> u32 {
        (self.layout.work_area.width.saturating_sub(DIVIDER) / 3) & !3
    }

    fn thirds_exist(&self) -> bool {
        self.third() >= NARROWEST_SIDE
    }

    fn left_width(&self, share: Share) -> u32 {
        let width = self.layout.work_area.width;
        match (share, self.thirds_exist()) {
            (Share::Third, true) => self.third(),
            (Share::TwoThirds, true) => width - DIVIDER - self.third(),
            _ => width.saturating_sub(DIVIDER) / 2,
        }
    }

    /// Side `right`'s rect for a pair of `share`.
    pub(super) fn side_rect(&self, right: bool, share: Share) -> Rect {
        let work = self.layout.work_area;
        let left = self.left_width(share);
        match right {
            false => Rect::new(work.x, work.y, left, work.height),
            true => Rect::new(
                work.x + left + DIVIDER,
                work.y,
                work.width - left - DIVIDER,
                work.height,
            ),
        }
    }

    fn divider_rect(&self, share: Share) -> Rect {
        let work = self.layout.work_area;
        Rect::new(
            work.x + self.left_width(share),
            work.y,
            DIVIDER,
            work.height,
        )
    }

    fn divider_pad(&self, share: Share) -> Rect {
        let divider = self.divider_rect(share);
        let middle = divider.y + divider.height / 2;
        Rect::new(divider.x.saturating_sub(18), middle - 44, 44, 88)
    }

    /// The pair, if both its windows still exist (or a side waits for the chooser).
    pub(super) fn live_pair(&self) -> Option<Pair> {
        let pair = self.tablet.pair?;
        let alive = |side: Option<u8>| {
            side.is_none_or(|slot| {
                self.windows
                    .get(usize::from(slot))
                    .is_some_and(Option::is_some)
            })
        };
        (alive(pair.left) && alive(pair.right) && (pair.left.is_some() || pair.right.is_some()))
            .then_some(pair)
    }

    /// Whether the pair is on screen: both windows shown, or one and the chooser.
    pub(super) fn pair_shown(&self) -> bool {
        self.live_pair().is_some_and(|pair| {
            [pair.left, pair.right]
                .into_iter()
                .flatten()
                .all(|slot| self.is_visible(usize::from(slot)))
        })
    }

    /// The pair's side of `slot`, if it has one.
    pub(super) fn pair_side(&self, slot: usize) -> Option<bool> {
        let pair = self.live_pair()?;
        match (pair.left, pair.right) {
            (Some(left), _) if usize::from(left) == slot => Some(false),
            (_, Some(right)) if usize::from(right) == slot => Some(true),
            _ => None,
        }
    }

    /// Where window `slot` goes in the tablet form: its side of the pair, else the work area.
    pub(super) fn tablet_geometry(&self, slot: usize) -> Option<Geometry> {
        let side = self.pair_side(slot)?;
        let share = self.live_pair()?.share;
        let rect = self.side_rect(side, share);
        Some(Geometry {
            x: rect.x as i32 - 1,
            y: rect.y.saturating_sub(windows::CHROME.1 - 1),
            content_width: rect.width,
            content_height: rect.height,
        })
    }

    /// A normal window is about to be shown in the tablet form (section 5.2): it takes the
    /// side the chooser waits on, or the focused side of the shown pair; the window it
    /// replaces is the one to minimise. `None`: it is full.
    pub(super) fn tablet_place(&mut self, slot: usize) -> Option<Option<usize>> {
        let mut pair = self.live_pair()?;
        if let Some(side) = self.pair_side(slot) {
            pair.right_focused = side;
            self.tablet.pair = Some(pair);
            return Some(None);
        }
        if pair.left.is_none() {
            pair.left = Some(slot as u8);
            pair.right_focused = false;
            self.tablet.pair = Some(pair);
            return Some(None);
        }
        if pair.right.is_none() {
            pair.right = Some(slot as u8);
            pair.right_focused = true;
            self.tablet.pair = Some(pair);
            return Some(None);
        }
        if !self.pair_shown() {
            self.tablet.pair = None;
            return None;
        }
        let replaced = match pair.right_focused {
            true => pair.right.replace(slot as u8),
            false => pair.left.replace(slot as u8),
        };
        self.tablet.pair = Some(pair);
        Some(replaced.map(usize::from))
    }

    /// The other window of the pair `slot` belongs to.
    pub(super) fn pair_partner(&self, slot: usize) -> Option<usize> {
        let pair = self.live_pair()?;
        match self.pair_side(slot)? {
            false => pair.right.map(usize::from),
            true => pair.left.map(usize::from),
        }
    }

    fn refit_pair(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(pair) = self.live_pair() else {
            return;
        };
        for slot in [pair.left, pair.right].into_iter().flatten() {
            let slot = usize::from(slot);
            if let Some(geometry) = self.tablet_geometry(slot) {
                let old = self.window_at(slot).with_shadow();
                self.apply_geometry(slot, geometry, old, damage, events);
            }
        }
        damage.add(self.layout.work_area);
    }

    fn end_pair(&mut self, keep: Option<usize>, damage: &mut Damage, events: &mut dyn EventSink) {
        self.tablet.pair = None;
        if let Some(slot) = keep {
            self.fit_to_phone(slot, damage, events);
        }
        damage.add(self.screen());
    }

    /// Keeps the pair true after windows went away: one left alone becomes full.
    pub(super) fn tidy_pair(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(pair) = self.tablet.pair else {
            return;
        };
        if self.live_pair().is_some() {
            let minimised = |side: Option<u8>| {
                side.is_some_and(|slot| self.window_at(usize::from(slot)).minimised)
            };
            if minimised(pair.left) != minimised(pair.right)
                && pair.left.is_some()
                && pair.right.is_some()
            {
                let keep = [pair.left, pair.right]
                    .into_iter()
                    .flatten()
                    .map(usize::from)
                    .find(|&slot| !self.window_at(slot).minimised);
                self.end_pair(keep, damage, events);
            }
            return;
        }
        let keep = [pair.left, pair.right]
            .into_iter()
            .flatten()
            .map(usize::from)
            .find(|&slot| self.windows.get(slot).is_some_and(Option::is_some));
        self.end_pair(keep, damage, events);
    }

    fn dock_zone(&self, index: u32) -> Option<Rect> {
        let dock = self.layout.navigation_bar?;
        Some(Rect::new(
            dock.x + DOCK_ZONE * index,
            dock.y,
            DOCK_ZONE,
            dock.height,
        ))
    }

    /// The tablet's navigation zone `index`: `Back`, `Home`, `Apps`.
    pub(super) fn tablet_navigation_zone(&self, index: u32) -> Option<Rect> {
        (index < DOCK_ZONES)
            .then(|| self.dock_zone(index))
            .flatten()
    }

    fn dock_apps(&self) -> Vec<AppId> {
        let mut apps: Vec<AppId> = PINNED
            .into_iter()
            .filter(|app| self.catalogue.has(*app))
            .collect();
        let fits = (self.width.saturating_sub(433) / DOCK_ZONE) as usize;
        let mut order = [0u8; MAX_WINDOWS];
        let count = self.switcher_order(&mut order);
        let mut running: Vec<AppId> = Vec::new();
        for &slot in order[..count].iter().rev() {
            if let Some(app) = self.app_of(usize::from(slot))
                && !apps.contains(&app)
                && !running.contains(&app)
            {
                running.push(app);
            }
        }
        while apps.len() + running.len() > fits && !running.is_empty() {
            running.remove(0);
        }
        apps.extend(running);
        apps
    }

    fn dock_targets(&self) -> Vec<(AppId, Rect)> {
        let Some(dock) = self.layout.navigation_bar else {
            return Vec::new();
        };
        let apps = self.dock_apps();
        let pinned = apps.iter().take_while(|app| PINNED.contains(app)).count();
        let separator = if apps.len() > pinned {
            DOCK_SEPARATOR
        } else {
            0
        };
        let width = apps.len() as u32 * DOCK_ZONE + separator;
        let mut x = self.width.saturating_sub(width) / 2;
        let mut targets = Vec::new();
        for (index, app) in apps.into_iter().enumerate() {
            if index == pinned && separator > 0 {
                x += separator;
            }
            targets.push((app, Rect::new(x, dock.y, DOCK_ZONE, DOCK_ZONE)));
            x += DOCK_ZONE;
        }
        targets
    }

    fn title_button(&self, slot: usize) -> Rect {
        let window = self.window_at(slot);
        let glyphs = window.title().chars().count().min(TITLE_GLYPHS) as u32;
        let width = (glyphs * GLYPH_WIDTH as u32 + 13 + TITLE_PADDING).max(TITLE_MIN);
        let centre = match (self.pair_side(slot), self.live_pair()) {
            (Some(side), Some(pair)) => {
                let rect = self.side_rect(side, pair.share);
                rect.x + rect.width / 2
            }
            _ => self.width / 2,
        };
        let right_limit = self.width.saturating_sub(16 + 16 + 8 + 56 + 8);
        let x = centre
            .saturating_sub(width / 2)
            .clamp(TITLE_LEFT_LIMIT, right_limit.saturating_sub(width));
        Rect::new(x, 0, width, 44)
    }

    fn tablet_shown_windows(&self) -> Vec<usize> {
        match self.live_pair().filter(|_| self.pair_shown()) {
            Some(pair) => [pair.left, pair.right]
                .into_iter()
                .flatten()
                .map(usize::from)
                .collect(),
            None => self.phone_shown_window().into_iter().collect(),
        }
    }

    fn focused_side_slot(&self) -> Option<usize> {
        let pair = self.live_pair()?;
        match pair.right_focused {
            true => pair.right.map(usize::from),
            false => pair.left.map(usize::from),
        }
    }

    /// Draws the title buttons in the status bar.
    pub(super) fn draw_title_buttons(&self, canvas: &mut Canvas<'_>) {
        let focused = self.focused_side_slot().or(self.phone_shown_window());
        for slot in self.tablet_shown_windows() {
            let button = self.title_button(slot);
            let open = self
                .tablet
                .menu
                .is_some_and(|menu| usize::from(menu.slot) == slot);
            if open {
                canvas.fill(
                    Rect::new(button.x, button.y, button.width, button.height - 1),
                    colour::BAR_PRESSED,
                );
            }
            let is_focused = Some(slot) == focused;
            let ink = if is_focused || open {
                colour::TEXT
            } else {
                colour::TEXT_DIM
            };
            let (text, cut) =
                windows::cut_title(self.window_at(slot).title(), TITLE_GLYPHS.saturating_sub(3));
            let text_width =
                (text.chars().count() as u32 + if cut { 3 } else { 0 }) * GLYPH_WIDTH as u32;
            let x = button.x + (button.width.saturating_sub(text_width + 13)) / 2;
            canvas.text(x, TITLE_TEXT_Y, text, ink);
            if cut {
                canvas.text(
                    x + text.chars().count() as u32 * GLYPH_WIDTH as u32,
                    TITLE_TEXT_Y,
                    windows::ellipsis(),
                    ink,
                );
            }
            let caret_x = x + text_width + CARET_GAP;
            for (row, width) in CARET.into_iter().enumerate() {
                canvas.fill(
                    Rect::new(caret_x + (7 - width) / 2, CARET_TOP + row as u32, width, 1),
                    ink,
                );
            }
            if is_focused {
                canvas.fill(
                    Rect::new(x, TITLE_LINE.0, text_width, TITLE_LINE.1),
                    colour::ACCENT,
                );
            }
        }
    }

    /// Draws the dock's app group, under the three navigation zones the phone code draws.
    pub(super) fn draw_dock_apps(&self, canvas: &mut Canvas<'_>) {
        let focused_app = self.focused_normal().and_then(|slot| self.app_of(slot));
        let mut previous_pinned = true;
        for (app, target) in self.dock_targets() {
            let pinned = PINNED.contains(&app);
            if previous_pinned && !pinned {
                canvas.fill(
                    Rect::new(target.x - DOCK_SEPARATOR + 8, target.y + 12, 1, 40),
                    colour::MENU_BORDER,
                );
            }
            previous_pinned = pinned;
            draw_icon(
                canvas,
                self.catalogue.icon(app),
                IconSize::Large,
                target.x + DOCK_ICON.0,
                target.y + DOCK_ICON.1,
            );
            let mut windows = [0u8; MAX_WINDOWS];
            if self.app_windows(app, &mut windows) > 0 {
                canvas.fill(
                    Rect::new(
                        target.x + DOCK_RUNNING.0,
                        target.y + DOCK_RUNNING.1,
                        DOCK_RUNNING.2,
                        DOCK_RUNNING.2,
                    ),
                    colour::TEXT,
                );
            }
            if Some(app) == focused_app {
                canvas.fill(
                    Rect::new(
                        target.x + DOCK_FOCUS.0,
                        target.y + DOCK_FOCUS.1,
                        DOCK_FOCUS.2,
                        DOCK_FOCUS.3,
                    ),
                    colour::ACCENT,
                );
            }
        }
        let _ = DOCK_CLEAR;
    }

    /// Draws the divider and, on a side waiting for its app, the chooser.
    pub(super) fn draw_pair(&self, canvas: &mut Canvas<'_>) {
        let Some(pair) = self.live_pair().filter(|_| self.pair_shown()) else {
            return;
        };
        let divider = match self.tablet.dragging {
            Some(x) => Rect::new(
                x.saturating_sub(DIVIDER / 2),
                self.layout.work_area.y,
                DIVIDER,
                self.layout.work_area.height,
            ),
            None => self.divider_rect(pair.share),
        };
        let fill = if self.tablet.dragging.is_some() {
            colour::BAR_PRESSED
        } else {
            colour::BAR
        };
        canvas.fill(divider, fill);
        let waiting = pair.left.is_none() || pair.right.is_none();
        if !waiting {
            let grip = Rect::new(
                divider.x + (DIVIDER - GRIP.0) / 2,
                divider.y + (divider.height - GRIP.1) / 2,
                GRIP.0,
                GRIP.1,
            );
            let ink = if self.tablet.dragging.is_some() {
                colour::TEXT
            } else {
                colour::TEXT_DIM
            };
            canvas.fill(grip, ink);
            return;
        }
        let side = self.side_rect(pair.right.is_none(), pair.share);
        canvas.fill(side, colour::MENU);
        canvas.text(
            side.x + CHOOSER_HEADING.0,
            side.y + CHOOSER_HEADING.1,
            "Choose an app for this side",
            colour::TEXT_DIM,
        );
        for (app, cell) in self.chooser_cells(side) {
            draw_icon(
                canvas,
                self.catalogue.icon(app),
                IconSize::Large,
                cell.x + CELL_ICON.0,
                cell.y + CELL_ICON.1,
            );
            let (text, _) = windows::cut_title(self.catalogue.name(app), 12);
            canvas.text(cell.x + 8, cell.y + 64, text, colour::TEXT);
        }
    }

    fn chooser_cells(&self, side: Rect) -> Vec<(AppId, Rect)> {
        let columns = (side.width.saturating_sub(32) / CELL.0).max(1);
        let left = side.x + (side.width - columns * CELL.0) / 2;
        self.catalogue
            .desktop_apps()
            .enumerate()
            .map(|(index, app)| {
                let (column, row) = (index as u32 % columns, index as u32 / columns);
                (
                    app,
                    Rect::new(
                        left + CELL.0 * column,
                        side.y + CHOOSER_TOP + CELL.1 * row,
                        CELL.0,
                        CELL.1,
                    ),
                )
            })
            .filter(|(_, cell)| cell.bottom() <= side.bottom())
            .collect()
    }

    fn menu_entries(&self, slot: usize) -> Vec<(Entry, bool)> {
        let side = self.pair_side(slot);
        let share = self.live_pair().map(|pair| pair.share);
        let mut entries = alloc::vec![
            (Entry::FullScreen, side.is_some()),
            (Entry::SplitLeft, side != Some(false)),
            (Entry::SplitRight, side != Some(true)),
        ];
        if let (Some(right), Some(share)) = (side, share) {
            let mine = match (right, share) {
                (false, Share::Third) | (true, Share::TwoThirds) => 0,
                (_, Share::Half) => 1,
                _ => 2,
            };
            entries.push((Entry::MakeWider, self.thirds_exist() && mine < 2));
            entries.push((Entry::MakeNarrower, self.thirds_exist() && mine > 0));
            entries.push((Entry::SwapSides, true));
        }
        entries.push((Entry::Close, self.window_at(slot).closable));
        entries
    }

    fn menu_label(entry: Entry) -> &'static str {
        match entry {
            Entry::FullScreen => "Full screen",
            Entry::SplitLeft => "Split left",
            Entry::SplitRight => "Split right",
            Entry::MakeWider => "Make wider",
            Entry::MakeNarrower => "Make narrower",
            Entry::SwapSides => "Swap sides",
            Entry::Close => "Close",
        }
    }

    fn menu_rect(&self, menu: WindowMenu) -> Rect {
        let button = self.title_button(usize::from(menu.slot));
        let rows = self.menu_entries(usize::from(menu.slot)).len() as u32;
        let height = rows * MENU_ROW + 2 * MENU_PAD + 9;
        let x = button.x.min(self.width.saturating_sub(MENU_WIDTH + 8));
        Rect::new(x, MENU_TOP, MENU_WIDTH, height)
    }

    fn menu_row(&self, menu: WindowMenu, index: usize) -> Rect {
        let rect = self.menu_rect(menu);
        let entries = self.menu_entries(usize::from(menu.slot));
        let separator = if index + 1 == entries.len() { 9 } else { 0 };
        Rect::new(
            rect.x,
            rect.y + MENU_PAD + MENU_ROW * index as u32 + separator,
            MENU_WIDTH,
            MENU_ROW,
        )
    }

    /// Draws the open window menu.
    pub(super) fn draw_tablet_menu(&self, canvas: &mut Canvas<'_>) {
        let Some(menu) = self.tablet.menu else {
            return;
        };
        let rect = self.menu_rect(menu);
        canvas.shadow(rect);
        canvas.rounded(rect, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_R6,
            true,
            true,
            colour::MENU,
        );
        let entries = self.menu_entries(usize::from(menu.slot));
        for (index, (entry, applicable)) in entries.iter().enumerate() {
            let row = self.menu_row(menu, index);
            if index + 1 == entries.len() {
                canvas.fill(
                    Rect::new(rect.x + 4, row.y - 5, rect.width - 8, 1),
                    colour::MENU_BORDER,
                );
            }
            if menu.hover == Some(index) && *applicable {
                canvas.fill(
                    Rect::new(row.x + 1, row.y, row.width - 2, row.height),
                    colour::SELECTION,
                );
            }
            let ink = if *applicable {
                colour::TEXT
            } else {
                colour::TEXT_DIM
            };
            canvas.text(
                row.x + MENU_TEXT.0,
                row.y + MENU_TEXT.1,
                Self::menu_label(*entry),
                ink,
            );
        }
    }

    /// A press in the tablet form that the shell takes: the window menu, a title button, an
    /// app in the dock, the divider, the chooser. `None` leaves it to the phone code and the
    /// windows.
    pub(super) fn tablet_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        self.tidy_pair(damage, events);
        if let Some(menu) = self.tablet.menu.take() {
            damage.add(self.screen());
            let entries = self.menu_entries(usize::from(menu.slot));
            if let Some(index) =
                (0..entries.len()).find(|&index| contains(self.menu_row(menu, index), x, y))
                && entries[index].1
            {
                self.choose_entry(usize::from(menu.slot), entries[index].0, damage, events);
            }
            return Some(Action::None);
        }
        if self
            .layout
            .status_bar
            .is_some_and(|bar| contains(bar, x, y))
        {
            for slot in self.tablet_shown_windows() {
                if contains(self.title_button(slot), x, y) {
                    if let Some(side) = self.pair_side(slot)
                        && let Some(mut pair) = self.tablet.pair
                    {
                        pair.right_focused = side;
                        self.tablet.pair = Some(pair);
                    }
                    self.set_focus(Some(slot), damage, events);
                    self.tablet.menu = Some(WindowMenu {
                        slot: slot as u8,
                        hover: None,
                    });
                    damage.add(self.screen());
                    return Some(Action::None);
                }
            }
            return None;
        }
        if self
            .layout
            .navigation_bar
            .is_some_and(|bar| contains(bar, x, y))
        {
            if let Some((app, _)) = self
                .dock_targets()
                .into_iter()
                .find(|(_, target)| contains(*target, x, y))
            {
                return Some(self.phone_open_app(app, damage, events));
            }
            return None;
        }
        let pair = self.live_pair().filter(|_| self.pair_shown())?;
        if pair.left.is_none() || pair.right.is_none() {
            let side = self.side_rect(pair.right.is_none(), pair.share);
            if contains(side, x, y) {
                let chosen = self
                    .chooser_cells(side)
                    .into_iter()
                    .find(|(_, cell)| contains(*cell, x, y));
                return Some(match chosen {
                    Some((app, _)) => self.phone_open_app(app, damage, events),
                    None => Action::None,
                });
            }
            return None;
        }
        if contains(self.divider_rect(pair.share), x, y)
            || contains(self.divider_pad(pair.share), x, y)
        {
            self.tablet.dragging = Some(x);
            damage.add(self.layout.work_area);
            return Some(Action::None);
        }
        let side = contains(self.side_rect(true, pair.share), x, y);
        if let Some(mut pair) = self.tablet.pair {
            pair.right_focused = side;
            self.tablet.pair = Some(pair);
            damage.add(self.layout.status_bar.unwrap_or(self.screen()));
        }
        None
    }

    /// The pointer moved while the divider is dragged.
    pub(super) fn tablet_drag(&mut self, x: u32, damage: &mut Damage) -> bool {
        if self.tablet.dragging.is_none() {
            return false;
        }
        self.tablet.dragging = Some(x.min(self.width.saturating_sub(DIVIDER / 2)));
        damage.add(self.layout.work_area);
        true
    }

    /// The button went up: a dragged divider snaps to its nearest place, or ends the pair at
    /// an edge (section 8.1).
    pub(super) fn tablet_release(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(x) = self.tablet.dragging.take() else {
            return;
        };
        let Some(mut pair) = self.live_pair() else {
            return;
        };
        damage.add(self.screen());
        if x < SNAP_EDGE {
            let keep = pair.right.map(usize::from);
            if let Some(left) = pair.left {
                self.minimise(usize::from(left), damage, events);
            }
            self.end_pair(keep, damage, events);
            return;
        }
        if x + SNAP_EDGE > self.width {
            let keep = pair.left.map(usize::from);
            if let Some(right) = pair.right {
                self.minimise(usize::from(right), damage, events);
            }
            self.end_pair(keep, damage, events);
            return;
        }
        let shares = [Share::Third, Share::Half, Share::TwoThirds];
        pair.share = shares
            .into_iter()
            .filter(|share| self.thirds_exist() || *share == Share::Half)
            .min_by_key(|share| {
                let place = self.divider_rect(*share).x + DIVIDER / 2;
                let distance = place.abs_diff(x);
                (distance, u32::from(*share != Share::Half))
            })
            .unwrap_or(Share::Half);
        self.tablet.pair = Some(pair);
        self.refit_pair(damage, events);
    }

    fn choose_entry(
        &mut self,
        slot: usize,
        entry: Entry,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        match entry {
            Entry::FullScreen => {
                if let Some(partner) = self.pair_partner(slot) {
                    self.minimise(partner, damage, events);
                }
                self.end_pair(Some(slot), damage, events);
            }
            Entry::SplitLeft | Entry::SplitRight => {
                let right = entry == Entry::SplitRight;
                match (self.pair_side(slot), self.live_pair()) {
                    (Some(_), Some(mut pair)) => {
                        core::mem::swap(&mut pair.left, &mut pair.right);
                        pair.right_focused = right;
                        self.tablet.pair = Some(pair);
                    }
                    _ => {
                        let side = Some(slot as u8);
                        self.tablet.pair = Some(Pair {
                            left: if right { None } else { side },
                            right: if right { side } else { None },
                            share: Share::Half,
                            right_focused: right,
                        });
                    }
                }
                self.refit_pair(damage, events);
            }
            Entry::MakeWider | Entry::MakeNarrower => {
                let Some(mut pair) = self.live_pair() else {
                    return;
                };
                let right = self.pair_side(slot) == Some(true);
                let wider_left = (entry == Entry::MakeWider) != right;
                pair.share = match (pair.share, wider_left) {
                    (Share::Third, true) => Share::Half,
                    (Share::Half, true) => Share::TwoThirds,
                    (Share::Half, false) => Share::Third,
                    (Share::TwoThirds, false) => Share::Half,
                    (share, _) => share,
                };
                self.tablet.pair = Some(pair);
                self.refit_pair(damage, events);
            }
            Entry::SwapSides => {
                let Some(mut pair) = self.live_pair() else {
                    return;
                };
                core::mem::swap(&mut pair.left, &mut pair.right);
                pair.share = match pair.share {
                    Share::Third => Share::TwoThirds,
                    Share::TwoThirds => Share::Third,
                    Share::Half => Share::Half,
                };
                pair.right_focused = !pair.right_focused;
                self.tablet.pair = Some(pair);
                self.refit_pair(damage, events);
            }
            Entry::Close => self.request_close(slot, events),
        }
    }

    /// Leaving the tablet form with a pair shown: its two windows tile the work area's halves.
    pub(super) fn pair_to_halves(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(pair) = self.tablet.pair.take() else {
            return;
        };
        let work = self.work_area();
        for (side, right) in [(pair.left, false), (pair.right, true)] {
            let Some(slot) = side.map(usize::from) else {
                continue;
            };
            if self.windows.get(slot).is_none_or(Option::is_none) {
                continue;
            }
            let minimum = self.window_at(slot).minimum;
            let old = self.window_at(slot).with_shadow();
            self.window_mut(slot).maximised = false;
            self.apply_geometry(
                slot,
                windows::snapped(work, right, minimum),
                old,
                damage,
                events,
            );
        }
        let _ = Role::Normal;
    }
}
