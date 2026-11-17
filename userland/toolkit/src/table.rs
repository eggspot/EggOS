//! A table: rows of cells under a header of resizable, sortable columns, scrolling both ways
//! (docs/ux/toolkit-v0.5.md section 4). The tree keeps no cell text: it asks the program for
//! the cells of the rows it draws, so a table of a million rows costs only its state.

use crate::canvas::Canvas;
use crate::geometry::Rect;
use crate::text::{self, Text};
use crate::widget::{
    Align, LINE_HEIGHT, Look, MAX_COLUMNS, ScrollBars, TABLE_ARROW_ROOM, TABLE_CELL_PADDING,
    TABLE_FIRST_TEXT_X, TABLE_HANDLE, TABLE_HEADER_HEIGHT, TABLE_MARK_X, TABLE_MIN_COLUMN,
    TABLE_ROW_HEIGHT, THUMB, THUMB_X, clipped_text, focus_ring, scroll_bars, sort_arrow, viewport,
};
use crate::widget::{TABLE_FIT_SAMPLE_ROWS, TABLE_MAX_FIT};

/// A header being dragged to move its column (docs/ux/toolkit-v0.6.md section 4.1): the
/// column, the ghost's left edge, and the boundary (a visual position) a release would move it
/// to, None where it would not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnMove {
    pub column: usize,
    pub ghost_x: i32,
    pub slot: Option<usize>,
}

/// Writes the text of cell (`row`, `column`) into `out`, from the program's `data`.
pub type CellText = fn(data: u32, row: usize, column: usize, out: &mut Text<64>);

/// Whether row `row` is in the program's set of selected rows, from its `data`.
pub type RowSelected = fn(data: u32, row: usize) -> bool;

/// What a person did to a table's set of selected rows (docs/ux/toolkit-v0.6.md section 4.3):
/// the program applies it to the set it keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionChange {
    Only(usize),
    Toggle(usize),
    Range(usize, usize),
    AddRange(usize, usize),
    All,
    Clear,
}

/// A set of rows, 64 a word, for a program whose rows are at most `64 * WORDS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSet<const WORDS: usize> {
    words: [u64; WORDS],
}

impl<const WORDS: usize> Default for RowSet<WORDS> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<const WORDS: usize> RowSet<WORDS> {
    /// No row.
    pub const EMPTY: Self = Self { words: [0; WORDS] };

    /// Whether `row` is in it.
    pub fn contains(&self, row: usize) -> bool {
        self.words
            .get(row / 64)
            .is_some_and(|word| word & (1 << (row % 64)) != 0)
    }

    fn put(&mut self, row: usize, on: bool) {
        if let Some(word) = self.words.get_mut(row / 64) {
            match on {
                true => *word |= 1 << (row % 64),
                false => *word &= !(1 << (row % 64)),
            }
        }
    }

    /// How many rows it holds.
    pub fn count(&self) -> usize {
        self.words.iter().map(|word| word.count_ones() as usize).sum()
    }

    /// Applies `change` for a table of `rows` rows.
    pub fn apply(&mut self, change: SelectionChange, rows: usize) {
        let rows = rows.min(64 * WORDS);
        match change {
            SelectionChange::Only(row) => {
                *self = Self::EMPTY;
                self.put(row, row < rows);
            }
            SelectionChange::Toggle(row) => self.put(row, !self.contains(row) && row < rows),
            SelectionChange::Range(first, last) | SelectionChange::AddRange(first, last) => {
                if matches!(change, SelectionChange::Range(..)) {
                    *self = Self::EMPTY;
                }
                for row in first.min(last)..=first.max(last).min(rows.saturating_sub(1)) {
                    self.put(row, true);
                }
            }
            SelectionChange::All => {
                for row in 0..rows {
                    self.put(row, true);
                }
            }
            SelectionChange::Clear => *self = Self::EMPTY,
        }
    }
}

/// Which way a sorted column reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    /// The other way.
    pub const fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// Its name as an agent reads it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// One column: its title, its width now and its narrowest, how its cells align, and whether
