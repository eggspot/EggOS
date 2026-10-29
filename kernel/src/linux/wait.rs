//! Children's ends (D105): `wait4` and `waitid`, and what a process's end tells its parent,
//! `SIGCHLD` or the exit signal `clone` named, through the process table
//! ([`super::process_table`]), which keeps an ended child as a zombie until it is reaped.
//!
//! A thread waiting for a child sleeps on its process's child-wait key until a child's end
//! wakes it, a signal is due to it or its process ends. A signal that interrupts the wait runs
//! its handler and the call fails with `EINTR`, or is restarted if the handler has
//! `SA_RESTART`, as on Linux (`ERESTARTSYS`). Children never stop yet, so `WUNTRACED`,
//! `WSTOPPED` and `WCONTINUED` are accepted and never report anything; resource usage is
//! reported as zeros.

use super::futex::{self, Key};
use super::process_table::{self, ChildKind, Ends, Found, Notice, Selector, TABLE};
use super::signals::{self, Info, Interrupted};
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::{Errno, credentials};
use crate::process::{Process, ProcessExit};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The options of `wait4` and `waitid` (`W*`), the generic values, which x86_64 shares.
pub mod option {
    pub const WNOHANG: u32 = 0x1;
    pub const WUNTRACED: u32 = 0x2;
    pub const WSTOPPED: u32 = WUNTRACED;
    pub const WEXITED: u32 = 0x4;
    pub const WCONTINUED: u32 = 0x8;
    pub const WNOWAIT: u32 = 0x0100_0000;
    pub const WNOTHREAD: u32 = 0x2000_0000;
    pub const WALL: u32 = 0x4000_0000;
    pub const WCLONE: u32 = 0x8000_0000;
}

mod id_type {
    pub const P_ALL: u32 = 0;
    pub const P_PID: u32 = 1;
    pub const P_PGID: u32 = 2;
}

const WAIT4_OPTIONS: u32 = option::WNOHANG
    | option::WUNTRACED
    | option::WCONTINUED
    | option::WNOTHREAD
    | option::WALL
    | option::WCLONE;

const WAITID_OPTIONS: u32 = option::WNOHANG
    | option::WSTOPPED
    | option::WEXITED
    | option::WCONTINUED
    | option::WNOWAIT
    | option::WNOTHREAD
    | option::WALL
    | option::WCLONE;

const RUSAGE_SIZE: usize = 144;

const SIGINFO_SIZE: usize = 128;

const CHILD_WAIT_WORDS: u64 = 1 << 63;

static CHILD_ENDS: AtomicU32 = AtomicU32::new(0);

fn child_wait_key(parent: u32) -> Key {
    Key::new(0, CHILD_WAIT_WORDS | u64::from(parent), false)
}

/// Tells `process`'s parent of its end, recorded in the process table as `how` (see
/// [`process_table::Table::end`]), and the new parent of its children that ended already;
/// hangs up the console's foreground group if it led the console's session. Called once, as
/// the process ends.
#[cold]
#[inline(never)]
pub fn process_ended<P: Platform>(process: &Process<P>, how: ProcessExit) {
    let uid = credentials::get_real(process, credentials::Kind::User) as u32;
    let ended = TABLE.lock().end(process.id().get(), how, uid);
    if let Some(notice) = ended.to_parent {
        notify::<P>(notice);
    }
    if let Some(notice) = ended.to_new_parent {
        notify::<P>(notice);
    }
    for running in &ended.hung_up {
        for signal in [super::signal::SIGHUP, super::signal::SIGCONT] {
            send_to_running::<P>(running, Info::from_kernel(signal));
        }
    }
}

/// Wakes the threads of process `id` that wait for its children, which look again: for its
/// end or its `execve`, which its other threads do not outlive.
pub fn wake_child_waiters<P: Platform>(id: u32) {
    futex::wake::<P>(child_wait_key(id), u32::MAX, u32::MAX);
}

