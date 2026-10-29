//! New processes (D104): `clone` without `CLONE_THREAD`, and x86_64's `fork` and `vfork`.
//!
//! A child process gets a new id, the caller as its parent, a copy of the caller's credentials,
//! resource limits and signal actions, and one thread: the caller's registers returning 0, its
//! blocked signals, FP/SIMD registers and alternate stack (not with `CLONE_VM` alone), nothing
//! pending. Its memory is the caller's pages copy on write ([`ChildMemory::Copied`], D169):
//! shared memory stays shared, ranges marked `MADV_DONTFORK` are left out and ones marked
//! `MADV_WIPEONFORK` are empty; or with `CLONE_VM` the caller's own ([`ChildMemory::Shared`]);
//! its descriptor table and working directory are copies, or the caller's with `CLONE_FILES`
//! and `CLONE_FS`. With `CLONE_VFORK` the calling thread sleeps until the child's `execve`
//! passes its point of no return or the child ends ([`release_vfork_parent`]), or until the
//! caller's own process ends. The child is in the process table under its parent, in the
//! parent's group and session, until the parent reaps it ([`super::wait`], D105).

use super::Errno;
use super::futex::{self, Key};
use super::thread::{Arguments, LinuxThread, flag, running_fp};
use crate::process::{ChildMemory, NewChildError, Process};
use alloc::sync::Arc;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The flags a new process may have: its memory shared, its parent waiting, the file system
/// context and descriptor table shared, its thread pointer and the id words, and what changes
/// nothing without a tracer, an I/O scheduler or System V semaphores.
pub const PROCESS_OPTIONS: u64 = flag::VM
    | flag::VFORK
    | flag::FS
    | flag::FILES
    | flag::SYSVSEM
    | flag::SETTLS
    | flag::PARENT_SETTID
    | flag::CHILD_CLEARTID
    | flag::CHILD_SETTID
    | flag::DETACHED
    | flag::PTRACE
    | flag::UNTRACED
    | flag::IO;

const LAST_SIGNAL: u64 = 64;

const VFORK_WORD: u64 = 1 << 63;

const SIGCHLD: u64 = super::signal::SIGCHLD as u64;

fn vfork_key(futex_space: u32) -> Key {
    Key::new(futex_space, VFORK_WORD, false)
}

/// `fork()` (x86_64): `clone` with `SIGCHLD` as the exit signal and no flags.
pub fn fork<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &LinuxThread,
    context: &UserContext,
) -> Result<u64, Errno> {
    clone_process(process, thread, context, &Arguments::with_flags(0, SIGCHLD))
}

/// `vfork()` (x86_64): `clone` with `CLONE_VM | CLONE_VFORK` and `SIGCHLD` as the exit
/// signal.
pub fn vfork<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &LinuxThread,
    context: &UserContext,
) -> Result<u64, Errno> {
    let arguments = Arguments::with_flags(flag::VM | flag::VFORK, SIGCHLD);
    clone_process(process, thread, context, &arguments)
}

/// `clone` without `CLONE_THREAD`, its flags already past the checks every `clone` makes
/// ([`super::thread::clone`]): starts a child process (see the module documentation) whose
/// thread returns 0 from the call on `stack` (the caller's stack pointer if 0), with its thread
/// pointer `tls` with `CLONE_SETTLS`, and returns its id. With `CLONE_PARENT_SETTID` the id is
/// written to the 32-bit word at `parent_tid` in the caller's memory, with
/// `CLONE_CHILD_SETTID` to the one at `child_tid` in the child's, before the child runs,
/// silently not if they are not writable memory; with `CLONE_CHILD_CLEARTID` the child's
/// `exit`, `exit_group` or `execve` clears the word at `child_tid` and wakes a futex there
/// while its memory is shared.
///
/// Errors: `EINVAL` for an exit signal past 64; `ENOSYS` for `CLONE_SIGHAND` (sharing signal
/// actions between processes), `CLONE_PARENT` and unknown flags, and for a copy of memory
/// that has had `MAP_SHARED` anonymous memory with no access or executable, which the copy
/// would stop sharing; on x86_64,
/// `EPERM` for a thread pointer outside user space; `EAGAIN` when ids run out; `ENOMEM` without memory for
/// the child, its copy of the caller's memory or its thread, with the caller untouched.
pub fn clone_process<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &LinuxThread,
    context: &UserContext,
    arguments: &Arguments,
) -> Result<u64, Errno> {
    let flags = arguments.flags;
    let any = |set: u64| flags & set != 0;
    if arguments.exit_signal > LAST_SIGNAL {
        return Err(Errno::EINVAL);
    }
    if flags & !PROCESS_OPTIONS != 0 {
        return Err(Errno::ENOSYS);
    }
    let mut child_context = context.clone();
    if arguments.stack != 0 {
        child_context.set_stack(arguments.stack);
    }
    if any(flag::SETTLS) {
        child_context
            .set_thread_pointer(arguments.tls)
            .map_err(|_| Errno::EPERM)?;
    }
    child_context.set_syscall_result(0);
    let files = match any(flag::FILES) {
        true => thread.files().share(),
        false => thread.files().copy()?,
    };
    let working_directory = match any(flag::FS) {
        true => thread.working_directory().share(),
        false => thread.working_directory().copy(),
    };
    let memory = match any(flag::VM) {
        true => ChildMemory::Shared,
        false => ChildMemory::Copied,
    };
    let child = Arc::new(process.new_child(memory).map_err(|error| match error {
        NewChildError::OutOfIds => Errno::EAGAIN,
        NewChildError::OutOfMemory => Errno::ENOMEM,
        NewChildError::SharedAnonymousMemory => Errno::ENOSYS,
    })?);
    let id = child.id().get();
    let clear_child_tid = match any(flag::CHILD_CLEARTID) {
        true => arguments.child_tid,
        false => 0,
    };
    let keeps_alternate_stack = !any(flag::VM) || any(flag::VFORK);
    let mut first = thread.new_process_thread(
        id,
        clear_child_tid,
        files,
        working_directory,
        keeps_alternate_stack,
    );
    first.inherit_fp(running_fp::<P>());
    let exit_signal = arguments.exit_signal as u8;
    super::wait::record_running(&child, &first, |table, id| {
        table.set_exit_signal(id, exit_signal)
    });
    let id_bytes = id.to_ne_bytes();
    if any(flag::PARENT_SETTID) {
        let _ = process.write_user_memory(arguments.parent_tid, &id_bytes);
    }
    if any(flag::CHILD_SETTID) {
        let _ = child.write_user_memory(arguments.child_tid, &id_bytes);
    }
    if any(flag::VFORK) {
        child.set_vfork_parent(process.futex_space());
    }
    if any(flag::VM) {
        process.address_space().spread_over_cpus();
    }
    if super::spawn_thread(Arc::clone(&child), first, child_context).is_err() {
        return Err(Errno::ENOMEM);
    }
    if any(flag::VFORK) {
        wait_for_release(process, thread, &child);
    }
    Ok(u64::from(id))
}

