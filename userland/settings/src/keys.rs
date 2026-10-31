//! The settings, as the `settings` command names them (docs/ux/toolkit-v0.3.md section 11):
//! which can be set, what each holds now, and what a new value means. The window shows the
//! same values from the same code.

use crate::facts::Facts;
use core::fmt::Write;
use eggos_settings_file::{InputPreferences, KEYBOARD_LAYOUTS, KeyAccessibility, PROFILES};
use eggos_toolkit::text::Text;

/// A value as it is printed and shown.
pub type Value = Text<64>;

/// A setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    ShellProfile,
    DisplayScale,
    DisplayResolution,
    DisplayPointer,
    KeyboardLayout,
    TimeZone,
    MouseButtons,
    MouseScrolling,
    MouseSpeed,
    KeyboardRepeatDelay,
    KeyboardRepeatRate,
    StickyKeys,
    SlowKeys,
    SlowKeysMs,
    KeyboardNavigation,
    SystemVersion,
    SystemArch,
    SystemCpus,
    SystemMemoryMb,
    SystemUptimeS,
}

/// Why a setting could not be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No setting has that name.
    NoSuchSetting,
    /// The setting only reports a fact.
    ReadOnly,
    /// The value is not one the setting takes.
    InvalidValue,
}

/// What setting a value changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The desktop's layout profile: its index in [`PROFILES`].
    Profile(usize),
    /// The keyboard layout: its index in the settings file's `KEYBOARD_LAYOUTS` (D185).
    KeyboardLayout(usize),
    /// The time zone: its index in `eggos_calendar::zone::ZONES` (D194).
    TimeZone(usize),
    /// The display's scale, 1 or 2, from the next boot (D195).
    DisplayScale(u32),
    /// One mouse or key preference, used from the next event (D195).
    Input(InputChange),
    /// Sticky keys or slow keys, used from the next key (D205).
    Keys(KeysChange),
}

/// One key accessibility setting's new value (D205).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysChange {
    StickyKeys(bool),
    SlowKeys(bool),
    /// 100 to 1000 milliseconds.
    SlowKeysMs(u32),
}

impl KeysChange {
    /// `keys` with this one changed.
    pub const fn applied_to(self, keys: KeyAccessibility) -> KeyAccessibility {
        let mut changed = keys;
        match self {
            Self::StickyKeys(on) => changed.sticky_keys = on,
            Self::SlowKeys(on) => changed.slow_keys = on,
            Self::SlowKeysMs(delay) => changed.slow_keys_ms = delay,
        }
        changed
    }

    /// The setting it changes and its value as the settings file holds it.
    pub fn saved(self) -> (&'static str, Value) {
        let mut value = Value::new();
        let key = match self {
            Self::StickyKeys(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::STICKY_KEYS
            }
            Self::SlowKeys(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::SLOW_KEYS
            }
            Self::SlowKeysMs(delay) => {
                let _ = write!(value, "{delay}");
                eggos_settings_file::SLOW_KEYS_MS
            }
        };
        (key, value)
    }
}

const fn boolean_name(on: bool) -> &'static str {
    match on {
        true => "true",
        false => "false",
    }
}

fn boolean(value: &str) -> Result<bool, Refusal> {
    match value {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(Refusal::InvalidValue),
    }
}

/// One mouse or key preference's new value (D195).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChange {
    /// The buttons are for the left hand.
    LeftHanded(bool),
    /// The wheel scrolls the natural way.
    NaturalScrolling(bool),
    /// 1 to 10.
    PointerSpeed(u32),
    /// 150 to 2000 milliseconds.
    RepeatDelayMs(u32),
    /// 2 to 60 repeats a second.
    RepeatRate(u32),
}

impl InputChange {
    /// `preferences` with this one changed.
    pub const fn applied_to(self, preferences: InputPreferences) -> InputPreferences {
        let mut changed = preferences;
        match self {
            Self::LeftHanded(on) => changed.left_handed = on,
            Self::NaturalScrolling(on) => changed.natural_scrolling = on,
            Self::PointerSpeed(speed) => changed.pointer_speed = speed,
            Self::RepeatDelayMs(delay) => changed.repeat_delay_ms = delay,
            Self::RepeatRate(rate) => changed.repeat_rate = rate,
        }
        changed
    }

