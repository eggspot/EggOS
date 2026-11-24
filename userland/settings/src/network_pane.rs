//! The Network pane (D225, docs/ux/network-v0.3.md sections 0-3): the wired adapter as
//! `/proc/net/interfaces` reports it, through the parse `ip` and `net_status` use
//! (`eggos_net::status`), read-only and in one of five states named once for the GUI, the CLI
//! and MCP. Each of its widgets has one place; a state only shows, hides and fills them in, so
//! the pane fills in where it is as the machine connects. Under the traffic lines, the
//! adapter's MTU (a field and `Set`), its Wake-on-LAN (a checkbox) and a status line set the
//! link as `ip link set` does (D734, docs/ux/network-link-v0.1.md). Not the Agents pane's
//! [`crate::network_view`], which is the MCP network server.

use crate::settings_window::{CAPACITY, Request};
use core::fmt::Write;
use eggos_runtime::errno::Errno;
use eggos_toolkit::tree::Response;
use eggos_toolkit::widget::Escape;
use eggos_net::dhcp::INFINITE_LEASE_S;
use eggos_net::ipv4::Subnet;
use eggos_net::status::{InterfaceStatus, State};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

/// One of the pane's lines.
pub type Line = Text<64>;

/// The link rows' widgets (D734): the MTU label, field and `Set`, the Wake-on-LAN label and
/// checkbox, and the status line.
pub const LINK_WIDGETS: usize = 6;
/// The pane's widgets: the heading, the design's widgets 2 to 15, then the link rows'.
pub const WIDGETS: usize = 15 + LINK_WIDGETS;
/// How much lower the Wi-Fi section sits when the wired section has its link rows.
pub const LINK_ROWS_HEIGHT: i32 = 120;
/// The smallest MTU an adapter takes (IPv4's).
pub const MIN_MTU: u32 = 68;
/// What the Wake-on-LAN checkbox says.
pub const MAGIC_PACKET: &str = "Magic packet";
/// What the status line says once Wake-on-LAN is on.
pub const WAKE_ON_TEXT: &str =
    "Wake-on-LAN on: a magic packet wakes this computer from sleep or power-off.";
/// The first row's name for the adapter.
pub const ETHERNET: &str = "Ethernet";
/// The first row on a machine without an adapter.
pub const NO_ADAPTER: &str = "No network adapter.";
/// The hint on a machine without an adapter.
pub const NO_ADAPTER_HINT: &str = "EggOS found none at boot: there is nothing to set up.";
/// The names of the rows a connected adapter fills in, top to bottom.
pub const ROW_NAMES: [&str; 4] = ["IPv4 address", "Router", "DNS servers", "DHCP server"];
/// What a row holds when the network gave nothing for it.
pub const NOTHING_GIVEN: &str = "None";
/// The first row when the joined Wi-Fi network is the adapter (D283).
pub const WIFI: &str = "Wi-Fi";
/// The first row with no wired adapter but a Wi-Fi one (D283).
pub const NO_WIRED_ADAPTER: &str = "No wired adapter.";
/// Its hint (D283).
pub const JOIN_WIFI_HINT: &str = "Join a Wi-Fi network below.";
/// The unplugged cable's hint when there is a Wi-Fi adapter too (D283).
pub const CABLE_OR_WIFI_HINT: &str = "Plug in a network cable, or join a Wi-Fi network below.";

const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const FIRST_ROW_TEXT: i32 = 76;
const ROW_PITCH: i32 = 40;
const HARDWARE_LINE: i32 = 276;
const MTU_ROW: i32 = 350;
const WAKE_ROW: i32 = 390;
const ROW_LABEL_DOWN: i32 = 6;
const STATUS_TOP: i32 = 436;
const STATUS_WIDTH: i32 = 432;
const FIELD_PLACE: (i32, i32, i32, i32) = (272, MTU_ROW, 72, 28);
const SET_PLACE: (i32, i32, i32, i32) = (352, MTU_ROW, 96, 28);
const WAKE_X: i32 = 288;
const CONTROL_HEIGHT: i32 = 28;
const MOST_MTU_DIGITS: usize = 5;
const STANDARD_MTU: u32 = 1500;
const LINE_PITCH: i32 = 20;
const DOT: &str = " \u{b7} ";
const MINUTE_S: u32 = 60;
const HOUR_S: u32 = 60 * MINUTE_S;
const DAY_S: u32 = 24 * HOUR_S;

