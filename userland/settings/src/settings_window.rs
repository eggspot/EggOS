//! The Settings window (docs/ux/desktop-v0.3-gui.md section 10, docs/ux/toolkit-v0.3.md
//! section 11): the sidebar with its search and its panes, and the panes: Appearance with the
//! profile picker, Display, Keyboard (D185, docs/ux/keyboard-v0.3.md), Mouse, Storage (D192,
//! docs/ux/storage-v0.3.md), Date & time (D150, docs/ux/date-time-v0.3.md), Agents (D193,
//! docs/ux/agents-v0.3.md), Windows (D231, docs/ux/windows-registry-v0.3.md), Components (D235,
//! docs/ux/components-v0.3.md) and About. It holds only widgets and answers what the user does with what the
//! program must do; the program talks to the system.

use crate::about_pane::AboutPane;
use crate::accessibility_pane::{AccessibilityPane, Shown};
use crate::activity::{ActivityQuery, ActivityRead};
use crate::agents_pane::{self, AgentsPane, GrantsRead};
use crate::art;
use crate::automatic_clock::{Adapter, AutomaticClock, ClockWidgets};
use crate::components_pane::{self, ComponentsPane, ComponentsRead};
use crate::facts::Facts;
use crate::input_panes::{self, DisplayPane, InputPanes};
use crate::keys::{DisplayChange, InputChange, KeysChange};
use crate::network_pane::NetworkPane;
use crate::wifi_section::{self, WifiSection};
use crate::network_view::NetworkRead;
use crate::storage_pane::{Reply, StoragePane};
use crate::windows_pane::{self, WindowsPane};
use crate::zone_picker::{self, ZonePicker};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_calendar::{ParseError, parse_local_setting};
use eggos_components::index::Component;
use eggos_components::manager::{Failure, Refusal, RemoveError};
use eggos_eggfs::{CheckMode, DiskCheck};
use eggos_files::storage::{DiskRecord, StorageError};
use eggos_mcp::grant_files::SaveError;
use eggos_mcp::grants::{AgentId, AgentName, GrantStore};
use eggos_mcp::pairing::{Code, PairError};
use eggos_net::status::InterfaceStatus;
use eggos_runtime::errno::Errno;
use eggos_settings_file::clock_sync::Record;
use eggos_settings_file::{KEYBOARD_LAYOUTS, KeyboardLayoutSet, MAX_KEYBOARD_LAYOUTS, SwitchKey};
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::{Theme, Token};
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};
use eggos_windows_registry::registry::Store as RegistryFiles;

/// The window's title.
pub const TITLE: &str = "Settings";
/// The content's size when the window opens.
pub const DEFAULT_SIZE: (u32, u32) = (640, 440);
/// The smallest content the user may resize it to: the three cards must fit the pane.
pub const MINIMUM_SIZE: (u32, u32) = (640, 360);
/// The most widgets the window holds.
pub const CAPACITY: usize = 207
    + windows_pane::WIDGETS
    + components_pane::WIDGETS
    + crate::storage_pane::INSTALLER_WIDGETS
    + crate::wifi_section::WIDGETS;

const SIDEBAR_WIDTH: i32 = 176;
const SEARCH: (i32, i32, i32, i32) = (8, 8, 160, 28);
const LIST_TOP: i32 = 44;
const SIDEBAR_ROW: i32 = 32;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const FIRST_ROW: i32 = 64;
const SETTING_ROW: i32 = 40;
const TEXT_IN_ROW: i32 = 12;
const APPEARANCE_ROWS: i32 = 192;
const NO_DISK_LINE: i32 = 316;
const PANE_HEIGHT: i32 = 348;
const CARD: (i32, i32) = (128, 112);
const CARD_GAP: i32 = 16;
const ABOUT_TEXT_X: i32 = 80;
const ABOUT_LINES: i32 = 104;
const ABOUT_LINE_PITCH: i32 = 20;
const EGG_HEIGHT: i32 = 48;

const AUTOMATIC_ROW: i32 = 1;
const SET_TO_ROW: i32 = 2;
const CONTROL_IN_ROW: i32 = 6;
const SET_FIELD: (i32, i32, i32, i32) = (144, 150, 200, 28);
const SET_BUTTON: (i32, i32, i32, i32) = (352, 150, 96, 28);
const STATUS_LINE: i32 = 196;
const REMEDY_LINE: i32 = 216;
const DATE_AND_TIME_PANE_HEIGHT: i32 = 408;
/// The Display pane's height, its Rotation row included (docs/ux/display-rotation-v0.5.md).
const DISPLAY_PANE_HEIGHT: i32 = 428;

const LAYOUT_LIST: (i32, i32, i32, i32) = (16, 64, 208, 96);
const LAYOUT_ROW: i32 = 24;
const LAYOUT_MARK_X: i32 = 8;
const LAYOUT_TEXT_X: i32 = 24;
const LAYOUT_LABEL_X: i32 = 40;
const LAYOUT_LABEL_TOP: i32 = 68;
const ADD_LAYOUT: (i32, i32, i32, i32) = (16, 168, 208, 28);
const MOVE_UP: (i32, i32, i32, i32) = (16, 204, 100, 28);
const MOVE_DOWN: (i32, i32, i32, i32) = (124, 204, 100, 28);
const REMOVE: (i32, i32, i32, i32) = (16, 240, 96, 28);
const SWITCH_KEY_LABEL: (i32, i32) = (16, 282);
const SWITCH_KEY: (i32, i32, i32, i32) = (96, 276, 128, 28);
const ON_SCREEN_LABEL: (i32, i32) = (16, 318);
const ON_SCREEN: (i32, i32, i32, i32) = (16, 336, 208, 28);
/// The on-screen keyboard's modes as the Keyboard pane offers them, in `keyboard.on_screen`'s
/// order (docs/ux/on-screen-keyboard-v0.5.md 1.1, D317).
const ON_SCREEN_LABELS: [&str; 3] = ["Automatic", "Always", "Never"];
const NO_ROWS: [&str; MAX_KEYBOARD_LAYOUTS] = [""; MAX_KEYBOARD_LAYOUTS];
const FOUR_LAYOUTS: &str = "Four layouts is the most.";
const KEYBOARD_FACTS_X: i32 = 240;
const THIRD_LEVEL_LINE: i32 = 68;
const DEAD_KEYS_LINE: i32 = 88;
const TRY_FIELD: (i32, i32, i32, i32) = (240, 120, 208, 28);
const KEYBOARD_LINE: i32 = 156;

/// The keyboard layouts' titles, in the Keyboard pane's list order (the settings file's
/// `KEYBOARD_LAYOUTS`).
pub const LAYOUT_TITLES: [&str; KEYBOARD_LAYOUTS.len()] = {
    let mut titles = [""; KEYBOARD_LAYOUTS.len()];
    let mut index = 0;
    while index < titles.len() {
        titles[index] = KEYBOARD_LAYOUTS[index].title;
        index += 1;
    }
    titles
};

/// The `Add a layout...` dropdown's options: the prompt, then every layout's title (D221).
pub const ADD_LAYOUT_OPTIONS: [&str; KEYBOARD_LAYOUTS.len() + 1] = {
    let mut options = ["Add a layout..."; KEYBOARD_LAYOUTS.len() + 1];
    let mut index = 0;
    while index < KEYBOARD_LAYOUTS.len() {
        options[index + 1] = KEYBOARD_LAYOUTS[index].title;
        index += 1;
    }
    options
};

const NO_THIRD_LEVEL: &str = "No third level, so no \u{20ac} key";
const ALT_GRAPH_TYPES: &str = "Right Alt (AltGr) types \u{20ac} and more";
const OPTION_TYPES: &str = "Option types \u{20ac} and more";
const MACOS_PROFILE: usize = 1;
const CAPS_LOCK_ON: &str = "Caps Lock is on";
const NO_KEYBOARD: &str = "No keyboard attached.";
const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";

/// The panes, in the sidebar's order.
pub const PANES: [&str; 12] = [
    "Appearance",
    "Network",
    "Display",
    "Keyboard",
    "Mouse",
    "Accessibility",
    "Storage",
    "Date & time",
    "Agents",
    "Windows",
    "Components",
    "About",
];

const APPEARANCE: usize = 0;
/// The `Network` pane's place in [`PANES`] (D225, docs/ux/network-v0.3.md).
pub const NETWORK: usize = 1;
const DISPLAY: usize = 2;
/// The `Keyboard` pane's place in [`PANES`] (D185, docs/ux/keyboard-v0.3.md).
pub const KEYBOARD: usize = 3;
const MOUSE: usize = 4;
/// The `Accessibility` pane's place in [`PANES`] (D205, docs/ux/first-run-accessibility-v0.3.md).
pub const ACCESSIBILITY: usize = 5;
/// The `Storage` pane's place in [`PANES`] (D192, docs/ux/storage-v0.3.md).
pub const STORAGE: usize = 6;
/// The `Date & time` pane's place in [`PANES`] (D150, docs/ux/date-time-v0.3.md).
pub const DATE_AND_TIME: usize = 7;
/// The `Agents` pane's place in [`PANES`] (D193, docs/ux/agents-v0.3.md).
pub const AGENTS: usize = 8;
/// The `Windows` pane's place in [`PANES`] (D231, docs/ux/windows-registry-v0.3.md): its row
/// shows only while the Windows layer is installed.
pub const WINDOWS: usize = 9;
/// The `Components` pane's place in [`PANES`] (D235, docs/ux/components-v0.3.md).
pub const COMPONENTS: usize = 10;
/// The `About` pane's place in [`PANES`].
pub const ABOUT: usize = 11;

const EVERY_PANE_SYNONYMS: [&str; 4] =
    ["Control Panel", "System Settings", "Preferences", "options"];

const PANE_WORDS: [&[&str]; 12] = [
    &[
        "theme",
        "dark",
        "light",
        "dark mode",
        "light mode",
        "appearance",
        "colours",
        "colors",
        "Choose your mode",
        "layout",
        "shortcuts",
        "profile",
        "Windows",
        "macOS",
        "Linux",
        "Primary modifier",
        "Window controls",
    ],
    &[
        "Ethernet",
        "wired",
        "eth0",
        "Wi-Fi",
        "WiFi",
        "wireless",
        "internet",
        "online",
        "connection",
        "LAN",
        "cable",
        "adapter",
        "IP address",
        "IPv4",
        "DNS",
        "DHCP",
        "gateway",
        "router",
        "MAC address",
        "hardware address",
        "MTU",
        "lease",
        "IPv4 address",
        "DNS servers",
        "DHCP server",
        "Traffic",
    ],
    &[
        "Resolution",
        "screen size",
        "mode",
        "1080p",
        "1440p",
        "4K",
        "Full HD",
        "Automatic",
        "Scale",
        "HiDPI",
        "Pointer",
        "screen",
        "monitor",
        "Displays",
        "2x",
        "Retina",
        "DPI",
        "zoom",
        "size of text",
        "restart",
        "Rotation",
        "Orientation",
        "rotate",
        "turn",
        "portrait",
        "landscape",
        "upside down",
        "sideways",
    ],
    &[
        "Layout",
        "keymap",
        "language",
        "input source",
        "AltGr",
        "dead keys",
        "accents",
        "Caps Lock",
        "QWERTY",
        "QWERTZ",
        "AZERTY",
        "international",
        "swiss",
        "belgian",
        "canadian",
        "latin america",
        "portugal",
        "mac",
        "switch key",
        "Key repeat",
        "shortcuts",
        "Repeat delay",
        "Repeat rate",
        "delay until repeat",
        "typing",
        "held key",
    ],
    &[
        "Primary button",
        "Wheel",
        "trackpad",
        "pointer",
        "Buttons",
        "Right-handed",
        "Left-handed",
        "swap",
        "Scrolling",
        "natural scrolling",
        "scroll direction",
        "Pointer speed",
        "sensitivity",
        "tracking speed",
        "acceleration",
        "cursor",
        "touchpad",
    ],
    &[
        "accessibility",
        "a11y",
        "Ease of Access",
        "Universal Access",
        "Sticky keys",
        "Slow keys",
        "Filter keys",
        "keyboard navigation",
        "disability",
    ],
    &[
        "disks",
        "drives",
        "volume",
        "space",
        "free space",
        "format",
        "erase",
        "check",
        "repair",
        "fsck",
        "chkdsk",
        "install",
        "installer",
        "setup",
        "Install EggOS",
        "boot",
        "EFI",
        "GPT",
        "USB",
        "Disk Utility",
        "Disk Management",
        "EggFS",
        "label",
        "partition",
    ],
    &[
        "clock",
        "time",
        "date",
        "time zone",
        "timezone",
        "UTC",
        "RTC",
        "hwclock",
        "hardware clock",
        "calendar",
        "Now",
        "Set to",
        "zone",
        "daylight",
        "DST",
        "summer time",
        "offset",
        "city",
        "IANA",
        "Europe/Paris",
        "Time zone",
        "automatic",
        "automatically",
        "sync",
        "synchronize",
        "synchronise",
        "NTP",
        "SNTP",
        "network time",
        "internet time",
        "time server",
        "pool.ntp.org",
        "Set automatically",
    ],
    &[
        "agent",
        "AI",
        "assistant",
        "MCP",
        "Claude",
        "Copilot",
        "grant",
        "permission",
        "privacy",
        "security",
        "allow",
        "deny",
        "ask",
        "revoke",
        "forget",
        "audit",
        "log",
        "activity",
        "view",
        "write",
        "install",
        "run",
        "Privacy & Security",
        "Add",
        "Revoke all",
    ],
    &[
        "registry",
        "regedit",
        "Registry Editor",
        "reg",
        "hive",
        "HKEY",
        "HKCU",
        "HKLM",
        "HKCR",
        "HKEY_CURRENT_USER",
        "HKEY_LOCAL_MACHINE",
        "HKEY_CLASSES_ROOT",
        "key",
        "value",
        "REG_SZ",
        "DWORD",
        "QWORD",
        "binary",
        "string",
        "Win32",
        "exe",
        ".exe",
        "Windows programs",
        "New key",
        "New value",
        "Edit",
        "Delete",
        "Refresh",
        "Copy path",
    ],
    &[
        "component",
        "components",
        "install",
        "uninstall",
        "remove",
        "add or remove",
        "Add/Remove Programs",
        "Programs and Features",
        "Apps & features",
        "Optional features",
        "Windows features",
        "Software",
        "store",
        "component store",
        "layer",
        "layers",
        "compatibility",
        "Windows apps",
        "Mac apps",
        "Built-in apps",
        "Desktop",
        "size",
        "disk space",
        "Install",
        "Remove",
    ],
    &[
        "version",
        "system",
        "licence",
        "license",
        "memory",
        "CPU",
        "uptime",
        "devices",
        "hardware",
        "driver",
        "drivers",
        "Device Manager",
        "System Information",
        "System Report",
        "PCI",
        "virtio",
        "GPU",
        "graphics",
    ],
];

