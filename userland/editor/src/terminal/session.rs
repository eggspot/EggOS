//! The Text Editor's terminal mode (D582, docs/ux/text-editor-terminal-v0.1.md): the document
//! and its editing, the screen laid out in rows (title, text, status, key hints), the prompts
//! in the status row, and the frame of bytes that draws only the rows that changed, all as
//! state over an [`Editor`] and [`Files`], so it is host-tested; the program
//! ([`super::program`]) moves the bytes and the terminal's settings.

use super::keys::{self, Decoded, Key, Press};
use crate::columns::TAB_COLUMNS;
use crate::document::Full;
use crate::editor::{Editor, Motion};
use crate::file::{self, OpenProblem, SaveProblem, folder_of};
use crate::find::{self, Wrap};
use crate::highlight::{self, Kind, Language, State};
use core::fmt::Write;
use eggos_app::files::{self, Files, Path};
use eggos_app::messages::Line;
use eggos_runtime::errno::Errno;

/// The most rows drawn; a taller terminal shows this many.
pub const MAX_ROWS: usize = 256;
/// The most columns drawn; a wider terminal shows this many.
pub const MAX_COLUMNS: usize = 512;
/// How long a lone `ESC` waits for more bytes before it is the Escape key, in milliseconds.
pub const ESCAPE_WAIT_MS: u64 = 50;
/// What starts the terminal mode's screen: the alternate screen, bracketed paste, the cursor.
pub const START: &[u8] = b"\x1b[?1049h\x1b[?2004h\x1b[?25h";
/// What gives the screen back.
pub const END: &[u8] = b"\x1b[0m\x1b[?2004l\x1b[?1049l\x1b[?25h";
/// The question that asks a terminal for its size.
pub const SIZE_QUERY: &[u8] = b"\x1b[999;999H\x1b[6n";

const UNTITLED: &str = "Untitled";
const TITLE_SUFFIX: &str = " - Text Editor";
const KEYS_TITLE: &str = "Keys - Text Editor";
const HOME: &[u8] = b"/home/root";
const NEW_FILE_PATH: &[u8] = b"/home/root/Untitled.txt";
const FIELD_BYTES: usize = 1024;
const ROW_BYTES: usize = 8 * MAX_COLUMNS + 64;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
const INPUT_BYTES: usize = 256;
const RESULT_LIMIT: usize = 999;
const GO_TO_DIGITS: usize = 7;
const KEY_COLUMN: usize = 16;
const FULL: &str = "The document is full (4 MB).";

const HINTS: [(&str, &str); 8] = [
    ("^S", "Save"),
    ("^Q", "Quit"),
    ("F1", "Keys"),
    ("^F", "Find"),
    ("^R", "Replace"),
    ("^G", "Go to"),
    ("^Z", "Undo"),
    ("^Y", "Redo"),
];

const KEYS_LIST: &[(&str, &str)] = &[
    ("Ctrl+S", "Save"),
    ("Alt+S", "Save as"),
    ("Ctrl+Q", "Quit"),
    ("Ctrl+O", "Open"),
    ("Ctrl+N", "New"),
    ("Ctrl+Z", "Undo"),
    ("Ctrl+Y", "Redo"),
    ("Ctrl+X", "Cut"),
    ("Ctrl+C", "Copy"),
    ("Ctrl+V", "Paste"),
    ("Ctrl+A", "Select all"),
    ("Ctrl+F", "Find"),
    ("F3", "Find next"),
    ("Shift+F3", "Find previous"),
    ("Ctrl+R", "Replace"),
    ("Ctrl+G", "Go to line"),
    ("Ctrl+^", "Mark: select from the caret"),
    ("Shift+arrows", "Select"),
    ("Ctrl+Left", "Previous word"),
    ("Ctrl+Right", "Next word"),
    ("Home, End", "Line start, line end"),
    ("Ctrl+Home", "Document start"),
    ("Ctrl+End", "Document end"),
    ("PgUp, PgDn", "Page up, page down"),
    ("Esc", "Close, clear the selection"),
    ("F1", "These keys"),
    ("Ctrl+L", "Redraw the screen"),
    ("Alt+N", "Line numbers"),
    ("Alt+Y", "Syntax colours"),
];

/// How the terminal draws: in colour or monochrome, and whether every character takes one
/// cell (the EggOS terminal) or wide characters two and combining marks none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub colour: bool,
    pub one_cell: bool,
}

impl Style {
    /// The style `TERM`, `NO_COLOR` and `TERM_PROGRAM` ask for.
    pub fn from_environment(term: Option<&[u8]>, no_color: Option<&[u8]>, program: Option<&[u8]>) -> Self {
        let coloured_terminal = term.is_some_and(|term| {
            [
                &b"xterm"[..],
                b"linux",
                b"screen",
                b"tmux",
                b"rxvt",
                b"vt100",
                b"vt220",
                b"ansi",
                b"eggos",
            ]
            .iter()
            .any(|prefix| term.starts_with(prefix))
                || term.windows(5).any(|window| window == b"color")
        });
        Self {
            colour: coloured_terminal && no_color.is_none_or(<[u8]>::is_empty),
            one_cell: program == Some(b"EggTerminal"),
        }
    }
}

/// What the terminal mode wants of its program after taking keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Go on.
    Going,
    /// The person quit: give the screen back.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Quit,
    New,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prompt {
    SaveChanges,
    SaveAs,
    ReplaceFile,
    Open,
    Find,
    ReplaceFind,
    ReplaceWith,
    ReplaceEach { replaced: usize },
    GoTo { asked: bool },
}

#[derive(Debug, Clone, Copy)]
struct Field {
    bytes: [u8; FIELD_BYTES],
    length: usize,
    caret: usize,
}

impl Field {
    const fn new() -> Self {
        Self {
            bytes: [0; FIELD_BYTES],
            length: 0,
            caret: 0,
        }
    }

    fn text(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    fn set(&mut self, text: &[u8]) {
        let length = text.len().min(FIELD_BYTES);
        self.bytes[..length].copy_from_slice(&text[..length]);
        self.length = length;
        self.caret = length;
    }

    fn insert(&mut self, character: char) {
        let mut encoded = [0u8; 4];
        let bytes = character.encode_utf8(&mut encoded).as_bytes();
        if self.length + bytes.len() > FIELD_BYTES {
            return;
        }
        self.bytes.copy_within(self.caret..self.length, self.caret + bytes.len());
        self.bytes[self.caret..self.caret + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
        self.caret += bytes.len();
    }

    fn boundary_before(&self, at: usize) -> usize {
        let mut before = at.saturating_sub(1);
        while before > 0 && self.bytes[before] & 0xc0 == 0x80 {
            before -= 1;
        }
        before
    }

    fn boundary_after(&self, at: usize) -> usize {
        let mut after = (at + 1).min(self.length);
        while after < self.length && self.bytes[after] & 0xc0 == 0x80 {
            after += 1;
        }
        after
    }

    fn remove(&mut self, start: usize, end: usize) {
        self.bytes.copy_within(end..self.length, start);
        self.length -= end - start;
        self.caret = start;
    }

    fn key(&mut self, press: Press) -> bool {
        match (press.key, press.control) {
            (Key::Left, _) => self.caret = self.boundary_before(self.caret),
            (Key::Right, _) => self.caret = self.boundary_after(self.caret),
            (Key::Home, _) => self.caret = 0,
            (Key::End, _) => self.caret = self.length,
            (Key::Backspace, _) if self.caret > 0 => {
                let start = self.boundary_before(self.caret);
                self.remove(start, self.caret);
            }
            (Key::Delete, _) if self.caret < self.length => {
                let end = self.boundary_after(self.caret);
                self.remove(self.caret, end);
            }
            (Key::Character('u'), true) => {
                self.length = 0;
                self.caret = 0;
            }
            (Key::Character(character), false) if !press.alt && !character.is_control() => {
                self.insert(character)
            }
            _ => return false,
        }
        true
    }
}

/// Bytes written into a frame, cut when the buffer is full.
struct Out<'o> {
    bytes: &'o mut [u8],
    length: usize,
    full: bool,
}

impl Out<'_> {
    fn put(&mut self, bytes: &[u8]) {
        match self.bytes.get_mut(self.length..self.length + bytes.len()) {
            Some(place) => {
                place.copy_from_slice(bytes);
                self.length += bytes.len();
            }
            None => self.full = true,
        }
    }

