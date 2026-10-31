//! The builtins that come with the shell's language (D182): `break`, `continue`, `return`,
//! `shift`, `set` (with `--`, `-e` and `+e`), `local`, `eval`, `read`, `test` and `[`.

use super::script::Control;
use super::{LINE_CAPACITY, Output, Read, Shell, Streams, System, USAGE};
use crate::path::PATH_CAPACITY;
use crate::status;
use crate::syntax::{MAX_WORDS, Words, is_name};
use crate::test::{self, Access, FileStatus, FileTests, Problem};

/// The most bytes the values `local` keeps aside take.
pub const LOCALS_CAPACITY: usize = 4096;

/// The most variables `local` keeps aside at once.
pub const MAX_LOCALS: usize = 64;

const READ_CAPACITY: usize = 4096;

const IFS_CAPACITY: usize = 16;

const DEFAULT_IFS: &[u8] = b" \t\n";

#[derive(Debug, Clone, Copy, Default)]
struct Local {
    call: u8,
    start: usize,
    name_length: usize,
    length: usize,
    set: bool,
    exported: bool,
}

/// The variables `local` made local, with the values to give back when their call returns.
#[derive(Debug)]
pub(super) struct Locals {
    bytes: [u8; LOCALS_CAPACITY],
    length: usize,
    entries: [Local; MAX_LOCALS],
    count: usize,
}

impl Default for Locals {
    fn default() -> Self {
        Self {
            bytes: [0; LOCALS_CAPACITY],
            length: 0,
            entries: [Local::default(); MAX_LOCALS],
            count: 0,
        }
    }
}

impl Locals {
    fn name(&self, local: &Local) -> &[u8] {
        &self.bytes[local.start..local.start + local.name_length]
    }

    fn holds(&self, call: u8, name: &[u8]) -> bool {
        self.entries[..self.count]
            .iter()
            .any(|local| local.call == call && self.name(local) == name)
    }

    fn keep(&mut self, call: u8, name: &[u8], value: Option<&[u8]>, exported: bool) -> bool {
        let value = value.unwrap_or_default();
        let end = self.length + name.len() + value.len();
        if self.count == MAX_LOCALS || end > LOCALS_CAPACITY {
            return false;
        }
        let start = self.length;
        self.bytes[start..start + name.len()].copy_from_slice(name);
        self.bytes[start + name.len()..end].copy_from_slice(value);
        self.entries[self.count] = Local {
            call,
            start,
            name_length: name.len(),
            length: end - start,
            set: true,
            exported,
        };
        self.count += 1;
        self.length = end;
        true
    }
}

struct SystemFiles<'s, S> {
    system: &'s mut S,
    streams: Streams,
}

fn with_nul<'b>(path: &[u8], buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]> {
    if path.len() >= PATH_CAPACITY {
        return None;
    }
    buffer[..path.len()].copy_from_slice(path);
    buffer[path.len()] = 0;
    Some(&buffer[..=path.len()])
}

impl<S: System> FileTests for SystemFiles<'_, S> {
    fn status(&mut self, path: &[u8], follow: bool) -> Option<FileStatus> {
        let mut buffer = [0u8; PATH_CAPACITY];
        let path = with_nul(path, &mut buffer)?;
        self.system.file_status(path, follow)
    }

    fn accessible(&mut self, path: &[u8], access: Access) -> bool {
        let mut buffer = [0u8; PATH_CAPACITY];
        with_nul(path, &mut buffer).is_some_and(|path| self.system.accessible(path, access))
    }

    fn is_terminal(&mut self, descriptor: usize) -> bool {
        let descriptor = match descriptor {
            0..=2 => self.streams.get(descriptor as u8),
            other => other,
        };
        self.system.is_terminal(descriptor)
    }
}

fn count(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 9 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        text.iter()
            .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0')),
    )
}

struct Line {
    bytes: [u8; READ_CAPACITY],
    literal: [u64; READ_CAPACITY / 64],
    length: usize,
    ifs: [u8; IFS_CAPACITY],
    ifs_length: usize,
}

