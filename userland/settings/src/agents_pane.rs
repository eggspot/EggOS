//! Settings' Agents pane (D193 slice 41c, docs/ux/agents-v0.3.md): the agents EggOS knows and
//! each one's grants, set with the words and the store `agent list|grant|revoke|forget` use
//! (`eggos_mcp::grants`). It holds the widgets and the grants last read; the program reads and
//! writes the grants file (`eggos_mcp::grant_files`) when a [`Request`] asks it to. The controls
//! that change grants are person-only: an agent's `wm act` on them is refused.

use crate::settings_window::{CAPACITY, Request};
use crate::storage_pane::format_panel;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_mcp::grant_files::SaveError;
use eggos_mcp::grants::{
    AgentGrants, AgentId, AgentName, Grant, GrantStore, Grants, LOCAL_AGENT_ID, MAX_AGENTS,
    Permission, StoreError, StoreState,
};
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::text::{ELLIPSIS, Text};
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

const LABEL_BYTES: usize = 64;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const AGENT_LIST: (i32, i32, i32, i32) = (16, 64, 208, 72);
const AGENT_ROW: i32 = 24;
const MARK_X: i32 = 8;
const TEXT_X: i32 = 24;
const VISIBLE_ROWS: usize = 3;
const ROW_LABEL_X: i32 = 40;
const ROW_LABEL_TOP: i32 = 68;
const ROW_LABEL_WIDTH: i32 = 184;
const STATUS_LINE: i32 = 312;
const LINE_WIDTH: i32 = 432;
const ID_FIELD: (i32, i32, i32, i32) = (240, 64, 104, 28);
const BUTTON_SIZE: (i32, i32) = (96, 28);
const ADD_AT: (i32, i32) = (344, 64);
const REVOKE_ALL_AT: (i32, i32) = (240, 100);
const FORGET_AT: (i32, i32) = (344, 100);
const CLASS_LINE_TOP: i32 = 154;
const CLASS_RADIOS_TOP: i32 = 148;
const CLASS_PITCH: i32 = 32;
const CLASS_LINE_WIDTH: i32 = 224;
const CLASS_RADIOS: (i32, i32) = (242, 206);
const LASTS_LINE_TOP: i32 = 282;
const LASTS_RADIOS: (i32, i32, i32) = (158, 276, 290);
const CONTROL_HEIGHT: i32 = 28;
const PANEL: (i32, i32, i32, i32) = (32, 108, 400, 132);
const PANEL_LINES: [i32; 2] = [16, 36];
const PANEL_LINE_WIDTH: i32 = 368;
const CANCEL_AT: (i32, i32) = (192, 88);
const GRANT_AT: (i32, i32) = (288, 88);
const EMPTY_ROWS: [&str; 32] = [""; 32];

/// The class rows' titles, in [`Grant::ALL`] order.
pub const CLASS_TITLES: [&str; 4] = ["View", "Write", "Install", "Run"];
/// A class's choices, weakest first, in [`Permission`] order.
pub const PERMISSION_TITLES: [&str; 3] = ["Deny", "Ask", "Allow"];
const PERMISSIONS: [Permission; 3] = [Permission::Deny, Permission::Ask, Permission::Allow];
/// The `Grants last` choices: no end, then the CLI's `--for 15m`, `2h`, `7d`.
pub const LASTS: [&str; 4] = ["No end", "15 min", "2 h", "7 d"];
const LASTS_SECONDS: [Option<u64>; 4] = [None, Some(15 * 60), Some(2 * 3600), Some(7 * 86_400)];

const LOCAL_LINE: &str = "local is EggOS's own channel: revoke it, never forget it.";
const NO_DISK_LINE: &str = "No disk attached: changes last until power-off.";
const FULL_LINE: &str = "EggOS keeps at most 16 agents: forget one first.";
const BAD_ID_LINE: &str = "An id is 1 to 32 letters, digits, \".\", \"_\" or \"-\".";
const LOCAL_ID_LINE: &str = "local is always listed.";