    fn number(&mut self, value: usize) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        let mut rest = value;
        loop {
            at -= 1;
            digits[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.put(&digits[at..]);
    }
}

/// A row being built: its bytes and how many cells they fill.
struct Row {
    bytes: [u8; ROW_BYTES],
    length: usize,
    cells: usize,
}

impl Row {
    const fn new() -> Self {
        Self {
            bytes: [0; ROW_BYTES],
            length: 0,
            cells: 0,
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        if let Some(place) = self.bytes.get_mut(self.length..self.length + bytes.len()) {
            place.copy_from_slice(bytes);
            self.length += bytes.len();
        }
    }

    fn text(&mut self, text: &str, limit: usize) {
        for character in text.chars() {
            if self.cells >= limit {
                return;
            }
            let mut encoded = [0u8; 4];
            self.raw(character.encode_utf8(&mut encoded).as_bytes());
            self.cells += 1;
        }
    }

    fn blanks(&mut self, count: usize) {
        for _ in 0..count {
            self.raw(b" ");
        }
        self.cells += count;
    }

    fn hash(&self) -> u64 {
        self.bytes[..self.length]
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    }
}

/// The cells `character` takes on a terminal of `style`: one on the EggOS terminal, else two
/// for an East Asian wide or fullwidth character, none for a combining mark, one otherwise.
pub fn cells_of(character: char, style: Style) -> usize {
    if style.one_cell {
        return 1;
    }
    let code = character as u32;
    const WIDE: [(u32, u32); 15] = [
        (0x1100, 0x115f),
        (0x2e80, 0x303e),
        (0x3041, 0x33ff),
        (0x3400, 0x4dbf),
        (0x4e00, 0x9fff),
        (0xa000, 0xa4cf),
        (0xac00, 0xd7a3),
        (0xf900, 0xfaff),
        (0xfe30, 0xfe4f),
        (0xff00, 0xff60),
        (0xffe0, 0xffe6),
        (0x1f300, 0x1f64f),
        (0x1f900, 0x1f9ff),
        (0x20000, 0x2fffd),
        (0x30000, 0x3fffd),
    ];
    const COMBINING: [(u32, u32); 5] = [
        (0x0300, 0x036f),
        (0x1ab0, 0x1aff),
        (0x1dc0, 0x1dff),
        (0x20d0, 0x20ff),
        (0xfe20, 0xfe2f),
    ];
    if COMBINING.iter().any(|&(low, high)| (low..=high).contains(&code)) {
        0
    } else if WIDE.iter().any(|&(low, high)| (low..=high).contains(&code)) {
        2
    } else {
        1
    }
}

fn sgr_of(kind: Kind) -> &'static [u8] {
    match kind {
        Kind::Plain => b"39",
        Kind::Function | Kind::Macro => b"93",
        Kind::Type | Kind::Key | Kind::Link => b"96",
        Kind::String => b"92",
        Kind::Number | Kind::Escape | Kind::Attribute => b"95",
        Kind::Keyword | Kind::Tag | Kind::Heading => b"94",
        Kind::Comment => b"90",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    title: Option<usize>,
    text_top: usize,
    text_rows: usize,
    status: Option<usize>,
    hints: Option<usize>,
}

fn layout(rows: usize) -> Layout {
    match rows {
        0 | 1 => Layout {
            title: None,
            text_top: 0,
            text_rows: 1,
            status: None,
            hints: None,
        },
        2 => Layout {
            title: None,
            text_top: 0,
            text_rows: 1,
            status: Some(1),
            hints: None,
        },
        3 => Layout {
            title: Some(0),
            text_top: 1,
            text_rows: 1,
            status: Some(2),
            hints: None,
        },
        rows => Layout {
            title: Some(0),
            text_top: 1,
            text_rows: rows - 3,
            status: Some(rows - 2),
            hints: Some(rows - 1),
        },
    }
}

fn digits(mut number: usize) -> usize {
    let mut count = 1;
    while number >= 10 {
        number /= 10;
        count += 1;
    }
    count
}

/// The terminal mode. See the module documentation.
pub struct TerminalEditor<'a> {
    editor: Editor<'a>,
    scratch: &'a mut [u8],
    path: Option<Path>,
    exists: bool,
    rows: usize,
    columns: usize,
    scroll_x: usize,
    scroll_y: usize,
    style: Style,
    language: Option<Language>,
    colours: bool,
    numbers: bool,
    mark: bool,
    prompt: Option<Prompt>,
    field: Field,
    find: Field,
    replacement: Field,
    match_case: bool,
    last_wrap: Wrap,
    then: Option<Then>,
    message: Option<Line>,
    keys_list: Option<usize>,
    quit: bool,
    input: [u8; INPUT_BYTES],
    input_length: usize,
    pasting: Option<usize>,
    drawn: [u64; MAX_ROWS],
    redraw: bool,
    state_memo: Option<(usize, State)>,
}

impl core::fmt::Debug for TerminalEditor<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TerminalEditor")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

impl<'a> TerminalEditor<'a> {
    /// The terminal mode over `editor`, with `scratch` (as large as the document's room) for
    /// a paste and a replace-all, on a terminal of `style`, `rows` by `columns`.
    pub fn new(editor: Editor<'a>, scratch: &'a mut [u8], style: Style, rows: usize, columns: usize) -> Self {
        Self {
            editor,
            scratch,
            path: None,
            exists: false,
            rows: rows.clamp(1, MAX_ROWS),
            columns: columns.clamp(1, MAX_COLUMNS),
            scroll_x: 0,
            scroll_y: 0,
            style,
            language: None,
            colours: style.colour,
            numbers: false,
            mark: false,
            prompt: None,
            field: Field::new(),
            find: Field::new(),
            replacement: Field::new(),
            match_case: false,
            last_wrap: Wrap::No,
            then: None,
            message: None,
            keys_list: None,
            quit: false,
            input: [0; INPUT_BYTES],
            input_length: 0,
            pasting: None,
            drawn: [0; MAX_ROWS],
            redraw: true,
            state_memo: None,
        }
    }

    /// The editing.
    pub fn editor(&self) -> &Editor<'a> {
        &self.editor
    }

