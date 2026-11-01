//! The terminal emulator of docs/ux/terminal-v0.3.md (D109, D114), shared by the kernel's
//! console terminal and the terminal program (D187): a grid of cells fed the bytes programs
//! write, the ANSI subset the shell, the core utilities and a full-screen editor need, a
//! scrollback, and rendering of only what changed into a rectangle of a framebuffer; with the
//! rectangles and damage it reports ([`geometry`]) and the keymap that turns keys into the bytes
//! a terminal sends ([`keymap`], [`key`]).
//!
//! [`Terminal::feed`] parses bytes and updates the cells, recording which changed; it never
//! draws. [`Terminal::render`] draws the changed cells and the cursor, moving pixels instead of
//! redrawing them when the screen scrolled, and reports the rectangles it changed. Replies a
//! program asked for (`CSI 6 n`, `CSI c`) wait in [`Terminal::replies`] for the input side.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod colours;
pub mod geometry;
pub mod key;
pub mod keymap;
mod parser;

#[cfg(test)]
mod tests;

pub use colours::{
    ACCENT, ANSI_COLOURS, DEFAULT_COLOUR, TERM_BACKGROUND, TERM_CURSOR, TERM_FOREGROUND,
};

use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_font::{self as font, GLYPH_HEIGHT, GLYPH_WIDTH};
use geometry::{Damage, Rect};
use parser::Parser;

/// Lines the scrollback keeps; older ones are dropped.
pub const HISTORY_LINES: usize = 256;
/// Pixels between the edge of the terminal's content and its grid of cells.
pub const PADDING: u32 = 8;
/// Columns between tab stops.
pub const TAB_WIDTH: u16 = 8;
/// Lines a wheel notch moves the scrollback view.
pub const WHEEL_LINES: usize = 3;
/// Bytes of replies kept for the input side; more are dropped.
pub const REPLY_CAPACITY: usize = 64;
/// Bytes of a title a program sets that are kept ([`Terminal::title`]); more are dropped.
pub const TITLE_CAPACITY: usize = 64;

/// One character cell: a glyph id ([`eggos_font`]), two colours (0-15 the ANSI colours,
/// [`DEFAULT_COLOUR`] the terminal's own) and [`cell_flags`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub glyph: u8,
    pub foreground: u8,
    pub background: u8,
    pub flags: u8,
}

/// The bits of [`Cell::flags`].
pub mod cell_flags {
    pub const BOLD: u8 = 1;
    pub const UNDERLINE: u8 = 2;
    pub const INVERSE: u8 = 4;
    pub const WIDE_CONTINUATION: u8 = 8;
}

impl Cell {
    /// A space in the default colours.
    pub const BLANK: Cell = Cell {
        glyph: b' ',
        foreground: DEFAULT_COLOUR,
        background: DEFAULT_COLOUR,
        flags: 0,
    };

    /// Its foreground and background as pixels: bold brightens ANSI colours 0-7, inverse swaps
    /// the two.
    pub fn colours(self) -> (u32, u32) {
        let mut foreground = self.foreground;
        if self.flags & cell_flags::BOLD != 0 && foreground < 8 {
            foreground += 8;
        }
        let foreground = colours::pixel(foreground, TERM_FOREGROUND);
        let background = colours::pixel(self.background, TERM_BACKGROUND);
        if self.flags & cell_flags::INVERSE != 0 {
            (background, foreground)
        } else {
            (foreground, background)
        }
    }
}

/// Where a terminal's grid sits on a screen: its top-left pixel and its size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextArea {
    pub x: u32,
    pub y: u32,
    pub columns: u16,
    pub rows: u16,
}

impl TextArea {
    /// The grid of a terminal that fills a `width` by `height` screen: as many cells as fit
    /// inside the [`PADDING`], what is left over going to the right and bottom; `None` when not
    /// one cell fits.
    pub fn full_screen(width: u32, height: u32) -> Option<Self> {
        let columns = width.saturating_sub(2 * PADDING) / GLYPH_WIDTH as u32;
        let rows = height.saturating_sub(2 * PADDING) / GLYPH_HEIGHT as u32;
        (columns > 0 && rows > 0).then(|| Self {
            x: PADDING,
            y: PADDING,
            columns: columns.min(u32::from(u16::MAX)) as u16,
            rows: rows.min(u32::from(u16::MAX)) as u16,
        })
    }

    /// The grid's pixels.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            u32::from(self.columns) * GLYPH_WIDTH as u32,
            u32::from(self.rows) * GLYPH_HEIGHT as u32,
        )
    }
}

