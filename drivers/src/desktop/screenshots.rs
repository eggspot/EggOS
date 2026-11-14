//! Screenshots in the window server (D590, docs/ux/desktops-overview-screenshots-v0.3.md
//! section 6): the keys' targets, the region picker (a server mode that grabs the pointer and
//! the keys, draws the dragged rectangle and its size, and a hint), and the command the server
//! starts, `/bin/screenshot --notify` with the target.

use super::catalogue::Command;
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};

/// The program every screenshot is taken by.
pub const SCREENSHOT_PROGRAM: &str = "/bin/screenshot";
/// The hint in region mode.
pub const REGION_HINT: &str = "Drag to capture a region. Space: a window. Esc: cancel";
/// The hint in window mode.
pub const WINDOW_HINT: &str = "Click a window to capture it. Space: a region. Esc: cancel";
/// The notice when the program is missing.
pub const NOT_INSTALLED: &str = "Screenshots are not installed.";
/// The smallest region a release captures; a smaller one is a slip.
pub const SMALLEST_REGION: u32 = 8;

/// What a screenshot key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotTarget {
    Screen,
    FocusedWindow,
    Region,
}

/// What to capture, as the server asks the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotRequest {
    Screen,
    Window(u32),
    Region(Rect),
}

/// The picker's two modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    Region,
    Window,
}

/// The region picker's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picker {
    pub mode: PickerMode,
    /// Where the drag started, while the button is held.
    pub anchor: Option<(u32, u32)>,
    /// Where the pointer is.
    pub pointer: (u32, u32),
}

/// What the picker made of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerOutcome {
    /// The mode goes on.
    Going,
    /// The mode ended with nothing captured.
    Cancelled,
    /// The mode ended: capture this.
    Capture(ScreenshotRequest),
}

/// The rectangle between two corners, inclusive of both.
pub fn spanned((ax, ay): (u32, u32), (bx, by): (u32, u32)) -> Rect {
    let (left, right) = (ax.min(bx), ax.max(bx));
    let (top, bottom) = (ay.min(by), ay.max(by));
    Rect::new(left, top, right - left + 1, bottom - top + 1)
}

impl Picker {
    /// A picker in region mode with the pointer at `pointer`.
    pub const fn new(pointer: (u32, u32)) -> Self {
        Self {
            mode: PickerMode::Region,
            anchor: None,
            pointer,
        }
    }

    /// The hint it shows: none while a drag is in progress.
    pub fn hint(&self) -> Option<&'static str> {
        match (self.mode, self.anchor) {
            (_, Some(_)) => None,
            (PickerMode::Region, None) => Some(REGION_HINT),
            (PickerMode::Window, None) => Some(WINDOW_HINT),
        }
    }

    /// The rectangle being dragged, if one is.
    pub fn dragged(&self) -> Option<Rect> {
        self.anchor.map(|anchor| spanned(anchor, self.pointer))
    }

    /// The pointer moved to `at`.
    pub fn moved(&mut self, at: (u32, u32)) {
        self.pointer = at;
    }

    /// The main button went down (`pressed`) or up at the pointer; `window_at` is the window
    /// under it, for window mode.
    pub fn button(&mut self, pressed: bool, window_at: Option<u32>) -> PickerOutcome {
        match (self.mode, pressed) {
            (PickerMode::Region, true) => {
                self.anchor = Some(self.pointer);
                PickerOutcome::Going
            }
            (PickerMode::Region, false) => {
                let Some(rect) = self.dragged() else {
                    return PickerOutcome::Going;
                };
                self.anchor = None;
                match rect.width >= SMALLEST_REGION && rect.height >= SMALLEST_REGION {
                    true => PickerOutcome::Capture(ScreenshotRequest::Region(rect)),
                    false => PickerOutcome::Going,
                }
            }
            (PickerMode::Window, true) => match window_at {
                Some(window) => PickerOutcome::Capture(ScreenshotRequest::Window(window)),
                None => PickerOutcome::Going,
            },
            (PickerMode::Window, false) => PickerOutcome::Going,
        }
    }

    /// A key: Space switches the mode, Esc cancels.
    pub fn key(&mut self, code: u16) -> PickerOutcome {
        match code {
            crate::input::key::SPACE => {
                self.anchor = None;
                self.mode = match self.mode {
                    PickerMode::Region => PickerMode::Window,
                    PickerMode::Window => PickerMode::Region,
                };
                PickerOutcome::Going
            }
            crate::input::key::ESC => PickerOutcome::Cancelled,
            _ => PickerOutcome::Going,
        }
    }
}

/// The command that takes `request`: `/bin/screenshot --notify` and its target.
pub fn command(request: ScreenshotRequest) -> Option<Command> {
    let mut number = [0u8; 48];
    match request {
        ScreenshotRequest::Screen => Command::program(SCREENSHOT_PROGRAM, &[b"--notify"]),
        ScreenshotRequest::Window(window) => {
            let length = decimal(window, &mut number);
            Command::program(SCREENSHOT_PROGRAM, &[b"--notify", b"--window", &number[..length]])
        }
        ScreenshotRequest::Region(rect) => {
            let mut length = 0;
            for (index, value) in [rect.x, rect.y, rect.width, rect.height].into_iter().enumerate() {
                if index > 0 {
                    number[length] = b',';
                    length += 1;
                }
                let mut digits = [0u8; 10];
                let count = decimal(value, &mut digits);
                number[length..length + count].copy_from_slice(&digits[..count]);
                length += count;
            }
            Command::program(SCREENSHOT_PROGRAM, &[b"--notify", b"--region", &number[..length]])
        }
    }
}