    /// Opens the file at `path` (absolute), a new file when nothing is there; or an untitled
    /// document.
    pub fn open(&mut self, files: &mut impl Files, path: Option<&[u8]>) {
        match path {
            Some(path) => self.open_path(files, path, true),
            None => self.after_load(),
        }
    }

    fn name(&self) -> &str {
        self.path
            .as_ref()
            .map_or(UNTITLED, |path| core::str::from_utf8(files::file_name(path.as_bytes())).unwrap_or("?"))
    }

    /// The title row's text before it is fitted.
    pub fn title(&self) -> Line {
        let mut title = Line::new();
        if self.keys_list.is_some() {
            let _ = title.write_str(KEYS_TITLE);
            return title;
        }
        if self.editor.is_modified() {
            let _ = title.write_str("*");
        }
        let _ = title.write_str(self.name());
        let _ = title.write_str(TITLE_SUFFIX);
        title
    }

    /// The terminal changed size.
    pub fn resize(&mut self, rows: usize, columns: usize) {
        self.rows = rows.clamp(1, MAX_ROWS);
        self.columns = columns.clamp(1, MAX_COLUMNS);
        self.redraw = true;
        self.keep_caret_visible();
    }

    /// Draws every row again with the next frame (Ctrl+L, a resize).
    pub fn redraw_all(&mut self) {
        self.redraw = true;
    }

    /// Whether a lone `ESC` waits to be told apart from the start of a sequence.
    pub fn waits_for_escape(&self) -> bool {
        self.input[..self.input_length] == [0x1b]
    }

    /// No byte came within [`ESCAPE_WAIT_MS`] of a lone `ESC`: it was the Escape key.
    pub fn escape_timed_out(&mut self, files: &mut impl Files) -> Outcome {
        if self.waits_for_escape() {
            self.input_length = 0;
            self.press(files, Press::plain(Key::Escape));
        }
        self.outcome()
    }

    fn outcome(&self) -> Outcome {
        match self.quit {
            true => Outcome::Quit,
            false => Outcome::Going,
        }
    }

    /// Takes the bytes the terminal sent.
    pub fn feed(&mut self, files: &mut impl Files, bytes: &[u8]) -> Outcome {
        let mut rest = bytes;
        while !rest.is_empty() && !self.quit {
            let room = INPUT_BYTES - self.input_length;
            let taken = rest.len().min(room);
            self.input[self.input_length..self.input_length + taken].copy_from_slice(&rest[..taken]);
            self.input_length += taken;
            rest = &rest[taken..];
            self.take_input(files);
            if self.input_length == INPUT_BYTES {
                self.input_length = 0;
            }
        }
        self.outcome()
    }

    fn take_input(&mut self, files: &mut impl Files) {
        let mut at = 0;
        while at < self.input_length && !self.quit {
            let pending = &self.input[at..self.input_length];
            if let Some(filled) = self.pasting {
                let end = pending.windows(PASTE_END.len()).position(|window| window == PASTE_END);
                let take = match end {
                    Some(end) => end,
                    None => {
                        let keep = (1..PASTE_END.len())
                            .rev()
                            .find(|&length| pending.len() >= length && PASTE_END.starts_with(&pending[pending.len() - length..]))
                            .unwrap_or(0);
                        pending.len() - keep
                    }
                };
                let mut filled = filled;
                for index in 0..take {
                    let byte = self.input[at + index];
                    let next = self.input.get(at + index + 1).copied();
                    let kept = match (byte, next) {
                        (b'\r', Some(b'\n')) => None,
                        (b'\r', _) => Some(b'\n'),
                        (byte, _) => Some(byte),
                    };
                    if let Some(kept) = kept
                        && let Some(slot) = self.scratch.get_mut(filled)
                    {
                        *slot = kept;
                        filled += 1;
                    }
                }
                at += take;
                match end {
                    Some(_) => {
                        at += PASTE_END.len();
                        self.pasting = None;
                        self.finish_paste(filled);
                    }
                    None => {
                        self.pasting = Some(filled);
                        break;
                    }
                }
                continue;
            }
            if pending.starts_with(PASTE_START) {
                self.pasting = Some(0);
                at += PASTE_START.len();
                continue;
            }
            if let Some((rows, columns, used)) = size_report(pending) {
                at += used;
                self.resize(rows, columns);
                continue;
            }
            match keys::decode(pending) {
                Decoded::Press(press, used) => {
                    at += used;
                    self.press(files, press);
                }
                Decoded::Unknown(used) => at += used,
                Decoded::Incomplete => break,
            }
        }
        self.input.copy_within(at..self.input_length, 0);
        self.input_length -= at.min(self.input_length);
    }

    fn finish_paste(&mut self, filled: usize) {
        let text_length = filled.min(self.scratch.len());
        let text = &self.scratch[..text_length];
        let result = self.editor.insert_text(text);
        self.after_edit(result);
    }

    fn show(&mut self, text: &str) {
        let mut line = Line::new();
        let _ = line.write_str(text);
        self.message = Some(line);
    }

    fn after_edit(&mut self, result: Result<(), Full>) {
        if result.is_err() {
            self.show(FULL);
        }
        self.mark = false;
        let line = self.editor.caret_line().saturating_sub(1);
        if self.state_memo.is_some_and(|(memo, _)| memo > line) {
            self.state_memo = None;
        }
        self.update_language();
        self.keep_caret_visible();
    }

    fn update_language(&mut self) {
        let name = self.path.as_ref().map_or(&b""[..], |path| files::file_name(path.as_bytes()));
        let language = Language::detect(name, self.editor.document().line(0));
        if language != self.language {
            self.language = language;
            self.state_memo = None;
        }
    }

    fn after_load(&mut self) {
        self.scroll_x = 0;
        self.scroll_y = 0;
        self.state_memo = None;
        self.mark = false;
        self.update_language();
        self.redraw = true;
    }

    fn open_path(&mut self, files: &mut impl Files, path: &[u8], missing_is_new: bool) {
        let name = core::str::from_utf8(files::file_name(path)).unwrap_or("?");
        let Some(held) = Path::from(path) else {
            let mut line = Line::new();
            let _ = write!(line, "Could not open {name} (File name too long).");
            self.message = Some(line);
            return;
        };
        match file::check(files, path, self.editor.document().capacity()) {
            Err(OpenProblem::Missing) if missing_is_new => {
                self.editor.clear();
                self.path = Some(held);
                self.exists = false;
                self.after_load();
            }
            Err(problem) => self.message = Some(file::open_message(name, problem)),
            Ok(_) => {
                let read = file::read(files, path, self.editor.document_mut());
                self.editor.loaded();
                match read {
                    Ok(()) => {
                        self.path = Some(held);
                        self.exists = true;
                    }
                    Err(problem) => {
                        self.path = None;
                        self.exists = false;
                        self.message = Some(file::open_message(name, problem));
                    }
                }
                self.after_load();
            }
        }
    }

    fn write_to(&mut self, files: &mut impl Files, path: &[u8]) -> bool {
        let Some(held) = Path::from(path) else {
            self.message = Some(file::save_message(path, SaveProblem::Failed(Errno::ENAMETOOLONG)));
            return false;
        };
        match file::save(files, path, self.editor.document()) {
            Ok(()) => {
                self.editor.saved();
                self.path = Some(held);
                self.exists = true;
                self.update_language();
                true
            }
            Err(problem) => {
                self.message = Some(file::save_message(path, problem));
                false
            }
        }
    }

