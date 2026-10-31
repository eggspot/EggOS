//! The Task Manager (D200, docs/ux/task-manager-v0.3.md): every process `/proc/ps` lists, by
//! the app name of its window or its own name, with its id, state and share of the CPUs; a
//! filter, `Apps only`, three sorts, the machine's CPUs, uptime and memory, and `End task`,
//! which asks a window's app to close first and ends a process with `SIGTERM`, then `SIGKILL`
//! 3 s later, after the person confirms. It holds only widgets and the last facts; the program
//! reads `/proc` and the window list and sends the signals.

use alloc::vec::Vec;
use core::fmt::Write;
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Escape, Kind, Length, Place};

/// The window's title.
pub const TITLE: &str = "Task Manager";
/// The content's size when the window opens.
pub const DEFAULT_SIZE: (u32, u32) = (640, 440);
/// The smallest content the user may resize it to.
pub const MINIMUM_SIZE: (u32, u32) = (480, 300);
/// The most widgets the window holds.
pub const CAPACITY: usize = 72;
/// How often the facts are read again, in milliseconds.
pub const REFRESH_MS: u32 = 2000;
/// How long a process has after `SIGTERM` before it gets `SIGKILL`.
pub const FORCE_AFTER_MS: u64 = 3000;
/// `SIGTERM`.
pub const TERMINATE: u32 = 15;
/// `SIGKILL`.
pub const KILL: u32 = 9;
/// `SIGCONT`.
pub const CONTINUE: u32 = 18;

const ROWS: usize = 9;
const MAX_ROWS: usize = 32;
const ROW: i32 = 24;
const LIST: (i32, i32) = (16, 76);
const ROW_TEXT_TOP: i32 = 80;
const EMPTY_ROWS: [&str; MAX_ROWS] = [""; MAX_ROWS];
const SORTS: [&str; 3] = ["By memory", "By CPU", "By name"];
const INIT: u32 = 1;

/// A process as `/proc/ps` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessFact {
    pub pid: u32,
    pub parent: u32,
    /// `R`, `T` or `Z`.
    pub state: u8,
    pub cpu_ms: Option<u64>,
    pub name: Text<32>,
}

/// A window as the window list gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowFact {
    pub id: u32,
    pub client_pid: u32,
    pub title: Text<64>,
}

/// What the program read for one refresh.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// `None` when `/proc/ps` could not be read.
    pub processes: Option<Vec<ProcessFact>>,
    /// `None` when the window list could not be read.
    pub windows: Option<Vec<WindowFact>>,
    pub cpus: u32,
    /// Total and free bytes, when `/proc/meminfo` was read.
    pub memory: Option<(u64, u64)>,
    pub uptime_s: Option<u64>,
    /// When it was read, in milliseconds.
    pub now_ms: u64,
}

/// What the program must do for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Ask every window of this process to close ([`TaskManager::windows_of`]).
    CloseWindowsOf(u32),
    /// Send this signal to this process, then say how it went ([`TaskManager::signal_sent`]).
    Signal { pid: u32, signal: u32 },
    /// Read the facts again now.
    Refresh,
}

/// How the rows are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Memory,
    Cpu,
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    pid: u32,
    parent: u32,
    state: u8,
    shown: Text<64>,
    raw: Text<32>,
    has_window: bool,
    cpu_percent: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ending {
    pid: u32,
    name: Text<64>,
    since_ms: u64,
    killed: bool,
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    filter: WidgetId,
    apps_only: WidgetId,
    list: WidgetId,
    cells: [[WidgetId; 5]; ROWS],
    status: WidgetId,
    cpu_line: WidgetId,
    memory_line: WidgetId,
    memory_bar: WidgetId,
    sorts: WidgetId,
    end: WidgetId,
    panel: WidgetId,
    question: WidgetId,
    cancel: WidgetId,
    confirm: WidgetId,
}

