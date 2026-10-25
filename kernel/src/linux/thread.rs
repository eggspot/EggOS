//! Linux threads (D91): each thread's own state, its id, the address its exit clears and wakes
//! (`set_tid_address`, `clone`'s `CLONE_CHILD_CLEARTID`) and its signal state, which it keeps
//! in its loop and its system calls reach through the [`LinuxThread`] they are given; and
//! [`clone`], which starts a thread of the caller's process; and [`sleep_until`], where a
//! Linux thread waits for time or for a signal until its process ends.

use super::Errno;
use super::futex;
use super::signals::{self, LockedSignals, Signals, ThreadSignals};
use crate::process::{self, Process};
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The futex key thread `id` of `process` sleeps on: a word in the kernel's half, which no
/// program's futex can name, one per thread.
fn sleep_key<P: Platform>(process: &Process<P>, id: u32) -> futex::Key {
    const FIRST_SLEEP_WORD: u64 = !3;
    futex::Key::new(
        process.id().get(),
        FIRST_SLEEP_WORD - 4 * u64::from(id),
        false,
    )
}

/// Sleeps `thread`, the calling thread of `process`, until the clock reaches `deadline`
/// ([`Platform::clock_ticks`], `u64::MAX` for none), until another thread sends it a signal,
/// which it has taken in when this returns ([`signals::take_in_sent`]), or until the process
/// ends: `Break` if it has ended, before or during the sleep, so the thread stops. Call it as
/// a blocking system call runs: with interrupts masked and no lock held.
pub fn sleep_until<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    deadline: u64,
) -> ControlFlow<()> {
    let key = sleep_key(process, thread.id);
    let mut woken = false;
    loop {
        let taken_in = match woken {
            true => signals::take_in_sent_now(process, thread),
            false => signals::take_in_sent(process, thread),
        };
        if taken_in || process.has_ended() {
            break;
        }
        let seen = thread.signals_seen;
        let slept = futex::wait::<P>(key, u32::MAX, deadline, || {
            if process.has_ended() || process.signals_sent() != seen {
                Err(Errno::EINTR)
            } else {
                Ok(())
            }
        });
        if slept == Err(Errno::ETIMEDOUT) || P::clock_ticks() >= deadline {
            break;
        }
        woken = slept.is_ok();
    }
    match process.has_ended() {
        true => ControlFlow::Break(()),
        false => ControlFlow::Continue(()),
    }
}

/// Sleeps as [`sleep_until`] does, but on until `deadline` whatever signals arrive, for calls
/// that signals do not interrupt yet.
pub fn sleep_through<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    deadline: u64,
) -> ControlFlow<()> {
    while P::clock_ticks() < deadline {
        sleep_until::<P>(process, thread, deadline)?;
    }
    ControlFlow::Continue(())
}

/// Wakes thread `id` of `process` from [`sleep_until`], if it sleeps there, to take in a
/// signal sent to it.
pub fn wake_sleep<P: Platform>(process: &Process<P>, id: u32) {
    futex::wake::<P>(sleep_key(process, id), u32::MAX, 1);
}

/// A Linux thread's own state. See the module documentation.
#[derive(Debug)]
pub struct LinuxThread {
    id: u32,
    clear_child_tid: u64,
    robust_list: u64,
    signals: ThreadSignals,
    signals_seen: u16,
}

impl LinuxThread {
    /// The first thread of `process`, which has the process's id, as on Linux.
    pub fn first<P: Platform>(process: &Process<P>) -> Self {
        Self {
            id: process.id().get(),
            clear_child_tid: 0,
            robust_list: 0,
            signals: ThreadSignals::new(),
            signals_seen: process.signals_sent(),
        }
    }

    /// A thread with id `id` that this thread starts (`clone`), clearing and waking
    /// `clear_child_tid` when it exits, unless that is 0, and with its signal state
    /// ([`ThreadSignals::for_new_thread`]).
    pub fn new_thread(&self, id: u32, clear_child_tid: u64) -> Self {
        Self {
            id,
            clear_child_tid,
            robust_list: 0,
            signals: self.signals.for_new_thread(),
            signals_seen: self.signals_seen,
        }
    }

    /// Its id (Linux's `tid`).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The address its exit clears and wakes as a futex, 0 for none.
    pub fn clear_child_tid(&self) -> u64 {
        self.clear_child_tid
    }

    /// Sets the address its exit clears and wakes as a futex (`set_tid_address`).
    pub fn set_clear_child_tid(&mut self, address: u64) {
        self.clear_child_tid = address;
    }

