//! The settings, as the `settings` command names them (docs/ux/toolkit-v0.3.md section 11):
//! which can be set, what each holds now, and what a new value means. The window shows the
//! same values from the same code.

use crate::facts::Facts;
use crate::network_pane::servers_text;
use core::fmt::Write;
use eggos_components::state::UpdatePolicy;
use eggos_net::status::{InterfaceStatus, State, state_word};
use eggos_settings_file::clock_sync::Record;
use eggos_settings_file::{
    DeviceInputPreferences, DisplayAccessibility, InputPreferences, KEYBOARD_LAYOUT_SET_CAPACITY,
    KEYBOARD_LAYOUTS, KeyAccessibility, KeyboardLayoutSet, LayoutSetRefusal, MAX_KEYBOARD_LAYOUTS,
    PROFILES, ScreenReaderSettings, SwitchKey,
};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Theme;

/// The setting that holds the desktop's theme in the settings file (D219).
pub const APPEARANCE_THEME: &str = "appearance.theme";

/// A value as it is printed and shown.
pub type Value = Text<64>;

/// A setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    ShellProfile,
    ShellForm,
    ShellFormInUse,
    InputPresent,
    AppearanceTheme,
    DisplayScale,
    DisplayResolution,
    DisplaySize,
    DisplayResolutionChangeable,
    DisplayRotation,
    DisplayPointer,
    KeyboardLayout,
    KeyboardLayouts,
    KeyboardSwitchKey,
    KeyboardOnScreen,
    TimeZone,
    TimeAutomatic,
    TimeSync,
    MouseButtons,
    MouseScrolling,
    MouseSpeed,
    MouseAcceleration,
    MouseDevices,
    MouseDevice(u8, DeviceField),
    KeyboardRepeatDelay,
    KeyboardRepeatRate,
    HighContrast,
    ReduceMotion,
    StickyKeys,
    SlowKeys,
    SlowKeysMs,
    ScreenReader,
    ScreenReaderCaptions,
    ScreenReaderRate,
    ScreenReaderEcho,
    Zoom,
    ZoomLevel,
    PointerSize,
    BounceKeys,
    BounceKeysMs,
    ToggleKeys,
    MouseKeys,
    MouseKeysSpeed,
    KeyboardNavigation,
    NetworkState,
    NetworkAddress,
    NetworkRouter,
    NetworkDns,
    NetworkMac,
    NetworkMtu,
    NetworkWakeOnLan,
    NetworkMaxMtu,
    PowerBattery,
    PowerLine,
    PowerLid,
    PowerTemperature,
    PowerDock,
    PowerLidAction,
    UpdatePolicy,
    SystemVersion,
    SystemArch,
    SystemCpus,
    SystemMemoryMb,
    SystemUptimeS,
}

/// Which of a pointing device's own preferences a `mouse.device.<n>.*` key holds (D601), in
/// the global keys' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceField {
    Buttons,
    Scrolling,
    Speed,
    Acceleration,
}

impl DeviceField {
    /// Every field, in the order `settings list` prints them.
    pub const ALL: [Self; 4] = [Self::Buttons, Self::Scrolling, Self::Speed, Self::Acceleration];
}

/// One change of a pointing device's own preferences (D601): `None` follows the global one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceChange {
    LeftHanded(Option<bool>),
    NaturalScrolling(Option<bool>),
    Speed(Option<u32>),
    Acceleration(Option<bool>),
    /// Every field follows the global one.
    FollowAll,
}

impl DeviceChange {
    /// The preferences `own` become.
    pub fn applied_to(self, own: DeviceInputPreferences) -> DeviceInputPreferences {
        match self {
            Self::LeftHanded(value) => DeviceInputPreferences {
                left_handed: value,
                ..own
            },
            Self::NaturalScrolling(value) => DeviceInputPreferences {
                natural_scrolling: value,
                ..own
            },
            Self::Speed(value) => DeviceInputPreferences {
                pointer_speed: value,
                ..own
            },
            Self::Acceleration(value) => DeviceInputPreferences {
                acceleration: value,
                ..own
            },
            Self::FollowAll => DeviceInputPreferences::default(),
        }
    }

    /// The fields it saves, with each one's value in the settings file: `global` removes it.
    pub fn saved(self) -> impl Iterator<Item = (DeviceField, Value)> {
        let word = |value: Option<bool>, on: &str, off: &str| -> Value {
            let mut text = Value::new();
            let _ = text.write_str(match value {
                None => eggos_settings_file::FOLLOW_GLOBAL,
                Some(true) => on,
                Some(false) => off,
            });
            text
        };
        let follow = || word(None, "", "");
        let fields: [Option<(DeviceField, Value)>; 4] = match self {
            Self::LeftHanded(value) => {
                [Some((DeviceField::Buttons, word(value, "left", "right"))), None, None, None]
            }
            Self::NaturalScrolling(value) => [
                Some((DeviceField::Scrolling, word(value, "natural", "traditional"))),
                None,
                None,
                None,
            ],
            Self::Speed(value) => {
                let mut text = Value::new();
                let _ = match value {
                    Some(speed) => write!(text, "{speed}"),
                    None => text.write_str(eggos_settings_file::FOLLOW_GLOBAL),
                };
                [Some((DeviceField::Speed, text)), None, None, None]
            }
            Self::Acceleration(value) => [
                Some((DeviceField::Acceleration, word(value, "on", "off"))),
                None,
                None,
                None,
            ],
            Self::FollowAll => DeviceField::ALL.map(|field| Some((field, follow()))),
        };
        fields.into_iter().flatten()
    }
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
    /// The value is not a set of layouts `keyboard.layouts` takes, for this reason (D221).
    InvalidLayouts(LayoutSetRefusal),
}

/// What setting a value changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The desktop's layout profile: its index in [`PROFILES`].
    Profile(usize),
    /// The shell's form: its index in [`crate::facts::FORM_SETTINGS`], applied at once (D303).
    Form(usize),
    /// The desktop's theme, applied at once (D219).
    Theme(Theme),
    /// The keyboard layout: its index in the settings file's `KEYBOARD_LAYOUTS` (D185).
    KeyboardLayout(usize),
    /// The layouts the switch key cycles through (D221).
    KeyboardLayouts(KeyboardLayoutSet),
    /// The key that switches layouts (D221).
    KeyboardSwitchKey(SwitchKey),
    /// When the on-screen keyboard shows: its index in `KEYBOARD_ON_SCREEN_MODES`, applied at
    /// once (D314).
    OnScreenKeyboard(usize),
    /// How far the screen turns: its index in `DISPLAY_ROTATIONS`, applied at once (D330).
    DisplayRotation(usize),
    /// The time zone: its index in `eggos_calendar::zone::ZONES` (D194).
    TimeZone(usize),
    /// Whether the clock is set from the network; turning it on syncs at once (D225).
    Automatic(bool),
    /// The display's scale, 1 or 2, from the next boot (D195).
    DisplayScale(u32),
    /// The display's size from the next boot: `None` for the display's own (D287).
    DisplayResolution(Option<(u32, u32)>),
    /// One mouse or key preference, used from the next event (D195).
    Input(InputChange),
    /// Sticky keys or slow keys, used from the next key (D205).
    Keys(KeysChange),
    /// High contrast or reduced motion, shown at once (D224).
    Display(DisplayChange),
    /// One screen reader setting, used at once (D603).
    Reader(ReaderChange),
    /// What closing the lid does, from the next close (D599).
    LidAction(eggos_settings_file::LidAction),
    /// One pointing device's own preferences (`device` 1 to 16), used from its next move (D601).
    Device { device: u8, change: DeviceChange },
    /// How component updates are installed, from the next check (D658).
    UpdatePolicy(UpdatePolicy),
    /// The wired adapter's MTU, set at once and not saved (the kernel keeps it, D734).
    Mtu(u32),
    /// The wired adapter's Wake-on-LAN, set at once and not saved (D734).
    WakeOnLan(bool),
}

