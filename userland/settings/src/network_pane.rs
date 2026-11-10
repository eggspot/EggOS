//! The Network pane (D225, docs/ux/network-v0.3.md sections 0-3): the wired adapter as
//! `/proc/net/interfaces` reports it, through the parse `ip` and `net_status` use
//! (`eggos_net::status`), read-only and in one of five states named once for the GUI, the CLI
//! and MCP. Each of its 15 widgets has one place; a state only shows, hides and fills them in,
//! so the pane fills in where it is as the machine connects. Not the Agents pane's
//! [`crate::network_view`], which is the MCP network server.

use crate::settings_window::CAPACITY;
use core::fmt::Write;
use eggos_net::dhcp::INFINITE_LEASE_S;
use eggos_net::ipv4::Subnet;
use eggos_net::status::{InterfaceStatus, State};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

/// One of the pane's lines.
pub type Line = Text<64>;

/// The pane's widgets: the heading, then the design's widgets 2 to 15.
pub const WIDGETS: usize = 15;
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

const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const FIRST_ROW_TEXT: i32 = 76;
const ROW_PITCH: i32 = 40;
const HARDWARE_LINE: i32 = 276;
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
        adapter.mac,
        adapter.mtu
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

/// The Network pane's widgets and the adapter they show.
#[derive(Debug, Clone, Copy)]
pub struct NetworkPane {
    widgets: [WidgetId; WIDGETS],
    shown: Option<Option<InterfaceStatus>>,
}

const ADAPTER_LINE: usize = 1;
const STATE_VALUE: usize = 2;
const HINT: usize = 3;
const FIRST_ROW_NAME: usize = 4;
const HARDWARE: usize = 12;
const RECEIVED: usize = 13;
const SENT: usize = 14;

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
        widgets[ADAPTER_LINE] = add(
            at(PADDING, row_top(0)),
            Kind::label(ETHERNET, Token::Text),
        );
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
        let mut built = Self {
            widgets,
            shown: None,
        };
        built.show(tree, adapter);
        built
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
        let first_row = match adapter {
            Some(_) => ETHERNET,
            None => NO_ADAPTER,
        };
        tree.set_text(self.widgets[ADAPTER_LINE], first_row);
        if let Some(state) = state {
            tree.set_text(self.widgets[STATE_VALUE], state.text());
        }
        tree.set_shown(self.widgets[STATE_VALUE], state.is_some());
        let explanation = match state {
            Some(state) => hint(state),
            None => Some(NO_ADAPTER_HINT),
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
    }
}

#[cfg(test)]
mod tests;
