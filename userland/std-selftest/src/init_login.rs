//! The init boot test's login program (D106): init starts it in place of the shell, and it
//! checks what a login program gets from init. Its parent is init, process 1, and it leads a
//! session of its own whose group is the console's foreground group. A grandchild whose parent
//! ended is reparented to init (`getppid` 1) and init reaps it. `reboot` with bad magic is
//! refused with `EINVAL`. Each check writes its line ([`LINES`]); a failure writes
//! `EGGOS-INIT-TEST: FAILED: ` and the check, and the program exits with 1. Then it exits,
//! and init powers the machine off, as its configuration in the test initramfs says.

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The argument that runs [`log_in`].
pub const INIT_LOGIN: &str = "init-login";

/// The lines a passing run writes, in order.
pub const LINES: [&str; 4] = [
    "EGGOS-INIT-TEST: the login program is init's child, leading the console's session",
    "EGGOS-INIT-TEST: an orphan was reparented to init",
    "EGGOS-INIT-TEST: init reaped the orphan",
    "EGGOS-INIT-TEST: reboot with bad magic was refused",
];

const EINVAL: i32 = 22;
const ESRCH: i32 = 3;
const SYS_REBOOT: i64 = if cfg!(target_arch = "x86_64") {
    169
} else {
    142
};
const REBOOT_POWER_OFF: i64 = 0x4321_fedc;
const ORPHAN_FAILED: i32 = 1;
const NO_SMALL_PID: i32 = 255;
const DEADLINE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(1);

mod libc {
    unsafe extern "C" {
        pub fn fork() -> i32;
        pub fn _exit(status: i32) -> !;
        pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        pub fn syscall(number: i64, ...) -> i64;
        pub fn getpid() -> i32;
        pub fn getppid() -> i32;
        pub safe fn getpgid(pid: i32) -> i32;
        pub safe fn getsid(pid: i32) -> i32;
        pub safe fn tcgetpgrp(fd: i32) -> i32;
    }
}

fn errno() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

fn getpid() -> i32 {
    // SAFETY: a libc call with no arguments.
    unsafe { libc::getpid() }
}

fn getppid() -> i32 {
    // SAFETY: a libc call with no arguments.
    unsafe { libc::getppid() }
}

fn fork() -> i32 {
    // SAFETY: this program has one thread here; each child only makes plain calls and ends
    // with `_exit`.
    unsafe { libc::fork() }
}

fn exit_child(status: i32) -> ! {
    // SAFETY: `_exit` ends the process at once.
    unsafe { libc::_exit(status) }
}

fn waitpid(pid: i32) -> Option<i32> {
    let mut status = -1;
    // SAFETY: `status` is an `int` that lives across the call.
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    (waited == pid && status & 0x7f == 0).then_some((status >> 8) & 0xff)
}

fn until(done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() > DEADLINE {
            return false;
        }
        std::thread::sleep(POLL);
    }
    true
}

fn leads_the_console_session() -> bool {
    let me = getpid();
    getppid() == 1 && me != 1 && libc::getsid(0) == me && libc::tcgetpgrp(0) == me
}

fn orphan() -> ! {
    let reparented = until(|| getppid() == 1);
    if reparented {
        let _ = writeln!(std::io::stdout(), "{}", LINES[1]);
    }
    exit_child(if reparented { 0 } else { ORPHAN_FAILED })
}

fn start_an_orphan() -> Option<i32> {
    match fork() {
        0 => match fork() {
            0 => orphan(),
            grandchild if (1..NO_SMALL_PID).contains(&grandchild) => exit_child(grandchild),
            _ => exit_child(NO_SMALL_PID),
        },
        child if child > 0 => waitpid(child).filter(|&grandchild| grandchild != NO_SMALL_PID),
        _ => None,
    }
}

fn reaped_by_init(orphan: i32) -> bool {
    until(|| libc::getpgid(orphan) == -1 && errno() == Some(ESRCH))
}

fn bad_magic_is_refused() -> bool {
    // SAFETY: `reboot` with bad magic changes nothing; it only reads its four integers.
    let result = unsafe { libc::syscall(SYS_REBOOT, 0i64, 0i64, REBOOT_POWER_OFF, 0i64) };
    result == -1 && errno() == Some(EINVAL)
}

/// Runs the checks (see the module documentation) and returns the exit status.
pub fn log_in() -> ExitCode {
    let mut output = std::io::stdout();
    let fail = |output: &mut std::io::Stdout, check: &str| {
        let _ = writeln!(output, "EGGOS-INIT-TEST: FAILED: {check}");
        ExitCode::FAILURE
    };
    if !leads_the_console_session() {
        return fail(&mut output, "init's child leads the console's session");
    }
    let _ = writeln!(output, "{}", LINES[0]);
    let Some(orphan) = start_an_orphan() else {
        return fail(&mut output, "starting an orphan");
    };
    if !reaped_by_init(orphan) {
        return fail(&mut output, "init reaps the orphan");
    }
    let _ = writeln!(output, "{}", LINES[2]);
    if !bad_magic_is_refused() {
        return fail(&mut output, "reboot with bad magic is refused");
    }
    let _ = writeln!(output, "{}", LINES[3]);
    ExitCode::SUCCESS
}