/// a click on its header sorts by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub title: &'static str,
    pub width: i32,
    pub min_width: i32,
    pub align: Align,
    pub sortable: bool,
}

impl Column {
    /// A sortable column of `title`, `width` wide, aligned by `align`, at least
    /// [`TABLE_MIN_COLUMN`] wide.
    pub const fn new(title: &'static str, width: i32, align: Align) -> Self {
        Self {
            title,
            width,
            min_width: TABLE_MIN_COLUMN,
            align,
            sortable: true,
        }
    }
}

const NO_COLUMN: Column = Column::new("", 0, Align::Start);

/// The first part number of a table's rows: shown row `r` is part `ROW_PART + r`; its columns'
/// headers are parts `0 .. MAX_COLUMNS`.
pub const ROW_PART: usize = MAX_COLUMNS;
/// The first part number of a table's column handles (never an element).
pub const HANDLE_PART: usize = 1 << 20;
/// The first part number of a table's rows when the pointer is on them (by row index, never
/// an element).
pub const ROW_HIT: usize = 1 << 24;

/// A table's state (docs/ux/toolkit-v0.5.md section 4.1).
#[derive(Debug, Clone, Copy)]
pub struct Table {
    /// Its name for agents.
    pub name: &'static str,
    pub columns: [Column; MAX_COLUMNS],
    pub column_count: usize,
    pub rows: usize,
    pub cell: CellText,
    pub data: u32,
    pub selected: usize,
    pub sort: Option<(usize, SortOrder)>,
    pub offset_y: i32,
    pub offset_x: i32,
    pub empty_text: &'static str,
    /// The program's set of selected rows (docs/ux/toolkit-v0.6.md section 4.3); None for one
    /// selected row, `selected`. With a set, `selected` is the current row.
    pub selection: Option<RowSelected>,
    pub selected_count: usize,
    /// The row a range starts at.
    pub anchor: usize,
    /// Whether Left and Right move a focused cell rather than scroll (section 4.4).
    pub cell_focus: bool,
    /// The focused cell's column.
    pub column: usize,
    /// Visual position to column index: the columns as drawn, left to right.
    pub order: [u8; MAX_COLUMNS],
    /// The header being dragged, while one is.
    pub moving: Option<ColumnMove>,
}

impl Table {
    /// A table of `columns` (at most [`MAX_COLUMNS`], each at least its narrowest) and `rows`
    /// rows, the first selected, unsorted, at its start.
    pub fn new(
        name: &'static str,
        columns: &[Column],
        rows: usize,
        cell: CellText,
        data: u32,
    ) -> Self {
        let column_count = columns.len().min(MAX_COLUMNS);
        let mut held = [NO_COLUMN; MAX_COLUMNS];
        for (place, column) in held.iter_mut().zip(&columns[..column_count]) {
            let min_width = column.min_width.max(TABLE_MIN_COLUMN);
            *place = Column {
                min_width,
                width: column.width.max(min_width),
                ..*column
            };
        }
        Self {
            name,
            columns: held,
            column_count,
            rows,
            cell,
            data,
            selected: 0,
            sort: None,
            offset_y: 0,
            offset_x: 0,
            empty_text: "No items",
            selection: None,
            selected_count: 0,
            anchor: 0,
            cell_focus: false,
            column: 0,
            order: core::array::from_fn(|position| position as u8),
            moving: None,
        }
    }

    /// Its columns.
    pub fn columns(&self) -> &[Column] {
        &self.columns[..self.column_count]
    }

    /// Every column's width together.
    pub fn content_width(&self) -> i32 {
        self.columns().iter().map(|column| column.width).sum()
    }

    /// Every row's height together.
    pub fn content_height(&self) -> i32 {
        (self.rows as i64 * i64::from(TABLE_ROW_HEIGHT)).min(i64::from(i32::MAX / 2)) as i32
    }