/// The Task Manager's widgets and the facts they show.
#[derive(Debug)]
pub struct TaskManager {
    tree: WidgetTree<CAPACITY>,
    widgets: Widgets,
    own_pid: u32,
    snapshot: Snapshot,
    previous: Vec<(u32, u64)>,
    previous_ms: u64,
    rows: Vec<Row>,
    selected: Option<u32>,
    asked: Vec<u32>,
    ending: Option<Ending>,
    status: Option<Text<64>>,
}

fn add(
    tree: &mut WidgetTree<CAPACITY>,
    parent: Option<WidgetId>,
    place: Place,
    kind: Kind,
) -> WidgetId {
    tree.add(parent, place, kind)
        .expect("the Task Manager fits its tree")
}

fn label(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, x: i32, y: i32, ink: u32) -> WidgetId {
    add(
        tree,
        Some(parent),
        Place::at(x, y, Length::Content, Length::Content),
        Kind::label("", ink),
    )
}

fn right_label(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    margin: i32,
    y: i32,
    ink: u32,
) -> WidgetId {
    add(
        tree,
        Some(parent),
        Place::right(margin, y),
        Kind::label("", ink),
    )
}

/// The app name a window title gives: what follows its last ` - `, or all of it.
pub fn app_name(title: &str) -> &str {
    title.rsplit_once(" - ").map_or(title, |(_, app)| app)
}

/// Whole percent of `cpus` CPUs that `spent_ms` of CPU time over `interval_ms` is.
pub fn cpu_percent(spent_ms: u64, interval_ms: u64, cpus: u32) -> u32 {
    let room = interval_ms.saturating_mul(u64::from(cpus.max(1)));
    match room {
        0 => 0,
        room => (spent_ms.saturating_mul(100) / room).min(100) as u32,
    }
}

/// Bytes as Files writes a size: `512 KB`, `12.3 MB`, `1.0 GB`.
pub fn size_text(bytes: u64) -> Text<16> {
    let mut text = Text::new();
    let _ = match bytes {
        bytes if bytes < 1 << 20 => write!(text, "{} KB", bytes >> 10),
        bytes if bytes < 1 << 30 => write!(
            text,
            "{}.{} MB",
            bytes >> 20,
            ((bytes % (1 << 20)) * 10) >> 20
        ),
        bytes => write!(
            text,
            "{}.{} GB",
            bytes >> 30,
            ((bytes % (1 << 30)) * 10) >> 30
        ),
    };
    text
}

fn uptime_words(text: &mut Text<96>, seconds: u64) {
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    let plural = |count: u64| if count == 1 { "" } else { "s" };
    let _ = match (hours, minutes) {
        (0, 0) => write!(text, "{seconds} second{}", plural(seconds)),
        (0, minutes) => write!(text, "{minutes} minute{}", plural(minutes)),
        (hours, minutes) => write!(
            text,
            "{hours} hour{}, {minutes} minute{}",
            plural(hours),
            plural(minutes)
        ),
    };
}

