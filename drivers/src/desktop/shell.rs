//! The shell's panels (docs/ux/desktop-v0.3-gui.md, sections 6.1, 6.4 and 11): the top bar of
//! the macOS and Linux profiles (the system item or `Activities`, the Linux profile's chips, the
//! clock, the power button), the Windows profile's taskbar (Start, one item per window, the
//! clock, the power button) and the macOS profile's dock (the pinned apps, running apps and
//! minimised windows), their hover and pressed states and tooltips, and what a click on each
//! does. Which windows belong to which app is decided by title: the terminal's window is
//! Terminal's, and a window titled like a catalogue app is that app's.

use super::catalogue::AppId;
use super::icons::{Icon, IconSize, draw_icon};
use super::paint::{self, Canvas, egg_half_width};
use super::protocol::{EventSink, MAX_WINDOWS, Role};
use super::shortcuts::Profile;
use super::{
    Action, BAR_MARK_HEIGHT, BAR_MARK_X, BAR_TEXT_Y, CLOCK_GLYPHS, Desktop, MENU_GAP,
    MENU_ITEM_HEIGHT, MENU_ITEMS, MENU_WIDTH, POWER_BUTTON_MARGIN, POWER_ICON, POWER_ICON_SIZE,
    SYSTEM_NAME, SYSTEM_NAME_X, TERMINAL_SLOT, WindowId, colour, contains, round_percent, span,
};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};

/// Side of a taskbar item.
pub const TASKBAR_ITEM: u32 = 40;
/// Side of a dock icon.
pub const DOCK_ICON: u32 = 48;
/// How long the pointer rests on a taskbar or dock item before its tooltip shows.
pub const TOOLTIP_DELAY_MS: u32 = 600;

const SYSTEM_HIT_WIDTH: u32 = 96;
const ACTIVITIES: &str = "Activities";
const ACTIVITIES_X: u32 = 12;
const TASKBAR_GAP: u32 = 4;
const TASKBAR_MARGIN: u32 = 8;
const TASKBAR_CLOCK_FROM_RIGHT: u32 = 56;
const TASKBAR_ROOM_BEFORE_CLOCK: u32 = 16;
const TASKBAR_ICON_INSET: u32 = 4;
const TASKBAR_LINE_X: u32 = 8;
const TASKBAR_LINE_WIDTH: u32 = 24;
const TASKBAR_LINE_HEIGHT: u32 = 2;
const START_MARK_HEIGHT: u32 = 20;
const DOCK_GAP: u32 = 8;
const DOCK_PADDING: u32 = 8;
const DOCK_ICON_TOP: u32 = 2;
const DOCK_TILE: u32 = 52;
const DOCK_TILE_FROM_ICON: u32 = 2;
const DOCK_MARK: u32 = 4;
const DOCK_MARK_TOP: u32 = 52;
const DOCK_SEPARATOR_ROOM: u32 = 2 * DOCK_GAP + 1;
const DOCK_SEPARATOR_INSET: u32 = 8;
const DOCK_PINNED: [AppId; 3] = [AppId::Terminal, AppId::Files, AppId::Settings];
const DOCK_CAPACITY: usize = DOCK_PINNED.len() + MAX_WINDOWS;
const TOOLTIP_HEIGHT: u32 = 24;
const TOOLTIP_PADDING: u32 = 8;
const TOOLTIP_GAP: u32 = 4;
const TOOLTIP_TEXT_Y: u32 = 4;
const TOOLTIP_SCREEN_MARGIN: u32 = 8;
const START_TOOLTIP: &str = "Start";
const POWER_TOOLTIP: &str = "Power";

/// A thing on a panel that a click acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelItem {
    /// The top bar's system item (`EggOS`, or `Activities` in the Linux profile).
    System,
    /// The power button.
    Power,
    /// The Linux top bar's chip of the minimised window in this slot.
    Chip(u8),
    /// The taskbar's Start button.
    Start,
    /// The taskbar's item of the window in this slot.
    TaskbarWindow(u8),
    /// The dock's icon of this app.
    DockApp(AppId),
    /// The dock's icon of the window in this slot: minimised, or of an app the catalogue
    /// cannot name.
    DockWindow(u8),
}

