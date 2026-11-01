//! The Downloads window's widgets and the rows they show (docs/ux/downloads-v0.3.md sections
//! 2 to 9). It reads the records and acts through a [`DownloadSystem`], with the shared
//! `eggos-download-list` reading, judging, wording, cancelling and clearing them as
//! `download list`, `download cancel` and `download clear` do (D22).

use crate::address::{free_path, save_name, web_address};
use crate::cells::{self, RowCells, Shelf};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use eggos_download_list::{
    self as records, CancelError, MAX_DOWNLOADS, MAX_RECORD_BYTES, Record, SAVE_FOLDER, State,
    clear_refusal, newest_first, process_running, write_cleared,
};
use eggos_runtime::errno::Errno;
use eggos_toolkit::elements::{Action, Outcome};
use eggos_toolkit::input::{Input, key};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{
    Align, Column, Escape, Horizontal, Kind, LINE_HEIGHT, Length, Place, TABLE_MIN_COLUMN, Table,
};

/// The window's title, which the shell knows the app by.
pub const TITLE: &str = "Downloads";
/// The content's size when the window opens: seven whole rows.
pub const DEFAULT_SIZE: (u32, u32) = (640, 332);
/// The smallest content the user may resize it to: two whole rows.
pub const MINIMUM_SIZE: (u32, u32) = (400, 220);
/// The most widgets the window holds.
pub const CAPACITY: usize = 10;
/// How often the records are read again while the window shows, in milliseconds.
pub const REFRESH_MS: u32 = 1000;
/// The narrowest content laid out for a pointer; narrower is a phone's.
pub const NARROWEST_DESKTOP: i32 = 400;
/// The most bytes of `/proc/ps` read.
pub const PROCESSES_BYTES: usize = 32 * 1024;
/// F5's key code: refresh now.
pub const F5: u16 = 63;

const MINIMISED: u32 = 1;
const PLACEHOLDER: &str = "Web address (http:// or https://)";
const TABLE_NAME: &str = "Downloads";
const EMPTY: &str = "No downloads yet. Type a web address above, or use fetch.";
const UNREADABLE: &str = "The download list could not be read.";
const NOT_A_WEB_ADDRESS: &str = "Type a web address starting with http:// or https://.";
const NOT_ALLOWED: &str = "Not allowed: that download belongs to another user.";
const CLEAR_TIP: &str = "Remove done, failed and cancelled rows";
const CANCEL_TIP: &str = "Stop the selected download";
const CANCEL_KEY: &str = "Delete";
const MARGIN: i32 = 16;
const BUTTON_WIDTH: i32 = 96;
const CONTROL_HEIGHT: i32 = 28;
const TOUCH_HEIGHT: i32 = 40;
const SIZE_WIDTH: i32 = 152;
const STATE_WIDTH: i32 = 112;
const FROM_WIDTH: i32 = 112;
const PHONE_STATE_WIDTH: i32 = 120;
const LINE_BYTES: usize = 64;
const CUT: &str = "...";

const fn column(title: &'static str, width: i32, align: Align) -> Column {
    Column {
        title,
        width,
        min_width: TABLE_MIN_COLUMN,
        align,
        sortable: false,
    }
}

const DESKTOP_COLUMNS: [Column; 4] = [
    column("Name", 232, Align::Start),
    column("Size", SIZE_WIDTH, Align::End),
    column("State", STATE_WIDTH, Align::Start),
    column("From", FROM_WIDTH, Align::Start),
];
const PHONE_COLUMNS: [Column; 2] = [
    column("Name", 208, Align::Start),
    column("State", PHONE_STATE_WIDTH, Align::Start),
];

