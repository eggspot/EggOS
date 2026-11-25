//! Files' list columns and their order (D778, docs/ux/toolkit-v0.6.md section 4.1): `Name`,
//! `Size` and `Modified` (`Deleted` in the Trash) in the order the person dragged them to,
//! `Name` taking the room the other two leave, saved as one line (`name,size,modified`) in
//! `~/.config/eggos/files-columns` and read when Files opens.

/// One column of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Time,
}

impl Column {
    fn word(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Size => "size",
            Column::Time => "modified",
        }
    }

    fn of(word: &str) -> Option<Self> {
        [Column::Name, Column::Size, Column::Time]
            .into_iter()
            .find(|column| column.word() == word)
    }
}

/// Where the order is kept.
pub const COLUMNS_PATH: &[u8] = b"/home/root/.config/eggos/files-columns";
/// The size column's width.
pub const SIZE_SPAN: i32 = 80;
/// The time column's width.
pub const TIME_SPAN: i32 = 128;
/// The space between columns.
pub const GAP: i32 = 8;
/// The narrowest the name column gets.
pub const LEAST_NAME: i32 = 96;

/// The columns in their order, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    order: [Column; 3],
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            order: [Column::Name, Column::Size, Column::Time],
        }
    }
}

/// One column's place: its left edge and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub column: Column,
    pub x: i32,
    pub width: i32,
}

impl Columns {
    /// The order a saved line names, the default when it names not every column once.
    pub fn parse(text: &[u8]) -> Self {
        let text = core::str::from_utf8(text).unwrap_or("").trim();
        let mut order = [Column::Name; 3];
        let mut count = 0;
        for word in text.split(',') {
            let Some(column) = Column::of(word.trim()) else {
                return Self::default();
            };
            if count == 3 || order[..count].contains(&column) {
                return Self::default();
            }
            order[count] = column;
            count += 1;
        }
        match count {
            3 => Self { order },
            _ => Self::default(),
        }
    }

    /// The line it is saved as.
    pub fn line(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let mut at = 0;
        for (index, column) in self.order.iter().enumerate() {
            if index > 0 {
                out[at] = b',';
                at += 1;
            }
            let word = column.word().as_bytes();
            out[at..at + word.len()].copy_from_slice(word);
            at += word.len();
        }
        out
    }

    /// The line's length in [`Self::line`].
    pub fn line_length(&self) -> usize {
        self.order.iter().map(|column| column.word().len()).sum::<usize>() + 2
    }

    /// The columns left to right.
    pub fn order(&self) -> [Column; 3] {
        self.order
    }

    /// Each column's place in a list from `left` to `right`.
    pub fn spans(&self, left: i32, right: i32) -> [Span; 3] {
        let name = (right - left - SIZE_SPAN - TIME_SPAN - 2 * GAP).max(LEAST_NAME);
        let mut x = left;
        self.order.map(|column| {
            let width = match column {
                Column::Name => name,
                Column::Size => SIZE_SPAN,
                Column::Time => TIME_SPAN,
            };
            let span = Span { column, x, width };
            x += width + GAP;
            span
        })
    }

    /// The boundary (0 to 3, before the first column to after the last) a header dragged to
    /// `x` drops into: before a column over its left half, after it over its right half.
    pub fn boundary_at(spans: &[Span; 3], x: i32) -> usize {
        spans
            .iter()
            .position(|span| x < span.x + span.width / 2)
            .unwrap_or(3)
    }

    /// Moves the column at visual position `from` to `boundary`; the new position when it
    /// moved, `None` for the two boundaries beside it.
    pub fn move_to(&mut self, from: usize, boundary: usize) -> Option<usize> {
        if from >= 3 || boundary == from || boundary == from + 1 || boundary > 3 {
            return None;
        }
        let column = self.order[from];
        let to = if boundary > from { boundary - 1 } else { boundary };
        let mut order = [Column::Name; 3];
        let mut rest = self.order.iter().copied().filter(|each| *each != column);
        for (index, slot) in order.iter_mut().enumerate() {
            *slot = match index == to {
                true => column,
                false => rest.next().unwrap_or(column),
            };
        }
        self.order = order;
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_order_is_saved_as_a_line_and_read_back() {
        let mut columns = Columns::default();
        assert_eq!(columns.move_to(1, 0), Some(0));
        assert_eq!(columns.order(), [Column::Size, Column::Name, Column::Time]);
        let line = columns.line();
        let text = &line[..columns.line_length()];
        assert_eq!(text, b"size,name,modified");
        assert_eq!(Columns::parse(text), columns);
        assert_eq!(Columns::parse(b"name,name,size"), Columns::default());
        assert_eq!(Columns::parse(b"size"), Columns::default());
    }

    #[test]
    fn name_takes_the_room_left_and_the_boundaries_beside_a_column_move_nothing() {
        let columns = Columns::default();
        let spans = columns.spans(176, 816);
        assert_eq!(spans[0], Span { column: Column::Name, x: 176, width: 416 });
        assert_eq!(spans[1].x, 600);
        assert_eq!(spans[2].x, 688);
        assert_eq!(Columns::boundary_at(&spans, 200), 0);
        assert_eq!(Columns::boundary_at(&spans, 650), 2);
        assert_eq!(Columns::boundary_at(&spans, 800), 3);
        let mut moved = columns;
        assert_eq!(moved.move_to(0, 1), None);
        assert_eq!(moved.move_to(0, 3), Some(2));
        assert_eq!(moved.order(), [Column::Size, Column::Time, Column::Name]);
    }
}
