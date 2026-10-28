//! The desktop and its window server's state (D115, D127, D128): the procedural wallpaper, the
//! top bar with the system item, the clock, the chips of minimised windows and the power button
//! and its menu, and the windows: the terminal's and those programs open through
//! `/dev/window` (docs/windows.md), with their stacking, focus, chrome, dragging, resizing,
//! maximising, minimising, close requests, the switcher, the window menu, the server's
//! shortcuts and the pointer (docs/ux/desktop-v0.3-gui.md, sections 2 to 5).
//!
//! [`Desktop`] is pure state and drawing, no device: the kernel feeds it pointer and key
//! events and its clients' requests, it records the rectangles they change in a [`Damage`] and
//! the events its clients get in an [`EventSink`], and [`Desktop::draw`] repaints any
//! rectangle of a framebuffer from scratch, copying each window's content from its buffer
//! through [`Contents`]. Every window table is fixed ([`protocol::MAX_WINDOWS`]); after
//! [`Desktop::new`] only the catalogue of installed apps allocates, when the kernel hands the
//! desktop a new one ([`Desktop::set_catalogue`]).

mod catalogue;
mod desktop_icons;
mod events;
mod icons;
mod launcher;
pub mod layout;
mod management;
mod paint;
mod pointer;
mod popups;
pub mod protocol;
mod shell;
pub mod shortcuts;
mod windows;

#[cfg(test)]
mod management_tests;
#[cfg(test)]
mod shell_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;

pub use catalogue::{
    ARGUMENT_SEPARATOR, AppId, COMMAND_CAPACITY, Catalogue, Command, INSTALLER_PROGRAM, Launch,
    MAX_INSTALLED,
};
pub use desktop_icons::{ICON_CELL, ICON_GRID_MARGIN};
pub use events::EventQueue;
pub use icons::{Icon, IconSize, Source};
pub use launcher::{FILES_MISSING, LAUNCH_FAILED, LAUNCHER_ROW_HEIGHT, NOTICE_MS};
pub use layout::{IconSide, Layout, layout};
pub use management::{CLOSE_TIMEOUT_MS, KEYBOARD_STEP, ListedWindow, NOT_RESPONDING_AFTER_MS};
pub use popups::{SWITCHER_ROW_HEIGHT, SWITCHER_ROWS, SWITCHER_WIDTH, WINDOW_MENU_ITEMS};
pub use shell::{DOCK_ICON, TASKBAR_ITEM, TOOLTIP_DELAY_MS};
pub use shortcuts::{Profile, Shortcut, shortcut};
pub use windows::{
    CASCADE_STEP, CHROME, ControlSide, DEFAULT_MARGIN, DOUBLE_CLICK_DISTANCE, DOUBLE_CLICK_MS,
    KERNEL_CLIENT, MINIMUM_OUTSIDE, Part, RESTORE_DRAG_DISTANCE, WindowId, clamp_content_size,
    clamp_resized_content, edge, is_double_click, minimum_outside, resize_edges, resize_pointer,
    resized, slot_of,
};

use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::keymap::{Modifiers, printable};
use crate::input::{button, key};
use crate::terminal::{PADDING, TERM_BACKGROUND, TextArea};
use alloc::boxed::Box;
use alloc::vec::Vec;
use desktop_icons::DesktopIcons;
use launcher::{Launcher, Notice};
use management::Resize;
use paint::{Canvas, egg_half_width, isqrt};
use popups::{Switcher, WindowMenu};
use protocol::{
    EventRecord, EventSink, MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT, PointerShape, Role,
    TITLE_CAPACITY, WindowError, WindowState, event, flags, modifier,
};
use shell::PanelState;
use windows::{SLOT_BITS, Window};

/// The design's colour tokens (`desktop-v0.3.md` section 2, `desktop-v0.3-gui.md` section 1),
/// as `0x00RRGGBB` pixels.
pub mod colour {
    pub const WALLPAPER_TOP: u32 = 0x000e_1726;
    pub const WALLPAPER_BOTTOM: u32 = 0x001b_3a5c;
    pub const WALLPAPER_EGG: u32 = 0x001f_3f63;
    pub const ACCENT: u32 = 0x00f2_b33d;
    pub const BAR: u32 = 0x0014_1b26;
    pub const BAR_EDGE: u32 = 0x0020_2938;
    pub const BAR_HOVER: u32 = 0x0026_303f;
    pub const BAR_PRESSED: u32 = 0x0033_4052;
    pub const MENU: u32 = 0x001c_2430;
    pub const MENU_BORDER: u32 = 0x002a_3340;
    pub const MENU_HOVER: u32 = 0x002a_3440;
    pub const TEXT: u32 = 0x00e6_eaf0;
    pub const TEXT_DIM: u32 = 0x008a_94a6;
    pub const TITLE_ACTIVE: u32 = 0x002a_3340;
    pub const TITLE_INACTIVE: u32 = 0x001f_2630;
    pub const BORDER_ACTIVE: u32 = 0x003a_4454;
    pub const BORDER_INACTIVE: u32 = 0x002a_3340;
    pub const BUTTON_CLOSE: u32 = 0x00e5_534b;
    pub const BUTTON_MINIMISE: u32 = 0x00e8_b33c;
    pub const BUTTON_MAXIMISE: u32 = 0x003d_be5c;
    pub const GLYPH: u32 = 0x0000_0000;
    pub const FOCUS_RING: u32 = 0x00f2_b33d;
    pub const SELECTION: u32 = 0x002e_4a6b;
    pub const DOCK: u32 = 0x001c_2430;
    pub const BUTTON_INACTIVE: u32 = 0x004a_5466;
    pub const WINDOW_BACKGROUND: u32 = 0x0012_161d;
    pub const FIELD: u32 = 0x000b_0e13;
    pub const BUTTON_BORDER: u32 = 0x003a_4454;
    pub const ICON_TILE: u32 = 0x0026_303f;
    pub const ICON_TILE_BORDER: u32 = 0x003a_4454;
    pub const ICON_PROMPT: u32 = 0x005f_b865;
    pub const ICON_FOLDER: u32 = 0x00e2_b341;
    pub const ICON_FOLDER_BACK: u32 = 0x00b8_8a24;
    pub const ICON_GEAR: u32 = 0x008a_94a6;
    pub const ICON_GEAR_HOLE: u32 = 0x001f_2630;
    pub const ICON_WINDOWS: u32 = 0x004f_8fe0;
    pub const ICON_MAC: u32 = 0x00c9_d1db;
    pub const ICON_LINUX: u32 = 0x00e2_b341;
    pub const ICON_DARK: u32 = 0x000b_0e13;
}

/// Height of the top bar, the 1 px edge line included.
pub const BAR_HEIGHT: u32 = 28;
/// Height of a window's title bar.
pub const TITLE_HEIGHT: u32 = 28;
/// The terminal window's grid: 80 columns by 24 rows, fixed in 0.3.0.
pub const GRID_COLUMNS: u16 = 80;
/// See [`GRID_COLUMNS`].
pub const GRID_ROWS: u16 = 24;
/// The terminal's grid in pixels, the size of its buffer.
pub const GRID_SIZE: (u32, u32) = (
    GRID_COLUMNS as u32 * GLYPH_WIDTH as u32,
    GRID_ROWS as u32 * GLYPH_HEIGHT as u32,
);
/// The terminal window's width, its 1 px border included.
pub const WINDOW_WIDTH: u32 = GRID_SIZE.0 + 2 * PADDING + 2;
/// The terminal window's height, its 1 px border included.
pub const WINDOW_HEIGHT: u32 = GRID_SIZE.1 + 2 * PADDING + TITLE_HEIGHT + 2;
/// The terminal window's title.
pub const WINDOW_TITLE: &str = "Terminal";
/// The power menu's width.
pub const MENU_WIDTH: u32 = 160;
/// One power menu item's height.
pub const MENU_ITEM_HEIGHT: u32 = 32;
/// Glyphs the clock's rectangle holds, centred on the bar.
pub const CLOCK_GLYPHS: usize = 10;

const SHADOW_RINGS: u32 = 4;
const POWER_BUTTON_MARGIN: u32 = 8;
const POWER_ICON_SIZE: u32 = 13;
const POWER_ICON: [u16; 13] = [
    0b0000001000000,
    0b0000001000000,
    0b0001001001000,
    0b0010001000100,
    0b0100001000010,
    0b0100001000010,
    0b0100000000010,
    0b0100000000010,
    0b0010000000100,
    0b0001000001000,
    0b0000111110000,
    0,
    0,
];
const SYSTEM_NAME: &str = "EggOS";
const SYSTEM_NAME_X: u32 = 28;
const BAR_TEXT_Y: u32 = 6;
const BAR_MARK_X: u32 = 12;
const BAR_MARK_HEIGHT: u32 = 14;
const CHIP_X: u32 = 104;
const CHIP_Y: u32 = 4;
const CHIP_HEIGHT: u32 = 20;
const CHIP_PADDING: u32 = 8;
const CHIP_GAP: u32 = 8;
const CHIP_GLYPHS: usize = 16;
const CHIPS_CLEAR_OF_CLOCK: u32 = 16;
const MENU_GAP: u32 = 4;
pub(crate) const MENU_TEXT_X: u32 = 12;
const MENU_ITEMS: [(&str, Action); 2] = [
    ("Power off", Action::PowerOff),
    ("Restart", Action::Restart),
];
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
const MOST_DAYS_SHOWN: u64 = 999_999;
const TERMINAL_SLOT: usize = 0;
const FIRST_GENERATION: u16 = 1;
const GENERATIONS: u16 = 1 << (16 - SLOT_BITS);
const BUTTON_CODES: [u16; 3] = [button::LEFT, button::RIGHT, button::MIDDLE];
const CONTENT_CORNER_ROWS: u32 = 3;