    /// The size its content needs: every column, the header and every row.
    pub fn content_size(&self) -> (i32, i32) {
        (
            self.content_width(),
            TABLE_HEADER_HEIGHT.saturating_add(self.content_height()),
        )
    }

    /// Where its rows go in a table at `rect`: below the header.
    pub fn body(rect: Rect) -> Rect {
        Rect::new(
            rect.x,
            rect.y + TABLE_HEADER_HEIGHT,
            rect.width,
            (rect.height - TABLE_HEADER_HEIGHT).max(0),
        )
    }

    /// The part of the body its rows show through: the body less the bars that show.
    pub fn view(&self, rect: Rect) -> Rect {
        viewport(
            Self::body(rect),
            (self.content_width(), self.content_height()),
        )
    }

    /// Its scroll bars in a table at `rect`: the vertical one's track and thumb, then the
    /// horizontal one's.
    pub fn bars(&self, rect: Rect) -> ScrollBars {
        scroll_bars(
            Self::body(rect),
            (self.content_width(), self.content_height()),
            (self.offset_x, self.offset_y),
        )
    }

    /// Keeps both offsets within what its content and a table at `rect` allow.
    pub fn clamp_offsets(&mut self, rect: Rect) {
        let view = self.view(rect);
        self.offset_y = self
            .offset_y
            .clamp(0, (self.content_height() - view.height).max(0));
        self.offset_x = self
            .offset_x
            .clamp(0, (self.content_width() - view.width).max(0));
    }

