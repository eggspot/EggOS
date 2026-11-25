//! NSTabView's geometry (D751, docs/ux/macos-containers-v0.1.md section 1): the segmented strip
//! of tabs, centred on the frame and straddling the content box's line, the box, the selected
//! item's content rect for each `tabViewType`, where each label sits, the separators drawn
//! between segments and the focus ring. Rectangles are content pixels, top-left origin.

use eggos_toolkit::geometry::Rect;

/// The strip's height (a segment's, for a side strip) on the desktop.
pub const TAB_STRIP_HEIGHT: i32 = 28;
/// The strip's height on a touch screen.
pub const TOUCH_STRIP_HEIGHT: i32 = 40;
/// The narrowest segment.
pub const TAB_MIN_WIDTH: i32 = 48;
/// The room on each side of a label.
pub const LABEL_PADDING: i32 = 12;
/// The content's inset inside the box.
pub const CONTENT_INSET: i32 = 8;
/// The widest side strip.
pub const SIDE_STRIP_MOST: i32 = 160;
/// The shortest segment of a side strip cut to fit.
pub const SHORTEST_SIDE_SEGMENT: i32 = 20;
/// The label's top inside a segment.
pub const LABEL_TOP: i32 = 6;
/// The rows of a separator inside the strip, from its top.
pub const SEPARATOR_ROWS: (i32, i32) = (6, 22);
/// The focus ring's inset inside the selected segment.
pub const RING_INSET: i32 = 2;
/// The most tabs laid out.
pub const MOST_TABS: usize = 64;

/// `NSTabViewType`'s values.
pub mod kind {
    pub const TOP: usize = 0;
    pub const LEFT: usize = 1;
    pub const BOTTOM: usize = 2;
    pub const RIGHT: usize = 3;
    pub const NO_TABS_BEZEL: usize = 4;
    pub const NO_TABS_LINE: usize = 5;
    pub const NO_TABS_NO_BORDER: usize = 6;
}

/// How the content box is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxLook {
    Filled,
    Line,
    Nothing,
}

/// Where a tab view's parts are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub strip: Option<Rect>,
    pub segments: [Rect; MOST_TABS],
    pub count: usize,
    pub side: bool,
    pub content_box: Rect,
    pub look: BoxLook,
    pub content: Rect,
}

impl Layout {
    /// The segments laid out.
    pub fn segments(&self) -> &[Rect] {
        &self.segments[..self.count]
    }

    /// The tab under the content pixel `(x, y)`.
    pub fn segment_at(&self, x: i32, y: i32) -> Option<usize> {
        self.segments().iter().position(|rect| rect.contains(x, y))
    }

    /// The separators drawn beside no selected segment: a column between two segments of a
    /// top or bottom strip, a row between two of a side strip.
    pub fn separators(&self, selected: Option<usize>) -> impl Iterator<Item = Rect> + '_ {
        let side = self.side;
        self.segments()
            .windows(2)
            .enumerate()
            .filter(move |&(index, _)| selected != Some(index) && selected != Some(index + 1))
            .map(move |(_, pair)| {
                let segment = pair[0];
                match side {
                    false => Rect::new(
                        segment.x + segment.width - 1,
                        segment.y + SEPARATOR_ROWS.0,
                        1,
                        SEPARATOR_ROWS.1 - SEPARATOR_ROWS.0,
                    ),
                    true => Rect::new(
                        segment.x + SEPARATOR_ROWS.0,
                        segment.y + segment.height - 1,
                        segment.width - 2 * SEPARATOR_ROWS.0 + 1,
                        1,
                    ),
                }
            })
    }

    /// The focus ring's rect around segment `index`.
    pub fn ring(&self, index: usize) -> Option<Rect> {
        self.segments().get(index).map(|segment| {
            Rect::new(
                segment.x + RING_INSET,
                segment.y + RING_INSET,
                segment.width - 2 * RING_INSET,
                segment.height - 2 * RING_INSET,
            )
        })
    }
}

/// Where a label `label_width` wide starts in `segment` (its left and top) and the room it
/// has before its ellipsis.
pub fn label_place(segment: Rect, label_width: i32) -> (i32, i32, i32) {
    let room = segment.width - 2 * LABEL_PADDING;
    let left = match label_width <= room {
        true => segment.x + (segment.width - label_width) / 2,
        false => segment.x + LABEL_PADDING,
    };
    (left, segment.y + LABEL_TOP, room)
}

