//! The Display and Mouse panes and the Keyboard pane's key repeat rows (D195,
//! docs/ux/displays-mouse-v0.3.md): the display's scale for the next restart, the mouse's
//! buttons, scrolling and pointer speed, and how a held key repeats. Each change is one
//! [`Request`] the program saves, then applies; the window shows the outcome.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::keys::InputChange;
use crate::settings_window::{CAPACITY, Request};
use core::fmt::Write;
use eggos_runtime::errno::Errno;
use eggos_settings_file::InputPreferences;
use eggos_toolkit::colour;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const STATUS_LINE: i32 = 316;

const SCALES: [&str; 2] = ["1x", "2x"];
const HANDS: [&str; 2] = ["Right-handed", "Left-handed"];
const SCROLLING: [&str; 2] = ["Traditional", "Natural"];

const SCALE_HINT: &str = "Takes effect at the next restart";
const NO_DISK_FOR_SCALE: &str = "No disk attached: the scale cannot be saved for a restart.";
const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";
const NOT_TAKEN: &str = "Saved for the next boot, but the window server did not take it.";

/// What a pane did with a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// It was not for the pane.
    NotMine,
    /// The pane acted on it, and the program must do this, if anything.
    Done(Option<Request>),
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn label(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    x: i32,
    y: i32,
    text: &str,
) -> WidgetId {
    add(
        tree,
        parent,
        Place::at(x, y, Length::Content, Length::Content),
        Kind::label(text, colour::TEXT),
    )
}

fn from_right(margin: i32, top: i32, width: i32, height: i32) -> Place {
    Place {
        horizontal: Horizontal::Right(margin),
        top,
        width: Length::Px(width),
        height: Length::Px(height),
    }
}

fn show_status(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, text: Option<(&str, u32)>) {
    match text {
        Some((words, ink)) => {
            show_line(tree, id, words, ink);
            tree.set_shown(id, true);
        }
        None => tree.set_shown(id, false),
    }
}

/// A preference's range, as its slider sets it (docs/ux/toolkit-v0.4.md section 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// What an agent calls it, the words of the label before it.
    pub name: &'static str,
    /// The smallest value.
    pub low: u32,
    /// The largest value.
    pub high: u32,
    /// How far a key moves it.
    pub step: u32,
    /// What follows the number: `""`, `" ms"`, `"/s"`.
    pub unit: &'static str,
}

/// The pointer speed's range.
pub const SPEED: Range = Range {
    name: "Pointer speed",
    low: 1,
    high: 10,
    step: 1,
    unit: "",
};
/// The repeat delay's range.
pub const DELAY: Range = Range {
    name: "Repeat delay",
    low: 150,
    high: 2000,
    step: 50,
    unit: " ms",
};
/// The repeat rate's range.
pub const RATE: Range = Range {
    name: "Repeat rate",
    low: 2,
    high: 60,
    step: 1,
    unit: "/s",
};

const SLIDER_WIDTH: i32 = 216;

fn slider(
    tree: &mut WidgetTree<CAPACITY>,
    pane: WidgetId,
    y: i32,
    range: Range,
    value: u32,
) -> WidgetId {
    add(
        tree,
        pane,
        from_right(PADDING, y - 6, SLIDER_WIDTH, CONTROL_HEIGHT),
        Kind::slider(
            range.name,
            range.low as i32,
            range.high as i32,
            range.step as i32,
            value as i32,
            range.unit,
        ),
    )
}

fn halvable(resolution: (u32, u32)) -> bool {
    resolution.0.is_multiple_of(2) && resolution.1.is_multiple_of(2)
}

fn size_text(text: &mut Text<96>, (width, height): (u32, u32)) {
    let _ = write!(text, "{width} \u{d7} {height}");
}

/// The Display pane's status line for `facts` (section 2.3), and whether it is a result the
/// user caused (drawn in `text` rather than `text.dim`).
pub fn scale_line(facts: &Facts) -> (Text<96>, bool) {
    let device = facts.resolution;
    let half = (device.0 / 2, device.1 / 2);
    let mut text = Text::new();
    if !halvable(device) {
        let _ = text.write_str("2x is not available: ");
        size_text(&mut text, device);
        let _ = text.write_str(" cannot be halved.");
        return (text, false);
    }
    if !facts.on_disk {
        let _ = text.write_str(NO_DISK_FOR_SCALE);
        return (text, false);
    }
    let caused = facts.scale != facts.scale_in_force;
    match (facts.scale, facts.scale_in_force) {
        (2, 2) => {
            let _ = text.write_str("Drawn like ");
            size_text(&mut text, half);
            let _ = text.write_str(" and doubled to ");
            size_text(&mut text, device);
            let _ = text.write_str(".");
        }
        (2, _) => {
            let _ = text.write_str("From the next restart the desktop looks like ");
            size_text(&mut text, half);
            let _ = text.write_str(".");
        }
        (_, 2) => {
            let _ = text.write_str("From the next restart the desktop looks like ");
            size_text(&mut text, device);
            let _ = text.write_str(".");
        }
        _ => {
            let _ = text.write_str("At 2x the desktop looks like ");
            size_text(&mut text, half);
            let _ = text.write_str(".");
        }
    }
    (text, caused)
}

