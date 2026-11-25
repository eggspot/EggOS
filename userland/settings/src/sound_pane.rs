//! The Sound pane (D775, D777, docs/ux/sound-v0.1.md section 4.1 and sound-v0.2.md section
//! 3): the output and its volume and mute, a test tone, the alert sound and its volume, each
//! app playing with its own mute and volume (eight at most), the input, its volume and mute,
//! and its level. Each control's change is
//! one [`Request::SetSound`], which the program sends to the sound server; the controls follow
//! what the server says ([`SoundPane::show`]) when the pane is shown and after each change.

use crate::agents_pane::show_line;
use crate::settings_window::{CAPACITY, Request};
use alloc::string::String;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Kind, Length, Place};

/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 16 + 6 + 3 * APP_ROWS;
/// The pane's height with no app playing.
pub const SOUND_PANE_HEIGHT: i32 = 548;
/// The app rows the pane shows at most.
pub const APP_ROWS: usize = 8;
/// The alert sounds' titles, in `sound.alert`'s order of names.
pub const ALERT_TITLES: [&str; 4] = ["Chime", "Pop", "Glass", "None"];
const ALERT_NAMES: [&str; 4] = ["chime", "pop", "glass", "none"];
const ALERT_ROW: i32 = 230;
const ALERT_VOLUME_ROW: i32 = 270;
const APPS_ROW: i32 = 316;
const APP_FIRST: i32 = 350;
const APP_PITCH: i32 = 40;
const APP_NAME: (i32, i32) = (16, 168);
const APP_MUTE_X: i32 = 196;
const INPUT_BASE: i32 = 236;
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
    Alert(String),
    AlertVolume(u8),
    AppVolume(String, u8),
    AppMute(String, bool),
}

