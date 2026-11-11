//! The Network pane's Wi-Fi section (D283, docs/ux/wifi-v0.4.md, design by Claude Fable): the
//! Wi-Fi driver's state from `/proc/driver/wifi`, the networks its last scan heard, six rows a
//! page, and joining and forgetting one by writing and removing `/var/lib/eggos/wifi.conf` as
//! `wifi connect` and `wifi forget` do. A WPA2 network's passphrase is typed in a masked field in
//! a note under its row.

use crate::settings_window::{CAPACITY, Request};
use crate::storage_pane::shortened;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

/// The section's widgets: the design's 16 to 52.
pub const WIDGETS: usize = 37;
/// The Wi-Fi driver's state file.
pub const STATUS_PATH: &str = "/proc/driver/wifi";
/// The file naming the network to join, as `wifi connect` writes it.
pub const SETTINGS_PATH: &str = "/var/lib/eggos/wifi.conf";
/// The longest network name.
pub const MAX_NAME_BYTES: usize = 32;
/// The longest passphrase, in bytes, the field and the request hold.
pub const MAX_PASSPHRASE_BYTES: usize = 63 * 4;

const PADDING: i32 = 16;
const SECTION_TOP: i32 = 356;
const HINT_TOP: i32 = 376;
const COMPONENTS_TOP: i32 = 400;
const FIRST_ROW: i32 = 404;
const ROW_PITCH: i32 = 40;
const ROW_HEIGHT: i32 = 36;
const ROW_WIDTH: i32 = 432;
const NOTE_HEIGHT: i32 = 84;
const ROWS: usize = 6;
const BUTTON: (i32, i32) = (96, 28);
const SMALLEST_PASSPHRASE: usize = 8;
const LARGEST_PASSPHRASE: usize = 63;
const NAME_CHARACTERS_IN_HINT: usize = 24;
const STRONG_DBM: i32 = -60;
const GOOD_DBM: i32 = -72;
const DOT: &str = " \u{b7} ";

const CANNOT_JOIN_REASON: &str = "EggOS joins open, WPA2 and WPA3 networks. WEP and WPA are not safe to join.";
const LENGTH_RULE: &str = "Use 8 to 63 characters.";

/// How a network protects its traffic, as the driver names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    Wpa2,
    Wpa3,
    Legacy,
    Other,
}

impl Security {
    fn of(word: &str) -> Self {
        match word {
            "Open" => Security::Open,
            "Wpa2Personal" => Security::Wpa2,
            "Wpa3Personal" => Security::Wpa3,
            "Legacy" => Security::Legacy,
            _ => Security::Other,
        }
    }

    fn words(self) -> &'static str {
        match self {
            Security::Open => "Open",
            Security::Wpa2 => "WPA2",
            Security::Wpa3 => "WPA3",
            Security::Legacy => "WEP or WPA",
            Security::Other => "Other security",
        }
    }

    fn joinable(self) -> bool {
        matches!(self, Security::Open | Security::Wpa2 | Security::Wpa3)
    }
}

/// One network the last scan heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub name: String,
    pub security: Security,
    pub signal: i32,
}

/// What `/proc/driver/wifi` said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The first line's state, before its first `;`.
    pub state: String,
    pub radio_off: bool,
    /// The network it joined, from `; network <name>`.
    pub joined: Option<String>,
    /// The chosen network it has not joined yet, from `; joining <name>` (D284).
    pub joining: Option<String>,
    /// The joined network's signal, from `; signal <n> dBm` (D284).
    pub signal: Option<i32>,
    /// The last try's failure until the next try ends, from `; failed <code>: <words>` (D284).
    pub failure: Option<Failed>,
    /// One network a name, the strongest of each, strongest first.
    pub networks: Vec<Heard>,
}

/// A failed try to join: the driver's code and its sentence (D284).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub code: String,
    pub words: String,
}

/// The code the driver gives a passphrase the network did not accept.
pub const WRONG_PASSPHRASE: &str = "wrong-passphrase";
const WRONG_PASSPHRASE_SENTENCE: &str = "The network did not accept this passphrase.";

