//! Line editing at the prompt (D125, D181): the shell reads the terminal a byte at a time,
//! without the terminal's own echo, and edits the line itself, so the cursor can move within
//! the line and the up and down arrows (`ESC [ A` and `ESC [ B`, or `ESC O A` and `ESC O B`)
//! can bring back the lines typed before. Typed characters go in at the cursor. The left and
//! right arrows (and `^B`, `^F`) move the cursor a character, Home and End (and `^A`, `^E`) to
//! the start and the end; delete or backspace erases the character before the cursor, the
//! Delete key the one under it, `^U` everything before the cursor, `^K` everything from it,
//! `^W` the word before it; `^D` on an empty line ends the input and otherwise erases the
//! character under the cursor; a newline or a carriage return ends the line. `^C` stays the
//! terminal's: it interrupts the read. Other escape sequences and control characters are
//! ignored. The terminal is redrawn with plain backspaces, spaces and the line's own bytes, so
//! any terminal shows it; a line longer than the terminal is wide is redrawn wrongly once the
//! cursor moves back across its end.
//!
//! The history is also kept in a file across sessions (D181): [`HistoryLoader`] reads its
//! lines back, and [`should_remember`] says which lines go in it.

/// The most bytes of recent lines kept.
pub const HISTORY_CAPACITY: usize = 8192;

/// The most recent lines kept.
pub const MAX_HISTORY: usize = 64;

/// The longest line a history file gives back; longer ones are passed over.
pub const LOADED_LINE_CAPACITY: usize = 4096;

/// A history file larger than this many bytes is written again with only the lines kept,
/// when it is read (D181).
pub const HISTORY_FILE_LIMIT: usize = 8 * HISTORY_CAPACITY;

const BACKSPACE: u8 = 0x08;

/// The lines typed before, newest last.
#[derive(Debug, Clone)]
pub struct History {
    bytes: [u8; HISTORY_CAPACITY],
    ends: [u16; MAX_HISTORY],
    count: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            bytes: [0; HISTORY_CAPACITY],
            ends: [0; MAX_HISTORY],
            count: 0,
        }
    }
}

impl History {
    /// How many lines it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn end(&self, index: usize) -> usize {
        match index {
            0 => 0,
            _ => usize::from(self.ends[index - 1]),
        }
    }

    /// The line `back` lines before the newest (0 is the newest).
    pub fn get(&self, back: usize) -> Option<&[u8]> {
        let index = self.count.checked_sub(back + 1)?;
        Some(&self.bytes[self.end(index)..usize::from(self.ends[index])])
    }

    /// Adds `line` (without its newline) as the newest, forgetting the oldest lines it needs
    /// the room of, and says whether it did: an empty line, one longer than the history, and
    /// one the same as the newest are not kept.
    pub fn add(&mut self, line: &[u8]) -> bool {
        if line.is_empty() || line.len() > HISTORY_CAPACITY || self.get(0) == Some(line) {
            return false;
        }
        while self.count == MAX_HISTORY || self.end(self.count) + line.len() > HISTORY_CAPACITY {
            let first = usize::from(self.ends[0]);
            let used = self.end(self.count);
            self.bytes.copy_within(first..used, 0);
            self.ends.copy_within(1..self.count, 0);
            self.count -= 1;
            for end in &mut self.ends[..self.count] {
                *end -= first as u16;
            }
        }
        let start = self.end(self.count);
        self.bytes[start..start + line.len()].copy_from_slice(line);
        self.ends[self.count] = (start + line.len()) as u16;
        self.count += 1;
        true
    }
}

/// Whether a line typed at the prompt (without its newline) goes in the history and its file
/// (D181): not an empty one, and not one that starts with a space, so a secret typed on a
/// command line can be kept out of it, as with bash's `ignorespace`.
pub fn should_remember(line: &[u8]) -> bool {
    line.first().is_some_and(|&byte| byte != b' ')
}

/// Reads a history file's lines into a [`History`], fed the file's bytes in pieces (D181);
/// lines longer than [`LOADED_LINE_CAPACITY`] are passed over.
#[derive(Debug)]
pub struct HistoryLoader {
    line: [u8; LOADED_LINE_CAPACITY],
    length: usize,
    too_long: bool,
}

impl Default for HistoryLoader {
    fn default() -> Self {
        Self {
            line: [0; LOADED_LINE_CAPACITY],
            length: 0,
            too_long: false,
        }
    }
}

