//! The Agents pane's Activity view (D193 slice 41c2, docs/ux/agents-v0.3.md section 2.2): the
//! audit log's calls, newest first, narrowed by agent, tool, result and time. The rows are
//! `agentlog`'s, read the way it reads them (`eggos_mcp::audit::calls`); the program reads the
//! log when a [`Request::ReadActivity`] asks it to. Every control here only filters, so agents
//! may use them.

use crate::agents_pane::{EMPTY_ROWS, add, at, line, shortened, show_line, top_row};
use crate::settings_window::{CAPACITY, Request};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_mcp::audit::calls::{self, CallRow, CallRows, LogFiles};
use eggos_mcp::audit::{self, Filter, Outcome};
use eggos_mcp::grants::AgentId;
use eggos_runtime::errno::Errno;
use eggos_toolkit::colour;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

/// The most records the view lists: the newest that match.
pub const SHOWN_RECORDS: usize = 32;
/// The result filter's choices.
pub const RESULTS: [&str; 4] = ["Any result", "ok", "error", "refused"];
const RESULT_OUTCOMES: [Option<Outcome>; 4] = [
    None,
    Some(Outcome::Ok),
    Some(Outcome::Error),
    Some(Outcome::Refused),
];
/// The time filter's choices.
pub const SINCE: [&str; 4] = ["Last hour", "Last day", "Last week", "All time"];
const SINCE_SECONDS: [Option<u64>; 4] = [Some(3600), Some(86_400), Some(7 * 86_400), None];
const ALL_TIME: usize = 3;
/// The text shown when no record matches.
pub const NO_RECORDS: &str = "No records match.";

const VISIBLE_RECORDS: usize = 4;
const PADDING: i32 = 16;
const THIS_AGENT: (i32, i32, i32, i32) = (240, 64, 208, 28);
const TOOL_FIELD: (i32, i32, i32, i32) = (240, 100, 208, 28);
const RESULTS_TOP: i32 = 148;
const COUNT_TOP: i32 = 154;
const SINCE_TOP: i32 = 180;
const RECORD_LIST: (i32, i32, i32, i32) = (16, 216, 432, 96);
const RECORD_ROW: i32 = 24;
const MARK_X: i32 = 8;
const TEXT_X: i32 = 24;
const RECORD_LABEL_X: i32 = 40;
const RECORD_LABEL_TOP: i32 = 220;
const RECORD_LABEL_WIDTH: i32 = 408;
const SHORT_ID_BYTES: usize = 12;
const SHORT_ID_KEPT: usize = 9;
const FIELD_SHOWN_BYTES: usize = 256;
const SEPARATOR: &str = " \u{b7} ";

/// What the view asks the log for: [`audit::Filter`]'s fields, the time as how long ago.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityQuery {
    /// Only this agent's calls.
    pub agent: Option<AgentId>,
    /// Only calls of this tool.
    pub tool: Option<String>,
    /// Only calls that ended this way.
    pub outcome: Option<Outcome>,
    /// Only records of the last this many seconds.
    pub within: Option<u64>,
}

/// A row of the view: one call, its fields as `agentlog` shows them (escaped).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityRecord {
    /// The call's last record's time, in seconds of the wall clock.
    pub time: u64,
    /// The agent's id.
    pub agent: String,
    /// The tool.
    pub tool: String,
    /// How the call ended: an outcome's word or `interrupted`.
    pub result: &'static str,
    /// The arguments.
    pub arguments: String,
    /// The detail.
    pub detail: String,
}

/// What reading the log gave the view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityRead {
    /// The newest [`SHOWN_RECORDS`] rows that match, newest first.
    pub records: Vec<ActivityRecord>,
    /// How many rows match in all.
    pub matching: usize,
    /// Why the log could not be read, if it could not.
    pub error: Option<Errno>,
}

fn shown_field(field: &[u8], into: &mut String) {
    let mut out = [0u8; FIELD_SHOWN_BYTES];
    let length = audit::display_into(field, &mut out);
    into.clear();
    into.push_str(&String::from_utf8_lossy(
        out.get(..length).unwrap_or_default(),
    ));
}

