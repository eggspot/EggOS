//! The shell itself (D107): it reads lines, checks each one's syntax, then runs its commands
//! one after another, each a builtin or a program found through `PATH`. Everything it asks
//! of the system goes through [`System`], so all of it runs in host tests against a fake one.

use crate::path::{self, DEFAULT_PATH, Files, Lookup, PATH_CAPACITY};
use crate::status::{self, Ended, NOT_EXECUTABLE, NOT_FOUND, USAGE};
use crate::syntax::{
    MAX_WORDS, Parameters, Quoting, SyntaxError, Terminator, Value, Words, check_line, is_name,
    line_end, next_command,
};
use crate::variables::{MAX_VARIABLES, VARIABLES_CAPACITY, Variables};

/// The most bytes of a line the shell holds before its end: longer lines are refused.
pub const LINE_CAPACITY: usize = 4096;

/// How deeply `.` may run files from files.
pub const MAX_SOURCE_DEPTH: u8 = 16;

/// The positional parameters the shell keeps: `$0` to `$9`.
pub const MAX_POSITIONAL: usize = 10;

/// `$?` after `^C` at the prompt, as if the line had been interrupted by `SIGINT`.
pub const INTERRUPTED: i32 = 128 + status::SIGINT as i32;

const OUTPUT_CAPACITY: usize = 512;

const STANDARD_OUTPUT: usize = 1;

const STANDARD_ERROR: usize = 2;

const HELP: &[u8] = b"eggsh, the EggOS shell\n\
Commands are words separated by blanks, ended by a newline or `;`. Quote with '...' (as is)\n\
or \"...\" ($ expands); \\ keeps the next character; # starts a comment.\n\
$NAME, ${NAME}, $?, $$, $#, $0-$9 expand; ~ is $HOME; NAME=value sets a variable, and\n\
NAME=value command sets it for that command only.\n\
Builtins: . (source) : cd echo exit export false help pwd set true type unset\n\
Not supported yet: | && || & < > >> ( ) `...` $(...) and wildcards.\n";

/// A system call's error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub u16);

impl Errno {
    /// `ENOENT`.
    pub const NOT_FOUND: Self = Self(2);

    /// What the error means, as `strerror` says it.
    pub fn message(self) -> &'static [u8] {
        match self.0 {
            1 => b"Operation not permitted",
            2 => b"No such file or directory",
            7 => b"Argument list too long",
            8 => b"Exec format error",
            11 => b"Resource temporarily unavailable",
            12 => b"Cannot allocate memory",
            13 => b"Permission denied",
            20 => b"Not a directory",
            21 => b"Is a directory",
            36 => b"File name too long",
            38 => b"Function not implemented",
            40 => b"Too many levels of symbolic links",
            _ => b"Unknown error",
        }
    }
}

/// What a read from a descriptor gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// This many bytes.
    Bytes(usize),
    /// The end of the input (or an error, which ends it too).
    End,
    /// A signal interrupted the read: `^C` at the prompt.
    Interrupted,
}

/// What the shell needs from the system.
pub trait System: Files {
    /// Writes all of `bytes` to `descriptor`, as far as it can.
    fn write(&mut self, descriptor: usize, bytes: &[u8]);
    /// Reads from `descriptor` into `buffer`.
    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read;
    /// Opens the file at `path` (with its NUL) to read it.
    fn open(&mut self, path: &[u8]) -> Result<usize, Errno>;
    /// Closes `descriptor`.
    fn close(&mut self, descriptor: usize);
    /// Makes `path` (with its NUL) the working directory.
    fn change_directory(&mut self, path: &[u8]) -> Result<(), Errno>;
    /// The working directory, written in `buffer`.
    fn working_directory<'b>(&mut self, buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]>;
    /// Runs `program` (with its NUL) with `arguments` and `environment` (each with its NUL)
    /// and waits for it to end: its wait status. With `job_control`, in a process group of its
    /// own that has the terminal while it runs.
    fn run(
        &mut self,
        program: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        job_control: bool,
    ) -> Result<i32, Errno>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Colon,
    Cd,
    Echo,
    Exit,
    Export,
    False,
    Help,
    Pwd,
    Set,
    Source,
    True,
    Type,
    Unset,
}