    /// Writes the document to `<path>.save` (`Untitled.save` in `working_folder` without a
    /// path), as a hangup with unsaved changes does; whether it was written.
    pub fn rescue(&mut self, files: &mut impl Files, working_folder: &[u8]) -> bool {
        if !self.editor.is_modified() {
            return true;
        }
        let mut target = match &self.path {
            Some(path) => *path,
            None => match Path::from(working_folder).and_then(|folder| folder.join(UNTITLED.as_bytes())) {
                Some(path) => path,
                None => return false,
            },
        };
        target.append(b".save") && file::save(files, target.as_bytes(), self.editor.document()).is_ok()
    }

    fn carry_out(&mut self, then: Then) {
        match then {
            Then::Quit => self.quit = true,
            Then::New => {
                self.editor.clear();
                self.path = None;
                self.exists = false;
                self.after_load();
            }
            Then::Open => self.open_prompt(),
        }
    }

    fn ask_first(&mut self, then: Then) {
        match self.editor.is_modified() {
            true => {
                self.then = Some(then);
                self.prompt = Some(Prompt::SaveChanges);
            }
            false => self.carry_out(then),
        }
    }

    fn save(&mut self, files: &mut impl Files, then: Option<Then>) {
        let Some(path) = self.path else {
            self.then = then;
            self.save_as_prompt();
            return;
        };
        if self.write_to(files, path.as_bytes())
            && let Some(then) = then
        {
            self.carry_out(then);
        }
    }

    fn save_as_prompt(&mut self) {
        let shown = self.path.map_or(Path::from(NEW_FILE_PATH), Some);
        self.field.set(shown.as_ref().map_or(&b""[..], Path::as_bytes));
        self.prompt = Some(Prompt::SaveAs);
    }

    fn open_prompt(&mut self) {
        let folder = self.path.as_ref().map_or(HOME, |path| folder_of(path.as_bytes()));
        let mut text = Path::from(folder).unwrap_or_default();
        if text.as_bytes() != b"/" {
            text.append(b"/");
        }
        self.field.set(text.as_bytes());
        self.prompt = Some(Prompt::Open);
    }

    fn typed_path(&self) -> Option<Path> {
        let typed = self.field.text();
        if typed.is_empty() {
            return None;
        }
        if typed.starts_with(b"/") {
            return Path::from(typed);
        }
        let folder = self.path.as_ref().map_or(HOME, |path| folder_of(path.as_bytes()));
        Path::from(folder).and_then(|folder| folder.join(typed))
    }

    fn select_hit(&mut self, at: usize) {
        let length = self.find.length;
        self.editor.select(at, at + length);
        self.keep_caret_visible();
    }

    fn search(&mut self, from: usize, forward: bool) -> bool {
        let needle = self.find.text();
        if needle.is_empty() {
            return false;
        }
        let hit = find::search(self.editor.document().bytes(), from, needle, self.match_case, forward);
        match hit {
            Some(hit) => {
                self.last_wrap = hit.wrap;
                self.select_hit(hit.at);
                true
            }
            None => {
                self.last_wrap = Wrap::No;
                false
            }
        }
    }

    fn find_result(&self) -> Line {
        let mut line = Line::new();
        let needle = self.find.text();
        if needle.is_empty() {
            return line;
        }
        let text = self.editor.document().bytes();
        let (start, end) = self.editor.selection();
        let selected = end - start == needle.len() && find::is_at(text, start, needle, self.match_case);
        let (index, total) = find::rank(text, start, needle, self.match_case);
        if !selected || total == 0 {
            let _ = line.write_str("Not found");
            return line;
        }
        match total > RESULT_LIMIT {
            true => {
                let _ = write!(line, "{index} of 999+");
            }
            false => {
                let _ = write!(line, "{index} of {total}");
            }
        }
        match self.last_wrap {
            Wrap::FromTop => {
                let _ = line.write_str(", from top");
            }
            Wrap::FromEnd => {
                let _ = line.write_str(", from end");
            }
            Wrap::No => {}
        }
        line
    }

    fn replace_next(&mut self, from: usize) -> bool {
        let needle = self.find.text();
        match find::next(self.editor.document().bytes(), from, needle, self.match_case) {
            Some(at) => {
                self.select_hit(at);
                true
            }
            None => false,
        }
    }

    fn end_replacing(&mut self, replaced: usize) {
        self.prompt = None;
        let mut line = Line::new();
        let _ = write!(line, "{replaced} replaced");
        self.message = Some(line);
    }

    fn replace_all_from(&mut self, from: usize) -> usize {
        let needle_length = self.find.length;
        let replacement_length = self.replacement.length;
        let text = self.editor.document().bytes();
        let mut built = 0;
        let mut at = 0;
        let mut count = 0;
        let mut caret = 0;
        let mut search_from = from;
        while let Some(found) = find::next(text, search_from, &self.find.bytes[..needle_length], self.match_case) {
            let kept = &text[at..found];
            if built + kept.len() + replacement_length > self.scratch.len() {
                break;
            }
            self.scratch[built..built + kept.len()].copy_from_slice(kept);
            built += kept.len();
            self.scratch[built..built + replacement_length]
                .copy_from_slice(&self.replacement.bytes[..replacement_length]);
            built += replacement_length;
            caret = built;
            at = found + needle_length;
            search_from = at;
            count += 1;
        }
        if count == 0 {
            return 0;
        }
        let rest = &text[at..];
        if built + rest.len() > self.scratch.len() {
            self.show(FULL);
            return 0;
        }
        self.scratch[built..built + rest.len()].copy_from_slice(rest);
        built += rest.len();
        let result = self.editor.replace_all(&self.scratch[..built]);
        if result.is_ok() {
            self.editor.place(caret);
        }
        self.after_edit(result);
        count
    }

    fn press(&mut self, files: &mut impl Files, press: Press) {
        self.message = None;
        if self.keys_list.is_some() {
            self.keys_list_key(press);
            return;
        }
        match self.prompt {
            Some(prompt) => self.prompt_key(files, prompt, press),
            None => self.document_key(files, press),
        }
    }

    fn keys_list_key(&mut self, press: Press) {
        let scroll = self.keys_list.unwrap_or(0);
        let rows = layout(self.rows).text_rows;
        let most = KEYS_LIST.len().saturating_sub(rows);
        let scroll = match press.key {
            Key::Escape | Key::Function(1) => {
                self.keys_list = None;
                self.redraw = true;
                return;
            }
            Key::Character('q') if press.control => {
                self.keys_list = None;
                self.redraw = true;
                return;
            }
            Key::Up => scroll.saturating_sub(1),
            Key::Down => scroll + 1,
            Key::PageUp => scroll.saturating_sub(rows.saturating_sub(1).max(1)),
            Key::PageDown => scroll + rows.saturating_sub(1).max(1),
            Key::Home => 0,
            Key::End => most,
            _ => scroll,
        };
        self.keys_list = Some(scroll.min(most));
    }

