//! The shell itself (D107): it reads complete commands, checks each one's syntax, then runs it
//! (`script.rs` holds the grammar: lists, pipelines, `if`, `while`, `until`, `for`, `case`,
//! `{ }`, `( )` and functions, D182). A simple command is a builtin, a function or a program
//! found through `PATH`. A pipeline's commands run at the same time, each one's output piped
//! into the next one's input, and the shell waits for all of them; its status is the last one's
//! (D125). `&&` and `||` run the next pipeline only if the last one succeeded or failed, and `&`
//! starts a pipeline or a list in the background as a job, which `jobs`, `fg`, `bg`, `wait` and
//! `kill` act on; interactive, a program that a stop signal stops (`^Z`) becomes a job too
//! (D181). Subshells, command substitutions and compound commands in a pipeline run in a
//! forked copy of the shell (D182). Everything it asks of the system goes through [`System`],
//! so all of it runs in host tests against a fake one.

use crate::arguments::Arguments;
use crate::functions::Functions;
use crate::jobs::{Jobs, NewJob, State};
use crate::path::{self, DEFAULT_PATH, Files, Lookup, PATH_CAPACITY};
use crate::status::{self, Change, Ended, NOT_EXECUTABLE, NOT_FOUND, USAGE};
use crate::syntax::{
    MAX_REDIRECTIONS, MAX_WORDS, Parameters, Quoting, Redirect, Redirection, SyntaxError, Value,
    Words, is_name, line_end,
};
use crate::test::{Access, FileStatus};
use crate::variables::{MAX_VARIABLES, VARIABLES_CAPACITY, Variables};
pub use eggos_runtime::errno::Errno;
use language::Locals;
use script::{Control, Documents, Source};

mod job_control;
mod language;
mod script;

/// The most bytes of a complete command the shell holds before its end (a line, or a whole
/// `if`, loop or function definition with its here-documents): longer ones are refused.
pub const LINE_CAPACITY: usize = 16384;

/// How deeply `.` may run files from files.
pub const MAX_SOURCE_DEPTH: u8 = 16;

/// How deeply functions may call functions (D182).
pub const MAX_CALL_DEPTH: u8 = 64;

/// `$?` after `^C` at the prompt, as if the line had been interrupted by `SIGINT`.
pub const INTERRUPTED: i32 = 128 + status::SIGINT as i32;

/// The most commands a pipeline has.
pub const MAX_PIPELINE: usize = 16;

/// The history file's name in `$HOME`, `HISTFILE`'s value unless it is set (D181).
pub const HISTORY_FILE: &[u8] = b"/.eggsh_history";

const _: () = assert!(MAX_PIPELINE == crate::jobs::MAX_PROGRAMS);

const OUTPUT_CAPACITY: usize = 512;
const MESSAGE_OF_THE_DAY: &[u8] = b"/etc/motd\0";
const MESSAGE_PIECE: usize = 256;
const NULL_DEVICE: &[u8] = b"/dev/null\0";

const STANDARD_INPUT: usize = 0;

const STANDARD_OUTPUT: usize = 1;

const STANDARD_ERROR: usize = 2;

const HELP: &[u8] = b"eggsh, the EggOS shell\n\
Commands are words separated by blanks, ended by a newline or `;`. Quote with '...' (as is)\n\
or \"...\" ($ expands); \\ keeps the next character; # starts a comment.\n\
$NAME, ${NAME}, $?, $$, $!, $#, $0-$9, ${10}, $@, $* expand; ${NAME:-word}, :=, :?, :+,\n\
${#NAME}, ${NAME#pattern}, ##, %, %% change them; $(command) and `command` are its output;\n\
~ is $HOME; * ? [...] match file names. NAME=value sets a variable, NAME=value command\n\
sets it for that command only.\n\
a | b pipes a's output into b; a && b runs b if a succeeded, a || b if it failed; a & runs a\n\
in the background; ( list ) runs a list in a subshell, { list; } in this shell.\n\
< file reads a file, > file writes one, >> file adds to one; 2> file sends errors to a file,\n\
2>&1 where the output goes; <<word reads the lines up to word.\n\
if list; then list; elif list; then list; else list; fi\n\
while list; do list; done   until list; do list; done   for name in words; do list; done\n\
case word in pattern | pattern) list ;; esac   name() { list; }\n\
Builtins: . (source) : [ bg break cd continue echo eval exit export false fg help jobs kill\n\
local pwd read return set shift test true type unset wait\n\
help <command> describes an EggOS command, help --json all of them (/bin/help).\n\
At the prompt the arrows, Home, End, ^A ^E ^B ^F ^K ^U ^W edit the line, up and down bring\n\
back the lines typed before ($HISTFILE keeps them); ^Z stops the program running, fg and bg\n\
continue it.\n";

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

