//! An Android list's rows (D767, docs/ux/android-lists-v0.1.md sections 1, 4, 6 and 7): where
//! each row (or grid cell) goes in the list's content, from the heights of the rows made so
//! far and the last measured height of each view type for the rest, which rows are made for
//! the visible range and two more on each side, and the pools of row views per view type.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// The shortest row (a touch target).
pub const LIST_ROW_MIN: i32 = 40;
/// Rows made beyond the visible range, on each side.
pub const OVERSCAN: usize = 2;
/// Row views kept for reuse, per view type.
pub const RECYCLE_POOL: usize = 8;
/// The scroll bar's width, which rows leave free.
pub const BAR: i32 = 8;

/// What the list knows of its rows' heights: each made row's, and each type's last.
#[derive(Debug, Default)]
pub struct Heights {
    known: BTreeMap<usize, i32>,
    by_type: BTreeMap<u32, i32>,
}

impl Heights {
    /// Row `position` of type `kind` was made `height` tall (at least the minimum).
    pub fn measured(&mut self, position: usize, kind: u32, height: i32) {
        let height = height.max(LIST_ROW_MIN);
        self.known.insert(position, height);
        self.by_type.insert(kind, height);
    }

    /// Row `position`'s height: measured, else its type's last, else the minimum.
    pub fn of(&self, position: usize, kind: u32) -> i32 {
        self.known
            .get(&position)
            .or_else(|| self.by_type.get(&kind))
            .copied()
            .unwrap_or(LIST_ROW_MIN)
    }

    /// Forgets the rows' own heights (the data changed), keeping each type's.
    pub fn forget_rows(&mut self) {
        self.known.clear();
    }
}

/// Where the cells of a list of `count` rows go: their content rects as `(x, y, width,
/// height)`, `span` cells a row (1 for a plain list), in a list `width` wide, each row of cells
/// as tall as its tallest cell and `gap` apart (a spacing decoration's offsets).
pub fn cells(
    count: usize,
    span: usize,
    width: i32,
    gap: i32,
    height_of: impl Fn(usize) -> i32,
) -> Vec<(i32, i32, i32, i32)> {
    let span = span.max(1);
    let usable = (width - BAR).max(1);
    let cell = usable / span as i32;
    let mut out = Vec::with_capacity(count);
    let mut top = 0;
    for row_start in (0..count).step_by(span) {
        let row_end = (row_start + span).min(count);
        let tallest = (row_start..row_end).map(&height_of).max().unwrap_or(LIST_ROW_MIN);
        for (column, _) in (row_start..row_end).enumerate() {
            let x = cell * column as i32;
            let cell_width = match column + 1 == span {
                true => usable - x,
                false => cell,
            };
            out.push((x, top, cell_width, tallest));
        }
        top += tallest + gap;
    }
    out
}

/// The content's height for `cells`.
pub fn content_height(cells: &[(i32, i32, i32, i32)]) -> i32 {
    cells
        .iter()
        .map(|&(_, y, _, height)| y + height)
        .max()
        .unwrap_or(0)
}

/// The positions to make for a view `view_height` tall scrolled to `offset`: those whose
/// cells touch the view, and [`OVERSCAN`] more on each side (by cell rows of `span`).
pub fn made_range(
    cells: &[(i32, i32, i32, i32)],
    span: usize,
    offset: i32,
    view_height: i32,
) -> core::ops::Range<usize> {
    let span = span.max(1);
    let touching = |&(_, y, _, height): &(i32, i32, i32, i32)| {
        y + height > offset && y < offset + view_height
    };
    let Some(first) = cells.iter().position(touching) else {
        return 0..0;
    };
    let last = cells.iter().rposition(touching).unwrap_or(first);
    let first = first.saturating_sub(OVERSCAN * span) / span * span;
    let last = ((last / span + OVERSCAN + 1) * span).min(cells.len());
    first..last
}

/// Row views kept for reuse, per view type.
#[derive(Debug, Default)]
pub struct Pools {
    by_type: BTreeMap<u32, Vec<u64>>,
}

impl Pools {
    /// A kept row view of type `kind`, if one waits.
    pub fn take(&mut self, kind: u32) -> Option<u64> {
        self.by_type.get_mut(&kind)?.pop()
    }

    /// Keeps `view` of type `kind`; false when the type's pool is full (the view is dropped).
    pub fn give(&mut self, kind: u32, view: u64) -> bool {
        let pool = self.by_type.entry(kind).or_default();
        if pool.len() >= RECYCLE_POOL {
            return false;
        }
        pool.push(view);
        true
    }

    /// Every kept view, for the collector.
    pub fn views(&self) -> impl Iterator<Item = u64> + '_ {
        self.by_type.values().flatten().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thousand_rows_make_twelve_at_the_top_and_slide_on_a_notch() {
        let cells = cells(1000, 1, 360, 0, |_| 56);
        assert_eq!(cells[3], (0, 168, 352, 56));
        assert_eq!(content_height(&cells), 56_000);
        assert_eq!(made_range(&cells, 1, 0, 640), 0..14);
        let after_notch = made_range(&cells, 1, 72, 640);
        assert_eq!(after_notch.start, 0);
        assert_eq!(made_range(&cells, 1, 56 * 500, 640).start, 498);
    }

    #[test]
    fn a_grid_of_three_gives_the_last_column_the_remainder() {
        let cells = cells(7, 3, 360, 0, |_| 40);
        assert_eq!(cells[0], (0, 0, 117, 40));
        assert_eq!(cells[1], (117, 0, 117, 40));
        assert_eq!(cells[2], (234, 0, 118, 40));
        assert_eq!(cells[3], (0, 40, 117, 40));
        assert_eq!(made_range(&cells, 3, 0, 40), 0..7);
    }

    #[test]
    fn heights_fall_back_to_the_type_then_the_minimum_and_pools_cap_at_eight() {
        let mut heights = Heights::default();
        assert_eq!(heights.of(5, 1), LIST_ROW_MIN);
        heights.measured(0, 1, 96);
        assert_eq!(heights.of(5, 1), 96);
        heights.measured(1, 0, 10);
        assert_eq!(heights.of(1, 0), LIST_ROW_MIN);
        let mut pools = Pools::default();
        for view in 0..RECYCLE_POOL as u64 {
            assert!(pools.give(2, view));
        }
        assert!(!pools.give(2, 99));
        assert_eq!(pools.take(2), Some(7));
        assert_eq!(pools.take(3), None);
    }
}
