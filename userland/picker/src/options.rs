//! `pick`'s command line (docs/ux/file-picker-v0.3.md section 15): the mode, then options of
//! that mode. Every argument is untrusted bytes from whoever started the picker.

use crate::filter::{self, Filter};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// The longest title, in bytes, as every window title.
pub const TITLE_BYTES: usize = 64;
const BUTTON_BYTES: usize = 32;
const MOST_FILTERS: usize = 16;

/// What the person chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    Save,
    Folder,
}

impl Mode {
    /// The mode named `word`.
    pub fn named(word: &[u8]) -> Option<Self> {
        match word {
            b"open" => Some(Self::Open),
            b"save" => Some(Self::Save),
            b"folder" => Some(Self::Folder),
            _ => None,
        }
    }

    /// Its word on the command line.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Save => "save",
            Self::Folder => "folder",
        }
    }

    /// The window's title when none is given.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Save => "Save As",
            Self::Folder => "Choose a Folder",
        }
    }

    /// The primary button's words when none are given.
    pub const fn button(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Save => "Save",
            Self::Folder => "Choose",
        }
    }
}

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    pub title: String,
    pub folder: Option<Vec<u8>>,
    pub name: Vec<u8>,
    pub extension: Option<Vec<u8>>,
    /// The filters given, then [`filter::ALL_FILES`] unless one given is `*`; empty when none
    /// was given or in `folder` mode.
    pub filters: Vec<Filter>,
    /// The filter applied first, from 0.
    pub filter_index: usize,
    pub multiple: bool,
    pub folders: bool,
    pub overwrite: bool,
    pub button: String,
    pub over: Option<u32>,
}

/// Why a command line was refused: exit status 2, one line on standard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    NoMode,
    UnknownMode(String),
    UnknownOption(String),
    NotInMode(String, Mode),
    NoValue(String),
    BadFilter(String),
    BadFilterIndex(String),
    BadWindow(String),
    Operand(String),
}

impl UsageError {
    /// Its line, ending in a new line: `pick: unknown option --x`.
    pub fn message(&self) -> String {
        let mut line = String::from("pick: ");
        let _ = match self {
            Self::NoMode => write!(line, "missing mode: open, save or folder"),
            Self::UnknownMode(word) => write!(line, "unknown mode {word}"),
            Self::UnknownOption(word) => write!(line, "unknown option {word}"),
            Self::NotInMode(word, mode) => {
                write!(line, "{word} is not an option of {}", mode.word())
            }
            Self::NoValue(word) => write!(line, "{word} needs a value"),
            Self::BadFilter(text) => write!(line, "a filter is <description>|<patterns>: {text}"),
            Self::BadFilterIndex(text) => write!(line, "no filter numbered {text}"),
            Self::BadWindow(text) => write!(line, "not a window id: {text}"),
            Self::Operand(word) => write!(line, "unexpected operand {word}"),
        };
        line.push('\n');
        line
    }
}

fn shown(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    filter::cut(&mut text, TITLE_BYTES);
    text
}

fn number(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 10 {
        return None;
    }
    text.iter().try_fold(0u32, |sum, &byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
        sum.checked_mul(10)?.checked_add(u32::from(digit))
    })
}

fn last_component(name: &[u8]) -> &[u8] {
    name.rsplit(|&byte| byte == b'/').next().unwrap_or(name)
}

fn allowed(option: &[u8], mode: Mode) -> Option<bool> {
    let everywhere = matches!(option, b"--title" | b"--folder" | b"--button" | b"--over");
    let open = matches!(option, b"--multiple" | b"--folders");
    let save = matches!(option, b"--name" | b"--extension" | b"--overwrite");
    let both = matches!(option, b"--filter" | b"--filter-index");
    if !(everywhere || open || save || both) {
        return None;
    }
    Some(match mode {
        Mode::Open => everywhere || open || both,
        Mode::Save => everywhere || save || both,
        Mode::Folder => everywhere,
    })
}

fn takes_value(option: &[u8]) -> bool {
    !matches!(option, b"--multiple" | b"--folders" | b"--overwrite")
}

impl Options {
    /// The options of `arguments`, the words after `pick`.
    pub fn parse<'a>(arguments: impl IntoIterator<Item = &'a [u8]>) -> Result<Self, UsageError> {
        let mut words = arguments.into_iter();
        let first = words.next().ok_or(UsageError::NoMode)?;
        let mode = Mode::named(first).ok_or_else(|| UsageError::UnknownMode(shown(first)))?;
        let mut options = Self {
            mode,
            title: String::from(mode.title()),
            folder: None,
            name: b"Untitled".to_vec(),
            extension: None,
            filters: Vec::new(),
            filter_index: 0,
            multiple: false,
            folders: false,
            overwrite: false,
            button: String::from(mode.button()),
            over: None,
        };
        let mut index_text: Option<Vec<u8>> = None;
        while let Some(word) = words.next() {
            if !word.starts_with(b"--") {
                return Err(UsageError::Operand(shown(word)));
            }
            match allowed(word, mode) {
                None => return Err(UsageError::UnknownOption(shown(word))),
                Some(false) => return Err(UsageError::NotInMode(shown(word), mode)),
                Some(true) => {}
            }
            let value = match takes_value(word) {
                true => Some(words.next().ok_or_else(|| UsageError::NoValue(shown(word)))?),
                false => None,
            };
            let value = value.unwrap_or_default();
            match word {
                b"--title" => options.title = shown(value),
                b"--folder" => options.folder = Some(value.to_vec()),
                b"--button" => {
                    let mut button = String::from_utf8_lossy(value).into_owned();
                    filter::cut(&mut button, BUTTON_BYTES);
                    options.button = button;
                }
                b"--over" => {
                    let window = number(value).ok_or_else(|| UsageError::BadWindow(shown(value)))?;
                    options.over = Some(window);
                }
                b"--name" => options.name = last_component(value).to_vec(),
                b"--extension" => {
                    let extension = value.strip_prefix(b".").unwrap_or(value);
                    options.extension = (!extension.is_empty()).then(|| extension.to_vec());
                }
                b"--filter" => {
                    let filter =
                        Filter::parse(value).ok_or_else(|| UsageError::BadFilter(shown(value)))?;
                    if options.filters.len() < MOST_FILTERS {
                        options.filters.push(filter);
                    }
                }
                b"--filter-index" => index_text = Some(value.to_vec()),
                b"--multiple" => options.multiple = true,
                b"--folders" => options.folders = true,
                _ => options.overwrite = true,
            }
        }
        if !options.filters.is_empty() && !options.filters.iter().any(Filter::is_everything) {
            options.filters.push(Filter::all_files());
        }
        if let Some(text) = index_text {
            let index = number(&text)
                .map(|index| index as usize)
                .filter(|index| (1..=options.filters.len()).contains(index))
                .ok_or_else(|| UsageError::BadFilterIndex(shown(&text)))?;
            options.filter_index = index - 1;
        }
        Ok(options)
    }
}
