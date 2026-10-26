//! The commands the shell boot test runs from `eggsh` (D107): given [`SHELL_CHILD`] and one of
//! [`SHOW`], [`KILL_SELF`] or [`WAIT_FOR_INTERRUPT`], the program shows what the shell gave it,
//! ends by a signal, or waits in the console's foreground group for `^C`.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

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

/// The line [`WAIT_FOR_INTERRUPT`] writes once it waits in the foreground group.
pub const WAITING_LINE: &str =
    "EGGOS-SHELL-CHILD: waiting for an interrupt in the foreground group";

/// [`SHOW`]'s exit status.
pub const SHOW_STATUS: u8 = 3;

const SIGTERM: i32 = 15;
const WAIT: Duration = Duration::from_secs(60);

mod libc {
    unsafe extern "C" {
        pub fn getpid() -> i32;
        pub safe fn getpgid(pid: i32) -> i32;
        pub safe fn tcgetpgrp(fd: i32) -> i32;
        pub fn kill(pid: i32, signal: i32) -> i32;
    }
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
            // SAFETY: a libc call with no arguments.
            let pid = unsafe { libc::getpid() };
            if libc::getpgid(0) != pid || libc::tcgetpgrp(0) != pid {
                let _ = writeln!(output, "EGGOS-SHELL-CHILD: not in the foreground group");
                return ExitCode::FAILURE;
            }
            let _ = writeln!(output, "{WAITING_LINE}");
            let _ = output.flush();
            std::thread::sleep(WAIT);
            ExitCode::FAILURE
        }
        _ => {
            let _ = writeln!(output, "EGGOS-SHELL-CHILD: unknown mode");
            ExitCode::FAILURE
        }
    }
}
