//! NSCursor on the window server's pointer shapes (`docs/ux/macos-gui-v0.3.md` section 13):
//! AppKit's class cursors mapped to the closest of the server's ten shapes, the views' cursor
//! rects, and the order that decides which shape a window shows: hidden, then a cursor rect,
//! then the current cursor, then the arrow.

use crate::geometry::{Frame, Point};

/// The server's `Arrow`.
pub const ARROW: u32 = 0;
/// The server's `IBeam`.
pub const I_BEAM: u32 = 1;
/// The server's `ResizeNorthSouth`.
pub const RESIZE_NORTH_SOUTH: u32 = 2;
/// The server's `ResizeEastWest`.
pub const RESIZE_EAST_WEST: u32 = 3;
/// The server's `Hidden`.
pub const HIDDEN: u32 = 6;
/// The server's `Hand`.
pub const HAND: u32 = 7;
/// How many cursors `push` keeps.
pub const STACK_DEPTH: usize = 8;
/// How many cursor rects one view keeps; more are ignored.
pub const RECTS_PER_VIEW: usize = 8;

/// AppKit's class cursors by their method names, with the server shape each one shows.
pub const CLASS_CURSORS: [(&str, u32); 20] = [
    ("arrowCursor", ARROW),
    ("IBeamCursor", I_BEAM),
    ("IBeamCursorForVerticalLayout", I_BEAM),
    ("pointingHandCursor", HAND),
    ("resizeLeftRightCursor", RESIZE_EAST_WEST),
    ("resizeLeftCursor", RESIZE_EAST_WEST),
    ("resizeRightCursor", RESIZE_EAST_WEST),
    ("columnResizeCursor", RESIZE_EAST_WEST),
    ("resizeUpDownCursor", RESIZE_NORTH_SOUTH),
    ("resizeUpCursor", RESIZE_NORTH_SOUTH),
    ("resizeDownCursor", RESIZE_NORTH_SOUTH),
    ("rowResizeCursor", RESIZE_NORTH_SOUTH),
    ("crosshairCursor", ARROW),
    ("openHandCursor", ARROW),
    ("closedHandCursor", ARROW),
    ("operationNotAllowedCursor", ARROW),
    ("dragCopyCursor", ARROW),
    ("dragLinkCursor", ARROW),
    ("disappearingItemCursor", ARROW),
    ("contextualMenuCursor", ARROW),
];

/// The server shape of the class cursor `name`, if AppKit has one by that name.
pub fn shape_of(name: &str) -> Option<u32> {
    CLASS_CURSORS
        .iter()
        .find(|(each, _)| *each == name)
        .map(|&(_, shape)| shape)
}

/// The app's cursor state: the hide count, the current cursor and the `push` stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursors {
    hidden: u32,
    hidden_until_move: bool,
    current: u32,
    stack: [u32; STACK_DEPTH],
    depth: usize,
    in_zone: bool,
    last: u32,
}

impl Default for Cursors {
    fn default() -> Self {
        Self::new()
    }
}

impl Cursors {
    pub const fn new() -> Self {
        Self {
            hidden: 0,
            hidden_until_move: false,
            current: ARROW,
            stack: [ARROW; STACK_DEPTH],
            depth: 0,
            in_zone: false,
            last: ARROW,
        }
    }

    /// `+[NSCursor hide]`.
    pub fn hide(&mut self) {
        self.hidden = self.hidden.saturating_add(1);
    }

    /// `+[NSCursor unhide]`: never below 0.
    pub fn unhide(&mut self) {
        self.hidden = self.hidden.saturating_sub(1);
    }

    /// `+[NSCursor setHiddenUntilMouseMoves:]`.
    pub fn set_hidden_until_move(&mut self, hidden: bool) {
        self.hidden_until_move = hidden;
    }

    /// `-[NSCursor set]`: the current cursor, the stack untouched.
    pub fn set(&mut self, shape: u32) {
        self.current = shape;
    }

    /// `-[NSCursor push]`: the current cursor onto the stack (dropped when it is full), then
    /// `shape` as the current.
    pub fn push(&mut self, shape: u32) {
        if self.depth < STACK_DEPTH {
            self.stack[self.depth] = self.current;
            self.depth += 1;
        }
        self.current = shape;
    }

