//! NSSplitView's geometry (D751, docs/ux/macos-containers-v0.1.md section 2): panes laid out
//! along the split axis with a divider between each two, the 9-pixel pointer area centred on
//! each divider, a divider moved within its clamps (only its two neighbours change), and the
//! panes sharing a change of the view's length in proportion. Lengths are pixels along the
//! split axis (columns for a vertical split, rows for a horizontal one).

use eggos_toolkit::geometry::Rect;

/// The pointer area across every divider.
pub const SPLIT_HANDLE: i32 = 9;
/// A thin divider's thickness.
pub const THIN: i32 = 1;
/// A thick or pane-splitter divider's thickness.
pub const THICK: i32 = 9;
/// The grip dots' size and spacing on a thick divider.
pub const GRIP_DOT: i32 = 2;
pub const GRIP_SPACING: i32 = 4;
/// The most panes laid out.
pub const MOST_PANES: usize = 16;

/// `NSSplitViewDividerStyle`'s values.
pub mod style {
    pub const THICK: usize = 1;
    pub const THIN: usize = 2;
    pub const PANE_SPLITTER: usize = 3;
}

/// The thickness of a divider of `style_number`.
pub fn thickness(style_number: usize) -> i32 {
    match style_number {
        style::THICK | style::PANE_SPLITTER => THICK,
        _ => THIN,
    }
}

/// Where pane `index` starts along the axis, given every pane's length.
pub fn pane_start(lengths: &[i32], index: usize, divider: i32) -> i32 {
    lengths[..index.min(lengths.len())].iter().sum::<i32>() + divider * index as i32
}

/// Where divider `index` (between pane `index` and the next) starts along the axis.
pub fn divider_start(lengths: &[i32], index: usize, divider: i32) -> i32 {
    pane_start(lengths, index, divider) + lengths.get(index).copied().unwrap_or(0)
}

/// Pane `index`'s rect in a split view `across` pixels wide the other way.
pub fn pane_rect(lengths: &[i32], index: usize, divider: i32, across: i32, vertical: bool) -> Rect {
    let start = pane_start(lengths, index, divider);
    let length = lengths.get(index).copied().unwrap_or(0);
    along_rect(start, length, across, vertical)
}

/// Divider `index`'s drawn rect.
pub fn divider_rect(
    lengths: &[i32],
    index: usize,
    divider: i32,
    across: i32,
    vertical: bool,
) -> Rect {
    along_rect(divider_start(lengths, index, divider), divider, across, vertical)
}

/// Divider `index`'s pointer area: `SPLIT_HANDLE` pixels centred on it.
pub fn handle_rect(
    lengths: &[i32],
    index: usize,
    divider: i32,
    across: i32,
    vertical: bool,
) -> Rect {
    let middle = divider_start(lengths, index, divider) + divider / 2;
    along_rect(middle - SPLIT_HANDLE / 2, SPLIT_HANDLE, across, vertical)
}

fn along_rect(start: i32, length: i32, across: i32, vertical: bool) -> Rect {
    match vertical {
        true => Rect::new(start, 0, length, across),
        false => Rect::new(0, start, across, length),
    }
}

/// The divider whose pointer area holds `along` (the pointer's coordinate along the axis).
pub fn divider_at(lengths: &[i32], divider: i32, along: i32) -> Option<usize> {
    (0..lengths.len().saturating_sub(1)).find(|&index| {
        let middle = divider_start(lengths, index, divider) + divider / 2;
        (middle - SPLIT_HANDLE / 2..=middle + SPLIT_HANDLE / 2).contains(&along)
    })
}

/// The range divider `index` may start in: between the end of pane `index`'s minimum and the
/// start the next pane's minimum leaves.
pub fn divider_range(
    lengths: &[i32],
    index: usize,
    divider: i32,
    minimums: (i32, i32),
) -> (i32, i32) {
    let first = pane_start(lengths, index, divider);
    let next_end = pane_start(lengths, index + 1, divider)
        + lengths.get(index + 1).copied().unwrap_or(0);
    let low = first + minimums.0.max(0);
    let high = (next_end - divider - minimums.1.max(0)).max(low);
    (low, high)
}

