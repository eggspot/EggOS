//! Job control (D105, D181): process groups and sessions (`setpgid`, `getpgid`, `getpgrp`,
//! `setsid`, `getsid`), the console as a session's controlling terminal and its foreground
//! process group (`TIOCSCTTY`, `TIOCGPGRP`, `TIOCSPGRP`, `TIOCGSID`), which the console's
//! signal characters reach, `prctl`'s `PR_SET_CHILD_SUBREAPER`, which takes in a descendant's
//! orphans, and stopping and continuing processes. All of it is kept in the process table
//! ([`super::process_table`]).
//!
//! A process the kernel starts leads a new session and group, and the console becomes that
//! session's controlling terminal if it is no session's yet, as a session leader's `open` of a
//! terminal makes it (its descriptors 0 to 2 are the console already).
//!
//! A stop signal's default action stops the whole process, every thread, until `SIGCONT` or
//! `SIGKILL`; its parent is woken from its waits and sent `SIGCHLD` (`CLD_STOPPED`, and
//! `CLD_CONTINUED` when it continues) unless it set `SA_NOCLDSTOP`. `SIGTSTP`, `SIGTTIN` and
//! `SIGTTOU` do not stop a process whose group is orphaned. A process in a background group of
//! the console's session that reads the console gets `SIGTTIN` (sent to its whole group), and
//! one that changes the console's settings or foreground group gets `SIGTTOU`, unless it
//! ignores or blocks it; the call then starts again once the process continues, or fails with
//! `EIO` for a group that is orphaned (and for `SIGTTIN` ignored or blocked), as on Linux.

use super::process_table::{Stopping, TABLE};
use super::signals::{self, Info, Interrupted};
use super::thread::{self, LinuxThread};
use super::{Errno, credentials, signal, wait};
use crate::console::TerminalSignal;
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// What a call that would read the console or change it from a background group gets instead
/// of its result: its group was sent `SIGTTIN` or `SIGTTOU`, and it starts again once the
/// process continues ([`restart_after_signal`]). Never returned to a program.
pub const SIGNALLED: Errno = Errno(u16::MAX - 1);

/// Whether a process may read the console or change it ([`terminal_access`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// It may: the console is not its controlling terminal, or its group is the foreground.
    Allowed,
    /// It may not, and the call fails with `EIO`.
    Refused,
    /// Its group was sent the signal: the call starts again once the process continues.
    Signalled,
}

/// Whether `process`, of which `thread` is the calling thread, may read the console (`number`
/// `SIGTTIN`) or change its settings or foreground group (`SIGTTOU`), as Linux's
/// `tty_check_change` decides: see the module documentation.
pub fn terminal_access<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    number: u32,
) -> Access {
    let background = TABLE.lock().background_on_console(process.id().get());
    let Some((group, orphaned)) = background else {
        return Access::Allowed;
    };
    let blocked = thread.signal_state().blocked & (1u64 << (number - 1)) != 0;
    let ignored = process
        .signals()
        .lock()
        .action(number)
        .is_some_and(|action| action.handler == signals::IGNORE);
    if blocked || ignored {
        return match number == signal::SIGTTIN {
            true => Access::Refused,
            false => Access::Allowed,
        };
    }
    if orphaned {
        return Access::Refused;
    }
    signal_group::<P>(group, number);
    Access::Signalled
}

/// Ends a call that got [`SIGNALLED`] as Linux's `-ERESTARTSYS` does: the signal sent to the
/// caller's group is delivered, which stops the process until `SIGCONT`; if a handler runs,
/// the call starts again after it with `SA_RESTART` and fails with `EINTR` without; otherwise
/// it starts again at once, so it looks at the console anew.
pub fn restart_after_signal<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let number = context.syscall_number();
    let first_argument = context.syscall_arguments()[0];
    let interrupted = Interrupted::RestartsIfAsked {
        number,
        first_argument,
    };
    signals::take_in_sent_now(process, thread);
    if !signals::deliver_interrupting(process, thread, context, interrupted)? {
        context.restart_syscall(number, first_argument);
    }
    ControlFlow::Continue(())
}

