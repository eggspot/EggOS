//! The commands the shell boot test runs from `eggsh` (D107, D181): given [`SHELL_CHILD`] and
//! one of [`SHOW`], [`KILL_SELF`], [`WAIT_FOR_INTERRUPT`], [`STOP_AND_CONTINUE`],
//! [`READ_LINE`], [`SLEEP`] or [`WINDOW_CHANGE`], the program shows what the shell gave it,
//! ends by a signal, waits in the console's foreground group for `^C`, is stopped by `^Z` and
//! continued in the background and the foreground, reads a line (stopped by `SIGTTIN` until
//! it is in the foreground), sleeps until a signal ends it, or changes the console's window
//! size and sees `SIGWINCH`.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The argument that runs [`run`].
pub const SHELL_CHILD: &str = "shell-child";

/// Writes `EGGOS-SHELL-CHILD: NAME=value args=a|b` for the variable named by the next
/// argument and the arguments after it, then exits with [`SHOW_STATUS`].
pub const SHOW: &str = "show";

/// Sends itself `SIGTERM`.
pub const KILL_SELF: &str = "kill-self";

/// Checks it leads the console's foreground group, writes [`WAITING_LINE`] and sleeps until a
/// signal ends it; exits with 1 if it is not in the foreground or the sleep ends.
pub const WAIT_FOR_INTERRUPT: &str = "wait-for-interrupt";

/// Checks it leads the console's foreground group, writes [`STOP_WAITING_LINE`] and waits:
/// if it sees itself continued outside the foreground it writes [`BACKGROUND_LINE`], and once
/// continued and back in the foreground [`FOREGROUND_LINE`], then exits with
/// [`FOREGROUND_STATUS`] (D181).
pub const STOP_AND_CONTINUE: &str = "stop-and-continue";

/// Reads a line from its standard input and writes `EGGOS-SHELL-CHILD: read <line>` (D181).
pub const READ_LINE: &str = "read-line";

/// Sleeps until a signal ends it; exits with 1 if the sleep ends (D181).
pub const SLEEP: &str = "sleep";

/// Sets the console's window size twice and writes [`WINDOW_LINE`] once the second change sent
/// it `SIGWINCH` (D181).
pub const WINDOW_CHANGE: &str = "window-change";

/// The line [`WAIT_FOR_INTERRUPT`] writes once it waits in the foreground group.
pub const WAITING_LINE: &str =
    "EGGOS-SHELL-CHILD: waiting for an interrupt in the foreground group";

/// The line [`STOP_AND_CONTINUE`] writes once it waits for `^Z` in the foreground group.
pub const STOP_WAITING_LINE: &str = "EGGOS-SHELL-CHILD: waiting for ^Z in the foreground group";

/// The line [`STOP_AND_CONTINUE`] writes once it runs again in the background.
pub const BACKGROUND_LINE: &str = "EGGOS-SHELL-CHILD: continued in the background";

/// The line [`STOP_AND_CONTINUE`] writes once it is back in the foreground.
pub const FOREGROUND_LINE: &str = "EGGOS-SHELL-CHILD: back in the foreground";

/// The line [`WINDOW_CHANGE`] writes: the window size it set last, which `SIGWINCH` told it of.
pub const WINDOW_LINE: &str = "EGGOS-SHELL-CHILD: SIGWINCH 31x101";

/// [`SHOW`]'s exit status.
pub const SHOW_STATUS: u8 = 3;

/// [`STOP_AND_CONTINUE`]'s exit status once back in the foreground.
pub const FOREGROUND_STATUS: u8 = 4;

const SIGTERM: i32 = 15;
const SIGCONT: i32 = 18;
const SIGWINCH: i32 = 28;
const TIOCGWINSZ: i32 = 0x5413;
const TIOCSWINSZ: i32 = 0x5414;
const WAIT: Duration = Duration::from_secs(60);
const LOOK_EVERY: Duration = Duration::from_millis(20);
const SIGNAL_WAIT: Duration = Duration::from_secs(2);

static CONTINUED: AtomicU32 = AtomicU32::new(0);
static WINDOW_CHANGES: AtomicU32 = AtomicU32::new(0);

mod libc {
    unsafe extern "C" {
        pub fn getpid() -> i32;
        pub safe fn getpgid(pid: i32) -> i32;
        pub safe fn tcgetpgrp(fd: i32) -> i32;
        pub fn kill(pid: i32, signal: i32) -> i32;
        pub fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
        pub fn ioctl(fd: i32, request: i32, ...) -> i32;
    }
}

