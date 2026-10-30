//! Kernel objects and waits: events, mutexes, semaphores, threads, other processes and access
//! tokens (D178) live in one table of handles (`OBJECT_HANDLES` plus four times the slot), each
//! a reference-counted heap block, and their states change only under the table's lock. A wait
//! checks (and for wait-all takes) every object under the lock and otherwise sleeps on one
//! process-wide futex word, the wake sequence, which every state change bumps, so a wake can
//! never be lost between the check and the sleep; a process's end bumps nothing, so a wait for
//! one looks again every 10 ms, as does a wait on the event of an overlapped read in progress
//! (`overlapped.rs`). A named event, mutex or semaphore (D189) is a handle to a slot of the
//! table shared between processes (`named.rs`), whose state changes under that table's lock,
//! taken after this one; a wait for one looks again every 10 ms too, since another process's
//! change wakes nobody here. Critical sections, SRW locks, condition variables, one-time
//! initialisation and `WaitOnAddress` are futexes on the caller's own memory and need no handle.

use super::named::{self, Reference};
use super::{Bool, FALSE, SpinLock, TRUE, ansi, error, fail, heap, thread_environment};
use crate::handle_flags;
use crate::library::Export;
use crate::linux;
use crate::named_objects::{self, Shared, State};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use eggos_runtime::futex::WakeCount;

pub const OBJECT_HANDLES: usize = 0x4000_0000;
const MAX_OBJECTS: usize = 4096;
const MAX_PENDING_THREADS: usize = 256;
pub const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED_0: u32 = 0x80;
const WAIT_TIMEOUT: u32 = 0x102;
const WAIT_FAILED: u32 = u32::MAX;
const MAXIMUM_WAIT_OBJECTS: usize = 64;
pub const STILL_ACTIVE: u32 = 259;

/// A thread the layer started: its stack and TEB, which go once the kernel has cleared
/// `id_word` (the thread no longer runs), and its end.
#[derive(Debug)]
pub struct Thread {
    pub id_word: AtomicU32,
    pub run: AtomicU32,
    pub id: AtomicU32,
    pub ended: bool,
    pub exit_code: u32,
    pub stack: usize,
    pub stack_length: usize,
    pub teb: usize,
    pub thread_local_storage: usize,
    pub start: usize,
    pub parameter: usize,
}

#[derive(Debug)]
pub enum Kind {
    Event {
        manual_reset: bool,
        signaled: bool,
    },
    Mutex {
        owner: u32,
        recursion: u32,
        abandoned: bool,
    },
    Semaphore {
        count: i32,
        maximum: i32,
    },
    Thread(Thread),
    Process(super::processes::ProcessObject),
    Token,
    Named(Reference),
    Section(super::file_mapping::Section),
}

#[derive(Debug)]
pub struct Object {
    references: usize,
    pub kind: Kind,
}

struct Table {
    objects: [*mut Object; MAX_OBJECTS],
    flags: [u8; MAX_OBJECTS],
    pending_threads: [*mut Object; MAX_PENDING_THREADS],
}

// SAFETY: the objects the table points to are reached only while its lock is held.
unsafe impl Send for Table {}

static TABLE: SpinLock<Table> = SpinLock::new(Table {
    objects: [core::ptr::null_mut(); MAX_OBJECTS],
    flags: [0; MAX_OBJECTS],
    pending_threads: [core::ptr::null_mut(); MAX_PENDING_THREADS],
});
static WAKE_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static SLEEPERS: AtomicU32 = AtomicU32::new(0);

/// Whether `handle` names a kernel object rather than a file.
pub fn is_object_handle(handle: usize) -> bool {
    (OBJECT_HANDLES..OBJECT_HANDLES + MAX_OBJECTS * 4).contains(&handle)
}

fn slot_of(handle: usize) -> Option<usize> {
    (is_object_handle(handle) && handle.is_multiple_of(4)).then(|| (handle - OBJECT_HANDLES) / 4)
}

fn object(table: &mut Table, handle: usize) -> Option<&mut Object> {
    let pointer = table.objects[slot_of(handle)?];
    // SAFETY: a non-null slot points to a live object, which the table's lock (held by the
    // caller, who lent `table`) guards.
    unsafe { pointer.as_mut() }
}

/// Makes a new object and a handle for it; 0 with the last error set if there is no memory
/// or no free handle. `extra_references` counts owners other than the handle (a running
/// thread).
pub fn insert(kind: Kind, extra_references: usize) -> (usize, *mut Object) {
    let pointer = heap::allocate(core::mem::size_of::<Object>()).cast::<Object>();
    if pointer.is_null() {
        return fail(error::NOT_ENOUGH_MEMORY, (0, pointer));
    }
    // SAFETY: a fresh, 16-byte aligned heap block for one `Object`.
    unsafe {
        pointer.write(Object {
            references: 1 + extra_references,
            kind,
        })
    };
    let slot = TABLE.with(|table| {
        let slot = table.objects.iter().position(|slot| slot.is_null())?;
        table.objects[slot] = pointer;
        table.flags[slot] = 0;
        Some(slot)
    });
    match slot {
        Some(slot) => (OBJECT_HANDLES + slot * 4, pointer),
        None => {
            // SAFETY: the object was never published; dropping it in place before its block
            // goes back to the heap.
            unsafe { pointer.drop_in_place() };
            // SAFETY: as above.
            unsafe { heap::free(pointer.cast()) };
            fail(error::NOT_ENOUGH_MEMORY, (0, core::ptr::null_mut()))
        }
    }
}

fn release_reference(pointer: *mut Object) {
    // SAFETY: the caller holds one of the object's references, under the table's lock or as
    // its last owner.
    let object = unsafe { &mut *pointer };
    object.references -= 1;
    if object.references == 0 {
        // SAFETY: the last reference is gone, so nothing reaches the object any more.
        unsafe { pointer.drop_in_place() };
        // SAFETY: as above.
        unsafe { heap::free(pointer.cast()) };
    }
}

