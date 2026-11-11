//! What the picker shows and what choosing does (docs/ux/file-picker-v0.3.md sections 2, 3, 6,
//! 7, 9 and 10), apart from any window: the folder shown, its rows sorted and filtered, the
//! selection, and each step a person's choice takes. Typed text is untrusted: it is resolved
//! against the folder shown by its text alone, never written anywhere, and only a path the
//! person chose leaves the picker.

use crate::filter::Filter;
use crate::options::{Mode, Options};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;
use eggos_app::files::{Files, Path};
use eggos_files::clock;
use eggos_files::listing::{self, Entry, Listing};
use eggos_files::names::{joined, normalized, parent};
use eggos_runtime::errno::Errno;

/// The person's home, where the picker opens when the folder asked for cannot be shown.
pub const HOME: &[u8] = b"/home/root";
const ROOT: &[u8] = b"/";
const MOST_BACK_STEPS: usize = 64;

/// The places of the sidebar: their words and their folders.
pub const PLACES: [(&str, &[u8]); 7] = [
    ("Home", HOME),
    ("Desktop", b"/home/root/Desktop"),
    ("Documents", b"/home/root/Documents"),
    ("Downloads", b"/home/root/Downloads"),
    ("Host share", b"/media/host"),
    ("Work disk", b"/work"),
    ("System", ROOT),
];

/// A column the rows sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Modified,
}

impl Column {
    /// The column at `index` from the left.
    pub const fn at(index: usize) -> Self {
        match index {
            1 => Self::Size,
            2 => Self::Modified,
            _ => Self::Name,
        }
    }
}

/// One row: the listing's entry it shows and whether it is dim (a hidden name, or a file in
/// `folder` mode), which no click or arrow takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub entry: usize,
    pub dim: bool,
}

/// A notice the picker shows for 3 s (section 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    DoesNotExist(String),
    FolderDoesNotExist(String),
    CouldNotOpen(String, Errno),
    TypeAName,
    NotAFileName,
    PathTooLong,
    ReadOnly(String),
}

impl Notice {
    /// Its words.
    pub fn text(&self) -> String {
        let mut text = String::new();
        let _ = match self {
            Self::DoesNotExist(name) => write!(text, "{name} does not exist."),
            Self::FolderDoesNotExist(folder) => write!(text, "{folder} does not exist."),
            Self::CouldNotOpen(folder, error) => write!(
                text,
                "Could not open {folder} ({}).",
                String::from_utf8_lossy(error.message())
            ),
            Self::TypeAName => write!(text, "Type a name."),
            Self::NotAFileName => write!(text, "That is not a file name."),
            Self::PathTooLong => write!(text, "The path is too long."),
            Self::ReadOnly(folder) => write!(text, "Cannot save here: {folder} is read-only."),
        };
        text
    }
}

/// What a step of the person's left the picker doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing changed that the window must know beyond its rows.
    Nothing,
    /// Another folder is shown.
    Entered,
    Notice(Notice),
    /// `save` found the file there: the `Replace <name>?` panel asks before it is chosen.
    AskReplace(Path),
    /// The person chose these paths: the picker prints them and ends.
    Chosen(Vec<Path>),
}

/// How a click or key changes the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    Only,
    Toggle,
    Range,
}

/// `bytes` as text for the person: UTF-8 as it is, anything else replaced.
pub fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The end of `name`'s stem: the bytes before its last `.`, unless that `.` is its first byte.
pub fn stem_end(name: &[u8]) -> usize {
    match name.iter().rposition(|&byte| byte == b'.') {
        Some(0) | None => name.len(),
        Some(dot) => dot,
    }
}

/// The lines `pick` prints for `paths`: each path, then a new line.
pub fn output(paths: &[Path]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_bytes());
        out.push(b'\n');
    }
    out
}

fn is_folder(files: &mut impl Files, path: &[u8]) -> Option<bool> {
    files.status_following(path).ok().map(|status| status.kind == eggos_app::files::Kind::Directory)
}

/// See the module documentation.
#[derive(Debug, Clone)]
pub struct Picker {
    options: Options,
    working: Path,
    folder: Path,
    back: Vec<Path>,
    listing: Box<Listing>,
    rows: Vec<Row>,
    sort: (Column, bool),
    hidden: bool,
    filter: usize,
    selected: Vec<usize>,
    focus: Option<usize>,
    anchor: Option<usize>,
}