/// The panels' pointer state: the item under the pointer and since when, the one pressed,
/// and the item whose tooltip shows.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PanelState {
    pub hover: Option<PanelItem>,
    pub hover_since_ms: u32,
    pub pressed: Option<PanelItem>,
    pub tooltip: Option<PanelItem>,
    pub tooltip_spent: bool,
}

struct DockEntries {
    items: [PanelItem; DOCK_CAPACITY],
    length: usize,
    separator_before: Option<usize>,
}

impl Desktop {
    /// The shell's layout now ([`super::layout::layout`] for the screen and the profile).
    pub fn layout(&self) -> &super::layout::Layout {
        &self.layout
    }

    /// The system item's hit target on the top bar, in the macOS and Linux profiles.
    pub fn system_item(&self) -> Option<Rect> {
        self.layout
            .top_bar
            .map(|bar| Rect::new(0, bar.y, SYSTEM_HIT_WIDTH, bar.height - 1))
    }

    /// The taskbar's Start button, in the Windows profile.
    pub fn start_button(&self) -> Option<Rect> {
        let mut found = None;
        self.for_each_taskbar_item(|item, rect| {
            if item == PanelItem::Start {
                found = Some(rect);
            }
        });
        found
    }

    /// The dock's icon of `app`, in the macOS profile, if it has one.
    pub fn dock_icon(&self, app: AppId) -> Option<Rect> {
        let mut found = None;
        self.for_each_dock_item(|item, rect| {
            if item == PanelItem::DockApp(app) {
                found = Some(rect);
            }
        });
        found
    }

    /// Where a click restores or focuses window `id` from the window list: its chip (Linux),
    /// its taskbar item (Windows) or its dock icon (macOS: the minimised window's own, else
    /// its app's).
    pub fn window_entry(&self, id: WindowId) -> Option<Rect> {
        let slot = self.find(id)?;
        let slot_item = slot as u8;
        let app = self.app_of(slot);
        let mut found = None;
        let mut find = |item: PanelItem, rect: Rect| {
            let matched = match item {
                PanelItem::Chip(at) | PanelItem::TaskbarWindow(at) | PanelItem::DockWindow(at) => {
                    at == slot_item
                }
                PanelItem::DockApp(of) => {
                    Some(of) == app && !self.window_at(slot).minimised && found.is_none()
                }
                _ => false,
            };
            if matched {
                found = Some(rect);
            }
        };
        match self.profile {
            Profile::Linux => self.for_each_chip(|at, rect| find(PanelItem::Chip(at as u8), rect)),
            Profile::Windows => self.for_each_taskbar_item(&mut find),
            Profile::Macos => self.for_each_dock_item(&mut find),
        }
        found
    }

    /// The clock's rectangle: centred on the top bar, or left of the taskbar's power button.
    pub fn clock_rect(&self) -> Rect {
        let width = CLOCK_GLYPHS as u32 * GLYPH_WIDTH as u32;
        let height = GLYPH_HEIGHT as u32;
        match self.layout.taskbar {
            Some(bar) => Rect::new(
                self.width.saturating_sub(TASKBAR_CLOCK_FROM_RIGHT + width),
                bar.y + (bar.height - height) / 2,
                width,
                height,
            ),
            None => Rect::new((self.width - width) / 2, BAR_TEXT_Y, width, height),
        }
    }

