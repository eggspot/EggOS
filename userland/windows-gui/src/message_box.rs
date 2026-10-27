//! `MessageBoxW`'s box, as `docs/ux/windows-gui-v0.3.md` section 1 designs it: its layout from
//! the text and the type, its painting in each state, and how keys and the pointer drive its
//! buttons.

use crate::colour::{quarter, token};
use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_HEIGHT, CELL_WIDTH, Cell};

/// The most lines a box shows; longer text shows one line fewer and `...`.
pub const MAX_LINES: usize = 24;
/// The most buttons a box has.
pub const MAX_BUTTONS: usize = 4;

const PADDING: i32 = 16;
const ICON_SIZE: i32 = 32;
const ICON_GLYPH_X: i32 = 8;
const LINE_PITCH: i32 = 20;
const MINIMUM_WIDTH: i32 = 256;
const WIDEST_WRAP: i32 = 480;
const WRAP_MARGIN: i32 = 128;
const BUTTON_HEIGHT: i32 = 28;
const BUTTON_MINIMUM_WIDTH: i32 = 96;
const BUTTON_LABEL_PADDING: i32 = 16;
const BUTTON_GAP: i32 = 8;
const BUTTON_LABEL_TOP: i32 = 6;
const BUTTON_BORDER_WIDTH: i32 = 1;
const FOCUS_RING_INSET: i32 = 2;
const FOCUS_RING_WIDTH: i32 = 2;
const SINGLE_LINE_BESIDE_ICON: i32 = 8;
const INSET_R4: [i32; 4] = [2, 1, 0, 0];
const INSET_R6: [i32; 6] = [4, 2, 1, 1, 0, 0];
const INNER_CORNER: [i32; 1] = [1];

/// The results `MessageBoxW` returns, Win32's `IDxxx`.
pub mod result {
    pub const OK: i32 = 1;
    pub const CANCEL: i32 = 2;
    pub const ABORT: i32 = 3;
    pub const RETRY: i32 = 4;
    pub const IGNORE: i32 = 5;
    pub const YES: i32 = 6;
    pub const NO: i32 = 7;
    pub const TRY_AGAIN: i32 = 10;
    pub const CONTINUE: i32 = 11;
}

/// The parts of `MessageBoxW`'s type this box reads.
pub mod style {
    pub const BUTTONS: u32 = 0xf;
    pub const OK_CANCEL: u32 = 1;
    pub const ABORT_RETRY_IGNORE: u32 = 2;
    pub const YES_NO_CANCEL: u32 = 3;
    pub const YES_NO: u32 = 4;
    pub const RETRY_CANCEL: u32 = 5;
    pub const CANCEL_TRY_CONTINUE: u32 = 6;
    pub const ICON: u32 = 0xf0;
    pub const ICON_ERROR: u32 = 0x10;
    pub const ICON_QUESTION: u32 = 0x20;
    pub const ICON_WARNING: u32 = 0x30;
    pub const ICON_INFORMATION: u32 = 0x40;
    pub const DEFAULT_BUTTON: u32 = 0xf00;
    pub const HELP: u32 = 0x4000;
}

/// The picture beside the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Error,
    Question,
    Warning,
    Information,
}

impl Icon {
    fn of(kind: u32) -> Option<Icon> {
        match kind & style::ICON {
            style::ICON_ERROR => Some(Icon::Error),
            style::ICON_QUESTION => Some(Icon::Question),
            style::ICON_WARNING => Some(Icon::Warning),
            style::ICON_INFORMATION => Some(Icon::Information),
            _ => None,
        }
    }

    fn tile(self) -> u32 {
        match self {
            Icon::Error => token::BUTTON_CLOSE,
            Icon::Warning => token::BUTTON_MINIMISE,
            Icon::Question | Icon::Information => token::ICON_WINDOWS,
        }
    }

    fn glyph(self) -> u8 {
        match self {
            Icon::Error => b'X',
            Icon::Question => b'?',
            Icon::Warning => b'!',
            Icon::Information => b'i',
        }
    }
}