/// Closes an object handle.
pub fn close(handle: usize) -> Bool {
    reap_threads();
    let closed = TABLE.with(|table| {
        let slot = slot_of(handle)?;
        if u32::from(table.flags[slot]) & handle_flags::PROTECT_FROM_CLOSE != 0 {
            return None;
        }
        let pointer = core::mem::replace(&mut table.objects[slot], core::ptr::null_mut());
        if pointer.is_null() {
            return None;
        }
        release_reference(pointer);
        Some(())
    });
    match closed {
        Some(()) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

/// Another handle for the object `handle` names.
pub fn duplicate(handle: usize) -> Option<usize> {
    let pointer = TABLE.with(|table| {
        let object = object(table, handle)?;
        object.references += 1;
        Some(core::ptr::from_mut(object))
    })?;
    let slot = TABLE.with(|table| {
        let slot = table.objects.iter().position(|slot| slot.is_null());
        match slot {
            Some(slot) => {
                table.objects[slot] = pointer;
                table.flags[slot] = 0;
            }
            None => release_reference(pointer),
        }
        slot
    });
    slot.map(|slot| OBJECT_HANDLES + slot * 4)
}

/// The handle flags (`handle_flags.rs`) of the object handle `handle`, which the layer keeps
/// for the handle alone: objects live in the layer's memory, so children never inherit them.
pub fn flags_of(handle: usize) -> Option<u32> {
    TABLE.with(|table| {
        let slot = slot_of(handle)?;
        (!table.objects[slot].is_null()).then(|| u32::from(table.flags[slot]))
    })
}

/// Sets the handle flags of the object handle `handle`; `false` if it names no object.
pub fn set_flags(handle: usize, flags: u32) -> bool {
    TABLE.with(|table| match slot_of(handle) {
        Some(slot) if !table.objects[slot].is_null() => {
            table.flags[slot] = (flags & 0xff) as u8;
            true
        }
        _ => false,
    })
}

/// Runs `work` on the object `handle` names, under the table's lock, and wakes every waiter
/// afterwards if `work` says the object's state changed.
pub fn change<R>(handle: usize, work: impl FnOnce(&mut Kind) -> (R, bool)) -> Option<R> {
    let (result, changed) = TABLE.with(|table| {
        let object = object(table, handle)?;
        let (result, changed) = work(&mut object.kind);
        if changed {
            WAKE_SEQUENCE.fetch_add(1, Ordering::Release);
        }
        Some((result, changed))
    })?;
    if changed && SLEEPERS.load(Ordering::Acquire) != 0 {
        linux::futex_wake(WAKE_SEQUENCE.as_ptr() as usize, WakeCount::All);
    }
    Some(result)
}

/// Ends the thread whose object is `pointer`: records its exit code, wakes its waiters and
/// queues its stack and TEB for release once the kernel says it stopped running.
pub fn thread_ended(pointer: *mut Object, exit_code: u32) {
    TABLE.with(|table| {
        // SAFETY: the running thread holds a reference to its own object.
        let id = match unsafe { &(*pointer).kind } {
            Kind::Thread(thread) => thread.id.load(Ordering::Acquire),
            _ => 0,
        };
        abandon_mutexes_of(table, id);
        // SAFETY: as above; no other reference to the object is alive.
        let object = unsafe { &mut *pointer };
        if let Kind::Thread(thread) = &mut object.kind {
            thread.ended = true;
            thread.exit_code = exit_code;
        }
        match table.pending_threads.iter_mut().find(|slot| slot.is_null()) {
            Some(slot) => *slot = pointer,
            None => object.references += usize::MAX / 2,
        }
        WAKE_SEQUENCE.fetch_add(1, Ordering::Release);
    });
    if SLEEPERS.load(Ordering::Acquire) != 0 {
        linux::futex_wake(WAKE_SEQUENCE.as_ptr() as usize, WakeCount::All);
    }
}

fn abandon_mutexes_of(table: &mut Table, thread: u32) {
    if thread == 0 {
        return;
    }
    for pointer in table.objects.iter().filter(|pointer| !pointer.is_null()) {
        // SAFETY: a non-null slot points to a live object, which the table's lock (held by
        // the caller, who lent `table`) guards.
        let kind = unsafe { &mut (**pointer).kind };
        if let Kind::Mutex {
            owner,
            recursion,
            abandoned,
        } = kind
            && *owner == thread
        {
            *owner = 0;
            *recursion = 0;
            *abandoned = true;
        }
    }
    let named_handles = || {
        table.objects.iter().filter_map(|pointer| {
            // SAFETY: as above.
            match unsafe { (*pointer).as_ref() } {
                Some(Object {
                    kind: Kind::Named(reference),
                    ..
                }) => Some(reference),
                _ => None,
            }
        })
    };
    if named_handles().next().is_some() {
        let process = named::process();
        named::locked(|shared| {
            for reference in named_handles() {
                reference
                    .slot(shared)
                    .abandon_if_owned_by(process, Some(thread));
            }
        });
    }
}

fn free_thread_memory(thread: &Thread) {
    // SAFETY: the callers know the thread runs no more (or never ran), so nothing uses its
    // stack or TEB.
    let _ = unsafe { linux::unmap(thread.stack, thread.stack_length) };
    if thread.teb != 0 {
        // SAFETY: as above.
        let _ = unsafe { linux::unmap(thread.teb, thread_environment::TEB_SIZE) };
    }
    // SAFETY: as above; the copy was made for this thread alone.
    unsafe { thread_environment::free_thread_local_storage(thread.thread_local_storage) };
}

/// Takes back a thread object whose thread never started: its handle, its memory and both
/// references (the handle's and the thread's).
pub fn abandon_thread(handle: usize) {
    TABLE.with(|table| {
        let Some(slot) = slot_of(handle) else {
            return;
        };
        let pointer = core::mem::replace(&mut table.objects[slot], core::ptr::null_mut());
        if pointer.is_null() {
            return;
        }
        // SAFETY: the object was live in the table.
        if let Kind::Thread(thread) = unsafe { &(*pointer).kind } {
            free_thread_memory(thread);
        }
        release_reference(pointer);
        release_reference(pointer);
    });
}

/// Gives back the stacks, TEBs and TLS copies of the threads that have stopped running.
pub fn reap_threads() {
    TABLE.with(|table| {
        for slot in table.pending_threads.iter_mut() {
            let pointer = *slot;
            if pointer.is_null() {
                continue;
            }
            // SAFETY: a queued thread's object still holds the thread's own reference.
            let Kind::Thread(thread) = (unsafe { &(*pointer).kind }) else {
                continue;
            };
            if thread.id_word.load(Ordering::Acquire) != 0 {
                continue;
            }
            free_thread_memory(thread);
            *slot = core::ptr::null_mut();
            release_reference(pointer);
        }
    });
}

fn is_signaled(kind: &Kind, caller: u32, shared: Option<&Shared>) -> bool {
    match kind {
        Kind::Event { signaled, .. } => *signaled,
        Kind::Mutex { owner, .. } => *owner == 0 || *owner == caller,
        Kind::Semaphore { count, .. } => *count > 0,
        Kind::Thread(thread) => thread.ended,
        Kind::Process(process) => process.ended(),
        Kind::Named(reference) => shared.is_some_and(|shared| {
            shared.slots[reference.index()].is_signaled(named::process(), caller)
        }),
        Kind::Token | Kind::Section(_) => false,
    }
}

fn acquire(kind: &mut Kind, caller: u32, shared: Option<&mut Shared>) -> bool {
    if let Kind::Named(reference) = kind {
        return shared
            .is_some_and(|shared| reference.slot(shared).acquire(named::process(), caller));
    }
    match kind {
        Kind::Event {
            manual_reset: false,
            signaled,
        } => *signaled = false,
        Kind::Mutex {
            owner,
            recursion,
            abandoned,
        } => {
            *owner = caller;
            *recursion += 1;
            return core::mem::take(abandoned);
        }
        Kind::Semaphore { count, .. } => *count -= 1,
        Kind::Event { .. }
        | Kind::Thread(_)
        | Kind::Process(_)
        | Kind::Token
        | Kind::Named(_)
        | Kind::Section(_) => {}
    }
    false
}

enum Check {
    Done(u32),
    Sleep { sequence: u32, polling: bool },
}

const PROCESS_POLL_NANOSECONDS: u64 = 10_000_000;

fn check(handles: &[usize], wait_all: bool, caller: u32) -> Result<Check, u32> {
    TABLE.with(|table| {
        for (index, handle) in handles.iter().enumerate() {
            object(table, *handle).ok_or(error::INVALID_HANDLE)?;
            if wait_all && handles[..index].contains(handle) {
                return Err(error::INVALID_PARAMETER);
            }
        }
        let mut any_named = false;
        for handle in handles {
            match object(table, *handle) {
                Some(Object {
                    kind: Kind::Process(process),
                    ..
                }) => process.refresh(),
                Some(Object {
                    kind: Kind::Named(_),
                    ..
                }) => any_named = true,
                _ => {}
            }
        }
        match any_named {
            false => Ok(check_objects(table, handles, wait_all, caller, None)),
            true => named::locked(|shared| {
                check_objects(table, handles, wait_all, caller, Some(shared))
            })
            .ok_or(error::INVALID_HANDLE),
        }
    })
}

fn check_objects(
    table: &mut Table,
    handles: &[usize],
    wait_all: bool,
    caller: u32,
    mut shared: Option<&mut Shared>,
) -> Check {
    if let Some(shared) = shared.as_deref_mut() {
        for handle in handles {
            if let Some(Object {
                kind: Kind::Named(reference),
                ..
            }) = object(table, *handle)
            {
                named::abandon_if_owner_ended(reference.slot(shared));
            }
        }
    }
    let seen = shared.as_deref();
    let mut signaled = handles.iter().enumerate().filter(|(_, handle)| {
        object(table, **handle).is_some_and(|object| is_signaled(&object.kind, caller, seen))
    });
    let ready = match wait_all {
        true => (signaled.count() == handles.len()).then_some(0),
        false => signaled.next().map(|(index, _)| index),
    };
    match ready {
        Some(first) => {
            let taken = if wait_all {
                handles
            } else {
                &handles[first..=first]
            };
            let mut abandoned = None;
            for (index, handle) in taken.iter().enumerate() {
                if let Some(object) = object(table, *handle)
                    && acquire(&mut object.kind, caller, shared.as_deref_mut())
                {
                    abandoned.get_or_insert(first + index);
                }
            }
            Check::Done(match abandoned {
                Some(index) => WAIT_ABANDONED_0 + index as u32,
                None => WAIT_OBJECT_0 + first as u32,
            })
        }
        None => {
            let polling = handles.iter().any(|handle| {
                matches!(
                    object(table, *handle),
                    Some(Object {
                        kind: Kind::Process(_) | Kind::Named(_),
                        ..
                    })
                )
            });
            SLEEPERS.fetch_add(1, Ordering::AcqRel);
            Check::Sleep {
                sequence: WAKE_SEQUENCE.load(Ordering::Acquire),
                polling,
            }
        }
    }
}

/// Nanoseconds left until `deadline` (a monotonic time), `None` for no deadline.
pub fn deadline_after(milliseconds: u32) -> Option<u64> {
    (milliseconds != INFINITE).then(|| {
        linux::clock_nanoseconds(linux::CLOCK_MONOTONIC)
            .saturating_add(u64::from(milliseconds) * 1_000_000)
    })
}

/// Nanoseconds left until `deadline`; `Some(0)` once it passed.
pub fn remaining(deadline: Option<u64>) -> Option<u64> {
    deadline
        .map(|deadline| deadline.saturating_sub(linux::clock_nanoseconds(linux::CLOCK_MONOTONIC)))
}

fn wait(handles: &[usize], wait_all: bool, milliseconds: u32, alertable: bool) -> u32 {
    if handles.is_empty() || handles.len() > MAXIMUM_WAIT_OBJECTS {
        return fail(error::INVALID_PARAMETER, WAIT_FAILED);
    }
    let caller = thread_environment::thread_id();
    let mut deadline = None;
    loop {
        let reading = super::overlapped::finish_ready_reads(handles);
        let (sequence, polling) = match check(handles, wait_all, caller) {
            Err(code) => return fail(code, WAIT_FAILED),
            Ok(Check::Done(result)) => return result,
            Ok(Check::Sleep { sequence, polling }) => (sequence, polling),
        };
        if alertable && super::overlapped::run_queued_calls() {
            SLEEPERS.fetch_sub(1, Ordering::AcqRel);
            return super::overlapped::WAIT_IO_COMPLETION;
        }
        let left = match milliseconds {
            0 => Some(0),
            INFINITE => None,
            _ => remaining(*deadline.get_or_insert_with(|| deadline_after(milliseconds))),
        };
        if left != Some(0) {
            let slice = match polling || reading {
                true => Some(left.map_or(PROCESS_POLL_NANOSECONDS, |left| {
                    left.min(PROCESS_POLL_NANOSECONDS)
                })),
                false => left,
            };
            let _ = linux::futex_wait(&WAKE_SEQUENCE, sequence, slice);
        }
        SLEEPERS.fetch_sub(1, Ordering::AcqRel);
        if left == Some(0) {
            return WAIT_TIMEOUT;
        }
    }
}

/// Wakes every wait so that it looks again: a call was queued for an alertable one.
pub fn wake_waiters() {
    TABLE.with(|_| WAKE_SEQUENCE.fetch_add(1, Ordering::Release));
    if SLEEPERS.load(Ordering::Acquire) != 0 {
        linux::futex_wake(WAKE_SEQUENCE.as_ptr() as usize, WakeCount::All);
    }
}

/// `SleepEx` with `bAlertable`: sleeps `milliseconds` unless calls are queued for the thread,
/// or get queued meanwhile, which it runs; `WAIT_IO_COMPLETION` after running them, else 0.
pub fn sleep_alertably(milliseconds: u32) -> u32 {
    let deadline = deadline_after(milliseconds);
    loop {
        let sequence = TABLE.with(|_| {
            SLEEPERS.fetch_add(1, Ordering::AcqRel);
            WAKE_SEQUENCE.load(Ordering::Acquire)
        });
        let ran = super::overlapped::run_queued_calls();
        let left = remaining(deadline);
        if !ran && left != Some(0) {
            let _ = linux::futex_wait(&WAKE_SEQUENCE, sequence, left);
        }
        SLEEPERS.fetch_sub(1, Ordering::AcqRel);
        if ran {
            return super::overlapped::WAIT_IO_COMPLETION;
        }
        if remaining(deadline) == Some(0) {
            return 0;
        }
    }
}

extern "efiapi" fn wait_for_single_object(handle: usize, milliseconds: u32) -> u32 {
    wait(&[handle], false, milliseconds, false)
}

extern "efiapi" fn wait_for_single_object_ex(
    handle: usize,
    milliseconds: u32,
    alertable: Bool,
) -> u32 {
    wait(&[handle], false, milliseconds, alertable != FALSE)
}

unsafe fn handle_list<'a>(count: u32, handles: *const usize, most: usize) -> Option<&'a [usize]> {
    match count {
        0 => Some(&[]),
        _ if handles.is_null() || count as usize > most => None,
        // SAFETY: the wait functions' contract: `handles` holds `count` handles.
        _ => Some(unsafe { core::slice::from_raw_parts(handles, count as usize) }),
    }
}

unsafe extern "efiapi" fn wait_for_multiple_objects(
    count: u32,
    handles: *const usize,
    wait_all: Bool,
    milliseconds: u32,
) -> u32 {
    // SAFETY: `WaitForMultipleObjects`' contract, which the caller keeps.
    unsafe { wait_for_multiple_objects_ex(count, handles, wait_all, milliseconds, FALSE) }
}

unsafe extern "efiapi" fn wait_for_multiple_objects_ex(
    count: u32,
    handles: *const usize,
    wait_all: Bool,
    milliseconds: u32,
    alertable: Bool,
) -> u32 {
    // SAFETY: `WaitForMultipleObjectsEx`' contract, which the caller keeps.
    match unsafe { handle_list(count, handles, MAXIMUM_WAIT_OBJECTS) } {
        Some(handles) => wait(handles, wait_all != FALSE, milliseconds, alertable != FALSE),
        None => fail(error::INVALID_PARAMETER, WAIT_FAILED),
    }
}

const MWMO_WAITALL: u32 = 1;
const MWMO_ALERTABLE: u32 = 2;

/// `MsgWaitForMultipleObjectsEx`: waits for the objects, a message in the thread's queue
/// (`WAIT_OBJECT_0 + count`) or, when alertable, a queued call, looking every 10 ms while
/// the queue is empty, as waits on processes do.
unsafe extern "efiapi" fn msg_wait_for_multiple_objects_ex(
    count: u32,
    handles: *const usize,
    milliseconds: u32,
    _wake_mask: u32,
    flags: u32,
) -> u32 {
    // SAFETY: `MsgWaitForMultipleObjectsEx`' contract, which the caller keeps.
    let Some(handles) = (unsafe { handle_list(count, handles, MAXIMUM_WAIT_OBJECTS - 1) }) else {
        return fail(error::INVALID_PARAMETER, WAIT_FAILED);
    };
    let deadline = deadline_after(milliseconds);
    loop {
        if !handles.is_empty() {
            match wait(handles, flags & MWMO_WAITALL != 0, 0, false) {
                WAIT_TIMEOUT => {}
                result => return result,
            }
        }
        if super::gui::has_message() {
            return WAIT_OBJECT_0 + count;
        }
        if flags & MWMO_ALERTABLE != 0 && super::overlapped::run_queued_calls() {
            return super::overlapped::WAIT_IO_COMPLETION;
        }
        let left = remaining(deadline);
        if left == Some(0) {
            return WAIT_TIMEOUT;
        }
        let sequence = TABLE.with(|_| {
            SLEEPERS.fetch_add(1, Ordering::AcqRel);
            WAKE_SEQUENCE.load(Ordering::Acquire)
        });
        let slice = left.unwrap_or(u64::MAX).min(PROCESS_POLL_NANOSECONDS);
        let _ = linux::futex_wait(&WAKE_SEQUENCE, sequence, Some(slice));
        SLEEPERS.fetch_sub(1, Ordering::AcqRel);
    }
}

unsafe extern "efiapi" fn msg_wait_for_multiple_objects(
    count: u32,
    handles: *const usize,
    wait_all: Bool,
    milliseconds: u32,
    wake_mask: u32,
) -> u32 {
    let flags = if wait_all != FALSE { MWMO_WAITALL } else { 0 };
    // SAFETY: `MsgWaitForMultipleObjects`' contract, which the caller keeps.
    unsafe { msg_wait_for_multiple_objects_ex(count, handles, milliseconds, wake_mask, flags) }
}

fn new_object(kind: Kind) -> usize {
    let (handle, _) = insert(kind, 0);
    if handle != 0 {
        thread_environment::set_last_error(error::SUCCESS);
    }
    handle
}

/// The UTF-16 name at `name`; `None` for a null or empty one, which names nothing.
///
/// # Safety
/// `name` is null or a readable NUL-terminated UTF-16 string.
pub unsafe fn object_name<'a>(name: *const u16) -> Option<&'a [u16]> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the caller's duty.
    let units = unsafe { super::file::wide_string(name) };
    (!units.is_empty()).then_some(units)
}

/// Runs `work` with the ANSI name at `name` widened (`None` for a null or empty one); 0 with
/// `ERROR_NOT_ENOUGH_MEMORY` without memory.
///
/// # Safety
/// `name` is null or a readable NUL-terminated string.
pub unsafe fn with_ansi_name(name: *const u8, work: impl FnOnce(Option<&[u16]>) -> usize) -> usize {
    // SAFETY: the caller's duty.
    match unsafe { ansi::widen_text(name) } {
        Ok(wide) => work(
            wide.as_ref()
                .map(ansi::Wide::units)
                .filter(|units| !units.is_empty()),
        ),
        Err(code) => fail(code, 0),
    }
}

/// A handle for a new object: `local` without a name; with one, for the named object of
/// `object_kind`, made with `state` unless the name exists already, which leaves
/// `ERROR_ALREADY_EXISTS` as the last error.
fn new_named_object(name: Option<&[u16]>, object_kind: u32, state: State, local: Kind) -> usize {
    let Some(name) = name else {
        return new_object(local);
    };
    let created =
        named_objects::parse(name).and_then(|name| named::create(&name, object_kind, state));
    match created {
        Ok(created) => {
            let existed = created.existed;
            let handle = new_object(Kind::Named(created.reference));
            if handle != 0 && existed {
                thread_environment::set_last_error(error::ALREADY_EXISTS);
            }
            handle
        }
        Err(code) => fail(code, 0),
    }
}

/// A handle for the existing named object `name` of `object_kind` (`OpenEventW` and the like).
fn open_named_object(name: Option<&[u16]>, object_kind: u32) -> usize {
    let Some(name) = name else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let opened = named_objects::parse(name).and_then(|name| named::open(&name, object_kind));
    match opened {
        Ok(opened) => new_object(Kind::Named(opened.reference)),
        Err(code) => fail(code, 0),
    }
}

fn create_event(name: Option<&[u16]>, manual_reset: bool, signaled: bool) -> usize {
    new_named_object(
        name,
        named_objects::kind::EVENT,
        State::event(manual_reset, signaled),
        Kind::Event {
            manual_reset,
            signaled,
        },
    )
}

unsafe extern "efiapi" fn create_event_w(
    _: usize,
    manual_reset: Bool,
    initial: Bool,
    name: *const u16,
) -> usize {
    // SAFETY: `CreateEventW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_event(name, manual_reset != FALSE, initial != FALSE)
}

unsafe extern "efiapi" fn create_event_a(
    _: usize,
    manual_reset: Bool,
    initial: Bool,
    name: *const u8,
) -> usize {
    let create = |name: Option<&[u16]>| create_event(name, manual_reset != FALSE, initial != FALSE);
    // SAFETY: `CreateEventA`'s contract: `name` is null or a NUL-terminated string.
    unsafe { with_ansi_name(name, create) }
}

const CREATE_EVENT_MANUAL_RESET: u32 = 1;
const CREATE_EVENT_INITIAL_SET: u32 = 2;
const CREATE_MUTEX_INITIAL_OWNER: u32 = 1;

unsafe extern "efiapi" fn create_event_ex_w(
    _: usize,
    name: *const u16,
    flags: u32,
    _access: u32,
) -> usize {
    // SAFETY: `CreateEventExW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_event(
        name,
        flags & CREATE_EVENT_MANUAL_RESET != 0,
        flags & CREATE_EVENT_INITIAL_SET != 0,
    )
}