/// What a pointer or key event asks of the machine, beyond redrawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing more.
    None,
    /// Power the machine off, as `poweroff` does.
    PowerOff,
    /// Restart the machine, as `reboot` does.
    Restart,
    /// Start the app's program as a process ([`Catalogue::launch`]).
    Start(AppId),
    /// Type the app's command in the terminal, as if the user had ([`Catalogue::launch`]).
    Type(AppId),
    /// Start the installer's window to uninstall the app ([`Catalogue::uninstall`]).
    Uninstall(AppId),
}

/// Where the desktop reads windows' pixels: the buffers the kernel keeps for them.
pub trait Contents {
    /// Copies pixels `x..x + out.len()` of row `y` of window `window`'s buffer into `out`.
    fn copy_row(&self, window: WindowId, x: u32, y: u32, out: &mut [u32]);
}

/// A window the kernel's client asked for and the desktop made ([`Desktop::create`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Created {
    /// Its id.
    pub window: WindowId,
    /// Its content size, which its buffer must have.
    pub width: u32,
    /// See [`Self::width`].
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    window: usize,
    from_pointer_x: i64,
    from_pointer_y: i64,
    pressed_at: (u32, u32),
}

const NOT_RESPONDING: &str = " (not responding)";
const CONTROL_GLYPH_SIZE: u32 = 5;
const CONTROL_GLYPH_OFFSET: u32 = 4;
const CLOSE_GLYPH: [u8; 5] = [0b10001, 0b01010, 0b00100, 0b01010, 0b10001];
const MINIMISE_GLYPH: [u8; 5] = [0, 0, 0b11111, 0, 0];
const MAXIMISE_GLYPH: [u8; 5] = [0b11111, 0b10001, 0b10001, 0b10001, 0b11111];
const RESTORE_GLYPH: [u8; 5] = [0b01111, 0b01001, 0b11111, 0b10011, 0b11110];

/// The desktop on a screen of one size, with its windows.
#[derive(Debug)]
pub struct Desktop {
    width: u32,
    height: u32,
    gradient: Vec<u32>,
    egg: Vec<(u32, u32)>,
    yolk: (u32, u32, u32),
    windows: Box<[Option<Window>]>,
    generations: [u16; MAX_WINDOWS],
    stack: [u8; MAX_WINDOWS],
    stack_length: usize,
    created: [u8; MAX_WINDOWS],
    created_length: usize,
    focus: Option<usize>,
    hover: Option<usize>,
    grab: Option<usize>,
    buttons: u8,
    drag: Option<Drag>,
    resize: Option<Resize>,
    last_title_press: Option<(usize, u32, u32, u32)>,
    control_hover: Option<(usize, Part)>,
    menu_open: bool,
    menu_hover: Option<usize>,
    switcher: Option<Switcher>,
    window_menu: Option<WindowMenu>,
    pointer: (u32, u32),
    pointer_visible: bool,
    pointer_drawn: Option<(u32, u32, PointerShape)>,
    profile: Profile,
    layout: Layout,
    catalogue: Catalogue,
    catalogue_wanted: bool,
    panels: PanelState,
    launcher: Option<Launcher>,
    notice: Option<Notice>,
    icons: DesktopIcons,
    super_alone: bool,
    now_ms: u32,
    focus_serial: u32,
    dialogs: usize,
    clock: [u8; CLOCK_GLYPHS],
}

