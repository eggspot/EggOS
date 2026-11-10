//! A terminal window's selection (docs/ux/terminal-selection-v0.3.md sections 1.1, 1.2 and
//! 1.4): two boundaries on numbered lines, so the selection moves with its text as output
//! scrolls; whole words or whole logical lines after a double or triple click; and the text a
//! copy takes.

use alloc::string::String;
use eggos_terminal::{Cell, Highlight, LineNumber, Terminal, line_flags};

/// Presses closer together than this count as a double or triple click.
pub const DOUBLE_CLICK_MS: u32 = 400;

/// The characters that end a word, besides the space: each is a word of its own.
pub const WORD_BREAKS: &str = "\"'()[]{}<>,;|&`";

/// A place between two cells: the edge left of cell `column` of line `line`, `0..=columns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Boundary {
    pub line: LineNumber,
    pub column: u16,
}

/// What a press and a drag select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Cells, boundary to boundary.
    Cells,
    /// Whole words.
    Words,
    /// Whole logical lines.
    Lines,
}

/// A selection: the unit where it started (`anchor` to `anchor_end`, the same boundary for
/// [`SelectionMode::Cells`]), the place the pointer moved it to, `head`, and its mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Boundary,
    pub anchor_end: Boundary,
    pub head: Boundary,
    pub mode: SelectionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Break,
    Word,
}

fn class_of(cell: Cell) -> Class {
    match eggos_font::character(cell.glyph) {
        ' ' => Class::Space,
        character if WORD_BREAKS.contains(character) => Class::Break,
        _ => Class::Word,
    }
}

fn wrapped(terminal: &Terminal, line: LineNumber) -> bool {
    terminal
        .numbered_line(line)
        .is_some_and(|(_, flags)| flags & line_flags::WRAPPED != 0)
}

fn cell_at(terminal: &Terminal, line: LineNumber, column: u16) -> Option<Cell> {
    terminal
        .numbered_line(line)
        .and_then(|(cells, _)| cells.get(usize::from(column)).copied())
}

fn previous_cell(terminal: &Terminal, at: Boundary) -> Option<Boundary> {
    match at.column {
        0 if wrapped(terminal, at.line) => {
            let line = at.line.checked_sub(1)?;
            let columns = terminal.size().0;
            Some(Boundary {
                line,
                column: columns - 1,
            })
        }
        0 => None,
        column => Some(Boundary {
            line: at.line,
            column: column - 1,
        }),
    }
}

fn next_cell(terminal: &Terminal, at: Boundary) -> Option<Boundary> {
    let columns = terminal.size().0;
    match at.column + 1 < columns {
        true => Some(Boundary {
            line: at.line,
            column: at.column + 1,
        }),
        false if wrapped(terminal, at.line + 1) => Some(Boundary {
            line: at.line + 1,
            column: 0,
        }),
        false => None,
    }
}

fn after(cell: Boundary) -> Boundary {
    Boundary {
        line: cell.line,
        column: cell.column + 1,
    }
}

/// The word holding the cell at `cell` (its line and column): a run of cells of the same
/// kind, spaces or word characters, or one [`WORD_BREAKS`] character alone; it crosses a row's
/// end only onto a row flagged [`line_flags::WRAPPED`]. Its start and end boundaries.
pub fn word_at(terminal: &Terminal, cell: Boundary) -> (Boundary, Boundary) {
    let Some(at) = cell_at(terminal, cell.line, cell.column) else {
        return (cell, cell);
    };
    let class = class_of(at);
    if class == Class::Break {
        return (cell, after(cell));
    }
    let same = |place: Boundary| {
        cell_at(terminal, place.line, place.column).is_some_and(|other| class_of(other) == class)
    };
    let mut start = cell;
    while let Some(previous) = previous_cell(terminal, start).filter(|&place| same(place)) {
        start = previous;
    }
    let mut last = cell;
    while let Some(next) = next_cell(terminal, last).filter(|&place| same(place)) {
        last = next;
    }
    (start, after(last))
}

/// The logical line holding line `line`: from the start of the row it continues (the rows
/// flagged [`line_flags::WRAPPED`] are continuations) to the end of its last continuation.
pub fn logical_line_at(terminal: &Terminal, line: LineNumber) -> (Boundary, Boundary) {
    let first_kept = terminal.first_line_number();
    let mut start = line;
    while start > first_kept && wrapped(terminal, start) {
        start -= 1;
    }
    let mut end = line;
    while wrapped(terminal, end + 1) {
        end += 1;
    }
    let columns = terminal.size().0;
    (
        Boundary {
            line: start,
            column: 0,
        },
        Boundary {
            line: end,
            column: columns,
        },
    )
}