impl HistoryLoader {
    /// Takes the next `bytes` of the file, adding each line they end to `history`.
    pub fn feed(&mut self, bytes: &[u8], history: &mut History) {
        for &byte in bytes {
            if byte == b'\n' {
                self.end_line(history);
            } else if self.length < LOADED_LINE_CAPACITY {
                self.line[self.length] = byte;
                self.length += 1;
            } else {
                self.too_long = true;
            }
        }
    }

    /// Adds the file's last line, if it has no newline, to `history`.
    pub fn finish(&mut self, history: &mut History) {
        self.end_line(history);
    }

    fn end_line(&mut self, history: &mut History) {
        if !self.too_long {
            history.add(&self.line[..self.length]);
        }
        self.length = 0;
        self.too_long = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Started,
    Control { parameter: u16, more: bool },
    Single,
}

/// What a byte fed to the editor came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// The line goes on.
    Editing,
    /// The line ended: this many bytes of the buffer, its newline included.
    Line(usize),
    /// `^D` on an empty line: the input ended.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Older,
    Newer,
    Left,
    Right,
    Home,
    End,
    Delete,
}

/// A line being edited in a buffer of the caller's: see the module documentation.
#[derive(Debug)]
pub struct Editor {
    length: usize,
    cursor: usize,
    escape: Escape,
    back: Option<usize>,
    unfinished: Option<(usize, u8)>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

fn characters(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| !is_continuation(byte)).count()
}

fn continuations_after(lead: u8) -> u8 {
    match lead {
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        _ => 0,
    }
}

fn backspaces(count: usize, echo: &mut impl FnMut(&[u8])) {
    for _ in 0..count {
        echo(&[BACKSPACE]);
    }
}

fn spaces(count: usize, echo: &mut impl FnMut(&[u8])) {
    for _ in 0..count {
        echo(b" ");
    }
}

impl Editor {
    /// An empty line.
    pub const fn new() -> Self {
        Self {
            length: 0,
            cursor: 0,
            escape: Escape::None,
            back: None,
            unfinished: None,
        }
    }

    /// Takes `byte` into the line in `buffer`, the bytes held so far first, writing what the
    /// terminal must show through `echo`; `history` is what the arrows go through. The line
    /// never grows past the buffer, whose last byte is kept for the newline.
    pub fn feed(
        &mut self,
        byte: u8,
        buffer: &mut [u8],
        history: &History,
        echo: &mut impl FnMut(&[u8]),
    ) -> Fed {
        if self.escape != Escape::None {
            if let Some(key) = self.escape_key(byte) {
                self.finish_character(buffer, echo);
                match key {
                    Key::Older => self.recall(true, buffer, history, echo),
                    Key::Newer => self.recall(false, buffer, history, echo),
                    Key::Left => self.left(buffer, echo),
                    Key::Right => self.right(buffer, echo),
                    Key::Home => self.home(buffer, echo),
                    Key::End => self.end(buffer, echo),
                    Key::Delete => self.delete(buffer, echo),
                }
            }
            return Fed::Editing;
        }
        if is_continuation(byte) && self.unfinished.is_some() {
            self.insert(byte, buffer);
            self.continue_character(buffer, echo);
            return Fed::Editing;
        }
        self.finish_character(buffer, echo);
        match byte {
            0x1b => self.escape = Escape::Started,
            b'\r' | b'\n' => {
                echo(b"\n");
                let length = self.length;
                buffer[length] = b'\n';
                *self = Self::new();
                return Fed::Line(length + 1);
            }
            0x04 if self.length == 0 => {
                *self = Self::new();
                return Fed::End;
            }
            0x04 => self.delete(buffer, echo),
            0x01 => self.home(buffer, echo),
            0x05 => self.end(buffer, echo),
            0x02 => self.left(buffer, echo),
            0x06 => self.right(buffer, echo),
            0x7f | BACKSPACE => {
                if self.cursor > 0 {
                    let start = self.previous_boundary(buffer, self.cursor);
                    self.erase(start, self.cursor, buffer, echo);
                }
            }
            0x15 => self.erase(0, self.cursor, buffer, echo),
            0x0b => self.erase(self.cursor, self.length, buffer, echo),
            0x17 => {
                let before = &buffer[..self.cursor];
                let word_end = before
                    .iter()
                    .rposition(|&byte| byte != b' ' && byte != b'\t')
                    .map_or(0, |at| at + 1);
                let start = before[..word_end]
                    .iter()
                    .rposition(|&byte| byte == b' ' || byte == b'\t')
                    .map_or(0, |at| at + 1);
                self.erase(start, self.cursor, buffer, echo);
            }
            b'\t' | 0x20.. if self.length + 1 < buffer.len() => {
                let start = self.cursor;
                self.insert(byte, buffer);
                match continuations_after(byte) {
                    0 => self.redraw(start, buffer, echo),
                    expected => self.unfinished = Some((start, expected)),
                }
            }
            _ => {}
        }
        Fed::Editing
    }

