//! `NSAlert` as the toolkit's dialog panel filling a dialog window's content
//! (`docs/ux/macos-gui-v0.3.md` section 4): the message and informative text as its lines, the
//! buttons in macOS's order (the first added is the default, drawn rightmost), Enter for the
//! default, Esc for the button titled `Cancel` (none: Esc does nothing).

use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::elements::{Action, Bounds, Element, Outcome, Role, TreeWriter, flags};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;
use eggos_toolkit::panel::{self, Panel, PanelResponse};

/// The most buttons an alert shows.
pub const MAX_BUTTONS: usize = panel::MAX_BUTTONS;
/// The content's width.
pub const WIDTH: i32 = panel::WIDTH;
/// `NSAlertFirstButtonReturn`: the first added button's answer; the others count up from it.
pub const FIRST_BUTTON_RETURN: isize = 1000;
const LINE_PITCH: i32 = 20;
const FRAME_HEIGHT: i32 = 72;
const DEFAULT_BUTTONS: [&str; 1] = ["OK"];
const CANCEL: &str = "Cancel";

/// An alert: its panel, how many buttons it has and which one Esc chooses, by the order they
/// were added.
#[derive(Debug, Clone, Copy)]
pub struct Alert {
    panel: Panel,
    buttons: usize,
    lines: usize,
    cancel: Option<usize>,
}

impl Alert {
    /// The alert of `message`, `informative` (none when empty) and `buttons` as added (`OK`
    /// when none, at most [`MAX_BUTTONS`]).
    pub fn new(message: &str, informative: &str, buttons: &[&str]) -> Self {
        let buttons: &[&str] = match buttons.is_empty() {
            true => &DEFAULT_BUTTONS,
            false => &buttons[..buttons.len().min(MAX_BUTTONS)],
        };
        let count = buttons.len();
        let mut shown = [""; MAX_BUTTONS];
        for (index, &title) in buttons.iter().enumerate() {
            shown[count - 1 - index] = title;
        }
        let both = [message, informative];
        let lines = match informative.is_empty() {
            true => &both[..1],
            false => &both[..],
        };
        let cancel = buttons.iter().position(|title| *title == CANCEL);
        let panel = Panel::new(
            lines,
            &shown[..count],
            Some(count - 1),
            cancel.map_or(0, |added| count - 1 - added),
        );
        Self {
            panel,
            buttons: count,
            lines: lines.len(),
            cancel,
        }
    }

    /// The content size its dialog window asks for.
    pub fn size(&self) -> (u32, u32) {
        (
            WIDTH as u32,
            (LINE_PITCH * self.lines as i32 + FRAME_HEIGHT) as u32,
        )
    }

    fn area(&self) -> Rect {
        let (width, height) = self.size();
        Rect::new(0, 0, width as i32, height as i32)
    }

    fn added(&self, response: PanelResponse) -> Option<usize> {
        match response {
            PanelResponse::Pressed(shown) if shown < self.buttons => Some(self.buttons - 1 - shown),
            _ => None,
        }
    }

    /// Key `code` went down (`pressed`) or up, Shift held when `shift`: the button chosen, by
    /// the order added, if any.
    pub fn key(&mut self, code: u16, shift: bool, pressed: bool) -> Option<usize> {
        if code == key::ESC && self.cancel.is_none() {
            return None;
        }
        let response = self.panel.key(code, shift, pressed);
        self.added(response)
    }

    /// The left button went down (`pressed`) or up at content pixel `(x, y)`: the button
    /// chosen, by the order added, if any.
    pub fn button(&mut self, x: i32, y: i32, pressed: bool) -> Option<usize> {
        let area = self.area();
        let response = self.panel.button(area, x, y, pressed);
        self.added(response)
    }

    /// What `runModal` returns for the button added `index`th.
    pub fn answer(index: usize) -> isize {
        FIRST_BUTTON_RETURN + index as isize
    }

    fn first_button_element(&self) -> u32 {
        2 + self.lines as u32
    }

