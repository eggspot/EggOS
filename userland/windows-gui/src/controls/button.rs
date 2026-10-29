//! `BUTTON`: push buttons and check boxes (section 2 of the design), drawn from a [`ButtonLook`],
//! and the [`Button`] model the pointer and Space drive.

use super::{
    DARK_RING_ON_PRIMARY, MAX_LABEL_CELLS, Palette, cell_top, dotted_outline, draw_fitted, filled,
    framed, label_cells, outline, ring,
};
use crate::surface::{Rect, Surface};
use crate::text::{CELL_WIDTH, Cell};

const BOX_SIZE: i32 = 14;
const BOX_LEFT: i32 = 2;
const CHECK_LABEL_LEFT: i32 = 22;
const CHECK_LABEL_ROOM_LESS: i32 = 24;
const PUSH_LABEL_ROOM_LESS: i32 = 8;
const LIGHT_FOCUS_INSET: i32 = 3;
const DARK_FOCUS_INSET: i32 = 2;
const TICK: [(i32, i32); 8] = [
    (3, 7),
    (4, 8),
    (5, 9),
    (6, 8),
    (7, 7),
    (8, 6),
    (9, 5),
    (10, 4),
];
const TICK_SQUARE: i32 = 2;

/// `BUTTON`'s styles this layer draws (the low four bits of the window style).
pub mod style {
    pub const TYPE: u32 = 0xf;
    pub const PUSH_BUTTON: u32 = 0x0;
    pub const DEFAULT_PUSH_BUTTON: u32 = 0x1;
    pub const CHECK_BOX: u32 = 0x2;
    pub const AUTO_CHECK_BOX: u32 = 0x3;
}

/// What a button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Push,
    DefaultPush,
    CheckBox,
    AutoCheckBox,
}

impl ButtonKind {
    /// The kind a window style asks for; the styles this item leaves out are push buttons.
    pub fn of(window_style: u32) -> ButtonKind {
        match window_style & style::TYPE {
            style::DEFAULT_PUSH_BUTTON => ButtonKind::DefaultPush,
            style::CHECK_BOX => ButtonKind::CheckBox,
            style::AUTO_CHECK_BOX => ButtonKind::AutoCheckBox,
            _ => ButtonKind::Push,
        }
    }

    pub fn is_push(self) -> bool {
        matches!(self, ButtonKind::Push | ButtonKind::DefaultPush)
    }
}

/// How a button looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonLook {
    pub kind: ButtonKind,
    pub checked: bool,
    pub pressed: bool,
    pub focused: bool,
    pub default: bool,
    pub enabled: bool,
}

/// Paints a button over the whole of `rect` with the label `units` (`&` underlines).
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    units: &[u16],
    look: ButtonLook,
    palette: &Palette,
) {
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = label_cells(units, true, &mut cells);
    let cells = &cells[..count];
    match look.kind.is_push() {
        true => paint_push(surface, rect, cells, look, palette),
        false => paint_check_box(surface, rect, cells, look, palette),
    }
}

fn paint_push(
    surface: &mut Surface<'_>,
    rect: Rect,
    cells: &[Cell],
    look: ButtonLook,
    palette: &Palette,
) {
    let (fill, border, text) = match () {
        () if !look.enabled => (
            palette.button_disabled_fill,
            Some(palette.button_disabled_border),
            palette.text_disabled,
        ),
        () if look.default && look.pressed => (
            palette.default_pressed_fill,
            palette
                .default_border
                .map(|_| palette.button_pressed_border),
            palette.default_text,
        ),
        () if look.default => (
            palette.default_fill,
            palette.default_border,
            palette.default_text,
        ),
        () if look.pressed => (
            palette.button_pressed_fill,
            Some(palette.button_pressed_border),
            palette.text,
        ),
        () => (
            palette.button_fill,
            Some(palette.button_border),
            palette.text,
        ),
    };
    match border {
        Some(border) => {
            framed(surface, rect, palette.rounded, border, fill);
            if look.default && look.enabled && !palette.rounded {
                outline(surface, rect.inset(1), border);
            }
        }
        None => filled(surface, rect, palette.rounded, fill),
    }
    if look.focused {
        match palette.rounded {
            true => {
                let colour = match look.default && look.enabled {
                    true => DARK_RING_ON_PRIMARY,
                    false => palette.focus,
                };
                ring(surface, rect.inset(DARK_FOCUS_INSET), colour);
            }
            false => dotted_outline(surface, rect.inset(LIGHT_FOCUS_INSET), palette.focus),
        }
    }
    let room = rect.width() - PUSH_LABEL_ROOM_LESS;
    let (shown, ellipsis) = super::fit_cells(cells.len(), room);
    let wide = (shown + if ellipsis { 3 } else { 0 }) as i32 * CELL_WIDTH;
    let x = rect.left + (rect.width() - wide) / 2;
    let saved = surface.clip();
    surface.clip_to(rect);
    draw_fitted(
        surface,
        x,
        cell_top(rect.top, rect.height()),
        cells,
        room,
        text,
    );
    surface.set_clip(saved);
}