impl Line {
    fn push(&mut self, byte: u8, literal: bool) -> bool {
        if self.length == READ_CAPACITY {
            return false;
        }
        let (slot, bit) = (self.length / 64, 1u64 << (self.length % 64));
        match literal {
            true => self.literal[slot] |= bit,
            false => self.literal[slot] &= !bit,
        }
        self.bytes[self.length] = byte;
        self.length += 1;
        true
    }

    fn separates(&self, at: usize) -> bool {
        self.literal[at / 64] & (1 << (at % 64)) == 0
            && self.ifs[..self.ifs_length].contains(&self.bytes[at])
    }

    fn blank(&self, at: usize) -> bool {
        self.separates(at) && matches!(self.bytes[at], b' ' | b'\t' | b'\n')
    }

    fn skip_blanks(&self, mut at: usize) -> usize {
        while at < self.length && self.blank(at) {
            at += 1;
        }
        at
    }

    fn field(&self, at: usize, last: bool) -> (usize, usize, usize) {
        if last {
            let mut end = self.length;
            while end > at && self.blank(end - 1) {
                end -= 1;
            }
            return (at, end.max(at), self.length);
        }
        let mut end = at;
        while end < self.length && !self.separates(end) {
            end += 1;
        }
        let mut next = self.skip_blanks(end);
        if next < self.length && self.separates(next) && !self.blank(next) {
            next = self.skip_blanks(next + 1);
        }
        (at, end, next)
    }
}