/// The profile cards' labels, in the settings file's `PROFILES` order.
pub const PROFILE_LABELS: [&str; 3] = ["Windows", "macOS", "Linux"];

/// What the program must do for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Write `/var/lib/eggos/wifi.conf` naming this network and passphrase, as `wifi connect`
    /// does, then say how it went ([`SettingsWindow::wifi_join_saved`]) (D283).
    WifiJoin {
        name: Text<{ wifi_section::MAX_NAME_BYTES }>,
        passphrase: Text<{ wifi_section::MAX_PASSPHRASE_BYTES }>,
        /// Mark it `hidden=yes`, so the driver asks for it by name (D298, D319).
        hidden: bool,
    },
    /// Remove `/var/lib/eggos/wifi.conf`, as `wifi forget` does (D283).
    WifiForget,
    /// Show the Components pane (D283: the Wi-Fi firmware is installed there).
    OpenComponents,
    /// Make the profile at this index of the settings file's `PROFILES` the desktop's and
    /// save it.
    SetProfile(usize),
    /// Make this theme the desktop's and save it as `appearance.theme` (D219).
    SetTheme(Theme),
    /// Set the clock to these seconds since the epoch, then say how it went
    /// ([`SettingsWindow::show_set_outcome`]).
    SetTime(u64),
    /// Make the keyboard layout at this index of the settings file's `KEYBOARD_LAYOUTS` the
    /// keyboard's and save it (D185).
    SetKeyboardLayout(usize),
    /// Make these the layouts the switch key cycles through, `in_force` the one in force,
    /// and save both (D221).
    SetKeyboardLayouts {
        layouts: KeyboardLayoutSet,
        in_force: usize,
    },
    /// Make this the key that switches layouts and save it (D221).
    SetSwitchKey(SwitchKey),
    /// Save and apply `keyboard.on_screen` as mode `0` to `2` (auto, always, never) (D317).
    SetOnScreenKeyboard(usize),
    /// Save the time zone at this index of `eggos_calendar::zone::ZONES` as `time.zone`, then
    /// say how it went ([`SettingsWindow::time_zone_saved`]) (D194).
    SetTimeZone(usize),
    /// Save `time.automatic` as `on`, then, when `sync_now` and the save worked, start
    /// `date --sync --automatic`, then say how both went
    /// ([`SettingsWindow::time_automatic_saved`]) (D225).
    SetTimeAutomatic { on: bool, sync_now: bool },
    /// Save this scale, 1 or 2, as `display.scale` for the next boot, then say how it went
    /// ([`SettingsWindow::display_scale_saved`]) (D195).
    SetDisplayScale(u32),
    /// Save this size, `None` for the display's own, as `display.resolution` for the next boot,
    /// then say how it went ([`SettingsWindow::display_resolution_saved`]) (D287).
    SetDisplayResolution(Option<(u32, u32)>),
    /// Save and apply `shell.form` as the setting numbered this in
    /// [`crate::facts::FORM_SETTINGS`] (D303).
    SetShellForm(usize),
    /// Save, then apply, `display.rotation` as the value numbered this in
    /// `DISPLAY_ROTATIONS`, then say how both went ([`SettingsWindow::display_rotation_saved`],
    /// D333).
    SetDisplayRotation(usize),
    /// Save this mouse or key preference, then apply it, then say how both went
    /// ([`SettingsWindow::input_changed`]) (D195).
    ChangeInput(InputChange),
    /// Save this key accessibility setting, then apply it, then say how both went
    /// ([`SettingsWindow::keys_changed`]) (D205).
    ChangeKeys(KeysChange),
    /// Save this display accessibility setting, then apply it, then say how both went
    /// ([`SettingsWindow::display_changed`]) (D224).
    ChangeDisplay(DisplayChange),
    /// Read the disks and show them ([`SettingsWindow::show_disks`]) (D192).
    ReadDisks,
    /// Check this disk without writing, read the disks again, then show both
    /// ([`SettingsWindow::show_disks`], [`SettingsWindow::show_check`]).
    CheckDisk(&'static str),
    /// Repair this disk, read the disks again, then show both.
    RepairDisk(&'static str),
    /// Format this disk with the label [`SettingsWindow::format_label`], read the disks
    /// again, then show both ([`SettingsWindow::show_formatted`]).
    FormatDisk(&'static str),
    /// Install EggOS on this disk (D241), read the disks again, then show both
    /// ([`SettingsWindow::show_installed`]).
    InstallDisk(&'static str),
    /// Read the agents' grants and show them ([`SettingsWindow::show_grants`]) (D193).
    ReadGrants,
    /// Write [`SettingsWindow::grants_to_write`] as the grants file, read it again, then show
    /// both ([`SettingsWindow::grants_written`]).
    WriteGrants,
    /// Read the audit log's rows [`SettingsWindow::activity_query`] asks for and show them
    /// ([`SettingsWindow::show_activity`]) (D193).
    ReadActivity,
    /// Read the network file, the certificate and the pairing file and show them
    /// ([`SettingsWindow::show_network`]) (D193).
    ReadNetwork,
    /// Turn the network server on with this port, or off with `None`, from the next start, read
    /// the network again, then show both ([`SettingsWindow::network_turned`]).
    TurnNetwork(Option<u16>),
    /// Pair the agent [`SettingsWindow::to_pair`] names, read the grants and the network again,
    /// then show them ([`SettingsWindow::paired`]).
    BeginPairing,
    /// Read `/proc/driver/devices` and show it in About ([`SettingsWindow::show_devices`])
    /// (D220).
    ReadDevices,
    /// Read the component store and the installed components and show them
    /// ([`SettingsWindow::show_components`]) (D235).
    ReadComponents,
    /// Check that the component [`ComponentsPane::to_check`] names may be removed, then show
    /// how ([`SettingsWindow::show_remove_check`]).
    CheckComponentRemove,
    /// Install the component [`ComponentsPane::to_install`] names, then show how it went with
    /// the components read again ([`SettingsWindow::show_installed`]).
    InstallComponent,
    /// Remove the component [`ComponentsPane::to_remove`] names, then show how it went with
    /// the components read again ([`SettingsWindow::show_removed`]).
    RemoveComponent,
    /// Put [`WindowsPane::to_copy`] on the clipboard (D231).
    CopyToClipboard,
    /// Start the job [`SettingsWindow::take_component_job`] gives as `component ...` outside
    /// the window, and read the components every 100 ms while one the pane waits for runs;
    /// then [`SettingsWindow::show_component_job_ended`] (D300).
    StartComponentJob,
}

/// How setting the clock went, as the `Date & time` pane's status line says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    /// The clock is set, and the hardware clock holds the new time.
    Saved,
    /// The clock is set, but there is no hardware clock (or it did not take the time).
    NoHardwareClock,
    /// The system refused: this user may not set the clock.
    NotAllowed,
    /// The system could not set the clock for another reason.
    Failed,
}

const SHOWN_WITH_HARDWARE_CLOCK: &str = "From the hardware clock, read at boot.";
const SHOWN_WITHOUT_HARDWARE_CLOCK: &str = "No hardware clock: the time started at 1970 at boot.";

/// The primary modifier of each profile, in `PROFILES` order.
pub const PRIMARY_MODIFIERS: [&str; 3] = ["Ctrl", "Cmd (Super key)", "Ctrl"];
/// The theme radios' labels, at each theme's number.
pub const THEME_LABELS: [&str; 2] = ["Dark", "Light"];
/// The side of each profile's window controls.
pub const CONTROL_SIDES: [&str; 3] = ["Right", "Left", "Right"];

#[derive(Debug, Clone, Copy)]
struct Widgets {
    search: WidgetId,
    list: WidgetId,
    panes: [WidgetId; PANES.len()],
    cards: WidgetId,
    modifier: WidgetId,
    controls: WidgetId,
    theme: WidgetId,
    uptime: WidgetId,
    now: WidgetId,
    set_field: WidgetId,
    set_button: WidgetId,
    status: WidgetId,
    layouts: WidgetId,
    layout_titles: [WidgetId; MAX_KEYBOARD_LAYOUTS],
    add_layout: WidgetId,
    move_up: WidgetId,
    move_down: WidgetId,
    remove: WidgetId,
    switch_key: WidgetId,
    on_screen_keyboard: WidgetId,
    keyboard_status: WidgetId,
    third_level: WidgetId,
    dead_keys: WidgetId,
    try_field: WidgetId,
    keyboard_line: WidgetId,
}

/// The Settings window's widgets and what they show.
#[derive(Debug)]
pub struct SettingsWindow {
    tree: WidgetTree<CAPACITY>,
    widgets: Widgets,
    storage: StoragePane,
    agents: AgentsPane,
    zones: ZonePicker,
    clock: AutomaticClock,
    display: DisplayPane,
    inputs: InputPanes,
    accessibility: AccessibilityPane,
    about: AboutPane,
    network: NetworkPane,
    wifi: WifiSection,
    windows: WindowsPane,
    components: ComponentsPane,
    windows_layer: bool,
    facts: Facts,
    pane: usize,
}

fn every_pane_but_windows() -> u32 {
    ((1u32 << PANES.len()) - 1) & !(1 << WINDOWS)
}

fn lowercase_contains(haystack: &str, needle: &str) -> bool {
    eggos_font::contains_folded(haystack, needle)
}

/// The pane `settings --pane` names `name` (D221): its title in lower case, its spaces as `-`
/// and `&` as `and`.
pub fn pane_named(name: &str) -> Option<usize> {
    PANES.iter().position(|title| {
        let mut wanted = name.bytes();
        let matched = title.split(' ').enumerate().all(|(index, word)| {
            let word = if word == "&" { "and" } else { word };
            (index == 0 || wanted.next() == Some(b'-'))
                && word
                    .bytes()
                    .all(|byte| wanted.next() == Some(byte.to_ascii_lowercase()))
        });
        matched && wanted.next().is_none()
    })
}

/// Whether pane `pane` stays in the sidebar while the search holds `query`: the query is in
/// its name, a synonym or one of its settings' names, ignoring case.
pub fn pane_matches(pane: usize, query: &str) -> bool {
    let query = query.trim();
    let titles: &[&str] = match pane {
        KEYBOARD => &LAYOUT_TITLES,
        _ => &[],
    };
    lowercase_contains(PANES[pane], query)
        || EVERY_PANE_SYNONYMS
            .iter()
            .chain(PANE_WORDS[pane])
            .chain(titles)
            .any(|word| lowercase_contains(word, query))
}

pub(crate) fn third_level_text(layout: usize, profile: usize) -> &'static str {
    match (KEYBOARD_LAYOUTS[layout].third_level, profile) {
        (false, _) => NO_THIRD_LEVEL,
        (true, MACOS_PROFILE) => OPTION_TYPES,
        (true, _) => ALT_GRAPH_TYPES,
    }
}

pub(crate) fn dead_keys_text(layout: usize) -> Text<64> {
    let mut text = Text::new();
    let _ = match KEYBOARD_LAYOUTS[layout].dead_keys {
        "" => text.write_str("No dead keys"),
        accents => write!(text, "Dead keys: {accents}"),
    };
    text
}

fn layout_index(facts: &Facts) -> usize {
    facts.keyboard_layout.min(KEYBOARD_LAYOUTS.len() - 1)
}

fn uptime_text(seconds: u64) -> Text<64> {
    let mut text = Text::new();
    let _ = write!(
        text,
        "Up {}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    text
}

pub(crate) fn time_text(seconds: u64, zone: usize) -> Text<64> {
    let rule = eggos_calendar::zone::ZONES
        .get(zone)
        .and_then(|(_, rule)| eggos_calendar::zone::parse(rule));
    let mut text = Text::new();
    let _ = match rule {
        Some(rule) => {
            eggos_calendar::format::sentence_in(&mut text, seconds as i64, &rule.at(seconds as i64))
        }
        None => eggos_calendar::format::sentence(&mut text, seconds as i64),
    };
    text
}

pub(crate) fn shown_status(facts: &Facts) -> &'static str {
    match facts.hardware_clock {
        true => SHOWN_WITH_HARDWARE_CLOCK,
        false => SHOWN_WITHOUT_HARDWARE_CLOCK,
    }
}

struct Builder<'a> {
    tree: &'a mut WidgetTree<CAPACITY>,
}