    fn document_key(&mut self, files: &mut impl Files, press: Press) {
        let extend = press.shift || self.mark;
        let page = layout(self.rows).text_rows.saturating_sub(1).max(1);
        let word = press.control || press.alt;
        match press.key {
            Key::Character(letter) if press.control => match letter {
                's' => self.save(files, None),
                'q' => self.ask_first(Then::Quit),
                'o' => self.ask_first(Then::Open),
                'n' => self.ask_first(Then::New),
                'z' => {
                    self.editor.undo();
                    self.after_edit(Ok(()));
                }
                'y' => {
                    self.editor.redo();
                    self.after_edit(Ok(()));
                }
                'x' => {
                    self.editor.cut();
                    self.after_edit(Ok(()));
                }
                'c' => {
                    self.editor.copy();
                    self.mark = false;
                }
                'v' => {
                    let result = self.editor.paste();
                    self.after_edit(result);
                }
                'a' => self.editor.select_all(),
                'f' => {
                    let text = self.find;
                    self.field = text;
                    self.prompt = Some(Prompt::Find);
                }
                'r' => {
                    let text = self.find;
                    self.field = text;
                    self.prompt = Some(Prompt::ReplaceFind);
                }
                'g' => {
                    self.field.set(b"");
                    self.prompt = Some(Prompt::GoTo { asked: false });
                }
                '^' | ' ' => {
                    self.mark = !self.mark;
                    if self.mark {
                        let caret = self.editor.caret();
                        self.editor.place(caret);
                    }
                }
                'l' => self.redraw = true,
                _ => {}
            },
            Key::Character(letter) if press.alt => match letter {
                's' => {
                    self.then = None;
                    self.save_as_prompt();
                }
                'n' => {
                    self.numbers = !self.numbers;
                    self.redraw = true;
                }
                'y' if self.style.colour => {
                    self.colours = !self.colours;
                    self.redraw = true;
                }
                _ => {}
            },
            Key::Character(character) if !character.is_control() => {
                let result = self.editor.type_character(character);
                self.after_edit(result);
            }
            Key::Enter => {
                let result = self.editor.type_character('\n');
                self.after_edit(result);
            }
            Key::Tab if !press.shift => {
                let result = self.editor.type_character('\t');
                self.after_edit(result);
            }
            Key::Backspace => {
                self.editor.backspace();
                self.after_edit(Ok(()));
            }
            Key::Delete => {
                self.editor.delete();
                self.after_edit(Ok(()));
            }
            Key::Left if word => self.go(Motion::WordLeft, extend),
            Key::Right if word => self.go(Motion::WordRight, extend),
            Key::Home if word => self.go(Motion::DocumentStart, extend),
            Key::End if word => self.go(Motion::DocumentEnd, extend),
            Key::Left => self.go(Motion::Left, extend),
            Key::Right => self.go(Motion::Right, extend),
            Key::Up => self.go(Motion::Up, extend),
            Key::Down => self.go(Motion::Down, extend),
            Key::Home => self.go(Motion::LineStart, extend),
            Key::End => self.go(Motion::LineEnd, extend),
            Key::PageUp => self.go(Motion::PageUp(page), extend),
            Key::PageDown => self.go(Motion::PageDown(page), extend),
            Key::Escape => {
                self.mark = false;
                let caret = self.editor.caret();
                self.editor.place(caret);
            }
            Key::Function(1) => {
                self.keys_list = Some(0);
                self.redraw = true;
            }
            Key::Function(3) => {
                if self.find.length == 0 {
                    let text = self.find;
                    self.field = text;
                    self.prompt = Some(Prompt::Find);
                    return;
                }
                let (start, end) = self.editor.selection();
                match press.shift {
                    true => self.search(start, false),
                    false => self.search(end, true),
                };
            }
            _ => {}
        }
    }

    fn go(&mut self, motion: Motion, extend: bool) {
        self.editor.go(motion, extend);
        self.keep_caret_visible();
    }

    fn prompt_key(&mut self, files: &mut impl Files, prompt: Prompt, press: Press) {
        let cancel = press.key == Key::Escape || press == Press::control('c');
        let enter = press.key == Key::Enter;
        let letter = match press.key {
            Key::Character(letter) if !press.control && !press.alt => Some(letter.to_ascii_lowercase()),
            _ => None,
        };
        match prompt {
            Prompt::SaveChanges => match (cancel, enter, letter) {
                (true, _, _) => {
                    self.prompt = None;
                    self.then = None;
                }
                (_, true, _) | (_, _, Some('y')) => {
                    self.prompt = None;
                    let then = self.then.take();
                    self.save(files, then);
                }
                (_, _, Some('n')) => {
                    self.prompt = None;
                    if let Some(then) = self.then.take() {
                        self.carry_out(then);
                    }
                }
                _ => {}
            },
            Prompt::SaveAs => {
                if cancel {
                    self.prompt = None;
                    self.then = None;
                } else if enter {
                    let Some(path) = self.typed_path() else {
                        return;
                    };
                    let other = self.path.as_ref().is_none_or(|held| held.as_bytes() != path.as_bytes());
                    if other && files.status(path.as_bytes()).is_ok() {
                        self.prompt = Some(Prompt::ReplaceFile);
                        return;
                    }
                    self.prompt = None;
                    if self.write_to(files, path.as_bytes())
                        && let Some(then) = self.then.take()
                    {
                        self.carry_out(then);
                    }
                } else {
                    self.field.key(press);
                }
            }
            Prompt::ReplaceFile => match (cancel, enter, letter) {
                (_, true, _) | (_, _, Some('y')) => {
                    self.prompt = None;
                    if let Some(path) = self.typed_path()
                        && self.write_to(files, path.as_bytes())
                        && let Some(then) = self.then.take()
                    {
                        self.carry_out(then);
                    }
                }
                (true, _, _) | (_, _, Some('n')) => self.prompt = Some(Prompt::SaveAs),
                _ => {}
            },
            Prompt::Open => {
                if cancel {
                    self.prompt = None;
                } else if enter {
                    let Some(path) = self.typed_path() else {
                        return;
                    };
                    self.open_path(files, path.as_bytes(), false);
                    if self.message.is_none() {
                        self.prompt = None;
                    }
                } else {
                    self.field.key(press);
                }
            }
            Prompt::Find => {
                if cancel {
                    self.prompt = None;
                    return;
                }
                let (start, end) = self.editor.selection();
                match press.key {
                    Key::Enter | Key::Down => {
                        self.search(end, true);
                    }
                    Key::Up => {
                        self.search(start, false);
                    }
                    Key::Character('c') if press.alt => {
                        self.match_case = !self.match_case;
                        self.search(start, true);
                    }
                    _ => {
                        if self.field.key(press) {
                            self.find = self.field;
                            self.search(start, true);
                        }
                    }
                }
            }
            Prompt::ReplaceFind => {
                if cancel {
                    self.prompt = None;
                } else if enter {
                    if self.field.length > 0 {
                        self.find = self.field;
                        let text = self.replacement;
                        self.field = text;
                        self.prompt = Some(Prompt::ReplaceWith);
                    }
                } else if self.field.key(press) {
                    self.find = self.field;
                    let (start, _) = self.editor.selection();
                    self.search(start, true);
                }
            }
            Prompt::ReplaceWith => {
                if cancel {
                    self.prompt = None;
                } else if enter {
                    self.replacement = self.field;
                    let (start, _) = self.editor.selection();
                    match self.replace_next(start) {
                        true => self.prompt = Some(Prompt::ReplaceEach { replaced: 0 }),
                        false => {
                            let text = self.find;
                            self.field = text;
                            self.prompt = Some(Prompt::ReplaceFind);
                            self.show("Not found");
                        }
                    }
                } else {
                    self.field.key(press);
                }
            }
            Prompt::ReplaceEach { replaced } => match (cancel, enter, letter) {
                (true, _, _) => self.end_replacing(replaced),
                (_, true, _) | (_, _, Some('y')) => {
                    let length = self.replacement.length;
                    let result = self.editor.insert_text(&self.replacement.bytes[..length]);
                    self.after_edit(result);
                    let caret = self.editor.caret();
                    match self.replace_next(caret) {
                        true => self.prompt = Some(Prompt::ReplaceEach { replaced: replaced + 1 }),
                        false => self.end_replacing(replaced + 1),
                    }
                }
                (_, _, Some('n')) => {
                    let (_, end) = self.editor.selection();
                    if !self.replace_next(end) {
                        self.end_replacing(replaced);
                    }
                }
                (_, _, Some('a')) => {
                    let (start, _) = self.editor.selection();
                    let all = self.replace_all_from(start);
                    self.end_replacing(replaced + all);
                }
                _ => {}
            },
            Prompt::GoTo { .. } => {
                if cancel {
                    self.prompt = None;
                } else if enter {
                    let count = self.editor.document().line_count();
                    let wanted = core::str::from_utf8(self.field.text())
                        .ok()
                        .and_then(|text| text.parse::<usize>().ok())
                        .filter(|line| (1..=count).contains(line));
                    match wanted {
                        Some(line) => {
                            let start = self.editor.document().line_start(line - 1);
                            self.editor.place(start);
                            let rows = layout(self.rows).text_rows;
                            self.scroll_y = (line - 1).saturating_sub(rows / 2);
                            self.prompt = None;
                            self.keep_caret_visible();
                        }
                        None => self.prompt = Some(Prompt::GoTo { asked: true }),
                    }
                } else {
                    let digit = matches!(press.key, Key::Character(character) if character.is_ascii_digit());
                    let typing = matches!(press.key, Key::Character(_));
                    if (!typing || (digit && self.field.length < GO_TO_DIGITS)) || press.control {
                        self.field.key(press);
                    }
                }
            }
        }
    }

