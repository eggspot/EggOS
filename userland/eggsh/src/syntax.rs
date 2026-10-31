//! The command language's words (D107, D182): one simple command at a time, read from text and
//! expanded into words, which the shell's grammar (`shell/script.rs`) groups into pipelines,
//! lists and compound commands. Words are split on blanks; `'...'` keeps everything, `"..."`
//! keeps all but `$` and `` ` `` expansions and `\` before `$`, `` ` ``, `"`, `\` and a newline;
//! an unquoted `\` keeps the next character, and `\` before a newline joins the lines. `#` at
//! the start of a word comments to the end of the line. `$NAME`, `${NAME}`, the special
//! parameters (`$?`, `$$`, `$#`, `$!`, `$0`, `$1`..., `${10}`, `$@`, `$*`), `${NAME:-word}` and
//! the other operators, `$(command)` and `` `command` `` expand; unquoted, their values are
//! split into fields at the characters of `IFS`. `~` alone or before `/` at the start of a word
//! is `$HOME`. `NAME=value` words before the command are assignments. Commands end at `;`, a
//! newline, the end of the text, `|`, `&&`, `||`, `&`, `)` and `;;`. `< file`, `> file`,
//! `>> file`, `>| file`, `n>&m` and `<<word` (a here-document) anywhere in a command redirect
//! it. Words keep which of their bytes were quoted, so the shell can tell a pattern's `*` from a
//! quoted one. The operators eggsh does not have (`$((`, `<<<`, `&>`, `|&`) are refused as not
//! supported yet, never read as words.

use crate::pattern::Side;

/// The most bytes a command's words take, each with its terminating NUL.
pub const WORDS_CAPACITY: usize = 8192;

/// The most words a command has, assignments included.
pub const MAX_WORDS: usize = 256;

/// The most redirections a command has.
pub const MAX_REDIRECTIONS: usize = 8;

/// The most bytes a command's redirections' file names take, each with its terminating NUL.
pub const REDIRECTIONS_CAPACITY: usize = 4096;

/// The most bytes of a pattern, its quoted characters escaped (`${NAME#pattern}`, `case`).
pub const PATTERN_CAPACITY: usize = 4096;

/// The most bytes of a here-document's delimiter word.
pub const DELIMITER_CAPACITY: usize = 128;

const IFS_CAPACITY: usize = 16;

const QUOTED_MARKS: usize = WORDS_CAPACITY / 64;

const PATTERN_SPECIAL: &[u8] = b"*?[]\\-!^";

const DEFAULT_IFS: &[u8] = b" \t\n";

const UNSET_MESSAGE: &[u8] = b"parameter null or not set";

/// How a redirection opens its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redirection {
    /// `< file`: the descriptor reads the file.
    Input,
    /// `> file`: the descriptor replaces the file's contents, creating it if needed.
    Output,
    /// `>> file`: the descriptor adds to the end of the file, creating it if needed.
    Append,
}

/// One of a command's redirections, in the order written (D181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redirect<'a> {
    /// Descriptor `descriptor` opens `path` (with its NUL) as `redirection` says.
    File {
        descriptor: u8,
        redirection: Redirection,
        path: &'a [u8],
    },
    /// Descriptor `descriptor` becomes a copy of descriptor `source` (`2>&1`).
    Duplicate { descriptor: u8, source: u8 },
    /// Descriptor `descriptor` reads the here-document whose `<<` is `position` bytes into the
    /// command's text (D182).
    Document { descriptor: u8, position: usize },
}

/// Why a command could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxError {
    /// The text ends inside the command: more may complete it.
    Incomplete,
    /// The text ended for good inside a quote, `${`, `$(` or `` ` `` (its first character).
    UnexpectedEnd(u8),
    /// An operator the shell does not support yet.
    NotSupported(&'static str),
    /// An operator or a reserved word where it cannot be.
    UnexpectedToken(&'static str),
    /// A `${...}` that is not a parameter with an operator the shell knows.
    BadSubstitution,
    /// More than [`WORDS_CAPACITY`] bytes or [`MAX_WORDS`] words, or another limit.
    TooLong,
    /// The text ended for good where this reserved word or operator was needed (D182).
    Missing(&'static str),
    /// This reserved word, operator or word was needed where something else came (D182).
    Expected(&'static str),
    /// A word after a compound command, where only redirections and operators may be (D182).
    UnexpectedWord,
    /// `for` not followed by a variable's name (D182).
    BadName,
    /// An expansion failed and the shell said why (`${NAME?message}`): the command does not
    /// run (D182).
    Failed,
}

/// What ended a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    /// `;`: more commands follow on the line.
    Semicolon,
    /// A newline.
    Newline,
    /// The end of the text.
    End,
    /// `|`: the command's output is the next command's input.
    Pipe,
    /// `&&`: the next pipeline runs if this one succeeded (D181).
    And,
    /// `||`: the next pipeline runs if this one failed (D181).
    Or,
    /// `&`: the pipeline runs in the background (D181).
    Background,
    /// `)`: the end of a subshell or of a `case` pattern, left in the text (D182).
    Close,
    /// `;;`: the end of a `case` item's commands, left in the text (D182).
    CaseEnd,
}

impl Terminator {
    /// How many bytes the terminator takes in the text: none for those left in it.
    pub fn length(self) -> usize {
        match self {
            Self::End | Self::Close | Self::CaseEnd => 0,
            Self::And | Self::Or => 2,
            _ => 1,
        }
    }

    /// The terminator as a syntax error names it.
    pub fn token(self) -> &'static str {
        match self {
            Self::Semicolon => ";",
            Self::Pipe => "|",
            Self::And => "&&",
            Self::Or => "||",
            Self::Background => "&",
            Self::Close => ")",
            Self::CaseEnd => ";;",
            Self::Newline | Self::End => "newline",
        }
    }
}

/// A command read from text: how many bytes it took, its terminator included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    /// The bytes read.
    pub consumed: usize,
    /// What ended the command.
    pub terminator: Terminator,
}

/// The shell's reserved words, recognized only as the first word of a command (D182).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    /// `if`.
    If,
    /// `then`.
    Then,
    /// `elif`.
    Elif,
    /// `else`.
    Else,
    /// `fi`.
    Fi,
    /// `while`.
    While,
    /// `until`.
    Until,
    /// `for`.
    For,
    /// `do`.
    Do,
    /// `done`.
    Done,
    /// `case`.
    Case,
    /// `esac`.
    Esac,
    /// `in`, after `for name` and `case word`.
    In,
    /// `{`.
    OpenBrace,
    /// `}`.
    CloseBrace,
    /// `!`, before a pipeline.
    Bang,
}

const KEYWORDS: [(&[u8], Keyword); 16] = [
    (b"if", Keyword::If),
    (b"then", Keyword::Then),
    (b"elif", Keyword::Elif),
    (b"else", Keyword::Else),
    (b"fi", Keyword::Fi),
    (b"while", Keyword::While),
    (b"until", Keyword::Until),
    (b"for", Keyword::For),
    (b"do", Keyword::Do),
    (b"done", Keyword::Done),
    (b"case", Keyword::Case),
    (b"esac", Keyword::Esac),
    (b"in", Keyword::In),
    (b"{", Keyword::OpenBrace),
    (b"}", Keyword::CloseBrace),
    (b"!", Keyword::Bang),
];

