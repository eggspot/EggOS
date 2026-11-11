//! One window's state and geometry (docs/ux/desktop-v0.3-gui.md, sections 3.3 to 3.5): its
//! outside rectangle, title bar, controls on either side, content, title, the parts a point can
//! hit, its resize zones and the geometry resizing, maximising and snapping give it.

use super::protocol::{PointerShape, Role, TITLE_CAPACITY, flags};
use super::{BAR_HEIGHT, TITLE_HEIGHT, contains};
use crate::display::Rect;
use crate::font::GLYPH_WIDTH;

/// A window's id as the contract numbers it: its slot in the low 6 bits, a generation above.
pub type WindowId = u16;

/// The client of the kernel's own windows (the terminal).
pub const KERNEL_CLIENT: u16 = u16::MAX;

/// The smallest window, outside size (section 3.3).
pub const MINIMUM_OUTSIDE: (u32, u32) = (160, 92);
/// How far inside the work area a window's default size stays, each direction.
pub const DEFAULT_MARGIN: u32 = 32;
/// How far a cascaded window sits from the one below it.
pub const CASCADE_STEP: u32 = 32;
/// Columns the border and rows the border and title bar add to the content.
pub const CHROME: (u32, u32) = (2, TITLE_HEIGHT + 2);
/// The longest time between the two presses of a double-click.
pub const DOUBLE_CLICK_MS: u32 = 400;
/// The farthest the second press of a double-click may be from the first, each direction.
pub const DOUBLE_CLICK_DISTANCE: u32 = 4;
/// How far a maximised window's title bar is dragged before the window is restored.
pub const RESTORE_DRAG_DISTANCE: u32 = 8;

pub(super) const SLOT_BITS: u32 = 6;
pub(super) const CONTROL_SIZE: u32 = 12;
pub(super) const CONTROL_TOP: u32 = 8;
pub(super) const CONTROL_HIT_WIDTH: u32 = 20;
const CONTROL_CIRCLE_FROM_HIT: u32 = 4;
const CONTROLS_FROM_EDGE: u32 = 8;
const TITLE_SIDE: u32 = 68;
const ELLIPSIS: &str = "...";
const RESIZE_OUTSIDE: u32 = 2;
const RESIZE_INSIDE: u32 = 4;
const RESIZE_INSIDE_TITLE: u32 = 2;
const RESIZE_CORNER: u32 = 16;

/// The edges a resize moves, as bits.
pub mod edge {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const TOP: u8 = 4;
    pub const BOTTOM: u8 = 8;
}

/// The part of a window a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Close,
    Minimise,
    Maximise,
    TitleBar,
    Content,
    Border,
}

/// Which side of the title bar the controls are on (section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlSide {
    /// Close, minimise, maximise from the left edge (macOS habits).
    Left,
    /// Close, maximise, minimise from the right edge (Windows and Linux habits).
    Right,
}

/// A rectangle on the desktop's plane whose left edge may lie left of the screen: a window
/// hanging off the left edge (D222). [`Self::visible`] is the part on the screen's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    /// The rectangle `width` by `height` with its top-left corner at (`x`, `y`).
    pub const fn new(x: i32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Its right edge: one column past its last.
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// The part of it right of the screen's left edge, empty when none is.
    pub fn visible(&self) -> Rect {
        let left = i64::from(self.x).max(0);
        let width = (self.right() - left).max(0) as u32;
        Rect::new(left as u32, self.y, width, self.height)
    }
}

impl From<Rect> for Bounds {
    fn from(rect: Rect) -> Self {
        Self::new(rect.x as i32, rect.y, rect.width, rect.height)
    }
}

/// A window's geometry before it was maximised: its outside top-left and its content size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Geometry {
    pub x: i32,
    pub y: u32,
    pub content_width: u32,
    pub content_height: u32,
}