/// One button: its label, the index of its underlined mnemonic letter, the result it returns
/// (none for `Help`, which leaves the box open) and its rectangle in the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub mnemonic: Option<usize>,
    pub result: Option<i32>,
    pub rect: Rect,
}

const fn button(label: &'static str, mnemonic: Option<usize>, result: Option<i32>) -> Button {
    Button {
        label,
        mnemonic,
        result,
        rect: Rect::new(0, 0, 0, 0),
    }
}

const OK: Button = button("OK", None, Some(result::OK));
const CANCEL: Button = button("Cancel", None, Some(result::CANCEL));
const ABORT: Button = button("Abort", Some(0), Some(result::ABORT));
const RETRY: Button = button("Retry", Some(0), Some(result::RETRY));
const IGNORE: Button = button("Ignore", Some(0), Some(result::IGNORE));
const YES: Button = button("Yes", Some(0), Some(result::YES));
const NO: Button = button("No", Some(0), Some(result::NO));
const TRY_AGAIN: Button = button("Try Again", Some(0), Some(result::TRY_AGAIN));
const CONTINUE: Button = button("Continue", Some(0), Some(result::CONTINUE));
const HELP: Button = button("Help", Some(0), None);

fn buttons_of(kind: u32) -> (&'static [Button], Option<usize>) {
    match kind & style::BUTTONS {
        style::OK_CANCEL => (&[OK, CANCEL], Some(1)),
        style::ABORT_RETRY_IGNORE => (&[ABORT, RETRY, IGNORE], None),
        style::YES_NO_CANCEL => (&[YES, NO, CANCEL], Some(2)),
        style::YES_NO => (&[YES, NO], None),
        style::RETRY_CANCEL => (&[RETRY, CANCEL], Some(1)),
        style::CANCEL_TRY_CONTINUE => (&[CANCEL, TRY_AGAIN, CONTINUE], Some(0)),
        _ => (&[OK], Some(0)),
    }
}

/// Where everything of one box goes, for its text (as [`text::decode`]d cells) and type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub icon: Option<Icon>,
    pub text_x: i32,
    pub text_y: i32,
    lines: [(usize, usize); MAX_LINES],
    line_count: usize,
    cut: bool,
    buttons: [Button; MAX_BUTTONS],
    button_count: usize,
    escape: Option<usize>,
    initial_focus: usize,
}

impl Layout {
    /// The layout of a box of type `kind` showing `cells` on a work area `work_width` wide.
    pub fn new(cells: &[Cell], kind: u32, work_width: i32) -> Layout {
        let icon = Icon::of(kind);
        let text_x = match icon {
            Some(_) => PADDING + ICON_SIZE + PADDING,
            None => PADDING,
        };
        let wrap = WIDEST_WRAP.min(work_width - WRAP_MARGIN).max(CELL_WIDTH);
        let mut lines = [(0, 0); MAX_LINES];
        let mut line_count = 0;
        let mut cut = false;
        for line in text::lines(cells, Some((wrap / CELL_WIDTH) as usize)) {
            if line_count == MAX_LINES {
                cut = true;
                line_count = MAX_LINES - 1;
                break;
            }
            lines[line_count] = line;
            line_count += 1;
        }
        let shown_lines = line_count + usize::from(cut);
        let widest = lines[..line_count]
            .iter()
            .map(|(start, end)| end - start)
            .chain(cut.then_some(3))
            .max()
            .unwrap_or(0) as i32;
        let text_height = LINE_PITCH * shown_lines as i32 - (LINE_PITCH - CELL_HEIGHT);
        let body_height = match icon {
            Some(_) => text_height.max(ICON_SIZE),
            None => text_height,
        };
        let text_y = match (icon, shown_lines) {
            (Some(_), 1) => PADDING + SINGLE_LINE_BESIDE_ICON,
            _ => PADDING,
        };
        let (kinds, escape) = buttons_of(kind);
        let mut buttons = [OK; MAX_BUTTONS];
        let mut button_count = 0;
        for button in kinds
            .iter()
            .chain((kind & style::HELP != 0).then_some(&HELP))
        {
            buttons[button_count] = *button;
            button_count += 1;
        }
        let width_of = |button: &Button| {
            BUTTON_MINIMUM_WIDTH
                .max(button.label.len() as i32 * CELL_WIDTH + 2 * BUTTON_LABEL_PADDING)
        };
        let buttons_width = buttons[..button_count].iter().map(width_of).sum::<i32>()
            + BUTTON_GAP * (button_count as i32 - 1);
        let width = MINIMUM_WIDTH
            .max(text_x + widest * CELL_WIDTH + PADDING)
            .max(buttons_width + 2 * PADDING);
        let row = PADDING + body_height + PADDING;
        let mut x = width - PADDING - buttons_width;
        for button in &mut buttons[..button_count] {
            let width = width_of(button);
            button.rect = Rect::sized(x, row, width, BUTTON_HEIGHT);
            x += width + BUTTON_GAP;
        }
        let default_button = ((kind & style::DEFAULT_BUTTON) >> 8) as usize;
        Layout {
            width,
            height: row + BUTTON_HEIGHT + PADDING,
            icon,
            text_x,
            text_y,
            lines,
            line_count,
            cut,
            buttons,
            button_count,
            escape,
            initial_focus: if default_button < button_count {
                default_button
            } else {
                0
            },
        }
    }