    fn gutter(&self) -> usize {
        if !self.numbers {
            return 0;
        }
        let width = digits(self.editor.document().line_count()).max(2) + 1;
        match self.columns >= 2 + width + 8 {
            true => width,
            false => 0,
        }
    }

    fn text_width(&self) -> usize {
        self.columns.saturating_sub(self.gutter()).max(1)
    }

    fn cells_before(&self, line: &[u8], at: usize) -> usize {
        let mut cells = 0;
        for character in core::str::from_utf8(&line[..at.min(line.len())]).unwrap_or("").chars() {
            cells = self.advance(cells, character);
        }
        cells
    }

    fn advance(&self, cells: usize, character: char) -> usize {
        match character {
            '\t' => (cells / TAB_COLUMNS + 1) * TAB_COLUMNS,
            character if character.is_control() => cells + 1,
            character => cells + cells_of(character, self.style),
        }
    }

    fn keep_caret_visible(&mut self) {
        let rows = layout(self.rows).text_rows.max(1);
        let line = self.editor.caret_line();
        if line < self.scroll_y {
            self.scroll_y = line;
        } else if line >= self.scroll_y + rows {
            self.scroll_y = line + 1 - rows;
        }
        let document = self.editor.document();
        let start = document.line_start(line);
        let caret_cells = self.cells_before(document.line(line), self.editor.caret() - start);
        let width = self.text_width();
        let quarter = width / 4;
        if caret_cells + 1 > self.scroll_x + width.saturating_sub(1) {
            self.scroll_x = (caret_cells + 1 + quarter).saturating_sub(width);
        } else if caret_cells < self.scroll_x + usize::from(self.scroll_x > 0) {
            self.scroll_x = caret_cells.saturating_sub(quarter);
        }
    }

    fn state_at(&mut self, line: usize) -> State {
        let Some(language) = self.language.filter(|_| self.colours) else {
            return State::Normal;
        };
        let (mut at, mut state) = match self.state_memo {
            Some((memo, state)) if memo <= line => (memo, state),
            _ => (0, State::Normal),
        };
        while at < line {
            state = highlight::line(language, self.editor.document().line(at), state).1;
            at += 1;
        }
        self.state_memo = Some((line, state));
        state
    }

    fn sgr(row: &mut Row, codes: &[u8]) {
        row.raw(b"\x1b[");
        row.raw(codes);
        row.raw(b"m");
    }

    fn text_row(&self, row: &mut Row, line_index: usize, state: State) -> State {
        let document = self.editor.document();
        let gutter = self.gutter();
        let width = self.text_width();
        if line_index >= document.line_count() {
            return state;
        }
        if gutter > 0 {
            let number = line_index + 1;
            let colour = self.style.colour && line_index != self.editor.caret_line();
            if colour {
                Self::sgr(row, b"90");
            }
            row.blanks(gutter - 1 - digits(number).min(gutter - 1));
            let mut text = Line::new();
            let _ = write!(text, "{number}");
            row.text(text.as_str(), gutter - 1);
            if colour {
                Self::sgr(row, b"39");
            }
            row.blanks(1);
        }
        let line = document.line(line_index);
        let start = document.line_start(line_index);
        let (selection_start, selection_end) = self.editor.selection();
        let (runs, next_state) = match self.language.filter(|_| self.colours) {
            Some(language) => {
                let (runs, next) = highlight::line(language, line, state);
                (Some(runs), next)
            }
            None => (None, state),
        };
        let text = core::str::from_utf8(line).unwrap_or("");
        let total = text.chars().fold(0, |cells, character| self.advance(cells, character));
        let mut cells = 0;
        let mut drawn = 0;
        let mut current_kind = Kind::Plain;
        let mut inverse = false;
        let left_mark = self.scroll_x > 0 && total > 0;
        let right_mark = total > self.scroll_x + width;
        let last = width - usize::from(right_mark);
        for (at, character) in text.char_indices() {
            let next = self.advance(cells, character);
            let first_cell = cells;
            cells = next;
            if next <= self.scroll_x {
                continue;
            }
            if first_cell < self.scroll_x {
                let shown = next - self.scroll_x;
                row.blanks(shown.min(last.saturating_sub(drawn)));
                drawn += shown.min(last.saturating_sub(drawn));
                continue;
            }
            let span = next - first_cell;
            if drawn + span > last {
                break;
            }
            let offset = start + at;
            let selected = offset >= selection_start && offset < selection_end;
            let kind = runs.as_ref().map_or(Kind::Plain, |runs| runs.kind_at(at));
            if selected != inverse {
                match selected {
                    true => Self::sgr(row, b"39;7"),
                    false => {
                        Self::sgr(row, b"27");
                        current_kind = Kind::Plain;
                    }
                }
                inverse = selected;
            }
            if !selected && self.style.colour && kind != current_kind {
                Self::sgr(row, sgr_of(kind));
                current_kind = kind;
            }
            if drawn == 0 && left_mark {
                if self.style.colour {
                    Self::sgr(row, b"90");
                    current_kind = Kind::Comment;
                }
                row.raw(b"<");
                row.cells += 1;
                drawn += 1;
                if span > 1 {
                    row.blanks(span - 1);
                    drawn += span - 1;
                }
                continue;
            }
            match character {
                '\t' => row.blanks(span),
                character if character.is_control() => {
                    row.raw(b"?");
                    row.cells += 1;
                }
                character => {
                    let mut encoded = [0u8; 4];
                    row.raw(character.encode_utf8(&mut encoded).as_bytes());
                    row.cells += span;
                }
            }
            drawn += span;
        }
        if inverse {
            Self::sgr(row, b"27");
        }
        let line_end = start + line.len();
        let break_selected = line_index + 1 < document.line_count()
            && line_end >= selection_start
            && line_end < selection_end
            && !right_mark
            && drawn < width;
        if break_selected {
            Self::sgr(row, b"39;7");
            row.blanks(1);
            Self::sgr(row, b"27");
            drawn += 1;
        }
        if right_mark {
            row.blanks(last.saturating_sub(drawn));
            if self.style.colour {
                Self::sgr(row, b"90");
            }
            row.raw(b">");
            row.cells += 1;
        }
        Self::sgr(row, b"0");
        next_state
    }

