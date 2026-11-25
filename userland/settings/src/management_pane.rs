//! The Management pane (D664; editions-policy v0.1 section 5.3): who manages the computer,
//! what `/etc/eggos/policy` locks, in a read-only table, and what it allows agents. Listed
//! only while the `management` component is installed; the file is read each time the pane
//! is shown ([`crate::settings_window::Request::ReadPolicy`]).

use crate::facts::Facts;
use crate::keys::Key;
use crate::managed::PolicyFile;
use crate::settings_window::CAPACITY;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_settings_file::policy;
use eggos_toolkit::cell_store::{CellSlot, CellStore, data_of};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{WidgetId, WidgetTree};
use eggos_toolkit::widget::{Align, Column, Kind, Length, Place, Table};

/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 7;
/// The most locked settings the table holds.
pub const MAX_ROWS: usize = 64;
/// The table's columns: `Setting`, `Value` and `Pane`, none sortable.
pub const COLUMNS: [Column; 3] = [
    fixed("Setting", 216),
    fixed("Value", 104),
    fixed("Pane", 112),
];
/// What the table says when nothing is locked.
pub const NOTHING_LOCKED: &str = "No setting is locked.";

const PADDING: i32 = 16;
const MANAGER_TOP: i32 = 64;
const CONTACT_TOP: i32 = 84;
const FILE_TOP: i32 = 104;
const TABLE: (i32, i32, i32, i32) = (16, 132, 432, 148);
const AGENTS_TOP: i32 = 288;
const STATUS_TOP: i32 = 308;
const LINE_HEIGHT: i32 = 20;
const CELL_COLUMNS: usize = 3;
const SLOTS: usize = if cfg!(test) { 64 } else { 2 };

static POLICY_CELLS: CellStore<SLOTS, MAX_ROWS, CELL_COLUMNS> = CellStore::new();

type PolicySlot = CellSlot<'static, SLOTS, MAX_ROWS, CELL_COLUMNS>;

const fn fixed(title: &'static str, width: i32) -> Column {
    Column {
        sortable: false,
        ..Column::new(title, width, Align::Start)
    }
}

fn policy_cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    POLICY_CELLS.write(data, row, column, out);
}

/// The pane that shows a key, by its name's first part.
pub fn pane_of(key: &str) -> &'static str {
    match key.split('.').next().unwrap_or("") {
        "appearance" | "shell" => "Appearance",
        "display" => "Display",
        "keyboard" => "Keyboard",
        "mouse" => "Mouse",
        "accessibility" => "Accessibility",
        "power" => "Power",
        "time" => "Date & time",
        "update" => "Components",
        _ => "",
    }
}

/// The agents line, `Agents: at most view write. Denied: 3 tools.`; `None` when the policy
/// says nothing about agents.
pub fn agents_line(text: &str) -> Option<String> {
    let ceiling = policy::agent_ceiling(text);
    let denied = policy::denied_tools(text).count();
    let mut line = String::from("Agents:");
    match ceiling {
        Some(classes) if classes.trim().is_empty() => line.push_str(" reading only."),
        Some(classes) => line.push_str(&format!(
            " at most {}.",
            classes.split_ascii_whitespace().collect::<alloc::vec::Vec<_>>().join(" ")
        )),
        None if denied == 0 => return None,
        None => {}
    }
    match denied {
        0 => {}
        1 => line.push_str(" Denied: 1 tool."),
        _ => line.push_str(&format!(" Denied: {denied} tools.")),
    }
    Some(line)
}

/// The Management pane's widgets and its table's cells.
#[derive(Debug)]
pub struct ManagementPane {
    manager: WidgetId,
    contact: WidgetId,
    file: WidgetId,
    table: WidgetId,
    agents: WidgetId,
    status: WidgetId,
    slot: Option<PolicySlot>,
    row_panes: Vec<&'static str>,
}

