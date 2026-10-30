//! The command language (D107): one simple command at a time, read from text and expanded
//! into words. Words are split on blanks; `'...'` keeps everything, `"..."` keeps all but
//! `$` expansions and `\` before `$`, `` ` ``, `"`, `\` and a newline; an unquoted `\` keeps
//! the next character, and `\` before a newline joins the lines. `#` at the start of a word
//! comments to the end of the line. `$NAME`, `${NAME}`, `$?`, `$$`, `$#`, `$!` and `$0` to `$9`
//! expand; unquoted, their values are split on blanks too. `~` alone or before `/` at the
//! start of a word is `$HOME`. `NAME=value` words before the command are assignments.
//! Commands end at `;`, a newline, the end of the text, `|`, which pipes the command's
//! output into the next one (D125), `&&` and `||`, which run the next pipeline only if this
//! one succeeded or failed, and `&`, which runs the pipeline in the background (D181).
//! `< file`, `> file` and `>> file` anywhere in a command redirect its input and output; the
//! word after the operator names the file. A digit right before the operator names the
//! descriptor (`2> file`), and `>&1`, `2>&1` or `<&0` make a descriptor a copy of another
//! (D181). The operators other shells have that eggsh does not (`<<`, `(`, `)`, `` ` ``, `$(`,
//! `&>`, `|&`) are refused as not supported yet, never read as words, so adding them later
//! changes no command that works now.

/// The most bytes a command's words take, each with its terminating NUL.
pub const WORDS_CAPACITY: usize = 8192;

/// The most words a command has, assignments included.
pub const MAX_WORDS: usize = 256;

/// The most redirections a command has.
pub const MAX_REDIRECTIONS: usize = 8;

/// The most bytes a command's redirections' file names take, each with its terminating NUL.
pub const REDIRECTIONS_CAPACITY: usize = 4096;

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
}

