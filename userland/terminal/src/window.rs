//! A terminal window's state (docs/ux/terminal-windows-v0.3.md,
//! docs/ux/terminal-selection-v0.3.md): the engine fed the shell's output, the grid that follows
//! the window's size (and tells the shell, which gets `SIGWINCH`), the cursor's look and blink,
//! the size indicator while the grid changes, the selection and its copy, paste and context
//! menu, the question before closing a window whose shell runs a program, the end of a command
//! run with `-e`, the title, and the screen's rows as elements for agents. The shell is behind
//! [`Shell`], so all of it is host-tested.

use crate::keys::{KeyAction, key_action};
use crate::paste::{PASTE_CAPACITY, PendingPaste, prepare};
use crate::selection::{
    Boundary, DOUBLE_CLICK_MS, Selection, SelectionMode, copied_text, logical_line_at,
    select_all,
};
use crate::title::Title;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_element_tree::{
    Action, Bounds, Element, HEADER_BYTES, MAX_LABEL_BYTES, MAX_TEXT_BYTES, MAX_TREE_BYTES,
    Outcome, Role, TreeWriter, flags,
};
use eggos_terminal::geometry::Damage as GridDamage;
use eggos_terminal::keymap::CursorKeys;
use eggos_terminal::{CursorLook, PADDING, TERM_BACKGROUND, Terminal, ViewMove, WHEEL_LINES};
use eggos_toolkit::canvas::{Canvas, INSET_R4};
use eggos_toolkit::colour;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers, MouseButton, key as input_key};
use eggos_toolkit::menu::{Entry, Menu, MenuKey};
use eggos_toolkit::panel::{Panel, PanelResponse};
use eggos_toolkit::tree::PointerShape;

/// A new window's content at scale 1: 80 by 24 cells and the padding, as the console's.
pub const DEFAULT_CONTENT: (u32, u32) = (656, 400);

/// The smallest content at scale 1: 20 by 5 cells.
pub const MINIMUM_CONTENT: (u32, u32) = (176, 96);

/// The most columns a grid has; a wider content pads further.
pub const MAX_COLUMNS: u16 = 512;

/// The most rows a grid has.
pub const MAX_ROWS: u16 = 256;

/// How long the size indicator stays after the grid last changed.
pub const INDICATOR_MS: u32 = 600;

/// How long the cursor shows, then hides, in each blink.
pub const BLINK_HALF_MS: u32 = 500;

/// How often a drag above or below the grid scrolls the view a line.
pub const DRAG_SCROLL_MS: u32 = 50;

/// How soon a paste waiting for room in the pseudo-terminal tries again.
pub const PASTE_RETRY_MS: u32 = 16;

/// The close question's first line.
pub const CLOSE_QUESTION: &str = "Close this terminal?";

/// The close question's second line.
pub const CLOSE_EXPLANATION: &str = "A program is still running in it and will be ended.";

/// The id of the element holding the screen's rows; the rows follow it.
pub const SCREEN_ELEMENT: u32 = 2;

/// The context menu's `Copy`.
pub const COPY: u8 = 0;
/// The context menu's `Paste`.
pub const PASTE: u8 = 1;
/// The context menu's `Select All`.
pub const SELECT_ALL: u8 = 2;

const CELL_WIDTH: i32 = eggos_font::GLYPH_WIDTH as i32;
const CELL_HEIGHT: i32 = eggos_font::GLYPH_HEIGHT as i32;
const CANCEL: usize = 0;
const CLOSE: usize = 1;
const INDICATOR_HEIGHT: i32 = 24;
const INDICATOR_PADDING: i32 = 8;
const INDICATOR_TEXT_TOP: i32 = 4;
const MENU_MARGIN: i32 = 8;
const READ_CHUNK: usize = 4096;
const EXIT_LINE_START: &[u8] = b"\r\n\x1b[0;90m";
const EXIT_LINE_END: &[u8] = b"\x1b[0m\x1b[?25l";

/// What a read of the shell's output found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellOutput {
    /// This many bytes.
    Bytes(usize),
    /// Nothing for now.
    Nothing,
    /// The shell's side is gone: the shell ended.
    Ended,
}

/// How the program a window ran ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEnd {
    /// It exited with this status.
    Exited(u8),
    /// A signal of this number ended it.
    Signalled(u8),
}

/// The shell on the other side of the pseudo-terminal, and what the window asks of the system.
pub trait Shell {
    /// Reads what the shell wrote into `buffer`, never waiting.
    fn read(&mut self, buffer: &mut [u8]) -> ShellOutput;
    /// Sends `bytes` to the shell as typed.
    fn write(&mut self, bytes: &[u8]);
    /// Whether the pseudo-terminal has room for a paste's next chunk (`POLLOUT`).
    fn has_room(&mut self) -> bool;
    /// Tells the shell the grid is `columns` by `rows` cells (`TIOCSWINSZ`).
    fn set_window_size(&mut self, columns: u16, rows: u16);
    /// Whether the terminal's foreground process group is the shell's own: no program runs in
    /// the foreground.
    fn foreground_is_shell(&self) -> bool;
    /// Opens another terminal window in `folder`, or in this one's starting folder for `None`.
    fn open_new_window(&mut self, folder: Option<&str>);
    /// Makes `text` the clipboard every app shares.
    fn set_clipboard(&mut self, text: &str);
    /// Copies the shared clipboard's text into `into`, as much as fits; its whole length.
    fn clipboard(&mut self, into: &mut [u8]) -> usize;
    /// How the program ended, once its side closed; `None` when that cannot be learned.
    fn process_end(&mut self) -> Option<ProcessEnd>;
}