/// What the Downloads window asks of the system: the program makes the system calls, the host
/// tests a fake.
pub trait DownloadSystem {
    /// Puts the ids of the records in `/run/eggos/downloads` into `ids`, at most
    /// [`MAX_DOWNLOADS`]; none while the folder does not exist yet.
    fn record_ids(&mut self, ids: &mut Vec<u32>) -> Result<(), Errno>;
    /// Reads download `id`'s record into `text`: its length, or `None` when it cannot be read.
    fn read_record(&mut self, id: u32, text: &mut [u8]) -> Option<usize>;
    /// Reads `/proc/ps` into `text`: its length, or `None` when it cannot be read.
    fn read_processes(&mut self, text: &mut [u8]) -> Option<usize>;
    /// Sends `signal` to process `pid`; its errno on failure.
    fn send_signal(&mut self, pid: u32, signal: u32) -> Result<(), i32>;
    /// Removes download `id`'s record, never the downloaded file.
    fn remove_record(&mut self, id: u32) -> Result<(), Errno>;
    /// Whether anything is at `path`.
    fn exists(&mut self, path: &str) -> bool;
    /// Makes folder `path`; `EEXIST` when it is there already.
    fn make_folder(&mut self, path: &str) -> Result<(), Errno>;
    /// Starts `/bin/fetch -q <url> <path>`, which outlives the window.
    fn start_fetch(&mut self, url: &str, path: &str) -> Result<(), Errno>;
}

#[derive(Debug, Clone, Copy)]
struct Row {
    id: u32,
    started: u64,
    state: State,
    permille: Option<u16>,
    detail: Text<64>,
    phone_detail: Text<64>,
    cells: RowCells,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Awaiting {
    Start(String),
    Cancel(u32),
}

#[derive(Debug)]
struct Buffers {
    ids: Vec<u32>,
    record: Vec<u8>,
    processes: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    field: WidgetId,
    download: WidgetId,
    table: WidgetId,
    progress: WidgetId,
    line: WidgetId,
    clear: WidgetId,
    cancel: WidgetId,
}

/// The Downloads window: its widgets, the rows they show and what it waits for.
#[derive(Debug)]
pub struct DownloadsWindow {
    tree: WidgetTree<CAPACITY>,
    widgets: Widgets,
    shelf: Shelf,
    buffers: Buffers,
    rows: Vec<Row>,
    fresh: Vec<Row>,
    shown_cells: Vec<RowCells>,
    selected: Option<u32>,
    readable: bool,
    status: Option<Text<64>>,
    awaiting: Option<Awaiting>,
    phone: bool,
    minimised: bool,
    next_refresh_ms: u32,
}

fn add(
    tree: &mut WidgetTree<CAPACITY>,
    parent: Option<WidgetId>,
    kind: Kind,
) -> WidgetId {
    tree.add(parent, Place::fill(), kind)
        .expect("the Downloads window fits its tree")
}

fn written<const N: usize>(write: impl FnOnce(&mut dyn FnMut(&[u8]))) -> Text<N> {
    let mut text = Text::new();
    write(&mut |bytes: &[u8]| {
        text.push_str(&String::from_utf8_lossy(bytes));
    });
    text
}

fn line(full: &str) -> Text<64> {
    if full.len() <= LINE_BYTES {
        return Text::from(full);
    }
    let mut end = LINE_BYTES - CUT.len();
    while !full.is_char_boundary(end) {
        end -= 1;
    }
    let mut cut = Text::from(&full[..end]);
    cut.push_str(CUT);
    cut
}

fn sentence(parts: &[&str]) -> Text<64> {
    let mut full = Text::<512>::new();
    for part in parts {
        full.push_str(part);
    }
    line(full.as_str())
}

fn right(margin: i32, top: i32, width: i32, height: i32) -> Place {
    Place {
        horizontal: Horizontal::Right(margin),
        top,
        width: Length::Px(width),
        height: Length::Px(height),
    }
}

fn soonest(first: Option<u32>, second: Option<u32>, now_ms: u32) -> Option<u32> {
    let wait = |deadline: u32| deadline.wrapping_sub(now_ms) as i32;
    match (first, second) {
        (Some(a), Some(b)) => Some(if wait(a) <= wait(b) { a } else { b }),
        (deadline, None) | (None, deadline) => deadline,
    }
}

impl Row {
    fn of(id: u32, record: &Record<'_>) -> Self {
        let detail: Text<512> = written(|out| record.write_detail(out));
        let size: Text<64> = written(|out| record.write_size(out));
        let mut phone_detail = Text::<512>::from(size.as_str());
        phone_detail.push_str(" \u{b7} ");
        phone_detail.push_str(detail.as_str());
        Self {
            id,
            started: record.started,
            state: record.state,
            permille: match record.state {
                State::Downloading => record.permille(),
                _ => None,
            },
            detail: line(detail.as_str()),
            phone_detail: line(phone_detail.as_str()),
            cells: RowCells {
                name: written(|out| out(record.name())),
                size,
                state: written(|out| record.write_state(out)),
                short_state: written(|out| record.write_short_state(out)),
                host: written(|out| out(record.host())),
            },
        }
    }
}

fn judged_record<'b>(
    system: &mut impl DownloadSystem,
    record: &'b mut [u8],
    processes: Option<&[u8]>,
    id: u32,
) -> Option<Record<'b>> {
    let length = system.read_record(id, record)?;
    let record = Record::parse(&record[..length])?;
    Some(record.judged(processes.map(|processes| process_running(processes, id))))
}