/// What the program read for the pane: the grants file, the grants EggOS was started with, the
/// wall clock's seconds and whether the files are on a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantsRead {
    /// The grants file.
    pub store: StoreState,
    /// The grants on the kernel command line (`--mcp-allow`), every agent's ceiling.
    pub boot: Grants,
    /// The wall clock, in seconds since the epoch.
    pub now: u64,
    /// Whether `/var` is on a disk; without one a change lasts until power-off.
    pub on_disk: bool,
}

impl Default for GrantsRead {
    fn default() -> Self {
        Self {
            store: StoreState::Missing,
            boot: Grants::NONE,
            now: 0,
            on_disk: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Gave(Grant, Permission, Option<u64>),
    TookBack(Grant),
    RevokedAll,
    Forgot,
    Added,
}

/// What the pane did with a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// It was not for the pane.
    NotMine,
    /// The pane acted on it, and the program must do this, if anything.
    Done(Option<Request>),
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    list: WidgetId,
    rows: [WidgetId; VISIBLE_ROWS],
    status: WidgetId,
    id_field: WidgetId,
    add: WidgetId,
    revoke_all: WidgetId,
    forget: WidgetId,
    class_lines: [WidgetId; 4],
    class_radios: [WidgetId; 4],
    lasts: WidgetId,
    panel: WidgetId,
    question: WidgetId,
    meaning: WidgetId,
    cancel: WidgetId,
    grant: WidgetId,
}

/// The Agents pane's widgets, the grants last read and the change waiting to be written.
#[derive(Debug)]
pub struct AgentsPane {
    widgets: Widgets,
    read: GrantsRead,
    rows: Vec<AgentGrants>,
    to_write: GrantStore,
    change: Option<(AgentId, Change)>,
    confirming: Option<(Grant, Permission)>,
    status: Option<String>,
    default_button: WidgetId,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn line(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, x: i32, y: i32, width: i32) -> WidgetId {
    add(
        tree,
        parent,
        Place::at(x, y, Length::Px(width), Length::Content),
        Kind::label("", colour::TEXT),
    )
}

fn button(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    place: (i32, i32),
    text: &str,
) -> WidgetId {
    add(
        tree,
        parent,
        at(place.0, place.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
        Kind::button(text, false),
    )
}

fn shortened(words: &str) -> Text<LABEL_BYTES> {
    let mut text = Text::new();
    if text.push_str(words) {
        return text;
    }
    let room = LABEL_BYTES - ELLIPSIS.len();
    let end = (0..=room)
        .rev()
        .find(|&at| words.is_char_boundary(at))
        .unwrap_or(0);
    let mut text = Text::new();
    text.push_str(&words[..end]);
    text.push_str(ELLIPSIS);
    text
}

fn show_line(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, ink: u32) {
    tree.set_text(id, shortened(words).as_str());
    tree.change(id, |kind| {
        if let Kind::Label { colour, .. } = kind {
            *colour = ink;
        }
    });
}

fn bytes_text(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("?")
}

fn class_index(grant: Grant) -> usize {
    Grant::ALL
        .iter()
        .position(|&each| each == grant)
        .unwrap_or(0)
}

fn permission_index(permission: Permission) -> usize {
    PERMISSIONS
        .iter()
        .position(|&each| each == permission)
        .unwrap_or(0)
}

/// `2026-10-31 11:14`: a UTC stamp to the minute, as the class lines show an end.
pub fn minute_stamp(seconds: u64) -> String {
    let mut stamp = String::new();
    let full = i64::try_from(seconds)
        .ok()
        .and_then(eggos_calendar::rfc3339);
    if let Some(full) = full {
        stamp.push_str(bytes_text(full.get(..10).unwrap_or_default()));
        stamp.push(' ');
        stamp.push_str(bytes_text(full.get(11..16).unwrap_or_default()));
    }
    stamp
}

fn local_name() -> AgentName {
    AgentName::new(LOCAL_AGENT_ID).unwrap_or_else(|| unreachable!())
}

fn local_from_boot(boot: Grants) -> AgentGrants {
    let mut local = AgentGrants::new(AgentId::LOCAL, local_name());
    for grant in Grant::ALL.into_iter().filter(|&grant| boot.allows(grant)) {
        local.set(grant, Permission::Allow, None);
    }
    local
}

/// An agent's row: `phone · Claude on my phone`, or the id alone when the name is the id.
pub fn row_text(agent: &AgentGrants) -> String {
    let mut row = String::from(bytes_text(agent.id().as_bytes()));
    let name = bytes_text(agent.name().as_bytes());
    if name.as_bytes() != agent.id().as_bytes() {
        row.push_str(" \u{b7} ");
        row.push_str(name);
    }
    row
}

/// A class line: its title and the first note that applies (docs/ux/agents-v0.3.md section 3).
pub fn class_text(agent: &AgentGrants, grant: Grant, read: &GrantsRead) -> String {
    let mut text = String::from(CLASS_TITLES[class_index(grant)]);
    let class = agent.class(grant);
    let own = class.at(read.now).unwrap_or(Permission::Deny);
    let run = agent
        .class(Grant::Run)
        .at(read.now)
        .unwrap_or(Permission::Deny);
    let note = if !read.boot.allows(grant) {
        Some(String::from("started without it"))
    } else if matches!(read.store, StoreState::Unreadable(_)) {
        Some(String::from("file untrusted"))
    } else if grant != Grant::Run && run > own {
        Some(String::from("run brings it"))
    } else {
        match (class.permission, class.until) {
            (Permission::Deny, _) | (_, None) => None,
            (_, Some(until)) if until > read.now => {
                Some(alloc::format!("until {}", minute_stamp(until)))
            }
            (_, Some(until)) => Some(alloc::format!("ended {}", minute_stamp(until))),
        }
    };
    if let Some(note) = note {
        text.push_str(" \u{b7} ");
        text.push_str(&note);
    }
    text
}

fn boot_words(boot: Grants) -> String {
    let mut words = String::new();
    for grant in Grant::ALL.into_iter().filter(|&grant| boot.allows(grant)) {
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str(grant.name());
    }
    match words.is_empty() {
        true => String::from("Started with: none. Agents can only read."),
        false => alloc::format!("Started with: {words}. No agent gets more."),
    }
}

fn change_text(
    id: &AgentId,
    change: Change,
    agent: Option<&AgentGrants>,
    read: &GrantsRead,
) -> String {
    let id = bytes_text(id.as_bytes());
    let mut text = String::new();
    let _ = match change {
        Change::Gave(grant, _, _) if !read.boot.allows(grant) => write!(
            text,
            "Gave {} to {id}; EggOS was started without it.",
            grant.name()
        ),
        Change::Gave(grant, Permission::Ask, _) => {
            write!(text, "Gave {} to {id}, asking each time.", grant.name())
        }
        Change::Gave(grant, _, Some(until)) => write!(
            text,
            "Gave {} to {id} until {}.",
            grant.name(),
            minute_stamp(until)
        ),
        Change::Gave(grant, _, None) => {
            write!(text, "Gave {} to {id}, from its next call.", grant.name())
        }
        Change::TookBack(grant)
            if grant != Grant::Run
                && agent.is_some_and(|agent| {
                    agent
                        .class(Grant::Run)
                        .at(read.now)
                        .unwrap_or(Permission::Deny)
                        > Permission::Deny
                }) =>
        {
            write!(
                text,
                "Took {} back from {id}; run still brings it.",
                grant.name()
            )
        }
        Change::TookBack(grant) => {
            write!(
                text,
                "Took {} back from {id}, from its next call.",
                grant.name()
            )
        }
        Change::RevokedAll => write!(text, "Took every grant back from {id}."),
        Change::Forgot => write!(text, "Forgot {id}: it gets nothing until granted again."),
        Change::Added => write!(text, "Added {id} with no grants: choose what it may do."),
    };
    text
}

fn save_error_text(error: SaveError) -> String {
    match error {
        SaveError::Store(StoreError::Full) => {
            String::from("Grants file cannot hold this: it holds too many agents.")
        }
        SaveError::Store(error) => alloc::format!("Grants file not written: {}.", error.reason()),
        SaveError::File(_, errno) => {
            alloc::format!("Grants file not written: {}.", bytes_text(errno.message()))
        }
    }
}

impl AgentsPane {
    /// Builds the pane in `pane`, a container of the window's; `default_button` is the window's
    /// default button, given back when the confirmation panel closes.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        default_button: WidgetId,
    ) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading("Agents"),
        );
        let list = add(
            tree,
            pane,
            at(AGENT_LIST.0, AGENT_LIST.1, AGENT_LIST.2, AGENT_LIST.3),
            Kind::list(&EMPTY_ROWS, AGENT_ROW, MARK_X, TEXT_X),
        );
        let rows = core::array::from_fn(|row| {
            let y = ROW_LABEL_TOP + row as i32 * AGENT_ROW;
            line(tree, pane, ROW_LABEL_X, y, ROW_LABEL_WIDTH)
        });
        let status = line(tree, pane, PADDING, STATUS_LINE, LINE_WIDTH);
        let id_field = add(
            tree,
            pane,
            at(ID_FIELD.0, ID_FIELD.1, ID_FIELD.2, ID_FIELD.3),
            Kind::text_field("Agent id", Escape::Clear),
        );
        let add_button = button(tree, pane, ADD_AT, "Add");
        let revoke_all = button(tree, pane, REVOKE_ALL_AT, "Revoke all");
        let forget = button(tree, pane, FORGET_AT, "Forget");
        let mut class_lines = [list; 4];
        let mut class_radios = [list; 4];
        for (index, title) in CLASS_TITLES.iter().enumerate() {
            let offset = index as i32 * CLASS_PITCH;
            class_lines[index] = line(
                tree,
                pane,
                PADDING,
                CLASS_LINE_TOP + offset,
                CLASS_LINE_WIDTH,
            );
            tree.set_text(class_lines[index], title);
            class_radios[index] = add(
                tree,
                pane,
                at(
                    CLASS_RADIOS.0,
                    CLASS_RADIOS_TOP + offset,
                    CLASS_RADIOS.1,
                    CONTROL_HEIGHT,
                ),
                Kind::radios(&PERMISSION_TITLES, 0, true),
            );
        }
        let lasts_line = line(tree, pane, PADDING, LASTS_LINE_TOP, CLASS_LINE_WIDTH);
        tree.set_text(lasts_line, "Grants last");
        let lasts = add(
            tree,
            pane,
            at(
                LASTS_RADIOS.0,
                LASTS_RADIOS.1,
                LASTS_RADIOS.2,
                CONTROL_HEIGHT,
            ),
            Kind::radios(&LASTS, 0, true),
        );
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
        let question = line(tree, panel, PADDING, PANEL_LINES[0], PANEL_LINE_WIDTH);
        let meaning = line(tree, panel, PADDING, PANEL_LINES[1], PANEL_LINE_WIDTH);
        let cancel = button(tree, panel, CANCEL_AT, "Cancel");
        let grant = add(
            tree,
            panel,
            at(GRANT_AT.0, GRANT_AT.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
            Kind::button("Grant", true),
        );
        for id in [add_button, revoke_all, forget, lasts, cancel, grant]
            .into_iter()
            .chain(class_radios)
        {
            tree.set_person_only(id, true);
        }
        tree.set_shown(panel, false);
        let mut agents = Self {
            widgets: Widgets {
                list,
                rows,
                status,
                id_field,
                add: add_button,
                revoke_all,
                forget,
                class_lines,
                class_radios,
                lasts,
                panel,
                question,
                meaning,
                cancel,
                grant,
            },
            read: GrantsRead::default(),
            rows: Vec::new(),
            to_write: GrantStore::new(),
            change: None,
            confirming: None,
            status: None,
            default_button,
        };
        agents.show_grants(tree, GrantsRead::default());
        agents
    }

