//! The Agents pane's `Network` view and pairing panel (D193 slice 41d,
//! docs/ux/agents-network-v0.3.md): whether agents on other machines are answered over HTTPS,
//! the certificate they pin, and pairing one with a one-time code, through the functions `agent
//! network` and `agent pair` use (`eggos_mcp::pairing`). Turning the network on or off and
//! pairing are person-only: an agent's `wm act` on those controls is refused.

use crate::agents_pane::{add, at, line, show_line};
use crate::settings_window::{CAPACITY, Request};
use crate::storage_pane::format_panel;
use alloc::string::String;
use eggos_mcp::grants::{AgentGrants, AgentId, AgentName, LOCAL_AGENT_ID, MAX_AGENT_NAME_BYTES};
use eggos_mcp::pairing::{
    CODE_LIFETIME_S, Code, DEFAULT_PORT, FINGERPRINT_TEXT_BYTES, MAX_WRONG_CODES, NetworkState,
    PairError, PendingPairing, code_text,
};
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

const PADDING: i32 = 16;
const PAIR_BUTTON: (i32, i32, i32, i32) = (240, 64, 208, 28);
const NETWORK_LINE_TOP: i32 = 154;
const NETWORK_RADIOS_TOP: i32 = 148;
const LINES_TOP: [i32; 4] = [186, 206, 226, 246];
const SECOND_HALF_X: i32 = 60;
const LINE_WIDTH: i32 = 432;
const PANEL: (i32, i32, i32, i32) = (32, 56, 400, 236);
const PANEL_LINE_WIDTH: i32 = 368;
const FIELD_HEIGHT: i32 = 28;
const ID_FIELD: (i32, i32, i32) = (16, 48, 176);
const NAME_FIELD: (i32, i32, i32) = (208, 48, 176);
const PANEL_TITLE_TOP: i32 = 16;
const HINT_TOP: i32 = 88;
const CODE_AT: (i32, i32) = (16, 48);
const URL_TOP: i32 = 108;
const PANEL_FINGERPRINT_TOPS: [i32; 2] = [136, 156];
const COUNTDOWN_TOP: i32 = 198;
const BUTTON_SIZE: (i32, i32) = (96, 28);
const CANCEL_AT: (i32, i32) = (192, 192);
const PAIR_AT: (i32, i32) = (288, 192);
const FINGERPRINT_SPLIT: usize = 48;

/// The network switch's choices.
pub const NETWORK_CHOICES: [&str; 2] = ["Off", "On"];
const ON: usize = 1;

const ANSWERED_LINE: &str = "Agents on the network are answered from EggOS's next start.";
const NOT_ANSWERED_LINE: &str = "Agents on the network are not answered from EggOS's next start.";
const IDLE_LINE: &str = "Pairing shows a one-time code for an agent on another machine.";
const OFF_LINE: &str = "Off: agents on the network are not answered.";
const PEM_LINE: &str = "Certificate for the client: /var/lib/eggos/agent-network.pem";
const NO_CERTIFICATE_LINE: &str = "Certificate: made when the network server first starts";
const ASK_TITLE: &str = "Pair an agent on another machine";
const BAD_ID_LINE: &str = "An id is 1 to 32 letters, digits, \".\", \"_\" or \"-\".";
const LOCAL_LINE: &str = "local is the host's channel, which needs no pairing.";
const LONG_NAME_LINE: &str = "A name is at most 64 bytes.";
const FULL_LINE: &str = "EggOS keeps at most 16 agents: forget one first.";
const NEW_AGENT_LINE: &str = "A one-time code for its client follows; it works for 5 minutes.";
const SENDS_LINE: &str = "Its client sends this code once, within 5 minutes, to";
const OFF_NOTE: &str = "The network server is off: choose On, then start EggOS again.";
const ENDED_LINE: &str = "The code has ended; pair again for a new one.";

/// What the program read for the view: the network file's state, the certificate's
/// fingerprint, the pairing waiting and the wall clock's seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkRead {
    /// Whether agents on the network are answered from the next start, and on which port.
    pub state: NetworkState,
    /// The certificate's fingerprint, once the server made one.
    pub fingerprint: Option<[u8; FINGERPRINT_TEXT_BYTES]>,
    /// The pairing waiting in the pairing file.
    pub pending: Option<PendingPairing>,
    /// The wall clock, in seconds since the epoch.
    pub now: u64,
}

