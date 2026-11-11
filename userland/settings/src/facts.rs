//! What Settings reports about the system and the choices it shows, gathered by the program
//! from the kernel (or made up by a test), so the window and `settings` show the same values.

use eggos_net::status::InterfaceStatus;
use eggos_settings_file::clock_sync::Record;
use eggos_settings_file::{
    DisplayAccessibility, InputPreferences, KeyAccessibility, KeyboardLayoutSet, SwitchKey,
};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Theme;

/// The system's facts and the current choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facts {
    /// EggOS's version.
    pub version: &'static str,
    /// The machine's architecture, as `uname -m` says it.
    pub architecture: Text<16>,
    /// CPUs online.
    pub cpus: u32,
    /// Memory in whole MB.
    pub memory_mb: u64,
    /// Seconds since boot.
    pub uptime_s: u64,
    /// The screen's size in pixels.
    pub resolution: (u32, u32),
    /// The size saved for the next boot, `display.resolution`: `None` for the display's own
    /// (D287).
    pub resolution_choice: Option<(u32, u32)>,
    /// Whether the display can change size (`/proc/driver/display`, D287).
    pub resolution_changeable: bool,
    /// The display's scale as saved for the next boot (the scale in force when none is saved).
    pub scale: u32,
    /// The scale the display shows at now (D195).
    pub scale_in_force: u32,
    /// The desktop's layout profile: its index in the settings file's `PROFILES`.
    pub profile: usize,
    /// The desktop's theme, `appearance.theme` (D219).
    pub theme: Theme,
    /// Whether `/home` is on a disk, so a choice survives a power-off.
    pub on_disk: bool,
    /// The wall clock: seconds since 1970-01-01 00:00:00 UTC (D150).
    pub now_s: u64,
    /// Whether the machine has a hardware clock (`/proc/driver/rtc`), which keeps a set time.
    pub hardware_clock: bool,
    /// The keyboard layout: its index in the settings file's `KEYBOARD_LAYOUTS` (D185).
    pub keyboard_layout: usize,
    /// The layouts the switch key cycles through (D221).
    pub keyboard_layouts: KeyboardLayoutSet,
    /// The key that switches layouts (D221).
    pub switch_key: SwitchKey,
    /// Whether the machine has a keyboard.
    pub keyboard: bool,
    /// Whether Caps Lock is on.
    pub caps_lock: bool,
    /// The time zone: its index in `eggos_calendar::zone::ZONES` (D194).
    pub time_zone: usize,
    /// Whether the clock is set from the network, `time.automatic` (D225).
    pub time_automatic: bool,
    /// What the last network clock sync did, from `/run/eggos/clock-sync` (D225).
    pub clock_sync: Option<Record>,
    /// The mouse and key preferences (D195).
    pub input: InputPreferences,
    /// Sticky keys and slow keys (D205).
    pub keys: KeyAccessibility,
    /// High contrast and reduced motion (D224).
    pub display: DisplayAccessibility,
    /// The network adapter as `/proc/net/interfaces` reports it; `None` on a machine without
    /// one (D225).
    pub network_adapter: Option<InterfaceStatus>,
    /// `shell.form`: its index in [`FORM_SETTINGS`] (D303).
    pub form_setting: usize,
    /// The form on screen: its index in [`FORMS_IN_USE`] (D303).
    pub form_in_use: usize,
    /// The input kinds present: touch 1, keyboard 2, pointer 4, remote 8 (D303).
    pub input_kinds: u32,
}

/// The values `shell.form` takes, in `SET_FORM`'s numbering (D303).
pub const FORM_SETTINGS: [&str; 5] = ["auto", "desktop", "tablet", "phone", "tv"];
/// The forms `shell.form_in_use` says, in `GET_FORM`'s numbering (D303).
pub const FORMS_IN_USE: [&str; 5] = ["desktop", "tablet", "phone", "tv", "console"];
/// The input kinds `input.present` names, in their bits' order (D303).
pub const INPUT_KINDS: [&str; 4] = ["touch", "keyboard", "pointer", "remote"];

/// The `/proc/net/interfaces` line of [`Facts::example`]'s adapter: connected as
/// `10.0.2.15/24` through the router `10.0.2.2` with the DNS server `10.0.2.3`, 86342 of 86400
/// seconds of lease left, 3,600,000 bytes received in 2,905 packets and 1,300,000 sent in 812.
pub const EXAMPLE_ADAPTER: &str = "eth0 up 52:54:00:12:34:56 1500 configured 10.0.2.15/24 \
                                   10.0.2.2 10.0.2.3 10.0.2.2 86342 86400 2905 3600000 0 0 812 \
                                   1300000 0\n";

impl Facts {
    /// The facts of a 1280x800 aarch64 machine with 4 CPUs and 1 GiB, up 1:02:05, in the
    /// macOS profile with a disk, a hardware clock reading Thursday 29 October 2026, 14:05:09
    /// UTC, a keyboard in the US layout with Caps Lock off and the network adapter of
    /// [`EXAMPLE_ADAPTER`]: what tests show.
    pub fn example() -> Self {
        Self {
            version: "0.2.0",
            architecture: Text::from("aarch64"),
            cpus: 4,
            memory_mb: 1024,
            uptime_s: 3725,
            resolution: (1280, 800),
            resolution_choice: None,
            resolution_changeable: true,
            scale: 1,
            scale_in_force: 1,
            profile: 1,
            theme: Theme::Dark,
            on_disk: true,
            now_s: 1_793_282_709,
            hardware_clock: true,
            keyboard_layout: 0,
            keyboard_layouts: KeyboardLayoutSet::DEFAULT,
            switch_key: SwitchKey::Profile,
            time_zone: utc_zone(),
            time_automatic: true,
            clock_sync: None,
            keyboard: true,
            caps_lock: false,
            input: InputPreferences::DEFAULT,
            keys: KeyAccessibility::DEFAULT,
            display: DisplayAccessibility {
                high_contrast: false,
                reduce_motion: false,
            },
            network_adapter: eggos_net::status::parse_interface(EXAMPLE_ADAPTER.as_bytes()),
            form_setting: 0,
            form_in_use: 0,
            input_kinds: 6,
        }
    }
}

/// The index of `UTC` in `eggos_calendar::zone::ZONES`.
pub fn utc_zone() -> usize {
    zone_index("UTC").unwrap_or(0)
}

/// The index of the zone `name` in `eggos_calendar::zone::ZONES`.
pub fn zone_index(name: &str) -> Option<usize> {
    eggos_calendar::zone::ZONES
        .binary_search_by(|(zone, _)| (*zone).cmp(name))
        .ok()
}
