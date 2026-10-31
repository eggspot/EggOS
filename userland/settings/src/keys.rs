//! The settings, as the `settings` command names them (docs/ux/toolkit-v0.3.md section 11):
//! which can be set, what each holds now, and what a new value means. The window shows the
//! same values from the same code.

use crate::facts::Facts;
use core::fmt::Write;
use eggos_settings_file::{KEYBOARD_LAYOUTS, PROFILES};
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
}

/// The names `shell.profile` takes, as the refusal of another one lists them.
pub const PROFILE_CHOICES: &str = "windows, macos or linux";

/// What `time.zone` takes, as the refusal of another value says it.
pub const ZONE_CHOICES: &str = "an IANA zone such as Europe/Paris or America/New_York, or UTC";

impl Key {
    /// Every setting, in the order `settings list` prints them.
    pub const ALL: [Self; 11] = [
        Self::ShellProfile,
        Self::DisplayScale,
        Self::DisplayResolution,
        Self::DisplayPointer,
        Self::KeyboardLayout,
        Self::TimeZone,
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
            Self::ShellProfile | Self::KeyboardLayout | Self::TimeZone
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
        assert_eq!(change("display.scale", "2"), Err(Refusal::ReadOnly));
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
}
