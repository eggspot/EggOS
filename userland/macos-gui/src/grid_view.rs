//! NSGridView's layout (D385, D732): each column as wide and each row as tall as its widest and
//! tallest cell (or the width and height the app set) plus its padding, the spacing between
//! the shown ones (a hidden track takes no room and no spacing), the span of merged cells, and
//! where a cell's view goes in its cell by the x and y placements. Sizes in points, rectangles
//! with a top-left origin inside the grid.

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
pub fn sizes<const N: usize>(
    tracks: &[Track],
    count: usize,
    cell: impl Fn(usize, usize) -> i32,
) -> [i32; N] {
    let mut out = [0; N];
    for (index, (slot, set)) in out.iter_mut().zip(tracks).enumerate() {
        *slot = set.unwrap_or_else(|| {
            (0..count)
                .map(|other| cell(index, other))
                .max()
                .unwrap_or(0)
        });
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

/// The start of each track of `sizes`, a hidden one (`hidden[i]`) taking no room and no
/// spacing, from 0.
pub fn shown_starts<const N: usize>(sizes: &[i32], hidden: &[bool], spacing: i32) -> [i32; N] {
    let mut out = [0; N];
    let mut at = 0;
    let mut any = false;
    for (index, (slot, &size)) in out.iter_mut().zip(sizes).enumerate() {
        if hidden.get(index).copied().unwrap_or(false) {
            *slot = at;
            continue;
        }
        if any {
            at += spacing;
        }
        *slot = at;
        at += size;
        any = true;
    }
    out
}

/// The whole length of the shown tracks of `sizes`, with `spacing` between them.
pub fn shown_total(sizes: &[i32], hidden: &[bool], spacing: i32) -> i32 {
    let shown = (0..sizes.len()).filter(|&index| !hidden.get(index).copied().unwrap_or(false));
    let (count, sum) = shown.fold((0, 0), |(count, sum), index| (count + 1, sum + sizes[index]));
    sum + spacing * (count - 1).max(0)
}

/// The start and length of tracks `first .. first + count`, laid out at `starts` with `sizes`.
pub fn span(starts: &[i32], sizes: &[i32], first: usize, count: usize) -> (i32, i32) {
    let last = first + count.max(1) - 1;
    (starts[first], starts[last] + sizes[last] - starts[first])
}

/// `cell` less its padding: `leading` and `trailing` across, `top` and `bottom` down.
pub fn padded(cell: Rect, (leading, trailing): (i32, i32), (top, bottom): (i32, i32)) -> Rect {
    Rect::new(
        cell.x + leading,
        cell.y + top,
        (cell.width - leading - trailing).max(0),
        (cell.height - top - bottom).max(0),
    )
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
        let widths: [i32; 2] = sizes(&[None, Some(50)], 2, |column, row| {
            [[10, 30], [5, 5]][column][row]
        });
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
    fn hidden_tracks_take_no_room_and_no_spacing() {
        let hidden = [false, true, false];
        let starts: [i32; 3] = shown_starts(&[20, 0, 30], &hidden, 6);
        assert_eq!(starts, [0, 20, 26]);
        assert_eq!(shown_total(&[20, 0, 30], &hidden, 6), 56);
        assert_eq!(shown_total(&[0], &[true], 6), 0);
    }

    #[test]
    fn a_span_reaches_from_its_first_track_to_its_last() {
        let starts: [i32; 3] = starts(&[20, 20, 20], 6);
        assert_eq!(span(&starts, &[20, 20, 20], 1, 2), (26, 46));
        assert_eq!(span(&starts, &[20, 20, 20], 0, 1), (0, 20));
    }

    #[test]
    fn padding_insets_the_cell() {
        let cell = Rect::new(10, 20, 40, 30);
        assert_eq!(padded(cell, (2, 3), (4, 5)), Rect::new(12, 24, 35, 21));
        assert_eq!(padded(cell, (30, 30), (0, 0)).width, 0);
    }

    #[test]
    fn a_view_is_placed_by_its_placements() {
        let cell = Rect::new(10, 20, 40, 30);
        assert_eq!(
            place(cell, (20, 10), (placement::LEADING, placement::LEADING)),
            Rect::new(10, 20, 20, 10)
        );
        assert_eq!(
            place(cell, (20, 10), (placement::CENTER, placement::TRAILING)),
            Rect::new(20, 40, 20, 10)
        );
        assert_eq!(
            place(cell, (20, 10), (placement::FILL, placement::FILL)),
            cell
        );
    }
}