impl Default for NetworkRead {
    fn default() -> Self {
        Self {
            state: NetworkState::Off,
            fingerprint: None,
            pending: None,
            now: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Ask,
    Code(AgentId, Code),
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    pair: WidgetId,
    network_line: WidgetId,
    switch: WidgetId,
    lines: [WidgetId; 4],
    panel: WidgetId,
    title: WidgetId,
    id_field: WidgetId,
    name_field: WidgetId,
    hint: WidgetId,
    code: WidgetId,
    url: WidgetId,
    fingerprint: [WidgetId; 2],
    countdown: WidgetId,
    cancel: WidgetId,
    primary: WidgetId,
}

/// The `Network` view's widgets, the network last read and the pairing panel's step.
#[derive(Debug)]
pub struct NetworkView {
    widgets: Widgets,
    read: NetworkRead,
    written_this_run: bool,
    step: Option<Step>,
    status: Option<String>,
    hint_refusal: Option<String>,
}

fn fingerprint_halves(fingerprint: &[u8; FINGERPRINT_TEXT_BYTES]) -> (String, String) {
    let text = core::str::from_utf8(fingerprint).unwrap_or_default();
    let (first, second) = text.split_at(FINGERPRINT_SPLIT.min(text.len()));
    (alloc::format!("SHA-256 {first}"), String::from(second))
}

/// The clock time of `seconds` since the epoch, as `09:19:02` (UTC).
pub fn clock_time(seconds: u64) -> String {
    let of_day = seconds % 86_400;
    alloc::format!(
        "{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// Why pairing could not begin, in the pane's words.
pub fn pair_error_text(error: PairError) -> String {
    match error {
        PairError::Burned(wait) => {
            alloc::format!("The last code was burned by wrong tries: pair again in {wait} s.")
        }
        PairError::Randomness(errno) => alloc::format!(
            "No randomness for a code: {}.",
            core::str::from_utf8(errno.message()).unwrap_or("failed")
        ),
        PairError::Store(eggos_mcp::grants::StoreError::Full) => {
            String::from("Grants file cannot hold this: it holds too many agents.")
        }
        PairError::Store(error) => {
            alloc::format!("Grants file cannot be trusted: {}.", error.reason())
        }
        PairError::File(path, errno) => {
            let what = match path.to_bytes().starts_with(b"/run") {
                true => "Pairing file",
                false => "Grants file",
            };
            alloc::format!(
                "{what} not written: {}.",
                core::str::from_utf8(errno.message()).unwrap_or("failed")
            )
        }
    }
}

impl NetworkView {
    /// Builds the view's widgets and the pairing panel in `pane`, hidden.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId) -> Self {
        let pair = add(
            tree,
            pane,
            at(PAIR_BUTTON.0, PAIR_BUTTON.1, PAIR_BUTTON.2, PAIR_BUTTON.3),
            Kind::button("Pair an agent...", false),
        );
        let network_line = line(tree, pane, PADDING, NETWORK_LINE_TOP, LINE_WIDTH);
        tree.set_text(network_line, "Network");
        let switch = add(
            tree,
            pane,
            Place::right(PADDING, NETWORK_RADIOS_TOP),
            Kind::radios(&NETWORK_CHOICES, 0, true),
        );
        let lines = [
            line(tree, pane, PADDING, LINES_TOP[0], LINE_WIDTH),
            line(tree, pane, PADDING, LINES_TOP[1], LINE_WIDTH),
            line(tree, pane, PADDING, LINES_TOP[2], LINE_WIDTH),
            line(tree, pane, SECOND_HALF_X, LINES_TOP[3], LINE_WIDTH),
        ];
        let panel = add(
            tree,
            pane,
            at(PANEL.0, PANEL.1, PANEL.2, PANEL.3),
            Kind::Panel {
                fill: None,
                border_right: None,
            },
        );
        add(
            tree,
            panel,
            Place::fill(),
            Kind::Custom {
                painter: format_panel,
                data: 0,
            },
        );
        let title = line(tree, panel, PADDING, PANEL_TITLE_TOP, PANEL_LINE_WIDTH);
        let id_field = add(
            tree,
            panel,
            at(ID_FIELD.0, ID_FIELD.1, ID_FIELD.2, FIELD_HEIGHT),
            Kind::text_field("Agent id", Escape::Clear),
        );
        let name_field = add(
            tree,
            panel,
            at(NAME_FIELD.0, NAME_FIELD.1, NAME_FIELD.2, FIELD_HEIGHT),
            Kind::text_field("Name, optional", Escape::Clear),
        );
        let hint = line(tree, panel, PADDING, HINT_TOP, PANEL_LINE_WIDTH);
        let code = add(
            tree,
            panel,
            Place::at(CODE_AT.0, CODE_AT.1, Length::Content, Length::Content),
            Kind::heading(""),
        );
        let url = line(tree, panel, PADDING, URL_TOP, PANEL_LINE_WIDTH);
        let fingerprint = [
            line(
                tree,
                panel,
                PADDING,
                PANEL_FINGERPRINT_TOPS[0],
                PANEL_LINE_WIDTH,
            ),
            line(
                tree,
                panel,
                SECOND_HALF_X,
                PANEL_FINGERPRINT_TOPS[1],
                PANEL_LINE_WIDTH,
            ),
        ];
        let countdown = line(tree, panel, PADDING, COUNTDOWN_TOP, PANEL_LINE_WIDTH);
        let cancel = add(
            tree,
            panel,
            at(CANCEL_AT.0, CANCEL_AT.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
            Kind::button("Cancel", false),
        );
        let primary = add(
            tree,
            panel,
            at(PAIR_AT.0, PAIR_AT.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
            Kind::button("Pair", true),
        );
        for id in [pair, switch, id_field, name_field, cancel, primary] {
            tree.set_person_only(id, true);
        }
        tree.set_shown(panel, false);
        let view = Self {
            widgets: Widgets {
                pair,
                network_line,
                switch,
                lines,
                panel,
                title,
                id_field,
                name_field,
                hint,
                code,
                url,
                fingerprint,
                countdown,
                cancel,
                primary,
            },
            read: NetworkRead::default(),
            written_this_run: false,
            step: None,
            status: None,
            hint_refusal: None,
        };
        view.set_shown(tree, false);
        view
    }

    /// Shows or hides the view's widgets; the panel stays as it is.
    pub fn set_shown(&self, tree: &mut WidgetTree<CAPACITY>, shown: bool) {
        let widgets = self.widgets;
        for id in [widgets.pair, widgets.network_line, widgets.switch]
            .into_iter()
            .chain(widgets.lines)
        {
            tree.set_shown(id, shown);
        }
        if shown {
            self.show_lines(tree);
        }
    }

    /// Whether the pairing panel shows.
    pub fn panel_open(&self) -> bool {
        self.step.is_some()
    }

    /// Whether the panel shows a code, so the program reads the pairing file each second.
    pub fn counting_down(&self) -> bool {
        matches!(self.step, Some(Step::Code(..)))
    }

    /// The pane was shown again: the panel closes, the status line goes back to idle.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.close_panel(tree);
        self.status = None;
    }

    /// Shows the network just read; closes the panel when the code it shows was used.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, read: NetworkRead) {
        self.read = read;
        if let Some(Step::Code(agent, _)) = self.step
            && read.pending.is_none_or(|pending| *pending.agent() != agent)
        {
            let id = core::str::from_utf8(agent.as_bytes()).unwrap_or_default();
            self.status = Some(alloc::format!(
                "Paired {id}: it gets nothing until you grant it."
            ));
            self.close_panel(tree);
        }
        if tree.is_shown(self.widgets.pair) {
            self.show_lines(tree);
        }
        self.show_panel(tree);
    }

    fn show_lines(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let on = matches!(self.read.state, NetworkState::On(_));
        tree.set_selected(widgets.switch, usize::from(on));
        let title = match self.written_this_run {
            true => "Network \u{b7} from EggOS's next start",
            false => "Network",
        };
        tree.set_text(widgets.network_line, title);
        match self.read.state {
            NetworkState::On(port) => show_line(
                tree,
                widgets.lines[0],
                &alloc::format!("Agents connect to https://<this machine>:{port}/mcp"),
                colour::TEXT,
            ),
            NetworkState::Off => show_line(tree, widgets.lines[0], OFF_LINE, colour::TEXT_DIM),
        }
        match &self.read.fingerprint {
            Some(fingerprint) => {
                let (first, second) = fingerprint_halves(fingerprint);
                show_line(tree, widgets.lines[1], PEM_LINE, colour::TEXT);
                show_line(tree, widgets.lines[2], &first, colour::TEXT);
                show_line(tree, widgets.lines[3], &second, colour::TEXT);
                tree.set_shown(widgets.lines[2], tree.is_shown(widgets.pair));
                tree.set_shown(widgets.lines[3], tree.is_shown(widgets.pair));
            }
            None => {
                show_line(
                    tree,
                    widgets.lines[1],
                    NO_CERTIFICATE_LINE,
                    colour::TEXT_DIM,
                );
                tree.set_shown(widgets.lines[2], false);
                tree.set_shown(widgets.lines[3], false);
            }
        }
    }

    /// The status line while the view shows.
    pub fn status_line(&self) -> (String, u32) {
        if let Some(status) = &self.status {
            return (status.clone(), colour::TEXT);
        }
        if self.counting_down() && self.read.state == NetworkState::Off {
            return (String::from(OFF_NOTE), colour::TEXT);
        }
        match self.read.pending {
            Some(pending) if pending.is_open(self.read.now) => {
                let id = core::str::from_utf8(pending.agent().as_bytes()).unwrap_or_default();
                (
                    alloc::format!(
                        "Waiting to pair: {id}; its code ends {}.",
                        clock_time(pending.expires())
                    ),
                    colour::TEXT,
                )
            }
            _ => (String::from(IDLE_LINE), colour::TEXT_DIM),
        }
    }

    fn port(&self) -> u16 {
        match self.read.state {
            NetworkState::On(port) => port,
            NetworkState::Off => DEFAULT_PORT,
        }
    }

    fn typed(&self, tree: &WidgetTree<CAPACITY>) -> (String, String) {
        let id = tree.text_of(self.widgets.id_field).unwrap_or("").trim();
        let name = tree.text_of(self.widgets.name_field).unwrap_or("").trim();
        (String::from(id), String::from(name))
    }

    /// The agent the panel pairs and the name typed, once both are valid.
    pub fn to_pair(&self, tree: &WidgetTree<CAPACITY>) -> Option<(AgentId, Option<AgentName>)> {
        let (id, name) = self.typed(tree);
        let id = AgentId::new(id.as_bytes())?;
        let name = match name.is_empty() {
            true => None,
            false => Some(AgentName::new(name.as_bytes())?),
        };
        Some((id, name))
    }

    fn hint(
        &self,
        tree: &WidgetTree<CAPACITY>,
        rows: &[AgentGrants],
        full: bool,
    ) -> (String, bool) {
        let (id, name) = self.typed(tree);
        if id.as_bytes() == LOCAL_AGENT_ID {
            return (String::from(LOCAL_LINE), false);
        }
        let Some(agent) = AgentId::new(id.as_bytes()) else {
            return (String::from(BAD_ID_LINE), false);
        };
        if name.len() > MAX_AGENT_NAME_BYTES {
            return (String::from(LONG_NAME_LINE), false);
        }
        let listed = rows.iter().any(|row| *row.id() == agent);
        if !listed && full {
            return (String::from(FULL_LINE), false);
        }
        match listed {
            true => (
                alloc::format!("{id} is listed: a new token, and its grants stay."),
                true,
            ),
            false => (String::from(NEW_AGENT_LINE), true),
        }
    }

    fn show_panel(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let Some(Step::Code(agent, code)) = self.step else {
            return;
        };
        let id = core::str::from_utf8(agent.as_bytes()).unwrap_or_default();
        show_line(
            tree,
            widgets.title,
            &alloc::format!("Pairing {id}"),
            colour::TEXT,
        );
        tree.set_text(
            widgets.code,
            core::str::from_utf8(&code_text(&code)).unwrap_or_default(),
        );
        show_line(tree, widgets.hint, SENDS_LINE, colour::TEXT_DIM);
        show_line(
            tree,
            widgets.url,
            &alloc::format!("https://<this machine>:{}/pair", self.port()),
            colour::TEXT,
        );
        match &self.read.fingerprint {
            Some(fingerprint) => {
                let (first, second) = fingerprint_halves(fingerprint);
                show_line(tree, widgets.fingerprint[0], &first, colour::TEXT);
                show_line(tree, widgets.fingerprint[1], &second, colour::TEXT);
                tree.set_shown(widgets.fingerprint[1], true);
            }
            None => {
                show_line(
                    tree,
                    widgets.fingerprint[0],
                    NO_CERTIFICATE_LINE,
                    colour::TEXT_DIM,
                );
                tree.set_shown(widgets.fingerprint[1], false);
            }
        }
        let countdown = match self.read.pending {
            Some(pending) if *pending.agent() == agent => countdown_text(&pending, self.read.now),
            _ => String::from(ENDED_LINE),
        };
        show_line(tree, widgets.countdown, &countdown, colour::TEXT);
    }

    fn open_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let widgets = self.widgets;
        self.step = Some(Step::Ask);
        self.hint_refusal = None;
        tree.set_text(widgets.id_field, "");
        tree.set_text(widgets.name_field, "");
        show_line(tree, widgets.title, ASK_TITLE, colour::TEXT);
        for id in [widgets.id_field, widgets.name_field, widgets.cancel] {
            tree.set_shown(id, true);
        }
        for id in [
            widgets.code,
            widgets.url,
            widgets.fingerprint[0],
            widgets.fingerprint[1],
            widgets.countdown,
        ] {
            tree.set_shown(id, false);
        }
        tree.set_text(widgets.primary, "Pair");
        tree.set_shown(widgets.panel, true);
        tree.set_default_button(widgets.primary);
        tree.set_focus(Some(widgets.id_field), now_ms);
    }

    fn show_code_step(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        agent: AgentId,
        code: Code,
        now_ms: u32,
    ) {
        let widgets = self.widgets;
        self.step = Some(Step::Code(agent, code));
        for id in [widgets.id_field, widgets.name_field, widgets.cancel] {
            tree.set_shown(id, false);
        }
        for id in [
            widgets.code,
            widgets.url,
            widgets.fingerprint[0],
            widgets.fingerprint[1],
            widgets.countdown,
        ] {
            tree.set_shown(id, true);
        }
        tree.set_text(widgets.primary, "Done");
        tree.set_enabled(widgets.primary, true);
        self.show_panel(tree);
        tree.set_focus(Some(widgets.primary), now_ms);
    }

    fn close_panel(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.step = None;
        self.hint_refusal = None;
        tree.set_shown(self.widgets.panel, false);
    }

    /// Updates the ask step's hint and `Pair` after the fields or the agents changed.
    pub fn refresh_panel(&self, tree: &mut WidgetTree<CAPACITY>, rows: &[AgentGrants], full: bool) {
        if self.step != Some(Step::Ask) {
            return;
        }
        let (hint, ok) = self.hint(tree, rows, full);
        match &self.hint_refusal {
            Some(refusal) => show_line(tree, self.widgets.hint, refusal, colour::TEXT),
            None => show_line(tree, self.widgets.hint, &hint, colour::TEXT_DIM),
        }
        tree.set_enabled(self.widgets.primary, ok);
    }

    /// How turning the network on or off went, and the network read again after it.
    pub fn network_turned(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        turned: Result<(), eggos_runtime::errno::Errno>,
        read: NetworkRead,
    ) {
        self.status = Some(match turned {
            Ok(()) => {
                self.written_this_run = true;
                String::from(match read.state {
                    NetworkState::On(_) => ANSWERED_LINE,
                    NetworkState::Off => NOT_ANSWERED_LINE,
                })
            }
            Err(errno) => alloc::format!(
                "Network file not written: {}.",
                core::str::from_utf8(errno.message()).unwrap_or("failed")
            ),
        });
        self.show(tree, read);
    }

    /// How pairing went: the code step on success, the refusal in the ask step otherwise.
    pub fn paired(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        agent: AgentId,
        outcome: Result<Code, PairError>,
        read: NetworkRead,
        now_ms: u32,
    ) {
        self.read = read;
        match outcome {
            Ok(code) => {
                self.hint_refusal = None;
                self.show_code_step(tree, agent, code, now_ms);
            }
            Err(error) => {
                self.hint_refusal = Some(pair_error_text(error));
                if let Some(refusal) = &self.hint_refusal {
                    show_line(tree, self.widgets.hint, refusal, colour::TEXT);
                }
            }
        }
    }

    /// Acts on `response` when it is the view's or the panel's: `Some` with what the program
    /// must do, `None` when it is not.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
        rows: &[AgentGrants],
        full: bool,
    ) -> Option<Option<Request>> {
        let widgets = self.widgets;
        let request = match response {
            Response::Chosen(id, choice) if id == widgets.switch => {
                let wanted = choice == ON;
                let on = matches!(self.read.state, NetworkState::On(_));
                match wanted == on {
                    true => None,
                    false => Some(Request::TurnNetwork(wanted.then_some(self.port()))),
                }
            }
            Response::Activated(id) if id == widgets.pair => {
                let wait = self
                    .read
                    .pending
                    .map_or(0, |pending| pending.wait_before_next(self.read.now));
                if wait > 0 {
                    self.status = Some(pair_error_text(PairError::Burned(wait)));
                    return Some(Some(Request::ReadNetwork));
                }
                self.status = None;
                self.open_panel(tree, now_ms);
                self.refresh_panel(tree, rows, full);
                None
            }
            Response::Edited(id) if id == widgets.id_field || id == widgets.name_field => {
                self.hint_refusal = None;
                self.refresh_panel(tree, rows, full);
                None
            }
            Response::Unused {
                code: key::ENTER, ..
            } if self.step == Some(Step::Ask)
                && matches!(tree.focus(), Some(id) if id == widgets.id_field || id == widgets.name_field) =>
            {
                match tree.is_enabled(widgets.primary) {
                    true => Some(Request::BeginPairing),
                    false => None,
                }
            }
            Response::Activated(id) if id == widgets.primary => match self.step {
                Some(Step::Ask) => Some(Request::BeginPairing),
                _ => {
                    self.close_panel(tree);
                    None
                }
            },
            Response::Activated(id) if id == widgets.cancel => {
                self.close_panel(tree);
                None
            }
            Response::Unused { code: key::ESC, .. } if self.step.is_some() => {
                self.close_panel(tree);
                None
            }
            _ => return None,
        };
        Some(request)
    }

    /// The view's labels, in order: `Network`, the address line, the certificate line and the
    /// fingerprint's two halves.
    pub fn lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; 5] {
        let widgets = self.widgets;
        [
            widgets.network_line,
            widgets.lines[0],
            widgets.lines[1],
            widgets.lines[2],
            widgets.lines[3],
        ]
        .map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// The panel's title, hint, code, address and countdown while it shows.
    pub fn panel_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<[&'a str; 5]> {
        self.step?;
        let widgets = self.widgets;
        Some(
            [
                widgets.title,
                widgets.hint,
                widgets.code,
                widgets.url,
                widgets.countdown,
            ]
            .map(|id| tree.text_of(id).unwrap_or("")),
        )
    }

    /// The widgets tests and agents address.
    pub fn widget_ids(&self) -> NetworkWidgets {
        let widgets = self.widgets;
        NetworkWidgets {
            pair: widgets.pair,
            switch: widgets.switch,
            id_field: widgets.id_field,
            name_field: widgets.name_field,
            cancel: widgets.cancel,
            primary: widgets.primary,
        }
    }
}

/// The view's widgets that tests and agents address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkWidgets {
    /// `Pair an agent...`.
    pub pair: WidgetId,
    /// The `Off` `On` switch.
    pub switch: WidgetId,
    /// The panel's `Agent id` field.
    pub id_field: WidgetId,
    /// The panel's `Name, optional` field.
    pub name_field: WidgetId,
    /// The panel's `Cancel`.
    pub cancel: WidgetId,
    /// The panel's `Pair`, then `Done`.
    pub primary: WidgetId,
}

/// The panel's countdown line for `pending` at `now`.
pub fn countdown_text(pending: &PendingPairing, now: u64) -> String {
    let wait = pending.wait_before_next(now);
    if wait > 0 {
        return alloc::format!(
            "Burned by {MAX_WRONG_CODES} wrong tries; a new code may be made in {wait} s."
        );
    }
    if !pending.is_open(now) {
        return String::from(ENDED_LINE);
    }
    let left = pending.expires().saturating_sub(now).min(CODE_LIFETIME_S);
    let time = alloc::format!("{}:{:02}", left / 60, left % 60);
    match pending.wrong_codes() {
        0 => alloc::format!("Ends in {time}."),
        wrong => alloc::format!("Ends in {time}; {wrong} wrong tries, {MAX_WRONG_CODES} burn it."),
    }
}

#[cfg(test)]
mod tests;