impl Keyword {
    /// The word itself.
    pub fn text(self) -> &'static str {
        match self {
            Self::If => "if",
            Self::Then => "then",
            Self::Elif => "elif",
            Self::Else => "else",
            Self::Fi => "fi",
            Self::While => "while",
            Self::Until => "until",
            Self::For => "for",
            Self::Do => "do",
            Self::Done => "done",
            Self::Case => "case",
            Self::Esac => "esac",
            Self::In => "in",
            Self::OpenBrace => "{",
            Self::CloseBrace => "}",
            Self::Bang => "!",
        }
    }

    /// How many bytes it takes.
    pub fn length(self) -> usize {
        self.text().len()
    }
}

/// The reserved word `input` starts with: its first word whole, unquoted. `Incomplete` if
/// the text ends inside that word and more may come.
pub fn keyword(input: &[u8], final_input: bool) -> Result<Option<Keyword>, SyntaxError> {
    let length = input
        .iter()
        .position(|&byte| ends_a_word(Some(byte)))
        .unwrap_or(input.len());
    if length == input.len() && !final_input && length > 0 {
        return Err(SyntaxError::Incomplete);
    }
    let word = &input[..length];
    Ok(KEYWORDS
        .iter()
        .find(|(text, _)| *text == word)
        .map(|(_, keyword)| *keyword))
}

/// A function definition's start, `name()` (blanks allowed around the parentheses): the
/// name's length and where the text after `)` begins.
pub fn function_definition(input: &[u8]) -> Option<(usize, usize)> {
    let name = input.iter().take_while(|&&byte| is_name_byte(byte)).count();
    if name == 0 || !is_name_start(input[0]) {
        return None;
    }
    let mut at = name;
    while matches!(input.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    if input.get(at) != Some(&b'(') {
        return None;
    }
    at += 1;
    while matches!(input.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    (input.get(at) == Some(&b')')).then_some((name, at + 1))
}

/// A parameter's value as the shell has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    /// Text.
    Text(&'a [u8]),
    /// A number, written in decimal.
    Number(u64),
    /// Not set: expands to nothing.
    Unset,
}

/// Where expansions get their values, and how `$(...)` runs.
pub trait Parameters {
    /// Whether expansions take values and run commands: false when only reading syntax.
    const EXPANDS: bool = true;
    /// The value of `name`: a variable's name, a positional parameter's number, or one of
    /// `?`, `$`, `#`, `!`.
    fn value(&self, name: &[u8]) -> Value<'_>;
    /// How many positional parameters there are.
    fn positional_count(&self) -> usize {
        0
    }
    /// Positional parameter `number`, from 1 to [`Parameters::positional_count`].
    fn positional(&self, _number: usize) -> &[u8] {
        b""
    }
    /// Sets variable `name` to `value` (`${NAME:=value}`).
    fn assign(&mut self, _name: &[u8], _value: &[u8]) -> Result<(), SyntaxError> {
        Err(SyntaxError::BadSubstitution)
    }
    /// Says that `${name?message}` found `name` unset or empty.
    fn fail(&mut self, _name: &[u8], _message: &[u8]) {}
    /// Runs `command` (between `$(` and `)`, or backquotes if `backquoted`) and gives its
    /// output to `output` piece by piece.
    fn substitute(
        &mut self,
        _command: &[u8],
        _backquoted: bool,
        _output: &mut dyn FnMut(&[u8]) -> Result<(), SyntaxError>,
    ) -> Result<(), SyntaxError> {
        Ok(())
    }
}

/// No parameter is set and nothing runs: for reading a command's syntax without expanding it.
#[derive(Debug, Clone, Copy)]
pub struct NoParameters;

impl Parameters for NoParameters {
    const EXPANDS: bool = false;

    fn value(&self, _: &[u8]) -> Value<'_> {
        Value::Unset
    }
}

/// A command's words, each ended by a NUL, the assignments first, which of their bytes were
/// quoted, and the command's redirections.
#[derive(Debug, Clone)]
pub struct Words {
    bytes: [u8; WORDS_CAPACITY],
    quoted: [u64; QUOTED_MARKS],
    length: usize,
    ends: [u16; MAX_WORDS],
    count: usize,
    assignments: usize,
    files: [u8; REDIRECTIONS_CAPACITY],
    files_length: usize,
    redirections: [StoredRedirect; MAX_REDIRECTIONS],
    redirection_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredKind {
    File(Redirection),
    Duplicate,
    Document,
}

#[derive(Debug, Clone, Copy)]
struct StoredRedirect {
    descriptor: u8,
    kind: StoredKind,
    start: usize,
    end: usize,
}

impl Default for Words {
    fn default() -> Self {
        Self {
            bytes: [0; WORDS_CAPACITY],
            quoted: [0; QUOTED_MARKS],
            length: 0,
            ends: [0; MAX_WORDS],
            count: 0,
            assignments: 0,
            files: [0; REDIRECTIONS_CAPACITY],
            files_length: 0,
            redirections: [StoredRedirect {
                descriptor: 0,
                kind: StoredKind::Duplicate,
                start: 0,
                end: 0,
            }; MAX_REDIRECTIONS],
            redirection_count: 0,
        }
    }
}

impl Words {
    fn start(&self, index: usize) -> usize {
        match index {
            0 => 0,
            _ => usize::from(self.ends[index - 1]),
        }
    }

    /// Word `index` with its terminating NUL.
    pub fn with_nul(&self, index: usize) -> &[u8] {
        &self.bytes[self.start(index)..usize::from(self.ends[index])]
    }

    /// Word `index`.
    pub fn get(&self, index: usize) -> &[u8] {
        let word = self.with_nul(index);
        &word[..word.len() - 1]
    }

    /// How many words there are, assignments included.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The assignments (`NAME=value`) before the command.
    pub fn assignments(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.assignments).map(|index| self.get(index))
    }

    /// How many words the command has, its name included: the words after the assignments.
    pub fn command_length(&self) -> usize {
        self.count - self.assignments
    }

    /// The command's word `index`: 0 is its name.
    pub fn command(&self, index: usize) -> &[u8] {
        self.get(self.assignments + index)
    }

    /// The command's word `index` with its terminating NUL.
    pub fn command_with_nul(&self, index: usize) -> &[u8] {
        self.with_nul(self.assignments + index)
    }

    /// The assignment `index` with its terminating NUL.
    pub fn assignment_with_nul(&self, index: usize) -> &[u8] {
        self.with_nul(index)
    }

    /// How many assignments come before the command.
    pub fn assignment_count(&self) -> usize {
        self.assignments
    }

    /// The command's redirections in the order written, each file name with its NUL.
    pub fn redirections(&self) -> impl Iterator<Item = Redirect<'_>> {
        self.redirections[..self.redirection_count]
            .iter()
            .map(|stored| match stored.kind {
                StoredKind::File(redirection) => Redirect::File {
                    descriptor: stored.descriptor,
                    redirection,
                    path: &self.files[stored.start..stored.end],
                },
                StoredKind::Duplicate => Redirect::Duplicate {
                    descriptor: stored.descriptor,
                    source: stored.start as u8,
                },
                StoredKind::Document => Redirect::Document {
                    descriptor: stored.descriptor,
                    position: stored.start,
                },
            })
    }

    fn is_quoted(&self, position: usize) -> bool {
        self.quoted[position / 64] & (1 << (position % 64)) != 0
    }

    /// Whether word `index` has an unquoted `*`, `?` or `[`: whether it is a pattern that
    /// pathname expansion matches against file names (D182).
    pub fn is_pattern(&self, index: usize) -> bool {
        (self.start(index)..usize::from(self.ends[index]) - 1).any(|position| {
            matches!(self.bytes[position], b'*' | b'?' | b'[') && !self.is_quoted(position)
        })
    }

    /// Word `index` as a pattern: its quoted special characters escaped with `\`, written in
    /// `buffer`; `None` if it does not fit.
    pub fn pattern<'b>(&self, index: usize, buffer: &'b mut [u8]) -> Option<&'b [u8]> {
        self.escaped(self.start(index), usize::from(self.ends[index]) - 1, buffer)
    }

    fn escaped<'b>(&self, start: usize, end: usize, buffer: &'b mut [u8]) -> Option<&'b [u8]> {
        let mut length = 0;
        for position in start..end {
            let byte = self.bytes[position];
            if self.is_quoted(position) && PATTERN_SPECIAL.contains(&byte) {
                *buffer.get_mut(length)? = b'\\';
                length += 1;
            }
            *buffer.get_mut(length)? = byte;
            length += 1;
        }
        Some(&buffer[..length])
    }

    /// Replaces word `index` with `replacements` (a pattern with the paths it matched); the
    /// quoted marks of the words after it are lost, so the shell replaces the last first.
    pub fn replace(&mut self, index: usize, replacements: &[&[u8]]) -> Result<(), SyntaxError> {
        let start = self.start(index);
        let end = usize::from(self.ends[index]);
        let added: usize = replacements.iter().map(|word| word.len() + 1).sum();
        let length = self.length - (end - start) + added;
        let count = self.count - 1 + replacements.len();
        if length >= WORDS_CAPACITY || count > MAX_WORDS || replacements.is_empty() {
            return Err(SyntaxError::TooLong);
        }
        self.bytes.copy_within(end..self.length, start + added);
        let mut at = start;
        for word in replacements {
            self.bytes[at..at + word.len()].copy_from_slice(word);
            self.bytes[at + word.len()] = 0;
            at += word.len() + 1;
        }
        let shift = replacements.len() - 1;
        self.ends
            .copy_within(index + 1..self.count, index + 1 + shift);
        for later in &mut self.ends[index + 1 + shift..count] {
            *later = (usize::from(*later) + added - (end - start)) as u16;
        }
        let mut at = start;
        for (offset, word) in replacements.iter().enumerate() {
            at += word.len() + 1;
            self.ends[index + offset] = at as u16;
        }
        self.length = length;
        self.count = count;
        Ok(())
    }

    fn word_start(&self) -> usize {
        self.start(self.count)
    }

    fn push(&mut self, byte: u8, quoted: bool) -> Result<(), SyntaxError> {
        if self.length + 1 >= WORDS_CAPACITY {
            return Err(SyntaxError::TooLong);
        }
        let (slot, bit) = (self.length / 64, 1u64 << (self.length % 64));
        match quoted {
            true => self.quoted[slot] |= bit,
            false => self.quoted[slot] &= !bit,
        }
        self.bytes[self.length] = byte;
        self.length += 1;
        Ok(())
    }

    fn finish(&mut self, assignment: bool) -> Result<(), SyntaxError> {
        if self.count == MAX_WORDS || self.length >= WORDS_CAPACITY {
            return Err(SyntaxError::TooLong);
        }
        self.bytes[self.length] = 0;
        self.length += 1;
        self.ends[self.count] = self.length as u16;
        self.count += 1;
        if assignment {
            self.assignments += 1;
        }
        Ok(())
    }

    fn finish_redirection(
        &mut self,
        descriptor: u8,
        redirection: Redirection,
    ) -> Result<(), SyntaxError> {
        let start = self.word_start();
        let name = start..self.length;
        let end = self.files_length + name.len() + 1;
        if self.redirection_count == MAX_REDIRECTIONS || end > REDIRECTIONS_CAPACITY {
            return Err(SyntaxError::TooLong);
        }
        self.files[self.files_length..end - 1].copy_from_slice(&self.bytes[name]);
        self.files[end - 1] = 0;
        self.redirections[self.redirection_count] = StoredRedirect {
            descriptor,
            kind: StoredKind::File(redirection),
            start: self.files_length,
            end,
        };
        self.redirection_count += 1;
        self.files_length = end;
        self.length = start;
        Ok(())
    }

    fn add_redirection(
        &mut self,
        descriptor: u8,
        kind: StoredKind,
        start: usize,
    ) -> Result<(), SyntaxError> {
        if self.redirection_count == MAX_REDIRECTIONS {
            return Err(SyntaxError::TooLong);
        }
        self.redirections[self.redirection_count] = StoredRedirect {
            descriptor,
            kind,
            start,
            end: 0,
        };
        self.redirection_count += 1;
        Ok(())
    }

    /// Adds `word` as a quoted word: a value `for` loops over (D182).
    pub fn add(&mut self, word: &[u8]) -> Result<(), SyntaxError> {
        for &byte in word {
            self.push(byte, true)?;
        }
        self.finish(false)
    }

    /// Forgets every word and redirection.
    pub fn clear(&mut self) {
        self.length = 0;
        self.count = 0;
        self.assignments = 0;
        self.files_length = 0;
        self.redirection_count = 0;
    }

    fn only_assignments(&self) -> bool {
        self.count == self.assignments
    }

    fn tail(&self, from: usize) -> &[u8] {
        &self.bytes[from..self.length]
    }
}

