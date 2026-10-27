//! Line editing at the prompt (D125): the shell reads the terminal a byte at a time, without
//! the terminal's own echo, and edits the line itself, so the up and down arrows (`ESC [ A`
//! and `ESC [ B`, or `ESC O A` and `ESC O B`) can bring back the lines typed before. The keys
//! are the terminal's canonical ones: delete or backspace erases a character, `^U` the line,
//! `^W` the word before the end, `^D` on an empty line ends the input, and a newline or a
//! carriage return ends the line. `^C` stays the terminal's: it interrupts the read. Other
//! escape sequences and control characters are ignored.

/// The most bytes of recent lines kept.
pub const HISTORY_CAPACITY: usize = 8192;

/// The most recent lines kept.
pub const MAX_HISTORY: usize = 64;

const ERASE: &[u8] = b"\x08 \x08";

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
    /// the room of; an empty line, one longer than the history, and one the same as the newest
    /// are not kept.
    pub fn add(&mut self, line: &[u8]) {
        if line.is_empty() || line.len() > HISTORY_CAPACITY || self.get(0) == Some(line) {
            return;
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
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Started,
    Sequence,
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

/// A line being edited in a buffer of the caller's: see the module documentation.
#[derive(Debug)]
pub struct Editor {
    length: usize,
    escape: Escape,
    back: Option<usize>,
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

impl Editor {
    /// An empty line.
    pub const fn new() -> Self {
        Self {
            length: 0,
            escape: Escape::None,
            back: None,
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
        match self.escape {
            Escape::Started => {
                self.escape = match byte {
                    b'[' | b'O' => Escape::Sequence,
                    _ => Escape::None,
                };
                return Fed::Editing;
            }
            Escape::Sequence => {
                if (0x40..=0x7e).contains(&byte) {
                    self.escape = Escape::None;
                    match byte {
                        b'A' => self.recall(true, buffer, history, echo),
                        b'B' => self.recall(false, buffer, history, echo),
                        _ => {}
                    }
                }
                return Fed::Editing;
            }
            Escape::None => {}
        }
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
            0x7f | 0x08 => {
                let start = buffer[..self.length]
                    .iter()
                    .rposition(|&byte| !is_continuation(byte))
                    .unwrap_or(0);
                if self.length > 0 {
                    self.length = start;
                    echo(ERASE);
                }
            }
            0x15 => self.erase_to(0, buffer, echo),
            0x17 => {
                let line = &buffer[..self.length];
                let word_end = line
                    .iter()
                    .rposition(|&byte| byte != b' ' && byte != b'\t')
                    .map_or(0, |at| at + 1);
                let start = line[..word_end]
                    .iter()
                    .rposition(|&byte| byte == b' ' || byte == b'\t')
                    .map_or(0, |at| at + 1);
                self.erase_to(start, buffer, echo);
            }
            b'\t' | 0x20.. => {
                if self.length + 1 < buffer.len() {
                    buffer[self.length] = byte;
                    self.length += 1;
                    echo(&[byte]);
                }
            }
            _ => {}
        }
        Fed::Editing
    }

    fn erase_to(&mut self, start: usize, buffer: &[u8], echo: &mut impl FnMut(&[u8])) {
        for _ in 0..characters(&buffer[start..self.length]) {
            echo(ERASE);
        }
        self.length = start;
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
        self.erase_to(0, buffer, echo);
        let line = back.and_then(|back| history.get(back)).unwrap_or_default();
        let length = line.len().min(buffer.len() - 1);
        buffer[..length].copy_from_slice(&line[..length]);
        self.length = length;
        echo(&buffer[..length]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Typed {
        editor: Editor,
        buffer: [u8; 64],
        shown: Vec<u8>,
    }

    impl Typed {
        fn new() -> Self {
            Self {
                editor: Editor::new(),
                buffer: [0; 64],
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
            results
        }
    }

    #[test]
    fn typed_characters_are_echoed_and_a_newline_ends_the_line() {
        let mut typed = Typed::new();
        assert_eq!(typed.keys(b"ls -l\r", &History::default()), [Fed::Line(6)]);
        assert_eq!(&typed.buffer[..6], b"ls -l\n");
        assert_eq!(typed.shown, b"ls -l\n");
    }

    #[test]
    fn erasing_keys_remove_a_character_a_word_or_the_line() {
        let mut typed = Typed::new();
        let history = History::default();
        typed.keys("ab\u{e9}\x7f\x7fc".as_bytes(), &history);
        typed.keys(b" two words \x17", &history);
        assert_eq!(typed.keys(b"\n", &history), [Fed::Line(8)]);
        assert_eq!(&typed.buffer[..8], b"ac two \n");
        let shown = String::from_utf8_lossy(&typed.shown).into_owned();
        assert_eq!(shown.matches("\x08 \x08").count(), 2 + 6);
        typed.keys(b"gone\x15kept\r", &history);
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
    fn the_history_keeps_the_newest_lines_in_its_room() {
        let mut history = History::default();
        history.add(b"");
        assert!(history.is_empty());
        for index in 0..MAX_HISTORY + 3 {
            history.add(format!("line {index}").as_bytes());
        }
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.get(0), Some(&b"line 66"[..]));
        assert_eq!(history.get(MAX_HISTORY - 1), Some(&b"line 3"[..]));
        history.add(b"line 66");
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
}
