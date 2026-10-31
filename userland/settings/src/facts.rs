//! What Settings reports about the system and the choices it shows, gathered by the program
//! from the kernel (or made up by a test), so the window and `settings` show the same values.

use eggos_settings_file::InputPreferences;
use eggos_toolkit::text::Text;

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
    /// The display's scale as saved for the next boot (the scale in force when none is saved).
    pub scale: u32,
    /// The scale the display shows at now (D195).
    pub scale_in_force: u32,
    /// The desktop's layout profile: its index in the settings file's `PROFILES`.
    pub profile: usize,
    /// Whether `/home` is on a disk, so a choice survives a power-off.
    pub on_disk: bool,
    /// The wall clock: seconds since 1970-01-01 00:00:00 UTC (D150).
    pub now_s: u64,
    /// Whether the machine has a hardware clock (`/proc/driver/rtc`), which keeps a set time.
    pub hardware_clock: bool,
    /// The keyboard layout: its index in the settings file's `KEYBOARD_LAYOUTS` (D185).
    pub keyboard_layout: usize,
    /// Whether the machine has a keyboard.
    pub keyboard: bool,
    /// Whether Caps Lock is on.
    pub caps_lock: bool,
    /// The time zone: its index in `eggos_calendar::zone::ZONES` (D194).
    pub time_zone: usize,
    /// The mouse and key preferences (D195).
    pub input: InputPreferences,
}

impl Facts {
    /// The facts of a 1280x800 aarch64 machine with 4 CPUs and 1 GiB, up 1:02:05, in the
    /// macOS profile with a disk, a hardware clock reading Thursday 29 October 2026, 14:05:09
    /// UTC, and a keyboard in the US layout with Caps Lock off: what tests show.
    pub fn example() -> Self {
        Self {
            version: "0.2.0",
            architecture: Text::from("aarch64"),
            cpus: 4,
            memory_mb: 1024,
            uptime_s: 3725,
            resolution: (1280, 800),
            scale: 1,
            scale_in_force: 1,
            profile: 1,
            on_disk: true,
            now_s: 1_793_282_709,
            hardware_clock: true,
            keyboard_layout: 0,
            time_zone: utc_zone(),
            keyboard: true,
            caps_lock: false,
            input: InputPreferences::DEFAULT,
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
