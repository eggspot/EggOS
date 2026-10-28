//! What the server needs from the machine, as a trait: running a CLI twin with its output
//! captured, asking whether a path is a symbolic link, appending to the audit log, the clock
//! and the console. `linux.rs` makes the system calls; the tests script a fake.

use eggos_runtime::errno::Errno;

/// A command to run: the program, its words and, last after `--`, a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The program's absolute path.
    pub program: &'a str,
    /// The words after the program.
    pub words: &'a [&'a str],
    /// The path, given after `--`.
    pub path: Option<&'a str>,
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
}

/// The machine, as the server uses it.
pub trait System {
    /// Runs `invocation`, keeping its output in `stdout` and its errors in `stderr`; a command
    /// whose output passes `stdout`'s size is stopped at once.
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> RunOutcome;
    /// Whether `path` is a symbolic link (an error counts as not).
    fn is_link(&mut self, path: &str) -> bool;
    /// Appends `record` to the audit log, durably.
    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno>;
    /// The wall clock's seconds.
    fn now(&mut self) -> u64;
    /// Tells the console `message` (a line).
    fn report(&mut self, message: &[u8]);
}
