//! Settings' Components pane (D235, docs/ux/components-v0.3.md): the components the attached
//! store offers and the machine has installed, each with `Install` or `Remove`, in the words
//! and with the rules `component` and the MCP tools `component_*` use (`eggos_components`). It
//! holds only widgets and what was last read; the program reads the store and the installed
//! records, checks a remove, installs and removes when a [`Request`] asks it to, one action at
//! a time. The note under the acting row (a message, the progress bar or a confirmation) is one
//! set of widgets placed under whichever row acts.

use crate::settings_window::{CAPACITY, Request};
use crate::storage_pane::shortened;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_components::command::Row;
use eggos_components::index::{Component, MAX_COMPONENTS};
use eggos_components::manager::{self, Failure, Refusal, Store};
use eggos_components::messages;
use eggos_toolkit::input::key;
use eggos_toolkit::text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

/// The most rows the pane shows: the most components a store offers.
pub const ROWS: usize = MAX_COMPONENTS;
/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 4 + 5 * ROWS + 5;
/// Row `r`'s top in the pane, with no note open above it: `ROWS_TOP + ROW_HEIGHT * r`.
pub const ROWS_TOP: i32 = 124;
/// A row's height.
pub const ROW_HEIGHT: i32 = 48;
/// The height of a one-line message note.
pub const MESSAGE_HEIGHT: i32 = 24;
/// The height of a two-line message note.
pub const TWO_LINE_HEIGHT: i32 = 44;
/// The height of the progress note.
pub const PROGRESS_HEIGHT: i32 = 40;
/// The height of a confirmation note.
pub const CONFIRMATION_HEIGHT: i32 = 84;

const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const STORE_LINES: [i32; 3] = [56, 76, 96];
const LINE_WIDTH: i32 = 432;
const ROW_WIDTH: i32 = 432;
const ROW_TEXT_X: i32 = 24;
const TITLE_TOP: i32 = 6;
const SUMMARY_TOP: i32 = 26;
const STATE_END: i32 = 320;
const STATE_GAP: i32 = 16;
const ROW_BUTTON: (i32, i32, i32, i32) = (336, 10, 96, 28);
const NOTE_X: i32 = 40;
const NOTE_WIDTH: i32 = 408;
const FIRST_LINE: i32 = 4;
const SECOND_LINE: i32 = 24;
const BAR_TOP: i32 = 8;
const BAR_HEIGHT: i32 = 8;
const BYTES_LINE: i32 = 24;
const NOTE_BUTTONS_TOP: i32 = 48;
const NOTE_CANCEL: (i32, i32) = (216, 96);
const NOTE_CONFIRM: (i32, i32) = (320, 128);
const BUTTON_HEIGHT: i32 = 28;
const BOTTOM_MARGIN: i32 = 16;
const PERMILLE: u64 = 1000;
const INSTALLED: &str = "installed";
const INSTALLING: &str = "installing";
const REMOVING: &str = "removing";
const NOT_INSTALLED: &str = "not-installed";