/// The line under the state that says what it means, for every state but connected.
pub const fn hint(state: State) -> Option<&'static str> {
    match state {
        State::CableUnplugged => Some("Plug in a network cable. EggOS connects by itself."),
        State::GettingAddress => Some("Asking the network for an address (DHCP)."),
        State::NoAddress => Some("No DHCP server answered. EggOS keeps trying."),
        State::Connected => None,
    }
}

/// `10.0.2.15/24 · mask 255.255.255.0`.
pub fn address_text(subnet: Subnet) -> Line {
    let mut line = Line::new();
    let _ = write!(line, "{subnet}{DOT}mask {}", subnet.mask());
    line
}

/// The DNS servers in the DHCP server's order with `separator` between them (`, ` in the pane,
/// `,` in `settings get network.dns`), or [`NOTHING_GIVEN`] when there is none.
pub fn servers_text(adapter: &InterfaceStatus, separator: &str) -> Line {
    let mut line = Line::new();
    for (index, server) in adapter.dns_servers().enumerate() {
        if index > 0 {
            line.push_str(separator);
        }
        let _ = write!(line, "{server}");
    }
    if line.is_empty() {
        line.push_str(NOTHING_GIVEN);
    }
    line
}

/// A lease's seconds left, every unit floored: `1 d 0 h`, `23 h 59 min`, `59 min`, `59 s`.
pub fn lease_left_text(seconds: u32) -> Text<32> {
    let mut text = Text::new();
    let _ = match seconds {
        DAY_S.. => write!(text, "{} d {} h", seconds / DAY_S, seconds % DAY_S / HOUR_S),
        HOUR_S.. => write!(
            text,
            "{} h {} min",
            seconds / HOUR_S,
            seconds % HOUR_S / MINUTE_S
        ),
        MINUTE_S.. => write!(text, "{} min", seconds / MINUTE_S),
        _ => write!(text, "{seconds} s"),
    };
    text
}

/// The DHCP row's value: `10.0.2.2 · 23 h 59 min of lease left`, or `10.0.2.2 · lease does
/// not end` for an infinite lease (RFC 2132).
pub fn dhcp_text(adapter: &InterfaceStatus) -> Line {
    let mut line = Line::new();
    match adapter.dhcp_server {
        Some(server) => {
            let _ = write!(line, "{server}");
        }
        None => {
            line.push_str(NOTHING_GIVEN);
        }
    }
    let _ = match adapter.lease_left_s {
        Some(INFINITE_LEASE_S) => write!(line, "{DOT}lease does not end"),
        Some(seconds) => write!(
            line,
            "{DOT}{} of lease left",
            lease_left_text(seconds).as_str()
        ),
        None => Ok(()),
    };
    line
}

/// `Hardware address 52:54:00:12:34:56 · MTU 1500`.
pub fn hardware_text(adapter: &InterfaceStatus) -> Line {
    let mut line = Line::new();
    let _ = write!(
        line,
        "Hardware address {}{DOT}MTU {}",
        adapter.mac, adapter.mtu
    );
    line
}

fn traffic_text(verb: &str, bytes: u64, packets: u64, errors: u64) -> Line {
    let mut line = Line::new();
    let _ = write!(
        line,
        "{verb} {} ({} packets",
        eggos_files::listing::size_text(bytes).as_str(),
        eggos_files::storage::grouped(packets)
    );
    let _ = match errors {
        0 => Ok(()),
        1 => line.write_str(", 1 error"),
        errors => write!(line, ", {} errors", eggos_files::storage::grouped(errors)),
    };
    line.push_str(")");
    line
}

