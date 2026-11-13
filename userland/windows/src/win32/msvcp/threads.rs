//! msvcp140's threads (D230): `std::mutex`, `std::recursive_mutex` and the timed mutexes over
//! `_Mtx_*`, `std::condition_variable` over `_Cnd_*`, `std::thread`'s join, detach and
//! `hardware_concurrency` over `_Thrd_*`, `std::this_thread`'s sleeps, and the clocks'
//! `_Query_perf_counter`, `_Query_perf_frequency` and `_Xtime_get_ticks`; and the Universal C
//! Runtime's `_beginthreadex`, which `std::thread` starts its threads with. A mutex is the
//! program's own `_Mtx_internal_imp_t` (80 bytes, which MSVC's headers fill in place or
//! `_Mtx_init_in_situ` does): its type word, an SRW lock at the slot MSVC keeps its lock in,
//! the owning thread and the recursion count; a condition variable's word is its own
//! `CONDITION_VARIABLE`. Waits are kernel32's, on futexes.

use super::super::sync::{
    acquire_srw_lock_exclusive, release_srw_lock_exclusive, sleep_condition_variable_srw,
    try_acquire_srw_lock_exclusive, wait_for_single_object, wake_all_condition_variable,
    wake_condition_variable,
};
use super::super::thread::{create_thread, exit_thread, get_exit_code_thread};
use super::super::thread_environment::thread_id;
use super::super::{FALSE, SpinLock, file, heap, time};
use crate::cpp_errors::result;
use crate::library::Export;
use crate::linux::{self, CLOCK_MONOTONIC, CLOCK_REALTIME};
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

const MUTEX_SIZE: usize = 80;
const MUTEX_TYPE: usize = 0;
const MUTEX_CRITICAL_SECTION: usize = 8;
const MUTEX_LOCK: usize = 16;
const MUTEX_OWNER: usize = 72;
const MUTEX_COUNT: usize = 76;
const MUTEX_RECURSIVE: i32 = 0x100;
const CONDITION_SIZE: usize = 72;
const CONDITION_VARIABLE: usize = 8;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;
const POLL_MILLISECONDS: u64 = 1;
const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

#[repr(C)]
struct Time {
    seconds: i64,
    nanoseconds: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ThreadHandle {
    handle: usize,
    id: u32,
}

#[cfg(target_arch = "x86_64")]
type PassedThreadHandle = *const ThreadHandle;

#[cfg(target_arch = "aarch64")]
type PassedThreadHandle = ThreadHandle;

#[cfg(target_arch = "x86_64")]
fn received(thread: PassedThreadHandle) -> ThreadHandle {
    // SAFETY: the x64 calling convention passes a 16-byte `_Thrd_t` as the address of the
    // caller's copy, which stays in place for the call.
    unsafe { thread.read_unaligned() }
}

#[cfg(target_arch = "aarch64")]
fn received(thread: PassedThreadHandle) -> ThreadHandle {
    thread
}

fn word(mutex: usize, offset: usize) -> &'static AtomicI32 {
    // SAFETY: the program's mutex or condition variable, whose words at these offsets the
    // layer alone uses, through atomics.
    unsafe { AtomicI32::from_ptr((mutex + offset) as *mut i32) }
}

fn owner(mutex: usize) -> &'static AtomicU32 {
    // SAFETY: as in `word`.
    unsafe { AtomicU32::from_ptr((mutex + MUTEX_OWNER) as *mut u32) }
}

fn initialize_mutex(mutex: usize, kind: i32) {
    word(mutex, MUTEX_TYPE).store(kind, Ordering::Relaxed);
    word(mutex, MUTEX_LOCK).store(0, Ordering::Relaxed);
    word(mutex, MUTEX_LOCK + 4).store(0, Ordering::Relaxed);
    owner(mutex).store(0, Ordering::Relaxed);
    word(mutex, MUTEX_COUNT).store(0, Ordering::Release);
}

extern "efiapi" fn mutex_init_in_situ(mutex: usize, kind: i32) {
    initialize_mutex(mutex, kind);
}

extern "efiapi" fn mutex_destroy_in_situ(_mutex: usize) {}

unsafe extern "efiapi" fn mutex_init(target: *mut usize, kind: i32) -> i32 {
    let mutex = heap::allocate(MUTEX_SIZE) as usize;
    // SAFETY: `_Mtx_init`'s contract: a writable `_Mtx_t`.
    unsafe { target.write(mutex) };
    if mutex == 0 {
        return result::NO_MEMORY;
    }
    initialize_mutex(mutex, kind);
    result::SUCCESS
}