/// What the program read for the pane: the store, if one is attached, the rows `component
/// list` prints, and the installed component this window's program runs from, if any (the
/// desktop's, D235).
#[derive(Debug, Clone, Default)]
pub struct ComponentsRead {
    pub store: Option<Store>,
    pub rows: Vec<Row>,
    pub runs_this_window: Option<String>,
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
struct RowWidgets {
    panel: WidgetId,
    title: WidgetId,
    state: WidgetId,
    summary: WidgetId,
    button: WidgetId,
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    pane: WidgetId,
    store: [WidgetId; 3],
    rows: [RowWidgets; ROWS],
    first: WidgetId,
    second: WidgetId,
    bar: WidgetId,
    cancel: WidgetId,
    confirm: WidgetId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Note {
    Closed,
    Message {
        name: String,
        first: String,
        second: String,
    },
    Progress {
        name: String,
    },
    Remove {
        name: String,
    },
    Install {
        name: String,
        plan: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Work {
    Idle,
    Checking(String),
    Installing {
        plan: Vec<String>,
        received: u64,
        total: u64,
        cancelled: bool,
    },
    Removing(String),
}

struct Look {
    state: String,
    word: String,
    button: &'static str,
    action: &'static str,
    enabled: bool,
}

/// The Components pane's widgets and what it last read.
#[derive(Debug)]
pub struct ComponentsPane {
    widgets: Widgets,
    read: ComponentsRead,
    note: Note,
    work: Work,
    failed: Option<String>,
    min_height: i32,
    default_button: WidgetId,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn line_at(x: i32, y: i32, width: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Content)
}

fn show_line(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, ink: Token) {
    tree.set_text(id, shortened(words).as_str());
    let inked = matches!(tree.kind(id), Some(Kind::Label { colour, .. }) if *colour == ink);
    if !inked {
        tree.change(id, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
    }
}

fn label(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place) -> WidgetId {
    add(tree, parent, place, Kind::label("", Token::Text))
}

impl ComponentsPane {
    /// Builds the pane in `pane`, a container of the window's at least `min_height` tall;
    /// `default_button` is the window's default button, given back when a confirmation closes.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        min_height: i32,
        default_button: WidgetId,
    ) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading("Components"),
        );
        let store = STORE_LINES.map(|y| label(tree, pane, line_at(PADDING, y, LINE_WIDTH)));
        let rows = core::array::from_fn(|row| {
            let panel = add(
                tree,
                pane,
                at(
                    PADDING,
                    ROWS_TOP + ROW_HEIGHT * row as i32,
                    ROW_WIDTH,
                    ROW_HEIGHT,
                ),
                Kind::panel(Token::Menu),
            );
            let title = label(tree, panel, line_at(ROW_TEXT_X, TITLE_TOP, 0));
            let state = label(tree, panel, Place::right(ROW_WIDTH - STATE_END, TITLE_TOP));
            let summary = add(
                tree,
                panel,
                line_at(ROW_TEXT_X, SUMMARY_TOP, STATE_END - ROW_TEXT_X),
                Kind::label("", Token::TextDim),
            );
            let button = add(
                tree,
                panel,
                at(ROW_BUTTON.0, ROW_BUTTON.1, ROW_BUTTON.2, ROW_BUTTON.3),
                Kind::button("Install", false),
            );
            RowWidgets {
                panel,
                title,
                state,
                summary,
                button,
            }
        });
        let first = label(tree, pane, line_at(NOTE_X, 0, NOTE_WIDTH));
        let second = add(
            tree,
            pane,
            line_at(NOTE_X, 0, NOTE_WIDTH),
            Kind::label("", Token::TextDim),
        );
        let bar = add(
            tree,
            pane,
            at(NOTE_X, 0, NOTE_WIDTH, BAR_HEIGHT),
            Kind::Progress { permille: 0 },
        );
        let cancel = add(
            tree,
            pane,
            at(NOTE_CANCEL.0, 0, NOTE_CANCEL.1, BUTTON_HEIGHT),
            Kind::button("Cancel", false),
        );
        let confirm = add(
            tree,
            pane,
            at(NOTE_CONFIRM.0, 0, NOTE_CONFIRM.1, BUTTON_HEIGHT),
            Kind::button("Remove", true),
        );
        let mut components = Self {
            widgets: Widgets {
                pane,
                store,
                rows,
                first,
                second,
                bar,
                cancel,
                confirm,
            },
            read: ComponentsRead::default(),
            note: Note::Closed,
            work: Work::Idle,
            failed: None,
            min_height,
            default_button,
        };
        components.refresh(tree);
        components
    }

    /// The pane is shown again: messages and a `Failed` state are forgotten, an open
    /// confirmation closes; the program reads the store and the records next.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if self.confirming() {
            tree.set_default_button(self.default_button);
        }
        if !matches!(self.note, Note::Progress { .. }) {
            self.note = Note::Closed;
        }
        self.failed = None;
        self.refresh(tree);
    }

    /// Shows what the program just read.
    pub fn show_components(&mut self, tree: &mut WidgetTree<CAPACITY>, read: ComponentsRead) {
        self.read = read;
        self.refresh(tree);
    }

