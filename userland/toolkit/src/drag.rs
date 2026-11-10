//! Drag and drop inside one window (docs/ux/toolkit-v0.5.md section 7): what a widget takes
//! when something is dropped on it, and how the highlight of where it will land is drawn.

use crate::canvas::{Canvas, INSET_R4};
use crate::geometry::Rect;

/// How a widget takes a drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropMode {
    /// A row, or the whole widget when it has no rows, takes the drop: a folder, a bin.
    Onto,
    /// The drop goes between rows: reordering.
    Between,
}

/// Where a drag would land now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Onto row `row` (0 for a whole widget) whose rectangle is `rect`.
    Onto { row: usize, rect: Rect },
    /// Before row `at` (`rows` for after the last), the line at y `y` from `left` to `right`.
    Between {
        at: usize,
        y: i32,
        left: i32,
        right: i32,
    },
}

const LINE: i32 = 2;
const LINE_INSET: i32 = 8;

impl Landing {
    /// The rectangle its highlight covers.
    pub fn rect(&self) -> Rect {
        match *self {
            Self::Onto { rect, .. } => rect,
            Self::Between { y, left, right, .. } => Rect::new(left, y, right - left, LINE),
        }
    }

    /// Where a `Between` drop lands in a target at `rect` of `rows` rows, the first row's top at
    /// `first_top` and each `row_height` tall, for the pointer at y `y`: above the row when the
    /// pointer is in its top half, below otherwise.
    pub fn between(rect: Rect, rows: usize, first_top: i32, row_height: i32, y: i32) -> Self {
        let row = ((y - first_top).max(0) / row_height.max(1)) as usize;
        let at = match row >= rows {
            true => rows,
            false => {
                let top = first_top + row as i32 * row_height;
                match y - top < row_height / 2 {
                    true => row,
                    false => row + 1,
                }
            }
        };
        Self::Between {
            at,
            y: first_top + at as i32 * row_height,
            left: rect.x + LINE_INSET,
            right: rect.right() - LINE_INSET,
        }
    }

    /// Draws the highlight over the target, which drew the row under it hovered: a 2 px
    /// `accent` outline round the row, or the 2 px `accent` line.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        let palette = canvas.palette();
        match *self {
            Self::Onto { rect, .. } => canvas.outline(rect, &INSET_R4, 2, palette.accent),
            Self::Between { .. } => canvas.fill(self.rect(), palette.accent),
        }
    }
}
