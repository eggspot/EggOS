//! NSScrollView's geometry (D372, docs/ux/macos-gui-v0.3.md section 10): the border, the
//! clip view, the 8-pixel scrollers and their thumbs, the offset's clamp, and how the wheel,
//! the track, a thumb drag and the paging keys move it. Offsets count from the document's
//! top (0 shows its top), in pixels; rectangles are content pixels, top-left origin.

use eggos_toolkit::geometry::Rect;

/// A scroller's thickness, which `+[NSScroller scrollerWidth]` answers.
pub const SCROLLER_WIDTH: i32 = 8;
/// The shortest thumb.
pub const MINIMUM_THUMB: i32 = 24;
/// What one wheel notch scrolls: the Mouse pane's three lines of 24.
pub const WHEEL_STEP: i32 = 72;
/// What a page keeps of the view it leaves.
pub const PAGE_OVERLAP: i32 = 24;

/// Which scrollers a scroll view asks for and whether they hide when there is nothing to
/// scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scrollers {
    pub vertical: bool,
    pub horizontal: bool,
    pub autohides: bool,
}

/// Where a scroll view's parts are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The clip view: the frame, inset by 1 inside a border.
    pub clip: Rect,
    /// The clip view less the scrollers shown: `contentSize`.
    pub content: (i32, i32),
    pub vertical: Option<Rect>,
    pub horizontal: Option<Rect>,
    /// The square left between two scrollers.
    pub corner: Option<Rect>,
}

/// The border's four 1-pixel lines inside `frame`'s edge: top, bottom, left, right.
pub fn border_lines(frame: Rect) -> [Rect; 4] {
    [
        Rect::new(frame.x, frame.y, frame.width, 1),
        Rect::new(frame.x, frame.bottom() - 1, frame.width, 1),
        Rect::new(frame.x, frame.y, 1, frame.height),
        Rect::new(frame.right() - 1, frame.y, 1, frame.height),
    ]
}

/// The parts of a scroll view at `frame`, with or without a border, showing a document
/// `document` (width, height) pixels.
pub fn layout(frame: Rect, bordered: bool, scrollers: Scrollers, document: (i32, i32)) -> Layout {
    let clip = match bordered {
        true => frame.inset(1),
        false => frame,
    };
    let needs =
        |asked: bool, document: i32, view: i32| asked && (!scrollers.autohides || document > view);
    let mut vertical = needs(scrollers.vertical, document.1, clip.height);
    let width = clip.width - if vertical { SCROLLER_WIDTH } else { 0 };
    let horizontal = needs(scrollers.horizontal, document.0, width);
    let height = clip.height - if horizontal { SCROLLER_WIDTH } else { 0 };
    if !vertical {
        vertical = needs(scrollers.vertical, document.1, height);
    }
    let content = (
        (clip.width - if vertical { SCROLLER_WIDTH } else { 0 }).max(0),
        height.max(0),
    );
    let vertical_rect = vertical.then(|| {
        Rect::new(
            clip.right() - SCROLLER_WIDTH,
            clip.y,
            SCROLLER_WIDTH,
            content.1,
        )
    });
    let horizontal_rect = horizontal.then(|| {
        Rect::new(
            clip.x,
            clip.bottom() - SCROLLER_WIDTH,
            content.0,
            SCROLLER_WIDTH,
        )
    });
    let corner = (vertical && horizontal).then(|| {
        Rect::new(
            clip.right() - SCROLLER_WIDTH,
            clip.bottom() - SCROLLER_WIDTH,
            SCROLLER_WIDTH,
            SCROLLER_WIDTH,
        )
    });
    Layout {
        clip,
        content,
        vertical: vertical_rect,
        horizontal: horizontal_rect,
        corner,
    }
}

/// The largest offset on an axis whose view is `view` long and whose document `document`.
pub fn most(view: i32, document: i32) -> i32 {
    (document - view).max(0)
}

