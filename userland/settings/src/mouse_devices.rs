//! The Mouse pane's acceleration row and each pointing device's own settings (D601,
//! docs/ux/mouse-devices-v0.1.md): the `Settings for` picker, shown with two or more pointing
//! devices or any device's own value; the All devices view keeps the global controls (the
//! [`crate::input_panes::InputPanes`] radios and slider) and the acceleration radios; a device's
//! view shows the same four rows as dropdowns whose first option follows all devices, and a
//! button that puts the whole device back to following.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::input_panes::Reply;
use crate::keys::{DeviceChange, InputChange};
use crate::settings_window::{CAPACITY, Request};
use eggos_device_list::pointers::PointerKind;
use eggos_runtime::errno::Errno;
use eggos_settings_file::{DeviceInputPreferences, MOST_POINTING_DEVICES};
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const ROW: i32 = 40;
const PICKER_ROW: i32 = 64;
const TEXT_IN_ROW: i32 = 12;
const CONTROL_IN_ROW: i32 = 6;
const CONTROL_WIDTH: i32 = 216;
const HINT_RIGHT: i32 = 248;
const STATUS_LINE: i32 = 316;
const TOUCH_CONTROL: i32 = 48;
const NARROW_ROW: i32 = 80;
const NARROW_CONTROL: i32 = 24;
const NARROW_PICKER_SHIFT: i32 = 120;
const FOLLOW: &str = "Same as all devices";
const HELP: &str = "Farther the faster you move it.";
/// The status line with no pointing device attached.
pub const NO_MOUSE: &str = "No mouse attached: these apply when one is.";
const NOT_ATTACHED: &str = "not attached";
const NOT_TAKEN: &str = "Saved for the next boot, but the window server did not take it.";
const ON_OFF: [&str; 2] = ["On", "Off"];
/// The picker's options: `All devices`, then `Device 1` to `Device 16`, a prefix of which shows.
pub const PICKER_OPTIONS: [&str; MOST_POINTING_DEVICES + 1] = [
    "All devices",
    "Device 1",
    "Device 2",
    "Device 3",
    "Device 4",
    "Device 5",
    "Device 6",
    "Device 7",
    "Device 8",
    "Device 9",
    "Device 10",
    "Device 11",
    "Device 12",
    "Device 13",
    "Device 14",
    "Device 15",
    "Device 16",
];
const BUTTONS_OPTIONS: [&str; 3] = [FOLLOW, "Right-handed", "Left-handed"];
const SCROLLING_OPTIONS: [&str; 3] = [FOLLOW, "Traditional", "Natural"];
const SPEED_OPTIONS: [&str; 11] = [FOLLOW, "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
const ACCELERATION_OPTIONS: [&str; 3] = [FOLLOW, "On", "Off"];
const ROW_NAMES: [&str; 4] = ["Buttons", "Scrolling", "Pointer speed", "Acceleration"];

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn text_at(x: i32, y: i32) -> Place {
    Place::at(x, y, Length::Content, Length::Content)
}

fn from_right(margin: i32, top: i32, width: i32, height: i32) -> Place {
    Place {
        horizontal: Horizontal::Right(margin),
        top,
        width: Length::Px(width),
        height: Length::Px(height),
    }
}

fn option_of(value: Option<bool>, on: usize, off: usize) -> usize {
    match value {
        None => 0,
        Some(true) => on,
        Some(false) => off,
    }
}

/// The highest device number the picker lists in `facts`: the highest present, or with a
/// value of its own.
pub fn listed_devices(facts: &Facts) -> usize {
    (1..=MOST_POINTING_DEVICES)
        .rev()
        .find(|&number| {
            facts.pointing_devices.kind(number).is_some()
                || !facts.device_input[number - 1].follows_all()
        })
        .unwrap_or(0)
}

/// Whether the picker shows for `facts`: two or more pointing devices, or a value of one's own.
pub fn picker_shown(facts: &Facts) -> bool {
    facts.pointing_devices.count() >= 2
        || facts.device_input.iter().any(|own| !own.follows_all())
}

/// The global controls this pane hides in a device's view, with their labels: the buttons and
/// scrolling radios and the speed slider, and the Mouse pane's status line.
#[derive(Debug, Clone, Copy)]
pub struct GlobalRows {
    pub labels: [WidgetId; 3],
    pub controls: [WidgetId; 3],
    pub status: WidgetId,
}

/// The Mouse pane's widgets this module adds.
#[derive(Debug)]
pub struct MouseDevices {
    global: GlobalRows,
    picker_label: WidgetId,
    hint: WidgetId,
    picker: WidgetId,
    acceleration_label: WidgetId,
    help: WidgetId,
    acceleration: WidgetId,
    acceleration_width: i32,
    rows: [WidgetId; 4],
    follow: WidgetId,
    picked: usize,
    with_picker: bool,
    content: Option<i32>,
}

impl MouseDevices {
    /// Builds the rows in the Mouse pane `mouse`, beside `global`'s, showing `facts`.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        mouse: WidgetId,
        global: GlobalRows,
        facts: &Facts,
    ) -> Self {
        let picker_label = add(
            tree,
            mouse,
            text_at(PADDING, PICKER_ROW + TEXT_IN_ROW),
            Kind::label("Settings for", Token::Text),
        );
        let hint = add(
            tree,
            mouse,
            Place::right(HINT_RIGHT, PICKER_ROW + TEXT_IN_ROW),
            Kind::label("", Token::TextDim),
        );
        let picker = add(
            tree,
            mouse,
            from_right(PADDING, PICKER_ROW + CONTROL_IN_ROW, CONTROL_WIDTH, CONTROL_HEIGHT),
            Kind::dropdown("Settings for", &PICKER_OPTIONS[..1], 0),
        );
        let acceleration_label = add(
            tree,
            mouse,
            text_at(PADDING, PICKER_ROW),
            Kind::label("Acceleration", Token::Text),
        );
        let radios = Kind::radios(&ON_OFF, 0, true);
        let acceleration_width = radios.content_size().0;
        let acceleration = add(tree, mouse, Place::right(PADDING, PICKER_ROW), radios);
        let help = add(
            tree,
            mouse,
            Place::right(PADDING, PICKER_ROW),
            Kind::label(HELP, Token::TextDim),
        );
        let options: [&'static [&'static str]; 4] = [
            &BUTTONS_OPTIONS,
            &SCROLLING_OPTIONS,
            &SPEED_OPTIONS,
            &ACCELERATION_OPTIONS,
        ];
        let rows = core::array::from_fn(|index| {
            add(
                tree,
                mouse,
                from_right(PADDING, PICKER_ROW, CONTROL_WIDTH, CONTROL_HEIGHT),
                Kind::dropdown(ROW_NAMES[index], options[index], 0),
            )
        });
        let follow = add(
            tree,
            mouse,
            Place::right(PADDING, PICKER_ROW),
            Kind::button(FOLLOW, false),
        );
        let mut devices = Self {
            global,
            picker_label,
            hint,
            picker,
            acceleration_label,
            help,
            acceleration,
            acceleration_width,
            rows,
            follow,
            picked: 0,
            with_picker: false,
            content: None,
        };
        devices.show(tree, facts);
        devices
    }

    /// The device whose view shows, from 1; 0 for All devices.
    pub fn picked(&self) -> usize {
        self.picked
    }

    /// Shows `facts`: the picker's devices, the view picked, its controls' values, the rows
    /// placed again; returns the pane's new height when the picker appeared or went.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) -> Option<i32> {
        let listed = listed_devices(facts);
        if self.picked > listed {
            self.picked = 0;
        }
        tree.change(self.picker, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.options = &PICKER_OPTIONS[..listed + 1];
            }
        });
        let with_picker = picker_shown(facts);
        if !with_picker {
            self.picked = 0;
        }
        tree.set_selected(self.picker, self.picked);
        for id in [self.picker_label, self.picker] {
            tree.set_shown(id, with_picker);
        }
        tree.set_selected(
            self.acceleration,
            usize::from(!facts.input.pointer_acceleration),
        );
        let device_view = self.picked > 0;
        for id in self.global.controls.into_iter().chain([self.acceleration]) {
            tree.set_shown(id, !device_view);
        }
        for id in self.rows.into_iter().chain([self.follow, self.hint]) {
            tree.set_shown(id, device_view);
        }
        if device_view {
            let kind = facts.pointing_devices.kind(self.picked);
            show_line(
                tree,
                self.hint,
                kind.map_or(NOT_ATTACHED, PointerKind::hint),
                Token::TextDim,
            );
            let own = facts.device_input[self.picked - 1];
            tree.set_selected(self.rows[0], option_of(own.left_handed, 2, 1));
            tree.set_selected(self.rows[1], option_of(own.natural_scrolling, 2, 1));
            tree.set_selected(self.rows[2], own.pointer_speed.map_or(0, |speed| speed as usize));
            tree.set_selected(self.rows[3], option_of(own.acceleration, 1, 2));
            let scaled = kind.is_none_or(PointerKind::has_speed);
            tree.set_enabled(self.rows[2], scaled);
            tree.set_enabled(self.rows[3], scaled);
            tree.set_enabled(self.follow, !own.follows_all());
        }
        let moved = with_picker != self.with_picker;
        self.with_picker = with_picker;
        let height = self.reflow(tree, self.content);
        moved.then_some(height)
    }

    /// Shows the pane afresh at `facts`; the status line says when no pointing device is
    /// attached, before the no-disk line.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) -> Option<i32> {
        let moved = self.show(tree, facts);
        if facts.pointing_devices.count() == 0 {
            show_line(tree, self.global.status, NO_MOUSE, Token::TextDim);
            tree.set_shown(self.global.status, true);
        }
        moved
    }

    /// Places every row of the Mouse pane for the desktop (`None`) or one column `content`
    /// wide on a narrow page; returns the pane's height.
    pub fn reflow(&mut self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) -> i32 {
        self.content = content;
        let [buttons, scrolling, speed] = self.global.labels;
        let labels = [buttons, scrolling, speed, self.acceleration_label];
        let globals = [
            self.global.controls[0],
            self.global.controls[1],
            self.global.controls[2],
            self.acceleration,
        ];
        let Some(c) = content else {
            let first = match self.with_picker {
                true => PICKER_ROW + ROW,
                false => PICKER_ROW,
            };
            tree.set_place(self.picker_label, text_at(PADDING, PICKER_ROW + TEXT_IN_ROW));
            tree.set_place(self.hint, Place::right(HINT_RIGHT, PICKER_ROW + TEXT_IN_ROW));
            tree.set_place(
                self.picker,
                from_right(PADDING, PICKER_ROW + CONTROL_IN_ROW, CONTROL_WIDTH, CONTROL_HEIGHT),
            );
            for index in 0..4 {
                let row = first + ROW * index as i32;
                tree.set_place(labels[index], text_at(PADDING, row + TEXT_IN_ROW));
                let global = match index {
                    2 => from_right(PADDING, row + CONTROL_IN_ROW, CONTROL_WIDTH, CONTROL_HEIGHT),
                    _ => Place::right(PADDING, row + CONTROL_IN_ROW),
                };
                tree.set_place(globals[index], global);
                tree.set_place(
                    self.rows[index],
                    from_right(PADDING, row + CONTROL_IN_ROW, CONTROL_WIDTH, CONTROL_HEIGHT),
                );
            }
            let acceleration_row = first + 3 * ROW;
            let help_right = match self.picked {
                0 => PADDING + self.acceleration_width + PADDING,
                _ => HINT_RIGHT,
            };
            tree.set_place(
                self.help,
                Place::right(help_right, acceleration_row + TEXT_IN_ROW),
            );
            tree.set_place(
                self.follow,
                Place::right(PADDING, first + 4 * ROW + CONTROL_IN_ROW),
            );
            tree.set_place(self.global.status, text_at(PADDING, STATUS_LINE));
            return STATUS_LINE + 2 * PADDING;
        };
        let shift = match self.with_picker {
            true => 0,
            false => NARROW_PICKER_SHIFT,
        };
        let wide = |y: i32| Place::at(PADDING, y, Length::Px(c), Length::Px(TOUCH_CONTROL));
        tree.set_place(self.picker_label, text_at(PADDING, PICKER_ROW));
        tree.set_place(self.picker, wide(PICKER_ROW + NARROW_CONTROL));
        tree.set_place(self.hint, text_at(PADDING, 148));
        for index in 0..4 {
            let row = 184 + NARROW_ROW * index as i32 - shift;
            tree.set_place(labels[index], text_at(PADDING, row));
            let global = match index {
                2 => wide(row + NARROW_CONTROL),
                _ => text_at(PADDING, row + NARROW_CONTROL),
            };
            tree.set_place(globals[index], global);
            tree.set_place(self.rows[index], wide(row + NARROW_CONTROL));
        }
        tree.set_place(self.help, text_at(PADDING, 508 - shift));
        tree.set_place(self.follow, wide(544 - shift));
        let status = match self.with_picker {
            true => 608,
            false => 424,
        };
        tree.set_place(self.global.status, text_at(PADDING, status));
        status + 2 * PADDING
    }

    /// How saving, then applying, a device change went; `facts` hold what is in force after.
    pub fn changed(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        facts: &Facts,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) -> Option<i32> {
        let moved = self.show(tree, facts);
        let line = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => {
                "Not allowed: this user may not change the mouse settings."
            }
            (Err(_), _) => "No disk attached: the mouse settings could not be saved.",
            (Ok(()), Err(_)) => NOT_TAKEN,
            (Ok(()), Ok(())) => return moved,
        };
        show_line(tree, self.global.status, line, Token::Text);
        tree.set_shown(self.global.status, true);
        moved
    }

    /// Acts on a response with `facts` in force: the picker switches the view and writes
    /// nothing (the caller then reflows); the rest ask for a change.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        facts: &Facts,
    ) -> Reply {
        match response {
            Response::Chosen(id, choice) if id == self.picker => {
                self.picked = choice.min(MOST_POINTING_DEVICES);
                self.show(tree, facts);
                Reply::Done(None)
            }
            Response::Chosen(id, choice) if id == self.acceleration => {
                let on = choice == 0;
                Reply::Done(
                    (on != facts.input.pointer_acceleration)
                        .then_some(Request::ChangeInput(InputChange::PointerAcceleration(on))),
                )
            }
            Response::Activated(id) if id == self.follow && self.picked > 0 => {
                self.device_request(DeviceChange::FollowAll, facts)
            }
            Response::Chosen(id, choice) if self.picked > 0 && self.rows.contains(&id) => {
                let choice_of = |on: usize| match choice {
                    0 => None,
                    _ => Some(choice == on),
                };
                let change = match self.rows.iter().position(|&row| row == id) {
                    Some(0) => DeviceChange::LeftHanded(choice_of(2)),
                    Some(1) => DeviceChange::NaturalScrolling(choice_of(2)),
                    Some(2) => DeviceChange::Speed((choice > 0).then_some(choice as u32)),
                    _ => DeviceChange::Acceleration(choice_of(1)),
                };
                self.device_request(change, facts)
            }
            _ => Reply::NotMine,
        }
    }

    fn device_request(&self, change: DeviceChange, facts: &Facts) -> Reply {
        let own: DeviceInputPreferences = facts.device_input[self.picked - 1];
        Reply::Done((change.applied_to(own) != own).then_some(Request::ChangeDevice {
            device: self.picked as u8,
            change,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_device_list::pointers::PointingDevices;

    #[test]
    fn the_picker_shows_with_two_devices_or_a_value_of_one_s_own() {
        let mut facts = Facts::example();
        assert!(!picker_shown(&facts));
        assert_eq!(listed_devices(&facts), 0);
        facts.pointing_devices = PointingDevices::parse(b"mouse\n");
        assert!(!picker_shown(&facts));
        assert_eq!(listed_devices(&facts), 1);
        facts.pointing_devices = PointingDevices::parse(b"mouse\ntablet\n");
        assert!(picker_shown(&facts));
        assert_eq!(listed_devices(&facts), 2);
        facts.pointing_devices = PointingDevices::parse(b"mouse\n");
        facts.device_input[2].pointer_speed = Some(7);
        assert!(picker_shown(&facts));
        assert_eq!(listed_devices(&facts), 3);
    }

    #[test]
    fn the_options_follow_first_then_the_global_words() {
        assert_eq!(option_of(None, 2, 1), 0);
        assert_eq!(BUTTONS_OPTIONS[option_of(Some(true), 2, 1)], "Left-handed");
        assert_eq!(SCROLLING_OPTIONS[option_of(Some(false), 2, 1)], "Traditional");
        assert_eq!(ACCELERATION_OPTIONS[option_of(Some(false), 1, 2)], "Off");
        assert_eq!(PICKER_OPTIONS[2], "Device 2");
    }
}
