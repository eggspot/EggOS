//! A terminal window's state (docs/ux/terminal-windows-v0.3.md): the engine fed the shell's
//! output, the grid that follows the window's size (and tells the shell, which gets `SIGWINCH`),
//! the cursor's look and blink, the size indicator while the grid changes, the question before
//! closing a window whose shell runs a program, the title, and the screen's rows as elements for
//! agents. The shell is behind [`Shell`], so all of it is host-tested.

use crate::keys::{KeyAction, key_action};
use crate::title::Title;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_element_tree::{
    Bounds, Element, HEADER_BYTES, MAX_LABEL_BYTES, MAX_TREE_BYTES, Outcome, Role, TreeWriter,
    flags,
};
use eggos_terminal::geometry::Damage as GridDamage;
use eggos_terminal::keymap::CursorKeys;
use eggos_terminal::{CursorLook, PADDING, TERM_BACKGROUND, Terminal, ViewMove, WHEEL_LINES};
use eggos_toolkit::canvas::{Canvas, INSET_R4};
use eggos_toolkit::colour;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, MouseButton};
use eggos_toolkit::panel::{Panel, PanelResponse};

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

/// The close question's first line.
pub const CLOSE_QUESTION: &str = "Close this terminal?";

/// The close question's second line.
pub const CLOSE_EXPLANATION: &str = "A program is still running in it and will be ended.";

/// The id of the element holding the screen's rows; the rows follow it.
pub const SCREEN_ELEMENT: u32 = 2;

const CELL_WIDTH: i32 = eggos_font::GLYPH_WIDTH as i32;
const CELL_HEIGHT: i32 = eggos_font::GLYPH_HEIGHT as i32;
const CANCEL: usize = 0;
const CLOSE: usize = 1;
const INDICATOR_HEIGHT: i32 = 24;
const INDICATOR_PADDING: i32 = 8;
const INDICATOR_TEXT_TOP: i32 = 4;
const READ_CHUNK: usize = 4096;

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

/// The shell on the other side of the pseudo-terminal, and what the window asks of the system.
pub trait Shell {
    /// Reads what the shell wrote into `buffer`, never waiting.
    fn read(&mut self, buffer: &mut [u8]) -> ShellOutput;
    /// Sends `bytes` to the shell as typed.
    fn write(&mut self, bytes: &[u8]);
    /// Tells the shell the grid is `columns` by `rows` cells (`TIOCSWINSZ`).
    fn set_window_size(&mut self, columns: u16, rows: u16);
    /// Whether the terminal's foreground process group is the shell's own: no program runs in
    /// the foreground.
    fn foreground_is_shell(&self) -> bool;
    /// Opens another terminal window.
    fn open_new_window(&mut self);
}

/// The grid a content of `width` by `height` (scale 1) holds: the cells inside the padding, at
/// least one and at most [`MAX_COLUMNS`] by [`MAX_ROWS`].
pub fn grid_for(width: i32, height: i32) -> (u16, u16) {
    let padding = 2 * PADDING as i32;
    let columns = ((width - padding) / CELL_WIDTH).clamp(1, i32::from(MAX_COLUMNS));
    let rows = ((height - padding) / CELL_HEIGHT).clamp(1, i32::from(MAX_ROWS));
    (columns as u16, rows as u16)
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
    closing: bool,
    now: u32,
}

