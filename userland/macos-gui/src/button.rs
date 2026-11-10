//! NSButton's types and states (`docs/ux/macos-gui-v0.3.md` section 9): every `NSButtonType`
//! draws as the push button, the push button with an on look, the toolkit's checkbox or its
//! radio, 28 tall and centred vertically in the app's frame; `state` moves on a click in
//! AppKit's order.

use crate::geometry::Frame;
use crate::paint::control_rect;
use eggos_toolkit::canvas::{Canvas, INSET_R4, SQUARE};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text::Text;
use eggos_toolkit::widget::{self, Align, Kind, LABEL_OFFSET, Look, MARK_SIZE, MAX_OPTIONS};

/// `NSControlStateValueOff`.
pub const OFF: isize = 0;
/// `NSControlStateValueOn`.
pub const ON: isize = 1;
/// `NSControlStateValueMixed`.
pub const MIXED: isize = -1;

/// `NSButtonTypeMomentaryLight`.
pub const MOMENTARY_LIGHT: usize = 0;
/// `NSButtonTypePushOnPushOff`.
pub const PUSH_ON_PUSH_OFF: usize = 1;
/// `NSButtonTypeToggle`.
pub const TOGGLE: usize = 2;
/// `NSButtonTypeSwitch`: the checkbox.
pub const SWITCH: usize = 3;
/// `NSButtonTypeRadio`.
pub const RADIO: usize = 4;
/// `NSButtonTypeMomentaryChange`.
pub const MOMENTARY_CHANGE: usize = 5;
/// `NSButtonTypeOnOff`.
pub const ON_OFF: usize = 6;
/// `NSButtonTypeMomentaryPushIn`, what `initWithFrame:` starts as.
pub const MOMENTARY_PUSH_IN: usize = 7;

const TITLE_BYTES: usize = 32;
const BUTTON_PADDING: i32 = 8;
const MIXED_BAR: Rect = Rect::new(3, 7, 10, 2);

/// What a button type is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Section 2's push button; its state is never drawn.
    Push,
    /// The push button with an on look; its state flips on every click.
    Toggle,
    Checkbox,
    Radio,
}

/// The family of the `NSButtonType` `button_type`; unknown types are push buttons.
pub fn family(button_type: usize) -> Family {
    match button_type {
        PUSH_ON_PUSH_OFF | TOGGLE | ON_OFF => Family::Toggle,
        SWITCH => Family::Checkbox,
        RADIO => Family::Radio,
        _ => Family::Push,
    }
}

/// The state `state` is stored as: off, on, or mixed only when the button `allows_mixed` (and
/// is not a radio); any other value is on.
pub fn stored_state(state: isize, allows_mixed: bool, family: Family) -> isize {
    match state {
        OFF => OFF,
        MIXED if allows_mixed && family == Family::Checkbox => MIXED,
        _ => ON,
    }
}

/// The state a click moves `state` to (`nextState`): on, off, mixed, on with three states;
/// on, off, on with two. A radio that is on stays on.
pub fn next_state(state: isize, allows_mixed: bool, family: Family) -> isize {
    match (family, state) {
        (Family::Push, state) => state,
        (Family::Radio, _) => ON,
        (_, ON) => OFF,
        (Family::Checkbox, OFF) if allows_mixed => MIXED,
        _ => ON,
    }
}

/// The title a button shows: `alternate` (when not empty) while a toggle of type `Toggle` is
/// on or a `MomentaryChange` button is pressed, else `title`.
pub fn shown_title<'a>(
    button_type: usize,
    state: isize,
    pressed: bool,
    title: &'a str,
    alternate: &'a str,
) -> &'a str {
    let alternate_shown = match button_type {
        TOGGLE => state == ON,
        MOMENTARY_CHANGE => pressed,
        _ => false,
    };
    match alternate_shown && !alternate.is_empty() {
        true => alternate,
        false => title,
    }
}

