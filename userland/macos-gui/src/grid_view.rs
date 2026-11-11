//! NSGridView's layout (D385): each column as wide and each row as tall as its widest and
//! tallest cell (or the width and height the app set), the spacing between them, and where a
//! cell's view goes in its cell by the x and y placements. Sizes in points, rectangles with a
//! top-left origin inside the grid.

use eggos_toolkit::geometry::Rect;

/// `NSGridCellPlacement`.
pub mod placement {
    pub const INHERITED: usize = 0;
    pub const NONE: usize = 1;
    pub const LEADING: usize = 2;
    pub const TRAILING: usize = 3;
    pub const CENTER: usize = 4;
    pub const FILL: usize = 5;
}

/// What the grid's spacing is unless the app sets it.
pub const DEFAULT_SPACING: i32 = 6;

/// A column's or row's size: what the app set, or `None` for the largest cell's.
pub type Track = Option<i32>;

/// The sizes of `tracks` tracks (columns or rows): each the one the app set, else its largest
/// cell's along that axis (`cell(track, index)` for each of `count` cells in it).
pub fn sizes<const N: usize>(tracks: &[Track], count: usize, cell: impl Fn(usize, usize) -> i32) -> [i32; N] {
    let mut out = [0; N];
    for (index, (slot, set)) in out.iter_mut().zip(tracks).enumerate() {
        *slot = set.unwrap_or_else(|| (0..count).map(|other| cell(index, other)).max().unwrap_or(0));
    }
    out
}

/// The start of each track of `sizes` with `spacing` between them, from 0.
pub fn starts<const N: usize>(sizes: &[i32], spacing: i32) -> [i32; N] {
    let mut out = [0; N];
    let mut at = 0;
    for (slot, &size) in out.iter_mut().zip(sizes) {
        *slot = at;
        at += size + spacing;
    }
    out
}

/// The whole length of tracks of `sizes` with `spacing` between them.
pub fn total(sizes: &[i32], spacing: i32) -> i32 {
    let tracks = sizes.len() as i32;
    sizes.iter().sum::<i32>() + spacing * (tracks - 1).max(0)
}

fn place_on_axis(start: i32, room: i32, size: i32, placement: usize) -> (i32, i32) {
    match placement {
        placement::TRAILING => (start + room - size, size),
        placement::CENTER => (start + (room - size) / 2, size),
        placement::FILL => (start, room),
        _ => (start, size),
    }
}

/// Where a view of `size` goes in the cell `cell`, by `x` and `y` placements (`LEADING` and
/// `TOP` share a value, as do `TRAILING` and `BOTTOM`).
pub fn place(cell: Rect, size: (i32, i32), (x, y): (usize, usize)) -> Rect {
    let (left, width) = place_on_axis(cell.x, cell.width, size.0, x);
    let (top, height) = place_on_axis(cell.y, cell.height, size.1, y);
    Rect::new(left, top, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_take_their_largest_cell_unless_set() {
        let widths: [i32; 2] = sizes(&[None, Some(50)], 2, |column, row| [[10, 30], [5, 5]][column][row]);
        assert_eq!(widths, [30, 50]);
    }

    #[test]
    fn tracks_follow_one_another_with_spacing() {
        let starts: [i32; 3] = starts(&[20, 20, 20], 6);
        assert_eq!(starts, [0, 26, 52]);
        assert_eq!(total(&[20, 20, 20], 6), 72);
        assert_eq!(total(&[], 6), 0);
    }

    #[test]
    fn a_view_is_placed_by_its_placements() {
        let cell = Rect::new(10, 20, 40, 30);
        assert_eq!(place(cell, (20, 10), (placement::LEADING, placement::LEADING)), Rect::new(10, 20, 20, 10));
        assert_eq!(place(cell, (20, 10), (placement::CENTER, placement::TRAILING)), Rect::new(20, 40, 20, 10));
        assert_eq!(place(cell, (20, 10), (placement::FILL, placement::FILL)), cell);
    }
}