/// Wakes `notice`'s parent from its waits for its children and sends it the notice's signal,
/// if any.
pub fn notify<P: Platform>(notice: Notice) {
    CHILD_ENDS.fetch_add(1, Ordering::Release);
    futex::wake::<P>(child_wait_key(notice.parent), u32::MAX, u32::MAX);
    if notice.signal != 0
        && let Some(running) = &notice.running
    {
        let info = Info::child_ended(
            u32::from(notice.signal),
            notice.child,
            notice.uid,
            notice.exit,
        );
        send_to_running::<P>(running, info);
    }
}

/// Sends the signal `info` describes to the process `running` names, if it still runs.
pub fn send_to_running<P: Platform>(running: &process_table::Running, info: Info) {
    let Some(process) = running.process.upgrade() else {
        return;
    };
    if let Ok(process) = process.downcast::<Process<P>>() {
        signals::send_from_outside::<P>(&process, &running.first_thread, info);
    }
}

fn running<P: Platform>(process: &Arc<Process<P>>, thread: &LinuxThread) -> process_table::Running {
    process_table::Running {
        process: Arc::downgrade(process) as _,
        first_thread: Arc::clone(thread.wait_slot()),
        files: thread.files().downgrade(),
    }
}

/// Records `process`, whose first thread waits on `thread`'s slot, as the program its id runs
/// now, for signals sent to it from outside, adding it to the process table as one the kernel
/// started if it is not there yet; then, with the table's lock still held, does `also`.
pub fn record_running<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &LinuxThread,
    also: impl FnOnce(&mut process_table::Table, u32),
) {
    let running = running(process, thread);
    let id = process.id().get();
    let added = {
        let mut table = TABLE.lock();
        let added = table.add_if_missing(id);
        table.set_running(id, running);
        also(&mut table, id);
        added
    };
    if added.is_ok() {
        process.note_in_table();
    }
}

struct Request {
    selector: Selector,
    kind: ChildKind,
    no_hang: bool,
    ends: Ends,
}

fn selector_for_wait4(caller: u32, pid: i32) -> Result<Selector, Errno> {
    Ok(match pid {
        i32::MIN => return Err(Errno::ESRCH),
        -1 => Selector::Any,
        0 => Selector::Group(own_group(caller)),
        pid if pid < 0 => Selector::Group(pid.unsigned_abs()),
        pid => Selector::Process(pid as u32),
    })
}

fn own_group(caller: u32) -> u32 {
    TABLE
        .lock()
        .group_and_session(caller)
        .map_or(caller, |(group, _)| group)
}

fn kind(options: u32) -> ChildKind {
    if options & option::WALL != 0 {
        ChildKind::All
    } else if options & option::WCLONE != 0 {
        ChildKind::Clone
    } else {
        ChildKind::Sigchld
    }
}

/// `wait4(pid, status, options, rusage)`: waits for a child `pid` names to end (a child by id
/// for `pid > 0`, any child in the caller's group for 0, any child for -1, any in group `-pid`
/// below that) and reaps it: returns its id, with its status at `status` (`exit << 8`, or the
/// signal that ended it) and zeros at `rusage`, unless they are null. With `WNOHANG`, returns
/// 0 at once if none has ended. As Linux: `EINVAL` for an unknown option; `ESRCH` for
/// `INT_MIN`; `ECHILD` with no such child; `EFAULT` if `status` or `rusage` is not the
/// program's writable memory (the child is reaped all the same).
#[inline(never)]
pub fn wait4<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [pid, status, options, rusage, ..] = context.syscall_arguments();
    let options = options as u32;
    let caller = process.id().get();
    let request = match (
        options & !WAIT4_OPTIONS,
        selector_for_wait4(caller, pid as i32),
    ) {
        (0, Ok(selector)) => Ok(Request {
            selector,
            kind: kind(options),
            no_hang: options & option::WNOHANG != 0,
            ends: Ends::Reap,
        }),
        (0, Err(error)) => Err(error),
        _ => Err(Errno::EINVAL),
    };
    let found = match request {
        Ok(request) => match wait_interruptibly(process, thread, context, &request)? {
            Some(found) => found,
            None => return ControlFlow::Continue(()),
        },
        Err(error) => Err(error),
    };
    let result = found.and_then(|found| match found {
        Some((id, _, exit)) => {
            let written = write_wait_status(process, status, exit)
                .and_then(|()| write_zeros::<P, RUSAGE_SIZE>(process, rusage));
            written.map(|()| u64::from(id))
        }
        None => Ok(0),
    });
    finish(process, thread, context, result)
}