/// Why a command could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxError {
    /// The text ends inside the command: more may complete it.
    Incomplete,
    /// The text ended for good inside a quote (the quote character) or `${`.
    UnexpectedEnd(u8),
    /// An operator the shell does not support yet.
    NotSupported(&'static str),
    /// An operator where a command should be.
    UnexpectedToken(&'static str),
    /// A `${...}` that is not a name or a special parameter.
    BadSubstitution,
    /// More than [`WORDS_CAPACITY`] bytes or [`MAX_WORDS`] words.
    TooLong,
    /// `&` after a list joined by `&&` or `||`, which the shell cannot run in the background
    /// yet (D181).
    BackgroundList,
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
}

impl Terminator {
    /// How many bytes the terminator takes in the text.
    pub fn length(self) -> usize {
        match self {
            Self::End => 0,
            Self::And | Self::Or => 2,
            _ => 1,
        }
    }

    fn needs_command(self) -> bool {
        matches!(self, Self::Pipe | Self::And | Self::Or)
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

/// Where expansions get their values.
pub trait Parameters {
    /// The value of `name`: a variable's name, or one of `?`, `$`, `#`, `!` and `0` to `9`.
    fn value(&self, name: &[u8]) -> Value<'_>;
}

/// No parameter is set: for checking a line's syntax without expanding it.
#[derive(Debug, Clone, Copy)]
pub struct NoParameters;

impl Parameters for NoParameters {
    fn value(&self, _: &[u8]) -> Value<'_> {
        Value::Unset
    }
}

/// Where a command's words go.
pub trait Sink {
    /// Adds `byte` to the word being made.
    fn push(&mut self, byte: u8) -> Result<(), SyntaxError>;
    /// Ends the word being made, an assignment if `assignment`.
    fn finish(&mut self, assignment: bool) -> Result<(), SyntaxError>;
    /// Ends the word being made as the file descriptor `descriptor` opens for `redirection`,
    /// not one of the command's words.
    fn finish_redirection(
        &mut self,
        descriptor: u8,
        redirection: Redirection,
    ) -> Result<(), SyntaxError>;
    /// Records that descriptor `descriptor` becomes a copy of descriptor `source`.
    fn duplicate(&mut self, descriptor: u8, source: u8) -> Result<(), SyntaxError>;
    /// Forgets every word.
    fn clear(&mut self);
    /// How many words were finished.
    fn count(&self) -> usize;
    /// Whether every finished word is an assignment (true with none).
    fn only_assignments(&self) -> bool;
}

/// A command's words, each ended by a NUL, the assignments first, and its redirections.
#[derive(Debug, Clone)]
pub struct Words {
    bytes: [u8; WORDS_CAPACITY],
    length: usize,
    ends: [u16; MAX_WORDS],
    count: usize,
    assignments: usize,
    files: [u8; REDIRECTIONS_CAPACITY],
    files_length: usize,
    redirections: [StoredRedirect; MAX_REDIRECTIONS],
    redirection_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct StoredRedirect {
    descriptor: u8,
    kind: Option<Redirection>,
    start: u16,
    end: u16,
}

impl Default for Words {
    fn default() -> Self {
        Self {
            bytes: [0; WORDS_CAPACITY],
            length: 0,
            ends: [0; MAX_WORDS],
            count: 0,
            assignments: 0,
            files: [0; REDIRECTIONS_CAPACITY],
            files_length: 0,
            redirections: [StoredRedirect {
                descriptor: 0,
                kind: None,
                start: 0,
                end: 0,
            }; MAX_REDIRECTIONS],
            redirection_count: 0,
        }
    }
}

impl Words {
    /// Word `index` with its terminating NUL.
    pub fn with_nul(&self, index: usize) -> &[u8] {
        let start = match index {
            0 => 0,
            _ => usize::from(self.ends[index - 1]),
        };
        &self.bytes[start..usize::from(self.ends[index])]
    }

    /// Word `index`.
    pub fn get(&self, index: usize) -> &[u8] {
        let word = self.with_nul(index);
        &word[..word.len() - 1]
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
                Some(redirection) => Redirect::File {
                    descriptor: stored.descriptor,
                    redirection,
                    path: &self.files[usize::from(stored.start)..usize::from(stored.end)],
                },
                None => Redirect::Duplicate {
                    descriptor: stored.descriptor,
                    source: stored.start as u8,
                },
            })
    }

    fn word_start(&self) -> usize {
        match self.count {
            0 => 0,
            count => usize::from(self.ends[count - 1]),
        }
    }
}

impl Sink for Words {
    fn push(&mut self, byte: u8) -> Result<(), SyntaxError> {
        if self.length + 1 >= WORDS_CAPACITY {
            return Err(SyntaxError::TooLong);
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
            kind: Some(redirection),
            start: self.files_length as u16,
            end: end as u16,
        };
        self.redirection_count += 1;
        self.files_length = end;
        self.length = start;
        Ok(())
    }

    fn duplicate(&mut self, descriptor: u8, source: u8) -> Result<(), SyntaxError> {
        if self.redirection_count == MAX_REDIRECTIONS {
            return Err(SyntaxError::TooLong);
        }
        self.redirections[self.redirection_count] = StoredRedirect {
            descriptor,
            kind: None,
            start: u16::from(source),
            end: 0,
        };
        self.redirection_count += 1;
        Ok(())
    }

    fn clear(&mut self) {
        self.length = 0;
        self.count = 0;
        self.assignments = 0;
        self.files_length = 0;
        self.redirection_count = 0;
    }

    fn count(&self) -> usize {
        self.count
    }

    fn only_assignments(&self) -> bool {
        self.count == self.assignments
    }
}

/// Counts words without keeping them: for checking a line's syntax.
#[derive(Debug, Clone, Copy, Default)]
pub struct CountedWords {
    count: usize,
    assignments: usize,
    redirected: bool,
}

impl Sink for CountedWords {
    fn push(&mut self, _: u8) -> Result<(), SyntaxError> {
        Ok(())
    }

    fn finish(&mut self, assignment: bool) -> Result<(), SyntaxError> {
        self.count += 1;
        self.assignments += usize::from(assignment);
        Ok(())
    }

    fn finish_redirection(&mut self, _: u8, _: Redirection) -> Result<(), SyntaxError> {
        self.redirected = true;
        Ok(())
    }

    fn duplicate(&mut self, _: u8, _: u8) -> Result<(), SyntaxError> {
        self.redirected = true;
        Ok(())
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn count(&self) -> usize {
        self.count
    }

    fn only_assignments(&self) -> bool {
        self.count == self.assignments
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

fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n')
}

/// Reads the first command of `input` into `words`, expanding it with `parameters`. If
/// `final_input`, the text ends there for good; otherwise running out of text before a
/// terminator is [`SyntaxError::Incomplete`].
pub fn next_command<P: Parameters, S: Sink>(
    input: &[u8],
    final_input: bool,
    parameters: &P,
    words: &mut S,
) -> Result<Parsed, SyntaxError> {
    words.clear();
    let mut lexer = Lexer {
        input,
        at: 0,
        final_input,
        parameters,
        words,
        in_word: false,
        assignment: false,
        redirection: None,
        descriptor: None,
        redirected: false,
    };
    lexer.command()
}

/// Checks the syntax of the first line of `input`, its commands up to a newline (or the end,
/// if `final_input`), without expanding anything: the length of the line, its newline
/// included. A `|`, `&&` or `||` must be followed by a command on the same line, and a list
/// joined by `&&` or `||` cannot end with `&` yet.
pub fn check_line(input: &[u8], final_input: bool) -> Result<usize, SyntaxError> {
    let mut at = 0;
    let mut words = CountedWords::default();
    let mut needs_command = false;
    let mut in_list = false;
    loop {
        let parsed = next_command(&input[at..], final_input, &NoParameters, &mut words)?;
        at += parsed.consumed;
        if needs_command && words.count() == 0 && !words.redirected {
            return Err(SyntaxError::UnexpectedToken(terminator_token(
                parsed.terminator,
            )));
        }
        match parsed.terminator {
            Terminator::And | Terminator::Or => in_list = true,
            Terminator::Background if in_list => return Err(SyntaxError::BackgroundList),
            Terminator::Pipe => {}
            _ => in_list = false,
        }
        needs_command = parsed.terminator.needs_command();
        if matches!(parsed.terminator, Terminator::Newline | Terminator::End) {
            return Ok(at);
        }
    }
}

/// How the pipeline that starts `line` ends: the terminator of its last command, `|` passed
/// over; `End` for text whose syntax is wrong (the shell checked the line before).
pub fn pipeline_end(line: &[u8]) -> Terminator {
    let mut at = 0;
    let mut words = CountedWords::default();
    while let Ok(parsed) = next_command(&line[at..], true, &NoParameters, &mut words) {
        if parsed.terminator != Terminator::Pipe {
            return parsed.terminator;
        }
        at += parsed.consumed;
    }
    Terminator::End
}

fn terminator_token(terminator: Terminator) -> &'static str {
    match terminator {
        Terminator::Semicolon => ";",
        Terminator::Pipe => "|",
        Terminator::And => "&&",
        Terminator::Or => "||",
        Terminator::Background => "&",
        Terminator::Newline | Terminator::End => "newline",
    }
}

fn ends_a_word(byte: Option<u8>) -> bool {
    matches!(
        byte,
        None | Some(b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>')
    )
}

struct Lexer<'i, 'p, 'w, P, S> {
    input: &'i [u8],
    at: usize,
    final_input: bool,
    parameters: &'p P,
    words: &'w mut S,
    in_word: bool,
    assignment: bool,
    redirection: Option<(u8, Redirection)>,
    descriptor: Option<u8>,
    redirected: bool,
}

impl<P: Parameters, S: Sink> Lexer<'_, '_, '_, P, S> {
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
            match self.redirection.take() {
                Some((descriptor, redirection)) => {
                    self.words.finish_redirection(descriptor, redirection)?;
                    self.redirected = true;
                }
                None => self.words.finish(self.assignment)?,
            }
            self.in_word = false;
            self.assignment = false;
        }
        Ok(())
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
                self.words.duplicate(descriptor, source - b'0')?;
                self.redirected = true;
                Ok(())
            }
            None if !self.final_input => Err(SyntaxError::Incomplete),
            _ => Err(SyntaxError::NotSupported(token)),
        }
    }

    fn push(&mut self, byte: u8) -> Result<(), SyntaxError> {
        self.in_word = true;
        self.words.push(byte)
    }

    fn terminate(&mut self, terminator: Terminator) -> Result<Parsed, SyntaxError> {
        self.end_word()?;
        if self.redirection.is_some() || self.descriptor.is_some() {
            return Err(SyntaxError::UnexpectedToken(terminator_token(terminator)));
        }
        let empty = self.words.count() == 0 && !self.redirected;
        if !matches!(terminator, Terminator::Newline | Terminator::End) && empty {
            return Err(SyntaxError::UnexpectedToken(terminator_token(terminator)));
        }
        self.at += terminator.length();
        Ok(Parsed {
            consumed: self.at,
            terminator,
        })
    }

    fn command(&mut self) -> Result<Parsed, SyntaxError> {
        loop {
            let Some(byte) = self.peek(0) else {
                if !self.final_input {
                    return Err(SyntaxError::Incomplete);
                }
                return self.terminate(Terminator::End);
            };
            let next = self.peek(1);
            let doubled = next == Some(byte);
            match byte {
                b' ' | b'\t' => {
                    self.end_word()?;
                    self.at += 1;
                }
                b'\n' => return self.terminate(Terminator::Newline),
                b';' if doubled => return Err(SyntaxError::UnexpectedToken(";;")),
                b';' => return self.terminate(Terminator::Semicolon),
                b'#' if !self.in_word => self.comment()?,
                b'|' if doubled => return self.terminate(Terminator::Or),
                b'|' if next == Some(b'&') => return Err(SyntaxError::NotSupported("|&")),
                b'|' => return self.terminate(Terminator::Pipe),
                b'&' if doubled => return self.terminate(Terminator::And),
                b'&' if next == Some(b'>') => return Err(SyntaxError::NotSupported("&>")),
                b'&' => return self.terminate(Terminator::Background),
                b'<' if doubled => return Err(SyntaxError::NotSupported("<<")),
                b'<' if next == Some(b'&') => self.duplicate("<&", 0)?,
                b'<' => self.redirect(Redirection::Input, "<")?,
                b'>' if doubled => self.redirect(Redirection::Append, ">>")?,
                b'>' if next == Some(b'&') => self.duplicate(">&", 1)?,
                b'>' => self.redirect(Redirection::Output, ">")?,
                b'0'..=b'9'
                    if !self.in_word
                        && self.redirection.is_none()
                        && matches!(next, Some(b'<' | b'>')) =>
                {
                    self.descriptor = Some(byte - b'0');
                    self.at += 1;
                }
                b'(' => return Err(SyntaxError::NotSupported("(")),
                b')' => return Err(SyntaxError::NotSupported(")")),
                b'`' => return Err(SyntaxError::NotSupported("`")),
                b'\'' => self.single_quoted()?,
                b'"' => self.double_quoted()?,
                b'\\' => self.escaped()?,
                b'$' => self.dollar(false)?,
                b'~' if !self.in_word && self.tilde_ends_here() => {
                    self.at += 1;
                    self.in_word = true;
                    let home = self.parameters.value(b"HOME");
                    self.append(home, false)?;
                }
                _ => {
                    if self.in_word
                        || self.redirection.is_some()
                        || !self.words.only_assignments()
                        || !self.assignment_name()?
                    {
                        self.push(byte)?;
                        self.at += 1;
                    }
                }
            }
        }
    }

    fn tilde_ends_here(&self) -> bool {
        matches!(
            self.peek(1),
            None | Some(b'/' | b' ' | b'\t' | b'\n' | b';' | b'&' | b'|')
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
        for index in start..start + length {
            self.words.push(self.input[index])?;
        }
        self.at = start + length + 1;
        Ok(())
    }

    fn double_quoted(&mut self) -> Result<(), SyntaxError> {
        self.at += 1;
        self.in_word = true;
        loop {
            let Some(byte) = self.peek(0) else {
                return Err(self.end_of_text(b'"'));
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(());
                }
                b'\\' => match self.peek(1) {
                    None => return Err(self.end_of_text(b'"')),
                    Some(b'\n') => self.at += 2,
                    Some(escaped @ (b'$' | b'`' | b'"' | b'\\')) => {
                        self.words.push(escaped)?;
                        self.at += 2;
                    }
                    Some(_) => {
                        self.words.push(b'\\')?;
                        self.at += 1;
                    }
                },
                b'$' => self.dollar(true)?,
                b'`' => return Err(SyntaxError::NotSupported("`")),
                _ => {
                    self.words.push(byte)?;
                    self.at += 1;
                }
            }
        }
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
                self.push(byte)?;
                self.at += 2;
            }
        }
        Ok(())
    }

    fn dollar(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        let Some(next) = self.peek(1) else {
            if !self.final_input {
                return Err(SyntaxError::Incomplete);
            }
            self.at += 1;
            return self.push(b'$');
        };
        match next {
            b'(' => Err(SyntaxError::NotSupported("$(")),
            b'{' => self.braced(quoted),
            b'?' | b'$' | b'#' | b'!' | b'0'..=b'9' => {
                self.at += 2;
                let value = self.parameters.value(&[next]);
                self.append(value, quoted)
            }
            _ if is_name_start(next) => {
                let start = self.at + 1;
                let length = self.input[start..]
                    .iter()
                    .take_while(|&&byte| is_name_byte(byte))
                    .count();
                if start + length == self.input.len() && !self.final_input {
                    return Err(SyntaxError::Incomplete);
                }
                self.at = start + length;
                let value = self.parameters.value(&self.input[start..start + length]);
                self.append(value, quoted)
            }
            _ => {
                self.at += 1;
                self.push(b'$')
            }
        }
    }

    fn braced(&mut self, quoted: bool) -> Result<(), SyntaxError> {
        let start = self.at + 2;
        let Some(length) = self.input[start..].iter().position(|&byte| byte == b'}') else {
            return Err(self.end_of_text(b'{'));
        };
        let name = &self.input[start..start + length];
        let special = matches!(name, [b'?' | b'$' | b'#' | b'!' | b'0'..=b'9']);
        if !special && !is_name(name) {
            return Err(SyntaxError::BadSubstitution);
        }
        self.at = start + length + 1;
        let value = self.parameters.value(name);
        self.append(value, quoted)
    }

    fn append(&mut self, value: Value<'_>, quoted: bool) -> Result<(), SyntaxError> {
        let mut digits = [0; 20];
        let text = match value {
            Value::Text(text) => text,
            Value::Number(number) => decimal(number, &mut digits),
            Value::Unset => return Ok(()),
        };
        if quoted || self.assignment {
            self.in_word = true;
            for &byte in text {
                self.words.push(byte)?;
            }
            return Ok(());
        }
        for &byte in text {
            if is_blank(byte) {
                self.end_word()?;
            } else {
                self.push(byte)?;
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
mod tests {
    use super::*;

    struct Fixed;

    impl Parameters for Fixed {
        fn value(&self, name: &[u8]) -> Value<'_> {
            match name {
                b"HOME" => Value::Text(b"/home/egg"),
                b"X" => Value::Text(b"abc"),
                b"SPACED" => Value::Text(b"  one two\tthree "),
                b"EMPTY" => Value::Text(b""),
                b"?" => Value::Number(127),
                b"$" => Value::Number(42),
                b"#" => Value::Number(2),
                b"0" => Value::Text(b"eggsh"),
                b"1" => Value::Text(b"first arg"),
                _ => Value::Unset,
            }
        }
    }

    fn words(input: &str) -> Vec<String> {
        let (words, _) = parse(input).unwrap();
        words
    }

    fn parse(input: &str) -> Result<(Vec<String>, Parsed), SyntaxError> {
        let mut words = Words::default();
        let parsed = next_command(input.as_bytes(), true, &Fixed, &mut words)?;
        let all = (0..words.count())
            .map(|index| String::from_utf8(words.get(index).to_vec()).unwrap())
            .collect();
        Ok((all, parsed))
    }

    #[test]
    fn blanks_split_words_and_end_the_command() {
        assert_eq!(words("  echo   a\tb  "), ["echo", "a", "b"]);
        assert_eq!(words(""), Vec::<String>::new());
        let (all, parsed) = parse("a b\nc").unwrap();
        assert_eq!(all, ["a", "b"]);
        assert_eq!(parsed.consumed, 4);
        assert_eq!(parsed.terminator, Terminator::Newline);
    }

    #[test]
    fn single_quotes_keep_everything() {
        assert_eq!(
            words(r#"echo 'a  b $X \n "q"' x'y'z ''"#),
            ["echo", r#"a  b $X \n "q""#, "xyz", ""]
        );
    }

    #[test]
    fn double_quotes_expand_without_splitting_and_escape_four_characters() {
        assert_eq!(
            words(
                r#"echo "$X $SPACED" "\$X \" \\ \a" "a\
b""#
            ),
            ["echo", "abc   one two\tthree ", r#"$X " \ \a"#, "ab"]
        );
        assert_eq!(words(r#""""#), [""]);
        assert_eq!(words(r#""$UNSET""#), [""]);
    }

    #[test]
    fn backslash_keeps_the_next_character_and_joins_lines() {
        assert_eq!(words(r"a\ b \$X \\ c\;d"), [r"a b", "$X", r"\", "c;d"]);
        assert_eq!(words("ab\\\ncd"), ["abcd"]);
        assert_eq!(words(r"end\"), [r"end\"]);
        let mut words = Words::default();
        let incomplete = next_command(br"end\", false, &Fixed, &mut words);
        assert_eq!(incomplete, Err(SyntaxError::Incomplete));
    }

    #[test]
    fn unquoted_expansions_are_split_on_blanks_and_empty_ones_vanish() {
        assert_eq!(
            words("cmd $SPACED x$X$X y${X}z $EMPTY $UNSET"),
            ["cmd", "one", "two", "three", "xabcabc", "yabcz"]
        );
        assert_eq!(
            words(r#"cmd ""$SPACED"#),
            ["cmd", "", "one", "two", "three"]
        );
        assert_eq!(words("a$SPACED"), ["a", "one", "two", "three"]);
    }

    #[test]
    fn special_parameters_expand() {
        assert_eq!(
            words("echo $? $$ $# $0 ${?} \"$1\" $1x $"),
            [
                "echo",
                "127",
                "42",
                "2",
                "eggsh",
                "127",
                "first arg",
                "first",
                "argx",
                "$"
            ]
        );
        assert_eq!(words("echo $- $/ a$"), ["echo", "$-", "$/", "a$"]);
    }

    #[test]
    fn tilde_is_home_only_alone_or_before_a_slash_at_a_word_s_start() {
        assert_eq!(
            words("cd ~ ~/bin a~ ~x '~' ~;"),
            [
                "cd",
                "/home/egg",
                "/home/egg/bin",
                "a~",
                "~x",
                "~",
                "/home/egg"
            ]
        );
    }

    #[test]
    fn assignments_come_only_before_the_command() {
        let mut words = Words::default();
        next_command(b"A=1 B=\"x y\" C=$SPACED cmd D=2", true, &Fixed, &mut words).unwrap();
        assert_eq!(words.assignment_count(), 3);
        let assignments: Vec<&[u8]> = words.assignments().collect();
        assert_eq!(assignments, [&b"A=1"[..], b"B=x y", b"C=  one two\tthree "]);
        assert_eq!(words.command_length(), 2);
        assert_eq!(words.command(0), b"cmd");
        assert_eq!(words.command(1), b"D=2");
        assert_eq!(words.command_with_nul(1), b"D=2\0");
        next_command(b"1A=x =y 'A'=z", true, &Fixed, &mut words).unwrap();
        assert_eq!(words.assignment_count(), 0);
        assert_eq!(words.command_length(), 3);
        next_command(b"ONLY=", true, &Fixed, &mut words).unwrap();
        assert_eq!(words.assignment_count(), 1);
        assert_eq!(words.command_length(), 0);
    }

    #[test]
    fn comments_run_to_the_end_of_the_line() {
        let (all, parsed) = parse("echo a#b # c 'd\nnext").unwrap();
        assert_eq!(all, ["echo", "a#b"]);
        assert_eq!(parsed.terminator, Terminator::Newline);
        assert_eq!(parsed.consumed, 16);
        assert_eq!(words("# only"), Vec::<String>::new());
    }

    #[test]
    fn semicolons_separate_commands() {
        let (all, parsed) = parse("a b; c").unwrap();
        assert_eq!(all, ["a", "b"]);
        assert_eq!(parsed.terminator, Terminator::Semicolon);
        assert_eq!(parsed.consumed, 4);
        assert_eq!(parse("; a"), Err(SyntaxError::UnexpectedToken(";")));
        assert_eq!(parse("a;; b"), Err(SyntaxError::UnexpectedToken(";;")));
        assert_eq!(check_line(b"a; b ;c\nd", true), Ok(8));
        assert_eq!(check_line(b"a; b", true), Ok(4));
    }

    #[test]
    fn operators_not_supported_yet_are_refused_not_read_as_words() {
        for (input, operator) in [
            ("a << f", "<<"),
            ("(a)", "("),
            ("a)", ")"),
            ("echo `a`", "`"),
            ("echo \"`a`\"", "`"),
            ("echo $(a)", "$("),
            ("echo \"$(a)\"", "$("),
            ("a &> f", "&>"),
            ("a |& b", "|&"),
            ("a >&f", ">&"),
            ("a 2>&-", ">&"),
        ] {
            assert_eq!(
                parse(input),
                Err(SyntaxError::NotSupported(operator)),
                "{input}"
            );
        }
        assert_eq!(words("echo '|' \\& \"<>\""), ["echo", "|", "&", "<>"]);
    }

    #[test]
    fn unterminated_quotes_need_more_text_or_fail_at_the_end() {
        let mut words = Words::default();
        for input in [
            "echo 'a", "echo \"a", "echo ${X", "echo $X", "echo a", "# c",
        ] {
            let result = next_command(input.as_bytes(), false, &Fixed, &mut words);
            assert_eq!(result, Err(SyntaxError::Incomplete), "{input}");
        }
        assert_eq!(parse("echo 'a"), Err(SyntaxError::UnexpectedEnd(b'\'')));
        assert_eq!(parse("echo \"a"), Err(SyntaxError::UnexpectedEnd(b'"')));
        assert_eq!(parse("echo ${X"), Err(SyntaxError::UnexpectedEnd(b'{')));
        assert_eq!(check_line(b"a; 'b\n", false), Err(SyntaxError::Incomplete));
        assert_eq!(check_line(b"a; 'b\n'\n", false), Ok(8));
    }

    #[test]
    fn bad_substitutions_are_refused() {
        for input in ["${X:-y}", "${}", "${#X}", "${1x}"] {
            assert_eq!(parse(input), Err(SyntaxError::BadSubstitution), "{input}");
        }
    }

    #[test]
    fn too_many_words_or_bytes_are_refused() {
        let many = "a ".repeat(MAX_WORDS + 1);
        assert_eq!(parse(&many), Err(SyntaxError::TooLong));
        let long = "a".repeat(WORDS_CAPACITY);
        assert_eq!(parse(&long), Err(SyntaxError::TooLong));
        let fits = "a ".repeat(MAX_WORDS);
        assert_eq!(words(&fits).len(), MAX_WORDS);
    }

    #[test]
    fn checking_a_line_does_not_need_the_parameters() {
        assert_eq!(check_line(b"echo $X \"${Y}\" ~; Z=1\n", false), Ok(22));
        assert_eq!(check_line(b"echo ok; echo || x\n", false), Ok(19));
        assert_eq!(check_line(b"echo ok | grep o | wc\n", false), Ok(22));
    }

    #[test]
    fn and_or_and_background_end_a_pipeline() {
        for (input, terminator, consumed) in [
            ("a && b", Terminator::And, 4),
            ("a||b", Terminator::Or, 3),
            ("a & b", Terminator::Background, 3),
            ("a&", Terminator::Background, 2),
        ] {
            let mut words = Words::default();
            let parsed = next_command(input.as_bytes(), true, &Fixed, &mut words).unwrap();
            assert_eq!(parsed.terminator, terminator, "{input}");
            assert_eq!(parsed.consumed, consumed, "{input}");
            assert_eq!(words.get(0), b"a", "{input}");
        }
        assert_eq!(check_line(b"a && b || c; d & e\n", true), Ok(19));
        assert_eq!(check_line(b"a | b && c | d\n", true), Ok(15));
        assert_eq!(check_line(b"sleep 1 &\n", true), Ok(10));
        for (line, token) in [
            ("a &&\n", "newline"),
            ("a || ; b\n", ";"),
            ("&& a\n", "&&"),
            ("& a\n", "&"),
            ("a & ; b\n", ";"),
            ("a && | b\n", "|"),
        ] {
            assert_eq!(
                check_line(line.as_bytes(), true),
                Err(SyntaxError::UnexpectedToken(token)),
                "{line}"
            );
        }
        assert_eq!(
            check_line(b"a && b &\n", true),
            Err(SyntaxError::BackgroundList)
        );
        assert_eq!(check_line(b"a && b; c &\n", true), Ok(12));
        assert_eq!(pipeline_end(b"a | b | c & d"), Terminator::Background);
        assert_eq!(pipeline_end(b"a && b"), Terminator::And);
        assert_eq!(pipeline_end(b"a | b\n"), Terminator::Newline);
    }

    #[test]
    fn a_digit_before_a_redirection_names_the_descriptor_and_ampersand_copies_one() {
        let mut words = Words::default();
        next_command(
            b"cc 2> err 2>&1 >&2 <&0 0<in 3>>log x2>y 2 > z",
            true,
            &Fixed,
            &mut words,
        )
        .unwrap();
        let redirections: Vec<_> = words.redirections().collect();
        assert_eq!(
            redirections,
            [
                Redirect::File {
                    descriptor: 2,
                    redirection: Redirection::Output,
                    path: b"err\0"
                },
                Redirect::Duplicate {
                    descriptor: 2,
                    source: 1
                },
                Redirect::Duplicate {
                    descriptor: 1,
                    source: 2
                },
                Redirect::Duplicate {
                    descriptor: 0,
                    source: 0
                },
                Redirect::File {
                    descriptor: 0,
                    redirection: Redirection::Input,
                    path: b"in\0"
                },
                Redirect::File {
                    descriptor: 3,
                    redirection: Redirection::Append,
                    path: b"log\0"
                },
                Redirect::File {
                    descriptor: 1,
                    redirection: Redirection::Output,
                    path: b"y\0"
                },
                Redirect::File {
                    descriptor: 1,
                    redirection: Redirection::Output,
                    path: b"z\0"
                },
            ]
        );
        let words: Vec<&[u8]> = (0..words.command_length())
            .map(|index| words.command(index))
            .collect();
        assert_eq!(words, [&b"cc"[..], b"x2", b"2"]);
        assert_eq!(check_line(b"a 2>&1 | b\n", true), Ok(11));
        assert_eq!(words("echo 2>& 1"), ["echo"]);
    }

    #[test]
    fn the_last_background_process_is_a_special_parameter() {
        struct Background;
        impl Parameters for Background {
            fn value(&self, name: &[u8]) -> Value<'_> {
                match name {
                    b"!" => Value::Number(77),
                    _ => Value::Unset,
                }
            }
        }
        let mut words = Words::default();
        next_command(b"wait $! ${!}", true, &Background, &mut words).unwrap();
        assert_eq!(words.get(1), b"77");
        assert_eq!(words.get(2), b"77");
    }

    #[test]
    fn a_pipe_ends_a_command_and_needs_a_command_on_each_side() {
        let mut words = Words::default();
        let parsed = next_command(b"ls / | grep bin", true, &Fixed, &mut words).unwrap();
        assert_eq!(parsed.terminator, Terminator::Pipe);
        assert_eq!(parsed.consumed, 6);
        assert_eq!(words.get(1), b"/");
        assert_eq!(words.command_length(), 2);
        assert_eq!(parse("| a"), Err(SyntaxError::UnexpectedToken("|")));
        assert_eq!(
            check_line(b"a |\n", true),
            Err(SyntaxError::UnexpectedToken("newline"))
        );
        assert_eq!(
            check_line(b"a | ; b\n", true),
            Err(SyntaxError::UnexpectedToken(";"))
        );
        assert_eq!(
            check_line(b"a | | b\n", true),
            Err(SyntaxError::UnexpectedToken("|"))
        );
        assert_eq!(check_line(b"a|b|c", true), Ok(5));
    }

    #[test]
    fn redirections_take_the_next_word_as_their_file_anywhere_in_a_command() {
        let mut words = Words::default();
        next_command(
            b"X=1 <in cat >\"out file\" -n >> $HOME/log",
            true,
            &Fixed,
            &mut words,
        )
        .unwrap();
        assert_eq!(words.command_length(), 2);
        assert_eq!(words.command(0), b"cat");
        assert_eq!(words.command(1), b"-n");
        assert_eq!(words.assignments().collect::<Vec<_>>(), [b"X=1"]);
        let redirections: Vec<_> = words.redirections().collect();
        let file = |descriptor, redirection, path| Redirect::File {
            descriptor,
            redirection,
            path,
        };
        assert_eq!(
            redirections,
            [
                file(0, Redirection::Input, &b"in\0"[..]),
                file(1, Redirection::Output, b"out file\0"),
                file(1, Redirection::Append, b"/home/egg/log\0"),
            ]
        );
        next_command(b"echo a>f", true, &Fixed, &mut words).unwrap();
        assert_eq!(words.command_length(), 2);
        assert_eq!(
            words.redirections().next(),
            Some(file(1, Redirection::Output, b"f\0"))
        );
        next_command(b">f X=1", true, &Fixed, &mut words).unwrap();
        assert_eq!(words.command_length(), 0);
        assert_eq!(words.assignment_count(), 1);
        assert_eq!(
            parse("echo >"),
            Err(SyntaxError::UnexpectedToken("newline"))
        );
        assert_eq!(parse("echo > | a"), Err(SyntaxError::UnexpectedToken("|")));
        assert_eq!(parse("echo > > a"), Err(SyntaxError::UnexpectedToken(">")));
        assert_eq!(check_line(b"> f; < g\n", true), Ok(9));
        let many = "a >f ".repeat(MAX_REDIRECTIONS + 1);
        assert_eq!(parse(&many), Err(SyntaxError::TooLong));
    }

    #[test]
    fn a_line_ends_only_at_a_newline_outside_quotes_comments_and_escapes() {
        let mut quoting = Quoting::WordStart;
        assert_eq!(line_end(&mut quoting, b"echo a\nnext"), Some(7));
        assert_eq!(quoting, Quoting::WordStart);
        let text = b"echo 'a\nb' \"c\nd\" e\\\nf # it's \"\nls\n";
        let mut quoting = Quoting::WordStart;
        let first = line_end(&mut quoting, text).unwrap();
        assert_eq!(&text[..first], b"echo 'a\nb' \"c\nd\" e\\\nf # it's \"\n");
        assert_eq!(line_end(&mut quoting, &text[first..]), Some(3));
        let mut quoting = Quoting::WordStart;
        assert_eq!(line_end(&mut quoting, b"echo 'start\nyyy"), None);
        assert_eq!(quoting, Quoting::Single);
        assert_eq!(line_end(&mut quoting, b"y\nls\n'\necho after\n"), Some(7));
        let mut quoting = Quoting::WordStart;
        assert_eq!(line_end(&mut quoting, b"a#b'\nc\n"), None);
        assert_eq!(quoting, Quoting::Single);
        let mut quoting = Quoting::WordStart;
        assert_eq!(line_end(&mut quoting, b"x \"\\\"\n\"\n"), Some(8));
    }

    #[test]
    fn decimal_writes_every_digit() {
        let mut buffer = [0; 20];
        assert_eq!(decimal(0, &mut buffer), b"0");
        assert_eq!(decimal(u64::MAX, &mut buffer), b"18446744073709551615");
    }
}