extern "efiapi" fn mutex_destroy(mutex: usize) {
    if mutex != 0 {
        // SAFETY: `_Mtx_init` allocated it from this heap.
        unsafe { heap::free(mutex as *mut u8) };
    }
}

fn relock(mutex: usize) -> Option<i32> {
    if owner(mutex).load(Ordering::Relaxed) != thread_id() {
        return None;
    }
    if word(mutex, MUTEX_TYPE).load(Ordering::Relaxed) & MUTEX_RECURSIVE == 0 {
        return Some(result::BUSY);
    }
    word(mutex, MUTEX_COUNT).fetch_add(1, Ordering::Relaxed);
    Some(result::SUCCESS)
}

fn take(mutex: usize) -> i32 {
    owner(mutex).store(thread_id(), Ordering::Relaxed);
    word(mutex, MUTEX_COUNT).store(1, Ordering::Relaxed);
    result::SUCCESS
}

extern "efiapi" fn mutex_lock(mutex: usize) -> i32 {
    if let Some(answer) = relock(mutex) {
        return answer;
    }
    acquire_srw_lock_exclusive(mutex + MUTEX_LOCK);
    take(mutex)
}

extern "efiapi" fn mutex_trylock(mutex: usize) -> i32 {
    if let Some(answer) = relock(mutex) {
        return answer;
    }
    match try_acquire_srw_lock_exclusive(mutex + MUTEX_LOCK) {
        0 => result::BUSY,
        _ => take(mutex),
    }
}

fn utc_nanoseconds() -> u64 {
    linux::clock_nanoseconds(CLOCK_REALTIME)
}

fn deadline_nanoseconds(time: *const Time) -> u64 {
    // SAFETY: the timed functions' contract: a readable `_timespec64`.
    let time = unsafe { &*time };
    let seconds = u64::try_from(time.seconds).unwrap_or(0);
    let nanoseconds = u64::try_from(time.nanoseconds).unwrap_or(0);
    seconds
        .saturating_mul(NANOSECONDS_PER_SECOND)
        .saturating_add(nanoseconds)
}

fn milliseconds_until(deadline: u64) -> u32 {
    let left = deadline.saturating_sub(utc_nanoseconds());
    u32::try_from(left.div_ceil(NANOSECONDS_PER_MILLISECOND)).unwrap_or(INFINITE - 1)
}

extern "efiapi" fn mutex_timedlock(mutex: usize, time: *const Time) -> i32 {
    let deadline = deadline_nanoseconds(time);
    loop {
        match mutex_trylock(mutex) {
            result::BUSY if utc_nanoseconds() < deadline => {
                linux::sleep_nanoseconds(POLL_MILLISECONDS * NANOSECONDS_PER_MILLISECOND)
            }
            result::BUSY => return result::TIMED_OUT,
            answer => return answer,
        }
    }
}

extern "efiapi" fn mutex_unlock(mutex: usize) -> i32 {
    if owner(mutex).load(Ordering::Relaxed) != thread_id() {
        return result::ERROR;
    }
    if word(mutex, MUTEX_COUNT).fetch_sub(1, Ordering::Relaxed) == 1 {
        owner(mutex).store(0, Ordering::Relaxed);
        release_srw_lock_exclusive(mutex + MUTEX_LOCK);
    }
    result::SUCCESS
}

extern "efiapi" fn mutex_current_owns(mutex: usize) -> i32 {
    i32::from(
        owner(mutex).load(Ordering::Relaxed) == thread_id()
            && word(mutex, MUTEX_COUNT).load(Ordering::Relaxed) > 0,
    )
}

extern "efiapi" fn mutex_critical_section(mutex: usize) -> usize {
    mutex + MUTEX_CRITICAL_SECTION
}

extern "efiapi" fn mutex_clear_owner(mutex: usize) {
    owner(mutex).store(0, Ordering::Relaxed);
    word(mutex, MUTEX_COUNT).fetch_sub(1, Ordering::Relaxed);
}

extern "efiapi" fn mutex_reset_owner(mutex: usize) {
    owner(mutex).store(thread_id(), Ordering::Relaxed);
    word(mutex, MUTEX_COUNT).fetch_add(1, Ordering::Relaxed);
}

extern "efiapi" fn condition_init_in_situ(condition: usize) {
    word(condition, CONDITION_VARIABLE).store(0, Ordering::Release);
}

extern "efiapi" fn condition_destroy_in_situ(_condition: usize) {}