impl Selection {
    /// A selection starting at boundary `at` in [`SelectionMode::Cells`], of no cell yet.
    pub fn cells(at: Boundary) -> Self {
        Self {
            anchor: at,
            anchor_end: at,
            head: at,
            mode: SelectionMode::Cells,
        }
    }

    /// A selection of the unit (`mode`'s word or logical line) holding cell `cell`.
    pub fn unit(terminal: &Terminal, cell: Boundary, mode: SelectionMode) -> Self {
        let (anchor, anchor_end) = unit_at(terminal, cell, mode);
        Self {
            anchor,
            anchor_end,
            head: cell,
            mode,
        }
    }

    /// Its first and last boundaries in reading order: for words and lines, the anchor's unit
    /// and the unit holding the head's cell together.
    pub fn span(&self, terminal: &Terminal) -> (Boundary, Boundary) {
        match self.mode {
            SelectionMode::Cells => match self.anchor <= self.head {
                true => (self.anchor, self.head),
                false => (self.head, self.anchor),
            },
            mode => {
                let (start, end) = unit_at(terminal, self.head, mode);
                (start.min(self.anchor), end.max(self.anchor_end))
            }
        }
    }

    /// Whether it selects no cell.
    pub fn is_empty(&self, terminal: &Terminal) -> bool {
        let (start, end) = self.span(terminal);
        start == end
    }

    /// The cells the engine fills, `None` for no cell.
    pub fn highlight(&self, terminal: &Terminal) -> Option<Highlight> {
        let (start, end) = self.span(terminal);
        (start != end).then_some(Highlight {
            start: (start.line, start.column),
            end: (end.line, end.column),
        })
    }

    /// Whether it starts on a line the scrollback no longer keeps.
    pub fn is_dropped(&self, terminal: &Terminal) -> bool {
        self.anchor.line.min(self.head.line) < terminal.first_line_number()
    }
}

fn unit_at(terminal: &Terminal, cell: Boundary, mode: SelectionMode) -> (Boundary, Boundary) {
    let columns = terminal.size().0;
    let cell = Boundary {
        line: cell.line,
        column: cell.column.min(columns.saturating_sub(1)),
    };
    match mode {
        SelectionMode::Cells => (cell, cell),
        SelectionMode::Words => word_at(terminal, cell),
        SelectionMode::Lines => logical_line_at(terminal, cell.line),
    }
}

/// The text of the cells from `start` to `end` (docs/ux/terminal-selection-v0.3.md section
/// 1.4): each row's characters with its trailing spaces trimmed, rows joined with `\n` except
/// onto a row flagged [`line_flags::WRAPPED`], and a final `\n` when it ends at a row's end
/// that does not continue.
pub fn text_between(terminal: &Terminal, start: Boundary, end: Boundary) -> String {
    let mut text = String::new();
    if start >= end {
        return text;
    }
    let columns = terminal.size().0;
    let mut line = start.line;
    while let Some((cells, _)) = terminal.numbered_line(line) {
        let from = match line == start.line {
            true => start.column,
            false => 0,
        };
        let to = match line == end.line {
            true => end.column,
            false => columns,
        };
        let row = cells
            .get(usize::from(from)..usize::from(to.max(from)))
            .unwrap_or(&[]);
        let used = row
            .iter()
            .rposition(|cell| cell.glyph != b' ')
            .map_or(0, |last| last + 1);
        for cell in &row[..used] {
            text.push(eggos_font::character(cell.glyph));
        }
        if line == end.line {
            if to >= columns && !wrapped(terminal, line + 1) {
                text.push('\n');
            }
            break;
        }
        if !wrapped(terminal, line + 1) {
            text.push('\n');
        }
        line += 1;
    }
    text
}

/// The text a copy of `selection` takes.
pub fn copied_text(terminal: &Terminal, selection: &Selection) -> String {
    let (start, end) = selection.span(terminal);
    text_between(terminal, start, end)
}

/// The selection Select All makes: from the scrollback's first line to the end of the live
/// screen's last row that is not blank; `None` when every line is blank.
pub fn select_all(terminal: &Terminal) -> Option<Selection> {
    let rows = terminal.size().1;
    let (last_row, used) = (0..rows).rev().find_map(|row| {
        let used = terminal
            .line(row)
            .iter()
            .rposition(|cell| *cell != Cell::BLANK)?;
        Some((row, used as u16 + 1))
    })?;
    let first = Boundary {
        line: terminal.first_line_number(),
        column: 0,
    };
    let end = Boundary {
        line: terminal.line_number_of_row(last_row),
        column: used,
    };
    Some(Selection {
        anchor: first,
        anchor_end: first,
        head: end,
        mode: SelectionMode::Cells,
    })
}
