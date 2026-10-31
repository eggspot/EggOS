//! What the server needs from the machine, as a trait: running a CLI twin with its input given
//! and its output captured, asking whether a path is a symbolic link and what is there,
//! making, reading and removing the file `wm_capture`'s twin writes, appending to the audit
//! log, reading the agents' grants, the clocks and the console. `linux.rs` makes the system calls;
//! the tests script a fake.

use eggos_runtime::errno::Errno;

/// The most words a command line takes, the program and the operand included.
pub const MAX_COMMAND_WORDS: usize = 40;

/// A command to run: the program, its words, last after `--` an operand, what it reads on its
/// standard input, whether its output is kept, and how long it may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The program's absolute path.
    pub program: &'a str,
    /// The words after the program.
    pub words: &'a [&'a str],
    /// The operand, given after `--`.
    pub operand: Option<&'a str>,
    /// Its standard input; `None` for none (end of input at once).
    pub stdin: Option<&'a [u8]>,
    /// Whether its standard output is read and dropped instead of kept.
    pub discard_output: bool,
    /// How long it may run, in milliseconds.
    pub time_limit_ms: u64,
    /// Whether it runs with `EGGOS_NO_SYMLINKS=1`, so it refuses a symbolic link on any path it
    /// opens (D170): the twins whose operand is a checked path.
    pub no_symlinks: bool,
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this status.
    Exited(i32),
    /// A signal ended it.
    Signalled(i32),
    /// It did not end in time and was stopped.
    TimedOut,
    /// It could not be started.
    NotStarted(Errno),
}

/// What running a command gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOutcome {
    /// How it ended.
    pub ended: Ended,
    /// How many bytes of its standard output were kept.
    pub stdout: usize,
    /// Whether it printed more than was kept (it was stopped then).
    pub stdout_cut: bool,
    /// How many bytes of its standard error were kept.
    pub stderr: usize,
    /// How many bytes of its standard input it took.
    pub stdin_taken: usize,
    /// Why it could not be stopped when it had to be, if it could not.
    pub stop_failed: Option<Errno>,
}

impl RunOutcome {
    /// A command that could not start because of `error`.
    pub fn not_started(error: Errno) -> Self {
        Self {
            ended: Ended::NotStarted(error),
            stdout: 0,
            stdout_cut: false,
            stderr: 0,
            stdin_taken: 0,
            stop_failed: None,
        }
    }
}

/// What is at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Nothing (or it cannot be looked at).
    Missing,
    /// A regular file of this many bytes.
    File(u64),
    /// A folder.
    Directory,
    /// Something else (a device, a link).
    Other,
}

/// The capture file, open for reading: its descriptor and its size, both from the one opening.
#[derive(Debug, PartialEq, Eq)]
pub struct CaptureFile {
    /// The open file's descriptor.
    pub descriptor: usize,
    /// Its size when it was opened, from that descriptor.
    pub size: u64,
}

/// The machine, as the server uses it.
pub trait System {
    /// Runs `invocation`, keeping its output in `stdout` and its errors in `stderr`; a command
    /// whose kept output passes `stdout`'s size, or that runs past its time limit, is stopped
    /// at once, with every process it started.
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> RunOutcome;
    /// Whether `path` is a symbolic link (an error counts as not).
    fn is_link(&mut self, path: &str) -> bool;
    /// What is at `path`, not following a last symbolic link.
    fn file_state(&mut self, path: &str) -> FileState;
    /// Removes the file at `path` if there is one (a symbolic link itself, never what it
    /// names).
    fn remove_capture(&mut self, path: &str);
    /// Makes an empty file at `path` that only its owner can read and write, failing if
    /// anything is already there (`O_EXCL`, `O_NOFOLLOW`), and closes it.
    fn create_capture(&mut self, path: &str) -> Result<(), Errno>;
    /// Opens the regular file at `path` for reading, never through a symbolic link
    /// (`O_NOFOLLOW`), and sizes it from the open descriptor: `EINVAL` for anything but a
    /// regular file.
    fn open_capture(&mut self, path: &str) -> Result<CaptureFile, Errno>;
    /// Reads the next bytes of `file` into `out`; 0 at its end.
    fn read_capture(&mut self, file: &CaptureFile, out: &mut [u8]) -> Result<usize, Errno>;
    /// Closes `file`.
    fn close_capture(&mut self, file: CaptureFile);
    /// Appends `record` to the audit log, durably.
    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno>;
    /// Reads the agents' grants file ([`crate::grants::STORE_PATH`]) into `out`, at most its
    /// size; returns how many bytes it read, `ENOENT` when there is none. The server only ever
    /// reads it: only the user's front-ends write it.
    fn read_grant_store(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        let _ = out;
        Err(Errno::ENOENT)
    }
    /// Shows the grant prompt on EggOS's own screen ([`crate::grant_prompt`]) with `question`,
    /// one JSON line; the answer comes later, to [`crate::server::Server::prompt_answered`].
    /// An error when there is no desktop or the prompt cannot start: the server then asks
    /// through the client.
    fn start_grant_prompt(&mut self, question: &[u8]) -> Result<(), Errno> {
        let _ = question;
        Err(Errno::ENODEV)
    }
    /// Closes the grant prompt if one is shown, unanswered or not.
    fn end_grant_prompt(&mut self) {}
    /// The wall clock's seconds.
    fn now(&mut self) -> u64;
    /// A clock that never goes back, in milliseconds.
    fn monotonic_ms(&mut self) -> u64;
    /// Tells the console `message` (a line).
    fn report(&mut self, message: &[u8]);
}