unsafe extern "efiapi" fn open_event_w(_access: u32, _inherit: Bool, name: *const u16) -> usize {
    // SAFETY: `OpenEventW`'s contract: `name` is a NUL-terminated string.
    open_named_object(unsafe { object_name(name) }, named_objects::kind::EVENT)
}

unsafe extern "efiapi" fn open_event_a(_access: u32, _inherit: Bool, name: *const u8) -> usize {
    let open = |name: Option<&[u16]>| open_named_object(name, named_objects::kind::EVENT);
    // SAFETY: `OpenEventA`'s contract: `name` is a NUL-terminated string.
    unsafe { with_ansi_name(name, open) }
}

/// Sets or resets event `handle` without touching the last error; nothing when it is no event.
pub fn set_event_quietly(handle: usize, state: bool) {
    let _ = change(handle, |kind| match kind {
        Kind::Event { signaled, .. } => {
            *signaled = state;
            ((), state)
        }
        _ => ((), false),
    });
}

fn set_event_kind(kind: &mut Kind, state: bool) -> (Result<(), u32>, bool) {
    match kind {
        Kind::Event { signaled, .. } => {
            *signaled = state;
            (Ok(()), state)
        }
        Kind::Named(reference) => {
            let set = reference.with_slot(|slot| match slot.kind {
                named_objects::kind::EVENT => {
                    slot.state.signaled = u32::from(state);
                    Ok(())
                }
                _ => Err(error::INVALID_HANDLE),
            });
            let result = set.unwrap_or(Err(error::INVALID_HANDLE));
            let changed = result.is_ok() && state;
            (result, changed)
        }
        _ => (Err(error::INVALID_HANDLE), false),
    }
}