impl Desktop {
    /// The desktop of a `width` by `height` screen in the default profile with the terminal's
    /// window centred in the work area, shown and focused, the clock at `up 0:00`, and only the
    /// built-in apps in the catalogue; `None` when the window does not fit every profile's
    /// work area.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let fits = Profile::ALL.into_iter().all(|profile| {
            let work = layout(width, height, 1, profile).work_area;
            work.width >= WINDOW_WIDTH && work.height >= WINDOW_HEIGHT
        });
        if !fits {
            return None;
        }
        let gradient = (0..height).map(|row| gradient_pixel(row, height)).collect();
        let egg_height = height;
        let egg_top = round_percent(25, height);
        let centre = round_percent(82, width);
        let egg = (0..height)
            .map(|row| match row.checked_sub(egg_top) {
                Some(row) => span(centre, egg_half_width(egg_height, row), width),
                None => (0, 0),
            })
            .collect();
        let yolk = (
            centre,
            egg_top + round_percent(62, height),
            round_percent(9, height),
        );
        let mut windows = Vec::new();
        windows.resize(MAX_WINDOWS, None);
        let mut desktop = Self {
            width,
            height,
            gradient,
            egg,
            yolk,
            windows: windows.into_boxed_slice(),
            generations: [FIRST_GENERATION; MAX_WINDOWS],
            stack: [0; MAX_WINDOWS],
            stack_length: 0,
            created: [0; MAX_WINDOWS],
            created_length: 0,
            focus: None,
            hover: None,
            grab: None,
            buttons: 0,
            drag: None,
            resize: None,
            last_title_press: None,
            control_hover: None,
            menu_open: false,
            menu_hover: None,
            switcher: None,
            window_menu: None,
            pointer: (0, 0),
            pointer_visible: false,
            pointer_drawn: None,
            profile: Profile::default(),
            layout: layout(width, height, 1, Profile::default()),
            catalogue: Catalogue::new(),
            catalogue_wanted: false,
            panels: PanelState::default(),
            launcher: None,
            notice: None,
            icons: DesktopIcons::default(),
            super_alone: false,
            now_ms: 0,
            focus_serial: 0,
            dialogs: 0,
            clock: [b' '; CLOCK_GLYPHS],
        };
        let content = (WINDOW_WIDTH - CHROME.0, WINDOW_HEIGHT - CHROME.1);
        let mut terminal = Window::new(
            desktop.id_for(TERMINAL_SLOT),
            KERNEL_CLIENT,
            Role::Normal,
            content,
            TERM_BACKGROUND,
        );
        terminal.set_title(WINDOW_TITLE);
        terminal.flags = flags::FIXED_SIZE | flags::TAKES_CTRL_KEYS;
        terminal.closable = false;
        terminal.ready = true;
        terminal.buffer = Rect::new(PADDING, PADDING, GRID_SIZE.0, GRID_SIZE.1);
        desktop.windows[TERMINAL_SLOT] = Some(terminal);
        desktop.created[0] = TERMINAL_SLOT as u8;
        desktop.created_length = 1;
        let mut damage = Damage::new();
        let mut events = protocol::DiscardEvents;
        desktop.show(TERMINAL_SLOT, &mut damage, &mut events);
        desktop.set_uptime(0, &mut damage);
        Some(desktop)
    }

    fn id_for(&self, slot: usize) -> WindowId {
        self.generations[slot] << SLOT_BITS | slot as u16
    }

    /// The whole screen.
    pub fn screen(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// The work area: the screen less the profile's panels, where windows go.
    pub fn work_area(&self) -> Rect {
        self.layout.work_area
    }

    /// The apps the shell offers.
    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    /// Makes `catalogue` the shell's, redrawing what lists apps if it differs from the one it
    /// had; an open icon menu follows its app, and closes when the app is gone.
    pub fn set_catalogue(
        &mut self,
        catalogue: Catalogue,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        if catalogue == self.catalogue {
            return;
        }
        if let Some(menu) = self.icons.menu {
            let name = self.catalogue.name(menu.app);
            match catalogue
                .desktop_apps()
                .find(|&app| catalogue.name(app) == name)
            {
                Some(app) => {
                    if let Some(open) = self.icons.menu.as_mut() {
                        open.app = app;
                    }
                }
                None => self.close_icon_menu(damage, events),
            }
        }
        self.catalogue = catalogue;
        self.icons_changed(damage);
        self.refilter_launcher(damage);
        self.damage_window_list(damage);
        if let Some(bar) = self.layout.taskbar {
            damage.add(bar);
        }
    }

    /// Whether the shell wants the catalogue read again (the launcher opened since the last
    /// call); clears the request.
    pub fn take_catalogue_request(&mut self) -> bool {
        core::mem::take(&mut self.catalogue_wanted)
    }

    /// What launching `app` asks of the kernel, as [`Action::Start`] and [`Action::Type`]
    /// name it.
    pub fn launch_command(&self, app: AppId) -> Option<Launch> {
        self.catalogue.launch(app)
    }

    /// What [`Action::Uninstall`] of `app` starts: the installer's window and its arguments.
    pub fn uninstall_command(&self, app: AppId) -> Option<Command> {
        self.catalogue.uninstall(app)
    }

    fn terminal(&self) -> &Window {
        self.windows[TERMINAL_SLOT]
            .as_ref()
            .expect("the terminal's window is never destroyed")
    }

    /// The terminal's window's id.
    pub fn terminal_window(&self) -> WindowId {
        self.terminal().id
    }

    /// The terminal window, its border included, wherever it is (also while minimised).
    pub fn window(&self) -> Rect {
        self.terminal().outside()
    }

    /// The terminal window's title bar, its border's top row included.
    pub fn title_bar(&self) -> Rect {
        self.terminal().title_bar()
    }

    /// Where the terminal's grid sits: inside the window's padding.
    pub fn text_area(&self) -> TextArea {
        let content = self.terminal().content();
        TextArea {
            x: content.x + PADDING,
            y: content.y + PADDING,
            columns: GRID_COLUMNS,
            rows: GRID_ROWS,
        }
    }

    /// Whether the terminal's window is on screen.
    pub fn terminal_shown(&self) -> bool {
        self.is_visible(TERMINAL_SLOT)
    }

    /// Whether keys go to the terminal: its window is focused and shown, and no menu or other
    /// popup is open.
    pub fn keys_go_to_terminal(&self) -> bool {
        self.focus == Some(TERMINAL_SLOT)
            && self.is_visible(TERMINAL_SLOT)
            && !self.menu_open
            && !self.popup_open()
    }

    /// Whether the terminal's window is focused.
    pub fn focused(&self) -> bool {
        self.focus == Some(TERMINAL_SLOT)
    }

    /// Whether the terminal's window is minimised to a chip in the bar.
    pub fn minimised(&self) -> bool {
        self.terminal().minimised
    }

    /// Whether the power menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    /// The focused window, if one is.
    pub fn focused_window(&self) -> Option<WindowId> {
        self.focus
            .and_then(|slot| self.windows[slot].map(|window| window.id))
    }

    /// The windows on screen, bottom to top.
    pub fn stacking(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.stack[..self.stack_length]
            .iter()
            .map(|&slot| usize::from(slot))
            .filter(|&slot| self.is_visible(slot))
            .filter_map(|slot| self.windows[slot].map(|window| window.id))
    }

    /// Window `id`'s rectangle, its border included, if it exists.
    pub fn window_rect(&self, id: WindowId) -> Option<Rect> {
        self.find(id).map(|slot| self.window_at(slot).outside())
    }

    /// Window `id`'s content on the screen, if it exists.
    pub fn content_rect(&self, id: WindowId) -> Option<Rect> {
        self.find(id).map(|slot| self.window_at(slot).content())
    }

    /// Window `id`'s close control's hit target, if it exists.
    pub fn close_control(&self, id: WindowId) -> Option<Rect> {
        self.control_of(id, Part::Close)
    }

    /// The hit target of window `id`'s control `part`, if the window exists and has it.
    pub fn control_of(&self, id: WindowId, part: Part) -> Option<Rect> {
        self.find(id).and_then(|slot| {
            self.window_at(slot)
                .control(part, self.profile.control_side())
        })
    }

    /// Window `id`'s title bar, its border's top row included, if it exists.
    pub fn title_bar_of(&self, id: WindowId) -> Option<Rect> {
        self.find(id).map(|slot| self.window_at(slot).title_bar())
    }

    /// Whether window `id` exists and is on screen.
    pub fn is_shown(&self, id: WindowId) -> bool {
        self.find(id).is_some_and(|slot| self.is_visible(slot))
    }

    /// Whether window `id` is `client`'s.
    pub fn owns(&self, client: u16, id: WindowId) -> bool {
        self.owned(client, u32::from(id)).is_ok()
    }

    /// The content size [`Self::set_size`] would give `client`'s window `id` for `width` by
    /// `height`.
    pub fn size_for(
        &self,
        client: u16,
        id: u32,
        width: u32,
        height: u32,
    ) -> Result<(u32, u32), WindowError> {
        let slot = self.owned(client, id)?;
        if slot == TERMINAL_SLOT {
            return Err(WindowError::Invalid);
        }
        let (minimum_width, minimum_height) = self.window_at(slot).minimum;
        let work = self.work_area();
        Ok(clamp_resized_content(
            work.width,
            work.height,
            width.max(minimum_width),
            height.max(minimum_height),
        ))
    }

    /// The content size [`Self::create`] gives a window asking for `width` by `height`.
    pub fn created_size(&self, width: u32, height: u32) -> (u32, u32) {
        let work = self.work_area();
        clamp_content_size(work.width, work.height, width, height)
    }

    /// How many windows `client` has.
    pub fn windows_of(&self, client: u16) -> usize {
        self.windows
            .iter()
            .flatten()
            .filter(|window| window.client == client)
            .count()
    }

    fn find(&self, id: WindowId) -> Option<usize> {
        let slot = slot_of(id);
        self.windows
            .get(slot)
            .copied()
            .flatten()
            .filter(|window| window.id == id)
            .map(|_| slot)
    }

    fn owned(&self, client: u16, id: u32) -> Result<usize, WindowError> {
        let id = WindowId::try_from(id).map_err(|_| WindowError::Invalid)?;
        self.find(id)
            .filter(|&slot| self.window_at(slot).client == client)
            .ok_or(WindowError::Invalid)
    }

    fn window_at(&self, slot: usize) -> &Window {
        self.windows[slot].as_ref().expect("a slot in use")
    }

    fn window_mut(&mut self, slot: usize) -> &mut Window {
        self.windows[slot].as_mut().expect("a slot in use")
    }

    fn is_visible(&self, slot: usize) -> bool {
        let Some(window) = self.windows[slot].as_ref() else {
            return false;
        };
        window.shown
            && !window.minimised
            && window
                .parent
                .is_none_or(|parent| self.is_visible(usize::from(parent)))
    }

    /// The terminal window's minimise control's hit target.
    pub fn minimise_control(&self) -> Rect {
        self.terminal()
            .control(Part::Minimise, self.profile.control_side())
            .expect("the terminal's window is a normal one")
    }

    /// The clock's text ([`Self::set_uptime`]), centred in [`CLOCK_GLYPHS`] cells.
    pub fn clock_text(&self) -> &str {
        core::str::from_utf8(&self.clock).unwrap_or("")
    }

    /// The terminal's chip in the bar, where it shows when the terminal is the only window
    /// minimised.
    pub fn chip(&self) -> Rect {
        chip_rect(CHIP_X, WINDOW_TITLE)
    }

    /// Window `id`'s chip in the bar, if it is minimised and its chip fits.
    pub fn chip_of(&self, id: WindowId) -> Option<Rect> {
        let mut found = None;
        self.for_each_chip(|slot, rect| {
            if self.window_at(slot).id == id {
                found = Some(rect);
            }
        });
        found
    }

    pub(super) fn chips_area(&self) -> Rect {
        let end = self.clock_rect().x.saturating_sub(CHIPS_CLEAR_OF_CLOCK);
        Rect::new(CHIP_X, CHIP_Y, end.saturating_sub(CHIP_X), CHIP_HEIGHT)
    }

    pub(super) fn for_each_chip(&self, mut chip: impl FnMut(usize, Rect)) {
        if self.profile != Profile::Linux {
            return;
        }
        let end = self.chips_area().right();
        let mut x = CHIP_X;
        for &slot in &self.created[..self.created_length] {
            let slot = usize::from(slot);
            let window = self.window_at(slot);
            if !window.shown || !window.minimised || window.role != Role::Normal {
                continue;
            }
            let rect = chip_rect(x, window.title());
            if rect.right() > end {
                return;
            }
            chip(slot, rect);
            x = rect.right() + CHIP_GAP;
        }
    }

    fn menu_with_shadow(&self) -> Rect {
        let menu = self.menu();
        Rect::new(
            menu.x - SHADOW_RINGS,
            menu.y,
            menu.width + 2 * SHADOW_RINGS,
            menu.height + SHADOW_RINGS,
        )
    }

    /// Shows `seconds` of uptime on the clock, in at most [`CLOCK_GLYPHS`]: `up m:ss` for the
    /// first hour, then `up h:mm:ss`, `up 10h 05m` from ten hours and `up 4d 03h` from a
    /// hundred hours, `up 100d` from a hundred days. Returns whether its text changed, adding
    /// its rectangle to `damage` if so.
    pub fn set_uptime(&mut self, seconds: u64, damage: &mut Damage) -> bool {
        let mut text = [b' '; CLOCK_GLYPHS];
        let mut written = Writer::new(&mut text);
        written.push_str("up ");
        let (minutes, hours) = (seconds / SECONDS_PER_MINUTE, seconds / SECONDS_PER_HOUR);
        let days = hours / HOURS_PER_DAY;
        let fields: [(u64, usize, &str); 3] = match hours {
            0 => [
                (minutes, 1, ":"),
                (seconds % SECONDS_PER_MINUTE, 2, ""),
                (0, 0, ""),
            ],
            1..10 => [
                (hours, 1, ":"),
                (minutes % MINUTES_PER_HOUR, 2, ":"),
                (seconds % SECONDS_PER_MINUTE, 2, ""),
            ],
            10..100 => [
                (hours, 2, "h "),
                (minutes % MINUTES_PER_HOUR, 2, "m"),
                (0, 0, ""),
            ],
            _ if days < 100 => [(days, 1, "d "), (hours % HOURS_PER_DAY, 2, "h"), (0, 0, "")],
            _ => [(days.min(MOST_DAYS_SHOWN), 1, "d"), (0, 0, ""), (0, 0, "")],
        };
        for (value, digits, unit) in fields.into_iter().filter(|field| field.1 > 0) {
            written.push_number(value, digits);
            written.push_str(unit);
        }
        let length = written.length;
        let shift = (CLOCK_GLYPHS - length) / 2;
        let mut centred = [b' '; CLOCK_GLYPHS];
        centred[shift..shift + length].copy_from_slice(&text[..length]);
        if centred == self.clock {
            return false;
        }
        self.clock = centred;
        damage.add(self.clock_rect());
        true
    }

    /// A window for `client`, not shown until its first [`Self::commit`]: `role` and `parent`
    /// as the contract numbers them, a content of `width` by `height` clamped as
    /// [`clamp_content_size`] says, and `window_flags` ([`protocol::flags`]). Clients may make
    /// normal windows and dialogs of their own normal windows.
    pub fn create(
        &mut self,
        client: u16,
        role: u32,
        parent: u32,
        width: u32,
        height: u32,
        window_flags: u32,
    ) -> Result<Created, WindowError> {
        let role = Role::from_value(role).ok_or(WindowError::Invalid)?;
        if !matches!(role, Role::Normal | Role::Dialog) {
            return Err(WindowError::NotAllowed);
        }
        if window_flags & !flags::KNOWN != 0 {
            return Err(WindowError::Invalid);
        }
        let parent = match role {
            Role::Dialog => {
                let parent = self.owned(client, parent)?;
                if self.window_at(parent).role != Role::Normal {
                    return Err(WindowError::Invalid);
                }
                Some(parent as u8)
            }
            _ => None,
        };
        if self.windows_of(client) >= MAX_WINDOWS_PER_CLIENT {
            return Err(WindowError::TableFull);
        }
        let slot = self
            .windows
            .iter()
            .position(Option::is_none)
            .ok_or(WindowError::TableFull)?;
        let (width, height) = self.created_size(width, height);
        let mut window = Window::new(
            self.id_for(slot),
            client,
            role,
            (width, height),
            colour::WINDOW_BACKGROUND,
        );
        window.parent = parent;
        window.flags = window_flags;
        self.dialogs += usize::from(parent.is_some());
        self.windows[slot] = Some(window);
        self.created[self.created_length] = slot as u8;
        self.created_length += 1;
        Ok(Created {
            window: window.id,
            width,
            height,
        })
    }

    /// Gives `client`'s window `id` the title `title` (at most [`TITLE_CAPACITY`] bytes of
    /// UTF-8).
    pub fn set_title(
        &mut self,
        client: u16,
        id: u32,
        title: &[u8],
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        if title.len() > TITLE_CAPACITY {
            return Err(WindowError::Invalid);
        }
        let title = core::str::from_utf8(title).map_err(|_| WindowError::Invalid)?;
        if self.window_at(slot).title() == title {
            return Ok(());
        }
        self.window_mut(slot).set_title(title);
        if self.is_visible(slot) {
            damage.add(self.window_at(slot).title_bar());
        }
        let window = self.window_at(slot);
        if window.shown && window.role == Role::Normal {
            self.damage_window_list(damage);
        }
        Ok(())
    }

    /// Gives `client`'s window `id` a buffer of `width` by `height`, clamped to its minimum
    /// and the work area ([`Self::size_for`]), and returns the size it got; its content shows
    /// [`colour::WINDOW_BACKGROUND`] until the next commit. While a `Resize` the server sent
    /// waits for this answer, the window keeps the size the server gave it (the buffer is shown
    /// clipped to it); otherwise the window takes the new size, keeping its top-left corner
    /// unless the work area's edge moves it, and a maximised window becomes a normal one.
    pub fn set_size(
        &mut self,
        client: u16,
        id: u32,
        width: u32,
        height: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(u32, u32), WindowError> {
        let (width, height) = self.size_for(client, id, width, height)?;
        let slot = self.owned(client, id)?;
        let window = *self.window_at(slot);
        let visible = self.is_visible(slot);
        if visible {
            damage.add(window.content());
        }
        let resized = self.window_mut(slot);
        resized.buffer = Rect::new(0, 0, width, height);
        resized.ready = false;
        if resized.pending_resize == Some((width, height)) {
            resized.pending_resize = None;
        }
        let own_size = (window.content_width, window.content_height) == (width, height);
        if window.pending_resize.is_some() || own_size {
            return Ok((width, height));
        }
        if visible {
            damage.add(window.with_shadow());
        }
        let resized = self.window_mut(slot);
        resized.content_width = width;
        resized.content_height = height;
        resized.maximised = false;
        resized.before_maximised = None;
        resized.square_corners = false;
        self.end_resize_of(slot);
        self.move_window(slot, i64::from(window.x), i64::from(window.y), damage);
        if visible {
            damage.add(self.window_at(slot).with_shadow());
        }
        if window.maximised {
            let state = WindowState::Normal.value();
            self.send(
                slot,
                EventRecord::new(event::STATE_CHANGED, 0, state as i32, 0, 0, 0),
                events,
            );
        }
        self.update_pointer(damage);
        Ok((width, height))
    }

    /// Records the smallest content `client`'s window `id` may be resized to.
    pub fn set_minimum_size(
        &mut self,
        client: u16,
        id: u32,
        width: u32,
        height: u32,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        let work = self.work_area();
        self.window_mut(slot).minimum = (
            width.min(work.width.saturating_sub(CHROME.0)),
            height.min(work.height.saturating_sub(CHROME.1)),
        );
        Ok(())
    }

    /// Places `client`'s window `id` with its top-left corner at (`x`, `y`) of the work area,
    /// clamped so the whole window is inside it: at once if it is shown, else when it first is.
    pub fn set_position(
        &mut self,
        client: u16,
        id: u32,
        x: i32,
        y: i32,
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        self.window_mut(slot).requested_position = Some((x, y));
        if self.window_at(slot).shown {
            let work = self.work_area();
            self.move_window(
                slot,
                i64::from(work.x) + i64::from(x),
                i64::from(work.y) + i64::from(y),
                damage,
            );
        }
        Ok(())
    }

    /// Minimises, maximises or restores `client`'s window `id`, as its controls and chip do.
    /// Only a normal window that is not fixed-size maximises; dialogs do not minimise.
    pub fn set_state(
        &mut self,
        client: u16,
        id: u32,
        state: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        match WindowState::from_value(state).ok_or(WindowError::Invalid)? {
            WindowState::Maximised if !self.window_at(slot).can_maximise() => {
                return Err(WindowError::Invalid);
            }
            WindowState::Maximised => {
                if self.window_at(slot).shown {
                    self.restore(slot, damage, events);
                }
                self.maximise(slot, damage, events);
            }
            WindowState::Minimised if self.window_at(slot).role != Role::Normal => {
                return Err(WindowError::Invalid);
            }
            WindowState::Minimised if self.window_at(slot).shown => {
                self.minimise(slot, damage, events)
            }
            WindowState::Minimised => self.window_mut(slot).minimised = true,
            WindowState::Normal => {
                self.unmaximise(slot, damage, events);
                match self.window_at(slot).shown {
                    true => self.restore(slot, damage, events),
                    false => self.window_mut(slot).minimised = false,
                }
            }
        }
        self.update_pointer(damage);
        Ok(())
    }

    /// Records the pointer shape `client`'s window `id` asks for over its content.
    pub fn set_pointer(
        &mut self,
        client: u16,
        id: u32,
        shape: u32,
        damage: &mut Damage,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        self.window_mut(slot).pointer_shape =
            PointerShape::from_value(shape).ok_or(WindowError::Invalid)?;
        self.update_pointer(damage);
        Ok(())
    }

    /// `client` drew `rects` of window `id`'s content (all of it when there are none): shows
    /// the window if this is its first commit (placed, on top, focused), and adds what changed
    /// on screen to `damage`.
    pub fn commit(
        &mut self,
        client: u16,
        id: u32,
        rects: &[Rect],
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        let window = *self.window_at(slot);
        let was_ready = window.ready;
        self.window_mut(slot).ready = true;
        if !window.shown {
            self.show(slot, damage, events);
            return Ok(());
        }
        if !self.is_visible(slot) {
            return Ok(());
        }
        let content = window.content();
        if !was_ready || rects.is_empty() {
            damage.add(content);
            return Ok(());
        }
        for rect in rects {
            let clipped = rect.clipped(content.width, content.height);
            damage.add(Rect::new(
                content.x + clipped.x,
                content.y + clipped.y,
                clipped.width,
                clipped.height,
            ));
        }
        Ok(())
    }

    /// Destroys `client`'s window `id` and its dialogs.
    pub fn destroy(
        &mut self,
        client: u16,
        id: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        if slot == TERMINAL_SLOT {
            return Err(WindowError::Invalid);
        }
        self.destroy_slot(slot, damage, events);
        self.update_pointer(damage);
        Ok(())
    }

    /// Destroys every window of `client`'s, as when it closes `/dev/window`.
    pub fn destroy_client(&mut self, client: u16, damage: &mut Damage, events: &mut dyn EventSink) {
        if client == KERNEL_CLIENT {
            return;
        }
        for slot in 0..MAX_WINDOWS {
            if self.windows[slot].is_some_and(|window| window.client == client) {
                self.destroy_slot(slot, damage, events);
            }
        }
        self.update_pointer(damage);
    }

    fn destroy_slot(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        for dialog in 0..MAX_WINDOWS {
            if self.windows[dialog].is_some_and(|window| window.parent == Some(slot as u8)) {
                self.destroy_slot(dialog, damage, events);
            }
        }
        if self.is_visible(slot) {
            damage.add(self.window_at(slot).with_shadow());
        }
        self.damage_window_list(damage);
        self.forget_in_panels(slot);
        let focused = self.focus == Some(slot);
        if self.drag.is_some_and(|drag| drag.window == slot) {
            self.drag = None;
        }
        if self.grab == Some(slot) {
            self.grab = None;
        }
        if self.hover == Some(slot) {
            self.hover = None;
        }
        self.end_resize_of(slot);
        if self.last_title_press.is_some_and(|press| press.0 == slot) {
            self.last_title_press = None;
        }
        if self.control_hover.is_some_and(|hover| hover.0 == slot) {
            self.control_hover = None;
        }
        self.forget_in_popups(slot, damage, events);
        if focused {
            self.focus = None;
        }
        remove(&mut self.stack, &mut self.stack_length, slot);
        remove(&mut self.created, &mut self.created_length, slot);
        if self.window_at(slot).parent.is_some() {
            self.dialogs -= 1;
        }
        self.windows[slot] = None;
        self.generations[slot] = match self.generations[slot] + 1 {
            GENERATIONS => FIRST_GENERATION,
            next => next,
        };
        if focused {
            self.focus_top(damage, events);
        }
    }

    fn show(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        self.place(slot);
        self.window_mut(slot).shown = true;
        self.stack[self.stack_length] = slot as u8;
        self.stack_length += 1;
        if self.window_at(slot).role == Role::Normal {
            self.damage_window_list(damage);
            self.app_window_shown(slot);
        }
        if self.window_at(slot).minimised {
            return;
        }
        if !self.is_visible(slot) {
            return;
        }
        self.raise(slot, damage);
        damage.add(self.window_at(slot).with_shadow());
        let focus = self.top_dialog(slot).unwrap_or(slot);
        self.set_focus(Some(focus), damage, events);
        self.update_pointer(damage);
    }

    fn place(&mut self, slot: usize) {
        let window = *self.window_at(slot);
        if window.maximised {
            return;
        }
        let outside = window.outside();
        let work = self.work_area();
        let (x, y) = if let Some((x, y)) = window.requested_position {
            (
                i64::from(work.x) + i64::from(x),
                i64::from(work.y) + i64::from(y),
            )
        } else if let Some(parent) = window.parent {
            let parent = self.window_at(usize::from(parent)).outside();
            (
                i64::from(parent.x) + (i64::from(parent.width) - i64::from(outside.width)) / 2,
                i64::from(parent.y) + (i64::from(parent.height) - i64::from(outside.height)) / 2,
            )
        } else {
            match self.top_normal(slot) {
                None => (
                    i64::from(work.x + work.width.saturating_sub(outside.width) / 2),
                    i64::from(work.y + work.height.saturating_sub(outside.height) / 2),
                ),
                Some(top) => {
                    let top = self.window_at(top).outside();
                    let fits = |x: u32, y: u32| {
                        x + outside.width <= work.right() && y + outside.height <= work.bottom()
                    };
                    let cascaded = (top.x + CASCADE_STEP, top.y + CASCADE_STEP);
                    let restarted = (work.x + CASCADE_STEP, work.y + CASCADE_STEP);
                    let (x, y) = if fits(cascaded.0, cascaded.1) {
                        cascaded
                    } else if fits(restarted.0, restarted.1) {
                        restarted
                    } else {
                        (work.x, work.y)
                    };
                    (i64::from(x), i64::from(y))
                }
            }
        };
        let (x, y) = self.clamped(slot, x, y);
        let window = self.window_mut(slot);
        window.x = x;
        window.y = y;
    }

    fn top_normal(&self, except: usize) -> Option<usize> {
        self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&slot| usize::from(slot))
            .find(|&slot| {
                slot != except && self.is_visible(slot) && self.window_at(slot).role == Role::Normal
            })
    }

    fn clamped(&self, slot: usize, x: i64, y: i64) -> (u32, u32) {
        let outside = self.window_at(slot).outside();
        let work = self.work_area();
        let x = x.clamp(
            i64::from(work.x),
            i64::from(work.x + work.width.saturating_sub(outside.width)),
        );
        let y = y.clamp(
            i64::from(work.y),
            i64::from(work.y + work.height.saturating_sub(outside.height)),
        );
        (x as u32, y as u32)
    }

    fn move_window(&mut self, slot: usize, x: i64, y: i64, damage: &mut Damage) {
        let (x, y) = self.clamped(slot, x, y);
        let window = self.window_at(slot);
        if (x, y) == (window.x, window.y) {
            return;
        }
        let visible = self.is_visible(slot);
        if visible {
            damage.add(window.with_shadow());
        }
        let window = self.window_mut(slot);
        window.x = x;
        window.y = y;
        window.square_corners = false;
        if visible {
            damage.add(self.window_at(slot).with_shadow());
        }
    }

    fn raise(&mut self, slot: usize, damage: &mut Damage) {
        if let Some(parent) = self.window_at(slot).parent {
            self.raise(usize::from(parent), damage);
            to_top(&mut self.stack, self.stack_length, slot);
            if self.is_visible(slot) {
                damage.add(self.window_at(slot).with_shadow());
            }
            return;
        }
        let top = self.stack[..self.stack_length]
            .last()
            .map(|&top| usize::from(top));
        to_top(&mut self.stack, self.stack_length, slot);
        let mut index = 0;
        let mut moved = 0;
        while index + moved < self.stack_length {
            let candidate = usize::from(self.stack[index]);
            if candidate != slot && self.window_at(candidate).parent == Some(slot as u8) {
                to_top(&mut self.stack, self.stack_length, candidate);
                moved += 1;
            } else {
                index += 1;
            }
        }
        if top != Some(slot) && self.is_visible(slot) {
            damage.add(self.window_at(slot).with_shadow());
            for &dialog in &self.stack[self.stack_length - moved..self.stack_length] {
                let dialog = usize::from(dialog);
                if self.is_visible(dialog) {
                    damage.add(self.window_at(dialog).with_shadow());
                }
            }
        }
    }

    fn top_dialog(&self, slot: usize) -> Option<usize> {
        if self.dialogs == 0 {
            return None;
        }
        self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&dialog| usize::from(dialog))
            .find(|&dialog| {
                self.window_at(dialog).parent == Some(slot as u8) && self.is_visible(dialog)
            })
    }

    fn set_focus(&mut self, focus: Option<usize>, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.focus == focus {
            return;
        }
        if focus.is_some() {
            self.set_desktop_focused(false, damage);
        }
        self.damage_focus_in_window_list(damage);
        let quiet = self.popup_open();
        if let Some(old) = self.focus {
            if self.is_visible(old) {
                damage.add(self.window_at(old).outside());
            }
            if !quiet {
                self.send(
                    old,
                    EventRecord::new(event::FOCUS_OUT, 0, 0, 0, 0, 0),
                    events,
                );
            }
        }
        self.focus = focus;
        if let Some(new) = focus {
            self.focus_serial = self.focus_serial.wrapping_add(1);
            let top = self.window_at(new).parent.map_or(new, usize::from);
            let serial = self.focus_serial;
            self.window_mut(top).focus_serial = serial;
            damage.add(self.window_at(new).outside());
            if !quiet {
                self.send(
                    new,
                    EventRecord::new(event::FOCUS_IN, 0, 0, 0, 0, 0),
                    events,
                );
            }
        }
    }

    fn focus_top(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let top = self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&slot| usize::from(slot))
            .find(|&slot| self.is_visible(slot));
        self.set_focus(top, damage, events);
        if top.is_none() {
            self.set_desktop_focused(true, damage);
        }
    }

    fn send(&self, slot: usize, mut record: EventRecord, events: &mut dyn EventSink) {
        let window = self.window_at(slot);
        if window.client != KERNEL_CLIENT {
            record.window = window.id;
            events.send(window.client, record);
        }
    }

    fn minimise(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        if self.window_at(slot).minimised {
            return;
        }
        let group = |desktop: &Self, other: usize| {
            other == slot || desktop.window_at(other).parent == Some(slot as u8)
        };
        for &other in &self.stack[..self.stack_length] {
            let other = usize::from(other);
            if self.is_visible(other) && group(self, other) {
                damage.add(self.window_at(other).with_shadow());
            }
        }
        if self.drag.is_some_and(|drag| group(self, drag.window)) {
            self.drag = None;
        }
        if let Some(hover) = self.hover.filter(|&hover| group(self, hover)) {
            self.send(
                hover,
                EventRecord::new(event::POINTER_LEAVE, 0, 0, 0, 0, 0),
                events,
            );
            self.hover = None;
        }
        if self.grab.is_some_and(|grab| group(self, grab)) {
            self.grab = None;
        }
        if self.resize.is_some_and(|resize| group(self, resize.window)) {
            self.resize = None;
        }
        if self
            .window_menu
            .is_some_and(|menu| group(self, menu.window))
        {
            self.close_popups(damage, events);
        }
        self.window_mut(slot).minimised = true;
        self.damage_window_list(damage);
        self.send_state(slot, events);
        if self.focus.is_some_and(|focus| group(self, focus)) {
            self.focus_top(damage, events);
        }
    }

    fn restore(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        if !self.window_at(slot).minimised {
            return;
        }
        self.window_mut(slot).minimised = false;
        self.damage_window_list(damage);
        self.raise(slot, damage);
        damage.add(self.window_at(slot).with_shadow());
        self.send_state(slot, events);
        let focus = self.top_dialog(slot).unwrap_or(slot);
        self.set_focus(Some(focus), damage, events);
    }

    fn top_window_at(&self, x: u32, y: u32) -> Option<usize> {
        if self.over_panel(x, y) {
            return None;
        }
        self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&slot| usize::from(slot))
            .find(|&slot| self.is_visible(slot) && contains(self.window_at(slot).outside(), x, y))
    }

    fn content_window_at(&self, x: u32, y: u32) -> Option<usize> {
        self.content_under(self.hit(x, y))
    }

    fn content_point(&self, slot: usize) -> (i32, i32) {
        let content = self.window_at(slot).content();
        let (x, y) = self.pointer;
        (
            (i64::from(x) - i64::from(content.x)) as i32,
            (i64::from(y) - i64::from(content.y)) as i32,
        )
    }

    /// The pointer moved to (`x`, `y`): drags or resizes the window while a drag or resize is
    /// on, moves the menus' hover, tells the window under the pointer (or the one holding it)
    /// where it is, and draws the pointer there in the shape the place asks for.
    pub fn pointer_moved(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.pointer = (x, y);
        self.pointer_visible = true;
        if let Some(resize) = self.resize {
            self.resize_to(resize, x, y, damage, events);
        }
        if let Some(drag) = self.drag {
            self.drag_to(drag, x, y, damage, events);
        }
        if self.menu_open {
            let hover = self.menu_item_at(x, y);
            if hover != self.menu_hover {
                self.menu_hover = hover;
                damage.add(self.menu());
            }
        }
        self.window_menu_hover(x, y, damage);
        self.launcher_hover(x, y, damage);
        self.icon_menu_hover(x, y, damage);
        self.update_panel_hover(x, y, damage);
        let hit = self.hit(x, y);
        let under = match (self.grab, self.drag.is_some() || self.resize.is_some()) {
            (Some(grab), _) => Some(grab),
            (None, true) => None,
            (None, false) => self.content_under(hit),
        };
        if under != self.hover {
            if let Some(old) = self.hover {
                self.send(
                    old,
                    EventRecord::new(event::POINTER_LEAVE, 0, 0, 0, 0, 0),
                    events,
                );
            }
            if let Some(new) = under {
                let (cx, cy) = self.content_point(new);
                self.send(
                    new,
                    EventRecord::new(event::POINTER_ENTER, 0, cx, cy, 0, 0),
                    events,
                );
            }
            self.hover = under;
        }
        if let Some(slot) = under {
            let (cx, cy) = self.content_point(slot);
            let buttons = u32::from(self.buttons);
            self.send(
                slot,
                EventRecord::new(event::POINTER_MOVE, 0, cx, cy, buttons, 0),
                events,
            );
        }
        self.update_control_hover_over(hit, damage);
        self.update_pointer_over(hit, damage);
    }

    fn drag_to(
        &mut self,
        mut drag: Drag,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let window = *self.window_at(drag.window);
        if window.maximised {
            let moved = x
                .abs_diff(drag.pressed_at.0)
                .max(y.abs_diff(drag.pressed_at.1));
            if moved <= RESTORE_DRAG_DISTANCE {
                return;
            }
            let width = window.outside().width.max(1);
            let restored = window
                .before_maximised
                .map_or(width, |before| before.content_width + CHROME.0);
            drag.from_pointer_x = drag.from_pointer_x * i64::from(restored) / i64::from(width);
            self.unmaximise(drag.window, damage, events);
            self.drag = Some(drag);
        }
        self.move_window(
            drag.window,
            i64::from(x) - drag.from_pointer_x,
            i64::from(y) - drag.from_pointer_y,
            damage,
        );
    }

    fn menu_item_at(&self, x: u32, y: u32) -> Option<usize> {
        let menu = self.menu();
        if !contains(menu, x, y) {
            return None;
        }
        let item = (y.saturating_sub(menu.y + 1) / MENU_ITEM_HEIGHT) as usize;
        (item < MENU_ITEMS.len()).then_some(item)
    }

    /// The left button went down (`pressed`) or up at the pointer.
    pub fn left_button(
        &mut self,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        self.button(button::LEFT, pressed, damage, events)
    }

    /// Pointer button `code` (`BTN_LEFT`, `BTN_RIGHT` or `BTN_MIDDLE`) went down (`pressed`) or
    /// up at the pointer: the popups, the bar's items, the power menu, a window's resize zones,
    /// controls and title bar (a double-click maximises or restores, a right-click opens the
    /// window menu), or its content, which raises and focuses it and holds the pointer for it
    /// until the buttons are up.
    pub fn button(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let action = self.button_change(code, pressed, damage, events);
        self.update_control_hover(damage);
        self.update_pointer(damage);
        self.refresh_panel_hover(damage);
        action
    }

    fn button_change(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let Some(bit) = BUTTON_CODES.iter().position(|&known| known == code) else {
            return Action::None;
        };
        let bit = 1u8 << bit;
        if !pressed {
            self.buttons &= !bit;
            if code == button::LEFT {
                self.drag = None;
                self.resize = None;
                self.panel_release(damage);
            }
            if let Some(grab) = self.grab {
                let (cx, cy) = self.content_point(grab);
                self.send(
                    grab,
                    EventRecord::new(event::BUTTON, 0, cx, cy, u32::from(code), 0),
                    events,
                );
                if self.buttons == 0 {
                    self.grab = None;
                    let (x, y) = self.pointer;
                    self.pointer_moved(x, y, damage, events);
                }
            }
            return Action::None;
        }
        self.buttons |= bit;
        if let Some(grab) = self.grab {
            let (cx, cy) = self.content_point(grab);
            self.send(
                grab,
                EventRecord::new(event::BUTTON, 0, cx, cy, u32::from(code), 1),
                events,
            );
            return Action::None;
        }
        let (x, y) = self.pointer;
        self.super_alone = false;
        self.close_notice(damage);
        if self.switcher_press(x, y, damage, events) || self.window_menu_press(x, y, damage, events)
        {
            return Action::None;
        }
        if let Some(action) = self.icon_menu_press(x, y, damage, events) {
            return action;
        }
        let opener = self.panel_item_at(x, y);
        let launcher_was_open = self.launcher.is_some();
        if let Some(action) = self.launcher_press(code, x, y, damage, events) {
            return action;
        }
        let opens_launcher = matches!(
            opener,
            Some(shell::PanelItem::Start | shell::PanelItem::System)
        );
        if launcher_was_open && opens_launcher {
            return Action::None;
        }
        let last_title_press = self.last_title_press.take();
        if code != button::LEFT {
            return self.other_button_down(code, damage, events);
        }
        if self.menu_open {
            let item = self.menu_item_at(x, y);
            self.close_menu(damage);
            if let Some(item) = item {
                return MENU_ITEMS[item].1;
            }
            if contains(self.power_button(), x, y) {
                return Action::None;
            }
        }
        if let Some(item) = opener {
            return self.panel_press(item, damage, events);
        }
        if self.over_panel(x, y) {
            return Action::None;
        }
        if let Some((slot, edges)) = self.resize_target(x, y) {
            self.raise(slot, damage);
            self.set_focus(Some(slot), damage, events);
            self.start_resize(slot, edges);
            return Action::None;
        }
        let Some(slot) = self.top_window_at(x, y) else {
            return self.wallpaper_press(code, x, y, damage, events);
        };
        self.raise(slot, damage);
        if let Some(dialog) = self.top_dialog(slot) {
            self.set_focus(Some(dialog), damage, events);
            return Action::None;
        }
        self.set_focus(Some(slot), damage, events);
        let window = *self.window_at(slot);
        match window.part_at(x, y, self.profile.control_side()) {
            Some(Part::Minimise) => self.minimise(slot, damage, events),
            Some(Part::Maximise) if window.can_maximise() => {
                self.toggle_maximise(slot, damage, events)
            }
            Some(Part::Close) => self.request_close(slot, events),
            Some(Part::TitleBar) => {
                let press = (x, y, self.now_ms);
                let double = last_title_press.is_some_and(|(previous, px, py, at)| {
                    previous == slot && is_double_click((px, py, at), press)
                });
                if double && window.can_maximise() {
                    self.toggle_maximise(slot, damage, events);
                } else {
                    self.last_title_press = Some((slot, x, y, self.now_ms));
                    self.drag = Some(Drag {
                        window: slot,
                        from_pointer_x: i64::from(x) - i64::from(window.x),
                        from_pointer_y: i64::from(y) - i64::from(window.y),
                        pressed_at: (x, y),
                    });
                }
            }
            Some(Part::Content) => self.press_in_content(slot, code, events),
            Some(Part::Maximise | Part::Border) | None => {}
        }
        Action::None
    }

    fn other_button_down(
        &mut self,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let (x, y) = self.pointer;
        if self.menu_open || self.over_panel(x, y) {
            return Action::None;
        }
        let Some(slot) = self.top_window_at(x, y) else {
            return self.wallpaper_press(code, x, y, damage, events);
        };
        self.raise(slot, damage);
        if let Some(dialog) = self.top_dialog(slot) {
            self.set_focus(Some(dialog), damage, events);
            return Action::None;
        }
        self.set_focus(Some(slot), damage, events);
        match self
            .window_at(slot)
            .part_at(x, y, self.profile.control_side())
        {
            Some(Part::Content) => self.press_in_content(slot, code, events),
            Some(Part::TitleBar) if code == button::RIGHT => {
                self.open_window_menu(slot, Some((x, y)), damage, events)
            }
            _ => {}
        }
        Action::None
    }

    fn press_in_content(&mut self, slot: usize, code: u16, events: &mut dyn EventSink) {
        self.grab = Some(slot);
        if self.hover != Some(slot) {
            let (cx, cy) = self.content_point(slot);
            self.send(
                slot,
                EventRecord::new(event::POINTER_ENTER, 0, cx, cy, 0, 0),
                events,
            );
            self.hover = Some(slot);
        }
        let (cx, cy) = self.content_point(slot);
        self.send(
            slot,
            EventRecord::new(event::BUTTON, 0, cx, cy, u32::from(code), 1),
            events,
        );
    }

    /// The wheel turned by `notches`, up when positive, over the window under the pointer.
    pub fn wheel(&mut self, notches: i32, events: &mut dyn EventSink) {
        let (x, y) = self.pointer;
        let target = self.grab.or_else(|| self.content_window_at(x, y));
        if let Some(slot) = target {
            let (cx, cy) = self.content_point(slot);
            self.send(
                slot,
                EventRecord::new(event::WHEEL, 0, cx, cy, 0, notches as u32),
                events,
            );
        }
    }

    /// Key `code` went down (`pressed`) or up with `modifiers` in effect: the open popup's
    /// keys, the server's shortcuts, the power menu's Up, Down, Enter and Esc, or a `Key` event
    /// for the focused window's client.
    pub fn key(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let action = self.key_change(code, pressed, modifiers, damage, events);
        self.update_control_hover(damage);
        self.update_pointer(damage);
        self.refresh_panel_hover(damage);
        action
    }

    fn key_change(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let super_key = matches!(code, key::LEFT_SUPER | key::RIGHT_SUPER);
        let alone_released = super_key && !pressed && self.super_alone;
        self.super_alone = super_key
            && pressed
            && modifiers.bits() & !Modifiers::SUPER.bits() & !Modifiers::CAPS_LOCK.bits() == 0;
        if pressed {
            self.close_notice(damage);
        }
        if self.switcher.is_some() {
            self.switcher_key(code, pressed, modifiers, damage, events);
            return Action::None;
        }
        if alone_released && self.window_menu.is_none() && self.icons.menu.is_none() {
            self.close_menu_if_open(damage);
            self.toggle_launcher(damage, events);
            return Action::None;
        }
        if self.icons.menu.is_some() {
            return self.icon_menu_key(code, pressed, damage, events);
        }
        if self.launcher.is_some() {
            if pressed && shortcut(code, modifiers, self.profile, false) == Some(Shortcut::Launcher)
            {
                self.close_launcher(damage, events);
                return Action::None;
            }
            return self.launcher_key(code, pressed, modifiers, damage, events);
        }
        if self.window_menu.is_some() {
            self.window_menu_key(code, pressed, damage, events);
            return Action::None;
        }
        if !self.menu_open {
            if pressed
                && let Some(shortcut) = shortcut(
                    code,
                    modifiers,
                    self.profile,
                    self.focused_takes_ctrl_keys(),
                )
            {
                return self.run_shortcut(shortcut, damage, events);
            }
            if pressed && self.desktop_focused() {
                return self.desktop_key(code, modifiers, damage, events);
            }
            if let Some(slot) = self.focus.filter(|&slot| self.is_visible(slot)) {
                let record = EventRecord::new(
                    event::KEY,
                    0,
                    i32::from(code),
                    i32::from(pressed),
                    key_modifiers(modifiers),
                    codepoint(code, modifiers),
                );
                self.send(slot, record, events);
            }
            return Action::None;
        }
        if !pressed {
            return Action::None;
        }
        let last = MENU_ITEMS.len() - 1;
        match code {
            key::ESC => self.close_menu(damage),
            key::UP => {
                self.menu_hover = Some(self.menu_hover.map_or(last, |item| item.saturating_sub(1)));
                damage.add(self.menu());
            }
            key::DOWN => {
                self.menu_hover = Some(self.menu_hover.map_or(0, |item| (item + 1).min(last)));
                damage.add(self.menu());
            }
            key::ENTER | key::KEYPAD_ENTER => {
                if let Some(item) = self.menu_hover {
                    self.close_menu(damage);
                    return MENU_ITEMS[item].1;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn open_menu(&mut self, damage: &mut Damage) {
        self.menu_open = true;
        self.menu_hover = self.menu_item_at(self.pointer.0, self.pointer.1);
        damage.add(self.menu_with_shadow());
        damage.add(self.power_button());
    }

    fn close_menu_if_open(&mut self, damage: &mut Damage) {
        if self.menu_open {
            self.close_menu(damage);
        }
    }

    fn close_menu(&mut self, damage: &mut Damage) {
        self.menu_open = false;
        self.menu_hover = None;
        damage.add(self.menu_with_shadow());
        damage.add(self.power_button());
    }

    /// Hides the terminal's window and shows its chip in the bar.
    pub fn minimise_terminal(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        self.minimise(TERMINAL_SLOT, damage, events);
    }

    /// Paints the part of the desktop inside `clip` into `pixels`, rows `stride` pixels apart,
    /// from scratch: the wallpaper, the windows bottom to top with their shadows and their
    /// content from `contents`, the bar, the menus and the switcher, then the pointer. A
    /// rectangle inside the content of the top window there is only copied from that window's
    /// buffer, under the pointer. Paints nothing outside the screen or `pixels`.
    pub fn draw(&self, pixels: &mut [u32], stride: usize, clip: Rect, contents: &dyn Contents) {
        let clip = clip.clipped(self.width, self.height);
        let Some(mut canvas) = Canvas::new(pixels, stride, clip) else {
            return;
        };
        if let Some(slot) = self.content_holding(clip) {
            self.draw_content(&mut canvas, self.window_at(slot), contents);
            self.draw_pointer(&mut canvas);
            return;
        }
        self.draw_wallpaper(&mut canvas);
        self.draw_desktop_icons(&mut canvas);
        for &slot in &self.stack[..self.stack_length] {
            let slot = usize::from(slot);
            let window = self.window_at(slot);
            if self.is_visible(slot) && window.with_shadow().overlaps(&clip) {
                if window.has_shadow() {
                    canvas.shadow(window.outside());
                }
                self.draw_window(&mut canvas, slot, contents);
            }
        }
        self.draw_panels(&mut canvas);
        if self.menu_open {
            canvas.shadow(self.menu());
            self.draw_menu(&mut canvas);
        }
        self.draw_popups(&mut canvas);
        self.draw_tooltip(&mut canvas);
        self.draw_pointer(&mut canvas);
    }

    fn draw_pointer(&self, canvas: &mut Canvas<'_>) {
        let Some((x, y, shape)) = self.pointer_drawn else {
            return;
        };
        let area = pointer::area(shape, x, y);
        let clip = canvas.clip();
        if !area.overlaps(&clip) {
            return;
        }
        for row in area.y.max(clip.y)..area.bottom().min(clip.bottom()) {
            for column in area.x.max(clip.x)..area.right().min(clip.right()) {
                if let Some(pixel) = pointer::pixel_at(shape, (x, y), column, row) {
                    canvas.pixel(column, row, pixel);
                }
            }
        }
    }

    fn content_holding(&self, clip: Rect) -> Option<usize> {
        let tooltip = self
            .panels
            .tooltip_area
            .is_some_and(|rect| rect.overlaps(&clip));
        let panels = self.layout.panels().any(|panel| panel.overlaps(&clip));
        if (self.menu_open && self.menu_with_shadow().overlaps(&clip))
            || self.popups_overlap(clip)
            || tooltip
            || panels
        {
            return None;
        }
        let slot = self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&slot| usize::from(slot))
            .find(|&slot| {
                self.is_visible(slot) && self.window_at(slot).with_shadow().overlaps(&clip)
            })?;
        let content = self.window_at(slot).content();
        let inside = Rect::new(
            content.x,
            content.y,
            content.width,
            content.height.saturating_sub(CONTENT_CORNER_ROWS),
        );
        (clip.x >= inside.x
            && clip.y >= inside.y
            && clip.right() <= inside.right()
            && clip.bottom() <= inside.bottom())
        .then_some(slot)
    }

    fn draw_wallpaper(&self, canvas: &mut Canvas<'_>) {
        let clip = canvas.clip();
        let (yolk_x, yolk_y, yolk_radius) = self.yolk;
        for row in clip.y..clip.bottom() {
            canvas.row(row, clip.x, clip.right(), self.gradient[row as usize]);
            let (left, right) = self.egg[row as usize];
            canvas.row(row, left, right, colour::WALLPAPER_EGG);
            let distance = row.abs_diff(yolk_y);
            if distance < yolk_radius {
                let half = isqrt(yolk_radius * yolk_radius - distance * distance);
                let (left, right) = span(yolk_x, half, self.width);
                canvas.row(row, left, right, colour::ACCENT);
            }
        }
    }

    fn draw_window(&self, canvas: &mut Canvas<'_>, slot: usize, contents: &dyn Contents) {
        let window = self.window_at(slot);
        let outside = window.outside();
        let focused = self.focus == Some(slot);
        let (border, title, title_text) = if focused {
            (colour::BORDER_ACTIVE, colour::TITLE_ACTIVE, colour::TEXT)
        } else {
            (
                colour::BORDER_INACTIVE,
                colour::TITLE_INACTIVE,
                colour::TEXT_DIM,
            )
        };
        let (corners, inside_corners): (&[u32], &[u32]) = match window.has_square_corners() {
            true => (&[], &[]),
            false => (&paint::INSET_R6, &paint::INSET_INSIDE_R6),
        };
        canvas.rounded(outside, corners, true, true, border);
        canvas.rounded(
            Rect::new(
                outside.x + 1,
                outside.y + 1,
                outside.width - 2,
                TITLE_HEIGHT,
            ),
            inside_corners,
            true,
            false,
            title,
        );
        self.draw_content(canvas, window, contents);
        let side = self.profile.control_side();
        for &part in window.drawn_controls() {
            let Some(circle) = window.control_circle(part, side) else {
                continue;
            };
            let (active, colour) = match part {
                Part::Close => (window.closable, colour::BUTTON_CLOSE),
                Part::Minimise => (true, colour::BUTTON_MINIMISE),
                _ => (window.can_maximise(), colour::BUTTON_MAXIMISE),
            };
            let fill = match focused && active {
                true => colour,
                false => colour::BUTTON_INACTIVE,
            };
            canvas.rounded(circle, &paint::INSET_R6, true, true, fill);
            if active && self.control_hover == Some((slot, part)) {
                let glyph = match part {
                    Part::Close => &CLOSE_GLYPH,
                    Part::Minimise => &MINIMISE_GLYPH,
                    _ if window.maximised => &RESTORE_GLYPH,
                    _ => &MAXIMISE_GLYPH,
                };
                for (row, bits) in (0..).zip(glyph) {
                    for column in 0..CONTROL_GLYPH_SIZE {
                        if bits & (1 << (CONTROL_GLYPH_SIZE - 1 - column)) != 0 {
                            canvas.pixel(
                                circle.x + CONTROL_GLYPH_OFFSET + column,
                                circle.y + CONTROL_GLYPH_OFFSET + row,
                                colour::GLYPH,
                            );
                        }
                    }
                }
            }
        }
        let suffix = match window.not_responding {
            true => NOT_RESPONDING,
            false => "",
        };
        let room = windows::title_room(outside.width).saturating_sub(suffix.len());
        let (text, cut) = windows::cut_title(window.title(), room);
        let ellipsis = if cut { windows::ellipsis() } else { "" };
        let glyphs = text.chars().count() + ellipsis.len() + suffix.len();
        let width = glyphs as u32 * GLYPH_WIDTH as u32;
        let mut x = outside.x + outside.width.saturating_sub(width) / 2;
        let y = outside.y + 1 + (TITLE_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        for part in [text, ellipsis, suffix] {
            canvas.text(x, y, part, title_text);
            x += part.chars().count() as u32 * GLYPH_WIDTH as u32;
        }
    }

    fn draw_content(&self, canvas: &mut Canvas<'_>, window: &Window, contents: &dyn Contents) {
        let content = window.content();
        let clip = canvas.clip();
        let buffer = Rect::new(
            content.x + window.buffer.x,
            content.y + window.buffer.y,
            window.buffer.width,
            window.buffer.height,
        );
        let fill = match window.ready {
            true => window.fill,
            false => colour::WINDOW_BACKGROUND,
        };
        let top = content.y.max(clip.y);
        let bottom = content.bottom().min(clip.bottom());
        let corners: &[u32] = match window.has_square_corners() {
            true => &[],
            false => &paint::INSET_INSIDE_R6,
        };
        for row in top..bottom {
            let from_bottom = content.bottom() - 1 - row;
            let inset = corners.get(from_bottom as usize).copied().unwrap_or(0);
            let left = (content.x + inset).max(clip.x);
            let right = content.right().saturating_sub(inset).min(clip.right());
            if left >= right {
                continue;
            }
            let in_buffer = window.ready && row >= buffer.y && row < buffer.bottom();
            if !in_buffer {
                canvas.row(row, left, right, fill);
                continue;
            }
            let copy_left = left.max(buffer.x).min(right);
            let copy_right = right.min(buffer.right()).max(copy_left);
            canvas.row(row, left, copy_left, fill);
            canvas.row(row, copy_right, right, fill);
            if let Some(out) = canvas.row_pixels(row, copy_left, copy_right) {
                contents.copy_row(window.id, copy_left - buffer.x, row - buffer.y, out);
            }
        }
    }

    fn draw_menu(&self, canvas: &mut Canvas<'_>) {
        let menu = self.menu();
        canvas.rounded(menu, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(menu.x + 1, menu.y + 1, menu.width - 2, menu.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::MENU,
        );
        for (index, (label, _)) in MENU_ITEMS.iter().enumerate() {
            let top = menu.y + 1 + index as u32 * MENU_ITEM_HEIGHT;
            if self.menu_hover == Some(index) {
                canvas.fill(
                    Rect::new(menu.x + 1, top + 2, menu.width - 2, MENU_ITEM_HEIGHT - 4),
                    colour::MENU_HOVER,
                );
            }
            canvas.text(
                menu.x + MENU_TEXT_X,
                top + (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                label,
                colour::TEXT,
            );
        }
    }
}

/// The modifier bits of a `Key` record ([`protocol::modifier`]) for `modifiers`.
pub fn key_modifiers(modifiers: Modifiers) -> u32 {
    [
        (Modifiers::SHIFT, modifier::SHIFT),
        (Modifiers::CTRL, modifier::CTRL),
        (Modifiers::ALT, modifier::ALT),
        (Modifiers::SUPER, modifier::SUPER),
        (Modifiers::CAPS_LOCK, modifier::CAPS_LOCK),
    ]
    .into_iter()
    .filter(|(held, _)| modifiers.contains(*held))
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// The character key `code` types under the US keymap with `modifiers`: Shift and Caps Lock
/// for letters, Shift for the rest, Ctrl making a letter its control character; Enter, Tab,
/// Backspace and Esc their control characters; 0 for a key that types none.
pub fn codepoint(code: u16, modifiers: Modifiers) -> u32 {
    match code {
        key::ENTER | key::KEYPAD_ENTER => return 0x0d,
        key::TAB => return 0x09,
        key::BACKSPACE => return 0x08,
        key::ESC => return 0x1b,
        _ => {}
    }
    let Some([plain, shifted]) = printable(code) else {
        return 0;
    };
    let shift = modifiers.contains(Modifiers::SHIFT);
    let character = match plain.is_ascii_lowercase() {
        true if shift != modifiers.contains(Modifiers::CAPS_LOCK) => shifted,
        true => plain,
        false if shift => shifted,
        false => plain,
    };
    if modifiers.contains(Modifiers::CTRL) && character.is_ascii_alphabetic() {
        return u32::from(character & 0x1f);
    }
    u32::from(character)
}

fn draw_chip_text(canvas: &mut Canvas<'_>, chip: Rect, title: &str) {
    let (text, cut) = windows::cut_title(title, CHIP_GLYPHS);
    let x = chip.x + CHIP_PADDING;
    let y = chip.y + (chip.height - GLYPH_HEIGHT as u32) / 2;
    canvas.text(x, y, text, colour::TEXT);
    if cut {
        let after = x + text.chars().count() as u32 * GLYPH_WIDTH as u32;
        canvas.text(after, y, windows::ellipsis(), colour::TEXT);
    }
}

fn chip_rect(x: u32, title: &str) -> Rect {
    let (text, cut) = windows::cut_title(title, CHIP_GLYPHS);
    let glyphs = text.chars().count() + if cut { windows::ellipsis().len() } else { 0 };
    Rect::new(
        x,
        CHIP_Y,
        glyphs as u32 * GLYPH_WIDTH as u32 + 2 * CHIP_PADDING,
        CHIP_HEIGHT,
    )
}

fn to_top(stack: &mut [u8], length: usize, slot: usize) {
    if let Some(at) = stack[..length]
        .iter()
        .position(|&in_stack| usize::from(in_stack) == slot)
    {
        stack[at..length].rotate_left(1);
    }
}

fn remove(list: &mut [u8], length: &mut usize, slot: usize) {
    if let Some(at) = list[..*length]
        .iter()
        .position(|&in_list| usize::from(in_list) == slot)
    {
        list[at..*length].rotate_left(1);
        *length -= 1;
    }
}

pub(crate) fn contains(rect: Rect, x: u32, y: u32) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

fn round_percent(percent: u32, of: u32) -> u32 {
    ((u64::from(percent) * u64::from(of) + 50) / 100) as u32
}

fn span(centre: u32, half: u32, width: u32) -> (u32, u32) {
    (
        centre.saturating_sub(half).min(width),
        centre.saturating_add(half).min(width),
    )
}

fn gradient_pixel(row: u32, height: u32) -> u32 {
    let last = height.saturating_sub(1).max(1) as i64;
    let channel = |shift: u32| {
        let top = i64::from((colour::WALLPAPER_TOP >> shift) & 0xff);
        let bottom = i64::from((colour::WALLPAPER_BOTTOM >> shift) & 0xff);
        ((top + (bottom - top) * i64::from(row) / last) as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

struct Writer<'a> {
    bytes: &'a mut [u8],
    length: usize,
}

impl<'a> Writer<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, length: 0 }
    }

    fn push_str(&mut self, text: &str) {
        for &byte in text.as_bytes() {
            if self.length < self.bytes.len() {
                self.bytes[self.length] = byte;
                self.length += 1;
            }
        }
    }

    fn push_number(&mut self, value: u64, minimum_digits: usize) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut rest = value;
        while count < minimum_digits || rest > 0 {
            digits[count] = b'0' + (rest % 10) as u8;
            rest /= 10;
            count += 1;
        }
        for &digit in digits[..count].iter().rev() {
            if self.length < self.bytes.len() {
                self.bytes[self.length] = digit;
                self.length += 1;
            }
        }
    }
}