/// Reads `/proc/driver/wifi`'s `text`; lines it cannot read are skipped.
pub fn parse(text: &str) -> Status {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split("; ");
    let state = String::from(parts.next().unwrap_or(""));
    let mut status = Status {
        state,
        ..Status::default()
    };
    for part in parts {
        if part == "the radio is switched off" {
            status.radio_off = true;
        } else if let Some(name) = part.strip_prefix("network ") {
            status.joined = Some(String::from(name));
        } else if let Some(name) = part.strip_prefix("joining ") {
            status.joining = Some(String::from(name));
        } else if let Some(signal) = part.strip_prefix("signal ").and_then(|rest| rest.strip_suffix(" dBm")) {
            status.signal = signal.parse().ok();
        } else if let Some((code, words)) = part.strip_prefix("failed ").and_then(|rest| rest.split_once(": ")) {
            status.failure = Some(Failed {
                code: String::from(code),
                words: String::from(words),
            });
        }
    }
    for line in lines {
        let mut fields = line.splitn(6, ' ');
        let (Some(_bssid), Some(_channel), Some(signal), Some("dBm"), Some(security)) =
            (fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(signal) = signal.parse::<i32>() else {
            continue;
        };
        let name = String::from(fields.next().unwrap_or(""));
        if status.networks.iter().any(|known| known.name == name && !name.is_empty()) {
            continue;
        }
        status.networks.push(Heard {
            name,
            security: Security::of(security),
            signal,
        });
    }
    status
}

/// The `ssid=` line of `wifi.conf`'s `text`: the chosen network.
pub fn chosen_in(text: &str) -> Option<String> {
    text.lines().find_map(|line| line.strip_prefix("ssid=")).map(String::from)
}

/// Whether `passphrase` is one `wifi connect` takes: 8 to 63 characters on one line.
pub fn passphrase_fits(passphrase: &str) -> bool {
    (SMALLEST_PASSPHRASE..=LARGEST_PASSPHRASE).contains(&passphrase.chars().count()) && !passphrase.contains('\n')
}

fn signal_words(dbm: i32) -> &'static str {
    match dbm {
        dbm if dbm >= STRONG_DBM => "Strong",
        dbm if dbm >= GOOD_DBM => "Good",
        _ => "Weak",
    }
}

fn in_hint(name: &str) -> String {
    let mut shown: String = name.chars().take(NAME_CHARACTERS_IN_HINT).collect();
    if name.chars().count() > NAME_CHARACTERS_IN_HINT {
        shown.push_str("...");
    }
    shown
}

fn sentence(words: &str) -> String {
    let mut chars = words.chars();
    let mut text: String = chars.next().map(|first| first.to_uppercase().collect()).unwrap_or_default();
    text.push_str(chars.as_str());
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Join,
    Forget,
    CannotJoin,
    Forgotten,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    name: String,
    detail: String,
    detail_strong: bool,
    button: Button,
    needs_passphrase: bool,
}

#[derive(Debug, Clone, Copy)]
struct RowWidgets {
    panel: WidgetId,
    name: WidgetId,
    detail: WidgetId,
    button: WidgetId,
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    title: WidgetId,
    value: WidgetId,
    hint: WidgetId,
    components: WidgetId,
    rows: [RowWidgets; ROWS],
    mark: WidgetId,
    page: WidgetId,
    more: WidgetId,
    reason: WidgetId,
    field: WidgetId,
    show: WidgetId,
    message: WidgetId,
    cancel: WidgetId,
    join: WidgetId,
}

/// What the section did with an input.
pub enum Reply {
    NotMine,
    Done(Option<Request>),
}

/// The Wi-Fi section's widgets and what they show.
pub struct WifiSection {
    widgets: Widgets,
    default_button: WidgetId,
    status: Status,
    chosen: Option<String>,
    failure: Option<String>,
    page: usize,
    rows: Vec<Row>,
    note: Option<usize>,
}

fn at(x: i32, y: i32) -> Place {
    Place::at(x, y, Length::Content, Length::Content)
}

