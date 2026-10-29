//! What `mac-paint.app`, the macOS GUI test app (D147, `tests/macos-gui/main.m`), shows, as
//! data and a model the host shares with the app's source: the frames and texts `main.m` uses,
//! the main menu the layer sends for it, and the window content the layer draws for each state,
//! painted with the same `eggos_macos_gui` functions the layer draws with. A change to `main.m`
//! is mirrored here.

#![no_std]

use eggos_macos_gui::alert::Alert;
use eggos_macos_gui::colour::{self, Colour};
use eggos_macos_gui::geometry::{Frame, Point, pixel_rect, point_of_pixel};
use eggos_macos_gui::keys::modifier;
use eggos_macos_gui::menu::{Pool, key_label};
use eggos_macos_gui::paint;
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text::Text;
use eggos_toolkit::widget::{self, Escape, Field, Look};

/// The app's name, its window's first title and its bundle's folder name without `.app`.
pub const NAME: &str = "mac-paint";
/// The window's content size.
pub const CONTENT: (u32, u32) = (400, 300);
const HEIGHT: i32 = CONTENT.1 as i32;
/// The paint view's frame.
pub const PAINT: Frame = Frame::new(0.0, 48.0, 400.0, 252.0);
/// The text field's frame and placeholder.
pub const FIELD: Frame = Frame::new(8.0, 10.0, 184.0, 28.0);
pub const PLACEHOLDER: &str = "Title";
/// The `Clear` button's frame.
pub const CLEAR: Frame = Frame::new(200.0, 10.0, 96.0, 28.0);
/// The `Info` button's frame: the window's default button.
pub const INFO: Frame = Frame::new(304.0, 8.0, 88.0, 32.0);
const OVAL: Frame = Frame::new(364.0, 216.0, 24.0, 24.0);
const CAPTION_AT: Point = Point::new(8.0, 228.0);
const CAPTION: &str = "Click to paint, c clears";
const RULE_FROM: Point = Point::new(8.0, 224.0);
const RULE_TO: Point = Point::new(192.0, 224.0);
const SQUARE: f64 = 8.0;
const MAX_SQUARES: usize = 8;
/// The alert `Info` opens.
pub const ALERT_MESSAGE: &str = "mac-paint";
pub const ALERT_INFORMATIVE: &str = "A programmatic AppKit app on EggOS.";
pub const ALERT_BUTTONS: [&str; 2] = ["OK", "Cancel"];

fn named(name: &[u8]) -> Colour {
    colour::named(name).unwrap_or(colour::DEFAULT_TEXT)
}

/// The main menu's pool, as the layer sends it for `main.m`'s menu.
pub fn menu_pool() -> Pool {
    let mut pool = Pool::new();
    let quit = key_label(u32::from(b'q'), modifier::COMMAND);
    let clear = key_label(u32::from(b'k'), modifier::COMMAND);
    let close = key_label(u32::from(b'w'), modifier::COMMAND);
    pool.menu(NAME);
    pool.item("About mac-paint", "", true);
    pool.separator();
    pool.item("Quit mac-paint", quit.as_str(), true);
    pool.menu("File");
    pool.item("Clear", clear.as_str(), true);
    pool.item("Close", close.as_str(), true);
    pool.menu("Help");
    pool.item("mac-paint Help", "", false);
    pool
}

/// The alert `Info` opens, as the layer lays it out.
pub fn alert() -> Alert {
    Alert::new(ALERT_MESSAGE, ALERT_INFORMATIVE, &ALERT_BUTTONS)
}

/// Paints the alert's content, `Alert::size` big, focused.
pub fn paint_alert(pixels: &mut [u32]) {
    let alert = alert();
    let (width, height) = alert.size();
    if let Some(mut canvas) = Canvas::new(pixels, width, height, 1) {
        alert.paint(&mut canvas, true);
    }
}

/// The app's state as far as its window shows it.
#[derive(Debug, Clone, Copy)]
pub struct MacPaint {
    squares: [Point; MAX_SQUARES],
    count: usize,
    /// The field's text, caret and scroll.
    pub field: Field,
    /// Whether the field is the first responder (the paint view is, once clicked).
    pub field_focused: bool,
    /// Whether the window has the keyboard.
    pub key: bool,
    /// The window's title.
    pub title: Text<64>,
}

impl Default for MacPaint {
    fn default() -> Self {
        Self::new()
    }
}

impl MacPaint {
    /// The app as it opens: no squares, an empty focused field, titled [`NAME`].
    pub fn new() -> Self {
        Self {
            squares: [Point::default(); MAX_SQUARES],
            count: 0,
            field: Field {
                text: Text::new(),
                caret: 0,
                scroll: 0,
                placeholder: PLACEHOLDER,
                escape: Escape::Clear,
            },
            field_focused: true,
            key: true,
            title: Text::from(NAME),
        }
    }