impl Picker {
    /// The picker `options` ask for, started in `working` (the app's current folder): at
    /// `--folder` (relative to `working`), else `working`; at home when that cannot be shown,
    /// and at `/` when home cannot either (section 10).
    pub fn new(files: &mut impl Files, options: Options, working: &[u8]) -> Self {
        let working = normalized(working)
            .filter(|path| path.as_bytes().first() == Some(&b'/'))
            .unwrap_or_else(|| Path::from(ROOT).unwrap_or_default());
        let filter = options.filter_index;
        let mut picker = Self {
            options,
            working,
            folder: working,
            back: Vec::new(),
            listing: Box::default(),
            rows: Vec::new(),
            sort: (Column::Name, false),
            hidden: false,
            filter,
            selected: Vec::new(),
            focus: None,
            anchor: None,
        };
        let asked = match picker.options.folder.clone() {
            Some(folder) => picker.resolve(&folder),
            None => Some(working),
        };
        let starts = [asked, Path::from(HOME), Path::from(ROOT)];
        for start in starts.into_iter().flatten() {
            if picker.show(files, start).is_ok() {
                break;
            }
        }
        picker
    }

    /// What the command line asked for.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// The folder shown.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether [`Self::back`] has somewhere to go.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether [`Self::up`] has somewhere to go.
    pub fn can_go_up(&self) -> bool {
        self.folder.as_bytes() != ROOT
    }

    /// The rows shown, in order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The column the rows sort by, and whether the order is reversed.
    pub fn sort(&self) -> (Column, bool) {
        self.sort
    }

    /// Whether hidden names are shown.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// The filter applied, from 0, when there are filters.
    pub fn filter(&self) -> usize {
        self.filter
    }

    /// The rows selected, in order.
    pub fn selected(&self) -> &[usize] {
        &self.selected
    }

    /// The row the keys are on.
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// Why the folder could not be read in full, if it could not.
    pub fn error(&self) -> Option<Errno> {
        self.listing.error()
    }

    /// The entry `row` shows.
    pub fn entry(&self, row: usize) -> Option<&Entry> {
        self.listing.get(self.rows.get(row)?.entry)
    }

    /// The name `row` shows.
    pub fn name(&self, row: usize) -> &[u8] {
        self.rows.get(row).map_or(&[], |row| self.listing.name(row.entry))
    }

    /// The text of `row`'s cell in `column`: the name, the size (none for a folder) or when it
    /// last changed.
    pub fn cell(&self, row: usize, column: Column) -> String {
        let Some(entry) = self.entry(row) else {
            return String::new();
        };
        match column {
            Column::Name => shown(self.name(row)),
            Column::Size if entry.kind.is_folder() => String::new(),
            Column::Size => String::from(listing::size_text(entry.size).as_str()),
            Column::Modified => String::from(clock::list_time(entry.time).as_str()),
        }
    }

    /// The words of the line shown when no row is: an empty folder, a filter that lets none
    /// through, or a folder that cannot be read; `None` while rows are shown.
    pub fn empty_text(&self) -> Option<String> {
        if !self.rows.is_empty() {
            return None;
        }
        if let Some(error) = self.listing.error() {
            return Some(Notice::CouldNotOpen(shown(self.folder.as_bytes()), error).text())
                .map(|text| text.replace("Could not open", "Cannot read"));
        }
        let files = (0..self.listing.len()).any(|entry| {
            self.listing.get(entry).is_some_and(|entry| !entry.kind.is_folder())
        });
        match (files, self.active_filter()) {
            (true, Some(filter)) => {
                let mut text = String::from("No files match ");
                text.push_str(&filter.description);
                Some(text)
            }
            _ => Some(String::from("This folder is empty")),
        }
    }

    fn active_filter(&self) -> Option<&Filter> {
        match self.options.mode {
            Mode::Folder => None,
            _ => self.options.filters.get(self.filter),
        }
    }

    fn resolve(&self, typed: &[u8]) -> Option<Path> {
        let mut path = Path::new();
        let fits = match typed {
            [b'/', ..] => path.append(typed),
            [b'~'] => path.append(HOME),
            [b'~', b'/', rest @ ..] => path.append(HOME) && path.append(b"/") && path.append(rest),
            _ => path.append(self.folder.as_bytes()) && path.append(b"/") && path.append(typed),
        };
        fits.then(|| normalized(path.as_bytes())).flatten()
    }

