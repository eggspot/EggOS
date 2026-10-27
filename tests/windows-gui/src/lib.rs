//! What `win-paint.exe`, the Windows GUI test program (D129), paints, as data both sides share:
//! the program paints it through GDI's own calls into its window, and the QEMU test paints it
//! with `eggos_windows_gui` on the host to know the picture the desktop must show. Colours are
//! `COLORREF`s (`0x00BBGGRR`), rectangles Win32's `RECT`.

#![no_std]

/// The window's client area the program asks for.
pub const CLIENT: (i32, i32) = (400, 300);
/// The window's outside size it passes to `CreateWindowExW`: the client area and EggOS's
/// chrome (a 1 px border and a 28 px title bar).
pub const OUTSIDE: (i32, i32) = (CLIENT.0 + 2, CLIENT.1 + 30);
/// The window's title.
pub const TITLE: &str = "win-paint";
/// The message box's text and caption.
pub const MESSAGE: &str = "You pressed the space bar.";
pub const MESSAGE_CAPTION: &str = "win-paint";
/// The message box's type: `MB_OKCANCEL | MB_ICONINFORMATION`.
pub const MESSAGE_TYPE: u32 = 0x1 | 0x40;
/// The message box a second thread shows, and its type: `MB_OK | MB_ICONINFORMATION`.
pub const THREAD_MESSAGE: &str = "From a thread.";
pub const THREAD_MESSAGE_TYPE: u32 = 0x40;
/// The class background, `COLOR_WINDOW + 1`.
pub const BACKGROUND: usize = 5 + 1;
/// The window's background: `COLOR_WINDOW`, white.
pub const WINDOW_COLOUR: u32 = 0x00ff_ffff;

const BLACK: u32 = 0x0000_0000;
const WHITE: u32 = 0x00ff_ffff;
const BAND: u32 = 0x0090_5020;
const SQUARE: u32 = 0x003d_b3f2;
const RED: u32 = 0x0040_40e0;
const GREEN: u32 = 0x0050_b030;
const BLUE: u32 = 0x00d0_6010;
const FIELD: u32 = 0x00e8_e8e8;
const DIM: u32 = 0x0060_6060;
const MAX_TYPED: usize = 32;
const TYPED_PREFIX: &str = "Typed: ";
const DOTS: i32 = 12;

/// `DrawTextW`'s flags the scene uses.
pub mod format {
    pub const CENTER: u32 = 0x1;
    pub const VCENTER: u32 = 0x4;
    pub const SINGLELINE: u32 = 0x20;
}

/// Win32's `RECT`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

/// A pen: its colour and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pen {
    pub colour: u32,
    pub width: i32,
}

/// The GDI calls the scene is painted with; text is ASCII.
pub trait Painter {
    /// `FillRect` with a solid brush.
    fn fill(&mut self, rect: Rect, colour: u32);
    /// `Rectangle` with a solid pen and a solid brush, or the null brush.
    fn rectangle(&mut self, rect: Rect, pen: Pen, brush: Option<u32>);
    /// `Ellipse` likewise.
    fn ellipse(&mut self, rect: Rect, pen: Pen, brush: Option<u32>);
    /// `MoveToEx` and `LineTo`.
    fn line(&mut self, from: (i32, i32), to: (i32, i32), pen: Pen);
    /// `SetPixel`.
    fn pixel(&mut self, x: i32, y: i32, colour: u32);
    /// `TextOutW`, opaque on `background` or transparent.
    fn text(&mut self, x: i32, y: i32, text: &str, colour: u32, background: Option<u32>);
    /// `DrawTextW` with `format`.
    fn text_in(
        &mut self,
        rect: Rect,
        text: &str,
        format: u32,
        colour: u32,
        background: Option<u32>,
    );
    /// `GetTextExtentPoint32W`'s width.
    fn text_width(&mut self, text: &str) -> i32;
}

/// The program's state, which decides what it paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scene {
    pub clicked: bool,
    typed: [u8; MAX_TYPED],
    typed_length: usize,
    pub message_box_result: Option<i32>,
    pub thread_message_box_result: Option<i32>,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