    /// The setting it changes and its value as the settings file holds it.
    pub fn saved(self) -> (&'static str, Value) {
        let mut value = Value::new();
        let key = match self {
            Self::LeftHanded(on) => {
                let _ = value.write_str(buttons_name(on));
                eggos_settings_file::MOUSE_BUTTONS
            }
            Self::NaturalScrolling(on) => {
                let _ = value.write_str(scrolling_name(on));
                eggos_settings_file::MOUSE_SCROLLING
            }
            Self::PointerSpeed(speed) => {
                let _ = write!(value, "{speed}");
                eggos_settings_file::MOUSE_SPEED
            }
            Self::RepeatDelayMs(delay) => {
                let _ = write!(value, "{delay}");
                eggos_settings_file::KEYBOARD_REPEAT_DELAY
            }
            Self::RepeatRate(rate) => {
                let _ = write!(value, "{rate}");
                eggos_settings_file::KEYBOARD_REPEAT_RATE
            }
        };
        (key, value)
    }
}

const fn buttons_name(left_handed: bool) -> &'static str {
    match left_handed {
        true => "left",
        false => "right",
    }
}

const fn scrolling_name(natural: bool) -> &'static str {
    match natural {
        true => "natural",
        false => "traditional",
    }
}

fn number_within(value: &str, low: u32, high: u32) -> Result<u32, Refusal> {
    let all_digits = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    match all_digits.then(|| value.parse::<u32>().ok()).flatten() {
        Some(number) if (low..=high).contains(&number) => Ok(number),
        _ => Err(Refusal::InvalidValue),
    }
}

/// The names `shell.profile` takes, as the refusal of another one lists them.
pub const PROFILE_CHOICES: &str = "windows, macos or linux";

/// What `time.zone` takes, as the refusal of another value says it.
pub const ZONE_CHOICES: &str = "an IANA zone such as Europe/Paris or America/New_York, or UTC";

impl Key {
    /// Every setting, in the order `settings list` prints them.
    pub const ALL: [Self; 20] = [
        Self::ShellProfile,
        Self::DisplayScale,
        Self::DisplayResolution,
        Self::DisplayPointer,
        Self::KeyboardLayout,
        Self::TimeZone,
        Self::MouseButtons,
        Self::MouseScrolling,
        Self::MouseSpeed,
        Self::KeyboardRepeatDelay,
        Self::KeyboardRepeatRate,
        Self::StickyKeys,
        Self::SlowKeys,
        Self::SlowKeysMs,
        Self::KeyboardNavigation,
        Self::SystemVersion,
        Self::SystemArch,
        Self::SystemCpus,
        Self::SystemMemoryMb,
        Self::SystemUptimeS,
    ];