    /// The power button's hit target.
    pub fn power_button(&self) -> Rect {
        let (bar, size) = match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => (bar, TASKBAR_ITEM),
            (None, Some(bar)) => (bar, bar.height),
            (None, None) => (self.layout.screen, 0),
        };
        Rect::new(
            self.width.saturating_sub(POWER_BUTTON_MARGIN + size),
            bar.y,
            size,
            size,
        )
    }

    /// The power menu, its border included, where it opens: under the top bar's button, or
    /// above the taskbar's.
    pub fn menu(&self) -> Rect {
        let button = self.power_button();
        let height = MENU_ITEMS.len() as u32 * MENU_ITEM_HEIGHT + 2;
        let y = match self.layout.taskbar {
            Some(bar) => bar.y.saturating_sub(MENU_GAP + height),
            None => button.bottom() + MENU_GAP,
        };
        Rect::new(
            button.right().saturating_sub(MENU_WIDTH),
            y,
            MENU_WIDTH,
            height,
        )
    }

    /// The dock, its border included, in the macOS profile: as wide as its icons.
    pub(super) fn dock_rect(&self) -> Option<Rect> {
        let band = self.layout.dock_band?;
        let entries = self.dock_entries();
        let width = self.dock_width(&entries, entries.length);
        Some(Rect::new(
            (self.width.saturating_sub(width)) / 2,
            band.y,
            width,
            band.height,
        ))
    }

    fn dock_width(&self, entries: &DockEntries, shown: usize) -> u32 {
        let separator = match entries.separator_before {
            Some(at) if at < shown => DOCK_SEPARATOR_ROOM - DOCK_GAP,
            _ => 0,
        };
        2 * DOCK_PADDING + shown as u32 * (DOCK_ICON + DOCK_GAP) - DOCK_GAP + separator
    }

    /// Whether (`x`, `y`) is on a panel.
    pub(super) fn over_panel(&self, x: u32, y: u32) -> bool {
        [self.layout.top_bar, self.layout.taskbar, self.dock_rect()]
            .into_iter()
            .flatten()
            .any(|panel| contains(panel, x, y))
    }

    /// The app window `slot` belongs to, if the catalogue knows it: the terminal's window is
    /// Terminal's; another window (a dialog: its parent) is the app its title names.
    pub(super) fn app_of(&self, slot: usize) -> Option<AppId> {
        let slot = self.window_at(slot).parent.map_or(slot, usize::from);
        if slot == TERMINAL_SLOT {
            return Some(AppId::Terminal);
        }
        let title = self.window_at(slot).title();
        [AppId::Files, AppId::Settings]
            .into_iter()
            .find(|&app| self.catalogue.name(app) == title)
            .or_else(|| self.catalogue.installed_named(title))
    }

    /// Window `slot`'s icon: its app's, else the generic tile of its title (6.4.1).
    pub(super) fn window_icon(&self, slot: usize) -> Icon {
        match self.app_of(slot) {
            Some(app) => self.catalogue.icon(app),
            None => Icon::named(self.window_at(slot).title(), None),
        }
    }

    fn listed_windows(&self) -> impl Iterator<Item = usize> + '_ {
        self.created[..self.created_length]
            .iter()
            .map(|&slot| usize::from(slot))
            .filter(|&slot| {
                let window = self.window_at(slot);
                window.shown && window.role == Role::Normal
            })
    }

    fn dock_entries(&self) -> DockEntries {
        let mut entries = DockEntries {
            items: [PanelItem::System; DOCK_CAPACITY],
            length: 0,
            separator_before: None,
        };
        let push = |entries: &mut DockEntries, item: PanelItem| {
            if entries.length < DOCK_CAPACITY && !entries.items[..entries.length].contains(&item) {
                entries.items[entries.length] = item;
                entries.length += 1;
            }
        };
        for app in DOCK_PINNED {
            push(&mut entries, PanelItem::DockApp(app));
        }
        let pinned = entries.length;
        for slot in self.listed_windows() {
            if self.window_at(slot).minimised {
                continue;
            }
            match self.app_of(slot) {
                Some(app) if DOCK_PINNED.contains(&app) => {}
                Some(app) => push(&mut entries, PanelItem::DockApp(app)),
                None => push(&mut entries, PanelItem::DockWindow(slot as u8)),
            }
        }
        for slot in self.listed_windows() {
            if self.window_at(slot).minimised {
                push(&mut entries, PanelItem::DockWindow(slot as u8));
            }
        }
        if entries.length > pinned {
            entries.separator_before = Some(pinned);
        }
        entries
    }

    fn for_each_dock_item(&self, mut item: impl FnMut(PanelItem, Rect)) {
        let Some(band) = self.layout.dock_band else {
            return;
        };
        let entries = self.dock_entries();
        let mut shown = entries.length;
        while shown > 0 && self.dock_width(&entries, shown) > self.width {
            shown -= 1;
        }
        let width = self.dock_width(&entries, shown);
        let mut x = self.width.saturating_sub(width) / 2 + DOCK_PADDING;
        for (index, &entry) in entries.items[..shown].iter().enumerate() {
            if entries.separator_before == Some(index) {
                x += DOCK_SEPARATOR_ROOM - DOCK_GAP;
            }
            item(
                entry,
                Rect::new(x, band.y + DOCK_ICON_TOP, DOCK_ICON, DOCK_ICON),
            );
            x += DOCK_ICON + DOCK_GAP;
        }
    }

    fn for_each_taskbar_item(&self, mut item: impl FnMut(PanelItem, Rect)) {
        let Some(bar) = self.layout.taskbar else {
            return;
        };
        let count = 1 + self.listed_windows().count() as u32;
        let group = count * (TASKBAR_ITEM + TASKBAR_GAP) - TASKBAR_GAP;
        let limit = self
            .clock_rect()
            .x
            .saturating_sub(TASKBAR_ROOM_BEFORE_CLOCK);
        let centred = self.width.saturating_sub(group) / 2;
        let mut x = match centred >= TASKBAR_MARGIN && centred + group <= limit {
            true => centred,
            false => TASKBAR_MARGIN,
        };
        let items = core::iter::once(PanelItem::Start).chain(
            self.listed_windows()
                .map(|slot| PanelItem::TaskbarWindow(slot as u8)),
        );
        for entry in items {
            if x + TASKBAR_ITEM > limit {
                return;
            }
            item(entry, Rect::new(x, bar.y, TASKBAR_ITEM, TASKBAR_ITEM));
            x += TASKBAR_ITEM + TASKBAR_GAP;
        }
    }

    pub(super) fn for_each_panel_item(&self, mut item: impl FnMut(PanelItem, Rect)) {
        if let Some(system) = self.system_item() {
            item(PanelItem::System, system);
        }
        if self.layout.top_bar.is_some() || self.layout.taskbar.is_some() {
            item(PanelItem::Power, self.power_button());
        }
        self.for_each_chip(|slot, rect| item(PanelItem::Chip(slot as u8), rect));
        self.for_each_taskbar_item(&mut item);
        self.for_each_dock_item(&mut item);
    }

    /// The panel item at (`x`, `y`), if any.
    pub(super) fn panel_item_at(&self, x: u32, y: u32) -> Option<PanelItem> {
        let mut found = None;
        self.for_each_panel_item(|item, rect| {
            if contains(rect, x, y) {
                found = Some(item);
            }
        });
        found
    }

    fn panel_item_rect(&self, wanted: PanelItem) -> Option<Rect> {
        let mut found = None;
        self.for_each_panel_item(|item, rect| {
            if item == wanted {
                found = Some(rect);
            }
        });
        found
    }

    /// Damages what lists the windows: the Linux top bar's chips, the taskbar or the dock.
    pub(super) fn damage_window_list(&self, damage: &mut Damage) {
        match self.profile {
            Profile::Linux => damage.add(self.chips_area()),
            Profile::Windows => {
                if let Some(bar) = self.layout.taskbar {
                    damage.add(bar);
                }
            }
            Profile::Macos => {
                if let Some(band) = self.layout.dock_band {
                    damage.add(Rect::new(
                        0,
                        band.y.saturating_sub(TOOLTIP_HEIGHT + TOOLTIP_GAP),
                        self.width,
                        self.height - band.y.saturating_sub(TOOLTIP_HEIGHT + TOOLTIP_GAP),
                    ));
                }
            }
        }
        if let Some(rect) = self.tooltip_rect() {
            damage.add(rect);
        }
    }

    /// The window list changed while the window that is focused changed: the taskbar's focus
    /// line moves.
    pub(super) fn damage_focus_in_window_list(&self, damage: &mut Damage) {
        if let Some(bar) = self.layout.taskbar {
            damage.add(bar);
        }
    }

    pub(super) fn app_windows(&self, app: AppId, out: &mut [u8; MAX_WINDOWS]) -> usize {
        let mut length = 0;
        for slot in self.listed_windows() {
            if self.app_of(slot) != Some(app) {
                continue;
            }
            let serial = self.window_at(slot).focus_serial;
            let mut at = length;
            while at > 0 && self.window_at(usize::from(out[at - 1])).focus_serial < serial {
                out[at] = out[at - 1];
                at -= 1;
            }
            out[at] = slot as u8;
            length += 1;
        }
        length
    }

    /// Focuses `app`'s most recently focused window, restoring it if it is minimised; when
    /// one of its windows is focused already, the one focused longest ago instead, so clicks
    /// cycle; launches the app if it has none.
    pub(super) fn activate_app(
        &mut self,
        app: AppId,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let mut windows = [0u8; MAX_WINDOWS];
        let length = self.app_windows(app, &mut windows);
        if length == 0 {
            return self.launch_new(app, damage, events);
        }
        let windows = &windows[..length];
        let shown = |slot: &&u8| !self.window_at(usize::from(**slot)).minimised;
        let focused_is_app = self
            .focused_normal()
            .is_some_and(|slot| self.app_of(slot) == Some(app));
        let open_count = windows.iter().filter(shown).count();
        let chosen = match (focused_is_app && open_count > 1, windows.iter().find(shown)) {
            (true, _) => windows.iter().rev().find(shown).copied(),
            (false, Some(&first)) => Some(first),
            (false, None) => windows.first().copied(),
        };
        if let Some(slot) = chosen {
            self.activate(usize::from(slot), damage, events);
        }
        Action::None
    }

    /// Launches `app` (docs/ux/desktop-v0.3-gui.md, section 6.2): Terminal focuses the
    /// terminal's window; Settings and Files focus their window if they have one, else ask the
    /// kernel to start them (Files says it is not in the build when it is not); the host share
    /// and installed apps focus the terminal and ask the kernel to type their command in it.
    pub fn launch(
        &mut self,
        app: AppId,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        match app {
            AppId::Files | AppId::Settings => self.activate_app(app, damage, events),
            _ => self.launch_new(app, damage, events),
        }
    }

    fn launch_new(
        &mut self,
        app: AppId,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        if !self.catalogue.has(app) {
            return Action::None;
        }
        match app {
            AppId::Terminal => {
                self.activate(TERMINAL_SLOT, damage, events);
                Action::None
            }
            AppId::Files if !self.catalogue.files_present() => {
                self.show_notice(super::launcher::FILES_MISSING, damage);
                Action::None
            }
            AppId::Files | AppId::Settings => Action::Start(app),
            AppId::HostShare | AppId::Installed(_) => {
                self.activate(TERMINAL_SLOT, damage, events);
                Action::Type(app)
            }
        }
    }

    /// A button press on panel item `item`.
    pub(super) fn panel_press(
        &mut self,
        item: PanelItem,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        self.panels.pressed = Some(item);
        self.panels.tooltip_spent = true;
        self.hide_tooltip(damage);
        if let Some(rect) = self.panel_item_rect(item) {
            damage.add(rect);
        }
        match item {
            PanelItem::System | PanelItem::Start => {
                self.toggle_launcher(damage, events);
                Action::None
            }
            PanelItem::Power => {
                self.open_menu(damage);
                Action::None
            }
            PanelItem::Chip(slot) => {
                self.restore(usize::from(slot), damage, events);
                Action::None
            }
            PanelItem::TaskbarWindow(slot) => {
                let slot = usize::from(slot);
                let focused = self.focused_normal() == Some(slot);
                match focused && !self.window_at(slot).minimised {
                    true => self.minimise(slot, damage, events),
                    false => self.activate(slot, damage, events),
                }
                Action::None
            }
            PanelItem::DockApp(app) => self.activate_app(app, damage, events),
            PanelItem::DockWindow(slot) => {
                self.activate(usize::from(slot), damage, events);
                Action::None
            }
        }
    }

    /// The left button went up: the pressed panel item is no longer drawn pressed.
    pub(super) fn panel_release(&mut self, damage: &mut Damage) {
        if let Some(item) = self.panels.pressed.take()
            && let Some(rect) = self.panel_item_rect(item)
        {
            damage.add(rect);
        }
    }

    /// The pointer is at (`x`, `y`): the item under it is drawn hovered, its tooltip's clock
    /// starts.
    pub(super) fn update_panel_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        let hover = self.panel_item_at(x, y);
        if hover == self.panels.hover {
            return;
        }
        for item in [self.panels.hover, hover].into_iter().flatten() {
            if let Some(rect) = self.panel_item_rect(item) {
                damage.add(rect);
            }
        }
        self.hide_tooltip(damage);
        self.panels.hover = hover;
        self.panels.hover_since_ms = self.now_ms;
        self.panels.tooltip_spent = false;
    }

    /// Forgets the panels' pointer state, as a profile change does.
    pub(super) fn reset_panels(&mut self) {
        self.panels = PanelState::default();
    }

    pub(super) fn hide_tooltip(&mut self, damage: &mut Damage) {
        if let Some(rect) = self.tooltip_rect() {
            damage.add(rect);
        }
        self.panels.tooltip = None;
    }

    /// The clock reached `now_ms`: the hovered item's tooltip shows once the pointer has
    /// rested on it for [`TOOLTIP_DELAY_MS`], unless a popup is open or it was clicked.
    pub(super) fn tick_tooltip(&mut self, damage: &mut Damage) {
        let Some(hover) = self.panels.hover else {
            return;
        };
        let rested = self.now_ms.wrapping_sub(self.panels.hover_since_ms) >= TOOLTIP_DELAY_MS;
        let blocked = self.panels.tooltip_spent || self.popup_open() || self.menu_open;
        if self.panels.tooltip.is_some() || !rested || blocked {
            return;
        }
        self.panels.tooltip = Some(hover);
        if let Some(rect) = self.tooltip_rect() {
            damage.add(rect);
        }
    }

    fn tooltip_text(&self, item: PanelItem) -> Option<&str> {
        match (item, self.profile) {
            (PanelItem::Start, _) => Some(START_TOOLTIP),
            (PanelItem::Power, Profile::Windows) => Some(POWER_TOOLTIP),
            (PanelItem::DockApp(app), _) => Some(self.catalogue.name(app)),
            (PanelItem::TaskbarWindow(slot) | PanelItem::DockWindow(slot), _) => {
                Some(self.window_at(usize::from(slot)).title())
            }
            _ => None,
        }
    }

    /// The tooltip shown now, its rectangle.
    pub fn tooltip_rect(&self) -> Option<Rect> {
        let item = self.panels.tooltip?;
        let text = self.tooltip_text(item)?;
        let anchor = self.panel_item_rect(item)?;
        let panel_top = match (self.layout.taskbar, self.dock_rect()) {
            (Some(bar), _) => bar.y,
            (None, Some(dock)) => dock.y,
            (None, None) => return None,
        };
        let width = text.chars().count() as u32 * GLYPH_WIDTH as u32 + 2 * TOOLTIP_PADDING;
        let centred = (anchor.x + anchor.width / 2).saturating_sub(width / 2);
        let x = centred
            .min(self.width.saturating_sub(TOOLTIP_SCREEN_MARGIN + width))
            .max(TOOLTIP_SCREEN_MARGIN);
        let y = panel_top.saturating_sub(TOOLTIP_GAP + TOOLTIP_HEIGHT);
        Some(Rect::new(x, y, width, TOOLTIP_HEIGHT))
    }

    pub(super) fn draw_tooltip(&self, canvas: &mut Canvas<'_>) {
        let (Some(item), Some(rect)) = (self.panels.tooltip, self.tooltip_rect()) else {
            return;
        };
        let Some(text) = self.tooltip_text(item) else {
            return;
        };
        canvas.rounded(rect, &paint::INSET_R4, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
            &paint::INSET_INSIDE_R4,
            true,
            true,
            colour::MENU,
        );
        canvas.text(
            rect.x + TOOLTIP_PADDING,
            rect.y + TOOLTIP_TEXT_Y,
            text,
            colour::TEXT,
        );
    }

    fn item_fill(&self, item: PanelItem, open: bool) -> Option<u32> {
        if open || self.panels.pressed == Some(item) {
            return Some(colour::BAR_PRESSED);
        }
        (self.panels.hover == Some(item)).then_some(colour::BAR_HOVER)
    }

    pub(super) fn draw_panels(&self, canvas: &mut Canvas<'_>) {
        let clip = canvas.clip();
        if let Some(bar) = self.layout.top_bar
            && bar.overlaps(&clip)
        {
            self.draw_top_bar(canvas, bar);
        }
        if let Some(bar) = self.layout.taskbar
            && bar.overlaps(&clip)
        {
            self.draw_taskbar(canvas, bar);
        }
        if let Some(dock) = self.dock_rect()
            && Rect::new(
                dock.x.saturating_sub(4),
                dock.y,
                dock.width + 8,
                dock.height + 4,
            )
            .overlaps(&clip)
        {
            self.draw_dock(canvas, dock);
        }
    }

    fn draw_power(&self, canvas: &mut Canvas<'_>, bar: Rect) {
        let button = self.power_button();
        let (fill, area) = match self.layout.taskbar {
            Some(_) => (
                self.item_fill(PanelItem::Power, self.menu_open),
                Rect::new(button.x, bar.y + 1, button.width, bar.height - 1),
            ),
            None => (
                self.menu_open.then_some(colour::BAR_PRESSED),
                Rect::new(button.x, bar.y, button.width, bar.height - 1),
            ),
        };
        if let Some(fill) = fill {
            canvas.fill(area, fill);
        }
        let offset_x = (button.width - POWER_ICON_SIZE) / 2;
        let offset_y = (button.height - POWER_ICON_SIZE) / 2;
        for (row, bits) in POWER_ICON.iter().enumerate() {
            for column in 0..POWER_ICON_SIZE {
                if bits & (1 << (POWER_ICON_SIZE - 1 - column)) != 0 {
                    canvas.pixel(
                        button.x + offset_x + column,
                        button.y + offset_y + row as u32,
                        colour::TEXT,
                    );
                }
            }
        }
    }

    fn draw_egg_mark(canvas: &mut Canvas<'_>, left: u32, top: u32, height: u32, width: u32) {
        let mark_width = round_percent(72, height);
        for row in 0..height {
            let (from, to) = span(left + mark_width / 2, egg_half_width(height, row), width);
            canvas.row(top + row, from, to, colour::ACCENT);
        }
    }

    fn draw_top_bar(&self, canvas: &mut Canvas<'_>, bar: Rect) {
        canvas.fill(Rect::new(0, bar.y, self.width, bar.height - 1), colour::BAR);
        canvas.fill(
            Rect::new(0, bar.bottom() - 1, self.width, 1),
            colour::BAR_EDGE,
        );
        let launcher_open = self.launcher.is_some();
        if let (Some(fill), Some(system)) = (
            self.item_fill(PanelItem::System, launcher_open)
                .filter(|&fill| fill == colour::BAR_PRESSED),
            self.system_item(),
        ) {
            canvas.fill(system, fill);
        }
        match self.profile {
            Profile::Linux => canvas.text(ACTIVITIES_X, BAR_TEXT_Y, ACTIVITIES, colour::TEXT),
            _ => {
                let top = (bar.height - BAR_MARK_HEIGHT) / 2;
                Self::draw_egg_mark(canvas, BAR_MARK_X, top, BAR_MARK_HEIGHT, self.width);
                canvas.text(SYSTEM_NAME_X, BAR_TEXT_Y, SYSTEM_NAME, colour::TEXT);
            }
        }
        self.for_each_chip(|slot, chip| {
            let fill = match self.panels.hover == Some(PanelItem::Chip(slot as u8)) {
                true => colour::BAR_PRESSED,
                false => colour::BAR_HOVER,
            };
            canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
            super::draw_chip_text(canvas, chip, self.window_at(slot).title());
        });
        let clock = self.clock_rect();
        canvas.text(clock.x, clock.y, self.clock_text(), colour::TEXT_DIM);
        self.draw_power(canvas, bar);
    }

    fn draw_taskbar(&self, canvas: &mut Canvas<'_>, bar: Rect) {
        canvas.fill(bar, colour::BAR);
        canvas.fill(Rect::new(0, bar.y, self.width, 1), colour::BAR_EDGE);
        let focused = self.focused_normal();
        self.for_each_taskbar_item(|item, rect| {
            let open = item == PanelItem::Start && self.launcher.is_some();
            if let Some(fill) = self.item_fill(item, open) {
                canvas.fill(
                    Rect::new(rect.x, rect.y + 1, rect.width, rect.height - 1),
                    fill,
                );
            }
            match item {
                PanelItem::Start => {
                    let width = round_percent(72, START_MARK_HEIGHT);
                    Self::draw_egg_mark(
                        canvas,
                        rect.x + (rect.width - width) / 2,
                        rect.y + (rect.height - START_MARK_HEIGHT) / 2,
                        START_MARK_HEIGHT,
                        self.width,
                    );
                }
                PanelItem::TaskbarWindow(slot) => {
                    let slot = usize::from(slot);
                    draw_icon(
                        canvas,
                        self.window_icon(slot),
                        IconSize::Small,
                        rect.x + TASKBAR_ICON_INSET,
                        rect.y + TASKBAR_ICON_INSET,
                    );
                    let line = match (self.window_at(slot).minimised, focused == Some(slot)) {
                        (true, _) => None,
                        (false, true) => Some(colour::ACCENT),
                        (false, false) => Some(colour::TEXT_DIM),
                    };
                    if let Some(line) = line {
                        canvas.fill(
                            Rect::new(
                                rect.x + TASKBAR_LINE_X,
                                rect.bottom() - TASKBAR_LINE_HEIGHT,
                                TASKBAR_LINE_WIDTH,
                                TASKBAR_LINE_HEIGHT,
                            ),
                            line,
                        );
                    }
                }
                _ => {}
            }
        });
        let clock = self.clock_rect();
        canvas.text(clock.x, clock.y, self.clock_text(), colour::TEXT_DIM);
        self.draw_power(canvas, bar);
    }

    fn draw_dock(&self, canvas: &mut Canvas<'_>, dock: Rect) {
        canvas.shadow(dock);
        canvas.rounded(dock, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(dock.x + 1, dock.y + 1, dock.width - 2, dock.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::DOCK,
        );
        let entries = self.dock_entries();
        let mut index = 0;
        self.for_each_dock_item(|item, rect| {
            if entries.separator_before == Some(index) {
                canvas.fill(
                    Rect::new(
                        rect.x - DOCK_GAP - 1,
                        dock.y + DOCK_SEPARATOR_INSET,
                        1,
                        dock.height - 2 * DOCK_SEPARATOR_INSET,
                    ),
                    colour::MENU_BORDER,
                );
            }
            index += 1;
            if let Some(fill) = self.item_fill(item, false) {
                canvas.rounded(
                    Rect::new(rect.x - DOCK_TILE_FROM_ICON, dock.y, DOCK_TILE, DOCK_TILE),
                    &paint::INSET_R6,
                    true,
                    true,
                    fill,
                );
            }
            let (icon, running) = match item {
                PanelItem::DockApp(app) => {
                    let mut windows = [0u8; MAX_WINDOWS];
                    (
                        self.catalogue.icon(app),
                        self.app_windows(app, &mut windows) > 0,
                    )
                }
                PanelItem::DockWindow(slot) => {
                    let slot = usize::from(slot);
                    (self.window_icon(slot), !self.window_at(slot).minimised)
                }
                _ => return,
            };
            draw_icon(canvas, icon, IconSize::Large, rect.x, rect.y);
            if running {
                canvas.fill(
                    Rect::new(
                        rect.x + (DOCK_ICON - DOCK_MARK) / 2,
                        dock.y + DOCK_MARK_TOP,
                        DOCK_MARK,
                        DOCK_MARK,
                    ),
                    colour::TEXT,
                );
            }
        });
    }
}