    /// The pane is shown again: `local` is selected once the grants are read, the id field is
    /// emptied and `Grants last` goes back to `No end`.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if self.confirming.is_some() {
            self.close_panel(tree, None);
        }
        self.status = None;
        self.change = None;
        tree.set_text(self.widgets.id_field, "");
        tree.set_selected(self.widgets.lasts, 0);
        tree.set_selected(self.widgets.list, 0);
    }

    /// Shows the grants just read; the selection stays on the same agent when it is still
    /// listed.
    pub fn show_grants(&mut self, tree: &mut WidgetTree<CAPACITY>, read: GrantsRead) {
        let selected = self.selected(tree).map(|agent| *agent.id());
        self.read = read;
        let store = match read.store {
            StoreState::Loaded(store) => store,
            _ => GrantStore::new(),
        };
        let local = store
            .get(&AgentId::LOCAL)
            .copied()
            .unwrap_or_else(|| local_from_boot(read.boot));
        self.rows.clear();
        self.rows.push(local);
        self.rows.extend(
            store
                .agents()
                .filter(|agent| !agent.id().is_local())
                .copied(),
        );
        self.to_write = store;
        let shown = match self.rows.len() {
            count if count >= 32 => u32::MAX,
            count => (1u32 << count) - 1,
        };
        tree.set_shown_items(self.widgets.list, shown);
        let index = selected
            .and_then(|id| self.rows.iter().position(|agent| *agent.id() == id))
            .unwrap_or(0);
        tree.set_selected(self.widgets.list, index);
        self.refresh(tree);
    }

    /// The written change's outcome, and the grants read again after it.
    pub fn grants_written(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        written: Result<(), SaveError>,
        read: GrantsRead,
    ) {
        let change = self.change.take();
        if let Some((id, Change::Added)) = change
            && written.is_ok()
        {
            tree.set_text(self.widgets.id_field, "");
            self.show_grants(tree, read);
            if let Some(index) = self.rows.iter().position(|agent| *agent.id() == id) {
                tree.set_selected(self.widgets.list, index);
            }
        } else {
            self.show_grants(tree, read);
        }
        self.status = match (written, change) {
            (Err(error), _) => Some(save_error_text(error)),
            (Ok(()), Some((id, change))) => {
                let agent = self.rows.iter().find(|agent| *agent.id() == id);
                Some(change_text(&id, change, agent, &self.read))
            }
            (Ok(()), None) => None,
        };
        self.refresh(tree);
    }

    /// The grants file the program must write for [`Request::WriteGrants`].
    pub fn store_to_write(&self) -> &GrantStore {
        &self.to_write
    }

    /// The agent selected in the list.
    pub fn selected<'a>(&'a self, tree: &WidgetTree<CAPACITY>) -> Option<&'a AgentGrants> {
        self.rows.get(tree.selected(self.widgets.list)?)
    }

    /// Whether the confirmation panel is open.
    pub fn panel_open(&self) -> bool {
        self.confirming.is_some()
    }

    fn writable(&self) -> bool {
        !matches!(self.read.store, StoreState::Unreadable(_))
    }

    fn top_row(&self, tree: &WidgetTree<CAPACITY>) -> usize {
        match tree.kind(self.widgets.list) {
            Some(Kind::List(list)) if list.row_height > 0 => {
                (list.offset / list.row_height) as usize
            }
            _ => 0,
        }
    }

    fn id_typed(&self, tree: &WidgetTree<CAPACITY>) -> Result<Option<AgentId>, &'static str> {
        let typed = tree.text_of(self.widgets.id_field).unwrap_or("").trim();
        match typed {
            "" => Ok(None),
            _ if typed.as_bytes() == LOCAL_AGENT_ID => Err(LOCAL_ID_LINE),
            _ => AgentId::new(typed.as_bytes()).map(Some).ok_or(BAD_ID_LINE),
        }
    }

    fn idle_line(&self, tree: &WidgetTree<CAPACITY>) -> (String, u32) {
        if let StoreState::Unreadable(error) = self.read.store {
            let line = alloc::format!("Grants file cannot be trusted: {}.", error.reason());
            return (line, colour::TEXT_DIM);
        }
        if let Err(problem) = self.id_typed(tree) {
            return (String::from(problem), colour::TEXT_DIM);
        }
        let local = self
            .selected(tree)
            .is_none_or(|agent| agent.id().is_local());
        let line = match (local, self.read.on_disk) {
            (true, _) => String::from(LOCAL_LINE),
            _ if self.store_full() => String::from(FULL_LINE),
            (false, false) => String::from(NO_DISK_LINE),
            (false, true) => boot_words(self.read.boot),
        };
        (line, colour::TEXT_DIM)
    }

    fn store_full(&self) -> bool {
        self.to_write.agents().count() >= MAX_AGENTS
    }

    fn refresh(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let top = self.top_row(tree);
        for (row, id) in widgets.rows.iter().enumerate() {
            let text = self.rows.get(top + row).map(row_text).unwrap_or_default();
            show_line(tree, *id, &text, colour::TEXT);
        }
        let writable = self.writable() && self.confirming.is_none();
        let selected = self.selected(tree).copied();
        for (index, grant) in Grant::ALL.into_iter().enumerate() {
            let (text, choice) = match &selected {
                Some(agent) => {
                    let choice = agent
                        .class(grant)
                        .at(self.read.now)
                        .unwrap_or(Permission::Deny);
                    (class_text(agent, grant, &self.read), choice)
                }
                None => (String::from(CLASS_TITLES[index]), Permission::Deny),
            };
            show_line(tree, widgets.class_lines[index], &text, colour::TEXT);
            tree.set_selected(widgets.class_radios[index], permission_index(choice));
            tree.set_enabled(widgets.class_radios[index], writable && selected.is_some());
        }
        let local = selected.is_none_or(|agent| agent.id().is_local());
        let id_ok = matches!(self.id_typed(tree), Ok(Some(_)));
        tree.set_enabled(widgets.lasts, writable);
        tree.set_enabled(widgets.revoke_all, writable && selected.is_some());
        tree.set_enabled(widgets.forget, writable && !local);
        tree.set_enabled(widgets.add, writable && id_ok && !self.store_full());
        tree.set_enabled(widgets.id_field, self.writable());
        let (line, ink) = match &self.status {
            Some(status) => (status.clone(), colour::TEXT),
            None => self.idle_line(tree),
        };
        show_line(tree, widgets.status, &line, ink);
    }

    fn write(&mut self, id: AgentId, change: Change) -> Option<Request> {
        self.change = Some((id, change));
        self.status = None;
        Some(Request::WriteGrants)
    }

    fn entry_to_write(&mut self, agent: &AgentGrants) -> Option<&mut AgentGrants> {
        if self.to_write.get(agent.id()).is_none() {
            let entry = self.to_write.add(*agent.id(), *agent.name()).ok()?;
            *entry = *agent;
            return Some(entry);
        }
        self.to_write.get_mut(agent.id())
    }

    fn choose(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        grant: Grant,
        permission: Permission,
        now_ms: u32,
    ) -> Option<Request> {
        let agent = *self.selected(tree)?;
        let current = agent
            .class(grant)
            .at(self.read.now)
            .unwrap_or(Permission::Deny);
        if permission == current || !self.writable() {
            self.refresh(tree);
            return None;
        }
        let needs_yes =
            matches!(grant, Grant::Run | Grant::Install) && permission != Permission::Deny;
        if needs_yes {
            self.open_panel(tree, &agent, grant, permission, now_ms);
            return None;
        }
        self.apply(tree, &agent, grant, permission)
    }

    fn apply(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        agent: &AgentGrants,
        grant: Grant,
        permission: Permission,
    ) -> Option<Request> {
        let lasts = tree.selected(self.widgets.lasts).unwrap_or(0);
        let until = match permission {
            Permission::Deny => None,
            _ => LASTS_SECONDS
                .get(lasts)
                .copied()
                .flatten()
                .map(|seconds| self.read.now.saturating_add(seconds)),
        };
        self.entry_to_write(agent)?.set(grant, permission, until);
        let change = match permission {
            Permission::Deny => Change::TookBack(grant),
            _ => Change::Gave(grant, permission, until),
        };
        self.write(*agent.id(), change)
    }

    fn open_panel(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        agent: &AgentGrants,
        grant: Grant,
        permission: Permission,
        now_ms: u32,
    ) {
        let widgets = self.widgets;
        let id = bytes_text(agent.id().as_bytes());
        let question = alloc::format!("Grant {} to {id}?", grant.name());
        let meaning = match (grant, permission) {
            (Grant::Run, Permission::Ask) => {
                "Each call asks you. Run gives it full control of EggOS."
            }
            (Grant::Run, _) => "Run gives it full control of EggOS.",
            (_, Permission::Ask) => "Each call asks you. Install lets it add programs.",
            _ => "Install lets it add programs.",
        };
        show_line(tree, widgets.question, &question, colour::TEXT);
        show_line(tree, widgets.meaning, meaning, colour::TEXT_DIM);
        self.confirming = Some((grant, permission));
        tree.set_shown(widgets.panel, true);
        tree.set_default_button(widgets.grant);
        self.refresh(tree);
        tree.set_focus(Some(widgets.cancel), now_ms);
    }

    fn close_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, focus_at_ms: Option<u32>) {
        let widgets = self.widgets;
        let grant = self.confirming.take().map(|(grant, _)| grant);
        tree.set_shown(widgets.panel, false);
        tree.set_default_button(self.default_button);
        self.refresh(tree);
        if let (Some(now_ms), Some(grant)) = (focus_at_ms, grant) {
            tree.set_focus(Some(widgets.class_radios[class_index(grant)]), now_ms);
        }
    }

    fn revoke_all(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let agent = *self.selected(tree)?;
        let entry = self.entry_to_write(&agent)?;
        for grant in Grant::ALL {
            entry.revoke(grant);
        }
        self.write(*agent.id(), Change::RevokedAll)
    }

    fn forget(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let agent = *self.selected(tree)?;
        if agent.id().is_local() || !self.to_write.forget(agent.id()) {
            return None;
        }
        let above = tree
            .selected(self.widgets.list)
            .unwrap_or(1)
            .saturating_sub(1);
        tree.set_selected(self.widgets.list, above);
        self.write(*agent.id(), Change::Forgot)
    }

    fn add_typed(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let Ok(Some(id)) = self.id_typed(tree) else {
            return None;
        };
        if let Some(index) = self.rows.iter().position(|agent| *agent.id() == id) {
            tree.set_text(self.widgets.id_field, "");
            tree.set_selected(self.widgets.list, index);
            self.refresh(tree);
            return None;
        }
        let name = AgentName::new(id.as_bytes())?;
        if self.store_full() || !self.writable() {
            return None;
        }
        self.to_write.add(id, name).ok()?;
        self.write(id, Change::Added)
    }

    /// Acts on `response` when it is the pane's.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
    ) -> Reply {
        let widgets = self.widgets;
        let request = match response {
            Response::Chosen(id, _) if id == widgets.list => {
                self.status = None;
                self.refresh(tree);
                None
            }
            Response::Chosen(id, choice) if widgets.class_radios.contains(&id) => {
                let index = widgets
                    .class_radios
                    .iter()
                    .position(|&radios| radios == id)
                    .unwrap_or(0);
                let permission = PERMISSIONS.get(choice).copied().unwrap_or(Permission::Deny);
                self.choose(tree, Grant::ALL[index], permission, now_ms)
            }
            Response::Chosen(id, _) if id == widgets.lasts => None,
            Response::Edited(id) if id == widgets.id_field => {
                self.status = None;
                self.refresh(tree);
                None
            }
            Response::Unused {
                code: key::ENTER, ..
            } if tree.focus() == Some(widgets.id_field) => self.add_typed(tree),
            Response::Activated(id) if id == widgets.add => self.add_typed(tree),
            Response::Activated(id) if id == widgets.revoke_all => self.revoke_all(tree),
            Response::Activated(id) if id == widgets.forget => self.forget(tree),
            Response::Activated(id) if id == widgets.cancel => {
                self.close_panel(tree, Some(now_ms));
                None
            }
            Response::Activated(id) if id == widgets.grant => {
                let confirming = self.confirming;
                self.close_panel(tree, Some(now_ms));
                let agent = self.selected(tree).copied();
                match (confirming, agent) {
                    (Some((grant, permission)), Some(agent)) => {
                        self.apply(tree, &agent, grant, permission)
                    }
                    _ => None,
                }
            }
            Response::Unused { code: key::ESC, .. } if self.confirming.is_some() => {
                self.close_panel(tree, Some(now_ms));
                None
            }
            Response::Nothing => {
                self.refresh_rows(tree);
                return Reply::NotMine;
            }
            _ => return Reply::NotMine,
        };
        Reply::Done(request)
    }

    fn refresh_rows(&self, tree: &mut WidgetTree<CAPACITY>) {
        let top = self.top_row(tree);
        for (row, id) in self.widgets.rows.iter().enumerate() {
            let text = self.rows.get(top + row).map(row_text).unwrap_or_default();
            if tree.text_of(*id) != Some(shortened(&text).as_str()) {
                show_line(tree, *id, &text, colour::TEXT);
            }
        }
    }

    /// The visible agent rows' texts.
    pub fn row_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; VISIBLE_ROWS] {
        self.widgets.rows.map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// The four class lines' texts.
    pub fn class_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; 4] {
        self.widgets
            .class_lines
            .map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// The four class choices shown, as indexes of [`PERMISSION_TITLES`].
    pub fn class_choices(&self, tree: &WidgetTree<CAPACITY>) -> [Option<usize>; 4] {
        self.widgets.class_radios.map(|id| tree.selected(id))
    }

    /// The status line's text.
    pub fn status<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        tree.text_of(self.widgets.status)
    }

    /// Whether `Add`, `Revoke all` and `Forget` are enabled.
    pub fn buttons_enabled(&self, tree: &WidgetTree<CAPACITY>) -> [bool; 3] {
        [
            self.widgets.add,
            self.widgets.revoke_all,
            self.widgets.forget,
        ]
        .map(|id| tree.is_enabled(id))
    }

    /// The confirmation panel's two lines while it shows.
    pub fn panel_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<[&'a str; 2]> {
        self.confirming?;
        Some([self.widgets.question, self.widgets.meaning].map(|id| tree.text_of(id).unwrap_or("")))
    }

    /// The widgets tests and agents address: the agents list, the id field, `Add`, `Revoke all`,
    /// `Forget`, the class groups, `Grants last`, `Cancel` and `Grant`.
    pub fn widget_ids(&self) -> AgentsWidgets {
        let widgets = self.widgets;
        AgentsWidgets {
            list: widgets.list,
            id_field: widgets.id_field,
            add: widgets.add,
            revoke_all: widgets.revoke_all,
            forget: widgets.forget,
            class_radios: widgets.class_radios,
            lasts: widgets.lasts,
            cancel: widgets.cancel,
            grant: widgets.grant,
        }
    }
}

/// The Agents pane's widgets that tests and agents address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentsWidgets {
    /// The agents list.
    pub list: WidgetId,
    /// The `Agent id` field.
    pub id_field: WidgetId,
    /// `Add`.
    pub add: WidgetId,
    /// `Revoke all`.
    pub revoke_all: WidgetId,
    /// `Forget`.
    pub forget: WidgetId,
    /// The class groups, in [`Grant::ALL`] order.
    pub class_radios: [WidgetId; 4],
    /// `Grants last`.
    pub lasts: WidgetId,
    /// The panel's `Cancel`.
    pub cancel: WidgetId,
    /// The panel's `Grant`.
    pub grant: WidgetId,
}

#[cfg(test)]
mod tests;
