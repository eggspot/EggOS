//! What clients and the sound server say to each other on `/run/soundd`, one line each:
//!
//! - `get` → `state <output> <volume> <mute> <input> <input volume> <input mute>`, a missing
//!   device `none`;
//! - `devices` → one `device <id> <output|input> <name>` line a device, then `end`;
//! - `set volume <0-100>`, `set mute <true|false>`, `set output <id|auto>`, `set input
//!   <id|auto>`, `set input_volume <0-100>`, `set input_mute <true|false>` → `ok` or `error
//!   <code>`;
//! - `test <milliseconds>` → `ok` once the tone is queued;
//! - `level` → `level <0-100>`, the input's peak;
//! - `watch` → a `state` line now and on every change, until the client goes;
//! - `play <rate> <channels> <app>` → `ok`, then the connection carries the stream's
//!   interleaved 16-bit little-endian samples until the client closes it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The socket's path.
pub const SOCKET_PATH: &str = "/run/soundd";
/// The test tone's pitch.
pub const TEST_TONE_HZ: u32 = 440;
/// The test tone's default length.
pub const TEST_TONE_MS: u32 = 1000;
/// The longest test tone.
pub const MOST_TEST_MS: u32 = 10_000;

/// One change to the sound settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Volume(u8),
    Mute(bool),
    Output(String),
    Input(String),
    InputVolume(u8),
    InputMute(bool),
}

impl Change {
    /// Its settings key and value, as the settings file holds them.
    pub fn setting(&self) -> (&'static str, String) {
        use eggos_settings_file as keys;
        match self {
            Change::Volume(volume) => (keys::SOUND_VOLUME, format!("{volume}")),
            Change::Mute(mute) => (keys::SOUND_MUTE, format!("{mute}")),
            Change::Output(id) => (keys::SOUND_OUTPUT, id.clone()),
            Change::Input(id) => (keys::SOUND_INPUT, id.clone()),
            Change::InputVolume(volume) => (keys::SOUND_INPUT_VOLUME, format!("{volume}")),
            Change::InputMute(mute) => (keys::SOUND_INPUT_MUTE, format!("{mute}")),
        }
    }

    /// The change a `settings set <key> <value>` asks for, if the key is a sound key and the
    /// value fits it.
    pub fn of_setting(key: &str, value: &str) -> Option<Self> {
        use eggos_settings_file as keys;
        let percent = || value.parse::<u8>().ok().filter(|&volume| volume <= 100);
        let flag = || match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        let device = || {
            (!value.is_empty() && !value.contains(char::is_whitespace)).then(|| String::from(value))
        };
        Some(match key {
            keys::SOUND_VOLUME => Change::Volume(percent()?),
            keys::SOUND_MUTE => Change::Mute(flag()?),
            keys::SOUND_OUTPUT => Change::Output(device()?),
            keys::SOUND_INPUT => Change::Input(device()?),
            keys::SOUND_INPUT_VOLUME => Change::InputVolume(percent()?),
            keys::SOUND_INPUT_MUTE => Change::InputMute(flag()?),
            _ => return None,
        })
    }
}

/// A request a client makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Get,
    Devices,
    Set(Change),
    Test { milliseconds: u32 },
    Level,
    Watch,
    Play { rate: u32, channels: u8, app: String },
}

impl Request {
    /// Its line, without the newline.
    pub fn line(&self) -> String {
        match self {
            Request::Get => String::from("get"),
            Request::Devices => String::from("devices"),
            Request::Level => String::from("level"),
            Request::Watch => String::from("watch"),
            Request::Test { milliseconds } => format!("test {milliseconds}"),
            Request::Play {
                rate,
                channels,
                app,
            } => format!("play {rate} {channels} {app}"),
            Request::Set(change) => {
                let (key, value) = change.setting();
                let name = key.strip_prefix("sound.").unwrap_or(key);
                format!("set {name} {value}")
            }
        }
    }

    /// The request `line` makes, if it is one.
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let verb = words.next()?;
        let request = match verb {
            "get" => Request::Get,
            "devices" => Request::Devices,
            "level" => Request::Level,
            "watch" => Request::Watch,
            "test" => {
                let milliseconds = words.next()?.parse::<u32>().ok()?;
                Request::Test {
                    milliseconds: milliseconds.clamp(1, MOST_TEST_MS),
                }
            }
            "play" => {
                let rate = words.next()?.parse::<u32>().ok().filter(|&rate| rate > 0)?;
                let channels = words.next()?.parse::<u8>().ok().filter(|&count| count > 0)?;
                let app = String::from(words.next().unwrap_or("unknown"));
                Request::Play {
                    rate,
                    channels: channels.min(8),
                    app,
                }
            }
            "set" => {
                let name = words.next()?;
                let value = words.next()?;
                let key = format!("sound.{name}");
                Request::Set(Change::of_setting(&key, value)?)
            }
            _ => return None,
        };
        words.next().is_none().then_some(request)
    }
}

/// The sound settings in force: each side's device id (`None` without one), the volumes in
/// percent and the mutes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub output: Option<String>,
    pub volume: u8,
    pub mute: bool,
    pub input: Option<String>,
    pub input_volume: u8,
    pub input_mute: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            output: None,
            volume: 50,
            mute: false,
            input: None,
            input_volume: 50,
            input_mute: false,
        }
    }
}