/// The grid a content of `width` by `height` (scale 1) holds: the cells inside the padding, at
/// least one and at most [`MAX_COLUMNS`] by [`MAX_ROWS`].
pub fn grid_for(width: i32, height: i32) -> (u16, u16) {
    let padding = 2 * PADDING as i32;
    let columns = ((width - padding) / CELL_WIDTH).clamp(1, i32::from(MAX_COLUMNS));
    let rows = ((height - padding) / CELL_HEIGHT).clamp(1, i32::from(MAX_ROWS));
    (columns as u16, rows as u16)
}

/// The name the shell (D181) gives signal `number`, `None` for one it has no name for.
pub fn signal_name(number: u8) -> Option<&'static str> {
    const NAMES: [&str; 31] = [
        "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE",
        "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM", "SIGSTKFLT",
        "SIGCHLD", "SIGCONT", "SIGSTOP", "SIGTSTP", "SIGTTIN", "SIGTTOU", "SIGURG", "SIGXCPU",
        "SIGXFSZ", "SIGVTALRM", "SIGPROF", "SIGWINCH", "SIGIO", "SIGPWR", "SIGSYS",
    ];
    NAMES.get(usize::from(number).checked_sub(1)?).copied()
}

/// The line a window whose command failed shows: `[process exited with status <n>]` or
/// `[process ended by signal <n> (<name>)]`.
pub fn exit_line(end: ProcessEnd) -> String {
    let mut line = String::new();
    let _ = match end {
        ProcessEnd::Exited(status) => write!(line, "[process exited with status {status}]"),
        ProcessEnd::Signalled(number) => match signal_name(number) {
            Some(name) => write!(line, "[process ended by signal {number} ({name})]"),
            None => write!(line, "[process ended by signal {number}]"),
        },
    };
    line
}

#[derive(Default)]
struct SmallText {
    bytes: [u8; 16],
    length: usize,
}

