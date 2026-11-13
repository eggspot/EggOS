//! NSTableView's geometry (D373, docs/ux/macos-gui-v0.3.md section 11): the header kept at
//! the top of the clip view, the columns' places, the resize handles, the rows under the
//! offset, the alternating fills and the grid, and how keys move the selection. Rectangles are
//! in the clip view's pixels, top-left origin, the clip view's left edge at 0.

use eggos_toolkit::geometry::Rect;
use eggos_toolkit::widget::{
    TABLE_HANDLE, TABLE_HEADER_HEIGHT, TABLE_MIN_COLUMN, TABLE_ROW_HEIGHT,
};

/// `NSTableViewSolidVerticalGridLineMask`.
pub const VERTICAL_GRID: usize = 1;
/// `NSTableViewSolidHorizontalGridLineMask`.
pub const HORIZONTAL_GRID: usize = 2;
/// `NSTableViewDashedHorizontalGridLineMask`, drawn solid.
pub const DASHED_HORIZONTAL_GRID: usize = 8;

/// Where the body's rows start below the clip view's top.
pub fn body_top(header: bool) -> i32 {
    if header { TABLE_HEADER_HEIGHT } else { 0 }
}

/// The table's height: the header and every row.
pub fn table_height(header: bool, rows: usize) -> i32 {
    let rows = (rows as i64 * i64::from(TABLE_ROW_HEIGHT)).min(i64::from(i32::MAX / 2)) as i32;
    body_top(header) + rows
}

/// The left edges of the shown columns of `widths`, scrolled left by `scroll_x`, in order.
pub fn column_lefts<const N: usize>(widths: &[i32], scroll_x: i32) -> [i32; N] {
    let mut lefts = [0; N];
    let mut x = -scroll_x;
    for (left, &width) in lefts.iter_mut().zip(widths) {
        *left = x;
        x += width;
    }
    lefts
}

/// The header's rect in a clip view `width` wide.
pub fn header_rect(width: i32) -> Rect {
    Rect::new(0, 0, width, TABLE_HEADER_HEIGHT)
}

/// The column whose resize handle holds header point `x` (the 7 pixels centred on its right
/// separator), of columns at `lefts` with `widths`.
pub fn handle_at(x: i32, lefts: &[i32], widths: &[i32]) -> Option<usize> {
    let half = TABLE_HANDLE / 2;
    lefts.iter().zip(widths).position(|(&left, &width)| {
        let separator = left + width;
        x >= separator - half - 1 && x < separator + half
    })
}

/// The column holding point `x`, of columns at `lefts` with `widths`.
pub fn column_at(x: i32, lefts: &[i32], widths: &[i32]) -> Option<usize> {
    lefts
        .iter()
        .zip(widths)
        .position(|(&left, &width)| x >= left && x < left + width)
}

/// Row `row`'s rect in a clip view `width` wide, scrolled `offset` from the top.
pub fn row_rect(row: usize, header: bool, width: i32, offset: i32) -> Rect {
    let y = body_top(header) + row as i32 * TABLE_ROW_HEIGHT - offset;
    Rect::new(0, y, width, TABLE_ROW_HEIGHT)
}

/// The rows a body `view` pixels tall (the clip view less the header) shows at `offset`:
/// first and one past the last, of `rows`.
pub fn shown_rows(rows: usize, view: i32, offset: i32) -> (usize, usize) {
    let first = (offset.max(0) / TABLE_ROW_HEIGHT) as usize;
    let last = ((offset.max(0) + view.max(0) + TABLE_ROW_HEIGHT - 1) / TABLE_ROW_HEIGHT) as usize;
    (first.min(rows), last.min(rows))
}

/// The row under clip-view point `y` at `offset`, if the point is on a row.
pub fn row_at(y: i32, header: bool, offset: i32, rows: usize) -> Option<usize> {
    let inside = y - body_top(header) + offset;
    if y < body_top(header) || inside < 0 {
        return None;
    }
    let row = (inside / TABLE_ROW_HEIGHT) as usize;
    (row < rows).then_some(row)
}

/// Whether row `row` takes the alternate fill.
pub fn is_alternate(row: usize) -> bool {
    row % 2 == 1
}

