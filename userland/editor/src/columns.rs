//! Columns of a line (docs/ux/text-editor-v0.3.md sections 4 and 9): the text view's columns,
//! where a tab reaches the next multiple of [`TAB_COLUMNS`] and every other character takes
//! one, and the status bar's column, which counts characters.

use crate::document::is_continuation;

/// Where tabs stop: every 8 columns.
pub const TAB_COLUMNS: usize = 8;

fn advance(column: usize, byte: u8) -> usize {
    match byte {
        b'\t' => (column / TAB_COLUMNS + 1) * TAB_COLUMNS,
        _ => column + 1,
    }
}

/// The text view's column at which byte `at` of `line` (a character boundary) is.
pub fn view_column(line: &[u8], at: usize) -> usize {
    line[..at.min(line.len())]
        .iter()
        .filter(|&&byte| !is_continuation(byte))
        .fold(0, |column, &byte| advance(column, byte))
}

/// How many columns `line` takes in the text view.
pub fn view_width(line: &[u8]) -> usize {
    view_column(line, line.len())
}

/// How many characters of `line` come before byte `at`: the status bar's column, less one.
pub fn character_column(line: &[u8], at: usize) -> usize {
    line[..at.min(line.len())]
        .iter()
        .filter(|&&byte| !is_continuation(byte))
        .count()
}

fn characters(line: &[u8]) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let mut column = 0;
    line.iter()
        .enumerate()
        .filter(|(_, byte)| !is_continuation(**byte))
        .map(move |(at, &byte)| {
            let start = column;
            column = advance(column, byte);
            (at, start, column)
        })
}

/// The byte of `line` to put the caret at for view column `column`, as Up and Down keep it:
/// before the character that covers the column, else the line's end.
pub fn byte_at_column(line: &[u8], column: usize) -> usize {
    characters(line)
        .find(|&(_, _, end)| end > column)
        .map_or(line.len(), |(at, _, _)| at)
}

/// The byte of `line` nearest to `x` pixels from its first column's left edge, columns
/// `cell` pixels wide: a click past a character's middle goes after it.
pub fn byte_nearest(line: &[u8], x: i32, cell: i32) -> usize {
    characters(line)
        .find(|&(_, start, end)| {
            let middle = (start as i32 * cell + end as i32 * cell) / 2;
            x < middle
        })
        .map_or(line.len(), |(at, _, _)| at)
}

/// The byte of the character of `line` whose cells hold `x` pixels from its first column's
/// left edge, columns `cell` pixels wide; the line's end past its last character.
pub fn byte_under(line: &[u8], x: i32, cell: i32) -> usize {
    characters(line)
        .find(|&(_, _, end)| x < end as i32 * cell)
        .map_or(line.len(), |(at, _, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_reach_the_next_stop_and_characters_take_one_column() {
        let line = "a\tbé\t".as_bytes();
        assert_eq!(view_column(line, 0), 0);
        assert_eq!(view_column(line, 1), 1);
        assert_eq!(view_column(line, 2), 8);
        assert_eq!(view_column(line, 3), 9);
        assert_eq!(view_column(line, 5), 10);
        assert_eq!(view_width(line), 16);
        assert_eq!(view_width(b"\t\t"), 16);
        assert_eq!(view_width(b"1234567\t"), 8);
        assert_eq!(view_width(b"12345678\t"), 16);
    }

    #[test]
    fn the_status_bar_counts_characters_and_a_tab_is_one() {
        let line = "a\tbé€".as_bytes();
        assert_eq!(character_column(line, 0), 0);
        assert_eq!(character_column(line, 2), 2);
        assert_eq!(character_column(line, 6), 5);
        assert_eq!(character_column(line, line.len()), 5);
    }

    #[test]
    fn up_and_down_land_before_the_character_covering_the_column() {
        let line = b"ab\tc";
        assert_eq!(byte_at_column(line, 0), 0);
        assert_eq!(byte_at_column(line, 1), 1);
        assert_eq!(byte_at_column(line, 2), 2);
        assert_eq!(byte_at_column(line, 5), 2);
        assert_eq!(byte_at_column(line, 8), 3);
        assert_eq!(byte_at_column(line, 9), 4);
        assert_eq!(byte_at_column(line, 40), 4);
        assert_eq!(byte_at_column(b"", 3), 0);
    }

    #[test]
    fn a_click_goes_to_the_nearest_boundary_and_a_double_click_to_the_cell() {
        let line = b"ab\tc";
        assert_eq!(byte_nearest(line, 0, 8), 0);
        assert_eq!(byte_nearest(line, 3, 8), 0);
        assert_eq!(byte_nearest(line, 4, 8), 1);
        assert_eq!(byte_nearest(line, 39, 8), 2);
        assert_eq!(byte_nearest(line, 40, 8), 3);
        assert_eq!(byte_nearest(line, 500, 8), 4);
        assert_eq!(byte_nearest(line, -5, 8), 0);
        assert_eq!(byte_under(line, 7, 8), 0);
        assert_eq!(byte_under(line, 8, 8), 1);
        assert_eq!(byte_under(line, 60, 8), 2);
        assert_eq!(byte_under(line, 64, 8), 3);
        assert_eq!(byte_under(line, 72, 8), 4);
    }
}
