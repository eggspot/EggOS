//! Settings' Components pane (D235, docs/ux/components-v0.3.md, with v0.5's Updates band,
//! updates and the store download, D300): the components the attached
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
use eggos_components::manager::{self, Failure, Refusal, RemoveError, Store};
use eggos_components::messages;
use eggos_components::state::{Checked, Job};
use eggos_toolkit::input::key;
use eggos_toolkit::text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

/// The most rows the pane shows: the most components a store offers.
pub const ROWS: usize = MAX_COMPONENTS;
/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 4 + 7 * ROWS + 5 + EDITION_WIDGETS + UPDATES_WIDGETS + 1 + PLAN_LINES;
const PLAN_LINES: usize = 6;
const PLAN_LINE_PITCH: i32 = 20;
const UPDATES_WIDGETS: usize = 4;
const UPDATES_BAND: (i32, i32, i32, i32) = (16, 124, 432, 48);
const UPDATES_LINE: (i32, i32, i32) = (24, 6, 296);
const DAILY_BOX: (i32, i32) = (24, 24);
const UPDATES_BUTTON: (i32, i32, i32, i32) = (336, 10, 96, 28);
const ROW_UPDATE: (i32, i32, i32, i32) = (232, 10, 96, 28);
const UPDATE_TEXT_END: i32 = 216;
const DOWNLOAD: (i32, i32, i32, i32) = (352, 66, 96, 28);
const STORE_NOTE_TOP: i32 = 116;
const OFFLINE: &str = "The network is offline.";
const OFFLINE_RETRY: &str = "Connect or attach the store disk, then try again.";
const UPDATING: &str = "updating";
const EDITION_WIDGETS: usize = 4;
const EDITION_BAND: (i32, i32, i32, i32) = (16, 180, 432, 64);
const EDITION_LABEL: (i32, i32) = (24, 10);
const EDITION_DROPDOWN: (i32, i32, i32, i32) = (272, 4, 160, 28);
const EDITION_SUMMARY: (i32, i32, i32) = (24, 38, 408);
const EDITION_TITLES: [&str; 4] = ["Server", "Personal", "Professional", "Enterprise"];
const EDITION_TITLES_WITH_CUSTOM: [&str; 5] =
    ["Custom", "Server", "Personal", "Professional", "Enterprise"];
const CUSTOM_SUMMARY: &str = "Your own set of components: no edition matches it exactly.";
/// Row `r`'s top in the pane, with no note open above it: `ROWS_TOP + ROW_HEIGHT * r`.
pub const ROWS_TOP: i32 = 252;
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
    /// The edition recorded as last chosen (D263).
    pub recorded_edition: Option<String>,
    /// The last update check's stored result (D299).
    pub checked: Option<Checked>,
    /// Whether the daily check is on.
    pub daily: bool,
    /// The component job running, whoever started it.
    pub job: Option<Job>,
    /// The wall clock in seconds, when the machine has one.
    pub now_s: Option<u64>,
    /// Whether a network is up, so the store can be downloaded.
    pub network_up: bool,
    /// The component the last update job failed on and why (D421).
    pub failure: Option<(String, Failure)>,
}

/// A job the pane asks the program to start as `component ...` outside its window (D300).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentJob {
    /// `component update --dry-run --json`.
    Check,
    /// `component update [<name>]`.
    Update(Option<String>),
    /// `component update --daily on|off`.
    Daily(bool),
    /// `component list --download`.
    Download,
    /// `component cancel`.
    Cancel,
    /// `component install <name>` (D454).
    Install(String),
    /// `component remove <name> --yes`, once the pane confirmed it (D458).
    Remove(String),
}

impl ComponentJob {
    /// The words after `component`.
    pub fn words(&self) -> Vec<String> {
        let words: &[&str] = match self {
            Self::Check => &["update", "--dry-run", "--json"],
            Self::Update(_) => &["update"],
            Self::Daily(true) => &["update", "--daily", "on"],
            Self::Daily(false) => &["update", "--daily", "off"],
            Self::Download => &["list", "--download"],
            Self::Cancel => &["cancel"],
            Self::Install(_) => &["install"],
            Self::Remove(_) => &["remove"],
        };
        let mut words: Vec<String> = words.iter().map(|word| String::from(*word)).collect();
        if let Self::Update(Some(name)) | Self::Install(name) | Self::Remove(name) = self {
            words.push(name.clone());
        }
        if let Self::Remove(_) = self {
            words.push(String::from("--yes"));
        }
        words
    }

    /// Whether the pane waits for it to end.
    pub fn is_awaited(&self) -> bool {
        matches!(
            self,
            Self::Check | Self::Update(_) | Self::Download | Self::Install(_) | Self::Remove(_)
        )
    }
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
    update: WidgetId,
    tip: WidgetId,
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
    edition: WidgetId,
    edition_label: WidgetId,
    edition_summary: WidgetId,
    edition_band: WidgetId,
    updates_band: WidgetId,
    updates_line: WidgetId,
    daily: WidgetId,
    updates_button: WidgetId,
    download: WidgetId,
    plan_lines: [WidgetId; PLAN_LINES],
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
        edition: Option<&'static str>,
    },
    Band {
        first: String,
        second: String,
    },
    Store {
        first: String,
        second: String,
    },
    StoreProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
    Store,
    Band,
    Edition,
    Row(usize),
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
    CheckingUpdates,
    Updating {
        plan: Vec<String>,
        current: String,
        cancelling: bool,
    },
    DownloadingStore {
        cancelling: bool,
    },
}

struct Look {
    state: String,
    word: String,
    button: &'static str,
    action: &'static str,
    enabled: bool,
    reason: &'static str,
    update: Option<UpdateLook>,
}

struct UpdateLook {
    line: String,
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
    /// The edition being installed from the band, recorded once its plan is done (D263).
    chosen_edition: Option<&'static str>,
    asked: Option<ComponentJob>,
    time_zone: usize,
    metrics: Metrics,
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

/// The device class the pane lays itself out for (docs/ux/components-v0.5.md section 10): a
/// phone's pane is `width` wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Desktop,
    Tablet,
    Phone {
        width: i32,
    },
}