impl ActivityRecord {
    fn fill(&mut self, call: &CallRow<'_>) {
        let record = &call.record;
        self.time = record.time;
        self.result = call.result();
        shown_field(record.agent, &mut self.agent);
        shown_field(record.tool, &mut self.tool);
        shown_field(record.arguments, &mut self.arguments);
        shown_field(record.detail, &mut self.detail);
    }
}

/// Reads the log's rows that `query` asks for, the time measured back from `now`.
pub fn read_activity(files: &mut impl LogFiles, query: &ActivityQuery, now: u64) -> ActivityRead {
    let filter = Filter {
        agent: query.agent.as_ref().map(AgentId::as_bytes),
        tool: query.tool.as_deref().map(str::as_bytes),
        outcome: query.outcome,
        since: query.within.map(|within| now.saturating_sub(within)),
        until: None,
    };
    let mut calls = CallRows::new();
    let mut newest: VecDeque<ActivityRecord> = VecDeque::with_capacity(SHOWN_RECORDS);
    let mut matching = 0;
    let mut keep = |call: &CallRow<'_>| {
        matching += 1;
        let mut record = match newest.len() {
            SHOWN_RECORDS => newest.pop_front().unwrap_or_default(),
            _ => ActivityRecord::default(),
        };
        record.fill(call);
        newest.push_back(record);
    };
    let read = calls::each_line(files, &mut |_, line| {
        calls.take(line, &filter, &mut keep);
    });
    if let Err(error) = read {
        return ActivityRead {
            records: Vec::new(),
            matching: 0,
            error: Some(error.errno),
        };
    }
    calls.finish(&filter, &mut keep);
    ActivityRead {
        records: newest.into_iter().rev().collect(),
        matching,
        error: None,
    }
}

fn stamp_part(stamp: &[u8], from: usize, to: usize) -> &str {
    core::str::from_utf8(stamp.get(from..to).unwrap_or_default()).unwrap_or("")
}

fn short_id(id: &str) -> String {
    if id.len() <= SHORT_ID_BYTES {
        return String::from(id);
    }
    let end = (0..=SHORT_ID_KEPT)
        .rev()
        .find(|&at| id.is_char_boundary(at))
        .unwrap_or(0);
    let mut short = String::from(&id[..end]);
    short.push_str("...");
    short
}

/// A record's row: `10-31 09:14:02 · phone · tee · ok`, the month, day and UTC time, the id
/// (past 12 bytes, its first 9 and `...`), the tool and how the call ended.
pub fn record_row(record: &ActivityRecord) -> String {
    let full = i64::try_from(record.time)
        .ok()
        .and_then(eggos_calendar::rfc3339)
        .unwrap_or(*b"????-??-??T??:??:??Z");
    let mut row = String::from(stamp_part(&full, 5, 10));
    row.push(' ');
    row.push_str(stamp_part(&full, 11, 19));
    for part in [
        short_id(&record.agent).as_str(),
        record.tool.as_str(),
        record.result,
    ] {
        row.push_str(SEPARATOR);
        row.push_str(part);
    }
    row
}

/// A record's detail line: `phone · exit status 0, 5 bytes · {"path":"/home/a"}`, the id, the
/// detail and the arguments, each left out when empty.
pub fn detail_line(record: &ActivityRecord) -> String {
    let mut text = record.agent.clone();
    for part in [&record.detail, &record.arguments] {
        if !part.is_empty() {
            text.push_str(SEPARATOR);
            text.push_str(part);
        }
    }
    text
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    this_agent: WidgetId,
    tool: WidgetId,
    results: WidgetId,
    count: WidgetId,
    since: WidgetId,
    list: WidgetId,
    rows: [WidgetId; VISIBLE_RECORDS],
}

/// The Activity view's widgets and the rows last read.
#[derive(Debug)]
pub struct ActivityView {
    widgets: Widgets,
    read: ActivityRead,
}