/// `Received 3.4 MB (2,905 packets)`, with the receive errors and drops summed after the
/// packets when there are any: `Received 3.4 MB (2,905 packets, 3 errors)`.
pub fn received_text(adapter: &InterfaceStatus) -> Line {
    let counters = adapter.counters;
    traffic_text(
        "Received",
        counters.received_bytes,
        counters.received_packets,
        counters
            .received_errors
            .saturating_add(counters.received_dropped),
    )
}

/// `Sent 1.2 MB (812 packets)`.
pub fn sent_text(adapter: &InterfaceStatus) -> Line {
    let counters = adapter.counters;
    traffic_text("Sent", counters.sent_bytes, counters.sent_packets, 0)
}

/// The link status line's text.
pub type Status = Text<128>;

/// A link change asked of the kernel and not yet seen in its facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Mtu(u32),
    Wake(bool),
}

/// What the MTU field's text asks for (docs/ux/network-link-v0.1.md section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtuInput {
    NotANumber,
    OutOfRange,
    Unchanged,
    Set(u32),
}

/// Reads the MTU field's `text` against the adapter's `current` and `max_mtu`.
pub fn mtu_input(text: &str, current: u32, max_mtu: u32) -> MtuInput {
    let text = text.trim();
    let digits = !text.is_empty()
        && text.len() <= MOST_MTU_DIGITS
        && text.bytes().all(|byte| byte.is_ascii_digit());
    let Some(value) = text.parse::<u32>().ok().filter(|_| digits) else {
        return MtuInput::NotANumber;
    };
    match value {
        value if !(MIN_MTU..=max_mtu.max(MIN_MTU)).contains(&value) => MtuInput::OutOfRange,
        value if value == current => MtuInput::Unchanged,
        value => MtuInput::Set(value),
    }
}

/// The status line at rest for an adapter taking up to `max_mtu`, waking or not.
pub fn rest_text(max_mtu: u32, can_wake: bool) -> Status {
    let mut text = Status::new();
    let _ = match max_mtu > STANDARD_MTU {
        true => write!(
            text,
            "MTU from {MIN_MTU} to {max_mtu} bytes on this adapter; 1500 is the standard."
        ),
        false => write!(
            text,
            "MTU from {MIN_MTU} to {max_mtu} bytes on this adapter, which takes no jumbo frames."
        ),
    };
    if !can_wake {
        let _ = write!(text, " It cannot wake the computer.");
    }
    text
}

/// The sentence for an MTU field that holds no number, or one out of `max_mtu`'s range.
pub fn mtu_problem_text(input: MtuInput, max_mtu: u32) -> Option<Status> {
    let mut text = Status::new();
    let _ = match input {
        MtuInput::NotANumber => write!(
            text,
            "Not a number. Type the MTU in bytes, {MIN_MTU} to {max_mtu}."
        ),
        MtuInput::OutOfRange => write!(
            text,
            "The MTU must be {MIN_MTU} to {max_mtu} on this adapter."
        ),
        MtuInput::Unchanged | MtuInput::Set(_) => return None,
    };
    Some(text)
}

/// What a refused link change says, by the kernel's `errno`, for the MTU `mtu` (`None`: the
/// change was Wake-on-LAN's); shared by the pane and `settings set` (D734).
pub fn refusal_text(errno: Errno, mtu: Option<u32>) -> Status {
    let mut text = Status::new();
    let words = core::str::from_utf8(errno.message()).unwrap_or("error");
    let _ = match (errno, mtu) {
        (Errno::EPERM, _) => write!(text, "Not allowed: this user may not change the adapter."),
        (Errno::EINVAL, Some(mtu)) => write!(text, "The adapter does not take MTU {mtu}."),
        (Errno::EOPNOTSUPP, Some(_)) => write!(text, "The adapter cannot do it."),
        (Errno::EOPNOTSUPP | Errno::EINVAL, None) => {
            write!(text, "This adapter cannot wake the computer.")
        }
        (Errno::ENODEV, _) => write!(text, "No such adapter any more."),
        (_, Some(_)) => write!(text, "Could not set the MTU: {words}."),
        (_, None) => write!(text, "Could not change Wake-on-LAN: {words}."),
    };
    text
}

