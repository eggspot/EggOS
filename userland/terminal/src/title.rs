//! The window's title (docs/ux/terminal-windows-v0.3.md section 1): `Terminal`, or the text a
//! program set with `OSC 0` or `OSC 2` followed by ` - Terminal`, the suffix by which the
//! desktop knows the window is a terminal's.

/// The title of a window whose program set none.
pub const DEFAULT_TITLE: &str = "Terminal";

/// What follows a program's text in the title.
pub const TITLE_SUFFIX: &str = " - Terminal";

/// The most bytes a title holds, the window server's limit.
pub const TITLE_CAPACITY: usize = 64;

/// The longest text that fits whole with the suffix.
pub const LONGEST_TEXT: usize = TITLE_CAPACITY - TITLE_SUFFIX.len();

/// The bytes a longer text keeps before `...`.
pub const CUT_TEXT: usize = LONGEST_TEXT - ELLIPSIS.len();

const ELLIPSIS: &str = "...";

/// A window title of at most [`TITLE_CAPACITY`] bytes of UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Title {
    bytes: [u8; TITLE_CAPACITY],
    length: usize,
}

impl Default for Title {
    fn default() -> Self {
        let mut title = Self::empty();
        title.push(DEFAULT_TITLE);
        title
    }
}

impl Title {
    fn empty() -> Self {
        Self {
            bytes: [0; TITLE_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, text: &str) {
        let end = (self.length + text.len()).min(TITLE_CAPACITY);
        let count = end - self.length;
        self.bytes[self.length..end].copy_from_slice(&text.as_bytes()[..count]);
        self.length = end;
    }

    /// The title for `text` a program set, of which bytes were dropped when `cut`: its valid
    /// UTF-8 start and the suffix, cut to its first [`CUT_TEXT`] bytes (on a character boundary)
    /// and `...` when longer than [`LONGEST_TEXT`] or cut; [`DEFAULT_TITLE`] for no text.
    pub fn of(text: &[u8], cut: bool) -> Self {
        let valid = match core::str::from_utf8(text) {
            Ok(valid) => valid,
            Err(error) => core::str::from_utf8(&text[..error.valid_up_to()]).unwrap_or(""),
        };
        if valid.is_empty() {
            return Self::default();
        }
        let mut title = Self::empty();
        if cut || valid.len() > LONGEST_TEXT {
            let mut end = CUT_TEXT.min(valid.len());
            while !valid.is_char_boundary(end) {
                end -= 1;
            }
            title.push(&valid[..end]);
            title.push(ELLIPSIS);
        } else {
            title.push(valid);
        }
        title.push(TITLE_SUFFIX);
        title
    }

    /// The title.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or(DEFAULT_TITLE)
    }
}
