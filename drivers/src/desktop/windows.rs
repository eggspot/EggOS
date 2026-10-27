//! One window's state and geometry (docs/ux/desktop-v0.3-gui.md, sections 3.3 and 3.4): its
//! outside rectangle, title bar, controls, content, title and the parts a point can hit.

use super::protocol::{PointerShape, Role, TITLE_CAPACITY};
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

pub(super) const SLOT_BITS: u32 = 6;
pub(super) const CONTROL_SIZE: u32 = 12;
pub(super) const CONTROL_TOP: u32 = 8;
pub(super) const CLOSE_X: u32 = 12;
pub(super) const MINIMISE_X: u32 = 32;
pub(super) const CONTROL_HIT_WIDTH: u32 = 20;
pub(super) const CLOSE_HIT_X: u32 = 8;
pub(super) const MINIMISE_HIT_X: u32 = 28;
const TITLE_SIDE: u32 = 68;
const ELLIPSIS: &str = "...";

/// The part of a window a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Close,
    Minimise,
    TitleBar,
    Content,
    Border,
}

/// A window: who owns it, what it is, where it is and what it shows.
#[derive(Debug, Clone, Copy)]
pub(super) struct Window {
    pub id: WindowId,
    pub client: u16,
    pub role: Role,
    pub parent: Option<u8>,
    pub flags: u32,
    pub x: u32,
    pub y: u32,
    pub content_width: u32,
    pub content_height: u32,
    pub minimum: (u32, u32),
    pub requested_position: Option<(i32, i32)>,
    pub title: [u8; TITLE_CAPACITY],
    pub title_length: u8,
    pub shown: bool,
    pub minimised: bool,
    pub ready: bool,
    pub closable: bool,
    pub fill: u32,
    pub buffer: Rect,
    pub pointer_shape: PointerShape,
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
            ready: false,
            closable: true,
            fill,
            buffer: Rect::new(0, 0, content.0, content.1),
            pointer_shape: PointerShape::Arrow,
        }
    }

    pub fn outside(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.content_width + CHROME.0,
            self.content_height + CHROME.1,
        )
    }

    pub fn with_shadow(&self) -> Rect {
        let window = self.outside();
        Rect::new(
            window.x.saturating_sub(super::SHADOW_RINGS),
            window.y,
            window.width + 2 * super::SHADOW_RINGS,
            window.height + super::SHADOW_RINGS,
        )
    }

    pub fn title_bar(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.content_width + CHROME.0,
            TITLE_HEIGHT + 1,
        )
    }

    pub fn content(&self) -> Rect {
        Rect::new(
            self.x + 1,
            self.y + 1 + TITLE_HEIGHT,
            self.content_width,
            self.content_height,
        )
    }

    pub fn close_control(&self) -> Rect {
        Rect::new(
            self.x + CLOSE_HIT_X,
            self.y,
            CONTROL_HIT_WIDTH,
            TITLE_HEIGHT,
        )
    }

    pub fn minimise_control(&self) -> Option<Rect> {
        (self.role == Role::Normal).then(|| {
            Rect::new(
                self.x + MINIMISE_HIT_X,
                self.y,
                CONTROL_HIT_WIDTH,
                TITLE_HEIGHT,
            )
        })
    }

    pub fn part_at(&self, x: u32, y: u32) -> Option<Part> {
        if !contains(self.outside(), x, y) {
            return None;
        }
        Some(match () {
            () if contains(self.close_control(), x, y) => Part::Close,
            () if self
                .minimise_control()
                .is_some_and(|control| contains(control, x, y)) =>
            {
                Part::Minimise
            }
            () if contains(self.title_bar(), x, y) => Part::TitleBar,
            () if contains(self.content(), x, y) => Part::Content,
            () => Part::Border,
        })
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

/// The content size a window asking for `width` by `height` gets on a `screen_width` by
/// `screen_height` screen: at least the minimum window and at most the work area less
/// [`DEFAULT_MARGIN`] each way (the minimum giving way when the work area is smaller).
pub fn clamp_content_size(
    screen_width: u32,
    screen_height: u32,
    width: u32,
    height: u32,
) -> (u32, u32) {
    let work_height = screen_height.saturating_sub(BAR_HEIGHT);
    let clamp = |wanted: u32, chrome: u32, minimum: u32, room: u32| {
        let largest = room.saturating_sub(DEFAULT_MARGIN).max(chrome + 1);
        wanted
            .saturating_add(chrome)
            .max(minimum)
            .min(largest)
            .max(chrome + 1)
            - chrome
    };
    (
        clamp(width, CHROME.0, MINIMUM_OUTSIDE.0, screen_width),
        clamp(height, CHROME.1, MINIMUM_OUTSIDE.1, work_height),
    )
}

/// The slot of window `id`.
pub fn slot_of(id: WindowId) -> usize {
    usize::from(id) & ((1 << SLOT_BITS) - 1)
}