    fn escape_key(&mut self, byte: u8) -> Option<Key> {
        match self.escape {
            Escape::None => None,
            Escape::Started => {
                self.escape = match byte {
                    b'[' => Escape::Control {
                        parameter: 0,
                        more: false,
                    },
                    b'O' => Escape::Single,
                    _ => Escape::None,
                };
                None
            }
            Escape::Single => {
                self.escape = Escape::None;
                final_key(byte, 0)
            }
            Escape::Control { parameter, more } => match byte {
                b'0'..=b'9' if !more => {
                    let parameter = parameter
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                    self.escape = Escape::Control { parameter, more };
                    None
                }
                0x20..=0x3f => {
                    self.escape = Escape::Control {
                        parameter,
                        more: true,
                    };
                    None
                }
                0x40..=0x7e => {
                    self.escape = Escape::None;
                    final_key(byte, parameter)
                }
                _ => {
                    self.escape = Escape::None;
                    None
                }
            },
        }
    }

    fn insert(&mut self, byte: u8, buffer: &mut [u8]) {
        if self.length + 1 >= buffer.len() {
            return;
        }
        buffer.copy_within(self.cursor..self.length, self.cursor + 1);
        buffer[self.cursor] = byte;
        self.cursor += 1;
        self.length += 1;
    }

    fn continue_character(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        let Some((start, expected)) = self.unfinished else {
            return;
        };
        match expected {
            1 => {
                self.unfinished = None;
                self.redraw(start, buffer, echo);
            }
            _ => self.unfinished = Some((start, expected - 1)),
        }
    }

    fn finish_character(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        if let Some((start, _)) = self.unfinished.take() {
            self.redraw(start, buffer, echo);
        }
    }

    fn redraw(&self, start: usize, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        echo(&buffer[start..self.length]);
        backspaces(characters(&buffer[self.cursor..self.length]), echo);
    }

    fn erase(&mut self, start: usize, end: usize, buffer: &mut [u8], echo: &mut impl FnMut(&[u8])) {
        if start >= end {
            return;
        }
        backspaces(characters(&buffer[start..self.cursor]), echo);
        let removed = characters(&buffer[start..end]);
        buffer.copy_within(end..self.length, start);
        self.length -= end - start;
        self.cursor = start;
        echo(&buffer[start..self.length]);
        spaces(removed, echo);
        backspaces(removed + characters(&buffer[start..self.length]), echo);
    }

    fn previous_boundary(&self, buffer: &[u8], at: usize) -> usize {
        buffer[..at]
            .iter()
            .rposition(|&byte| !is_continuation(byte))
            .unwrap_or(0)
    }

    fn next_boundary(&self, buffer: &[u8], at: usize) -> usize {
        buffer[at + 1..self.length]
            .iter()
            .position(|&byte| !is_continuation(byte))
            .map_or(self.length, |offset| at + 1 + offset)
    }

    fn left(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        if self.cursor > 0 {
            self.cursor = self.previous_boundary(buffer, self.cursor);
            echo(&[BACKSPACE]);
        }
    }

    fn right(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        if self.cursor < self.length {
            let next = self.next_boundary(buffer, self.cursor);
            echo(&buffer[self.cursor..next]);
            self.cursor = next;
        }
    }

    fn home(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        backspaces(characters(&buffer[..self.cursor]), echo);
        self.cursor = 0;
    }

    fn end(&mut self, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        echo(&buffer[self.cursor..self.length]);
        self.cursor = self.length;
    }

    fn delete(&mut self, buffer: &mut [u8], echo: &mut impl FnMut(&[u8])) {
        if self.cursor < self.length {
            let next = self.next_boundary(buffer, self.cursor);
            self.erase(self.cursor, next, buffer, echo);
        }
    }

    fn recall(
        &mut self,
        older: bool,
        buffer: &mut [u8],
        history: &History,
        echo: &mut impl FnMut(&[u8]),
    ) {
        let back = match (older, self.back) {
            (true, None) if !history.is_empty() => Some(0),
            (true, Some(back)) => Some((back + 1).min(history.len() - 1)),
            (false, Some(0)) => None,
            (false, Some(back)) => Some(back - 1),
            _ => return,
        };
        self.back = back;
        self.home(buffer, echo);
        let shown = characters(&buffer[..self.length]);
        let line = back.and_then(|back| history.get(back)).unwrap_or_default();
        let length = line.len().min(buffer.len() - 1);
        buffer[..length].copy_from_slice(&line[..length]);
        self.length = length;
        self.cursor = length;
        echo(&buffer[..length]);
        let now = characters(&buffer[..length]);
        if shown > now {
            spaces(shown - now, echo);
            backspaces(shown - now, echo);
        }
    }
}