    /// The head of its robust futex list (`set_robust_list`), 0 for none: its exit releases
    /// the locks on the list that it holds ([`super::futex::release_robust_list`]).
    pub fn robust_list(&self) -> u64 {
        self.robust_list
    }

    /// Sets the head of its robust futex list (`set_robust_list`).
    pub fn set_robust_list(&mut self, head: u64) {
        self.robust_list = head;
    }

    /// Its signal state joined with its process's, `process`, whose lock the caller holds.
    pub fn signals<'a>(&'a mut self, process: &'a mut Signals) -> LockedSignals<'a> {
        process.with_thread(&mut self.signals, self.id)
    }

    /// The count of signals sent to the process's threads ([`Process::signals_sent`]) when it
    /// last took in those sent to it ([`signals::take_in_sent`]).
    pub fn signals_seen(&self) -> u16 {
        self.signals_seen
    }

    /// Records that it took in the signals sent to it up to `sent`.
    pub fn set_signals_seen(&mut self, sent: u16) {
        self.signals_seen = sent;
    }

    /// Its own signal state, as it is now.
    pub fn signal_state(&self) -> ThreadSignals {
        self.signals
    }
}

/// `clone`'s flags, the low 32 bits of its first argument but for the exit signal's byte.
pub mod flag {
    pub const VM: u64 = 0x100;
    pub const FS: u64 = 0x200;
    pub const FILES: u64 = 0x400;
    pub const SIGHAND: u64 = 0x800;
    pub const PIDFD: u64 = 0x1000;
    pub const PTRACE: u64 = 0x2000;
    pub const VFORK: u64 = 0x4000;
    pub const PARENT: u64 = 0x8000;
    pub const THREAD: u64 = 0x1_0000;
    pub const NEWNS: u64 = 0x2_0000;
    pub const NEWCGROUP: u64 = 0x200_0000;
    pub const NEWUTS: u64 = 0x400_0000;
    pub const NEWIPC: u64 = 0x800_0000;
    pub const SYSVSEM: u64 = 0x4_0000;
    pub const SETTLS: u64 = 0x8_0000;
    pub const PARENT_SETTID: u64 = 0x10_0000;
    pub const CHILD_CLEARTID: u64 = 0x20_0000;
    pub const DETACHED: u64 = 0x40_0000;
    pub const UNTRACED: u64 = 0x80_0000;
    pub const CHILD_SETTID: u64 = 0x100_0000;
    pub const NEWUSER: u64 = 0x1000_0000;
    pub const NEWPID: u64 = 0x2000_0000;
    pub const NEWNET: u64 = 0x4000_0000;
    pub const IO: u64 = 0x8000_0000;

    /// The namespaces a new thread or process may get, none of which exists yet.
    pub const NAMESPACES: u64 = NEWNS | NEWCGROUP | NEWUTS | NEWIPC | NEWUSER | NEWPID | NEWNET;

    /// The bits that are the exit signal, not flags.
    pub const EXIT_SIGNAL: u64 = 0xff;

    /// What a thread of the caller's process needs: its memory, its signal actions, its
    /// thread group.
    pub const NEW_THREAD: u64 = VM | SIGHAND | THREAD;

    /// What a new thread may also ask for: sharing what threads here always share (the file
    /// system context, the descriptors, System V semaphore undo), its thread pointer and the
    /// id words, and what changes nothing without a tracer or I/O scheduler (`CLONE_DETACHED`
    /// is ignored, as on Linux).
    pub const THREAD_OPTIONS: u64 = FS
        | FILES
        | SYSVSEM
        | SETTLS
        | PARENT_SETTID
        | CHILD_CLEARTID
        | CHILD_SETTID
        | DETACHED
        | PTRACE
        | UNTRACED
        | IO;
}

/// `clone`'s arguments, in the architecture's order: x86_64 has the child's id word before
/// the thread pointer, AArch64 and RISC-V after it (Linux's `CLONE_BACKWARDS` there).
struct Arguments {
    flags: u64,
    stack: u64,
    parent_tid: u64,
    child_tid: u64,
    tls: u64,
}

impl Arguments {
    fn of(context: &UserContext) -> Self {
        let [flags, stack, parent_tid, fourth, fifth, _] = context.syscall_arguments();
        let (child_tid, tls) = if cfg!(target_arch = "x86_64") {
            (fourth, fifth)
        } else {
            (fifth, fourth)
        };
        Self {
            flags: flags & u64::from(u32::MAX) & !flag::EXIT_SIGNAL,
            stack,
            parent_tid,
            child_tid,
            tls,
        }
    }
}