    /// The buttons, left to right.
    pub fn buttons(&self) -> &[Button] {
        &self.buttons[..self.button_count]
    }

    /// The lines shown, as ranges of the cells; `...` follows them when the text was cut.
    pub fn lines(&self) -> &[(usize, usize)] {
        &self.lines[..self.line_count]
    }

    /// Whether the text was too long, so `...` ends it.
    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// The button Esc and the close request choose, if the type has one.
    pub fn escape(&self) -> Option<usize> {
        self.escape
    }

    /// The button focused first.
    pub fn initial_focus(&self) -> usize {
        self.initial_focus
    }

    /// The button under (`x`, `y`), if any.
    pub fn button_at(&self, x: i32, y: i32) -> Option<usize> {
        self.buttons()
            .iter()
            .position(|button| button.rect.contains(x, y))
    }

    /// The button whose mnemonic is `character`, compared ignoring ASCII case.
    pub fn mnemonic_button(&self, character: u32) -> Option<usize> {
        let character = u8::try_from(character).ok()?.to_ascii_lowercase();
        self.buttons().iter().position(|button| {
            button
                .mnemonic
                .and_then(|index| button.label.as_bytes().get(index))
                .is_some_and(|letter| letter.to_ascii_lowercase() == character)
        })
    }
}

/// The buttons' state: the focused one (also the default), and the one under the pointer and
/// the one held down, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Look {
    pub focused: usize,
    pub hovered: Option<usize>,
    pub pressed: Option<usize>,
}

/// Paints the box laid out as `layout`, showing `cells`, in the state `look`, over the whole
/// surface.
pub fn paint(surface: &mut Surface<'_>, layout: &Layout, cells: &[Cell], look: Look) {
    surface.fill(Rect::sized(0, 0, layout.width, layout.height), token::MENU);
    if let Some(icon) = layout.icon {
        let tile = Rect::sized(PADDING, PADDING, ICON_SIZE, ICON_SIZE);
        surface.rounded(tile, &INSET_R6, icon.tile());
        text::glyph_2x(
            surface,
            tile.left + ICON_GLYPH_X,
            tile.top,
            icon.glyph(),
            token::ICON_DARK,
            Some(icon.tile()),
        );
    }
    let mut y = layout.text_y;
    for &(start, end) in layout.lines() {
        text::draw_cells(
            surface,
            layout.text_x,
            y,
            &cells[start..end],
            token::TEXT,
            None,
        );
        y += LINE_PITCH;
    }
    if layout.is_cut() {
        let ellipsis = [Cell::Glyph {
            id: b'.',
            underline: false,
        }; 3];
        text::draw_cells(surface, layout.text_x, y, &ellipsis, token::TEXT_DIM, None);
    }
    for (index, button) in layout.buttons().iter().enumerate() {
        paint_button(surface, button, index, look);
    }
}