const BAND_RECT: Rect = rect(0, 0, CLIENT.0, 40);
const SQUARE_RECT: Rect = rect(24, 64, 184, 184);
const ELLIPSE_RECT: Rect = rect(216, 64, 376, 184);
const TYPED_RECT: Rect = rect(24, 236, 376, 260);
const STATUS_RECT: Rect = rect(0, 268, CLIENT.0, 288);

impl Scene {
    pub const fn new() -> Self {
        Self {
            clicked: false,
            typed: [0; MAX_TYPED],
            typed_length: 0,
            message_box_result: None,
            thread_message_box_result: None,
        }
    }

    /// Adds `character` to the typed text when it is an ASCII letter or digit and there is
    /// room; whether it did.
    pub fn type_character(&mut self, character: u32) -> bool {
        let Ok(byte) = u8::try_from(character) else {
            return false;
        };
        if !byte.is_ascii_alphanumeric() || self.typed_length == MAX_TYPED {
            return false;
        }
        self.typed[self.typed_length] = byte;
        self.typed_length += 1;
        true
    }

    /// Where the typed text is, which a key repaints.
    pub fn typed_rect(&self) -> Rect {
        TYPED_RECT
    }

    /// Where the status line is, which the message box's answer repaints.
    pub fn status_rect(&self) -> Rect {
        STATUS_RECT
    }

    /// The point a click toggles the ellipse's colour at: its centre.
    pub fn ellipse_centre(&self) -> (i32, i32) {
        (
            (ELLIPSE_RECT.left + ELLIPSE_RECT.right) / 2,
            (ELLIPSE_RECT.top + ELLIPSE_RECT.bottom) / 2,
        )
    }

    /// Paints the ellipse alone, as a click redraws it through `GetDC`.
    pub fn paint_ellipse(&self, painter: &mut impl Painter) {
        let colour = if self.clicked { GREEN } else { RED };
        painter.ellipse(
            ELLIPSE_RECT,
            Pen {
                colour: BLACK,
                width: 1,
            },
            Some(colour),
        );
    }

    /// Paints everything but the background, which the class's brush erases first.
    pub fn paint(&self, painter: &mut impl Painter) {
        painter.fill(BAND_RECT, BAND);
        let heading = "EggOS win-paint";
        let x = (CLIENT.0 - painter.text_width(heading)) / 2;
        painter.text(x, 12, heading, WHITE, None);
        painter.rectangle(
            SQUARE_RECT,
            Pen {
                colour: BLACK,
                width: 2,
            },
            Some(SQUARE),
        );
        painter.rectangle(
            rect(48, 88, 160, 160),
            Pen {
                colour: BLUE,
                width: 1,
            },
            None,
        );
        self.paint_ellipse(painter);
        painter.line(
            (24, 200),
            (376, 200),
            Pen {
                colour: BLUE,
                width: 1,
            },
        );
        painter.line(
            (24, 208),
            (376, 226),
            Pen {
                colour: RED,
                width: 3,
            },
        );
        for dot in 0..DOTS {
            painter.pixel(24 + dot * 4, 230, BLACK);
        }
        let mut typed = [0u8; TYPED_PREFIX.len() + MAX_TYPED];
        typed[..TYPED_PREFIX.len()].copy_from_slice(TYPED_PREFIX.as_bytes());
        typed[TYPED_PREFIX.len()..TYPED_PREFIX.len() + self.typed_length]
            .copy_from_slice(&self.typed[..self.typed_length]);
        let typed = core::str::from_utf8(&typed[..TYPED_PREFIX.len() + self.typed_length])
            .unwrap_or(TYPED_PREFIX);
        painter.fill(TYPED_RECT, FIELD);
        painter.text(
            TYPED_RECT.left + 4,
            TYPED_RECT.top + 4,
            typed,
            BLACK,
            Some(FIELD),
        );
        let status = match (self.thread_message_box_result, self.message_box_result) {
            (Some(1), _) => "Thread's message box: OK",
            (Some(_), _) => "Thread's message box: other",
            (None, None) => "Space: message box",
            (None, Some(1)) => "Message box: OK",
            (None, Some(2)) => "Message box: Cancel",
            (None, Some(_)) => "Message box: other",
        };
        painter.text_in(
            STATUS_RECT,
            status,
            format::CENTER | format::VCENTER | format::SINGLELINE,
            DIM,
            None,
        );
    }
}