/// An app playing sound, as the pane shows it: its id, name, volume and mute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppRead {
    pub id: String,
    pub name: String,
    pub volume: u8,
    pub mute: bool,
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
    pub alert: String,
    pub alert_volume: u8,
    pub apps: alloc::vec::Vec<AppRead>,
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
    alert: WidgetId,
    alert_volume: WidgetId,
    apps_line: WidgetId,
    more_apps: WidgetId,
    app_rows: [(WidgetId, WidgetId, WidgetId); APP_ROWS],
    app_ids: alloc::vec::Vec<String>,
    input_block: [(WidgetId, i32); 8],
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
        label(tree, pane, ALERT_ROW, "Alert sound");
        let alert = add(
            tree,
            pane,
            control(DROPDOWN.0, ALERT_ROW, DROPDOWN.1),
            Kind::dropdown("Alert sound", &ALERT_TITLES, 0),
        );
        label(tree, pane, ALERT_VOLUME_ROW, "Alert volume");
        let alert_volume = add(
            tree,
            pane,
            control(SLIDER.0, ALERT_VOLUME_ROW, SLIDER.1),
            Kind::slider("Alert volume", 0, 100, 1, 70, "%"),
        );
        let apps_line = add(
            tree,
            pane,
            Place::at(PADDING, APPS_ROW, Length::Content, Length::Content),
            Kind::label("Apps", Token::Text),
        );
        let app_rows: [(WidgetId, WidgetId, WidgetId); APP_ROWS] = core::array::from_fn(|row| {
            let top = APP_FIRST + APP_PITCH * row as i32;
            let name = add(
                tree,
                pane,
                Place::at(APP_NAME.0, top + LABEL_DROP, Length::Px(APP_NAME.1), Length::Content),
                Kind::label("", Token::Text),
            );
            let mute = add(
                tree,
                pane,
                Place::at(APP_MUTE_X, top, Length::Content, Length::Px(CONTROL_HEIGHT)),
                Kind::checkbox("", false),
            );
            let volume = add(
                tree,
                pane,
                control(SLIDER.0, top, SLIDER.1),
                Kind::slider("App volume", 0, 100, 1, 100, "%"),
            );
            (name, mute, volume)
        });
        let more_apps = add(
            tree,
            pane,
            Place::at(PADDING, APP_FIRST, Length::Content, Length::Content),
            Kind::label("", Token::TextDim),
        );
        let input_label = label(tree, pane, INPUT_ROW, "Input");
        let input = add(
            tree,
            pane,
            control(DROPDOWN.0, INPUT_ROW, DROPDOWN.1),
            Kind::dropdown("Input", &ONLY_AUTOMATIC, 0),
        );
        let input_volume_label = label(tree, pane, INPUT_VOLUME_ROW, "Input volume");
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
        let level_label = label(tree, pane, LEVEL_ROW - LABEL_DROP, "Level");
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
        let mut made = Self {
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
            alert,
            alert_volume,
            apps_line,
            more_apps,
            app_rows,
            app_ids: alloc::vec::Vec::new(),
            input_block: [
                (input_label, INPUT_ROW + LABEL_DROP),
                (input, INPUT_ROW),
                (input_volume_label, INPUT_VOLUME_ROW + LABEL_DROP),
                (input_volume, INPUT_VOLUME_ROW),
                (input_mute, INPUT_MUTE_ROW),
                (level_label, LEVEL_ROW),
                (level, LEVEL.1),
                (status, STATUS_ROW),
            ],
        };
        made.show(tree, None);
        made
    }

    fn place_input_block(&self, tree: &mut WidgetTree<CAPACITY>, shift: i32) {
        for (index, &(id, top)) in self.input_block.iter().enumerate() {
            let place = match index {
                1 => control(DROPDOWN.0, top + shift, DROPDOWN.1),
                3 => control(SLIDER.0, top + shift, SLIDER.1),
                4 => Place::at(PADDING, top + shift, Length::Content, Length::Px(CONTROL_HEIGHT)),
                6 => Place::at(LEVEL.0, top + shift, Length::Px(SLIDER.1), Length::Px(LEVEL.2)),
                _ => Place::at(PADDING, top + shift, Length::Content, Length::Content),
            };
            tree.set_place(id, place);
        }
    }

    fn show_apps(&mut self, tree: &mut WidgetTree<CAPACITY>, apps: &[AppRead]) -> i32 {
        let shown = apps.len().min(APP_ROWS);
        self.app_ids = apps[..shown].iter().map(|app| app.id.clone()).collect();
        let words = if apps.is_empty() { "No app is playing." } else { "Apps" };
        let ink = if apps.is_empty() { Token::TextDim } else { Token::Text };
        show_line(tree, self.apps_line, words, ink);
        for (row, &(name, mute, volume)) in self.app_rows.iter().enumerate() {
            let app = apps.get(row).filter(|_| row < shown);
            for id in [name, mute, volume] {
                tree.set_shown(id, app.is_some());
            }
            let Some(app) = app else {
                continue;
            };
            tree.set_text(name, &app.name);
            tree.set_selected(mute, usize::from(app.mute));
            tree.set_value(volume, i32::from(app.volume));
            tree.set_accessible_name(mute, &alloc::format!("Mute {}", app.name));
            tree.set_accessible_name(volume, &alloc::format!("{} volume", app.name));
        }
        let rows = shown.max(1) as i32;
        let more = apps.len().saturating_sub(APP_ROWS);
        tree.set_shown(self.more_apps, more > 0);
        if more > 0 {
            let line = alloc::format!("and {more} more in sound apps");
            tree.set_text(self.more_apps, &line);
            let top = APP_FIRST + APP_PITCH * APP_ROWS as i32 + LABEL_DROP;
            tree.set_place(
                self.more_apps,
                Place::at(PADDING, top, Length::Content, Length::Content),
            );
        }
        let extra = i32::from(more > 0) * APP_PITCH;
        let shift = APP_FIRST + APP_PITCH * rows + extra - INPUT_BASE + LABEL_DROP;
        self.place_input_block(tree, shift);
        SOUND_PANE_HEIGHT - APP_PITCH + APP_PITCH * rows + extra
    }

    fn controls(&self) -> [WidgetId; 10] {
        [
            self.output,
            self.volume,
            self.mute,
            self.test,
            self.alert,
            self.alert_volume,
            self.input,
            self.input_volume,
            self.input_mute,
            self.level,
        ]
    }

    /// Shows what the sound server said, `None` without a sound device: the controls then
    /// disabled under `No sound device.`; the pane's height.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, read: Option<&SoundRead>) -> i32 {
        let Some(read) = read.filter(|read| read.output.is_some()) else {
            for id in self.controls() {
                tree.set_enabled(id, false);
            }
            let height = self.show_apps(tree, &[]);
            show_line(tree, self.status, NO_SOUND_DEVICE, Token::TextDim);
            tree.set_shown(self.status, true);
            return height;
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
        let alert = ALERT_NAMES
            .iter()
            .position(|name| *name == read.alert)
            .unwrap_or(0);
        tree.set_selected(self.alert, alert);
        tree.set_value(self.alert_volume, i32::from(read.alert_volume));
        self.show_apps(tree, &read.apps)
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
            Response::Chosen(id, option) if id == self.alert => {
                SoundChange::Alert(String::from(*ALERT_NAMES.get(option)?))
            }
            Response::Adjusted(id, value) if id == self.alert_volume => {
                SoundChange::AlertVolume(value.clamp(0, 100) as u8)
            }
            Response::Toggled(id, on) => {
                let row = self.app_rows.iter().position(|&(_, mute, _)| mute == id)?;
                SoundChange::AppMute(self.app_ids.get(row)?.clone(), on)
            }
            Response::Adjusted(id, value) => {
                let row = self.app_rows.iter().position(|&(.., volume)| volume == id)?;
                SoundChange::AppVolume(self.app_ids.get(row)?.clone(), value.clamp(0, 100) as u8)
            }
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
            alert: String::from("chime"),
            alert_volume: 70,
            apps: alloc::vec![
                AppRead {
                    id: String::from("files"),
                    name: String::from("Files"),
                    volume: 100,
                    mute: false,
                },
                AppRead {
                    id: String::from("windows-apps:notepad"),
                    name: String::from("Notepad"),
                    volume: 60,
                    mute: true,
                },
            ],
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
        let pane = window.sound_pane().clone();
        let (_, notepad_mute, _) = pane.app_rows[1];
        assert_eq!(
            window.respond(Response::Toggled(notepad_mute, false), 0),
            Some(Request::SetSound(SoundChange::AppMute(
                String::from("windows-apps:notepad"),
                false
            )))
        );
        assert_eq!(
            window.respond(Response::Chosen(pane.alert, 1), 0),
            Some(Request::SetSound(SoundChange::Alert(String::from("pop"))))
        );
        let (input_label, _) = pane.input_block[0];
        let input = window.widgets().rect(input_label).unwrap();
        let heading = window.widgets().rect(pane.apps_line).unwrap();
        assert_eq!(input.y - heading.y, 436 - 316);
        assert_eq!(output_options("Headphones"), &[AUTOMATIC, "Headphones"]);
        assert_eq!(output_options("Something"), &[AUTOMATIC]);
    }
}