/// Moves divider `index` to start at `position`, already clamped by the caller: pane `index`
/// and the next share their joint length, nothing else moves.
pub fn move_divider(lengths: &mut [i32], index: usize, divider: i32, position: i32) {
    if index + 1 >= lengths.len() {
        return;
    }
    let joint = lengths[index] + lengths[index + 1];
    let first = (position - pane_start(lengths, index, divider)).clamp(0, joint);
    lengths[index] = first;
    lengths[index + 1] = joint - first;
}

/// Fits the panes to a view `total` long: the panes not `fixed` share what the dividers leave
/// in proportion to their lengths (evenly when all are 0), the last of them taking the
/// rounding.
pub fn fit(lengths: &mut [i32], total: i32, divider: i32, fixed: &[bool]) {
    let count = lengths.len();
    if count == 0 {
        return;
    }
    let room = (total - divider * (count as i32 - 1)).max(0);
    let held: i32 = lengths
        .iter()
        .zip(fixed.iter().chain(core::iter::repeat(&false)))
        .filter(|&(_, &fixed)| fixed)
        .map(|(&length, _)| length)
        .sum();
    let free = (room - held).max(0);
    let is_flexible = |index: usize| !fixed.get(index).copied().unwrap_or(false);
    let flexible_count = (0..count).filter(|&index| is_flexible(index)).count();
    if flexible_count == 0 {
        return;
    }
    let old: i64 = (0..count)
        .filter(|&index| is_flexible(index))
        .map(|index| i64::from(lengths[index]))
        .sum();
    let mut given = 0;
    let mut order = 0;
    for index in (0..count).filter(|&index| is_flexible(index)) {
        order += 1;
        let share = if order == flexible_count {
            free - given
        } else if old == 0 {
            free / flexible_count as i32
        } else {
            (i64::from(lengths[index]) * i64::from(free) / old) as i32
        };
        lengths[index] = share.max(0);
        given += lengths[index];
    }
}

/// The three grip dots' rects on a thick divider `rect`.
pub fn grip_dots(rect: Rect, vertical: bool) -> [Rect; 3] {
    let middle_x = rect.x + rect.width / 2;
    let middle_y = rect.y + rect.height / 2;
    core::array::from_fn(|index| {
        let step = (index as i32 - 1) * GRIP_SPACING;
        let (x, y) = match vertical {
            true => (middle_x, middle_y + step),
            false => (middle_x + step, middle_y),
        };
        Rect::new(x - GRIP_DOT / 2, y - GRIP_DOT / 2, GRIP_DOT, GRIP_DOT)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worked_example_lays_out_as_designed() {
        let mut lengths = [200, 279];
        assert_eq!(pane_rect(&lengths, 1, THIN, 360, true), Rect::new(201, 0, 279, 360));
        assert_eq!(divider_rect(&lengths, 0, THIN, 360, true), Rect::new(200, 0, 1, 360));
        assert_eq!(handle_rect(&lengths, 0, THIN, 360, true), Rect::new(196, 0, 9, 360));
        assert_eq!(divider_at(&lengths, THIN, 198), Some(0));
        assert_eq!(divider_at(&lengths, THIN, 210), None);
        move_divider(&mut lengths, 0, THIN, 150);
        assert_eq!(lengths, [150, 329]);
        let (low, high) = divider_range(&lengths, 0, THIN, (120, 200));
        assert_eq!((low, high), (120, 279));
        move_divider(&mut lengths, 0, THIN, 50_i32.clamp(low, high));
        assert_eq!(lengths, [120, 359]);
        move_divider(&mut lengths, 0, THIN, 0);
        assert_eq!(lengths, [0, 479]);
    }

    #[test]
    fn fitting_shares_the_change_in_proportion() {
        let mut lengths = [100, 300];
        fit(&mut lengths, 801, THIN, &[]);
        assert_eq!(lengths, [200, 600]);
        let mut fixed = [100, 300];
        fit(&mut fixed, 501, THIN, &[true, false]);
        assert_eq!(fixed, [100, 400]);
        let mut empty = [0, 0, 0];
        fit(&mut empty, 302, THIN, &[]);
        assert_eq!(empty, [100, 100, 100]);
    }

    #[test]
    fn thick_dividers_have_three_dots() {
        let dots = grip_dots(Rect::new(100, 0, 9, 360), true);
        assert_eq!(dots[1], Rect::new(103, 179, 2, 2));
        assert_eq!(dots[0].y + GRIP_SPACING, dots[1].y);
        assert_eq!(thickness(style::THICK), 9);
        assert_eq!(thickness(style::THIN), 1);
    }
}
