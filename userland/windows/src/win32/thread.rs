//! Threads: `CreateThread` maps a stack (with a guard page below it) and a TEB, and starts a
//! Linux thread of the process on them that points GS at its TEB and calls the start routine;
//! its return value, or `ExitThread`'s code, is the exit code its waiters see. The process ends
//! when its last thread does, with that thread's code, as on Windows.

use super::sync::{self, Kind, Object, Thread};
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::library::Export;
use crate::linux::{self, PROT_NONE, PROT_READ, PROT_WRITE};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const CREATE_SUSPENDED: u32 = 0x4;
const STACK_SIZE_PARAM_IS_A_RESERVATION: u32 = 0x1_0000;
const DEFAULT_STACK: usize = 1 << 20;
const STACK_GRANULARITY: usize = 64 << 10;
const MAX_STACK: usize = 256 << 20;
const GUARD: usize = linux::PAGE_SIZE;
const THREAD_PRIORITY_NORMAL: i32 = 0;
const RUN: u32 = 1;

static LIVE_THREADS: AtomicUsize = AtomicUsize::new(1);

fn stack_length(requested: usize) -> Option<usize> {
    let length = match requested {
        0 => DEFAULT_STACK,
        requested => requested.checked_next_multiple_of(STACK_GRANULARITY)?,
    };
    (length <= MAX_STACK).then_some(length)
}