/// The largest MTU `network.mtu` takes before the adapter's own limit is checked.
pub const MOST_MTU: u32 = 65_535;

/// What the keyboard uses, all of it (D221): the layouts the switch key cycles through, the
/// one in force (an index of the settings file's `KEYBOARD_LAYOUTS`, always in the set) and
/// the switch key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardChoice {
    pub layouts: KeyboardLayoutSet,
    pub in_force: usize,
    pub switch_key: SwitchKey,
}

impl KeyboardChoice {
    /// What the keyboard uses as `facts` tell it.
    pub fn of(facts: &Facts) -> Self {
        Self {
            layouts: facts.keyboard_layouts,
            in_force: facts.keyboard_layout,
            switch_key: facts.switch_key,
        }
    }

    /// With layout `number` in force: the set keeps it, gains it while it has room, or has it
    /// take the place of the layout that was in force.
    pub fn choosing(self, number: usize) -> Self {
        Self {
            layouts: self.layouts.with_chosen(number, self.in_force),
            in_force: number,
            ..self
        }
    }

    /// With `layouts` the set: the layout in force stays when listed, else the first is.
    pub fn listing(self, layouts: KeyboardLayoutSet) -> Self {
        Self {
            layouts,
            in_force: layouts.keeping(self.in_force),
            ..self
        }
    }

    /// With `switch_key` the switch key.
    pub fn switching_with(self, switch_key: SwitchKey) -> Self {
        Self { switch_key, ..self }
    }
}

impl Change {
    /// What the keyboard uses after this change, from what it uses as `facts` tell it; `None`
    /// for a change of something else.
    pub fn keyboard_choice(self, facts: &Facts) -> Option<KeyboardChoice> {
        let now = KeyboardChoice::of(facts);
        match self {
            Self::KeyboardLayout(number) => Some(now.choosing(number)),
            Self::KeyboardLayouts(layouts) => Some(now.listing(layouts)),
            Self::KeyboardSwitchKey(switch_key) => Some(now.switching_with(switch_key)),
            _ => None,
        }
    }
}

/// One key accessibility setting's new value (D205).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysChange {
    StickyKeys(bool),
    SlowKeys(bool),
    /// 100 to 1000 milliseconds.
    SlowKeysMs(u32),
    BounceKeys(bool),
    /// 100 to 1000 milliseconds.
    BounceKeysMs(u32),
    ToggleKeys(bool),
    MouseKeys(bool),
    /// 100 to 1000 pixels a second.
    MouseKeysSpeed(u32),
}

impl KeysChange {
    /// `keys` with this one changed.
    pub const fn applied_to(self, keys: KeyAccessibility) -> KeyAccessibility {
        let mut changed = keys;
        match self {
            Self::StickyKeys(on) => changed.sticky_keys = on,
            Self::SlowKeys(on) => changed.slow_keys = on,
            Self::SlowKeysMs(delay) => changed.slow_keys_ms = delay,
            Self::BounceKeys(on) => changed.bounce_keys = on,
            Self::BounceKeysMs(delay) => changed.bounce_keys_ms = delay,
            Self::ToggleKeys(on) => changed.toggle_keys = on,
            Self::MouseKeys(on) => changed.mouse_keys = on,
            Self::MouseKeysSpeed(speed) => changed.mouse_keys_speed = speed,
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
            Self::BounceKeys(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::BOUNCE_KEYS
            }
            Self::BounceKeysMs(delay) => {
                let _ = write!(value, "{delay}");
                eggos_settings_file::BOUNCE_KEYS_MS
            }
            Self::ToggleKeys(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::TOGGLE_KEYS
            }
            Self::MouseKeys(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::MOUSE_KEYS
            }
            Self::MouseKeysSpeed(speed) => {
                let _ = write!(value, "{speed}");
                eggos_settings_file::MOUSE_KEYS_SPEED
            }
        };
        (key, value)
    }
}

/// One display accessibility setting's new value (D224).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayChange {
    HighContrast(bool),
    ReduceMotion(bool),
    Zoom(bool),
    /// One of the settings file's `ZOOM_LEVELS`.
    ZoomLevel(u32),
    /// The pointer's size in halves: 2, 3, 4 or 6.
    PointerSizeHalves(u32),
}

impl DisplayChange {
    /// `display` with this one changed.
    pub const fn applied_to(self, display: DisplayAccessibility) -> DisplayAccessibility {
        let mut changed = display;
        match self {
            Self::HighContrast(on) => changed.high_contrast = on,
            Self::ReduceMotion(on) => changed.reduce_motion = on,
            Self::Zoom(on) => changed.zoom = on,
            Self::ZoomLevel(level) => changed.zoom_level = level,
            Self::PointerSizeHalves(halves) => changed.pointer_size_halves = halves,
        }
        changed
    }

    /// The setting it changes and its value as the settings file holds it.
    pub fn saved(self) -> (&'static str, Value) {
        let mut value = Value::new();
        let key = match self {
            Self::HighContrast(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::HIGH_CONTRAST
            }
            Self::ReduceMotion(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::REDUCE_MOTION
            }
            Self::Zoom(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::ZOOM
            }
            Self::ZoomLevel(level) => {
                let _ = write!(value, "{level}");
                eggos_settings_file::ZOOM_LEVEL
            }
            Self::PointerSizeHalves(halves) => {
                let _ = value.write_str(pointer_size_name(halves));
                eggos_settings_file::POINTER_SIZE
            }
        };
        (key, value)
    }
}

/// One screen reader setting's new value (D603).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderChange {
    On(bool),
    Captions(bool),
    /// Words a minute, 100 to 400.
    Rate(u32),
    EchoCharacters(bool),
}

impl ReaderChange {
    /// `reader` with this one changed.
    pub const fn applied_to(self, reader: ScreenReaderSettings) -> ScreenReaderSettings {
        let mut changed = reader;
        match self {
            Self::On(on) => changed.on = on,
            Self::Captions(on) => changed.captions = on,
            Self::Rate(rate) => changed.rate = rate,
            Self::EchoCharacters(on) => changed.echo_characters = on,
        }
        changed
    }

    /// The setting it changes and its value as the settings file holds it.
    pub fn saved(self) -> (&'static str, Value) {
        let mut value = Value::new();
        let key = match self {
            Self::On(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::SCREEN_READER
            }
            Self::Captions(on) => {
                let _ = value.write_str(boolean_name(on));
                eggos_settings_file::SCREEN_READER_CAPTIONS
            }
            Self::Rate(rate) => {
                let _ = write!(value, "{rate}");
                eggos_settings_file::SCREEN_READER_RATE
            }
            Self::EchoCharacters(on) => {
                let _ = value.write_str(echo_name(on));
                eggos_settings_file::SCREEN_READER_ECHO
            }
        };
        (key, value)
    }
}