impl TaskManager {
    /// The window for process `own_pid` (itself), `apps_only` ticked when started with `--apps`.
    pub fn new(own_pid: u32, apps_only: bool) -> Self {
        let mut tree = WidgetTree::new();
        let root = add(
            &mut tree,
            None,
            Place::fill(),
            Kind::panel(colour::WINDOW_BACKGROUND),
        );
        let filter = add(
            &mut tree,
            Some(root),
            Place::at(16, 16, Length::Px(208), Length::Px(28)),
            Kind::text_field("Filter processes", Escape::Clear),
        );
        let apps_only_box = add(
            &mut tree,
            Some(root),
            Place::at(240, 16, Length::Content, Length::Px(28)),
            Kind::Checkbox {
                text: Text::from("Apps only"),
                checked: apps_only,
            },
        );
        for (x, words, right) in [
            (40, "Name", false),
            (264, "PID", true),
            (392, "State", false),
            (88, "Memory", true),
            (24, "CPU", true),
        ] {
            let header = match right {
                true => right_label(&mut tree, root, x, 56, colour::TEXT_DIM),
                false => label(&mut tree, root, x, 56, colour::TEXT_DIM),
            };
            tree.set_text(header, words);
        }
        let list = add(
            &mut tree,
            Some(root),
            Place::at(
                LIST.0,
                LIST.1,
                Length::Fill(16),
                Length::Px(ROW * ROWS as i32),
            ),
            Kind::list(&EMPTY_ROWS, ROW, 8, 24),
        );
        let cells = core::array::from_fn(|row| {
            let y = ROW_TEXT_TOP + row as i32 * ROW;
            [
                label(&mut tree, root, 40, y, colour::TEXT),
                right_label(&mut tree, root, 264, y, colour::TEXT),
                label(&mut tree, root, 392, y, colour::TEXT),
                right_label(&mut tree, root, 88, y, colour::TEXT),
                right_label(&mut tree, root, 24, y, colour::TEXT),
            ]
        });
        let status = label(&mut tree, root, 16, 304, colour::TEXT_DIM);
        let cpu_line = label(&mut tree, root, 16, 328, colour::TEXT);
        let memory_line = label(&mut tree, root, 16, 348, colour::TEXT);
        let memory_bar = add(
            &mut tree,
            Some(root),
            Place::at(16, 372, Length::Px(288), Length::Px(8)),
            Kind::Progress { permille: 0 },
        );
        let sorts = add(
            &mut tree,
            Some(root),
            Place::at(16, 396, Length::Content, Length::Px(28)),
            Kind::radios(&SORTS, 0, true),
        );
        let end = add(
            &mut tree,
            Some(root),
            Place::at(528, 396, Length::Px(96), Length::Px(28)),
            Kind::button("End task", false),
        );
        let panel = add(
            &mut tree,
            Some(root),
            Place::at(120, 164, Length::Px(400), Length::Px(112)),
            Kind::panel(colour::MENU),
        );
        let question = label(&mut tree, panel, 16, 16, colour::TEXT);
        let warning = label(&mut tree, panel, 16, 36, colour::TEXT_DIM);
        tree.set_text(warning, "Unsaved work in it is lost.");
        let cancel = add(
            &mut tree,
            Some(panel),
            Place::at(184, 68, Length::Px(96), Length::Px(28)),
            Kind::button("Cancel", false),
        );
        let confirm = add(
            &mut tree,
            Some(panel),
            Place::at(288, 68, Length::Px(96), Length::Px(28)),
            Kind::button("End", true),
        );
        tree.set_shown(panel, false);
        tree.set_focus(Some(list), 0);
        let (width, height) = DEFAULT_SIZE;
        tree.resize(width as i32, height as i32);
        Self {
            tree,
            widgets: Widgets {
                filter,
                apps_only: apps_only_box,
                list,
                cells,
                status,
                cpu_line,
                memory_line,
                memory_bar,
                sorts,
                end,
                panel,
                question,
                cancel,
                confirm,
            },
            own_pid,
            snapshot: Snapshot::default(),
            previous: Vec::new(),
            previous_ms: 0,
            rows: Vec::new(),
            selected: None,
            asked: Vec::new(),
            ending: None,
            status: None,
        }
    }