/// A window: who owns it, what it is, where it is and what it shows.
#[derive(Debug, Clone, Copy)]
pub(super) struct Window {
    pub id: WindowId,
    pub client: u16,
    pub role: Role,
    pub parent: Option<u8>,
    pub flags: u32,
    pub x: i32,
    pub y: u32,
    pub content_width: u32,
    pub content_height: u32,
    pub minimum: (u32, u32),
    pub requested_position: Option<(i32, i32)>,
    pub title: [u8; TITLE_CAPACITY],
    pub title_length: u8,
    pub shown: bool,
    pub minimised: bool,
    pub maximised: bool,
    pub square_corners: bool,
    pub before_maximised: Option<Geometry>,
    pub ready: bool,
    pub closable: bool,
    pub fill: u32,
    pub buffer: Rect,
    pub pointer_shape: PointerShape,
    pub pending_resize: Option<(u32, u32)>,
    pub close_requested_ms: Option<u32>,
    pub not_responding: bool,
    pub focus_serial: u32,
    /// How opaque its content is, 255 fully (a layered window's constant alpha, D358).
    pub opacity: u8,
    /// The colour its content shows through where it has exactly this colour (D358).
    pub colour_key: Option<u32>,
    pub hidden_by_show_desktop: bool,
    /// The virtual desktop a normal window is on, from 0 (D204).
    pub desktop: u8,
    /// How many times [`flags::PERSON_VIEW_ONLY`] has turned on, so a capture that copied
    /// while it was off can tell it turned on meanwhile (D193).
    pub view_only_turns: u32,
}

impl Window {
    pub fn new(id: WindowId, client: u16, role: Role, content: (u32, u32), fill: u32) -> Self {
        Self {
            id,
            client,
            role,
            parent: None,
            flags: 0,
            x: 0,
            y: BAR_HEIGHT,
            content_width: content.0,
            content_height: content.1,
            minimum: (0, 0),
            requested_position: None,
            title: [0; TITLE_CAPACITY],
            title_length: 0,
            shown: false,
            minimised: false,
            maximised: false,
            square_corners: false,
            before_maximised: None,
            ready: false,
            closable: true,
            fill,
            buffer: Rect::new(0, 0, content.0, content.1),
            pointer_shape: PointerShape::Arrow,
            pending_resize: None,
            close_requested_ms: None,
            not_responding: false,
            focus_serial: 0,
            opacity: u8::MAX,
            colour_key: None,
            hidden_by_show_desktop: false,
            desktop: 0,
            view_only_turns: 0,
        }
    }

    pub fn bounds(&self) -> Bounds {
        Bounds::new(
            self.x,
            self.y,
            self.content_width + CHROME.0,
            self.content_height + CHROME.1,
        )
    }

    pub fn outside(&self) -> Rect {
        self.bounds().visible()
    }

    pub fn has_shadow(&self) -> bool {
        !self.maximised
    }

    pub fn with_shadow(&self) -> Rect {
        let window = self.bounds();
        if !self.has_shadow() {
            return window.visible();
        }
        Bounds::new(
            window.x.saturating_sub(super::SHADOW_RINGS as i32),
            window.y,
            window.width + 2 * super::SHADOW_RINGS,
            window.height + super::SHADOW_RINGS,
        )
        .visible()
    }

    pub fn title_bar(&self) -> Rect {
        Bounds::new(
            self.x,
            self.y,
            self.content_width + CHROME.0,
            TITLE_HEIGHT + 1,
        )
        .visible()
    }

    pub fn content_bounds(&self) -> Bounds {
        Bounds::new(
            self.x + 1,
            self.y + 1 + TITLE_HEIGHT,
            self.content_width,
            self.content_height,
        )
    }

    pub fn content(&self) -> Rect {
        self.content_bounds().visible()
    }

    pub fn content_origin(&self) -> (i64, i64) {
        let content = self.content_bounds();
        (i64::from(content.x), i64::from(content.y))
    }