/// `offset` kept inside `0 ..= most(view, document)`.
pub fn clamp(offset: i32, view: i32, document: i32) -> i32 {
    offset.clamp(0, most(view, document))
}

/// A thumb's start in its track and its length, for a track `track` long, a view `view`, a
/// document `document` and the offset `offset`; `None` when there is nothing to scroll.
pub fn thumb(track: i32, view: i32, document: i32, offset: i32) -> Option<(i32, i32)> {
    if document <= view || track <= 0 {
        return None;
    }
    let length = (i64::from(track) * i64::from(view) / i64::from(document)) as i32;
    let length = length.max(MINIMUM_THUMB).min(track);
    let room = i64::from(track - length);
    let start = room * i64::from(clamp(offset, view, document)) / i64::from(most(view, document));
    Some((start as i32, length))
}

/// The offset after `notches` wheel notches (positive scrolls towards the start, as a wheel
/// turned up does).
pub fn wheel(offset: i32, notches: i32, view: i32, document: i32) -> i32 {
    clamp(offset - notches * WHEEL_STEP, view, document)
}

/// The offset after a page towards the end (`forward`) or the start.
pub fn page(offset: i32, forward: bool, view: i32, document: i32) -> i32 {
    let step = (view - PAGE_OVERLAP).max(1);
    let moved = match forward {
        true => offset + step,
        false => offset - step,
    };
    clamp(moved, view, document)
}

/// The offset after a press at `at` along a track `track` long: a page towards the press when
/// it misses the thumb, the same offset on the thumb.
pub fn track_press(at: i32, track: i32, view: i32, document: i32, offset: i32) -> i32 {
    let Some((start, length)) = thumb(track, view, document, offset) else {
        return offset;
    };
    if at < start {
        page(offset, false, view, document)
    } else if at >= start + length {
        page(offset, true, view, document)
    } else {
        offset
    }
}

/// The offset after dragging the thumb `moved` pixels from where it was grabbed at offset
/// `grabbed`.
pub fn drag(grabbed: i32, moved: i32, track: i32, view: i32, document: i32) -> i32 {
    let Some((_, length)) = thumb(track, view, document, grabbed) else {
        return grabbed;
    };
    let room = i64::from(track - length);
    if room == 0 {
        return grabbed;
    }
    let delta = i64::from(moved) * i64::from(most(view, document)) / room;
    clamp(grabbed + delta as i32, view, document)
}

/// The bounds origin's y of a clip view showing a document `document` tall in a view `view`
/// tall scrolled `offset` from its top: from the top for a flipped document, from the bottom
/// for one that is not (AppKit's y grows upward).
pub fn bounds_y(flipped: bool, offset: i32, view: i32, document: i32) -> i32 {
    let offset = clamp(offset, view, document);
    match flipped {
        true => offset,
        false => most(view, document) - offset,
    }
}

/// The offset from the top a clip view's bounds origin `y` means; the inverse of
/// [`bounds_y`].
pub fn offset_of_bounds_y(flipped: bool, y: i32, view: i32, document: i32) -> i32 {
    match flipped {
        true => clamp(y, view, document),
        false => clamp(most(view, document) - y, view, document),
    }
}