impl<S: System> Shell<'_, S> {
    /// `break [n]` and `continue [n]`: leave, or start the next round of, the `n`th
    /// enclosing loop.
    pub(super) fn break_loop(&mut self, words: &Words, again: bool) -> i32 {
        let name: &[u8] = if again { b"continue" } else { b"break" };
        let loops = match words.command_length() {
            1 => 1,
            2 => match count(words.command(1)) {
                Some(loops) if loops > 0 => loops,
                _ => {
                    self.error(&[name, b": ", words.command(1), b": loop count out of range"]);
                    return 1;
                }
            },
            _ => {
                self.error(&[name, b": too many arguments"]);
                return 1;
            }
        };
        if self.loops == 0 {
            self.error(&[name, b": only meaningful in a loop"]);
            return 0;
        }
        let loops = loops.min(self.loops);
        self.control = Some(match again {
            true => Control::Continue(loops),
            false => Control::Break(loops),
        });
        0
    }

    /// `return [n]`: ends the function or the file `.` runs, with status `n`.
    pub(super) fn return_from(&mut self, words: &Words) -> i32 {
        if self.calls == 0 && self.depth == 0 {
            self.error(&[b"return: can only return from a function or a sourced file"]);
            return 1;
        }
        let status = match words.command_length() {
            1 => self.status,
            2 => match status::parse_exit_status(words.command(1)) {
                Some(status) => status,
                None => {
                    self.error(&[
                        b"return: ",
                        words.command(1),
                        b": numeric argument required",
                    ]);
                    USAGE
                }
            },
            _ => {
                self.error(&[b"return: too many arguments"]);
                return 1;
            }
        };
        self.control = Some(Control::Return);
        status
    }

    /// `shift [n]`: drops the first `n` positional parameters.
    pub(super) fn shift(&mut self, words: &Words) -> i32 {
        let shifted = match words.command_length() {
            1 => 1,
            2 => match count(words.command(1)) {
                Some(shifted) => shifted,
                None => {
                    self.error(&[b"shift: ", words.command(1), b": numeric argument required"]);
                    return 1;
                }
            },
            _ => {
                self.error(&[b"shift: too many arguments"]);
                return 1;
            }
        };
        if !self.arguments.shift(shifted as usize) {
            self.error(&[b"shift: shift count out of range"]);
            return 1;
        }
        0
    }

    /// `set`: lists the variables; `set [-e | +e] [--] [words]` turns `errexit` on or off and
    /// makes the words the positional parameters.
    pub(super) fn set(&mut self, words: &Words) -> i32 {
        let length = words.command_length();
        if length == 1 {
            let mut output = Output::new(&mut self.system, self.output);
            for variable in self.variables.iter() {
                if let Some(value) = variable.value {
                    output.push(variable.name);
                    output.push(b"=");
                    output.push_quoted(value);
                    output.push(b"\n");
                }
            }
            return 0;
        }
        let mut index = 1;
        while index < length {
            match words.command(index) {
                b"--" => return self.set_arguments(words, index + 1),
                [sign @ (b'-' | b'+'), options @ ..] if !options.is_empty() => {
                    for &option in options {
                        if option != b'e' {
                            self.error(&[b"set: ", &[*sign, option], b": not supported yet"]);
                            return USAGE;
                        }
                        self.errexit = *sign == b'-';
                    }
                    index += 1;
                }
                _ => return self.set_arguments(words, index),
            }
        }
        0
    }

    fn set_arguments(&mut self, words: &Words, first: usize) -> i32 {
        let values = (first..words.command_length()).map(|index| words.command(index));
        if self.arguments.set(values).is_err() {
            self.error(&[b"set: too many arguments"]);
            return 1;
        }
        0
    }

    /// `local name[=value] ...`: the variables keep their value (or take `value`) until the
    /// function returns, when their earlier values come back.
    pub(super) fn local(&mut self, words: &Words) -> i32 {
        if self.calls == 0 {
            self.error(&[b"local: can only be used in a function"]);
            return 1;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            let name = word.split(|&byte| byte == b'=').next().unwrap_or(word);
            if !is_name(name) {
                self.error(&[b"local: `", word, b"': not a valid identifier"]);
                status = 1;
                continue;
            }
            let call = self.calls;
            if !self.locals.holds(call, name) {
                let exported = self
                    .variables
                    .iter()
                    .any(|variable| variable.name == name && variable.exported);
                let value = self.variables.get(name);
                let set = value.is_some();
                if !self.locals.keep(call, name, value, exported) {
                    self.error(&[b"local: too many local variables"]);
                    status = 1;
                    continue;
                }
                self.locals.entries[self.locals.count - 1].set = set;
            }
            if name.len() < word.len() && self.variables.assign(word, false).is_err() {
                self.error(&[b"local: too many variables"]);
                status = 1;
            }
        }
        status
    }

    /// Gives the variables `local` kept aside in the call that ends their earlier values.
    pub(super) fn restore_locals(&mut self) {
        let call = self.calls;
        while self.locals.count > 0 && self.locals.entries[self.locals.count - 1].call == call {
            let local = self.locals.entries[self.locals.count - 1];
            let name = &self.locals.bytes[local.start..local.start + local.name_length];
            let value =
                &self.locals.bytes[local.start + local.name_length..local.start + local.length];
            match local.set {
                true => {
                    let _ = self.variables.set(name, value, local.exported);
                }
                false => self.variables.unset(name),
            }
            self.locals.count -= 1;
            self.locals.length = local.start;
        }
    }

    /// `eval words`: runs the words, joined by spaces, as commands.
    pub(super) fn eval(&mut self, words: &mut Words) -> i32 {
        let mut text = [0u8; LINE_CAPACITY];
        let mut length = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            let separator = usize::from(index > 1);
            if length + separator + word.len() > LINE_CAPACITY {
                self.error(&[b"eval: too long"]);
                return 1;
            }
            if index > 1 {
                text[length] = b' ';
            }
            length += separator;
            text[length..length + word.len()].copy_from_slice(word);
            length += word.len();
        }
        self.status = 0;
        self.run_buffer(&text[..length], true, words);
        self.status
    }

    /// `read [-r] [name ...]`: reads a line from standard input and splits it at the
    /// characters of `IFS` into the variables, the last taking the rest of the line (`REPLY`
    /// when none is named). Without `-r`, `\` keeps the next character and joins lines.
    pub(super) fn read_line(&mut self, words: &Words) -> i32 {
        let mut raw = false;
        let mut first = 1;
        while first < words.command_length() {
            match words.command(first) {
                b"-r" => raw = true,
                b"--" => {
                    first += 1;
                    break;
                }
                _ => break,
            }
            first += 1;
        }
        for index in first..words.command_length() {
            if !is_name(words.command(index)) {
                self.error(&[
                    b"read: `",
                    words.command(index),
                    b"': not a valid identifier",
                ]);
                return 2;
            }
        }
        let mut line = Line {
            bytes: [0; READ_CAPACITY],
            literal: [0; READ_CAPACITY / 64],
            length: 0,
            ifs: [0; IFS_CAPACITY],
            ifs_length: 0,
        };
        let ifs = self.variables.get(b"IFS").unwrap_or(DEFAULT_IFS);
        line.ifs_length = ifs.len().min(IFS_CAPACITY);
        line.ifs[..line.ifs_length].copy_from_slice(&ifs[..line.ifs_length]);
        let (ended, fits) = self.read_into(&mut line, raw);
        if !fits {
            self.error(&[b"read: line too long"]);
            return 1;
        }
        let names = words.command_length() - first;
        let mut at = line.skip_blanks(0);
        for index in 0..names.max(1) {
            let name = match names {
                0 => &b"REPLY"[..],
                _ => words.command(first + index),
            };
            let (start, end, next) = line.field(at.min(line.length), index + 1 == names.max(1));
            at = next;
            if self
                .variables
                .set(name, &line.bytes[start..end], false)
                .is_err()
            {
                self.error(&[b"read: too many variables"]);
                return 1;
            }
        }
        i32::from(ended)
    }

    fn read_into(&mut self, line: &mut Line, raw: bool) -> (bool, bool) {
        let mut escaped = false;
        let mut fits = true;
        loop {
            let mut byte = [0u8; 1];
            match self.system.read_data(self.input, &mut byte) {
                Read::Bytes(_) => {}
                Read::Interrupted => continue,
                Read::End => return (true, fits),
            }
            let byte = byte[0];
            if escaped {
                escaped = false;
                if byte != b'\n' {
                    fits &= line.push(byte, true);
                }
            } else if byte == b'\\' && !raw {
                escaped = true;
            } else if byte == b'\n' {
                return (false, fits);
            } else {
                fits &= line.push(byte, false);
            }
        }
    }

    /// `test expression` and `[ expression ]`: 0 if the expression is true, 1 if it is false,
    /// 2 if it cannot be read.
    pub(super) fn test(&mut self, words: &Words, bracket: bool) -> i32 {
        let name: &[u8] = if bracket { b"[" } else { b"test" };
        let mut arguments: [&[u8]; MAX_WORDS] = [b""; MAX_WORDS];
        let mut length = words.command_length() - 1;
        for (index, slot) in arguments[..length].iter_mut().enumerate() {
            *slot = words.command(index + 1);
        }
        if bracket {
            if length == 0 || arguments[length - 1] != b"]" {
                self.error(&[b"[: missing `]`"]);
                return USAGE;
            }
            length -= 1;
        }
        let mut files = SystemFiles {
            system: &mut self.system,
            streams: Streams {
                input: self.input,
                output: self.output,
                error: self.error_output,
            },
        };
        match test::evaluate(&arguments[..length], &mut files) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(problem) => {
                match problem {
                    Problem::NotAnInteger(index) => self.error(&[
                        name,
                        b": ",
                        arguments[index],
                        b": integer expression expected",
                    ]),
                    Problem::OperatorExpected(index) => {
                        self.error(&[name, b": ", arguments[index], b": operator expected"])
                    }
                    Problem::ArgumentExpected => self.error(&[name, b": argument expected"]),
                    Problem::CloseExpected => self.error(&[name, b": `)` expected"]),
                    Problem::TooMany(_) => self.error(&[name, b": too many arguments"]),
                }
                USAGE
            }
        }
    }
}