/// The Display pane's widgets.
#[derive(Debug)]
pub struct DisplayPane {
    resolution: WidgetId,
    scales: WidgetId,
    status: WidgetId,
}

impl DisplayPane {
    /// Builds the pane's rows in `pane` under its heading, showing `facts`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, facts: &Facts) -> Self {
        label(tree, pane, PADDING, 76, "Resolution");
        let resolution = add(
            tree,
            pane,
            Place::right(PADDING, 76),
            Kind::label("", colour::TEXT),
        );
        label(tree, pane, PADDING, 116, "Scale");
        let radios = Kind::radios(&SCALES, 0, true);
        let radios_width = radios.content_size().0;
        let scales = add(tree, pane, Place::right(PADDING, 110), radios);
        add(
            tree,
            pane,
            Place::right(PADDING + radios_width + PADDING, 116),
            Kind::label(SCALE_HINT, colour::TEXT_DIM),
        );
        let status = label(tree, pane, PADDING, 156, "");
        label(tree, pane, PADDING, 196, "Pointer");
        add(
            tree,
            pane,
            Place::right(PADDING, 196),
            Kind::label("Software", colour::TEXT),
        );
        let display = Self {
            resolution,
            scales,
            status,
        };
        display.show(tree, facts);
        display
    }

    /// Shows `facts`: the device's size, the saved scale, the state line.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        let mut size = Text::<96>::new();
        size_text(&mut size, facts.resolution);
        tree.set_text(self.resolution, size.as_str());
        let choosable = facts.on_disk && halvable(facts.resolution);
        let shown = match choosable {
            true => facts.scale,
            false => facts.scale_in_force,
        };
        tree.set_selected(self.scales, usize::from(shown == 2));
        tree.set_enabled(self.scales, choosable);
        let (line, caused) = scale_line(facts);
        let ink = match caused {
            true => colour::TEXT,
            false => colour::TEXT_DIM,
        };
        show_status(tree, self.status, Some((line.as_str(), ink)));
    }

    /// How saving the scale went: `facts` hold the saved scale when it worked.
    pub fn saved(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts, saved: Result<(), Errno>) {
        self.show(tree, facts);
        let refusal = match saved {
            Ok(()) => return,
            Err(Errno::EPERM | Errno::EACCES) => "Not allowed: this user may not set the scale.",
            Err(_) => "No disk attached: the scale could not be saved.",
        };
        show_status(tree, self.status, Some((refusal, colour::TEXT)));
    }

    /// The device's size as shown.
    pub fn resolution_shown<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        tree.text_of(self.resolution)
    }

    /// The radios' selection and the state line, for tests.
    pub fn state<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> (Option<usize>, Option<&'a str>) {
        (tree.selected(self.scales), tree.text_of(self.status))
    }

    /// Acts on a response.
    pub fn respond(&self, response: Response, facts: &Facts) -> Reply {
        match response {
            Response::Chosen(id, choice) if id == self.scales => {
                let scale = if choice == 1 { 2 } else { 1 };
                Reply::Done((scale != facts.scale).then_some(Request::SetDisplayScale(scale)))
            }
            _ => Reply::NotMine,
        }
    }
}

/// The Mouse pane's widgets and the Keyboard pane's repeat rows.
#[derive(Debug)]
pub struct InputPanes {
    hands: WidgetId,
    scrolling: WidgetId,
    speed: WidgetId,
    mouse_status: WidgetId,
    delay: WidgetId,
    rate: WidgetId,
    keyboard_status: WidgetId,
}

