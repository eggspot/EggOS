//! The Settings window (docs/ux/desktop-v0.3-gui.md section 10, docs/ux/toolkit-v0.3.md
//! section 11): the sidebar with its search and its panes, and the panes: Appearance with the
//! profile picker, Display, Keyboard (D185, docs/ux/keyboard-v0.3.md), Mouse, Storage (D192,
//! docs/ux/storage-v0.3.md), Date & time (D150, docs/ux/date-time-v0.3.md), Agents (D193,
//! docs/ux/agents-v0.3.md), Components and About. It holds only widgets and answers what the user does with what the
//! program must do; the program talks to the system.

use crate::activity::{ActivityQuery, ActivityRead};
use crate::agents_pane::{self, AgentsPane, GrantsRead};
use crate::art;
use crate::facts::Facts;
use crate::storage_pane::{Reply, StoragePane};
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_calendar::{ParseError, parse_setting};
use eggos_eggfs::{CheckMode, DiskCheck};
use eggos_files::storage::{DiskRecord, StorageError};
use eggos_mcp::grant_files::SaveError;
use eggos_mcp::grants::GrantStore;
use eggos_settings_file::KEYBOARD_LAYOUTS;
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

/// The window's title.
pub const TITLE: &str = "Settings";
/// The content's size when the window opens.
pub const DEFAULT_SIZE: (u32, u32) = (640, 440);
/// The smallest content the user may resize it to: the three cards must fit the pane.
pub const MINIMUM_SIZE: (u32, u32) = (640, 360);
/// The most widgets the window holds.
pub const CAPACITY: usize = 120;

const SIDEBAR_WIDTH: i32 = 176;
const SEARCH: (i32, i32, i32, i32) = (8, 8, 160, 28);
const LIST_TOP: i32 = 44;
const SIDEBAR_ROW: i32 = 32;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const FIRST_ROW: i32 = 64;
const SETTING_ROW: i32 = 40;
const TEXT_IN_ROW: i32 = 12;
const CONTROL_IN_ROW: i32 = 6;
const APPEARANCE_ROWS: i32 = 192;
const NO_DISK_LINE: i32 = 316;
const PANE_HEIGHT: i32 = 348;
const CARD: (i32, i32) = (128, 112);
const CARD_GAP: i32 = 16;
const ABOUT_TEXT_X: i32 = 80;
const ABOUT_LINES: i32 = 104;
const ABOUT_LINE_PITCH: i32 = 20;
const EGG_HEIGHT: i32 = 48;

const SET_FIELD: (i32, i32, i32, i32) = (144, 110, 200, 28);
const SET_BUTTON: (i32, i32, i32, i32) = (352, 110, 96, 28);
const STATUS_LINE: i32 = 156;

const LAYOUT_LIST: (i32, i32, i32, i32) = (16, 64, 208, 240);
const LAYOUT_ROW: i32 = 24;
const LAYOUT_MARK_X: i32 = 8;
const LAYOUT_TEXT_X: i32 = 24;
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

const NO_THIRD_LEVEL: &str = "No third level, so no \u{20ac} key";
const ALT_GRAPH_TYPES: &str = "Right Alt (AltGr) types \u{20ac} and more";
const OPTION_TYPES: &str = "Option types \u{20ac} and more";
const MACOS_PROFILE: usize = 1;
const CAPS_LOCK_ON: &str = "Caps Lock is on";
const NO_KEYBOARD: &str = "No keyboard attached.";
const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";

/// The panes, in the sidebar's order.
pub const PANES: [&str; 9] = [
    "Appearance",
    "Display",
    "Keyboard",
    "Mouse",
    "Storage",
    "Date & time",
    "Agents",
    "Components",
    "About",
];

const APPEARANCE: usize = 0;
const DISPLAY: usize = 1;
/// The `Keyboard` pane's place in [`PANES`] (D185, docs/ux/keyboard-v0.3.md).
pub const KEYBOARD: usize = 2;
const MOUSE: usize = 3;
/// The `Storage` pane's place in [`PANES`] (D192, docs/ux/storage-v0.3.md).
pub const STORAGE: usize = 4;
/// The `Date & time` pane's place in [`PANES`] (D150, docs/ux/date-time-v0.3.md).
pub const DATE_AND_TIME: usize = 5;
/// The `Agents` pane's place in [`PANES`] (D193, docs/ux/agents-v0.3.md).
pub const AGENTS: usize = 6;
const COMPONENTS: usize = 7;
/// The `About` pane's place in [`PANES`].
pub const ABOUT: usize = 8;