impl State {
    /// Its `state` line, without the newline.
    pub fn line(&self) -> String {
        format!(
            "state {} {} {} {} {} {}",
            self.output.as_deref().unwrap_or("none"),
            self.volume,
            self.mute,
            self.input.as_deref().unwrap_or("none"),
            self.input_volume,
            self.input_mute
        )
    }

    /// The state a `state` line says.
    pub fn parse(line: &str) -> Option<Self> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let ["state", output, volume, mute, input, input_volume, input_mute] = words[..] else {
            return None;
        };
        let device = |id: &str| (id != "none").then(|| String::from(id));
        let flag = |word: &str| match word {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        Some(Self {
            output: device(output),
            volume: volume.parse().ok().filter(|&volume: &u8| volume <= 100)?,
            mute: flag(mute)?,
            input: device(input),
            input_volume: input_volume.parse().ok().filter(|&volume: &u8| volume <= 100)?,
            input_mute: flag(input_mute)?,
        })
    }
}

/// A sound device's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Output,
    Input,
}

impl Kind {
    /// Its word.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Output => "output",
            Kind::Input => "input",
        }
    }
}

/// One sound device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub rate: u32,
}

impl Device {
    /// Its `device` line, without the newline.
    pub fn line(&self) -> String {
        format!("device {} {} {}", self.id, self.kind.word(), self.name)
    }

    /// The device a `device` line describes.
    pub fn parse(line: &str) -> Option<Self> {
        let rest = line.strip_prefix("device ")?;
        let (id, rest) = rest.split_once(' ')?;
        let (kind, name) = rest.split_once(' ')?;
        let kind = match kind {
            "output" => Kind::Output,
            "input" => Kind::Input,
            _ => return None,
        };
        Some(Self {
            id: String::from(id),
            kind,
            name: String::from(name),
            rate: 0,
        })
    }

    /// The devices `/proc/driver/sound` lists (`hda0-out output 48000 Built-in speakers`), none
    /// for `none`.
    pub fn from_driver_text(text: &str) -> Vec<Self> {
        text.lines()
            .filter_map(|line| {
                let mut words = line.splitn(4, ' ');
                let id = words.next()?;
                let kind = match words.next()? {
                    "output" => Kind::Output,
                    "input" => Kind::Input,
                    _ => return None,
                };
                let rate = words.next()?.parse().ok()?;
                let name = words.next()?;
                Some(Self {
                    id: String::from(id),
                    kind,
                    name: String::from(name),
                    rate,
                })
            })
            .collect()
    }
}

/// The device `wanted` (`auto` or an id) picks among `devices` of `kind`: the id when one has
/// it, for `auto` the first headphones, else the first of the kind.
pub fn chosen<'a>(devices: &'a [Device], kind: Kind, wanted: &str) -> Option<&'a Device> {
    let of_kind = || devices.iter().filter(move |device| device.kind == kind);
    match wanted {
        "auto" => of_kind()
            .find(|device| device.name == "Headphones")
            .or_else(|| of_kind().next()),
        id => of_kind().find(|device| device.id == id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_states_go_through_their_lines_and_back() {
        for request in [
            Request::Get,
            Request::Devices,
            Request::Set(Change::Volume(40)),
            Request::Set(Change::Output(String::from("hda0-out"))),
            Request::Set(Change::InputMute(true)),
            Request::Test { milliseconds: 1000 },
            Request::Play {
                rate: 44_100,
                channels: 1,
                app: String::from("windows-apps:notepad"),
            },
        ] {
            assert_eq!(Request::parse(&request.line()), Some(request));
        }
        assert_eq!(Request::parse("set volume 101"), None);
        assert_eq!(Request::parse("set loudness 3"), None);
        assert_eq!(
            Request::parse("test 60000"),
            Some(Request::Test {
                milliseconds: MOST_TEST_MS
            })
        );
        let state = State {
            output: Some(String::from("hda0-out")),
            mute: true,
            ..State::default()
        };
        assert_eq!(state.line(), "state hda0-out 50 true none 50 false");
        assert_eq!(State::parse(&state.line()), Some(state));
    }

    #[test]
    fn devices_come_from_the_driver_and_auto_prefers_headphones() {
        let devices = Device::from_driver_text(
            "hda0-out output 48000 Built-in speakers\nhda1-out output 48000 Headphones\n",
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "Built-in speakers");
        assert_eq!(devices[1].rate, 48_000);
        assert_eq!(chosen(&devices, Kind::Output, "auto").unwrap().id, "hda1-out");
        assert_eq!(chosen(&devices, Kind::Output, "hda0-out").unwrap().id, "hda0-out");
        assert!(chosen(&devices, Kind::Output, "usb9-out").is_none());
        assert!(chosen(&devices, Kind::Input, "auto").is_none());
        assert!(Device::from_driver_text("none").is_empty());
        let line = devices[0].line();
        assert_eq!(line, "device hda0-out output Built-in speakers");
        assert_eq!(Device::parse(&line).unwrap().name, "Built-in speakers");
    }
}