/// The least offset that shows `start .. start + length` in a view `view` long now at
/// `offset` (`scrollRectToVisible:`).
pub fn reveal(offset: i32, start: i32, length: i32, view: i32, document: i32) -> i32 {
    let wanted = if start < offset {
        start
    } else if start + length > offset + view {
        (start + length - view).min(start)
    } else {
        offset
    };
    clamp(wanted, view, document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERTICAL: Scrollers = Scrollers {
        vertical: true,
        horizontal: false,
        autohides: true,
    };

    #[test]
    fn the_border_is_four_one_pixel_lines_inside_the_frame() {
        let lines = border_lines(Rect::new(192, 16, 272, 288));
        assert_eq!(lines[0], Rect::new(192, 16, 272, 1));
        assert_eq!(lines[1], Rect::new(192, 303, 272, 1));
        assert_eq!(lines[2], Rect::new(192, 16, 1, 288));
        assert_eq!(lines[3], Rect::new(463, 16, 1, 288));
    }

    #[test]
    fn the_worked_example_s_clip_view_and_scroller() {
        let laid = layout(Rect::new(192, 16, 272, 288), true, VERTICAL, (262, 508));
        assert_eq!(laid.clip, Rect::new(193, 17, 270, 286));
        assert_eq!(laid.content, (262, 286));
        assert_eq!(laid.vertical, Some(Rect::new(455, 17, 8, 286)));
        assert_eq!(laid.horizontal, None);
    }

    #[test]
    fn content_size_shrinks_by_each_scroller_shown() {
        let frame = Rect::new(0, 0, 200, 100);
        let none = layout(frame, false, Scrollers::default(), (500, 500));
        assert_eq!(none.content, (200, 100));
        let both = Scrollers {
            vertical: true,
            horizontal: true,
            autohides: true,
        };
        let one = layout(frame, false, both, (100, 500));
        assert_eq!(one.content, (192, 100));
        let two = layout(frame, false, both, (500, 500));
        assert_eq!(two.content, (192, 92));
        assert_eq!(two.corner, Some(Rect::new(192, 92, 8, 8)));
    }

    #[test]
    fn a_scroller_that_does_not_hide_shows_with_nothing_to_scroll() {
        let shown = Scrollers {
            autohides: false,
            ..VERTICAL
        };
        let laid = layout(Rect::new(0, 0, 100, 100), false, shown, (50, 50));
        assert!(laid.vertical.is_some());
        assert_eq!(thumb(100, 100, 50, 0), None);
    }

    #[test]
    fn the_thumb_s_length_and_place() {
        assert_eq!(thumb(258, 258, 480, 0), Some((0, 138)));
        assert_eq!(thumb(258, 258, 480, 216), Some((116, 138)));
        assert_eq!(thumb(100, 100, 10_000, 0), Some((0, 24)));
    }

    #[test]
    fn the_first_shown_origin_is_the_document_s_top() {
        assert_eq!(bounds_y(true, 0, 300, 1000), 0);
        assert_eq!(bounds_y(false, 0, 300, 1000), 700);
        assert_eq!(offset_of_bounds_y(false, 700, 300, 1000), 0);
    }

    #[test]
    fn the_offset_clamps_at_both_ends() {
        assert_eq!(clamp(-5, 300, 1000), 0);
        assert_eq!(clamp(900, 300, 1000), 700);
        assert_eq!(clamp(10, 300, 200), 0);
    }

    #[test]
    fn the_wheel_scrolls_by_72_and_stops_at_the_end() {
        assert_eq!(wheel(0, -3, 258, 480), 216);
        assert_eq!(wheel(216, -1, 258, 480), 222);
        assert_eq!(wheel(216, 4, 258, 480), 0);
    }

    #[test]
    fn a_press_on_the_track_pages_by_the_view_less_24() {
        assert_eq!(track_press(250, 258, 258, 1000, 0), 234);
        assert_eq!(track_press(0, 258, 258, 1000, 500), 266);
        assert_eq!(track_press(10, 258, 258, 1000, 0), 0);
    }

    #[test]
    fn dragging_the_thumb_moves_the_offset_in_proportion() {
        let length = thumb(258, 258, 480, 0).unwrap().1;
        let expected = 40 * (480 - 258) / (258 - length);
        assert_eq!(drag(0, 40, 258, 258, 480), expected);
    }

    #[test]
    fn revealing_a_rect_scrolls_the_least() {
        assert_eq!(reveal(0, 400, 20, 300, 1000), 120);
        assert_eq!(reveal(200, 100, 20, 300, 1000), 100);
        assert_eq!(reveal(200, 250, 20, 300, 1000), 200);
    }
}