const EVERY_PANE_SYNONYMS: [&str; 4] =
    ["Control Panel", "System Settings", "Preferences", "options"];

const PANE_WORDS: [&[&str]; 9] = [
    &[
        "theme",
        "layout",
        "shortcuts",
        "profile",
        "Windows",
        "macOS",
        "Linux",
        "Primary modifier",
        "Window controls",
        "Scale",
        "HiDPI",
    ],
    &[
        "Resolution",
        "Scale",
        "HiDPI",
        "Pointer",
        "screen",
        "monitor",
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
        "Key repeat",
        "shortcuts",
    ],
    &["Primary button", "Wheel", "trackpad", "pointer"],
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
    &["layers", "apps", "Remove"],
    &[
        "version", "system", "licence", "license", "memory", "CPU", "uptime",
    ],
];

/// The profile cards' labels, in the settings file's `PROFILES` order.
pub const PROFILE_LABELS: [&str; 3] = ["Windows", "macOS", "Linux"];

const SCALES: [&str; 2] = ["1x", "2x"];

/// What the program must do for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Make the profile at this index of the settings file's `PROFILES` the desktop's and
    /// save it.
    SetProfile(usize),
    /// Set the clock to these seconds since the epoch, then say how it went
    /// ([`SettingsWindow::show_set_outcome`]).
    SetTime(u64),
    /// Make the keyboard layout at this index of the settings file's `KEYBOARD_LAYOUTS` the
    /// keyboard's and save it (D185).
    SetKeyboardLayout(usize),
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
    /// Read the agents' grants and show them ([`SettingsWindow::show_grants`]) (D193).
    ReadGrants,
    /// Write [`SettingsWindow::grants_to_write`] as the grants file, read it again, then show
    /// both ([`SettingsWindow::grants_written`]).
    WriteGrants,
    /// Read the audit log's rows [`SettingsWindow::activity_query`] asks for and show them
    /// ([`SettingsWindow::show_activity`]) (D193).
    ReadActivity,
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
    resolution: WidgetId,
    uptime: WidgetId,
    now: WidgetId,
    set_field: WidgetId,
    set_button: WidgetId,
    status: WidgetId,
    layouts: WidgetId,
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
    facts: Facts,
    pane: usize,
}

fn lowercase_contains(haystack: &str, needle: &str) -> bool {
    let lower = |byte: u8| byte.to_ascii_lowercase();
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || haystack.windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(&a, &b)| lower(a) == lower(b))
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

fn third_level_text(layout: usize, profile: usize) -> &'static str {
    match (KEYBOARD_LAYOUTS[layout].third_level, profile) {
        (false, _) => NO_THIRD_LEVEL,
        (true, MACOS_PROFILE) => OPTION_TYPES,
        (true, _) => ALT_GRAPH_TYPES,
    }
}

