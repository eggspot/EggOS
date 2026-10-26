//! Given [`CHILDREN`], the program waits for its children as C programs do through musl
//! (D105): `waitpid` (`wait4`) for a child that already ended, one that ends during the wait
//! and one a signal ended, `WNOHANG` and `ECHILD`, `waitid` with `WNOWAIT`, a `SIGCHLD`
//! handler, children reaped by the kernel while `SIGCHLD` is ignored, an orphaned grandchild
//! taken in as a subreaper, and process groups and sessions. It exits with 0 if every check
//! passed, or names each that failed and exits with 1.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const CHILDREN: &str = "children";

const SIGTERM: i32 = 15;
const SIGCHLD: i32 = 17;
const SIG_IGN: usize = 1;
const SIG_BLOCK: i32 = 0;
const SIG_SETMASK: i32 = 2;
const WNOHANG: i32 = 1;
const WEXITED: i32 = 4;
const WNOWAIT: i32 = 0x0100_0000;
const P_PID: i32 = 1;
const CLD_EXITED: i32 = 1;
const ECHILD: i32 = 10;
const EPERM: i32 = 1;
const ENOTTY: i32 = 25;
const PR_SET_CHILD_SUBREAPER: i32 = 36;
const SIGSET_WORDS: usize = 16;
const ORPHAN_STATUS: i32 = 40;
const MOST_POLLS: u32 = 5000;
const SHORT: Duration = Duration::from_millis(20);
const POLL: Duration = Duration::from_millis(1);

static CHILD_SIGNALS: AtomicU32 = AtomicU32::new(0);

mod libc {
    unsafe extern "C" {
        pub fn fork() -> i32;
        pub fn _exit(status: i32) -> !;
        pub fn getpid() -> i32;
        pub fn getppid() -> i32;
        pub fn kill(pid: i32, signal: i32) -> i32;
        pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        pub fn waitid(id_type: i32, id: u32, info: *mut [u8; 128], options: i32) -> i32;
        pub fn bsd_signal(signal: i32, handler: usize) -> usize;
        pub fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32;
        pub fn sigsuspend(mask: *const u64) -> i32;
        pub fn prctl(option: i32, ...) -> i32;
        pub safe fn setpgid(pid: i32, group: i32) -> i32;
        pub safe fn getpgid(pid: i32) -> i32;
        pub safe fn getpgrp() -> i32;
        pub safe fn setsid() -> i32;
        pub safe fn getsid(pid: i32) -> i32;
        pub safe fn tcgetpgrp(fd: i32) -> i32;
    }
}

fn errno() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

use libc::{getpgid, getpgrp, getsid, setpgid, setsid, tcgetpgrp};

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

/// `waitpid(pid, &status, options)`: the id it returned and the status.
fn waitpid(pid: i32, options: i32) -> (i32, i32) {
    let mut status = -1;
    // SAFETY: `status` is an `int` that lives across the call.
    let waited = unsafe { libc::waitpid(pid, &mut status, options) };
    (waited, status)
}

fn exited_with(status: i32) -> Option<i32> {
    (status & 0x7f == 0).then_some((status >> 8) & 0xff)
}

fn killed_by(status: i32) -> Option<i32> {
    let signal = status & 0x7f;
    (signal != 0 && signal != 0x7f).then_some(signal)
}

fn set_handler(signal: i32, handler: usize) -> usize {
    // SAFETY: `handler` is `SIG_DFL`, `SIG_IGN` or an `extern "C" fn(i32)` that only touches
    // atomics.
    unsafe { libc::bsd_signal(signal, handler) }
}

fn set_mask(how: i32, set: &[u64; SIGSET_WORDS]) -> [u64; SIGSET_WORDS] {
    let mut old = [0; SIGSET_WORDS];
    // SAFETY: two signal sets of musl's size that live across the call.
    unsafe { libc::pthread_sigmask(how, set.as_ptr(), old.as_mut_ptr()) };
    old
}

fn only(signal: i32) -> [u64; SIGSET_WORDS] {
    let mut set = [0; SIGSET_WORDS];
    set[0] = 1 << (signal - 1);
    set
}

