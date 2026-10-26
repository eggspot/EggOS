//! Minimal job control (D105): process groups and sessions (`setpgid`, `getpgid`, `getpgrp`,
//! `setsid`, `getsid`), the console as a session's controlling terminal and its foreground
//! process group (`TIOCSCTTY`, `TIOCGPGRP`, `TIOCSPGRP`, `TIOCGSID`), which the console's
//! signal characters reach, and `prctl`'s `PR_SET_CHILD_SUBREAPER`, which takes in a
//! descendant's orphans. All of it is kept in the process table ([`super::process_table`]).
//!
//! A process the kernel starts leads a new session and group, and the console becomes that
//! session's controlling terminal if it is no session's yet, as a session leader's `open` of a
//! terminal makes it (its descriptors 0 to 2 are the console already). A process in a
//! background group that reads the console is not stopped (`SIGTTIN`), nor one that changes
//! its settings (`SIGTTOU`): processes do not stop yet.

use super::process_table::TABLE;
use super::signals::Info;
use super::thread::LinuxThread;
use super::{Errno, credentials, signal, wait};
use crate::console::TerminalSignal;
use crate::process::Process;
use alloc::sync::Arc;
use eggos_hal::Platform;

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

/// `prctl`'s options answered.
mod option {
    pub const PR_SET_CHILD_SUBREAPER: u32 = 36;
    pub const PR_GET_CHILD_SUBREAPER: u32 = 37;
}

/// For the first thread of a process the kernel starts, `thread`: records it running, and
/// gives its session the console as controlling terminal if the console is no session's.
pub fn start_session_leader<P: Platform>(process: &Arc<Process<P>>, thread: &LinuxThread) {
    wait::record_running(process, thread);
    TABLE.lock().give_console_if_free(process.id().get());
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
            process
                .read_user_memory(argument, &mut bytes)
                .map_err(|_| Errno::EFAULT)
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
    };
    let members = TABLE.lock().console_foreground_members();
    for running in &members {
        wait::send_to_running::<P>(running, Info::from_kernel(number));
    }
}

/// Gives the console back to no session, for a test that needs it.
#[cfg(test)]
pub fn release_console_for_test(id: u32) {
    let mut table = TABLE.lock();
    if table.controlling_terminal(id).is_ok() {
        table.release_console();
    }
}