fn dead_keys_text(layout: usize) -> Text<64> {
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

fn time_text(seconds: u64) -> Text<64> {
    let mut text = Text::new();
    let _ = eggos_calendar::format::sentence(&mut text, seconds as i64);
    text
}

fn shown_status(facts: &Facts) -> &'static str {
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

    fn label(&mut self, parent: WidgetId, x: i32, y: i32, text: &str, ink: u32) -> WidgetId {
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
            Kind::label(text, colour::TEXT),
        )
    }

    fn heading(&mut self, parent: WidgetId, text: &str) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading(text),
        )
    }

    fn pane(&mut self, parent: WidgetId) -> WidgetId {
        self.add(
            Some(parent),
            Place::at(0, 0, Length::Fill(0), Length::Px(PANE_HEIGHT)),
            Kind::Panel {
                fill: None,
                border_right: None,
            },
        )
    }

    fn row(&mut self, pane: WidgetId, row: i32, name: &str, value: &str) -> WidgetId {
        let y = FIRST_ROW + row * SETTING_ROW + TEXT_IN_ROW;
        self.label(pane, PADDING, y, name, colour::TEXT);
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
                fill: Some(colour::DOCK),
                border_right: Some(colour::MENU_BORDER),
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
            Kind::panel(colour::MENU),
        );
        let scroll = build.add(
            Some(pane_area),
            Place::fill(),
            Kind::ScrollView {
                offset: 0,
                content_height: 0,
            },
        );
        let panes = [(); PANES.len()].map(|()| build.pane(scroll));
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
            colour::TEXT,
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
            colour::TEXT,
        );
        let controls = build.value(appearance, row_y(1) + TEXT_IN_ROW, CONTROL_SIDES[profile]);
        build.label(
            appearance,
            PADDING,
            row_y(2) + TEXT_IN_ROW,
            "Scale",
            colour::TEXT,
        );
        let scale_radios = Kind::radios(&SCALES, 0, true);
        let radios_width = scale_radios.content_size().0;
        let radios = build.add(
            Some(appearance),
            Place::right(PADDING, row_y(2) + CONTROL_IN_ROW),
            scale_radios,
        );
        build.add(
            Some(appearance),
            Place::right(PADDING + radios_width + PADDING, row_y(2) + TEXT_IN_ROW),
            Kind::label("2x comes in a later version", colour::TEXT_DIM),
        );
        let no_disk = build.label(appearance, PADDING, NO_DISK_LINE, NO_DISK, colour::TEXT_DIM);

        let display = panes[DISPLAY];
        build.heading(display, PANES[DISPLAY]);
        let mut resolution_text = Text::<64>::new();
        let _ = write!(
            resolution_text,
            "{} \u{d7} {}",
            facts.resolution.0, facts.resolution.1
        );
        let resolution = build.row(display, 0, "Resolution", resolution_text.as_str());
        let mut scale_text = Text::<64>::new();
        let _ = write!(scale_text, "{}x", facts.scale);
        build.row(display, 1, "Scale", scale_text.as_str());
        build.row(display, 2, "Pointer", "Software");

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
            Kind::list(&LAYOUT_TITLES, LAYOUT_ROW, LAYOUT_MARK_X, LAYOUT_TEXT_X),
        );
        let third_level = build.label(
            keyboard,
            KEYBOARD_FACTS_X,
            THIRD_LEVEL_LINE,
            third_level_text(layout, profile),
            colour::TEXT,
        );
        let dead_keys = build.label(
            keyboard,
            KEYBOARD_FACTS_X,
            DEAD_KEYS_LINE,
            dead_keys_text(layout).as_str(),
            colour::TEXT,
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
            colour::TEXT,
        );
        let keyboard_no_disk =
            build.label(keyboard, PADDING, NO_DISK_LINE, NO_DISK, colour::TEXT_DIM);

        for later in [MOUSE, COMPONENTS] {
            build.heading(panes[later], PANES[later]);
            build.label(
                panes[later],
                PADDING,
                FIRST_ROW,
                "Coming in a later version.",
                colour::TEXT_DIM,
            );
        }

        let date_and_time = panes[DATE_AND_TIME];
        build.heading(date_and_time, PANES[DATE_AND_TIME]);
        let now = build.row(date_and_time, 0, "Now", time_text(facts.now_s).as_str());
        build.label(
            date_and_time,
            PADDING,
            FIRST_ROW + SETTING_ROW + TEXT_IN_ROW,
            "Set to",
            colour::TEXT,
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
            colour::TEXT_DIM,
        );

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
            colour::TEXT,
        );
        let uptime = build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + ABOUT_LINE_PITCH,
            uptime_text(facts.uptime_s).as_str(),
            colour::TEXT,
        );
        build.label(
            about,
            ABOUT_TEXT_X,
            ABOUT_LINES + 2 * ABOUT_LINE_PITCH,
            "MIT OR Apache-2.0",
            colour::TEXT,
        );

        let storage = StoragePane::build(&mut tree, panes[STORAGE], set_button);
        let agents = AgentsPane::build(&mut tree, panes[AGENTS], set_button);
        tree.set_enabled(radios, false);
        tree.set_enabled(set_button, false);
        tree.set_default_button(set_button);
        tree.set_shown(no_disk, !facts.on_disk);
        tree.set_shown(keyboard_no_disk, !facts.on_disk);
        tree.set_selected(layouts, layout);
        for (index, pane) in panes.iter().enumerate() {
            tree.set_shown(*pane, index == APPEARANCE);
        }
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
                resolution,
                uptime,
                now,
                set_field,
                set_button,
                status,
                layouts,
                third_level,
                dead_keys,
                try_field,
                keyboard_line,
            },
            storage,
            agents,
            facts,
            pane: APPEARANCE,
        };
        window.show_keyboard(facts.keyboard, facts.caps_lock);
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
        self.tree.text_of(self.widgets.resolution)
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
        if pane == STORAGE {
            self.storage.reset(&mut self.tree);
            return Some(Request::ReadDisks);
        }
        if pane == AGENTS {
            self.agents.reset(&mut self.tree);
            return Some(Request::ReadGrants);
        }
        None
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

    /// Shows keyboard layout `index` (of the settings file's `KEYBOARD_LAYOUTS`) as the chosen
    /// one: its row selected, its third level and dead keys in the lines beside the list.
    pub fn show_keyboard_layout(&mut self, index: usize) {
        let index = index.min(KEYBOARD_LAYOUTS.len() - 1);
        self.facts.keyboard_layout = index;
        self.tree.set_selected(self.widgets.layouts, index);
        self.tree.set_text(
            self.widgets.third_level,
            third_level_text(index, self.facts.profile),
        );
        self.tree
            .set_text(self.widgets.dead_keys, dead_keys_text(index).as_str());
    }

    /// Shows the keyboard on the Keyboard pane: `Caps Lock is on` while it is, `No keyboard
    /// attached.` without one (the try field then disabled), nothing otherwise.
    pub fn show_keyboard(&mut self, present: bool, caps_lock: bool) {
        self.facts.keyboard = present;
        self.facts.caps_lock = caps_lock;
        let line = self.widgets.keyboard_line;
        let (text, ink) = match present {
            true => (CAPS_LOCK_ON, colour::TEXT),
            false => (NO_KEYBOARD, colour::TEXT_DIM),
        };
        self.tree.set_text(line, text);
        self.tree.change(line, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
        self.tree.set_shown(line, !present || caps_lock);
        self.tree.set_enabled(self.widgets.try_field, present);
    }

    /// The keyboard layout shown as chosen.
    pub fn keyboard_layout_shown(&self) -> Option<usize> {
        self.tree.selected(self.widgets.layouts)
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
        self.tree
            .set_text(self.widgets.now, time_text(seconds).as_str());
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
        self.set_status(text, colour::TEXT);
    }

    /// The `Date & time` pane's status line.
    pub fn status_shown(&self) -> Option<&str> {
        self.tree.text_of(self.widgets.status)
    }

    fn set_status(&mut self, text: &str, ink: u32) {
        self.tree.set_text(self.widgets.status, text);
        self.tree.change(self.widgets.status, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
    }

    fn reset_date_and_time(&mut self) {
        self.tree.set_text(self.widgets.set_field, "");
        self.tree.set_enabled(self.widgets.set_button, false);
        self.set_status(shown_status(&self.facts), colour::TEXT_DIM);
    }

    fn ask_to_set(&mut self) -> Option<Request> {
        let typed = Text::<255>::from(self.tree.text_of(self.widgets.set_field).unwrap_or(""));
        let refusal = match parse_setting(typed.as_str(), self.facts.now_s as i64) {
            Ok(seconds) => return Some(Request::SetTime(seconds as u64)),
            Err(ParseError::NotADateAndTime) => {
                "Not a date and time. Type YYYY-MM-DD HH:MM or HH:MM."
            }
            Err(ParseError::NeedsTheDate) => "Type the date too: YYYY-MM-DD HH:MM.",
            Err(ParseError::YearOutOfRange) => "The year must be 2000 to 2099.",
        };
        self.set_status(refusal, colour::TEXT);
        None
    }

    /// The facts shown.
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    fn filter(&mut self) -> Option<Request> {
        let query = Text::<255>::from(self.tree.text_of(self.widgets.search).unwrap_or(""));
        let mut shown = 0u32;
        for pane in 0..PANES.len() {
            if pane_matches(pane, query.as_str()) {
                shown |= 1 << pane;
            }
        }
        self.tree.set_shown_items(self.widgets.list, shown);
        match self.tree.selected(self.widgets.list) {
            Some(selected) if shown != 0 => self.show_pane(selected),
            _ => None,
        }
    }

    /// Acts on what an input did at `now_ms`: what the program must do, if anything.
    pub fn respond(&mut self, response: Response, now_ms: u32) -> Option<Request> {
        if let Reply::Done(request) = self.storage.respond(&mut self.tree, response, now_ms) {
            return request;
        }
        if let agents_pane::Reply::Done(request) =
            self.agents.respond(&mut self.tree, response, now_ms)
        {
            return request;
        }
        let widgets = self.widgets;
        match response {
            Response::Chosen(id, card) if id == widgets.cards => {
                self.show_profile(card);
                Some(Request::SetProfile(card))
            }
            Response::Chosen(id, layout) if id == widgets.layouts => {
                if layout == self.facts.keyboard_layout {
                    return None;
                }
                self.show_keyboard_layout(layout);
                Some(Request::SetKeyboardLayout(layout))
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
        click(&mut window, 40, 44 + 8 * 32 + 10);
        assert_eq!(window.pane(), ABOUT);
        assert!(window.widgets().rect(window.widgets.uptime).is_some());
        click(&mut window, 40, 20);
        type_text(&mut window, "hidpi");
        assert_eq!(window.pane(), APPEARANCE);
        type_text(&mut window, "x");
        assert_eq!(window.pane(), APPEARANCE);
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
            (316..332).any(|y| (192..500).any(|x| pixels[y * 640 + x] == colour::TEXT_DIM))
        };
        assert!(!line(&with_disk));
        assert!(line(&without));
    }

    #[test]
    fn the_uptime_is_hours_minutes_and_seconds() {
        assert_eq!(uptime_text(3725).as_str(), "Up 1:02:05");
        assert_eq!(uptime_text(59).as_str(), "Up 0:00:59");
    }

    const DATE_AND_TIME_ROW: i32 = 44 + 5 * 32 + 10;

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
        let mut window = SettingsWindow::new(Facts::example());
        assert_eq!(PANES[DATE_AND_TIME], "Date & time");
        click(&mut window, 40, DATE_AND_TIME_ROW);
        assert_eq!(window.pane(), DATE_AND_TIME);
        let tree = window.widgets();
        let rect = |id: WidgetId| tree.rect(id).unwrap();
        assert_eq!(
            rect(window.widgets.set_field),
            Rect::new(176 + 144, 110, 200, 28)
        );
        assert_eq!(
            rect(window.widgets.set_button),
            Rect::new(176 + 352, 110, 96, 28)
        );
        let now = rect(window.widgets.now);
        assert_eq!((now.right(), now.y), (640 - 16, 76));
        let status = rect(window.widgets.status);
        assert_eq!((status.x, status.y), (176 + 16, 156));
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
        let mut window = SettingsWindow::new(Facts::example());
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
            ..Facts::example()
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
    fn the_keyboard_pane_lists_the_layouts_and_choosing_one_asks_for_it() {
        let mut window = SettingsWindow::new(Facts::example());
        click(&mut window, 50, 120);
        assert_eq!(window.pane(), KEYBOARD);
        assert_eq!(window.keyboard_layout_shown(), Some(0));
        assert_eq!(
            window.keyboard_lines(),
            [
                Some("No third level, so no \u{20ac} key"),
                Some("No dead keys"),
                None
            ]
        );
        let german = (176 + 16 + 50, 64 + 8 * LAYOUT_ROW + 12);
        assert_eq!(
            click(&mut window, german.0, german.1),
            Some(Request::SetKeyboardLayout(8))
        );
        assert_eq!(window.facts().keyboard_layout, 8);
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
        assert_eq!(click(&mut window, german.0, german.1), None);
        window.show_keyboard(true, true);
        assert_eq!(window.keyboard_lines()[2], Some(CAPS_LOCK_ON));
        window.show_keyboard(false, false);
        assert_eq!(window.keyboard_lines()[2], Some(NO_KEYBOARD));
        window.show_keyboard_layout(7);
        assert_eq!(window.keyboard_lines()[1], Some("Dead keys: ^ \u{a8}"));
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
        ] {
            assert!(pane_matches(KEYBOARD, query), "{query}");
            assert!(!pane_matches(APPEARANCE, query), "{query}");
        }
        assert!(pane_matches(KEYBOARD, "layout") && pane_matches(APPEARANCE, "layout"));
    }
}