    pub fn content_to_screen(&self, rect: Rect) -> Rect {
        let clipped = rect.clipped(self.content_width, self.content_height);
        let (x, y) = self.content_origin();
        Bounds::new(
            (x + i64::from(clipped.x)) as i32,
            (y + i64::from(clipped.y)) as u32,
            clipped.width,
            clipped.height,
        )
        .visible()
    }

    pub fn overhang(&self) -> u32 {
        match self.x < 0 {
            true => self.x.unsigned_abs() + super::SHADOW_RINGS,
            false => 0,
        }
    }

    pub fn moved_right(&self, by: u32) -> Self {
        let mut moved = *self;
        moved.x = self.x.saturating_add(by as i32);
        moved
    }

    pub fn geometry(&self) -> Geometry {
        Geometry {
            x: self.x,
            y: self.y,
            content_width: self.content_width,
            content_height: self.content_height,
        }
    }

    pub fn is_fixed_size(&self) -> bool {
        self.flags & flags::FIXED_SIZE != 0
    }

    pub fn takes_ctrl_keys(&self) -> bool {
        self.flags & flags::TAKES_CTRL_KEYS != 0
    }

    pub fn takes_person_input_only(&self) -> bool {
        self.flags & flags::PERSON_INPUT_ONLY != 0
    }

    pub fn is_person_view_only(&self) -> bool {
        self.flags & flags::PERSON_VIEW_ONLY != 0
    }

    pub fn is_resizable(&self) -> bool {
        self.role == Role::Normal && !self.maximised && !self.is_fixed_size()
    }

    pub fn can_maximise(&self) -> bool {
        self.role == Role::Normal && !self.is_fixed_size()
    }

    pub fn has_square_corners(&self) -> bool {
        self.maximised || self.square_corners
    }

    fn controls(&self) -> &'static [Part] {
        match self.role {
            Role::Normal => &[Part::Close, Part::Minimise, Part::Maximise],
            _ => &[Part::Close],
        }
    }

    fn control_index(part: Part, side: ControlSide) -> Option<u32> {
        Some(match (part, side) {
            (Part::Close, _) => 0,
            (Part::Minimise, ControlSide::Left) | (Part::Maximise, ControlSide::Right) => 1,
            (Part::Maximise, ControlSide::Left) | (Part::Minimise, ControlSide::Right) => 2,
            _ => return None,
        })
    }

    fn control_left(&self, part: Part, side: ControlSide) -> Option<i64> {
        if !self.controls().contains(&part) {
            return None;
        }
        let index = Self::control_index(part, side)?;
        let offset = i64::from(CONTROLS_FROM_EDGE + index * CONTROL_HIT_WIDTH);
        Some(match side {
            ControlSide::Left => i64::from(self.x) + offset,
            ControlSide::Right => self.bounds().right() - offset - i64::from(CONTROL_HIT_WIDTH),
        })
    }

    /// The hit target of control `part` with the controls on `side`, if the window has it: the
    /// part of it on the screen.
    pub fn control(&self, part: Part, side: ControlSide) -> Option<Rect> {
        let left = self.control_left(part, side)?;
        Some(Bounds::new(left as i32, self.y, CONTROL_HIT_WIDTH, TITLE_HEIGHT).visible())
    }

    /// The circle of control `part`, if the window has it: the part of it on the screen.
    pub fn control_circle(&self, part: Part, side: ControlSide) -> Option<Rect> {
        let left = self.control_left(part, side)?;
        Some(
            Bounds::new(
                (left + i64::from(CONTROL_CIRCLE_FROM_HIT)) as i32,
                self.y + CONTROL_TOP,
                CONTROL_SIZE,
                CONTROL_SIZE,
            )
            .visible(),
        )
    }

    /// The controls the window draws, in drawing order.
    pub fn drawn_controls(&self) -> &'static [Part] {
        self.controls()
    }

    pub fn part_at(&self, x: u32, y: u32, side: ControlSide) -> Option<Part> {
        if !contains(self.outside(), x, y) {
            return None;
        }
        if contains(self.content(), x, y) {
            return Some(Part::Content);
        }
        if !contains(self.title_bar(), x, y) {
            return Some(Part::Border);
        }
        let control = self.controls().iter().find(|&&part| {
            self.control(part, side)
                .is_some_and(|rect| contains(rect, x, y))
        });
        Some(control.copied().unwrap_or(Part::TitleBar))
    }

    /// The edges a press at (`x`, `y`) would resize the window by ([`edge`] bits), 0 when the
    /// point is in none of its resize zones or the window is not resizable.
    pub fn resize_edges(&self, x: u32, y: u32) -> u8 {
        if !self.is_resizable() {
            return 0;
        }
        resize_edges(self.bounds(), x, y)
    }

    pub fn title(&self) -> &str {
        core::str::from_utf8(&self.title[..usize::from(self.title_length)]).unwrap_or("")
    }

    pub fn set_title(&mut self, title: &str) {
        let length = title.len().min(TITLE_CAPACITY);
        self.title[..length].copy_from_slice(&title.as_bytes()[..length]);
        self.title_length = length as u8;
    }
}