/// `waitid(idtype, id, infop, options, rusage)`: as [`wait4`], with the children named by
/// `idtype` (`P_ALL`, `P_PID`, `P_PGID`, 0 for the caller's own group) and the child's end
/// written to the `siginfo_t` at `infop` (`SIGCHLD`, `CLD_EXITED` or `CLD_KILLED`, its id, real
/// user id and status); the child stays a zombie with `WNOWAIT`. Returns 0; with `WNOHANG`
/// and no child ended, the `siginfo_t` is zeros. As Linux: `EINVAL` for an unknown option,
/// none of `WEXITED`, `WSTOPPED` and `WCONTINUED`, an unknown `idtype` (`P_PIDFD` among them)
/// or an id below 1 for `P_PID`, or below 0 for `P_PGID`; `ECHILD` with no such child.
#[inline(never)]
pub fn waitid<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [id_type, id, infop, options, rusage, ..] = context.syscall_arguments();
    let options = options as u32;
    let id = id as u32 as i32;
    let caller = process.id().get();
    let reported = option::WEXITED | option::WSTOPPED | option::WCONTINUED;
    let selector = match id_type as u32 {
        _ if options & !WAITID_OPTIONS != 0 || options & reported == 0 => Err(Errno::EINVAL),
        id_type::P_ALL => Ok(Selector::Any),
        id_type::P_PID if id > 0 => Ok(Selector::Process(id as u32)),
        id_type::P_PGID if id == 0 => Ok(Selector::Group(own_group(caller))),
        id_type::P_PGID if id > 0 => Ok(Selector::Group(id as u32)),
        _ => Err(Errno::EINVAL),
    };
    let found = match selector {
        Ok(selector) => {
            let request = Request {
                selector,
                kind: kind(options),
                no_hang: options & option::WNOHANG != 0,
                ends: match (options & option::WEXITED, options & option::WNOWAIT) {
                    (0, _) => Ends::Ignore,
                    (_, 0) => Ends::Reap,
                    _ => Ends::Report,
                },
            };
            match wait_interruptibly(process, thread, context, &request)? {
                Some(found) => found,
                None => return ControlFlow::Continue(()),
            }
        }
        Err(error) => Err(error),
    };
    let result = found.and_then(|found| {
        write_child_info(process, infop, found)?;
        write_zeros::<P, RUSAGE_SIZE>(process, rusage)?;
        Ok(0)
    });
    finish(process, thread, context, result)
}

fn finish<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    result: Result<u64, Errno>,
) -> ControlFlow<()> {
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

type Reaped = Option<(u32, u32, ProcessExit)>;

fn wait_interruptibly<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    request: &Request,
) -> ControlFlow<(), Option<Result<Reaped, Errno>>> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    loop {
        if let Some(result) = wait_or_sleep(process, thread, request)? {
            return ControlFlow::Continue(Some(result));
        }
        if signals::deliver_interrupting(process, thread, context, interrupted)? {
            return ControlFlow::Continue(None);
        }
    }
}