/// Stops `process`, of which `thread` is the calling thread, for the stop signal `number`
/// whose default action its delivery found (D181): unless it is a terminal's stop signal
/// (`SIGTSTP`, `SIGTTIN`, `SIGTTOU`) and the process's group is orphaned, which discards it.
/// Its parent is told, its other threads stop too, and the thread sleeps until the process
/// continues or ends: `Break` once it has ended.
#[cold]
#[inline(never)]
pub fn stop<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    number: u32,
) -> ControlFlow<()> {
    let id = process.id().get();
    let uid = credentials::get_real(process, credentials::Kind::User) as u32;
    let stopping = {
        let mut table = TABLE.lock();
        if number != signal::SIGSTOP && table.is_in_orphaned_group(id) {
            return ControlFlow::Continue(());
        }
        table.stop(id, number as u8, uid)
    };
    match stopping {
        Stopping::Gone => return ControlFlow::Continue(()),
        Stopping::Already => {}
        Stopping::Stopped(notice) => {
            if let Some(notice) = notice {
                wait::notify_job_change::<P>(notice);
            }
            signals::stop_other_threads(process, thread);
        }
    }
    stay_stopped(process, thread)
}

/// Sleeps `thread`, the calling thread of `process`, while the process is stopped (D181),
/// taking in what is sent to it meanwhile: until `SIGCONT` continues it, `SIGKILL` is due to
/// the thread or the process ends (`Break`).
#[cold]
#[inline(never)]
pub fn stay_stopped<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
) -> ControlFlow<()> {
    let id = process.id().get();
    thread::sleep_while(process, thread, || TABLE.lock().is_stopped(id))
}

/// For `SIGCONT` sent to `process` (D181): continues it if it is stopped, telling its parent;
/// its stopped threads, which the signal's sending interrupts, find it continued and go on.
#[cold]
#[inline(never)]
pub fn continue_process<P: Platform>(process: &Process<P>) {
    let notice = TABLE.lock().resume(process.id().get());
    if let Some(notice) = notice {
        wait::notify_job_change::<P>(notice);
    }
}

fn signal_group<P: Platform>(group: u32, number: u32) {
    let members = TABLE.lock().members(group);
    for running in &members {
        wait::send_to_running::<P>(running, Info::from_kernel(number));
    }
}

/// Sends signal `number` from the kernel to every process of the console's foreground group,
/// as a change of the console's window size sends `SIGWINCH` (D181).
pub fn send_to_console_foreground<P: Platform>(number: u32) {
    let members = TABLE.lock().console_foreground_members();
    for running in &members {
        wait::send_to_running::<P>(running, Info::from_kernel(number));
    }
}

/// The terminal `ioctl` requests of job control, the generic numbers, which AArch64, RISC-V
/// and x86_64 share.
pub mod request {
    /// Make the terminal the caller's session's controlling terminal (an `int`: 1 to take it
    /// from another session, privileged).
    pub const TIOCSCTTY: u32 = 0x540e;
    /// Read the foreground process group (a `pid_t`).
    pub const TIOCGPGRP: u32 = 0x540f;
    /// Set the foreground process group (a `pid_t`).
    pub const TIOCSPGRP: u32 = 0x5410;
    /// Read the terminal's session (a `pid_t`).
    pub const TIOCGSID: u32 = 0x5429;
}

mod option {
    pub const PR_SET_CHILD_SUBREAPER: u32 = 36;
    pub const PR_GET_CHILD_SUBREAPER: u32 = 37;
}

/// For the first thread of a process the kernel starts, `thread`: records it running, and
/// gives its session the console as controlling terminal if the console is no session's.
pub fn start_session_leader<P: Platform>(process: &Arc<Process<P>>, thread: &LinuxThread) {
    wait::record_running(process, thread, |table, id| table.give_console_if_free(id));
}

fn target(caller: u32, pid: u64) -> Result<u32, Errno> {
    match pid as u32 as i32 {
        0 => Ok(caller),
        pid if pid < 0 => Err(Errno::ESRCH),
        pid => Ok(pid as u32),
    }
}

/// `setpgid(pid, pgid)`: see [`super::process_table::Table::set_group`].
pub fn setpgid<P: Platform>(process: &Process<P>, pid: u64, pgid: u64) -> Result<u64, Errno> {
    TABLE
        .lock()
        .set_group(process.id().get(), pid as u32 as i32, pgid as u32 as i32)
        .map(|()| 0)
}

