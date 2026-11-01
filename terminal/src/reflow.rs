use super::{Cell, HISTORY_LINES, LineNumber, MAIN, Span, Terminal, filled, line_flags};
use alloc::boxed::Box;

#[derive(Clone, Copy)]
struct Source<'a> {
    terminal: &'a Terminal,
    rows: usize,
}

impl<'a> Source<'a> {
    fn row(&self, index: usize) -> (&'a [Cell], u8) {
        let terminal = self.terminal;
        if index < terminal.history_length {
            let slot = (terminal.history_start + index) % HISTORY_LINES;
            (terminal.history_line(index), terminal.history_flags[slot])
        } else {
            let row = (index - terminal.history_length) as u16;
            (terminal.line_of(MAIN, row), terminal.row_flags[MAIN][usize::from(row)])
        }
    }

    fn line_end(&self, start: usize) -> usize {
        let mut end = start + 1;
        while end < self.rows && self.row(end).1 & line_flags::WRAPPED != 0 {
            end += 1;
        }
        end
    }

    fn content_length(&self, start: usize, end: usize, width: usize) -> usize {
        let (last, _) = self.row(end - 1);
        let used = last
            .iter()
            .rposition(|cell| *cell != Cell::BLANK)
            .map_or(0, |at| at + 1);
        (end - 1 - start) * width + used
    }
}

fn rows_for(length: usize, columns: usize) -> usize {
    length.div_ceil(columns).max(1)
}

fn is_blank(cells: &[Cell]) -> bool {
    cells.iter().all(|cell| *cell == Cell::BLANK)
}

struct Destination {
    columns: usize,
    rows: usize,
    screen: Box<[Cell]>,
    screen_flags: Box<[u8]>,
    history: Box<[Cell]>,
    history_flags: Box<[u8]>,
    screen_start: usize,
    history_start: usize,
}

impl Destination {
    fn row(&mut self, index: usize) -> Option<(&mut [Cell], &mut u8)> {
        let columns = self.columns;
        if index >= self.screen_start {
            let row = index - self.screen_start;
            if row >= self.rows {
                return None;
            }
            let cells = &mut self.screen[row * columns..(row + 1) * columns];
            return Some((cells, &mut self.screen_flags[row]));
        }
        let slot = index.checked_sub(self.history_start)?;
        let cells = &mut self.history[slot * columns..(slot + 1) * columns];
        Some((cells, &mut self.history_flags[slot]))
    }
}

pub(super) fn resize(terminal: &mut Terminal, columns: u16, rows: u16) -> bool {
    let width = usize::from(columns);
    let height = usize::from(rows);
    let cells = width * height;
    let (
        Some(screen),
        Some(alternate),
        Some(history),
        Some(dirty),
        Some(screen_flags),
        Some(alternate_flags),
        Some(history_flags),
    ) = (
        filled(cells, Cell::BLANK),
        filled(cells, Cell::BLANK),
        filled(HISTORY_LINES * width, Cell::BLANK),
        filled(height, Span::CLEAN),
        filled(height, 0u8),
        filled(height, 0u8),
        filled(HISTORY_LINES, 0u8),
    )
    else {
        return false;
    };
    let old_width = usize::from(terminal.columns);
    let (cursor_row, cursor_column) = match terminal.active {
        MAIN => (terminal.row, terminal.column),
        _ => (terminal.saved[MAIN].row, terminal.saved[MAIN].column),
    };
    let last_written = (0..terminal.rows)
        .rev()
        .find(|&row| !is_blank(terminal.line_of(MAIN, row)))
        .unwrap_or(0);
    let end_row = usize::from(cursor_row.max(last_written));
    let source = Source {
        terminal: &*terminal,
        rows: terminal.history_length + end_row + 1,
    };
    let cursor_index = terminal.history_length + usize::from(cursor_row);
    let mut laid = 0;
    let mut cursor_at = (0, 0);
    let mut start = 0;
    while start < source.rows {
        let end = source.line_end(start);
        let length = source.content_length(start, end, old_width);
        let count = rows_for(length, width);
        if (start..end).contains(&cursor_index) {
            let offset = (cursor_index - start) * old_width + usize::from(cursor_column);
            cursor_at = match offset / width < count {
                true => (laid + offset / width, offset % width),
                false => (laid + count - 1, width - 1),
            };
        }
        laid += count;
        start = end;
    }
    let mut screen_start = laid.saturating_sub(height);
    if laid > height {
        screen_start = screen_start.min(cursor_at.0);
    }
    let kept = screen_start.min(HISTORY_LINES);
    let mut destination = Destination {
        columns: width,
        rows: height,
        screen,
        screen_flags,
        history,
        history_flags,
        screen_start,
        history_start: screen_start - kept,
    };
    let mut index = 0;
    let mut start = 0;
    while start < source.rows && index < screen_start + height {
        let end = source.line_end(start);
        let length = source.content_length(start, end, old_width);
        let count = rows_for(length, width);
        for part in 0..count {
            let Some((cells, flags)) = destination.row(index + part) else {
                continue;
            };
            if part > 0 {
                *flags = line_flags::WRAPPED;
            }
            let from = part * width;
            let to = ((part + 1) * width).min(length);
            let mut at = from;
            while at < to {
                let (old, _) = source.row(start + at / old_width);
                let column = at % old_width;
                let take = (old_width - column).min(to - at);
                cells[at - from..at - from + take].copy_from_slice(&old[column..column + take]);
                at += take;
            }
        }
        index += count;
        start = end;
    }
    let first = terminal.first_line_number();
    terminal.screens = [destination.screen, alternate];
    terminal.row_flags = [destination.screen_flags, alternate_flags];
    terminal.history = destination.history;
    terminal.history_flags = destination.history_flags;
    terminal.history_start = 0;
    terminal.history_length = kept;
    terminal.pushed = first.wrapping_add(screen_start as LineNumber);
    terminal.dirty = dirty;
    terminal.columns = columns;
    terminal.rows = rows;
    let (last_row, last_column) = (rows - 1, columns - 1);
    let reflowed = (
        ((cursor_at.0 - screen_start) as u16).min(last_row),
        (cursor_at.1 as u16).min(last_column),
    );
    for saved in terminal.saved.iter_mut() {
        saved.row = saved.row.min(last_row);
        saved.column = saved.column.min(last_column);
    }
    match terminal.active {
        MAIN => (terminal.row, terminal.column) = reflowed,
        _ => {
            terminal.row = terminal.row.min(last_row);
            terminal.column = terminal.column.min(last_column);
            (terminal.saved[MAIN].row, terminal.saved[MAIN].column) = reflowed;
        }
    }
    terminal.position_saved = (
        terminal.position_saved.0.min(last_row),
        terminal.position_saved.1.min(last_column),
    );
    terminal.top = 0;
    terminal.bottom = last_row;
    terminal.pending_wrap = false;
    terminal.view_back = 0;
    terminal.highlight = None;
    terminal.selection_reset = true;
    terminal.invalidate();
    true
}