    /// Writes its element tree (D377, section 16): the window named `title`, the message and
    /// informative lines as labels, then the buttons left to right; false when the tree is
    /// full.
    pub fn describe(&self, title: &str, focused: bool, tree: &mut TreeWriter<'_>) -> bool {
        let area = self.area();
        let root = Element::new(1, 0, Role::Window, bounds(area)).named(title.as_bytes());
        if !tree.push(&root) {
            return false;
        }
        for line in 0..self.lines {
            let text = self.panel.line(line);
            let element = Element::new(
                2 + line as u32,
                1,
                Role::Label,
                bounds(self.panel.line_rect(area, line)),
            )
            .named(text.as_bytes());
            if !tree.push(&element) {
                return false;
            }
        }
        for shown in 0..self.panel.button_count() {
            let mut state = flags::FOCUSABLE;
            if focused && self.panel.focused() == shown {
                state |= flags::FOCUSED;
            }
            if self.panel.primary() == Some(shown) {
                state |= flags::DEFAULT;
            }
            let element = Element::new(
                self.first_button_element() + shown as u32,
                1,
                Role::Button,
                bounds(self.panel.button_rect(area, shown)),
            )
            .named(self.panel.button_title(shown).as_bytes())
            .flagged(state);
            if !tree.push(&element) {
                return false;
            }
        }
        true
    }

    /// An agent's `action` on `element`: what it did, and the button chosen by the order
    /// added when it pressed one.
    pub fn act(&mut self, element: u32, action: Action) -> (Outcome, Option<usize>) {
        let first = self.first_button_element();
        let last = first + self.panel.button_count() as u32;
        if element == 0 || element >= last {
            return (Outcome::NoSuchElement, None);
        }
        if element < first {
            return (Outcome::NotSupported, None);
        }
        let shown = (element - first) as usize;
        match action {
            Action::Invoke => (Outcome::Done, Some(self.buttons - 1 - shown)),
            Action::Focus => {
                self.panel.set_focused(shown);
                (Outcome::Done, None)
            }
            Action::SetText | Action::Select => (Outcome::NotSupported, None),
        }
    }

    /// Draws it over the whole content, its focused button ringed when the dialog has the
    /// keyboard.
    pub fn paint(&self, canvas: &mut Canvas<'_>, focused: bool) {
        self.panel.paint(canvas, self.area(), focused);
    }
}

fn bounds(rect: Rect) -> Bounds {
    Bounds::new(rect.x, rect.y, rect.width, rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_button_added_is_the_default_and_drawn_rightmost() {
        let mut alert = Alert::new(
            "mac-paint",
            "A programmatic AppKit app on EggOS.",
            &["OK", "Cancel"],
        );
        assert_eq!(alert.size(), (400, 112));
        assert_eq!(alert.key(key::ENTER, false, true), Some(0));
        assert_eq!(alert.key(key::ESC, false, true), Some(1));
        assert_eq!(alert.button(300, 80, true), None);
        assert_eq!(alert.button(300, 80, false), Some(0));
        assert_eq!(alert.button(200, 80, true), None);
        assert_eq!(alert.button(200, 80, false), Some(1));
        assert_eq!(Alert::answer(1), 1001);
    }

    #[test]
    fn without_cancel_esc_does_nothing_and_no_button_means_ok() {
        let mut alert = Alert::new("Saved.", "", &[]);
        assert_eq!(alert.size(), (400, 92));
        assert_eq!(alert.key(key::ESC, false, true), None);
        assert_eq!(alert.key(key::ENTER, false, true), Some(0));
        let mut pixels = vec![0u32; 400 * 92];
        let mut canvas = Canvas::new(&mut pixels, 400, 92, 1).unwrap();
        alert.paint(&mut canvas, true);
        assert_eq!(pixels[10 * 400 + 200], eggos_toolkit::colour::MENU);
    }

    #[test]
    fn invoking_a_button_answers_its_order_added() {
        let mut alert = Alert::new("mac-paint", "Info.", &["OK", "Cancel"]);
        assert_eq!(alert.act(4, Action::Invoke), (Outcome::Done, Some(1)));
        assert_eq!(alert.act(5, Action::Invoke), (Outcome::Done, Some(0)));
        assert_eq!(alert.act(5, Action::SetText), (Outcome::NotSupported, None));
        assert_eq!(alert.act(2, Action::Invoke), (Outcome::NotSupported, None));
        assert_eq!(alert.act(6, Action::Invoke), (Outcome::NoSuchElement, None));
    }

    #[test]
    fn focus_moves_the_ring_and_invoke_still_answers_the_button() {
        let mut alert = Alert::new("mac-paint", "Info.", &["OK", "Cancel"]);
        assert_eq!(alert.act(4, Action::Focus), (Outcome::Done, None));
        assert_eq!(alert.panel.focused(), 0);
        assert_eq!(alert.act(5, Action::Invoke), (Outcome::Done, Some(0)));
    }

    #[test]
    fn a_one_line_alert_s_button_is_element_three() {
        let mut alert = Alert::new("Saved.", "", &[]);
        assert_eq!(alert.act(3, Action::Invoke), (Outcome::Done, Some(0)));
    }
}