fn paint_check_box(
    surface: &mut Surface<'_>,
    rect: Rect,
    cells: &[Cell],
    look: ButtonLook,
    palette: &Palette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, palette.surface);
    let box_rect = Rect::sized(
        rect.left + BOX_LEFT,
        rect.top + (rect.height() - BOX_SIZE) / 2,
        BOX_SIZE,
        BOX_SIZE,
    );
    let (fill, border, mark) = match () {
        () if !look.enabled => (
            palette.box_disabled_fill,
            palette.box_disabled_border,
            palette.box_disabled_mark,
        ),
        () if look.pressed => (
            palette.box_pressed_fill,
            palette.box_border,
            palette.box_mark,
        ),
        () if look.checked => (
            palette.box_checked_fill,
            palette.box_border,
            palette.box_mark,
        ),
        () => (palette.box_fill, palette.box_border, palette.box_mark),
    };
    surface.fill(box_rect, fill);
    outline(surface, box_rect, border);
    if look.checked {
        tick(surface, box_rect.left, box_rect.top, mark);
    }
    let x = rect.left + CHECK_LABEL_LEFT;
    let top = cell_top(rect.top, rect.height());
    let colour = match look.enabled {
        true => palette.text,
        false => palette.text_disabled,
    };
    let drawn = draw_fitted(
        surface,
        x,
        top,
        cells,
        rect.width() - CHECK_LABEL_ROOM_LESS,
        colour,
    );
    if look.focused {
        match palette.rounded {
            true => ring(
                surface,
                Rect::sized(rect.left, box_rect.top - 2, BOX_SIZE + 4, BOX_SIZE + 4),
                palette.focus,
            ),
            false => dotted_outline(
                surface,
                Rect::sized(x - 1, top - 1, drawn as i32 * CELL_WIDTH + 2, 18),
                palette.focus,
            ),
        }
    }
    surface.set_clip(saved);
}

/// The check mark: eight 2x2 squares from the 14x14 box whose top-left is (`x`, `y`).
pub fn tick(surface: &mut Surface<'_>, x: i32, y: i32, colour: u32) {
    for (dx, dy) in TICK {
        surface.fill(
            Rect::sized(x + dx, y + dy, TICK_SQUARE, TICK_SQUARE),
            colour,
        );
    }
}

/// What one input did to a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonOutcome {
    Unchanged,
    Repaint,
    Clicked,
}

/// A button's state: its kind, whether it is checked, and the press in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub kind: ButtonKind,
    pub checked: bool,
    pub pressed: bool,
    pointer_held: bool,
    space_held: bool,
}

impl Button {
    pub fn new(kind: ButtonKind) -> Button {
        Button {
            kind,
            checked: false,
            pressed: false,
            pointer_held: false,
            space_held: false,
        }
    }

    /// The left button went down (`down`) or up with the pointer `inside` the button or not.
    pub fn left_button(&mut self, inside: bool, down: bool) -> ButtonOutcome {
        match down {
            true if inside => {
                self.pointer_held = true;
                self.pressed = true;
                ButtonOutcome::Repaint
            }
            true => ButtonOutcome::Unchanged,
            false if self.pointer_held => {
                self.pointer_held = false;
                self.pressed = false;
                match inside {
                    true => self.click(),
                    false => ButtonOutcome::Repaint,
                }
            }
            false => ButtonOutcome::Unchanged,
        }
    }

    /// The pointer moved, `inside` the button or not, while a press may be held.
    pub fn pointer_moved(&mut self, inside: bool) -> ButtonOutcome {
        if !self.pointer_held || self.pressed == inside {
            return ButtonOutcome::Unchanged;
        }
        self.pressed = inside;
        ButtonOutcome::Repaint
    }

    /// Space went down or up while the button has the focus.
    pub fn space(&mut self, down: bool) -> ButtonOutcome {
        match down {
            true if self.space_held => ButtonOutcome::Unchanged,
            true => {
                self.space_held = true;
                self.pressed = true;
                ButtonOutcome::Repaint
            }
            false if self.space_held => {
                self.space_held = false;
                self.pressed = false;
                self.click()
            }
            false => ButtonOutcome::Unchanged,
        }
    }

    /// `BM_CLICK` or a finished press: an automatic check box toggles.
    pub fn click(&mut self) -> ButtonOutcome {
        if self.kind == ButtonKind::AutoCheckBox {
            self.checked = !self.checked;
        }
        ButtonOutcome::Clicked
    }