/// Whether `byte` may start a name.
pub fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Whether `byte` may continue a name.
pub fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `name` is a variable's name.
pub fn is_name(name: &[u8]) -> bool {
    name.first().is_some_and(|&byte| is_name_start(byte))
        && name.iter().all(|&byte| is_name_byte(byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    Command,
    Trailer,
    Words,
    Word,
}

/// Reads the first command of `input` into `words`, expanding it with `parameters`. If
/// `final_input`, the text ends there for good; otherwise running out of text before a
/// terminator is [`SyntaxError::Incomplete`].
pub fn next_command<P: Parameters>(
    input: &[u8],
    final_input: bool,
    parameters: &mut P,
    words: &mut Words,
) -> Result<Parsed, SyntaxError> {
    Lexer::new(input, final_input, parameters, words, Reading::Command).command()
}

/// As [`next_command`], but nothing before the terminator is no error: what follows a
/// compound command, its redirections (D182).
pub fn next_trailer<P: Parameters>(
    input: &[u8],
    final_input: bool,
    parameters: &mut P,
    words: &mut Words,
) -> Result<Parsed, SyntaxError> {
    Lexer::new(input, final_input, parameters, words, Reading::Trailer).command()
}

/// As [`next_command`], but every word is a plain word, never an assignment: the words of
/// `for name in words` (D182).
pub fn next_words<P: Parameters>(
    input: &[u8],
    final_input: bool,
    parameters: &mut P,
    words: &mut Words,
) -> Result<Parsed, SyntaxError> {
    Lexer::new(input, final_input, parameters, words, Reading::Words).command()
}

/// Reads one word after blanks, expanded but neither split nor a pattern's matches: `case`'s
/// word and its patterns (D182). `None` if an operator, a newline or the end comes first;
/// otherwise where the word ends (`words` may hold no word if it expanded to nothing).
pub fn next_word<P: Parameters>(
    input: &[u8],
    final_input: bool,
    parameters: &mut P,
    words: &mut Words,
) -> Result<Option<usize>, SyntaxError> {
    Lexer::new(input, final_input, parameters, words, Reading::Word).word()
}

/// Expands a here-document's `body` (`$` and `` ` `` expansions, `\` before `$`, `` ` ``, `\`
/// and a newline) into one word of `words` (D182).
pub fn expand_document<P: Parameters>(
    body: &[u8],
    parameters: &mut P,
    words: &mut Words,
) -> Result<(), SyntaxError> {
    let mut lexer = Lexer::new(body, true, parameters, words, Reading::Word);
    lexer.scan.in_word = true;
    while let Some(byte) = lexer.scan.peek(0) {
        match byte {
            b'\\' => match lexer.scan.peek(1) {
                Some(escaped @ (b'$' | b'`' | b'\\')) => {
                    lexer.scan.push_quoted(escaped)?;
                    lexer.scan.at += 2;
                }
                Some(b'\n') => lexer.scan.at += 2,
                _ => {
                    lexer.scan.push_quoted(b'\\')?;
                    lexer.scan.at += 1;
                }
            },
            b'$' => lexer.dollar(true)?,
            b'`' => lexer.backquoted(true)?,
            _ => {
                lexer.scan.push_quoted(byte)?;
                lexer.scan.at += 1;
            }
        }
    }
    lexer.scan.words.finish(false)
}

/// A here-document's delimiter, read from the text at its `<<` (D182).
#[derive(Debug, Clone, Copy)]
pub struct Delimiter {
    bytes: [u8; DELIMITER_CAPACITY],
    length: usize,
    /// Whether any of the word was quoted: then the body is not expanded.
    pub quoted: bool,
    /// Whether the operator was `<<-`: leading tabs are removed from each line.
    pub strip_tabs: bool,
}

impl Delimiter {
    /// The delimiter's text, its quotes removed.
    pub fn text(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// The delimiter of the here-document whose `<<` starts `operator`.
pub fn document_delimiter(operator: &[u8]) -> Result<Delimiter, SyntaxError> {
    let strip_tabs = operator.get(2) == Some(&b'-');
    let mut at = if strip_tabs { 3 } else { 2 };
    while matches!(operator.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let length = word_length(&operator[at..]);
    let mut delimiter = Delimiter {
        bytes: [0; DELIMITER_CAPACITY],
        length: 0,
        quoted: false,
        strip_tabs,
    };
    let mut quote = None;
    let mut escaped = false;
    for &byte in &operator[at..at + length] {
        let kept = match (quote, byte) {
            _ if escaped => {
                escaped = false;
                true
            }
            (None, b'\\') | (Some(b'"'), b'\\') => {
                escaped = true;
                delimiter.quoted = true;
                false
            }
            (None, b'\'' | b'"') => {
                quote = Some(byte);
                delimiter.quoted = true;
                false
            }
            (Some(open), _) if open == byte => {
                quote = None;
                false
            }
            _ => true,
        };
        if kept {
            if delimiter.length == DELIMITER_CAPACITY {
                return Err(SyntaxError::TooLong);
            }
            delimiter.bytes[delimiter.length] = byte;
            delimiter.length += 1;
        }
    }
    Ok(delimiter)
}

/// Where a here-document's body that starts at `body` ends in `text`: the start of the line
/// that is its delimiter (leading tabs removed if `strip_tabs`) and where the text goes on
/// after that line. `None` if no such line comes: the body runs to the end if `final_input`,
/// otherwise more text is needed.
pub fn document_end(
    text: &[u8],
    body: usize,
    delimiter: &[u8],
    strip_tabs: bool,
    final_input: bool,
) -> Option<(usize, usize)> {
    let mut line = body;
    while line < text.len() {
        let (end, after) = match text[line..].iter().position(|&byte| byte == b'\n') {
            Some(newline) => (line + newline, line + newline + 1),
            None if final_input => (text.len(), text.len()),
            None => return None,
        };
        let mut content = &text[line..end];
        if strip_tabs {
            content = &content[content.iter().take_while(|&&byte| byte == b'\t').count()..];
        }
        if content == delimiter {
            return Some((line, after));
        }
        line = after;
    }
    None
}

/// Where the command substitution whose text starts `input` (right after `$(`) ends: the
/// index of its `)`. Quotes, escapes, comments and nested `$(...)` are passed over, and other
/// parentheses counted; a `case` pattern inside must be written with its leading `(`.
pub fn substitution_end(input: &[u8], final_input: bool) -> Result<usize, SyntaxError> {
    let end_of_text = || match final_input {
        true => SyntaxError::UnexpectedEnd(b'('),
        false => SyntaxError::Incomplete,
    };
    let mut depth = 0usize;
    let mut at = 0;
    let mut word_start = true;
    while let Some(&byte) = input.get(at) {
        let next = match byte {
            b'\\' => at + 2,
            b'\'' => {
                let close = input[at + 1..]
                    .iter()
                    .position(|&byte| byte == b'\'')
                    .ok_or_else(end_of_text)?;
                at + 1 + close + 1
            }
            b'"' => skip_double_quoted(input, at + 1, final_input)?,
            b'`' => skip_backquoted(input, at + 1).ok_or_else(end_of_text)?,
            b'#' if word_start => input[at..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(input.len(), |newline| at + newline),
            b'$' if input.get(at + 1) == Some(&b'(') => {
                at + 2 + substitution_end(&input[at + 2..], final_input)? + 1
            }
            b'(' => {
                depth += 1;
                at + 1
            }
            b')' if depth == 0 => return Ok(at),
            b')' => {
                depth -= 1;
                at + 1
            }
            _ => at + 1,
        };
        word_start = matches!(byte, b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(');
        at = next;
    }
    Err(end_of_text())
}

fn skip_double_quoted(
    input: &[u8],
    mut at: usize,
    final_input: bool,
) -> Result<usize, SyntaxError> {
    while let Some(&byte) = input.get(at) {
        at = match byte {
            b'"' => return Ok(at + 1),
            b'\\' => at + 2,
            b'$' if input.get(at + 1) == Some(&b'(') => {
                at + 2 + substitution_end(&input[at + 2..], final_input)? + 1
            }
            b'`' => match skip_backquoted(input, at + 1) {
                Some(after) => after,
                None => break,
            },
            _ => at + 1,
        };
    }
    match final_input {
        true => Err(SyntaxError::UnexpectedEnd(b'"')),
        false => Err(SyntaxError::Incomplete),
    }
}

fn skip_backquoted(input: &[u8], mut at: usize) -> Option<usize> {
    while let Some(&byte) = input.get(at) {
        match byte {
            b'`' => return Some(at + 1),
            b'\\' => at += 2,
            _ => at += 1,
        }
    }
    None
}

fn brace_word_end(input: &[u8], mut at: usize, quoted: bool) -> Option<usize> {
    let mut depth = 0usize;
    while let Some(&byte) = input.get(at) {
        at = match byte {
            b'\\' => at + 2,
            b'\'' if !quoted => at + 1 + input[at + 1..].iter().position(|&b| b == b'\'')? + 1,
            b'"' => skip_double_quoted(input, at + 1, true).ok()?,
            b'`' => skip_backquoted(input, at + 1)?,
            b'$' if input.get(at + 1) == Some(&b'(') => {
                at + 2 + substitution_end(&input[at + 2..], true).ok()? + 1
            }
            b'$' if input.get(at + 1) == Some(&b'{') => {
                depth += 1;
                at + 2
            }
            b'}' if depth == 0 => return Some(at),
            b'}' => {
                depth -= 1;
                at + 1
            }
            _ => at + 1,
        };
    }
    None
}

fn word_length(input: &[u8]) -> usize {
    let mut at = 0;
    while let Some(&byte) = input.get(at) {
        at = match byte {
            b'\\' => at + 2,
            b'\'' | b'"' => match input[at + 1..].iter().position(|&other| other == byte) {
                Some(close) => at + 1 + close + 1,
                None => input.len(),
            },
            _ if ends_a_word(Some(byte)) => return at,
            _ => at + 1,
        };
    }
    input.len()
}

fn ends_a_word(byte: Option<u8>) -> bool {
    matches!(
        byte,
        None | Some(b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
    )
}

#[derive(Debug, Clone, Copy)]
struct Ifs {
    bytes: [u8; IFS_CAPACITY],
    length: usize,
}

impl Ifs {
    fn of(value: Value<'_>) -> Self {
        let text = match value {
            Value::Text(text) => text,
            Value::Number(_) | Value::Unset => DEFAULT_IFS,
        };
        let length = text.len().min(IFS_CAPACITY);
        let mut bytes = [0; IFS_CAPACITY];
        bytes[..length].copy_from_slice(&text[..length]);
        Self { bytes, length }
    }

    fn contains(&self, byte: u8) -> bool {
        self.bytes[..self.length].contains(&byte)
    }

    fn first(&self) -> Option<u8> {
        self.bytes[..self.length].first().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Split {
    InField,
    AfterBlank { ended: bool },
    AfterSeparator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Plain,
    Default { colon: bool },
    Assign { colon: bool },
    Error { colon: bool },
    Alternative { colon: bool },
    Remove { side: Side, longest: bool },
}

fn removal(side: Side, longest: bool) -> Operator {
    Operator::Remove { side, longest }
}

struct Scanner<'i, 'w> {
    input: &'i [u8],
    at: usize,
    final_input: bool,
    words: &'w mut Words,
    in_word: bool,
    tokens: bool,
    empty_allowed: bool,
    assignment: bool,
    assignments: bool,
    splitting: bool,
    redirection: Option<(u8, Redirection)>,
    descriptor: Option<u8>,
    redirected: bool,
    ifs: Ifs,
    split: Split,
    vanish: bool,
    newlines: usize,
}

impl Scanner<'_, '_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.input.get(self.at + ahead).copied()
    }

    fn end_of_text(&self, quote: u8) -> SyntaxError {
        if self.final_input {
            SyntaxError::UnexpectedEnd(quote)
        } else {
            SyntaxError::Incomplete
        }
    }

    fn end_word(&mut self) -> Result<(), SyntaxError> {
        if self.in_word {
            let vanished = self.vanish && self.words.length == self.words.word_start();
            match self.redirection.take() {
                Some((descriptor, redirection)) => {
                    self.words.finish_redirection(descriptor, redirection)?;
                    self.redirected = true;
                }
                None if vanished => {}
                None => self.words.finish(self.assignment)?,
            }
            self.in_word = false;
            self.assignment = false;
        }
        self.vanish = false;
        Ok(())
    }

    fn push(&mut self, byte: u8) -> Result<(), SyntaxError> {
        self.in_word = true;
        self.tokens = true;
        self.words.push(byte, false)
    }

    fn push_quoted(&mut self, byte: u8) -> Result<(), SyntaxError> {
        self.in_word = true;
        self.tokens = true;
        self.words.push(byte, true)
    }

    fn expansion_start(&mut self) {
        self.tokens = true;
        self.split = match self.in_word {
            true => Split::InField,
            false => Split::AfterBlank { ended: false },
        };
        self.newlines = 0;
    }

    fn expanded(&mut self, byte: u8, quoted: bool) -> Result<(), SyntaxError> {
        if quoted || self.assignment || !self.splitting || !self.ifs.contains(byte) {
            self.split = Split::InField;
            return match quoted {
                true => self.push_quoted(byte),
                false => self.push(byte),
            };
        }
        let blank = matches!(byte, b' ' | b'\t' | b'\n');
        match (blank, self.split) {
            (true, Split::InField) => {
                self.end_word()?;
                self.split = Split::AfterBlank { ended: true };
            }
            (true, _) => {}
            (false, Split::InField | Split::AfterBlank { ended: true }) => {
                self.end_word()?;
                self.split = Split::AfterSeparator;
            }
            (false, _) => {
                self.end_word()?;
                self.words.finish(false)?;
                self.split = Split::AfterSeparator;
            }
        }
        Ok(())
    }

    fn append(&mut self, value: Value<'_>, quoted: bool) -> Result<(), SyntaxError> {
        let mut digits = [0; 20];
        let text = match value {
            Value::Text(text) => text,
            Value::Number(number) => decimal(number, &mut digits),
            Value::Unset => b"",
        };
        self.expansion_start();
        if quoted {
            self.in_word = true;
        }
        for &byte in text {
            self.expanded(byte, quoted)?;
        }
        Ok(())
    }

    fn feed(&mut self, output: &[u8], quoted: bool) -> Result<(), SyntaxError> {
        for &byte in output {
            if byte == b'\n' {
                self.newlines += 1;
                continue;
            }
            while self.newlines > 0 {
                self.newlines -= 1;
                self.expanded(b'\n', quoted)?;
            }
            self.expanded(byte, quoted)?;
        }
        Ok(())
    }

    fn terminate(&mut self, terminator: Terminator) -> Result<Parsed, SyntaxError> {
        self.end_word()?;
        if self.redirection.is_some() || self.descriptor.is_some() {
            return Err(SyntaxError::UnexpectedToken(terminator.token()));
        }
        let empty = !self.tokens && !self.redirected && !self.empty_allowed;
        if !matches!(terminator, Terminator::Newline | Terminator::End) && empty {
            return Err(SyntaxError::UnexpectedToken(terminator.token()));
        }
        self.at += terminator.length();
        Ok(Parsed {
            consumed: self.at,
            terminator,
        })
    }

    fn redirect(
        &mut self,
        redirection: Redirection,
        token: &'static str,
    ) -> Result<(), SyntaxError> {
        self.end_word()?;
        if self.redirection.is_some() {
            return Err(SyntaxError::UnexpectedToken(token));
        }
        let default = match redirection {
            Redirection::Input => 0,
            Redirection::Output | Redirection::Append => 1,
        };
        self.at += token.len();
        self.redirection = Some((self.descriptor.take().unwrap_or(default), redirection));
        Ok(())
    }

    fn duplicate(&mut self, token: &'static str, default: u8) -> Result<(), SyntaxError> {
        self.end_word()?;
        if self.redirection.is_some() {
            return Err(SyntaxError::UnexpectedToken(token));
        }
        self.at += token.len();
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.at += 1;
        }
        match self.peek(0) {
            Some(source @ b'0'..=b'9') if ends_a_word(self.peek(1)) => {
                self.at += 1;
                let descriptor = self.descriptor.take().unwrap_or(default);
                self.words.add_redirection(
                    descriptor,
                    StoredKind::Duplicate,
                    usize::from(source - b'0'),
                )?;
                self.redirected = true;
                Ok(())
            }
            None if !self.final_input => Err(SyntaxError::Incomplete),
            _ => Err(SyntaxError::NotSupported(token)),
        }
    }

    fn document(&mut self) -> Result<(), SyntaxError> {
        self.end_word()?;
        if self.redirection.is_some() {
            return Err(SyntaxError::UnexpectedToken("<<"));
        }
        let position = self.at;
        match self.peek(2) {
            Some(b'<') => return Err(SyntaxError::NotSupported("<<<")),
            Some(b'-') => self.at += 3,
            _ => self.at += 2,
        }
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.at += 1;
        }
        let length = word_length(&self.input[self.at..]);
        if self.at + length == self.input.len() && !self.final_input {
            return Err(SyntaxError::Incomplete);
        }
        if length == 0 {
            let next = self.peek(0).map_or(Terminator::End, |byte| match byte {
                b';' => Terminator::Semicolon,
                b'|' => Terminator::Pipe,
                b'&' => Terminator::Background,
                b')' => Terminator::Close,
                _ => Terminator::Newline,
            });
            return Err(SyntaxError::UnexpectedToken(next.token()));
        }
        self.at += length;
        let descriptor = self.descriptor.take().unwrap_or(0);
        self.words
            .add_redirection(descriptor, StoredKind::Document, position)?;
        self.redirected = true;
        self.tokens = true;
        Ok(())
    }

    fn tilde_ends_here(&self) -> bool {
        matches!(
            self.peek(1),
            None | Some(b'/' | b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b')')
        )
    }

    fn assignment_name(&mut self) -> Result<bool, SyntaxError> {
        let rest = &self.input[self.at..];
        let name = rest.iter().take_while(|&&byte| is_name_byte(byte)).count();
        if name == 0 || !is_name_start(rest[0]) || rest.get(name) != Some(&b'=') {
            return Ok(false);
        }
        for index in 0..=name {
            self.push(self.input[self.at + index])?;
        }
        self.at += name + 1;
        self.assignment = true;
        Ok(true)
    }

    fn comment(&mut self) -> Result<(), SyntaxError> {
        match self.input[self.at..].iter().position(|&byte| byte == b'\n') {
            Some(newline) => {
                self.at += newline;
                Ok(())
            }
            None if self.final_input => {
                self.at = self.input.len();
                Ok(())
            }
            None => Err(SyntaxError::Incomplete),
        }
    }

    fn single_quoted(&mut self) -> Result<(), SyntaxError> {
        let start = self.at + 1;
        let Some(length) = self.input[start..].iter().position(|&byte| byte == b'\'') else {
            return Err(self.end_of_text(b'\''));
        };
        self.in_word = true;
        self.tokens = true;
        for index in start..start + length {
            self.words.push(self.input[index], true)?;
        }
        self.at = start + length + 1;
        Ok(())
    }

    fn escaped(&mut self) -> Result<(), SyntaxError> {
        match self.peek(1) {
            None if self.final_input => {
                self.push(b'\\')?;
                self.at += 1;
            }
            None => return Err(SyntaxError::Incomplete),
            Some(b'\n') => self.at += 2,
            Some(byte) => {
                self.push_quoted(byte)?;
                self.at += 2;
            }
        }
        Ok(())
    }
}

struct Lexer<'i, 'p, 'w, P> {
    parameters: &'p mut P,
    scan: Scanner<'i, 'w>,
}

impl<'i, 'p, 'w, P: Parameters> Lexer<'i, 'p, 'w, P> {
    fn new(
        input: &'i [u8],
        final_input: bool,
        parameters: &'p mut P,
        words: &'w mut Words,
        reading: Reading,
    ) -> Self {
        words.clear();
        let ifs = Ifs::of(parameters.value(b"IFS"));
        Self {
            parameters,
            scan: Scanner {
                input,
                at: 0,
                final_input,
                words,
                in_word: false,
                tokens: false,
                empty_allowed: reading == Reading::Trailer,
                assignment: false,
                assignments: matches!(reading, Reading::Command | Reading::Trailer),
                splitting: reading != Reading::Word,
                redirection: None,
                descriptor: None,
                redirected: false,
                ifs,
                split: Split::AfterBlank { ended: false },
                vanish: false,
                newlines: 0,
            },
        }
    }

    fn command(&mut self) -> Result<Parsed, SyntaxError> {
        loop {
            let Some(byte) = self.scan.peek(0) else {
                if !self.scan.final_input {
                    return Err(SyntaxError::Incomplete);
                }
                return self.scan.terminate(Terminator::End);
            };
            let next = self.scan.peek(1);
            let doubled = next == Some(byte);
            let scan = &mut self.scan;
            match byte {
                b' ' | b'\t' => {
                    scan.end_word()?;
                    scan.at += 1;
                }
                b'\n' => return scan.terminate(Terminator::Newline),
                b';' if doubled => return scan.terminate(Terminator::CaseEnd),
                b';' => return scan.terminate(Terminator::Semicolon),
                b'#' if !scan.in_word => scan.comment()?,
                b'|' if doubled => return scan.terminate(Terminator::Or),
                b'|' if next == Some(b'&') => return Err(SyntaxError::NotSupported("|&")),
                b'|' => return scan.terminate(Terminator::Pipe),
                b'&' if doubled => return scan.terminate(Terminator::And),
                b'&' if next == Some(b'>') => return Err(SyntaxError::NotSupported("&>")),
                b'&' => return scan.terminate(Terminator::Background),
                b'<' if doubled => scan.document()?,
                b'<' if next == Some(b'&') => scan.duplicate("<&", 0)?,
                b'<' => scan.redirect(Redirection::Input, "<")?,
                b'>' if doubled => scan.redirect(Redirection::Append, ">>")?,
                b'>' if next == Some(b'&') => scan.duplicate(">&", 1)?,
                b'>' if next == Some(b'|') => scan.redirect(Redirection::Output, ">|")?,
                b'>' => scan.redirect(Redirection::Output, ">")?,
                b'0'..=b'9'
                    if !scan.in_word
                        && scan.redirection.is_none()
                        && matches!(next, Some(b'<' | b'>')) =>
                {
                    scan.descriptor = Some(byte - b'0');
                    scan.at += 1;
                }
                b'(' => return Err(SyntaxError::UnexpectedToken("(")),
                b')' => return scan.terminate(Terminator::Close),
                _ => {
                    let assignment = !scan.in_word
                        && scan.assignments
                        && scan.redirection.is_none()
                        && scan.words.only_assignments()
                        && scan.assignment_name()?;
                    if !assignment {
                        self.word_byte(byte)?;
                    }
                }
            }
        }
    }

    fn word(&mut self) -> Result<Option<usize>, SyntaxError> {
        while matches!(self.scan.peek(0), Some(b' ' | b'\t')) {
            self.scan.at += 1;
        }
        let start = self.scan.at;
        loop {
            let byte = self.scan.peek(0);
            if byte.is_none() && !self.scan.final_input {
                return Err(SyntaxError::Incomplete);
            }
            match byte {
                Some(byte) if !ends_a_word(Some(byte)) => self.word_byte(byte)?,
                _ => {
                    self.scan.end_word()?;
                    return Ok((self.scan.at > start).then_some(self.scan.at));
                }
            }
        }
    }

    fn word_byte(&mut self, byte: u8) -> Result<(), SyntaxError> {
        self.scan.tokens = true;
        match byte {
            b'\'' => self.scan.single_quoted(),
            b'"' => self.double_quoted(),
            b'\\' => self.scan.escaped(),
            b'$' => self.dollar(false),
            b'`' => self.backquoted(false),
            b'~' if !self.scan.in_word && self.scan.tilde_ends_here() => {
                self.scan.at += 1;
                self.scan.in_word = true;
                let home = self.parameters.value(b"HOME");
                self.scan.append(home, true)
            }
            _ => {
                self.scan.at += 1;
                self.scan.push(byte)
            }
        }
    }

    fn double_quoted(&mut self) -> Result<(), SyntaxError> {
        self.scan.at += 1;
        self.scan.in_word = true;
        loop {
            let Some(byte) = self.scan.peek(0) else {
                return Err(self.scan.end_of_text(b'"'));
            };
            if byte == b'"' {
                self.scan.at += 1;
                return Ok(());
            }
            self.quoted_byte(byte)?;
        }
    }

    fn quoted_byte(&mut self, byte: u8) -> Result<(), SyntaxError> {
        match byte {
            b'\\' => match self.scan.peek(1) {
                None if self.scan.final_input => {
                    self.scan.at += 1;
                    self.scan.push_quoted(b'\\')
                }
                None => Err(self.scan.end_of_text(b'"')),
                Some(b'\n') => {
                    self.scan.at += 2;
                    Ok(())
                }
                Some(escaped @ (b'$' | b'`' | b'"' | b'\\')) => {
                    self.scan.at += 2;
                    self.scan.push_quoted(escaped)
                }
                Some(_) => {
                    self.scan.at += 1;
                    self.scan.push_quoted(b'\\')
                }
            },
            b'$' => self.dollar(true),
            b'`' => self.backquoted(true),
            _ => {
                self.scan.at += 1;
                self.scan.push_quoted(byte)
            }
        }
    }

    fn dollar(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        let input = self.scan.input;
        let at = self.scan.at;
        let Some(next) = self.scan.peek(1) else {
            if !self.scan.final_input {
                return Err(SyntaxError::Incomplete);
            }
            self.scan.at += 1;
            return match quoted {
                true => self.scan.push_quoted(b'$'),
                false => self.scan.push(b'$'),
            };
        };
        match next {
            b'(' if self.scan.peek(2) == Some(b'(') => Err(SyntaxError::NotSupported("$((")),
            b'(' => {
                let start = at + 2;
                let end = start + substitution_end(&input[start..], self.scan.final_input)?;
                self.scan.at = end + 1;
                self.substitute(&input[start..end], false, quoted)
            }
            b'{' => self.braced(quoted),
            b'@' | b'*' => {
                self.scan.at += 2;
                self.positional_all(next == b'@', quoted)
            }
            b'?' | b'$' | b'#' | b'!' | b'0'..=b'9' => {
                self.scan.at += 2;
                let value = self.parameters.value(&input[at + 1..at + 2]);
                self.scan.append(value, quoted)
            }
            _ if is_name_start(next) => {
                let start = at + 1;
                let length = input[start..]
                    .iter()
                    .take_while(|&&byte| is_name_byte(byte))
                    .count();
                if start + length == input.len() && !self.scan.final_input {
                    return Err(SyntaxError::Incomplete);
                }
                self.scan.at = start + length;
                let value = self.parameters.value(&input[start..start + length]);
                self.scan.append(value, quoted)
            }
            _ => {
                self.scan.at += 1;
                match quoted {
                    true => self.scan.push_quoted(b'$'),
                    false => self.scan.push(b'$'),
                }
            }
        }
    }

    fn backquoted(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        let input = self.scan.input;
        let start = self.scan.at + 1;
        let Some(after) = skip_backquoted(input, start) else {
            return Err(self.scan.end_of_text(b'`'));
        };
        self.scan.at = after;
        self.substitute(&input[start..after - 1], true, quoted)
    }

    fn substitute(
        &mut self,
        command: &[u8],
        backquoted: bool,
        quoted: bool,
    ) -> Result<(), SyntaxError> {
        self.scan.expansion_start();
        if quoted {
            self.scan.in_word = true;
        }
        if !P::EXPANDS {
            return Ok(());
        }
        let Lexer { parameters, scan } = self;
        parameters.substitute(command, backquoted, &mut |output| scan.feed(output, quoted))
    }

    fn positional_all(&mut self, each: bool, quoted: bool) -> Result<(), SyntaxError> {
        self.scan.tokens = true;
        let count = self.parameters.positional_count();
        let joined = !each || self.scan.assignment || !self.scan.splitting;
        if quoted && each && !joined {
            if count == 0 {
                self.scan.vanish = true;
                return Ok(());
            }
            for number in 1..=count {
                if number > 1 {
                    self.scan.end_word()?;
                    self.scan.in_word = true;
                }
                for &byte in self.parameters.positional(number) {
                    self.scan.words.push(byte, true)?;
                }
            }
            return Ok(());
        }
        if quoted || joined {
            let separator = match each {
                true => Some(b' '),
                false => self.scan.ifs.first(),
            };
            self.scan.expansion_start();
            if quoted {
                self.scan.in_word = true;
            }
            for number in 1..=count {
                if number > 1
                    && let Some(separator) = separator
                {
                    self.scan.expanded(separator, quoted)?;
                }
                for &byte in self.parameters.positional(number) {
                    self.scan.expanded(byte, quoted)?;
                }
            }
            return Ok(());
        }
        for number in 1..=count {
            if number > 1 {
                self.scan.end_word()?;
            }
            let value = Value::Text(self.parameters.positional(number));
            self.scan.append(value, false)?;
        }
        Ok(())
    }

    fn braced(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        let input = self.scan.input;
        let start = self.scan.at + 2;
        let length_of = input.get(start) == Some(&b'#') && input.get(start + 1) != Some(&b'}');
        let name_start = if length_of { start + 1 } else { start };
        let name_end = match input.get(name_start) {
            Some(&byte) if is_name_start(byte) => {
                name_start
                    + input[name_start..]
                        .iter()
                        .take_while(|&&byte| is_name_byte(byte))
                        .count()
            }
            Some(b'0'..=b'9') => {
                name_start
                    + input[name_start..]
                        .iter()
                        .take_while(|byte| byte.is_ascii_digit())
                        .count()
            }
            Some(b'?' | b'$' | b'#' | b'!' | b'@' | b'*' | b'-') => name_start + 1,
            None => return Err(self.scan.end_of_text(b'{')),
            Some(_) => return Err(SyntaxError::BadSubstitution),
        };
        let name = &input[name_start..name_end];
        let rest = &input[name_end..];
        let (operator, word_start) = match rest {
            [] => return Err(self.scan.end_of_text(b'{')),
            [b'}', ..] => (Operator::Plain, name_end),
            [b':', b'-', ..] => (Operator::Default { colon: true }, name_end + 2),
            [b':', b'=', ..] => (Operator::Assign { colon: true }, name_end + 2),
            [b':', b'?', ..] => (Operator::Error { colon: true }, name_end + 2),
            [b':', b'+', ..] => (Operator::Alternative { colon: true }, name_end + 2),
            [b'-', ..] => (Operator::Default { colon: false }, name_end + 1),
            [b'=', ..] => (Operator::Assign { colon: false }, name_end + 1),
            [b'?', ..] => (Operator::Error { colon: false }, name_end + 1),
            [b'+', ..] => (Operator::Alternative { colon: false }, name_end + 1),
            [b'#', b'#', ..] => (removal(Side::Prefix, true), name_end + 2),
            [b'#', ..] => (removal(Side::Prefix, false), name_end + 1),
            [b'%', b'%', ..] => (removal(Side::Suffix, true), name_end + 2),
            [b'%', ..] => (removal(Side::Suffix, false), name_end + 1),
            _ => return Err(SyntaxError::BadSubstitution),
        };
        let all = matches!(name, b"@" | b"*");
        if (length_of || all) && operator != Operator::Plain {
            return Err(SyntaxError::BadSubstitution);
        }
        let word_end = match operator {
            Operator::Plain => name_end,
            _ => match brace_word_end(input, word_start, quoted) {
                Some(end) => end,
                None => return Err(self.scan.end_of_text(b'{')),
            },
        };
        self.scan.at = word_end + 1;
        if all {
            return self.positional_all(name == b"@", quoted);
        }
        self.scan.tokens = true;
        if !P::EXPANDS {
            if quoted {
                self.scan.in_word = true;
            }
            return Ok(());
        }
        if length_of {
            let length = match self.parameters.value(name) {
                Value::Text(text) => text.len() as u64,
                Value::Number(number) => decimal(number, &mut [0; 20]).len() as u64,
                Value::Unset => 0,
            };
            return self.scan.append(Value::Number(length), quoted);
        }
        let (set, empty) = match self.parameters.value(name) {
            Value::Unset => (false, true),
            Value::Text(text) => (true, text.is_empty()),
            Value::Number(_) => (true, false),
        };
        let missing = |colon: bool| !set || (colon && empty);
        match operator {
            Operator::Plain => {
                let value = self.parameters.value(name);
                self.scan.append(value, quoted)
            }
            Operator::Default { colon } if missing(colon) => {
                self.expand_range(word_start, word_end, quoted, true)
            }
            Operator::Alternative { colon } if !missing(colon) => {
                self.expand_range(word_start, word_end, quoted, true)
            }
            Operator::Alternative { .. } => {
                self.scan.expansion_start();
                if quoted {
                    self.scan.in_word = true;
                }
                Ok(())
            }
            Operator::Assign { colon } if missing(colon) => {
                if !is_name(name) {
                    return Err(SyntaxError::BadSubstitution);
                }
                let (mark, in_word) = (self.scan.words.length, self.scan.in_word);
                self.expand_range(word_start, word_end, true, false)?;
                let Lexer { parameters, scan } = self;
                let assigned = parameters.assign(name, scan.words.tail(mark));
                scan.words.length = mark;
                scan.in_word = in_word;
                assigned?;
                let value = self.parameters.value(name);
                self.scan.append(value, quoted)
            }
            Operator::Error { colon } if missing(colon) => {
                let mark = self.scan.words.length;
                self.expand_range(word_start, word_end, true, false)?;
                let Lexer { parameters, scan } = self;
                let message = match scan.words.tail(mark) {
                    [] => UNSET_MESSAGE,
                    message => message,
                };
                parameters.fail(name, message);
                scan.words.length = mark;
                Err(SyntaxError::Failed)
            }
            Operator::Remove { side, longest } => {
                let (mark, in_word) = (self.scan.words.length, self.scan.in_word);
                self.expand_range(word_start, word_end, false, false)?;
                let mut buffer = [0u8; PATTERN_CAPACITY];
                let words = &self.scan.words;
                let pattern = words.escaped(mark, words.length, &mut buffer);
                self.scan.words.length = mark;
                self.scan.in_word = in_word;
                let pattern = pattern.ok_or(SyntaxError::TooLong)?;
                let mut digits = [0; 20];
                let value = match self.parameters.value(name) {
                    Value::Text(text) => text,
                    Value::Number(number) => decimal(number, &mut digits),
                    Value::Unset => b"",
                };
                let kept = crate::pattern::remove(value, pattern, side, longest);
                self.scan.expansion_start();
                if quoted {
                    self.scan.in_word = true;
                }
                for &byte in kept {
                    self.scan.expanded(byte, quoted)?;
                }
                Ok(())
            }
            _ => {
                let value = self.parameters.value(name);
                self.scan.append(value, quoted)
            }
        }
    }

    fn expand_range(
        &mut self,
        start: usize,
        end: usize,
        quoted: bool,
        splitting: bool,
    ) -> Result<(), SyntaxError> {
        let (input, at, final_input, kept_splitting) = (
            self.scan.input,
            self.scan.at,
            self.scan.final_input,
            self.scan.splitting,
        );
        self.scan.input = &input[..end];
        self.scan.at = start;
        self.scan.final_input = true;
        self.scan.splitting = kept_splitting && splitting;
        self.scan.expansion_start();
        if quoted {
            self.scan.in_word = true;
        }
        let result = self.range_bytes(quoted);
        self.scan.input = input;
        self.scan.at = at;
        self.scan.final_input = final_input;
        self.scan.splitting = kept_splitting;
        result
    }

    fn range_bytes(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        while let Some(byte) = self.scan.peek(0) {
            match byte {
                b'"' if quoted => self.scan.at += 1,
                _ if quoted => self.quoted_byte(byte)?,
                b'\'' | b'"' | b'\\' | b'$' | b'`' => self.word_byte(byte)?,
                _ => {
                    self.scan.at += 1;
                    self.scan.expanded(byte, false)?;
                }
            }
        }
        Ok(())
    }
}

/// Where a scan for the end of a line stands: what the bytes so far left open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Quoting {
    /// Inside a word, nothing open.
    #[default]
    Plain,
    /// Between words: a `#` here starts a comment.
    WordStart,
    /// Inside `'...'`.
    Single,
    /// Inside `"..."`.
    Double,
    /// After a `\`, inside `"..."` if `double`.
    Escaped {
        /// Whether the `\` is inside `"..."`.
        double: bool,
    },
    /// Inside a comment.
    Comment,
}

/// Scans `bytes` from where `quoting` stands for the end of a line: a newline outside quotes
/// and not escaped. Returns the length up to it, newline included, and leaves `quoting` at
/// [`Quoting::WordStart`]; or `None` and leaves `quoting` where the bytes end. For skipping a
/// line the shell refuses without running what an open quote holds.
pub fn line_end(quoting: &mut Quoting, bytes: &[u8]) -> Option<usize> {
    for (index, &byte) in bytes.iter().enumerate() {
        *quoting = match (*quoting, byte) {
            (Quoting::Single, b'\'') => Quoting::Plain,
            (Quoting::Single, _) => Quoting::Single,
            (Quoting::Double, b'"') => Quoting::Plain,
            (Quoting::Double, b'\\') => Quoting::Escaped { double: true },
            (Quoting::Double, _) => Quoting::Double,
            (Quoting::Escaped { double: true }, _) => Quoting::Double,
            (Quoting::Escaped { double: false }, _) => Quoting::Plain,
            (Quoting::Comment, b'\n') | (_, b'\n') => {
                *quoting = Quoting::WordStart;
                return Some(index + 1);
            }
            (Quoting::Comment, _) => Quoting::Comment,
            (Quoting::WordStart, b'#') => Quoting::Comment,
            (_, b'\'') => Quoting::Single,
            (_, b'"') => Quoting::Double,
            (_, b'\\') => Quoting::Escaped { double: false },
            (_, b' ' | b'\t' | b';') => Quoting::WordStart,
            _ => Quoting::Plain,
        };
    }
    None
}

/// `value` in decimal, written at the end of `buffer`.
pub fn decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut start = buffer.len();
    loop {
        start -= 1;
        buffer[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &buffer[start..];
        }
    }
}

#[cfg(test)]
mod tests;