unsafe extern "efiapi" fn condition_init(target: *mut usize) -> i32 {
    let condition = heap::allocate(CONDITION_SIZE) as usize;
    // SAFETY: `_Cnd_init`'s contract: a writable `_Cnd_t`.
    unsafe { target.write(condition) };
    match condition {
        0 => result::NO_MEMORY,
        _ => result::SUCCESS,
    }
}

extern "efiapi" fn condition_destroy(condition: usize) {
    if condition != 0 {
        // SAFETY: `_Cnd_init` allocated it from this heap.
        unsafe { heap::free(condition as *mut u8) };
    }
}

fn wait(condition: usize, mutex: usize, milliseconds: u32) -> i32 {
    let count = word(mutex, MUTEX_COUNT).swap(0, Ordering::Relaxed);
    owner(mutex).store(0, Ordering::Relaxed);
    let woken = sleep_condition_variable_srw(
        condition + CONDITION_VARIABLE,
        mutex + MUTEX_LOCK,
        milliseconds,
        0,
    );
    owner(mutex).store(thread_id(), Ordering::Relaxed);
    word(mutex, MUTEX_COUNT).store(count, Ordering::Relaxed);
    match woken {
        FALSE => result::TIMED_OUT,
        _ => result::SUCCESS,
    }
}

extern "efiapi" fn condition_wait(condition: usize, mutex: usize) -> i32 {
    wait(condition, mutex, INFINITE)
}

extern "efiapi" fn condition_timedwait(condition: usize, mutex: usize, time: *const Time) -> i32 {
    let milliseconds = milliseconds_until(deadline_nanoseconds(time));
    wait(condition, mutex, milliseconds)
}

extern "efiapi" fn condition_timedwait_for(condition: usize, mutex: usize, ms: u32) -> i32 {
    wait(condition, mutex, ms)
}

extern "efiapi" fn condition_signal(condition: usize) -> i32 {
    wake_condition_variable(condition + CONDITION_VARIABLE);
    result::SUCCESS
}

extern "efiapi" fn condition_broadcast(condition: usize) -> i32 {
    wake_all_condition_variable(condition + CONDITION_VARIABLE);
    result::SUCCESS
}

extern "efiapi" fn thread_join(thread: PassedThreadHandle, code: *mut i32) -> i32 {
    let thread = received(thread);
    if thread.id == thread_id() {
        return result::ERROR;
    }
    if wait_for_single_object(thread.handle, INFINITE) != WAIT_OBJECT_0 {
        return result::ERROR;
    }
    let mut exit_code = 0u32;
    // SAFETY: a thread handle and a writable word.
    if unsafe { get_exit_code_thread(thread.handle, &raw mut exit_code) } == FALSE {
        return result::ERROR;
    }
    if !code.is_null() {
        // SAFETY: `_Thrd_join`'s contract: null or a writable `int`.
        unsafe { code.write(exit_code as i32) };
    }
    file::close_handle(thread.handle);
    result::SUCCESS
}

extern "efiapi" fn thread_detach(thread: PassedThreadHandle) -> i32 {
    match file::close_handle(received(thread).handle) {
        FALSE => result::ERROR,
        _ => result::SUCCESS,
    }
}

extern "efiapi" fn thread_id_of_caller() -> u32 {
    thread_id()
}

extern "efiapi" fn hardware_concurrency() -> u32 {
    linux::cpu_count()
}

extern "efiapi" fn thread_sleep_for(milliseconds: u32) {
    linux::sleep_nanoseconds(u64::from(milliseconds) * NANOSECONDS_PER_MILLISECOND);
}

extern "efiapi" fn thread_sleep(time: *const Time) {
    let left = deadline_nanoseconds(time).saturating_sub(utc_nanoseconds());
    linux::sleep_nanoseconds(left);
}

extern "efiapi" fn thread_yield() {
    linux::yield_now();
}

extern "efiapi" fn query_performance_counter() -> i64 {
    (linux::clock_nanoseconds(CLOCK_MONOTONIC) / time::NANOSECONDS_PER_TICK) as i64
}

extern "efiapi" fn query_performance_frequency() -> i64 {
    time::PERFORMANCE_FREQUENCY
}

extern "efiapi" fn time_ticks() -> i64 {
    time::now() as i64
}