const fn echo_name(characters: bool) -> &'static str {
    match characters {
        true => "characters",
        false => "none",
    }
}

fn pointer_size_name(halves: u32) -> &'static str {
    eggos_settings_file::POINTER_SIZES
        .iter()
        .find(|&&(_, known)| known == halves)
        .map_or("1", |&(name, _)| name)
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
    /// Mice go further the faster they move (D589).
    PointerAcceleration(bool),
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
            Self::PointerAcceleration(on) => changed.pointer_acceleration = on,
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
            Self::PointerAcceleration(on) => {
                let _ = value.write_str(match on {
                    true => "on",
                    false => "off",
                });
                eggos_settings_file::MOUSE_ACCELERATION
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

/// The update policy's key, there only while the `management` component is installed (D658).
pub const UPDATE_POLICY: &str = "update.policy";

/// Whether `key` is one the machine has: `update.policy` only with the `management`
/// component.
pub fn present(key: Key, facts: &Facts) -> bool {
    key != Key::UpdatePolicy || facts.update_policy.is_some()
}

/// The names `shell.profile` takes, as the refusal of another one lists them.
pub const PROFILE_CHOICES: &str = "windows, macos or linux";

/// What `time.zone` takes, as the refusal of another value says it.
pub const ZONE_CHOICES: &str = "an IANA zone such as Europe/Paris or America/New_York, or UTC";

impl Key {
    /// Every setting, in the order `settings list` prints them.
    pub const ALL: [Self; 63] = [
        Self::ShellProfile,
        Self::ShellForm,
        Self::ShellFormInUse,
        Self::InputPresent,
        Self::AppearanceTheme,
        Self::DisplayScale,
        Self::DisplayResolution,
        Self::DisplaySize,
        Self::DisplayResolutionChangeable,
        Self::DisplayRotation,
        Self::DisplayPointer,
        Self::KeyboardLayout,
        Self::KeyboardLayouts,
        Self::KeyboardSwitchKey,
        Self::KeyboardOnScreen,
        Self::TimeZone,
        Self::TimeAutomatic,
        Self::TimeSync,
        Self::MouseButtons,
        Self::MouseScrolling,
        Self::MouseSpeed,
        Self::MouseAcceleration,
        Self::MouseDevices,
        Self::KeyboardRepeatDelay,
        Self::KeyboardRepeatRate,
        Self::HighContrast,
        Self::ReduceMotion,
        Self::StickyKeys,
        Self::SlowKeys,
        Self::SlowKeysMs,
        Self::ScreenReader,
        Self::ScreenReaderCaptions,
        Self::ScreenReaderRate,
        Self::ScreenReaderEcho,
        Self::Zoom,
        Self::ZoomLevel,
        Self::PointerSize,
        Self::BounceKeys,
        Self::BounceKeysMs,
        Self::ToggleKeys,
        Self::MouseKeys,
        Self::MouseKeysSpeed,
        Self::KeyboardNavigation,
        Self::NetworkState,
        Self::NetworkAddress,
        Self::NetworkRouter,
        Self::NetworkDns,
        Self::NetworkMac,
        Self::NetworkMtu,
        Self::NetworkWakeOnLan,
        Self::NetworkMaxMtu,
        Self::PowerBattery,
        Self::PowerLine,
        Self::PowerLid,
        Self::PowerTemperature,
        Self::PowerDock,
        Self::PowerLidAction,
        Self::UpdatePolicy,
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
            Self::ShellForm => eggos_settings_file::SHELL_FORM,
            Self::ShellFormInUse => "shell.form_in_use",
            Self::InputPresent => "input.present",
            Self::AppearanceTheme => APPEARANCE_THEME,
            Self::DisplayScale => eggos_settings_file::DISPLAY_SCALE,
            Self::DisplayResolution => eggos_settings_file::DISPLAY_RESOLUTION,
            Self::DisplaySize => "display.size",
            Self::DisplayResolutionChangeable => "display.resolution_changeable",
            Self::DisplayRotation => eggos_settings_file::DISPLAY_ROTATION,
            Self::DisplayPointer => "display.pointer",
            Self::KeyboardLayout => eggos_settings_file::KEYBOARD_LAYOUT,
            Self::KeyboardLayouts => eggos_settings_file::KEYBOARD_LAYOUT_SET,
            Self::KeyboardSwitchKey => eggos_settings_file::KEYBOARD_SWITCH_KEY,
            Self::KeyboardOnScreen => eggos_settings_file::KEYBOARD_ON_SCREEN,
            Self::TimeZone => eggos_settings_file::TIME_ZONE,
            Self::TimeAutomatic => eggos_settings_file::TIME_AUTOMATIC,
            Self::TimeSync => "time.sync",
            Self::MouseButtons => eggos_settings_file::MOUSE_BUTTONS,
            Self::MouseScrolling => eggos_settings_file::MOUSE_SCROLLING,
            Self::MouseSpeed => eggos_settings_file::MOUSE_SPEED,
            Self::MouseAcceleration => eggos_settings_file::MOUSE_ACCELERATION,
            Self::MouseDevices => "mouse.devices",
            Self::MouseDevice(index, field) => {
                eggos_settings_file::MOUSE_DEVICE_KEYS[index as usize][field as usize]
            }
            Self::KeyboardRepeatDelay => eggos_settings_file::KEYBOARD_REPEAT_DELAY,
            Self::KeyboardRepeatRate => eggos_settings_file::KEYBOARD_REPEAT_RATE,
            Self::HighContrast => eggos_settings_file::HIGH_CONTRAST,
            Self::ReduceMotion => eggos_settings_file::REDUCE_MOTION,
            Self::StickyKeys => eggos_settings_file::STICKY_KEYS,
            Self::SlowKeys => eggos_settings_file::SLOW_KEYS,
            Self::SlowKeysMs => eggos_settings_file::SLOW_KEYS_MS,
            Self::ScreenReader => eggos_settings_file::SCREEN_READER,
            Self::ScreenReaderCaptions => eggos_settings_file::SCREEN_READER_CAPTIONS,
            Self::ScreenReaderRate => eggos_settings_file::SCREEN_READER_RATE,
            Self::ScreenReaderEcho => eggos_settings_file::SCREEN_READER_ECHO,
            Self::Zoom => eggos_settings_file::ZOOM,
            Self::ZoomLevel => eggos_settings_file::ZOOM_LEVEL,
            Self::PointerSize => eggos_settings_file::POINTER_SIZE,
            Self::BounceKeys => eggos_settings_file::BOUNCE_KEYS,
            Self::BounceKeysMs => eggos_settings_file::BOUNCE_KEYS_MS,
            Self::ToggleKeys => eggos_settings_file::TOGGLE_KEYS,
            Self::MouseKeys => eggos_settings_file::MOUSE_KEYS,
            Self::MouseKeysSpeed => eggos_settings_file::MOUSE_KEYS_SPEED,
            Self::KeyboardNavigation => "accessibility.keyboard_navigation",
            Self::NetworkState => "network.state",
            Self::NetworkAddress => "network.address",
            Self::NetworkRouter => "network.router",
            Self::NetworkDns => "network.dns",
            Self::NetworkMac => "network.mac",
            Self::NetworkMtu => "network.mtu",
            Self::NetworkWakeOnLan => "network.wake_on_lan",
            Self::NetworkMaxMtu => "network.max_mtu",
            Self::PowerBattery => "power.battery",
            Self::PowerLine => "power.line",
            Self::PowerLid => "power.lid",
            Self::PowerTemperature => "power.temperature",
            Self::PowerDock => "power.dock",
            Self::PowerLidAction => eggos_settings_file::POWER_LID_ACTION,
            Self::UpdatePolicy => UPDATE_POLICY,
            Self::SystemVersion => "system.version",
            Self::SystemArch => "system.arch",
            Self::SystemCpus => "system.cpus",
            Self::SystemMemoryMb => "system.memory_mb",
            Self::SystemUptimeS => "system.uptime_s",
        }
    }

    /// The setting named `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.name() == name).or_else(|| {
            eggos_settings_file::MOUSE_DEVICE_KEYS
                .iter()
                .enumerate()
                .find_map(|(index, names)| {
                    let field = names.iter().position(|own| *own == name)?;
                    Some(Self::MouseDevice(index as u8, DeviceField::ALL[field]))
                })
        })
    }

    /// The keys `settings list` prints with `facts`: [`Self::ALL`], with after `mouse.devices`
    /// the four keys of every pointing device present or with a value of its own (D601).
    pub fn listed(facts: &Facts) -> alloc::vec::Vec<Self> {
        let mut keys = alloc::vec::Vec::with_capacity(Self::ALL.len());
        for key in Self::ALL.into_iter().filter(|key| present(*key, facts)) {
            keys.push(key);
            if key != Self::MouseDevices {
                continue;
            }
            for index in 0..eggos_settings_file::MOST_POINTING_DEVICES {
                let present = facts.pointing_devices.kind(index + 1).is_some();
                if present || !facts.device_input[index].follows_all() {
                    keys.extend(
                        DeviceField::ALL.map(|field| Self::MouseDevice(index as u8, field)),
                    );
                }
            }
        }
        keys
    }

    /// Whether it can be set; the others report facts.
    pub const fn is_settable(self) -> bool {
        matches!(
            self,
            Self::ShellProfile
                | Self::ShellForm
                | Self::AppearanceTheme
                | Self::DisplayScale
                | Self::DisplayResolution
                | Self::DisplayRotation
                | Self::KeyboardLayout
                | Self::KeyboardLayouts
                | Self::KeyboardSwitchKey
                | Self::KeyboardOnScreen
                | Self::TimeZone
                | Self::TimeAutomatic
                | Self::MouseButtons
                | Self::MouseScrolling
                | Self::MouseSpeed
                | Self::MouseAcceleration
                | Self::MouseDevice(..)
                | Self::KeyboardRepeatDelay
                | Self::KeyboardRepeatRate
                | Self::HighContrast
                | Self::ReduceMotion
                | Self::StickyKeys
                | Self::SlowKeys
                | Self::SlowKeysMs
                | Self::ScreenReader
                | Self::ScreenReaderCaptions
                | Self::ScreenReaderRate
                | Self::ScreenReaderEcho
                | Self::Zoom
                | Self::ZoomLevel
                | Self::PointerSize
                | Self::BounceKeys
                | Self::BounceKeysMs
                | Self::ToggleKeys
                | Self::MouseKeys
                | Self::MouseKeysSpeed
                | Self::PowerLidAction
                | Self::UpdatePolicy
                | Self::NetworkMtu
                | Self::NetworkWakeOnLan
        )
    }

    /// The values it takes, as the refusal of another one lists them: `a, b or c`.
    pub fn choices(self) -> Text<256> {
        let mut choices = Text::<256>::new();
        match self {
            Self::KeyboardLayout | Self::KeyboardLayouts => {
                let names = KEYBOARD_LAYOUTS.map(|layout| layout.name);
                write_choices(&mut choices, &names);
            }
            Self::KeyboardSwitchKey => write_choices(&mut choices, &SwitchKey::NAMES),
            Self::KeyboardOnScreen => {
                write_choices(&mut choices, &eggos_settings_file::KEYBOARD_ON_SCREEN_MODES)
            }
            Self::DisplayRotation => {
                write_choices(&mut choices, &eggos_settings_file::DISPLAY_ROTATIONS)
            }
            Self::PowerLidAction => write_choices(&mut choices, &eggos_settings_file::LID_ACTIONS),
            Self::UpdatePolicy => {
                let names = UpdatePolicy::ALL.map(UpdatePolicy::name);
                write_choices(&mut choices, &names);
            }
            Self::TimeZone => {
                let _ = choices.write_str(ZONE_CHOICES);
            }
            Self::DisplayScale => {
                let _ = choices.write_str("1 or 2");
            }
            Self::ShellForm => write_choices(&mut choices, &crate::facts::FORM_SETTINGS),
            Self::DisplayResolution => {
                let _ = choices.write_str("auto");
                for (width, height) in eggos_settings_file::RESOLUTIONS {
                    let _ = write!(choices, ", {width}x{height}");
                }
            }
            Self::TimeAutomatic | Self::NetworkWakeOnLan => {
                let _ = choices.write_str("on or off");
            }
            Self::NetworkMtu => {
                let _ = choices.write_str("a number of bytes from 68 to the adapter's largest");
            }
            Self::AppearanceTheme => {
                let _ = choices.write_str("dark or light");
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
            Self::MouseAcceleration => {
                let _ = choices.write_str("on or off");
            }
            Self::MouseDevice(_, field) => {
                let _ = choices.write_str(match field {
                    DeviceField::Buttons => "global, right or left",
                    DeviceField::Scrolling => "global, traditional or natural",
                    DeviceField::Speed => "global or 1 to 10",
                    DeviceField::Acceleration => "global, on or off",
                });
            }
            Self::KeyboardRepeatDelay => {
                let _ = choices.write_str("150 to 2000");
            }
            Self::KeyboardRepeatRate => {
                let _ = choices.write_str("2 to 60");
            }
            Self::HighContrast
            | Self::ReduceMotion
            | Self::StickyKeys
            | Self::SlowKeys
            | Self::ScreenReader
            | Self::ScreenReaderCaptions
            | Self::Zoom
            | Self::BounceKeys
            | Self::ToggleKeys
            | Self::MouseKeys => {
                let _ = choices.write_str("true or false");
            }
            Self::SlowKeysMs | Self::BounceKeysMs | Self::MouseKeysSpeed => {
                let _ = choices.write_str("100 to 1000");
            }
            Self::ZoomLevel => {
                let _ = choices.write_str("2, 3, 4, 6 or 8");
            }
            Self::ScreenReaderRate => {
                let _ = choices.write_str("100 to 400");
            }
            Self::ScreenReaderEcho => {
                write_choices(&mut choices, &eggos_settings_file::SCREEN_READER_ECHOES)
            }
            Self::PointerSize => {
                let _ = choices.write_str("1, 1.5, 2 or 3");
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
        let connected = connected(facts);
        let _ = match self {
            Self::ShellProfile => value.write_str(PROFILES[facts.profile.min(PROFILES.len() - 1)]),
            Self::ShellForm => value.write_str(
                crate::facts::FORM_SETTINGS[facts
                    .form_setting
                    .min(crate::facts::FORM_SETTINGS.len() - 1)],
            ),
            Self::ShellFormInUse => value.write_str(
                crate::facts::FORMS_IN_USE
                    [facts.form_in_use.min(crate::facts::FORMS_IN_USE.len() - 1)],
            ),
            Self::InputPresent => {
                let mut first = true;
                for (bit, name) in crate::facts::INPUT_KINDS.iter().enumerate() {
                    if facts.input_kinds & (1 << bit) == 0 {
                        continue;
                    }
                    if !first {
                        let _ = value.write_str(",");
                    }
                    first = false;
                    let _ = value.write_str(name);
                }
                match first {
                    true => value.write_str("none"),
                    false => Ok(()),
                }
            }
            Self::AppearanceTheme => value.write_str(facts.theme.name()),
            Self::DisplayScale => write!(value, "{}", facts.scale),
            Self::DisplayResolution => match facts.resolution_choice {
                Some((width, height)) => write!(value, "{width}x{height}"),
                None => value.write_str("auto"),
            },
            Self::DisplaySize => write!(value, "{}x{}", facts.resolution.0, facts.resolution.1),
            Self::DisplayResolutionChangeable => {
                value.write_str(match facts.resolution_changeable {
                    true => "yes",
                    false => "no",
                })
            }
            Self::DisplayPointer => value.write_str("software"),
            Self::KeyboardLayout => value.write_str(
                KEYBOARD_LAYOUTS[facts.keyboard_layout.min(KEYBOARD_LAYOUTS.len() - 1)].name,
            ),
            Self::KeyboardLayouts => {
                let mut text = [0u8; KEYBOARD_LAYOUT_SET_CAPACITY];
                value.write_str(facts.keyboard_layouts.write(&mut text))
            }
            Self::KeyboardSwitchKey => value.write_str(facts.switch_key.name()),
            Self::KeyboardOnScreen => value.write_str(
                eggos_settings_file::KEYBOARD_ON_SCREEN_MODES[facts.on_screen_keyboard.min(2)],
            ),
            Self::DisplayRotation => {
                value.write_str(eggos_settings_file::DISPLAY_ROTATIONS[facts.rotation.min(4)])
            }
            Self::TimeZone => value.write_str(
                eggos_calendar::zone::ZONES
                    .get(facts.time_zone)
                    .map_or("UTC", |(name, _)| *name),
            ),
            Self::TimeAutomatic => value.write_str(match facts.time_automatic {
                true => "on",
                false => "off",
            }),
            Self::TimeSync => write_clock_sync(&mut value, facts.clock_sync.as_ref()),
            Self::MouseButtons => value.write_str(buttons_name(facts.input.left_handed)),
            Self::MouseScrolling => value.write_str(scrolling_name(facts.input.natural_scrolling)),
            Self::MouseSpeed => write!(value, "{}", facts.input.pointer_speed),
            Self::MouseAcceleration => value.write_str(match facts.input.pointer_acceleration {
                true => "on",
                false => "off",
            }),
            Self::MouseDevices => facts.pointing_devices.write_words(&mut value),
            Self::MouseDevice(index, field) => {
                let own = facts.device_input[index as usize];
                let global = eggos_settings_file::FOLLOW_GLOBAL;
                let choice = |value: Option<bool>, on: &'static str, off: &'static str| match value {
                    None => global,
                    Some(true) => on,
                    Some(false) => off,
                };
                match field {
                    DeviceField::Buttons => {
                        value.write_str(choice(own.left_handed, "left", "right"))
                    }
                    DeviceField::Scrolling => {
                        value.write_str(choice(own.natural_scrolling, "natural", "traditional"))
                    }
                    DeviceField::Speed => match own.pointer_speed {
                        Some(speed) => write!(value, "{speed}"),
                        None => value.write_str(global),
                    },
                    DeviceField::Acceleration => {
                        value.write_str(choice(own.acceleration, "on", "off"))
                    }
                }
            }
            Self::KeyboardRepeatDelay => write!(value, "{}", facts.input.repeat_delay_ms),
            Self::KeyboardRepeatRate => write!(value, "{}", facts.input.repeat_rate),
            Self::HighContrast => value.write_str(boolean_name(facts.display.high_contrast)),
            Self::ReduceMotion => value.write_str(boolean_name(facts.display.reduce_motion)),
            Self::StickyKeys => value.write_str(boolean_name(facts.keys.sticky_keys)),
            Self::SlowKeys => value.write_str(boolean_name(facts.keys.slow_keys)),
            Self::SlowKeysMs => write!(value, "{}", facts.keys.slow_keys_ms),
            Self::ScreenReader => value.write_str(boolean_name(facts.screen_reader.on)),
            Self::ScreenReaderCaptions => {
                value.write_str(boolean_name(facts.screen_reader.captions))
            }
            Self::ScreenReaderRate => write!(value, "{}", facts.screen_reader.rate),
            Self::ScreenReaderEcho => value.write_str(echo_name(facts.screen_reader.echo_characters)),
            Self::Zoom => value.write_str(boolean_name(facts.display.zoom)),
            Self::ZoomLevel => write!(value, "{}", facts.display.zoom_level),
            Self::PointerSize => {
                value.write_str(pointer_size_name(facts.display.pointer_size_halves))
            }
            Self::BounceKeys => value.write_str(boolean_name(facts.keys.bounce_keys)),
            Self::BounceKeysMs => write!(value, "{}", facts.keys.bounce_keys_ms),
            Self::ToggleKeys => value.write_str(boolean_name(facts.keys.toggle_keys)),
            Self::MouseKeys => value.write_str(boolean_name(facts.keys.mouse_keys)),
            Self::MouseKeysSpeed => write!(value, "{}", facts.keys.mouse_keys_speed),
            Self::KeyboardNavigation => value.write_str("always"),
            Self::NetworkState => value.write_str(state_word(facts.network_adapter.as_ref())),
            Self::NetworkAddress => {
                write_or_none(&mut value, connected.and_then(|adapter| adapter.subnet))
            }
            Self::NetworkRouter => {
                write_or_none(&mut value, connected.and_then(|adapter| adapter.router))
            }
            Self::NetworkDns => match connected {
                Some(adapter) if adapter.dns_servers().next().is_some() => {
                    value.write_str(servers_text(adapter, ",").as_str())
                }
                _ => value.write_str(NETWORK_NONE),
            },
            Self::NetworkMac => {
                write_or_none(&mut value, facts.network_adapter.map(|status| status.mac))
            }
            Self::NetworkMtu => {
                write_or_none(&mut value, facts.network_adapter.map(|status| status.mtu))
            }
            Self::NetworkMaxMtu => {
                write_or_none(&mut value, facts.network_adapter.map(|status| status.max_mtu))
            }
            Self::NetworkWakeOnLan => match facts.network_adapter.map(|status| status.wake_on_lan) {
                Some(Some(true)) => value.write_str("on"),
                Some(Some(false)) => value.write_str("off"),
                Some(None) => value.write_str("not available"),
                None => value.write_str(NETWORK_NONE),
            },
            Self::PowerBattery => match facts.power.batteries().next() {
                Some(battery) => {
                    let state = match facts.power.is_low() {
                        true => "low",
                        false => battery.charge.word(),
                    };
                    write!(value, "{}% {state}", battery.percent)
                }
                None => value.write_str("none"),
            },
            Self::PowerLine => value.write_str(fact_word(facts.power.line_power, "online", "offline")),
            Self::PowerLid => value.write_str(fact_word(facts.power.lid_open, "open", "closed")),
            Self::PowerTemperature => match facts.power.hottest() {
                Some(zone) => eggos_device_list::power::write_celsius(&mut value, zone.tenths_c),
                None => value.write_str("none"),
            },
            Self::PowerDock => {
                value.write_str(fact_word(facts.power.docked, "attached", "detached"))
            }
            Self::PowerLidAction => value.write_str(facts.lid_action.name()),
            Self::UpdatePolicy => value.write_str(facts.update_policy.map_or("", UpdatePolicy::name)),
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
            Self::ShellForm => crate::facts::FORM_SETTINGS
                .iter()
                .position(|form| *form == value)
                .map(Change::Form)
                .ok_or(Refusal::InvalidValue),
            Self::AppearanceTheme => Theme::named(value)
                .map(Change::Theme)
                .ok_or(Refusal::InvalidValue),
            Self::KeyboardLayout => eggos_settings_file::keyboard_layout_named(value.as_bytes())
                .map(Change::KeyboardLayout)
                .ok_or(Refusal::InvalidValue),
            Self::KeyboardLayouts => KeyboardLayoutSet::parse(value.as_bytes())
                .map(Change::KeyboardLayouts)
                .map_err(Refusal::InvalidLayouts),
            Self::KeyboardOnScreen => eggos_settings_file::KEYBOARD_ON_SCREEN_MODES
                .iter()
                .position(|mode| *mode == value)
                .map(Change::OnScreenKeyboard)
                .ok_or(Refusal::InvalidValue),
            Self::DisplayRotation => eggos_settings_file::DISPLAY_ROTATIONS
                .iter()
                .position(|rotation| *rotation == value)
                .map(Change::DisplayRotation)
                .ok_or(Refusal::InvalidValue),
            Self::KeyboardSwitchKey => SwitchKey::named(value.as_bytes())
                .map(Change::KeyboardSwitchKey)
                .ok_or(Refusal::InvalidValue),
            Self::TimeZone => crate::facts::zone_index(value)
                .map(Change::TimeZone)
                .ok_or(Refusal::InvalidValue),
            Self::NetworkMtu => value
                .parse::<u32>()
                .ok()
                .filter(|mtu| (crate::network_pane::MIN_MTU..=MOST_MTU).contains(mtu))
                .map(Change::Mtu)
                .ok_or(Refusal::InvalidValue),
            Self::NetworkWakeOnLan => match value {
                "on" => Ok(Change::WakeOnLan(true)),
                "off" => Ok(Change::WakeOnLan(false)),
                _ => Err(Refusal::InvalidValue),
            },
            Self::TimeAutomatic => match value {
                "on" => Ok(Change::Automatic(true)),
                "off" => Ok(Change::Automatic(false)),
                _ => Err(Refusal::InvalidValue),
            },
            Self::DisplayScale => match value {
                "1" => Ok(Change::DisplayScale(1)),
                "2" => Ok(Change::DisplayScale(2)),
                _ => Err(Refusal::InvalidValue),
            },
            Self::DisplayResolution => eggos_settings_file::resolution_named(value.as_bytes())
                .map(Change::DisplayResolution)
                .ok_or(Refusal::InvalidValue),
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
            Self::MouseAcceleration => match value {
                "on" => Ok(Change::Input(InputChange::PointerAcceleration(true))),
                "off" => Ok(Change::Input(InputChange::PointerAcceleration(false))),
                _ => Err(Refusal::InvalidValue),
            },
            Self::KeyboardRepeatDelay => number_within(value, 150, 2000)
                .map(|delay| Change::Input(InputChange::RepeatDelayMs(delay))),
            Self::KeyboardRepeatRate => {
                number_within(value, 2, 60).map(|rate| Change::Input(InputChange::RepeatRate(rate)))
            }
            Self::HighContrast => {
                boolean(value).map(|on| Change::Display(DisplayChange::HighContrast(on)))
            }
            Self::ReduceMotion => {
                boolean(value).map(|on| Change::Display(DisplayChange::ReduceMotion(on)))
            }
            Self::StickyKeys => boolean(value).map(|on| Change::Keys(KeysChange::StickyKeys(on))),
            Self::SlowKeys => boolean(value).map(|on| Change::Keys(KeysChange::SlowKeys(on))),
            Self::SlowKeysMs => number_within(value, 100, 1000)
                .map(|delay| Change::Keys(KeysChange::SlowKeysMs(delay))),
            Self::ScreenReader => boolean(value).map(|on| Change::Reader(ReaderChange::On(on))),
            Self::ScreenReaderCaptions => {
                boolean(value).map(|on| Change::Reader(ReaderChange::Captions(on)))
            }
            Self::ScreenReaderRate => number_within(value, 100, 400)
                .map(|rate| Change::Reader(ReaderChange::Rate(rate))),
            Self::ScreenReaderEcho => match value {
                "characters" => Ok(Change::Reader(ReaderChange::EchoCharacters(true))),
                "none" => Ok(Change::Reader(ReaderChange::EchoCharacters(false))),
                _ => Err(Refusal::InvalidValue),
            },
            Self::Zoom => boolean(value).map(|on| Change::Display(DisplayChange::Zoom(on))),
            Self::ZoomLevel => number_within(value, 2, 8)
                .ok()
                .filter(|level| eggos_settings_file::ZOOM_LEVELS.contains(level))
                .map(|level| Change::Display(DisplayChange::ZoomLevel(level)))
                .ok_or(Refusal::InvalidValue),
            Self::PointerSize => eggos_settings_file::POINTER_SIZES
                .iter()
                .find(|(name, _)| *name == value)
                .map(|&(_, halves)| Change::Display(DisplayChange::PointerSizeHalves(halves)))
                .ok_or(Refusal::InvalidValue),
            Self::BounceKeys => boolean(value).map(|on| Change::Keys(KeysChange::BounceKeys(on))),
            Self::BounceKeysMs => number_within(value, 100, 1000)
                .map(|delay| Change::Keys(KeysChange::BounceKeysMs(delay))),
            Self::ToggleKeys => boolean(value).map(|on| Change::Keys(KeysChange::ToggleKeys(on))),
            Self::MouseKeys => boolean(value).map(|on| Change::Keys(KeysChange::MouseKeys(on))),
            Self::MouseKeysSpeed => number_within(value, 100, 1000)
                .map(|speed| Change::Keys(KeysChange::MouseKeysSpeed(speed))),
            Self::PowerLidAction => eggos_settings_file::LidAction::named(value.as_bytes())
                .map(Change::LidAction)
                .ok_or(Refusal::InvalidValue),
            Self::UpdatePolicy => UpdatePolicy::named(value)
                .map(Change::UpdatePolicy)
                .ok_or(Refusal::InvalidValue),
            Self::MouseDevice(index, field) => {
                let follows = value == eggos_settings_file::FOLLOW_GLOBAL;
                let choice = |on: &str, off: &str| match value {
                    _ if follows => Ok(None),
                    _ if value == on => Ok(Some(true)),
                    _ if value == off => Ok(Some(false)),
                    _ => Err(Refusal::InvalidValue),
                };
                let change = match field {
                    DeviceField::Buttons => DeviceChange::LeftHanded(choice("left", "right")?),
                    DeviceField::Scrolling => {
                        DeviceChange::NaturalScrolling(choice("natural", "traditional")?)
                    }
                    DeviceField::Speed => DeviceChange::Speed(match follows {
                        true => None,
                        false => Some(number_within(value, 1, 10)?),
                    }),
                    DeviceField::Acceleration => DeviceChange::Acceleration(choice("on", "off")?),
                };
                Ok(Change::Device {
                    device: index + 1,
                    change,
                })
            }
            _ => Err(Refusal::ReadOnly),
        }
    }
}

/// What a `network.*` key holds when the network gave nothing for it (D225).
pub const NETWORK_NONE: &str = "none";

fn connected(facts: &Facts) -> Option<&InterfaceStatus> {
    facts
        .network_adapter
        .as_ref()
        .filter(|adapter| adapter.state() == State::Connected)
}

fn write_or_none(value: &mut Value, shown: Option<impl core::fmt::Display>) -> core::fmt::Result {
    match shown {
        Some(shown) => write!(value, "{shown}"),
        None => value.write_str(NETWORK_NONE),
    }
}

/// Writes `time.sync`'s value: `<result> <finished, or started while syncing, in ISO 8601
/// UTC> <server> <moved_ms or offset_ms or 0>`, or `none` without a record (D225).
pub fn write_clock_sync(value: &mut Value, record: Option<&Record>) -> core::fmt::Result {
    let Some(record) = record else {
        return value.write_str("none");
    };
    let when = record.finished.unwrap_or(record.started);
    let iso = eggos_calendar::rfc3339(when).unwrap_or([b'?'; 20]);
    write!(
        value,
        "{} {} {} {}",
        record.outcome.name(),
        core::str::from_utf8(&iso).unwrap_or("?"),
        core::str::from_utf8(record.server()).unwrap_or("?"),
        record.moved_ms
    )
}

fn write_choices(choices: &mut Text<256>, names: &[&str]) {
    let last = names.len().saturating_sub(1);
    for (index, name) in names.iter().enumerate() {
        let separator = match index {
            0 => "",
            _ if index == last => " or ",
            _ => ", ",
        };
        let _ = write!(choices, "{separator}{name}");
    }
}

/// Why `value` was refused for `keyboard.layouts`, as `settings set` says it: the value or
/// the unknown name in it, and the reason (D221).
pub fn layouts_refusal(value: &str, refusal: LayoutSetRefusal) -> (&str, Text<256>) {
    let mut reason = Text::<256>::new();
    let quoted = match refusal {
        LayoutSetRefusal::TooMany => {
            let _ = write!(reason, "at most {MAX_KEYBOARD_LAYOUTS} layouts");
            value
        }
        LayoutSetRefusal::Repeated(number) => {
            let name = KEYBOARD_LAYOUTS
                .get(number)
                .map_or("?", |layout| layout.name);
            let _ = write!(reason, "'{name}' is listed twice");
            value
        }
        LayoutSetRefusal::Unknown { start, end } => {
            reason = Key::KeyboardLayouts.choices();
            value.get(start..end).unwrap_or(value)
        }
    };
    (quoted, reason)
}

fn fact_word(fact: Option<bool>, yes: &'static str, no: &'static str) -> &'static str {
    match fact {
        Some(true) => yes,
        Some(false) => no,
        None => "none",
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
    fn a_device_key_takes_global_or_the_global_key_s_values() {
        assert_eq!(
            change("mouse.device.2.speed", "7"),
            Ok((
                Key::MouseDevice(1, DeviceField::Speed),
                Change::Device {
                    device: 2,
                    change: DeviceChange::Speed(Some(7)),
                }
            ))
        );
        assert_eq!(
            change("mouse.device.1.scrolling", "global").map(|(_, change)| change),
            Ok(Change::Device {
                device: 1,
                change: DeviceChange::NaturalScrolling(None),
            })
        );
        assert_eq!(
            change("mouse.device.1.speed", "11"),
            Err(Refusal::InvalidValue)
        );
        assert_eq!(change("mouse.device.17.speed", "7"), Err(Refusal::NoSuchSetting));
        assert_eq!(change("mouse.devices", "mouse"), Err(Refusal::ReadOnly));
        assert_eq!(
            Key::MouseDevice(0, DeviceField::Buttons).choices().as_str(),
            "global, right or left"
        );
    }

    #[test]
    fn the_list_holds_the_present_devices_and_those_with_values() {
        let mut facts = Facts::example();
        facts.pointing_devices = eggos_device_list::pointers::PointingDevices::parse(b"mouse\n");
        facts.device_input[2].pointer_speed = Some(7);
        let names: Vec<&str> = Key::listed(&facts)
            .into_iter()
            .map(Key::name)
            .filter(|name| name.starts_with("mouse.device"))
            .collect();
        assert_eq!(
            names,
            [
                "mouse.devices",
                "mouse.device.1.buttons",
                "mouse.device.1.scrolling",
                "mouse.device.1.speed",
                "mouse.device.1.acceleration",
                "mouse.device.3.buttons",
                "mouse.device.3.scrolling",
                "mouse.device.3.speed",
                "mouse.device.3.acceleration",
            ]
        );
        assert_eq!(
            Key::MouseDevice(2, DeviceField::Speed).value(&facts).as_str(),
            "7"
        );
    }

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
                ("shell.form", "auto".to_owned()),
                ("shell.form_in_use", "desktop".to_owned()),
                ("input.present", "keyboard,pointer".to_owned()),
                ("appearance.theme", "dark".to_owned()),
                ("display.scale", "1".to_owned()),
                ("display.resolution", "auto".to_owned()),
                ("display.size", "1280x800".to_owned()),
                ("display.resolution_changeable", "yes".to_owned()),
                ("display.rotation", "auto".to_owned()),
                ("display.pointer", "software".to_owned()),
                ("keyboard.layout", "us".to_owned()),
                ("keyboard.layouts", "us".to_owned()),
                ("keyboard.switch_key", "profile".to_owned()),
                ("keyboard.on_screen", "auto".to_owned()),
                ("time.zone", "UTC".to_owned()),
                ("time.automatic", "on".to_owned()),
                ("time.sync", "none".to_owned()),
                ("mouse.buttons", "right".to_owned()),
                ("mouse.scrolling", "traditional".to_owned()),
                ("mouse.speed", "5".to_owned()),
                ("mouse.acceleration", "on".to_owned()),
                ("mouse.devices", String::new()),
                ("keyboard.repeat_delay_ms", "400".to_owned()),
                ("keyboard.repeat_rate", "25".to_owned()),
                ("accessibility.high_contrast", "false".to_owned()),
                ("accessibility.reduce_motion", "false".to_owned()),
                ("accessibility.sticky_keys", "false".to_owned()),
                ("accessibility.slow_keys", "false".to_owned()),
                ("accessibility.slow_keys_ms", "300".to_owned()),
                ("accessibility.screen_reader", "false".to_owned()),
                ("accessibility.screen_reader_captions", "true".to_owned()),
                ("accessibility.screen_reader_rate", "200".to_owned()),
                ("accessibility.screen_reader_echo", "characters".to_owned()),
                ("accessibility.zoom", "false".to_owned()),
                ("accessibility.zoom_level", "2".to_owned()),
                ("accessibility.pointer_size", "1".to_owned()),
                ("accessibility.bounce_keys", "false".to_owned()),
                ("accessibility.bounce_keys_ms", "300".to_owned()),
                ("accessibility.toggle_keys", "false".to_owned()),
                ("accessibility.mouse_keys", "false".to_owned()),
                ("accessibility.mouse_keys_speed", "500".to_owned()),
                ("accessibility.keyboard_navigation", "always".to_owned()),
                ("network.state", "connected".to_owned()),
                ("network.address", "10.0.2.15/24".to_owned()),
                ("network.router", "10.0.2.2".to_owned()),
                ("network.dns", "10.0.2.3".to_owned()),
                ("network.mac", "52:54:00:12:34:56".to_owned()),
                ("power.battery", "87% discharging".to_owned()),
                ("power.line", "offline".to_owned()),
                ("power.lid", "open".to_owned()),
                ("power.temperature", "47.0".to_owned()),
                ("power.dock", "detached".to_owned()),
                ("power.lid_action", "sleep".to_owned()),
                ("system.version", "0.2.0".to_owned()),
                ("system.arch", "aarch64".to_owned()),
                ("system.cpus", "4".to_owned()),
                ("system.memory_mb", "1024".to_owned()),
                ("system.uptime_s", "3725".to_owned()),
            ]
        );
    }

    fn network_values(line: &str) -> [String; 5] {
        let facts = Facts {
            network_adapter: eggos_net::status::adapter(line.as_bytes()),
            ..Facts::example()
        };
        [
            Key::NetworkState,
            Key::NetworkAddress,
            Key::NetworkRouter,
            Key::NetworkDns,
            Key::NetworkMac,
        ]
        .map(|key| key.value(&facts).as_str().to_owned())
    }

    #[test]
    fn the_network_keys_say_the_state_in_net_status_words_and_only_a_connected_address() {
        use crate::facts::EXAMPLE_ADAPTER;
        let three = EXAMPLE_ADAPTER.replacen(" 10.0.2.3 ", " 10.0.2.3,10.0.2.4,10.0.2.5 ", 1);
        assert_eq!(
            network_values(&three),
            [
                "connected",
                "10.0.2.15/24",
                "10.0.2.2",
                "10.0.2.3,10.0.2.4,10.0.2.5",
                "52:54:00:12:34:56",
            ]
        );
        let unplugged = EXAMPLE_ADAPTER.replacen(" up ", " down ", 1);
        assert_eq!(
            network_values(&unplugged),
            [
                "cable-unplugged",
                "none",
                "none",
                "none",
                "52:54:00:12:34:56"
            ]
        );
        let asking = "eth0 up 52:54:00:12:34:56 1500 asking - - - - - - 3 900 0 0 3 1026 0\n";
        assert_eq!(network_values(asking)[0], "getting-address");
        let no_address = asking.replacen("asking", "no-address", 1);
        assert_eq!(network_values(&no_address)[..2], ["no-address", "none"]);
        assert_eq!(network_values(""), ["none", "none", "none", "none", "none"]);
        for key in [
            "network.state",
            "network.address",
            "network.router",
            "network.dns",
            "network.mac",
        ] {
            assert!(!Key::named(key).unwrap().is_settable(), "{key}");
            assert_eq!(change(key, "x"), Err(Refusal::ReadOnly), "{key}");
        }
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
            Ok((Key::KeyboardLayout, Change::KeyboardLayout(11)))
        );
        assert_eq!(change("keyboard.layout", "DE"), Err(Refusal::InvalidValue));
        assert!(
            Key::KeyboardLayout
                .choices()
                .as_str()
                .starts_with("us, us-intl, us-mac, gb, us-dvorak")
        );
        assert!(
            Key::KeyboardLayout
                .choices()
                .as_str()
                .ends_with(", es, latam or se")
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

    #[test]
    fn the_layouts_and_the_switch_key_take_only_their_values() {
        let us_de = KeyboardLayoutSet::parse(b"us,de").unwrap();
        assert_eq!(
            change("keyboard.layouts", "us,de"),
            Ok((Key::KeyboardLayouts, Change::KeyboardLayouts(us_de)))
        );
        for (value, quoted, reason) in [
            (
                "us,de,fr,es,it",
                "us,de,fr,es,it",
                "at most 4 layouts".to_owned(),
            ),
            ("us,us", "us,us", "'us' is listed twice".to_owned()),
            (
                "us,xx",
                "xx",
                Key::KeyboardLayout.choices().as_str().to_owned(),
            ),
        ] {
            let Err(Refusal::InvalidLayouts(refusal)) = change("keyboard.layouts", value) else {
                panic!("{value}");
            };
            let (shown, why) = layouts_refusal(value, refusal);
            assert_eq!((shown, why.as_str()), (quoted, reason.as_str()));
        }
        assert_eq!(
            change("keyboard.switch_key", "alt+shift"),
            Ok((
                Key::KeyboardSwitchKey,
                Change::KeyboardSwitchKey(SwitchKey::AltShift)
            ))
        );
        assert_eq!(
            change("keyboard.switch_key", "x"),
            Err(Refusal::InvalidValue)
        );
        assert_eq!(
            Key::KeyboardSwitchKey.choices().as_str(),
            "profile, super+space, ctrl+space, alt+shift, ctrl+shift or none"
        );
    }

    #[test]
    fn a_layout_set_from_the_cli_keeps_the_layout_in_force_or_makes_the_first_one() {
        let named = |name: &[u8]| eggos_settings_file::keyboard_layout_named(name).unwrap();
        let mut facts = Facts::example();
        facts.keyboard_layouts = KeyboardLayoutSet::parse(b"us,de").unwrap();
        let mut text = [0u8; KEYBOARD_LAYOUT_SET_CAPACITY];
        let chosen = Change::KeyboardLayout(named(b"fr"))
            .keyboard_choice(&facts)
            .unwrap();
        assert_eq!(chosen.layouts.write(&mut text), "us,de,fr");
        assert_eq!(chosen.in_force, named(b"fr"));
        facts.keyboard_layouts = KeyboardLayoutSet::parse(b"us,de,fr,es").unwrap();
        facts.keyboard_layout = named(b"de");
        let chosen = Change::KeyboardLayout(named(b"it"))
            .keyboard_choice(&facts)
            .unwrap();
        assert_eq!(chosen.layouts.write(&mut text), "us,it,fr,es");
        let listed = Change::KeyboardLayouts(KeyboardLayoutSet::parse(b"gb,se").unwrap())
            .keyboard_choice(&facts)
            .unwrap();
        assert_eq!(listed.in_force, named(b"gb"));
        assert_eq!(Change::TimeZone(0).keyboard_choice(&facts), None);
    }
}