    fn centred(row: &mut Row, text: &str, columns: usize) {
        let mut shown = Line::new();
        let characters = text.chars().count();
        let room = columns.saturating_sub(2);
        if characters <= room {
            let _ = shown.write_str(text);
        } else {
            let without = text.strip_suffix(TITLE_SUFFIX).unwrap_or(text);
            if without.chars().count() <= room {
                let _ = shown.write_str(without);
            } else {
                for character in without.chars().take(room.saturating_sub(3)) {
                    let _ = shown.write_char(character);
                }
                let _ = shown.write_str("...");
            }
        }
        let length = shown.as_str().chars().count().min(columns);
        let left = (columns - length) / 2;
        Self::sgr(row, b"7");
        row.blanks(left);
        row.text(shown.as_str(), columns);
        row.blanks(columns - left - length);
        Self::sgr(row, b"0");
    }

    fn status_row(&self, row: &mut Row) {
        let columns = self.columns;
        Self::sgr(row, b"7");
        row.blanks(1.min(columns));
        if let Some(prompt) = self.prompt {
            self.prompt_row(row, prompt);
            row.blanks(columns.saturating_sub(row.cells));
            Self::sgr(row, b"0");
            return;
        }
        if self.keys_list.is_some() {
            row.text("Keys", columns);
            row.blanks(columns.saturating_sub(row.cells));
            Self::sgr(row, b"0");
            return;
        }
        let mut left = Line::new();
        match &self.message {
            Some(message) => {
                let _ = left.write_str(message.as_str());
            }
            None => {
                let (line, column) = self.editor.line_and_column();
                let _ = write!(left, "Ln {line}, Col {column}");
                let selected = self.editor.selected_characters();
                if selected > 0 {
                    let dot = if self.style.colour { "\u{b7}" } else { "-" };
                    let _ = write!(left, " {dot} {selected} selected");
                }
            }
        }
        let language = self.language.map_or("Plain Text", Language::name);
        let state = match (self.editor.is_modified(), self.exists, self.path.is_some()) {
            (true, _, _) => "Modified",
            (false, false, true) => "New file",
            _ => "",
        };
        let ending = self.editor.document().ending().label();
        let mut parts: [&str; 3] = [language, state, ending];
        let room = columns.saturating_sub(2);
        let left_cells = left.as_str().chars().count();
        let right_cells = |parts: &[&str; 3]| -> usize {
            let shown: usize = parts.iter().filter(|part| !part.is_empty()).map(|part| part.chars().count()).sum();
            let gaps = parts.iter().filter(|part| !part.is_empty()).count().saturating_sub(1) * 2;
            shown + gaps
        };
        for index in 0..3 {
            if left_cells + 2 + right_cells(&parts) <= room {
                break;
            }
            parts[index] = "";
        }
        let right = right_cells(&parts);
        let shown_left = match left_cells + usize::from(right > 0) * 2 + right <= room {
            true => left.as_str(),
            false => {
                let text = left.as_str();
                let cut = text.find(" \u{b7}").or_else(|| text.find(" -")).unwrap_or(text.len());
                let text = &text[..cut];
                match text.chars().count() + right <= room {
                    true => text,
                    false => text.split(',').next().unwrap_or(text),
                }
            }
        };
        row.text(shown_left, room);
        let used = row.cells;
        let wanted = columns.saturating_sub(1).saturating_sub(right);
        row.blanks(wanted.saturating_sub(used));
        let mut first = true;
        for part in parts.iter().filter(|part| !part.is_empty()) {
            if !first {
                row.blanks(2);
            }
            first = false;
            row.text(part, columns.saturating_sub(1));
        }
        row.blanks(columns.saturating_sub(row.cells));
        Self::sgr(row, b"0");
    }

    fn prompt_label(&self, prompt: Prompt) -> Line {
        let mut label = Line::new();
        let name = self.name();
        let _ = match prompt {
            Prompt::SaveChanges => write!(label, "Save changes to {name}?"),
            Prompt::SaveAs => label.write_str("Save as: "),
            Prompt::ReplaceFile => {
                let typed = self.typed_path();
                let typed_name = typed
                    .as_ref()
                    .map_or("?", |path| core::str::from_utf8(files::file_name(path.as_bytes())).unwrap_or("?"));
                let folder = typed
                    .as_ref()
                    .map_or("/", |path| core::str::from_utf8(folder_of(path.as_bytes())).unwrap_or("/"));
                write!(label, "Replace {typed_name}? A file with that name already exists in {folder}.")
            }
            Prompt::Open => label.write_str("Open: "),
            Prompt::Find => match self.match_case {
                true => label.write_str("Find (match case): "),
                false => label.write_str("Find: "),
            },
            Prompt::ReplaceFind => label.write_str("Replace: "),
            Prompt::ReplaceWith => label.write_str("Replace with: "),
            Prompt::ReplaceEach { .. } => label.write_str("Replace this one?"),
            Prompt::GoTo { .. } => label.write_str("Go to line: "),
        };
        label
    }

    fn has_field(prompt: Prompt) -> bool {
        matches!(
            prompt,
            Prompt::SaveAs | Prompt::Open | Prompt::Find | Prompt::ReplaceFind | Prompt::ReplaceWith | Prompt::GoTo { .. }
        )
    }