/// A column's width after a resize drag of `moved` pixels from `start`, kept between its
/// minimum (at least the toolkit's 40) and maximum.
pub fn resized(start: i32, moved: i32, minimum: i32, maximum: i32) -> i32 {
    let minimum = minimum.max(TABLE_MIN_COLUMN);
    (start + moved).clamp(minimum, maximum.max(minimum))
}

/// The key that moves a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// The selection after `step` from `selected` (-1 for none) in a table of `rows` showing
/// `visible` whole rows; `None` when the table is empty.
pub fn moved(selected: isize, step: Move, rows: usize, visible: usize) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    let last = rows - 1;
    let page = visible.saturating_sub(1).max(1);
    let current = usize::try_from(selected).ok();
    Some(match (step, current) {
        (Move::Down, None) | (Move::Home, _) => 0,
        (Move::Up, None) | (Move::End, _) => last,
        (Move::Down, Some(row)) => (row + 1).min(last),
        (Move::Up, Some(row)) => row.saturating_sub(1),
        (Move::PageDown, Some(row)) => (row + page).min(last),
        (Move::PageDown, None) => page.min(last),
        (Move::PageUp, Some(row)) => row.saturating_sub(page),
        (Move::PageUp, None) => 0,
    })
}

/// The least offset that shows row `row` whole in a body `view` tall now at `offset`.
pub fn reveal_row(row: usize, view: i32, offset: i32, rows: usize) -> i32 {
    let top = row as i32 * TABLE_ROW_HEIGHT;
    let most = (rows as i32 * TABLE_ROW_HEIGHT - view).max(0);
    let wanted = if top < offset {
        top
    } else if top + TABLE_ROW_HEIGHT > offset + view {
        top + TABLE_ROW_HEIGHT - view
    } else {
        offset
    };
    wanted.clamp(0, most)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worked_example_s_columns_header_and_handles() {
        let widths = [160, 100];
        let lefts: [i32; 2] = column_lefts(&widths, 0);
        assert_eq!(lefts, [0, 160]);
        assert_eq!(header_rect(270), Rect::new(0, 0, 270, 28));
        assert_eq!(handle_at(350 - 193, &lefts, &widths), Some(0));
        assert_eq!(handle_at(346 - 193, &lefts, &widths), None);
        assert_eq!(column_at(346 - 193, &lefts, &widths), Some(0));
    }

    #[test]
    fn rows_sit_under_the_header_and_follow_the_offset() {
        assert_eq!(row_rect(0, true, 262, 0), Rect::new(0, 28, 262, 24));
        assert_eq!(row_rect(9, true, 262, 216), Rect::new(0, 28, 262, 24));
        assert_eq!(shown_rows(20, 258, 0), (0, 11));
        assert_eq!(shown_rows(20, 258, 216), (9, 20));
        assert_eq!(table_height(true, 20), 508);
    }

    #[test]
    fn a_press_finds_its_row() {
        assert_eq!(row_at(150 - 17, true, 0, 20), Some(4));
        assert_eq!(row_at(10, true, 0, 20), None);
        assert_eq!(row_at(28 + 24 * 3, true, 0, 3), None);
    }

    #[test]
    fn odd_rows_take_the_alternate_fill() {
        assert!(!is_alternate(0));
        assert!(is_alternate(1));
    }

    #[test]
    fn keys_move_the_selection() {
        assert_eq!(moved(-1, Move::Down, 20, 10), Some(0));
        assert_eq!(moved(-1, Move::Up, 20, 10), Some(19));
        assert_eq!(moved(4, Move::PageDown, 20, 10), Some(13));
        assert_eq!(moved(19, Move::Down, 20, 10), Some(19));
        assert_eq!(moved(0, Move::End, 0, 10), None);
    }

    #[test]
    fn end_scrolls_the_last_row_into_view() {
        assert_eq!(reveal_row(19, 258, 0, 20), 222);
        assert_eq!(reveal_row(4, 258, 216, 20), 96);
    }

    #[test]
    fn a_resize_keeps_the_width_between_its_bounds() {
        assert_eq!(resized(160, 20, 10, 1000), 180);
        assert_eq!(resized(160, -200, 10, 1000), 40);
        assert_eq!(resized(160, 2000, 10, 1000), 1000);
    }
}