fn decimal(value: u32, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut rest = value;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let length = digits.len() - at;
    out[..length].copy_from_slice(&digits[at..]);
    length
}

const HINT_HEIGHT: u32 = 40;
const HINT_TOP: u32 = 16;
const HINT_TEXT_Y: u32 = 12;
const HINT_PADDING: u32 = 32;
const SIZE_PILL_HEIGHT: u32 = 20;
const SIZE_PILL_PADDING: u32 = 8;
const WINDOW_OUTLINE: u32 = 2;

fn outline(canvas: &mut super::paint::Canvas<'_>, rect: Rect, thickness: u32, colour: u32) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let thickness = thickness.min(rect.width).min(rect.height);
    canvas.fill(Rect::new(rect.x, rect.y, rect.width, thickness), colour);
    canvas.fill(
        Rect::new(rect.x, rect.y + rect.height - thickness, rect.width, thickness),
        colour,
    );
    canvas.fill(Rect::new(rect.x, rect.y, thickness, rect.height), colour);
    canvas.fill(
        Rect::new(rect.x + rect.width - thickness, rect.y, thickness, rect.height),
        colour,
    );
}

fn grown(rect: Rect, by: u32) -> Rect {
    Rect::new(
        rect.x.saturating_sub(by),
        rect.y.saturating_sub(by),
        rect.width + 2 * by,
        rect.height + 2 * by,
    )
}

impl super::Desktop {
    /// A screenshot key (D590): the screen and the focused window are asked of the program at
    /// once; a region opens the picker.
    pub(super) fn screenshot(&mut self, target: ScreenshotTarget, damage: &mut Damage) -> super::Action {
        match target {
            ScreenshotTarget::Screen => super::Action::Screenshot(ScreenshotRequest::Screen),
            ScreenshotTarget::FocusedWindow => match self.focused_window() {
                Some(window) => super::Action::Screenshot(ScreenshotRequest::Window(u32::from(window))),
                None => super::Action::Screenshot(ScreenshotRequest::Screen),
            },
            ScreenshotTarget::Region => {
                self.open_picker(damage);
                super::Action::None
            }
        }
    }

    /// Opens the region picker (`screenshot --pick` and the region keys): it grabs the pointer
    /// and the keys until it ends.
    pub fn open_picker(&mut self, damage: &mut Damage) {
        let picker = Picker::new(self.pointer);
        self.picker = Some(picker);
        damage.add(self.picker_area(&picker));
        self.update_pointer(damage);
    }

    /// Whether the region picker is open.
    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    fn hint_rect(&self, text: &str) -> Rect {
        let work = self.layout.work_area;
        let width = (text.chars().count() as u32 * GLYPH_WIDTH as u32 + HINT_PADDING).min(work.width);
        Rect::new(work.x + (work.width - width) / 2, work.y + HINT_TOP, width, HINT_HEIGHT)
    }

    fn window_under(&self, (x, y): (u32, u32)) -> Option<(usize, u32)> {
        let (slot, _) = self.hit(x, y).window?;
        Some((slot, u32::from(self.window_at(slot).id)))
    }

    /// What the picker draws now, with room for its outlines, size pill and shadows.
    pub(super) fn picker_area(&self, picker: &Picker) -> Rect {
        let mut area = Rect::default();
        if let Some(text) = picker.hint() {
            area = area.union(super::popups::with_shadow(self.hint_rect(text)));
        }
        if let Some(rect) = picker.dragged() {
            area = area.union(grown(rect, SIZE_PILL_HEIGHT + 2 * SIZE_PILL_PADDING + 80));
        }
        if picker.mode == PickerMode::Window
            && let Some((slot, _)) = self.window_under(picker.pointer)
        {
            area = area.union(grown(self.window_at(slot).outside(), WINDOW_OUTLINE));
        }
        area.clipped(self.width, self.height)
    }

    fn end_picker(&mut self, damage: &mut Damage) {
        if let Some(picker) = self.picker.take() {
            damage.add(self.picker_area(&picker));
        }
        self.update_pointer(damage);
    }

    /// The picker's part in a button event: `None` when it is not open.
    pub(super) fn picker_button(&mut self, code: u16, pressed: bool, damage: &mut Damage) -> Option<super::Action> {
        let mut picker = self.picker?;
        if code != crate::input::button::LEFT {
            return Some(super::Action::None);
        }
        damage.add(self.picker_area(&picker));
        let under = self.window_under(picker.pointer).map(|(_, id)| id);
        let outcome = picker.button(pressed, under);
        self.picker = Some(picker);
        damage.add(self.picker_area(&picker));
        Some(self.picker_outcome(outcome, damage))
    }

