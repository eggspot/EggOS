//! The terminal emulator of docs/ux/terminal-v0.3.md (D109, D114): a grid of cells fed the
//! bytes programs write, the ANSI subset the shell, the core utilities and a full-screen editor
//! need, a scrollback, and rendering of only what changed into a rectangle of a framebuffer.
//!
//! [`Terminal::feed`] parses bytes and updates the cells, recording which changed; it never
//! draws. [`Terminal::render`] draws the changed cells and the cursor, moving pixels instead of
//! redrawing them when the screen scrolled, and reports the rectangles it changed. Replies a
//! program asked for (`CSI 6 n`, `CSI c`) wait in [`Terminal::replies`] for the input side.

mod colours;
mod parser;

#[cfg(test)]
mod tests;

pub use colours::{
    ACCENT, ANSI_COLOURS, DEFAULT_COLOUR, TERM_BACKGROUND, TERM_CURSOR, TERM_FOREGROUND,
};

use crate::display::{Damage, Rect};
use crate::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};
use alloc::boxed::Box;
use alloc::vec::Vec;
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

/// One character cell: a glyph id ([`crate::font`]), two colours (0-15 the ANSI colours,
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
    replies: [u8; REPLY_CAPACITY],
    reply_length: usize,
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
            replies: [0; REPLY_CAPACITY],
            reply_length: 0,
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

    /// Whether something changed since the last [`Self::render`].
    pub fn needs_render(&self) -> bool {
        self.shift.is_some() || self.dirty.iter().any(|span| !span.is_clean())
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
    }

    fn report_position(&mut self) {
        let row = if self.modes.origin {
            self.row - self.top
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
                let (foreground, background) = if cursor == Some((row, column)) {
                    (TERM_BACKGROUND, TERM_CURSOR)
                } else {
                    cell.colours()
                };
                let underline =
                    cell.flags & cell_flags::UNDERLINE != 0 && cursor != Some((row, column));
                font::draw_glyph(
                    pixels, stride, at, cell.glyph, foreground, background, underline,
                );
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