/// The Activity view's widgets that tests and agents address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityWidgets {
    /// `This agent only`.
    pub this_agent: WidgetId,
    /// The `Tool name` field.
    pub tool: WidgetId,
    /// The result filter.
    pub results: WidgetId,
    /// The count, `7 of 418`.
    pub count: WidgetId,
    /// The time filter.
    pub since: WidgetId,
    /// The records list.
    pub list: WidgetId,
}

impl ActivityView {
    /// Builds the view, hidden, in `pane`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId) -> Self {
        let this_agent = add(
            tree,
            pane,
            at(THIS_AGENT.0, THIS_AGENT.1, THIS_AGENT.2, THIS_AGENT.3),
            Kind::checkbox("This agent only", false),
        );
        let tool = add(
            tree,
            pane,
            at(TOOL_FIELD.0, TOOL_FIELD.1, TOOL_FIELD.2, TOOL_FIELD.3),
            Kind::text_field("Tool name", Escape::Clear),
        );
        let results = add(
            tree,
            pane,
            Place::at(PADDING, RESULTS_TOP, Length::Content, Length::Content),
            Kind::radios(&RESULTS, 0, true),
        );
        let count = add(
            tree,
            pane,
            Place::right(PADDING, COUNT_TOP),
            Kind::label("", colour::TEXT_DIM),
        );
        let since = add(
            tree,
            pane,
            Place::at(PADDING, SINCE_TOP, Length::Content, Length::Content),
            Kind::radios(&SINCE, ALL_TIME, true),
        );
        let list = add(
            tree,
            pane,
            at(RECORD_LIST.0, RECORD_LIST.1, RECORD_LIST.2, RECORD_LIST.3),
            Kind::list(&EMPTY_ROWS, RECORD_ROW, MARK_X, TEXT_X),
        );
        tree.change(list, |kind| {
            if let Kind::List(list) = kind {
                list.empty_text = NO_RECORDS;
            }
        });
        tree.set_shown_items(list, 0);
        let rows = core::array::from_fn(|row| {
            let y = RECORD_LABEL_TOP + row as i32 * RECORD_ROW;
            line(tree, pane, RECORD_LABEL_X, y, RECORD_LABEL_WIDTH)
        });
        let view = Self {
            widgets: Widgets {
                this_agent,
                tool,
                results,
                count,
                since,
                list,
                rows,
            },
            read: ActivityRead::default(),
        };
        view.set_shown(tree, false);
        view
    }

    fn ids(&self) -> [WidgetId; 6 + VISIBLE_RECORDS] {
        let widgets = self.widgets;
        let mut ids = [widgets.list; 6 + VISIBLE_RECORDS];
        ids[..6].copy_from_slice(&[
            widgets.this_agent,
            widgets.tool,
            widgets.results,
            widgets.count,
            widgets.since,
            widgets.list,
        ]);
        ids[6..].copy_from_slice(&widgets.rows);
        ids
    }

    /// Shows or hides every widget of the view; the count stays hidden while nothing matches.
    pub fn set_shown(&self, tree: &mut WidgetTree<CAPACITY>, shown: bool) {
        for id in self.ids() {
            tree.set_shown(id, shown);
        }
        tree.set_shown(self.widgets.count, shown && self.read.matching > 0);
    }

    /// Whether `id` is one of the view's widgets.
    pub fn holds(&self, id: WidgetId) -> bool {
        self.ids().contains(&id)
    }

    /// The pane is shown again: every filter goes back to how the pane opens.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        tree.set_selected(widgets.this_agent, 0);
        tree.set_text(widgets.tool, "");
        tree.set_selected(widgets.results, 0);
        tree.set_selected(widgets.since, ALL_TIME);
        self.show(tree, ActivityRead::default());
    }

    /// What the filters ask for; `agent` is the agents list's selected agent.
    pub fn query(&self, tree: &WidgetTree<CAPACITY>, agent: Option<AgentId>) -> ActivityQuery {
        let widgets = self.widgets;
        let tool = tree.text_of(widgets.tool).unwrap_or("").trim();
        ActivityQuery {
            agent: agent.filter(|_| self.this_agent_only(tree)),
            tool: (!tool.is_empty()).then(|| String::from(tool)),
            outcome: tree
                .selected(widgets.results)
                .and_then(|index| RESULT_OUTCOMES.get(index).copied())
                .flatten(),
            within: tree
                .selected(widgets.since)
                .and_then(|index| SINCE_SECONDS.get(index).copied())
                .flatten(),
        }
    }

    /// Whether `This agent only` is ticked.
    pub fn this_agent_only(&self, tree: &WidgetTree<CAPACITY>) -> bool {
        tree.selected(self.widgets.this_agent) == Some(1)
    }

    /// Shows the rows just read, the newest selected.
    pub fn show(&mut self, tree: &mut WidgetTree<CAPACITY>, read: ActivityRead) {
        let widgets = self.widgets;
        let shown = match read.records.len() {
            count if count >= 32 => u32::MAX,
            count => (1u32 << count) - 1,
        };
        tree.set_shown_items(widgets.list, shown);
        tree.set_selected(widgets.list, 0);
        let mut count = String::new();
        if read.matching > 0 {
            count = alloc::format!("{} of {}", read.records.len(), read.matching);
        }
        tree.set_text(widgets.count, &count);
        let showing = tree.is_shown(widgets.list);
        tree.set_shown(widgets.count, showing && read.matching > 0);
        self.read = read;
        self.retitle(tree);
    }

    /// Writes the visible records' rows into their labels, after a scroll or a new read.
    pub fn retitle(&self, tree: &mut WidgetTree<CAPACITY>) {
        let top = top_row(tree, self.widgets.list);
        for (row, id) in self.widgets.rows.iter().enumerate() {
            let text = self
                .read
                .records
                .get(top + row)
                .map(record_row)
                .unwrap_or_default();
            if tree.text_of(*id) != Some(shortened(&text).as_str()) {
                show_line(tree, *id, &text, colour::TEXT);
            }
        }
    }

    /// The status line this view shows: the selected record's detail line, or why there is
    /// none, and its colour.
    pub fn status_line(&self, tree: &WidgetTree<CAPACITY>) -> (String, u32) {
        if let Some(errno) = self.read.error {
            let reason = core::str::from_utf8(errno.message()).unwrap_or("?");
            return (
                alloc::format!("The log could not be read: {reason}."),
                colour::TEXT,
            );
        }
        let selected = tree
            .selected(self.widgets.list)
            .and_then(|index| self.read.records.get(index));
        match selected {
            Some(record) => (detail_line(record), colour::TEXT),
            None => (String::from(NO_RECORDS), colour::TEXT_DIM),
        }
    }

    /// Acts on `response` when it is the view's: `Some` with what the program must do, if
    /// anything (the status line must then be shown again), `None` when it is not the view's.
    pub fn respond(&mut self, response: Response) -> Option<Option<Request>> {
        let widgets = self.widgets;
        match response {
            Response::Toggled(id, _) if id == widgets.this_agent => {
                Some(Some(Request::ReadActivity))
            }
            Response::Edited(id) if id == widgets.tool => Some(Some(Request::ReadActivity)),
            Response::Chosen(id, _) if id == widgets.results || id == widgets.since => {
                Some(Some(Request::ReadActivity))
            }
            Response::Chosen(id, _) if id == widgets.list => Some(None),
            _ => None,
        }
    }

    /// The visible records' rows.
    pub fn record_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; VISIBLE_RECORDS] {
        self.widgets.rows.map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// The count's text while it shows.
    pub fn count<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        let id = self.widgets.count;
        tree.is_shown(id).then(|| tree.text_of(id)).flatten()
    }

    /// The view's widgets that tests and agents address.
    pub fn widget_ids(&self) -> ActivityWidgets {
        let widgets = self.widgets;
        ActivityWidgets {
            this_agent: widgets.this_agent,
            tool: widgets.tool,
            results: widgets.results,
            count: widgets.count,
            since: widgets.since,
            list: widgets.list,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