fn sized(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn ink(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, colour: Token) {
    tree.set_text(id, shortened(words).as_str());
    tree.change(id, |kind| {
        if let Kind::Label { colour: current, .. } = kind {
            *current = colour;
        }
    });
}

impl WifiSection {
    /// Builds the section in `pane` under the wired section; `default_button` is the window's,
    /// given back when the passphrase note closes.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, default_button: WidgetId) -> Self {
        let mut add = |place: Place, kind: Kind| add_to(tree, pane, place, kind);
        let title = add(at(PADDING, SECTION_TOP), Kind::label("Wi-Fi", Token::Text));
        let value = add(Place::right(PADDING, SECTION_TOP), Kind::label("", Token::Text));
        let hint = add(Place::at(PADDING, HINT_TOP, Length::Px(ROW_WIDTH), Length::Content), Kind::label("", Token::TextDim));
        let components = add(sized(PADDING, COMPONENTS_TOP, 144, BUTTON.1), Kind::button("Open Components", false));
        let rows = core::array::from_fn(|row| {
            let top = FIRST_ROW + row as i32 * ROW_PITCH;
            let panel = add_to(tree, pane, sized(PADDING, top, ROW_WIDTH, ROW_HEIGHT), Kind::panel(Token::Menu));
            let name = add_to(tree, panel, sized(24, 10, 160, 16), Kind::label("", Token::Text));
            let detail = add_to(tree, panel, Place::right(ROW_WIDTH - 320, 10), Kind::label("", Token::TextDim));
            let button = add_to(tree, panel, sized(336, 4, BUTTON.0, BUTTON.1), Kind::button("Join", false));
            RowWidgets { panel, name, detail, button }
        });
        let mut add = |place: Place, kind: Kind| add_to(tree, pane, place, kind);
        let mark = add(sized(PADDING, FIRST_ROW + 6, 4, 24), Kind::panel(Token::Accent));
        let page = add(at(PADDING, 0), Kind::label("", Token::TextDim));
        let more = add(sized(352, 0, BUTTON.0, BUTTON.1), Kind::button("More", false));
        let reason = add(Place::at(PADDING, 0, Length::Px(ROW_WIDTH), Length::Content), Kind::label(CANNOT_JOIN_REASON, Token::TextDim));
        let field = add(sized(40, 0, 288, BUTTON.1), Kind::masked_text_field("Passphrase", Escape::Clear));
        let show = add(at(344, 0), Kind::checkbox("Show", false));
        let message = add(Place::at(40, 0, Length::Px(192), Length::Content), Kind::label(LENGTH_RULE, Token::TextDim));
        let cancel = add(sized(248, 0, BUTTON.0, BUTTON.1), Kind::button("Cancel", false));
        let join = add(sized(352, 0, BUTTON.0, BUTTON.1), Kind::button("Join", true));
        let widgets = Widgets {
            title,
            value,
            hint,
            components,
            rows,
            mark,
            page,
            more,
            reason,
            field,
            show,
            message,
            cancel,
            join,
        };
        tree.set_person_only(field, true);
        let mut section = Self {
            widgets,
            default_button,
            status: Status::default(),
            chosen: None,
            failure: None,
            page: 0,
            rows: Vec::new(),
            note: None,
        };
        section.lay_out(tree);
        section
    }

    /// The section's title, the design's widget 16.
    pub fn title(&self) -> WidgetId {
        self.widgets.title
    }

    /// Whether the driver says the station joined a network: the wired section then names the
    /// adapter `Wi-Fi`.
    pub fn connected(&self) -> bool {
        self.status.state == "connected"
    }

    /// Whether there is a Wi-Fi adapter at all.
    pub fn has_adapter(&self) -> bool {
        !matches!(self.status.state.as_str(), "" | "no Intel Wi-Fi adapter" | "no Wi-Fi driver in this kernel")
    }

    /// Shows the driver's `status` and the `chosen` network (`wifi.conf`'s), setting only what
    /// changed; the rows stay as they are while the passphrase note is open.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, status: Status, chosen: Option<String>) {
        if chosen != self.chosen || status.state == "connected" {
            self.failure = None;
        }
        if let Some(failed) = &status.failure {
            self.failure = Some(match failed.code.as_str() {
                WRONG_PASSPHRASE => String::from(WRONG_PASSPHRASE_SENTENCE),
                _ => failed.words.clone(),
            });
        } else if !self.is_driver_progress(&status.state) {
            self.failure = Some(status.state.clone());
        }
        let changed = status != self.status || chosen != self.chosen;
        self.status = status;
        self.chosen = chosen;
        if changed && self.note.is_none() {
            self.lay_out(tree);
        }
    }

    fn is_driver_progress(&self, state: &str) -> bool {
        matches!(
            state,
            "" | "no Intel Wi-Fi adapter"
                | "no Wi-Fi driver in this kernel"
                | "waiting for the file systems"
                | "waiting for the wifi-firmware component"
                | "starting the adapter"
                | "scanning"
                | "ready"
                | "connecting"
                | "connected"
        )
    }

    fn value_and_hint(&self) -> (&'static str, String, bool) {
        let chosen = self.chosen.as_deref().map(in_hint);
        let state = self.status.state.as_str();
        if self.status.radio_off {
            return ("Radio off", String::from("Switch Wi-Fi on with the computer's wireless key or switch."), false);
        }
        match state {
            "" | "no Intel Wi-Fi adapter" => ("No adapter", String::from("EggOS found no Intel Wi-Fi 6 or 6E adapter at boot."), false),
            "no Wi-Fi driver in this kernel" => ("No driver", String::from("This build of EggOS has no Wi-Fi driver."), false),
            "waiting for the file systems" => ("Starting...", String::from("Waiting for the disk that holds the firmware."), false),
            "waiting for the wifi-firmware component" => {
                ("Needs firmware", String::from("Install Intel Wi-Fi firmware in Settings > Components."), false)
            }
            "starting the adapter" => ("Starting...", String::from("Loading the firmware into the adapter."), false),
            "connected" => {
                let name = self.status.joined.as_deref().map(in_hint).unwrap_or_default();
                match self.chosen.is_some() {
                    true => ("Connected", alloc::format!("Joined {name}. Its address is at the top of this pane."), false),
                    false => ("Connected", alloc::format!("Forgotten: EggOS stays on {name} until the next restart."), false),
                }
            }
            "connecting" => ("Connecting...", alloc::format!("Joining {}.", chosen.unwrap_or_default()), false),
            _ => match (&self.failure, chosen) {
                (Some(failure), _) if failure.as_str() == state => ("Not connected", sentence(failure), true),
                (Some(failure), Some(_)) => ("Trying again...", sentence(failure), true),
                (None, Some(name)) => ("Connecting...", alloc::format!("Joining {name}."), false),
                (_, None) if state == "scanning" && self.status.networks.is_empty() => ("Searching...", String::from("Listening for networks."), false),
                (_, None) if self.status.networks.is_empty() => {
                    ("Not connected", String::from("No networks heard. EggOS listens again every 30 seconds."), false)
                }
                _ => ("Not connected", String::from("Choose a network to join."), false),
            },
        }
    }

    fn all_rows(&self) -> Vec<Row> {
        let state = self.status.state.as_str();
        let mut rows = Vec::new();
        let joined = match state == "connected" {
            true => self.status.joined.clone(),
            false => None,
        };
        if let Some(name) = &joined {
            let detail = match self.status.signal {
                Some(signal) => alloc::format!("Connected{DOT}{}", signal_words(signal)),
                None => String::from("Connected"),
            };
            rows.push(Row {
                name: name.clone(),
                detail,
                detail_strong: true,
                button: if self.chosen.is_some() { Button::Forget } else { Button::Forgotten },
                needs_passphrase: false,
            });
        }
        let listing = !matches!(
            state,
            "" | "no Intel Wi-Fi adapter" | "no Wi-Fi driver in this kernel" | "waiting for the file systems" | "waiting for the wifi-firmware component" | "starting the adapter"
        ) && !self.status.radio_off;
        if !listing {
            return Vec::new();
        }
        if let Some(chosen) = self.chosen.as_ref().filter(|_| joined.is_none()) {
            let detail = match (self.failure.is_some(), self.status.networks.iter().any(|heard| &heard.name == chosen)) {
                (true, _) => "Not joined",
                (false, false) if state != "connecting" => "Not in range",
                _ => "Joining...",
            };
            rows.push(Row {
                name: chosen.clone(),
                detail: String::from(detail),
                detail_strong: true,
                button: Button::Forget,
                needs_passphrase: false,
            });
        }
        for heard in &self.status.networks {
            if self.chosen.as_ref() == Some(&heard.name) || joined.as_ref() == Some(&heard.name) {
                continue;
            }
            let hidden = heard.name.is_empty() || heard.name.len() > MAX_NAME_BYTES;
            rows.push(Row {
                name: match hidden {
                    true => String::from("Hidden network"),
                    false => heard.name.clone(),
                },
                detail: alloc::format!("{}{DOT}{}", heard.security.words(), signal_words(heard.signal)),
                detail_strong: false,
                button: match heard.security.joinable() && !hidden {
                    true => Button::Join,
                    false => Button::CannotJoin,
                },
                needs_passphrase: matches!(heard.security, Security::Wpa2 | Security::Wpa3),
            });
        }
        rows
    }

    fn lay_out(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let (value, hint, strong) = self.value_and_hint();
        tree.set_text(widgets.value, value);
        ink(tree, widgets.hint, &hint, if strong { Token::Text } else { Token::TextDim });
        tree.set_shown(widgets.components, self.status.state == "waiting for the wifi-firmware component");
        let all = self.all_rows();
        let pages = all.len().div_ceil(ROWS).max(1);
        self.page = self.page.min(pages - 1);
        let first = self.page * ROWS;
        self.rows = all.iter().skip(first).take(ROWS).cloned().collect();
        let mut top = FIRST_ROW;
        let mut bottom = HINT_TOP + 16;
        let mut cannot = false;
        for (index, row_widgets) in widgets.rows.iter().enumerate() {
            let Some(row) = self.rows.get(index) else {
                tree.set_shown(row_widgets.panel, false);
                continue;
            };
            tree.set_shown(row_widgets.panel, true);
            tree.set_place(row_widgets.panel, sized(PADDING, top, ROW_WIDTH, ROW_HEIGHT));
            tree.set_text(row_widgets.name, shortened(&row.name).as_str());
            ink(tree, row_widgets.detail, &row.detail, if row.detail_strong { Token::Text } else { Token::TextDim });
            let words = match row.button {
                Button::Join => "Join",
                Button::Forget => "Forget",
                Button::CannotJoin => "Cannot join",
                Button::Forgotten => "Forgotten",
            };
            tree.set_text(row_widgets.button, words);
            tree.set_enabled(row_widgets.button, matches!(row.button, Button::Join | Button::Forget));
            tree.set_shown(row_widgets.button, self.note != Some(index));
            cannot |= row.button == Button::CannotJoin;
            bottom = top + ROW_HEIGHT;
            top += ROW_PITCH;
            if self.note == Some(index) {
                self.place_note(tree, bottom + 4);
                bottom += NOTE_HEIGHT;
                top += NOTE_HEIGHT;
            }
        }
        let connected = self.connected() && !self.rows.is_empty();
        tree.set_shown(widgets.mark, connected);
        if connected {
            tree.set_place(widgets.mark, sized(PADDING, FIRST_ROW + 6, 4, 24));
        }
        let footer = bottom + 8;
        let paged = all.len() > ROWS;
        tree.set_shown(widgets.page, paged);
        tree.set_shown(widgets.more, paged);
        if paged {
            let last = (first + self.rows.len()).min(all.len());
            tree.set_text(widgets.page, &alloc::format!("Networks {} to {last} of {}", first + 1, all.len()));
            tree.set_place(widgets.page, at(PADDING, footer + 6));
            tree.set_text(widgets.more, if self.page + 1 == pages { "Strongest" } else { "More" });
            tree.set_place(widgets.more, sized(352, footer, BUTTON.0, BUTTON.1));
        }
        tree.set_shown(widgets.reason, cannot);
        if cannot {
            let reason_top = footer + 6 + if paged { 34 } else { 0 };
            tree.set_place(widgets.reason, Place::at(PADDING, reason_top, Length::Px(ROW_WIDTH), Length::Content));
        }
        for id in [widgets.field, widgets.show, widgets.message, widgets.cancel, widgets.join] {
            tree.set_shown(id, self.note.is_some());
        }
    }

    fn place_note(&self, tree: &mut WidgetTree<CAPACITY>, top: i32) {
        let widgets = self.widgets;
        tree.set_place(widgets.field, sized(40, top + 4, 288, BUTTON.1));
        tree.set_place(widgets.show, at(344, top + 8));
        tree.set_place(widgets.message, Place::at(40, top + 46, Length::Px(192), Length::Content));
        tree.set_place(widgets.cancel, sized(248, top + 40, BUTTON.0, BUTTON.1));
        tree.set_place(widgets.join, sized(352, top + 40, BUTTON.0, BUTTON.1));
    }

    fn open_note(&mut self, tree: &mut WidgetTree<CAPACITY>, row: usize, now_ms: u32) {
        self.note = Some(row);
        tree.set_text(self.widgets.field, "");
        tree.change(self.widgets.show, |kind| {
            if let Kind::Checkbox { checked, .. } = kind {
                *checked = false;
            }
        });
        set_masked(tree, self.widgets.field, true);
        ink(tree, self.widgets.message, LENGTH_RULE, Token::TextDim);
        tree.set_default_button(self.widgets.join);
        self.lay_out(tree);
        tree.set_focus(Some(self.widgets.field), now_ms);
    }

    fn close_note(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let row = self.note.take();
        tree.set_text(self.widgets.field, "");
        tree.set_default_button(self.default_button);
        self.lay_out(tree);
        if let Some(row) = row {
            tree.set_focus(Some(self.widgets.rows[row].button), now_ms);
        }
    }

    fn join_from_note(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let row = self.note?;
        let name = self.rows.get(row)?.name.clone();
        let passphrase = String::from(tree.text_of(self.widgets.field).unwrap_or(""));
        if !passphrase_fits(&passphrase) {
            ink(tree, self.widgets.message, LENGTH_RULE, Token::Text);
            return None;
        }
        Some(Request::WifiJoin {
            name: Text::from(name.as_str()),
            passphrase: Text::from(passphrase.as_str()),
        })
    }

    /// The program saved the join (`Ok`) or could not (`Err` with the error's words).
    pub fn join_saved(&mut self, tree: &mut WidgetTree<CAPACITY>, saved: Result<(), &str>, now_ms: u32) {
        match saved {
            Ok(()) => {
                self.close_note(tree, now_ms);
                tree.set_focus(Some(self.widgets.rows[0].button), now_ms);
            }
            Err(words) => ink(tree, self.widgets.message, &alloc::format!("Could not save: {words}."), Token::Text),
        }
    }

    /// The program could not forget the network.
    pub fn forget_failed(&mut self, tree: &mut WidgetTree<CAPACITY>, words: &str) {
        ink(tree, self.widgets.hint, &alloc::format!("Could not forget: {words}."), Token::Text);
    }

    /// Acts on `response` when it is the section's.
    pub fn respond(&mut self, tree: &mut WidgetTree<CAPACITY>, response: Response, now_ms: u32) -> Reply {
        let widgets = self.widgets;
        let row_of = |id: WidgetId| widgets.rows.iter().position(|row| row.button == id);
        let in_note = |id: Option<WidgetId>| id.is_some_and(|id| [widgets.field, widgets.show, widgets.cancel, widgets.join].contains(&id));
        let request = match response {
            Response::Activated(id) if id == widgets.components => Some(Request::OpenComponents),
            Response::Activated(id) if id == widgets.more => {
                let pages = self.all_rows().len().div_ceil(ROWS).max(1);
                self.page = (self.page + 1) % pages;
                self.lay_out(tree);
                None
            }
            Response::Activated(id) if row_of(id).is_some() => {
                let row = row_of(id).unwrap_or(0);
                let Some(chosen) = self.rows.get(row).cloned() else {
                    return Reply::Done(None);
                };
                match chosen.button {
                    Button::Forget => Some(Request::WifiForget),
                    Button::Join if chosen.needs_passphrase => {
                        self.open_note(tree, row, now_ms);
                        None
                    }
                    Button::Join => Some(Request::WifiJoin {
                        name: Text::from(chosen.name.as_str()),
                        passphrase: Text::new(),
                    }),
                    _ => None,
                }
            }
            Response::Activated(id) if id == widgets.cancel => {
                self.close_note(tree, now_ms);
                None
            }
            Response::Activated(id) if id == widgets.join && self.note.is_some() => self.join_from_note(tree),
            Response::Toggled(id, checked) if id == widgets.show => {
                set_masked(tree, widgets.field, !checked);
                None
            }
            Response::Edited(id) if id == widgets.field => {
                ink(tree, widgets.message, LENGTH_RULE, Token::TextDim);
                None
            }
            Response::Unused { code: key::ESC, .. } if self.note.is_some() && in_note(tree.focus()) => {
                self.close_note(tree, now_ms);
                None
            }
            _ => return Reply::NotMine,
        };
        Reply::Done(request)
    }
}

