//! The builtins and options that complete the shell (D572): `let`, `trap`, `getopts`,
//! `printf`, `exec`, `command`, `readonly`, `umask`, `times`, `alias`, `unalias`, `disown`,
//! `suspend` and `history`, `set`'s `-u`, `-x`, `-f`, `-n` and `-b` with `$-`, and alias
//! expansion of a command's first word.

use super::{Catch, Output, Pipeline, Ran, Shell, Streams, System, USAGE};
use crate::arithmetic;
use crate::syntax::{Words, is_name};

const MAX_ALIAS_DEPTH: u8 = 8;
const ALIAS_TEXT_CAPACITY: usize = 4096;
const FORMAT_CAPACITY: usize = 4096;
const SIGSTOP: u8 = 19;

/// The shell's option letters beside `-e`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// `-u`: an unset parameter's expansion is an error.
    pub nounset: bool,
    /// `-x`: each command is written to standard error before it runs.
    pub xtrace: bool,
    /// `-f`: no pathname expansion.
    pub noglob: bool,
    /// `-n`: commands are read, not run (ignored by an interactive shell).
    pub noexec: bool,
    /// `-b`: a background job's end is told at once.
    pub notify: bool,
    /// `-e`, mirrored from the shell's own flag for `$-`.
    pub errexit: bool,
    /// `-i`, mirrored for `$-`.
    pub interactive: bool,
    letters: [u8; 8],
    count: usize,
}

impl Options {
    /// The options of a shell that is interactive or not, every other one off.
    pub fn new(interactive: bool) -> Self {
        let mut options = Self {
            interactive,
            ..Self::default()
        };
        options.refresh();
        options
    }

    /// Sets option `letter` on or off: whether the shell knows it.
    pub fn set(&mut self, letter: u8, on: bool) -> bool {
        match letter {
            b'u' => self.nounset = on,
            b'x' => self.xtrace = on,
            b'f' => self.noglob = on,
            b'n' => self.noexec = on,
            b'b' => self.notify = on,
            b'e' => self.errexit = on,
            _ => return false,
        }
        self.refresh();
        true
    }

    fn refresh(&mut self) {
        let flags = [
            (self.errexit, b'e'),
            (self.notify, b'b'),
            (self.noglob, b'f'),
            (self.interactive, b'i'),
            (self.noexec, b'n'),
            (self.nounset, b'u'),
            (self.xtrace, b'x'),
        ];
        self.count = 0;
        for (on, letter) in flags {
            if on {
                self.letters[self.count] = letter;
                self.count += 1;
            }
        }
    }

    /// `$-`: the letters of the options on.
    pub fn letters(&self) -> &[u8] {
        &self.letters[..self.count]
    }
}

/// The signal a `trap` condition names: its number, 0 for `EXIT`.
fn signal_number(name: &[u8]) -> Option<u8> {
    let name = name.strip_prefix(b"SIG").unwrap_or(name);
    if let Some(number) = arithmetic::parse_number(name).filter(|_| name.iter().all(u8::is_ascii_digit)) {
        return u8::try_from(number).ok();
    }
    Some(match name {
        b"EXIT" => 0,
        b"HUP" => 1,
        b"INT" => 2,
        b"QUIT" => 3,
        b"ABRT" => 6,
        b"KILL" => 9,
        b"USR1" => 10,
        b"SEGV" => 11,
        b"USR2" => 12,
        b"PIPE" => 13,
        b"ALRM" => 14,
        b"TERM" => 15,
        b"CHLD" => 17,
        b"CONT" => 18,
        b"TSTP" => 20,
        b"WINCH" => 28,
        _ => return None,
    })
}

const SIGNAL_NAMES: [(&[u8], u8); 15] = [
    (b"EXIT", 0),
    (b"HUP", 1),
    (b"INT", 2),
    (b"QUIT", 3),
    (b"ABRT", 6),
    (b"USR1", 10),
    (b"SEGV", 11),
    (b"USR2", 12),
    (b"PIPE", 13),
    (b"ALRM", 14),
    (b"TERM", 15),
    (b"CHLD", 17),
    (b"CONT", 18),
    (b"TSTP", 20),
    (b"WINCH", 28),
];