extern "C" fn count_continue(_: i32) {
    CONTINUED.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn count_window_change(_: i32) {
    WINDOW_CHANGES.fetch_add(1, Ordering::SeqCst);
}

fn leads_the_foreground() -> Option<i32> {
    // SAFETY: a libc call with no arguments.
    let pid = unsafe { libc::getpid() };
    (libc::getpgid(0) == pid && libc::tcgetpgrp(0) == pid).then_some(pid)
}

fn set_window_size(rows: u16, columns: u16) {
    let size: [u16; 4] = [rows, columns, 0, 0];
    // SAFETY: `size` is a `struct winsize` (rows, columns, width, height) that lives across
    // the call.
    unsafe { libc::ioctl(0, TIOCSWINSZ, size.as_ptr()) };
}

fn window_changes_after(before: u32) -> bool {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_WAIT {
        if WINDOW_CHANGES.load(Ordering::SeqCst) > before {
            return true;
        }
        std::thread::sleep(LOOK_EVERY);
    }
    false
}

/// Runs the mode the arguments after [`SHELL_CHILD`] name.
pub fn run() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(2).collect();
    let mut output = std::io::stdout().lock();
    match arguments.first().map(String::as_str) {
        Some(SHOW) => {
            let name = arguments.get(1).map_or("", String::as_str);
            let value = std::env::var(name).unwrap_or_else(|_| "<unset>".to_owned());
            let rest = arguments.get(2..).unwrap_or_default().join("|");
            let _ = writeln!(output, "EGGOS-SHELL-CHILD: {name}={value} args={rest}");
            ExitCode::from(SHOW_STATUS)
        }
        Some(KILL_SELF) => {
            // SAFETY: a libc call with no arguments.
            let pid = unsafe { libc::getpid() };
            // SAFETY: a libc call that sends a signal to this process.
            unsafe { libc::kill(pid, SIGTERM) };
            std::thread::sleep(WAIT);
            ExitCode::FAILURE
        }
        Some(WAIT_FOR_INTERRUPT) => {
            if leads_the_foreground().is_none() {
                let _ = writeln!(output, "EGGOS-SHELL-CHILD: not in the foreground group");
                return ExitCode::FAILURE;
            }
            let _ = writeln!(output, "{WAITING_LINE}");
            let _ = output.flush();
            std::thread::sleep(WAIT);
            ExitCode::FAILURE
        }
        Some(STOP_AND_CONTINUE) => {
            let Some(pid) = leads_the_foreground() else {
                let _ = writeln!(output, "EGGOS-SHELL-CHILD: not in the foreground group");
                return ExitCode::FAILURE;
            };
            // SAFETY: `count_continue` only adds to an atomic, which a handler may do.
            unsafe { libc::signal(SIGCONT, count_continue) };
            let _ = writeln!(output, "{STOP_WAITING_LINE}");
            let _ = output.flush();
            let started = Instant::now();
            let mut in_background = false;
            while started.elapsed() < WAIT {
                std::thread::sleep(LOOK_EVERY);
                let foreground = libc::tcgetpgrp(0) == pid;
                let continued = CONTINUED.load(Ordering::SeqCst) > 0;
                if !in_background && !foreground && continued {
                    in_background = true;
                    let _ = writeln!(output, "{BACKGROUND_LINE}");
                    let _ = output.flush();
                }
                if continued && foreground {
                    let _ = writeln!(output, "{FOREGROUND_LINE}");
                    return ExitCode::from(FOREGROUND_STATUS);
                }
            }
            ExitCode::FAILURE
        }
        Some(READ_LINE) => {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(1..) => {
                    let _ = writeln!(output, "EGGOS-SHELL-CHILD: read {}", line.trim_end());
                    ExitCode::SUCCESS
                }
                _ => {
                    let _ = writeln!(output, "EGGOS-SHELL-CHILD: no line to read");
                    ExitCode::FAILURE
                }
            }
        }
        Some(SLEEP) => {
            std::thread::sleep(WAIT);
            ExitCode::FAILURE
        }
        Some(WINDOW_CHANGE) => {
            // SAFETY: `count_window_change` only adds to an atomic, which a handler may do.
            unsafe { libc::signal(SIGWINCH, count_window_change) };
            set_window_size(30, 100);
            let _ = window_changes_after(0);
            let before = WINDOW_CHANGES.load(Ordering::SeqCst);
            set_window_size(31, 101);
            let told = window_changes_after(before);
            let mut size = [0u16; 4];
            // SAFETY: `size` is a writable `struct winsize` that lives across the call.
            unsafe { libc::ioctl(0, TIOCGWINSZ, size.as_mut_ptr()) };
            let line = format!("EGGOS-SHELL-CHILD: SIGWINCH {}x{}", size[0], size[1]);
            match told && line == WINDOW_LINE {
                true => {
                    let _ = writeln!(output, "{line}");
                    ExitCode::SUCCESS
                }
                false => {
                    let _ = writeln!(output, "EGGOS-SHELL-CHILD: no SIGWINCH ({line})");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            let _ = writeln!(output, "EGGOS-SHELL-CHILD: unknown mode");
            ExitCode::FAILURE
        }
    }
}