fn read_rows(
    system: &mut impl DownloadSystem,
    buffers: &mut Buffers,
    starting: Option<&str>,
    rows: &mut Vec<Row>,
) -> Result<Option<u32>, Errno> {
    rows.clear();
    buffers.ids.clear();
    system.record_ids(&mut buffers.ids)?;
    let processes = system
        .read_processes(&mut buffers.processes)
        .map(|length| &buffers.processes[..length]);
    let mut started = None;
    for &id in buffers.ids.iter().take(MAX_DOWNLOADS) {
        let Some(record) = judged_record(system, &mut buffers.record, processes, id) else {
            continue;
        };
        if starting.is_some_and(|path| path.as_bytes() == record.path) {
            started = Some(id);
        }
        rows.push(Row::of(id, &record));
    }
    rows.sort_unstable_by(|first, second| {
        newest_first((first.started, first.id), (second.started, second.id))
    });
    Ok(started)
}

fn cancel_download(
    system: &mut impl DownloadSystem,
    buffers: &mut Buffers,
    id: u32,
) -> Result<(), CancelError> {
    let processes = system
        .read_processes(&mut buffers.processes)
        .map(|length| &buffers.processes[..length]);
    let record = judged_record(system, &mut buffers.record, processes, id);
    records::cancel(record.as_ref(), id, |pid, signal| {
        system.send_signal(pid, signal)
    })
}

fn clear_over(system: &mut impl DownloadSystem, buffers: &mut Buffers) -> usize {
    buffers.ids.clear();
    if system.record_ids(&mut buffers.ids).is_err() {
        return 0;
    }
    let processes = system
        .read_processes(&mut buffers.processes)
        .map(|length| &buffers.processes[..length]);
    let mut cleared = 0;
    for &id in buffers.ids.iter().take(MAX_DOWNLOADS) {
        let record = judged_record(system, &mut buffers.record, processes, id);
        if clear_refusal(record.as_ref()).is_some() {
            continue;
        }
        if matches!(system.remove_record(id), Ok(()) | Err(Errno::ENOENT)) {
            cleared += 1;
        }
    }
    cleared
}

