//! The Sound pane (D775, docs/ux/sound-v0.1.md section 4.1): the output and its volume and
//! mute, a test tone, the input, its volume and mute, and its level. Each control's change is
//! one [`Request::SetSound`], which the program sends to the sound server; the controls follow
//! what the server says ([`SoundPane::show`]) when the pane is shown and after each change.

use crate::agents_pane::show_line;
use crate::settings_window::{CAPACITY, Request};
use alloc::string::String;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Kind, Length, Place};

/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 16;
/// The pane's height.
pub const SOUND_PANE_HEIGHT: i32 = 428;
/// What the status line says without a sound device.
pub const NO_SOUND_DEVICE: &str = "No sound device.";
/// The test tone's line.
pub const TONE_LINE: &str = "Plays a 1 s tone on the output above.";

const PADDING: i32 = 16;
const DROPDOWN: (i32, i32) = (288, 160);
const SLIDER: (i32, i32) = (232, 216);
const OUTPUT_ROW: i32 = 70;
const VOLUME_ROW: i32 = 110;
const MUTE_ROW: i32 = 150;
const TEST_ROW: i32 = 190;
const INPUT_ROW: i32 = 230;
const INPUT_VOLUME_ROW: i32 = 270;
const INPUT_MUTE_ROW: i32 = 310;
const LEVEL_ROW: i32 = 356;
const LEVEL: (i32, i32, i32) = (232, 360, 8);
const STATUS_ROW: i32 = 396;
const LABEL_DROP: i32 = 6;
const TEST_BUTTON_WIDTH: i32 = 102;
const AUTOMATIC: &str = "Automatic";
const KNOWN_OUTPUTS: [[&str; 2]; 4] = [
    [AUTOMATIC, "Built-in speakers"],
    [AUTOMATIC, "Headphones"],
    [AUTOMATIC, "Line out"],
    [AUTOMATIC, "virtio sound"],
];
const ONLY_AUTOMATIC: [&str; 1] = [AUTOMATIC];

/// One change a control asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundChange {
    Volume(u8),
    Mute(bool),
    Output(String),
    Input(String),
    InputVolume(u8),
    InputMute(bool),
    Test,
}

/// What the sound server said: its state and the output's id and name; `None` without a
/// server (no sound device).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundRead {
    pub output: Option<(String, String)>,
    pub input: Option<(String, String)>,
    pub volume: u8,
    pub mute: bool,
    pub input_volume: u8,
    pub input_mute: bool,
    pub automatic_output: bool,
}

/// The pane's widgets.
#[derive(Debug, Clone)]
pub struct SoundPane {
    output: WidgetId,
    volume: WidgetId,
    mute: WidgetId,
    test: WidgetId,
    tone_line: WidgetId,
    input: WidgetId,
    input_volume: WidgetId,
    input_mute: WidgetId,
    level: WidgetId,
    status: WidgetId,
    output_id: Option<String>,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn label(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, row: i32, words: &str) -> WidgetId {
    add(
        tree,
        pane,
        Place::at(PADDING, row + LABEL_DROP, Length::Content, Length::Content),
        Kind::label(words, Token::Text),
    )
}

fn control(x: i32, row: i32, width: i32) -> Place {
    Place::at(x, row, Length::Px(width), Length::Px(CONTROL_HEIGHT))
}

fn output_options(name: &str) -> &'static [&'static str] {
    KNOWN_OUTPUTS
        .iter()
        .find(|options| options[1] == name)
        .map_or(&ONLY_AUTOMATIC[..], |options| &options[..])
}