fn wait_for_release<P: Platform>(process: &Process<P>, thread: &LinuxThread, child: &Process<P>) {
    let key = vfork_key(process.futex_space());
    let released = || child.has_ended() || process.has_ended();
    while !released() {
        let _ = futex::wait_interruptibly::<P>(key, u32::MAX, u64::MAX, thread.wait_slot(), || {
            match released() {
                true => Err(Errno::EINTR),
                false => Ok(()),
            }
        });
    }
}

/// Wakes the thread that started `process` with `CLONE_VFORK` and waits for it, if any: call it
/// once `process` has ended or passed its `execve`'s point of no return.
#[inline]
pub fn release_vfork_parent<P: Platform>(process: &Process<P>) {
    if let Some(futex_space) = process.take_vfork_parent() {
        futex::wake::<P>(vfork_key(futex_space), u32::MAX, u32::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory;
    use crate::paging::PageTables;
    use crate::process::ProcessExit;
    use crate::tests::TestPlatform as P;
    use crate::thread::{self, host::frames_for_test};
    use core::sync::atomic::{AtomicBool, Ordering};

    fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
        let mut frames = memory::FRAMES.lock();
        let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
        tables.mark_active();
        tables
    }

    #[derive(Debug, Clone, Copy)]
    enum Release {
        ChildExits,
        ChildExecs,
        ParentEnds,
    }

    #[test]
    fn a_vfork_parent_sleeps_until_its_child_execs_or_ends_or_its_own_process_ends() {
        let _frames = frames_for_test();
        let kernel = kernel_tables();
        thread::start::<P>().unwrap();
        static RELEASED: AtomicBool = AtomicBool::new(false);
        for release in [
            Release::ChildExits,
            Release::ChildExecs,
            Release::ParentEnds,
        ] {
            RELEASED.store(false, Ordering::SeqCst);
            // SAFETY: test tables, never active; the test platform's switch only records roots.
            let parent = Arc::new(unsafe { Process::<P>::new(&kernel) }.unwrap());
            let child = Arc::new(parent.new_child(ChildMemory::Shared).unwrap());
            child.set_vfork_parent(parent.futex_space());
            let waiter = thread::spawn::<P, _>({
                let (parent, child) = (Arc::clone(&parent), Arc::clone(&child));
                move || {
                    let thread = LinuxThread::first(&parent);
                    wait_for_release(&parent, &thread, &child);
                    RELEASED.store(true, Ordering::SeqCst);
                }
            })
            .unwrap();
            for _ in 0..4 {
                thread::yield_now::<P>();
            }
            assert!(!RELEASED.load(Ordering::SeqCst), "{release:?}");
            match release {
                Release::ChildExits => {
                    assert!(child.exit(ProcessExit::Exited(0)));
                    release_vfork_parent::<P>(&child);
                }
                Release::ChildExecs => {
                    assert!(child.end_for_exec());
                    release_vfork_parent::<P>(&child);
                }
                Release::ParentEnds => {
                    assert!(parent.exit(ProcessExit::Exited(0)));
                    futex::wake_space::<P>(parent.futex_space());
                }
            }
            waiter.join::<P>();
            assert!(RELEASED.load(Ordering::SeqCst), "{release:?}");
        }
    }
}