    /// Its name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ShellProfile => eggos_settings_file::SHELL_PROFILE,
            Self::DisplayScale => eggos_settings_file::DISPLAY_SCALE,
            Self::DisplayResolution => "display.resolution",
            Self::DisplayPointer => "display.pointer",
            Self::KeyboardLayout => eggos_settings_file::KEYBOARD_LAYOUT,
            Self::TimeZone => eggos_settings_file::TIME_ZONE,
            Self::MouseButtons => eggos_settings_file::MOUSE_BUTTONS,
            Self::MouseScrolling => eggos_settings_file::MOUSE_SCROLLING,
            Self::MouseSpeed => eggos_settings_file::MOUSE_SPEED,
            Self::KeyboardRepeatDelay => eggos_settings_file::KEYBOARD_REPEAT_DELAY,
            Self::KeyboardRepeatRate => eggos_settings_file::KEYBOARD_REPEAT_RATE,
            Self::StickyKeys => eggos_settings_file::STICKY_KEYS,
            Self::SlowKeys => eggos_settings_file::SLOW_KEYS,
            Self::SlowKeysMs => eggos_settings_file::SLOW_KEYS_MS,
            Self::KeyboardNavigation => "accessibility.keyboard_navigation",
            Self::SystemVersion => "system.version",
            Self::SystemArch => "system.arch",
            Self::SystemCpus => "system.cpus",
            Self::SystemMemoryMb => "system.memory_mb",
            Self::SystemUptimeS => "system.uptime_s",
        }
    }

    /// The setting named `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.name() == name)
    }

    /// Whether it can be set; the others report facts.
    pub const fn is_settable(self) -> bool {
        matches!(
            self,
            Self::ShellProfile
                | Self::DisplayScale
                | Self::KeyboardLayout
                | Self::TimeZone
                | Self::MouseButtons
                | Self::MouseScrolling
                | Self::MouseSpeed
                | Self::KeyboardRepeatDelay
                | Self::KeyboardRepeatRate
                | Self::StickyKeys
                | Self::SlowKeys
                | Self::SlowKeysMs
        )
    }

    /// The values it takes, as the refusal of another one lists them: `a, b or c`.
    pub fn choices(self) -> Text<256> {
        let mut choices = Text::<256>::new();
        match self {
            Self::KeyboardLayout => {
                let last = KEYBOARD_LAYOUTS.len() - 1;
                for (index, layout) in KEYBOARD_LAYOUTS.iter().enumerate() {
                    let separator = match index {
                        0 => "",
                        _ if index == last => " or ",
                        _ => ", ",
                    };
                    let _ = write!(choices, "{separator}{}", layout.name);
                }
            }
            Self::TimeZone => {
                let _ = choices.write_str(ZONE_CHOICES);
            }
            Self::DisplayScale => {
                let _ = choices.write_str("1 or 2");
            }
            Self::MouseButtons => {
                let _ = choices.write_str("right or left");
            }
            Self::MouseScrolling => {
                let _ = choices.write_str("traditional or natural");
            }
            Self::MouseSpeed => {
                let _ = choices.write_str("1 to 10");
            }
            Self::KeyboardRepeatDelay => {
                let _ = choices.write_str("150 to 2000");
            }
            Self::KeyboardRepeatRate => {
                let _ = choices.write_str("2 to 60");
            }
            Self::StickyKeys | Self::SlowKeys => {
                let _ = choices.write_str("true or false");
            }
            Self::SlowKeysMs => {
                let _ = choices.write_str("100 to 1000");
            }
            _ => {
                let _ = choices.write_str(PROFILE_CHOICES);
            }
        }
        choices
    }

    /// Its value now.
    pub fn value(self, facts: &Facts) -> Value {
        let mut value = Value::new();
        let _ = match self {
            Self::ShellProfile => value.write_str(PROFILES[facts.profile.min(PROFILES.len() - 1)]),
            Self::DisplayScale => write!(value, "{}", facts.scale),
            Self::DisplayResolution => {
                write!(value, "{}x{}", facts.resolution.0, facts.resolution.1)
            }
            Self::DisplayPointer => value.write_str("software"),
            Self::KeyboardLayout => value.write_str(
                KEYBOARD_LAYOUTS[facts.keyboard_layout.min(KEYBOARD_LAYOUTS.len() - 1)].name,
            ),
            Self::TimeZone => value.write_str(
                eggos_calendar::zone::ZONES
                    .get(facts.time_zone)
                    .map_or("UTC", |(name, _)| *name),
            ),
            Self::MouseButtons => value.write_str(buttons_name(facts.input.left_handed)),
            Self::MouseScrolling => value.write_str(scrolling_name(facts.input.natural_scrolling)),
            Self::MouseSpeed => write!(value, "{}", facts.input.pointer_speed),
            Self::KeyboardRepeatDelay => write!(value, "{}", facts.input.repeat_delay_ms),
            Self::KeyboardRepeatRate => write!(value, "{}", facts.input.repeat_rate),
            Self::StickyKeys => value.write_str(boolean_name(facts.keys.sticky_keys)),
            Self::SlowKeys => value.write_str(boolean_name(facts.keys.slow_keys)),
            Self::SlowKeysMs => write!(value, "{}", facts.keys.slow_keys_ms),
            Self::KeyboardNavigation => value.write_str("always"),
            Self::SystemVersion => value.write_str(facts.version),
            Self::SystemArch => value.write_str(facts.architecture.as_str()),
            Self::SystemCpus => write!(value, "{}", facts.cpus),
            Self::SystemMemoryMb => write!(value, "{}", facts.memory_mb),
            Self::SystemUptimeS => write!(value, "{}", facts.uptime_s),
        };
        value
    }

    /// What setting it to `value` changes.
    pub fn change(self, value: &str) -> Result<Change, Refusal> {
        match self {
            Self::ShellProfile => PROFILES
                .iter()
                .position(|profile| *profile == value)
                .map(Change::Profile)
                .ok_or(Refusal::InvalidValue),
            Self::KeyboardLayout => eggos_settings_file::keyboard_layout_named(value.as_bytes())
                .map(Change::KeyboardLayout)
                .ok_or(Refusal::InvalidValue),
            Self::TimeZone => crate::facts::zone_index(value)
                .map(Change::TimeZone)
                .ok_or(Refusal::InvalidValue),
            Self::DisplayScale => match value {
                "1" => Ok(Change::DisplayScale(1)),
                "2" => Ok(Change::DisplayScale(2)),
                _ => Err(Refusal::InvalidValue),
            },
            Self::MouseButtons => match value {
                "right" => Ok(Change::Input(InputChange::LeftHanded(false))),
                "left" => Ok(Change::Input(InputChange::LeftHanded(true))),
                _ => Err(Refusal::InvalidValue),
            },
            Self::MouseScrolling => match value {
                "traditional" => Ok(Change::Input(InputChange::NaturalScrolling(false))),
                "natural" => Ok(Change::Input(InputChange::NaturalScrolling(true))),
                _ => Err(Refusal::InvalidValue),
            },
            Self::MouseSpeed => number_within(value, 1, 10)
                .map(|speed| Change::Input(InputChange::PointerSpeed(speed))),
            Self::KeyboardRepeatDelay => number_within(value, 150, 2000)
                .map(|delay| Change::Input(InputChange::RepeatDelayMs(delay))),
            Self::KeyboardRepeatRate => {
                number_within(value, 2, 60).map(|rate| Change::Input(InputChange::RepeatRate(rate)))
            }
            Self::StickyKeys => boolean(value).map(|on| Change::Keys(KeysChange::StickyKeys(on))),
            Self::SlowKeys => boolean(value).map(|on| Change::Keys(KeysChange::SlowKeys(on))),
            Self::SlowKeysMs => number_within(value, 100, 1000)
                .map(|delay| Change::Keys(KeysChange::SlowKeysMs(delay))),
            _ => Err(Refusal::ReadOnly),
        }
    }
}