fn final_key(byte: u8, parameter: u16) -> Option<Key> {
    match (byte, parameter) {
        (b'A', _) => Some(Key::Older),
        (b'B', _) => Some(Key::Newer),
        (b'C', _) => Some(Key::Right),
        (b'D', _) => Some(Key::Left),
        (b'H', _) | (b'~', 1 | 7) => Some(Key::Home),
        (b'F', _) | (b'~', 4 | 8) => Some(Key::End),
        (b'~', 3) => Some(Key::Delete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Screen {
        cells: Vec<char>,
        column: usize,
        lines: Vec<String>,
        pending: Vec<u8>,
    }

    impl Screen {
        fn show(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                match byte {
                    BACKSPACE => self.column = self.column.saturating_sub(1),
                    b'\n' => {
                        self.lines.push(self.cells.iter().collect());
                        self.cells.clear();
                        self.column = 0;
                    }
                    _ => {
                        self.pending.push(byte);
                        let Ok(text) = core::str::from_utf8(&self.pending) else {
                            continue;
                        };
                        let character = text.chars().next().unwrap();
                        self.pending.clear();
                        if self.column == self.cells.len() {
                            self.cells.push(character);
                        } else {
                            self.cells[self.column] = character;
                        }
                        self.column += 1;
                    }
                }
            }
        }

        fn text(&self) -> String {
            self.cells.iter().collect::<String>().trim_end().to_owned()
        }
    }

    struct Typed {
        editor: Editor,
        buffer: [u8; 64],
        screen: Screen,
        shown: Vec<u8>,
    }

    impl Typed {
        fn new() -> Self {
            Self {
                editor: Editor::new(),
                buffer: [0; 64],
                screen: Screen::default(),
                shown: Vec::new(),
            }
        }

        fn keys(&mut self, keys: &[u8], history: &History) -> Vec<Fed> {
            let mut results = Vec::new();
            for &key in keys {
                let shown = &mut self.shown;
                let fed = self
                    .editor
                    .feed(key, &mut self.buffer, history, &mut |bytes| {
                        shown.extend_from_slice(bytes)
                    });
                if fed != Fed::Editing {
                    results.push(fed);
                }
            }
            self.screen.show(&core::mem::take(&mut self.shown));
            results
        }

        fn line(&self) -> String {
            String::from_utf8(self.buffer[..self.editor.length].to_vec()).unwrap()
        }

        fn assert_shown(&self) {
            assert_eq!(self.screen.text(), self.line().trim_end());
            assert_eq!(
                self.screen.column,
                characters(&self.buffer[..self.editor.cursor])
            );
        }
    }

    #[test]
    fn typed_characters_are_echoed_and_a_newline_ends_the_line() {
        let mut typed = Typed::new();
        assert_eq!(typed.keys(b"ls -l\r", &History::default()), [Fed::Line(6)]);
        assert_eq!(&typed.buffer[..6], b"ls -l\n");
        assert_eq!(typed.screen.lines, ["ls -l"]);
    }

    #[test]
    fn erasing_keys_remove_a_character_a_word_or_the_line() {
        let mut typed = Typed::new();
        let history = History::default();
        typed.keys("ab\u{e9}\x7f\x7fc".as_bytes(), &history);
        typed.assert_shown();
        typed.keys(b" two words \x17", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "ac two ");
        assert_eq!(typed.keys(b"\n", &history), [Fed::Line(8)]);
        assert_eq!(&typed.buffer[..8], b"ac two \n");
        typed.keys(b"gone\x15kept", &history);
        typed.assert_shown();
        typed.keys(b"\r", &history);
        assert_eq!(&typed.buffer[..5], b"kept\n");
    }

    #[test]
    fn control_d_ends_the_input_only_on_an_empty_line() {
        let mut typed = Typed::new();
        let history = History::default();
        assert_eq!(typed.keys(b"a\x04\x7f\x04", &history), [Fed::End]);
    }

    #[test]
    fn the_arrows_bring_back_older_and_newer_lines() {
        let mut history = History::default();
        history.add(b"first");
        history.add(b"second");
        let mut typed = Typed::new();
        assert_eq!(
            typed.keys(b"x\x1b[A\x1b[A\x1b[A\r", &history),
            [Fed::Line(6)]
        );
        assert_eq!(&typed.buffer[..6], b"first\n");
        let mut typed = Typed::new();
        typed.keys(b"\x1bOA\x1b[A\x1b[B", &history);
        typed.assert_shown();
        assert_eq!(typed.keys(b"\r", &history), [Fed::Line(7)]);
        assert_eq!(&typed.buffer[..7], b"second\n");
        let mut typed = Typed::new();
        typed.keys(b"\x1b[A\x1b[B\x1b[B\x1b[3~\x1b[C", &history);
        assert_eq!(typed.keys(b"\r", &history), [Fed::Line(1)]);
        let mut typed = Typed::new();
        assert_eq!(
            typed.keys(b"\x1b[Az\r", &History::default()),
            [Fed::Line(2)]
        );
    }

    #[test]
    fn the_cursor_moves_within_the_line_and_typing_goes_in_at_it() {
        let history = History::default();
        let mut typed = Typed::new();
        typed.keys(b"EGGSH cursr\x1b[Do\x01echo \x05 moved", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "echo EGGSH cursor moved");
        let mut typed = Typed::new();
        typed.keys(b"abcd\x1b[D\x1b[D\x7f", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "acd");
        typed.keys(b"\x1b[3~", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "ad");
        typed.keys(b"\x1b[H\x1b[C\x0b", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "a");
        typed.keys(b"xyz\x1b[1~\x04\x1b[4~\x02\x02\x15", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "yz");
        typed.keys(b"\x1bOH\x06\x1b[1;5C-", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "yz-");
    }

    #[test]
    fn characters_of_several_bytes_move_and_go_in_whole() {
        let history = History::default();
        let mut typed = Typed::new();
        typed.keys("a\u{e9}b".as_bytes(), &history);
        typed.keys(b"\x1b[D\x1b[D", &history);
        typed.keys("\u{20ac}".as_bytes(), &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "a\u{20ac}\u{e9}b");
        typed.keys(b"\x1b[C\x7f", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "a\u{20ac}b");
    }

    #[test]
    fn a_recalled_line_shorter_than_the_one_shown_blanks_the_rest() {
        let mut history = History::default();
        history.add(b"ls");
        let mut typed = Typed::new();
        typed.keys(b"a long line\x1b[D\x1b[D\x1b[A", &history);
        typed.assert_shown();
        assert_eq!(typed.line(), "ls");
    }

    #[test]
    fn the_history_keeps_the_newest_lines_in_its_room() {
        let mut history = History::default();
        assert!(!history.add(b""));
        assert!(history.is_empty());
        for index in 0..MAX_HISTORY + 3 {
            assert!(history.add(format!("line {index}").as_bytes()));
        }
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.get(0), Some(&b"line 66"[..]));
        assert_eq!(history.get(MAX_HISTORY - 1), Some(&b"line 3"[..]));
        assert!(!history.add(b"line 66"));
        assert_eq!(history.len(), MAX_HISTORY);
        let long = vec![b'x'; HISTORY_CAPACITY - 10];
        history.add(&long);
        assert_eq!(history.get(0), Some(&long[..]));
        assert!(history.len() < 3);
        history.add(&vec![b'y'; HISTORY_CAPACITY + 1]);
        assert_eq!(history.get(0), Some(&long[..]));
    }

    #[test]
    fn a_line_stops_growing_at_the_end_of_the_buffer() {
        let mut typed = Typed::new();
        typed.keys(&[b'a'; 100], &History::default());
        assert_eq!(typed.keys(b"\n", &History::default()), [Fed::Line(64)]);
    }

    #[test]
    fn a_history_file_s_lines_come_back_in_order_from_pieces() {
        let mut history = History::default();
        let mut loader = HistoryLoader::default();
        let file = format!("first\nsec{}\n\nthird", "o".repeat(LOADED_LINE_CAPACITY));
        for piece in file.as_bytes().chunks(7) {
            loader.feed(piece, &mut history);
        }
        loader.finish(&mut history);
        assert_eq!(history.len(), 2);
        assert_eq!(history.get(0), Some(&b"third"[..]));
        assert_eq!(history.get(1), Some(&b"first"[..]));
        loader.feed(b"fourth\n", &mut history);
        loader.finish(&mut history);
        assert_eq!(history.get(0), Some(&b"fourth"[..]));
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn lines_starting_with_a_space_are_not_remembered() {
        assert!(should_remember(b"ls"));
        assert!(!should_remember(b" export TOKEN=x"));
        assert!(!should_remember(b""));
    }
}