impl DownloadsWindow {
    /// The window at its default size, showing the records `system` has now: the first row
    /// selected and the table focused, or the field focused when there is none.
    pub fn new(system: &mut impl DownloadSystem, now_ms: u32) -> Self {
        let shelf = Shelf::new();
        let mut tree = WidgetTree::new();
        let root = add(&mut tree, None, Kind::panel(Token::WindowBackground));
        let field = add(
            &mut tree,
            Some(root),
            Kind::text_field(PLACEHOLDER, Escape::Clear),
        );
        let download = add(&mut tree, Some(root), Kind::button("Download", true));
        let table = add(
            &mut tree,
            Some(root),
            Kind::table(TABLE_NAME, &DESKTOP_COLUMNS, 0, cells::cell, shelf.data()),
        );
        let progress = add(&mut tree, Some(root), Kind::Progress { permille: 0 });
        let line = add(&mut tree, Some(root), Kind::label("", Token::TextDim));
        let clear = add(&mut tree, Some(root), Kind::button("Clear", false));
        let cancel = add(&mut tree, Some(root), Kind::button("Cancel", false));
        let tip = Place::at(0, 0, Length::Content, Length::Content);
        for (owner, words, shortcut) in [(clear, CLEAR_TIP, ""), (cancel, CANCEL_TIP, CANCEL_KEY)] {
            tree.add(Some(owner), tip, Kind::tooltip(words, shortcut))
                .expect("the Downloads window fits its tree");
        }
        tree.set_default_button(download);
        tree.set_person_only(download, true);
        tree.set_person_only(cancel, true);
        tree.set_empty_text(table, EMPTY);
        for disabled in [download, clear, cancel] {
            tree.set_enabled(disabled, false);
        }
        tree.set_shown(progress, false);
        tree.set_shown(line, false);
        let mut window = Self {
            tree,
            widgets: Widgets {
                field,
                download,
                table,
                progress,
                line,
                clear,
                cancel,
            },
            shelf,
            buffers: Buffers {
                ids: Vec::new(),
                record: vec![0; MAX_RECORD_BYTES],
                processes: vec![0; PROCESSES_BYTES],
            },
            rows: Vec::new(),
            fresh: Vec::new(),
            shown_cells: Vec::new(),
            selected: None,
            readable: true,
            status: None,
            awaiting: None,
            phone: false,
            minimised: false,
            next_refresh_ms: now_ms,
        };
        let (width, height) = DEFAULT_SIZE;
        window.resize(width as i32, height as i32);
        window.refresh(system, now_ms);
        let focused = match window.rows.is_empty() {
            true => field,
            false => table,
        };
        window.tree.set_focus(Some(focused), now_ms);
        window
    }

    /// The window's widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The window's widgets, to draw or describe them.
    pub fn widgets(&self) -> &WidgetTree<CAPACITY> {
        &self.tree
    }

    fn places(&self, width: i32, height: i32) -> [(WidgetId, Place); 7] {
        let w = self.widgets;
        let px = Length::Px;
        match self.phone {
            false => [
                (
                    w.field,
                    Place::at(MARGIN, MARGIN, Length::Fill(120), px(CONTROL_HEIGHT)),
                ),
                (
                    w.download,
                    right(MARGIN, MARGIN, BUTTON_WIDTH, CONTROL_HEIGHT),
                ),
                (
                    w.table,
                    Place::at(MARGIN, 52, Length::Fill(MARGIN), Length::Fill(84)),
                ),
                (
                    w.progress,
                    Place::at(MARGIN, height - 72, Length::Fill(MARGIN), px(8)),
                ),
                (
                    w.line,
                    Place::at(MARGIN, height - 38, Length::Fill(232), px(LINE_HEIGHT)),
                ),
                (
                    w.clear,
                    right(120, height - 44, BUTTON_WIDTH, CONTROL_HEIGHT),
                ),
                (
                    w.cancel,
                    right(MARGIN, height - 44, BUTTON_WIDTH, CONTROL_HEIGHT),
                ),
            ],
            true => {
                let full = |top: i32, height: Length| {
                    Place::at(MARGIN, top, Length::Px(width - 2 * MARGIN), height)
                };
                [
                    (w.field, full(MARGIN, px(CONTROL_HEIGHT))),
                    (w.download, full(52, px(TOUCH_HEIGHT))),
                    (w.table, full(100, Length::Fill(156))),
                    (w.progress, full(height - 144, px(8))),
                    (w.line, full(height - 128, px(LINE_HEIGHT))),
                    (w.clear, full(height - 104, px(TOUCH_HEIGHT))),
                    (w.cancel, full(height - 56, px(TOUCH_HEIGHT))),
                ]
            }
        }
    }

