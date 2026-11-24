//! The rows of an `LVS_OWNERDATA` list view (D747), which only its program knows: how many
//! there are, which of them are selected (as ranges, so selecting a million rows costs one
//! range) and focused, and the window of at most [`MAX_ROWS`] of them the [`ListView`] model
//! holds as callback rows, starting at `base`. The window moves to keep the shown rows away
//! from its edges; the selection inside it lives in the model's row states and is folded back
//! into the ranges before it moves. Pure logic with host tests.

use eggos_windows_gui::controls::list_view::{ListView, MAX_ROWS, StateChange, state};
use alloc::vec::Vec;

/// How close to the window's edge the shown rows may come before it moves.
pub const MARGIN: usize = 64;

/// What a virtual list knows about its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VirtualRows {
    pub total: usize,
    pub base: usize,
    selected: Vec<(usize, usize)>,
    focused: Option<usize>,
}

impl VirtualRows {
    /// How many rows the window holds.
    pub fn window_length(&self) -> usize {
        self.total.saturating_sub(self.base).min(MAX_ROWS)
    }

    /// Whether row `index` lies in the window.
    pub fn in_window(&self, index: usize) -> bool {
        index >= self.base && index < self.base + self.window_length()
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected
            .iter()
            .any(|&(start, end)| index >= start && index < end)
    }

    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// Selects or deselects rows `start..end`, merging the ranges.
    pub fn set_selected(&mut self, (start, end): (usize, usize), on: bool) {
        let end = end.min(self.total);
        if start >= end {
            return;
        }
        let mut kept: Vec<(usize, usize)> = Vec::with_capacity(self.selected.len() + 2);
        for &(from, to) in &self.selected {
            if to <= start || from >= end {
                kept.push((from, to));
                continue;
            }
            if from < start {
                kept.push((from, start));
            }
            if to > end {
                kept.push((end, to));
            }
        }
        if on {
            kept.push((start, end));
        }
        kept.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(kept.len());
        for (from, to) in kept {
            match merged.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        self.selected = merged;
    }

    /// How many rows are selected.
    pub fn selected_count(&self) -> usize {
        self.selected.iter().map(|&(start, end)| end - start).sum()
    }

    /// The model's states of row `index` from the ranges and the focus.
    pub fn state_of(&self, index: usize) -> u32 {
        let selected = match self.is_selected(index) {
            true => state::SELECTED,
            false => 0,
        };
        let focused = match self.focused == Some(index) {
            true => state::FOCUSED,
            false => 0,
        };
        selected | focused
    }

    /// Folds the window's row states in `list` back into the ranges and the focus.
    pub fn remember(&mut self, list: &ListView) {
        let length = list.count().min(self.window_length());
        if self.focused.is_some_and(|focused| self.in_window(focused)) {
            self.focused = None;
        }
        self.set_selected((self.base, self.base + length), false);
        let mut run: Option<usize> = None;
        for index in 0..=length {
            let selected = index < length && list.state(index) & state::SELECTED != 0;
            if index < length && list.state(index) & state::FOCUSED != 0 {
                self.focused = Some(self.base + index);
            }
            match (run, selected) {
                (None, true) => run = Some(index),
                (Some(from), false) => {
                    self.set_selected((self.base + from, self.base + index), true);
                    run = None;
                }
                _ => {}
            }
        }
    }

    /// The base that puts row `index` a quarter into the window, as far as the rows allow.
    pub fn base_for(&self, index: usize) -> usize {
        let latest = self.total.saturating_sub(MAX_ROWS);
        index.saturating_sub(MAX_ROWS / 4).min(latest)
    }

    /// The base the window should move to so rows `top..top + visible` (model indices) stay
    /// [`MARGIN`] rows away from its edges; `None` when it may stay.
    pub fn follow(&self, top: usize, visible: usize) -> Option<usize> {
        let length = self.window_length();
        let near_end = top + visible + MARGIN > length && self.base + length < self.total;
        let near_start = top < MARGIN && self.base > 0;
        (near_end || near_start)
            .then(|| self.base_for(self.base + top))
            .filter(|&base| base != self.base)
    }

    /// Clears every row: the count, the selection and the focus.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Fills `list` with the window's rows: every cell of the `columns` a callback (`mark`), the
/// image a callback (`image`), the states from `rows`.
pub fn fill(list: &mut ListView, rows: &VirtualRows, columns: usize, (mark, image): (&[u16], i32)) {
    list.clear();
    for index in 0..rows.window_length() {
        let Some(at) = list.insert(index, mark, image, 0) else {
            break;
        };
        for column in 1..columns {
            list.set_text(at, column, mark);
        }
        let wanted = rows.state_of(rows.base + index);
        if wanted != 0 {
            list.apply(&StateChange {
                index: at,
                old: 0,
                new: wanted,
                lparam: 0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_split() {
        let mut rows = VirtualRows {
            total: 1000,
            ..VirtualRows::default()
        };
        rows.set_selected((10, 20), true);
        rows.set_selected((15, 30), true);
        assert_eq!(rows.selected_count(), 20);
        rows.set_selected((12, 14), false);
        assert!(rows.is_selected(11) && !rows.is_selected(12) && rows.is_selected(14));
        assert_eq!(rows.selected_count(), 18);
        rows.set_selected((0, 5000), true);
        assert_eq!(rows.selected_count(), 1000);
    }

    #[test]
    fn the_window_follows_the_view() {
        let rows = VirtualRows {
            total: 100_000,
            base: 0,
            ..VirtualRows::default()
        };
        assert_eq!(rows.follow(10, 20), None);
        let moved = rows.follow(MAX_ROWS - MARGIN, 20).unwrap();
        assert_eq!(moved, MAX_ROWS - MARGIN - MAX_ROWS / 4);
        let small = VirtualRows {
            total: 100,
            ..VirtualRows::default()
        };
        assert_eq!(small.follow(90, 20), None);
        assert_eq!(small.window_length(), 100);
    }

    #[test]
    fn the_window_keeps_its_selection_across_moves() {
        let mut rows = VirtualRows {
            total: 10_000,
            ..VirtualRows::default()
        };
        rows.set_selected((5, 8), true);
        let mut list = ListView::new();
        fill(&mut list, &rows, 1, (&[0xffff], -1));
        assert_eq!(list.count(), MAX_ROWS);
        assert_eq!(list.state(6) & state::SELECTED, state::SELECTED);
        list.apply(&StateChange {
            index: 9,
            old: 0,
            new: state::SELECTED | state::FOCUSED,
            lparam: 0,
        });
        rows.remember(&list);
        assert!(rows.is_selected(9));
        assert_eq!(rows.focused(), Some(9));
        rows.base = 5000;
        fill(&mut list, &rows, 1, (&[0xffff], -1));
        assert_eq!(list.state(0), 0);
        assert_eq!(rows.selected_count(), 4);
    }
}