fn signal_key(number: u8) -> [u8; 3] {
    [b'#', b'0' + number / 10, b'0' + number % 10]
}

impl<S: System> Shell<'_, S> {
    /// Writes `+ word word ...` to standard error (`set -x`).
    pub(super) fn trace(&mut self, words: &Words) {
        let mut output = Output::new(&mut self.system, self.error_output);
        output.push(b"+");
        for index in 0..words.command_length() {
            output.push(b" ");
            output.push(words.command(index));
        }
        output.push(b"\n");
    }

    /// Runs the alias named by `words`' first word with the other words after its text, as the
    /// command (D572); `None` when it names no alias, or aliases nest too deeply.
    pub(super) fn expand_alias(
        &mut self,
        words: &mut Words,
        _streams: Streams,
        _pipeline: &mut Pipeline,
        _forked: bool,
    ) -> Option<Ran> {
        if self.alias_depth >= MAX_ALIAS_DEPTH {
            return None;
        }
        let value = self.aliases.get(words.command(0))?;
        let mut text = [0u8; ALIAS_TEXT_CAPACITY];
        let mut length = value.len().min(ALIAS_TEXT_CAPACITY);
        text[..length].copy_from_slice(&value[..length]);
        for index in 1..words.command_length() {
            let word = words.command(index);
            let needed = 3 + word.len() + word.iter().filter(|&&byte| byte == b'\'').count() * 3;
            if length + needed > ALIAS_TEXT_CAPACITY {
                self.error(&[b"alias: the command is too long"]);
                return Some(Ran::Finished(1));
            }
            text[length] = b' ';
            text[length + 1] = b'\'';
            length += 2;
            for &byte in word {
                if byte == b'\'' {
                    text[length..length + 4].copy_from_slice(b"'\\''");
                    length += 4;
                } else {
                    text[length] = byte;
                    length += 1;
                }
            }
            text[length] = b'\'';
            length += 1;
        }
        self.alias_depth += 1;
        let mut scratch = Words::default();
        let ran = self.nested(|shell| {
            shell.run_buffer(&text[..length], true, &mut scratch);
            Ok(())
        });
        self.alias_depth -= 1;
        if let Err(error) = ran {
            self.syntax_error(error);
        }
        Some(Ran::Finished(self.status))
    }

    /// `alias [name[=value] ...]`.
    pub(super) fn alias(&mut self, words: &Words) -> i32 {
        if words.command_length() == 1 {
            let mut output = Output::new(&mut self.system, self.output);
            self.aliases.each(&mut |name, value| {
                output.push(b"alias ");
                output.push(name);
                output.push(b"=");
                output.push_quoted(value);
                output.push(b"\n");
            });
            return 0;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            match word.iter().position(|&byte| byte == b'=') {
                Some(at) => {
                    let (name, value) = (&word[..at], &word[at + 1..]);
                    if name.is_empty() || name.iter().any(|&byte| byte == b'/' || byte == b' ') {
                        self.error(&[b"alias: `", name, b"': invalid alias name"]);
                        status = 1;
                    } else if self.aliases.define(name, value).is_err() {
                        self.error(&[b"alias: too many aliases"]);
                        status = 1;
                    }
                }
                None => match self.aliases.get(word) {
                    Some(value) => {
                        let mut output = Output::new(&mut self.system, self.output);
                        output.push(b"alias ");
                        output.push(word);
                        output.push(b"=");
                        output.push_quoted(value);
                        output.push(b"\n");
                    }
                    None => {
                        self.error(&[b"alias: ", word, b": not found"]);
                        status = 1;
                    }
                },
            }
        }
        status
    }

    /// `unalias [-a] name ...`.
    pub(super) fn unalias(&mut self, words: &Words) -> i32 {
        if words.command_length() == 2 && words.command(1) == b"-a" {
            self.aliases = crate::functions::Functions::default();
            return 0;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let name = words.command(index);
            if !self.aliases.contains(name) {
                self.error(&[b"unalias: ", name, b": not found"]);
                status = 1;
            }
            self.aliases.remove(name);
        }
        status
    }

    /// `let expression ...`: works each out; 1 when the last is 0, else 0.
    pub(super) fn let_expressions(&mut self, words: &Words) -> i32 {
        if words.command_length() < 2 {
            self.error(&[b"let: an expression is needed"]);
            return USAGE;
        }
        let mut last = 0;
        for index in 1..words.command_length() {
            match crate::syntax::Parameters::arithmetic(self, words.command(index)) {
                Ok(value) => last = value,
                Err(_) => return 1,
            }
        }
        i32::from(last == 0)
    }

    /// `trap [action condition ...]`: with no arguments lists the traps; `-` restores the
    /// default, an empty action ignores the signal, any other runs it when the signal comes
    /// (`EXIT` when the shell ends).
    pub(super) fn trap(&mut self, words: &Words) -> i32 {
        let count = words.command_length();
        if count == 1 {
            let mut output = Output::new(&mut self.system, self.output);
            for (name, number) in SIGNAL_NAMES {
                if let Some(action) = self.traps.get(&signal_key(number)) {
                    output.push(b"trap -- ");
                    output.push_quoted(action);
                    output.push(b" ");
                    output.push(name);
                    output.push(b"\n");
                }
            }
            return 0;
        }
        let (action, first) = match signal_number(words.command(1)) {
            Some(_) if count == 2 => (None, 1),
            _ => match words.command(1) {
                b"-" => (None, 2),
                action => (Some(action), 2),
            },
        };
        let mut status = 0;
        for index in first..count {
            let Some(number) = signal_number(words.command(index)) else {
                self.error(&[b"trap: ", words.command(index), b": invalid signal specification"]);
                status = 1;
                continue;
            };
            let key = signal_key(number);
            match action {
                None => {
                    self.traps.remove(&key);
                    if number != 0 {
                        self.system.catch_signal(number, Catch::Default);
                    }
                }
                Some(action) => {
                    if self.traps.define(&key, action).is_err() {
                        self.error(&[b"trap: too many traps"]);
                        status = 1;
                        continue;
                    }
                    if number != 0 {
                        let how = match action.is_empty() {
                            true => Catch::Ignore,
                            false => Catch::Record,
                        };
                        self.system.catch_signal(number, how);
                    }
                }
            }
        }
        status
    }

    /// Runs the traps of the signals caught since the last look (between commands).
    pub(super) fn run_caught_traps(&mut self) {
        let caught = self.system.take_caught();
        if caught == 0 {
            return;
        }
        for number in 1..64u8 {
            if caught & (1 << (number - 1)) == 0 {
                continue;
            }
            self.run_trap(number);
        }
    }

    /// Runs the `EXIT` trap, once, as the shell ends.
    pub fn run_exit_trap(&mut self) {
        self.run_trap(0);
        self.traps.remove(&signal_key(0));
    }

    fn run_trap(&mut self, number: u8) {
        let Some(action) = self.traps.get(&signal_key(number)) else {
            return;
        };
        let mut text = [0u8; ALIAS_TEXT_CAPACITY];
        let length = action.len().min(ALIAS_TEXT_CAPACITY);
        text[..length].copy_from_slice(&action[..length]);
        let status = self.status;
        let mut scratch = Words::default();
        let ran = self.nested(|shell| {
            shell.run_buffer(&text[..length], true, &mut scratch);
            Ok(())
        });
        if let Err(error) = ran {
            self.syntax_error(error);
        }
        self.status = status;
    }

    /// `getopts optstring name [argument ...]`: the next option of the arguments (the
    /// positional parameters when none are given) into `name`, its argument into `OPTARG`,
    /// the next index into `OPTIND`; 1 at the end of the options.
    pub(super) fn getopts(&mut self, words: &Words) -> i32 {
        if words.command_length() < 3 {
            self.error(&[b"getopts: usage: getopts optstring name [arg ...]"]);
            return USAGE;
        }
        let spec = words.command(1);
        let name = words.command(2);
        if !is_name(name) {
            self.error(&[b"getopts: `", name, b"': not a valid identifier"]);
            return 1;
        }
        let (silent, spec) = match spec.first() {
            Some(b':') => (true, &spec[1..]),
            _ => (false, spec),
        };
        let index = self
            .variables
            .get(b"OPTIND")
            .and_then(arithmetic::parse_number)
            .unwrap_or(1)
            .max(1) as usize;
        let place = self
            .variables
            .get(b"OPTPLACE")
            .and_then(arithmetic::parse_number)
            .unwrap_or(1)
            .max(1) as usize;
        let explicit = words.command_length() > 3;
        let argument = |shell: &Self, number: usize| -> Option<&[u8]> {
            match explicit {
                true => (number + 2 < words.command_length()).then(|| words.command(number + 2)),
                false => shell.arguments.get(number),
            }
        };
        let mut set = |shell: &mut Self, variable: &[u8], value: &[u8]| {
            let _ = shell.variables.set(variable, value, false);
        };
        let mut digits = [0u8; 20];
        let Some(current) = argument(self, index).map(FixedBytes::from) else {
            set(self, name, b"?");
            return 1;
        };
        let current = current.bytes();
        if place == 1 && (current.len() < 2 || current[0] != b'-' || current == b"--") {
            if current == b"--" {
                let next = arithmetic::decimal(index as i64 + 1, &mut digits);
                set(self, b"OPTIND", next);
            }
            set(self, name, b"?");
            return 1;
        }
        let letter = current[place];
        let at_end = place + 1 >= current.len();
        let known = spec.iter().position(|&byte| byte == letter && byte != b':');
        let takes_argument = known.is_some_and(|at| spec.get(at + 1) == Some(&b':'));
        let mut next_index = index;
        let mut next_place = place + 1;
        if known.is_none() {
            let unknown = [letter];
            match silent {
                true => {
                    set(self, name, b"?");
                    set(self, b"OPTARG", &unknown);
                }
                false => {
                    self.error(&[b"getopts: illegal option -- ", &unknown]);
                    set(self, name, b"?");
                }
            }
        } else if takes_argument {
            let value = match at_end {
                false => Some(FixedBytes::from(&current[place + 1..])),
                true => {
                    next_index += 1;
                    argument(self, next_index).map(FixedBytes::from)
                }
            };
            next_index += 1;
            next_place = 1;
            match value {
                Some(value) => {
                    set(self, name, &[letter]);
                    set(self, b"OPTARG", value.bytes());
                }
                None => {
                    let missing = [letter];
                    match silent {
                        true => {
                            set(self, name, b":");
                            set(self, b"OPTARG", &missing);
                        }
                        false => {
                            self.error(&[b"getopts: option requires an argument -- ", &missing]);
                            set(self, name, b"?");
                        }
                    }
                }
            }
        } else {
            set(self, name, &[letter]);
        }
        if at_end && !takes_argument {
            next_index += 1;
            next_place = 1;
        }
        let text = arithmetic::decimal(next_index as i64, &mut digits);
        set(self, b"OPTIND", text);
        let mut place_digits = [0u8; 20];
        let text = arithmetic::decimal(next_place as i64, &mut place_digits);
        set(self, b"OPTPLACE", text);
        0
    }

    /// `printf format [argument ...]`: the format with `\` escapes and `%s`, `%b`, `%c`,
    /// `%d`, `%i`, `%u`, `%o`, `%x`, `%X` and `%%` (flags `-`, `0`, a width), reused while
    /// arguments are left.
    pub(super) fn printf(&mut self, words: &Words) -> i32 {
        if words.command_length() < 2 {
            self.error(&[b"printf: usage: printf format [arguments]"]);
            return USAGE;
        }
        let format = words.command(1);
        let mut next = 2;
        let mut status = 0;
        let mut buffer = [0u8; FORMAT_CAPACITY];
        loop {
            let mut length = 0;
            let mut push = |bytes: &[u8], length: &mut usize| {
                let room = FORMAT_CAPACITY - *length;
                let count = bytes.len().min(room);
                buffer[*length..*length + count].copy_from_slice(&bytes[..count]);
                *length += count;
            };
            let used_before = next;
            let mut at = 0;
            while at < format.len() {
                match format[at] {
                    b'\\' => {
                        let (byte, consumed) = escape(&format[at + 1..]);
                        push(&[byte], &mut length);
                        at += 1 + consumed;
                    }
                    b'%' if format.get(at + 1) == Some(&b'%') => {
                        push(b"%", &mut length);
                        at += 2;
                    }
                    b'%' => {
                        at += 1;
                        let left = format.get(at) == Some(&b'-');
                        at += usize::from(left);
                        let zero = format.get(at) == Some(&b'0');
                        at += usize::from(zero);
                        let mut width = 0usize;
                        while let Some(digit) = format.get(at).filter(|byte| byte.is_ascii_digit()) {
                            width = width * 10 + usize::from(digit - b'0');
                            at += 1;
                        }
                        let Some(&conversion) = format.get(at) else {
                            break;
                        };
                        at += 1;
                        let argument: &[u8] = match next < words.command_length() {
                            true => words.command(next),
                            false => b"",
                        };
                        next += 1;
                        let mut converted = [0u8; 64];
                        let mut escaped = [0u8; FORMAT_CAPACITY];
                        let text: &[u8] = match conversion {
                            b's' => argument,
                            b'b' => {
                                let mut written = 0;
                                let mut index = 0;
                                while index < argument.len() && written < FORMAT_CAPACITY {
                                    if argument[index] == b'\\' {
                                        let (byte, consumed) = escape(&argument[index + 1..]);
                                        escaped[written] = byte;
                                        index += 1 + consumed;
                                    } else {
                                        escaped[written] = argument[index];
                                        index += 1;
                                    }
                                    written += 1;
                                }
                                &escaped[..written]
                            }
                            b'c' => argument.get(..1).unwrap_or_default(),
                            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => {
                                let value = match argument.first() {
                                    Some(b'\'' | b'"') => {
                                        argument.get(1).map_or(0, |&byte| i64::from(byte))
                                    }
                                    _ => match arithmetic::parse_number(argument) {
                                        Some(value) => value,
                                        None => {
                                            self.error(&[b"printf: ", argument, b": invalid number"]);
                                            status = 1;
                                            0
                                        }
                                    },
                                };
                                number_text(value, conversion, &mut converted)
                            }
                            _ => {
                                self.error(&[b"printf: %", &[conversion], b": invalid directive"]);
                                return 1;
                            }
                        };
                        let pad = width.saturating_sub(text.len());
                        let filler: &[u8] = match zero && !left && conversion != b's' {
                            true => b"0",
                            false => b" ",
                        };
                        if !left {
                            for _ in 0..pad {
                                push(filler, &mut length);
                            }
                        }
                        push(text, &mut length);
                        if left {
                            for _ in 0..pad {
                                push(b" ", &mut length);
                            }
                        }
                    }
                    byte => {
                        push(&[byte], &mut length);
                        at += 1;
                    }
                }
            }
            let mut output = Output::new(&mut self.system, self.output);
            output.push(&buffer[..length]);
            drop(output);
            if next >= words.command_length() || next == used_before {
                break;
            }
        }
        status
    }

    /// `exec [command [argument ...]]`: replaces the shell with the command; with none, the
    /// redirections stay for the shell (they were applied).
    pub(super) fn exec(&mut self, words: &Words) -> i32 {
        if words.command_length() < 2 {
            return 0;
        }
        let name = words.command(1);
        let mut program = [0; crate::path::PATH_CAPACITY];
        let search = self.variables.get(b"PATH").unwrap_or(crate::path::DEFAULT_PATH);
        let length = match crate::path::find(name, search, &mut self.system, &mut program) {
            crate::path::Lookup::Program(length) => length,
            _ => {
                self.error(&[b"exec: ", name, b": not found"]);
                return super::NOT_FOUND;
            }
        };
        let mut arguments: [&[u8]; crate::syntax::MAX_WORDS] = [b""; crate::syntax::MAX_WORDS];
        let count = words.command_length() - 1;
        for (index, slot) in arguments[..count].iter_mut().enumerate() {
            *slot = words.command_with_nul(index + 1);
        }
        let mut environment: [&[u8]; crate::variables::MAX_VARIABLES] =
            [b""; crate::variables::MAX_VARIABLES];
        let mut variables = 0;
        for (_, entry) in self.variables.environment() {
            if variables < environment.len() {
                environment[variables] = entry;
                variables += 1;
            }
        }
        self.run_exit_trap();
        let errno = self.system.replace(
            &program[..=length],
            &arguments[..count],
            &environment[..variables],
        );
        self.error(&[b"exec: ", name, b": ", errno.message()]);
        super::NOT_EXECUTABLE
    }

    /// `command [-v|-V] name [argument ...]`: runs `name` skipping functions and aliases, or
    /// says what it is.
    pub(super) fn command(&mut self, words: &mut Words) -> i32 {
        let (describe, first) = match words.command_length() > 1 {
            true => match words.command(1) {
                b"-v" => (Some(false), 2),
                b"-V" => (Some(true), 2),
                _ => (None, 1),
            },
            false => return 0,
        };
        if let Some(verbose) = describe {
            let mut status = 0;
            for index in first..words.command_length() {
                let name = words.command(index);
                if !self.describe(name, verbose) {
                    status = 1;
                }
            }
            return status;
        }
        let shifted = words.drop_first_command_word();
        if shifted.is_err() {
            return USAGE;
        }
        let name = words.command(0);
        match super::builtin(name) {
            Some(builtin) => self.builtin(builtin, words),
            None => {
                let streams = self.streams();
                let mut pipeline = Pipeline::new(0);
                match self.external(words, streams, &mut pipeline) {
                    Ran::Finished(status) => status,
                    Ran::Started(child) => match self.wait_for(child) {
                        Ok(crate::status::Change::Ended(ended)) => ended.shell_status(),
                        Ok(crate::status::Change::Stopped(signal)) => 128 + i32::from(signal),
                        Ok(crate::status::Change::Continued) | Err(_) => 1,
                    },
                }
            }
        }
    }

    fn describe(&mut self, name: &[u8], verbose: bool) -> bool {
        let kind: Option<&[u8]> = if self.aliases.contains(name) {
            Some(b"alias")
        } else if super::builtin(name).is_some() {
            Some(b"builtin")
        } else if self.functions.contains(name) {
            Some(b"function")
        } else {
            None
        };
        let mut program = [0; crate::path::PATH_CAPACITY];
        let search = self.variables.get(b"PATH").unwrap_or(crate::path::DEFAULT_PATH);
        let found = match kind {
            Some(_) => None,
            None => match crate::path::find(name, search, &mut self.system, &mut program) {
                crate::path::Lookup::Program(length) => Some(length),
                _ => None,
            },
        };
        let mut output = Output::new(&mut self.system, self.output);
        match (kind, found) {
            (Some(kind), _) => {
                output.push(name);
                if verbose {
                    output.push(b" is a ");
                    output.push(kind);
                }
                output.push(b"\n");
                true
            }
            (None, Some(length)) => {
                if verbose {
                    output.push(name);
                    output.push(b" is ");
                }
                output.push(&program[..length]);
                output.push(b"\n");
                true
            }
            (None, None) => false,
        }
    }

    /// `readonly [name[=value] ...]`: the variables can no longer be set or unset.
    pub(super) fn readonly(&mut self, words: &Words) -> i32 {
        if words.command_length() == 1 || words.command(1) == b"-p" {
            let mut output = Output::new(&mut self.system, self.output);
            self.readonly.each(&mut |name, _| {
                output.push(b"readonly ");
                output.push(name);
                output.push(b"\n");
            });
            return 0;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            let name = word.split(|&byte| byte == b'=').next().unwrap_or(word);
            if !is_name(name) {
                self.error(&[b"readonly: `", word, b"': not a valid identifier"]);
                status = 1;
                continue;
            }
            if word.len() > name.len() && !self.readonly.contains(name) {
                if self.variables.assign(word, false).is_err() {
                    self.error(&[b"too many variables"]);
                    status = 1;
                    continue;
                }
            }
            if self.readonly.define(name, b"").is_err() {
                self.error(&[b"readonly: too many read-only variables"]);
                status = 1;
            }
        }
        status
    }

    /// Whether variable `name` is read-only; says so when it is.
    pub(super) fn refuses_change(&mut self, name: &[u8]) -> bool {
        let refused = self.readonly.contains(name);
        if refused {
            self.error(&[name, b": readonly variable"]);
        }
        refused
    }

    /// `umask [-S] [mode]`.
    pub(super) fn umask(&mut self, words: &Words) -> i32 {
        let symbolic = words.command_length() > 1 && words.command(1) == b"-S";
        let given = (1 + usize::from(symbolic) < words.command_length())
            .then(|| words.command(1 + usize::from(symbolic)));
        match given {
            None => {
                let mask = self.system.umask(None);
                let mut output = Output::new(&mut self.system, self.output);
                if symbolic {
                    let allowed = !mask & 0o777;
                    let part = |bits: u32, out: &mut [u8; 3]| -> usize {
                        let mut length = 0;
                        for (bit, letter) in [(4, b'r'), (2, b'w'), (1, b'x')] {
                            if bits & bit != 0 {
                                out[length] = letter;
                                length += 1;
                            }
                        }
                        length
                    };
                    for (who, shift) in [(&b"u="[..], 6), (&b",g="[..], 3), (&b",o="[..], 0)] {
                        let mut letters = [0u8; 3];
                        let length = part(allowed >> shift & 7, &mut letters);
                        output.push(who);
                        output.push(&letters[..length]);
                    }
                    output.push(b"\n");
                } else {
                    let digits = [
                        b'0',
                        b'0' + (mask >> 6 & 7) as u8,
                        b'0' + (mask >> 3 & 7) as u8,
                        b'0' + (mask & 7) as u8,
                    ];
                    output.push(&digits);
                    output.push(b"\n");
                }
                0
            }
            Some(mode) => {
                let parsed = mode.iter().try_fold(0u32, |value, &digit| match digit {
                    b'0'..=b'7' => Some(value * 8 + u32::from(digit - b'0')),
                    _ => None,
                });
                match parsed.filter(|&value| value <= 0o777) {
                    Some(mask) => {
                        self.system.umask(Some(mask));
                        0
                    }
                    None => {
                        self.error(&[b"umask: ", mode, b": invalid mode (an octal number)"]);
                        1
                    }
                }
            }
        }
    }

    /// `times`: the shell's user and system times, then its children's.
    pub(super) fn times(&mut self) -> i32 {
        let times = self.system.times();
        let mut output = Output::new(&mut self.system, self.output);
        for pair in [[times[0], times[1]], [times[2], times[3]]] {
            for (index, milliseconds) in pair.into_iter().enumerate() {
                let mut digits = [0u8; 20];
                let minutes = milliseconds / 60_000;
                let seconds = milliseconds % 60_000;
                output.push(arithmetic::decimal(minutes as i64, &mut digits));
                output.push(b"m");
                output.push(arithmetic::decimal((seconds / 1000) as i64, &mut digits));
                output.push(b".");
                let thousandths = seconds % 1000;
                output.push(&[
                    b'0' + (thousandths / 100) as u8,
                    b'0' + (thousandths / 10 % 10) as u8,
                    b'0' + (thousandths % 10) as u8,
                ]);
                output.push(if index == 0 { b"s " } else { b"s\n" });
            }
        }
        0
    }

    /// `disown [job ...]`: the jobs leave the table; they run on, unwatched.
    pub(super) fn disown(&mut self, words: &Words) -> i32 {
        let specs = (1..words.command_length()).map(|index| Some(words.command(index)));
        let mut status = 0;
        let mut specs_seen = false;
        for spec in specs {
            specs_seen = true;
            match self.jobs.find(spec) {
                Ok(number) => self.jobs.remove(number),
                Err(_) => {
                    self.error(&[b"disown: ", spec.unwrap_or_default(), b": no such job"]);
                    status = 1;
                }
            }
        }
        if !specs_seen {
            match self.jobs.find(None) {
                Ok(number) => self.jobs.remove(number),
                Err(_) => {
                    self.error(&[b"disown: no current job"]);
                    status = 1;
                }
            }
        }
        status
    }

    /// `suspend`: stops the shell until a `SIGCONT`; a login shell refuses without `-f`.
    pub(super) fn suspend(&mut self, words: &Words) -> i32 {
        let forced = words.command_length() > 1 && words.command(1) == b"-f";
        if self.name.first() == Some(&b'-') && !forced {
            self.error(&[b"suspend: cannot suspend a login shell"]);
            return 1;
        }
        let pid = self.pid as i32;
        match self.system.kill(pid, SIGSTOP) {
            Ok(()) => 0,
            Err(errno) => {
                self.error(&[b"suspend: ", errno.message()]);
                1
            }
        }
    }

    /// `history [count]`: the lines typed at the prompt, oldest first, numbered.
    pub(super) fn history(&mut self, words: &Words) -> i32 {
        let wanted = match words.command_length() {
            1 => usize::MAX,
            _ => match arithmetic::parse_number(words.command(1)) {
                Some(count) if count >= 0 => count as usize,
                _ => {
                    self.error(&[b"history: ", words.command(1), b": numeric argument required"]);
                    return USAGE;
                }
            },
        };
        let total = self.system.history_length();
        let shown = wanted.min(total);
        for back in (0..shown).rev() {
            let number = total - back;
            let mut digits = [0u8; 20];
            let mut line = [0u8; super::LINE_CAPACITY];
            let length = self.system.history_entry(back, &mut line);
            let mut output = Output::new(&mut self.system, self.output);
            let text = arithmetic::decimal(number as i64, &mut digits);
            for _ in text.len()..5 {
                output.push(b" ");
            }
            output.push(text);
            output.push(b"  ");
            output.push(&line[..length]);
            output.push(b"\n");
        }
        0
    }
}