/// How [`System::wait`] waits for a child (D181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Until the child changes: it ends, or, under job control, stops.
    Block,
    /// Not at all: `None` if no child changed; under job control, stops and continuations
    /// are reported too.
    Poll,
}

/// Which copy of the shell [`System::fork`] returned in (D182).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forked {
    /// The new copy, a subshell.
    Child,
    /// The shell itself; the subshell has this process id.
    Parent(i32),
}

/// What the shell needs from the system.
pub trait System: Files {
    /// Writes all of `bytes` to `descriptor`, as far as it can.
    fn write(&mut self, descriptor: usize, bytes: &[u8]);
    /// Reads from `descriptor` into `buffer`; at the prompt, a line edited at the terminal.
    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read;
    /// Reads from `descriptor` into `buffer` as it comes, never through the line editor: for
    /// `read` and the output of `$(...)` (D182).
    fn read_data(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read;
    /// Opens the file at `path` (with its NUL) to read it.
    fn open(&mut self, path: &[u8]) -> Result<usize, Errno>;
    /// Closes `descriptor`.
    fn close(&mut self, descriptor: usize);
    /// Makes `path` (with its NUL) the working directory.
    fn change_directory(&mut self, path: &[u8]) -> Result<(), Errno>;
    /// The working directory, written in `buffer`.
    fn working_directory<'b>(&mut self, buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]>;
    /// Opens the file at `path` (with its NUL) for `redirection`, closed by `execve`.
    fn open_redirection(&mut self, path: &[u8], redirection: Redirection) -> Result<usize, Errno>;
    /// A descriptor to read `contents` from, from its start, closed by `execve`: a
    /// here-document's body (D182).
    fn document(&mut self, contents: &[u8]) -> Result<usize, Errno>;
    /// A new pipe, both ends closed by `execve`: the end to read, then the end to write.
    fn pipe(&mut self) -> Result<(usize, usize), Errno>;
    /// Starts `program` (with its NUL) with `arguments` and `environment` (each with its NUL),
    /// `streams` as its standard input, output and error, in `group`, and returns its process
    /// id once it runs. An error if it could not be run (it was waited for then).
    fn start(
        &mut self,
        program: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        streams: Streams,
        group: Group,
    ) -> Result<i32, Errno>;
    /// Makes a copy of the shell in `group`, which runs on from here as [`Forked::Child`]
    /// without the terminal or job control (D182).
    fn fork(&mut self, group: Group) -> Result<Forked, Errno>;
    /// Ends a copy [`System::fork`] made with `status`; it does not return on a real system.
    fn end_subshell(&mut self, status: i32);
    /// Waits for child `child` (any child for -1) to change as `how` says: the child and its
    /// wait status, `None` when polling found no change.
    fn wait(&mut self, child: i32, how: Wait) -> Result<Option<(i32, i32)>, Errno>;
    /// Gives the terminal back to the shell's own group once a pipeline that had it ended.
    fn take_back_terminal(&mut self);
    /// Gives the terminal to process group `group`, a job `fg` continues, with the terminal's
    /// settings as they were before the shell's line editing (D181).
    fn give_terminal(&mut self, group: i32);
    /// Sends signal `signal` to process `target`, or to process group `-target` (D181).
    fn kill(&mut self, target: i32, signal: u8) -> Result<(), Errno>;
    /// Keeps the lines typed at the prompt in the file at `file` (no NUL) from now on, `None`
    /// or empty for none; the first file named gives back the lines it holds (D181).
    fn keep_history_in(&mut self, file: Option<&[u8]>);
    /// Calls `each` with the name of every entry of the directory at `path` (with its NUL)
    /// (D182).
    fn read_directory(&mut self, path: &[u8], each: &mut dyn FnMut(&[u8])) -> Result<(), Errno>;
    /// What `test` needs to know of the file at `path` (with its NUL), of a symbolic link
    /// itself unless `follow` (D182).
    fn file_status(&mut self, path: &[u8], follow: bool) -> Option<FileStatus>;
    /// Whether the shell may `access` the file at `path` (with its NUL) (D182).
    fn accessible(&mut self, path: &[u8], access: Access) -> bool;
    /// Whether `descriptor` is a terminal (D182).
    fn is_terminal(&mut self, descriptor: usize) -> bool;
}

/// A started program's standard input, output and error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Streams {
    /// The descriptor its standard input reads.
    pub input: usize,
    /// The descriptor its standard output writes.
    pub output: usize,
    /// The descriptor its standard error writes (D181).
    pub error: usize,
}

impl Streams {
    fn get(&self, descriptor: u8) -> usize {
        match descriptor {
            0 => self.input,
            1 => self.output,
            _ => self.error,
        }
    }

    fn set(&mut self, descriptor: u8, file: usize) {
        match descriptor {
            0 => self.input = file,
            1 => self.output = file,
            _ => self.error = file,
        }
    }
}