fn set_event_state(handle: usize, state: bool) -> Bool {
    match change(handle, |kind| set_event_kind(kind, state)) {
        Some(Ok(())) => TRUE,
        Some(Err(code)) => fail(code, FALSE),
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn set_event(handle: usize) -> Bool {
    set_event_state(handle, true)
}

extern "efiapi" fn reset_event(handle: usize) -> Bool {
    set_event_state(handle, false)
}

fn create_mutex(name: Option<&[u16]>, initial_owner: bool) -> usize {
    let thread = thread_environment::thread_id();
    let owner = if initial_owner { thread } else { 0 };
    let named_owner = initial_owner.then(|| (named::process(), thread));
    new_named_object(
        name,
        named_objects::kind::MUTEX,
        State::mutex(named_owner),
        Kind::Mutex {
            owner,
            recursion: u32::from(initial_owner),
            abandoned: false,
        },
    )
}

unsafe extern "efiapi" fn create_mutex_w(_: usize, initial_owner: Bool, name: *const u16) -> usize {
    // SAFETY: `CreateMutexW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_mutex(name, initial_owner != FALSE)
}

unsafe extern "efiapi" fn create_mutex_a(_: usize, initial_owner: Bool, name: *const u8) -> usize {
    let create = |name: Option<&[u16]>| create_mutex(name, initial_owner != FALSE);
    // SAFETY: `CreateMutexA`'s contract: `name` is null or a NUL-terminated string.
    unsafe { with_ansi_name(name, create) }
}

unsafe extern "efiapi" fn create_mutex_ex_w(
    _: usize,
    name: *const u16,
    flags: u32,
    _access: u32,
) -> usize {
    // SAFETY: `CreateMutexExW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_mutex(name, flags & CREATE_MUTEX_INITIAL_OWNER != 0)
}

unsafe extern "efiapi" fn open_mutex_w(_access: u32, _inherit: Bool, name: *const u16) -> usize {
    // SAFETY: `OpenMutexW`'s contract: `name` is a NUL-terminated string.
    open_named_object(unsafe { object_name(name) }, named_objects::kind::MUTEX)
}

unsafe extern "efiapi" fn open_mutex_a(_access: u32, _inherit: Bool, name: *const u8) -> usize {
    let open = |name: Option<&[u16]>| open_named_object(name, named_objects::kind::MUTEX);
    // SAFETY: `OpenMutexA`'s contract: `name` is a NUL-terminated string.
    unsafe { with_ansi_name(name, open) }
}

fn release_mutex_kind(kind: &mut Kind, caller: u32) -> (Result<(), u32>, bool) {
    match kind {
        Kind::Mutex {
            owner, recursion, ..
        } if *owner == caller && *recursion > 0 => {
            *recursion -= 1;
            if *recursion == 0 {
                *owner = 0;
            }
            (Ok(()), *recursion == 0)
        }
        Kind::Mutex { .. } => (Err(error::NOT_OWNER), false),
        Kind::Named(reference) => {
            let released = reference.with_slot(|slot| match slot.kind {
                named_objects::kind::MUTEX => slot.release_mutex(named::process(), caller),
                _ => Err(error::INVALID_HANDLE),
            });
            match released.unwrap_or(Err(error::INVALID_HANDLE)) {
                Ok(free) => (Ok(()), free),
                Err(code) => (Err(code), false),
            }
        }
        _ => (Err(error::INVALID_HANDLE), false),
    }
}

extern "efiapi" fn release_mutex(handle: usize) -> Bool {
    let caller = thread_environment::thread_id();
    match change(handle, |kind| release_mutex_kind(kind, caller)) {
        Some(Ok(())) => TRUE,
        Some(Err(code)) => fail(code, FALSE),
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn create_semaphore(name: Option<&[u16]>, initial: i32, maximum: i32) -> usize {
    if maximum <= 0 || initial < 0 || initial > maximum {
        return fail(error::INVALID_PARAMETER, 0);
    }
    new_named_object(
        name,
        named_objects::kind::SEMAPHORE,
        State::semaphore(initial, maximum),
        Kind::Semaphore {
            count: initial,
            maximum,
        },
    )
}

unsafe extern "efiapi" fn create_semaphore_w(
    _: usize,
    initial: i32,
    maximum: i32,
    name: *const u16,
) -> usize {
    // SAFETY: `CreateSemaphoreW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_semaphore(name, initial, maximum)
}

unsafe extern "efiapi" fn create_semaphore_a(
    _: usize,
    initial: i32,
    maximum: i32,
    name: *const u8,
) -> usize {
    let create = |name: Option<&[u16]>| create_semaphore(name, initial, maximum);
    // SAFETY: `CreateSemaphoreA`'s contract: `name` is null or a NUL-terminated string.
    unsafe { with_ansi_name(name, create) }
}

unsafe extern "efiapi" fn create_semaphore_ex_w(
    _: usize,
    initial: i32,
    maximum: i32,
    name: *const u16,
    _flags: u32,
    _access: u32,
) -> usize {
    // SAFETY: `CreateSemaphoreExW`'s contract: `name` is null or a NUL-terminated string.
    let name = unsafe { object_name(name) };
    create_semaphore(name, initial, maximum)
}

unsafe extern "efiapi" fn open_semaphore_w(
    _access: u32,
    _inherit: Bool,
    name: *const u16,
) -> usize {
    // SAFETY: `OpenSemaphoreW`'s contract: `name` is a NUL-terminated string.
    open_named_object(unsafe { object_name(name) }, named_objects::kind::SEMAPHORE)
}

unsafe extern "efiapi" fn open_semaphore_a(_access: u32, _inherit: Bool, name: *const u8) -> usize {
    let open = |name: Option<&[u16]>| open_named_object(name, named_objects::kind::SEMAPHORE);
    // SAFETY: `OpenSemaphoreA`'s contract: `name` is a NUL-terminated string.
    unsafe { with_ansi_name(name, open) }
}

fn release_semaphore_kind(kind: &mut Kind, release: i32) -> (Result<i32, u32>, bool) {
    match kind {
        Kind::Semaphore { count, maximum } => match count.checked_add(release) {
            Some(new) if new <= *maximum => (Ok(core::mem::replace(count, new)), true),
            _ => (Err(error::TOO_MANY_POSTS), false),
        },
        Kind::Named(reference) => {
            let released = reference.with_slot(|slot| match slot.kind {
                named_objects::kind::SEMAPHORE => slot.release_semaphore(release),
                _ => Err(error::INVALID_HANDLE),
            });
            let released = released.unwrap_or(Err(error::INVALID_HANDLE));
            let changed = released.is_ok();
            (released, changed)
        }
        _ => (Err(error::INVALID_HANDLE), false),
    }
}

unsafe extern "efiapi" fn release_semaphore(
    handle: usize,
    release: i32,
    previous: *mut i32,
) -> Bool {
    if release <= 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match change(handle, |kind| release_semaphore_kind(kind, release)) {
        Some(Ok(old)) => {
            if !previous.is_null() {
                // SAFETY: the caller passes a writable `LONG` for the previous count.
                unsafe { previous.write(old) };
            }
            TRUE
        }
        Some(Err(code)) => fail(code, FALSE),
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn signal_object(kind: &mut Kind, caller: u32) -> (Result<(), u32>, bool) {
    let target = match kind {
        Kind::Event { .. } => named_objects::kind::EVENT,
        Kind::Mutex { .. } => named_objects::kind::MUTEX,
        Kind::Semaphore { .. } => named_objects::kind::SEMAPHORE,
        Kind::Named(reference) => reference
            .with_slot(|slot| slot.kind)
            .unwrap_or(named_objects::kind::FREE),
        _ => named_objects::kind::FREE,
    };
    match target {
        named_objects::kind::EVENT => set_event_kind(kind, true),
        named_objects::kind::MUTEX => release_mutex_kind(kind, caller),
        named_objects::kind::SEMAPHORE => {
            let (result, changed) = release_semaphore_kind(kind, 1);
            (result.map(|_| ()), changed)
        }
        _ => (Err(error::INVALID_HANDLE), false),
    }
}

/// `SignalObjectAndWait`: sets the event, releases the mutex or adds one to the semaphore
/// `signal`, then waits for `wait_for` as `WaitForSingleObjectEx` does.
extern "efiapi" fn signal_object_and_wait(
    signal: usize,
    wait_for: usize,
    milliseconds: u32,
    alertable: Bool,
) -> u32 {
    let caller = thread_environment::thread_id();
    match change(signal, |kind| signal_object(kind, caller)) {
        Some(Ok(())) => wait(&[wait_for], false, milliseconds, alertable != FALSE),
        Some(Err(code)) => fail(code, WAIT_FAILED),
        None => fail(error::INVALID_HANDLE, WAIT_FAILED),
    }
}

/// The exit code of the thread `handle` names (`STILL_ACTIVE` while it runs), and its id.
pub fn thread_state(handle: usize) -> Option<(u32, u32)> {
    change(handle, |kind| match kind {
        Kind::Thread(thread) => {
            let code = if thread.ended {
                thread.exit_code
            } else {
                STILL_ACTIVE
            };
            (Some((code, thread.id.load(Ordering::Acquire))), false)
        }
        _ => (None, false),
    })
    .flatten()
}

fn word<'a>(address: usize) -> &'a AtomicU32 {
    // SAFETY: the callers pass the 4-byte aligned address of a 32-bit word in the program's
    // memory that the program gave for this use (a lock, a condition variable, an address to
    // wait on), which outlives the call.
    unsafe { AtomicU32::from_ptr(address as *mut u32) }
}

const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
const CONTENDED: u32 = 2;

fn lock(word: &AtomicU32, spins: u32) {
    for _ in 0..spins {
        if word
            .compare_exchange_weak(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
        core::hint::spin_loop();
    }
    if word
        .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        return;
    }
    while word.swap(CONTENDED, Ordering::Acquire) != UNLOCKED {
        let _ = linux::futex_wait(word, CONTENDED, None);
    }
}

fn unlock(word: &AtomicU32) {
    if word.swap(UNLOCKED, Ordering::Release) == CONTENDED {
        linux::futex_wake(word.as_ptr() as usize, WakeCount::One);
    }
}

mod critical_section {
    pub const LOCK_WORD: usize = 8;
    pub const RECURSION_COUNT: usize = 12;
    pub const OWNING_THREAD: usize = 16;
    pub const SPIN_COUNT: usize = 32;
    pub const SIZE: usize = 40;
}

const DEFAULT_SPINS: u32 = 64;

unsafe extern "efiapi" fn initialize_critical_section(section: *mut u8) {
    // SAFETY: `InitializeCriticalSection`'s contract: a writable `CRITICAL_SECTION`.
    unsafe { section.write_bytes(0, critical_section::SIZE) };
}

unsafe extern "efiapi" fn initialize_critical_section_and_spin_count(
    section: *mut u8,
    spins: u32,
) -> Bool {
    // SAFETY: the same contract as `InitializeCriticalSection`'s.
    unsafe { initialize_critical_section(section) };
    // SAFETY: as above; `SpinCount` is 8-byte aligned in the structure.
    unsafe {
        (section.wrapping_add(critical_section::SPIN_COUNT) as *mut u64).write(u64::from(spins))
    };
    TRUE
}

unsafe extern "efiapi" fn initialize_critical_section_ex(
    section: *mut u8,
    spins: u32,
    _flags: u32,
) -> Bool {
    // SAFETY: the same contract as `InitializeCriticalSectionAndSpinCount`'s.
    unsafe { initialize_critical_section_and_spin_count(section, spins) }
}

struct CriticalSection {
    lock: &'static AtomicU32,
    recursion: *mut u32,
    owner: &'static AtomicU64,
    spins: u32,
}

unsafe fn critical(section: *mut u8) -> CriticalSection {
    let at = section as usize;
    // SAFETY: the caller's duty; `SpinCount` is 8-byte aligned in the structure.
    let spins = unsafe { ((at + critical_section::SPIN_COUNT) as *const u64).read() };
    CriticalSection {
        lock: word(at + critical_section::LOCK_WORD),
        recursion: (at + critical_section::RECURSION_COUNT) as *mut u32,
        // SAFETY: the caller's duty; `OwningThread` is 8-byte aligned.
        owner: unsafe { AtomicU64::from_ptr((at + critical_section::OWNING_THREAD) as *mut u64) },
        spins: (spins as u32).clamp(DEFAULT_SPINS, 4000),
    }
}

pub(super) unsafe extern "efiapi" fn enter_critical_section(section: *mut u8) {
    // SAFETY: `EnterCriticalSection`'s contract: an initialized `CRITICAL_SECTION`.
    let section = unsafe { critical(section) };
    let caller = u64::from(thread_environment::thread_id());
    if section.owner.load(Ordering::Relaxed) == caller {
        // SAFETY: only the owner touches the recursion count.
        unsafe { *section.recursion += 1 };
        return;
    }
    lock(section.lock, section.spins);
    section.owner.store(caller, Ordering::Relaxed);
    // SAFETY: the caller owns the section now.
    unsafe { section.recursion.write(1) };
}

unsafe extern "efiapi" fn try_enter_critical_section(section: *mut u8) -> Bool {
    // SAFETY: `TryEnterCriticalSection`'s contract: an initialized `CRITICAL_SECTION`.
    let section = unsafe { critical(section) };
    let caller = u64::from(thread_environment::thread_id());
    if section.owner.load(Ordering::Relaxed) == caller {
        // SAFETY: only the owner touches the recursion count.
        unsafe { *section.recursion += 1 };
        return TRUE;
    }
    if section
        .lock
        .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return FALSE;
    }
    section.owner.store(caller, Ordering::Relaxed);
    // SAFETY: the caller owns the section now.
    unsafe { section.recursion.write(1) };
    TRUE
}

pub(super) unsafe extern "efiapi" fn leave_critical_section(section: *mut u8) {
    // SAFETY: `LeaveCriticalSection`'s contract: a `CRITICAL_SECTION` the caller entered.
    let section = unsafe { critical(section) };
    let caller = u64::from(thread_environment::thread_id());
    if section.owner.load(Ordering::Relaxed) != caller {
        return;
    }
    // SAFETY: only the owner touches the recursion count.
    let left = unsafe { section.recursion.read() }.saturating_sub(1);
    // SAFETY: as above.
    unsafe { section.recursion.write(left) };
    if left == 0 {
        section.owner.store(0, Ordering::Relaxed);
        unlock(section.lock);
    }
}

unsafe extern "efiapi" fn delete_critical_section(_section: *mut u8) {}

const SRW_WRITER: u32 = 0x4000_0000;
const SRW_WAITERS: u32 = 0x8000_0000;
const SRW_READERS: u32 = SRW_WRITER - 1;

fn srw_wait(lock: &AtomicU32, state: u32) {
    if lock
        .compare_exchange(
            state,
            state | SRW_WAITERS,
            Ordering::Relaxed,
            Ordering::Relaxed,
        )
        .is_ok()
    {
        let _ = linux::futex_wait(lock, state | SRW_WAITERS, None);
    }
}

fn srw_try_exclusive(lock: &AtomicU32) -> Option<u32> {
    let state = lock.load(Ordering::Relaxed);
    if state & !SRW_WAITERS != 0 {
        return Some(state);
    }
    lock.compare_exchange(
        state,
        state | SRW_WRITER,
        Ordering::Acquire,
        Ordering::Relaxed,
    )
    .err()
}

fn srw_try_shared(lock: &AtomicU32) -> Option<u32> {
    let state = lock.load(Ordering::Relaxed);
    if state & SRW_WRITER != 0 || state & SRW_READERS == SRW_READERS {
        return Some(state);
    }
    lock.compare_exchange(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
        .err()
}

fn srw_lock(address: usize) -> &'static AtomicU32 {
    word(address)
}

extern "efiapi" fn initialize_srw_lock(lock: usize) {
    srw_lock(lock).store(0, Ordering::Relaxed);
}

extern "efiapi" fn acquire_srw_lock_exclusive(lock: usize) {
    let lock = srw_lock(lock);
    while let Some(state) = srw_try_exclusive(lock) {
        if state & !SRW_WAITERS != 0 {
            srw_wait(lock, state);
        }
    }
}

extern "efiapi" fn acquire_srw_lock_shared(lock: usize) {
    let lock = srw_lock(lock);
    while let Some(state) = srw_try_shared(lock) {
        if state & SRW_WRITER != 0 {
            srw_wait(lock, state);
        }
    }
}

extern "efiapi" fn try_acquire_srw_lock_exclusive(lock: usize) -> u8 {
    u8::from(srw_try_exclusive(srw_lock(lock)).is_none())
}

extern "efiapi" fn try_acquire_srw_lock_shared(lock: usize) -> u8 {
    u8::from(srw_try_shared(srw_lock(lock)).is_none())
}

fn srw_wake(lock: &AtomicU32, previous: u32) {
    if previous & SRW_WAITERS != 0 {
        linux::futex_wake(lock.as_ptr() as usize, WakeCount::All);
    }
}

extern "efiapi" fn release_srw_lock_exclusive(lock: usize) {
    let lock = srw_lock(lock);
    let previous = lock.fetch_and(!(SRW_WRITER | SRW_WAITERS), Ordering::Release);
    srw_wake(lock, previous);
}

extern "efiapi" fn release_srw_lock_shared(lock: usize) {
    let lock = srw_lock(lock);
    let previous = lock.fetch_sub(1, Ordering::Release);
    if previous & SRW_READERS == 1 {
        let previous = lock.fetch_and(!SRW_WAITERS, Ordering::Relaxed);
        srw_wake(lock, previous);
    }
}

extern "efiapi" fn initialize_condition_variable(variable: usize) {
    word(variable).store(0, Ordering::Relaxed);
}

fn sleep_on_condition(variable: usize, milliseconds: u32, unlock: impl FnOnce()) -> Bool {
    let sequence = word(variable).load(Ordering::Acquire);
    unlock();
    let timeout = (milliseconds != INFINITE).then(|| u64::from(milliseconds) * 1_000_000);
    match linux::futex_wait(word(variable), sequence, timeout) {
        Err(errno) if errno.0 == 110 => fail(error::TIMEOUT, FALSE),
        _ => TRUE,
    }
}

unsafe extern "efiapi" fn sleep_condition_variable_cs(
    variable: usize,
    section: *mut u8,
    milliseconds: u32,
) -> Bool {
    // SAFETY: `SleepConditionVariableCS`' contract: a critical section the caller entered once.
    let result = sleep_on_condition(variable, milliseconds, || unsafe {
        leave_critical_section(section)
    });
    // SAFETY: as above; the caller holds it again on return, as Windows promises.
    unsafe { enter_critical_section(section) };
    result
}

const CONDITION_VARIABLE_LOCKMODE_SHARED: u32 = 1;

extern "efiapi" fn sleep_condition_variable_srw(
    variable: usize,
    lock: usize,
    milliseconds: u32,
    flags: u32,
) -> Bool {
    let shared = flags & CONDITION_VARIABLE_LOCKMODE_SHARED != 0;
    let result = sleep_on_condition(variable, milliseconds, || match shared {
        true => release_srw_lock_shared(lock),
        false => release_srw_lock_exclusive(lock),
    });
    match shared {
        true => acquire_srw_lock_shared(lock),
        false => acquire_srw_lock_exclusive(lock),
    }
    result
}

extern "efiapi" fn wake_condition_variable(variable: usize) {
    word(variable).fetch_add(1, Ordering::Release);
    linux::futex_wake(variable, WakeCount::One);
}

extern "efiapi" fn wake_all_condition_variable(variable: usize) {
    word(variable).fetch_add(1, Ordering::Release);
    linux::futex_wake(variable, WakeCount::All);
}

const ONCE_RUNNING: u64 = 1;
const ONCE_DONE: u64 = 2;
const ONCE_STATE: u64 = 3;
const INIT_ONCE_CHECK_ONLY: u32 = 1;
const INIT_ONCE_ASYNC: u32 = 2;
const INIT_ONCE_INIT_FAILED: u32 = 4;

fn once_word<'a>(once: usize) -> &'a AtomicU64 {
    // SAFETY: an `INIT_ONCE` is one pointer-sized, pointer-aligned value the program keeps
    // alive while it uses it.
    unsafe { AtomicU64::from_ptr(once as *mut u64) }
}

enum Begin {
    Done(u64),
    Pending,
}

fn begin_once(once: usize, check_only: bool) -> Option<Begin> {
    let value = once_word(once);
    loop {
        let state = value.load(Ordering::Acquire);
        match state & ONCE_STATE {
            ONCE_DONE => return Some(Begin::Done(state & !ONCE_STATE)),
            _ if check_only => return None,
            0 => {
                if value
                    .compare_exchange(0, ONCE_RUNNING, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return Some(Begin::Pending);
                }
            }
            _ => {
                let _ = linux::futex_wait(word(once), state as u32, None);
            }
        }
    }
}

fn complete_once(once: usize, context: u64, failed: bool) {
    let new = if failed {
        0
    } else {
        context & !ONCE_STATE | ONCE_DONE
    };
    once_word(once).store(new, Ordering::Release);
    linux::futex_wake(once, WakeCount::All);
}

extern "efiapi" fn init_once_initialize(once: usize) {
    once_word(once).store(0, Ordering::Relaxed);
}

unsafe extern "efiapi" fn init_once_execute_once(
    once: usize,
    function: usize,
    parameter: usize,
    context: *mut u64,
) -> Bool {
    match begin_once(once, false) {
        Some(Begin::Done(value)) => {
            if !context.is_null() {
                // SAFETY: the caller passes a writable `LPVOID` for the context.
                unsafe { context.write(value) };
            }
            TRUE
        }
        Some(Begin::Pending) => {
            let mut value = 0u64;
            // SAFETY: `InitOnceExecuteOnce`'s contract: `function` is a `PINIT_ONCE_FN`.
            let result = unsafe {
                super::call_program(function, [once, parameter, (&raw mut value) as usize, 0])
            };
            let succeeded = result as Bool != FALSE;
            complete_once(once, value, !succeeded);
            if succeeded && !context.is_null() {
                // SAFETY: as above.
                unsafe { context.write(value & !ONCE_STATE) };
            }
            if succeeded { TRUE } else { FALSE }
        }
        None => FALSE,
    }
}

unsafe extern "efiapi" fn init_once_begin_initialize(
    once: usize,
    flags: u32,
    pending: *mut Bool,
    context: *mut u64,
) -> Bool {
    if flags & INIT_ONCE_ASYNC != 0 || pending.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let (is_pending, value) = match begin_once(once, flags & INIT_ONCE_CHECK_ONLY != 0) {
        Some(Begin::Done(value)) => (FALSE, value),
        Some(Begin::Pending) => (TRUE, 0),
        None => return fail(error::INVALID_PARAMETER, FALSE),
    };
    // SAFETY: the caller passes a writable `BOOL`.
    unsafe { pending.write(is_pending) };
    if !context.is_null() && is_pending == FALSE {
        // SAFETY: the caller passes a writable `LPVOID`.
        unsafe { context.write(value) };
    }
    TRUE
}

extern "efiapi" fn init_once_complete(once: usize, flags: u32, context: u64) -> Bool {
    if once_word(once).load(Ordering::Acquire) & ONCE_STATE != ONCE_RUNNING {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    complete_once(once, context, flags & INIT_ONCE_INIT_FAILED != 0);
    TRUE
}

unsafe extern "efiapi" fn wait_on_address(
    address: usize,
    compare: usize,
    size: usize,
    milliseconds: u32,
) -> Bool {
    if !matches!(size, 1 | 2 | 4 | 8) || !address.is_multiple_of(size) {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let containing = word(address & !3);
    let guard = containing.load(Ordering::Acquire);
    let mut current = [0u8; 8];
    let mut expected = [0u8; 8];
    // SAFETY: `WaitOnAddress`' contract: `address` holds `size` readable bytes, which the
    // program changes only with atomic writes.
    unsafe { core::ptr::copy(address as *const u8, current.as_mut_ptr(), size) };
    // SAFETY: as above: `compare` holds `size` readable bytes.
    unsafe { core::ptr::copy(compare as *const u8, expected.as_mut_ptr(), size) };
    let differs = current != expected;
    if differs {
        return TRUE;
    }
    let requested = (milliseconds != INFINITE).then(|| u64::from(milliseconds) * 1_000_000);
    let rechecked = size == 8 && requested.is_none_or(|requested| requested > WIDE_VALUE_RECHECK);
    let timeout = if rechecked {
        Some(WIDE_VALUE_RECHECK)
    } else {
        requested
    };
    match linux::futex_wait(containing, guard, timeout) {
        Err(errno) if errno.0 == 110 && rechecked => TRUE,
        Err(errno) if errno.0 == 110 => fail(error::TIMEOUT, FALSE),
        _ => TRUE,
    }
}

const WIDE_VALUE_RECHECK: u64 = 1_000_000;

extern "efiapi" fn wake_by_address_single(address: usize) {
    linux::futex_wake(address & !3, WakeCount::One);
}

extern "efiapi" fn wake_by_address_all(address: usize) {
    linux::futex_wake(address & !3, WakeCount::All);
}

const DUPLICATE_CLOSE_SOURCE: u32 = 1;

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn duplicate_handle(
    source_process: usize,
    source: usize,
    target_process: usize,
    target: *mut usize,
    _access: u32,
    inherit: Bool,
    options: u32,
) -> Bool {
    let own = |process| super::processes::process_id_of(process) == Some(linux::process_id());
    if !own(source_process) || !own(target_process) {
        return fail(error::ACCESS_DENIED, FALSE);
    }
    if !target.is_null() {
        let new = match duplicated(source, inherit != FALSE) {
            Ok(new) => new,
            Err(code) => return fail(code, FALSE),
        };
        // SAFETY: the caller passes a writable `HANDLE` for the new handle.
        unsafe { target.write(new) };
    }
    if options & DUPLICATE_CLOSE_SOURCE != 0 {
        super::file::close_handle(source);
    }
    TRUE
}

/// Another handle for what `source` names (D189): the same object; for a file or pipe a new
/// descriptor, inherited by children when `inherit`; for the caller's own pseudo-handle a
/// real handle for its process.
fn duplicated(source: usize, inherit: bool) -> Result<usize, u32> {
    if source == super::CURRENT_PROCESS {
        return super::processes::own_process_handle().ok_or(error::NOT_ENOUGH_MEMORY);
    }
    if is_object_handle(source) {
        let new = duplicate(source).ok_or(error::INVALID_HANDLE)?;
        let flags = match inherit {
            true => handle_flags::INHERIT,
            false => 0,
        };
        set_flags(new, flags);
        return Ok(new);
    }
    let fd = super::file::fd_of(source).ok_or(error::INVALID_HANDLE)?;
    let command = match inherit {
        true => linux::F_DUPFD,
        false => linux::F_DUPFD_CLOEXEC,
    };
    linux::control(fd, command, 0)
        .map(super::file::handle_of)
        .map_err(super::error_for)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DuplicateHandle" => duplicate_handle,
    b"WaitForSingleObject" => wait_for_single_object,
    b"WaitForSingleObjectEx" => wait_for_single_object_ex,
    b"WaitForMultipleObjects" => wait_for_multiple_objects,
    b"WaitForMultipleObjectsEx" => wait_for_multiple_objects_ex,
    b"SignalObjectAndWait" => signal_object_and_wait,
    b"CreateEventW" => create_event_w,
    b"CreateEventA" => create_event_a,
    b"CreateEventExW" => create_event_ex_w,
    b"OpenEventW" => open_event_w,
    b"OpenEventA" => open_event_a,
    b"SetEvent" => set_event,
    b"ResetEvent" => reset_event,
    b"CreateMutexW" => create_mutex_w,
    b"CreateMutexA" => create_mutex_a,
    b"CreateMutexExW" => create_mutex_ex_w,
    b"OpenMutexW" => open_mutex_w,
    b"OpenMutexA" => open_mutex_a,
    b"ReleaseMutex" => release_mutex,
    b"CreateSemaphoreW" => create_semaphore_w,
    b"CreateSemaphoreA" => create_semaphore_a,
    b"CreateSemaphoreExW" => create_semaphore_ex_w,
    b"OpenSemaphoreW" => open_semaphore_w,
    b"OpenSemaphoreA" => open_semaphore_a,
    b"ReleaseSemaphore" => release_semaphore,
    b"InitializeCriticalSection" => initialize_critical_section,
    b"InitializeCriticalSectionAndSpinCount" => initialize_critical_section_and_spin_count,
    b"InitializeCriticalSectionEx" => initialize_critical_section_ex,
    b"EnterCriticalSection" => enter_critical_section,
    b"TryEnterCriticalSection" => try_enter_critical_section,
    b"LeaveCriticalSection" => leave_critical_section,
    b"DeleteCriticalSection" => delete_critical_section,
    b"InitializeSRWLock" => initialize_srw_lock,
    b"AcquireSRWLockExclusive" => acquire_srw_lock_exclusive,
    b"AcquireSRWLockShared" => acquire_srw_lock_shared,
    b"TryAcquireSRWLockExclusive" => try_acquire_srw_lock_exclusive,
    b"TryAcquireSRWLockShared" => try_acquire_srw_lock_shared,
    b"ReleaseSRWLockExclusive" => release_srw_lock_exclusive,
    b"ReleaseSRWLockShared" => release_srw_lock_shared,
    b"InitializeConditionVariable" => initialize_condition_variable,
    b"SleepConditionVariableCS" => sleep_condition_variable_cs,
    b"SleepConditionVariableSRW" => sleep_condition_variable_srw,
    b"WakeConditionVariable" => wake_condition_variable,
    b"WakeAllConditionVariable" => wake_all_condition_variable,
    b"InitOnceInitialize" => init_once_initialize,
    b"InitOnceExecuteOnce" => init_once_execute_once,
    b"InitOnceBeginInitialize" => init_once_begin_initialize,
    b"InitOnceComplete" => init_once_complete,
    b"WaitOnAddress" => wait_on_address,
    b"WakeByAddressSingle" => wake_by_address_single,
    b"WakeByAddressAll" => wake_by_address_all,
];

/// user32's waits that also wake for the thread's messages.
pub(super) const USER32_EXPORTS: &[Export] = exports![
    b"MsgWaitForMultipleObjects" => msg_wait_for_multiple_objects,
    b"MsgWaitForMultipleObjectsEx" => msg_wait_for_multiple_objects_ex,
];