    /// The picker's part in a key event: it takes every key while open.
    pub(super) fn picker_key(&mut self, code: u16, pressed: bool, damage: &mut Damage) -> Option<super::Action> {
        let mut picker = self.picker?;
        if !pressed {
            return Some(super::Action::None);
        }
        damage.add(self.picker_area(&picker));
        let outcome = picker.key(code);
        self.picker = Some(picker);
        damage.add(self.picker_area(&picker));
        Some(self.picker_outcome(outcome, damage))
    }

    fn picker_outcome(&mut self, outcome: PickerOutcome, damage: &mut Damage) -> super::Action {
        match outcome {
            PickerOutcome::Going => super::Action::None,
            PickerOutcome::Cancelled => {
                self.end_picker(damage);
                super::Action::None
            }
            PickerOutcome::Capture(request) => {
                self.end_picker(damage);
                super::Action::Screenshot(request)
            }
        }
    }

    /// Draws the picker over everything: the hint, the dragged rectangle and its size, or the
    /// window under the pointer.
    pub(super) fn draw_picker(&self, canvas: &mut super::paint::Canvas<'_>) {
        let Some(picker) = self.picker else {
            return;
        };
        if let Some(text) = picker.hint() {
            let rect = self.hint_rect(text);
            Self::draw_popup_surface(canvas, rect, super::colour::MENU);
            canvas.text(rect.x + HINT_PADDING / 2, rect.y + HINT_TEXT_Y, text, super::colour::TEXT);
        }
        if let Some(rect) = picker.dragged() {
            outline(canvas, rect, 1, super::colour::ACCENT);
            if rect.width > 2 && rect.height > 2 {
                outline(
                    canvas,
                    Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2),
                    1,
                    super::colour::ICON_DARK,
                );
            }
            let mut size = [0u8; 24];
            let length = size_text(rect, &mut size);
            let text = core::str::from_utf8(&size[..length]).unwrap_or("");
            let width = length as u32 * GLYPH_WIDTH as u32 + 2 * SIZE_PILL_PADDING;
            let inside = rect.height >= 36 && rect.width >= width + 16;
            let pill = match inside {
                true => Rect::new(
                    rect.x + rect.width - width - 8,
                    rect.y + rect.height - SIZE_PILL_HEIGHT - 8,
                    width,
                    SIZE_PILL_HEIGHT,
                ),
                false => Rect::new(rect.x, rect.y + rect.height + 4, width, SIZE_PILL_HEIGHT),
            };
            canvas.rounded(pill, &super::paint::INSET_R4, true, true, super::colour::BAR);
            canvas.text(
                pill.x + SIZE_PILL_PADDING,
                pill.y + (SIZE_PILL_HEIGHT - GLYPH_HEIGHT as u32) / 2,
                text,
                super::colour::TEXT,
            );
        }
        if picker.mode == PickerMode::Window
            && let Some((slot, _)) = self.window_under(picker.pointer)
        {
            outline(
                canvas,
                grown(self.window_at(slot).outside(), WINDOW_OUTLINE),
                WINDOW_OUTLINE,
                super::colour::ACCENT,
            );
        }
    }

    /// The command that takes `request` (D590).
    pub fn screenshot_command(&self, request: ScreenshotRequest) -> Option<Command> {
        command(request)
    }
}

fn size_text(rect: Rect, out: &mut [u8; 24]) -> usize {
    let mut length = decimal(rect.width, &mut out[..]);
    out[length..length + 3].copy_from_slice(b" x ");
    length += 3;
    length + decimal(rect.height, &mut out[length..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_captures_its_rectangle_and_a_slip_does_not() {
        let mut picker = Picker::new((10, 10));
        assert_eq!(picker.hint(), Some(REGION_HINT));
        picker.button(true, None);
        picker.moved((4, 3));
        assert_eq!(picker.hint(), None);
        assert_eq!(picker.button(false, None), PickerOutcome::Going);
        picker.button(true, None);
        picker.moved((109, 59));
        assert_eq!(
            picker.button(false, None),
            PickerOutcome::Capture(ScreenshotRequest::Region(Rect::new(4, 3, 106, 57)))
        );
    }

    #[test]
    fn space_picks_a_window_and_esc_cancels() {
        let mut picker = Picker::new((0, 0));
        picker.key(crate::input::key::SPACE);
        assert_eq!(picker.hint(), Some(WINDOW_HINT));
        assert_eq!(picker.button(true, None), PickerOutcome::Going);
        assert_eq!(
            picker.button(true, Some(129)),
            PickerOutcome::Capture(ScreenshotRequest::Window(129))
        );
        assert_eq!(picker.key(crate::input::key::ESC), PickerOutcome::Cancelled);
    }

    #[test]
    fn the_command_names_the_target() {
        let region = command(ScreenshotRequest::Region(Rect::new(1, 2, 30, 40))).unwrap();
        assert!(region.as_bytes().windows(9).any(|window| window == b"1,2,30,40"));
        assert!(command(ScreenshotRequest::Window(7)).is_some());
    }
}
