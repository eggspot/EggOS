//! A modal dialog panel drawn inside a window's content (docs/ux/desktop-v0.3-gui.md section
//! 8.1 items 3 and 5): 400 wide, lines of text at pitch 20, push buttons at the bottom right
//! with the primary last, or a progress bar and `Cancel`; Tab moves between the buttons, Space
//! presses the focused one, Enter the primary, Esc the cancel.

use crate::canvas::{Canvas, INSET_R6};
use crate::colour;
use crate::geometry::Rect;
use crate::input::key;
use crate::text::Text;
use crate::widget::{self, CONTROL_HEIGHT, Kind, Look, PROGRESS_DONE, PROGRESS_HEIGHT};

/// The panel's width.
pub const WIDTH: i32 = 400;
/// The most lines a panel holds.
pub const MAX_LINES: usize = 7;
/// The most buttons a panel holds.
pub const MAX_BUTTONS: usize = 3;
const PADDING: i32 = 16;
const LINE_PITCH: i32 = 20;
const BUTTON_GAP: i32 = 8;
const EDGE: i32 = 2;
const PROGRESS_TOP: i32 = 44;
const PROGRESS_BUTTON_TOP: i32 = 68;
const PROGRESS_PANEL_HEIGHT: i32 = 112;

/// What an input did to a panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelResponse {
    Nothing,
    /// Button `index` was pressed.
    Pressed(usize),
}

/// A dialog panel. See the module documentation.
#[derive(Debug, Clone, Copy)]
pub struct Panel {
    lines: [Text<128>; MAX_LINES],
    line_count: usize,
    buttons: [Text<32>; MAX_BUTTONS],
    button_count: usize,
    primary: Option<usize>,
    cancel: usize,
    focused: usize,
    hovered: Option<usize>,
    pressed: Option<usize>,
    progress: Option<u16>,
}

impl Panel {
    /// A panel of `lines` (the first in `text`, the rest in `text.dim`) and `buttons`, left to
    /// right; `primary` is Enter's, `cancel` Esc's. The focus starts on the primary.
    pub fn new(lines: &[&str], buttons: &[&str], primary: Option<usize>, cancel: usize) -> Self {
        let mut panel = Self {
            lines: [Text::new(); MAX_LINES],
            line_count: lines.len().min(MAX_LINES),
            buttons: [Text::new(); MAX_BUTTONS],
            button_count: buttons.len().clamp(1, MAX_BUTTONS),
            primary,
            cancel,
            focused: primary.unwrap_or(cancel),
            hovered: None,
            pressed: None,
            progress: None,
        };
        for (held, line) in panel.lines.iter_mut().zip(lines) {
            held.set(line);
        }
        for (held, button) in panel.buttons.iter_mut().zip(buttons) {
            held.set(button);
        }
        panel
    }

    /// A progress panel: `line`, the bar `permille` done, and `Cancel`.
    pub fn progress(line: &str, permille: u16) -> Self {
        let mut panel = Self::new(&[line], &["Cancel"], None, 0);
        panel.progress = Some(permille.min(PROGRESS_DONE));
        panel
    }

    /// Whether it is a progress panel.
    pub fn is_progress(&self) -> bool {
        self.progress.is_some()
    }

    /// Moves a progress panel's bar.
    pub fn set_progress(&mut self, permille: u16) {
        if self.progress.is_some() {
            self.progress = Some(permille.min(PROGRESS_DONE));
        }
    }

    /// Line `index`'s text.
    pub fn line(&self, index: usize) -> &str {
        self.lines[..self.line_count]
            .get(index)
            .map_or("", |line| line.as_str())
    }

    fn height(&self) -> i32 {
        match self.progress {
            Some(_) => PROGRESS_PANEL_HEIGHT,
            None => LINE_PITCH * self.line_count as i32 + 72,
        }
    }

    /// Where the panel is when centred in `area`.
    pub fn rect(&self, area: Rect) -> Rect {
        let height = self.height();
        Rect::new(
            area.x + (area.width - WIDTH) / 2,
            area.y + (area.height - height) / 2,
            WIDTH,
            height,
        )
    }