impl Write for SmallText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let room = self
            .bytes
            .get_mut(self.length..end)
            .ok_or(core::fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

impl SmallText {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outside {
    Above,
    Inside,
    Below,
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    boundary: Boundary,
    cell: Boundary,
    outside: Outside,
}

/// A terminal window: see the module documentation.
pub struct TerminalWindow<S: Shell> {
    shell: S,
    terminal: Box<Terminal>,
    pixels: Vec<u32>,
    width: i32,
    height: i32,
    damage: Damage,
    render_due: bool,
    focused: bool,
    active_at: u32,
    look: CursorLook,
    title: Title,
    title_changed: bool,
    indicator_until: Option<u32>,
    sized: bool,
    panel: Option<Panel>,
    menu: Option<Menu>,
    closing: bool,
    now: u32,
    selection: Option<Selection>,
    dragging: bool,
    drag_outside: Outside,
    drag_scroll_at: Option<u32>,
    pointer: Option<(i32, i32)>,
    clicks: u8,
    last_press_at: Option<u32>,
    shift_held: bool,
    paste: PendingPaste,
    command: bool,
    ended: Option<ProcessEnd>,
}

impl<S: Shell> core::fmt::Debug for TerminalWindow<S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TerminalWindow")
            .field("terminal", &self.terminal)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("title", &self.title)
            .field("selection", &self.selection)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

fn blank_pixels(width: i32, height: i32) -> Option<Vec<u32>> {
    let count = width.max(0) as usize * height.max(0) as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).ok()?;
    pixels.resize(count, TERM_BACKGROUND);
    Some(pixels)
}

fn is_shift(code: u16) -> bool {
    code == eggos_terminal::key::LEFT_SHIFT || code == eggos_terminal::key::RIGHT_SHIFT
}

impl<S: Shell> TerminalWindow<S> {
    /// A window of `width` by `height` (scale 1) on `shell`, told its grid, at `now_ms`; `None`
    /// without memory.
    pub fn new(mut shell: S, width: i32, height: i32, now_ms: u32) -> Option<Self> {
        let (columns, rows) = grid_for(width, height);
        let terminal = Box::new(Terminal::new(columns, rows)?);
        shell.set_window_size(columns, rows);
        Some(Self {
            shell,
            terminal,
            pixels: blank_pixels(width, height)?,
            width,
            height,
            damage: Damage::new(),
            render_due: true,
            focused: true,
            active_at: now_ms,
            look: CursorLook::Block,
            title: Title::default(),
            title_changed: false,
            indicator_until: None,
            sized: false,
            panel: None,
            menu: None,
            closing: false,
            now: now_ms,
            selection: None,
            dragging: false,
            drag_outside: Outside::Inside,
            drag_scroll_at: None,
            pointer: None,
            clicks: 0,
            last_press_at: None,
            shift_held: false,
            paste: PendingPaste::default(),
            command: false,
            ended: None,
        })
    }

    /// Makes it a `terminal -e` window: when its program ends with status 0 it closes, when it
    /// fails or a signal ends it the window stays with the line [`exit_line`] says.
    pub fn run_command(&mut self) {
        self.command = true;
    }

    /// Names the window `text` followed by ` - Terminal`, as `-t` does.
    pub fn set_title_text(&mut self, text: &str) {
        let title = Title::of(text.as_bytes(), false);
        if title != self.title {
            self.title = title;
            self.title_changed = true;
        }
    }

    /// The shell.
    pub fn shell(&self) -> &S {
        &self.shell
    }

    /// The shell, to change.
    pub fn shell_mut(&mut self) -> &mut S {
        &mut self.shell
    }

    /// The engine, for what the window shows.
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// The window's image of its grid at scale 1, as the last [`Self::take_damage`] drew it,
    /// rows [`Self::size`]'s width apart.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The content's size (scale 1).
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The window's title.
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    /// The title once after it changed.
    pub fn take_title(&mut self) -> Option<&str> {
        match core::mem::take(&mut self.title_changed) {
            true => Some(self.title.as_str()),
            false => None,
        }
    }

    /// Whether the close question shows.
    pub fn is_asking_to_close(&self) -> bool {
        self.panel.is_some()
    }

    /// Whether the context menu is open.
    pub fn menu(&self) -> Option<&Menu> {
        self.menu.as_ref()
    }

    /// The selection, if any.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// The text a copy takes now; empty without a selection.
    pub fn selected_text(&self) -> String {
        match &self.selection {
            Some(selection) => copied_text(&self.terminal, selection),
            None => String::new(),
        }
    }

    /// How the `-e` program ended, once it did and the window stays.
    pub fn ended(&self) -> Option<ProcessEnd> {
        self.ended
    }

    /// Whether a paste waits for room in the pseudo-terminal.
    pub fn is_pasting(&self) -> bool {
        !self.paste.is_empty()
    }

    /// Whether the size indicator shows: the grid changed less than [`INDICATOR_MS`] ago.
    pub fn is_showing_size(&self) -> bool {
        self.indicator_until.is_some()
    }

    fn indicator(&self) -> Option<(SmallText, Rect)> {
        self.indicator_until?;
        let (columns, rows) = self.terminal.size();
        let mut text = SmallText::default();
        let _ = write!(text, "{columns}x{rows}");
        let width = text.length as i32 * CELL_WIDTH + 2 * INDICATOR_PADDING;
        let rect = Rect::new(
            (self.width - width) / 2,
            (self.height - INDICATOR_HEIGHT) / 2,
            width,
            INDICATOR_HEIGHT,
        );
        Some((text, rect))
    }

    fn whole(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    fn grid_rect(&self) -> Rect {
        let (columns, rows) = self.terminal.size();
        Rect::new(
            PADDING as i32,
            PADDING as i32,
            i32::from(columns) * CELL_WIDTH,
            i32::from(rows) * CELL_HEIGHT,
        )
    }

    /// The pointer's shape over the content: the I-beam over the grid, the arrow over the
    /// padding, a menu or the close question.
    pub fn pointer_shape(&self) -> PointerShape {
        match self.pointer {
            Some((x, y))
                if self.grid_rect().contains(x, y)
                    && self.panel.is_none()
                    && !self
                        .menu
                        .as_ref()
                        .is_some_and(|menu| menu.rect().contains(x, y)) =>
            {
                PointerShape::IBeam
            }
            _ => PointerShape::Arrow,
        }
    }

    /// The content is now `width` by `height` (scale 1): the grid follows, reflowed, the shell
    /// told and, but for the window's first size, the indicator shown when it changes;
    /// everything is drawn again.
    pub fn resize(&mut self, width: i32, height: i32) {
        if (width, height) != (self.width, self.height) {
            let Some(pixels) = blank_pixels(width, height) else {
                return;
            };
            self.pixels = pixels;
            self.width = width;
            self.height = height;
        }
        let grid = grid_for(width, height);
        if grid != self.terminal.size() && self.terminal.resize(grid.0, grid.1) {
            self.shell.set_window_size(grid.0, grid.1);
            if self.sized {
                self.indicator_until = Some(self.now.wrapping_add(INDICATOR_MS));
            }
        }
        self.check_selection();
        self.close_menu();
        self.sized = true;
        self.terminal.invalidate();
        self.render_due = true;
        self.damage.add(self.whole());
    }

    /// Feeds the engine what the shell wrote until it has nothing more, answering what the
    /// engine replies (`CSI 6 n`) and taking the title a program set; `false` once the window
    /// is to close because its shell or command ended.
    pub fn read_shell(&mut self, now_ms: u32) -> bool {
        self.now = now_ms;
        if self.ended.is_some() {
            return true;
        }
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match self.shell.read(&mut chunk) {
                ShellOutput::Bytes(count) => self.output(&chunk[..count]),
                ShellOutput::Nothing => {
                    self.pump_paste();
                    return true;
                }
                ShellOutput::Ended => return self.program_ended(),
            }
        }
    }

    fn program_ended(&mut self) -> bool {
        let end = match self.command {
            true => self.shell.process_end(),
            false => None,
        };
        match end {
            None | Some(ProcessEnd::Exited(0)) => {
                self.closing = true;
                false
            }
            Some(end) => {
                self.finish(end);
                true
            }
        }
    }

    fn finish(&mut self, end: ProcessEnd) {
        self.move_view(ViewMove::Live);
        self.terminal.feed(EXIT_LINE_START);
        self.terminal.feed(exit_line(end).as_bytes());
        self.terminal.feed(EXIT_LINE_END);
        self.ended = Some(end);
        self.paste = PendingPaste::default();
        self.check_selection();
        self.render_due = true;
    }

    /// The `-e` program could not start: `message` shows in the grid and the window stays as
    /// for a program that exited with `status`.
    pub fn failed_to_start(&mut self, message: &str, status: u8) {
        self.terminal.feed(message.as_bytes());
        self.finish(ProcessEnd::Exited(status));
    }

    fn output(&mut self, bytes: &[u8]) {
        self.terminal.feed(bytes);
        let replies = self.terminal.replies();
        if !replies.is_empty() {
            let mut answer = [0u8; eggos_terminal::REPLY_CAPACITY];
            let length = replies.len();
            answer[..length].copy_from_slice(replies);
            self.terminal.clear_replies();
            self.shell.write(&answer[..length]);
        }
        if self.terminal.take_title_change() {
            let (text, cut) = self.terminal.title();
            let title = Title::of(text, cut);
            if title != self.title {
                self.title = title;
                self.title_changed = true;
            }
        }
        self.terminal.take_folder_change();
        self.check_selection();
        self.active_at = self.now;
        self.render_due = true;
    }

    fn check_selection(&mut self) {
        let reset = self.terminal.take_selection_reset();
        let dropped = self
            .selection
            .is_some_and(|selection| selection.is_dropped(&self.terminal));
        if reset || dropped {
            self.clear_selection();
        }
    }

    fn set_selection(&mut self, selection: Option<Selection>) {
        self.selection = selection;
        let highlight = selection.and_then(|selection| selection.highlight(&self.terminal));
        self.terminal.set_highlight(highlight);
        self.render_due = true;
    }

    fn clear_selection(&mut self) {
        self.dragging = false;
        self.drag_scroll_at = None;
        self.set_selection(None);
    }

    fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.move_view(ViewMove::Live);
        if self.paste.is_empty() {
            self.shell.write(bytes);
        } else {
            self.paste.push(bytes);
            self.pump_paste();
        }
        self.active_at = self.now;
        self.render_due = true;
    }

    fn pump_paste(&mut self) {
        while !self.paste.is_empty() && self.shell.has_room() {
            let chunk = self.paste.next_chunk();
            let count = chunk.len();
            self.shell.write(chunk);
            self.paste.written(count);
        }
    }

    fn move_view(&mut self, change: ViewMove) {
        let before = self.terminal.view_back();
        self.terminal.move_view(change);
        if self.terminal.view_back() != before {
            self.render_due = true;
        }
    }

    /// Copies the selection to the shared clipboard; `false` without one.
    pub fn copy(&mut self) -> bool {
        let text = self.selected_text();
        if text.is_empty() {
            return false;
        }
        self.shell.set_clipboard(&text);
        true
    }

    /// Pastes the shared clipboard (docs/ux/terminal-selection-v0.3.md section 1.3).
    pub fn paste(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let length = self.shell.clipboard(&mut []);
        if length == 0 {
            return;
        }
        let mut buffer = Vec::new();
        let wanted = length.min(PASTE_CAPACITY);
        if buffer.try_reserve_exact(wanted).is_err() {
            return;
        }
        buffer.resize(wanted, 0);
        let got = self.shell.clipboard(&mut buffer).min(wanted);
        let text = match core::str::from_utf8(&buffer[..got]) {
            Ok(text) => text,
            Err(error) => core::str::from_utf8(&buffer[..error.valid_up_to()]).unwrap_or(""),
        };
        let bytes = prepare(text, self.terminal.bracketed_paste());
        if bytes.is_empty() {
            return;
        }
        self.move_view(ViewMove::Live);
        self.paste.push(&bytes);
        self.pump_paste();
        self.active_at = self.now;
        self.render_due = true;
    }

    /// Selects every line, from the scrollback's first to the live screen's last written.
    pub fn select_all(&mut self) {
        if let Some(selection) = select_all(&self.terminal) {
            self.set_selection(Some(selection));
        }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let (columns, rows) = self.terminal.size();
        let (x, y) = (x - PADDING as i32, y - PADDING as i32);
        let row = y.div_euclid(CELL_HEIGHT);
        let outside = match row {
            row if row < 0 => Outside::Above,
            row if row >= i32::from(rows) => Outside::Below,
            _ => Outside::Inside,
        };
        let row = row.clamp(0, i32::from(rows) - 1) as u16;
        let line = self.terminal.line_number_of_view_row(row);
        let column = (x + CELL_WIDTH / 2)
            .div_euclid(CELL_WIDTH)
            .clamp(0, i32::from(columns)) as u16;
        let cell = x.div_euclid(CELL_WIDTH).clamp(0, i32::from(columns) - 1) as u16;
        Hit {
            boundary: Boundary { line, column },
            cell: Boundary { line, column: cell },
            outside,
        }
    }

    fn head_for(&self, hit: Hit, mode: SelectionMode) -> Boundary {
        match mode {
            SelectionMode::Cells => hit.boundary,
            _ => hit.cell,
        }
    }

    fn press(&mut self, x: i32, y: i32, time_ms: u32) {
        let within = self
            .last_press_at
            .is_some_and(|at| time_ms.wrapping_sub(at) < DOUBLE_CLICK_MS);
        self.clicks = match within && self.clicks < 3 {
            true => self.clicks + 1,
            false => 1,
        };
        self.last_press_at = Some(time_ms);
        let hit = self.hit(x, y);
        let selection = match (self.clicks, self.selection) {
            (1, Some(mut selection)) if self.shift_held => {
                selection.head = self.head_for(hit, selection.mode);
                selection
            }
            (2, _) => Selection::unit(&self.terminal, hit.cell, SelectionMode::Words),
            (3, _) => Selection::unit(&self.terminal, hit.cell, SelectionMode::Lines),
            _ => Selection::cells(hit.boundary),
        };
        self.dragging = true;
        self.drag_outside = Outside::Inside;
        self.set_selection(Some(selection));
    }

    fn drag_to(&mut self, x: i32, y: i32) {
        let Some(mut selection) = self.selection else {
            return;
        };
        let hit = self.hit(x, y);
        selection.head = self.head_for(hit, selection.mode);
        self.drag_outside = hit.outside;
        match hit.outside {
            Outside::Inside => self.drag_scroll_at = None,
            _ if self.drag_scroll_at.is_none() => {
                self.drag_scroll_at = Some(self.now.wrapping_add(DRAG_SCROLL_MS));
            }
            _ => {}
        }
        self.set_selection(Some(selection));
    }

    fn release(&mut self) {
        self.dragging = false;
        self.drag_scroll_at = None;
        if self
            .selection
            .is_some_and(|selection| selection.is_empty(&self.terminal))
        {
            self.set_selection(None);
        }
    }

    fn open_menu(&mut self, x: i32, y: i32) {
        let selected = !self.selected_text().is_empty();
        let pastable = self.shell.clipboard(&mut []) > 0;
        let any_text = select_all(&self.terminal).is_some();
        let entries = [
            Entry::Item {
                label: "Copy",
                key: "",
                enabled: selected,
                id: COPY,
            },
            Entry::Item {
                label: "Paste",
                key: "",
                enabled: pastable,
                id: PASTE,
            },
            Entry::Separator,
            Entry::Item {
                label: "Select All",
                key: "",
                enabled: any_text,
                id: SELECT_ALL,
            },
        ];
        let bounds = Rect::new(
            MENU_MARGIN,
            MENU_MARGIN,
            self.width - MENU_MARGIN,
            self.height - MENU_MARGIN,
        );
        self.close_menu();
        let menu = Menu::new(&entries, x, y, bounds);
        self.damage.add(menu.rect());
        self.menu = Some(menu);
    }

    fn open_menu_at_cursor(&mut self) {
        let (row, column) = self.terminal.cursor();
        let x = PADDING as i32 + i32::from(column) * CELL_WIDTH;
        let y = PADDING as i32 + (i32::from(row) + 1) * CELL_HEIGHT;
        self.open_menu(x, y);
    }

    fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.damage.add(menu.rect());
        }
    }

    fn choose(&mut self, id: u8) {
        self.close_menu();
        match id {
            COPY => {
                self.copy();
            }
            PASTE => self.paste(),
            SELECT_ALL => self.select_all(),
            _ => {}
        }
    }

    fn handle_menu(&mut self, input: Input) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match input {
            Input::Key {
                code,
                pressed: true,
                ..
            } => {
                match menu.key(code) {
                    MenuKey::Chosen(id) => self.choose(id),
                    MenuKey::Closed => self.close_menu(),
                    MenuKey::Nothing => self.damage.add(menu.rect()),
                }
                true
            }
            Input::Key { .. } => true,
            Input::PointerMove { x, y } => {
                if menu.hover(x, y) {
                    self.damage.add(menu.rect());
                }
                self.pointer = Some((x, y));
                true
            }
            Input::PointerLeave => {
                if menu.leave() {
                    self.damage.add(menu.rect());
                }
                self.pointer = None;
                true
            }
            Input::Button {
                x,
                y,
                pressed: true,
                ..
            } if menu.rect().contains(x, y) => {
                if let Some(id) = menu.id_at(x, y) {
                    self.choose(id);
                }
                true
            }
            Input::Button { x, y, .. } if menu.rect().contains(x, y) => true,
            Input::Button { pressed: true, .. } | Input::FocusOut => {
                self.close_menu();
                false
            }
            _ => false,
        }
    }

    fn handle_key(&mut self, code: u16, modifiers: Modifiers, character: u32) {
        let held = |bit: u32| modifiers.0 & bit != 0;
        let plain_ctrl = held(Modifiers::CTRL)
            && !held(Modifiers::SHIFT)
            && !held(Modifiers::ALT)
            && !held(Modifiers::SUPER);
        if code == input_key::C && plain_ctrl && self.copy() {
            self.clear_selection();
            return;
        }
        let cursor_keys = match self.terminal.application_cursor_keys() {
            true => CursorKeys::Application,
            false => CursorKeys::Normal,
        };
        match key_action(code, modifiers, character, cursor_keys) {
            KeyAction::Send(_) if self.ended.is_some() => self.closing = true,
            KeyAction::Send(bytes) => self.send(bytes.as_bytes()),
            KeyAction::View(change) => self.move_view(change),
            KeyAction::NewWindow => {
                let folder = self.terminal.current_folder();
                self.shell.open_new_window(folder);
            }
            KeyAction::Close => {
                self.close_requested(self.now);
            }
            KeyAction::Copy => {
                self.copy();
            }
            KeyAction::Paste => self.paste(),
            KeyAction::SelectAll => self.select_all(),
            KeyAction::ContextMenu => self.open_menu_at_cursor(),
            KeyAction::Nothing => {}
        }
    }

    /// Acts on `input` at `time_ms`; `false` once the window should close.
    pub fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        self.now = time_ms;
        if self.panel.is_some() {
            return self.handle_question(input);
        }
        if let Input::Key {
            code, modifiers, pressed, ..
        } = input
        {
            self.shift_held = match is_shift(code) {
                true => pressed,
                false => modifiers.shift(),
            };
        }
        if self.handle_menu(input) {
            return !self.closing;
        }
        match input {
            Input::Key {
                code,
                pressed: true,
                modifiers,
                character,
            } => self.handle_key(code, modifiers, character),
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => match pressed {
                true => self.press(x, y, time_ms),
                false => self.release(),
            },
            Input::Button {
                x,
                y,
                button: MouseButton::Right,
                pressed: true,
            } => self.open_menu(x, y),
            Input::Button {
                button: MouseButton::Middle,
                pressed: true,
                ..
            } => self.paste(),
            Input::PointerMove { x, y } => {
                self.pointer = Some((x, y));
                if self.dragging {
                    self.drag_to(x, y);
                }
            }
            Input::PointerLeave => self.pointer = None,
            Input::Wheel { notches, .. } => {
                let lines = notches.unsigned_abs() as usize * WHEEL_LINES;
                self.move_view(match notches > 0 {
                    true => ViewMove::Back(lines),
                    false => ViewMove::Forward(lines),
                });
                if let (true, Some((x, y))) = (self.dragging, self.pointer) {
                    self.drag_to(x, y);
                }
            }
            Input::FocusIn | Input::FocusOut => {
                self.focused = input == Input::FocusIn;
                if !self.focused {
                    self.shift_held = false;
                    self.release();
                }
                self.active_at = time_ms;
                self.render_due = true;
            }
            _ => {}
        }
        !self.closing
    }

    fn handle_question(&mut self, input: Input) -> bool {
        let area = self.whole();
        let Some(panel) = self.panel.as_mut() else {
            return true;
        };
        let response = match input {
            Input::Key {
                code,
                pressed,
                modifiers,
                ..
            } => panel.key(code, modifiers.shift(), pressed),
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => panel.button(area, x, y, pressed),
            Input::PointerMove { x, y } => {
                panel.pointer_move(area, x, y);
                PanelResponse::Nothing
            }
            Input::PointerLeave => {
                panel.pointer_leave();
                PanelResponse::Nothing
            }
            Input::FocusIn | Input::FocusOut => {
                self.focused = input == Input::FocusIn;
                PanelResponse::Nothing
            }
            _ => PanelResponse::Nothing,
        };
        let panel_rect = panel.rect(area);
        self.damage.add(panel_rect);
        match response {
            PanelResponse::Pressed(CLOSE) => {
                self.closing = true;
                false
            }
            PanelResponse::Pressed(_) => {
                self.panel = None;
                self.render_due = true;
                true
            }
            PanelResponse::Nothing => true,
        }
    }

    /// The user asked to close the window: `true` closes it now, when the shell runs no
    /// program in the foreground or the `-e` program ended; otherwise the close question shows
    /// and `false` keeps it open.
    pub fn close_requested(&mut self, _time_ms: u32) -> bool {
        if self.closing || self.ended.is_some() || self.shell.foreground_is_shell() {
            self.closing = true;
            return true;
        }
        self.close_menu();
        if self.panel.is_none() {
            let panel = Panel::new(
                &[CLOSE_QUESTION, CLOSE_EXPLANATION],
                &["Cancel", "Close"],
                Some(CLOSE),
                CANCEL,
            );
            self.damage.add(panel.rect(self.whole()));
            self.panel = Some(panel);
            self.render_due = true;
        }
        false
    }

    /// Whether the window is to close: the shell ended or the user said so.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// Whether the pseudo-terminal is still watched: not once the `-e` program ended.
    pub fn is_watching_the_shell(&self) -> bool {
        self.ended.is_none()
    }

    fn look_at(&self, now: u32) -> (CursorLook, Option<u32>) {
        if !self.focused || self.panel.is_some() {
            return (CursorLook::Outline, None);
        }
        if !self.terminal.cursor_blinks() {
            return (CursorLook::Block, None);
        }
        let halves = now.wrapping_sub(self.active_at) / BLINK_HALF_MS;
        let look = match halves % 2 {
            0 => CursorLook::Block,
            _ => CursorLook::Blinked,
        };
        let next = self
            .active_at
            .wrapping_add(halves.wrapping_add(1).wrapping_mul(BLINK_HALF_MS));
        (look, Some(next))
    }

    /// When the window next wants [`Self::tick`]: the cursor's next blink, the indicator's end,
    /// a drag's next scroll, a paste's next try.
    pub fn deadline(&self, now_ms: u32) -> Option<u32> {
        let (_, blink) = self.look_at(now_ms);
        let paste = (!self.paste.is_empty()).then(|| now_ms.wrapping_add(PASTE_RETRY_MS));
        [blink, self.indicator_until, self.drag_scroll_at, paste]
            .into_iter()
            .flatten()
            .min_by_key(|at| at.wrapping_sub(now_ms) as i32)
    }

    /// Looks at the clock: the cursor's blink, the indicator's end, a drag past the grid's top
    /// or bottom scrolling the view, a paste waiting for room.
    pub fn tick(&mut self, now_ms: u32) {
        self.now = now_ms;
        let (look, _) = self.look_at(now_ms);
        if look != self.look {
            self.look = look;
            self.render_due = true;
        }
        if let Some(until) = self.indicator_until
            && until.wrapping_sub(now_ms) as i32 <= 0
        {
            if let Some((_, rect)) = self.indicator() {
                self.damage.add(rect);
            }
            self.indicator_until = None;
        }
        if let Some(at) = self.drag_scroll_at
            && at.wrapping_sub(now_ms) as i32 <= 0
        {
            self.scroll_drag();
            self.drag_scroll_at = match self.dragging && self.drag_outside != Outside::Inside {
                true => Some(now_ms.wrapping_add(DRAG_SCROLL_MS)),
                false => None,
            };
        }
        self.pump_paste();
    }

    fn scroll_drag(&mut self) {
        let (columns, rows) = self.terminal.size();
        let (change, row, column) = match self.drag_outside {
            Outside::Above => (ViewMove::Back(1), 0, 0),
            Outside::Below => (ViewMove::Forward(1), rows - 1, columns),
            Outside::Inside => return,
        };
        self.move_view(change);
        let Some(mut selection) = self.selection else {
            return;
        };
        let line = self.terminal.line_number_of_view_row(row);
        selection.head = Boundary { line, column };
        if selection.mode != SelectionMode::Cells {
            selection.head.column = column.min(columns - 1);
        }
        self.set_selection(Some(selection));
    }

    /// Renders what changed into the window's image and returns what to show again.
    pub fn take_damage(&mut self) -> Damage {
        let (look, _) = self.look_at(self.now);
        if look != self.look {
            self.look = look;
            self.render_due = true;
        }
        if self.render_due {
            self.render_due = false;
            self.terminal.set_cursor_look(self.look);
            let mut drawn = GridDamage::new();
            let stride = self.width.max(0) as usize;
            self.terminal
                .render(&mut self.pixels, stride, PADDING, PADDING, &mut drawn);
            for rect in drawn.rects() {
                self.damage.add(Rect::new(
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as i32,
                    rect.height as i32,
                ));
            }
            if let Some((_, rect)) = self.indicator() {
                self.damage.add(rect);
            }
            if let Some(menu) = &self.menu {
                self.damage.add(menu.rect());
            }
            if let Some(panel) = &self.panel {
                self.damage.add(panel.rect(self.whole()));
            }
        }
        self.damage.take()
    }

    /// Draws the content: the cells, then the size indicator, the context menu and the close
    /// question over them.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        canvas.blit(&self.pixels, self.width.max(0) as usize);
        if let Some((text, rect)) = self.indicator() {
            canvas.rounded(rect, &INSET_R4, colour::MENU);
            canvas.outline(rect, &INSET_R4, 1, colour::MENU_BORDER);
            let mut x = rect.x + INDICATOR_PADDING;
            for character in text.as_str().chars() {
                let glyph = eggos_font::glyph_id(character as u32);
                canvas.mono_glyph(x, rect.y + INDICATOR_TEXT_TOP, glyph, colour::TEXT);
                x += CELL_WIDTH;
            }
        }
        if let Some(menu) = &self.menu {
            menu.paint(canvas);
        }
        if let Some(panel) = &self.panel {
            panel.paint(canvas, self.whole(), self.focused);
        }
    }

    fn selection_element(&self) -> u32 {
        SCREEN_ELEMENT + 1 + u32::from(self.terminal.size().1)
    }

    fn folder_element(&self) -> u32 {
        self.selection_element() + 1
    }

    /// Writes the screen for an agent (docs/ux/terminal-windows-v0.3.md section 6,
    /// docs/ux/terminal-selection-v0.3.md section 8): a `group` named `screen` with the grid's
    /// size, a `text` element per row shown, named `row <n>`, holding the row's characters, the
    /// cursor's row focused and rows with a selected cell selected; then, with a selection, a
    /// `text` named `selection` holding what a copy takes, and once a program reported its
    /// folder, a `text` named `folder`. The top rows are left out when they do not all fit.
    pub fn describe(&self, tree: &mut TreeWriter<'_>) {
        let (columns, rows) = self.terminal.size();
        let mut size = SmallText::default();
        let _ = write!(size, "{columns}x{rows}");
        let grid = Bounds::new(
            PADDING as i32,
            PADDING as i32,
            i32::from(columns) * CELL_WIDTH,
            i32::from(rows) * CELL_HEIGHT,
        );
        let mut screen = Element::new(
            SCREEN_ELEMENT,
            eggos_toolkit::tree::ROOT_ELEMENT,
            Role::Group,
            grid,
        );
        screen.name = b"screen";
        screen.value = size.as_bytes();
        if !tree.push(&screen) {
            return;
        }
        let copied = self.selected_text();
        let mut end = copied.len().min(MAX_TEXT_BYTES);
        while !copied.is_char_boundary(end) {
            end -= 1;
        }
        let copied = &copied.as_bytes()[..end];
        let folder = self.terminal.current_folder().map(str::as_bytes);
        let folder = folder.map(|folder| &folder[..folder.len().min(MAX_TEXT_BYTES)]);
        let mut extra = 0;
        if self.selection.is_some() {
            extra += HEADER_BYTES + "selection".len() + copied.len();
        }
        if let Some(folder) = folder {
            extra += HEADER_BYTES + "folder".len() + folder.len();
        }
        let mut text = [0u8; MAX_LABEL_BYTES];
        let room = MAX_TREE_BYTES
            .saturating_sub(tree.bytes().len())
            .saturating_sub(extra);
        let mut needed = 0;
        let mut first = rows;
        while first > 0 {
            let row = first - 1;
            let mut name = SmallText::default();
            let _ = write!(name, "row {}", row + 1);
            let length = HEADER_BYTES + name.length + self.terminal.row_text(row, &mut text);
            if needed + length > room {
                break;
            }
            needed += length;
            first = row;
        }
        let cursor_row = self.terminal.cursor_row();
        let highlight = self.terminal.highlight();
        for row in first..rows {
            let length = self.terminal.row_text(row, &mut text);
            let mut name = SmallText::default();
            let _ = write!(name, "row {}", row + 1);
            let bounds = Bounds::new(
                PADDING as i32,
                PADDING as i32 + i32::from(row) * CELL_HEIGHT,
                i32::from(columns) * CELL_WIDTH,
                CELL_HEIGHT,
            );
            let mut element = Element::new(
                SCREEN_ELEMENT + 1 + u32::from(row),
                SCREEN_ELEMENT,
                Role::Text,
                bounds,
            );
            element.name = name.as_bytes();
            element.value = &text[..length];
            if cursor_row == Some(row) {
                element.flags |= flags::FOCUSED;
            }
            let line = self.terminal.line_number_of_view_row(row);
            if highlight.is_some_and(|highlight| !highlight.columns_of(line, columns).is_empty()) {
                element.flags |= flags::SELECTED;
            }
            if !tree.push(&element) {
                return;
            }
        }
        if self.selection.is_some() {
            let mut element =
                Element::new(self.selection_element(), SCREEN_ELEMENT, Role::Text, grid);
            element.name = b"selection";
            element.value = copied;
            if !tree.push(&element) {
                return;
            }
        }
        if let Some(folder) = folder {
            let mut element = Element::new(self.folder_element(), SCREEN_ELEMENT, Role::Text, grid);
            element.name = b"folder";
            element.value = folder;
            let _ = tree.push(&element);
        }
    }

    /// What doing `action` to element `element` does: `select` on a row selects its logical
    /// line, on the screen selects all; everything else is not supported (an agent types with
    /// `wm type` and `wm press`, as a person does).
    pub fn act(&mut self, element: u32, action: Action) -> Outcome {
        let rows = u32::from(self.terminal.size().1);
        match (element, action) {
            (eggos_toolkit::tree::ROOT_ELEMENT, _) => Outcome::NotSupported,
            (SCREEN_ELEMENT, Action::Select) => match select_all(&self.terminal) {
                Some(selection) => {
                    self.set_selection(Some(selection));
                    Outcome::Done
                }
                None => Outcome::NotSupported,
            },
            (row, Action::Select) if (SCREEN_ELEMENT + 1..=SCREEN_ELEMENT + rows).contains(&row) => {
                let view_row = (row - SCREEN_ELEMENT - 1) as u16;
                let line = self.terminal.line_number_of_view_row(view_row);
                let (anchor, anchor_end) = logical_line_at(&self.terminal, line);
                self.set_selection(Some(Selection {
                    anchor,
                    anchor_end,
                    head: anchor,
                    mode: SelectionMode::Lines,
                }));
                Outcome::Done
            }
            (element, _) if (SCREEN_ELEMENT..=SCREEN_ELEMENT + rows).contains(&element) => {
                Outcome::NotSupported
            }
            (element, _)
                if (element == self.selection_element() && self.selection.is_some())
                    || (element == self.folder_element()
                        && self.terminal.current_folder().is_some()) =>
            {
                Outcome::NotSupported
            }
            _ => Outcome::NoSuchElement,
        }
    }
}