fn paint_button(surface: &mut Surface<'_>, button: &Button, index: usize, look: Look) {
    let focused = look.focused == index;
    let pressed = look.pressed == Some(index);
    let hovered = look.hovered == Some(index);
    let plain_fill = match focused {
        true => token::BUTTON_PRIMARY,
        false => token::BUTTON,
    };
    let fill = match pressed {
        true => quarter(plain_fill),
        false => plain_fill,
    };
    let border = match () {
        () if hovered || pressed => token::TEXT_DIM,
        () if focused => token::BUTTON_PRIMARY,
        () => token::BUTTON_BORDER,
    };
    let label_colour = match focused {
        true => token::ICON_DARK,
        false => token::TEXT,
    };
    let rect = button.rect;
    surface.rounded(rect, &INSET_R4, border);
    surface.rounded(rect.inset(BUTTON_BORDER_WIDTH), &INNER_CORNER, fill);
    if focused {
        let ring = rect.inset(FOCUS_RING_INSET);
        surface.rounded(ring, &INSET_R4, token::ICON_DARK);
        surface.rounded(ring.inset(FOCUS_RING_WIDTH), &INNER_CORNER, fill);
    }
    let label_width = button.label.len() as i32 * CELL_WIDTH;
    let x = rect.left + (rect.width() - label_width) / 2;
    for (offset, letter) in button.label.bytes().enumerate() {
        text::glyph(
            surface,
            x + offset as i32 * CELL_WIDTH,
            rect.top + BUTTON_LABEL_TOP,
            letter,
            button.mnemonic == Some(offset),
            label_colour,
            None,
        );
    }
}

/// The Linux key codes the box reads from the server's `Key` events.
pub mod key {
    pub const ESCAPE: u16 = 1;
    pub const TAB: u16 = 15;
    pub const ENTER: u16 = 28;
    pub const SPACE: u16 = 57;
    pub const KEYPAD_ENTER: u16 = 96;
    pub const UP: u16 = 103;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const DOWN: u16 = 108;
}

const SHIFT: u32 = 1;

/// What one input did to the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Repaint,
    Activated(usize),
}

/// A box's buttons as keys and the pointer drive them (section 1.5 of the design).
#[derive(Debug, Clone, Copy)]
pub struct Controller {
    layout: Layout,
    look: Look,
    pointer_press: Option<usize>,
    space_held: bool,
}