unsafe extern "win64" fn create_thread(
    _security: usize,
    stack_size: usize,
    start: usize,
    parameter: usize,
    flags: u32,
    thread_id: *mut u32,
) -> usize {
    sync::reap_threads();
    if start == 0 || flags & !(CREATE_SUSPENDED | STACK_SIZE_PARAM_IS_A_RESERVATION) != 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let Some(length) = stack_length(stack_size) else {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let Ok(stack) = linux::map_anonymous(0, length + GUARD, PROT_READ | PROT_WRITE, false) else {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let unmap_stack = || {
        // SAFETY: the new stack is not used by any thread.
        let _ = unsafe { linux::unmap(stack, length + GUARD) };
    };
    // SAFETY: the guard page is the bottom of the new mapping, which nothing uses yet.
    let _ = unsafe { linux::protect(stack, GUARD, PROT_NONE) };
    let Some(slots) = thread_environment::new_thread_local_storage() else {
        unmap_stack();
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let run = if flags & CREATE_SUSPENDED != 0 {
        0
    } else {
        RUN
    };
    let (handle, object) = sync::insert(
        Kind::Thread(Thread {
            id_word: AtomicU32::new(0),
            run: AtomicU32::new(run),
            id: AtomicU32::new(0),
            ended: false,
            exit_code: 0,
            stack,
            stack_length: length + GUARD,
            teb: 0,
            thread_local_storage: slots,
            start,
            parameter,
        }),
        1,
    );
    if handle == 0 {
        // SAFETY: the copy was made for the thread that never started.
        unsafe { thread_environment::free_thread_local_storage(slots) };
        unmap_stack();
        return 0;
    }
    let top = stack + GUARD + length;
    let Ok(teb) = thread_environment::create_for_thread(top, stack + GUARD, slots, object as usize)
    else {
        abandon(handle);
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let id_word = {
        // SAFETY: the object lives while its handle and the thread's reference do, and no
        // other thread reaches it before the thread starts.
        let state = unsafe { thread_of(object) };
        state.teb = teb;
        state.id_word.as_ptr()
    };
    LIVE_THREADS.fetch_add(1, Ordering::AcqRel);
    // SAFETY: the stack's top is 16-byte aligned and only the new thread uses the stack until
    // the kernel clears `id_word`, which lives in the object the thread holds a reference to.
    let started = unsafe { linux::spawn_thread(top, id_word, thread_main, object as usize) };
    match started {
        Ok(id) => {
            sync::change(handle, |kind| {
                if let Kind::Thread(thread) = kind {
                    thread.id.store(id as u32, Ordering::Release);
                }
                ((), false)
            });
            if !thread_id.is_null() {
                // SAFETY: the caller passes a writable `DWORD` for the id.
                unsafe { thread_id.write(id as u32) };
            }
            thread_environment::set_last_error(error::SUCCESS);
            handle
        }
        Err(_) => {
            LIVE_THREADS.fetch_sub(1, Ordering::AcqRel);
            abandon(handle);
            fail(error::NOT_ENOUGH_MEMORY, 0)
        }
    }
}

unsafe fn thread_of<'a>(object: *mut Object) -> &'a mut Thread {
    // SAFETY: the caller's duty.
    match unsafe { &mut (*object).kind } {
        Kind::Thread(thread) => thread,
        _ => unreachable!(),
    }
}

fn abandon(handle: usize) {
    sync::abandon_thread(handle);
}

struct Start {
    run: *const AtomicU32,
    teb: usize,
    start: usize,
    parameter: usize,
}

extern "C" fn thread_main(object: usize) -> ! {
    let object = object as *const Object;
    // SAFETY: the thread holds a reference to its object; the fields read here were written
    // before it started and do not change.
    let start = unsafe {
        match &(*object).kind {
            Kind::Thread(thread) => Start {
                run: &raw const thread.run,
                teb: thread.teb,
                start: thread.start,
                parameter: thread.parameter,
            },
            _ => unreachable!(),
        }
    };
    // SAFETY: the run word lives in the object, which outlives this thread's use of it.
    let run = unsafe { &*start.run };
    while run.load(Ordering::Acquire) != RUN {
        let _ = linux::futex_wait(run, 0, None);
    }
    // SAFETY: the TEB `CreateThread` made for this thread.
    unsafe { thread_environment::enter(start.teb) };
    // SAFETY: `CreateThread`'s contract: `start` is a `LPTHREAD_START_ROUTINE`.
    let routine: extern "win64" fn(usize) -> u32 = unsafe { core::mem::transmute(start.start) };
    finish(routine(start.parameter))
}

/// Ends the calling thread with exit code `code`, as `ExitThread` does: the process ends,
/// with that code, when it was its last thread.
pub fn finish(code: u32) -> ! {
    thread_environment::leave();
    let object = thread_environment::thread_object() as *mut Object;
    if !object.is_null() {
        sync::thread_ended(object, code);
    }
    if LIVE_THREADS.fetch_sub(1, Ordering::AcqRel) == 1 {
        linux::exit(code);
    }
    linux::exit_thread()
}

extern "win64" fn exit_thread(code: u32) -> ! {
    finish(code)
}

unsafe extern "win64" fn get_exit_code_thread(handle: usize, code: *mut u32) -> Bool {
    let state = match handle {
        super::CURRENT_THREAD => Some((sync::STILL_ACTIVE, 0)),
        handle => sync::thread_state(handle),
    };
    let Some((exit_code, _)) = state else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    // SAFETY: the caller passes a writable `DWORD`.
    unsafe { code.write(exit_code) };
    TRUE
}

extern "win64" fn get_thread_id(handle: usize) -> u32 {
    if handle == super::CURRENT_THREAD {
        return thread_environment::thread_id();
    }
    match sync::thread_state(handle) {
        Some((_, id)) => id,
        None => fail(error::INVALID_HANDLE, 0),
    }
}

extern "win64" fn resume_thread(handle: usize) -> u32 {
    let run = sync::change(handle, |kind| match kind {
        Kind::Thread(thread) => (thread.run.as_ptr() as usize, false),
        _ => (0, false),
    });
    let Some(run) = run.filter(|run| *run != 0) else {
        return fail(error::INVALID_HANDLE, u32::MAX);
    };
    // SAFETY: the run word lives in the thread's object, which the caller's handle keeps alive.
    let word = unsafe { AtomicU32::from_ptr(run as *mut u32) };
    match word.swap(RUN, Ordering::AcqRel) {
        RUN => 0,
        _ => {
            linux::futex_wake(run, 1);
            1
        }
    }
}

extern "win64" fn get_current_thread_id() -> u32 {
    thread_environment::thread_id()
}

extern "win64" fn switch_to_thread() -> Bool {
    linux::yield_now();
    TRUE
}

extern "win64" fn set_thread_priority(_handle: usize, _priority: i32) -> Bool {
    TRUE
}

extern "win64" fn get_thread_priority(_handle: usize) -> i32 {
    THREAD_PRIORITY_NORMAL
}

extern "win64" fn set_thread_description(_handle: usize, _description: *const u16) -> i32 {
    0
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateThread" => create_thread,
    b"ExitThread" => exit_thread,
    b"GetExitCodeThread" => get_exit_code_thread,
    b"GetThreadId" => get_thread_id,
    b"ResumeThread" => resume_thread,
    b"GetCurrentThreadId" => get_current_thread_id,
    b"SwitchToThread" => switch_to_thread,
    b"SetThreadPriority" => set_thread_priority,
    b"GetThreadPriority" => get_thread_priority,
    b"SetThreadDescription" => set_thread_description,
];