/// Where a button at `frame` is drawn, and where it takes clicks: the frame's width (at least
/// the mark's for a checkbox or radio), 28 tall, centred vertically in the frame.
pub fn drawn_rect(frame: Frame, height: i32, family: Family) -> Rect {
    let rect = control_rect(frame, height);
    match family {
        Family::Checkbox | Family::Radio => {
            Rect::new(rect.x, rect.y, rect.width.max(MARK_SIZE), rect.height)
        }
        Family::Push | Family::Toggle => rect,
    }
}

/// A button to draw: its family, the title it shows, its state and whether it is the window's
/// default button.
#[derive(Debug, Clone, Copy)]
pub struct Button<'a> {
    pub family: Family,
    pub title: &'a str,
    pub state: isize,
    pub primary: bool,
}

/// Draws `button` at `frame` in a window whose content is `height` tall.
pub fn paint(canvas: &mut Canvas<'_>, height: i32, frame: Frame, button: Button<'_>, look: Look) {
    let rect = drawn_rect(frame, height, button.family);
    match button.family {
        Family::Toggle if button.state == ON => paint_toggle_on(canvas, rect, button.title, look),
        Family::Push | Family::Toggle => {
            let primary = button.primary && button.family == Family::Push;
            widget::paint(canvas, &Kind::button(button.title, primary), rect, look);
        }
        Family::Checkbox => paint_checkbox(canvas, rect, button.title, button.state, look),
        Family::Radio => paint_radio(canvas, rect, button.title, button.state == ON, look),
    }
}

fn mark_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y + (rect.height - MARK_SIZE) / 2,
        MARK_SIZE,
        MARK_SIZE,
    )
}

fn paint_checkbox(canvas: &mut Canvas<'_>, rect: Rect, title: &str, state: isize, look: Look) {
    let mut held = Text::<TITLE_BYTES>::new();
    if rect.width >= LABEL_OFFSET {
        held.set(title);
    }
    widget::paint(
        canvas,
        &Kind::checkbox(held.as_str(), state != OFF),
        rect,
        look,
    );
    if state != MIXED {
        return;
    }
    let palette = canvas.palette();
    let mark = mark_rect(rect);
    let pressed = look.enabled && look.pressed.is_some();
    let hovered = look.enabled && look.hovered.is_some();
    let (fill, ink) = match (look.enabled, pressed, hovered) {
        (false, _, _) => (palette.button, palette.text_dim),
        (true, true, _) => (palette.button_primary_pressed, palette.icon_dark),
        (true, false, true) => (palette.button_primary_hover, palette.icon_dark),
        (true, false, false) => (palette.button_primary, palette.icon_dark),
    };
    canvas.rounded(mark, &SQUARE, fill);
    canvas.fill(
        Rect::new(
            mark.x + MIXED_BAR.x,
            mark.y + MIXED_BAR.y,
            MIXED_BAR.width,
            MIXED_BAR.height,
        ),
        ink,
    );
}

fn paint_radio(canvas: &mut Canvas<'_>, rect: Rect, title: &str, on: bool, look: Look) {
    let kind = Kind::Radios {
        options: [""; MAX_OPTIONS],
        count: 1,
        selected: if on { 0 } else { 1 },
        horizontal: false,
    };
    widget::paint(canvas, &kind, rect, look);
    if rect.width < LABEL_OFFSET {
        return;
    }
    let palette = canvas.palette();
    let colour = if look.enabled {
        palette.text
    } else {
        palette.text_dim
    };
    widget::label_text(
        canvas,
        Rect::new(
            rect.x + LABEL_OFFSET,
            rect.y,
            rect.width - LABEL_OFFSET,
            rect.height,
        ),
        title,
        colour,
        1,
        Align::Start,
    );
}