    /// The window's widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The window's widgets, to draw them.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    fn sort(&self) -> Sort {
        match self.tree.selected(self.widgets.sorts) {
            Some(1) => Sort::Cpu,
            Some(2) => Sort::Name,
            _ => Sort::Memory,
        }
    }

    fn apps_only(&self) -> bool {
        self.tree.selected(self.widgets.apps_only) == Some(1)
    }

    /// The windows of process `pid`, newest first.
    pub fn windows_of(&self, pid: u32) -> Vec<u32> {
        let mut windows: Vec<u32> = self
            .snapshot
            .windows
            .iter()
            .flatten()
            .filter(|window| window.client_pid == pid)
            .map(|window| window.id)
            .collect();
        windows.sort_unstable_by(|a, b| b.cmp(a));
        windows
    }

    /// Shows `snapshot`, the facts just read; a signal to send when a process asked to end
    /// has not after [`FORCE_AFTER_MS`].
    pub fn refreshed(&mut self, snapshot: Snapshot) -> Option<Request> {
        let interval = snapshot.now_ms.saturating_sub(self.previous_ms);
        let mut rows = Vec::new();
        if let Some(processes) = &snapshot.processes {
            for process in processes {
                let window = snapshot
                    .windows
                    .iter()
                    .flatten()
                    .filter(|window| window.client_pid == process.pid)
                    .max_by_key(|window| window.id);
                let shown = match window {
                    Some(window) => Text::from(app_name(window.title.as_str())),
                    None => Text::from(process.name.as_str()),
                };
                let before = self
                    .previous
                    .iter()
                    .find(|(pid, _)| *pid == process.pid)
                    .map(|(_, spent)| *spent);
                let cpu_percent = match (process.cpu_ms, before) {
                    (Some(now), Some(before)) if interval > 0 => Some(cpu_percent(
                        now.saturating_sub(before),
                        interval,
                        snapshot.cpus,
                    )),
                    _ => None,
                };
                rows.push(Row {
                    pid: process.pid,
                    parent: process.parent,
                    state: process.state,
                    shown,
                    raw: process.name,
                    has_window: window.is_some(),
                    cpu_percent,
                });
            }
        }
        self.previous = snapshot
            .processes
            .iter()
            .flatten()
            .filter_map(|process| Some((process.pid, process.cpu_ms?)))
            .collect();
        self.previous_ms = snapshot.now_ms;
        self.asked
            .retain(|pid| rows.iter().any(|row| row.pid == *pid));
        let forced = self.check_ending(&rows, snapshot.now_ms);
        self.rows = rows;
        self.snapshot = snapshot;
        self.show_rows();
        self.show_summary();
        forced
    }

    fn check_ending(&mut self, rows: &[Row], now_ms: u64) -> Option<Request> {
        let ending = self.ending?;
        let listed = rows
            .iter()
            .any(|row| row.pid == ending.pid && row.state != b'Z');
        let mut line = Text::<64>::new();
        if !listed {
            let _ = match ending.killed {
                false => write!(line, "{} ended.", ending.name.as_str()),
                true => write!(
                    line,
                    "{} did not stop; EggOS forced it to end.",
                    ending.name.as_str()
                ),
            };
            self.ending = None;
            self.status = Some(line);
            return None;
        }
        if ending.killed {
            if now_ms.saturating_sub(ending.since_ms) >= FORCE_AFTER_MS + 1000 {
                let _ = write!(line, "{} could not be ended.", ending.name.as_str());
                self.status = Some(line);
                self.ending = None;
            }
            return None;
        }
        if now_ms.saturating_sub(ending.since_ms) < FORCE_AFTER_MS {
            return None;
        }
        self.ending = Some(Ending {
            killed: true,
            since_ms: now_ms,
            ..ending
        });
        Some(Request::Signal {
            pid: ending.pid,
            signal: KILL,
        })
    }

    fn visible_rows(&self) -> Vec<Row> {
        let query = Text::<255>::from(self.tree.text_of(self.widgets.filter).unwrap_or(""));
        let query = query.as_str().trim();
        let apps_only = self.apps_only();
        let mut digits = Text::<12>::new();
        let mut rows: Vec<Row> = self
            .rows
            .iter()
            .filter(|row| !apps_only || row.has_window)
            .filter(|row| {
                digits = Text::new();
                let _ = write!(digits, "{}", row.pid);
                query.is_empty()
                    || contains_ignoring_case(row.shown.as_str(), query)
                    || contains_ignoring_case(row.raw.as_str(), query)
                    || digits.as_str().contains(query)
            })
            .copied()
            .collect();
        match self.sort() {
            Sort::Cpu => rows.sort_by(|a, b| {
                b.cpu_percent
                    .map_or(-1, i64::from)
                    .cmp(&a.cpu_percent.map_or(-1, i64::from))
                    .then(a.pid.cmp(&b.pid))
            }),
            Sort::Name => rows.sort_by(|a, b| {
                lowercase_order(a.shown.as_str(), b.shown.as_str()).then(a.pid.cmp(&b.pid))
            }),
            Sort::Memory => rows.sort_by_key(|row| row.pid),
        }
        rows
    }

    fn show_rows(&mut self) {
        let rows = self.visible_rows();
        let count = rows.len().min(MAX_ROWS);
        let shown = match count {
            MAX_ROWS => u32::MAX,
            count => (1u32 << count) - 1,
        };
        self.tree.set_shown_items(self.widgets.list, shown);
        let index = self
            .selected
            .and_then(|pid| rows[..count].iter().position(|row| row.pid == pid))
            .or_else(|| (count > 0).then_some(0));
        if let Some(index) = index {
            self.tree.set_selected(self.widgets.list, index);
        }
        self.selected = index.and_then(|index| rows.get(index)).map(|row| row.pid);
        let top = top_row(&self.tree, self.widgets.list);
        for (slot, cells) in self.widgets.cells.iter().enumerate() {
            let texts = rows
                .get(top + slot)
                .filter(|_| top + slot < count)
                .map(row_texts);
            for (column, id) in cells.iter().enumerate() {
                let text = texts.as_ref().map_or("", |texts| texts[column].as_str());
                if self.tree.text_of(*id) != Some(text) {
                    self.tree.set_text(*id, text);
                }
            }
        }
        let empty = match (self.snapshot.processes.is_none(), self.apps_only()) {
            (true, _) => "The process list could not be read.",
            (false, true) if count == 0 => "No apps are open.",
            _ => "No processes match.",
        };
        self.tree.change(self.widgets.list, |kind| {
            if let Kind::List(list) = kind {
                list.empty_text = empty;
            }
        });
        if rows.len() > MAX_ROWS && self.status.is_none() {
            let mut line = Text::<64>::new();
            let _ = write!(line, "Showing the first 32 of {} processes.", rows.len());
            self.tree.set_text(self.widgets.status, line.as_str());
        }
        self.show_selection();
    }

    fn selected_row(&self) -> Option<Row> {
        let pid = self.selected?;
        self.rows.iter().find(|row| row.pid == pid).copied()
    }

    fn show_selection(&mut self) {
        let row = self.selected_row();
        let force = row.is_some_and(|row| self.asked.contains(&row.pid));
        self.tree.set_text(
            self.widgets.end,
            if force { "Force end" } else { "End task" },
        );
        let mut line = Text::<64>::new();
        let reason = match row {
            None => None,
            Some(row) if row.pid == INIT => {
                Some("init runs EggOS: use Power off to stop everything.")
            }
            Some(row) if row.pid == self.own_pid => {
                Some("To close the Task Manager, close its window.")
            }
            Some(row) if row.state == b'Z' => {
                let _ = write!(
                    line,
                    "{} has ended; its parent has not collected it yet.",
                    row.raw.as_str()
                );
                Some("")
            }
            Some(_) => None,
        };
        self.tree
            .set_enabled(self.widgets.end, row.is_some() && reason.is_none());
        let words = match (&self.status, reason) {
            (Some(status), _) => status.as_str(),
            (None, Some("")) => line.as_str(),
            (None, Some(reason)) => reason,
            (None, None) => {
                if let Some(row) = row {
                    let parent = self
                        .rows
                        .iter()
                        .find(|parent| parent.pid == row.parent)
                        .map_or("", |parent| parent.raw.as_str());
                    let _ = write!(
                        line,
                        "{} \u{b7} pid {} \u{b7} parent {} ({parent})",
                        row.raw.as_str(),
                        row.pid,
                        row.parent
                    );
                }
                line.as_str()
            }
        };
        let words = Text::<64>::from(words);
        self.tree.set_text(self.widgets.status, words.as_str());
    }

    fn show_summary(&mut self) {
        let mut cpus = Text::<96>::new();
        let snapshot = &self.snapshot;
        let _ = write!(
            cpus,
            "{} CPU{}",
            snapshot.cpus,
            if snapshot.cpus == 1 { "" } else { "s" }
        );
        match snapshot.uptime_s {
            Some(seconds) => {
                let _ = cpus.write_str(" \u{b7} up ");
                uptime_words(&mut cpus, seconds);
            }
            None => {
                cpus = Text::from("CPUs: not reported");
            }
        }
        self.tree.set_text(self.widgets.cpu_line, cpus.as_str());
        let mut memory = Text::<96>::new();
        let permille = match snapshot.memory {
            Some((total, free)) => {
                let used = total.saturating_sub(free);
                let _ = write!(
                    memory,
                    "Memory {} of {} used",
                    size_text(used).as_str(),
                    size_text(total).as_str()
                );
                (used.saturating_mul(1000) / total.max(1)) as u16
            }
            None => {
                let _ = memory.write_str("Memory: not reported");
                0
            }
        };
        self.tree
            .set_text(self.widgets.memory_line, memory.as_str());
        self.tree.set_progress(self.widgets.memory_bar, permille);
        self.tree
            .set_shown(self.widgets.memory_bar, snapshot.memory.is_some());
    }

    fn press_end(&mut self) -> Option<Request> {
        let row = self.selected_row()?;
        if !self.tree.is_enabled(self.widgets.end) {
            return None;
        }
        self.status = None;
        if row.has_window && !self.asked.contains(&row.pid) {
            self.asked.push(row.pid);
            let mut line = Text::<64>::new();
            let _ = write!(
                line,
                "Asked {} to close; Force end ends it without saving.",
                row.shown.as_str()
            );
            self.status = Some(line);
            self.show_selection();
            return Some(Request::CloseWindowsOf(row.pid));
        }
        let mut question = Text::<64>::new();
        let _ = write!(question, "End {}?", row.shown.as_str());
        self.tree.set_text(self.widgets.question, question.as_str());
        self.tree.set_shown(self.widgets.panel, true);
        self.tree.set_default_button(self.widgets.confirm);
        self.tree.set_focus(Some(self.widgets.cancel), 0);
        None
    }

    fn close_panel(&mut self) {
        self.tree.set_shown(self.widgets.panel, false);
        self.tree.set_focus(Some(self.widgets.list), 0);
    }

    fn confirmed(&mut self, now_ms: u64) -> Option<Request> {
        self.close_panel();
        let row = self.selected_row()?;
        self.ending = Some(Ending {
            pid: row.pid,
            name: row.shown,
            since_ms: now_ms,
            killed: false,
        });
        let mut line = Text::<64>::new();
        let _ = write!(line, "Ending {}...", row.shown.as_str());
        self.status = Some(line);
        self.show_selection();
        Some(Request::Signal {
            pid: row.pid,
            signal: TERMINATE,
        })
    }

    /// Whether process `pid` is stopped, so `SIGCONT` must follow `SIGTERM`.
    pub fn is_stopped(&self, pid: u32) -> bool {
        self.rows
            .iter()
            .any(|row| row.pid == pid && row.state == b'T')
    }

    /// How sending `signal` to `pid` went.
    pub fn signal_sent(&mut self, pid: u32, result: Result<(), eggos_runtime::errno::Errno>) {
        let name = self
            .rows
            .iter()
            .find(|row| row.pid == pid)
            .map_or(Text::<64>::new(), |row| row.shown);
        let mut line = Text::<64>::new();
        let _ = match result {
            Ok(()) => return,
            Err(eggos_runtime::errno::Errno::ESRCH) => {
                write!(line, "{} has already ended.", name.as_str())
            }
            Err(_) => write!(
                line,
                "You may not end {}: it belongs to another user.",
                name.as_str()
            ),
        };
        self.ending = None;
        self.status = Some(line);
        self.show_selection();
    }

    /// The texts of the visible rows, a row's five cells each.
    pub fn row_lines(&self) -> Vec<[&str; 5]> {
        self.widgets
            .cells
            .iter()
            .map(|cells| cells.map(|id| self.tree.text_of(id).unwrap_or("")))
            .take_while(|cells| !cells[0].is_empty())
            .collect()
    }

    /// The status line, the CPU line and the memory line.
    pub fn lines(&self) -> [&str; 3] {
        [
            self.widgets.status,
            self.widgets.cpu_line,
            self.widgets.memory_line,
        ]
        .map(|id| self.tree.text_of(id).unwrap_or(""))
    }

    /// The `End task` button's text and whether it is enabled.
    pub fn end_button(&self) -> (&str, bool) {
        (
            self.tree.text_of(self.widgets.end).unwrap_or(""),
            self.tree.is_enabled(self.widgets.end),
        )
    }

    /// Acts on what an input did at `now_ms`: what the program must do, if anything.
    pub fn respond(&mut self, response: Response, now_ms: u64) -> Option<Request> {
        let widgets = self.widgets;
        match response {
            Response::Edited(id) if id == widgets.filter => {
                self.show_rows();
                None
            }
            Response::Toggled(id, _) if id == widgets.apps_only => {
                self.show_rows();
                None
            }
            Response::Chosen(id, _) if id == widgets.sorts => {
                self.show_rows();
                None
            }
            Response::Chosen(id, index) if id == widgets.list => {
                let rows = self.visible_rows();
                self.selected = rows.get(index).map(|row| row.pid);
                self.status = None;
                self.show_selection();
                None
            }
            Response::Activated(id) if id == widgets.end => self.press_end(),
            Response::Activated(id) if id == widgets.cancel => {
                self.close_panel();
                None
            }
            Response::Activated(id) if id == widgets.confirm => self.confirmed(now_ms),
            Response::Unused {
                code: key::DELETE, ..
            } if self.tree.focus() == Some(widgets.list) => self.press_end(),
            Response::Unused { code: key::ESC, .. } if self.tree.is_shown(widgets.panel) => {
                self.close_panel();
                None
            }
            Response::Unused { code: F5, .. } => Some(Request::Refresh),
            Response::Nothing => {
                self.show_rows();
                None
            }
            _ => None,
        }
    }
}