    fn show(&mut self, files: &mut impl Files, folder: Path) -> Result<(), Notice> {
        if is_folder(files, folder.as_bytes()) != Some(true) {
            return Err(Notice::FolderDoesNotExist(shown(folder.as_bytes())));
        }
        let mut listing = Box::<Listing>::default();
        listing.read_folder_showing(files, &folder, self.hidden);
        if let Some(error) = listing.error().filter(|_| listing.is_empty()) {
            return Err(Notice::CouldNotOpen(shown(folder.as_bytes()), error));
        }
        self.listing = listing;
        self.folder = folder;
        self.selected.clear();
        self.focus = None;
        self.anchor = None;
        self.arrange();
        Ok(())
    }

    fn enter(&mut self, files: &mut impl Files, folder: Path) -> Step {
        let left = self.folder;
        match self.show(files, folder) {
            Ok(()) => {
                if left != self.folder {
                    if self.back.len() == MOST_BACK_STEPS {
                        self.back.remove(0);
                    }
                    self.back.push(left);
                }
                Step::Entered
            }
            Err(notice) => Step::Notice(notice),
        }
    }

    /// Reads the folder shown again (F5), keeping the selected names selected.
    pub fn reload(&mut self, files: &mut impl Files) {
        let names: Vec<Vec<u8>> = self.selected.iter().map(|&row| self.name(row).to_vec()).collect();
        let folder = self.folder;
        if self.show(files, folder).is_err() {
            return;
        }
        for name in names {
            if let Some(row) = self.row_named(&name) {
                self.select(row, Select::Toggle);
            }
        }
    }

    /// Shows the folder `place` names (an index of [`PLACES`]).
    pub fn go_to_place(&mut self, files: &mut impl Files, place: usize) -> Step {
        match PLACES.get(place).and_then(|(_, folder)| Path::from(folder)) {
            Some(folder) => self.enter(files, folder),
            None => Step::Nothing,
        }
    }

    /// The place the folder shown is, if it is one.
    pub fn place(&self) -> Option<usize> {
        PLACES
            .iter()
            .position(|(_, folder)| *folder == self.folder.as_bytes())
    }

    /// Goes up one folder, selecting the one it left.
    pub fn up(&mut self, files: &mut impl Files) -> Step {
        if !self.can_go_up() {
            return Step::Nothing;
        }
        let left = self.folder;
        let Some(above) = Path::from(parent(left.as_bytes())) else {
            return Step::Nothing;
        };
        let step = self.enter(files, above);
        let name = left.as_bytes().rsplit(|&byte| byte == b'/').next().unwrap_or(&[]);
        if let Some(row) = self.row_named(name) {
            self.select(row, Select::Only);
        }
        step
    }

    /// Goes back to the folder shown before.
    pub fn back(&mut self, files: &mut impl Files) -> Step {
        let Some(folder) = self.back.pop() else {
            return Step::Nothing;
        };
        match self.show(files, folder) {
            Ok(()) => Step::Entered,
            Err(notice) => Step::Notice(notice),
        }
    }

    /// Sorts by `column`: ascending, or reversed when it is the column sorted by already.
    pub fn sort_by(&mut self, column: Column) {
        self.sort = match self.sort {
            (sorted, reversed) if sorted == column => (column, !reversed),
            _ => (column, false),
        };
        self.rearrange();
    }

    /// Sorts by `column` in the order given.
    pub fn sort_as(&mut self, column: Column, reversed: bool) {
        self.sort = (column, reversed);
        self.rearrange();
    }

    /// Shows or hides the names that start with `.`.
    pub fn set_hidden(&mut self, files: &mut impl Files, hidden: bool) {
        self.hidden = hidden;
        self.reload(files);
    }

    /// Applies filter `index`.
    pub fn set_filter(&mut self, index: usize) {
        if index < self.options.filters.len() {
            self.filter = index;
            self.rearrange();
        }
    }

    fn rearrange(&mut self) {
        let names: Vec<Vec<u8>> = self.selected.iter().map(|&row| self.name(row).to_vec()).collect();
        let focused = self.focus.map(|row| self.name(row).to_vec());
        self.arrange();
        self.selected.clear();
        for name in names {
            if let Some(row) = self.row_named(&name) {
                self.selected.push(row);
            }
        }
        self.selected.sort_unstable();
        self.focus = focused.and_then(|name| self.row_named(&name));
        self.anchor = self.focus;
    }

