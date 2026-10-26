//! `execve` from a file (D103), through musl as a C program makes it: the errors it gives
//! before the point of no return, then a copy of this program started from the initramfs
//! with arguments and an environment of its own, a close-on-exec descriptor and a plain one,
//! a caught signal and an ignored one; the copy checks what it was given and what it kept.

use std::ffi::{CStr, CString, c_char};
use std::fs::File;
use std::io::Read;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::process::ExitCode;

pub const EXEC: &str = "exec";

pub const EXEC_CHECK: &str = "exec-check";

pub mod exec_status {
    pub const REFUSED_WRONGLY: u8 = 2;
    pub const EXECVE_RETURNED: u8 = 3;
    pub const CHECK_FAILED: u8 = 4;
}

const PROGRAM: &CStr = c"/usr/libexec/eggos-std-selftest";
const ENVIRONMENT: &CStr = c"EGGOS_EXEC=from-a-file";
const HOSTNAME: &[u8] = b"eggos\n";

const ENOENT: i32 = 2;
const EACCES: i32 = 13;
const EFAULT: i32 = 14;
const ENOTDIR: i32 = 20;
const EBADF: i32 = 9;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const SIGUSR1: i32 = 10;
const SIGUSR2: i32 = 12;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    restorer: usize,
}

mod libc {
    use super::SigAction;
    use std::ffi::c_char;

    unsafe extern "C" {
        pub fn execve(
            path: *const c_char,
            arguments: *const *const c_char,
            environment: *const *const c_char,
        ) -> i32;
        pub fn sigaction(signal: i32, new: *const SigAction, old: *mut SigAction) -> i32;
        pub fn fcntl(fd: i32, command: i32, ...) -> i32;
        pub fn getpid() -> i32;
    }
}

extern "C" fn on_signal(_: i32) {}

fn error() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn execve(path: *const c_char, arguments: &[*const c_char], environment: &[*const c_char]) -> i32 {
    // SAFETY: `path` is null or a string, and both lists end with a null pointer to strings
    // that outlive the call, or are deliberately invalid addresses the kernel must refuse.
    unsafe { libc::execve(path, arguments.as_ptr(), environment.as_ptr()) }
}

fn refused_with(path: &CStr, arguments: *const *const c_char, errno: i32) -> bool {
    let environment = [std::ptr::null()];
    // SAFETY: as in `execve`; `arguments` may be an invalid address on purpose.
    let result = unsafe { libc::execve(path.as_ptr(), arguments, environment.as_ptr()) };
    result == -1 && error() == errno
}

fn set_action(signal: i32, handler: usize) -> bool {
    let action = SigAction {
        handler,
        mask: [0; 16],
        flags: 0,
        restorer: 0,
    };
    // SAFETY: `action` is a valid `struct sigaction` for the call's length.
    unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) == 0 }
}

fn handler_of(signal: i32) -> Option<usize> {
    let mut old = SigAction {
        handler: usize::MAX,
        mask: [0; 16],
        flags: 0,
        restorer: 0,
    };
    // SAFETY: `old` is a writable `struct sigaction`.
    let result = unsafe { libc::sigaction(signal, std::ptr::null(), &mut old) };
    (result == 0).then_some(old.handler)
}

fn descriptor_flags(fd: i32) -> i32 {
    // SAFETY: `F_GETFD` takes no argument and touches no memory.
    unsafe { libc::fcntl(fd, F_GETFD) }
}

fn getpid() -> i32 {
    // SAFETY: `getpid` touches no memory.
    unsafe { libc::getpid() }
}