const F5: u16 = 63;

fn top_row(tree: &WidgetTree<CAPACITY>, list: WidgetId) -> usize {
    match tree.kind(list) {
        Some(Kind::List(list)) if list.row_height > 0 => (list.offset / list.row_height) as usize,
        _ => 0,
    }
}

fn row_texts(row: &Row) -> [Text<64>; 5] {
    let mut pid = Text::new();
    let _ = write!(pid, "{}", row.pid);
    let state = match (row.state, row.has_window) {
        (b'Z', _) => "Ended",
        (b'T', _) => "Stopped",
        _ => "Running",
    };
    let mut cpu = Text::new();
    let _ = match row.cpu_percent {
        Some(percent) => write!(cpu, "{percent} %"),
        None => cpu.write_str("-"),
    };
    [row.shown, pid, Text::from(state), Text::from("-"), cpu]
}

fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn lowercase_order(a: &str, b: &str) -> core::cmp::Ordering {
    a.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_runtime::errno::Errno;

    fn process(pid: u32, parent: u32, state: u8, cpu_ms: u64, name: &str) -> ProcessFact {
        ProcessFact {
            pid,
            parent,
            state,
            cpu_ms: Some(cpu_ms),
            name: Text::from(name),
        }
    }

    fn snapshot(now_ms: u64, editor_cpu: u64) -> Snapshot {
        Snapshot {
            processes: Some(alloc::vec![
                process(1, 0, b'R', 50, "init"),
                process(7, 1, b'R', 30, "eggsh"),
                process(42, 7, b'R', editor_cpu, "text-editor"),
                process(50, 7, b'R', 10, "task-manager"),
            ]),
            windows: Some(alloc::vec![WindowFact {
                id: 129,
                client_pid: 42,
                title: Text::from("notes.txt - Text Editor"),
            }]),
            cpus: 4,
            memory: Some((1 << 30, 700 << 20)),
            uptime_s: Some(3725),
            now_ms,
        }
    }

    #[test]
    fn rows_show_app_names_ids_states_and_cpu_shares() {
        let mut manager = TaskManager::new(50, false);
        manager.refreshed(snapshot(1000, 100));
        assert_eq!(
            manager.row_lines()[2],
            ["Text Editor", "42", "Running", "-", "-"]
        );
        manager.refreshed(snapshot(3000, 1700));
        assert_eq!(
            manager.row_lines()[2],
            ["Text Editor", "42", "Running", "-", "20 %"]
        );
        assert_eq!(
            manager.lines()[1..],
            [
                "4 CPUs \u{b7} up 1 hour, 2 minutes",
                "Memory 324.0 MB of 1.0 GB used"
            ]
        );
        assert_eq!(cpu_percent(8000, 2000, 4), 100);
        assert_eq!(app_name("Untitled - Notepad"), "Notepad");
    }

    #[test]
    fn the_filter_and_apps_only_narrow_the_rows() {
        let mut manager = TaskManager::new(50, true);
        manager.refreshed(snapshot(1000, 100));
        assert_eq!(manager.row_lines().len(), 1);
        let mut manager = TaskManager::new(50, false);
        manager.refreshed(snapshot(1000, 100));
        let filter = manager.widgets.filter;
        manager.tree.set_text(filter, "SH");
        manager.respond(Response::Edited(filter), 1000);
        assert_eq!(manager.row_lines().len(), 1);
        assert_eq!(manager.row_lines()[0][0], "eggsh");
    }

    #[test]
    fn end_task_asks_a_window_first_then_ends_after_a_yes_and_forces_three_seconds_later() {
        let mut manager = TaskManager::new(50, false);
        manager.refreshed(snapshot(1000, 100));
        let list = manager.widgets.list;
        manager.respond(Response::Chosen(list, 2), 1000);
        let end = manager.widgets.end;
        assert_eq!(
            manager.respond(Response::Activated(end), 1000),
            Some(Request::CloseWindowsOf(42))
        );
        assert_eq!(manager.end_button(), ("Force end", true));
        assert_eq!(manager.windows_of(42), [129]);
        assert_eq!(manager.respond(Response::Activated(end), 1000), None);
        let confirm = manager.widgets.confirm;
        assert_eq!(
            manager.respond(Response::Activated(confirm), 1500),
            Some(Request::Signal {
                pid: 42,
                signal: TERMINATE
            })
        );
        assert_eq!(manager.refreshed(snapshot(3000, 200)), None);
        assert_eq!(
            manager.refreshed(snapshot(4600, 200)),
            Some(Request::Signal {
                pid: 42,
                signal: KILL
            })
        );
        let mut gone = snapshot(6000, 200);
        if let Some(processes) = gone.processes.as_mut() {
            processes.retain(|process| process.pid != 42);
        }
        manager.refreshed(gone);
        assert_eq!(
            manager.lines()[0],
            "Text Editor did not stop; EggOS forced it to end."
        );
    }

    #[test]
    fn init_and_itself_cannot_be_ended_and_a_refused_signal_says_why() {
        let mut manager = TaskManager::new(50, false);
        manager.refreshed(snapshot(1000, 100));
        assert_eq!(manager.end_button(), ("End task", false));
        assert_eq!(
            manager.lines()[0],
            "init runs EggOS: use Power off to stop everything."
        );
        let list = manager.widgets.list;
        manager.respond(Response::Chosen(list, 3), 1000);
        assert!(!manager.end_button().1);
        manager.respond(Response::Chosen(list, 1), 1000);
        manager.signal_sent(7, Err(Errno::ESRCH));
        assert_eq!(manager.lines()[0], "eggsh has already ended.");
    }
}