    fn arrange(&mut self) {
        let mode = self.options.mode;
        let filter = self.active_filter().cloned();
        let listing = &self.listing;
        let mut rows: Vec<Row> = (0..listing.len())
            .filter_map(|index| {
                let entry = listing.get(index)?;
                let name = listing.name(index);
                let folder = entry.kind.is_folder();
                let admitted = folder || filter.as_ref().is_none_or(|filter| filter.admits(name));
                admitted.then_some(Row {
                    entry: index,
                    dim: name.first() == Some(&b'.') || (mode == Mode::Folder && !folder),
                })
            })
            .collect();
        let (column, reversed) = self.sort;
        rows.sort_by(|first, second| {
            let (Some(one), Some(other)) = (listing.get(first.entry), listing.get(second.entry))
            else {
                return Ordering::Equal;
            };
            let by_name = || compare_names(listing.name(first.entry), listing.name(second.entry));
            let order = match column {
                Column::Name => by_name(),
                Column::Size => one.size.cmp(&other.size).then_with(by_name),
                Column::Modified => one.time.cmp(&other.time).then_with(by_name),
            };
            let order = if reversed { order.reverse() } else { order };
            other.kind.is_folder().cmp(&one.kind.is_folder()).then(order)
        });
        self.rows = rows;
    }

    fn row_named(&self, name: &[u8]) -> Option<usize> {
        (0..self.rows.len()).find(|&row| self.name(row) == name)
    }