impl InputPanes {
    /// Builds the Mouse pane's rows in `mouse` and the repeat rows in `keyboard`, whose status
    /// line is `keyboard_status`, showing `input`.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        mouse: WidgetId,
        keyboard: WidgetId,
        keyboard_status: WidgetId,
        input: InputPreferences,
    ) -> Self {
        label(tree, mouse, PADDING, 76, "Buttons");
        let hands = add(
            tree,
            mouse,
            Place::right(PADDING, 70),
            Kind::radios(&HANDS, 0, true),
        );
        label(tree, mouse, PADDING, 116, "Scrolling");
        let scrolling = add(
            tree,
            mouse,
            Place::right(PADDING, 110),
            Kind::radios(&SCROLLING, 0, true),
        );
        label(tree, mouse, PADDING, 156, "Pointer speed");
        let speed = slider(tree, mouse, 156, SPEED, input.pointer_speed);
        let mouse_status = label(tree, mouse, PADDING, STATUS_LINE, "");
        label(tree, keyboard, 240, 188, "Repeat delay");
        let delay = slider(tree, keyboard, 188, DELAY, input.repeat_delay_ms);
        label(tree, keyboard, 240, 228, "Repeat rate");
        let rate = slider(tree, keyboard, 228, RATE, input.repeat_rate);
        let panes = Self {
            hands,
            scrolling,
            speed,
            mouse_status,
            delay,
            rate,
            keyboard_status,
        };
        panes.show(tree, input);
        tree.set_shown(mouse_status, false);
        panes
    }

    /// Shows `input` in the controls.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, input: InputPreferences) {
        tree.set_selected(self.hands, usize::from(input.left_handed));
        tree.set_selected(self.scrolling, usize::from(input.natural_scrolling));
        tree.set_value(self.speed, input.pointer_speed as i32);
        tree.set_value(self.delay, input.repeat_delay_ms as i32);
        tree.set_value(self.rate, input.repeat_rate as i32);
    }

    /// Shows the panes afresh: the controls at `facts`, the status lines at their "shown" state.
    pub fn reset(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        self.show(tree, facts.input);
        let line = (!facts.on_disk).then_some((NO_DISK, colour::TEXT_DIM));
        show_status(tree, self.mouse_status, line);
        show_status(tree, self.keyboard_status, line);
    }

    /// How a change went: `input` are the preferences in force after it (the old ones when
    /// the save failed), `saved` the save's outcome and `applied` the window server's.
    pub fn changed(
        &self,
        tree: &mut WidgetTree<CAPACITY>,
        change: InputChange,
        input: InputPreferences,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        self.show(tree, input);
        let keyboard = matches!(
            change,
            InputChange::RepeatDelayMs(_) | InputChange::RepeatRate(_)
        );
        let (status, refused, failed) = match keyboard {
            true => (
                self.keyboard_status,
                "Not allowed: this user may not change the keyboard settings.",
                "No disk attached: the keyboard settings could not be saved.",
            ),
            false => (
                self.mouse_status,
                "Not allowed: this user may not change the mouse settings.",
                "No disk attached: the mouse settings could not be saved.",
            ),
        };
        let line = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => refused,
            (Err(_), _) => failed,
            (Ok(()), Err(_)) => NOT_TAKEN,
            (Ok(()), Ok(())) => return,
        };
        show_status(tree, status, Some((line, colour::TEXT)));
    }

    /// The Mouse pane's status line when shown.
    pub fn mouse_status<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        tree.is_shown(self.mouse_status)
            .then(|| tree.text_of(self.mouse_status))
            .flatten()
    }

    /// The three sliders' values as shown: speed, delay, rate.
    pub fn values(&self, tree: &WidgetTree<CAPACITY>) -> [Option<i32>; 3] {
        [self.speed, self.delay, self.rate].map(|slider| tree.value(slider))
    }

    /// Acts on a response, the preferences in force being `input`.
    pub fn respond(&self, response: Response, input: InputPreferences) -> Reply {
        let change = match response {
            Response::Chosen(id, choice) if id == self.hands => {
                InputChange::LeftHanded(choice == 1)
            }
            Response::Chosen(id, choice) if id == self.scrolling => {
                InputChange::NaturalScrolling(choice == 1)
            }
            Response::Adjusted(id, value) if id == self.speed => {
                InputChange::PointerSpeed(value as u32)
            }
            Response::Adjusted(id, value) if id == self.delay => {
                InputChange::RepeatDelayMs(value as u32)
            }
            Response::Adjusted(id, value) if id == self.rate => {
                InputChange::RepeatRate(value as u32)
            }
            _ => return Reply::NotMine,
        };
        let unchanged = change.applied_to(input) == input;
        Reply::Done((!unchanged).then_some(Request::ChangeInput(change)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_line_says_what_the_next_restart_brings() {
        let line = |scale, scale_in_force, resolution, on_disk| {
            let facts = Facts {
                scale,
                scale_in_force,
                resolution,
                on_disk,
                ..Facts::example()
            };
            let (text, caused) = scale_line(&facts);
            (String::from(text.as_str()), caused)
        };
        assert_eq!(
            line(1, 1, (1280, 800), true),
            ("At 2x the desktop looks like 640 \u{d7} 400.".into(), false)
        );
        assert_eq!(
            line(2, 2, (1280, 800), true),
            (
                "Drawn like 640 \u{d7} 400 and doubled to 1280 \u{d7} 800.".into(),
                false
            )
        );
        assert_eq!(
            line(2, 1, (1280, 800), true),
            (
                "From the next restart the desktop looks like 640 \u{d7} 400.".into(),
                true
            )
        );
        assert_eq!(
            line(1, 2, (1280, 800), true),
            (
                "From the next restart the desktop looks like 1280 \u{d7} 800.".into(),
                true
            )
        );
        assert_eq!(
            line(1, 1, (1280, 800), false),
            (NO_DISK_FOR_SCALE.into(), false)
        );
        assert_eq!(
            line(1, 1, (1281, 801), true),
            (
                "2x is not available: 1281 \u{d7} 801 cannot be halved.".into(),
                false
            )
        );
    }
}