    fn change_columns(&mut self) {
        let columns: &[Column] = match self.phone {
            true => &PHONE_COLUMNS,
            false => &DESKTOP_COLUMNS,
        };
        let data = self.shelf.data();
        self.tree.change(self.widgets.table, |kind| {
            if let Kind::Table(table) = kind {
                *table = Table {
                    selected: table.selected,
                    empty_text: table.empty_text,
                    ..Table::new(TABLE_NAME, columns, table.rows, cells::cell, data)
                };
            }
        });
        self.shelf.set(&self.shown_cells, self.phone);
    }

    /// Lays the window out for a content `width` by `height`: the desktop's layout, or the
    /// phone's below [`NARROWEST_DESKTOP`], with the `Name` column taking what is left.
    pub fn resize(&mut self, width: i32, height: i32) {
        let phone = width < NARROWEST_DESKTOP;
        if phone != self.phone {
            self.phone = phone;
            self.change_columns();
        }
        for (id, place) in self.places(width, height) {
            self.tree.set_place(id, place);
        }
        self.tree.resize(width, height);
        let table = self.widgets.table;
        let room = width - 2 * MARGIN;
        match self.phone {
            true => {
                self.tree
                    .set_column_width(table, 0, room - PHONE_STATE_WIDTH);
                self.tree.set_column_width(table, 1, PHONE_STATE_WIDTH);
            }
            false => self
                .tree
                .set_column_width(table, 0, room - SIZE_WIDTH - STATE_WIDTH - FROM_WIDTH),
        }
        self.show_line();
    }

    fn selected_row(&self) -> Option<&Row> {
        let id = self.selected?;
        self.rows.iter().find(|row| row.id == id)
    }

    fn settle(&mut self) {
        self.awaiting = None;
        self.status = None;
    }

    /// Reads the records and the processes again and shows them, keeping the selection on the
    /// same download.
    pub fn refresh(&mut self, system: &mut impl DownloadSystem, now_ms: u32) {
        self.next_refresh_ms = now_ms.wrapping_add(REFRESH_MS);
        let starting = match &self.awaiting {
            Some(Awaiting::Start(path)) => Some(path.as_str()),
            _ => None,
        };
        let read = read_rows(system, &mut self.buffers, starting, &mut self.fresh);
        self.readable = read.is_ok();
        if read.is_err() {
            self.fresh.clear();
        }
        let previous = self.tree.selected(self.widgets.table).unwrap_or(0);
        core::mem::swap(&mut self.rows, &mut self.fresh);
        if let Ok(Some(started)) = read {
            self.selected = Some(started);
            self.settle();
        }
        if let Some(Awaiting::Cancel(id)) = self.awaiting {
            let running = self
                .rows
                .iter()
                .any(|row| row.id == id && row.state == State::Downloading);
            if !running {
                self.settle();
            }
        }
        self.show_rows(previous, now_ms);
    }

    fn show_rows(&mut self, previous: usize, now_ms: u32) {
        let w = self.widgets;
        let count = self.rows.len();
        self.shown_cells.clear();
        self.shown_cells
            .extend(self.rows.iter().map(|row| row.cells));
        if self.shelf.rows() != self.shown_cells.as_slice() {
            self.shelf.set(&self.shown_cells, self.phone);
            self.tree.set_rows(w.table, count);
        }
        let kept = self
            .selected
            .and_then(|id| self.rows.iter().position(|row| row.id == id));
        let index = match (kept, self.selected) {
            (Some(index), _) => Some(index),
            _ if count == 0 => None,
            (None, None) => Some(0),
            (None, Some(_)) => Some(previous.min(count - 1)),
        };
        self.selected = index.map(|index| self.rows[index].id);
        if let Some(index) = index {
            self.tree.set_selected(w.table, index);
        }
        let empty_text = match self.readable {
            true => EMPTY,
            false => UNREADABLE,
        };
        let shown_empty = match self.tree.kind(w.table) {
            Some(Kind::Table(table)) => table.empty_text,
            _ => EMPTY,
        };
        if shown_empty != empty_text {
            self.tree.set_empty_text(w.table, empty_text);
        }
        self.show_selection();
        let stranded = self
            .tree
            .focus()
            .is_some_and(|focus| focus == w.table || !self.tree.is_enabled(focus));
        if count == 0 && stranded {
            self.tree.set_focus(Some(w.field), now_ms);
        }
    }

