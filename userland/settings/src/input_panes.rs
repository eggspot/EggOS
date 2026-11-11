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
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const STATUS_LINE: i32 = 316;

const SCALES: [&str; 2] = ["1x", "2x"];
const HANDS: [&str; 2] = ["Right-handed", "Left-handed"];
const SCROLLING: [&str; 2] = ["Traditional", "Natural"];

const SCALE_HINT: &str = "Takes effect at the next restart";
const RESOLUTION_WIDTH: i32 = 160;
const LAYOUT_TOP: i32 = 356;
/// The Rotation row (docs/ux/display-rotation-v0.5.md, D333).
const ROTATION_TOP: i32 = 236;
const POINTER_TOP: i32 = 316;
/// The Rotation dropdown's options, in `DISPLAY_ROTATIONS`' order.
const ROTATION_OPTIONS: [&str; 5] = ["Automatic", "Upright", "90° clockwise", "Upside down", "90° counter-clockwise"];
/// The Rotation dropdown's width.
const ROTATION_WIDTH: i32 = 160;
const LAYOUT_OPTIONS: [&str; 5] = ["Automatic", "Desktop", "Tablet", "Phone", "TV"];
const AUTOMATIC: &str = "Automatic";
/// The smallest screen the desktop is laid out for, which a size at 2x must not go under.
pub const DESKTOP_MINIMUM: (u32, u32) = (640, 400);
const RESOLUTION_OPTIONS: [&str; 13] = [
    AUTOMATIC,
    "1024 \u{d7} 768",
    "1280 \u{d7} 720",
    "1280 \u{d7} 800",
    "1366 \u{d7} 768",
    "1440 \u{d7} 900",
    "1600 \u{d7} 900",
    "1680 \u{d7} 1050",
    "1920 \u{d7} 1080",
    "1920 \u{d7} 1200",
    "2560 \u{d7} 1440",
    "2560 \u{d7} 1600",
    "3840 \u{d7} 2160",
];
const RESOLUTION_OPTIONS_AT_2X: [&str; 10] = [
    AUTOMATIC,
    "1280 \u{d7} 800",
    "1440 \u{d7} 900",
    "1600 \u{d7} 900",
    "1680 \u{d7} 1050",
    "1920 \u{d7} 1080",
    "1920 \u{d7} 1200",
    "2560 \u{d7} 1440",
    "2560 \u{d7} 1600",
    "3840 \u{d7} 2160",
];
const FIXED_SIZE: &str = "This display's size is set by the computer and cannot be changed.";
const NO_DISK_FOR_RESOLUTION: &str = "No disk attached: the resolution cannot be saved for a restart.";
const RESTING: &str = "Automatic uses the display's own size.";
const SMALL_SIZES_LEFT_OUT: &str = "Scale 2x leaves out the smallest sizes. Choose 1x to use them.";
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
        Kind::label(text, Token::Text),
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

fn show_status(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, text: Option<(&str, Token)>) {
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

/// Whether size `size` is offered at scale `scale` (docs/ux/display-resolution-v0.4.md): whole
/// when divided by it and not under [`DESKTOP_MINIMUM`].
pub fn offered(size: (u32, u32), scale: u32) -> bool {
    let scale = scale.max(1);
    size.0.is_multiple_of(scale)
        && size.1.is_multiple_of(scale)
        && size.0 / scale >= DESKTOP_MINIMUM.0
        && size.1 / scale >= DESKTOP_MINIMUM.1
}

fn sizes_offered(scale: u32) -> impl Iterator<Item = (u32, u32)> {
    eggos_settings_file::RESOLUTIONS.into_iter().filter(move |size| offered(*size, scale))
}

fn options_for(scale: u32) -> &'static [&'static str] {
    match scale {
        2 => &RESOLUTION_OPTIONS_AT_2X,
        _ => &RESOLUTION_OPTIONS,
    }
}

fn size_at_next_restart(facts: &Facts) -> (u32, u32) {
    facts.resolution_choice.unwrap_or(facts.resolution)
}

/// The resolution line for `facts` (the design's section 3), and whether the user caused it.
pub fn resolution_line(facts: &Facts) -> (Text<96>, bool) {
    let mut text = Text::new();
    if !facts.resolution_changeable {
        let _ = text.write_str(FIXED_SIZE);
        return (text, false);
    }
    if !facts.on_disk {
        let _ = text.write_str(NO_DISK_FOR_RESOLUTION);
        return (text, false);
    }
    if let Some(size) = facts.resolution_choice
        && size != facts.resolution
    {
        let _ = text.write_str("From the next restart the display is ");
        size_text(&mut text, size);
        let _ = text.write_str(".");
        return (text, true);
    }
    let left_out = facts.scale == 2 && sizes_offered(2).count() < eggos_settings_file::RESOLUTIONS.len();
    let _ = text.write_str(if left_out { SMALL_SIZES_LEFT_OUT } else { RESTING });
    (text, false)
}

fn size_text(text: &mut Text<96>, (width, height): (u32, u32)) {
    let _ = write!(text, "{width} \u{d7} {height}");
}