fn a_child_that_already_ended_is_reaped_with_its_status() -> bool {
    let child = fork();
    if child == 0 {
        exit_child(7);
    }
    std::thread::sleep(SHORT);
    let (waited, status) = waitpid(child, 0);
    waited == child && exited_with(status) == Some(7) && waitpid(child, 0).0 == -1
}

fn a_wait_wakes_as_its_child_ends() -> bool {
    let child = fork();
    if child == 0 {
        std::thread::sleep(SHORT);
        exit_child(8);
    }
    let (waited, status) = waitpid(-1, 0);
    waited == child && exited_with(status) == Some(8)
}

fn a_child_a_signal_ended_reports_the_signal() -> bool {
    let child = fork();
    if child == 0 {
        // SAFETY: a plain libc call; the default action of `SIGTERM` ends this child.
        unsafe { libc::kill(getpid(), SIGTERM) };
        exit_child(1);
    }
    let (waited, status) = waitpid(child, 0);
    waited == child && killed_by(status) == Some(SIGTERM)
}

fn wnohang_returns_at_once_and_no_child_is_echild() -> bool {
    let child = fork();
    if child == 0 {
        std::thread::sleep(SHORT);
        exit_child(0);
    }
    let running = waitpid(-1, WNOHANG).0 == 0;
    let reaped = waitpid(0, 0).0 == child;
    let none = waitpid(-1, 0).0 == -1 && errno() == Some(ECHILD);
    let none_without_waiting = waitpid(-1, WNOHANG).0 == -1 && errno() == Some(ECHILD);
    running && reaped && none && none_without_waiting
}

fn word(info: &[u8; 128], offset: usize) -> i32 {
    i32::from_ne_bytes([
        info[offset],
        info[offset + 1],
        info[offset + 2],
        info[offset + 3],
    ])
}

fn waitid_reports_the_end_and_wnowait_leaves_the_child() -> bool {
    let child = fork();
    if child == 0 {
        exit_child(9);
    }
    let mut info = [0xffu8; 128];
    // SAFETY: `info` is a `siginfo_t`-sized buffer that lives across the call.
    let looked = unsafe { libc::waitid(P_PID, child as u32, &mut info, WEXITED | WNOWAIT) };
    let reported = looked == 0
        && word(&info, 0) == SIGCHLD
        && word(&info, 8) == CLD_EXITED
        && word(&info, 16) == child
        && word(&info, 24) == 9;
    let (waited, status) = waitpid(child, 0);
    reported && waited == child && exited_with(status) == Some(9)
}

extern "C" fn count_child_signal(_: i32) {
    CHILD_SIGNALS.fetch_add(1, Ordering::SeqCst);
}

fn a_sigchld_handler_runs_as_a_child_ends() -> bool {
    let before = CHILD_SIGNALS.load(Ordering::SeqCst);
    let previous = set_handler(SIGCHLD, count_child_signal as extern "C" fn(i32) as usize);
    let old_mask = set_mask(SIG_BLOCK, &only(SIGCHLD));
    let child = fork();
    if child == 0 {
        exit_child(0);
    }
    let none = [0; SIGSET_WORDS];
    while CHILD_SIGNALS.load(Ordering::SeqCst) == before {
        // SAFETY: `none` is a signal set of musl's size.
        unsafe { libc::sigsuspend(none.as_ptr()) };
    }
    set_mask(SIG_SETMASK, &old_mask);
    set_handler(SIGCHLD, previous);
    let (waited, status) = waitpid(child, 0);
    waited == child && exited_with(status) == Some(0)
}

fn ignored_sigchld_leaves_no_zombie() -> bool {
    let previous = set_handler(SIGCHLD, SIG_IGN);
    let child = fork();
    if child == 0 {
        std::thread::sleep(SHORT);
        exit_child(0);
    }
    let waited_for_all = waitpid(-1, 0).0 == -1 && errno() == Some(ECHILD);
    let restored = set_handler(SIGCHLD, previous) == SIG_IGN;
    child > 0 && waited_for_all && restored
}