impl ManagementPane {
    /// Builds the pane in `pane`, showing nothing read yet.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId) -> Self {
        let mut add = |place: Place, kind: Kind| {
            tree.add(Some(pane), place, kind)
                .expect("the Settings window fits its tree")
        };
        let line = |top: i32| Place::at(PADDING, top, Length::Content, Length::Content);
        add(line(PADDING), Kind::heading("Management"));
        let manager = add(line(MANAGER_TOP), Kind::label("", Token::Text));
        let contact = add(line(CONTACT_TOP), Kind::label("", Token::TextDim));
        let file = add(line(FILE_TOP), Kind::label("", Token::TextDim));
        let mut table = Table::new("Locked settings", &COLUMNS, 0, policy_cell, 0);
        table.empty_text = NOTHING_LOCKED;
        let table = add(
            Place::at(TABLE.0, TABLE.1, Length::Px(TABLE.2), Length::Px(TABLE.3)),
            Kind::Table(table),
        );
        let agents = add(line(AGENTS_TOP), Kind::label("", Token::TextDim));
        let status = add(line(STATUS_TOP), Kind::label("", Token::Text));
        Self {
            manager,
            contact,
            file,
            table,
            agents,
            status,
            slot: None,
            row_panes: Vec::new(),
        }
    }

    /// Places the pane for a page `content` wide (editions-policy v0.1 section 10: the three
    /// lines, the table the page's width, then the two lines), or the desktop's places; the
    /// pane's height.
    pub fn reflow(&self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) -> i32 {
        let width = content.map_or(Length::Px(TABLE.2), Length::Px);
        tree.set_place(
            self.table,
            Place::at(TABLE.0, TABLE.1, width, Length::Px(TABLE.3)),
        );
        let line = |top: i32| Place::at(PADDING, top, Length::Content, Length::Content);
        tree.set_place(self.agents, line(AGENTS_TOP));
        tree.set_place(self.status, line(STATUS_TOP));
        STATUS_TOP + LINE_HEIGHT + PADDING
    }

    /// The pane that shows the setting on row `row` of the table, if one does.
    pub fn pane_of_row(&self, row: usize) -> Option<&'static str> {
        self.row_panes
            .get(row)
            .copied()
            .filter(|pane| !pane.is_empty())
    }

    /// The table of locked settings.
    pub fn table(&self) -> WidgetId {
        self.table
    }

    /// Shows what `file` says, with `(pending)` after a value `facts` do not have yet.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, file: &PolicyFile, facts: &Facts) {
        if self.slot.is_none() {
            self.slot = POLICY_CELLS.take();
        }
        let (manager, contact, facts_line, agents) = match file {
            PolicyFile::Trusted(text) => {
                let count = file.locks().len();
                let noun = if count == 1 { "setting" } else { "settings" };
                (
                    format!("Managed by {}", file.manager()),
                    policy::contact(text).map(String::from),
                    format!("{} \u{b7} {count} {noun} locked", policy::PATH),
                    agents_line(text),
                )
            }
            PolicyFile::Untrusted(why) => (
                String::from("The policy file cannot be trusted"),
                None,
                format!("{} Nothing is locked; agents can only read.", why.sentence()),
                None,
            ),
            PolicyFile::Absent | PolicyFile::NotInstalled => (
                String::from("Not managed"),
                None,
                format!("No policy file at {}.", policy::PATH),
                None,
            ),
        };
        tree.set_text(self.manager, &manager);
        tree.set_text(self.contact, contact.as_deref().unwrap_or(""));
        tree.set_shown(self.contact, contact.is_some());
        tree.set_text(self.file, &facts_line);
        tree.set_text(self.agents, agents.as_deref().unwrap_or(""));
        tree.set_shown(self.agents, agents.is_some());
        let mut rows = 0;
        let mut pending = false;
        self.row_panes.clear();
        for (key, value) in file.locks().into_iter().take(MAX_ROWS) {
            self.row_panes.push(pane_of(key));
            let running = Key::named(key).map(|own| own.value(facts));
            let differs = running.is_some_and(|running| running.as_str() != value);
            pending |= differs;
            if let Some(slot) = &self.slot {
                slot.set(rows, 0, key);
                let shown = match differs {
                    true => format!("{value} (pending)"),
                    false => String::from(value),
                };
                slot.set(rows, 1, &shown);
                slot.set(rows, 2, pane_of(key));
            }
            rows += 1;
        }
        tree.set_cell_data(self.table, data_of(self.slot.as_ref()));
        tree.set_rows(self.table, rows);
        let ignored = file.text().map_or(0, |text| {
            policy::lines(text)
                .filter(|(number, _, said)| {
                    crate::managed::ignored_reason(text, *number, *said).is_some()
                })
                .count()
        });
        let status = match (ignored, pending) {
            (0, false) => String::new(),
            (0, true) => String::from("Some values are not applied yet: settings policy --apply."),
            (1, _) => String::from("1 line ignored: settings policy lists it."),
            (count, _) => format!("{count} lines ignored: settings policy lists them."),
        };
        tree.set_text(self.status, &status);
        tree.set_shown(self.status, !status.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agents_line_counts_denied_tools_and_keys_find_their_panes() {
        assert_eq!(
            agents_line("agents.grants=view  write\nagents.deny=a b c\n").as_deref(),
            Some("Agents: at most view write. Denied: 3 tools.")
        );
        assert_eq!(agents_line("agents.deny=disk_format\n").as_deref(), Some("Agents: Denied: 1 tool."));
        assert_eq!(agents_line("shell.profile=linux\n"), None);
        assert_eq!(pane_of("appearance.theme"), "Appearance");
        assert_eq!(pane_of("update.policy"), "Components");
        assert_eq!(pane_of("network.state"), "");
    }

    #[test]
    fn the_row_shows_with_the_component_and_the_pane_reads_the_file() {
        use crate::settings_window::{COMPONENTS, MANAGEMENT, Request, SettingsWindow};
        let mut window = SettingsWindow::new(Facts::example());
        assert!(!window.management_shown());
        let file = PolicyFile::Trusted(String::from(
            "policy.manager=Contoso IT\nshell.profile=linux\nagents.grants=view\n",
        ));
        assert_eq!(window.show_policy(file), None);
        assert!(window.management_shown());
        assert_eq!(window.open_pane(MANAGEMENT), Some(Request::ReadPolicy));
        let table = window.management_pane().table();
        let rows = match window.widgets().kind(table) {
            Some(Kind::Table(shown)) => shown.rows,
            _ => 0,
        };
        assert_eq!(rows, 1);
        assert_eq!(window.show_policy(PolicyFile::NotInstalled), Some(Request::ReadComponents));
        assert_eq!(window.pane(), COMPONENTS);
        assert!(!window.management_shown());
    }
}