/// The Display pane's status line for `facts` (section 2.3), and whether it is a result the
/// user caused (drawn in `text` rather than `text.dim`).
pub fn scale_line(facts: &Facts) -> (Text<96>, bool) {
    let device = facts.resolution;
    let next = size_at_next_restart(facts);
    let half = (device.0 / 2, device.1 / 2);
    let next_half = (next.0 / 2, next.1 / 2);
    let mut text = Text::new();
    if facts.scale != 2 && !offered(next, 2) && halvable(device) && facts.on_disk {
        let _ = text.write_str("2x is not available at ");
        size_text(&mut text, next);
        let _ = text.write_str(": choose a larger size first.");
        return (text, false);
    }
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
            size_text(&mut text, next_half);
            let _ = text.write_str(".");
        }
        (_, 2) => {
            let _ = text.write_str("From the next restart the desktop looks like ");
            size_text(&mut text, next);
            let _ = text.write_str(".");
        }
        _ => {
            let _ = text.write_str("At 2x the desktop looks like ");
            size_text(&mut text, next_half);
            let _ = text.write_str(".");
        }
    }
    (text, caused || next != device)
}

/// The Display pane's widgets.
#[derive(Debug)]
pub struct DisplayPane {
    layout: WidgetId,
    layout_status: WidgetId,
    resolution: WidgetId,
    choice: WidgetId,
    resolution_status: WidgetId,
    scales: WidgetId,
    status: WidgetId,
    rotation: WidgetId,
    rotation_status: WidgetId,
}

