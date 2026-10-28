//! The Files window (docs/ux/desktop-v0.3-gui.md sections 8 and 8.1, D135): the toolbar with
//! back, up and the path field, the sidebar's places, the list of the folder's entries, the
//! status bar, and the context menus, dialogs, notices and rename field drawn inside it. It is
//! pure state over [`Files`]: it reads and changes the file systems through the trait and asks
//! the program for what only the system does ([`Request`]), so host tests and the QEMU test's
//! expected frames run the same code the program shows.

mod paint;
#[cfg(test)]
mod tests;

use crate::job::{Job, STEP_BYTES, Step, Work};
use crate::kind::{self, Kind};
use crate::listing::{Listing, items_text, size_text};
use crate::names::{
    self, Name, copy_name, extension_start, free_path, is_valid, is_within, joined,
    new_folder_name, parent,
};
use crate::trash::{RestoreError, Trash, make_folders};
use core::fmt::Write;
use eggos_app::architecture::Architecture;
use eggos_app::files::{self, Files, Path};
use eggos_app::installer::{self, APPLICATIONS};
use eggos_app::messages::Line;
use eggos_app_manifest as manifest;
use eggos_runtime::errno::Errno;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers, MouseButton, key};
use eggos_toolkit::menu::{Entry, Menu, MenuKey};
use eggos_toolkit::panel::{Panel, PanelResponse};
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::widget::{
    self, Escape, Field, FieldKey, MIN_THUMB, SCROLL_BAR_WIDTH, scroll_bar,
};

/// The window's title, which the shell matches to the Files app.
pub const TITLE: &str = "Files";
/// The content's size when it opens.
pub const DEFAULT_SIZE: (u32, u32) = (720, 480);
/// The smallest content.
pub const MINIMUM_SIZE: (u32, u32) = (480, 300);
pub use crate::trash::HOME;
/// Where the home folders' file system is mounted: what is in it can be trashed.
pub const HOMES: &[u8] = b"/home";
/// Where the host's shared folder is mounted (D130).
pub const HOST_SHARE: &[u8] = b"/media/host";
/// The folder the host share is mounted in.
pub const MEDIA: &[u8] = b"/media";
/// The memory file system's folder, which has no Trash.
pub const TMP: &[u8] = b"/tmp";
/// How long a notice shows.
pub const NOTICE_MS: u32 = 3000;
/// The largest text file opened in the Terminal.
pub const LARGEST_SHOWN_TEXT: u64 = 256 * 1024;
/// How many steps of a long job run per tick.
pub const STEPS_PER_TICK: usize = 16;
/// The longest line Files types in the Terminal.
pub const LINE_CAPACITY: usize = 256;

pub(crate) const TOOLBAR_HEIGHT: i32 = 32;
const PATH_FIELD_CAPACITY: usize = 255;
pub(crate) const SIDEBAR_WIDTH: i32 = 160;
pub(crate) const ROW_HEIGHT: i32 = 24;
pub(crate) const HEADER_HEIGHT: i32 = 24;
pub(crate) const STATUS_HEIGHT: i32 = 24;
pub(crate) const PLACES_TOP: i32 = 60;
/// The toolbar's back button.
pub const BACK_BUTTON: Rect = Rect::new(4, 2, 28, 28);
/// The toolbar's up button.
pub const UP_BUTTON: Rect = Rect::new(36, 2, 28, 28);
pub(crate) const PATH_FIELD_X: i32 = 72;
pub(crate) const NAME_X: i32 = 28;
pub(crate) const RENAME_X: i32 = 20;
pub(crate) const SIZE_END_FROM_RIGHT: i32 = 144;
pub(crate) const TIME_WIDTH: i32 = 128;
pub(crate) const COLUMN_GAP: i32 = 8;
pub(crate) const SIZE_WIDTH: i32 = 80;
const DOUBLE_CLICK_MS: u32 = 400;
const CARET_BLINK_MS: u32 = 500;
const TYPE_AHEAD_MS: u32 = 1000;
const HISTORY: usize = 16;
const WHEEL_ROWS: i32 = 3;
const MAX_REQUESTS: usize = 4;
const MAX_PLACES: usize = 9;
const PROPERTIES_LINES: usize = 5;

mod code {
    pub const N: u16 = 49;
    pub const C: u16 = 46;
    pub const X: u16 = 45;
    pub const V: u16 = 47;
    pub const L: u16 = 38;
    pub const R: u16 = 19;
    pub const I: u16 = 23;
    pub const O: u16 = 24;
    pub const F2: u16 = 60;
    pub const F5: u16 = 63;
    pub const F10: u16 = 68;
    pub const MENU: u16 = 127;
}

mod item {
    pub const OPEN: u8 = 1;
    pub const INSTALL: u8 = 2;
    pub const UNINSTALL: u8 = 3;
    pub const CUT: u8 = 4;
    pub const COPY: u8 = 5;
    pub const PASTE: u8 = 6;
    pub const RENAME: u8 = 7;
    pub const DELETE: u8 = 8;
    pub const DELETE_PERMANENTLY: u8 = 9;
    pub const PROPERTIES: u8 = 10;
    pub const NEW_FOLDER: u8 = 11;
    pub const RELOAD: u8 = 12;
    pub const EMPTY_TRASH: u8 = 13;
    pub const RESTORE: u8 = 14;
}

/// A place in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Home,
    Documents,
    Downloads,
    Desktop,
    Tmp,
    HostShare,
    Apps,
    System,
    Trash,
}

impl Place {
    /// The place's label; `windows` for the Windows profile's words.
    pub fn label(self, windows: bool) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Tmp => "tmp",
            Self::HostShare => "Host share",
            Self::Apps => "Apps",
            Self::System => "System",
            Self::Trash if windows => "Recycle Bin",
            Self::Trash => "Trash",
        }
    }

    fn location(self) -> Location {
        let folder = |path: &[u8]| Location::Folder(Path::from(path).unwrap_or_default());
        let in_home = |name: &[u8]| {
            Location::Folder(
                Path::from(HOME)
                    .and_then(|home| home.join(name))
                    .unwrap_or_default(),
            )
        };
        match self {
            Self::Home => folder(HOME),
            Self::Documents => in_home(b"Documents"),
            Self::Downloads => in_home(b"Downloads"),
            Self::Desktop => in_home(b"Desktop"),
            Self::Tmp => folder(TMP),
            Self::HostShare => folder(HOST_SHARE),
            Self::Apps => folder(APPLICATIONS),
            Self::System => folder(b"/"),
            Self::Trash => Location::Trash,
        }
    }
}

/// Where the window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Location {
    Folder(Path),
    Trash,
}

/// What the window asks of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Request {
    /// Type this line in the Terminal window, after erasing a half-typed one, press Enter and
    /// focus the Terminal (`/dev/window`'s `TYPE_IN_TERMINAL`).
    TypeInTerminal(TerminalLine),
    /// Start the installer's window on this program or `.app` (D134).
    Install(Path),
    /// Start the installer's window asking to uninstall this app.
    Uninstall(Name),
    /// Open this installed app as its icon does (`OPEN_APP`).
    OpenApp(Name),
}

/// A line to type in the Terminal, at most [`LINE_CAPACITY`] bytes of printable text.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TerminalLine {
    bytes: [u8; LINE_CAPACITY],
    length: usize,
}

impl core::fmt::Debug for TerminalLine {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl TerminalLine {
    fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> bool {
        let end = self.length + bytes.len();
        if end > LINE_CAPACITY || bytes.iter().any(|&byte| byte < 0x20 || byte == 0x7f) {
            return false;
        }
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
        true
    }