/// What a refused link change says where no number can be filled in (`settings set`): by the
/// kernel's `errno`, for the MTU when `mtu` says so, else for Wake-on-LAN.
pub fn refusal_words(errno: Errno, mtu: bool) -> &'static [u8] {
    match (errno, mtu) {
        (Errno::EPERM, _) => b"not allowed: this user may not change the adapter",
        (Errno::EINVAL, true) => b"the adapter does not take that MTU",
        (Errno::EOPNOTSUPP, true) => b"the adapter cannot do it",
        (Errno::EOPNOTSUPP | Errno::EINVAL, false) => b"this adapter cannot wake the computer",
        (Errno::ENODEV, _) => b"no such adapter any more",
        (other, _) => other.message(),
    }
}

/// The Network pane's widgets and the adapter they show.
#[derive(Debug, Clone, Copy)]
pub struct NetworkPane {
    widgets: [WidgetId; WIDGETS],
    shown: Option<Option<InterfaceStatus>>,
    wifi_adapter: bool,
    wifi_connected: bool,
    field_value: Option<u32>,
    pending: Option<Pending>,
    result_shown: bool,
}

const ADAPTER_LINE: usize = 1;
const STATE_VALUE: usize = 2;
const HINT: usize = 3;
const FIRST_ROW_NAME: usize = 4;
const HARDWARE: usize = 12;
const RECEIVED: usize = 13;
const SENT: usize = 14;
const MTU_LABEL: usize = 15;
const MTU_FIELD: usize = 16;
const SET_MTU: usize = 17;
const WAKE_LABEL: usize = 18;
const WAKE_BOX: usize = 19;
const LINK_STATUS: usize = 20;
const LINK_ROWS: [usize; LINK_WIDGETS] =
    [MTU_LABEL, MTU_FIELD, SET_MTU, WAKE_LABEL, WAKE_BOX, LINK_STATUS];

/// What a response did to the link rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkReply {
    NotMine,
    Done(Option<Request>),
}

fn set_ink(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, ink: Token) {
    tree.change(id, |kind| {
        if let Kind::Label { colour, .. } = kind {
            *colour = ink;
        }
    });
}

fn set_checked(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, wanted: bool) {
    tree.change(id, |kind| {
        if let Kind::Checkbox { checked, .. } = kind {
            *checked = wanted;
        }
    });
}

fn field_number(tree: &WidgetTree<CAPACITY>, field: WidgetId) -> Option<u32> {
    tree.text_of(field)
        .and_then(|text| text.trim().parse::<u32>().ok())
}