fn an_orphan_goes_to_the_subreaper_which_reaps_it() -> bool {
    let subreaper = getpid();
    // SAFETY: a plain libc call.
    if unsafe { libc::prctl(PR_SET_CHILD_SUBREAPER, 1u64) } != 0 {
        return false;
    }
    let middle = fork();
    if middle == 0 {
        let middle = getpid();
        let orphan = fork();
        if orphan == 0 {
            let mut polls = 0;
            while getppid() == middle && polls < MOST_POLLS {
                std::thread::sleep(POLL);
                polls += 1;
            }
            exit_child(match getppid() == subreaper {
                true => ORPHAN_STATUS,
                false => 1,
            });
        }
        exit_child(0);
    }
    let middle_ended = waitpid(middle, 0);
    let orphan_ended = waitpid(-1, 0);
    // SAFETY: a plain libc call.
    let cleared = unsafe { libc::prctl(PR_SET_CHILD_SUBREAPER, 0u64) } == 0;
    middle_ended.0 == middle
        && exited_with(middle_ended.1) == Some(0)
        && orphan_ended.0 > 0
        && orphan_ended.0 != middle
        && exited_with(orphan_ended.1) == Some(ORPHAN_STATUS)
        && cleared
}

fn in_new_session() -> i32 {
    let own = getpid();
    let checks = setsid() == own && getsid(0) == own && getpgid(0) == own && tcgetpgrp(0) == -1;
    match checks && errno() == Some(ENOTTY) {
        true => 0,
        false => 1,
    }
}

fn in_new_group() -> i32 {
    let own = getpid();
    let grouped = setpgid(0, 0) == 0 && getpgrp() == own && getpgid(0) == own;
    let refused = setsid() == -1 && errno() == Some(EPERM);
    let leader = fork();
    if leader == 0 {
        exit_child(in_new_session());
    }
    let (waited, status) = waitpid(leader, 0);
    match grouped && refused && waited == leader && exited_with(status) == Some(0) {
        true => 0,
        false => 1,
    }
}

fn processes_move_between_groups_and_sessions() -> bool {
    let own = getpid();
    let leads = getsid(0) == own && getpgrp() == own && tcgetpgrp(0) == own;
    let leader_stays = setpgid(0, 0) == -1 && errno() == Some(EPERM);
    let child = fork();
    if child == 0 {
        exit_child(in_new_group());
    }
    let (waited, status) = waitpid(child, 0);
    leads && leader_stays && waited == child && exited_with(status) == Some(0)
}

type Check = (&'static str, fn() -> bool);

const CHECKS: [Check; 9] = [
    (
        "waitpid reaps a child that already ended, with its status, once",
        a_child_that_already_ended_is_reaped_with_its_status,
    ),
    (
        "waitpid wakes as its child ends",
        a_wait_wakes_as_its_child_ends,
    ),
    (
        "waitpid reports the signal that ended a child",
        a_child_a_signal_ended_reports_the_signal,
    ),
    (
        "WNOHANG returns 0 while a child runs and no child is ECHILD",
        wnohang_returns_at_once_and_no_child_is_echild,
    ),
    (
        "waitid reports CLD_EXITED and WNOWAIT leaves the child to reap",
        waitid_reports_the_end_and_wnowait_leaves_the_child,
    ),
    (
        "a SIGCHLD handler runs as a child ends",
        a_sigchld_handler_runs_as_a_child_ends,
    ),
    (
        "the kernel reaps the children of a process ignoring SIGCHLD",
        ignored_sigchld_leaves_no_zombie,
    ),
    (
        "an orphan goes to the subreaper, which reaps it",
        an_orphan_goes_to_the_subreaper_which_reaps_it,
    ),
    (
        "setpgid, setsid and their getters move and report groups and sessions",
        processes_move_between_groups_and_sessions,
    ),
];

pub fn wait_for_children() -> ExitCode {
    let mut passed = true;
    for (name, check) in CHECKS {
        if !check() {
            println!("EGGOS-USER: children: {name}: FAILED");
            passed = false;
        }
    }
    if !passed {
        return ExitCode::FAILURE;
    }
    println!("EGGOS-USER: a std program waited for its children and moved them between groups");
    ExitCode::SUCCESS
}