/// The edges ([`edge`] bits) a point at (`x`, `y`) resizes a window whose outside rectangle is
/// `outside` by: a ring 2 px outside the border and 4 px inside it (2 px inside along the top,
/// so the title bar keeps its drag), with 16 by 16 corners that resize both edges; 0 outside
/// the ring.
pub fn resize_edges(outside: impl Into<Bounds>, x: u32, y: u32) -> u8 {
    let outside = outside.into();
    let (x, y) = (i64::from(x), i64::from(y));
    let left = i64::from(outside.x) - i64::from(RESIZE_OUTSIDE);
    let top = i64::from(outside.y) - i64::from(RESIZE_OUTSIDE);
    let right = outside.right() + i64::from(RESIZE_OUTSIDE);
    let bottom = i64::from(outside.y) + i64::from(outside.height) + i64::from(RESIZE_OUTSIDE);
    if x < left || x >= right || y < top || y >= bottom {
        return 0;
    }
    let ring = RESIZE_OUTSIDE + RESIZE_INSIDE;
    let mut edges = 0;
    if x < left + i64::from(ring) {
        edges |= edge::LEFT;
    } else if x >= right - i64::from(ring) {
        edges |= edge::RIGHT;
    }
    if y < top + i64::from(RESIZE_OUTSIDE + RESIZE_INSIDE_TITLE) {
        edges |= edge::TOP;
    } else if y >= bottom - i64::from(ring) {
        edges |= edge::BOTTOM;
    }
    if edges == 0 {
        return 0;
    }
    let corner = i64::from(RESIZE_CORNER);
    let horizontal = match () {
        () if x < left + corner => edge::LEFT,
        () if x >= right - corner => edge::RIGHT,
        () => 0,
    };
    let vertical = match () {
        () if y < top + corner => edge::TOP,
        () if y >= bottom - corner => edge::BOTTOM,
        () => 0,
    };
    match horizontal != 0 && vertical != 0 {
        true => horizontal | vertical,
        false => edges,
    }
}

/// The pointer shape over a resize zone of `edges`; the arrow for none.
pub fn resize_pointer(edges: u8) -> PointerShape {
    const NORTH_WEST: u8 = edge::TOP | edge::LEFT;
    const SOUTH_EAST: u8 = edge::BOTTOM | edge::RIGHT;
    const NORTH_EAST: u8 = edge::TOP | edge::RIGHT;
    const SOUTH_WEST: u8 = edge::BOTTOM | edge::LEFT;
    match edges {
        NORTH_WEST | SOUTH_EAST => PointerShape::ResizeNorthWestSouthEast,
        NORTH_EAST | SOUTH_WEST => PointerShape::ResizeNorthEastSouthWest,
        edge::TOP | edge::BOTTOM => PointerShape::ResizeNorthSouth,
        edge::LEFT | edge::RIGHT => PointerShape::ResizeEastWest,
        _ => PointerShape::Arrow,
    }
}