impl Builder<'_> {
    fn add(&mut self, parent: Option<WidgetId>, place: Place, kind: Kind) -> WidgetId {
        self.tree
            .add(parent, place, kind)
            .expect("the Settings window fits its tree")
    }

    fn label(&mut self, parent: WidgetId, x: i32, y: i32, text: &str, ink: Token) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(x, y, Length::Content, Length::Content),
            Kind::label(text, ink),
        )
    }

    fn value(&mut self, parent: WidgetId, y: i32, text: &str) -> WidgetId {
        self.add(
            Some(parent),
            Place::right(PADDING, y),
            Kind::label(text, Token::Text),
        )
    }

    fn heading(&mut self, parent: WidgetId, text: &str) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading(text),
        )
    }

    fn pane(&mut self, parent: WidgetId, height: Length) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(0, 0, Length::Fill(0), height),
            Kind::Panel {
                fill: None,
                border_right: None,
            },
        )
    }

    fn row(&mut self, pane: WidgetId, row: i32, name: &str, value: &str) -> WidgetId {
        let y = FIRST_ROW + row * SETTING_ROW + TEXT_IN_ROW;
        self.label(pane, PADDING, y, name, Token::Text);
        self.value(pane, y, value)
    }
}

impl SettingsWindow {
    /// The window showing `facts`, the Appearance pane first, the search field focused.
    pub fn new(facts: Facts) -> Self {
        let mut tree = WidgetTree::new();
        let mut build = Builder { tree: &mut tree };
        let sidebar = build.add(
            None,
            Place::at(0, 0, Length::Px(SIDEBAR_WIDTH), Length::Fill(0)),
            Kind::Panel {
                fill: Some(Token::Dock),
                border_right: Some(Token::MenuBorder),
            },
        );
        let search = build.add(
            Some(sidebar),
            Place::at(
                SEARCH.0,
                SEARCH.1,
                Length::Px(SEARCH.2),
                Length::Px(SEARCH.3),
            ),
            Kind::text_field("Search settings", Escape::Clear),
        );
        let list = build.add(
            Some(sidebar),
            Place::at(0, LIST_TOP, Length::Fill(1), Length::Fill(0)),
            Kind::list(&PANES, SIDEBAR_ROW, 4, 16),
        );
        let pane_area = build.add(
            None,
            Place::at(SIDEBAR_WIDTH, 0, Length::Fill(0), Length::Fill(0)),
            Kind::panel(Token::Menu),
        );
        let scroll = build.add(Some(pane_area), Place::fill(), Kind::scroll_view());
        let panes: [WidgetId; PANES.len()] = core::array::from_fn(|pane| match pane {
            ABOUT => build.pane(scroll, Length::Fill(0)),
            DATE_AND_TIME => build.pane(scroll, Length::Px(DATE_AND_TIME_PANE_HEIGHT)),
            DISPLAY => build.pane(scroll, Length::Px(DISPLAY_PANE_HEIGHT)),
            _ => build.pane(scroll, Length::Px(PANE_HEIGHT)),
        });
        let profile = facts.profile.min(PROFILE_LABELS.len() - 1);

        let appearance = panes[APPEARANCE];
        build.heading(appearance, "Where are you coming from?");
        let cards = build.add(
            Some(appearance),
            Place::at(PADDING, FIRST_ROW, Length::Content, Length::Content),
            Kind::cards(
                &PROFILE_LABELS,
                profile,
                CARD,
                CARD_GAP,
                art::profile_miniature,
            ),
        );
        let row_y = |row: i32| APPEARANCE_ROWS + row * SETTING_ROW;
        build.label(
            appearance,
            PADDING,
            row_y(0) + TEXT_IN_ROW,
            "Primary modifier",
            Token::Text,
        );
        let modifier = build.value(
            appearance,
            row_y(0) + TEXT_IN_ROW,
            PRIMARY_MODIFIERS[profile],
        );
        build.label(
            appearance,
            PADDING,
            row_y(1) + TEXT_IN_ROW,
            "Window controls",
            Token::Text,
        );
        let controls = build.value(appearance, row_y(1) + TEXT_IN_ROW, CONTROL_SIDES[profile]);
        build.label(
            appearance,
            PADDING,
            row_y(2) + TEXT_IN_ROW,
            "Theme",
            Token::Text,
        );
        let theme = build.add(
            Some(appearance),
            Place::right(PADDING, row_y(2)),
            Kind::radios(&THEME_LABELS, facts.theme.number() as usize, true),
        );
        let no_disk = build.label(appearance, PADDING, NO_DISK_LINE, NO_DISK, Token::TextDim);

        build.heading(panes[DISPLAY], PANES[DISPLAY]);
        build.heading(panes[MOUSE], PANES[MOUSE]);
        build.heading(panes[ACCESSIBILITY], PANES[ACCESSIBILITY]);

        let keyboard = panes[KEYBOARD];
        build.heading(keyboard, PANES[KEYBOARD]);
        let layout = layout_index(&facts);
        let layouts = build.add(
            Some(keyboard),
            Place::at(
                LAYOUT_LIST.0,
                LAYOUT_LIST.1,
                Length::Px(LAYOUT_LIST.2),
                Length::Px(LAYOUT_LIST.3),
            ),
            Kind::list(&NO_ROWS, LAYOUT_ROW, LAYOUT_MARK_X, LAYOUT_TEXT_X),
        );
        let layout_titles = core::array::from_fn(|row| {
            build.label(
                keyboard,
                LAYOUT_LABEL_X,
                LAYOUT_LABEL_TOP + row as i32 * LAYOUT_ROW,
                "",
                Token::Text,
            )
        });
        let at = |(x, y, width, height): (i32, i32, i32, i32)| {
            Place::at(x, y, Length::Px(width), Length::Px(height))
        };
        let add_layout = build.add(
            Some(keyboard),
            at(ADD_LAYOUT),
            Kind::dropdown("Add a layout", &ADD_LAYOUT_OPTIONS, 0),
        );
        let move_up = build.add(Some(keyboard), at(MOVE_UP), Kind::button("Move up", false));
        let move_down = build.add(
            Some(keyboard),
            at(MOVE_DOWN),
            Kind::button("Move down", false),
        );
        let remove = build.add(Some(keyboard), at(REMOVE), Kind::button("Remove", false));
        build.label(
            keyboard,
            SWITCH_KEY_LABEL.0,
            SWITCH_KEY_LABEL.1,
            "Switch key",
            Token::Text,
        );
        let switch_key = build.add(
            Some(keyboard),
            at(SWITCH_KEY),
            Kind::dropdown("Switch key", &SwitchKey::LABELS, facts.switch_key.number()),
        );
        build.label(
            keyboard,
            ON_SCREEN_LABEL.0,
            ON_SCREEN_LABEL.1,
            "On-screen keyboard",
            Token::Text,
        );
        let on_screen_keyboard = build.add(
            Some(keyboard),
            at(ON_SCREEN),
            Kind::dropdown("On-screen keyboard", &ON_SCREEN_LABELS, facts.on_screen_keyboard.min(2)),
        );
        let third_level = build.label(
            keyboard,
            KEYBOARD_FACTS_X,
            THIRD_LEVEL_LINE,
            third_level_text(layout, profile),
            Token::Text,
        );
        let dead_keys = build.label(
            keyboard,
            KEYBOARD_FACTS_X,
            DEAD_KEYS_LINE,
            dead_keys_text(layout).as_str(),
            Token::Text,
        );
        let try_field = build.add(
            Some(keyboard),
            Place::at(
                TRY_FIELD.0,
                TRY_FIELD.1,
                Length::Px(TRY_FIELD.2),
                Length::Px(TRY_FIELD.3),
            ),
            Kind::text_field("Type here to try it", Escape::Clear),
        );
        let keyboard_line = build.label(
            keyboard,
            KEYBOARD_FACTS_X,
            KEYBOARD_LINE,
            CAPS_LOCK_ON,
            Token::Text,
        );
        let keyboard_no_disk =
            build.label(keyboard, PADDING, NO_DISK_LINE, NO_DISK, Token::TextDim);

        let date_and_time = panes[DATE_AND_TIME];
        build.heading(date_and_time, PANES[DATE_AND_TIME]);
        let now = build.row(
            date_and_time,
            0,
            "Now",
            time_text(facts.now_s, facts.time_zone).as_str(),
        );
        let automatic = build.add(
            Some(date_and_time),
            Place::at(
                PADDING,
                FIRST_ROW + AUTOMATIC_ROW * SETTING_ROW + CONTROL_IN_ROW,
                Length::Content,
                Length::Content,
            ),
            Kind::checkbox(crate::automatic_clock::LABEL, facts.time_automatic),
        );
        let set_to = build.label(
            date_and_time,
            PADDING,
            FIRST_ROW + SET_TO_ROW * SETTING_ROW + TEXT_IN_ROW,
            "Set to",
            Token::Text,
        );
        let set_field = build.add(
            Some(date_and_time),
            Place::at(
                SET_FIELD.0,
                SET_FIELD.1,
                Length::Px(SET_FIELD.2),
                Length::Px(SET_FIELD.3),
            ),
            Kind::text_field("YYYY-MM-DD HH:MM", Escape::Clear),
        );
        let set_button = build.add(
            Some(date_and_time),
            Place::at(
                SET_BUTTON.0,
                SET_BUTTON.1,
                Length::Px(SET_BUTTON.2),
                Length::Px(SET_BUTTON.3),
            ),
            Kind::button("Set", true),
        );
        let status = build.label(
            date_and_time,
            PADDING,
            STATUS_LINE,
            shown_status(&facts),
            Token::TextDim,
        );
        let remedy = build.label(date_and_time, PADDING, REMEDY_LINE, "", Token::TextDim);

        let about = panes[ABOUT];
        build.add(
            Some(about),
            Place::at(
                PADDING,
                FIRST_ROW,
                Length::Px(EGG_HEIGHT),
                Length::Px(EGG_HEIGHT),
            ),
            Kind::Custom {
                painter: art::egg,
                data: 0,
            },
        );
        let mut title = Text::<64>::new();
        let _ = write!(title, "EggOS {}", facts.version);
        build.add(
            Some(about),
            Place::at(ABOUT_TEXT_X, FIRST_ROW, Length::Content, Length::Content),
            Kind::heading(title.as_str()),
        );
        let mut machine = Text::<64>::new();
        let _ = write!(
            machine,
            "{} \u{b7} {} CPUs \u{b7} {} MB",
            facts.architecture.as_str(),
            facts.cpus,
            facts.memory_mb
        );
        build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES,
            machine.as_str(),
            Token::Text,
        );
        let uptime = build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + ABOUT_LINE_PITCH,
            uptime_text(facts.uptime_s).as_str(),
            Token::Text,
        );
        build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + 2 * ABOUT_LINE_PITCH,
            "MIT OR Apache-2.0",
            Token::Text,
        );

        let storage = StoragePane::build(&mut tree, panes[STORAGE], set_button);
        let windows = WindowsPane::build(&mut tree, panes[WINDOWS], set_button);
        let components =
            ComponentsPane::build(&mut tree, panes[COMPONENTS], PANE_HEIGHT, set_button);
        let agents = AgentsPane::build(&mut tree, panes[AGENTS], set_button);
        let display = DisplayPane::build(&mut tree, panes[DISPLAY], &facts);
        let inputs = InputPanes::build(
            &mut tree,
            panes[MOUSE],
            panes[KEYBOARD],
            keyboard_no_disk,
            facts.input,
        );
        let accessibility =
            AccessibilityPane::build(&mut tree, panes[ACCESSIBILITY], Shown::of(&facts));
        let about_devices = AboutPane::build(&mut tree, panes[ABOUT]);
        let network = NetworkPane::build(
            &mut tree,
            panes[NETWORK],
            PANES[NETWORK],
            facts.network_adapter.as_ref(),
        );
        let wifi = WifiSection::build(&mut tree, panes[NETWORK], set_button);
        let zones = ZonePicker::build(
            &mut tree,
            panes[DATE_AND_TIME],
            facts.time_zone,
            facts.now_s as i64,
        );
        tree.set_enabled(set_button, false);
        tree.set_default_button(set_button);
        let clock = AutomaticClock::new(
            &mut tree,
            ClockWidgets {
                check_box: automatic,
                set_to,
                field: set_field,
                button: set_button,
                line_1: status,
                line_2: remedy,
            },
            &facts,
        );
        tree.set_shown(no_disk, !facts.on_disk);
        tree.set_shown(keyboard_no_disk, !facts.on_disk);
        for (index, pane) in panes.iter().enumerate() {
            tree.set_shown(*pane, index == APPEARANCE);
        }
        tree.set_shown_items(list, every_pane_but_windows());
        tree.set_focus(Some(search), 0);
        let (width, height) = DEFAULT_SIZE;
        tree.resize(width as i32, height as i32);
        let mut window = Self {
            tree,
            widgets: Widgets {
                search,
                list,
                panes,
                cards,
                modifier,
                controls,
                theme,
                uptime,
                now,
                set_field,
                set_button,
                status,
                layouts,
                layout_titles,
                add_layout,
                move_up,
                move_down,
                remove,
                switch_key,
                on_screen_keyboard,
                keyboard_status: keyboard_no_disk,
                third_level,
                dead_keys,
                try_field,
                keyboard_line,
            },
            storage,
            agents,
            zones,
            clock,
            display,
            inputs,
            accessibility,
            about: about_devices,
            network,
            wifi,
            windows,
            components,
            windows_layer: false,
            facts,
            pane: APPEARANCE,
        };
        window.show_keyboard(facts.keyboard, facts.caps_lock);
        window.show_layout_set();
        window
    }

    /// The window's widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The window's widgets, to draw them.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    /// The pane shown.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// Where the card of profile `index` is in the window.
    pub fn card_rect(&self, index: usize) -> Option<eggos_toolkit::geometry::Rect> {
        let kind = self.tree.kind(self.widgets.cards)?;
        let rect = self.tree.rect(self.widgets.cards)?;
        Some(eggos_toolkit::widget::card_rect(kind, rect, index))
    }

    /// The resolution shown in the Display pane.
    pub fn resolution_shown(&self) -> Option<&str> {
        self.display.resolution_shown(&self.tree)
    }

    /// The Display pane.
    pub fn display_pane(&self) -> &DisplayPane {
        &self.display
    }

    /// The Mouse pane and the Keyboard pane's repeat rows.
    pub fn input_panes(&self) -> &InputPanes {
        &self.inputs
    }

    /// The Accessibility pane.
    pub fn accessibility_pane(&self) -> &AccessibilityPane {
        &self.accessibility
    }

    /// How saving, then applying, a key accessibility setting went ([`Request::ChangeKeys`]):
    /// `applied` is only looked at when `saved` worked.
    pub fn keys_changed(
        &mut self,
        change: KeysChange,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        if saved.is_ok() {
            self.facts.keys = change.applied_to(self.facts.keys);
        }
        self.accessibility
            .changed(&mut self.tree, Shown::of(&self.facts), saved, applied);
    }

    /// How saving, then applying, a display accessibility setting went
    /// ([`Request::ChangeDisplay`]): `applied` is only looked at when `saved` worked.
    pub fn display_changed(
        &mut self,
        change: DisplayChange,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        if saved.is_ok() {
            self.facts.display = change.applied_to(self.facts.display);
        }
        self.accessibility
            .changed(&mut self.tree, Shown::of(&self.facts), saved, applied);
    }

    /// How setting `shell.form` went ([`Request::SetShellForm`]), with the form shown after.
    pub fn shell_form_saved(&mut self, setting: usize, form_in_use: usize, saved: Result<(), Errno>) {
        if saved.is_ok() {
            self.facts.form_setting = setting;
        }
        self.facts.form_in_use = form_in_use;
        self.display.layout_saved(&mut self.tree, &self.facts, saved);
    }

    /// How setting `display.rotation` went ([`Request::SetDisplayRotation`]): saved, then
    /// applied (D333).
    pub fn display_rotation_saved(&mut self, index: usize, saved: Result<(), Errno>, applied: Result<(), Errno>) {
        if saved.is_ok() {
            self.facts.rotation = index;
        }
        self.display.rotation_saved(&mut self.tree, &self.facts, saved, applied);
    }

    /// How saving the display's size went ([`Request::SetDisplayResolution`]).
    pub fn display_resolution_saved(&mut self, size: Option<(u32, u32)>, saved: Result<(), Errno>) {
        if saved.is_ok() {
            self.facts.resolution_choice = size;
        }
        self.display.resolution_saved(&mut self.tree, &self.facts, saved);
    }

    /// How saving the display's scale went ([`Request::SetDisplayScale`]).
    pub fn display_scale_saved(&mut self, scale: u32, saved: Result<(), Errno>) {
        if saved.is_ok() {
            self.facts.scale = scale;
        }
        self.display.saved(&mut self.tree, &self.facts, saved);
    }

    /// How saving, then applying, a mouse or key preference went ([`Request::ChangeInput`]):
    /// `applied` is only looked at when `saved` worked.
    pub fn input_changed(
        &mut self,
        change: InputChange,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        if saved.is_ok() {
            self.facts.input = change.applied_to(self.facts.input);
        }
        self.inputs
            .changed(&mut self.tree, change, self.facts.input, saved, applied);
    }

    fn show_pane(&mut self, pane: usize) -> Option<Request> {
        if pane == self.pane || pane >= PANES.len() {
            return None;
        }
        self.tree.set_shown(self.widgets.panes[self.pane], false);
        self.tree.set_shown(self.widgets.panes[pane], true);
        self.pane = pane;
        if pane == DATE_AND_TIME {
            self.reset_date_and_time();
        }
        if pane == KEYBOARD {
            self.tree.set_text(self.widgets.try_field, "");
        }
        if pane == DISPLAY {
            self.display.show(&mut self.tree, &self.facts);
        }
        if pane == MOUSE || pane == KEYBOARD {
            self.inputs.reset(&mut self.tree, &self.facts);
        }
        if pane == ACCESSIBILITY {
            self.accessibility.reset(&mut self.tree, &self.facts);
        }
        if pane == STORAGE {
            self.storage.reset(&mut self.tree);
            return Some(Request::ReadDisks);
        }
        if pane == AGENTS {
            self.agents.reset(&mut self.tree);
            return Some(Request::ReadGrants);
        }
        if pane == ABOUT {
            self.about.reset(&mut self.tree);
            return Some(Request::ReadDevices);
        }
        if pane == WINDOWS {
            self.windows.reset(&mut self.tree);
        }
        if pane == COMPONENTS {
            self.components.reset(&mut self.tree);
            return Some(Request::ReadComponents);
        }
        None
    }

    /// Shows whether the Windows layer is installed (D231): the `Windows` row shows only while
    /// it is; when it goes while its pane shows, `Components` shows instead.
    pub fn show_windows_layer(&mut self, installed: bool) -> Option<Request> {
        if installed == self.windows_layer {
            return None;
        }
        self.windows_layer = installed;
        let request = match (installed, self.pane == WINDOWS) {
            (false, true) => self.open_pane(COMPONENTS),
            _ => None,
        };
        self.filter_list();
        request
    }

    /// Whether the `Windows` row shows.
    pub fn windows_layer_shown(&self) -> bool {
        self.windows_layer
    }

    /// Gives the Windows pane the registry's files (D231).
    pub fn attach_registry(&mut self, files: Box<dyn RegistryFiles>) {
        self.windows.attach(&mut self.tree, files);
    }

    /// The Windows pane.
    pub fn windows_pane(&self) -> &WindowsPane {
        &self.windows
    }

    /// The Components pane.
    pub fn components_pane(&self) -> &ComponentsPane {
        &self.components
    }

    /// Shows the component store and the installed components just read (D235).
    pub fn show_components(&mut self, read: ComponentsRead) {
        self.components.show_components(&mut self.tree, read);
    }

    /// Shows how checking a component's remove went, at `now_ms`.
    pub fn show_remove_check(&mut self, checked: Result<Option<Component>, Refusal>, now_ms: u32) {
        self.components
            .show_remove_check(&mut self.tree, checked, now_ms);
    }

    /// Shows how an install went, with the components read after; the next install of a plan
    /// when there is one.
    pub fn show_installed(
        &mut self,
        result: Result<(), Failure>,
        read: ComponentsRead,
    ) -> Option<Request> {
        self.components.show_installed(&mut self.tree, result, read)
    }

    /// The job the Components pane asked for, taken once (D300).
    pub fn take_component_job(&mut self) -> Option<crate::components_pane::ComponentJob> {
        self.components.take_job()
    }

    /// Shows how the component job the pane waited for ended, with what was read after.
    pub fn show_component_job_ended(&mut self, succeeded: bool, read: ComponentsRead) {
        self.components.show_job_ended(&mut self.tree, succeeded, read);
    }

    /// The edition the Components pane finished installing, to record (D263).
    pub fn take_finished_edition(&mut self) -> Option<&'static str> {
        self.components.take_finished_edition()
    }

    /// Shows how a remove went, with the components read after.
    pub fn show_removed(&mut self, result: Result<(), RemoveError>, read: ComponentsRead) {
        self.components.show_removed(&mut self.tree, result, read);
    }

    /// Shows the devices just read from `/proc/driver/devices` in About (D220).
    pub fn show_devices(&mut self, read: Result<&[u8], Errno>) {
        self.about.show_devices(&mut self.tree, read);
    }

    /// The About pane's `Devices` section.
    pub fn about_pane(&self) -> &AboutPane {
        &self.about
    }

    /// Shows `adapter`, just read from `/proc/net/interfaces` (`None`: the machine has none),
    /// in the Network pane, changing only what differs from what it shows (D225).
    pub fn show_network_adapter(&mut self, adapter: Option<InterfaceStatus>) {
        self.facts.network_adapter = adapter;
        self.network.show(&mut self.tree, adapter.as_ref());
    }

    /// Shows the Wi-Fi driver's `status` and the `chosen` network in the Network pane's Wi-Fi
    /// section, and names the wired section's adapter after it (D283).
    pub fn show_wifi(&mut self, status: wifi_section::Status, chosen: Option<alloc::string::String>) {
        self.wifi.show(&mut self.tree, status, chosen);
        let adapter = self.facts.network_adapter;
        self.network
            .show_wifi(&mut self.tree, self.wifi.has_adapter(), self.wifi.connected(), adapter.as_ref());
    }

    /// The program wrote `wifi.conf` for a join (`Ok`) or could not (`Err` with its words).
    pub fn wifi_join_saved(&mut self, saved: Result<(), &str>, now_ms: u32) {
        self.wifi.join_saved(&mut self.tree, saved, now_ms);
    }

    /// The program could not remove `wifi.conf`.
    pub fn wifi_forget_failed(&mut self, words: &str) {
        self.wifi.forget_failed(&mut self.tree, words);
    }

    /// The Network pane (D225).
    pub fn network_pane(&self) -> &NetworkPane {
        &self.network
    }

    /// Shows the agents' grants just read on the Agents pane (D193).
    pub fn show_grants(&mut self, read: GrantsRead) {
        self.agents.show_grants(&mut self.tree, read);
    }

    /// The grants file the Agents pane asks to write ([`Request::WriteGrants`]).
    pub fn grants_to_write(&self) -> &GrantStore {
        self.agents.store_to_write()
    }

    /// How writing the grants file went, and the grants read again after it.
    pub fn grants_written(&mut self, written: Result<(), SaveError>, read: GrantsRead) {
        self.agents.grants_written(&mut self.tree, written, read);
    }

    /// What the Agents pane's Activity view asks the log for ([`Request::ReadActivity`]).
    pub fn activity_query(&self) -> ActivityQuery {
        self.agents.activity_query(&self.tree)
    }

    /// Shows the audit log's rows just read in the Agents pane's Activity view (D193).
    pub fn show_activity(&mut self, read: ActivityRead) {
        self.agents.show_activity(&mut self.tree, read);
    }

    /// Shows the network just read in the Agents pane's Network view (D193).
    pub fn show_network(&mut self, read: NetworkRead) {
        self.agents.show_network(&mut self.tree, read);
    }

    /// How turning the network on or off went ([`Request::TurnNetwork`]).
    pub fn network_turned(&mut self, turned: Result<(), Errno>, read: NetworkRead) {
        self.agents.network_turned(&mut self.tree, turned, read);
    }

    /// The agent the pairing panel asks to pair ([`Request::BeginPairing`]).
    pub fn to_pair(&self) -> Option<(AgentId, Option<AgentName>)> {
        self.agents.to_pair(&self.tree)
    }

    /// How pairing went, with the grants and the network read again after it.
    pub fn paired(
        &mut self,
        agent: AgentId,
        outcome: Result<Code, PairError>,
        grants: GrantsRead,
        network: NetworkRead,
        now_ms: u32,
    ) {
        self.agents
            .paired(&mut self.tree, agent, outcome, grants, network, now_ms);
    }

    /// Shows pane `pane` (an index of [`PANES`]) and selects it in the sidebar, as
    /// `settings --pane` asks (D221): what the program reads for it, as for a click on the
    /// sidebar.
    pub fn open_pane(&mut self, pane: usize) -> Option<Request> {
        if pane >= PANES.len() {
            return None;
        }
        self.tree.set_selected(self.widgets.list, pane);
        self.show_pane(pane)
    }

    /// Shows the Agents pane's Network view with its pairing panel filled in with agent `id`
    /// and `name` (empty for none), from `grants` and `network` just read, as `settings --pair`
    /// asks (D193): nothing is paired until the person presses Pair.
    pub fn open_pairing(
        &mut self,
        id: &str,
        name: &str,
        grants: GrantsRead,
        network: NetworkRead,
        now_ms: u32,
    ) {
        self.tree.set_selected(self.widgets.list, AGENTS);
        self.show_pane(AGENTS);
        self.agents.show_grants(&mut self.tree, grants);
        self.agents
            .ask_to_pair(&mut self.tree, id, name, network, now_ms);
    }

    /// Whether the Agents pane's pairing panel counts a code down, so the window wants a tick
    /// each second.
    pub fn pairing_counting_down(&self) -> bool {
        self.pane() == AGENTS && self.agents.pairing_counting_down()
    }

    /// The Agents pane.
    pub fn agents_pane(&self) -> &AgentsPane {
        &self.agents
    }

    /// Shows the disks just read on the Storage pane (D192).
    pub fn show_disks(&mut self, listed: Result<Vec<DiskRecord>, StorageError>) {
        self.storage.show_disks(&mut self.tree, listed);
    }

    /// Shows on the Storage pane how checking or repairing `device` went.
    pub fn show_check(
        &mut self,
        device: &'static str,
        mode: CheckMode,
        result: Result<DiskCheck, StorageError>,
    ) {
        self.storage
            .show_check(&mut self.tree, device, mode, result);
    }

    /// Shows on the Storage pane how formatting `device` with `label` went, at `now_ms`.
    pub fn show_formatted(
        &mut self,
        device: &'static str,
        label: &str,
        result: Result<(), StorageError>,
        now_ms: u32,
    ) {
        self.storage
            .show_formatted(&mut self.tree, device, label, result, now_ms);
    }

    /// Shows on the Storage pane how installing EggOS on `device` went, at `now_ms`.
    pub fn show_installed(&mut self, device: &'static str, result: Result<(), StorageError>, now_ms: u32) {
        self.storage
            .show_installed(&mut self.tree, device, result, now_ms);
    }

    /// Tells the Storage pane whether the kernel kept the boot files installing needs.
    pub fn set_boot_files(&mut self, kept: bool) {
        self.storage.set_boot_files(&mut self.tree, kept);
    }

    /// The label typed in the Storage pane's format panel.
    pub fn format_label(&self) -> &str {
        self.storage.format_label(&self.tree)
    }

    /// The Storage pane.
    pub fn storage_pane(&self) -> &StoragePane {
        &self.storage
    }

    /// Shows profile `index` as the chosen one: its card selected, its modifier and control
    /// side in the rows.
    pub fn show_profile(&mut self, index: usize) {
        let index = index.min(PROFILE_LABELS.len() - 1);
        self.facts.profile = index;
        self.tree.set_selected(self.widgets.cards, index);
        self.tree
            .set_text(self.widgets.modifier, PRIMARY_MODIFIERS[index]);
        self.tree
            .set_text(self.widgets.controls, CONTROL_SIDES[index]);
        let layout = layout_index(&self.facts);
        self.tree
            .set_text(self.widgets.third_level, third_level_text(layout, index));
    }

    /// Shows keyboard layout `index` (of the settings file's `KEYBOARD_LAYOUTS`) as the one in
    /// force: its row of your layouts selected (it joins them, as the CLI would make it,
    /// when it is not one of them), its third level and dead keys in the lines beside them.
    pub fn show_keyboard_layout(&mut self, index: usize) {
        let index = index.min(KEYBOARD_LAYOUTS.len() - 1);
        self.facts.keyboard_layouts = self
            .facts
            .keyboard_layouts
            .with_chosen(index, self.facts.keyboard_layout);
        self.facts.keyboard_layout = index;
        self.show_layout_set();
    }

    /// Shows `layouts` as your layouts and `in_force` as the one in force (D221), as a set
    /// changed from the CLI or MCP leaves them.
    pub fn show_keyboard_layouts(&mut self, layouts: KeyboardLayoutSet, in_force: usize) {
        self.facts.keyboard_layouts = layouts;
        self.facts.keyboard_layout = layouts.keeping(in_force);
        self.show_layout_set();
    }

    fn show_layout_set(&mut self) {
        let layouts = self.facts.keyboard_layouts;
        let in_force = layout_index(&self.facts);
        let widgets = self.widgets;
        for (row, &title) in widgets.layout_titles.iter().enumerate() {
            let layout = layouts.get(row);
            self.tree.set_shown(title, layout.is_some());
            if let Some(layout) = layout {
                self.tree.set_text(title, KEYBOARD_LAYOUTS[layout].title);
            }
        }
        self.tree
            .set_shown_items(widgets.layouts, (1u32 << layouts.len()) - 1);
        let row = layouts.position(in_force).unwrap_or(0);
        self.tree.set_selected(widgets.layouts, row);
        self.tree.set_enabled(widgets.move_up, row > 0);
        self.tree
            .set_enabled(widgets.move_down, row + 1 < layouts.len());
        self.tree.set_enabled(widgets.remove, layouts.len() > 1);
        let full = layouts.len() >= MAX_KEYBOARD_LAYOUTS;
        self.tree.set_enabled(widgets.add_layout, !full);
        self.tree.set_selected(widgets.add_layout, 0);
        self.tree
            .set_selected(widgets.switch_key, self.facts.switch_key.number());
        self.tree
            .set_enabled(widgets.switch_key, self.facts.keyboard);
        let status = widgets.keyboard_status;
        let (text, shown) = match full {
            true => (FOUR_LAYOUTS, true),
            false => (NO_DISK, !self.facts.on_disk),
        };
        self.tree.set_text(status, text);
        self.tree.set_shown(status, shown);
        self.tree.set_text(
            widgets.third_level,
            third_level_text(in_force, self.facts.profile),
        );
        self.tree
            .set_text(widgets.dead_keys, dead_keys_text(in_force).as_str());
    }

    fn change_layout_set(&mut self, layouts: KeyboardLayoutSet, in_force: usize) -> Request {
        self.facts.keyboard_layouts = layouts;
        self.facts.keyboard_layout = in_force;
        self.show_layout_set();
        Request::SetKeyboardLayouts { layouts, in_force }
    }

    fn choose_layout_in_set(&mut self, layout: usize) -> Option<Request> {
        if layout == self.facts.keyboard_layout {
            self.show_layout_set();
            return None;
        }
        self.facts.keyboard_layout = layout;
        self.show_layout_set();
        Some(Request::SetKeyboardLayout(layout))
    }

    fn add_layout(&mut self, option: usize) -> Option<Request> {
        self.tree.set_selected(self.widgets.add_layout, 0);
        let layout = option.checked_sub(1)?;
        let layouts = self.facts.keyboard_layouts;
        if layouts.contains(layout) {
            return self.choose_layout_in_set(layout);
        }
        if layouts.len() >= MAX_KEYBOARD_LAYOUTS {
            return None;
        }
        let added = layouts.with_chosen(layout, self.facts.keyboard_layout);
        Some(self.change_layout_set(added, layout))
    }

    fn move_layout(&mut self, down: bool) -> Option<Request> {
        let layouts = self.facts.keyboard_layouts;
        let in_force = self.facts.keyboard_layout;
        let row = layouts.position(in_force)?;
        let other = match down {
            true => row + 1,
            false => row.checked_sub(1)?,
        };
        let mut numbers = [0usize; MAX_KEYBOARD_LAYOUTS];
        for (place, number) in numbers.iter_mut().zip(layouts.numbers()) {
            *place = number;
        }
        if other >= layouts.len() {
            return None;
        }
        numbers.swap(row, other);
        let moved = KeyboardLayoutSet::of(&numbers[..layouts.len()])?;
        Some(self.change_layout_set(moved, in_force))
    }

    fn remove_layout(&mut self) -> Option<Request> {
        let layouts = self.facts.keyboard_layouts;
        let in_force = self.facts.keyboard_layout;
        let row = layouts.position(in_force)?;
        if layouts.len() < 2 {
            return None;
        }
        let kept: [usize; MAX_KEYBOARD_LAYOUTS] = core::array::from_fn(|index| {
            let from = if index < row { index } else { index + 1 };
            layouts.get(from).unwrap_or(0)
        });
        let left = KeyboardLayoutSet::of(&kept[..layouts.len() - 1])?;
        let next = left.get(row.min(left.len() - 1)).unwrap_or(0);
        Some(self.change_layout_set(left, next))
    }

    /// Shows the keyboard on the Keyboard pane: `Caps Lock is on` while it is, `No keyboard
    /// attached.` without one (the try field then disabled), nothing otherwise.
    pub fn show_keyboard(&mut self, present: bool, caps_lock: bool) {
        self.facts.keyboard = present;
        self.facts.caps_lock = caps_lock;
        let line = self.widgets.keyboard_line;
        let (text, ink) = match present {
            true => (CAPS_LOCK_ON, Token::Text),
            false => (NO_KEYBOARD, Token::TextDim),
        };
        self.tree.set_text(line, text);
        self.tree.change(line, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
        self.tree.set_shown(line, !present || caps_lock);
        self.tree.set_enabled(self.widgets.try_field, present);
        self.tree.set_enabled(self.widgets.switch_key, present);
    }

    /// The keyboard layout shown as the one in force: the selected row's.
    pub fn keyboard_layout_shown(&self) -> Option<usize> {
        let row = self.tree.selected(self.widgets.layouts)?;
        self.facts.keyboard_layouts.get(row)
    }

    /// The titles of your layouts the Keyboard pane shows, in order.
    pub fn keyboard_layout_titles(&self) -> Vec<&str> {
        self.widgets
            .layout_titles
            .iter()
            .filter(|&&title| self.tree.is_shown(title))
            .filter_map(|&title| self.tree.text_of(title))
            .collect()
    }

    /// The Keyboard pane's lines beside the list: the third level, the dead keys, and the
    /// keyboard's line when it is shown.
    pub fn keyboard_lines(&self) -> [Option<&str>; 3] {
        let line = self.widgets.keyboard_line;
        [
            self.tree.text_of(self.widgets.third_level),
            self.tree.text_of(self.widgets.dead_keys),
            self.tree
                .is_shown(line)
                .then(|| self.tree.text_of(line))
                .flatten(),
        ]
    }

    /// Shows `seconds` as the uptime in the About pane.
    pub fn show_uptime(&mut self, seconds: u64) {
        self.facts.uptime_s = seconds;
        self.tree
            .set_text(self.widgets.uptime, uptime_text(seconds).as_str());
    }

    /// Shows `seconds` since the epoch as the time in the `Date & time` pane.
    pub fn show_time(&mut self, seconds: u64) {
        self.facts.now_s = seconds;
        self.tree.set_text(
            self.widgets.now,
            time_text(seconds, self.facts.time_zone).as_str(),
        );
    }

    /// How saving time zone `zone` went ([`Request::SetTimeZone`]): saved, the pane shows it in
    /// `Now`, its name and its facts; refused, the selection goes back to the chosen zone and
    /// the status line says why (docs/ux/date-time-v0.3.md section 8.4).
    pub fn time_zone_saved(&mut self, zone: usize, saved: Result<(), Errno>) {
        match saved {
            Ok(()) => {
                self.facts.time_zone = zone;
                self.zones.choose(&mut self.tree, zone);
                self.show_time(self.facts.now_s);
            }
            Err(Errno::EPERM | Errno::EACCES) => {
                self.zones.reselect_chosen(&mut self.tree);
                self.set_status(
                    "Not allowed: this user may not set the time zone.",
                    Token::Text,
                );
            }
            Err(_) => {
                self.zones.reselect_chosen(&mut self.tree);
                self.set_status(
                    "No disk attached: the time zone could not be saved.",
                    Token::Text,
                );
            }
        }
    }

    /// The time zone picker.
    pub fn zone_picker(&self) -> &ZonePicker {
        &self.zones
    }

    /// The time zone picker's list.
    pub fn zone_picker_list(&self) -> WidgetId {
        self.zones.list()
    }

    /// Says on the `Date & time` pane's status line how setting the clock went, at `now_ms`.
    /// After a set the field is emptied and `Set` disabled, the focus staying in the field; after
    /// a refusal the typed text stays for correction.
    pub fn show_set_outcome(&mut self, outcome: SetOutcome, now_ms: u32) {
        let text = match outcome {
            SetOutcome::Saved => "Set, and saved to the hardware clock.",
            SetOutcome::NoHardwareClock => {
                "Set. No hardware clock: this time lasts until power-off."
            }
            SetOutcome::NotAllowed => "Not allowed: this user may not set the clock.",
            SetOutcome::Failed => "The clock could not be set.",
        };
        if matches!(outcome, SetOutcome::Saved | SetOutcome::NoHardwareClock) {
            self.tree.set_text(self.widgets.set_field, "");
            self.tree.set_enabled(self.widgets.set_button, false);
            self.tree.set_focus(Some(self.widgets.set_field), now_ms);
        }
        self.set_status(text, Token::Text);
    }

    /// The `Date & time` pane's status line.
    pub fn status_shown(&self) -> Option<&str> {
        self.tree.text_of(self.widgets.status)
    }

    fn set_status(&mut self, text: &str, ink: Token) {
        self.tree.set_text(self.widgets.status, text);
        self.tree.change(self.widgets.status, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
    }

    fn reset_date_and_time(&mut self) {
        self.clock.reset(&mut self.tree, &self.facts);
        self.zones.reset(&mut self.tree, self.facts.now_s as i64);
    }

    /// How saving `time.automatic` as `on` went ([`Request::SetTimeAutomatic`]), with the wall
    /// clock's seconds when the sync it started began, if one did (D225, section 9.3).
    pub fn time_automatic_saved(
        &mut self,
        on: bool,
        saved: Result<(), Errno>,
        sync_started_at: Option<i64>,
    ) {
        if saved.is_ok() {
            self.facts.time_automatic = on;
        }
        self.clock
            .saved(&mut self.tree, &self.facts, saved, sync_started_at);
    }

    /// Shows what the network clock's record (`record`, `None` before the first sync) and the
    /// network adapter (`adapter`, `None` without one) say now; `sync_running`, whether the
    /// sync this window started still runs (D225, section 9.4).
    pub fn show_clock_sync(
        &mut self,
        record: Option<Record>,
        adapter: Option<InterfaceStatus>,
        sync_running: bool,
    ) {
        self.facts.clock_sync = record;
        self.facts.network_adapter = adapter;
        self.clock.show_sync(
            &mut self.tree,
            &self.facts,
            record,
            Adapter::of(adapter.as_ref()),
            sync_running,
        );
    }

    /// The `Date & time` pane's `Set automatically` (D225).
    pub fn automatic_clock(&self) -> &AutomaticClock {
        &self.clock
    }

    #[cfg(test)]
    pub(crate) fn pane_widget(&self, pane: usize) -> WidgetId {
        self.widgets.panes[pane]
    }

    fn ask_to_set(&mut self) -> Option<Request> {
        let typed = Text::<255>::from(self.tree.text_of(self.widgets.set_field).unwrap_or(""));
        let rule = eggos_calendar::zone::ZONES
            .get(self.facts.time_zone)
            .and_then(|(_, rule)| eggos_calendar::zone::parse(rule))
            .or_else(|| eggos_calendar::zone::parse("UTC0"))
            .expect("UTC0 is a rule");
        let refusal = match parse_local_setting(typed.as_str(), self.facts.now_s as i64, &rule) {
            Ok(seconds) => return Some(Request::SetTime(seconds as u64)),
            Err(ParseError::NotADateAndTime) => {
                "Not a date and time. Type YYYY-MM-DD HH:MM or HH:MM."
            }
            Err(ParseError::NeedsTheDate) => "Type the date too: YYYY-MM-DD HH:MM.",
            Err(ParseError::YearOutOfRange) => "The year must be 2000 to 2099.",
        };
        self.set_status(refusal, Token::Text);
        None
    }

    /// The facts shown.
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    fn filter_list(&mut self) -> u32 {
        let query = Text::<255>::from(self.tree.text_of(self.widgets.search).unwrap_or(""));
        let mut shown = 0u32;
        for pane in 0..PANES.len() {
            if pane_matches(pane, query.as_str()) {
                shown |= 1 << pane;
            }
        }
        if !self.windows_layer {
            shown &= every_pane_but_windows();
        }
        self.tree.set_shown_items(self.widgets.list, shown);
        shown
    }

    fn filter(&mut self) -> Option<Request> {
        let shown = self.filter_list();
        match self.tree.selected(self.widgets.list) {
            Some(selected) if shown != 0 => self.show_pane(selected),
            _ => None,
        }
    }

    /// Acts on what an input did at `now_ms`: what the program must do, if anything.
    pub fn respond(&mut self, response: Response, now_ms: u32) -> Option<Request> {
        if self.about.respond(&mut self.tree, response) {
            return None;
        }
        if let wifi_section::Reply::Done(request) = self.wifi.respond(&mut self.tree, response, now_ms) {
            return match request {
                Some(Request::OpenComponents) => self.open_pane(COMPONENTS),
                other => other,
            };
        }
        if let components_pane::Reply::Done(request) =
            self.components.respond(&mut self.tree, response, now_ms)
        {
            return request;
        }
        if let windows_pane::Reply::Done(copy) =
            self.windows.respond(&mut self.tree, response, now_ms)
        {
            return copy.then_some(Request::CopyToClipboard);
        }
        if let Reply::Done(request) = self.storage.respond(&mut self.tree, response, now_ms) {
            return request;
        }
        if let agents_pane::Reply::Done(request) =
            self.agents.respond(&mut self.tree, response, now_ms)
        {
            return request;
        }
        if let input_panes::Reply::Done(request) = self.display.respond(response, &self.facts) {
            return request;
        }
        if let input_panes::Reply::Done(request) = self.inputs.respond(response, self.facts.input) {
            return request;
        }
        if let input_panes::Reply::Done(request) =
            self.accessibility.respond(response, Shown::of(&self.facts))
        {
            return request;
        }
        if let zone_picker::Reply::Done(request) =
            self.zones
                .respond(&mut self.tree, response, now_ms, self.widgets.set_button)
        {
            return request;
        }
        if let Some(request) = self.clock.respond(response) {
            return Some(request);
        }
        let widgets = self.widgets;
        match response {
            Response::Chosen(id, index) if id == widgets.theme => {
                let theme = Theme::from_number(index as u32)?;
                self.facts.theme = theme;
                Some(Request::SetTheme(theme))
            }
            Response::Chosen(id, card) if id == widgets.cards => {
                self.show_profile(card);
                Some(Request::SetProfile(card))
            }
            Response::Chosen(id, row) if id == widgets.layouts => {
                let layout = self.facts.keyboard_layouts.get(row)?;
                self.choose_layout_in_set(layout)
            }
            Response::Chosen(id, option) if id == widgets.add_layout => self.add_layout(option),
            Response::Activated(id) if id == widgets.move_up => self.move_layout(false),
            Response::Activated(id) if id == widgets.move_down => self.move_layout(true),
            Response::Activated(id) if id == widgets.remove => self.remove_layout(),
            Response::Chosen(id, number) if id == widgets.on_screen_keyboard => {
                if number == self.facts.on_screen_keyboard {
                    return None;
                }
                self.facts.on_screen_keyboard = number;
                Some(Request::SetOnScreenKeyboard(number))
            }
            Response::Chosen(id, number) if id == widgets.switch_key => {
                let switch_key = SwitchKey::from_number(number);
                if switch_key == self.facts.switch_key {
                    return None;
                }
                self.facts.switch_key = switch_key;
                Some(Request::SetSwitchKey(switch_key))
            }
            Response::Chosen(id, pane) | Response::Opened(id, pane) if id == widgets.list => {
                self.show_pane(pane)
            }
            Response::Edited(id) if id == widgets.search => self.filter(),
            Response::Edited(id) if id == widgets.set_field => {
                let typed = self
                    .tree
                    .text_of(widgets.set_field)
                    .is_some_and(|text| !text.is_empty());
                self.tree.set_enabled(widgets.set_button, typed);
                None
            }
            Response::Activated(id) if id == widgets.set_button => self.ask_to_set(),
            Response::Unused {
                code: key::DOWN, ..
            } if self.tree.focus() == Some(widgets.search) => {
                self.tree.set_focus(Some(widgets.list), now_ms);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_installer_s_window_holds_as_many_widgets_as_settings() {
        assert_eq!(eggos_app::dialog::CAPACITY, CAPACITY);
    }

    use super::*;
    use eggos_settings_file::InputPreferences;
    use eggos_toolkit::input::{Input, Modifiers, MouseButton};

    fn click(window: &mut SettingsWindow, x: i32, y: i32) -> Option<Request> {
        let mut request = None;
        for input in [
            Input::PointerMove { x, y },
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed: true,
            },
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            let response = window.tree().handle(input, 0);
            request = request.or(window.respond(response, 0));
        }
        request
    }

    fn type_text(window: &mut SettingsWindow, text: &str) {
        for character in text.chars() {
            let response = window.tree().handle(
                Input::Key {
                    code: 30,
                    pressed: true,
                    modifiers: Modifiers::default(),
                    character: character as u32,
                },
                0,
            );
            window.respond(response, 0);
        }
    }

    #[test]
    fn the_window_is_laid_out_as_the_design_says() {
        let window = SettingsWindow::new(Facts::example());
        let tree = window.widgets();
        assert_eq!(
            tree.rect(window.widgets.search),
            Some(eggos_toolkit::geometry::Rect::new(8, 8, 160, 28))
        );
        assert_eq!(
            window.card_rect(0),
            Some(eggos_toolkit::geometry::Rect::new(176 + 16, 64, 128, 112))
        );
        assert_eq!(
            window.card_rect(2).map(|rect| rect.x),
            Some(176 + 16 + 2 * 144)
        );
        let modifier = tree.rect(window.widgets.modifier).unwrap();
        assert_eq!((modifier.right(), modifier.y), (640 - 16, 192 + 12));
        assert_eq!(
            tree.text_of(window.widgets.modifier),
            Some("Cmd (Super key)")
        );
        assert_eq!(tree.focus(), Some(window.widgets.search));
        assert_eq!(window.resolution_shown(), Some("1280 \u{d7} 800"));
    }

    #[test]
    fn the_theme_row_sits_in_row_2_and_choosing_light_asks_for_it() {
        let mut window = SettingsWindow::new(Facts::example());
        let pane = window.tree.rect(window.widgets.panes[APPEARANCE]).unwrap();
        let radios = window.tree.rect(window.widgets.theme).unwrap();
        assert_eq!(
            (
                radios.x - pane.x,
                radios.y - pane.y,
                radios.width,
                radios.height
            ),
            (331, 272, 117, 28)
        );
        assert_eq!(window.tree.selected(window.widgets.theme), Some(0));
        let request = click(&mut window, radios.x + 70 + 8, radios.y + 14);
        assert_eq!(request, Some(Request::SetTheme(Theme::Light)));
        assert_eq!(window.facts().theme, Theme::Light);
        assert!(pane_matches(APPEARANCE, "dark mode"));
    }

    #[test]
    fn clicking_a_card_asks_for_its_profile_and_updates_the_rows() {
        let mut window = SettingsWindow::new(Facts::example());
        let card = window.card_rect(0).unwrap();
        let request = click(&mut window, card.x + 20, card.y + 20);
        assert_eq!(request, Some(Request::SetProfile(0)));
        let tree = window.widgets();
        assert_eq!(tree.text_of(window.widgets.modifier), Some("Ctrl"));
        assert_eq!(tree.text_of(window.widgets.controls), Some("Right"));
        assert_eq!(tree.selected(window.widgets.cards), Some(0));
        assert_eq!(window.facts().profile, 0);
        assert_eq!(click(&mut window, card.x + 20, card.y + 20), None);
    }

    #[test]
    fn the_sidebar_switches_panes_and_the_search_filters_them_by_synonyms() {
        let mut window = SettingsWindow::new(Facts::example());
        click(
            &mut window,
            40,
            LIST_TOP + (ABOUT as i32 - 1) * SIDEBAR_ROW + 10,
        );
        assert_eq!(window.pane(), ABOUT);
        assert!(window.widgets().rect(window.widgets.uptime).is_some());
        click(&mut window, 40, 20);
        type_text(&mut window, "hidpi");
        assert_eq!(window.pane(), DISPLAY);
        type_text(&mut window, "x");
        assert_eq!(window.pane(), DISPLAY);
        let response = window.tree().handle(
            Input::Key {
                code: key::ESC,
                pressed: true,
                modifiers: Modifiers::default(),
                character: 27,
            },
            0,
        );
        window.respond(response, 0);
        type_text(&mut window, "resolution");
        assert_eq!(window.pane(), DISPLAY);
        assert!(pane_matches(ABOUT, "control panel"));
        assert!(pane_matches(KEYBOARD, "SHORTCUTS"));
        assert!(!pane_matches(MOUSE, "licence"));
    }

    #[test]
    fn the_no_disk_line_shows_only_without_a_disk() {
        let render = |on_disk: bool| {
            let window = SettingsWindow::new(Facts {
                on_disk,
                ..Facts::example()
            });
            let mut pixels = vec![0u32; 640 * 440];
            window.widgets().render(&mut pixels, 640, 440, 1);
            pixels
        };
        let with_disk = render(true);
        let without = render(false);
        let line = |pixels: &[u32]| {
            (316..332)
                .any(|y| (192..500).any(|x| pixels[y * 640 + x] == eggos_toolkit::colour::TEXT_DIM))
        };
        assert!(!line(&with_disk));
        assert!(line(&without));
    }

    #[test]
    fn the_uptime_is_hours_minutes_and_seconds() {
        assert_eq!(uptime_text(3725).as_str(), "Up 1:02:05");
        assert_eq!(uptime_text(59).as_str(), "Up 0:00:59");
    }

    const DATE_AND_TIME_ROW: i32 = LIST_TOP + DATE_AND_TIME as i32 * SIDEBAR_ROW + 10;

    fn manual() -> Facts {
        Facts {
            time_automatic: false,
            ..Facts::example()
        }
    }

    fn press(window: &mut SettingsWindow, code: u16, character: u32) -> Option<Request> {
        let response = window.tree().handle(
            Input::Key {
                code,
                pressed: true,
                modifiers: Modifiers::default(),
                character,
            },
            0,
        );
        window.respond(response, 0)
    }

    fn field_text(window: &SettingsWindow) -> Option<&str> {
        window.widgets().text_of(window.widgets.set_field)
    }

    #[test]
    fn the_date_and_time_pane_is_laid_out_and_found_as_the_design_says() {
        use eggos_toolkit::geometry::Rect;
        let mut window = SettingsWindow::new(manual());
        assert_eq!(PANES[DATE_AND_TIME], "Date & time");
        click(&mut window, 40, DATE_AND_TIME_ROW);
        assert_eq!(window.pane(), DATE_AND_TIME);
        let tree = window.widgets();
        let rect = |id: WidgetId| tree.rect(id).unwrap();
        assert_eq!(
            rect(window.widgets.set_field),
            Rect::new(176 + 144, 150, 200, 28)
        );
        assert_eq!(
            rect(window.widgets.set_button),
            Rect::new(176 + 352, 150, 96, 28)
        );
        let now = rect(window.widgets.now);
        assert_eq!((now.right(), now.y), (640 - 16, 76));
        let status = rect(window.widgets.status);
        assert_eq!((status.x, status.y), (176 + 16, 196));
        assert_eq!(
            tree.text_of(window.widgets.now),
            Some("Thursday 29 October 2026, 14:05:09 UTC")
        );
        assert_eq!(
            window.status_shown(),
            Some("From the hardware clock, read at boot.")
        );
        for word in [
            "clock",
            "UTC",
            "hardware clock",
            "hwclock",
            "time zone",
            "set to",
        ] {
            for pane in 0..PANES.len() {
                assert_eq!(
                    pane_matches(pane, word),
                    pane == DATE_AND_TIME,
                    "{word} {pane}"
                );
            }
        }
        assert!(pane_matches(ABOUT, "time") && pane_matches(DATE_AND_TIME, "time"));
        window.show_time(1_793_282_710);
        assert_eq!(
            window.widgets().text_of(window.widgets.now),
            Some("Thursday 29 October 2026, 14:05:10 UTC")
        );
    }

    #[test]
    fn typing_a_moment_and_enter_asks_to_set_the_clock_and_errors_keep_the_text() {
        let mut window = SettingsWindow::new(manual());
        click(&mut window, 40, DATE_AND_TIME_ROW);
        let field = window.widgets().rect(window.widgets.set_field).unwrap();
        click(&mut window, field.x + 10, field.y + 10);
        assert_eq!(press(&mut window, key::ENTER, 13), None);
        type_text(&mut window, "2026-10-29 14:05");
        assert_eq!(
            press(&mut window, key::ENTER, 13),
            Some(Request::SetTime(1_793_282_700))
        );
        window.show_set_outcome(SetOutcome::Saved, 0);
        assert_eq!(
            window.status_shown(),
            Some("Set, and saved to the hardware clock.")
        );
        assert_eq!(field_text(&window), Some(""));
        assert_eq!(window.widgets().focus(), Some(window.widgets.set_field));
        assert_eq!(press(&mut window, key::ENTER, 13), None);
        for (typed, line) in [
            (
                "2026-02-30 10:00",
                "Not a date and time. Type YYYY-MM-DD HH:MM or HH:MM.",
            ),
            (
                "29/10/2026",
                "Not a date and time. Type YYYY-MM-DD HH:MM or HH:MM.",
            ),
            ("1999-12-31 23:59", "The year must be 2000 to 2099."),
        ] {
            type_text(&mut window, typed);
            assert_eq!(press(&mut window, key::ENTER, 13), None, "{typed}");
            assert_eq!(window.status_shown(), Some(line));
            assert_eq!(field_text(&window), Some(typed));
            press(&mut window, key::ESC, 27);
            assert_eq!(field_text(&window), Some(""));
        }
        type_text(&mut window, "14:05:30");
        assert_eq!(
            press(&mut window, key::ENTER, 13),
            Some(Request::SetTime(1_793_282_730))
        );
        window.show_set_outcome(SetOutcome::NotAllowed, 0);
        assert_eq!(
            window.status_shown(),
            Some("Not allowed: this user may not set the clock.")
        );
        assert_eq!(field_text(&window), Some("14:05:30"));
        click(&mut window, 40, 44 + 10);
        click(&mut window, 40, DATE_AND_TIME_ROW);
        assert_eq!(
            window.status_shown(),
            Some("From the hardware clock, read at boot.")
        );
        assert_eq!(field_text(&window), Some(""));
    }

    #[test]
    fn without_a_hardware_clock_the_pane_says_so_and_wants_the_date() {
        let mut window = SettingsWindow::new(Facts {
            now_s: 12,
            hardware_clock: false,
            ..manual()
        });
        click(&mut window, 40, DATE_AND_TIME_ROW);
        assert_eq!(
            window.widgets().text_of(window.widgets.now),
            Some("Thursday 1 January 1970, 00:00:12 UTC")
        );
        assert_eq!(
            window.status_shown(),
            Some("No hardware clock: the time started at 1970 at boot.")
        );
        let field = window.widgets().rect(window.widgets.set_field).unwrap();
        click(&mut window, field.x + 10, field.y + 10);
        type_text(&mut window, "14:05");
        assert_eq!(press(&mut window, key::ENTER, 13), None);
        assert_eq!(
            window.status_shown(),
            Some("Type the date too: YYYY-MM-DD HH:MM.")
        );
        window.show_set_outcome(SetOutcome::NoHardwareClock, 0);
        assert_eq!(
            window.status_shown(),
            Some("Set. No hardware clock: this time lasts until power-off.")
        );
    }

    #[test]
    fn the_keyboard_pane_lists_your_layouts_and_choosing_one_asks_for_it() {
        let named = |name: &[u8]| eggos_settings_file::keyboard_layout_named(name).unwrap();
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 50, 150);
        assert_eq!(window.pane(), KEYBOARD);
        assert_eq!(window.keyboard_layout_shown(), Some(0));
        assert_eq!(window.keyboard_layout_titles(), ["English (US)"]);
        let widgets = window.widgets;
        assert!(!window.tree.is_enabled(widgets.remove));
        assert!(!window.tree.is_enabled(widgets.move_up));
        assert!(!window.tree.is_enabled(widgets.move_down));
        assert!(window.tree.is_enabled(widgets.switch_key));
        assert_eq!(
            window.keyboard_lines(),
            [
                Some("No third level, so no \u{20ac} key"),
                Some("No dead keys"),
                None
            ]
        );
        let german = named(b"de");
        let us_de = KeyboardLayoutSet::parse(b"us,de").unwrap();
        assert_eq!(
            window.respond(Response::Chosen(widgets.add_layout, german + 1), 0),
            Some(Request::SetKeyboardLayouts {
                layouts: us_de,
                in_force: german
            })
        );
        assert_eq!(window.keyboard_layout_titles(), ["English (US)", "German"]);
        assert_eq!(window.keyboard_layout_shown(), Some(german));
        assert_eq!(window.tree.selected(widgets.add_layout), Some(0));
        assert_eq!(
            window.keyboard_lines()[..2],
            [
                Some("Option types \u{20ac} and more"),
                Some("Dead keys: \u{b4} ` ^")
            ]
        );
        window.show_profile(0);
        assert_eq!(
            window.keyboard_lines()[0],
            Some("Right Alt (AltGr) types \u{20ac} and more")
        );
        let first_row = (176 + 16 + 190, 64 + 12);
        assert_eq!(
            click(&mut window, first_row.0, first_row.1),
            Some(Request::SetKeyboardLayout(0))
        );
        assert_eq!(click(&mut window, first_row.0, first_row.1), None);
        assert_eq!(
            window.respond(Response::Activated(widgets.move_down), 0),
            Some(Request::SetKeyboardLayouts {
                layouts: KeyboardLayoutSet::parse(b"de,us").unwrap(),
                in_force: 0
            })
        );
        assert_eq!(window.keyboard_layout_titles(), ["German", "English (US)"]);
        assert_eq!(window.keyboard_layout_shown(), Some(0));
        assert_eq!(
            window.respond(Response::Activated(widgets.remove), 0),
            Some(Request::SetKeyboardLayouts {
                layouts: KeyboardLayoutSet::parse(b"de").unwrap(),
                in_force: german
            })
        );
        window.show_keyboard_layouts(KeyboardLayoutSet::parse(b"us,de,fr,es").unwrap(), 0);
        assert!(!window.tree.is_enabled(widgets.add_layout));
        assert_eq!(
            window.tree.text_of(widgets.keyboard_status),
            Some(FOUR_LAYOUTS)
        );
        assert_eq!(
            window.respond(Response::Chosen(widgets.switch_key, 3), 0),
            Some(Request::SetSwitchKey(SwitchKey::AltShift))
        );
        window.show_keyboard(true, true);
        assert_eq!(window.keyboard_lines()[2], Some(CAPS_LOCK_ON));
        window.show_keyboard(false, false);
        assert_eq!(window.keyboard_lines()[2], Some(NO_KEYBOARD));
        assert!(!window.tree.is_enabled(widgets.switch_key));
        window.show_keyboard_layout(named(b"fr"));
        assert_eq!(window.keyboard_lines()[1], Some("Dead keys: ^ \u{a8}"));
        assert_eq!(ADD_LAYOUT_OPTIONS[0], "Add a layout...");
        assert_eq!(ADD_LAYOUT_OPTIONS[1], "English (US)");
        assert_eq!(LAYOUT_TITLES[0], "English (US)");
    }

    #[test]
    fn the_keyboard_pane_is_found_by_its_layouts_and_words() {
        for query in [
            "keymap",
            "AltGr",
            "dead keys",
            "caps lock",
            "azerty",
            "german",
            "swiss",
            "azertÿ",
        ] {
            assert!(pane_matches(KEYBOARD, query), "{query}");
            assert!(!pane_matches(APPEARANCE, query), "{query}");
        }
        assert!(pane_matches(KEYBOARD, "layout") && pane_matches(APPEARANCE, "layout"));
    }

    fn zone(name: &str) -> usize {
        crate::facts::zone_index(name).unwrap()
    }

    fn in_zone(name: &str) -> SettingsWindow {
        let mut window = SettingsWindow::new(Facts {
            time_zone: zone(name),
            ..manual()
        });
        click(&mut window, 40, DATE_AND_TIME_ROW);
        window
    }

    fn zone_rows(window: &SettingsWindow) -> [&str; 3] {
        window.zone_picker().row_lines(window.widgets())
    }

    #[test]
    fn the_pane_shows_the_chosen_zone_s_time_name_and_facts() {
        let window = in_zone("Europe/Paris");
        let tree = window.widgets();
        assert_eq!(
            tree.text_of(window.widgets.now),
            Some("Thursday 29 October 2026, 15:05:09 CET")
        );
        assert_eq!(
            window.zone_picker().lines(tree),
            [
                Some("Europe/Paris"),
                Some("UTC+01:00 CET, standard time"),
                None
            ]
        );
        assert_eq!(
            zone_rows(&window),
            ["Europe/Paris \u{b7} UTC+01:00 CET", "", ""]
        );
        let window = in_zone("UTC");
        assert_eq!(
            window.widgets().text_of(window.widgets.now),
            Some("Thursday 29 October 2026, 14:05:09 UTC")
        );
        assert_eq!(zone_rows(&window), ["UTC \u{b7} UTC+00:00", "", ""]);
    }

    #[test]
    fn typing_in_the_zone_field_lists_the_chosen_zone_then_the_matches() {
        let mut window = in_zone("UTC");
        click(&mut window, 200, 296);
        type_text(&mut window, "par");
        assert_eq!(
            zone_rows(&window),
            [
                "UTC \u{b7} UTC+00:00",
                "America/Paramaribo \u{b7} UTC-03:00",
                "Europe/Paris \u{b7} UTC+01:00 CET"
            ]
        );
        assert_eq!(
            window.zone_picker().lines(window.widgets())[2],
            Some("2 zones")
        );
        type_text(&mut window, "xyz");
        assert_eq!(zone_rows(&window), ["", "", ""]);
        assert_eq!(window.zone_picker().lines(window.widgets())[2], None);
    }

    #[test]
    fn arrows_in_the_list_choose_zones_and_a_refused_write_chooses_none() {
        let mut window = in_zone("UTC");
        click(&mut window, 200, 296);
        type_text(&mut window, "par");
        assert_eq!(press(&mut window, key::DOWN, 0), None);
        let paramaribo = zone("America/Paramaribo");
        assert_eq!(
            press(&mut window, key::DOWN, 0),
            Some(Request::SetTimeZone(paramaribo))
        );
        window.time_zone_saved(paramaribo, Ok(()));
        assert_eq!(
            window.widgets().text_of(window.widgets.now),
            Some("Thursday 29 October 2026, 11:05:09 UTC-03:00")
        );
        let paris = zone("Europe/Paris");
        assert_eq!(
            press(&mut window, key::DOWN, 0),
            Some(Request::SetTimeZone(paris))
        );
        window.time_zone_saved(paris, Err(Errno::EPERM));
        assert_eq!(
            window.status_shown(),
            Some("Not allowed: this user may not set the time zone.")
        );
        assert_eq!(window.facts().time_zone, paramaribo);
        assert_eq!(
            window.widgets().selected(window.zone_picker_list()),
            Some(1)
        );
    }

    #[test]
    fn the_time_typed_to_set_is_the_chosen_zone_s_local_time() {
        let mut window = in_zone("Europe/Paris");
        click(&mut window, 330, 160);
        type_text(&mut window, "2026-10-29 15:05");
        assert_eq!(
            click(&mut window, 540, 160),
            Some(Request::SetTime(1_793_282_700))
        );
    }

    const DISPLAY_ROW: i32 = LIST_TOP + DISPLAY as i32 * SIDEBAR_ROW + 10;
    const KEYBOARD_ROW: i32 = LIST_TOP + KEYBOARD as i32 * SIDEBAR_ROW + 10;
    const MOUSE_ROW: i32 = LIST_TOP + MOUSE as i32 * SIDEBAR_ROW + 10;

    #[test]
    fn the_display_pane_saves_the_scale_for_the_next_restart() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 40, DISPLAY_ROW);
        assert_eq!(
            window.display_pane().state(window.widgets()),
            (
                Some(0),
                Some("At 2x the desktop looks like 640 \u{d7} 400.")
            )
        );
        assert_eq!(
            click(&mut window, 600, 124),
            Some(Request::SetDisplayScale(2))
        );
        window.display_scale_saved(2, Ok(()));
        assert_eq!(
            window.display_pane().state(window.widgets()),
            (
                Some(1),
                Some("From the next restart the desktop looks like 640 \u{d7} 400.")
            )
        );
        assert_eq!(
            click(&mut window, 540, 124),
            Some(Request::SetDisplayScale(1))
        );
        window.display_scale_saved(1, Err(Errno::EPERM));
        assert_eq!(
            window.display_pane().state(window.widgets()),
            (
                Some(1),
                Some("Not allowed: this user may not set the scale.")
            )
        );
    }

    #[test]
    fn without_a_disk_the_scale_cannot_be_chosen() {
        let mut window = SettingsWindow::new(Facts {
            on_disk: false,
            ..Facts::example()
        });
        click(&mut window, 40, DISPLAY_ROW);
        assert_eq!(click(&mut window, 600, 124), None);
        assert_eq!(
            window.display_pane().state(window.widgets()).1,
            Some("No disk attached: the scale cannot be saved for a restart.")
        );
    }

    #[test]
    fn the_mouse_pane_asks_for_each_change_and_shows_how_it_went() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 40, MOUSE_ROW);
        assert_eq!(
            click(&mut window, 560, 84),
            Some(Request::ChangeInput(InputChange::LeftHanded(true)))
        );
        window.input_changed(InputChange::LeftHanded(true), Err(Errno::EPERM), Ok(()));
        assert_eq!(
            window.input_panes().mouse_status(window.widgets()),
            Some("Not allowed: this user may not change the mouse settings.")
        );
        assert!(!window.facts().input.left_handed);
        assert_eq!(
            click(&mut window, 580, 124),
            Some(Request::ChangeInput(InputChange::NaturalScrolling(true)))
        );
        assert_eq!(
            click(&mut window, 496, 164),
            Some(Request::ChangeInput(InputChange::PointerSpeed(6)))
        );
        window.input_changed(InputChange::PointerSpeed(6), Ok(()), Err(Errno::ENODEV));
        assert_eq!(window.input_panes().values(window.widgets())[0], Some(6));
        assert_eq!(
            window.input_panes().mouse_status(window.widgets()),
            Some("Saved for the next boot, but the window server did not take it.")
        );
        let mut fastest = SettingsWindow::new(Facts {
            input: InputPreferences {
                pointer_speed: 10,
                ..InputPreferences::DEFAULT
            },
            ..Facts::example()
        });
        click(&mut fastest, 40, MOUSE_ROW);
        assert_eq!(click(&mut fastest, 560, 164), None);
    }

    #[test]
    fn the_keyboard_pane_steps_the_repeat_delay_and_rate() {
        let mut window = SettingsWindow::new(Facts {
            input: InputPreferences {
                repeat_delay_ms: 437,
                repeat_rate: 23,
                ..InputPreferences::DEFAULT
            },
            ..Facts::example()
        });
        click(&mut window, 40, KEYBOARD_ROW);
        assert_eq!(
            window.input_panes().values(window.widgets()),
            [Some(5), Some(450), Some(23)]
        );
        assert_eq!(
            click(&mut window, 468, 196),
            Some(Request::ChangeInput(InputChange::RepeatDelayMs(800)))
        );
        assert_eq!(
            click(&mut window, 436, 236),
            Some(Request::ChangeInput(InputChange::RepeatRate(10)))
        );
        window.input_changed(InputChange::RepeatRate(10), Ok(()), Ok(()));
        assert_eq!(window.input_panes().values(window.widgets())[2], Some(10));
    }

    const ACCESSIBILITY_ROW: i32 = LIST_TOP + ACCESSIBILITY as i32 * SIDEBAR_ROW + 10;

    #[test]
    fn the_accessibility_pane_turns_sticky_and_slow_keys_on() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 40, ACCESSIBILITY_ROW);
        assert_eq!(window.pane(), ACCESSIBILITY);
        assert_eq!(
            window.accessibility_pane().state(window.widgets()),
            (false, false, Some(300), false)
        );
        assert_eq!(
            click(&mut window, 200, 164),
            Some(Request::ChangeKeys(KeysChange::StickyKeys(true)))
        );
        window.keys_changed(KeysChange::StickyKeys(true), Ok(()), Ok(()));
        assert_eq!(
            click(&mut window, 200, 204),
            Some(Request::ChangeKeys(KeysChange::SlowKeys(true)))
        );
        window.keys_changed(KeysChange::SlowKeys(true), Ok(()), Ok(()));
        assert_eq!(
            window.accessibility_pane().state(window.widgets()),
            (true, true, Some(300), true)
        );
        window.keys_changed(KeysChange::SlowKeysMs(500), Err(Errno::EPERM), Ok(()));
        assert_eq!(window.facts().keys.slow_keys_ms, 300);
    }

    #[test]
    fn the_accessibility_pane_turns_high_contrast_and_reduce_motion_on() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 40, ACCESSIBILITY_ROW);
        assert_eq!(
            window.accessibility_pane().display_state(window.widgets()),
            (false, false)
        );
        assert_eq!(
            click(&mut window, 200, 84),
            Some(Request::ChangeDisplay(DisplayChange::HighContrast(true)))
        );
        window.display_changed(DisplayChange::HighContrast(true), Ok(()), Ok(()));
        assert!(window.facts().display.high_contrast);
        assert_eq!(
            click(&mut window, 200, 84),
            Some(Request::ChangeDisplay(DisplayChange::HighContrast(false)))
        );
        window.display_changed(
            DisplayChange::HighContrast(false),
            Err(Errno::EPERM),
            Ok(()),
        );
        assert_eq!(
            window.accessibility_pane().display_state(window.widgets()),
            (true, false),
            "a refused save puts the box back"
        );
        assert_eq!(
            click(&mut window, 200, 124),
            Some(Request::ChangeDisplay(DisplayChange::ReduceMotion(true)))
        );
    }
}