impl SoundPane {
    /// Builds the pane in `pane`, showing no device yet.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, PADDING, Length::Content, Length::Content),
            Kind::heading("Sound"),
        );
        label(tree, pane, OUTPUT_ROW, "Output");
        let output = add(
            tree,
            pane,
            control(DROPDOWN.0, OUTPUT_ROW, DROPDOWN.1),
            Kind::dropdown("Output", &ONLY_AUTOMATIC, 0),
        );
        label(tree, pane, VOLUME_ROW, "Volume");
        let volume = add(
            tree,
            pane,
            control(SLIDER.0, VOLUME_ROW, SLIDER.1),
            Kind::slider("Volume", 0, 100, 1, 50, "%"),
        );
        let mute = add(
            tree,
            pane,
            Place::at(PADDING, MUTE_ROW, Length::Content, Length::Px(CONTROL_HEIGHT)),
            Kind::checkbox("Mute", false),
        );
        let test = add(
            tree,
            pane,
            control(PADDING, TEST_ROW, TEST_BUTTON_WIDTH),
            Kind::button("Play test tone", false),
        );
        let tone_line = add(
            tree,
            pane,
            Place::right(PADDING, TEST_ROW + LABEL_DROP),
            Kind::label(TONE_LINE, Token::TextDim),
        );
        label(tree, pane, INPUT_ROW, "Input");
        let input = add(
            tree,
            pane,
            control(DROPDOWN.0, INPUT_ROW, DROPDOWN.1),
            Kind::dropdown("Input", &ONLY_AUTOMATIC, 0),
        );
        label(tree, pane, INPUT_VOLUME_ROW, "Input volume");
        let input_volume = add(
            tree,
            pane,
            control(SLIDER.0, INPUT_VOLUME_ROW, SLIDER.1),
            Kind::slider("Input volume", 0, 100, 1, 50, "%"),
        );
        let input_mute = add(
            tree,
            pane,
            Place::at(
                PADDING,
                INPUT_MUTE_ROW,
                Length::Content,
                Length::Px(CONTROL_HEIGHT),
            ),
            Kind::checkbox("Mute microphone", false),
        );
        label(tree, pane, LEVEL_ROW - LABEL_DROP, "Level");
        let level = add(
            tree,
            pane,
            Place::at(LEVEL.0, LEVEL.1, Length::Px(SLIDER.1), Length::Px(LEVEL.2)),
            Kind::Progress { permille: 0 },
        );
        tree.set_accessible_name(level, "Level");
        let status = add(
            tree,
            pane,
            Place::at(PADDING, STATUS_ROW, Length::Content, Length::Content),
            Kind::label(NO_SOUND_DEVICE, Token::TextDim),
        );
        let made = Self {
            output,
            volume,
            mute,
            test,
            tone_line,
            input,
            input_volume,
            input_mute,
            level,
            status,
            output_id: None,
        };
        made.show(tree, None);
        made
    }

    fn controls(&self) -> [WidgetId; 8] {
        [
            self.output,
            self.volume,
            self.mute,
            self.test,
            self.input,
            self.input_volume,
            self.input_mute,
            self.level,
        ]
    }

    /// Shows what the sound server said, `None` without a sound device: the controls then
    /// disabled under `No sound device.`.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, read: Option<&SoundRead>) {
        let Some(read) = read.filter(|read| read.output.is_some()) else {
            for id in self.controls() {
                tree.set_enabled(id, false);
            }
            show_line(tree, self.status, NO_SOUND_DEVICE, Token::TextDim);
            tree.set_shown(self.status, true);
            return;
        };
        for id in self.controls() {
            tree.set_enabled(id, true);
        }
        tree.set_shown(self.status, false);
        let name = read.output.as_ref().map_or("", |(_, name)| name.as_str());
        let options = output_options(name);
        let selected = usize::from(!read.automatic_output && options.len() > 1);
        tree.change(self.output, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.options = options;
                dropdown.selected = selected;
                dropdown.highlighted = selected;
            }
        });
        tree.set_value(self.volume, i32::from(read.volume));
        tree.set_selected(self.mute, usize::from(read.mute));
        tree.set_enabled(self.input, read.input.is_some());
        tree.set_enabled(self.input_volume, read.input.is_some());
        tree.set_enabled(self.input_mute, read.input.is_some());
        tree.set_value(self.input_volume, i32::from(read.input_volume));
        tree.set_selected(self.input_mute, usize::from(read.input_mute));
        tree.set_progress(self.level, 0);
        tree.set_shown(self.tone_line, true);
    }

    /// Remembers the output's id, which the dropdown's second option chooses.
    pub fn remember_output(&mut self, read: Option<&SoundRead>) {
        self.output_id = read
            .and_then(|read| read.output.as_ref())
            .map(|(id, _)| id.clone());
    }

    /// The change `response` asks for, if it is one of the pane's controls.
    pub fn respond(&self, response: Response) -> Option<Request> {
        let change = match response {
            Response::Chosen(id, option) if id == self.output => {
                let id = match option {
                    0 => None,
                    _ => self.output_id.clone(),
                };
                SoundChange::Output(id.unwrap_or_else(|| String::from("auto")))
            }
            Response::Chosen(id, _) if id == self.input => SoundChange::Input(String::from("auto")),
            Response::Adjusted(id, value) if id == self.volume => {
                SoundChange::Volume(value.clamp(0, 100) as u8)
            }
            Response::Adjusted(id, value) if id == self.input_volume => {
                SoundChange::InputVolume(value.clamp(0, 100) as u8)
            }
            Response::Toggled(id, on) if id == self.mute => SoundChange::Mute(on),
            Response::Toggled(id, on) if id == self.input_mute => SoundChange::InputMute(on),
            Response::Activated(id) if id == self.test => SoundChange::Test,
            _ => return None,
        };
        Some(Request::SetSound(change))
    }

    /// The volume slider, for tests and the element tree.
    pub fn volume_slider(&self) -> WidgetId {
        self.volume
    }

    /// The status line.
    pub fn status_line(&self) -> WidgetId {
        self.status
    }

    /// The test tone's button.
    pub fn test_button(&self) -> WidgetId {
        self.test
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::Facts;
    use crate::settings_window::{SOUND, SettingsWindow};

    fn read() -> SoundRead {
        SoundRead {
            output: Some((String::from("hda0-out"), String::from("Built-in speakers"))),
            input: None,
            volume: 40,
            mute: false,
            input_volume: 50,
            input_mute: false,
            automatic_output: true,
        }
    }

    #[test]
    fn without_a_device_the_pane_is_disabled_and_with_one_it_follows_the_server() {
        let mut window = SettingsWindow::new(Facts::example());
        let _ = window.open_pane(SOUND);
        let pane = window.sound_pane().clone();
        assert!(!window.widgets().is_enabled(pane.volume_slider()));
        assert_eq!(
            window.widgets().text_of(pane.status_line()),
            Some(NO_SOUND_DEVICE)
        );
        window.show_sound(Some(read()));
        assert!(window.widgets().is_enabled(pane.volume_slider()));
        assert!(!window.widgets().is_shown(pane.status_line()));
        assert_eq!(
            window.respond(Response::Adjusted(pane.volume_slider(), 55), 0),
            Some(Request::SetSound(SoundChange::Volume(55)))
        );
        assert_eq!(
            window.respond(Response::Activated(pane.test_button()), 0),
            Some(Request::SetSound(SoundChange::Test))
        );
        assert_eq!(output_options("Headphones"), &[AUTOMATIC, "Headphones"]);
        assert_eq!(output_options("Something"), &[AUTOMATIC]);
    }
}