    /// Shows how checking the remove the pane asked for went: a refusal under the row, or the
    /// confirmation with the focus on its `Remove`, at `now_ms`.
    pub fn show_remove_check(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        checked: Result<Option<Component>, Refusal>,
        now_ms: u32,
    ) {
        let Work::Checking(name) = core::mem::replace(&mut self.work, Work::Idle) else {
            return;
        };
        match checked {
            Ok(Some(component)) => {
                show_line(
                    tree,
                    self.widgets.first,
                    &messages::remove_question(&component),
                    Token::Text,
                );
                show_line(
                    tree,
                    self.widgets.second,
                    &component.on_remove,
                    Token::TextDim,
                );
                self.open_confirmation(tree, Note::Remove { name }, "Remove", now_ms);
            }
            Ok(None) => {
                let title = self.title_of(&name);
                self.note = Note::Message {
                    name,
                    first: messages::not_installed(&title),
                    second: String::new(),
                };
                self.refresh(tree);
            }
            Err(refusal) => {
                let (first, _) = messages::refusal(&refusal, "");
                self.note = Note::Message {
                    name,
                    first,
                    second: String::new(),
                };
                self.refresh(tree);
            }
        }
    }

    /// Shows the install's progress: `received` of `total` bytes copied.
    pub fn show_install_progress(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        received: u64,
        total: u64,
    ) {
        if let Work::Installing {
            received: shown_received,
            total: shown_total,
            ..
        } = &mut self.work
        {
            *shown_received = received.min(total);
            *shown_total = total;
            self.refresh(tree);
        }
    }

    /// Shows how installing the component [`Self::to_install`] named went, with what was read
    /// after: the next of a plan is asked for when this one is done.
    pub fn show_installed(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        result: Result<(), Failure>,
        read: ComponentsRead,
    ) -> Option<Request> {
        let Work::Installing {
            mut plan,
            cancelled,
            ..
        } = core::mem::replace(&mut self.work, Work::Idle)
        else {
            self.show_components(tree, read);
            return None;
        };
        self.read = read;
        if plan.is_empty() {
            self.refresh(tree);
            return None;
        }
        let name = plan.remove(0);
        match result {
            Ok(()) if !plan.is_empty() && !cancelled => return self.start_install(tree, plan),
            Ok(()) => self.note = Note::Closed,
            Err(failure) => {
                let component = self.component(&name);
                let (first, second) = match &component {
                    Some(component) => {
                        let (first, second, _) = messages::install_failure(&failure, component);
                        (first, String::from(second))
                    }
                    None => (
                        String::from(messages::CANCELLED),
                        String::from(messages::NOTHING_INSTALLED),
                    ),
                };
                if failure != Failure::Cancelled {
                    self.failed = Some(name.clone());
                }
                self.note = Note::Message {
                    name,
                    first,
                    second,
                };
            }
        }
        self.refresh(tree);
        None
    }

    /// Shows how removing the component [`Self::to_remove`] named went, with what was read
    /// after.
    pub fn show_removed(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        result: Result<(), Failure>,
        read: ComponentsRead,
    ) {
        let removing = match core::mem::replace(&mut self.work, Work::Idle) {
            Work::Removing(name) => Some(name),
            _ => None,
        };
        let component = removing.as_deref().and_then(|name| self.component(name));
        self.read = read;
        self.note = match (result, removing, component) {
            (Err(failure), Some(name), Some(component)) => {
                let (first, second) = messages::remove_failure(&failure, &component);
                Note::Message {
                    name,
                    first,
                    second: String::from(second),
                }
            }
            _ => Note::Closed,
        };
        self.refresh(tree);
    }

    /// The component the program must check for a remove, while the pane waits for it.
    pub fn to_check(&self) -> Option<&str> {
        match &self.work {
            Work::Checking(name) => Some(name),
            _ => None,
        }
    }

    /// The component the program must install now.
    pub fn to_install(&self) -> Option<&str> {
        match &self.work {
            Work::Installing { plan, .. } => plan.first().map(String::as_str),
            _ => None,
        }
    }

    /// The component the program must remove now.
    pub fn to_remove(&self) -> Option<&str> {
        match &self.work {
            Work::Removing(name) => Some(name),
            _ => None,
        }
    }

    /// Whether the person cancelled the install in progress.
    pub fn cancelled(&self) -> bool {
        matches!(
            self.work,
            Work::Installing {
                cancelled: true,
                ..
            }
        )
    }