impl<S: Shell> core::fmt::Debug for TerminalWindow<S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TerminalWindow")
            .field("terminal", &self.terminal)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("title", &self.title)
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
            closing: false,
            now: now_ms,
        })
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

    /// The content is now `width` by `height` (scale 1): the grid follows, the shell told and,
    /// but for the window's first size, the indicator shown when it changes; everything is drawn
    /// again.
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
        self.sized = true;
        self.terminal.invalidate();
        self.render_due = true;
        self.damage.add(self.whole());
    }

    /// Feeds the engine what the shell wrote until it has nothing more, answering what the
    /// engine replies (`CSI 6 n`) and taking the title a program set; `false` once the shell
    /// ended.
    pub fn read_shell(&mut self, now_ms: u32) -> bool {
        self.now = now_ms;
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match self.shell.read(&mut chunk) {
                ShellOutput::Bytes(count) => self.output(&chunk[..count]),
                ShellOutput::Nothing => return true,
                ShellOutput::Ended => {
                    self.closing = true;
                    return false;
                }
            }
        }
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
        self.active_at = self.now;
        self.render_due = true;
    }

    fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.move_view(ViewMove::Live);
        self.shell.write(bytes);
        self.active_at = self.now;
        self.render_due = true;
    }

    fn move_view(&mut self, change: ViewMove) {
        let before = self.terminal.view_back();
        self.terminal.move_view(change);
        if self.terminal.view_back() != before {
            self.render_due = true;
        }
    }

    /// Acts on `input` at `time_ms`; `false` once the window should close.
    pub fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        self.now = time_ms;
        if self.panel.is_some() {
            return self.handle_question(input);
        }
        match input {
            Input::Key {
                code,
                pressed: true,
                modifiers,
                character,
            } => {
                let cursor_keys = match self.terminal.application_cursor_keys() {
                    true => CursorKeys::Application,
                    false => CursorKeys::Normal,
                };
                match key_action(code, modifiers, character, cursor_keys) {
                    KeyAction::Send(bytes) => self.send(bytes.as_bytes()),
                    KeyAction::View(change) => self.move_view(change),
                    KeyAction::NewWindow => self.shell.open_new_window(),
                    KeyAction::Close => {
                        self.close_requested(time_ms);
                    }
                    KeyAction::Nothing => {}
                }
            }
            Input::Wheel { notches, .. } => {
                let lines = notches.unsigned_abs() as usize * WHEEL_LINES;
                self.move_view(match notches > 0 {
                    true => ViewMove::Back(lines),
                    false => ViewMove::Forward(lines),
                });
            }
            Input::FocusIn | Input::FocusOut => {
                self.focused = input == Input::FocusIn;
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
    /// program in the foreground; otherwise the close question shows and `false` keeps it open.
    pub fn close_requested(&mut self, _time_ms: u32) -> bool {
        if self.closing || self.shell.foreground_is_shell() {
            self.closing = true;
            return true;
        }
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

    /// When the window next wants [`Self::tick`]: the cursor's next blink, the indicator's end.
    pub fn deadline(&self, now_ms: u32) -> Option<u32> {
        let (_, blink) = self.look_at(now_ms);
        match (blink, self.indicator_until) {
            (Some(blink), Some(until)) => {
                let wait = |at: u32| at.wrapping_sub(now_ms) as i32;
                Some(if wait(blink) <= wait(until) {
                    blink
                } else {
                    until
                })
            }
            (deadline, None) | (None, deadline) => deadline,
        }
    }

    /// Looks at the clock: the cursor's blink, the indicator's end.
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
            if let Some(panel) = &self.panel {
                self.damage.add(panel.rect(self.whole()));
            }
        }
        self.damage.take()
    }

    /// Draws the content: the cells, then the size indicator and the close question over them.
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
        if let Some(panel) = &self.panel {
            panel.paint(canvas, self.whole(), self.focused);
        }
    }

    /// Writes the screen for an agent (docs/ux/terminal-windows-v0.3.md section 6): a `group`
    /// named `screen` with the grid's size, and a `text` element per row shown, named
    /// `row <n>`, holding the row's characters, the cursor's row focused; the top rows are left
    /// out when they do not all fit the tree.
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
        let mut text = [0u8; MAX_LABEL_BYTES];
        let room = MAX_TREE_BYTES.saturating_sub(tree.bytes().len());
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
                element.flags = flags::FOCUSED;
            }
            if !tree.push(&element) {
                return;
            }
        }
    }

    /// What doing an action to element `element` does: nothing the screen's elements take (an
    /// agent types with `wm type` and `wm press`, as a person does).
    pub fn act(&self, element: u32) -> Outcome {
        let rows = u32::from(self.terminal.size().1);
        match element {
            eggos_toolkit::tree::ROOT_ELEMENT => Outcome::NotSupported,
            element if (SCREEN_ELEMENT..=SCREEN_ELEMENT + rows).contains(&element) => {
                Outcome::NotSupported
            }
            _ => Outcome::NoSuchElement,
        }
    }
}