/// A short copy of bytes, for a value read while the shell is borrowed.
struct FixedBytes {
    bytes: [u8; 256],
    length: usize,
}

impl FixedBytes {
    fn from(bytes: &[u8]) -> Self {
        let length = bytes.len().min(256);
        let mut copy = [0u8; 256];
        copy[..length].copy_from_slice(&bytes[..length]);
        Self {
            bytes: copy,
            length,
        }
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// The byte a `\` escape at the start of `after` stands for, and how many bytes it took.
fn escape(after: &[u8]) -> (u8, usize) {
    match after.first() {
        Some(b'n') => (b'\n', 1),
        Some(b't') => (b'\t', 1),
        Some(b'r') => (b'\r', 1),
        Some(b'a') => (0x07, 1),
        Some(b'b') => (0x08, 1),
        Some(b'f') => (0x0c, 1),
        Some(b'v') => (0x0b, 1),
        Some(b'e') => (0x1b, 1),
        Some(b'\\') => (b'\\', 1),
        Some(b'0'..=b'7') => {
            let digits = after
                .iter()
                .take(3)
                .take_while(|byte| (b'0'..=b'7').contains(byte))
                .count();
            let value = after[..digits]
                .iter()
                .fold(0u32, |value, &digit| value * 8 + u32::from(digit - b'0'));
            (value as u8, digits)
        }
        Some(&other) => (other, 1),
        None => (b'\\', 0),
    }
}

fn number_text(value: i64, conversion: u8, out: &mut [u8; 64]) -> &[u8] {
    let (radix, upper) = match conversion {
        b'o' => (8u64, false),
        b'x' => (16, false),
        b'X' => (16, true),
        _ => (10, false),
    };
    let negative = value < 0 && radix == 10 && conversion != b'u';
    let mut magnitude = match negative {
        true => value.unsigned_abs(),
        false => value as u64,
    };
    let mut at = out.len();
    loop {
        at -= 1;
        let digit = (magnitude % radix) as u8;
        out[at] = match digit {
            0..=9 => b'0' + digit,
            _ if upper => b'A' + digit - 10,
            _ => b'a' + digit - 10,
        };
        magnitude /= radix;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        at -= 1;
        out[at] = b'-';
    }
    &out[at..]
}