type Box4 = (i32, i32, i32, i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Metrics {
    width: i32,
    download: Box4,
    download_shift: i32,
    updates_band: Box4,
    updates_line: (i32, i32, i32),
    daily: (i32, i32),
    updates_button: Box4,
    edition_band: Box4,
    edition_label: (i32, i32),
    edition_dropdown: Box4,
    edition_summary: (i32, i32, i32),
    rows_top: i32,
    row_height: i32,
    row_height_with_update: i32,
    row_button: Box4,
    row_update: Box4,
    button_under_update: i32,
    stacked: bool,
}

const STACKED_TEXT: (i32, i32, i32, i32) = (16, 8, 28, 48);
const PHONE_GAP: i32 = 16;
const TOUCH_BUTTON: i32 = 48;
const NOTE_X_PHONE: i32 = 24;
const FINISHING: &str = "finishing";

impl Metrics {
    fn of(layout: Layout) -> Self {
        match layout {
            Layout::Desktop => Self {
                width: ROW_WIDTH,
                download: DOWNLOAD,
                download_shift: 0,
                updates_band: UPDATES_BAND,
                updates_line: UPDATES_LINE,
                daily: DAILY_BOX,
                updates_button: UPDATES_BUTTON,
                edition_band: EDITION_BAND,
                edition_label: EDITION_LABEL,
                edition_dropdown: EDITION_DROPDOWN,
                edition_summary: EDITION_SUMMARY,
                rows_top: ROWS_TOP,
                row_height: ROW_HEIGHT,
                row_height_with_update: ROW_HEIGHT,
                row_button: ROW_BUTTON,
                row_update: ROW_UPDATE,
                button_under_update: ROW_BUTTON.1,
                stacked: false,
            },
            Layout::Tablet => Self {
                width: ROW_WIDTH,
                download: (DOWNLOAD.0, DOWNLOAD.1, DOWNLOAD.2, 40),
                download_shift: 0,
                updates_band: (16, 124, ROW_WIDTH, 56),
                updates_line: (24, 8, 296),
                daily: (24, 28),
                updates_button: (336, 8, 96, 40),
                edition_band: (16, 188, ROW_WIDTH, 72),
                edition_label: (24, 14),
                edition_dropdown: (272, 8, 160, 40),
                edition_summary: (24, 48, 408),
                rows_top: 268,
                row_height: 56,
                row_height_with_update: 56,
                row_button: (336, 8, 96, 40),
                row_update: (232, 8, 96, 40),
                button_under_update: 8,
                stacked: false,
            },
            Layout::Phone { width } => {
                let band = width - 2 * PADDING;
                let inner = band - PHONE_GAP;
                Self {
                    width: band,
                    download: (PADDING, 120, band, 48),
                    download_shift: 56,
                    updates_band: (PADDING, 124, band, 132),
                    updates_line: (8, 8, inner),
                    daily: (8, 28),
                    updates_button: (8, 76, inner, 48),
                    edition_band: (PADDING, 264, band, 116),
                    edition_label: (8, 8),
                    edition_dropdown: (8, 28, inner, 48),
                    edition_summary: (8, 84, inner),
                    rows_top: 388,
                    row_height: 128,
                    row_height_with_update: 184,
                    row_button: (8, 72, inner, 48),
                    row_update: (8, 72, inner, 48),
                    button_under_update: 128,
                    stacked: true,
                }
            }
        }
    }
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
        let updates_band = add(
            tree,
            pane,
            at(
                UPDATES_BAND.0,
                UPDATES_BAND.1,
                UPDATES_BAND.2,
                UPDATES_BAND.3,
            ),
            Kind::panel(Token::Menu),
        );
        tree.set_accessible_name(updates_band, "Updates");
        let updates_line = label(
            tree,
            updates_band,
            line_at(UPDATES_LINE.0, UPDATES_LINE.1, UPDATES_LINE.2),
        );
        let daily = add(
            tree,
            updates_band,
            Place::at(DAILY_BOX.0, DAILY_BOX.1, Length::Content, Length::Content),
            Kind::checkbox("Check every day", true),
        );
        let updates_button = add(
            tree,
            updates_band,
            at(
                UPDATES_BUTTON.0,
                UPDATES_BUTTON.1,
                UPDATES_BUTTON.2,
                UPDATES_BUTTON.3,
            ),
            Kind::button("Check now", false),
        );
        let band = add(
            tree,
            pane,
            at(
                EDITION_BAND.0,
                EDITION_BAND.1,
                EDITION_BAND.2,
                EDITION_BAND.3,
            ),
            Kind::panel(Token::Menu),
        );
        let edition_label = add(
            tree,
            band,
            line_at(EDITION_LABEL.0, EDITION_LABEL.1, 0),
            Kind::label("Edition", Token::Text),
        );
        let edition = add(
            tree,
            band,
            at(
                EDITION_DROPDOWN.0,
                EDITION_DROPDOWN.1,
                EDITION_DROPDOWN.2,
                EDITION_DROPDOWN.3,
            ),
            Kind::dropdown("Edition", &EDITION_TITLES, 0),
        );
        let edition_summary = add(
            tree,
            band,
            line_at(EDITION_SUMMARY.0, EDITION_SUMMARY.1, EDITION_SUMMARY.2),
            Kind::label("", Token::TextDim),
        );
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
            let update = add(
                tree,
                panel,
                at(ROW_UPDATE.0, ROW_UPDATE.1, ROW_UPDATE.2, ROW_UPDATE.3),
                Kind::button("Update", false),
            );
            let button = add(
                tree,
                panel,
                at(ROW_BUTTON.0, ROW_BUTTON.1, ROW_BUTTON.2, ROW_BUTTON.3),
                Kind::button("Install", false),
            );
            let tip = add(
                tree,
                button,
                Place::at(0, 0, Length::Content, Length::Content),
                Kind::tooltip("", ""),
            );
            tree.set_shown(tip, false);
            RowWidgets {
                panel,
                title,
                state,
                summary,
                button,
                update,
                tip,
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
        let download = add(
            tree,
            pane,
            at(DOWNLOAD.0, DOWNLOAD.1, DOWNLOAD.2, DOWNLOAD.3),
            Kind::button("Download", false),
        );
        tree.set_accessible_name(download, "Download the component store");
        let plan_lines =
            core::array::from_fn(|_| label(tree, pane, line_at(NOTE_X, 0, NOTE_WIDTH)));
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
                edition,
                edition_label,
                edition_summary,
                edition_band: band,
                updates_band,
                updates_line,
                daily,
                updates_button,
                download,
                plan_lines,
            },
            read: ComponentsRead::default(),
            note: Note::Closed,
            work: Work::Idle,
            failed: None,
            min_height,
            default_button,
            chosen_edition: None,
            asked: None,
            time_zone: crate::facts::utc_zone(),
            metrics: Metrics::of(Layout::Desktop),
        };
        components.refresh(tree);
        components
    }

    /// Lays the pane out for `layout`'s device class (docs/ux/components-v0.5.md section 10).
    pub fn set_layout(&mut self, tree: &mut WidgetTree<CAPACITY>, layout: Layout) {
        let metrics = Metrics::of(layout);
        if metrics == self.metrics {
            return;
        }
        self.metrics = metrics;
        let widgets = self.widgets;
        let place = |tree: &mut WidgetTree<CAPACITY>, id: WidgetId, (x, y, w, h): Box4| {
            tree.set_place(id, at(x, y, w, h));
        };
        place(tree, widgets.download, metrics.download);
        let (x, y, w) = metrics.updates_line;
        tree.set_place(widgets.updates_line, line_at(x, y, w));
        tree.set_place(
            widgets.daily,
            Place::at(
                metrics.daily.0,
                metrics.daily.1,
                Length::Content,
                Length::Content,
            ),
        );
        place(tree, widgets.updates_button, metrics.updates_button);
        tree.set_place(
            widgets.edition_label,
            line_at(metrics.edition_label.0, metrics.edition_label.1, 0),
        );
        place(tree, widgets.edition, metrics.edition_dropdown);
        let (x, y, w) = metrics.edition_summary;
        tree.set_place(widgets.edition_summary, line_at(x, y, w));
        for row in widgets.rows {
            place(tree, row.update, metrics.row_update);
            place(tree, row.button, metrics.row_button);
        }
        self.refresh(tree);
    }

    /// Says the time zone the last check's time is shown in, an index of
    /// `eggos_calendar::zone::ZONES`.
    pub fn set_time_zone(&mut self, tree: &mut WidgetTree<CAPACITY>, zone: usize) {
        if self.time_zone != zone {
            self.time_zone = zone;
            self.refresh(tree);
        }
    }

    fn local(&self, utc: u64) -> u64 {
        let offset = eggos_calendar::zone::ZONES
            .get(self.time_zone)
            .and_then(|(_, rule)| eggos_calendar::zone::parse(rule))
            .map_or(0, |rule| rule.at(utc as i64).offset);
        utc.saturating_add_signed(offset)
    }

    /// The pane is shown again: messages and a `Failed` state are forgotten, an open
    /// confirmation closes; the program reads the store and the records next.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if self.confirming() {
            tree.set_default_button(self.default_button);
        }
        if !matches!(self.note, Note::Progress { .. } | Note::StoreProgress) {
            self.note = Note::Closed;
        }
        self.failed = None;
        self.refresh(tree);
    }

    /// Shows what the program just read.
    pub fn show_components(&mut self, tree: &mut WidgetTree<CAPACITY>, read: ComponentsRead) {
        self.read = read;
        self.follow_job();
        self.refresh(tree);
    }

    fn follow_job(&mut self) {
        let Some(job) = self.read.job.clone() else {
            return;
        };
        if let Work::Installing {
            plan,
            received,
            total,
            ..
        } = &mut self.work
            && job.kind == INSTALLING
            && plan.first() == Some(&job.name)
        {
            *received = job.done;
            *total = job.total.max(*total);
        }
        if let Work::Updating { current, .. } = &mut self.work
            && job.kind == UPDATING
            && *current != job.name
        {
            current.clone_from(&job.name);
            if matches!(self.note, Note::Progress { .. }) {
                self.note = Note::Progress { name: job.name };
            }
        }
    }

    /// The job the program must start now, taken once.
    pub fn take_job(&mut self) -> Option<ComponentJob> {
        self.asked.take()
    }

    /// Whether the pane waits for a job it asked for to end, so the program reads the
    /// components every 100 ms.
    pub fn awaits_job(&self) -> bool {
        matches!(
            self.work,
            Work::CheckingUpdates | Work::Updating { .. } | Work::DownloadingStore { .. }
        )
    }

    /// Shows how the job the pane waited for ended, with what was read after.
    pub fn show_job_ended(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        succeeded: bool,
        read: ComponentsRead,
    ) -> Option<Request> {
        if let Work::Removing(name) = &self.work {
            let result = match succeeded {
                true => Ok(()),
                false => Err(RemoveError::Failed(match &read.failure {
                    Some((failed, failure)) if failed == name => failure.clone(),
                    _ => Failure::Errno(eggos_runtime::errno::Errno::EIO),
                })),
            };
            self.show_removed(tree, result, read);
            return None;
        }
        if let Work::Installing {
            plan, cancelled, ..
        } = &self.work
        {
            let name = plan.first().cloned().unwrap_or_default();
            let result = match (succeeded, *cancelled) {
                (true, _) => Ok(()),
                (false, true) => Err(Failure::Cancelled),
                (false, false) => Err(match &read.failure {
                    Some((failed, failure)) if *failed == name => failure.clone(),
                    _ => Failure::Errno(eggos_runtime::errno::Errno::EIO),
                }),
            };
            return self.show_installed(tree, result, read);
        }
        let work = core::mem::replace(&mut self.work, Work::Idle);
        self.read = read;
        self.note = match work {
            Work::CheckingUpdates if !succeeded => Note::Band {
                first: String::from(messages::CHECK_FAILED),
                second: String::from(match self.read.network_up {
                    true => messages::RELEASE_SILENT,
                    false => OFFLINE,
                }),
            },
            Work::Updating {
                current,
                cancelling: true,
                ..
            } => Note::Message {
                name: current,
                first: String::from(messages::CANCELLED),
                second: String::from(messages::INSTALLED_COPY_STAYS),
            },
            Work::Updating { current, .. } if !succeeded => {
                let title = self.title_of(&current);
                let (first, second) = match &self.read.failure {
                    Some((name, failure)) if *name == current => {
                        let (first, second) = messages::update_failure(failure, &title);
                        (first, String::from(second))
                    }
                    _ => (
                        messages::update_failed(&title),
                        String::from(messages::INSTALLED_COPY_STAYS),
                    ),
                };
                Note::Message {
                    name: current,
                    first,
                    second,
                }
            }
            Work::DownloadingStore { cancelling: true } => Note::Store {
                first: String::from(messages::CANCELLED),
                second: String::from(messages::NOTHING_DOWNLOADED),
            },
            Work::DownloadingStore { .. } if !succeeded => Note::Store {
                first: String::from(messages::STORE_DOWNLOAD_FAILED),
                second: String::from(messages::NOTHING_DOWNLOADED),
            },
            _ => Note::Closed,
        };
        self.refresh(tree);
        None
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
        result: Result<(), RemoveError>,
        read: ComponentsRead,
    ) {
        let removing = match core::mem::replace(&mut self.work, Work::Idle) {
            Work::Removing(name) => Some(name),
            _ => None,
        };
        let component = removing.as_deref().and_then(|name| self.component(name));
        self.read = read;
        self.note = match (result, removing, component) {
            (Err(RemoveError::Failed(failure)), Some(name), Some(component)) => {
                let (first, second) = messages::remove_failure(&failure, &component);
                Note::Message {
                    name,
                    first,
                    second: String::from(second),
                }
            }
            (Err(RemoveError::Refused(refusal)), Some(name), _) => {
                let (first, _) = messages::refusal(&refusal, "");
                Note::Message {
                    name,
                    first,
                    second: String::new(),
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
        let update_of = |id: WidgetId| widgets.rows.iter().position(|row| row.update == id);
        let request = match response {
            Response::Chosen(id, option) if id == widgets.edition => {
                self.choose_edition(tree, option, now_ms)
            }
            Response::Activated(id) if id == widgets.updates_button => self.press_updates(tree),
            Response::Toggled(id, on) if id == widgets.daily => {
                self.read.daily = on;
                self.asked = Some(ComponentJob::Daily(on));
                Some(Request::StartComponentJob)
            }
            Response::Activated(id) if update_of(id).is_some() => {
                self.press_update(tree, update_of(id).unwrap_or(0))
            }
            Response::Activated(id) if id == widgets.download => self.press_download(tree),
            Response::Activated(id) if id == widgets.cancel && self.note == Note::StoreProgress => {
                self.cancel_job(tree)
            }
            Response::Unused { code, .. }
                if matches!(code, key::LEFT | key::RIGHT)
                    && tree
                        .focus()
                        .is_some_and(|id| row_of(id).is_some() || update_of(id).is_some()) =>
            {
                let focus = tree.focus().unwrap_or(widgets.pane);
                let row = row_of(focus).or_else(|| update_of(focus)).unwrap_or(0);
                let target = match code {
                    key::LEFT => widgets.rows[row].update,
                    _ => widgets.rows[row].button,
                };
                if tree.is_shown(target) && tree.is_enabled(target) {
                    tree.set_focus(Some(target), now_ms);
                }
                None
            }
            Response::Unused {
                code: key::DELETE, ..
            } if tree
                .focus()
                .and_then(update_of)
                .is_some_and(|row| self.updating_row(row)) =>
            {
                self.cancel_job(tree)
            }
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
                self.cancel_install(tree)
            }
            _ => return Reply::NotMine,
        };
        Reply::Done(request)
    }

    fn current_edition(&self) -> Option<&'static eggos_components::editions::Edition> {
        let installed = self.installed();
        let names = installed.iter().map(|component| component.name.as_str());
        eggos_components::editions::current_preferring(names, self.read.recorded_edition.as_deref())
    }

    fn show_edition(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let current = self.current_edition();
        let options: &'static [&'static str] = match current {
            Some(_) => &EDITION_TITLES,
            None => &EDITION_TITLES_WITH_CUSTOM,
        };
        tree.change(widgets.edition, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.options = options;
            }
        });
        let index = current.map_or(0, |edition| {
            eggos_components::editions::EDITIONS
                .iter()
                .position(|each| each.name == edition.name)
                .unwrap_or(0)
        });
        tree.set_selected(widgets.edition, index);
        let summary = current.map_or(CUSTOM_SUMMARY, |edition| edition.summary);
        show_line(tree, widgets.edition_summary, summary, Token::TextDim);
        tree.set_enabled(widgets.edition, !self.busy() && !self.confirming());
        tree.set_accessible_value(
            widgets.edition,
            current.map_or("none", |edition| edition.name),
        );
    }

    fn choose_edition(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        option: usize,
        now_ms: u32,
    ) -> Option<Request> {
        let custom_shown = self.current_edition().is_none();
        let index = if custom_shown {
            option.checked_sub(1)?
        } else {
            option
        };
        let edition = eggos_components::editions::EDITIONS.get(index)?;
        if self
            .current_edition()
            .is_some_and(|current| current.name == edition.name)
            || self.others_busy()
        {
            return None;
        }
        let Some(store) = self.read.store.clone() else {
            show_line(
                tree,
                self.widgets.edition_summary,
                "No component store: component edition downloads it, or attach the store disk.",
                Token::Text,
            );
            return None;
        };
        let mut installed = self.installed();
        let mut plan: Vec<String> = Vec::new();
        for wanted in edition.components {
            if installed.iter().any(|component| component.name == *wanted)
                || !store
                    .index
                    .components
                    .iter()
                    .any(|offered| offered.name == *wanted)
            {
                continue;
            }
            let Ok(needs) = manager::plan_install(&store.index, &installed, wanted) else {
                continue;
            };
            let needs: Vec<Component> = needs.into_iter().cloned().collect();
            for need in needs {
                if !plan.contains(&need.name) {
                    plan.push(need.name.clone());
                    installed.push(need);
                }
            }
        }
        if plan.is_empty() {
            let line = format!("{} is installed: nothing to add.", edition.title);
            show_line(tree, self.widgets.edition_summary, &line, Token::Text);
            return None;
        }
        let first = plan.first().cloned().unwrap_or_default();
        let line = format!("Would install for {}:", edition.title);
        let components: Vec<Component> = plan
            .iter()
            .filter_map(|name| self.component(name))
            .collect();
        let parts: Vec<&Component> = components.iter().collect();
        show_line(tree, self.widgets.first, &line, Token::Text);
        show_line(
            tree,
            self.widgets.second,
            &messages::in_all(&parts),
            Token::TextDim,
        );
        self.chosen_edition = Some(edition.name);
        let edition = Some(edition.title);
        self.open_confirmation(
            tree,
            Note::Install {
                name: first,
                plan,
                edition,
            },
            "Install",
            now_ms,
        );
        None
    }

    fn updating_row(&self, row: usize) -> bool {
        self.read
            .rows
            .get(row)
            .is_some_and(|row| matches!(&self.work, Work::Updating { current, .. } if *current == row.component.name))
    }

    fn rows_with_updates(&self) -> Vec<&Row> {
        self.read
            .rows
            .iter()
            .filter(|row| row.state == INSTALLED && row.update.is_some())
            .collect()
    }

    fn press_updates(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        if self.others_busy() || self.installed().is_empty() {
            return None;
        }
        if matches!(self.note, Note::Band { .. }) {
            self.note = Note::Closed;
        }
        let plan: Vec<String> = self
            .rows_with_updates()
            .iter()
            .map(|row| row.component.name.clone())
            .collect();
        let job = match plan.first().cloned() {
            Some(current) => {
                self.note = Note::Progress {
                    name: current.clone(),
                };
                self.work = Work::Updating {
                    plan,
                    current,
                    cancelling: false,
                };
                ComponentJob::Update(None)
            }
            None => {
                self.work = Work::CheckingUpdates;
                ComponentJob::Check
            }
        };
        self.asked = Some(job);
        self.refresh(tree);
        Some(Request::StartComponentJob)
    }

    fn press_update(&mut self, tree: &mut WidgetTree<CAPACITY>, row: usize) -> Option<Request> {
        if self.updating_row(row) {
            return self.cancel_job(tree);
        }
        let name = self.read.rows.get(row)?.component.name.clone();
        if self.others_busy() {
            return None;
        }
        if self.confirming() {
            tree.set_default_button(self.default_button);
        }
        self.note = Note::Progress { name: name.clone() };
        self.work = Work::Updating {
            plan: alloc::vec![name.clone()],
            current: name.clone(),
            cancelling: false,
        };
        self.asked = Some(ComponentJob::Update(Some(name)));
        self.refresh(tree);
        Some(Request::StartComponentJob)
    }

    fn press_download(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        if self.read.store.is_some() || self.others_busy() {
            return None;
        }
        if !self.read.network_up {
            self.note = Note::Store {
                first: String::from(OFFLINE),
                second: String::from(OFFLINE_RETRY),
            };
            self.refresh(tree);
            return None;
        }
        self.work = Work::DownloadingStore { cancelling: false };
        self.note = Note::StoreProgress;
        self.asked = Some(ComponentJob::Download);
        self.refresh(tree);
        Some(Request::StartComponentJob)
    }

    fn cancel_job(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        let swapping = self
            .read
            .job
            .as_ref()
            .is_some_and(|job| job.phase == "swapping");
        match &mut self.work {
            Work::Updating { cancelling, .. } | Work::DownloadingStore { cancelling }
                if !*cancelling && !swapping =>
            {
                *cancelling = true;
            }
            _ => return None,
        }
        self.asked = Some(ComponentJob::Cancel);
        self.refresh(tree);
        Some(Request::StartComponentJob)
    }

    fn show_updates_band(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let others_busy = self.others_busy();
        let with_updates = self.rows_with_updates();
        let bytes: u64 = with_updates
            .iter()
            .filter_map(|row| row.update.as_ref())
            .map(|update| update.fetch_bytes)
            .sum();
        let from_disk = self
            .read
            .checked
            .as_ref()
            .is_some_and(|checked| checked.source == "disk");
        let (line, button, enabled) = match (&self.work, &self.read.checked) {
            _ if self.installed().is_empty() => (
                String::from(messages::NOTHING_TO_UPDATE),
                "Check now",
                false,
            ),
            (Work::CheckingUpdates, _) => (String::from(messages::CHECKING), "Check now", false),
            (Work::Updating { plan, current, .. }, _) if plan.len() > 1 => {
                let at = plan.iter().position(|name| name == current).unwrap_or(0) + 1;
                (
                    messages::updating_all(at, plan.len(), &self.title_of(current)),
                    "Update all",
                    false,
                )
            }
            (_, None) => (
                String::from(messages::NOT_CHECKED),
                "Check now",
                !others_busy,
            ),
            _ if !with_updates.is_empty() => (
                messages::updates_line(with_updates.len(), bytes, from_disk),
                "Update all",
                !others_busy,
            ),
            (_, Some(checked)) => (
                messages::up_to_date(&messages::checked_when(
                    self.local(checked.at),
                    self.read.now_s.map(|now| self.local(now)),
                )),
                "Check now",
                !others_busy,
            ),
        };
        show_line(tree, widgets.updates_line, &line, Token::Text);
        tree.set_text(widgets.updates_button, button);
        tree.set_enabled(widgets.updates_button, enabled);
        tree.set_accessible_name(
            widgets.updates_button,
            match button {
                "Update all" => "Update all components",
                _ => "Check for updates now",
            },
        );
        tree.set_selected(widgets.daily, usize::from(self.read.daily));
        tree.set_enabled(widgets.daily, !self.awaits_job());
        let value = match &self.read.checked {
            None => String::from("unchecked"),
            Some(_) => format!("{}", with_updates.len()),
        };
        tree.set_accessible_value(widgets.updates_band, &value);
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
            || self.read.job.is_some()
            || self.read.rows.iter().any(|row| {
                row.state == INSTALLING || row.state == REMOVING || row.state == UPDATING
            })
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
            return self.cancel_install(tree);
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
                self.open_confirmation(
                    tree,
                    Note::Install {
                        name,
                        plan: names,
                        edition: None,
                    },
                    confirm,
                    now_ms,
                );
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
        self.note = Note::Progress { name: name.clone() };
        self.refresh(tree);
        self.asked = Some(ComponentJob::Install(name));
        Some(Request::StartComponentJob)
    }

    fn cancel_install(&mut self, tree: &mut WidgetTree<CAPACITY>) -> Option<Request> {
        if let Work::Installing { cancelled, .. } = &mut self.work {
            *cancelled = true;
        }
        self.refresh(tree);
        self.asked = Some(ComponentJob::Cancel);
        Some(Request::StartComponentJob)
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
                self.work = Work::Removing(name.clone());
                self.refresh(tree);
                self.asked = Some(ComponentJob::Remove(name));
                Some(Request::StartComponentJob)
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

    fn note_anchor(&self) -> Option<Anchor> {
        let name = match &self.note {
            Note::Closed => return None,
            Note::Store { .. } | Note::StoreProgress => return Some(Anchor::Store),
            Note::Band { .. } => return Some(Anchor::Band),
            Note::Install {
                edition: Some(_), ..
            } => return Some(Anchor::Edition),
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
            .map(Anchor::Row)
    }

    fn note_height(&self) -> i32 {
        match &self.note {
            Note::Closed => 0,
            Note::Message { second, .. }
            | Note::Band { second, .. }
            | Note::Store { second, .. }
                if second.is_empty() =>
            {
                MESSAGE_HEIGHT
            }
            Note::Message { .. } | Note::Band { .. } | Note::Store { .. } => TWO_LINE_HEIGHT,
            Note::Progress { .. } => PROGRESS_HEIGHT,
            Note::Install {
                edition: Some(_),
                plan,
                ..
            } => {
                CONFIRMATION_HEIGHT
                    + PLAN_LINE_PITCH * plan.len().min(PLAN_LINES) as i32
                    + self.stacked_buttons()
            }
            Note::Remove { .. } | Note::Install { .. } => {
                CONFIRMATION_HEIGHT + self.stacked_buttons()
            }
            Note::StoreProgress => CONFIRMATION_HEIGHT,
        }
    }

    fn note_column(&self) -> (i32, i32) {
        match self.metrics.stacked {
            true => (NOTE_X_PHONE, self.metrics.width + 2 * PADDING - 48),
            false => (NOTE_X, NOTE_WIDTH),
        }
    }

    fn install_finishing(&self) -> bool {
        matches!(&self.work, Work::Installing { plan, .. }
        if self.read.job.as_ref().is_some_and(|job| {
            job.kind == INSTALLING && job.phase == FINISHING && plan.first() == Some(&job.name)
        }))
    }

    fn stacked_buttons(&self) -> i32 {
        match self.metrics.stacked {
            true => 2 * TOUCH_BUTTON + 8 - BUTTON_HEIGHT,
            false => 0,
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
                enabled: !cancelled && !self.install_finishing(),
                reason: messages::CANCELLING_REASON,
                update: None,
            };
        }
        if self.to_remove() == Some(component.name.as_str()) || row.state == REMOVING {
            return Look {
                state: String::from(messages::REMOVING_STATE),
                word: String::from(REMOVING),
                button: "Remove",
                action: "Remove",
                enabled: false,
                reason: messages::BEING_REMOVED_REASON,
                update: None,
            };
        }
        if row.state == INSTALLING {
            return Look {
                state: String::from(messages::INSTALLING_ELSEWHERE_STATE),
                word: String::from(INSTALLING),
                button: "Install",
                action: "Install",
                enabled: false,
                reason: messages::INSTALLING_REASON,
                update: None,
            };
        }
        let updating_here =
            matches!(&self.work, Work::Updating { current, .. } if *current == component.name);
        let job_here = row.job.as_ref().filter(|job| job.kind == UPDATING);
        if updating_here || job_here.is_some() || row.state == UPDATING {
            let percent = job_here.map_or(0, Job::percent);
            let cancelling = matches!(
                &self.work,
                Work::Updating {
                    cancelling: true,
                    ..
                }
            );
            let swapping = job_here.is_some_and(|job| job.phase == "swapping");
            return Look {
                state: messages::installed_state(size),
                word: format!("{UPDATING} {percent}"),
                button: "Remove",
                action: "Remove",
                enabled: false,
                reason: messages::UPDATING_REASON,
                update: Some(UpdateLook {
                    line: messages::updating_state(percent),
                    button: "Cancel",
                    action: "Cancel updating",
                    enabled: updating_here && !cancelling && !swapping,
                }),
            };
        }
        if row.state == INSTALLED {
            let from_disk = self
                .read
                .checked
                .as_ref()
                .is_some_and(|checked| checked.source == "disk");
            let update = row.update.as_ref().map(|update| UpdateLook {
                line: messages::update_row_line(update.fetch_bytes, from_disk),
                button: "Update",
                action: "Update",
                enabled: !others_busy,
            });
            return Look {
                state: messages::installed_state(size),
                word: match &row.update {
                    Some(update) => format!(
                        "{INSTALLED} {} update {}",
                        messages::size_text(size),
                        messages::size_text(update.fetch_bytes)
                    ),
                    None => String::from(INSTALLED),
                },
                button: "Remove",
                action: "Remove",
                enabled: !others_busy,
                reason: self.busy_reason(),
                update,
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
            reason: match self.read.store {
                Some(_) => self.busy_reason(),
                None => messages::NO_STORE_REASON,
            },
            update: None,
        }
    }

    fn busy_reason(&self) -> &'static str {
        let removing =
            self.to_remove().is_some() || self.read.rows.iter().any(|row| row.state == REMOVING);
        let updating = matches!(self.work, Work::Updating { .. })
            || self.read.rows.iter().any(|row| row.state == UPDATING);
        match (removing, updating) {
            (true, _) => messages::OTHER_REMOVING_REASON,
            (false, true) => messages::OTHER_UPDATING_REASON,
            (false, false) => messages::OTHER_INSTALLING_REASON,
        }
    }

    /// The edition whose plan the band started and that finished installing: the program
    /// records it (D263). Taken once.
    pub fn take_finished_edition(&mut self) -> Option<&'static str> {
        if matches!(self.work, Work::Idle) && self.failed.is_none() {
            return self.chosen_edition.take();
        }
        None
    }

    fn refresh(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        self.show_edition(tree);
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
                let lines = messages::DOWNLOAD_STORE_LINES[usize::from(!self.read.network_up)];
                for ((id, words), ink) in widgets.store.iter().zip(lines).zip(inks) {
                    show_line(tree, *id, words, ink);
                }
            }
        }
        tree.set_shown(
            widgets.download,
            self.read.store.is_none() && self.note != Note::StoreProgress,
        );
        tree.set_enabled(widgets.download, !self.others_busy());
        self.show_updates_band(tree);
        let shown = self.read.rows.len().min(ROWS);
        let anchor = self.note_anchor();
        let note_height = match anchor {
            Some(_) => self.note_height(),
            None => 0,
        };
        let below_store = if anchor == Some(Anchor::Store) {
            note_height
        } else {
            0
        };
        let below_band = match anchor {
            Some(Anchor::Store | Anchor::Band) => note_height,
            _ => 0,
        };
        let below_edition = match anchor {
            Some(Anchor::Store | Anchor::Band | Anchor::Edition) => note_height,
            _ => 0,
        };
        let m = self.metrics;
        let shift = match self.read.store.is_none() && self.note != Note::StoreProgress {
            true => m.download_shift,
            false => 0,
        };
        let (x, y, w, h) = m.updates_band;
        tree.set_place(widgets.updates_band, at(x, y + shift + below_store, w, h));
        let (x, y, w, h) = m.edition_band;
        tree.set_place(widgets.edition_band, at(x, y + shift + below_band, w, h));
        let mut next_top = m.rows_top + shift + below_edition;
        let mut row_bottoms = [0i32; ROWS];
        let acting = match anchor {
            Some(Anchor::Row(row)) => Some(row),
            _ => None,
        };
        let others_busy = self.others_busy();
        for (index, row_widgets) in widgets.rows.iter().enumerate() {
            let Some(row) = self.read.rows.get(index).filter(|_| index < shown) else {
                tree.set_shown(row_widgets.panel, false);
                continue;
            };
            let look = self.look(row, others_busy);
            let height = match look.update.is_some() {
                true => m.row_height_with_update,
                false => m.row_height,
            };
            let top = next_top;
            next_top += height;
            row_bottoms[index] = next_top;
            if acting == Some(index) {
                next_top += note_height;
            }
            tree.set_place(row_widgets.panel, at(PADDING, top, m.width, height));
            tree.set_shown(row_widgets.panel, true);
            let component = &row.component;
            if m.stacked {
                let (x, title_y, state_y, summary_y) = STACKED_TEXT;
                tree.set_place(row_widgets.title, line_at(x, title_y, m.width - 2 * x));
                tree.set_place(
                    row_widgets.state,
                    Place::at(x, state_y, Length::Content, Length::Content),
                );
                tree.set_place(row_widgets.summary, line_at(x, summary_y, m.width - 2 * x));
                let (bx, _, bw, bh) = m.row_button;
                let button_y = match look.update.is_some() {
                    true => m.button_under_update,
                    false => m.row_button.1,
                };
                tree.set_place(row_widgets.button, at(bx, button_y, bw, bh));
            }
            let title_width = match &look.update {
                Some(_) => UPDATE_TEXT_END - ROW_TEXT_X,
                None => (STATE_END - STATE_GAP - text::width(&look.state) - ROW_TEXT_X).max(0),
            };
            if !m.stacked {
                tree.set_place(
                    row_widgets.title,
                    line_at(ROW_TEXT_X, TITLE_TOP, title_width),
                );
            }
            show_line(tree, row_widgets.title, &component.title, Token::Text);
            show_line(tree, row_widgets.state, &look.state, Token::Text);
            tree.set_shown(row_widgets.state, look.update.is_none() || m.stacked);
            match &look.update {
                Some(update) => {
                    if !m.stacked {
                        tree.set_place(
                            row_widgets.summary,
                            line_at(ROW_TEXT_X, SUMMARY_TOP, UPDATE_TEXT_END - ROW_TEXT_X),
                        );
                    }
                    show_line(tree, row_widgets.summary, &update.line, Token::Text);
                    tree.set_text(row_widgets.update, update.button);
                    tree.set_enabled(row_widgets.update, update.enabled);
                    tree.set_accessible_name(
                        row_widgets.update,
                        &format!("{} {}", update.action, component.title),
                    );
                }
                None => {
                    if !m.stacked {
                        tree.set_place(
                            row_widgets.summary,
                            line_at(ROW_TEXT_X, SUMMARY_TOP, STATE_END - ROW_TEXT_X),
                        );
                    }
                    show_line(
                        tree,
                        row_widgets.summary,
                        &component.summary,
                        Token::TextDim,
                    );
                }
            }
            tree.set_shown(row_widgets.update, look.update.is_some());
            tree.set_text(row_widgets.button, look.button);
            tree.set_enabled(row_widgets.button, look.enabled);
            tree.set_text(row_widgets.tip, look.reason);
            tree.set_shown(row_widgets.tip, !look.enabled);
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
        let note_top = anchor.map(|anchor| match anchor {
            Anchor::Store => STORE_NOTE_TOP,
            Anchor::Band => m.updates_band.1 + shift + m.updates_band.3,
            Anchor::Edition => m.edition_band.1 + shift + m.edition_band.3,
            Anchor::Row(row) => row_bottoms.get(row).copied().unwrap_or(next_top),
        });
        self.place_note(tree, note_top);
        let bottom = next_top + BOTTOM_MARGIN;
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
        let plan_shown = self.show_plan_lines(tree, top);
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
        let below_plan = PLAN_LINE_PITCH * plan_shown as i32;
        let mut cancel_shown = false;
        let (first_shown, second_shown, bar_shown, buttons_shown) = match &self.note {
            Note::Closed => (false, false, false, false),
            Note::Message { first, second, .. }
            | Note::Band { first, second }
            | Note::Store { first, second } => {
                show_line(tree, widgets.first, first, Token::Text);
                show_line(tree, widgets.second, second, Token::TextDim);
                (true, !second.is_empty(), false, false)
            }
            Note::StoreProgress => {
                let (done, total) = self
                    .read
                    .job
                    .as_ref()
                    .map_or((0, 0), |job| (job.done, job.total));
                let permille = done.saturating_mul(PERMILLE) / total.max(1);
                tree.set_progress(widgets.bar, permille.min(PERMILLE) as u16);
                tree.set_accessible_name(widgets.bar, "Downloading the component store");
                show_line(
                    tree,
                    widgets.first,
                    &messages::downloading_store(done, total),
                    Token::Text,
                );
                let cancelling = matches!(self.work, Work::DownloadingStore { cancelling: true });
                tree.set_text(widgets.cancel, "Cancel");
                tree.set_enabled(widgets.cancel, !cancelling);
                cancel_shown = true;
                (true, false, total > 0, false)
            }
            Note::Progress { name } if matches!(self.work, Work::Updating { .. }) => {
                let job = self.read.job.as_ref().filter(|job| job.name == *name);
                let (phase, done, total) = job.map_or(("copying", 0, 0), |job| {
                    (job.phase.as_str(), job.done, job.total)
                });
                let total = match (job, self.component(name)) {
                    (None, Some(component)) => component.size_bytes(),
                    _ => total,
                };
                let permille = done.saturating_mul(PERMILLE) / total.max(1);
                tree.set_progress(widgets.bar, permille.min(PERMILLE) as u16);
                tree.set_accessible_name(widgets.bar, &format!("Updating {}", self.title_of(name)));
                show_line(
                    tree,
                    widgets.first,
                    &messages::job_phase_line(phase, done, total),
                    Token::Text,
                );
                (true, false, true, false)
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
                let line = match self.install_finishing() {
                    true => messages::job_phase_line(FINISHING, received, total),
                    false => messages::copied_of(received, total),
                };
                show_line(tree, widgets.first, &line, Token::Text);
                (true, false, total > 0, false)
            }
            Note::Remove { .. } | Note::Install { .. } => {
                tree.set_text(widgets.cancel, "Cancel");
                tree.set_enabled(widgets.cancel, true);
                (true, true, false, true)
            }
        };
        let first_top = match &self.note {
            Note::Progress { .. } | Note::StoreProgress => BYTES_LINE,
            _ => FIRST_LINE,
        };
        let (note_x, note_width) = self.note_column();
        tree.set_place(widgets.first, line_at(note_x, top + first_top, note_width));
        tree.set_place(
            widgets.second,
            line_at(note_x, top + SECOND_LINE + below_plan, note_width),
        );
        tree.set_place(
            widgets.bar,
            at(note_x, top + BAR_TOP, note_width, BAR_HEIGHT),
        );
        let buttons_top = top + NOTE_BUTTONS_TOP + below_plan;
        match self.metrics.stacked {
            true => {
                let width = self.metrics.width;
                tree.set_place(
                    widgets.cancel,
                    at(PADDING, buttons_top, width, TOUCH_BUTTON),
                );
                tree.set_place(
                    widgets.confirm,
                    at(PADDING, buttons_top + TOUCH_BUTTON + 8, width, TOUCH_BUTTON),
                );
            }
            false => {
                tree.set_place(
                    widgets.cancel,
                    at(NOTE_CANCEL.0, buttons_top, NOTE_CANCEL.1, BUTTON_HEIGHT),
                );
                tree.set_place(
                    widgets.confirm,
                    at(NOTE_CONFIRM.0, buttons_top, NOTE_CONFIRM.1, BUTTON_HEIGHT),
                );
            }
        }
        tree.set_shown(widgets.first, first_shown);
        tree.set_shown(widgets.second, second_shown);
        tree.set_shown(widgets.bar, bar_shown);
        tree.set_shown(widgets.cancel, buttons_shown || cancel_shown);
        tree.set_shown(widgets.confirm, buttons_shown);
    }

    fn show_plan_lines(&self, tree: &mut WidgetTree<CAPACITY>, top: Option<i32>) -> usize {
        let plan = match (&self.note, top) {
            (
                Note::Install {
                    edition: Some(_),
                    plan,
                    ..
                },
                Some(_),
            ) => plan.as_slice(),
            _ => &[],
        };
        let shown = plan.len().min(PLAN_LINES);
        for (index, id) in self.widgets.plan_lines.iter().enumerate() {
            let Some(name) = plan.get(index).filter(|_| index < shown) else {
                tree.set_shown(*id, false);
                continue;
            };
            let more = plan.len() - shown;
            let line = match (index + 1 == shown, more) {
                (true, 1..) => format!("and {} more", more + 1),
                _ => match self.component(name) {
                    Some(component) => format!(
                        "{} · {}",
                        component.title,
                        messages::size_text(component.size_bytes())
                    ),
                    None => self.title_of(name),
                },
            };
            let line_top = top.unwrap_or(0) + SECOND_LINE + PLAN_LINE_PITCH * index as i32;
            let (note_x, note_width) = self.note_column();
            tree.set_place(*id, line_at(note_x, line_top, note_width));
            show_line(tree, *id, &line, Token::Text);
            tree.set_shown(*id, true);
        }
        shown
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