/// The process group a started program joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// The shell's: no job control.
    Shell,
    /// A new one, led by the program, that takes the terminal: a pipeline's first program.
    New,
    /// A new one, led by the program, that leaves the terminal to the shell: a background
    /// pipeline's first program (D181).
    Background,
    /// The group led by this process: the pipeline's later programs.
    Join(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ran {
    Started(i32),
    Finished(i32),
}

impl Ran {
    fn as_job_end(self) -> Result<i32, Ended> {
        match self {
            Ran::Started(child) => Ok(child),
            Ran::Finished(status) => Err(Ended::Exited(status as u8)),
        }
    }
}

#[derive(Debug)]
struct Pipeline {
    input: Option<usize>,
    children: [i32; MAX_PIPELINE],
    count: usize,
    group: Option<i32>,
    last: Option<Ran>,
    background: bool,
    text_start: usize,
}

impl Pipeline {
    fn new(text_start: usize) -> Self {
        Self {
            input: None,
            children: [0; MAX_PIPELINE],
            count: 0,
            group: None,
            last: None,
            background: false,
            text_start,
        }
    }

    fn record(&mut self, ran: Ran) {
        if let Ran::Started(child) = ran {
            self.children[self.count] = child;
            self.count += 1;
        }
        self.last = Some(ran);
    }
}

#[derive(Debug, Default)]
struct Opened {
    descriptors: [usize; MAX_REDIRECTIONS + 3],
    count: usize,
}

impl Opened {
    fn push(&mut self, descriptor: usize) {
        if self.count < self.descriptors.len() {
            self.descriptors[self.count] = descriptor;
            self.count += 1;
        }
    }

    fn close_all(&mut self, system: &mut impl System) {
        for &descriptor in &self.descriptors[..self.count] {
            system.close(descriptor);
        }
        self.count = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Background,
    Bracket,
    Break,
    Colon,
    Cd,
    Continue,
    Echo,
    Eval,
    Exit,
    Export,
    False,
    Foreground,
    Help,
    Jobs,
    Kill,
    Local,
    Pwd,
    ReadLine,
    Return,
    Set,
    Shift,
    Source,
    Test,
    True,
    Type,
    Unset,
    Wait,
}

const BUILTINS: [(&[u8], Builtin); 28] = [
    (b".", Builtin::Source),
    (b":", Builtin::Colon),
    (b"[", Builtin::Bracket),
    (b"bg", Builtin::Background),
    (b"break", Builtin::Break),
    (b"cd", Builtin::Cd),
    (b"continue", Builtin::Continue),
    (b"echo", Builtin::Echo),
    (b"eval", Builtin::Eval),
    (b"exit", Builtin::Exit),
    (b"export", Builtin::Export),
    (b"false", Builtin::False),
    (b"fg", Builtin::Foreground),
    (b"help", Builtin::Help),
    (b"jobs", Builtin::Jobs),
    (b"kill", Builtin::Kill),
    (b"local", Builtin::Local),
    (b"pwd", Builtin::Pwd),
    (b"read", Builtin::ReadLine),
    (b"return", Builtin::Return),
    (b"set", Builtin::Set),
    (b"shift", Builtin::Shift),
    (b"source", Builtin::Source),
    (b"test", Builtin::Test),
    (b"true", Builtin::True),
    (b"type", Builtin::Type),
    (b"unset", Builtin::Unset),
    (b"wait", Builtin::Wait),
];

fn builtin(name: &[u8]) -> Option<Builtin> {
    BUILTINS
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, builtin)| *builtin)
}

impl Builtin {
    fn is_special(self) -> bool {
        matches!(
            self,
            Self::Source
                | Self::Colon
                | Self::Break
                | Self::Continue
                | Self::Eval
                | Self::Exit
                | Self::Export
                | Self::Return
                | Self::Set
                | Self::Shift
                | Self::Unset
        )
    }
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

/// The shell: its variables, functions, `$?`, its positional parameters, and the system it
/// runs on.
#[derive(Debug)]
pub struct Shell<'a, S> {
    /// The system the shell runs on.
    pub system: S,
    variables: Variables,
    functions: Functions,
    status: i32,
    pid: u32,
    name: &'a [u8],
    arguments: Arguments,
    interactive: bool,
    root: bool,
    exit: Option<i32>,
    control: Option<Control>,
    depth: u8,
    calls: u8,
    loops: u32,
    conditions: u32,
    errexit: bool,
    last_substitution: Option<i32>,
    input: usize,
    output: usize,
    error_output: usize,
    jobs: Jobs,
    last_background: Option<i32>,
    lines: u64,
    warned_at: Option<u64>,
    documents: Documents,
    locals: Locals,
}