    /// Ends a press without a click, as losing the focus or the capture does.
    pub fn cancel(&mut self) -> ButtonOutcome {
        let was = self.pressed;
        self.pressed = false;
        self.pointer_held = false;
        self.space_held = false;
        match was {
            true => ButtonOutcome::Repaint,
            false => ButtonOutcome::Unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{DARK, LIGHT};

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn look(kind: ButtonKind) -> ButtonLook {
        ButtonLook {
            kind,
            checked: false,
            pressed: false,
            focused: false,
            default: false,
            enabled: true,
        }
    }

    #[test]
    fn a_default_light_button_has_a_two_pixel_border_and_a_centred_underlined_label() {
        let mut pixels = vec![0u32; 88 * 24];
        let mut surface = Surface::new(&mut pixels, 88, 24).unwrap();
        let default = ButtonLook {
            default: true,
            ..look(ButtonKind::DefaultPush)
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 88, 24),
            &units("&Add"),
            default,
            &LIGHT,
        );
        assert_eq!(surface.get(0, 0), Some(0x0000_78d7));
        assert_eq!(surface.get(1, 1), Some(0x0000_78d7));
        assert_eq!(surface.get(86, 22), Some(0x0000_78d7));
        assert_eq!(surface.get(2, 2), Some(0x00e1_e1e1));
        for x in 32..40 {
            assert_eq!(surface.get(x, 4 + 14), Some(0));
        }
        assert_eq!(surface.get(31, 18), Some(0x00e1_e1e1));
    }

    #[test]
    fn a_focused_light_button_shows_the_dotted_rectangle_at_inset_three() {
        let mut pixels = vec![0u32; 40 * 24];
        let mut surface = Surface::new(&mut pixels, 40, 24).unwrap();
        let focused = ButtonLook {
            focused: true,
            ..look(ButtonKind::Push)
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 40, 24),
            &units(""),
            focused,
            &LIGHT,
        );
        assert_eq!(surface.get(3, 3), Some(0));
        assert_eq!(surface.get(4, 3), Some(0x00e1_e1e1));
        assert_eq!(surface.get(5, 3), Some(0));
    }

    #[test]
    fn a_dark_default_button_is_primary_with_rounded_corners() {
        let mut pixels = vec![0u32; 96 * 28];
        let mut surface = Surface::new(&mut pixels, 96, 28).unwrap();
        let default = ButtonLook {
            default: true,
            ..look(ButtonKind::DefaultPush)
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 96, 28),
            &units("Open"),
            default,
            &DARK,
        );
        assert_eq!(surface.get(0, 0), Some(0));
        assert_eq!(surface.get(2, 0), Some(DARK.default_fill));
        assert_eq!(surface.get(40, 1), Some(DARK.default_fill));
    }

    #[test]
    fn a_checked_box_draws_the_tick_and_the_label_beside_it() {
        let mut pixels = vec![0u32; 160 * 24];
        let mut surface = Surface::new(&mut pixels, 160, 24).unwrap();
        let checked = ButtonLook {
            checked: true,
            ..look(ButtonKind::AutoCheckBox)
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 160, 24),
            &units("&Upper case"),
            checked,
            &LIGHT,
        );
        assert_eq!(surface.get(2, 5), Some(0x0033_3333));
        assert_eq!(surface.get(3, 6), Some(0x00ff_ffff));
        assert_eq!(surface.get(2 + 3, 5 + 7), Some(0));
        assert_eq!(surface.get(2 + 11, 5 + 5), Some(0));
        assert_eq!(surface.get(22, 18), Some(0));
        assert_eq!(surface.get(0, 0), Some(0x00f0_f0f0));
    }

    #[test]
    fn a_press_released_inside_clicks_and_an_auto_check_box_toggles() {
        let mut button = Button::new(ButtonKind::AutoCheckBox);
        assert_eq!(button.left_button(true, true), ButtonOutcome::Repaint);
        assert!(button.pressed);
        assert_eq!(button.pointer_moved(false), ButtonOutcome::Repaint);
        assert!(!button.pressed);
        assert_eq!(button.pointer_moved(true), ButtonOutcome::Repaint);
        assert_eq!(button.left_button(true, false), ButtonOutcome::Clicked);
        assert!(button.checked && !button.pressed);
        assert_eq!(button.left_button(true, true), ButtonOutcome::Repaint);
        assert_eq!(button.left_button(false, false), ButtonOutcome::Repaint);
        assert!(button.checked);
        assert_eq!(button.left_button(true, false), ButtonOutcome::Unchanged);
        assert_eq!(button.space(true), ButtonOutcome::Repaint);
        assert_eq!(button.space(true), ButtonOutcome::Unchanged);
        assert_eq!(button.space(false), ButtonOutcome::Clicked);
        assert!(!button.checked);
        let mut push = Button::new(ButtonKind::Push);
        assert_eq!(push.click(), ButtonOutcome::Clicked);
        assert!(!push.checked);
    }
}