    fn button_width(&self, index: usize) -> i32 {
        Kind::button(self.buttons[index].as_str(), false)
            .content_size()
            .0
    }

    /// Where button `index` is when the panel is centred in `area`.
    pub fn button_rect(&self, area: Rect, index: usize) -> Rect {
        let rect = self.rect(area);
        let top = match self.progress {
            Some(_) => rect.y + PROGRESS_BUTTON_TOP,
            None => rect.y + LINE_PITCH * self.line_count as i32 + 28,
        };
        let mut right = rect.right() - PADDING;
        for later in (index + 1..self.button_count).rev() {
            right -= self.button_width(later) + BUTTON_GAP;
        }
        let width = self.button_width(index);
        Rect::new(right - width, top, width, CONTROL_HEIGHT)
    }

    fn button_at(&self, area: Rect, x: i32, y: i32) -> Option<usize> {
        (0..self.button_count).find(|&index| self.button_rect(area, index).contains(x, y))
    }

    /// Key `code` went down (`pressed`) or up, with Shift held when `shift`.
    pub fn key(&mut self, code: u16, shift: bool, pressed: bool) -> PanelResponse {
        if !pressed {
            return match (code, self.pressed.take()) {
                (key::SPACE, Some(index)) if index == self.focused => PanelResponse::Pressed(index),
                _ => PanelResponse::Nothing,
            };
        }
        match code {
            key::TAB => {
                let count = self.button_count;
                self.focused = match shift {
                    true => (self.focused + count - 1) % count,
                    false => (self.focused + 1) % count,
                };
                PanelResponse::Nothing
            }
            key::SPACE => {
                self.pressed = Some(self.focused);
                PanelResponse::Nothing
            }
            key::ENTER => match self.primary {
                Some(primary) => PanelResponse::Pressed(primary),
                None => PanelResponse::Nothing,
            },
            key::ESC => PanelResponse::Pressed(self.cancel),
            _ => PanelResponse::Nothing,
        }
    }