impl<'a, S: System> Shell<'a, S> {
    /// A shell on `system`, its process id `pid`, `$0` and the arguments in `positional`;
    /// `interactive` if it reads commands from a terminal (then it writes prompts and runs
    /// each program in a group of its own), `root` if its user is root (`#` prompt). Arguments
    /// past what the shell keeps (8 KiB, 256 of them) are left out.
    pub fn new(
        system: S,
        pid: u32,
        positional: &[&'a [u8]],
        interactive: bool,
        root: bool,
    ) -> Self {
        let mut arguments = Arguments::default();
        let given = positional.get(1..).unwrap_or_default();
        let mut kept = given.len();
        while arguments.set(given[..kept].iter().copied()).is_err() {
            kept -= 1;
        }
        Self {
            system,
            variables: Variables::default(),
            functions: Functions::default(),
            status: 0,
            pid,
            name: positional.first().copied().unwrap_or(b"eggsh"),
            arguments,
            interactive,
            root,
            exit: None,
            control: None,
            depth: 0,
            calls: 0,
            loops: 0,
            conditions: 0,
            errexit: false,
            last_substitution: None,
            input: STANDARD_INPUT,
            output: STANDARD_OUTPUT,
            error_output: STANDARD_ERROR,
            jobs: Jobs::default(),
            last_background: None,
            lines: 0,
            warned_at: None,
            documents: Documents::default(),
            locals: Locals::default(),
        }
    }

    /// Takes a `NAME=value` from the environment the shell started with, exported.
    pub fn import(&mut self, variable: &[u8]) {
        let _ = self.variables.assign(variable, true);
    }

    /// What an interactive shell does before its first prompt: goes to `$HOME`, sets
    /// `HISTFILE` to `$HOME/.eggsh_history` unless it is set (D181), and writes `/etc/motd`,
    /// the welcome (docs/ux/desktop-v0.3-gui.md 14.4). Nothing otherwise.
    pub fn greet(&mut self) {
        if !self.interactive {
            return;
        }
        let mut home = [0u8; PATH_CAPACITY];
        let length = match self.variables.get(b"HOME") {
            Some(value) if value.len() + HISTORY_FILE.len() < PATH_CAPACITY => {
                home[..value.len()].copy_from_slice(value);
                Some(value.len())
            }
            _ => None,
        };
        if let Some(length) = length
            && self.system.change_directory(&home[..=length]).is_ok()
        {
            self.set_working_directory_variable();
        }
        if let Some(length) = length
            && self.variables.get(b"HISTFILE").is_none()
        {
            let end = length + HISTORY_FILE.len();
            home[length..end].copy_from_slice(HISTORY_FILE);
            let _ = self.variables.set(b"HISTFILE", &home[..end], false);
        }
        let Ok(descriptor) = self.system.open(MESSAGE_OF_THE_DAY) else {
            return;
        };
        let mut buffer = [0u8; MESSAGE_PIECE];
        while let Read::Bytes(count) = self.system.read(descriptor, &mut buffer) {
            self.system.write(self.output, &buffer[..count]);
        }
        self.system.close(descriptor);
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

    fn unwinding(&self) -> bool {
        self.exit.is_some() || self.control.is_some()
    }

    fn streams(&self) -> Streams {
        Streams {
            input: self.input,
            output: self.output,
            error: self.error_output,
        }
    }

    fn set_streams(&mut self, streams: Streams) -> Streams {
        let old = self.streams();
        self.input = streams.input;
        self.output = streams.output;
        self.error_output = streams.error;
        old
    }

    /// Runs the commands read from `descriptor` until its end, `exit` or `return`, writing a
    /// prompt before each line if `prompt`.
    pub fn run_descriptor(&mut self, descriptor: usize, prompt: bool) {
        let mut buffer = [0; LINE_CAPACITY];
        let mut words = Words::default();
        let mut length = 0;
        let mut discarding = None;
        while !self.unwinding() {
            if prompt {
                if length == 0 {
                    self.report_jobs();
                    self.system.keep_history_in(self.variables.get(b"HISTFILE"));
                }
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
                        if self.stopped_jobs_keep_the_shell() {
                            length = 0;
                            discarding = None;
                            continue;
                        }
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
        let kept = self.documents;
        let mut at = 0;
        let mut discard = None;
        while at < text.len() && !self.unwinding() {
            self.documents = Documents::default();
            let source = Source::new(text, final_input);
            match self.check(source, at, words) {
                Ok(end) => {
                    self.lines += 1;
                    let complete = Source::new(&text[..end], true);
                    if let Err(error) = self.run_complete(complete, at, words) {
                        self.syntax_error(error);
                    }
                    at = end;
                }
                Err(SyntaxError::Incomplete) => break,
                Err(error) => {
                    self.syntax_error(error);
                    let mut quoting = Quoting::WordStart;
                    let mut rest = at;
                    while let Some(end) = line_end(&mut quoting, &text[rest..]) {
                        rest += end;
                    }
                    if rest < text.len() && !final_input {
                        discard = Some(quoting);
                    }
                    at = text.len();
                    break;
                }
            }
        }
        self.documents = kept;
        (at, discard)
    }

    fn stage(
        &mut self,
        source: Source<'_>,
        start: usize,
        words: &mut Words,
        pipeline: &mut Pipeline,
        piped: bool,
    ) {
        let first = pipeline.last.is_none();
        let (mut streams, mut opened, mut ran) = self.stage_streams(pipeline, piped);
        if ran.is_none() {
            ran = self.redirect(source, start, words, &mut streams, &mut opened);
        }
        let forked = !first || piped || pipeline.background;
        let ran = ran.unwrap_or_else(|| self.execute(words, streams, pipeline, forked));
        opened.close_all(&mut self.system);
        pipeline.record(ran);
    }

    fn stage_streams(
        &mut self,
        pipeline: &mut Pipeline,
        piped: bool,
    ) -> (Streams, Opened, Option<Ran>) {
        let mut opened = Opened::default();
        let first = pipeline.last.is_none();
        let mut streams = self.streams();
        if let Some(input) = pipeline.input.take() {
            streams.input = input;
            opened.push(input);
        }
        let mut ran = None;
        if piped {
            if pipeline.count == MAX_PIPELINE {
                self.error(&[b"pipeline too long"]);
                ran = Some(Ran::Finished(USAGE));
            } else {
                match self.system.pipe() {
                    Ok((read, write)) => {
                        pipeline.input = Some(read);
                        streams.output = write;
                        opened.push(write);
                    }
                    Err(errno) => {
                        self.error(&[b"pipe: ", errno.message()]);
                        ran = Some(Ran::Finished(1));
                    }
                }
            }
        }
        if first
            && pipeline.background
            && !self.interactive
            && let Ok(null) = self
                .system
                .open_redirection(NULL_DEVICE, Redirection::Input)
        {
            streams.input = null;
            opened.push(null);
        }
        (streams, opened, ran)
    }

    fn redirect(
        &mut self,
        source: Source<'_>,
        start: usize,
        words: &Words,
        streams: &mut Streams,
        opened: &mut Opened,
    ) -> Option<Ran> {
        for redirect in words.redirections() {
            match redirect {
                Redirect::File {
                    descriptor,
                    redirection,
                    path,
                } => {
                    if descriptor > 2 {
                        self.bad_descriptor(descriptor);
                        return Some(Ran::Finished(1));
                    }
                    let file = match self.system.open_redirection(path, redirection) {
                        Ok(file) => file,
                        Err(errno) => {
                            self.error(&[&path[..path.len() - 1], b": ", errno.message()]);
                            return Some(Ran::Finished(1));
                        }
                    };
                    opened.push(file);
                    streams.set(descriptor, file);
                }
                Redirect::Duplicate { descriptor, source } => {
                    if descriptor > 2 || source > 2 {
                        self.bad_descriptor(descriptor.max(source));
                        return Some(Ran::Finished(1));
                    }
                    streams.set(descriptor, streams.get(source));
                }
                Redirect::Document {
                    descriptor,
                    position,
                } => {
                    if descriptor > 2 {
                        self.bad_descriptor(descriptor);
                        return Some(Ran::Finished(1));
                    }
                    match self.open_document(source, start + position) {
                        Ok(file) => {
                            opened.push(file);
                            streams.set(descriptor, file);
                        }
                        Err(status) => return Some(Ran::Finished(status)),
                    }
                }
            }
        }
        None
    }

    fn bad_descriptor(&mut self, descriptor: u8) {
        let digit = [b'0' + descriptor];
        self.error(&[
            &digit,
            b": only descriptors 0, 1 and 2 can be redirected yet",
        ]);
    }

    fn wait_for(&mut self, child: i32) -> Result<Change, Errno> {
        loop {
            match self.system.wait(child, Wait::Block)? {
                Some((_, status)) => match Change::from_wait_status(status) {
                    Change::Continued => {}
                    change => return Ok(change),
                },
                None => return Err(Errno::ECHILD),
            }
        }
    }

    fn finish(&mut self, pipeline: &mut Pipeline, text: &[u8]) {
        if let Some(input) = pipeline.input.take() {
            self.system.close(input);
        }
        let Some(last) = pipeline.last.take() else {
            return;
        };
        let mut last_change = None;
        let mut ended = 0u16;
        let mut stopped = None;
        for (index, &child) in pipeline.children[..pipeline.count].iter().enumerate() {
            match self.wait_for(child) {
                Ok(Change::Stopped(signal)) => {
                    stopped = Some(signal);
                    break;
                }
                Ok(change) => {
                    ended |= 1 << index;
                    if last == Ran::Started(child) {
                        last_change = Some(change);
                    }
                }
                Err(_) => ended |= 1 << index,
            }
        }
        if pipeline.group.is_some() {
            self.system.take_back_terminal();
        }
        if let Some(signal) = stopped {
            self.keep_stopped(pipeline, ended, last, text, signal);
        } else {
            self.status = match (last, last_change) {
                (Ran::Finished(status), _) => status,
                (Ran::Started(_), Some(Change::Ended(ended))) => {
                    let report = ended.report();
                    if !report.is_empty() {
                        self.system.write(STANDARD_ERROR, report);
                    }
                    ended.shell_status()
                }
                (Ran::Started(_), _) => 1,
            };
        }
        *pipeline = Pipeline::new(0);
        if !self.jobs.is_empty() {
            self.collect_job_changes();
        }
    }

    fn keep_stopped(
        &mut self,
        pipeline: &Pipeline,
        ended: u16,
        last: Ran,
        text: &[u8],
        signal: u8,
    ) {
        self.status = 128 + i32::from(signal);
        let added = self.jobs.add(NewJob {
            group: pipeline.group.unwrap_or(0),
            programs: &pipeline.children[..pipeline.count],
            ended,
            last: last.as_job_end(),
            text,
            state: State::Stopped(signal),
        });
        match added {
            Ok(number) => {
                self.system.write(STANDARD_ERROR, b"\n");
                self.jobs.set_state(number, State::Stopped(signal));
                self.write_job(number, STANDARD_ERROR, false);
            }
            Err(_) => self.error(&[b"too many jobs: the stopped program is not kept"]),
        }
    }

    fn run_in_background(&mut self, pipeline: &mut Pipeline, text: &[u8]) {
        if let Some(input) = pipeline.input.take() {
            self.system.close(input);
        }
        let Some(last) = pipeline.last.take() else {
            return;
        };
        if pipeline.count == 0 {
            self.status = match last {
                Ran::Finished(status) => status,
                Ran::Started(_) => 0,
            };
            *pipeline = Pipeline::new(0);
            return;
        }
        let programs = pipeline.children;
        let count = pipeline.count;
        let group = pipeline.group.unwrap_or(0);
        self.add_background_job(group, &programs[..count], last.as_job_end(), text);
        *pipeline = Pipeline::new(0);
    }

    fn add_background_job(
        &mut self,
        group: i32,
        programs: &[i32],
        last: Result<i32, Ended>,
        text: &[u8],
    ) {
        self.last_background = programs.last().copied();
        let added = self.jobs.add(NewJob {
            group,
            programs,
            ended: 0,
            last,
            text,
            state: State::Running,
        });
        match added {
            Ok(number) if self.interactive => {
                let mut digits = [0; 20];
                let mut output = Output::new(&mut self.system, STANDARD_ERROR);
                output.push(b"[");
                output.push(crate::syntax::decimal(u64::from(number), &mut digits));
                output.push(b"] ");
                let pid = self.last_background.unwrap_or(0) as u64;
                output.push(crate::syntax::decimal(pid, &mut digits));
                output.push(b"\n");
            }
            Ok(_) => {}
            Err(_) => self.error(&[b"too many jobs: the background job is not followed"]),
        }
        self.status = 0;
    }

    fn write_job(&mut self, number: u16, descriptor: usize, program: bool) {
        let Some(job) = self.jobs.get(number).copied() else {
            return;
        };
        let mut output = Output::new(&mut self.system, descriptor);
        self.jobs
            .write_line(&job, program, &mut |bytes| output.push(bytes));
    }

    fn collect_job_changes(&mut self) {
        while let Ok(Some((child, status))) = self.system.wait(-1, Wait::Poll) {
            self.jobs.record(child, Change::from_wait_status(status));
        }
    }

    fn report_jobs(&mut self) {
        if self.jobs.is_empty() {
            return;
        }
        self.collect_job_changes();
        while let Some(job) = self.jobs.take_changed() {
            self.write_job(job.number(), STANDARD_ERROR, false);
        }
        self.jobs.remove_reported();
    }

    fn stopped_jobs_keep_the_shell(&mut self) -> bool {
        if !self.interactive || self.jobs.is_empty() {
            return false;
        }
        self.collect_job_changes();
        if !self.jobs.any_stopped() || self.warned_at.is_some_and(|at| at + 1 >= self.lines) {
            return false;
        }
        self.warned_at = Some(self.lines);
        self.system
            .write(STANDARD_ERROR, b"There are stopped jobs.\n");
        true
    }

    fn syntax_error(&mut self, error: SyntaxError) {
        let status = match error {
            SyntaxError::Failed => 1,
            _ => USAGE,
        };
        match error {
            SyntaxError::Incomplete | SyntaxError::UnexpectedEnd(b'\'') => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `'`"])
            }
            SyntaxError::UnexpectedEnd(b'"') => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `\"`"])
            }
            SyntaxError::UnexpectedEnd(b'`') => self
                .error(&[b"syntax error: unexpected end of file, looking for the closing `` ` ``"]),
            SyntaxError::UnexpectedEnd(b'(') => {
                self.error(&[b"syntax error: unexpected end of file, looking for the closing `)`"])
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
            SyntaxError::Missing(word) => self.error(&[
                b"syntax error: unexpected end of file, looking for `",
                word.as_bytes(),
                b"`",
            ]),
            SyntaxError::Expected(word) => {
                self.error(&[b"syntax error: `", word.as_bytes(), b"` expected"])
            }
            SyntaxError::UnexpectedWord => {
                self.error(&[b"syntax error: a word after a compound command"])
            }
            SyntaxError::BadName => self.error(&[b"syntax error: `for` needs a variable's name"]),
            SyntaxError::BadSubstitution => self.error(&[b"bad substitution"]),
            SyntaxError::TooLong => self.error(&[b"command too long"]),
            SyntaxError::Failed => {}
        }
        self.status = status;
        if !self.interactive && self.exit.is_none() {
            self.exit = Some(status);
        }
    }

    fn error(&mut self, parts: &[&[u8]]) {
        let mut output = Output::new(&mut self.system, self.error_output);
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

    fn execute(
        &mut self,
        words: &mut Words,
        streams: Streams,
        pipeline: &mut Pipeline,
        forked: bool,
    ) -> Ran {
        if words.command_length() == 0 {
            for assignment in words.assignments() {
                if self.variables.assign(assignment, false).is_err() {
                    self.error(&[b"too many variables"]);
                    return Ran::Finished(1);
                }
            }
            return Ran::Finished(self.last_substitution.unwrap_or(0));
        }
        let name = words.command(0);
        let chosen =
            builtin(name).filter(|&found| found != Builtin::Help || words.command_length() == 1);
        if !chosen.is_some_and(Builtin::is_special) && self.functions.contains(name) {
            return match forked {
                true => self.fork_function(words, streams, pipeline),
                false => Ran::Finished(self.call_function(words, streams)),
            };
        }
        match chosen {
            Some(builtin) => {
                let saved = self.set_streams(streams);
                let status = self.builtin(builtin, words);
                self.set_streams(saved);
                Ran::Finished(status)
            }
            None => self.external(words, streams, pipeline),
        }
    }

    fn builtin(&mut self, builtin: Builtin, words: &mut Words) -> i32 {
        match builtin {
            Builtin::Colon | Builtin::True => 0,
            Builtin::False => 1,
            Builtin::Background => self.continue_in_background(words),
            Builtin::Bracket => self.test(words, true),
            Builtin::Break => self.break_loop(words, false),
            Builtin::Cd => self.cd(words),
            Builtin::Continue => self.break_loop(words, true),
            Builtin::Echo => self.echo(words),
            Builtin::Eval => self.eval(words),
            Builtin::Exit => self.exit(words),
            Builtin::Export => self.export(words),
            Builtin::Foreground => self.continue_in_foreground(words),
            Builtin::Help => {
                self.system.write(self.output, HELP);
                0
            }
            Builtin::Jobs => self.list_jobs(words),
            Builtin::Kill => self.kill(words),
            Builtin::Local => self.local(words),
            Builtin::Pwd => self.pwd(),
            Builtin::ReadLine => self.read_line(words),
            Builtin::Return => self.return_from(words),
            Builtin::Set => self.set(words),
            Builtin::Shift => self.shift(words),
            Builtin::Source => self.source(words),
            Builtin::Test => self.test(words, false),
            Builtin::Type => self.type_of(words),
            Builtin::Unset => self.unset(words),
            Builtin::Wait => self.wait(words),
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
            self.error(&[b"cd: ", Errno::ENAMETOOLONG.message()]);
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
            let mut output = Output::new(&mut self.system, self.output);
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
        let mut output = Output::new(&mut self.system, self.output);
        output.push(directory);
        output.push(b"\n");
        0
    }

    fn echo(&mut self, words: &Words) -> i32 {
        let newline = !(words.command_length() > 1 && words.command(1) == b"-n");
        let first = if newline { 1 } else { 2 };
        let mut output = Output::new(&mut self.system, self.output);
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
        if self.stopped_jobs_keep_the_shell() {
            return 1;
        }
        self.exit = Some(status);
        status
    }

    fn export(&mut self, words: &Words) -> i32 {
        if words.command_length() == 1 {
            let mut output = Output::new(&mut self.system, self.output);
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
        let mut functions = false;
        let mut first = 1;
        match words.command_length() > 1 {
            true if words.command(1) == b"-f" => {
                functions = true;
                first = 2;
            }
            true if words.command(1) == b"-v" => first = 2,
            _ => {}
        }
        for index in first..words.command_length() {
            let name = words.command(index);
            if !is_name(name) {
                self.error(&[b"unset: `", name, b"': not a valid identifier"]);
                status = 1;
            } else if functions {
                self.functions.remove(name);
            } else {
                self.variables.unset(name);
            }
        }
        status
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
        let loops = core::mem::take(&mut self.loops);
        self.run_descriptor(descriptor, false);
        self.loops = loops;
        if self.control == Some(Control::Return) {
            self.control = None;
        }
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
            let known = builtin(name);
            if known.is_none_or(|found| !found.is_special()) && self.functions.contains(name) {
                let mut output = Output::new(&mut self.system, self.output);
                output.push(name);
                output.push(b" is a function\n");
            } else if known.is_some() {
                let mut output = Output::new(&mut self.system, self.output);
                output.push(name);
                output.push(b" is a shell builtin\n");
            } else if let Lookup::Program(length) =
                path::find(name, search, &mut self.system, &mut program)
            {
                let mut output = Output::new(&mut self.system, self.output);
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

    fn group_for(&self, pipeline: &Pipeline) -> Group {
        match (self.interactive, pipeline.group, pipeline.background) {
            (false, _, _) => Group::Shell,
            (true, None, false) => Group::New,
            (true, None, true) => Group::Background,
            (true, Some(leader), _) => Group::Join(leader),
        }
    }

    fn external(&mut self, words: &Words, streams: Streams, pipeline: &mut Pipeline) -> Ran {
        match self.start(words, streams, pipeline) {
            Ok(child) => {
                if self.interactive && pipeline.group.is_none() {
                    pipeline.group = Some(child);
                }
                Ran::Started(child)
            }
            Err(status) => Ran::Finished(status),
        }
    }

    fn start(&mut self, words: &Words, streams: Streams, pipeline: &Pipeline) -> Result<i32, i32> {
        let name = words.command(0);
        let mut program = [0; PATH_CAPACITY];
        let search = self.variables.get(b"PATH").unwrap_or(DEFAULT_PATH);
        let length = match path::find(name, search, &mut self.system, &mut program) {
            Lookup::Program(length) => length,
            Lookup::NotFound => {
                self.error(&[name, b": command not found"]);
                return Err(NOT_FOUND);
            }
            Lookup::Missing => {
                self.error(&[name, b": ", Errno::ENOENT.message()]);
                return Err(NOT_FOUND);
            }
            Lookup::TooLong => {
                self.error(&[name, b": ", Errno::ENAMETOOLONG.message()]);
                return Err(NOT_FOUND);
            }
            Lookup::Directory => {
                self.error(&[name, b": ", Errno::EISDIR.message()]);
                return Err(NOT_EXECUTABLE);
            }
            Lookup::NotExecutable => {
                self.error(&[name, b": ", Errno::EACCES.message()]);
                return Err(NOT_EXECUTABLE);
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
        let group = self.group_for(pipeline);
        let result = self.system.start(
            &program[..length],
            &arguments[..count],
            &environment[..variables],
            streams,
            group,
        );
        result.map_err(|errno| {
            self.error(&[name, b": ", errno.message()]);
            if errno == Errno::ENOENT {
                NOT_FOUND
            } else {
                NOT_EXECUTABLE
            }
        })
    }
}

impl<S: System> Parameters for Shell<'_, S> {
    fn value(&self, name: &[u8]) -> Value<'_> {
        match name {
            b"?" => Value::Number(self.status as u32 as u64),
            b"$" => Value::Number(u64::from(self.pid)),
            b"!" => self
                .last_background
                .map_or(Value::Unset, |pid| Value::Number(pid as u64)),
            b"#" => Value::Number(self.arguments.count() as u64),
            b"0" => Value::Text(self.name),
            [b'0'..=b'9', ..] if name.iter().all(u8::is_ascii_digit) => {
                let number = name.iter().try_fold(0usize, |value, &digit| {
                    value
                        .checked_mul(10)?
                        .checked_add(usize::from(digit - b'0'))
                });
                number
                    .and_then(|number| self.arguments.get(number))
                    .map_or(Value::Unset, Value::Text)
            }
            _ => self.variables.get(name).map_or(Value::Unset, Value::Text),
        }
    }

    fn positional_count(&self) -> usize {
        self.arguments.count()
    }

    fn positional(&self, number: usize) -> &[u8] {
        self.arguments.get(number).unwrap_or_default()
    }

    fn assign(&mut self, name: &[u8], value: &[u8]) -> Result<(), SyntaxError> {
        if self.variables.set(name, value, false).is_err() {
            self.error(&[b"too many variables"]);
            return Err(SyntaxError::Failed);
        }
        Ok(())
    }

    fn fail(&mut self, name: &[u8], message: &[u8]) {
        self.error(&[name, b": ", message]);
    }

    fn substitute(
        &mut self,
        command: &[u8],
        backquoted: bool,
        output: &mut dyn FnMut(&[u8]) -> Result<(), SyntaxError>,
    ) -> Result<(), SyntaxError> {
        self.run_substitution(command, backquoted, output)
    }
}

#[cfg(test)]
mod tests;