    /// The columns as drawn, left to right, by index.
    pub fn visual_columns(&self) -> impl Iterator<Item = usize> + '_ {
        self.order[..self.column_count]
            .iter()
            .map(|&column| usize::from(column))
    }

    /// Where column `column` is drawn, from 0 at the left; the column count for none.
    pub fn position_of(&self, column: usize) -> usize {
        self.visual_columns()
            .position(|found| found == column)
            .unwrap_or(self.column_count)
    }

    /// Column `column`'s left edge in a table at `rect`, by the drawn order; past the last
    /// column for an index beyond them.
    pub fn column_x(&self, rect: Rect, column: usize) -> i32 {
        let position = self.position_of(column);
        rect.x - self.offset_x
            + self
                .visual_columns()
                .take(position)
                .map(|before| self.columns[before].width)
                .sum::<i32>()
    }

    /// Column `column`'s right edge in a table at `rect`.
    pub fn column_right(&self, rect: Rect, column: usize) -> i32 {
        let width = self.columns.get(column).map_or(0, |held| held.width);
        self.column_x(rect, column) + width
    }

    /// The boundary between drawn columns (0 before the first, the count after the last)
    /// nearest `x` in a table at `rect`: before a column over its left half, after it over its
    /// right half.
    pub fn slot_at(&self, rect: Rect, x: i32) -> usize {
        for (position, column) in self.visual_columns().enumerate() {
            let left = self.column_x(rect, column);
            let right = self.column_right(rect, column);
            if x < (left + right) / 2 {
                return position;
            }
            if x < right {
                return position + 1;
            }
        }
        self.column_count
    }

    /// Moves the column drawn at `from` to boundary `slot`: where it is drawn now.
    pub fn move_column(&mut self, from: usize, slot: usize) -> usize {
        let count = self.column_count;
        if from >= count {
            return from;
        }
        let to = match slot > from {
            true => (slot - 1).min(count - 1),
            false => slot,
        };
        let moved = self.order[from];
        match to > from {
            true => self.order.copy_within(from + 1..=to, from),
            false => self.order.copy_within(to..from, to + 1),
        }
        self.order[to] = moved;
        to
    }

    /// The width that fits column `column`'s title and the cells of the rows shown in a table
    /// at `rect` and of the first [`TABLE_FIT_SAMPLE_ROWS`] rows, within its narrowest and
    /// [`TABLE_MAX_FIT`] (docs/ux/toolkit-v0.6.md section 4.2).
    pub fn fit_width(&self, rect: Rect, column: usize) -> i32 {
        let Some(held) = self.columns().get(column) else {
            return 0;
        };
        let arrow = match self.sort {
            Some((sorted, _)) if sorted == column => TABLE_ARROW_ROOM,
            _ => 0,
        };
        let first = match self.position_of(column) {
            0 => TABLE_FIRST_TEXT_X - TABLE_CELL_PADDING,
            _ => 0,
        };
        let padding = 2 * TABLE_CELL_PADDING;
        let sample = 0..self.rows.min(TABLE_FIT_SAMPLE_ROWS);
        let shown = self.shown_rows(rect);
        let cells = shown
            .filter(|row| !sample.contains(row))
            .chain(sample.clone())
            .map(|row| text::width(self.cell_text(row, column).as_str()) + padding + first)
            .max()
            .unwrap_or(0);
        let title = text::width(held.title) + padding + arrow;
        title
            .max(cells)
            .clamp(held.min_width, TABLE_MAX_FIT.max(held.min_width))
    }

    /// The rectangle of row `row` in a table at `rect`: the table's full width less the
    /// vertical bar.
    pub fn row_rect(&self, rect: Rect, row: usize) -> Rect {
        let view = self.view(rect);
        Rect::new(
            rect.x,
            view.y + row as i32 * TABLE_ROW_HEIGHT - self.offset_y,
            view.width,
            TABLE_ROW_HEIGHT,
        )
    }

    /// The rows that meet the body of a table at `rect`, first to last.
    pub fn shown_rows(&self, rect: Rect) -> core::ops::Range<usize> {
        let view = self.view(rect);
        if self.rows == 0 || view.height <= 0 {
            return 0..0;
        }
        let first = (self.offset_y / TABLE_ROW_HEIGHT).max(0) as usize;
        let last = ((self.offset_y + view.height - 1) / TABLE_ROW_HEIGHT).max(0) as usize;
        first.min(self.rows)..(last + 1).min(self.rows)
    }

    /// The column whose resize handle is at (`x`, `y`) in a table at `rect`.
    pub fn handle_at(&self, rect: Rect, x: i32, y: i32) -> Option<usize> {
        if y < rect.y || y >= rect.y + TABLE_HEADER_HEIGHT {
            return None;
        }
        (0..self.column_count).find(|&column| {
            let edge = self.column_right(rect, column);
            (edge - TABLE_HANDLE / 2 - 1..edge + TABLE_HANDLE / 2).contains(&x)
        })
    }

    /// The column whose header is at (`x`, `y`) in a table at `rect`.
    pub fn header_at(&self, rect: Rect, x: i32, y: i32) -> Option<usize> {
        if y < rect.y || y >= rect.y + TABLE_HEADER_HEIGHT || x < rect.x || x >= rect.right() {
            return None;
        }
        (0..self.column_count)
            .find(|&column| x >= self.column_x(rect, column) && x < self.column_right(rect, column))
    }

    /// The part of a table at `rect` at (`x`, `y`): a handle, a header, a bar's thumb or a row
    /// (by index, from [`ROW_HIT`]).
    pub fn part_at(&self, rect: Rect, x: i32, y: i32) -> Option<usize> {
        if let Some(column) = self.handle_at(rect, x, y) {
            return Some(HANDLE_PART + column);
        }
        if let Some(column) = self.header_at(rect, x, y) {
            return Some(column);
        }
        let (vertical, horizontal) = self.bars(rect);
        if let Some((track, _)) = vertical
            && track.contains(x, y)
        {
            return Some(THUMB);
        }
        if let Some((track, _)) = horizontal
            && track.contains(x, y)
        {
            return Some(THUMB_X);
        }
        let view = self.view(rect);
        if !view.contains(x, y) {
            return None;
        }
        let row = ((y - view.y + self.offset_y) / TABLE_ROW_HEIGHT).max(0) as usize;
        (row < self.rows).then_some(ROW_HIT + row)
    }

    /// The offset that shows row `row` whole in a table at `rect`.
    pub fn offset_showing(&self, rect: Rect, row: usize) -> i32 {
        let view = self.view(rect);
        let top = row as i32 * TABLE_ROW_HEIGHT;
        let bottom = top + TABLE_ROW_HEIGHT;
        match () {
            () if top < self.offset_y => top,
            () if bottom > self.offset_y + view.height => bottom - view.height,
            () => self.offset_y,
        }
    }

    /// Whether row `row` is drawn selected: in the program's set, or the one selected row.
    pub fn is_row_selected(&self, row: usize) -> bool {
        match self.selection {
            Some(selected) => selected(self.data, row),
            None => row == self.selected,
        }
    }

    /// The column whose cells are at `x` in a table at `rect`.
    pub fn column_at_x(&self, rect: Rect, x: i32) -> Option<usize> {
        (0..self.column_count)
            .find(|&column| x >= self.column_x(rect, column) && x < self.column_right(rect, column))
    }

    /// The offset that shows column `column` whole in a table at `rect`, its left edge first.
    pub fn offset_showing_column(&self, rect: Rect, column: usize) -> i32 {
        let view = self.view(rect);
        let left = self.column_x(rect, column) + self.offset_x - rect.x;
        let right = left + self.columns[column.min(MAX_COLUMNS - 1)].width;
        match () {
            () if left < self.offset_x => left,
            () if right > self.offset_x + view.width => (right - view.width).min(left),
            () => self.offset_x,
        }
    }

    /// The text of cell (`row`, `column`).
    pub fn cell_text(&self, row: usize, column: usize) -> Text<64> {
        let mut out = Text::new();
        (self.cell)(self.data, row, column, &mut out);
        out
    }

    fn cell_line(
        &self,
        canvas: &mut Canvas<'_>,
        column: usize,
        left: i32,
        y: i32,
        words: &str,
        colour: u32,
    ) {
        let held = self.columns[column];
        let start = match self.position_of(column) {
            0 => left + TABLE_FIRST_TEXT_X,
            _ => left + TABLE_CELL_PADDING,
        };
        let end = left + held.width - TABLE_CELL_PADDING;
        match held.align {
            Align::End => {
                let width = text::width(words).min(end - start);
                clipped_text(canvas, end - width, y, width, words, colour);
            }
            _ => clipped_text(canvas, start, y, end - start, words, colour),
        }
    }

    fn paint_column_move(&self, canvas: &mut Canvas<'_>, rect: Rect, moving: ColumnMove) {
        let palette = canvas.palette();
        let previous = canvas.narrow(rect);
        let view = self.view(rect);
        if let Some(slot) = moving.slot {
            let boundary = match self.visual_columns().nth(slot) {
                Some(column) => self.column_x(rect, column),
                None => rect.x - self.offset_x + self.content_width(),
            };
            let x = (boundary - 1).clamp(view.x, (view.right() - 2).max(view.x));
            canvas.fill(
                Rect::new(x, rect.y, 2, view.bottom() - rect.y),
                palette.accent,
            );
        }
        if let Some(held) = self.columns().get(moving.column) {
            let ghost = Rect::new(moving.ghost_x, rect.y, held.width, TABLE_HEADER_HEIGHT);
            canvas.fill(ghost, palette.menu_hover);
            canvas.outline(ghost, &crate::canvas::SQUARE, 1, palette.menu_border);
            let x = ghost.x + TABLE_CELL_PADDING;
            let end = ghost.right() - TABLE_CELL_PADDING;
            clipped_text(canvas, x, rect.y + 6, end - x, held.title, palette.text);
        }
        canvas.restore(previous);
    }

    /// Draws it at `rect` as `look` says: the header (the hovered or pressed column by its
    /// part, the handle being dragged as [`HANDLE_PART`] plus its column), the rows that meet
    /// the body, and the bars.
    pub fn paint(&self, canvas: &mut Canvas<'_>, rect: Rect, look: Look) {
        let palette = canvas.palette();
        canvas.fill(rect, palette.menu);
        let header = Rect::new(rect.x, rect.y, rect.width, TABLE_HEADER_HEIGHT);
        let previous = canvas.narrow(header);
        for column in 0..self.column_count {
            let left = self.column_x(rect, column);
            let held = self.columns[column];
            let cell = Rect::new(left, rect.y, held.width, TABLE_HEADER_HEIGHT - 1);
            let dragged = self.moving.is_some_and(|moving| moving.column == column);
            let fill = match (look.enabled && held.sortable, look.pressed, look.hovered) {
                _ if dragged => Some(palette.button_pressed),
                (true, Some(pressed), _) if pressed == column => Some(palette.button_pressed),
                (true, None, Some(hovered)) if hovered == column => Some(palette.menu_hover),
                _ => None,
            };
            if let Some(fill) = fill {
                canvas.fill(cell, fill);
            }
            let sorted = self.sort.filter(|(sorted, _)| *sorted == column);
            let ink = match (look.enabled, sorted) {
                _ if dragged => palette.text_dim,
                (true, Some(_)) => palette.text,
                _ => palette.text_dim,
            };
            let room = match sorted {
                Some(_) if held.align != Align::End => TABLE_ARROW_ROOM - TABLE_CELL_PADDING,
                _ => 0,
            };
            let x = left + TABLE_CELL_PADDING;
            let end = left + held.width - TABLE_CELL_PADDING - room;
            match held.align {
                Align::End => {
                    let width = text::width(held.title).min(end - x);
                    clipped_text(canvas, end - width, rect.y + 6, width, held.title, ink);
                }
                _ => clipped_text(canvas, x, rect.y + 6, end - x, held.title, ink),
            }
            if let Some((_, order)) = sorted {
                let arrow_left = match held.align {
                    Align::End => left + TABLE_CELL_PADDING,
                    _ => left + held.width - TABLE_CELL_PADDING - 10,
                };
                sort_arrow(
                    canvas,
                    arrow_left,
                    rect.y + 11,
                    order == SortOrder::Ascending,
                    palette.accent,
                );
            }
            let edge = left + held.width - 1;
            let handle = HANDLE_PART + column;
            let active = look.hovered == Some(handle) || look.pressed == Some(handle);
            match active {
                true => canvas.fill(
                    Rect::new(edge, rect.y, 1, TABLE_HEADER_HEIGHT),
                    palette.text_dim,
                ),
                false => canvas.fill(Rect::new(edge, rect.y + 6, 1, 16), palette.menu_border),
            }
        }
        canvas.restore(previous);
        canvas.fill(
            Rect::new(rect.x, rect.y + TABLE_HEADER_HEIGHT - 1, rect.width, 1),
            palette.menu_border,
        );
        let view = self.view(rect);
        let previous = canvas.narrow(view);
        let mark_y = (TABLE_ROW_HEIGHT - LINE_HEIGHT) / 2;
        if self.rows == 0 {
            canvas.text(
                rect.x - self.offset_x + TABLE_FIRST_TEXT_X,
                view.y + mark_y,
                self.empty_text,
                palette.text_dim,
                1,
            );
        }
        let clip = canvas.clip();
        for row in self.shown_rows(rect) {
            let row_rect = self.row_rect(rect, row);
            if row_rect.intersection(&clip).is_empty() {
                continue;
            }
            let chosen = self.is_row_selected(row);
            let current = row == self.selected;
            if chosen {
                canvas.fill(row_rect, palette.selection);
            } else if look.hovered == Some(ROW_HIT + row) {
                canvas.fill(row_rect, palette.menu_hover);
            }
            let ink = match look.enabled {
                true => palette.text,
                false => palette.text_dim,
            };
            if chosen && look.enabled {
                canvas.text(
                    rect.x - self.offset_x + TABLE_MARK_X,
                    row_rect.y + mark_y,
                    ">",
                    palette.accent,
                    1,
                );
            }
            for column in 0..self.column_count {
                let left = self.column_x(rect, column);
                if left >= clip.right() || left + self.columns[column].width <= clip.x {
                    continue;
                }
                let words = self.cell_text(row, column);
                self.cell_line(
                    canvas,
                    column,
                    left,
                    row_rect.y + mark_y,
                    words.as_str(),
                    ink,
                );
            }
            if current && look.focused {
                match self.cell_focus {
                    true => {
                        let column = self.column.min(self.column_count.saturating_sub(1));
                        let left = self.column_x(rect, column);
                        let cell = Rect::new(
                            left,
                            row_rect.y,
                            self.columns[column].width,
                            TABLE_ROW_HEIGHT,
                        );
                        focus_ring(canvas, cell, 0);
                    }
                    false => focus_ring(canvas, row_rect, 0),
                }
            }
        }
        canvas.restore(previous);
        if let Some(moving) = self.moving {
            self.paint_column_move(canvas, rect, moving);
        }
        let (vertical, horizontal) = self.bars(rect);
        for (bar, part) in [(vertical, THUMB), (horizontal, THUMB_X)] {
            let Some((track, thumb)) = bar else {
                continue;
            };
            canvas.fill(track, palette.progress_track);
            let colour = match look.hovered == Some(part) || look.pressed == Some(part) {
                true => palette.button_inactive,
                false => palette.bar_pressed,
            };
            canvas.rounded(thumb, &crate::canvas::INSET_R4, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_set_applies_each_change_within_its_rows() {
        let mut set = RowSet::<2>::EMPTY;
        set.apply(SelectionChange::Only(3), 100);
        assert!(set.contains(3) && set.count() == 1);
        set.apply(SelectionChange::Toggle(70), 100);
        set.apply(SelectionChange::AddRange(10, 8), 100);
        assert_eq!(set.count(), 5);
        set.apply(SelectionChange::Range(98, 96), 100);
        assert_eq!(set.count(), 3);
        assert!(set.contains(96) && !set.contains(3));
        set.apply(SelectionChange::All, 100);
        assert_eq!(set.count(), 100);
        set.apply(SelectionChange::Toggle(5), 100);
        assert!(!set.contains(5));
        set.apply(SelectionChange::Clear, 100);
        assert_eq!(set.count(), 0);
    }
    fn no_cell(_: u32, _: usize, _: usize, _: &mut Text<64>) {}

    #[test]
    fn a_moved_column_keeps_its_index_and_draws_in_its_new_place() {
        let columns = [
            Column::new("Name", 200, Align::Start),
            Column::new("Size", 80, Align::End),
            Column::new("Kind", 100, Align::Start),
        ];
        let mut table = Table::new("Files", &columns, 0, no_cell, 0);
        let rect = Rect::new(0, 0, 600, 300);
        assert_eq!(table.slot_at(rect, 50), 0);
        assert_eq!(table.slot_at(rect, 150), 1);
        assert_eq!(table.move_column(2, 0), 0);
        assert_eq!(&table.order[..3], &[2, 0, 1]);
        assert_eq!(table.column_x(rect, 2), 0);
        assert_eq!(table.column_x(rect, 0), 100);
        assert_eq!(table.column_right(rect, 1), 380);
        assert_eq!(table.header_at(rect, 50, 10), Some(2));
        assert_eq!(table.move_column(0, 3), 2);
        assert_eq!(&table.order[..3], &[0, 1, 2]);
    }

    #[test]
    fn fit_measures_the_title_with_its_arrow_and_stays_within_bounds() {
        let columns = [Column::new("Name", 200, Align::Start)];
        let mut table = Table::new("Files", &columns, 0, no_cell, 0);
        let rect = Rect::new(0, 0, 600, 300);
        let title = text::width("Name");
        assert_eq!(table.fit_width(rect, 0), (title + 16).max(TABLE_MIN_COLUMN));
        table.sort = Some((0, SortOrder::Ascending));
        assert_eq!(table.fit_width(rect, 0), (title + 36).max(TABLE_MIN_COLUMN));
    }
}