unsafe extern "efiapi" fn begin_thread_ex(
    security: usize,
    stack_size: u32,
    start: usize,
    argument: usize,
    flags: u32,
    thread_identifier: *mut u32,
) -> usize {
    // SAFETY: `_beginthreadex` takes `CreateThread`'s arguments, its start function returning
    // the thread's exit code as `CreateThread`'s does.
    unsafe {
        create_thread(
            security,
            stack_size as usize,
            start,
            argument,
            flags,
            thread_identifier,
        )
    }
}

extern "efiapi" fn end_thread_ex(code: u32) -> ! {
    exit_thread(code)
}

const MOST_AT_THREAD_EXIT: usize = 64;

#[derive(Clone, Copy)]
struct AtThreadExit {
    thread: u32,
    condition: usize,
    mutex: usize,
    ready: usize,
}

static AT_THREAD_EXIT: SpinLock<[Option<AtThreadExit>; MOST_AT_THREAD_EXIT]> =
    SpinLock::new([None; MOST_AT_THREAD_EXIT]);

extern "efiapi" fn register_at_thread_exit(condition: usize, mutex: usize, ready: usize) {
    let entry = AtThreadExit {
        thread: thread_id(),
        condition,
        mutex,
        ready,
    };
    AT_THREAD_EXIT.with(|entries| {
        if let Some(slot) = entries.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(entry);
        }
    });
}

extern "efiapi" fn unregister_at_thread_exit(mutex: usize) {
    AT_THREAD_EXIT.with(|entries| {
        for slot in entries.iter_mut() {
            if slot.is_some_and(|entry| entry.mutex == mutex) {
                *slot = None;
            }
        }
    });
}

extern "efiapi" fn broadcast_at_thread_exit() {
    let thread = thread_id();
    loop {
        let next = AT_THREAD_EXIT.with(|entries| {
            let slot = entries
                .iter_mut()
                .find(|slot| slot.is_some_and(|entry| entry.thread == thread))?;
            slot.take()
        });
        let Some(entry) = next else {
            return;
        };
        if entry.ready != 0 {
            // SAFETY: `_Cnd_register_at_thread_exit`'s contract: `ready` points to the
            // program's `int`, alive until the thread ends.
            unsafe { (entry.ready as *mut i32).write_volatile(1) };
        }
        mutex_unlock(entry.mutex);
        condition_broadcast(entry.condition);
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"_Mtx_init_in_situ" => mutex_init_in_situ,
    b"_Mtx_destroy_in_situ" => mutex_destroy_in_situ,
    b"_Mtx_init" => mutex_init,
    b"_Mtx_destroy" => mutex_destroy,
    b"_Mtx_lock" => mutex_lock,
    b"_Mtx_trylock" => mutex_trylock,
    b"_Mtx_timedlock" => mutex_timedlock,
    b"_Mtx_unlock" => mutex_unlock,
    b"_Mtx_current_owns" => mutex_current_owns,
    b"_Mtx_getconcrtcs" => mutex_critical_section,
    b"_Mtx_clear_owner" => mutex_clear_owner,
    b"_Mtx_reset_owner" => mutex_reset_owner,
    b"_Cnd_init_in_situ" => condition_init_in_situ,
    b"_Cnd_destroy_in_situ" => condition_destroy_in_situ,
    b"_Cnd_init" => condition_init,
    b"_Cnd_destroy" => condition_destroy,
    b"_Cnd_wait" => condition_wait,
    b"_Cnd_timedwait" => condition_timedwait,
    b"_Cnd_timedwait_for" => condition_timedwait_for,
    b"_Cnd_signal" => condition_signal,
    b"_Cnd_broadcast" => condition_broadcast,
    b"_Cnd_register_at_thread_exit" => register_at_thread_exit,
    b"_Cnd_unregister_at_thread_exit" => unregister_at_thread_exit,
    b"_Cnd_do_broadcast_at_thread_exit" => broadcast_at_thread_exit,
    b"_Thrd_join" => thread_join,
    b"_Thrd_detach" => thread_detach,
    b"_Thrd_id" => thread_id_of_caller,
    b"_Thrd_hardware_concurrency" => hardware_concurrency,
    b"_Thrd_sleep_for" => thread_sleep_for,
    b"_Thrd_sleep" => thread_sleep,
    b"_Thrd_yield" => thread_yield,
    b"_Query_perf_counter" => query_performance_counter,
    b"_Query_perf_frequency" => query_performance_frequency,
    b"_Xtime_get_ticks" => time_ticks,
];

/// The Universal C Runtime's thread functions, which `std::thread` uses.
pub(super) const UCRT_EXPORTS: &[Export] = exports![
    b"_beginthreadex" => begin_thread_ex,
    b"_endthreadex" => end_thread_ex,
];