/// What `settings set name value` changes.
pub fn change(name: &str, value: &str) -> Result<(Key, Change), Refusal> {
    let key = Key::named(name).ok_or(Refusal::NoSuchSetting)?;
    key.change(value).map(|change| (key, change))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_a_value_from_the_facts() {
        let facts = Facts::example();
        let values: Vec<(&str, String)> = Key::ALL
            .iter()
            .map(|key| (key.name(), key.value(&facts).as_str().to_owned()))
            .collect();
        assert_eq!(
            values,
            [
                ("shell.profile", "macos".to_owned()),
                ("display.scale", "1".to_owned()),
                ("display.resolution", "1280x800".to_owned()),
                ("display.pointer", "software".to_owned()),
                ("keyboard.layout", "us".to_owned()),
                ("time.zone", "UTC".to_owned()),
                ("mouse.buttons", "right".to_owned()),
                ("mouse.scrolling", "traditional".to_owned()),
                ("mouse.speed", "5".to_owned()),
                ("keyboard.repeat_delay_ms", "400".to_owned()),
                ("keyboard.repeat_rate", "25".to_owned()),
                ("accessibility.sticky_keys", "false".to_owned()),
                ("accessibility.slow_keys", "false".to_owned()),
                ("accessibility.slow_keys_ms", "300".to_owned()),
                ("accessibility.keyboard_navigation", "always".to_owned()),
                ("system.version", "0.2.0".to_owned()),
                ("system.arch", "aarch64".to_owned()),
                ("system.cpus", "4".to_owned()),
                ("system.memory_mb", "1024".to_owned()),
                ("system.uptime_s", "3725".to_owned()),
            ]
        );
    }

    #[test]
    fn only_the_profile_and_the_layout_can_be_set_and_only_to_their_values() {
        assert_eq!(
            change("shell.profile", "windows"),
            Ok((Key::ShellProfile, Change::Profile(0)))
        );
        assert_eq!(
            change("shell.profile", "linux"),
            Ok((Key::ShellProfile, Change::Profile(2)))
        );
        assert_eq!(change("shell.profile", "amiga"), Err(Refusal::InvalidValue));
        assert_eq!(
            change("shell.profile", "Windows"),
            Err(Refusal::InvalidValue)
        );
        assert_eq!(change("system.version", "9"), Err(Refusal::ReadOnly));
        assert_eq!(
            change("display.scale", "2"),
            Ok((Key::DisplayScale, Change::DisplayScale(2)))
        );
        assert_eq!(change("display.scale", "3"), Err(Refusal::InvalidValue));
        assert_eq!(
            change("keyboard.layout", "de"),
            Ok((Key::KeyboardLayout, Change::KeyboardLayout(8)))
        );
        assert_eq!(change("keyboard.layout", "DE"), Err(Refusal::InvalidValue));
        assert!(
            Key::KeyboardLayout
                .choices()
                .as_str()
                .starts_with("us, us-intl, gb, us-dvorak")
        );
        assert!(
            Key::KeyboardLayout
                .choices()
                .as_str()
                .ends_with(", es or se")
        );
        assert_eq!(Key::ShellProfile.choices().as_str(), PROFILE_CHOICES);
        assert_eq!(change("nope", "x"), Err(Refusal::NoSuchSetting));
    }

    #[test]
    fn the_mouse_and_key_preferences_take_only_their_values() {
        assert_eq!(
            change("mouse.buttons", "left"),
            Ok((
                Key::MouseButtons,
                Change::Input(InputChange::LeftHanded(true))
            ))
        );
        assert_eq!(change("mouse.buttons", "both"), Err(Refusal::InvalidValue));
        assert_eq!(
            change("mouse.scrolling", "natural"),
            Ok((
                Key::MouseScrolling,
                Change::Input(InputChange::NaturalScrolling(true))
            ))
        );
        assert_eq!(
            change("mouse.speed", "10"),
            Ok((
                Key::MouseSpeed,
                Change::Input(InputChange::PointerSpeed(10))
            ))
        );
        for refused in ["0", "11", "+5", "", "05x"] {
            assert_eq!(change("mouse.speed", refused), Err(Refusal::InvalidValue));
        }
        assert_eq!(
            change("keyboard.repeat_delay_ms", "150"),
            Ok((
                Key::KeyboardRepeatDelay,
                Change::Input(InputChange::RepeatDelayMs(150))
            ))
        );
        assert_eq!(
            change("keyboard.repeat_delay_ms", "2001"),
            Err(Refusal::InvalidValue)
        );
        assert_eq!(
            change("keyboard.repeat_rate", "1"),
            Err(Refusal::InvalidValue)
        );
        assert_eq!(Key::KeyboardRepeatRate.choices().as_str(), "2 to 60");
    }

    #[test]
    fn an_input_change_changes_one_preference_and_saves_as_the_file_holds_it() {
        let changed = InputChange::PointerSpeed(8).applied_to(InputPreferences::DEFAULT);
        assert_eq!(
            changed,
            InputPreferences {
                pointer_speed: 8,
                ..InputPreferences::DEFAULT
            }
        );
        let (key, value) = InputChange::LeftHanded(true).saved();
        assert_eq!((key, value.as_str()), ("mouse.buttons", "left"));
        let (key, value) = InputChange::NaturalScrolling(false).saved();
        assert_eq!((key, value.as_str()), ("mouse.scrolling", "traditional"));
        let (key, value) = InputChange::RepeatDelayMs(600).saved();
        assert_eq!((key, value.as_str()), ("keyboard.repeat_delay_ms", "600"));
    }
}