/// The outside rectangle a window that was `start` gets when its `edges` follow the pointer by
/// (`dx`, `dy`): the opposite edges stay put, the size is at least `minimum` (outside) and the
/// moving edges stay inside `work`. The minimum wins when the two disagree. A window hanging
/// off the left edge keeps its left edge there while its right or bottom edge moves (D222).
pub fn resized(
    start: impl Into<Bounds>,
    edges: u8,
    dx: i64,
    dy: i64,
    minimum: (u32, u32),
    work: Rect,
) -> Bounds {
    let start = start.into();
    let axis =
        |from: i64, size: u32, low: u8, high: u8, delta: i64, least: u32, area: (u32, u32)| {
            let (end, least) = (from + i64::from(size), i64::from(least));
            let (area_start, area_end) = (i64::from(area.0), i64::from(area.1));
            let (from, end) = if edges & high != 0 {
                let end = (end + delta)
                    .min(area_end.max(from + least))
                    .max(from + least);
                (from, end)
            } else if edges & low != 0 {
                let from = (from + delta)
                    .max(area_start.min(end - least))
                    .min(end - least);
                (from, end)
            } else {
                (from, end)
            };
            (from, (end - from).max(0) as u32)
        };
    let (x, width) = axis(
        i64::from(start.x),
        start.width,
        edge::LEFT,
        edge::RIGHT,
        dx,
        minimum.0,
        (work.x, work.right()),
    );
    let (y, height) = axis(
        i64::from(start.y),
        start.height,
        edge::TOP,
        edge::BOTTOM,
        dy,
        minimum.1,
        (work.y, work.bottom()),
    );
    Bounds::new(x as i32, y.max(0) as u32, width, height)
}

/// How much of a window's title bar stays inside the work area when it hangs off the left
/// edge, and when it hangs off the right, at `scale` (D222): [`super::HANGING_TITLE_KEPT`] of
/// its draggable part, and the controls too when they are on the side that stays visible.
pub fn kept_when_hanging(side: ControlSide, scale: u32) -> (u32, u32) {
    let grab = super::HANGING_TITLE_KEPT;
    let with_controls = grab + TITLE_SIDE;
    let (left, right) = match side {
        ControlSide::Right => (with_controls, grab),
        ControlSide::Left => (grab, with_controls),
    };
    (left * scale, right * scale)
}

/// The least and the greatest left edge a window `width` wide may have in `work` with the
/// controls on `side` (D222): hanging off either edge as far as [`kept_when_hanging`] lets it,
/// or wholly inside when it is narrower than the grab and the controls together.
pub fn hanging_x_range(width: u32, work: Rect, side: ControlSide) -> (i64, i64) {
    let start = i64::from(work.x);
    if width < super::HANGING_TITLE_KEPT + TITLE_SIDE {
        return (start, start + i64::from(work.width.saturating_sub(width)));
    }
    let (kept_left, kept_right) = kept_when_hanging(side, 1);
    let least = start - i64::from(width - kept_left);
    let most = (i64::from(work.right()) - i64::from(kept_right)).max(least);
    (least, most)
}

/// The smallest outside size of a window whose client asked for content of at least
/// `client_minimum`, in a work area of `work`: the chrome's minimum or the client's, whichever
/// is larger, giving way to the work area when that is smaller.
pub fn minimum_outside(client_minimum: (u32, u32), work: Rect) -> (u32, u32) {
    (
        (client_minimum.0 + CHROME.0)
            .max(MINIMUM_OUTSIDE.0)
            .min(work.width.max(CHROME.0 + 1)),
        (client_minimum.1 + CHROME.1)
            .max(MINIMUM_OUTSIDE.1)
            .min(work.height.max(CHROME.1 + 1)),
    )
}