    fn show_selection(&mut self) {
        let w = self.widgets;
        let row = self.selected_row().copied();
        match row.and_then(|row| row.permille) {
            Some(permille) => {
                self.tree.set_progress(w.progress, permille);
                self.tree.set_shown(w.progress, true);
            }
            None => self.tree.set_shown(w.progress, false),
        }
        let over = self.rows.iter().any(|row| row.state.is_over());
        self.tree.set_enabled(w.clear, over);
        let downloading = row.is_some_and(|row| row.state == State::Downloading);
        self.tree.set_enabled(w.cancel, downloading);
        self.show_line();
    }

    fn show_line(&mut self) {
        let w = self.widgets;
        let shown = match (self.status, self.selected_row()) {
            (Some(status), _) => Some((status, Token::Text)),
            (None, Some(row)) if self.phone => Some((row.phone_detail, Token::TextDim)),
            (None, Some(row)) => Some((row.detail, Token::TextDim)),
            (None, None) => None,
        };
        let Some((words, ink)) = shown else {
            self.tree.set_shown(w.line, false);
            return;
        };
        self.tree.set_text(w.line, words.as_str());
        let inked =
            matches!(self.tree.kind(w.line), Some(Kind::Label { colour, .. }) if *colour == ink);
        if !inked {
            self.tree.change(w.line, |kind| {
                if let Kind::Label { colour, .. } = kind {
                    *colour = ink;
                }
            });
        }
        self.tree.set_shown(w.line, true);
    }

    fn say(&mut self, words: Text<64>) {
        self.status = Some(words);
        self.awaiting = None;
        self.show_line();
    }

    fn field_changed(&mut self) {
        let typed = self
            .tree
            .text_of(self.widgets.field)
            .is_some_and(|text| !text.trim().is_empty());
        self.tree.set_enabled(self.widgets.download, typed);
    }

    fn start(&mut self, system: &mut impl DownloadSystem, now_ms: u32) {
        let field = self.widgets.field;
        let typed = Text::<255>::from(self.tree.text_of(field).unwrap_or(""));
        let Some(address) = web_address(typed.as_str()) else {
            self.say(Text::from(NOT_A_WEB_ADDRESS));
            return;
        };
        let parent = SAVE_FOLDER
            .rsplit_once('/')
            .map_or(SAVE_FOLDER, |(parent, _)| parent);
        for folder in [parent, SAVE_FOLDER] {
            let _ = system.make_folder(folder);
        }
        let name = save_name(address);
        let Some(path) = free_path(SAVE_FOLDER, name, |path| system.exists(path)) else {
            self.say(sentence(&["Could not start downloading ", name, "."]));
            return;
        };
        let saved_as = path.rsplit('/').next().unwrap_or(name);
        if system.start_fetch(address, &path).is_err() {
            self.say(sentence(&["Could not start downloading ", saved_as, "."]));
            return;
        }
        let starting = sentence(&["Starting ", saved_as, "..."]);
        self.tree.set_text(field, "");
        self.field_changed();
        self.tree.set_focus(Some(field), now_ms);
        self.status = Some(starting);
        self.awaiting = Some(Awaiting::Start(path));
        self.show_line();
    }