impl Controller {
    pub fn new(layout: Layout) -> Controller {
        Controller {
            look: Look {
                focused: layout.initial_focus(),
                ..Look::default()
            },
            layout,
            pointer_press: None,
            space_held: false,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn look(&self) -> Look {
        self.look
    }

    fn changed(&mut self, look: Look) -> Outcome {
        match look == self.look {
            true => Outcome::Unchanged,
            false => {
                self.look = look;
                Outcome::Repaint
            }
        }
    }

    fn step_focus(&mut self, forward: bool) -> Outcome {
        let count = self.layout.button_count;
        let focused = match forward {
            true => (self.look.focused + 1) % count,
            false => (self.look.focused + count - 1) % count,
        };
        self.space_held = false;
        self.changed(Look {
            focused,
            pressed: None,
            ..self.look
        })
    }

    /// A `Key` event: Linux key `code`, pressed or released, the server's modifier bits and
    /// the character it gives.
    pub fn key(&mut self, code: u16, pressed: bool, modifiers: u32, character: u32) -> Outcome {
        if !pressed {
            let held = self.space_held;
            if code == key::SPACE && held {
                self.space_held = false;
                self.look.pressed = None;
                return Outcome::Activated(self.look.focused);
            }
            return Outcome::Unchanged;
        }
        match code {
            key::ENTER | key::KEYPAD_ENTER => Outcome::Activated(self.look.focused),
            key::SPACE => {
                self.space_held = true;
                self.changed(Look {
                    pressed: Some(self.look.focused),
                    ..self.look
                })
            }
            key::TAB => self.step_focus(modifiers & SHIFT == 0),
            key::RIGHT | key::DOWN => self.step_focus(true),
            key::LEFT | key::UP => self.step_focus(false),
            key::ESCAPE => self.close_request(),
            _ => self
                .layout
                .mnemonic_button(character)
                .map_or(Outcome::Unchanged, Outcome::Activated),
        }
    }

    /// The close control, Alt+F4 or the profile's close key: what Esc does.
    pub fn close_request(&self) -> Outcome {
        self.layout
            .escape()
            .map_or(Outcome::Unchanged, Outcome::Activated)
    }

    /// The pointer moved to (`x`, `y`) in the content.
    pub fn pointer_moved(&mut self, x: i32, y: i32) -> Outcome {
        let under = self.layout.button_at(x, y);
        let pressed = self.pointer_press.filter(|&held| under == Some(held));
        self.changed(Look {
            hovered: under,
            pressed: if self.pointer_press.is_some() {
                pressed
            } else {
                self.look.pressed
            },
            ..self.look
        })
    }

    /// The pointer left the content.
    pub fn pointer_left(&mut self) -> Outcome {
        self.changed(Look {
            hovered: None,
            ..self.look
        })
    }

    /// The left button went down or up at (`x`, `y`).
    pub fn left_button(&mut self, x: i32, y: i32, down: bool) -> Outcome {
        let under = self.layout.button_at(x, y);
        if down {
            let Some(index) = under else {
                return Outcome::Unchanged;
            };
            self.pointer_press = Some(index);
            self.space_held = false;
            return self.changed(Look {
                focused: index,
                hovered: Some(index),
                pressed: Some(index),
            });
        }
        let held = self.pointer_press.take();
        self.look.pressed = None;
        match held {
            Some(index) if under == Some(index) => Outcome::Activated(index),
            Some(_) => Outcome::Repaint,
            None => Outcome::Unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Reading, decode};

    const WORK_WIDTH: i32 = 1280;

    fn cells(text: &str) -> Vec<Cell> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut cells = vec![Cell::Space; 4096];
        let filled = decode(&units, Reading::default(), &mut cells);
        cells.truncate(filled);
        cells
    }

    fn worked_example() -> (Vec<Cell>, Layout) {
        let cells = cells("You pressed the space bar.");
        let layout = Layout::new(
            &cells,
            style::OK_CANCEL | style::ICON_INFORMATION,
            WORK_WIDTH,
        );
        (cells, layout)
    }

    #[test]
    fn the_worked_example_lays_out_as_designed() {
        let (_, layout) = worked_example();
        assert_eq!((layout.width, layout.height), (288, 108));
        assert_eq!((layout.text_x, layout.text_y), (64, 24));
        assert_eq!(layout.icon, Some(Icon::Information));
        let rects: Vec<Rect> = layout.buttons().iter().map(|b| b.rect).collect();
        assert_eq!(
            rects,
            [Rect::sized(72, 64, 96, 28), Rect::sized(176, 64, 96, 28)]
        );
        assert_eq!(layout.initial_focus(), 0);
        assert_eq!(layout.escape(), Some(1));
    }

    fn painted(cells: &[Cell], layout: &Layout, look: Look) -> Vec<u32> {
        let mut pixels = vec![0; (layout.width * layout.height) as usize];
        let mut surface =
            Surface::new(&mut pixels, layout.width as u32, layout.height as u32).unwrap();
        paint(&mut surface, layout, cells, look);
        pixels
    }

    #[test]
    fn the_worked_example_paints_as_designed() {
        let (cells, layout) = worked_example();
        let look = Look::default();
        let pixels = painted(&cells, &layout, look);
        let at = |x: i32, y: i32| pixels[(y * layout.width + x) as usize];
        assert_eq!(at(0, 0), token::MENU);
        assert_eq!(at(16, 16), token::MENU);
        assert_eq!(at(20, 16), token::ICON_WINDOWS);
        assert_eq!(at(16, 20), token::ICON_WINDOWS);
        assert_eq!(at(30, 20), token::ICON_DARK);
        assert_eq!(at(28, 20), token::ICON_WINDOWS);
        assert_eq!(at(28, 41), token::ICON_DARK);
        assert_eq!(at(33, 41), token::ICON_DARK);
        assert_eq!(at(34, 41), token::ICON_WINDOWS);
        assert_eq!(at(28, 42), token::ICON_WINDOWS);
        assert_eq!(at(287, 107), token::MENU);
        assert_eq!(at(74, 64), token::BUTTON_PRIMARY);
        assert_eq!(at(72, 64), token::MENU);
        assert_eq!(at(74, 66), token::BUTTON_PRIMARY);
        assert_eq!(at(76, 66), token::ICON_DARK);
        assert_eq!(at(75, 67), token::ICON_DARK);
        assert_eq!(at(74, 68), token::ICON_DARK);
        assert_eq!(at(76, 68), token::ICON_DARK);
        assert_eq!(at(77, 69), token::BUTTON_PRIMARY);
        assert_eq!(at(178, 64), token::BUTTON_BORDER);
        assert_eq!(at(180, 68), token::BUTTON);
        let mut expected_text = vec![token::MENU; (208 * 16) as usize];
        let mut surface = Surface::new(&mut expected_text, 208, 16).unwrap();
        text::draw_cells(&mut surface, 0, 0, &cells, token::TEXT, None);
        for y in 0..16 {
            for x in 0..208 {
                assert_eq!(at(64 + x, 24 + y), expected_text[(y * 208 + x) as usize]);
            }
        }
        let mut label = vec![token::BUTTON; (48 * 16) as usize];
        let mut surface = Surface::new(&mut label, 48, 16).unwrap();
        text::draw_cells(
            &mut surface,
            0,
            0,
            &self::cells("Cancel"),
            token::TEXT,
            None,
        );
        for y in 0..16 {
            for x in 0..48 {
                assert_eq!(at(200 + x, 70 + y), label[(y * 48 + x) as usize]);
            }
        }
        let hovered = painted(
            &cells,
            &layout,
            Look {
                hovered: Some(1),
                ..look
            },
        );
        assert_eq!(hovered[(64 * 288 + 178) as usize], token::TEXT_DIM);
        let pressed = painted(
            &cells,
            &layout,
            Look {
                pressed: Some(1),
                ..look
            },
        );
        assert_eq!(pressed[(68 * 288 + 180) as usize], 0x0020_2730);
    }

    #[test]
    fn every_type_has_its_buttons_widths_and_escape() {
        let text = cells("x");
        let labels = |kind: u32| -> Vec<&str> {
            Layout::new(&text, kind, WORK_WIDTH)
                .buttons()
                .iter()
                .map(|b| b.label)
                .collect()
        };
        assert_eq!(labels(0), ["OK"]);
        assert_eq!(labels(9), ["OK"]);
        assert_eq!(labels(2), ["Abort", "Retry", "Ignore"]);
        assert_eq!(labels(3), ["Yes", "No", "Cancel"]);
        assert_eq!(labels(4), ["Yes", "No"]);
        assert_eq!(labels(5), ["Retry", "Cancel"]);
        assert_eq!(
            labels(6 | style::HELP),
            ["Cancel", "Try Again", "Continue", "Help"]
        );
        let layout = Layout::new(&text, 6, WORK_WIDTH);
        assert_eq!(layout.buttons()[1].rect.width(), 104);
        assert_eq!(layout.buttons()[2].rect.right, layout.width - 16);
        for (kind, escape) in [
            (0, Some(0)),
            (1, Some(1)),
            (2, None),
            (3, Some(2)),
            (4, None),
        ] {
            assert_eq!(Layout::new(&text, kind, WORK_WIDTH).escape(), escape);
        }
        assert_eq!(Layout::new(&text, 0, WORK_WIDTH).width, 256);
        assert_eq!(Layout::new(&text, 3 | 0x200, WORK_WIDTH).initial_focus(), 2);
        assert_eq!(Layout::new(&text, 4 | 0x300, WORK_WIDTH).initial_focus(), 0);
    }

    #[test]
    fn long_text_wraps_to_the_wrap_width_and_cuts_at_24_lines() {
        let word = "w".repeat(70);
        let layout = Layout::new(&cells(&word), 0, WORK_WIDTH);
        assert_eq!(layout.lines(), [(0, 60), (60, 70)]);
        assert_eq!(layout.width, 16 + 480 + 16);
        let narrow = Layout::new(&cells(&word), 0, 400);
        assert_eq!(narrow.lines()[0], (0, 34));
        let many = vec!["line"; 30].join("\n");
        let layout = Layout::new(&cells(&many), 0, WORK_WIDTH);
        assert_eq!(layout.lines().len(), 23);
        assert!(layout.is_cut());
        assert_eq!(layout.height, 20 * 24 - 4 + 76);
        let layout = Layout::new(&cells("a\r\nb"), style::ICON_ERROR, WORK_WIDTH);
        assert_eq!(layout.text_y, 16);
        assert_eq!(layout.height, 36 + 76);
    }

    #[test]
    fn keys_move_the_focus_activate_and_honour_escape_and_mnemonics() {
        let (_, layout) = worked_example();
        let mut controller = Controller::new(layout);
        assert_eq!(controller.key(key::TAB, true, 0, 9), Outcome::Repaint);
        assert_eq!(controller.look().focused, 1);
        assert_eq!(controller.key(key::TAB, true, SHIFT, 9), Outcome::Repaint);
        assert_eq!(controller.key(key::LEFT, true, 0, 0), Outcome::Repaint);
        assert_eq!(controller.look().focused, 1);
        assert_eq!(
            controller.key(key::ENTER, true, 0, 13),
            Outcome::Activated(1)
        );
        assert_eq!(
            controller.key(key::ESCAPE, true, 0, 27),
            Outcome::Activated(1)
        );
        assert_eq!(controller.key(key::SPACE, true, 0, 32), Outcome::Repaint);
        assert_eq!(controller.look().pressed, Some(1));
        assert_eq!(
            controller.key(key::SPACE, false, 0, 32),
            Outcome::Activated(1)
        );
        assert_eq!(controller.key(key::SPACE, false, 0, 32), Outcome::Unchanged);

        let text = cells("?");
        let mut yes_no = Controller::new(Layout::new(&text, style::YES_NO, WORK_WIDTH));
        assert_eq!(yes_no.key(key::ESCAPE, true, 0, 27), Outcome::Unchanged);
        assert_eq!(yes_no.close_request(), Outcome::Unchanged);
        assert_eq!(
            yes_no.key(30, true, 0, u32::from(b'N')),
            Outcome::Activated(1)
        );
        assert_eq!(
            yes_no.key(30, true, 4, u32::from(b'y')),
            Outcome::Activated(0)
        );
        assert_eq!(yes_no.key(30, true, 0, u32::from(b'q')), Outcome::Unchanged);
    }

    #[test]
    fn a_click_activates_only_when_released_on_the_pressed_button() {
        let (_, layout) = worked_example();
        let mut controller = Controller::new(layout);
        assert_eq!(controller.pointer_moved(200, 70), Outcome::Repaint);
        assert_eq!(controller.look().hovered, Some(1));
        assert_eq!(controller.left_button(200, 70, true), Outcome::Repaint);
        assert_eq!(controller.look().focused, 1);
        assert_eq!(controller.pointer_moved(10, 10), Outcome::Repaint);
        assert_eq!(controller.look().pressed, None);
        assert_eq!(controller.pointer_moved(200, 70), Outcome::Repaint);
        assert_eq!(controller.look().pressed, Some(1));
        assert_eq!(
            controller.left_button(200, 70, false),
            Outcome::Activated(1)
        );
        assert_eq!(controller.left_button(100, 70, true), Outcome::Repaint);
        assert_eq!(controller.left_button(10, 10, false), Outcome::Repaint);
        assert_eq!(controller.left_button(10, 10, true), Outcome::Unchanged);
        assert_eq!(controller.pointer_left(), Outcome::Repaint);
    }
}