    fn prompt_row(&self, row: &mut Row, prompt: Prompt) {
        let columns = self.columns;
        let label = self.prompt_label(prompt);
        let mut result = Line::new();
        match prompt {
            Prompt::Find | Prompt::ReplaceFind | Prompt::ReplaceEach { .. } => result = self.find_result(),
            Prompt::GoTo { asked } => {
                let count = self.editor.document().line_count();
                let _ = match asked {
                    true => write!(result, "Enter a line from 1 to {count}"),
                    false => write!(result, "Line 1-{count}"),
                };
            }
            _ => {}
        }
        if let Some(message) = &self.message {
            result = *message;
        }
        let result_cells = result.as_str().chars().count();
        let room = columns.saturating_sub(2);
        row.text(label.as_str(), room);
        if Self::has_field(prompt) {
            let field = core::str::from_utf8(self.field.text()).unwrap_or("");
            let space = room.saturating_sub(row.cells + result_cells + 2);
            let characters = field.chars().count();
            let skip = characters.saturating_sub(space);
            for character in field.chars().skip(skip) {
                if row.cells >= room {
                    break;
                }
                let mut encoded = [0u8; 4];
                row.raw(character.encode_utf8(&mut encoded).as_bytes());
                row.cells += 1;
            }
        }
        if result_cells > 0 && row.cells + 2 + result_cells <= room + 1 {
            let wanted = columns.saturating_sub(1).saturating_sub(result_cells);
            row.blanks(wanted.saturating_sub(row.cells));
            if self.style.colour {
                Self::sgr(row, b"90");
            }
            row.text(result.as_str(), columns.saturating_sub(1));
            if self.style.colour {
                Self::sgr(row, b"39");
            }
        }
    }

    fn prompt_hints(prompt: Prompt) -> &'static [(&'static str, &'static str)] {
        match prompt {
            Prompt::SaveChanges => &[("Y", "Save"), ("N", "Don't save"), ("Esc", "Cancel")],
            Prompt::SaveAs => &[("Enter", "Save"), ("Esc", "Cancel")],
            Prompt::ReplaceFile => &[("Y", "Replace"), ("Esc", "Cancel")],
            Prompt::Open => &[("Enter", "Open"), ("Esc", "Cancel")],
            Prompt::Find => &[
                ("Enter", "Next"),
                ("Down", "Next"),
                ("Up", "Previous"),
                ("Alt+C", "Match case"),
                ("Esc", "Close"),
            ],
            Prompt::ReplaceFind => &[("Enter", "Next"), ("Esc", "Close")],
            Prompt::ReplaceWith => &[("Enter", "Replace"), ("Esc", "Close")],
            Prompt::ReplaceEach { .. } => &[("Y", "Replace"), ("N", "Skip"), ("A", "All"), ("Esc", "Stop")],
            Prompt::GoTo { .. } => &[("Enter", "Go"), ("Esc", "Cancel")],
        }
    }

    fn hints_row(&self, row: &mut Row) {
        let hints: &[(&str, &str)] = match (self.keys_list.is_some(), self.prompt) {
            (true, _) => &[("Esc", "Back"), ("Up Down PgUp PgDn", "Scroll")],
            (false, Some(prompt)) => Self::prompt_hints(prompt),
            (false, None) => &HINTS,
        };
        let columns = self.columns;
        for (index, (key, word)) in hints.iter().enumerate() {
            let width = key.chars().count() + 1 + word.chars().count();
            let gap = if index == 0 { 0 } else { 2 };
            if row.cells + gap + width > columns {
                break;
            }
            row.blanks(gap);
            Self::sgr(row, b"7");
            row.text(key, columns);
            Self::sgr(row, b"0");
            row.blanks(1);
            row.text(word, columns);
        }
    }

    fn keys_list_row(&self, row: &mut Row, index: usize) {
        let Some((key, words)) = KEYS_LIST.get(index) else {
            return;
        };
        row.text(key, self.columns);
        row.blanks(KEY_COLUMN.saturating_sub(row.cells).min(self.columns.saturating_sub(row.cells)));
        row.text(words, self.columns);
        Self::sgr(row, b"0");
    }

    fn caret_cell(&self) -> (usize, usize) {
        let layout = layout(self.rows);
        if let Some(prompt) = self.prompt
            && Self::has_field(prompt)
        {
            let status = layout.status.unwrap_or(layout.text_top);
            let label = self.prompt_label(prompt).as_str().chars().count();
            let field = core::str::from_utf8(&self.field.bytes[..self.field.caret]).unwrap_or("").chars().count();
            return (status, (1 + label + field).min(self.columns.saturating_sub(1)));
        }
        let line = self.editor.caret_line();
        let document = self.editor.document();
        let start = document.line_start(line);
        let cells = self.cells_before(document.line(line), self.editor.caret() - start);
        let row = layout.text_top + line.saturating_sub(self.scroll_y);
        let column = self.gutter() + cells.saturating_sub(self.scroll_x);
        (row.min(self.rows - 1), column.min(self.columns - 1))
    }

    /// Writes into `out` the bytes that bring the terminal's screen up to date: the rows that
    /// changed since the last frame (all of them after a resize or Ctrl+L), each placed
    /// absolutely, and the cursor on the caret; how many bytes. A frame cut by a full buffer
    /// is drawn whole next time.
    pub fn frame(&mut self, out: &mut [u8]) -> usize {
        let mut out = Out {
            bytes: out,
            length: 0,
            full: false,
        };
        out.put(b"\x1b[?25l");
        let layout = layout(self.rows);
        let prompt_hides_text = self.rows == 1 && self.prompt.is_some();
        let mut state = match self.keys_list {
            Some(_) => State::Normal,
            None => self.state_at(self.scroll_y),
        };
        let redraw = core::mem::take(&mut self.redraw);
        let mut row = Row::new();
        for screen_row in 0..self.rows {
            row.length = 0;
            row.cells = 0;
            if Some(screen_row) == layout.title {
                let title = self.title();
                Self::centred(&mut row, title.as_str(), self.columns);
            } else if Some(screen_row) == layout.status || (prompt_hides_text && screen_row == 0) {
                self.status_row(&mut row);
            } else if Some(screen_row) == layout.hints {
                self.hints_row(&mut row);
            } else if screen_row >= layout.text_top && screen_row < layout.text_top + layout.text_rows {
                let index = screen_row - layout.text_top;
                match self.keys_list {
                    Some(scroll) => self.keys_list_row(&mut row, scroll + index),
                    None => state = self.text_row(&mut row, self.scroll_y + index, state),
                }
            }
            let hash = row.hash() ^ (row.length as u64);
            if !redraw && self.drawn[screen_row] == hash {
                continue;
            }
            self.drawn[screen_row] = hash;
            out.put(b"\x1b[");
            out.number(screen_row + 1);
            out.put(b";1H");
            out.put(&row.bytes[..row.length]);
            out.put(b"\x1b[0m\x1b[K");
        }
        let (caret_row, caret_column) = self.caret_cell();
        out.put(b"\x1b[");
        out.number(caret_row + 1);
        out.put(b";");
        out.number(caret_column + 1);
        out.put(b"H\x1b[?25h");
        if out.full {
            self.redraw = true;
        }
        out.length
    }
}

/// A terminal's answer to [`SIZE_QUERY`] at the start of `bytes`, `ESC [ rows ; columns R`
/// (not F3's `ESC [ 1 ; m R`): its rows, columns and length.
pub fn size_report(bytes: &[u8]) -> Option<(usize, usize, usize)> {
    let rest = bytes.strip_prefix(b"\x1b[")?;
    let end = rest.iter().position(|&byte| byte == b'R')?;
    let (rows, columns) = core::str::from_utf8(&rest[..end]).ok()?.split_once(';')?;
    let rows = rows.parse::<usize>().ok()?;
    let columns = columns.parse::<usize>().ok()?;
    (rows != 1 || columns > 8).then_some((rows, columns, end + 3))
}

#[cfg(test)]
mod tests;