    /// Types `text` into the focused field, as the layer applies each key.
    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            widget::field_key(
                &mut self.field,
                0,
                u32::from(character),
                false,
                &Text::new(),
            );
            widget::keep_caret_visible(&mut self.field, paint::control_rect(FIELD, HEIGHT));
        }
    }

    /// Enter in the field: its action renames the window.
    pub fn enter(&mut self) {
        self.title = Text::from(self.field.text.as_str());
    }

    /// A click at content pixel `(x, y)` inside the paint view: a square there, and the view
    /// becomes the first responder.
    pub fn click_paint(&mut self, x: i32, y: i32) {
        self.field_focused = false;
        if self.count == MAX_SQUARES {
            return;
        }
        let point = point_of_pixel(x, y, HEIGHT);
        self.squares[self.count] = Point::new(point.x - PAINT.x, point.y - PAINT.y);
        self.count += 1;
    }

    /// `c` in the paint view, `Clear` or `File` > `Clear`.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// The content pixel at the centre of `frame`'s drawn control, where a click presses it.
    pub fn centre_of(frame: Frame) -> (i32, i32) {
        let rect = paint::control_rect(frame, HEIGHT);
        (rect.x + rect.width / 2, rect.y + rect.height / 2)
    }

    fn paint_view(&self, canvas: &mut Canvas<'_>) {
        let previous = canvas.narrow(pixel_rect(PAINT, HEIGHT));
        let at = |point: Point| Point::new(point.x + PAINT.x, point.y + PAINT.y);
        paint::fill(canvas, HEIGHT, PAINT, named(b"textBackgroundColor"));
        paint::text(
            canvas,
            HEIGHT,
            at(CAPTION_AT),
            CAPTION,
            named(b"secondaryLabelColor"),
            1,
        );
        paint::line(
            canvas,
            HEIGHT,
            at(RULE_FROM),
            at(RULE_TO),
            named(b"separatorColor"),
        );
        let oval = OVAL.offset(PAINT.x, PAINT.y);
        paint::oval(canvas, HEIGHT, oval, named(b"systemYellowColor"), true);
        paint::oval(canvas, HEIGHT, oval, named(b"labelColor"), false);
        for centre in &self.squares[..self.count] {
            let square = Frame::new(
                centre.x - SQUARE / 2.0,
                centre.y - SQUARE / 2.0,
                SQUARE,
                SQUARE,
            );
            paint::fill(
                canvas,
                HEIGHT,
                square.offset(PAINT.x, PAINT.y),
                named(b"systemBlueColor"),
            );
        }
        canvas.restore(previous);
    }

    /// Paints the window's content, [`CONTENT`] big, as the layer draws it.
    pub fn paint(&self, pixels: &mut [u32]) {
        let Some(mut canvas) = Canvas::new(pixels, CONTENT.0, CONTENT.1, 1) else {
            return;
        };
        canvas.fill(
            Rect::new(0, 0, CONTENT.0 as i32, CONTENT.1 as i32),
            colour::WINDOW_BACKGROUND.pixel,
        );
        self.paint_view(&mut canvas);
        let enabled = Look {
            enabled: true,
            ..Look::default()
        };
        paint::text_field(
            &mut canvas,
            HEIGHT,
            FIELD,
            &self.field,
            Look {
                focused: self.field_focused && self.key,
                caret_shown: true,
                ..enabled
            },
        );
        paint::button(&mut canvas, HEIGHT, CLEAR, "Clear", false, enabled);
        paint::button(&mut canvas, HEIGHT, INFO, "Info", true, enabled);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;

    #[test]
    fn a_click_paints_a_blue_square_centred_on_it() {
        let mut app = MacPaint::new();
        app.click_paint(100, 100);
        assert!(!app.field_focused);
        let mut pixels = vec![0u32; (CONTENT.0 * CONTENT.1) as usize];
        app.paint(&mut pixels);
        let blue = colour::named(b"systemBlueColor").unwrap().pixel;
        assert_eq!(pixels[100 * 400 + 100], blue);
        assert_eq!(pixels[97 * 400 + 96], blue);
        assert_eq!(pixels[104 * 400 + 103], blue);
        assert_ne!(pixels[96 * 400 + 96], blue);
        app.clear();
        app.paint(&mut pixels);
        assert_ne!(pixels[100 * 400 + 100], blue);
    }

    #[test]
    fn typing_and_enter_rename_the_window() {
        let mut app = MacPaint::new();
        app.type_text("Hello");
        assert_eq!(app.field.text.as_str(), "Hello");
        assert_eq!(app.field.caret, 5);
        app.enter();
        assert_eq!(app.title.as_str(), "Hello");
        assert_eq!(MacPaint::centre_of(CLEAR), (248, 276));
        assert_eq!(MacPaint::centre_of(INFO), (348, 276));
    }

    #[test]
    fn the_menu_and_the_alert_are_as_main_m_makes_them() {
        assert_eq!(
            menu_pool().as_bytes(),
            b"Mmac-paint\nIAbout mac-paint\t\n-\nIQuit mac-paint\tCmd+Q\nMFile\nIClear\tCmd+K\nIClose\tCmd+W\nMHelp\nDmac-paint Help\t\n"
        );
        assert_eq!(alert().size(), (400, 112));
        let mut pixels = vec![0u32; 400 * 112];
        paint_alert(&mut pixels);
        assert_eq!(pixels[50 * 400 + 200], eggos_toolkit::colour::MENU);
    }
}