    /// Whether the pane waits for an install or a remove, so the program reads the rows once a
    /// second.
    pub fn busy(&self) -> bool {
        !matches!(self.work, Work::Idle)
    }

    /// Whether a confirmation shows under a row.
    pub fn confirming(&self) -> bool {
        matches!(self.note, Note::Remove { .. } | Note::Install { .. })
    }

    /// Acts on what an input did at `now_ms` when it was for the pane.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
    ) -> Reply {
        let widgets = self.widgets;
        let row_of = |id: WidgetId| widgets.rows.iter().position(|row| row.button == id);
        let request = match response {
            Response::Activated(id) if row_of(id).is_some() => {
                self.press_row(tree, row_of(id).unwrap_or(0), now_ms)
            }
            Response::Activated(id) if id == widgets.cancel && self.confirming() => {
                self.close_confirmation(tree, now_ms);
                None
            }
            Response::Activated(id) if id == widgets.confirm && self.confirming() => {
                self.confirm(tree)
            }
            Response::Unused { code: key::ESC, .. } if self.confirming() => {
                self.close_confirmation(tree, now_ms);
                None
            }
            Response::Unused { code, .. }
                if matches!(code, key::UP | key::DOWN | key::HOME | key::END)
                    && tree.focus().and_then(row_of).is_some() =>
            {
                let here = tree.focus().and_then(row_of).unwrap_or(0);
                self.move_focus(tree, here, code, now_ms);
                None
            }
            Response::Unused {
                code: key::DELETE, ..
            } if tree.focus().and_then(row_of).is_some_and(|row| {
                self.read
                    .rows
                    .get(row)
                    .is_some_and(|row| self.installing_here(&row.component.name))
            }) =>
            {
                self.cancel_install(tree);
                None
            }
            _ => return Reply::NotMine,
        };
        Reply::Done(request)
    }

    fn installing_here(&self, name: &str) -> bool {
        matches!(&self.work, Work::Installing { plan, .. } if plan.first().is_some_and(|first| first == name))
    }

    fn component(&self, name: &str) -> Option<Component> {
        self.read
            .rows
            .iter()
            .map(|row| &row.component)
            .chain(
                self.read
                    .store
                    .iter()
                    .flat_map(|store| store.index.components.iter()),
            )
            .find(|component| component.name == name)
            .cloned()
    }

    fn title_of(&self, name: &str) -> String {
        self.component(name)
            .map_or_else(|| String::from(name), |component| component.title)
    }

    fn installed(&self) -> Vec<Component> {
        self.read
            .rows
            .iter()
            .filter(|row| row.state == INSTALLED)
            .map(|row| row.component.clone())
            .collect()
    }

    fn others_busy(&self) -> bool {
        self.busy()
            || self
                .read
                .rows
                .iter()
                .any(|row| row.state == INSTALLING || row.state == REMOVING)
    }

    fn press_row(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        row: usize,
        now_ms: u32,
    ) -> Option<Request> {
        let shown = self.read.rows.get(row)?.clone();
        let name = shown.component.name.clone();
        if self.installing_here(&name) {
            self.cancel_install(tree);
            return None;
        }
        if self.others_busy() {
            return None;
        }
        if self.confirming() {
            tree.set_default_button(self.default_button);
        }
        self.failed = None;
        self.note = Note::Closed;
        if shown.state == INSTALLED {
            if self.read.runs_this_window.as_deref() == Some(name.as_str()) {
                let (first, second) = messages::desktop_in_use(true);
                self.note = Note::Message {
                    name,
                    first,
                    second: String::from(second),
                };
                self.refresh(tree);
                return None;
            }
            self.work = Work::Checking(name);
            self.refresh(tree);
            return Some(Request::CheckComponentRemove);
        }
        let store = self.read.store.clone()?;
        let installed = self.installed();
        match manager::plan_install(&store.index, &installed, &name) {
            Ok(plan) if plan.len() > 1 => {
                let names: Vec<String> = plan.iter().map(|need| need.name.clone()).collect();
                let also = messages::also_installs(&plan).unwrap_or_default();
                let in_all = messages::in_all(&plan);
                let confirm = match plan.len() {
                    2 => "Install both",
                    _ => "Install all",
                };
                show_line(tree, self.widgets.first, &also, Token::Text);
                show_line(tree, self.widgets.second, &in_all, Token::TextDim);
                self.open_confirmation(tree, Note::Install { name, plan: names }, confirm, now_ms);
                None
            }
            Ok(plan) if plan.is_empty() => {
                self.refresh(tree);
                None
            }
            Ok(_) => self.start_install(tree, alloc::vec![name]),
            Err(refusal) => {
                let (first, _) = messages::refusal(&refusal, &store.file_name());
                self.note = Note::Message {
                    name,
                    first,
                    second: String::new(),
                };
                self.refresh(tree);
                None
            }
        }
    }

    fn start_install(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        plan: Vec<String>,
    ) -> Option<Request> {
        let name = plan.first()?.clone();
        let total = self
            .component(&name)
            .map_or(0, |component| component.size_bytes());
        self.work = Work::Installing {
            plan,
            received: 0,
            total,
            cancelled: false,
        };
        self.failed = None;
        self.note = Note::Progress { name };
        self.refresh(tree);
        Some(Request::InstallComponent)
    }

    fn cancel_install(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if let Work::Installing { cancelled, .. } = &mut self.work {
            *cancelled = true;
        }
        self.refresh(tree);
    }

    fn open_confirmation(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        note: Note,
        confirm: &str,
        now_ms: u32,
    ) {
        let widgets = self.widgets;
        let removing = matches!(note, Note::Remove { .. });
        tree.set_text(widgets.confirm, confirm);
        tree.set_person_only(widgets.confirm, removing);
        self.note = note;
        self.refresh(tree);
        tree.set_default_button(widgets.confirm);
        tree.set_focus(Some(widgets.confirm), now_ms);
    }

    fn close_confirmation(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let name = match &self.note {
            Note::Remove { name } | Note::Install { name, .. } => name.clone(),
            _ => return,
        };
        self.note = Note::Closed;
        tree.set_default_button(self.default_button);
        self.refresh(tree);
        if let Some(row) = self
            .read
            .rows
            .iter()
            .position(|row| row.component.name == name)
            .filter(|&row| row < ROWS)
        {
            tree.set_focus(Some(self.widgets.rows[row].button), now_ms);
        }
    }

    fn confirm(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let note = core::mem::replace(&mut self.note, Note::Closed);
        tree.set_default_button(self.default_button);
        match note {
            Note::Remove { name } => {
                self.work = Work::Removing(name);
                self.refresh(tree);
                Some(Request::RemoveComponent)
            }
            Note::Install { plan, .. } => self.start_install(tree, plan),
            other => {
                self.note = other;
                None
            }
        }
    }

    fn move_focus(&mut self, tree: &mut WidgetTree<CAPACITY>, here: usize, code: u16, now_ms: u32) {
        let shown = self.read.rows.len().min(ROWS);
        let usable = |row: &usize| tree.is_enabled(self.widgets.rows[*row].button);
        let target = match code {
            key::UP => (0..here).rev().find(usable),
            key::DOWN => (here + 1..shown).find(usable),
            key::HOME => (0..shown).find(usable),
            _ => (0..shown).rev().find(usable),
        };
        if let Some(row) = target {
            tree.set_focus(Some(self.widgets.rows[row].button), now_ms);
        }
    }

    fn note_row(&self) -> Option<usize> {
        let name = match &self.note {
            Note::Closed => return None,
            Note::Message { name, .. }
            | Note::Progress { name }
            | Note::Remove { name }
            | Note::Install { name, .. } => name,
        };
        self.read
            .rows
            .iter()
            .position(|row| &row.component.name == name)
            .filter(|&row| row < ROWS)
    }

    fn note_height(&self) -> i32 {
        match &self.note {
            Note::Closed => 0,
            Note::Message { second, .. } if second.is_empty() => MESSAGE_HEIGHT,
            Note::Message { .. } => TWO_LINE_HEIGHT,
            Note::Progress { .. } => PROGRESS_HEIGHT,
            Note::Remove { .. } | Note::Install { .. } => CONFIRMATION_HEIGHT,
        }
    }

    fn look(&self, row: &Row, others_busy: bool) -> Look {
        let component = &row.component;
        let size = component.size_bytes();
        let can_install = !others_busy && self.read.store.is_some();
        if let Work::Installing {
            plan,
            received,
            total,
            cancelled,
        } = &self.work
            && plan.first() == Some(&component.name)
        {
            return Look {
                state: messages::installing_state(*received, *total),
                word: format!("{INSTALLING} {}", messages::percent(*received, *total)),
                button: "Cancel",
                action: "Cancel installing",
                enabled: !cancelled,
            };
        }
        if self.to_remove() == Some(component.name.as_str()) || row.state == REMOVING {
            return Look {
                state: String::from(messages::REMOVING_STATE),
                word: String::from(REMOVING),
                button: "Remove",
                action: "Remove",
                enabled: false,
            };
        }
        if row.state == INSTALLING {
            return Look {
                state: String::from(messages::INSTALLING_ELSEWHERE_STATE),
                word: String::from(INSTALLING),
                button: "Install",
                action: "Install",
                enabled: false,
            };
        }
        if row.state == INSTALLED {
            return Look {
                state: messages::installed_state(size),
                word: String::from(INSTALLED),
                button: "Remove",
                action: "Remove",
                enabled: !others_busy,
            };
        }
        let state = match self.failed.as_deref() == Some(component.name.as_str()) {
            true => String::from(messages::FAILED_STATE),
            false => messages::not_installed_state(size),
        };
        Look {
            state,
            word: String::from(NOT_INSTALLED),
            button: "Install",
            action: "Install",
            enabled: can_install,
        }
    }

    fn refresh(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        match &self.read.store {
            Some(store) => {
                let installed = self.installed();
                let shown = self.read.rows.len().min(ROWS);
                let bytes: u64 = installed.iter().map(Component::size_bytes).sum();
                show_line(
                    tree,
                    widgets.store[0],
                    &messages::store_title(store),
                    Token::Text,
                );
                show_line(
                    tree,
                    widgets.store[1],
                    &messages::store_place(store),
                    Token::TextDim,
                );
                show_line(
                    tree,
                    widgets.store[2],
                    &messages::count_line(installed.len(), shown, bytes),
                    Token::TextDim,
                );
            }
            None => {
                let inks = [Token::Text, Token::TextDim, Token::TextDim];
                for ((id, words), ink) in
                    widgets.store.iter().zip(messages::NO_STORE_LINES).zip(inks)
                {
                    show_line(tree, *id, words, ink);
                }
            }
        }
        let shown = self.read.rows.len().min(ROWS);
        let acting = self.note_row();
        let note_height = match acting {
            Some(_) => self.note_height(),
            None => 0,
        };
        let others_busy = self.others_busy();
        for (index, row_widgets) in widgets.rows.iter().enumerate() {
            let Some(row) = self.read.rows.get(index).filter(|_| index < shown) else {
                tree.set_shown(row_widgets.panel, false);
                continue;
            };
            let below = acting.is_some_and(|acting| index > acting);
            let top = ROWS_TOP + ROW_HEIGHT * index as i32 + if below { note_height } else { 0 };
            tree.set_place(row_widgets.panel, at(PADDING, top, ROW_WIDTH, ROW_HEIGHT));
            tree.set_shown(row_widgets.panel, true);
            let look = self.look(row, others_busy);
            let component = &row.component;
            let state_width = text::width(&look.state);
            let title_width = (STATE_END - STATE_GAP - state_width - ROW_TEXT_X).max(0);
            tree.set_place(
                row_widgets.title,
                line_at(ROW_TEXT_X, TITLE_TOP, title_width),
            );
            show_line(tree, row_widgets.title, &component.title, Token::Text);
            show_line(tree, row_widgets.state, &look.state, Token::Text);
            show_line(
                tree,
                row_widgets.summary,
                &component.summary,
                Token::TextDim,
            );
            tree.set_text(row_widgets.button, look.button);
            tree.set_enabled(row_widgets.button, look.enabled);
            tree.set_accessible_name(
                row_widgets.button,
                &format!("{} {}", look.action, component.title),
            );
            tree.set_accessible_name(row_widgets.panel, &component.title);
            tree.set_accessible_value(
                row_widgets.panel,
                &format!(
                    "{} {} {}",
                    component.name,
                    look.word,
                    messages::size_text(component.size_bytes())
                ),
            );
        }
        let note_top = acting.map(|row| ROWS_TOP + ROW_HEIGHT * (row as i32 + 1));
        self.place_note(tree, note_top);
        let bottom = ROWS_TOP + ROW_HEIGHT * shown as i32 + note_height + BOTTOM_MARGIN;
        tree.set_place(
            widgets.pane,
            Place::at(
                0,
                0,
                Length::Fill(0),
                Length::Px(bottom.max(self.min_height)),
            ),
        );
    }

    fn place_note(&self, tree: &mut WidgetTree<CAPACITY>, top: Option<i32>) {
        let widgets = self.widgets;
        let Some(top) = top else {
            for id in [
                widgets.first,
                widgets.second,
                widgets.bar,
                widgets.cancel,
                widgets.confirm,
            ] {
                tree.set_shown(id, false);
            }
            return;
        };
        let (first_shown, second_shown, bar_shown, buttons_shown) = match &self.note {
            Note::Closed => (false, false, false, false),
            Note::Message { first, second, .. } => {
                show_line(tree, widgets.first, first, Token::Text);
                show_line(tree, widgets.second, second, Token::TextDim);
                (true, !second.is_empty(), false, false)
            }
            Note::Progress { name } => {
                let (received, total) = match &self.work {
                    Work::Installing {
                        received, total, ..
                    } => (*received, *total),
                    _ => (0, 0),
                };
                let permille = received.saturating_mul(PERMILLE) / total.max(1);
                tree.set_progress(widgets.bar, permille.min(PERMILLE) as u16);
                tree.set_accessible_name(
                    widgets.bar,
                    &format!("Installing {}", self.title_of(name)),
                );
                show_line(
                    tree,
                    widgets.first,
                    &messages::copied_of(received, total),
                    Token::Text,
                );
                (true, false, total > 0, false)
            }
            Note::Remove { .. } | Note::Install { .. } => (true, true, false, true),
        };
        let first_top = match &self.note {
            Note::Progress { .. } => BYTES_LINE,
            _ => FIRST_LINE,
        };
        tree.set_place(widgets.first, line_at(NOTE_X, top + first_top, NOTE_WIDTH));
        tree.set_place(
            widgets.second,
            line_at(NOTE_X, top + SECOND_LINE, NOTE_WIDTH),
        );
        tree.set_place(
            widgets.bar,
            at(NOTE_X, top + BAR_TOP, NOTE_WIDTH, BAR_HEIGHT),
        );
        let buttons_top = top + NOTE_BUTTONS_TOP;
        tree.set_place(
            widgets.cancel,
            at(NOTE_CANCEL.0, buttons_top, NOTE_CANCEL.1, BUTTON_HEIGHT),
        );
        tree.set_place(
            widgets.confirm,
            at(NOTE_CONFIRM.0, buttons_top, NOTE_CONFIRM.1, BUTTON_HEIGHT),
        );
        tree.set_shown(widgets.first, first_shown);
        tree.set_shown(widgets.second, second_shown);
        tree.set_shown(widgets.bar, bar_shown);
        tree.set_shown(widgets.cancel, buttons_shown);
        tree.set_shown(widgets.confirm, buttons_shown);
    }

    /// The store's three lines.
    pub fn store_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> [&'a str; 3] {
        self.widgets.store.map(|id| tree.text_of(id).unwrap_or(""))
    }

    /// Each shown row's title, state and button text.
    pub fn rows<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Vec<[&'a str; 3]> {
        self.widgets
            .rows
            .iter()
            .filter(|row| tree.is_shown(row.panel))
            .map(|row| [row.title, row.state, row.button].map(|id| tree.text_of(id).unwrap_or("")))
            .collect()
    }

    /// The note's lines shown, first then second.
    pub fn note_lines<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Vec<&'a str> {
        [self.widgets.first, self.widgets.second]
            .into_iter()
            .filter(|id| tree.is_shown(*id))
            .filter_map(|id| tree.text_of(id))
            .collect()
    }
}

#[cfg(test)]
mod tests;