/// The geometry a maximised window has in `work`: the whole of it.
pub fn maximised(work: Rect) -> Geometry {
    Geometry {
        x: work.x as i32,
        y: work.y,
        content_width: work.width.saturating_sub(CHROME.0).max(1),
        content_height: work.height.saturating_sub(CHROME.1).max(1),
    }
}

/// The geometry a window snapped to the left (`right` false) or right half of `work` has,
/// at least `minimum` (outside) wide.
pub fn snapped(work: Rect, right: bool, minimum: (u32, u32)) -> Geometry {
    let half = (work.width / 2).max(minimum.0).min(work.width);
    let x = match right {
        true => work.right() - half,
        false => work.x,
    };
    Geometry {
        x: x as i32,
        y: work.y,
        content_width: half.saturating_sub(CHROME.0).max(1),
        content_height: work.height.saturating_sub(CHROME.1).max(1),
    }
}

/// Whether a press at `second` (x, y, time in ms) makes a double-click with one at `first`.
pub fn is_double_click(first: (u32, u32, u32), second: (u32, u32, u32)) -> bool {
    second.2.wrapping_sub(first.2) <= DOUBLE_CLICK_MS
        && first.0.abs_diff(second.0) <= DOUBLE_CLICK_DISTANCE
        && first.1.abs_diff(second.1) <= DOUBLE_CLICK_DISTANCE
}

/// The glyphs of `title` that fit `room` cells: all of them, or the first ones and `...`.
pub fn cut_title(title: &str, room: usize) -> (&str, bool) {
    if title.chars().count() <= room {
        return (title, false);
    }
    let kept = room.saturating_sub(ELLIPSIS.len());
    let end = title
        .char_indices()
        .nth(kept)
        .map_or(title.len(), |(at, _)| at);
    (&title[..end], true)
}

/// The glyph cells a title has in a title bar `width` pixels wide: centred on the window, clear
/// of the controls' side on both sides.
pub fn title_room(width: u32) -> usize {
    (width.saturating_sub(2 * TITLE_SIDE) / GLYPH_WIDTH as u32) as usize
}

/// The `...` drawn after a cut title.
pub fn ellipsis() -> &'static str {
    ELLIPSIS
}

/// The content size a window asking for `width` by `height` gets in a `work_width` by
/// `work_height` work area: at least the minimum window and at most the work area less
/// [`DEFAULT_MARGIN`] each way (the minimum giving way when the work area is smaller).
pub fn clamp_content_size(
    work_width: u32,
    work_height: u32,
    width: u32,
    height: u32,
) -> (u32, u32) {
    clamp_within(work_width, work_height, width, height, DEFAULT_MARGIN)
}

/// The content size `SET_SIZE` gives a window asking for `width` by `height` in a
/// `work_width` by `work_height` work area: at least the minimum window and at most the whole
/// work area.
pub fn clamp_resized_content(
    work_width: u32,
    work_height: u32,
    width: u32,
    height: u32,
) -> (u32, u32) {
    clamp_within(work_width, work_height, width, height, 0)
}

fn clamp_within(
    work_width: u32,
    work_height: u32,
    width: u32,
    height: u32,
    margin: u32,
) -> (u32, u32) {
    let clamp = |wanted: u32, chrome: u32, minimum: u32, room: u32| {
        let largest = room.saturating_sub(margin).max(chrome + 1);
        wanted
            .saturating_add(chrome)
            .max(minimum)
            .min(largest)
            .max(chrome + 1)
            - chrome
    };
    (
        clamp(width, CHROME.0, MINIMUM_OUTSIDE.0, work_width),
        clamp(height, CHROME.1, MINIMUM_OUTSIDE.1, work_height),
    )
}

/// The slot of window `id`.
pub fn slot_of(id: WindowId) -> usize {
    usize::from(id) & ((1 << SLOT_BITS) - 1)
}