    fn takes(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|row| !row.dim)
    }

    /// Changes the selection at `row` as `how` says; a dim row takes nothing. Without
    /// `--multiple`, every way selects only `row`.
    pub fn select(&mut self, row: usize, how: Select) {
        if !self.takes(row) {
            return;
        }
        let how = if self.options.multiple { how } else { Select::Only };
        match how {
            Select::Only => {
                self.selected.clear();
                self.selected.push(row);
                self.anchor = Some(row);
            }
            Select::Toggle => match self.selected.binary_search(&row) {
                Ok(at) => {
                    self.selected.remove(at);
                }
                Err(at) => self.selected.insert(at, row),
            },
            Select::Range => {
                let anchor = self.anchor.unwrap_or(row);
                let (low, high) = (anchor.min(row), anchor.max(row));
                self.selected = (low..=high).filter(|&row| self.takes(row)).collect();
            }
        }
        self.focus = Some(row);
    }

    /// Selects every row that takes it (`--multiple`, Mod+A).
    pub fn select_all(&mut self) {
        if self.options.multiple {
            self.selected = (0..self.rows.len()).filter(|&row| self.takes(row)).collect();
        }
    }

    /// Moves the focus `by` rows (negative is up), skipping dim rows, and selects there (with
    /// `extend`, from the anchor). Up or Down with nothing focused takes the first row.
    pub fn move_focus(&mut self, by: isize, extend: bool) {
        let count = self.rows.len() as isize;
        if count == 0 {
            return;
        }
        let step = if by < 0 { -1 } else { 1 };
        let mut at = match self.focus {
            None => -step,
            Some(row) => row as isize,
        };
        let mut left = by.unsigned_abs().max(1);
        let mut landed = self.focus;
        while left > 0 {
            at += step;
            if !(0..count).contains(&at) {
                break;
            }
            if self.takes(at as usize) {
                landed = Some(at as usize);
                left -= 1;
            }
        }
        if let Some(row) = landed {
            self.select(row, if extend { Select::Range } else { Select::Only });
        }
    }

    fn row_path(&self, row: usize) -> Option<Path> {
        joined(&self.folder, self.name(row))
    }

    /// The person double-clicked `row` or pressed Enter on it: a folder is entered; a file is
    /// chosen in `open` (with every selected file under `--multiple`) and is the name to save
    /// under in `save`.
    pub fn activate(&mut self, files: &mut impl Files, row: usize) -> Step {
        let (Some(entry), Some(path)) = (self.entry(row).copied(), self.row_path(row)) else {
            return Step::Nothing;
        };
        if self.rows[row].dim && !entry.kind.is_folder() {
            return Step::Nothing;
        }
        if entry.kind.is_folder() {
            return self.enter(files, path);
        }
        match self.options.mode {
            Mode::Open if self.options.multiple && self.selected.contains(&row) => self.primary(),
            Mode::Open => Step::Chosen(alloc::vec![path]),
            Mode::Save => self.save_at(files, path),
            Mode::Folder => Step::Nothing,
        }
    }

    /// The primary button (`Open`, `Save`, `Choose`); `name` is the name field's text in `save`.
    pub fn primary_with(&mut self, files: &mut impl Files, name: &[u8]) -> Step {
        match self.options.mode {
            Mode::Save => self.commit_name(files, name),
            _ => self.primary(),
        }
    }

    fn primary(&self) -> Step {
        let folders = match self.options.mode {
            Mode::Folder => true,
            Mode::Open => self.options.folders,
            Mode::Save => return Step::Nothing,
        };
        let chosen: Vec<Path> = self
            .selected
            .iter()
            .filter(|&&row| {
                self.entry(row).is_some_and(|entry| entry.kind.is_folder() == folders)
                    || (self.options.mode == Mode::Open && self.options.folders)
            })
            .filter(|&&row| self.takes(row))
            .filter_map(|&row| self.row_path(row))
            .collect();
        if !chosen.is_empty() {
            return Step::Chosen(chosen);
        }
        match self.options.mode {
            Mode::Folder => Step::Chosen(alloc::vec![self.folder]),
            _ => Step::Nothing,
        }
    }

    /// Whether the primary button does anything now: in `open`, only with a file (or, under
    /// `--folders`, a folder) selected.
    pub fn primary_enabled(&self) -> bool {
        match self.options.mode {
            Mode::Open => self.selected.iter().any(|&row| {
                self.entry(row)
                    .is_some_and(|entry| self.options.folders || !entry.kind.is_folder())
            }),
            Mode::Save | Mode::Folder => true,
        }
    }

    /// The person typed `text` in the path field and pressed Enter (section 7).
    pub fn type_path(&mut self, files: &mut impl Files, text: &[u8]) -> Step {
        if text.is_empty() {
            return Step::Nothing;
        }
        let Some(path) = self.resolve(text) else {
            return Step::Notice(Notice::PathTooLong);
        };
        match is_folder(files, path.as_bytes()) {
            Some(true) => self.enter(files, path),
            Some(false) => match self.options.mode {
                Mode::Open => Step::Chosen(alloc::vec![path]),
                Mode::Save => self.save_at(files, path),
                Mode::Folder => Step::Notice(Notice::FolderDoesNotExist(shown(path.as_bytes()))),
            },
            None => self.missing(files, path),
        }
    }

    fn missing(&mut self, files: &mut impl Files, path: Path) -> Step {
        let folder = parent(path.as_bytes());
        if is_folder(files, folder) != Some(true) {
            return Step::Notice(Notice::FolderDoesNotExist(shown(folder)));
        }
        let name = path.as_bytes().rsplit(|&byte| byte == b'/').next().unwrap_or(&[]);
        match self.options.mode {
            Mode::Save => self.save_at(files, path),
            Mode::Open => Step::Notice(Notice::DoesNotExist(shown(name))),
            Mode::Folder => Step::Notice(Notice::FolderDoesNotExist(shown(path.as_bytes()))),
        }
    }

    /// `save`: the person committed the name field's `name` (section 2).
    pub fn commit_name(&mut self, files: &mut impl Files, name: &[u8]) -> Step {
        if name.iter().all(|&byte| byte == b' ') {
            return Step::Notice(Notice::TypeAName);
        }
        if name.ends_with(b"/") || name.contains(&0) {
            return Step::Notice(Notice::NotAFileName);
        }
        let Some(path) = self.resolve(name) else {
            return Step::Notice(Notice::PathTooLong);
        };
        match is_folder(files, path.as_bytes()) {
            Some(true) => self.enter(files, path),
            Some(false) => self.save_at(files, path),
            None => self.missing(files, path),
        }
    }

    fn save_at(&mut self, files: &mut impl Files, path: Path) -> Step {
        let name = path.as_bytes().rsplit(|&byte| byte == b'/').next().unwrap_or(&[]);
        if name.is_empty() || name == b"." || name == b".." {
            return Step::Notice(Notice::NotAFileName);
        }
        let mut path = path;
        if let Some(extension) = self.options.extension.as_deref()
            && !name.contains(&b'.')
            && !(path.append(b".") && path.append(extension))
        {
            return Step::Notice(Notice::PathTooLong);
        }
        let folder = parent(path.as_bytes());
        let read_only = files.space(folder).is_ok_and(|space| space.read_only);
        if read_only || files.may_write(folder).is_err() {
            return Step::Notice(Notice::ReadOnly(shown(folder)));
        }
        match is_folder(files, path.as_bytes()) {
            Some(true) => self.enter(files, path),
            Some(false) if !self.options.overwrite => Step::AskReplace(path),
            _ => Step::Chosen(alloc::vec![path]),
        }
    }

    /// The working folder the picker started from.
    pub fn working(&self) -> &Path {
        &self.working
    }
}

/// Two names in the list's order: ASCII case ignored, then as bytes.
pub fn compare_names(first: &[u8], second: &[u8]) -> Ordering {
    let lower = |name: &[u8]| name.iter().map(u8::to_ascii_lowercase).collect::<Vec<u8>>();
    lower(first).cmp(&lower(second)).then_with(|| first.cmp(second))
}