/// `clone(flags, stack, parent_tid, child_tid, tls)` (x86_64's order; AArch64 and RISC-V swap
/// the last two), with the caller's registers `context`: starts a thread of the caller's
/// process, which returns 0 from the call on `stack` (the caller's stack pointer if 0) with
/// every other register the caller's, and returns its id. With `CLONE_SETTLS` its thread
/// pointer is `tls`; with `CLONE_PARENT_SETTID` and `CLONE_CHILD_SETTID` its id is written to
/// the 32-bit words at `parent_tid` and `child_tid` before it runs, silently not if they are
/// not the program's writable memory, as on Linux; with `CLONE_CHILD_CLEARTID` its exit clears
/// the word at `child_tid` and wakes a futex there. It blocks the signals the caller blocks,
/// has none pending and no alternate stack, as on Linux.
///
/// As Linux, in this order: `EINVAL` for `CLONE_PIDFD` with `CLONE_PARENT_SETTID`, for
/// `CLONE_NEWNS` or `CLONE_NEWUSER` with `CLONE_FS`, for `CLONE_THREAD` without
/// `CLONE_SIGHAND`, for `CLONE_SIGHAND` without `CLONE_VM`, for `CLONE_THREAD` with
/// `CLONE_NEWUSER` or `CLONE_NEWPID`, and for `CLONE_PIDFD` with `CLONE_DETACHED` or
/// `CLONE_THREAD`; then `EINVAL` for namespaces and `CLONE_PIDFD`, which do not exist yet (as
/// on a Linux built without them); `ENOSYS` for a new process (no `CLONE_VM`, or `CLONE_VFORK`
/// or `CLONE_PARENT`: `fork`, `vfork`, `posix_spawn`), until processes can be created, which
/// callers report as unsupported rather than retry; on x86_64, `EPERM` for a thread pointer
/// outside user space; `EAGAIN` when ids run out; `ENOMEM` without memory for the thread. The
/// flags' upper 32 bits are ignored, as Linux's `clone` does, and so is the exit signal, which
/// a thread does not have.
pub fn clone<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &LinuxThread,
    context: &UserContext,
) -> Result<u64, Errno> {
    let arguments = Arguments::of(context);
    let flags = arguments.flags;
    let all = |set: u64| flags & set == set;
    let any = |set: u64| flags & set != 0;
    let invalid = all(flag::PIDFD | flag::PARENT_SETTID)
        || all(flag::NEWNS | flag::FS)
        || all(flag::NEWUSER | flag::FS)
        || (any(flag::THREAD) && !any(flag::SIGHAND))
        || (any(flag::SIGHAND) && !any(flag::VM))
        || (any(flag::THREAD) && any(flag::NEWUSER | flag::NEWPID))
        || (any(flag::PIDFD) && any(flag::DETACHED | flag::THREAD));
    if invalid || any(flag::NAMESPACES | flag::PIDFD) {
        return Err(Errno::EINVAL);
    }
    if !all(flag::NEW_THREAD) || flags & !(flag::NEW_THREAD | flag::THREAD_OPTIONS) != 0 {
        return Err(Errno::ENOSYS);
    }
    let mut child = context.clone();
    if arguments.stack != 0 {
        child.set_stack(arguments.stack);
    }
    if any(flag::SETTLS) {
        child
            .set_thread_pointer(arguments.tls)
            .map_err(|_| Errno::EPERM)?;
    }
    child.set_syscall_result(0);
    let id = process::allocate_thread_id().ok_or(Errno::EAGAIN)?;
    let clear_child_tid = if any(flag::CHILD_CLEARTID) {
        arguments.child_tid
    } else {
        0
    };
    process
        .signals()
        .lock()
        .add_thread(id)
        .map_err(|_| Errno::ENOMEM)?;
    let new = thread.new_thread(id, clear_child_tid);
    let id_bytes = id.to_ne_bytes();
    if any(flag::PARENT_SETTID) {
        let _ = process.write_user_memory(arguments.parent_tid, &id_bytes);
    }
    if any(flag::CHILD_SETTID) {
        let _ = process.write_user_memory(arguments.child_tid, &id_bytes);
    }
    process.address_space().spread_over_cpus();
    super::spawn_thread(Arc::clone(process), new, child).map_err(|_| {
        process.signals().lock().remove_thread(id);
        Errno::ENOMEM
    })?;
    Ok(u64::from(id))
}