pub fn exec_into_a_checking_copy() -> ExitCode {
    let no_arguments = [std::ptr::null::<c_char>()];
    let refusals = [
        refused_with(c"/no-such-program", no_arguments.as_ptr(), ENOENT),
        refused_with(c"/etc", no_arguments.as_ptr(), EACCES),
        refused_with(c"/etc/hostname", no_arguments.as_ptr(), EACCES),
        refused_with(c"/etc/hostname/program", no_arguments.as_ptr(), ENOTDIR),
        refused_with(PROGRAM, std::ptr::dangling(), EFAULT),
    ];
    if refusals.contains(&false) {
        println!("EGGOS-USER: exec: refusals {refusals:?}");
        return ExitCode::from(exec_status::REFUSED_WRONGLY);
    }
    let Ok(closed_on_exec) = File::open("/etc/hostname") else {
        return ExitCode::from(exec_status::REFUSED_WRONGLY);
    };
    let Ok(kept) = File::open("/etc/hostname") else {
        return ExitCode::from(exec_status::REFUSED_WRONGLY);
    };
    let closed_on_exec = closed_on_exec.into_raw_fd();
    let kept = kept.into_raw_fd();
    // SAFETY: `F_SETFD` takes an `int` and touches no memory.
    let cleared = unsafe { libc::fcntl(kept, F_SETFD, 0) };
    if cleared != 0
        || !set_action(SIGUSR1, on_signal as extern "C" fn(i32) as usize)
        || !set_action(SIGUSR2, SIG_IGN)
    {
        return ExitCode::from(exec_status::REFUSED_WRONGLY);
    }
    let strings = [
        EXEC_CHECK.to_owned(),
        kept.to_string(),
        closed_on_exec.to_string(),
        getpid().to_string(),
    ]
    .map(|string| CString::new(string).unwrap_or_default());
    let mut arguments = vec![PROGRAM.as_ptr()];
    arguments.extend(strings.iter().map(|string| string.as_ptr()));
    arguments.push(std::ptr::null());
    let environment = [ENVIRONMENT.as_ptr(), std::ptr::null()];
    execve(PROGRAM.as_ptr(), &arguments, &environment);
    println!("EGGOS-USER: exec: execve returned with errno {}", error());
    ExitCode::from(exec_status::EXECVE_RETURNED)
}

pub fn check_after_exec() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    let number = |index: usize| -> i32 {
        arguments
            .get(index)
            .and_then(|text| text.parse().ok())
            .unwrap_or(-1)
    };
    let (kept, closed_on_exec, pid) = (number(2), number(3), number(4));
    let environment: Vec<(String, String)> = std::env::vars().collect();
    let mut kept_contents = Vec::new();
    // SAFETY: the descriptor was handed over open by the program before `execve`, and only
    // this `File` owns it from here on.
    let read = unsafe { File::from_raw_fd(kept) }.read_to_end(&mut kept_contents);
    let checks = [
        (
            "the arguments are those passed",
            arguments.len() == 5
                && arguments[0].as_bytes() == PROGRAM.to_bytes()
                && arguments[1] == EXEC_CHECK,
        ),
        (
            "the environment is the one passed",
            environment == [("EGGOS_EXEC".to_owned(), "from-a-file".to_owned())],
        ),
        ("the process id is kept", getpid() == pid),
        (
            "a plain descriptor stays open",
            read.is_ok() && kept_contents == HOSTNAME,
        ),
        (
            "a close-on-exec descriptor is closed",
            descriptor_flags(closed_on_exec) == -1 && error() == EBADF,
        ),
        (
            "a caught signal is back to its default action",
            handler_of(SIGUSR1) == Some(SIG_DFL),
        ),
        (
            "an ignored signal stays ignored",
            handler_of(SIGUSR2) == Some(SIG_IGN),
        ),
    ];
    let mut passed = true;
    for (name, check) in checks {
        if !check {
            println!("EGGOS-USER: exec: {name}: FAILED");
            passed = false;
        }
    }
    if !passed {
        return ExitCode::from(exec_status::CHECK_FAILED);
    }
    println!("EGGOS-USER: a std program started by execve from a file passed its checks");
    ExitCode::SUCCESS
}