fn wide_segments(label_widths: &[i32], room: i32) -> ([i32; MOST_TABS], i32) {
    let mut widths = [0; MOST_TABS];
    let count = label_widths.len().min(MOST_TABS);
    for (width, &label) in widths.iter_mut().zip(label_widths) {
        *width = (label + 2 * LABEL_PADDING).max(TAB_MIN_WIDTH);
    }
    let sum: i32 = widths[..count].iter().sum();
    if count > 0 && sum > room {
        let each = (room / count as i32).max(1);
        for width in &mut widths[..count] {
            *width = each;
        }
        return (widths, each * count as i32);
    }
    (widths, sum)
}

/// Lays a tab view out in `frame` (its own content pixels): `kind` is its `tabViewType`,
/// `label_widths` its items' label widths in pixels, `strip_height` 28 (40 on touch).
pub fn layout(frame: Rect, kind_number: usize, label_widths: &[i32], strip_height: i32) -> Layout {
    let count = label_widths.len().min(MOST_TABS);
    let mut laid = Layout {
        strip: None,
        segments: [Rect::new(0, 0, 0, 0); MOST_TABS],
        count: 0,
        side: false,
        content_box: frame,
        look: BoxLook::Filled,
        content: inset(frame, CONTENT_INSET),
    };
    let half = strip_height / 2;
    let (x, y, w, h) = (frame.x, frame.y, frame.width, frame.height);
    match kind_number {
        kind::NO_TABS_BEZEL => return laid,
        kind::NO_TABS_LINE => {
            laid.look = BoxLook::Line;
            return laid;
        }
        kind::NO_TABS_NO_BORDER => {
            laid.look = BoxLook::Nothing;
            laid.content = frame;
            return laid;
        }
        kind::LEFT | kind::RIGHT => {
            let widest = label_widths[..count].iter().copied().max().unwrap_or(0);
            let strip_width = (widest + 2 * LABEL_PADDING)
                .max(TAB_MIN_WIDTH)
                .min(SIDE_STRIP_MOST);
            let mut segment_height = strip_height;
            if count > 0 && segment_height * count as i32 > h - 2 * CONTENT_INSET {
                segment_height =
                    ((h - 2 * CONTENT_INSET) / count as i32).max(SHORTEST_SIDE_SEGMENT);
            }
            let strip_length = segment_height * count as i32;
            let left = match kind_number {
                kind::LEFT => x,
                _ => x + w - strip_width,
            };
            let top = y + (h - strip_length) / 2;
            laid.side = true;
            laid.content_box = match kind_number {
                kind::LEFT => Rect::new(x + strip_width / 2, y, w - strip_width / 2, h),
                _ => Rect::new(x, y, w - strip_width / 2, h),
            };
            let content_left = match kind_number {
                kind::LEFT => x + strip_width / 2 + CONTENT_INSET,
                _ => x + CONTENT_INSET,
            };
            laid.content = Rect::new(
                content_left,
                y + CONTENT_INSET,
                w - strip_width / 2 - 2 * CONTENT_INSET,
                h - 2 * CONTENT_INSET,
            );
            if count > 0 {
                laid.strip = Some(Rect::new(left, top, strip_width, strip_length));
                for (index, segment) in laid.segments[..count].iter_mut().enumerate() {
                    *segment = Rect::new(
                        left,
                        top + segment_height * index as i32,
                        strip_width,
                        segment_height,
                    );
                }
                laid.count = count;
            }
            return laid;
        }
        _ => {}
    }
    let bottom = kind_number == kind::BOTTOM;
    laid.content_box = match bottom {
        false => Rect::new(x, y + half, w, h - half),
        true => Rect::new(x, y, w, h - half),
    };
    laid.content = match bottom {
        false => Rect::new(
            x + CONTENT_INSET,
            y + strip_height + CONTENT_INSET,
            w - 2 * CONTENT_INSET,
            h - strip_height - 2 * CONTENT_INSET,
        ),
        true => Rect::new(
            x + CONTENT_INSET,
            y + CONTENT_INSET,
            w - 2 * CONTENT_INSET,
            h - strip_height - 2 * CONTENT_INSET,
        ),
    };
    if count == 0 {
        return laid;
    }
    let (widths, strip_width) = wide_segments(&label_widths[..count], w - 2 * CONTENT_INSET);
    let left = x + (w - strip_width) / 2;
    let top = match bottom {
        false => y,
        true => y + h - strip_height,
    };
    laid.strip = Some(Rect::new(left, top, strip_width, strip_height));
    let mut at = left;
    for (segment, &width) in laid.segments[..count].iter_mut().zip(&widths) {
        *segment = Rect::new(at, top, width, strip_height);
        at += width;
    }
    laid.count = count;
    laid
}