impl NetworkPane {
    /// Builds the pane's widgets in `pane` under the heading `title`, showing `adapter` (`None`:
    /// the machine has none).
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        title: &str,
        adapter: Option<&InterfaceStatus>,
    ) -> Self {
        let at = |x: i32, y: i32| Place::at(x, y, Length::Content, Length::Content);
        let row_top = |row: usize| FIRST_ROW_TEXT + row as i32 * ROW_PITCH;
        let mut add = |place: Place, kind: Kind| {
            tree.add(Some(pane), place, kind)
                .expect("the Settings window fits its tree")
        };
        let mut widgets = [add(at(PADDING, HEADING_TOP), Kind::heading(title)); WIDGETS];
        widgets[ADAPTER_LINE] = add(at(PADDING, row_top(0)), Kind::label(ETHERNET, Token::Text));
        widgets[STATE_VALUE] = add(
            Place::right(PADDING, row_top(0)),
            Kind::label("", Token::Text),
        );
        widgets[HINT] = add(at(PADDING, row_top(1)), Kind::label("", Token::TextDim));
        for (row, name) in ROW_NAMES.iter().enumerate() {
            let top = row_top(row + 1);
            widgets[FIRST_ROW_NAME + 2 * row] =
                add(at(PADDING, top), Kind::label(name, Token::Text));
            widgets[FIRST_ROW_NAME + 2 * row + 1] =
                add(Place::right(PADDING, top), Kind::label("", Token::Text));
        }
        for (line, widget) in [HARDWARE, RECEIVED, SENT].into_iter().enumerate() {
            let top = HARDWARE_LINE + line as i32 * LINE_PITCH;
            widgets[widget] = add(at(PADDING, top), Kind::label("", Token::Text));
        }
        let sized = |(x, y, width, height): (i32, i32, i32, i32)| {
            Place::at(x, y, Length::Px(width), Length::Px(height))
        };
        widgets[MTU_LABEL] = add(
            at(PADDING, MTU_ROW + ROW_LABEL_DOWN),
            Kind::label("MTU", Token::Text),
        );
        widgets[MTU_FIELD] = add(sized(FIELD_PLACE), Kind::text_field("", Escape::Clear));
        widgets[SET_MTU] = add(sized(SET_PLACE), Kind::button("Set", false));
        widgets[WAKE_LABEL] = add(
            at(PADDING, WAKE_ROW + ROW_LABEL_DOWN),
            Kind::label("Wake-on-LAN", Token::Text),
        );
        widgets[WAKE_BOX] = add(
            Place::at(WAKE_X, WAKE_ROW, Length::Content, Length::Px(CONTROL_HEIGHT)),
            Kind::checkbox(MAGIC_PACKET, false),
        );
        widgets[LINK_STATUS] = add(
            Place::at(PADDING, STATUS_TOP, Length::Px(STATUS_WIDTH), Length::Content),
            Kind::label("", Token::TextDim),
        );
        let mut built = Self {
            widgets,
            shown: None,
            wifi_adapter: false,
            wifi_connected: false,
            field_value: None,
            pending: None,
            result_shown: false,
        };
        built.show(tree, adapter);
        built
    }

    /// Names the adapter after the Wi-Fi section's state (D283, docs/ux/wifi-v0.4.md section 1):
    /// `Wi-Fi` when joined, and hints that point at the Wi-Fi section when there is an adapter.
    pub fn show_wifi(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        wifi_adapter: bool,
        wifi_connected: bool,
        adapter: Option<&InterfaceStatus>,
    ) {
        if (self.wifi_adapter, self.wifi_connected) == (wifi_adapter, wifi_connected) {
            return;
        }
        self.wifi_adapter = wifi_adapter;
        self.wifi_connected = wifi_connected;
        self.shown = None;
        self.show(tree, adapter);
    }

    /// The pane's widgets in the design's order (docs/ux/network-v0.3.md section 2): index 0 is
    /// the heading, widget `n` of the design is at `n - 1`.
    pub fn widgets(&self) -> [WidgetId; WIDGETS] {
        self.widgets
    }

    /// Shows `adapter` (`None`: the machine has none), setting only what differs from what is
    /// shown: a state change sets the first row, the value and the hint and shows or hides the
    /// connected rows; the lease and the traffic lines damage their own rects.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, adapter: Option<&InterfaceStatus>) {
        if self.shown == Some(adapter.copied()) {
            return;
        }
        self.shown = Some(adapter.copied());
        let state = adapter.map(InterfaceStatus::state);
        let first_row = match (adapter, self.wifi_adapter) {
            (Some(_), _) if self.wifi_connected => WIFI,
            (Some(_), _) => ETHERNET,
            (None, true) => NO_WIRED_ADAPTER,
            (None, false) => NO_ADAPTER,
        };
        tree.set_text(self.widgets[ADAPTER_LINE], first_row);
        if let Some(state) = state {
            tree.set_text(self.widgets[STATE_VALUE], state.text());
        }
        tree.set_shown(self.widgets[STATE_VALUE], state.is_some());
        let explanation = match (state, self.wifi_adapter) {
            (Some(State::CableUnplugged), true) if !self.wifi_connected => Some(CABLE_OR_WIFI_HINT),
            (Some(state), _) => hint(state),
            (None, true) => Some(JOIN_WIFI_HINT),
            (None, false) => Some(NO_ADAPTER_HINT),
        };
        if let Some(explanation) = explanation {
            tree.set_text(self.widgets[HINT], explanation);
        }
        tree.set_shown(self.widgets[HINT], explanation.is_some());
        let connected = adapter
            .filter(|adapter| adapter.state() == State::Connected)
            .and_then(|adapter| adapter.subnet.map(|subnet| (adapter, subnet)));
        if let Some((adapter, subnet)) = connected {
            let router = match adapter.router {
                Some(router) => {
                    let mut line = Line::new();
                    let _ = write!(line, "{router}");
                    line
                }
                None => Line::from(NOTHING_GIVEN),
            };
            let values = [
                address_text(subnet),
                router,
                servers_text(adapter, ", "),
                dhcp_text(adapter),
            ];
            for (row, value) in values.iter().enumerate() {
                tree.set_text(self.widgets[FIRST_ROW_NAME + 2 * row + 1], value.as_str());
            }
        }
        for widget in &self.widgets[FIRST_ROW_NAME..HARDWARE] {
            tree.set_shown(*widget, connected.is_some());
        }
        if let Some(adapter) = adapter {
            tree.set_text(self.widgets[HARDWARE], hardware_text(adapter).as_str());
            tree.set_text(self.widgets[RECEIVED], received_text(adapter).as_str());
            tree.set_text(self.widgets[SENT], sent_text(adapter).as_str());
        }
        for widget in [HARDWARE, RECEIVED, SENT] {
            tree.set_shown(self.widgets[widget], adapter.is_some());
        }
        self.show_link(tree, adapter);
    }

    fn confirm_pending(&mut self, tree: &mut WidgetTree<CAPACITY>, adapter: &InterfaceStatus) {
        let words = match self.pending {
            Some(Pending::Mtu(wanted)) if adapter.mtu == wanted => {
                let mut done = Status::new();
                let _ = write!(done, "MTU set to {wanted}.");
                done
            }
            Some(Pending::Wake(true)) if adapter.wake_on_lan == Some(true) => {
                Status::from(WAKE_ON_TEXT)
            }
            Some(Pending::Wake(false)) if adapter.wake_on_lan == Some(false) => {
                Status::from("Wake-on-LAN off.")
            }
            _ => return,
        };
        self.pending = None;
        self.field_value = None;
        self.say(tree, words.as_str());
    }

    fn show_link(&mut self, tree: &mut WidgetTree<CAPACITY>, adapter: Option<&InterfaceStatus>) {
        for widget in LINK_ROWS {
            tree.set_shown(self.widgets[widget], adapter.is_some());
        }
        let Some(adapter) = adapter else {
            return;
        };
        self.confirm_pending(tree, adapter);
        let can_wake = adapter.wake_on_lan.is_some();
        let field = self.widgets[MTU_FIELD];
        let empty = tree.text_of(field).is_none_or(|text| text.trim().is_empty());
        let untouched = empty
            || self
                .field_value
                .is_none_or(|value| field_number(tree, field) == Some(value));
        if self.pending.is_none() && tree.focus() != Some(field) && untouched {
            let mut value = Line::new();
            let _ = write!(value, "{}", adapter.mtu);
            tree.set_text(field, value.as_str());
            self.field_value = Some(adapter.mtu);
        }
        let mtu_busy = matches!(self.pending, Some(Pending::Mtu(_)));
        tree.set_enabled(field, !mtu_busy);
        let typed = tree.text_of(field).is_some_and(|text| !text.trim().is_empty());
        let differs = typed && field_number(tree, field) != Some(adapter.mtu);
        tree.set_enabled(self.widgets[SET_MTU], !mtu_busy && differs);
        let wake_busy = matches!(self.pending, Some(Pending::Wake(_)));
        if !wake_busy {
            set_checked(tree, self.widgets[WAKE_BOX], adapter.wake_on_lan == Some(true));
        }
        tree.set_enabled(self.widgets[WAKE_BOX], can_wake && !wake_busy);
        let wake_ink = match can_wake {
            true => Token::Text,
            false => Token::TextDim,
        };
        set_ink(tree, self.widgets[WAKE_LABEL], wake_ink);
        if !self.result_shown && self.pending.is_none() {
            let rest = rest_text(adapter.max_mtu, can_wake);
            tree.set_text(self.widgets[LINK_STATUS], rest.as_str());
            set_ink(tree, self.widgets[LINK_STATUS], Token::TextDim);
        }
    }

    fn say(&mut self, tree: &mut WidgetTree<CAPACITY>, words: &str) {
        tree.set_text(self.widgets[LINK_STATUS], words);
        set_ink(tree, self.widgets[LINK_STATUS], Token::Text);
        self.result_shown = true;
    }

    /// The pane was shown again: a result or error gives way to the rest sentence.
    pub fn reset_link(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        adapter: Option<&InterfaceStatus>,
    ) {
        self.result_shown = false;
        self.show_link(tree, adapter);
    }

    /// What `response` does to the link rows of `adapter`: `Set` (or Enter in the field, which
    /// reaches the window's `default_button`) checks the MTU and asks for it, the checkbox asks
    /// for Wake-on-LAN on or off, an edit enables `Set` and an emptied field is filled again.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        adapter: Option<&InterfaceStatus>,
        default_button: WidgetId,
    ) -> LinkReply {
        let field = self.widgets[MTU_FIELD];
        let Some(adapter) = adapter else {
            return LinkReply::NotMine;
        };
        match response {
            Response::Edited(id) if id == field => {
                self.show_link(tree, Some(adapter));
                LinkReply::Done(None)
            }
            Response::Activated(id)
                if id == self.widgets[SET_MTU]
                    || (id == default_button && tree.focus() == Some(field)) =>
            {
                let typed = tree.text_of(field).unwrap_or("");
                let input = mtu_input(typed, adapter.mtu, adapter.max_mtu);
                if let Some(problem) = mtu_problem_text(input, adapter.max_mtu) {
                    self.say(tree, problem.as_str());
                    return LinkReply::Done(None);
                }
                let MtuInput::Set(mtu) = input else {
                    return LinkReply::Done(None);
                };
                self.pending = Some(Pending::Mtu(mtu));
                let mut words = Status::new();
                let _ = write!(words, "Setting MTU {mtu}...");
                self.say(tree, words.as_str());
                tree.set_enabled(field, false);
                tree.set_enabled(self.widgets[SET_MTU], false);
                LinkReply::Done(Some(Request::SetMtu(mtu)))
            }
            Response::Toggled(id, on) if id == self.widgets[WAKE_BOX] => {
                self.pending = Some(Pending::Wake(on));
                tree.set_enabled(self.widgets[WAKE_BOX], false);
                let words = match on {
                    true => "Turning Wake-on-LAN on...",
                    false => "Turning Wake-on-LAN off...",
                };
                self.say(tree, words);
                LinkReply::Done(Some(Request::SetWakeOnLan(on)))
            }
            _ => LinkReply::NotMine,
        }
    }

    /// The kernel refused the link change asked for: the status line says why, the field keeps
    /// its text for correction and the checkbox goes back.
    pub fn link_refused(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        errno: Errno,
        adapter: Option<&InterfaceStatus>,
    ) {
        let mtu = match self.pending.take() {
            Some(Pending::Mtu(mtu)) => Some(mtu),
            _ => None,
        };
        let words = refusal_text(errno, mtu);
        self.show_link(tree, adapter);
        self.say(tree, words.as_str());
    }
}

#[cfg(test)]
mod tests;