/// How [`Terminal::render`] draws the cursor (docs/ux/terminal-v0.3.md, "Cursor").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorLook {
    /// A block in [`TERM_CURSOR`] with the cell's glyph in [`TERM_BACKGROUND`] on it: a focused
    /// terminal, and the first half of each blink.
    #[default]
    Block,
    /// The cell as it is, with no cursor: the second half of each blink.
    Blinked,
    /// The cell as it is inside a 1 px [`TERM_CURSOR`] outline: a terminal without focus.
    Outline,
}

/// A move of the scrollback view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMove {
    /// Toward older lines, by this many.
    Back(usize),
    /// Toward the live screen, by this many.
    Forward(usize),
    /// Back by a page: the grid's rows less one.
    PageBack,
    /// Forward by a page.
    PageForward,
    /// To the oldest line kept.
    Oldest,
    /// To the live screen.
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attributes {
    foreground: u8,
    background: u8,
    flags: u8,
}

impl Attributes {
    const DEFAULT: Attributes = Attributes {
        foreground: DEFAULT_COLOUR,
        background: DEFAULT_COLOUR,
        flags: 0,
    };

    fn cell(self, glyph: u8) -> Cell {
        Cell {
            glyph,
            foreground: self.foreground,
            background: self.background,
            flags: self.flags,
        }
    }