fn add_to(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind).expect("the Settings window fits its tree")
}

fn set_masked(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, masked: bool) {
    tree.change(id, |kind| {
        if let Kind::TextField(field) = kind {
            field.masked = masked;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRIVER: &str = "ready; address 00:11:22:33:44:55; 37 channels\n\
                          02:00:00:00:00:01 6 -48 dBm Wpa2Personal home\n\
                          02:00:00:00:00:02 11 -52 dBm Wpa2Personal home\n\
                          02:00:00:00:00:03 1 -66 dBm Open cafe\n\
                          02:00:00:00:00:04 36 -70 dBm Wpa3Personal lab\n\
                          02:00:00:00:00:05 40 -80 dBm Legacy old\n";

    #[test]
    fn the_driver_s_lines_become_one_network_a_name() {
        let status = parse(DRIVER);
        assert_eq!(status.state, "ready");
        let names: Vec<&str> = status.networks.iter().map(|heard| heard.name.as_str()).collect();
        assert_eq!(names, ["home", "cafe", "lab", "old"]);
        assert_eq!(status.networks[3].security, Security::Legacy);
        let joined = parse("connected; address 00:11:22:33:44:55; 37 channels; network home\n");
        assert_eq!(joined.joined.as_deref(), Some("home"));
        assert!(parse("ready; the radio is switched off; address x; 1 channels").radio_off);
        let joined = parse("connected; address x; 37 channels; network home; signal -51 dBm\n");
        assert_eq!(joined.signal, Some(-51));
        let failed = parse("ready; address x; 37 channels; joining home; failed wrong-passphrase: the network did not accept the passphrase\n");
        assert_eq!(failed.joining.as_deref(), Some("home"));
        let failure = failed.failure.expect("the failure segment");
        assert_eq!((failure.code.as_str(), failure.words.as_str()), (WRONG_PASSPHRASE, "the network did not accept the passphrase"));
    }

    #[test]
    fn signals_passphrases_and_the_chosen_network_read_as_the_design_says() {
        assert_eq!((signal_words(-48), signal_words(-66), signal_words(-80)), ("Strong", "Good", "Weak"));
        assert!(passphrase_fits("correct horse"));
        assert!(!passphrase_fits("short"));
        assert_eq!(chosen_in("ssid=home\npassphrase=x\n").as_deref(), Some("home"));
        assert_eq!(sentence("the network did not answer"), "The network did not answer.");
        assert_eq!(Security::Legacy.words(), "WEP or WPA");
    }
}