/// `getpgid(pid)`: the process group of process `pid`, the caller for 0. `ESRCH` for no
/// such process.
pub fn getpgid<P: Platform>(process: &Process<P>, pid: u64) -> Result<u64, Errno> {
    let pid = target(process.id().get(), pid)?;
    let (group, _) = TABLE.lock().group_and_session(pid).ok_or(Errno::ESRCH)?;
    Ok(u64::from(group))
}

/// `getsid(pid)`: the session of process `pid`, the caller for 0. `ESRCH` for no such
/// process.
pub fn getsid<P: Platform>(process: &Process<P>, pid: u64) -> Result<u64, Errno> {
    let pid = target(process.id().get(), pid)?;
    let (_, session) = TABLE.lock().group_and_session(pid).ok_or(Errno::ESRCH)?;
    Ok(u64::from(session))
}

/// `setsid()`: see [`super::process_table::Table::new_session`].
pub fn setsid<P: Platform>(process: &Process<P>) -> Result<u64, Errno> {
    TABLE.lock().new_session(process.id().get()).map(u64::from)
}

/// `prctl(option, argument, ...)`: `PR_SET_CHILD_SUBREAPER` (any argument other than 0 sets
/// it) and `PR_GET_CHILD_SUBREAPER` (an `int` written at `argument`, `EFAULT` if it is not the
/// program's writable memory). `EINVAL` for every other option, as Linux answers one it does
/// not know.
pub fn prctl<P: Platform>(process: &Process<P>, option: u64, argument: u64) -> Result<u64, Errno> {
    let id = process.id().get();
    match option as u32 {
        option::PR_SET_CHILD_SUBREAPER => {
            TABLE.lock().set_child_subreaper(id, argument != 0);
            Ok(0)
        }
        option::PR_GET_CHILD_SUBREAPER => {
            let subreaper = i32::from(TABLE.lock().is_child_subreaper(id));
            process
                .write_user_memory(argument, &subreaper.to_ne_bytes())
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// The job-control `ioctl`s on the console ([`request`]), `None` for another request. As
/// Linux: `ENOTTY` unless the console is the caller's controlling terminal (but for
/// `TIOCSCTTY`); `EFAULT` if `argument` is not the program's memory; `TIOCSPGRP`'s and
/// `TIOCSCTTY`'s errors as [`super::process_table::Table::set_console_foreground`] and
/// [`super::process_table::Table::take_console`] say.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    request: u32,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    let caller = process.id().get();
    let write = |value: u32| {
        process
            .write_user_memory(argument, &value.to_ne_bytes())
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT)
    };
    Some(match request {
        request::TIOCGPGRP => {
            let terminal = TABLE.lock().controlling_terminal(caller);
            terminal.and_then(|(_, foreground)| write(foreground))
        }
        request::TIOCGSID => {
            let terminal = TABLE.lock().controlling_terminal(caller);
            terminal.and_then(|(session, _)| write(session))
        }
        request::TIOCSPGRP => {
            let mut bytes = [0; 4];
            let terminal = TABLE.lock().controlling_terminal(caller);
            terminal
                .and_then(|_| {
                    process
                        .read_user_memory(argument, &mut bytes)
                        .map_err(|_| Errno::EFAULT)
                })
                .and_then(|()| {
                    TABLE
                        .lock()
                        .set_console_foreground(caller, i32::from_ne_bytes(bytes))
                })
                .map(|()| 0)
        }
        request::TIOCSCTTY => {
            let privileged = credentials::get_effective(process, credentials::Kind::User) == 0;
            let steal = argument as u32 == 1 && privileged;
            TABLE.lock().take_console(caller, steal).map(|()| 0)
        }
        _ => return None,
    })
}

/// Sends the signal a character typed on the console raises to every process of the
/// console's foreground group, with Linux's `siginfo_t` for a signal from the kernel. Called
/// from the console's interrupt.
pub fn signal_console_foreground<P: Platform>(sent: TerminalSignal) {
    let number = match sent {
        TerminalSignal::Interrupt => signal::SIGINT,
        TerminalSignal::Quit => signal::SIGQUIT,
        TerminalSignal::Suspend => signal::SIGTSTP,
    };
    send_to_console_foreground::<P>(number);
}

/// Gives the console back to no session, for a test that needs it.
#[cfg(test)]
pub fn release_console_for_test(id: u32) {
    let mut table = TABLE.lock();
    if table.controlling_terminal(id).is_ok() {
        table.release_console();
    }
}