    fn erased(self) -> Cell {
        Cell {
            background: self.background,
            ..Cell::BLANK
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SavedCursor {
    row: u16,
    column: u16,
    attributes: Attributes,
    pending_wrap: bool,
    origin: bool,
}

impl SavedCursor {
    const HOME: SavedCursor = SavedCursor {
        row: 0,
        column: 0,
        attributes: Attributes::DEFAULT,
        pending_wrap: false,
        origin: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Modes {
    autowrap: bool,
    origin: bool,
    cursor_visible: bool,
    cursor_blinking: bool,
    application_cursor_keys: bool,
}

impl Modes {
    const DEFAULT: Modes = Modes {
        autowrap: true,
        origin: false,
        cursor_visible: true,
        cursor_blinking: true,
        application_cursor_keys: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    start: u16,
    end: u16,
}

impl Span {
    const CLEAN: Span = Span {
        start: u16::MAX,
        end: 0,
    };

    fn is_clean(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shift {
    top: u16,
    bottom: u16,
    lines: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TitleText {
    bytes: [u8; TITLE_CAPACITY],
    length: u8,
    cut: bool,
}

impl TitleText {
    pub(crate) const EMPTY: TitleText = TitleText {
        bytes: [0; TITLE_CAPACITY],
        length: 0,
        cut: false,
    };

    pub(crate) fn push(&mut self, byte: u8) {
        let length = usize::from(self.length);
        if length < TITLE_CAPACITY {
            self.bytes[length] = byte;
            self.length += 1;
        } else {
            self.cut = true;
        }
    }
}

const MAIN: usize = 0;
const ALTERNATE: usize = 1;

/// A terminal: its screens, scrollback, cursor, modes and parser, and what changed since it
/// was last rendered.
pub struct Terminal {
    columns: u16,
    rows: u16,
    screens: [Box<[Cell]>; 2],
    active: usize,
    history: Box<[Cell]>,
    history_start: usize,
    history_length: usize,
    view_back: usize,
    row: u16,
    column: u16,
    pending_wrap: bool,
    attributes: Attributes,
    saved: [SavedCursor; 2],
    position_saved: (u16, u16),
    top: u16,
    bottom: u16,
    modes: Modes,
    onlcr: bool,
    parser: Parser,
    dirty: Box<[Span]>,
    shift: Option<Shift>,
    drawn_cursor: Option<(u16, u16)>,
    cursor_look: CursorLook,
    replies: [u8; REPLY_CAPACITY],
    reply_length: usize,
    title: TitleText,
    title_changed: bool,
}

impl core::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Terminal")
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("cursor", &(self.row, self.column))
            .field("alternate", &(self.active == ALTERNATE))
            .field("history_length", &self.history_length)
            .finish_non_exhaustive()
    }
}

fn filled<T: Copy>(count: usize, value: T) -> Option<Box<[T]>> {
    let mut items = Vec::new();
    items.try_reserve_exact(count).ok()?;
    items.resize(count, value);
    Some(items.into_boxed_slice())
}

impl Terminal {
    /// A blank terminal of `columns` by `rows` cells with an empty scrollback, everything to be
    /// drawn; `None` when there is no memory for it or it has no cell.
    pub fn new(columns: u16, rows: u16) -> Option<Self> {
        if columns == 0 || rows == 0 {
            return None;
        }
        let cells = usize::from(columns) * usize::from(rows);
        let mut terminal = Self {
            columns,
            rows,
            screens: [filled(cells, Cell::BLANK)?, filled(cells, Cell::BLANK)?],
            active: MAIN,
            history: filled(HISTORY_LINES * usize::from(columns), Cell::BLANK)?,
            history_start: 0,
            history_length: 0,
            view_back: 0,
            row: 0,
            column: 0,
            pending_wrap: false,
            attributes: Attributes::DEFAULT,
            saved: [SavedCursor::HOME; 2],
            position_saved: (0, 0),
            top: 0,
            bottom: rows - 1,
            modes: Modes::DEFAULT,
            onlcr: false,
            parser: Parser::new(),
            dirty: filled(usize::from(rows), Span::CLEAN)?,
            shift: None,
            drawn_cursor: None,
            cursor_look: CursorLook::Block,
            replies: [0; REPLY_CAPACITY],
            reply_length: 0,
            title: TitleText::EMPTY,
            title_changed: false,
        };
        terminal.invalidate();
        Some(terminal)
    }

    /// Its size: columns, then rows.
    pub fn size(&self) -> (u16, u16) {
        (self.columns, self.rows)
    }

    /// The cursor: row, then column, both from 0.
    pub fn cursor(&self) -> (u16, u16) {
        (self.row, self.column)
    }

    /// Whether a program asked for the cursor to be shown (`CSI ? 25 h`, the default).
    pub fn cursor_visible(&self) -> bool {
        self.modes.cursor_visible
    }

    /// Whether a program asked for a blinking cursor (`CSI ? 12 h`, the default).
    pub fn cursor_blinking(&self) -> bool {
        self.modes.cursor_blinking
    }

    /// How the cursor is drawn.
    pub fn cursor_look(&self) -> CursorLook {
        self.cursor_look
    }

    /// Draws the cursor as `look` from the next [`Self::render`] on, which always draws the
    /// cursor's cell.
    pub fn set_cursor_look(&mut self, look: CursorLook) {
        self.cursor_look = look;
    }

    /// Whether the cursor blinks where it is drawn: shown (`CSI ? 25 h`), blinking (`CSI ? 12
    /// h`) and on the live screen, not scrolled back.
    pub fn cursor_blinks(&self) -> bool {
        self.modes.cursor_blinking && self.shown_cursor().is_some()
    }

    /// Whether the arrow keys send their application forms (`CSI ? 1 h`).
    pub fn application_cursor_keys(&self) -> bool {
        self.modes.application_cursor_keys
    }

    /// Whether the alternate screen is shown.
    pub fn on_alternate_screen(&self) -> bool {
        self.active == ALTERNATE
    }

    /// Has a line feed also return the carriage, as a console's output processing (`ONLCR`)
    /// does before the bytes reach a serial terminal.
    pub fn set_onlcr(&mut self, on: bool) {
        self.onlcr = on;
    }

    /// The cells of row `row` of the live screen shown now.
    pub fn line(&self, row: u16) -> &[Cell] {
        let start = usize::from(row) * usize::from(self.columns);
        &self.screens[self.active][start..start + usize::from(self.columns)]
    }

    /// How many lines the scrollback holds.
    pub fn history_length(&self) -> usize {
        self.history_length
    }

    /// The scrollback's line `index`, 0 the oldest.
    pub fn history_line(&self, index: usize) -> &[Cell] {
        let slot = (self.history_start + index) % HISTORY_LINES;
        let start = slot * usize::from(self.columns);
        &self.history[start..start + usize::from(self.columns)]
    }

    /// How many lines back from the live screen the view is.
    pub fn view_back(&self) -> usize {
        self.view_back
    }

    /// Bytes the terminal answers programs with, for the console's input.
    pub fn replies(&self) -> &[u8] {
        &self.replies[..self.reply_length]
    }

    /// Forgets the replies once they are taken.
    pub fn clear_replies(&mut self) {
        self.reply_length = 0;
    }

    /// The title a program last set with xterm's `OSC 0` or `OSC 2` (`ESC ] 2 ; text BEL`, or
    /// `ESC \` at its end), empty when none did or one set it empty: its bytes but control
    /// characters, at most [`TITLE_CAPACITY`], and whether it was longer.
    pub fn title(&self) -> (&[u8], bool) {
        (
            &self.title.bytes[..usize::from(self.title.length)],
            self.title.cut,
        )
    }

    /// Whether a program set the title since the last call.
    pub fn take_title_change(&mut self) -> bool {
        core::mem::take(&mut self.title_changed)
    }

    /// The characters row `row` of the view shows (the scrollback while scrolled back), as
    /// UTF-8 written into `out` with the trailing spaces left out; returns how many bytes, cut
    /// before the first character that does not fit. Nothing for a row past the grid.
    pub fn row_text(&self, row: u16, out: &mut [u8]) -> usize {
        if row >= self.rows {
            return 0;
        }
        let line = self.visible_line(row);
        let end = line
            .iter()
            .rposition(|cell| cell.glyph != b' ')
            .map_or(0, |last| last + 1);
        let mut length = 0;
        for cell in &line[..end] {
            let mut character = [0u8; 4];
            let encoded = font::character(cell.glyph)
                .encode_utf8(&mut character)
                .as_bytes();
            let Some(room) = out.get_mut(length..length + encoded.len()) else {
                break;
            };
            room.copy_from_slice(encoded);
            length += encoded.len();
        }
        length
    }

    /// The row the cursor is drawn on: `None` while it is hidden or the view is scrolled back.
    pub fn cursor_row(&self) -> Option<u16> {
        self.shown_cursor().map(|(row, _)| row)
    }

    /// Changes the grid to `columns` by `rows` cells, as the terminal program does when its
    /// window's size changes (docs/ux/terminal-windows-v0.3.md section 3, D187), with no reflow:
    /// cells past a narrower width are cut from every row and the scrollback for good; a shorter
    /// grid drops the blank rows below the cursor first, then moves rows from the top into the
    /// scrollback, the cursor moving up with them; a taller one adds blank rows at the bottom.
    /// The alternate screen is cleared, the scrolling region is the whole grid again, the view
    /// goes back to the live screen and everything is drawn at the next render. Returns `false`,
    /// changing nothing, for a size of no cell or without memory for the new one.
    pub fn resize(&mut self, columns: u16, rows: u16) -> bool {
        if columns == 0 || rows == 0 {
            return false;
        }
        if (columns, rows) == (self.columns, self.rows) {
            return true;
        }
        let cells = usize::from(columns) * usize::from(rows);
        let (Some(mut main), Some(alternate), Some(mut history), Some(dirty)) = (
            filled(cells, Cell::BLANK),
            filled(cells, Cell::BLANK),
            filled(HISTORY_LINES * usize::from(columns), Cell::BLANK),
            filled(usize::from(rows), Span::CLEAN),
        ) else {
            return false;
        };
        let main_cursor_row = match self.active {
            MAIN => self.row,
            _ => self.saved[MAIN].row,
        };
        let excess = self.rows.saturating_sub(rows);
        let blank_below = (main_cursor_row.saturating_add(1)..self.rows)
            .rev()
            .take_while(|&row| {
                self.line_of(MAIN, row)
                    .iter()
                    .all(|cell| *cell == Cell::BLANK)
            })
            .count() as u16;
        let dropped_below = excess.min(blank_below);
        let to_history = excess - dropped_below;
        let kept = self.rows - excess;
        let width = usize::from(columns);
        let copied = usize::from(columns.min(self.columns));
        for row in 0..kept {
            let from = self.line_of(MAIN, row + to_history);
            let start = usize::from(row) * width;
            main[start..start + copied].copy_from_slice(&from[..copied]);
        }
        let lines = self.history_length + usize::from(to_history);
        let skipped = lines.saturating_sub(HISTORY_LINES);
        for index in skipped..lines {
            let from = match index < self.history_length {
                true => self.history_line(index),
                false => self.line_of(MAIN, (index - self.history_length) as u16),
            };
            let start = (index - skipped) * width;
            history[start..start + copied].copy_from_slice(&from[..copied]);
        }
        self.history_start = 0;
        self.history_length = lines - skipped;
        self.screens = [main, alternate];
        self.history = history;
        self.dirty = dirty;
        let (last_row, last_column) = (rows - 1, columns - 1);
        match self.active {
            MAIN => self.row = self.row.saturating_sub(to_history).min(last_row),
            _ => self.row = self.row.min(last_row),
        }
        self.column = self.column.min(last_column);
        let main_saved = &mut self.saved[MAIN];
        main_saved.row = main_saved.row.saturating_sub(to_history).min(last_row);
        main_saved.column = main_saved.column.min(last_column);
        let alternate_saved = &mut self.saved[ALTERNATE];
        alternate_saved.row = alternate_saved.row.min(last_row);
        alternate_saved.column = alternate_saved.column.min(last_column);
        self.position_saved = (
            self.position_saved.0.min(last_row),
            self.position_saved.1.min(last_column),
        );
        self.columns = columns;
        self.rows = rows;
        self.top = 0;
        self.bottom = last_row;
        self.pending_wrap = false;
        self.view_back = 0;
        self.invalidate();
        true
    }

    fn line_of(&self, screen: usize, row: u16) -> &[Cell] {
        let start = usize::from(row) * usize::from(self.columns);
        &self.screens[screen][start..start + usize::from(self.columns)]
    }

    /// Whether something changed since the last [`Self::render`].
    pub fn needs_render(&self) -> bool {
        self.shift.is_some() || self.dirty.iter().any(|span| !span.is_clean())
    }

    /// Records that the grid's pixels already show it blank in [`TERM_BACKGROUND`] (the screen
    /// was just filled), so the next render draws only what changed since and the cursor.
    pub fn assume_drawn_blank(&mut self) {
        self.dirty.fill(Span::CLEAN);
        self.shift = None;
        self.drawn_cursor = None;
    }

    /// Marks every cell as changed, so the next render draws the whole grid.
    pub fn invalidate(&mut self) {
        self.dirty.fill(Span {
            start: 0,
            end: self.columns,
        });
        self.shift = None;
        self.drawn_cursor = None;
    }

    /// Takes `bytes` a program wrote.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            parser::feed(self, byte);
        }
    }

    /// Moves the scrollback view; the alternate screen has none.
    pub fn move_view(&mut self, change: ViewMove) {
        let oldest = if self.active == MAIN {
            self.history_length
        } else {
            0
        };
        let page = usize::from(self.rows - 1).max(1);
        let back = match change {
            ViewMove::Back(lines) => self.view_back.saturating_add(lines),
            ViewMove::Forward(lines) => self.view_back.saturating_sub(lines),
            ViewMove::PageBack => self.view_back.saturating_add(page),
            ViewMove::PageForward => self.view_back.saturating_sub(page),
            ViewMove::Oldest => oldest,
            ViewMove::Live => 0,
        }
        .min(oldest);
        if back != self.view_back {
            self.view_back = back;
            self.invalidate();
        }
    }

    fn index(&self, row: u16, column: u16) -> usize {
        usize::from(row) * usize::from(self.columns) + usize::from(column)
    }

    fn screen(&mut self) -> &mut [Cell] {
        &mut self.screens[self.active]
    }

    fn touch(&mut self, row: u16, start: u16, end: u16) {
        let span = &mut self.dirty[usize::from(row)];
        span.start = span.start.min(start);
        span.end = span.end.max(end);
    }

    fn touch_rows(&mut self, first: u16, last: u16) {
        let full = Span {
            start: 0,
            end: self.columns,
        };
        self.dirty[usize::from(first)..=usize::from(last)].fill(full);
    }

    fn reply(&mut self, bytes: &[u8]) {
        let end = self.reply_length + bytes.len();
        if end <= REPLY_CAPACITY {
            self.replies[self.reply_length..end].copy_from_slice(bytes);
            self.reply_length = end;
        }
    }

    fn print(&mut self, glyph: u8) {
        if self.pending_wrap {
            self.pending_wrap = false;
            if self.modes.autowrap {
                self.column = 0;
                self.index_down();
            }
        }
        let (row, column) = (self.row, self.column);
        let cell = self.attributes.cell(glyph);
        let index = self.index(row, column);
        self.screen()[index] = cell;
        self.touch(row, column, column + 1);
        if column + 1 < self.columns {
            self.column += 1;
        } else if self.modes.autowrap {
            self.pending_wrap = true;
        }
    }

    fn carriage_return(&mut self) {
        self.column = 0;
        self.pending_wrap = false;
    }

    fn line_feed(&mut self) {
        if self.onlcr {
            self.column = 0;
        }
        self.index_down();
    }

    fn index_down(&mut self) {
        self.pending_wrap = false;
        if self.row == self.bottom {
            self.scroll_up(self.top, self.bottom, 1, true);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.row == self.top {
            self.scroll_down(self.top, self.bottom, 1);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }

    fn backspace(&mut self) {
        self.pending_wrap = false;
        self.column = self.column.saturating_sub(1);
    }

    fn tab(&mut self) {
        self.pending_wrap = false;
        let next = (self.column / TAB_WIDTH + 1).saturating_mul(TAB_WIDTH);
        self.column = next.min(self.columns - 1);
    }

    fn push_history(&mut self, row: u16) {
        let columns = usize::from(self.columns);
        let slot = if self.history_length < HISTORY_LINES {
            self.history_length += 1;
            (self.history_start + self.history_length - 1) % HISTORY_LINES
        } else {
            let oldest = self.history_start;
            self.history_start = (self.history_start + 1) % HISTORY_LINES;
            oldest
        };
        let from = usize::from(row) * columns;
        self.history[slot * columns..(slot + 1) * columns]
            .copy_from_slice(&self.screens[self.active][from..from + columns]);
    }

    fn scroll_up(&mut self, top: u16, bottom: u16, lines: u16, to_history: bool) {
        let height = bottom - top + 1;
        let lines = lines.min(height);
        let to_history = to_history && top == 0 && self.active == MAIN;
        if to_history {
            for row in top..top + lines {
                self.push_history(row);
            }
            if self.view_back > 0 {
                self.view_back = (self.view_back + usize::from(lines)).min(self.history_length);
            }
        }
        let columns = usize::from(self.columns);
        let erased = self.attributes.erased();
        let (first, end) = (
            usize::from(top) * columns,
            usize::from(bottom + 1) * columns,
        );
        let moved = usize::from(lines) * columns;
        let screen = self.screen();
        screen.copy_within(first + moved..end, first);
        screen[end - moved..end].fill(erased);
        self.record_scroll(top, bottom, lines);
    }

    fn record_scroll(&mut self, top: u16, bottom: u16, lines: u16) {
        let height = bottom - top + 1;
        let shiftable = match self.shift {
            None => true,
            Some(shift) => shift.top == top && shift.bottom == bottom,
        };
        if lines >= height || !shiftable || self.view_back > 0 {
            self.touch_rows(top, bottom);
            return;
        }
        let total = self.shift.map_or(0, |shift| shift.lines) + lines;
        self.shift = Some(Shift {
            top,
            bottom,
            lines: total.min(height),
        });
        let (top_index, bottom_index) = (usize::from(top), usize::from(bottom));
        self.dirty
            .copy_within(top_index + usize::from(lines)..=bottom_index, top_index);
        self.touch_rows(bottom + 1 - lines, bottom);
        if let Some((row, column)) = self.drawn_cursor
            && (top..=bottom).contains(&row)
        {
            self.drawn_cursor = (row >= top + lines).then(|| (row - lines, column));
        }
    }

    fn scroll_down(&mut self, top: u16, bottom: u16, lines: u16) {
        let height = bottom - top + 1;
        let lines = lines.min(height);
        let columns = usize::from(self.columns);
        let erased = self.attributes.erased();
        let (first, end) = (
            usize::from(top) * columns,
            usize::from(bottom + 1) * columns,
        );
        let moved = usize::from(lines) * columns;
        let screen = self.screen();
        screen.copy_within(first..end - moved, first + moved);
        screen[first..first + moved].fill(erased);
        self.touch_rows(top, bottom);
    }

    fn erase_cells(&mut self, row: u16, start: u16, end: u16) {
        let end = end.min(self.columns);
        if start >= end {
            return;
        }
        let erased = self.attributes.erased();
        let (from, to) = (self.index(row, start), self.index(row, end - 1) + 1);
        self.screen()[from..to].fill(erased);
        self.touch(row, start, end);
    }

    fn erase_rows(&mut self, first: u16, last: u16) {
        for row in first..=last {
            self.erase_cells(row, 0, self.columns);
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        let (row, column) = (self.row, self.column);
        match mode {
            0 => {
                self.erase_cells(row, column, self.columns);
                if row + 1 < self.rows {
                    self.erase_rows(row + 1, self.rows - 1);
                }
            }
            1 => {
                if row > 0 {
                    self.erase_rows(0, row - 1);
                }
                self.erase_cells(row, 0, column + 1);
            }
            2 => self.erase_rows(0, self.rows - 1),
            3 => {
                self.erase_rows(0, self.rows - 1);
                self.history_start = 0;
                self.history_length = 0;
                self.move_view(ViewMove::Live);
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let (row, column) = (self.row, self.column);
        match mode {
            0 => self.erase_cells(row, column, self.columns),
            1 => self.erase_cells(row, 0, column + 1),
            2 => self.erase_cells(row, 0, self.columns),
            _ => {}
        }
    }

    fn insert_characters(&mut self, count: u16) {
        let (row, column) = (self.row, self.column);
        let count = count.min(self.columns - column);
        let (from, end) = (
            self.index(row, column),
            self.index(row, self.columns - 1) + 1,
        );
        let erased = self.attributes.erased();
        let screen = self.screen();
        screen.copy_within(from..end - usize::from(count), from + usize::from(count));
        screen[from..from + usize::from(count)].fill(erased);
        self.pending_wrap = false;
        self.touch(row, column, self.columns);
    }

    fn delete_characters(&mut self, count: u16) {
        let (row, column) = (self.row, self.column);
        let count = count.min(self.columns - column);
        let (from, end) = (
            self.index(row, column),
            self.index(row, self.columns - 1) + 1,
        );
        let erased = self.attributes.erased();
        let screen = self.screen();
        screen.copy_within(from + usize::from(count)..end, from);
        screen[end - usize::from(count)..end].fill(erased);
        self.pending_wrap = false;
        self.touch(row, column, self.columns);
    }

    fn erase_characters(&mut self, count: u16) {
        let (row, column) = (self.row, self.column);
        self.erase_cells(row, column, column.saturating_add(count));
        self.pending_wrap = false;
    }

    fn insert_lines(&mut self, count: u16) {
        if (self.top..=self.bottom).contains(&self.row) {
            self.scroll_down(self.row, self.bottom, count);
            self.carriage_return();
        }
    }

    fn delete_lines(&mut self, count: u16) {
        if (self.top..=self.bottom).contains(&self.row) {
            self.scroll_up(self.row, self.bottom, count, false);
            self.carriage_return();
        }
    }

    fn cursor_up(&mut self, count: u16) {
        let limit = if self.row >= self.top { self.top } else { 0 };
        self.row = self.row.saturating_sub(count).max(limit);
        self.pending_wrap = false;
    }

    fn cursor_down(&mut self, count: u16) {
        let limit = if self.row <= self.bottom {
            self.bottom
        } else {
            self.rows - 1
        };
        self.row = self.row.saturating_add(count).min(limit);
        self.pending_wrap = false;
    }

    fn cursor_right(&mut self, count: u16) {
        self.column = self.column.saturating_add(count).min(self.columns - 1);
        self.pending_wrap = false;
    }

    fn cursor_left(&mut self, count: u16) {
        self.column = self.column.saturating_sub(count);
        self.pending_wrap = false;
    }

    fn set_column(&mut self, column: u16) {
        self.column = column.saturating_sub(1).min(self.columns - 1);
        self.pending_wrap = false;
    }

    fn set_row(&mut self, row: u16) {
        let row = row.saturating_sub(1);
        self.row = if self.modes.origin {
            self.top.saturating_add(row).min(self.bottom)
        } else {
            row.min(self.rows - 1)
        };
        self.pending_wrap = false;
    }

    fn set_position(&mut self, row: u16, column: u16) {
        self.set_row(row);
        self.set_column(column);
    }

    fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let top = top.max(1);
        let bottom = if bottom == 0 { self.rows } else { bottom };
        if top < bottom && bottom <= self.rows {
            self.top = top - 1;
            self.bottom = bottom - 1;
            self.set_position(1, 1);
        }
    }

    fn save_cursor(&mut self) {
        self.saved[self.active] = SavedCursor {
            row: self.row,
            column: self.column,
            attributes: self.attributes,
            pending_wrap: self.pending_wrap,
            origin: self.modes.origin,
        };
    }

    fn restore_cursor(&mut self) {
        let saved = self.saved[self.active];
        self.row = saved.row.min(self.rows - 1);
        self.column = saved.column.min(self.columns - 1);
        self.attributes = saved.attributes;
        self.pending_wrap = saved.pending_wrap;
        self.modes.origin = saved.origin;
        self.keep_inside_region_in_origin_mode();
    }

    fn restore_position(&mut self) {
        let (row, column) = self.position_saved;
        self.row = row.min(self.rows - 1);
        self.column = column.min(self.columns - 1);
        self.pending_wrap = false;
        self.keep_inside_region_in_origin_mode();
    }

    fn keep_inside_region_in_origin_mode(&mut self) {
        if self.modes.origin {
            self.row = self.row.clamp(self.top, self.bottom);
        }
    }

    fn report_position(&mut self) {
        let row = if self.modes.origin {
            self.row.saturating_sub(self.top)
        } else {
            self.row
        };
        let mut reply = [0u8; 16];
        let mut length = 0;
        let mut push = |byte: u8| {
            reply[length] = byte;
            length += 1;
        };
        push(0x1b);
        push(b'[');
        for (index, value) in [row + 1, self.column + 1].into_iter().enumerate() {
            if index == 1 {
                push(b';');
            }
            let mut digits = [0u8; 5];
            let mut count = 0;
            let mut rest = value;
            loop {
                digits[count] = b'0' + (rest % 10) as u8;
                count += 1;
                rest /= 10;
                if rest == 0 {
                    break;
                }
            }
            for &digit in digits[..count].iter().rev() {
                push(digit);
            }
        }
        push(b'R');
        self.reply(&reply[..length]);
    }

    fn use_alternate_screen(&mut self, alternate: bool, with_cursor: bool) {
        if alternate == (self.active == ALTERNATE) {
            return;
        }
        if alternate {
            if with_cursor {
                self.save_cursor();
            }
            self.active = ALTERNATE;
            self.screens[ALTERNATE].fill(Cell::BLANK);
        } else {
            self.active = MAIN;
            if with_cursor {
                self.restore_cursor();
            }
        }
        self.view_back = 0;
        self.invalidate();
    }

    fn reset(&mut self) {
        self.active = MAIN;
        self.screens[MAIN].fill(Cell::BLANK);
        self.row = 0;
        self.column = 0;
        self.pending_wrap = false;
        self.attributes = Attributes::DEFAULT;
        self.saved = [SavedCursor::HOME; 2];
        self.position_saved = (0, 0);
        self.top = 0;
        self.bottom = self.rows - 1;
        self.modes = Modes::DEFAULT;
        self.view_back = 0;
        self.invalidate();
    }

    fn visible_line(&self, row: u16) -> &[Cell] {
        if self.view_back == 0 {
            return self.line(row);
        }
        let index = self.history_length - self.view_back + usize::from(row);
        if index < self.history_length {
            self.history_line(index)
        } else {
            self.line((index - self.history_length) as u16)
        }
    }

    fn shown_cursor(&self) -> Option<(u16, u16)> {
        (self.modes.cursor_visible && self.view_back == 0).then_some((self.row, self.column))
    }

    /// Draws what changed since the last render into `pixels`, rows `stride` pixels apart, with
    /// the grid's top-left pixel at (`x`, `y`), and adds the rectangles it changed to `damage`.
    /// Draws nothing when the grid does not fit in `pixels`.
    pub fn render(
        &mut self,
        pixels: &mut [u32],
        stride: usize,
        x: u32,
        y: u32,
        damage: &mut Damage,
    ) {
        let area = TextArea {
            x,
            y,
            columns: self.columns,
            rows: self.rows,
        }
        .rect();
        let fits = area.right() as usize <= stride
            && (area.bottom() as usize).saturating_mul(stride) <= pixels.len();
        if !fits {
            return;
        }
        if let Some(shift) = self.shift.take() {
            self.shift_pixels(pixels, stride, area, shift);
            damage.add(Rect::new(
                area.x,
                area.y + u32::from(shift.top) * GLYPH_HEIGHT as u32,
                area.width,
                u32::from(shift.bottom - shift.top + 1) * GLYPH_HEIGHT as u32,
            ));
        }
        if self.view_back > 0 && self.needs_render() {
            self.invalidate();
        }
        if let Some((row, column)) = self.drawn_cursor.take() {
            self.touch(row, column, column + 1);
        }
        let cursor = self.shown_cursor();
        if let Some((row, column)) = cursor {
            self.touch(row, column, column + 1);
        }
        let mut band = Rect::default();
        for row in 0..self.rows {
            let span = core::mem::replace(&mut self.dirty[usize::from(row)], Span::CLEAN);
            if span.is_clean() {
                damage.add(core::mem::take(&mut band));
                continue;
            }
            let top = area.y as usize + usize::from(row) * GLYPH_HEIGHT;
            let line = self.visible_line(row);
            for column in span.start..span.end.min(self.columns) {
                let cell = line[usize::from(column)];
                let at = top * stride + area.x as usize + usize::from(column) * GLYPH_WIDTH;
                let look = match cursor == Some((row, column)) {
                    true => Some(self.cursor_look),
                    false => None,
                };
                let (foreground, background) = match look {
                    Some(CursorLook::Block) => (TERM_BACKGROUND, TERM_CURSOR),
                    _ => cell.colours(),
                };
                let underline =
                    cell.flags & cell_flags::UNDERLINE != 0 && look != Some(CursorLook::Block);
                font::draw_glyph(
                    pixels, stride, at, cell.glyph, foreground, background, underline,
                );
                if look == Some(CursorLook::Outline) {
                    outline_cell(pixels, stride, at);
                }
            }
            band = band.union(Rect::new(
                area.x + u32::from(span.start) * GLYPH_WIDTH as u32,
                top as u32,
                u32::from(span.end.min(self.columns) - span.start) * GLYPH_WIDTH as u32,
                GLYPH_HEIGHT as u32,
            ));
        }
        damage.add(band);
        if self.view_back > 0 {
            let column = self.columns - 1;
            let at = area.y as usize * stride + area.x as usize + usize::from(column) * GLYPH_WIDTH;
            font::draw_glyph(
                pixels,
                stride,
                at,
                font::UP_TRIANGLE,
                ACCENT,
                TERM_BACKGROUND,
                false,
            );
        }
        self.drawn_cursor = cursor;
    }

    fn shift_pixels(&self, pixels: &mut [u32], stride: usize, area: Rect, shift: Shift) {
        let height = usize::from(shift.bottom - shift.top + 1);
        let lines = usize::from(shift.lines);
        if lines >= height {
            return;
        }
        let width = area.width as usize;
        let first = area.y as usize + usize::from(shift.top) * GLYPH_HEIGHT;
        let distance = lines * GLYPH_HEIGHT * stride;
        for pixel_row in first..first + (height - lines) * GLYPH_HEIGHT {
            let to = pixel_row * stride + area.x as usize;
            pixels.copy_within(to + distance..to + distance + width, to);
        }
    }
}

fn outline_cell(pixels: &mut [u32], stride: usize, at: usize) {
    for row in 0..GLYPH_HEIGHT {
        let start = at + row * stride;
        let Some(line) = pixels.get_mut(start..start + GLYPH_WIDTH) else {
            return;
        };
        if row == 0 || row == GLYPH_HEIGHT - 1 {
            line.fill(TERM_CURSOR);
        } else {
            line[0] = TERM_CURSOR;
            line[GLYPH_WIDTH - 1] = TERM_CURSOR;
        }
    }
}