fn wait_or_sleep<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    request: &Request,
) -> ControlFlow<(), Option<Result<Reaped, Errno>>> {
    let caller = process.id().get();
    loop {
        let seen = CHILD_ENDS.load(Ordering::Acquire);
        let found = TABLE
            .lock()
            .wait(caller, request.selector, request.kind, request.ends);
        match found {
            Found::Ended { id, uid, exit } => {
                return ControlFlow::Continue(Some(Ok(Some((id, uid, exit)))));
            }
            Found::NoChild => return ControlFlow::Continue(Some(Err(Errno::ECHILD))),
            _ if request.no_hang => return ControlFlow::Continue(Some(Ok(None))),
            _ => {}
        }
        if process.has_ended() {
            return ControlFlow::Break(());
        }
        signals::take_in_sent(process, thread);
        if signals::is_due(thread) {
            return ControlFlow::Continue(None);
        }
        let unless_signalled_or_ended = unless_signalled_or_ended(process, thread);
        let waited = futex::wait_interruptibly::<P>(
            child_wait_key(caller),
            u32::MAX,
            u64::MAX,
            thread.wait_slot(),
            || {
                unless_signalled_or_ended()?;
                match CHILD_ENDS.load(Ordering::Acquire) == seen {
                    true => Ok(()),
                    false => Err(Errno::EAGAIN),
                }
            },
        );
        if waited == Err(Errno::EINTR) {
            signals::take_in_sent_now(process, thread);
        }
    }
}

/// The status `wait4` reports for a child that ended as `exit`: its exit status in bits 8 to
/// 15, or the signal that ended it in the low 7 bits.
pub fn wait_status(exit: ProcessExit) -> u32 {
    match exit {
        ProcessExit::Exited(status) => (status & 0xff) << 8,
        ProcessExit::Killed { signal } => signal & 0x7f,
    }
}

fn write_wait_status<P: Platform>(
    process: &Process<P>,
    address: u64,
    exit: ProcessExit,
) -> Result<(), Errno> {
    if address == 0 {
        return Ok(());
    }
    process
        .write_user_memory(address, &wait_status(exit).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)
}

fn write_zeros<P: Platform, const SIZE: usize>(
    process: &Process<P>,
    address: u64,
) -> Result<(), Errno> {
    if address == 0 {
        return Ok(());
    }
    process
        .write_user_memory(address, &[0; SIZE])
        .map_err(|_| Errno::EFAULT)
}

fn write_child_info<P: Platform>(
    process: &Process<P>,
    address: u64,
    found: Reaped,
) -> Result<(), Errno> {
    if address == 0 {
        return Ok(());
    }
    let mut bytes = [0; SIGINFO_SIZE];
    if let Some((id, uid, exit)) = found {
        let info = Info::child_ended(super::signal::SIGCHLD, id, uid, exit).to_bytes();
        bytes.copy_from_slice(&info[..SIGINFO_SIZE]);
    }
    process
        .write_user_memory(address, &bytes)
        .map_err(|_| Errno::EFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_encoded_as_linux_s_wait_macros_read_them() {
        assert_eq!(wait_status(ProcessExit::Exited(0)), 0);
        assert_eq!(wait_status(ProcessExit::Exited(7)), 0x700);
        assert_eq!(wait_status(ProcessExit::Exited(0x1ff)), 0xff00);
        assert_eq!(wait_status(ProcessExit::Killed { signal: 9 }), 9);
        assert_eq!(wait_status(ProcessExit::Killed { signal: 15 }), 15);
    }

    #[test]
    fn wait4_selects_children_as_linux_reads_its_pid() {
        assert_eq!(selector_for_wait4(5, -1), Ok(Selector::Any));
        assert_eq!(selector_for_wait4(5, 9), Ok(Selector::Process(9)));
        assert_eq!(selector_for_wait4(5, -9), Ok(Selector::Group(9)));
        assert_eq!(selector_for_wait4(5, i32::MIN), Err(Errno::ESRCH));
        assert_eq!(kind(option::WCLONE), ChildKind::Clone);
        assert_eq!(kind(option::WALL | option::WCLONE), ChildKind::All);
        assert_eq!(kind(option::WNOHANG), ChildKind::Sigchld);
    }
}