    /// The pointer is at (`x`, `y`) with the panel centred in `area`; whether its look changed.
    pub fn pointer_move(&mut self, area: Rect, x: i32, y: i32) -> bool {
        let hovered = self.button_at(area, x, y);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// The pointer left the content.
    pub fn pointer_leave(&mut self) -> bool {
        self.hovered.take().is_some()
    }

    /// The left button went down (`pressed`) or up at (`x`, `y`).
    pub fn button(&mut self, area: Rect, x: i32, y: i32, pressed: bool) -> PanelResponse {
        let at = self.button_at(area, x, y);
        if pressed {
            if let Some(index) = at {
                self.focused = index;
            }
            self.pressed = at;
            return PanelResponse::Nothing;
        }
        match (self.pressed.take(), at) {
            (Some(down), Some(up)) if down == up => PanelResponse::Pressed(up),
            _ => PanelResponse::Nothing,
        }
    }

    /// Draws it centred in `area`, its focused button ringed when the window has the keyboard.
    pub fn paint(&self, canvas: &mut Canvas<'_>, area: Rect, window_focused: bool) {
        let rect = self.rect(area);
        canvas.rounded(
            Rect::new(
                rect.x - EDGE,
                rect.y - EDGE,
                rect.width + 2 * EDGE,
                rect.height + 2 * EDGE,
            ),
            &INSET_R6,
            colour::FIELD,
        );
        canvas.rounded(rect, &INSET_R6, colour::MENU);
        canvas.outline(rect, &INSET_R6, 1, colour::MENU_BORDER);
        let inner = rect.width - 2 * PADDING;
        for (index, line) in self.lines[..self.line_count].iter().enumerate() {
            let ink = match index {
                0 => colour::TEXT,
                _ => colour::TEXT_DIM,
            };
            widget::clipped_text(
                canvas,
                rect.x + PADDING,
                rect.y + PADDING + LINE_PITCH * index as i32,
                inner,
                line.as_str(),
                ink,
            );
        }
        if let Some(permille) = self.progress {
            widget::paint(
                canvas,
                &Kind::Progress { permille },
                Rect::new(
                    rect.x + PADDING,
                    rect.y + PROGRESS_TOP,
                    inner,
                    PROGRESS_HEIGHT,
                ),
                Look::default(),
            );
        }
        for index in 0..self.button_count {
            let look = Look {
                hovered: (self.hovered == Some(index)).then_some(0),
                pressed: (self.pressed == Some(index)).then_some(0),
                focused: window_focused && self.focused == index,
                enabled: true,
                caret_shown: false,
            };
            widget::paint(
                canvas,
                &Kind::button(self.buttons[index].as_str(), self.primary == Some(index)),
                self.button_rect(area, index),
                look,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 32, 720, 424);

    #[test]
    fn a_panel_is_centred_with_its_buttons_at_the_bottom_right_primary_last() {
        let panel = Panel::new(
            &["Delete notes.txt permanently?", "This cannot be undone."],
            &["Cancel", "Delete"],
            Some(1),
            0,
        );
        let rect = panel.rect(AREA);
        assert_eq!(rect, Rect::new(160, 32 + (424 - 112) / 2, 400, 112));
        let delete = panel.button_rect(AREA, 1);
        let cancel = panel.button_rect(AREA, 0);
        assert_eq!(delete.right(), rect.right() - 16);
        assert_eq!(delete.y, rect.y + 68);
        assert_eq!(cancel.right(), delete.x - 8);
        assert_eq!(panel.line(1), "This cannot be undone.");
    }

    #[test]
    fn keys_move_the_focus_and_press_the_primary_or_the_cancel() {
        let mut panel = Panel::new(&["Empty the Trash?"], &["Cancel", "Empty"], Some(1), 0);
        assert_eq!(
            panel.key(key::ENTER, false, true),
            PanelResponse::Pressed(1)
        );
        assert_eq!(panel.key(key::ESC, false, true), PanelResponse::Pressed(0));
        panel.key(key::TAB, false, true);
        assert_eq!(panel.key(key::SPACE, false, true), PanelResponse::Nothing);
        assert_eq!(
            panel.key(key::SPACE, false, false),
            PanelResponse::Pressed(0)
        );
        panel.key(key::TAB, true, true);
        panel.key(key::SPACE, false, true);
        assert_eq!(
            panel.key(key::SPACE, false, false),
            PanelResponse::Pressed(1)
        );
    }

    #[test]
    fn a_click_presses_the_button_it_went_down_and_up_on() {
        let mut panel = Panel::new(&["Properties"], &["Close"], Some(0), 0);
        let close = panel.button_rect(AREA, 0);
        let (x, y) = (close.x + 4, close.y + 4);
        assert_eq!(panel.button(AREA, x, y, true), PanelResponse::Nothing);
        assert_eq!(panel.button(AREA, x, y, false), PanelResponse::Pressed(0));
        panel.button(AREA, x, y, true);
        assert_eq!(panel.button(AREA, 0, 0, false), PanelResponse::Nothing);
    }

    #[test]
    fn a_progress_panel_has_cancel_alone_and_enter_does_nothing() {
        let mut panel = Panel::progress("Copying big.bin...", 250);
        assert!(panel.is_progress());
        assert_eq!(panel.rect(AREA).height, 112);
        assert_eq!(panel.key(key::ENTER, false, true), PanelResponse::Nothing);
        assert_eq!(panel.key(key::ESC, false, true), PanelResponse::Pressed(0));
        panel.set_progress(2000);
        let mut pixels = vec![0u32; 720 * 480];
        let mut canvas = Canvas::new(&mut pixels, 720, 480, 1).unwrap();
        panel.paint(&mut canvas, AREA, true);
        let bar = panel.rect(AREA);
        let row = (bar.y + 44) as usize * 720;
        assert_eq!(pixels[row + (bar.right() - 20) as usize], colour::ACCENT);
    }
}