    /// The line `prefix` then `path` quoted for the shell: `'` around it, each `'` in it
    /// written `'\''`; `None` when it does not fit or holds a control character.
    pub fn quoted(prefix: &[u8], path: &[u8]) -> Option<Self> {
        let mut line = Self::new();
        let mut fits = line.push(prefix) && line.push(b"'");
        for piece in path.split(|&byte| byte == b'\'').enumerate() {
            if piece.0 > 0 {
                fits &= line.push(b"'\\''");
            }
            fits &= line.push(piece.1);
        }
        (fits && line.push(b"'")).then_some(line)
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Path,
    Sidebar,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hover {
    Back,
    Up,
    Place(usize),
    Thumb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pressed {
    Back,
    Up,
    Thumb { grab_y: i32, start_offset: i32 },
}

#[derive(Debug, Clone, Copy)]
struct Rename {
    index: usize,
    field: Field,
    original: Name,
    shown: Text<255>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Clip {
    path: Path,
    cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
enum Asked {
    DeletePermanently(Path),
    EmptyTrash,
    Properties,
    Working,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
enum After {
    Paste { folder: Path, name: Name, cut: bool },
    Delete { trashed: Option<Name> },
    EmptyTrash,
}

#[derive(Debug)]
struct Running {
    job: Job,
    name: Name,
    after: After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuOn {
    Row(usize),
    Space,
}

#[derive(Debug, Clone, Copy)]
struct Notice {
    text: Line,
    shown_at: u32,
}

/// The Files window. See the module documentation.
#[derive(Debug)]
pub struct FilesWindow {
    width: i32,
    height: i32,
    windows_words: bool,
    machine: Architecture,
    trash: Trash,
    location: Location,
    history: [Location; HISTORY],
    history_length: usize,
    listing: Listing,
    selected: Option<usize>,
    offset: i32,
    focus: Focus,
    place_ring: usize,
    places: [Place; MAX_PLACES],
    place_count: usize,
    path_field: Field,
    path_before: Text<255>,
    rename: Option<Rename>,
    menu: Option<(Menu, MenuOn)>,
    panel: Option<(Panel, Asked)>,
    notice: Option<Notice>,
    clipboard: Option<Clip>,
    running: Option<Running>,
    hover: Option<Hover>,
    pointer: Option<(i32, i32)>,
    pressed: Option<Pressed>,
    last_click: Option<(usize, u32)>,
    typed: Text<32>,
    typed_at: u32,
    window_focused: bool,
    caret_epoch: u32,
    caret_shown: bool,
    now_ms: u32,
    clock: u64,
    requests: [Option<Request>; MAX_REQUESTS],
    damage: Damage,
    buffer: [u8; STEP_BYTES],
}

fn text_of(bytes: &[u8]) -> Text<255> {
    let mut out = [0u8; names::NAME_CAPACITY * 2];
    let mut text = Text::new();
    text.set(names::display(bytes, &mut out));
    text
}

fn errno_text(error: Errno) -> &'static str {
    core::str::from_utf8(error.message()).unwrap_or("error")
}

impl FilesWindow {
    /// The window at `start` (home when `None`) for the `machine`, in the Windows profile's
    /// words when `windows`; the home folder is made if it is missing.
    pub fn new(
        files: &mut impl Files,
        start: Option<&[u8]>,
        machine: Architecture,
        windows: bool,
        clock: u64,
    ) -> Self {
        let _ = make_folders(files, HOME);
        let home = Location::Folder(Path::from(HOME).unwrap_or_default());
        let location = start.and_then(Path::from).map_or(home, Location::Folder);
        let mut window = Self {
            width: DEFAULT_SIZE.0 as i32,
            height: DEFAULT_SIZE.1 as i32,
            windows_words: windows,
            machine,
            trash: Trash::of_home(HOME).expect("the home folder's Trash path fits"),
            location,
            history: [home; HISTORY],
            history_length: 0,
            listing: Listing::new(),
            selected: None,
            offset: 0,
            focus: Focus::List,
            place_ring: 0,
            places: [Place::Home; MAX_PLACES],
            place_count: 0,
            path_field: Field {
                text: Text::new(),
                caret: 0,
                scroll: 0,
                placeholder: "",
                escape: Escape::Revert,
            },
            path_before: Text::new(),
            rename: None,
            menu: None,
            panel: None,
            notice: None,
            clipboard: None,
            running: None,
            hover: None,
            pointer: None,
            pressed: None,
            last_click: None,
            typed: Text::new(),
            typed_at: 0,
            window_focused: true,
            caret_epoch: 0,
            caret_shown: true,
            now_ms: 0,
            clock,
            requests: [None; MAX_REQUESTS],
            damage: Damage::new(),
            buffer: [0; STEP_BYTES],
        };
        window.reload(files);
        window.damage_all();
        window
    }

    /// The content is now `width` by `height`.
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.clamp_offset();
        self.damage_all();
    }

    /// What changed since the last call.
    pub fn take_damage(&mut self) -> Damage {
        self.damage.take()
    }

    /// The next request for the system, oldest first.
    pub fn take_request(&mut self) -> Option<Request> {
        let request = self.requests[0].take();
        self.requests.rotate_left(1);
        request
    }

    fn ask(&mut self, request: Request) {
        if let Some(slot) = self.requests.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(request);
        }
    }

    /// Where it is.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// Its listing.
    pub fn listing(&self) -> &Listing {
        &self.listing
    }

    /// The selected entry, by index in the listing.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The text of the notice shown, if one is.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    /// The lines of the dialog shown, if one is.
    pub fn dialog_line(&self, index: usize) -> Option<&str> {
        self.panel.as_ref().map(|(panel, _)| panel.line(index))
    }

    /// Whether a job is under way.
    pub fn is_working(&self) -> bool {
        self.running.is_some()
    }

    /// The window is closing: a job under way stops as its Cancel would stop it.
    pub fn close(&mut self, files: &mut impl Files) {
        if let Some(mut running) = self.running.take() {
            running.job.cancel(files);
        }
    }

    /// The pointer shape now: the I-beam over a text field.
    pub fn pointer_shape(&self) -> PointerShape {
        let Some((x, y)) = self
            .pointer
            .filter(|_| self.panel.is_none() && self.menu.is_none())
        else {
            return PointerShape::Arrow;
        };
        let over_rename = self
            .rename
            .is_some_and(|rename| self.rename_rect(rename.index).contains(x, y));
        match over_rename || self.path_field_rect().contains(x, y) {
            true => PointerShape::IBeam,
            false => PointerShape::Arrow,
        }
    }

    /// The places the sidebar shows, in order.
    pub fn places(&self) -> &[Place] {
        &self.places[..self.place_count]
    }

    pub(crate) fn content(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub(crate) fn toolbar(&self) -> Rect {
        Rect::new(0, 0, self.width, TOOLBAR_HEIGHT)
    }

    pub(crate) fn path_field_rect(&self) -> Rect {
        Rect::new(
            PATH_FIELD_X,
            2,
            (self.width - 8 - PATH_FIELD_X).max(0),
            widget::CONTROL_HEIGHT,
        )
    }

    pub(crate) fn sidebar(&self) -> Rect {
        Rect::new(0, TOOLBAR_HEIGHT, SIDEBAR_WIDTH, self.middle_height())
    }

    fn middle_height(&self) -> i32 {
        (self.height - TOOLBAR_HEIGHT - STATUS_HEIGHT).max(0)
    }

    pub(crate) fn list(&self) -> Rect {
        Rect::new(
            SIDEBAR_WIDTH,
            TOOLBAR_HEIGHT,
            (self.width - SIDEBAR_WIDTH).max(0),
            self.middle_height(),
        )
    }

    /// Where the list's rows scroll.
    pub fn rows(&self) -> Rect {
        let list = self.list();
        Rect::new(
            list.x,
            list.y + HEADER_HEIGHT,
            list.width,
            (list.height - HEADER_HEIGHT).max(0),
        )
    }

    pub(crate) fn status_bar(&self) -> Rect {
        Rect::new(0, self.height - STATUS_HEIGHT, self.width, STATUS_HEIGHT)
    }

    pub(crate) fn middle(&self) -> Rect {
        Rect::new(0, TOOLBAR_HEIGHT, self.width, self.middle_height())
    }

    fn content_height(&self) -> i32 {
        self.listing.len() as i32 * ROW_HEIGHT
    }

    pub(crate) fn scroll_bar(&self) -> Option<(Rect, Rect)> {
        scroll_bar(self.rows(), self.content_height(), self.offset)
    }

    /// The right edge the columns end at: the list's, less the scroll bar when it shows.
    pub(crate) fn columns_right(&self) -> i32 {
        let bar = match self.scroll_bar() {
            Some(_) => SCROLL_BAR_WIDTH,
            None => 0,
        };
        self.list().right() - bar
    }

    /// Row `index`'s rectangle (it may be scrolled out of the rows' rectangle).
    pub fn row_rect(&self, index: usize) -> Rect {
        let rows = self.rows();
        Rect::new(
            rows.x,
            rows.y + index as i32 * ROW_HEIGHT - self.offset,
            self.columns_right() - rows.x,
            ROW_HEIGHT,
        )
    }

    /// Where the rows' times are: the column a test that cannot know the guest's clock skips.
    pub fn time_column(&self) -> Rect {
        let rows = self.rows();
        Rect::new(
            self.columns_right() - COLUMN_GAP - TIME_WIDTH,
            rows.y,
            TIME_WIDTH,
            rows.height,
        )
    }

    /// Place row `index`'s rectangle in the sidebar.
    pub fn place_rect(&self, index: usize) -> Rect {
        Rect::new(
            0,
            PLACES_TOP + index as i32 * ROW_HEIGHT,
            SIDEBAR_WIDTH - 1,
            ROW_HEIGHT,
        )
    }

    /// The rectangle of the context menu shown, if one is.
    pub fn menu_rect(&self) -> Option<Rect> {
        self.menu.as_ref().map(|(menu, _)| menu.rect())
    }

    /// The rectangle of item `index` of the context menu shown.
    pub fn menu_entry_rect(&self, index: usize) -> Option<Rect> {
        self.menu.as_ref().map(|(menu, _)| menu.entry_rect(index))
    }

    /// The rectangle of button `index` of the dialog shown.
    pub fn dialog_button_rect(&self, index: usize) -> Option<Rect> {
        let area = self.middle();
        self.panel
            .as_ref()
            .map(|(panel, _)| panel.button_rect(area, index))
    }

    fn damage_all(&mut self) {
        self.damage.add(self.content());
    }

    fn damage_middle(&mut self) {
        self.damage.add(self.middle());
        self.damage.add(self.status_bar());
    }

    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let rows = self.rows();
        if !rows.contains(x, y) || x >= self.columns_right() {
            return None;
        }
        let index = ((y - rows.y + self.offset) / ROW_HEIGHT) as usize;
        (index < self.listing.len()).then_some(index)
    }

    fn place_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.place_count).find(|&index| self.place_rect(index).contains(x, y))
    }

    fn folder(&self) -> Option<&Path> {
        match &self.location {
            Location::Folder(path) => Some(path),
            Location::Trash => None,
        }
    }

    fn is_trash(&self) -> bool {
        self.location == Location::Trash
    }

    fn on_share(&self) -> bool {
        self.folder()
            .is_some_and(|folder| is_within(folder.as_bytes(), HOST_SHARE))
    }

    fn trashable(&self) -> bool {
        self.folder()
            .is_some_and(|folder| is_within(folder.as_bytes(), HOMES))
    }

    fn entry_path(&self, index: usize) -> Option<Path> {
        let name = self.listing.name(index);
        match &self.location {
            Location::Folder(folder) => joined(folder, name),
            Location::Trash => self.trash.item(name),
        }
    }

    fn share_mounted(files: &mut impl Files) -> bool {
        match (files.status(HOST_SHARE), files.status(MEDIA)) {
            (Ok(share), Ok(media)) => share.device != media.device,
            _ => false,
        }
    }

    fn set_places(&mut self, files: &mut impl Files) {
        let mounted = Self::share_mounted(files);
        let all = [
            Place::Home,
            Place::Documents,
            Place::Downloads,
            Place::Desktop,
            Place::Tmp,
            Place::HostShare,
            Place::Apps,
            Place::System,
            Place::Trash,
        ];
        self.place_count = 0;
        for place in all {
            if place == Place::HostShare && !mounted {
                continue;
            }
            self.places[self.place_count] = place;
            self.place_count += 1;
        }
        self.place_ring = self.place_ring.min(self.place_count.saturating_sub(1));
    }

    /// The place whose folder is where the window is, by index in the sidebar.
    pub fn current_place(&self) -> Option<usize> {
        self.places()
            .iter()
            .position(|place| place.location() == self.location)
    }

    fn set_path_text(&mut self) {
        let text = match &self.location {
            Location::Folder(path) => text_of(path.as_bytes()),
            Location::Trash => Text::from(Place::Trash.label(self.windows_words)),
        };
        self.path_field.text = text;
        self.path_field.caret = text.len();
        self.path_field.scroll = 0;
        let rect = self.path_field_rect();
        widget::keep_caret_visible(&mut self.path_field, rect);
    }

    /// Reads the folder again, keeping the selection on the same name.
    pub fn reload(&mut self, files: &mut impl Files) {
        let keep = self
            .selected
            .and_then(|index| Name::from(self.listing.name(index)));
        self.set_places(files);
        match self.location {
            Location::Folder(folder) => self.listing.read_folder(files, &folder),
            Location::Trash => self.listing.read_trash(files, &self.trash),
        }
        self.selected = keep.and_then(|name| self.listing.find(name.as_bytes()));
        if self
            .rename
            .is_some_and(|rename| self.listing.name(rename.index) != rename.original.as_bytes())
        {
            self.rename = None;
        }
        if self.focus != Focus::Path {
            self.set_path_text();
        }
        self.clamp_offset();
        self.damage_all();
    }

    fn clamp_offset(&mut self) {
        let most = (self.content_height() - self.rows().height).max(0);
        self.offset = self.offset.clamp(0, most);
    }

    fn keep_visible(&mut self, index: usize) {
        let rows = self.rows();
        let top = index as i32 * ROW_HEIGHT;
        if top < self.offset {
            self.offset = top;
        } else if top + ROW_HEIGHT > self.offset + rows.height {
            self.offset = top + ROW_HEIGHT - rows.height;
        }
        self.clamp_offset();
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|&index| index < self.listing.len());
        if let Some(index) = self.selected {
            self.keep_visible(index);
        }
        self.damage_middle();
    }

    fn go(&mut self, files: &mut impl Files, location: Location) {
        if let Location::Folder(folder) = &location {
            match files.status(folder.as_bytes()) {
                Ok(status) if status.kind == files::Kind::Directory => {}
                Ok(_) => {
                    self.show_notice(format_args!(
                        "Could not open {} (Not a directory).",
                        Shown(folder.as_bytes())
                    ));
                    return;
                }
                Err(error) => {
                    self.show_notice(format_args!(
                        "Could not open {} ({}).",
                        Shown(folder.as_bytes()),
                        errno_text(error)
                    ));
                    return;
                }
            }
        }
        if location != self.location {
            if self.history_length == HISTORY {
                self.history.rotate_left(1);
                self.history_length -= 1;
            }
            self.history[self.history_length] = self.location;
            self.history_length += 1;
        }
        self.enter(files, location, None);
    }

    fn enter(&mut self, files: &mut impl Files, location: Location, select: Option<&[u8]>) {
        self.location = location;
        self.rename = None;
        self.menu = None;
        self.selected = None;
        self.offset = 0;
        self.last_click = None;
        self.reload(files);
        self.set_path_text();
        self.path_before = self.path_field.text;
        if let Some(name) = select {
            let found = self.listing.find(name);
            self.select(found);
        }
    }

    fn back(&mut self, files: &mut impl Files) {
        if self.history_length == 0 {
            return;
        }
        self.history_length -= 1;
        let location = self.history[self.history_length];
        let came_from = self.folder().copied();
        let select = came_from.filter(|from| match &location {
            Location::Folder(folder) => parent(from.as_bytes()) == folder.as_bytes(),
            Location::Trash => false,
        });
        self.enter(
            files,
            location,
            select.as_ref().map(|from| from.file_name()),
        );
    }

    fn can_go_up(&self) -> bool {
        self.folder()
            .is_some_and(|folder| folder.as_bytes() != b"/")
    }

    fn up(&mut self, files: &mut impl Files) {
        let Some(folder) = self.folder().copied().filter(|_| self.can_go_up()) else {
            return;
        };
        let Some(above) = Path::from(parent(folder.as_bytes())) else {
            return;
        };
        self.go(files, Location::Folder(above));
        let found = self.listing.find(folder.file_name());
        self.select(found);
    }

    fn go_to_place(&mut self, files: &mut impl Files, index: usize) {
        let Some(&place) = self.places().get(index) else {
            return;
        };
        let location = place.location();
        if matches!(place, Place::Documents | Place::Downloads | Place::Desktop)
            && let Location::Folder(folder) = &location
        {
            let _ = make_folders(files, folder.as_bytes());
        }
        self.place_ring = index;
        self.go(files, location);
    }

    fn show_notice(&mut self, text: core::fmt::Arguments<'_>) {
        let mut line = Line::new();
        let _ = line.write_fmt(text);
        self.notice = Some(Notice {
            text: line,
            shown_at: self.now_ms,
        });
        self.damage.add(self.list());
    }

    fn close_notice(&mut self) {
        if self.notice.take().is_some() {
            self.damage.add(self.list());
        }
    }

    fn restart_caret(&mut self) {
        self.caret_epoch = self.now_ms;
        self.caret_shown = true;
    }

    fn editing(&self) -> Option<Rect> {
        if !self.window_focused {
            return None;
        }
        match (self.rename, self.focus) {
            (Some(rename), _) => Some(self.rename_rect(rename.index)),
            (None, Focus::Path) => Some(self.path_field_rect()),
            _ => None,
        }
    }

    pub(crate) fn rename_rect(&self, index: usize) -> Rect {
        let row = self.row_rect(index);
        Rect::new(
            row.x + RENAME_X,
            row.y,
            (self.columns_right() - 8 - (row.x + RENAME_X)).max(0),
            ROW_HEIGHT,
        )
    }

    /// When the window next wants [`Self::tick`]: the caret's blink, a notice's end, or at
    /// once while a job runs.
    pub fn deadline(&self, now_ms: u32) -> Option<u32> {
        if self.running.is_some() {
            return Some(now_ms);
        }
        let blink = self.editing().map(|_| {
            let since = now_ms.wrapping_sub(self.caret_epoch);
            now_ms.wrapping_add(CARET_BLINK_MS - since % CARET_BLINK_MS)
        });
        let notice = self
            .notice
            .as_ref()
            .map(|notice| notice.shown_at.wrapping_add(NOTICE_MS));
        match (blink, notice) {
            (Some(a), Some(b)) => Some(if a.wrapping_sub(now_ms) <= b.wrapping_sub(now_ms) {
                a
            } else {
                b
            }),
            (deadline, None) | (None, deadline) => deadline,
        }
    }

    /// The time is `now_ms`: the caret blinks, a notice ends, a job goes on.
    pub fn tick(&mut self, files: &mut impl Files, now_ms: u32, clock: u64) {
        self.now_ms = now_ms;
        self.clock = clock;
        if let Some(rect) = self.editing() {
            let shown = (now_ms.wrapping_sub(self.caret_epoch) / CARET_BLINK_MS).is_multiple_of(2);
            if shown != self.caret_shown {
                self.caret_shown = shown;
                self.damage.add(rect);
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now_ms.wrapping_sub(notice.shown_at) >= NOTICE_MS)
        {
            self.close_notice();
        }
        if self.running.is_some() {
            self.work(files);
        }
    }

    /// Hands the window `input`, which happened at `time_ms` when the clock of the date said
    /// `clock` seconds; `false` once the window should close.
    pub fn handle(
        &mut self,
        files: &mut impl Files,
        input: Input,
        time_ms: u32,
        clock: u64,
    ) -> bool {
        self.now_ms = time_ms;
        self.clock = clock;
        let closes_notice = matches!(
            input,
            Input::Key { pressed: true, .. } | Input::Button { pressed: true, .. }
        );
        if closes_notice {
            self.close_notice();
        }
        match input {
            Input::FocusIn | Input::FocusOut => {
                self.window_focused = input == Input::FocusIn;
                if !self.window_focused {
                    self.menu = None;
                    self.pressed = None;
                } else if self.running.is_none() && self.panel.is_none() {
                    self.reload(files);
                }
                self.restart_caret();
                self.damage_all();
                return true;
            }
            Input::PointerLeave => {
                self.pointer = None;
                if self.hover.take().is_some() {
                    self.damage_all();
                }
                if let Some((menu, _)) = self.menu.as_mut()
                    && menu.leave()
                {
                    self.damage.add(menu.rect());
                }
                if let Some((panel, _)) = self.panel.as_mut()
                    && panel.pointer_leave()
                {
                    self.damage_middle();
                }
                return true;
            }
            _ => {}
        }
        if let Input::PointerMove { x, y } = input {
            self.pointer = Some((x, y));
        }
        if self.panel.is_some() {
            self.panel_input(files, input);
            return true;
        }
        if self.menu.is_some() && self.menu_input(files, input) {
            return true;
        }
        if self.rename.is_some() && self.rename_input(files, input) {
            return true;
        }
        match input {
            Input::Key {
                code,
                pressed: true,
                modifiers,
                character,
            } => self.key(files, code, modifiers, character),
            Input::PointerMove { x, y } => self.pointer_moved(x, y),
            Input::Button {
                x,
                y,
                button,
                pressed,
            } => self.button(files, x, y, button, pressed),
            Input::Wheel { x, y, notches } if self.list().contains(x, y) => {
                self.offset -= notches * WHEEL_ROWS * ROW_HEIGHT;
                self.clamp_offset();
                self.damage_middle();
            }
            _ => {}
        }
        true
    }

    fn panel_input(&mut self, files: &mut impl Files, input: Input) {
        let area = self.middle();
        let Some((panel, _)) = self.panel.as_mut() else {
            return;
        };
        let response = match input {
            Input::Key {
                code,
                pressed,
                modifiers,
                ..
            } => panel.key(code, modifiers.shift(), pressed),
            Input::PointerMove { x, y } => {
                panel.pointer_move(area, x, y);
                PanelResponse::Nothing
            }
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => panel.button(area, x, y, pressed),
            _ => PanelResponse::Nothing,
        };
        self.damage_middle();
        if let PanelResponse::Pressed(button) = response {
            self.answered(files, button);
        }
    }

    fn answered(&mut self, files: &mut impl Files, button: usize) {
        let Some((_, asked)) = self.panel.take() else {
            return;
        };
        self.damage_middle();
        match asked {
            Asked::Working => {
                if let Some(mut running) = self.running.take() {
                    running.job.cancel(files);
                    let verb = match running.job.work() {
                        Work::Copy => "copying",
                        Work::Move => "moving",
                        Work::Delete => "deleting",
                    };
                    self.show_notice(format_args!(
                        "Stopped {verb} {}.",
                        Shown(running.name.as_bytes())
                    ));
                    self.reload(files);
                }
            }
            Asked::DeletePermanently(path) if button == 1 => {
                let trashed = match self.is_trash() {
                    true => Name::from(path.file_name()),
                    false => None,
                };
                self.start(files, Job::delete(&path), After::Delete { trashed });
            }
            Asked::EmptyTrash if button == 1 => {
                if let Some(items) = self.trash.items() {
                    match files.status(items.as_bytes()) {
                        Ok(_) => self.start(files, Job::delete(&items), After::EmptyTrash),
                        Err(_) => {
                            let _ = self.trash.empty(files);
                            self.reload(files);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn menu_input(&mut self, files: &mut impl Files, input: Input) -> bool {
        let Some((menu, on)) = self.menu.as_mut() else {
            return false;
        };
        let on = *on;
        let rect = menu.rect();
        match input {
            Input::Key {
                code,
                pressed: true,
                ..
            } => {
                let chosen = menu.key(code);
                self.damage.add(rect);
                match chosen {
                    MenuKey::Chosen(id) => {
                        self.menu = None;
                        self.choose(files, id, on);
                    }
                    MenuKey::Closed => self.menu = None,
                    MenuKey::Nothing => {}
                }
                true
            }
            Input::Key { .. } => true,
            Input::PointerMove { x, y } => {
                if menu.hover(x, y) {
                    self.damage.add(rect);
                }
                rect.contains(x, y)
            }
            Input::Button { x, y, pressed, .. } if rect.contains(x, y) => {
                if pressed && let Some(id) = menu.id_at(x, y) {
                    self.menu = None;
                    self.damage.add(rect);
                    self.choose(files, id, on);
                }
                true
            }
            Input::Button { pressed: true, .. } => {
                self.menu = None;
                self.damage.add(rect);
                false
            }
            _ => true,
        }
    }

    fn rename_input(&mut self, files: &mut impl Files, input: Input) -> bool {
        let Some(mut rename) = self.rename else {
            return false;
        };
        let rect = self.rename_rect(rename.index);
        match input {
            Input::Key {
                code,
                pressed: true,
                modifiers,
                character,
            } => {
                match code {
                    key::ENTER | key::TAB => self.commit_rename(files),
                    key::ESC => {
                        self.rename = None;
                        self.damage_middle();
                    }
                    _ => {
                        let before = Text::new();
                        let changed = widget::field_key(
                            &mut rename.field,
                            code,
                            character,
                            modifiers.command(),
                            &before,
                        );
                        if changed != FieldKey::Unused {
                            widget::keep_caret_visible(&mut rename.field, rect);
                            self.rename = Some(rename);
                            self.restart_caret();
                            self.damage.add(rect);
                        }
                    }
                }
                true
            }
            Input::Key { .. } => true,
            Input::Button {
                x,
                y,
                pressed: true,
                ..
            } if rect.contains(x, y) => {
                rename.field.caret = widget::caret_at(&rename.field, rect, x);
                self.rename = Some(rename);
                self.restart_caret();
                self.damage.add(rect);
                true
            }
            Input::Button { pressed: true, .. } => {
                self.commit_rename(files);
                false
            }
            _ => false,
        }
    }

    fn begin_rename(&mut self, index: usize) {
        if self.is_trash() || index >= self.listing.len() {
            return;
        }
        if self.on_share() {
            self.show_notice(format_args!("The host share is read-only."));
            return;
        }
        if let Some(Kind::App(_)) = self.listing.get(index).map(|entry| entry.kind) {
            self.show_notice(format_args!(
                "An installed app keeps its name: use Uninstall... to remove it."
            ));
            return;
        }
        let name = self.listing.name(index);
        let Some(original) = Name::from(name) else {
            return;
        };
        let text = text_of(name);
        let folder = self
            .listing
            .get(index)
            .is_some_and(|entry| entry.kind.is_folder());
        let mut caret = extension_start(text.as_str().as_bytes(), folder);
        while !text.as_str().is_char_boundary(caret) {
            caret -= 1;
        }
        let mut field = Field {
            text,
            caret,
            scroll: 0,
            placeholder: "",
            escape: Escape::Revert,
        };
        self.select(Some(index));
        widget::keep_caret_visible(&mut field, self.rename_rect(index));
        self.rename = Some(Rename {
            index,
            field,
            original,
            shown: text,
        });
        self.focus = Focus::List;
        self.restart_caret();
        self.damage_middle();
    }

    fn commit_rename(&mut self, files: &mut impl Files) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        self.damage_middle();
        let wanted = rename.field.text.as_str().as_bytes();
        if wanted.is_empty()
            || wanted == rename.original.as_bytes()
            || rename.field.text == rename.shown
        {
            return;
        }
        let keep_open = |window: &mut Self| window.rename = Some(rename);
        if wanted.contains(&b'/') {
            keep_open(self);
            self.show_notice(format_args!("A name cannot contain \"/\"."));
            return;
        }
        let Some(folder) = self.folder().copied() else {
            return;
        };
        let (Some(from), Some(to)) = (
            joined(&folder, rename.original.as_bytes()),
            joined(&folder, wanted),
        ) else {
            return;
        };
        if !is_valid(wanted) {
            keep_open(self);
            self.show_notice(format_args!(
                "Could not rename {} ({}).",
                Shown(rename.original.as_bytes()),
                errno_text(Errno::EINVAL)
            ));
            return;
        }
        if files.status(to.as_bytes()).is_ok() {
            keep_open(self);
            self.show_notice(format_args!("{} already exists here.", Shown(wanted)));
            return;
        }
        match files.rename(from.as_bytes(), to.as_bytes()) {
            Ok(()) => {
                self.reload(files);
                let found = self.listing.find(wanted);
                self.select(found);
            }
            Err(error) => {
                keep_open(self);
                self.show_notice(format_args!(
                    "Could not rename {} ({}).",
                    Shown(rename.original.as_bytes()),
                    errno_text(error)
                ));
            }
        }
    }

    fn new_folder(&mut self, files: &mut impl Files) {
        let Some(folder) = self.folder().copied() else {
            return;
        };
        if self.on_share() {
            self.show_notice(format_args!("The host share is read-only."));
            return;
        }
        let path = free_path(files, folder.as_bytes(), |number| {
            Some(new_folder_name(number))
        });
        let made = path.and_then(|path| files.make_directory(path.as_bytes()).map(|()| path));
        match made {
            Ok(path) => {
                self.reload(files);
                if let Some(index) = self.listing.find(path.file_name()) {
                    self.begin_rename(index);
                }
            }
            Err(error) => self.show_notice(format_args!(
                "Could not create {} ({}).",
                Shown(names::NEW_FOLDER),
                errno_text(error)
            )),
        }
    }

    fn key(&mut self, files: &mut impl Files, code: u16, modifiers: Modifiers, character: u32) {
        let held = modifiers.0;
        let primary = held & (Modifiers::CTRL | Modifiers::SUPER) != 0;
        let alt = held & Modifiers::ALT != 0;
        let shift = modifiers.shift();
        match (code, primary, alt) {
            (key::TAB, false, false) => {
                self.move_focus(shift);
                return;
            }
            (code::L, true, _) => {
                self.focus_path();
                return;
            }
            (code::F5, ..) | (code::R, true, false) => {
                self.reload(files);
                return;
            }
            (key::LEFT, false, true) => {
                self.back(files);
                return;
            }
            (key::UP, _, true) | (key::UP, true, _) => {
                self.up(files);
                return;
            }
            (code::C, true, false) => {
                self.clip(false);
                return;
            }
            (code::X, true, false) => {
                self.clip(true);
                return;
            }
            (code::V, true, false) => {
                let folder = self.folder().copied();
                if let Some(folder) = folder {
                    self.paste(files, folder);
                }
                return;
            }
            (code::N, true, false) => {
                self.new_folder(files);
                return;
            }
            (key::ENTER, false, true) | (code::I, true, false) => {
                self.properties(files);
                return;
            }
            (code::F10, false, false) if shift => {
                self.open_menu_at_selection();
                return;
            }
            (code::MENU, ..) => {
                self.open_menu_at_selection();
                return;
            }
            (key::BACKSPACE, true, alt) if self.focus == Focus::List => {
                self.delete_selected(files, alt);
                return;
            }
            (code::O, true, false) | (key::DOWN, true, false) if self.focus == Focus::List => {
                if let Some(index) = self.selected {
                    self.open(files, index);
                }
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Path => self.path_key(files, code, character, primary || alt),
            Focus::Sidebar => match code {
                key::UP => {
                    self.place_ring = self.place_ring.saturating_sub(1);
                    self.damage.add(self.sidebar());
                }
                key::DOWN => {
                    self.place_ring = (self.place_ring + 1).min(self.place_count.saturating_sub(1));
                    self.damage.add(self.sidebar());
                }
                key::ENTER | key::SPACE => self.go_to_place(files, self.place_ring),
                _ => {}
            },
            Focus::List => self.list_key(files, code, shift, primary || alt, character),
        }
    }

    fn move_focus(&mut self, back: bool) {
        self.focus = match (self.focus, back) {
            (Focus::Path, false) | (Focus::List, true) => Focus::Sidebar,
            (Focus::Sidebar, false) | (Focus::Path, true) => Focus::List,
            (Focus::List, false) | (Focus::Sidebar, true) => Focus::Path,
        };
        if self.focus == Focus::Sidebar {
            self.place_ring = self.current_place().unwrap_or(0);
        }
        if self.focus == Focus::Path {
            self.path_before = self.path_field.text;
        }
        self.restart_caret();
        self.damage_all();
    }

    fn focus_path(&mut self) {
        self.focus = Focus::Path;
        self.path_before = self.path_field.text;
        self.path_field.caret = self.path_field.text.len();
        let rect = self.path_field_rect();
        widget::keep_caret_visible(&mut self.path_field, rect);
        self.restart_caret();
        self.damage_all();
    }

    fn path_key(&mut self, files: &mut impl Files, code: u16, character: u32, command: bool) {
        let rect = self.path_field_rect();
        match code {
            key::ENTER => {
                let typed = self.path_field.text;
                let typed = typed.as_str().as_bytes();
                let trash_words = [Place::Trash.label(false), Place::Trash.label(true)];
                let location = match trash_words.iter().any(|word| word.as_bytes() == typed) {
                    true => Some(Location::Trash),
                    false => names::normalized(typed)
                        .filter(|path| path.as_bytes().first() == Some(&b'/'))
                        .map(Location::Folder),
                };
                if typed.len() + 4 > PATH_FIELD_CAPACITY {
                    self.show_notice(format_args!(
                        "Could not open this folder ({}).",
                        errno_text(Errno::ENAMETOOLONG)
                    ));
                    return;
                }
                if typed == self.path_before.as_str().as_bytes() {
                    self.focus = Focus::List;
                    self.set_path_text();
                    self.damage_all();
                    return;
                }
                match location {
                    Some(location) => {
                        self.go(files, location);
                        self.focus = Focus::List;
                        self.set_path_text();
                    }
                    None => self.show_notice(format_args!(
                        "Could not open {} ({}).",
                        Shown(typed),
                        errno_text(Errno::ENOENT)
                    )),
                }
                self.damage_all();
            }
            key::ESC => {
                self.path_field.text = self.path_before;
                self.set_path_text();
                self.focus = Focus::List;
                self.damage_all();
            }
            _ => {
                let before = self.path_before;
                let edited =
                    widget::field_key(&mut self.path_field, code, character, command, &before);
                if edited != FieldKey::Unused {
                    widget::keep_caret_visible(&mut self.path_field, rect);
                    self.restart_caret();
                    self.damage.add(self.toolbar());
                }
            }
        }
    }

    fn list_key(
        &mut self,
        files: &mut impl Files,
        code: u16,
        shift: bool,
        command: bool,
        character: u32,
    ) {
        let count = self.listing.len();
        let page = (self.rows().height / ROW_HEIGHT - 1).max(1) as usize;
        let last = count.saturating_sub(1);
        let moved = match (code, self.selected) {
            _ if count == 0 => None,
            (key::UP, Some(at)) => Some(at.saturating_sub(1)),
            (key::DOWN, Some(at)) => Some((at + 1).min(last)),
            (key::UP | key::DOWN | key::HOME, None) | (key::HOME, _) => Some(0),
            (key::END, _) => Some(last),
            (key::PAGE_UP, at) => Some(at.unwrap_or(0).saturating_sub(page)),
            (key::PAGE_DOWN, at) => Some((at.unwrap_or(0) + page).min(last)),
            _ => None,
        };
        if let Some(index) = moved {
            self.select(Some(index));
            return;
        }
        match code {
            key::ENTER => {
                if let Some(index) = self.selected {
                    self.open(files, index);
                }
            }
            key::BACKSPACE => self.up(files),
            key::DELETE => self.delete_selected(files, shift),
            code::F2 => {
                if let Some(index) = self.selected {
                    self.begin_rename(index);
                }
            }
            _ => {
                let typed = char::from_u32(character)
                    .filter(|typed| !command && !typed.is_control() && *typed != ' ');
                if let Some(typed) = typed {
                    self.type_ahead(typed);
                }
            }
        }
    }

    fn type_ahead(&mut self, typed: char) {
        if self.now_ms.wrapping_sub(self.typed_at) > TYPE_AHEAD_MS {
            self.typed.set("");
        }
        self.typed_at = self.now_ms;
        self.typed.insert(self.typed.len(), typed);
        let prefix = self.typed.as_str().as_bytes();
        let count = self.listing.len();
        let start = match self.typed.as_str().chars().count() {
            1 => self.selected.map_or(0, |at| at + 1),
            _ => self.selected.unwrap_or(0),
        };
        let found = (0..count)
            .map(|step| (start + step) % count.max(1))
            .find(|&index| {
                let name = self.listing.name(index);
                name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix)
            });
        if found.is_some() {
            self.select(found);
        }
    }

    fn pointer_moved(&mut self, x: i32, y: i32) {
        if let Some(Pressed::Thumb {
            grab_y,
            start_offset,
        }) = self.pressed
        {
            let rows = self.rows();
            let content = self.content_height();
            let thumb = (rows.height * rows.height / content.max(1))
                .max(MIN_THUMB)
                .min(rows.height);
            let travel = (rows.height - thumb).max(1);
            self.offset = start_offset + (y - grab_y) * (content - rows.height) / travel;
            self.clamp_offset();
            self.damage_middle();
            return;
        }
        let hover = match () {
            () if BACK_BUTTON.contains(x, y) => Some(Hover::Back),
            () if UP_BUTTON.contains(x, y) => Some(Hover::Up),
            () => match self.place_at(x, y) {
                Some(index) => Some(Hover::Place(index)),
                None => self
                    .scroll_bar()
                    .filter(|(_, thumb)| thumb.contains(x, y))
                    .map(|_| Hover::Thumb),
            },
        };
        if hover != self.hover {
            self.hover = hover;
            self.damage.add(self.toolbar());
            self.damage.add(self.sidebar());
            if let Some((track, _)) = self.scroll_bar() {
                self.damage.add(track);
            }
        }
    }

    fn button(
        &mut self,
        files: &mut impl Files,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    ) {
        match (button, pressed) {
            (MouseButton::Left, true) => self.left_pressed(files, x, y),
            (MouseButton::Left, false) => self.left_released(files, x, y),
            (MouseButton::Right, true) => self.right_pressed(x, y),
            _ => {}
        }
    }

    fn left_pressed(&mut self, files: &mut impl Files, x: i32, y: i32) {
        if BACK_BUTTON.contains(x, y) {
            self.pressed = Some(Pressed::Back);
            self.damage.add(self.toolbar());
            return;
        }
        if UP_BUTTON.contains(x, y) {
            self.pressed = Some(Pressed::Up);
            self.damage.add(self.toolbar());
            return;
        }
        let field = self.path_field_rect();
        if field.contains(x, y) {
            if self.focus != Focus::Path {
                self.focus_path();
            }
            self.path_field.caret = widget::caret_at(&self.path_field, field, x);
            self.restart_caret();
            self.damage.add(self.toolbar());
            return;
        }
        if let Some(index) = self.place_at(x, y) {
            self.focus = Focus::Sidebar;
            self.go_to_place(files, index);
            return;
        }
        if let Some((track, thumb)) = self.scroll_bar()
            && track.contains(x, y)
        {
            if thumb.contains(x, y) {
                self.pressed = Some(Pressed::Thumb {
                    grab_y: y,
                    start_offset: self.offset,
                });
            } else {
                let page = self.rows().height - ROW_HEIGHT;
                self.offset += if y < thumb.y { -page } else { page };
                self.clamp_offset();
            }
            self.damage_middle();
            return;
        }
        if self.rows().contains(x, y) {
            if self.focus != Focus::List {
                self.focus = Focus::List;
                self.damage_all();
            }
            match self.row_at(x, y) {
                Some(index) => {
                    let double = self.last_click.is_some_and(|(row, at)| {
                        row == index && self.now_ms.wrapping_sub(at) <= DOUBLE_CLICK_MS
                    });
                    self.select(Some(index));
                    match double {
                        true => {
                            self.last_click = None;
                            self.open(files, index);
                        }
                        false => self.last_click = Some((index, self.now_ms)),
                    }
                }
                None => {
                    self.last_click = None;
                    self.select(None);
                }
            }
        }
    }

    fn left_released(&mut self, files: &mut impl Files, x: i32, y: i32) {
        let Some(pressed) = self.pressed.take() else {
            return;
        };
        self.damage.add(self.toolbar());
        match pressed {
            Pressed::Back if BACK_BUTTON.contains(x, y) => self.back(files),
            Pressed::Up if UP_BUTTON.contains(x, y) => self.up(files),
            _ => self.damage_middle(),
        }
    }

    fn right_pressed(&mut self, x: i32, y: i32) {
        if !self.rows().contains(x, y) || x >= self.columns_right() {
            return;
        }
        self.focus = Focus::List;
        let row = self.row_at(x, y);
        self.select(row);
        let on = match row {
            Some(index) => MenuOn::Row(index),
            None => MenuOn::Space,
        };
        self.open_menu(on, x, y);
    }

    fn open_menu_at_selection(&mut self) {
        match self.selected {
            Some(index) => {
                let row = self.row_rect(index);
                self.open_menu(MenuOn::Row(index), row.x + NAME_X, row.bottom());
            }
            None => {
                let rows = self.rows();
                self.open_menu(MenuOn::Space, rows.x + NAME_X, rows.y);
            }
        }
    }

    fn open_menu(&mut self, on: MenuOn, x: i32, y: i32) {
        let mut entries = [Entry::Separator; eggos_toolkit::menu::MAX_ENTRIES];
        let count = self.menu_entries(on, &mut entries);
        let menu = Menu::new(&entries[..count], x, y, self.content());
        self.damage.add(menu.rect());
        self.menu = Some((menu, on));
    }

    fn menu_entries(
        &self,
        on: MenuOn,
        out: &mut [Entry; eggos_toolkit::menu::MAX_ENTRIES],
    ) -> usize {
        let windows = self.windows_words;
        let share = self.on_share();
        let pasteable = self.clipboard.is_some() && !share;
        let item = |label, enabled, id| Entry::Item { label, enabled, id };
        let delete_label = match (windows, self.trashable() || share) {
            (_, false) => "Delete permanently",
            (true, true) => "Delete",
            (false, true) => "Move to Trash",
        };
        let mut count = 0;
        let mut add = |entry: Entry| {
            out[count] = entry;
            count += 1;
        };
        match on {
            MenuOn::Space if self.is_trash() => {
                let label = if windows {
                    "Empty Recycle Bin"
                } else {
                    "Empty Trash"
                };
                add(item(label, !self.listing.is_empty(), item::EMPTY_TRASH));
                add(Entry::Separator);
                add(item("Reload", true, item::RELOAD));
            }
            MenuOn::Space => {
                add(item("New folder", !share, item::NEW_FOLDER));
                add(item("Paste", pasteable, item::PASTE));
                add(Entry::Separator);
                add(item("Reload", true, item::RELOAD));
            }
            MenuOn::Row(_) if self.is_trash() => {
                add(item("Restore", true, item::RESTORE));
                add(item("Delete permanently", true, item::DELETE_PERMANENTLY));
                add(Entry::Separator);
                add(item("Properties", true, item::PROPERTIES));
            }
            MenuOn::Row(index) => {
                let kind = self
                    .listing
                    .get(index)
                    .map_or(Kind::Other, |entry| entry.kind);
                if let Kind::App(_) = kind {
                    add(item("Open", true, item::OPEN));
                    add(item("Uninstall...", true, item::UNINSTALL));
                    add(Entry::Separator);
                    add(item("Properties", true, item::PROPERTIES));
                    return count;
                }
                add(item("Open", true, item::OPEN));
                if kind.installs() {
                    add(item("Install...", true, item::INSTALL));
                }
                add(Entry::Separator);
                add(item("Cut", !share, item::CUT));
                add(item("Copy", true, item::COPY));
                if kind == Kind::Folder {
                    add(item("Paste", pasteable, item::PASTE));
                }
                add(Entry::Separator);
                add(item("Rename", !share, item::RENAME));
                add(item(delete_label, !share, item::DELETE));
                add(Entry::Separator);
                add(item("Properties", true, item::PROPERTIES));
            }
        }
        count
    }

    fn choose(&mut self, files: &mut impl Files, id: u8, on: MenuOn) {
        let row = match on {
            MenuOn::Row(index) => Some(index),
            MenuOn::Space => None,
        };
        match (id, row) {
            (item::OPEN, Some(index)) => self.open(files, index),
            (item::INSTALL, Some(index)) => {
                if let Some(path) = self.entry_path(index) {
                    self.ask(Request::Install(path));
                }
            }
            (item::UNINSTALL, Some(index)) => self.uninstall(index),
            (item::CUT, Some(_)) => self.clip(true),
            (item::COPY, Some(_)) => self.clip(false),
            (item::PASTE, Some(index)) => {
                if let Some(path) = self.entry_path(index) {
                    self.paste(files, path);
                }
            }
            (item::PASTE, None) => {
                if let Some(folder) = self.folder().copied() {
                    self.paste(files, folder);
                }
            }
            (item::RENAME, Some(index)) => self.begin_rename(index),
            (item::DELETE, Some(_)) => self.delete_selected(files, false),
            (item::DELETE_PERMANENTLY, Some(_)) => self.delete_selected(files, true),
            (item::PROPERTIES, Some(_)) => self.properties(files),
            (item::NEW_FOLDER, None) => self.new_folder(files),
            (item::RELOAD, _) => self.reload(files),
            (item::EMPTY_TRASH, _) => self.ask_to_empty(files),
            (item::RESTORE, Some(index)) => self.restore(files, index),
            _ => {}
        }
    }

    fn uninstall(&mut self, index: usize) {
        if let Some(name) = Name::from(self.listing.name(index)) {
            self.ask(Request::Uninstall(name));
        }
    }

    /// Opens entry `index` as a double-click does (section 8.1 item 11).
    pub fn open(&mut self, files: &mut impl Files, index: usize) {
        let Some(entry) = self.listing.get(index).copied() else {
            return;
        };
        if self.is_trash() {
            self.properties(files);
            return;
        }
        let Some(path) = self.entry_path(index) else {
            return;
        };
        let owned = Name::from(self.listing.name(index)).unwrap_or_default();
        let name = owned.as_bytes();
        match entry.kind {
            Kind::Folder => self.go(files, Location::Folder(path)),
            Kind::App(_) => {
                if let Some(name) = Name::from(name) {
                    self.ask(Request::OpenApp(name));
                }
            }
            Kind::Bundle | Kind::Program(manifest::Source::Windows | manifest::Source::Macos) => {
                self.ask(Request::Install(path))
            }
            Kind::Program(manifest::Source::Linux) => self.type_in_terminal(b"", &path),
            Kind::Text if entry.size > LARGEST_SHOWN_TEXT => self.show_notice(format_args!(
                "{} is too large to show in the Terminal.",
                Shown(name)
            )),
            Kind::Text => self.type_in_terminal(b"cat ", &path),
            Kind::File | Kind::Other => {
                self.show_notice(format_args!("No app opens {} yet.", Shown(name)))
            }
        }
    }

    fn type_in_terminal(&mut self, prefix: &[u8], path: &Path) {
        match TerminalLine::quoted(prefix, path.as_bytes()) {
            Some(line) => self.ask(Request::TypeInTerminal(line)),
            None => self.show_notice(format_args!(
                "Could not open {} ({}).",
                Shown(path.file_name()),
                errno_text(Errno::ENAMETOOLONG)
            )),
        }
    }

    fn clip(&mut self, cut: bool) {
        let Some(index) = self.selected else {
            return;
        };
        if self.is_trash() {
            return;
        }
        if cut && self.on_share() {
            self.show_notice(format_args!("The host share is read-only."));
            return;
        }
        if let Some(path) = self.entry_path(index) {
            self.clipboard = Some(Clip { path, cut });
            self.damage_middle();
        }
    }

    /// Whether `index`'s entry is cut, waiting to be pasted.
    pub fn is_cut(&self, index: usize) -> bool {
        self.clipboard.is_some_and(|clip| {
            clip.cut && self.entry_path(index).is_some_and(|path| path == clip.path)
        })
    }

    fn paste(&mut self, files: &mut impl Files, folder: Path) {
        let Some(clip) = self.clipboard else {
            return;
        };
        if is_within(folder.as_bytes(), HOST_SHARE) {
            self.show_notice(format_args!("The host share is read-only."));
            return;
        }
        let name = clip.path.file_name();
        if clip.cut && parent(clip.path.as_bytes()) == folder.as_bytes() {
            self.clipboard = None;
            self.damage_middle();
            return;
        }
        if names::is_inside(folder.as_bytes(), clip.path.as_bytes()) {
            self.show_notice(format_args!("Cannot paste a folder into itself."));
            return;
        }
        let is_folder = files
            .status(clip.path.as_bytes())
            .is_ok_and(|status| status.kind == files::Kind::Directory);
        let target = free_path(files, folder.as_bytes(), |number| {
            copy_name(name, is_folder, number)
        });
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                self.show_notice(format_args!(
                    "Could not copy {} ({}).",
                    Shown(name),
                    errno_text(error)
                ));
                return;
            }
        };
        let job = match clip.cut {
            true => Job::moving(&clip.path, &target),
            false => Job::copy(&clip.path, &target),
        };
        let after = After::Paste {
            folder,
            name: Name::from(target.file_name()).unwrap_or_default(),
            cut: clip.cut,
        };
        self.start(files, job, after);
    }

    fn delete_selected(&mut self, files: &mut impl Files, permanently: bool) {
        let Some(index) = self.selected else {
            return;
        };
        let Some(path) = self.entry_path(index) else {
            return;
        };
        let owned = Name::from(self.listing.name(index)).unwrap_or_default();
        let name = owned.as_bytes();
        let kind = self
            .listing
            .get(index)
            .map_or(Kind::Other, |entry| entry.kind);
        if self.on_share() {
            self.show_notice(format_args!("The host share is read-only."));
            return;
        }
        if let Kind::App(_) = kind {
            self.uninstall(index);
            return;
        }
        if !permanently && !self.is_trash() && self.trashable() {
            match self.trash.put(files, path.as_bytes(), self.clock) {
                Ok(_) => {
                    self.reload(files);
                    let next = index.min(self.listing.len().saturating_sub(1));
                    let next = (!self.listing.is_empty()).then_some(next);
                    self.select(next);
                }
                Err(error) => self.show_notice(format_args!(
                    "Could not delete {} ({}).",
                    Shown(name),
                    errno_text(error)
                )),
            }
            return;
        }
        let mut first = Line::new();
        let _ = write!(first, "Delete {} permanently?", Shown(name));
        let second = match self
            .folder()
            .is_some_and(|folder| is_within(folder.as_bytes(), TMP))
        {
            true => "/tmp is not kept, so it has no Trash. This cannot be undone.",
            false => "This cannot be undone.",
        };
        let panel = Panel::new(&[first.as_str(), second], &["Cancel", "Delete"], Some(1), 0);
        self.panel = Some((panel, Asked::DeletePermanently(path)));
        self.damage_middle();
    }

    fn ask_to_empty(&mut self, files: &mut impl Files) {
        let count = self.trash.count(files);
        if count == 0 {
            return;
        }
        let title = match self.windows_words {
            true => "Empty the Recycle Bin?",
            false => "Empty the Trash?",
        };
        let mut second = Line::new();
        let _ = write!(
            second,
            "Deletes {}. This cannot be undone.",
            items_text(count).as_str()
        );
        let panel = Panel::new(&[title, second.as_str()], &["Cancel", "Empty"], Some(1), 0);
        self.panel = Some((panel, Asked::EmptyTrash));
        self.damage_middle();
    }

    fn restore(&mut self, files: &mut impl Files, index: usize) {
        let Some(name) = Name::from(self.listing.name(index)) else {
            return;
        };
        match self.trash.restore(files, name.as_bytes()) {
            Ok(_) => self.reload(files),
            Err(RestoreError::FolderGone(folder)) => self.show_notice(format_args!(
                "Cannot restore {}: {} no longer exists.",
                Shown(name.as_bytes()),
                Shown(folder.as_bytes())
            )),
            Err(RestoreError::NoRecord) => self.show_notice(format_args!(
                "Cannot restore {}: the Trash has no record of where it was.",
                Shown(name.as_bytes())
            )),
            Err(RestoreError::OutsideHome) => self.show_notice(format_args!(
                "Cannot restore {}: its record points outside the home folder.",
                Shown(name.as_bytes())
            )),
            Err(RestoreError::Failed(error)) => self.show_notice(format_args!(
                "Could not move {} ({}).",
                Shown(name.as_bytes()),
                errno_text(error)
            )),
        }
    }

    fn properties(&mut self, files: &mut impl Files) {
        let Some(index) = self.selected else {
            return;
        };
        let Some(entry) = self.listing.get(index).copied() else {
            return;
        };
        let Some(path) = self.entry_path(index) else {
            return;
        };
        let name = self.listing.name(index);
        let mut lines = [Line::new(); PROPERTIES_LINES];
        lines[0].push_bytes(name);
        let kind = kind::describe(files, path.as_bytes(), entry.kind, self.machine);
        let _ = write!(lines[1], "Kind: {}", kind.as_str());
        match entry.kind {
            Kind::Folder | Kind::Bundle => {
                let mut count = 0;
                let mut scratch = [0u8; 256];
                while let Ok(Some(_)) = files.entry(path.as_bytes(), count, &mut scratch) {
                    count += 1;
                }
                let _ = write!(lines[2], "Contains: {}", items_text(count).as_str());
            }
            Kind::App(_) => {
                let mut file = [0u8; manifest::LARGEST + 1];
                let run = installer::read_manifest(files, name, &mut file)
                    .and_then(|length| manifest::parse(&file[..length], name))
                    .map(|app| {
                        let mut run = Line::new();
                        run.push_bytes(app.run.as_bytes());
                        run
                    });
                let _ = write!(lines[2], "Runs: {}", run.as_ref().map_or("?", Line::as_str));
            }
            _ => {
                let _ = write!(
                    lines[2],
                    "Size: {} ({})",
                    size_text(entry.size).as_str(),
                    crate::listing::bytes_text(entry.size).as_str()
                );
            }
        }
        let time = crate::clock::list_time(entry.time);
        match self.is_trash() {
            true => {
                let _ = write!(lines[3], "Deleted: {}", time.as_str());
                let from = self.trash.record(files, name).map(|record| record.original);
                let _ = write!(
                    lines[4],
                    "From: {}",
                    Shown(
                        from.as_ref()
                            .map_or(&b"?"[..], |path| parent(path.as_bytes()))
                    )
                );
            }
            false => {
                let _ = write!(lines[3], "Modified: {}", time.as_str());
                let _ = write!(
                    lines[4],
                    "Where: {}",
                    Shown(self.folder().map_or(&b"/"[..], |folder| folder.as_bytes()))
                );
            }
        }
        let texts: [&str; PROPERTIES_LINES] = core::array::from_fn(|index| lines[index].as_str());
        let panel = Panel::new(&texts, &["Close"], Some(0), 0);
        self.panel = Some((panel, Asked::Properties));
        self.damage_middle();
    }

    fn start(&mut self, files: &mut impl Files, job: Job, after: After) {
        let name = Name::from(job.source().file_name()).unwrap_or_default();
        self.running = Some(Running { job, name, after });
        self.work(files);
        if let Some(running) = &self.running {
            let verb = match running.job.work() {
                Work::Copy => "Copying",
                Work::Move => "Moving",
                Work::Delete => "Deleting",
            };
            let mut line = Line::new();
            let _ = write!(line, "{verb} {}...", Shown(running.name.as_bytes()));
            let panel = Panel::progress(line.as_str(), running.job.permille());
            self.panel = Some((panel, Asked::Working));
            self.damage_middle();
        }
    }

    fn work(&mut self, files: &mut impl Files) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        let mut step = Step::Working;
        for _ in 0..STEPS_PER_TICK {
            step = running.job.step(files, &mut self.buffer);
            if step != Step::Working {
                break;
            }
        }
        match step {
            Step::Working => {
                if let Some((panel, Asked::Working)) = self.panel.as_mut() {
                    panel.set_progress(running.job.permille());
                    self.damage_middle();
                }
                self.running = Some(running);
            }
            Step::Done => {
                if matches!(self.panel, Some((_, Asked::Working))) {
                    self.panel = None;
                }
                self.finished(files, running.after);
            }
            Step::Failed(error, _) => {
                if matches!(self.panel, Some((_, Asked::Working))) {
                    self.panel = None;
                }
                let verb = match running.job.work() {
                    Work::Copy => "copy",
                    Work::Move => "move",
                    Work::Delete => "delete",
                };
                self.reload(files);
                self.show_notice(format_args!(
                    "Could not {verb} {} ({}).",
                    Shown(running.name.as_bytes()),
                    errno_text(error)
                ));
            }
        }
    }

    fn finished(&mut self, files: &mut impl Files, after: After) {
        match after {
            After::Paste { folder, name, cut } => {
                if cut {
                    self.clipboard = None;
                }
                self.reload(files);
                if self.folder() == Some(&folder) {
                    let found = self.listing.find(name.as_bytes());
                    self.select(found);
                }
            }
            After::Delete { trashed } => {
                if let Some(name) = trashed {
                    self.trash.forget(files, name.as_bytes());
                }
                let index = self.selected;
                self.reload(files);
                let next = index.map(|index| index.min(self.listing.len().saturating_sub(1)));
                self.select(next.filter(|_| !self.listing.is_empty()));
            }
            After::EmptyTrash => {
                let _ = self.trash.empty(files);
                self.reload(files);
            }
        }
        self.damage_all();
    }
}

struct Shown<'a>(&'a [u8]);

impl core::fmt::Display for Shown<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut out = [0u8; names::NAME_CAPACITY * 2];
        formatter.write_str(names::display(self.0, &mut out))
    }
}