const BUILTINS: [(&[u8], Builtin); 14] = [
    (b".", Builtin::Source),
    (b":", Builtin::Colon),
    (b"cd", Builtin::Cd),
    (b"echo", Builtin::Echo),
    (b"exit", Builtin::Exit),
    (b"export", Builtin::Export),
    (b"false", Builtin::False),
    (b"help", Builtin::Help),
    (b"pwd", Builtin::Pwd),
    (b"set", Builtin::Set),
    (b"source", Builtin::Source),
    (b"true", Builtin::True),
    (b"type", Builtin::Type),
    (b"unset", Builtin::Unset),
];

fn builtin(name: &[u8]) -> Option<Builtin> {
    BUILTINS
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, builtin)| *builtin)
}

struct Output<'s, S: System> {
    system: &'s mut S,
    descriptor: usize,
    buffer: [u8; OUTPUT_CAPACITY],
    length: usize,
}

impl<'s, S: System> Output<'s, S> {
    fn new(system: &'s mut S, descriptor: usize) -> Self {
        Self {
            system,
            descriptor,
            buffer: [0; OUTPUT_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.length + bytes.len() > OUTPUT_CAPACITY {
            self.flush();
            if bytes.len() > OUTPUT_CAPACITY {
                self.system.write(self.descriptor, bytes);
                return;
            }
        }
        self.buffer[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }

    fn push_quoted(&mut self, value: &[u8]) {
        self.push(b"'");
        for part in value.split(|&byte| byte == b'\'').enumerate() {
            if part.0 > 0 {
                self.push(b"'\\''");
            }
            self.push(part.1);
        }
        self.push(b"'");
    }

    fn flush(&mut self) {
        if self.length > 0 {
            self.system
                .write(self.descriptor, &self.buffer[..self.length]);
            self.length = 0;
        }
    }
}

impl<S: System> Drop for Output<'_, S> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The shell: its variables, `$?`, its positional parameters, and the system it runs on.
#[derive(Debug)]
pub struct Shell<'a, S> {
    /// The system the shell runs on.
    pub system: S,
    variables: Variables,
    status: i32,
    pid: u32,
    positional: [&'a [u8]; MAX_POSITIONAL],
    argument_count: usize,
    interactive: bool,
    root: bool,
    exit: Option<i32>,
    depth: u8,
}

impl<'a, S: System> Shell<'a, S> {
    /// A shell on `system`, its process id `pid`, `$0` and the first arguments in
    /// `positional` (at most [`MAX_POSITIONAL`] are kept), `argument_count` of them in all;
    /// `interactive` if it reads commands from a terminal (then it writes prompts and runs
    /// each program in a group of its own), `root` if its user is root (`#` prompt).
    pub fn new(
        system: S,
        pid: u32,
        positional: &[&'a [u8]],
        argument_count: usize,
        interactive: bool,
        root: bool,
    ) -> Self {
        let mut kept: [&'a [u8]; MAX_POSITIONAL] = [b""; MAX_POSITIONAL];
        for (slot, argument) in kept.iter_mut().zip(positional) {
            *slot = argument;
        }
        Self {
            system,
            variables: Variables::default(),
            status: 0,
            pid,
            positional: kept,
            argument_count,
            interactive,
            root,
            exit: None,
            depth: 0,
        }
    }

    /// Takes a `NAME=value` from the environment the shell started with, exported.
    pub fn import(&mut self, variable: &[u8]) {
        let _ = self.variables.assign(variable, true);
    }

    /// Sets `PWD` to the working directory, exported.
    pub fn set_working_directory_variable(&mut self) {
        let mut buffer = [0; PATH_CAPACITY];
        if let Some(directory) = self.system.working_directory(&mut buffer) {
            let _ = self.variables.set(b"PWD", directory, true);
        }
    }

    /// The status the shell exits with: `exit`'s, or the last command's.
    pub fn exit_status(&self) -> i32 {
        self.exit.unwrap_or(self.status)
    }

    /// Runs the commands of `text` (`eggsh -c`).
    pub fn run_text(&mut self, text: &[u8]) {
        let mut words = Words::default();
        self.run_buffer(text, true, &mut words);
    }

    /// Runs the commands read from `descriptor` until its end or `exit`, writing a prompt
    /// before each line if `prompt`.
    pub fn run_descriptor(&mut self, descriptor: usize, prompt: bool) {
        let mut buffer = [0; LINE_CAPACITY];
        let mut words = Words::default();
        let mut length = 0;
        let mut discarding = None;
        while self.exit.is_none() {
            if prompt {
                self.prompt(length > 0);
            }
            let count = match self.system.read(descriptor, &mut buffer[length..]) {
                Read::Bytes(count) => count,
                Read::Interrupted if prompt => {
                    length = 0;
                    discarding = None;
                    self.system.write(STANDARD_ERROR, b"\n");
                    self.status = INTERRUPTED;
                    continue;
                }
                Read::Interrupted => continue,
                Read::End => {
                    if length > 0 && discarding.is_none() {
                        self.run_buffer(&buffer[..length], true, &mut words);
                    }
                    if prompt && self.exit.is_none() {
                        self.system.write(STANDARD_ERROR, b"exit\n");
                    }
                    return;
                }
            };
            let mut from = 0;
            let start = length;
            length += count;
            if let Some(quoting) = &mut discarding {
                match line_end(quoting, &buffer[start..length]) {
                    Some(end) => {
                        from = start + end;
                        discarding = None;
                    }
                    None => {
                        length = 0;
                        continue;
                    }
                }
            }
            let (used, discard) = self.run_buffer(&buffer[from..length], false, &mut words);
            let used = from + used;
            buffer.copy_within(used..length, 0);
            length -= used;
            if discard.is_some() {
                discarding = discard;
                length = 0;
            } else if length == LINE_CAPACITY {
                self.error(&[b"line too long"]);
                self.status = USAGE;
                if !prompt {
                    if !self.interactive {
                        self.exit = Some(USAGE);
                    }
                    return;
                }
                let mut quoting = Quoting::WordStart;
                discarding = match line_end(&mut quoting, &buffer[..length]) {
                    Some(_) => Some(Quoting::WordStart),
                    None => Some(quoting),
                };
                length = 0;
            }
        }
    }

    fn run_buffer(
        &mut self,
        text: &[u8],
        final_input: bool,
        words: &mut Words,
    ) -> (usize, Option<Quoting>) {
        let mut at = 0;
        while at < text.len() && self.exit.is_none() {
            match check_line(&text[at..], final_input) {
                Ok(length) => {
                    self.run_line(&text[at..at + length], words);
                    at += length;
                }
                Err(SyntaxError::Incomplete) => break,
                Err(error) => {
                    self.syntax_error(error);
                    let mut quoting = Quoting::WordStart;
                    match line_end(&mut quoting, &text[at..]) {
                        Some(end) => at += end,
                        None if final_input => return (text.len(), None),
                        None => return (text.len(), Some(quoting)),
                    }
                }
            }
        }
        (at, None)
    }

    fn run_line(&mut self, line: &[u8], words: &mut Words) {
        let mut at = 0;
        while self.exit.is_none() {
            match next_command(&line[at..], true, &*self, words) {
                Ok(parsed) => {
                    at += parsed.consumed;
                    self.execute(words);
                    if parsed.terminator != Terminator::Semicolon {
                        return;
                    }
                }
                Err(error) => {
                    self.syntax_error(error);
                    return;
                }
            }
        }
    }

    fn syntax_error(&mut self, error: SyntaxError) {
        match error {
            SyntaxError::Incomplete | SyntaxError::UnexpectedEnd(b'\'') => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `'`"])
            }
            SyntaxError::UnexpectedEnd(b'"') => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `\"`"])
            }
            SyntaxError::UnexpectedEnd(_) => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `}`"])
            }
            SyntaxError::NotSupported(operator) => {
                self.error(&[b"`", operator.as_bytes(), b"` is not supported yet"])
            }
            SyntaxError::UnexpectedToken(token) => self.error(&[
                b"syntax error near unexpected token `",
                token.as_bytes(),
                b"`",
            ]),
            SyntaxError::BadSubstitution => self.error(&[b"bad substitution"]),
            SyntaxError::TooLong => self.error(&[b"command too long"]),
        }
        self.status = USAGE;
        if !self.interactive {
            self.exit = Some(USAGE);
        }
    }

    fn error(&mut self, parts: &[&[u8]]) {
        let mut output = Output::new(&mut self.system, STANDARD_ERROR);
        output.push(b"eggsh: ");
        for part in parts {
            output.push(part);
        }
        output.push(b"\n");
    }

    fn prompt(&mut self, continuation: bool) {
        let mut output = Output::new(&mut self.system, STANDARD_ERROR);
        let (variable, default) = if continuation {
            (b"PS2", &b"> "[..])
        } else {
            (b"PS1", &b""[..])
        };
        match self.variables.get(variable) {
            Some(prompt) => output.push(prompt),
            None if continuation => output.push(default),
            None => {
                output.push(b"eggsh:");
                output.push(self.variables.get(b"PWD").unwrap_or(b"?"));
                output.push(if self.root { b"# " } else { b"$ " });
            }
        }
    }

    fn execute(&mut self, words: &Words) {
        if words.command_length() == 0 {
            for assignment in words.assignments() {
                if self.variables.assign(assignment, false).is_err() {
                    self.error(&[b"too many variables"]);
                    self.status = 1;
                    return;
                }
            }
            self.status = 0;
            return;
        }
        self.status = match builtin(words.command(0)) {
            Some(builtin) => self.builtin(builtin, words),
            None => self.external(words),
        };
    }

    fn builtin(&mut self, builtin: Builtin, words: &Words) -> i32 {
        match builtin {
            Builtin::Colon | Builtin::True => 0,
            Builtin::False => 1,
            Builtin::Cd => self.cd(words),
            Builtin::Echo => self.echo(words),
            Builtin::Exit => self.exit(words),
            Builtin::Export => self.export(words),
            Builtin::Help => {
                self.system.write(STANDARD_OUTPUT, HELP);
                0
            }
            Builtin::Pwd => self.pwd(),
            Builtin::Set => self.set(words),
            Builtin::Source => self.source(words),
            Builtin::Type => self.type_of(words),
            Builtin::Unset => self.unset(words),
        }
    }

    fn cd(&mut self, words: &Words) -> i32 {
        if words.command_length() > 2 {
            self.error(&[b"cd: too many arguments"]);
            return 1;
        }
        let mut target = [0; PATH_CAPACITY];
        let back = words.command_length() == 2 && words.command(1) == b"-";
        let directory = match words.command_length() {
            1 => self.variables.get(b"HOME").unwrap_or(b"/"),
            _ if back => match self.variables.get(b"OLDPWD") {
                Some(directory) => directory,
                None => {
                    self.error(&[b"cd: OLDPWD not set"]);
                    return 1;
                }
            },
            _ => words.command(1),
        };
        let length = directory.len();
        let fits = length < PATH_CAPACITY;
        if fits {
            target[..length].copy_from_slice(directory);
        } else {
            self.error(&[b"cd: ", Errno(36).message()]);
            return 1;
        }
        if let Err(errno) = self.system.change_directory(&target[..length + 1]) {
            self.error(&[b"cd: ", &target[..length], b": ", errno.message()]);
            return 1;
        }
        let mut current = [0; PATH_CAPACITY];
        let Some(current) = self.system.working_directory(&mut current) else {
            return 0;
        };
        let mut previous = [0; VARIABLES_CAPACITY];
        let kept_previous = match self.variables.get(b"PWD") {
            Some(old) => {
                let old_length = old.len();
                previous[..old_length].copy_from_slice(old);
                self.variables
                    .set(b"OLDPWD", &previous[..old_length], false)
                    .is_ok()
            }
            None => true,
        };
        if !kept_previous {
            self.variables.unset(b"OLDPWD");
        }
        let kept_current = self.variables.set(b"PWD", current, false).is_ok();
        if !kept_previous || !kept_current {
            self.error(&[b"cd: too many variables"]);
            return 1;
        }
        if back {
            let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
            output.push(current);
            output.push(b"\n");
        }
        0
    }

    fn pwd(&mut self) -> i32 {
        let mut buffer = [0; PATH_CAPACITY];
        let Some(directory) = self.system.working_directory(&mut buffer) else {
            self.error(&[b"pwd: the working directory is unknown"]);
            return 1;
        };
        let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
        output.push(directory);
        output.push(b"\n");
        0
    }

    fn echo(&mut self, words: &Words) -> i32 {
        let newline = !(words.command_length() > 1 && words.command(1) == b"-n");
        let first = if newline { 1 } else { 2 };
        let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
        for index in first..words.command_length() {
            if index > first {
                output.push(b" ");
            }
            output.push(words.command(index));
        }
        if newline {
            output.push(b"\n");
        }
        0
    }

    fn exit(&mut self, words: &Words) -> i32 {
        let status = match words.command_length() {
            1 => self.status,
            2 => match status::parse_exit_status(words.command(1)) {
                Some(status) => status,
                None => {
                    self.error(&[b"exit: ", words.command(1), b": numeric argument required"]);
                    USAGE
                }
            },
            _ => {
                self.error(&[b"exit: too many arguments"]);
                return 1;
            }
        };
        self.exit = Some(status);
        status
    }

    fn export(&mut self, words: &Words) -> i32 {
        if words.command_length() == 1 {
            let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
            for variable in self.variables.iter().filter(|variable| variable.exported) {
                output.push(b"export ");
                output.push(variable.name);
                if let Some(value) = variable.value {
                    output.push(b"=");
                    output.push_quoted(value);
                }
                output.push(b"\n");
            }
            return 0;
        }
        let mut status = 0;
        for index in 1..words.command_length() {
            let word = words.command(index);
            let name = word.split(|&byte| byte == b'=').next().unwrap_or(word);
            let result = if !is_name(name) {
                self.error(&[b"export: `", word, b"': not a valid identifier"]);
                status = 1;
                continue;
            } else if name.len() < word.len() {
                self.variables.assign(word, true).map(|_| ())
            } else {
                self.variables.export(name)
            };
            if result.is_err() {
                self.error(&[b"export: too many variables"]);
                status = 1;
            }
        }
        status
    }

    fn unset(&mut self, words: &Words) -> i32 {
        let mut status = 0;
        for index in 1..words.command_length() {
            let name = words.command(index);
            if is_name(name) {
                self.variables.unset(name);
            } else {
                self.error(&[b"unset: `", name, b"': not a valid identifier"]);
                status = 1;
            }
        }
        status
    }

    fn set(&mut self, words: &Words) -> i32 {
        if words.command_length() > 1 {
            self.error(&[b"set: options are not supported yet"]);
            return USAGE;
        }
        let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
        for variable in self.variables.iter() {
            if let Some(value) = variable.value {
                output.push(variable.name);
                output.push(b"=");
                output.push_quoted(value);
                output.push(b"\n");
            }
        }
        0
    }

    fn source(&mut self, words: &Words) -> i32 {
        if words.command_length() != 2 {
            self.error(&[b".: usage: . file"]);
            return USAGE;
        }
        if self.depth == MAX_SOURCE_DEPTH {
            self.error(&[b".: too deeply nested"]);
            return 1;
        }
        let name = words.command_with_nul(1);
        let descriptor = match self.system.open(name) {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                self.error(&[b".: ", words.command(1), b": ", errno.message()]);
                return 1;
            }
        };
        self.depth += 1;
        self.status = 0;
        self.run_descriptor(descriptor, false);
        self.depth -= 1;
        self.system.close(descriptor);
        self.status
    }

    fn type_of(&mut self, words: &Words) -> i32 {
        let mut status = 0;
        for index in 1..words.command_length() {
            let name = words.command(index);
            let mut program = [0; PATH_CAPACITY];
            let search = self.variables.get(b"PATH").unwrap_or(DEFAULT_PATH);
            if builtin(name).is_some() {
                let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
                output.push(name);
                output.push(b" is a shell builtin\n");
            } else if let Lookup::Program(length) =
                path::find(name, search, &mut self.system, &mut program)
            {
                let mut output = Output::new(&mut self.system, STANDARD_OUTPUT);
                output.push(name);
                output.push(b" is ");
                output.push(&program[..length - 1]);
                output.push(b"\n");
            } else {
                self.error(&[b"type: ", name, b": not found"]);
                status = 1;
            }
        }
        status
    }

    fn external(&mut self, words: &Words) -> i32 {
        let name = words.command(0);
        let mut program = [0; PATH_CAPACITY];
        let search = self.variables.get(b"PATH").unwrap_or(DEFAULT_PATH);
        let length = match path::find(name, search, &mut self.system, &mut program) {
            Lookup::Program(length) => length,
            Lookup::NotFound => {
                self.error(&[name, b": command not found"]);
                return NOT_FOUND;
            }
            Lookup::Missing => {
                self.error(&[name, b": ", Errno::NOT_FOUND.message()]);
                return NOT_FOUND;
            }
            Lookup::TooLong => {
                self.error(&[name, b": ", Errno(36).message()]);
                return NOT_FOUND;
            }
            Lookup::Directory => {
                self.error(&[name, b": ", Errno(21).message()]);
                return NOT_EXECUTABLE;
            }
            Lookup::NotExecutable => {
                self.error(&[name, b": ", Errno(13).message()]);
                return NOT_EXECUTABLE;
            }
        };
        let mut arguments: [&[u8]; MAX_WORDS] = [b""; MAX_WORDS];
        let count = words.command_length();
        for (index, slot) in arguments[..count].iter_mut().enumerate() {
            *slot = words.command_with_nul(index);
        }
        let mut environment: [&[u8]; MAX_VARIABLES + MAX_WORDS] = [b""; MAX_VARIABLES + MAX_WORDS];
        let mut variables = 0;
        for index in 0..words.assignment_count() {
            environment[variables] = words.assignment_with_nul(index);
            variables += 1;
        }
        for (variable, entry) in self.variables.environment() {
            let overridden = words.assignments().any(|assignment| {
                assignment.starts_with(variable) && assignment.get(variable.len()) == Some(&b'=')
            });
            if !overridden {
                environment[variables] = entry;
                variables += 1;
            }
        }
        let result = self.system.run(
            &program[..length],
            &arguments[..count],
            &environment[..variables],
            self.interactive,
        );
        match result {
            Ok(wait_status) => {
                let ended = Ended::from_wait_status(wait_status);
                let report = ended.report();
                if !report.is_empty() {
                    self.system.write(STANDARD_ERROR, report);
                }
                ended.shell_status()
            }
            Err(errno) => {
                self.error(&[name, b": ", errno.message()]);
                if errno == Errno::NOT_FOUND {
                    NOT_FOUND
                } else {
                    NOT_EXECUTABLE
                }
            }
        }
    }
}

impl<S> Parameters for Shell<'_, S> {
    fn value(&self, name: &[u8]) -> Value<'_> {
        match name {
            b"?" => Value::Number(self.status as u32 as u64),
            b"$" => Value::Number(u64::from(self.pid)),
            b"#" => Value::Number(self.argument_count.saturating_sub(1) as u64),
            &[digit @ b'0'..=b'9'] => {
                let index = usize::from(digit - b'0');
                match index < self.argument_count.min(MAX_POSITIONAL) {
                    true => Value::Text(self.positional[index]),
                    false => Value::Unset,
                }
            }
            _ => self.variables.get(name).map_or(Value::Unset, Value::Text),
        }
    }
}

#[cfg(test)]
mod tests;