    fn cancel(&mut self, system: &mut impl DownloadSystem) {
        let Some(row) = self.selected_row().copied() else {
            return;
        };
        if row.state != State::Downloading {
            return;
        }
        let name = row.cells.name;
        match cancel_download(system, &mut self.buffers, row.id) {
            Ok(()) => {
                self.status = Some(sentence(&["Cancelling ", name.as_str(), "..."]));
                self.awaiting = Some(Awaiting::Cancel(row.id));
                self.show_line();
            }
            Err(CancelError::NotAllowed) => self.say(Text::from(NOT_ALLOWED)),
            Err(CancelError::NoSuchDownload | CancelError::AlreadyEnded) => {
                self.say(sentence(&[name.as_str(), " has already ended."]))
            }
        }
    }

    fn clear(&mut self, system: &mut impl DownloadSystem, now_ms: u32) {
        let cleared = clear_over(system, &mut self.buffers);
        self.refresh(system, now_ms);
        self.say(written(|out| write_cleared(cleared, out)));
    }

    /// Acts on what an input or an agent's action did to the widgets.
    pub fn respond(&mut self, system: &mut impl DownloadSystem, response: Response, now_ms: u32) {
        let w = self.widgets;
        let focus = self.tree.focus();
        match response {
            Response::Activated(id) if id == w.download => self.start(system, now_ms),
            Response::Activated(id) if id == w.clear => self.clear(system, now_ms),
            Response::Activated(id) if id == w.cancel => self.cancel(system),
            Response::Edited(id) if id == w.field => self.field_changed(),
            Response::Chosen(id, index) if id == w.table => {
                self.selected = self.rows.get(index).map(|row| row.id);
                self.settle();
                self.show_selection();
            }
            Response::Unused {
                code: key::DELETE, ..
            } if focus == Some(w.table) => self.cancel(system),
            Response::Unused { code: F5, .. } => self.refresh(system, now_ms),
            Response::Unused {
                code: key::DOWN, ..
            } if focus == Some(w.field) => self.tree.set_focus(Some(w.table), now_ms),
            _ => {}
        }
    }

    /// Gives `input` to the widgets, noting when the window is minimised (refreshing stops)
    /// and restored (one refresh at once); what the program then answers, then hands to
    /// [`Self::respond`].
    pub fn input(&mut self, input: Input, time_ms: u32) -> Response {
        if let Input::StateChanged { state } = input {
            let was = self.minimised;
            self.minimised = state == MINIMISED;
            if was && !self.minimised {
                self.next_refresh_ms = time_ms;
            }
        }
        self.tree.handle(input, time_ms)
    }

    /// [`Self::input`], then [`Self::respond`].
    pub fn handle(&mut self, system: &mut impl DownloadSystem, input: Input, time_ms: u32) {
        let response = self.input(input, time_ms);
        self.respond(system, response, time_ms);
    }

    /// Does an agent's `action` to `element` (D161), with `text` for `set-text`.
    pub fn act(
        &mut self,
        system: &mut impl DownloadSystem,
        element: u32,
        action: Action,
        text: &str,
        now_ms: u32,
    ) -> Outcome {
        let (outcome, response) = self.tree.act(element, action, text, now_ms);
        self.respond(system, response, now_ms);
        outcome
    }

    /// When the window next wants [`Self::tick`]: the widgets' next change, and the next
    /// refresh unless the window is minimised.
    pub fn deadline(&self, now_ms: u32) -> Option<u32> {
        let refresh = (!self.minimised).then_some(self.next_refresh_ms);
        soonest(self.tree.next_deadline(now_ms), refresh, now_ms)
    }

    /// Lets the widgets' time pass, and refreshes when a refresh is due.
    pub fn tick(&mut self, system: &mut impl DownloadSystem, now_ms: u32) {
        let response = self.tree.tick(now_ms);
        if response != Response::Nothing {
            self.respond(system, response, now_ms);
        }
        if !self.minimised && now_ms.wrapping_sub(self.next_refresh_ms) as i32 >= 0 {
            self.refresh(system, now_ms);
        }
    }
}

#[cfg(test)]
mod tests;