fn inset(rect: Rect, by: i32) -> Rect {
    Rect::new(
        rect.x + by,
        rect.y + by,
        rect.width - 2 * by,
        rect.height - 2 * by,
    )
}

/// The index Ctrl+Tab (`forward`) or Ctrl+Shift+Tab selects after `selected` among `count`.
pub fn next_index(selected: usize, count: usize, forward: bool) -> usize {
    match (count, forward) {
        (0, _) => 0,
        (_, true) => (selected + 1) % count,
        (_, false) => (selected + count - 1) % count,
    }
}

/// The item selected after the selected one at `removed` goes, among `count` left: the
/// nearest earlier one, else the next.
pub fn after_removal(removed: usize, count: usize) -> Option<usize> {
    match count {
        0 => None,
        _ => Some(removed.saturating_sub(1).min(count - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect::new(16, 16, 448, 328);

    #[test]
    fn the_worked_example_lays_out_as_designed() {
        let laid = layout(FRAME, kind::TOP, &[56, 72, 46], TAB_STRIP_HEIGHT);
        assert_eq!(laid.strip, Some(Rect::new(117, 16, 246, 28)));
        assert_eq!(laid.segments()[0], Rect::new(117, 16, 80, 28));
        assert_eq!(laid.segments()[1], Rect::new(197, 16, 96, 28));
        assert_eq!(laid.segments()[2], Rect::new(293, 16, 70, 28));
        assert_eq!(laid.content_box, Rect::new(16, 30, 448, 314));
        assert_eq!(laid.content, Rect::new(24, 52, 432, 284));
        assert_eq!(label_place(laid.segments()[0], 56), (129, 22, 56));
        assert_eq!(label_place(laid.segments()[1], 72).0, 209);
        assert_eq!(label_place(laid.segments()[2], 46).0, 305);
    }

    #[test]
    fn separators_skip_the_selected_segment() {
        let laid = layout(FRAME, kind::TOP, &[56, 72, 46], TAB_STRIP_HEIGHT);
        let mut none = laid.separators(Some(0));
        assert_eq!(none.next(), Some(Rect::new(292, 22, 1, 16)));
        assert_eq!(none.next(), None);
        assert_eq!(laid.separators(Some(1)).count(), 0);
        let first = laid.separators(None).next();
        assert_eq!(first, Some(Rect::new(196, 22, 1, 16)));
        assert_eq!(laid.ring(1), Some(Rect::new(199, 18, 92, 24)));
    }

    #[test]
    fn wide_labels_are_cut_evenly_to_fit() {
        let laid = layout(FRAME, kind::TOP, &[100; 8], TAB_STRIP_HEIGHT);
        assert!(laid.segments().iter().all(|segment| segment.width == 54));
        assert_eq!(label_place(laid.segments()[0], 100).0, laid.segments()[0].x + 12);
    }

    #[test]
    fn other_types_place_their_strips_and_content() {
        let bottom = layout(FRAME, kind::BOTTOM, &[56], TAB_STRIP_HEIGHT);
        assert_eq!(bottom.strip.map(|strip| strip.y), Some(16 + 328 - 28));
        assert_eq!(bottom.content, Rect::new(24, 24, 432, 284));
        let left = layout(FRAME, kind::LEFT, &[56, 72], TAB_STRIP_HEIGHT);
        assert_eq!(left.strip, Some(Rect::new(16, 16 + (328 - 56) / 2, 96, 56)));
        assert_eq!(left.content_box, Rect::new(64, 16, 400, 328));
        assert_eq!(left.content.x, 72);
        let right = layout(FRAME, kind::RIGHT, &[56], TAB_STRIP_HEIGHT);
        assert_eq!(right.strip.map(|strip| strip.x), Some(16 + 448 - 80));
        let none = layout(FRAME, kind::NO_TABS_NO_BORDER, &[56], TAB_STRIP_HEIGHT);
        assert_eq!((none.strip, none.content), (None, FRAME));
        let empty = layout(FRAME, kind::TOP, &[], TAB_STRIP_HEIGHT);
        assert_eq!(empty.strip, None);
    }

    #[test]
    fn selection_wraps_and_removal_picks_the_neighbour() {
        assert_eq!(next_index(2, 3, true), 0);
        assert_eq!(next_index(0, 3, false), 2);
        assert_eq!(after_removal(0, 2), Some(0));
        assert_eq!(after_removal(2, 2), Some(1));
        assert_eq!(after_removal(0, 0), None);
    }
}