    /// `pop`: the stack's top as the current, nothing on an empty stack.
    pub fn pop(&mut self) {
        if let Some(top) = self.depth.checked_sub(1) {
            self.depth = top;
            self.current = self.stack[top];
        }
    }

    /// The shape the last `resolve` gave (`currentCursor`).
    pub fn last(&self) -> u32 {
        self.last
    }

    /// The shape a window shows with the pointer in a cursor rect of `zone`'s shape (or none),
    /// `moved` when the pointer moved since the last time.
    pub fn resolve(&mut self, zone: Option<u32>, moved: bool) -> u32 {
        if moved {
            self.hidden_until_move = false;
        }
        let shape = match zone {
            Some(shape) => {
                self.in_zone = true;
                shape
            }
            None => {
                if self.in_zone {
                    self.in_zone = false;
                    self.current = ARROW;
                }
                self.current
            }
        };
        self.last = match self.hidden > 0 || self.hidden_until_move {
            true => HIDDEN,
            false => shape,
        };
        self.last
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CursorRect {
    view: usize,
    rect: Frame,
    shape: u32,
    cursor: usize,
}

/// The views' cursor rects, in each view's coordinates, at most `N` in all.
#[derive(Debug, Clone, Copy)]
pub struct CursorRects<const N: usize> {
    rects: [CursorRect; N],
    count: usize,
}

impl<const N: usize> Default for CursorRects<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> CursorRects<N> {
    pub const fn new() -> Self {
        Self {
            rects: [CursorRect {
                view: 0,
                rect: Frame::new(0.0, 0.0, 0.0, 0.0),
                shape: ARROW,
                cursor: 0,
            }; N],
            count: 0,
        }
    }

    fn of(&self, view: usize) -> impl Iterator<Item = &CursorRect> {
        self.rects[..self.count]
            .iter()
            .filter(move |each| each.view == view)
    }

    /// `addCursorRect:cursor:`: `rect` of `view` shows the cursor `cursor` of `shape`; false
    /// when the view has [`RECTS_PER_VIEW`] already or the table is full.
    pub fn add(&mut self, view: usize, rect: Frame, cursor: usize, shape: u32) -> bool {
        if self.count == N || self.of(view).count() >= RECTS_PER_VIEW {
            return false;
        }
        self.rects[self.count] = CursorRect {
            view,
            rect,
            shape,
            cursor,
        };
        self.count += 1;
        true
    }

    fn retain(&mut self, keep: impl Fn(&CursorRect) -> bool) {
        let mut kept = 0;
        for index in 0..self.count {
            if keep(&self.rects[index]) {
                self.rects[kept] = self.rects[index];
                kept += 1;
            }
        }
        self.count = kept;
    }

    /// `removeCursorRect:cursor:`.
    pub fn remove(&mut self, view: usize, rect: Frame, cursor: usize) {
        self.retain(|each| !(each.view == view && each.rect == rect && each.cursor == cursor));
    }

    /// `discardCursorRects`, and what a freed view leaves.
    pub fn discard(&mut self, view: usize) {
        self.retain(|each| each.view != view);
    }

    /// The shape of `view`'s first rect, in the order added, holding `point` (in the view's
    /// coordinates) inside the view's `bounds`.
    pub fn find(&self, view: usize, bounds: Frame, point: Point) -> Option<u32> {
        if !bounds.contains(point) {
            return None;
        }
        self.of(view)
            .find(|each| each.rect.contains(point))
            .map(|each| each.shape)
    }

    /// Whether any view has a rect.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_cursors_map_to_the_closest_server_shape() {
        assert_eq!(shape_of("arrowCursor"), Some(ARROW));
        assert_eq!(shape_of("IBeamCursor"), Some(I_BEAM));
        assert_eq!(shape_of("IBeamCursorForVerticalLayout"), Some(I_BEAM));
        assert_eq!(shape_of("pointingHandCursor"), Some(HAND));
        assert_eq!(shape_of("resizeLeftRightCursor"), Some(RESIZE_EAST_WEST));
        assert_eq!(shape_of("columnResizeCursor"), Some(RESIZE_EAST_WEST));
        assert_eq!(shape_of("resizeUpDownCursor"), Some(RESIZE_NORTH_SOUTH));
        assert_eq!(shape_of("rowResizeCursor"), Some(RESIZE_NORTH_SOUTH));
        for arrow in [
            "crosshairCursor",
            "openHandCursor",
            "closedHandCursor",
            "operationNotAllowedCursor",
            "dragCopyCursor",
            "dragLinkCursor",
            "disappearingItemCursor",
            "contextualMenuCursor",
        ] {
            assert_eq!(shape_of(arrow), Some(ARROW), "{arrow}");
        }
        assert_eq!(shape_of("spinningCursor"), None);
    }

    #[test]
    fn hiding_counts_and_never_goes_below_zero() {
        let mut cursors = Cursors::new();
        cursors.hide();
        cursors.hide();
        cursors.unhide();
        assert_eq!(cursors.resolve(None, false), HIDDEN);
        cursors.unhide();
        assert_eq!(cursors.resolve(None, false), ARROW);
        cursors.unhide();
        cursors.hide();
        assert_eq!(cursors.resolve(None, false), HIDDEN);
    }

    #[test]
    fn hidden_until_the_mouse_moves_ends_with_a_move() {
        let mut cursors = Cursors::new();
        cursors.set_hidden_until_move(true);
        assert_eq!(cursors.resolve(None, false), HIDDEN);
        assert_eq!(cursors.resolve(None, true), ARROW);
    }

    #[test]
    fn a_rect_wins_over_the_current_cursor_and_leaving_it_sets_the_arrow() {
        let mut cursors = Cursors::new();
        cursors.set(I_BEAM);
        assert_eq!(cursors.resolve(None, true), I_BEAM);
        assert_eq!(cursors.resolve(Some(HAND), true), HAND);
        assert_eq!(cursors.last(), HAND);
        assert_eq!(cursors.resolve(None, true), ARROW);
    }

    #[test]
    fn push_and_pop_keep_a_stack() {
        let mut cursors = Cursors::new();
        cursors.push(I_BEAM);
        cursors.push(HAND);
        assert_eq!(cursors.resolve(None, false), HAND);
        cursors.pop();
        assert_eq!(cursors.resolve(None, false), I_BEAM);
        cursors.pop();
        assert_eq!(cursors.resolve(None, false), ARROW);
        cursors.pop();
        assert_eq!(cursors.resolve(None, false), ARROW);
        cursors.set(HAND);
        cursors.pop();
        assert_eq!(cursors.resolve(None, false), HAND);
    }

    #[test]
    fn a_view_keeps_eight_rects_and_the_first_added_wins() {
        let mut rects = CursorRects::<16>::new();
        let bounds = Frame::new(0.0, 0.0, 96.0, 96.0);
        for index in 0..RECTS_PER_VIEW {
            let shape = if index == 0 { HAND } else { I_BEAM };
            assert!(rects.add(1, bounds, index, shape));
        }
        assert!(!rects.add(1, bounds, 9, ARROW));
        assert!(rects.add(2, bounds, 9, ARROW));
        assert_eq!(rects.find(1, bounds, Point::new(48.0, 48.0)), Some(HAND));
        assert_eq!(rects.find(1, bounds, Point::new(96.0, 48.0)), None);
        rects.remove(1, bounds, 0);
        assert_eq!(rects.find(1, bounds, Point::new(48.0, 48.0)), Some(I_BEAM));
        rects.discard(1);
        assert_eq!(rects.find(1, bounds, Point::new(48.0, 48.0)), None);
        assert_eq!(rects.find(2, bounds, Point::new(48.0, 48.0)), Some(ARROW));
        rects.discard(2);
        assert!(rects.is_empty());
    }

    #[test]
    fn a_rect_is_clipped_to_its_view() {
        let mut rects = CursorRects::<4>::new();
        let bounds = Frame::new(0.0, 0.0, 10.0, 10.0);
        rects.add(1, Frame::new(0.0, 0.0, 100.0, 100.0), 1, HAND);
        assert_eq!(rects.find(1, bounds, Point::new(5.0, 5.0)), Some(HAND));
        assert_eq!(rects.find(1, bounds, Point::new(50.0, 5.0)), None);
    }

    #[test]
    fn the_design_example_sends_one_shape_per_zone() {
        let mut cursors = Cursors::new();
        let mut sent = Vec::new();
        let mut last = ARROW;
        for zone in [Some(HAND), Some(HAND), Some(I_BEAM), None, None] {
            let shape = cursors.resolve(zone, true);
            if shape != last {
                sent.push(shape);
                last = shape;
            }
        }
        assert_eq!(sent, [HAND, I_BEAM, ARROW]);
    }
}