impl DisplayPane {
    /// Builds the pane's rows in `pane` under its heading, showing `facts`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, facts: &Facts) -> Self {
        label(tree, pane, PADDING, 76, "Resolution");
        let resolution = add(
            tree,
            pane,
            Place::right(PADDING, 76),
            Kind::label("", Token::Text),
        );
        let choice = add(
            tree,
            pane,
            from_right(PADDING, 70, RESOLUTION_WIDTH, CONTROL_HEIGHT),
            Kind::dropdown("Resolution", &RESOLUTION_OPTIONS, 0),
        );
        let resolution_status = label(tree, pane, PADDING, 116, "");
        label(tree, pane, PADDING, 156, "Scale");
        let radios = Kind::radios(&SCALES, 0, true);
        let radios_width = radios.content_size().0;
        let scales = add(tree, pane, Place::right(PADDING, 150), radios);
        add(
            tree,
            pane,
            Place::right(PADDING + radios_width + PADDING, 156),
            Kind::label(SCALE_HINT, Token::TextDim),
        );
        let status = label(tree, pane, PADDING, 196, "");
        label(tree, pane, PADDING, ROTATION_TOP, "Rotation");
        let rotation = add(
            tree,
            pane,
            from_right(PADDING, ROTATION_TOP - 6, ROTATION_WIDTH, CONTROL_HEIGHT),
            Kind::dropdown("Rotation", &ROTATION_OPTIONS, 0),
        );
        let rotation_status = label(tree, pane, PADDING, ROTATION_TOP + 40, "");
        label(tree, pane, PADDING, POINTER_TOP, "Pointer");
        add(
            tree,
            pane,
            Place::right(PADDING, POINTER_TOP),
            Kind::label("Software", Token::Text),
        );
        label(tree, pane, PADDING, LAYOUT_TOP, "Layout");
        let layout = add(
            tree,
            pane,
            from_right(PADDING, LAYOUT_TOP - 6, RESOLUTION_WIDTH, CONTROL_HEIGHT),
            Kind::dropdown("Layout", &LAYOUT_OPTIONS, 0),
        );
        let layout_status = label(tree, pane, PADDING, LAYOUT_TOP + 40, "");
        let display = Self {
            layout,
            layout_status,
            resolution,
            choice,
            resolution_status,
            scales,
            status,
            rotation,
            rotation_status,
        };
        display.show(tree, facts);
        display
    }

    /// Shows `facts`: the device's size, the saved scale, the state line.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        let mut size = Text::<96>::new();
        size_text(&mut size, facts.resolution);
        tree.set_text(self.resolution, size.as_str());
        self.show_resolution(tree, facts);
        let next = size_at_next_restart(facts);
        let choosable = facts.on_disk && halvable(facts.resolution) && (facts.scale == 2 || offered(next, 2));
        let shown = match choosable {
            true => facts.scale,
            false => facts.scale_in_force,
        };
        tree.set_selected(self.scales, usize::from(shown == 2));
        tree.set_enabled(self.scales, choosable);
        let (line, caused) = scale_line(facts);
        let ink = match caused {
            true => Token::Text,
            false => Token::TextDim,
        };
        show_status(tree, self.status, Some((line.as_str(), ink)));
        self.show_rotation(tree, facts);
        self.show_layout(tree, facts);
    }

    /// The Rotation row: the saved value and its state line (the design's section 3).
    fn show_rotation(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        tree.set_selected(self.rotation, facts.rotation.min(ROTATION_OPTIONS.len() - 1));
        let line = match (facts.on_disk, facts.rotation) {
            (false, _) => "No disk attached: the rotation lasts until power-off.",
            (_, 0) => "Automatic follows the device's orientation; with no sensor it stays upright.",
            (_, 1) => "The screen is upright.",
            (_, 2) => "The screen is turned 90° clockwise.",
            (_, 3) => "The screen is upside down.",
            _ => "The screen is turned 90° counter-clockwise.",
        };
        show_status(tree, self.rotation_status, Some((line, Token::TextDim)));
    }

    /// How choosing a rotation went: saved, then applied (the design's section 3).
    pub fn rotation_saved(
        &self,
        tree: &mut WidgetTree<CAPACITY>,
        facts: &Facts,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        self.show_rotation(tree, facts);
        let refusal = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => "Not allowed: this user may not turn the screen.",
            (Err(_), _) => "No disk attached: the rotation could not be saved.",
            (Ok(()), Err(Errno::ENOMEM)) => "Saved, but there is not enough memory to turn the screen now.",
            (Ok(()), Err(_)) => "Saved for the next boot, but the window server did not take it.",
            (Ok(()), Ok(())) => return,
        };
        show_status(tree, self.rotation_status, Some((refusal, Token::Text)));
    }

    fn show_layout(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        tree.set_selected(self.layout, facts.form_setting.min(LAYOUT_OPTIONS.len() - 1));
        let shown = crate::facts::FORMS_IN_USE
            .get(facts.form_in_use)
            .copied()
            .unwrap_or("desktop");
        let mut line = Text::<96>::new();
        let _ = write!(line, "Now: {shown}.");
        let ink = match facts.form_setting {
            0 => Token::TextDim,
            setting if crate::facts::FORM_SETTINGS[setting.min(4)] == shown => Token::TextDim,
            _ => Token::Text,
        };
        show_status(tree, self.layout_status, Some((line.as_str(), ink)));
    }

    /// How setting `shell.form` went: the form now shown, or why it did not change (D303).
    pub fn layout_saved(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts, saved: Result<(), Errno>) {
        self.show_layout(tree, facts);
        let refusal = match saved {
            Ok(()) => return,
            Err(Errno::EPERM | Errno::EACCES) => "Not allowed: this user may not set the layout.",
            Err(_) => "The layout could not be saved.",
        };
        show_status(tree, self.layout_status, Some((refusal, Token::Text)));
    }

    fn show_resolution(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        let changeable = facts.resolution_changeable;
        tree.set_shown(self.choice, changeable);
        let size_right = match changeable {
            true => PADDING + RESOLUTION_WIDTH + PADDING,
            false => PADDING,
        };
        tree.set_place(self.resolution, Place::right(size_right, 76));
        let scale = facts.scale.max(1);
        let options = options_for(scale);
        tree.change(self.choice, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.options = options;
            }
        });
        let selected = match facts.resolution_choice {
            Some(size) if offered(size, scale) => sizes_offered(scale).position(|offered| offered == size).map_or(0, |index| index + 1),
            _ => 0,
        };
        tree.set_selected(self.choice, selected);
        tree.set_enabled(self.choice, facts.on_disk);
        let (line, caused) = resolution_line(facts);
        let ink = match caused {
            true => Token::Text,
            false => Token::TextDim,
        };
        show_status(tree, self.resolution_status, Some((line.as_str(), ink)));
    }

    /// How saving the size went: `facts` hold the saved size when it worked.
    pub fn resolution_saved(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts, saved: Result<(), Errno>) {
        self.show(tree, facts);
        let refusal = match saved {
            Ok(()) => return,
            Err(Errno::EPERM | Errno::EACCES) => "Not allowed: this user may not set the resolution.",
            Err(Errno::EINVAL) => "This size cannot be used with the scale chosen below.",
            Err(_) => "No disk attached: the resolution could not be saved.",
        };
        show_status(tree, self.resolution_status, Some((refusal, Token::Text)));
    }

    /// How saving the scale went: `facts` hold the saved scale when it worked.
    pub fn saved(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts, saved: Result<(), Errno>) {
        self.show(tree, facts);
        let refusal = match saved {
            Ok(()) => return,
            Err(Errno::EPERM | Errno::EACCES) => "Not allowed: this user may not set the scale.",
            Err(_) => "No disk attached: the scale could not be saved.",
        };
        show_status(tree, self.status, Some((refusal, Token::Text)));
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
            Response::Chosen(id, choice) if id == self.rotation => {
                Reply::Done((choice != facts.rotation).then_some(Request::SetDisplayRotation(choice)))
            }
            Response::Chosen(id, choice) if id == self.layout => {
                Reply::Done((choice != facts.form_setting).then_some(Request::SetShellForm(choice)))
            }
            Response::Chosen(id, choice) if id == self.choice => {
                let size = match choice {
                    0 => None,
                    index => sizes_offered(facts.scale.max(1)).nth(index - 1),
                };
                Reply::Done((size != facts.resolution_choice).then_some(Request::SetDisplayResolution(size)))
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
        let line = (!facts.on_disk).then_some((NO_DISK, Token::TextDim));
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
        show_status(tree, status, Some((line, Token::Text)));
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