fn paint_toggle_on(canvas: &mut Canvas<'_>, rect: Rect, title: &str, look: Look) {
    let palette = canvas.palette();
    let pressed = look.enabled && look.pressed.is_some();
    let (fill, border, ink) = match (look.enabled, pressed) {
        (false, _) => (palette.button, palette.text_dim, palette.text_dim),
        (true, true) => (palette.button_pressed, palette.accent, palette.text),
        (true, false) => (palette.selection, palette.accent, palette.text),
    };
    canvas.rounded(rect, &INSET_R4, fill);
    canvas.outline(rect, &INSET_R4, 1, border);
    widget::label_text(
        canvas,
        Rect::new(
            rect.x + BUTTON_PADDING,
            rect.y,
            rect.width - 2 * BUTTON_PADDING,
            rect.height,
        ),
        title,
        ink,
        1,
        Align::Centre,
    );
}

/// Whether the radio `candidate` is in the group of the radio `radio`: the same superview and
/// the same action, which is not null.
pub fn same_group(radio: (usize, usize), candidate: (usize, usize)) -> bool {
    radio.1 != 0 && radio == candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_toolkit::theme::Palette;

    const WIDTH: u32 = 200;
    const HEIGHT: i32 = 360;

    fn render(frame: Frame, button: Button<'_>, look: Look) -> Vec<u32> {
        let mut pixels = vec![0; (WIDTH * HEIGHT as u32) as usize];
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        paint(&mut canvas, HEIGHT, frame, button, look);
        pixels
    }

    fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
        pixels[(y * WIDTH as i32 + x) as usize]
    }

    fn palette() -> &'static Palette {
        let mut pixels = vec![0; 4];
        Canvas::new(&mut pixels, 2, 2, 1).unwrap().palette()
    }

    fn enabled() -> Look {
        Look {
            enabled: true,
            ..Look::default()
        }
    }

    fn checkbox(state: isize) -> Button<'static> {
        Button {
            family: Family::Checkbox,
            title: "Show grid",
            state,
            primary: false,
        }
    }

    const SHOW_GRID: Frame = Frame::new(16.0, 316.0, 160.0, 28.0);

    #[test]
    fn types_fall_into_their_families() {
        assert_eq!(family(MOMENTARY_PUSH_IN), Family::Push);
        assert_eq!(family(MOMENTARY_LIGHT), Family::Push);
        assert_eq!(family(MOMENTARY_CHANGE), Family::Push);
        assert_eq!(family(8), Family::Push);
        assert_eq!(family(PUSH_ON_PUSH_OFF), Family::Toggle);
        assert_eq!(family(ON_OFF), Family::Toggle);
        assert_eq!(family(TOGGLE), Family::Toggle);
        assert_eq!(family(SWITCH), Family::Checkbox);
        assert_eq!(family(RADIO), Family::Radio);
    }

    #[test]
    fn controls_are_centred_in_frames_of_every_height() {
        for (frame_height, top) in [(18.0, 11), (22.0, 13), (28.0, 16), (32.0, 18)] {
            let frame_top = 16.0;
            let y = HEIGHT as f64 - frame_top - frame_height;
            let frame = Frame::new(16.0, y, 160.0, frame_height);
            assert_eq!(
                drawn_rect(frame, HEIGHT, Family::Checkbox).y,
                top,
                "{frame_height}"
            );
        }
    }

    #[test]
    fn the_show_grid_checkbox_is_where_the_design_puts_it() {
        assert_eq!(
            drawn_rect(SHOW_GRID, HEIGHT, Family::Checkbox),
            Rect::new(16, 16, 160, 28)
        );
        let palette = palette();
        let off = render(SHOW_GRID, checkbox(OFF), enabled());
        assert_eq!(at(&off, 16, 22), palette.button_border);
        let on = render(SHOW_GRID, checkbox(ON), enabled());
        assert_eq!(at(&on, 19, 30), palette.icon_dark);
        assert_eq!(at(&on, 17, 23), palette.button_primary);
        let mixed = render(SHOW_GRID, checkbox(MIXED), enabled());
        assert_eq!(at(&mixed, 20, 29), palette.icon_dark);
        assert_eq!(at(&mixed, 19, 31), palette.button_primary);
        assert_ne!(on, mixed);
    }

    #[test]
    fn a_disabled_mixed_checkbox_draws_its_bar_dim() {
        let palette = palette();
        let pixels = render(SHOW_GRID, checkbox(MIXED), Look::default());
        assert_eq!(at(&pixels, 20, 29), palette.text_dim);
        assert_eq!(at(&pixels, 17, 23), palette.button);
    }

    #[test]
    fn checkboxes_cycle_on_off_mixed_with_three_states_and_on_off_with_two() {
        let next = |state| next_state(state, true, Family::Checkbox);
        assert_eq!(next(ON), OFF);
        assert_eq!(next(OFF), MIXED);
        assert_eq!(next(MIXED), ON);
        let two = |state| next_state(state, false, Family::Checkbox);
        assert_eq!(two(ON), OFF);
        assert_eq!(two(OFF), ON);
        assert_eq!(next_state(ON, false, Family::Toggle), OFF);
        assert_eq!(next_state(OFF, true, Family::Toggle), ON);
        assert_eq!(next_state(ON, false, Family::Radio), ON);
        assert_eq!(next_state(OFF, false, Family::Push), OFF);
    }

    #[test]
    fn mixed_is_on_without_allows_mixed_state_and_for_radios() {
        assert_eq!(stored_state(MIXED, false, Family::Checkbox), ON);
        assert_eq!(stored_state(MIXED, true, Family::Checkbox), MIXED);
        assert_eq!(stored_state(MIXED, true, Family::Radio), ON);
        assert_eq!(stored_state(5, false, Family::Toggle), ON);
        assert_eq!(stored_state(OFF, true, Family::Checkbox), OFF);
    }

    #[test]
    fn radios_draw_their_dot_only_when_on() {
        let palette = palette();
        let large = Frame::new(16.0, 252.0, 160.0, 28.0);
        assert_eq!(
            drawn_rect(large, HEIGHT, Family::Radio),
            Rect::new(16, 80, 160, 28)
        );
        let radio = |state| Button {
            family: Family::Radio,
            title: "Large",
            state,
            primary: false,
        };
        let on = render(large, radio(ON), enabled());
        assert_eq!(at(&on, 23, 93), palette.icon_dark);
        let off = render(large, radio(OFF), enabled());
        assert_eq!(at(&off, 23, 93), palette.field);
    }

    #[test]
    fn a_toggle_that_is_on_is_filled_with_the_selection_inside_an_accent_border() {
        let palette = palette();
        let bold = Frame::new(16.0, 212.0, 96.0, 28.0);
        let toggle = |state| Button {
            family: Family::Toggle,
            title: "Bold",
            state,
            primary: true,
        };
        let on = render(bold, toggle(ON), enabled());
        assert_eq!(at(&on, 16, 133), palette.accent);
        assert_eq!(at(&on, 20, 133), palette.selection);
        let off = render(bold, toggle(OFF), enabled());
        assert_eq!(at(&off, 20, 133), palette.button);
    }

    #[test]
    fn alternate_titles_show_while_a_toggle_is_on_or_a_momentary_change_is_pressed() {
        assert_eq!(shown_title(TOGGLE, ON, false, "Off", "On"), "On");
        assert_eq!(shown_title(TOGGLE, OFF, false, "Off", "On"), "Off");
        assert_eq!(shown_title(TOGGLE, ON, false, "Off", ""), "Off");
        assert_eq!(
            shown_title(MOMENTARY_CHANGE, OFF, true, "Go", "Going"),
            "Going"
        );
        assert_eq!(
            shown_title(MOMENTARY_CHANGE, OFF, false, "Go", "Going"),
            "Go"
        );
        assert_eq!(shown_title(PUSH_ON_PUSH_OFF, ON, true, "Go", "Going"), "Go");
    }

    #[test]
    fn radios_group_by_superview_and_action() {
        assert!(same_group((1, 7), (1, 7)));
        assert!(!same_group((1, 7), (1, 8)));
        assert!(!same_group((1, 7), (2, 7)));
        assert!(!same_group((1, 0), (1, 0)));
    }
}
