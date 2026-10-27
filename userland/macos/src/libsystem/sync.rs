//! Locks and waiting: the layer's own lock (a futex word: 0 free, 1 held, 2 held with
//! waiters), pthread mutexes (normal, error-checking and recursive, including Darwin's static
//! initializers), condition variables, `pthread_once` and libdispatch's counting semaphores,
//! all on Linux futexes in the program's own memory.

use super::thread;
use crate::darwin::EINVAL;
use crate::linux;
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const FREE: u32 = 0;
const HELD: u32 = 1;
const CONTENDED: u32 = 2;
const EPERM: c_int = 1;
const EDEADLK: c_int = 11;
const EBUSY: c_int = 16;
const ETIMEDOUT: c_int = 60;
const MUTEX_SIGNATURE: u64 = 0x4d55_5458;
const ERRORCHECK_MUTEX_INITIALIZER: u64 = 0x32aa_aba1;
const RECURSIVE_MUTEX_INITIALIZER: u64 = 0x32aa_aba2;
const MUTEX_ATTRIBUTES_SIGNATURE: u64 = 0x4d54_5841;
const CONDITION_SIGNATURE: u64 = 0x434f_4e44;
const NORMAL: u32 = 0;
const ERRORCHECK: u32 = 1;
const RECURSIVE: u32 = 2;
const ONCE_RUNNING: u32 = 1;
const ONCE_DONE: u32 = 2;
const TIME_NOW: u64 = 0;
const TIME_FOREVER: u64 = !0;
const TIMED_OUT: isize = 49;

/// Takes the lock in `word`.
pub fn lock(word: &AtomicU32) {
    let Err(mut state) = word.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed)
    else {
        return;
    };
    if state != CONTENDED {
        state = word.swap(CONTENDED, Ordering::Acquire);
    }
    while state != FREE {
        let _ = linux::futex_wait(word, CONTENDED, None);
        state = word.swap(CONTENDED, Ordering::Acquire);
    }
}

/// Takes the lock in `word` if it is free.
pub fn try_lock(word: &AtomicU32) -> bool {
    word.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

/// Releases the lock in `word`, waking one waiter if there is one.
pub fn unlock(word: &AtomicU32) {
    if word.swap(FREE, Ordering::Release) == CONTENDED {
        linux::futex_wake(word, 1);
    }
}

/// Darwin's 64-byte `pthread_mutex_t`.
#[repr(C)]
pub struct Mutex {
    signature: AtomicU64,
    state: AtomicU32,
    kind: AtomicU32,
    owner: AtomicU32,
    depth: AtomicU32,
    unused: [u64; 5],
}

/// Darwin's 16-byte `pthread_mutexattr_t`.
#[repr(C)]
pub struct MutexAttributes {
    signature: u64,
    kind: u32,
    unused: u32,
}

/// Darwin's 48-byte `pthread_cond_t`.
#[repr(C)]
pub struct Condition {
    signature: AtomicU64,
    sequence: AtomicU32,
    unused: [u32; 9],
}

/// Darwin's 16-byte `pthread_once_t`.
#[repr(C)]
pub struct Once {
    signature: u64,
    state: AtomicU32,
    unused: u32,
}

impl Mutex {
    fn kind(&self) -> u32 {
        match self.signature.load(Ordering::Relaxed) {
            MUTEX_SIGNATURE => self.kind.load(Ordering::Relaxed),
            RECURSIVE_MUTEX_INITIALIZER => RECURSIVE,
            ERRORCHECK_MUTEX_INITIALIZER => ERRORCHECK,
            _ => NORMAL,
        }
    }

    fn acquire(&self, me: u32) {
        lock(&self.state);
        self.owner.store(me, Ordering::Relaxed);
    }

    fn release(&self) {
        self.owner.store(0, Ordering::Relaxed);
        unlock(&self.state);
    }
}

/// # Safety
/// `attributes` must be a writable `pthread_mutexattr_t`.
pub unsafe extern "C" fn pthread_mutexattr_init(attributes: *mut MutexAttributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe {
        attributes.write(MutexAttributes {
            signature: MUTEX_ATTRIBUTES_SIGNATURE,
            kind: NORMAL,
            unused: 0,
        })
    };
    0
}

/// # Safety
/// As [`pthread_mutexattr_init`].
pub unsafe extern "C" fn pthread_mutexattr_destroy(attributes: *mut MutexAttributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { (*attributes).signature = 0 };
    0
}

/// # Safety
/// As [`pthread_mutexattr_init`].
pub unsafe extern "C" fn pthread_mutexattr_settype(
    attributes: *mut MutexAttributes,
    kind: c_int,
) -> c_int {
    if !(0..=RECURSIVE as c_int).contains(&kind) {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe { (*attributes).kind = kind as u32 };
    0
}

/// # Safety
/// `mutex` must be a writable `pthread_mutex_t` and `attributes` null or initialized.
pub unsafe extern "C" fn pthread_mutex_init(
    mutex: *mut Mutex,
    attributes: *const MutexAttributes,
) -> c_int {
    let kind = if attributes.is_null() {
        NORMAL
    } else {
        // SAFETY: the caller's promise.
        unsafe { (*attributes).kind }
    };
    // SAFETY: the caller's promise; no thread uses a mutex being initialized.
    unsafe {
        mutex.write(Mutex {
            signature: AtomicU64::new(MUTEX_SIGNATURE),
            state: AtomicU32::new(FREE),
            kind: AtomicU32::new(kind),
            owner: AtomicU32::new(0),
            depth: AtomicU32::new(0),
            unused: [0; 5],
        })
    };
    0
}

/// # Safety
/// `mutex` must be a `pthread_mutex_t` no thread uses.
pub unsafe extern "C" fn pthread_mutex_destroy(mutex: *mut Mutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &*mutex };
    if mutex.state.load(Ordering::Relaxed) != FREE {
        return EBUSY;
    }
    mutex.signature.store(0, Ordering::Relaxed);
    0
}

/// # Safety
/// `mutex` must be an initialized `pthread_mutex_t`.
pub unsafe extern "C" fn pthread_mutex_lock(mutex: *mut Mutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &*mutex };
    let me = thread::current_id();
    let kind = mutex.kind();
    if kind != NORMAL && mutex.owner.load(Ordering::Relaxed) == me {
        if kind == ERRORCHECK {
            return EDEADLK;
        }
        mutex.depth.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    mutex.acquire(me);
    mutex.depth.store(1, Ordering::Relaxed);
    0
}

/// # Safety
/// As [`pthread_mutex_lock`].
pub unsafe extern "C" fn pthread_mutex_trylock(mutex: *mut Mutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &*mutex };
    let me = thread::current_id();
    if mutex.kind() == RECURSIVE && mutex.owner.load(Ordering::Relaxed) == me {
        mutex.depth.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if !try_lock(&mutex.state) {
        return EBUSY;
    }
    mutex.owner.store(me, Ordering::Relaxed);
    mutex.depth.store(1, Ordering::Relaxed);
    0
}

/// # Safety
/// As [`pthread_mutex_lock`].
pub unsafe extern "C" fn pthread_mutex_unlock(mutex: *mut Mutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &*mutex };
    let kind = mutex.kind();
    if kind != NORMAL && mutex.owner.load(Ordering::Relaxed) != thread::current_id() {
        return EPERM;
    }
    if kind == RECURSIVE && mutex.depth.fetch_sub(1, Ordering::Relaxed) > 1 {
        return 0;
    }
    mutex.release();
    0
}

/// # Safety
/// `condition` must be a writable `pthread_cond_t`.
pub unsafe extern "C" fn pthread_cond_init(condition: *mut Condition, _: *const c_void) -> c_int {
    // SAFETY: the caller's promise; no thread uses a condition being initialized.
    unsafe {
        condition.write(Condition {
            signature: AtomicU64::new(CONDITION_SIGNATURE),
            sequence: AtomicU32::new(0),
            unused: [0; 9],
        })
    };
    0
}

/// # Safety
/// As [`pthread_cond_init`].
pub unsafe extern "C" fn pthread_cond_destroy(condition: *mut Condition) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { &*condition }.signature.store(0, Ordering::Relaxed);
    0
}

fn wait(condition: &Condition, mutex: &Mutex, timeout: Option<u64>) -> c_int {
    let sequence = condition.sequence.load(Ordering::Acquire);
    let me = thread::current_id();
    mutex.release();
    let result = linux::futex_wait(&condition.sequence, sequence, timeout);
    mutex.acquire(me);
    match result {
        Err(errno) if errno == linux::ETIMEDOUT => ETIMEDOUT,
        _ => 0,
    }
}

/// # Safety
/// `condition` and `mutex` must be initialized, and the caller must hold `mutex`.
pub unsafe extern "C" fn pthread_cond_wait(condition: *mut Condition, mutex: *mut Mutex) -> c_int {
    // SAFETY: the caller's promise.
    let condition = unsafe { &*condition };
    // SAFETY: as above.
    wait(condition, unsafe { &*mutex }, None)
}

/// # Safety
/// As [`pthread_cond_wait`]; `deadline` must be a readable `struct timespec`.
pub unsafe extern "C" fn pthread_cond_timedwait(
    condition: *mut Condition,
    mutex: *mut Mutex,
    deadline: *const [i64; 2],
) -> c_int {
    // SAFETY: the caller's promise.
    let [seconds, nanoseconds] = unsafe { deadline.read() };
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return EINVAL;
    }
    let [now_seconds, now_nanoseconds] = linux::clock_time(0).unwrap_or([0, 0]);
    let total = |seconds: i64, nanoseconds: i64| {
        (seconds as i128) * 1_000_000_000 + nanoseconds as i128
    };
    let remaining = total(seconds, nanoseconds) - total(now_seconds, now_nanoseconds);
    if remaining <= 0 {
        return ETIMEDOUT;
    }
    // SAFETY: the caller's promise.
    let condition = unsafe { &*condition };
    // SAFETY: as above.
    let mutex = unsafe { &*mutex };
    wait(condition, mutex, Some(remaining.min(u64::MAX as i128) as u64))
}

/// # Safety
/// `condition` must be initialized.
pub unsafe extern "C" fn pthread_cond_signal(condition: *mut Condition) -> c_int {
    // SAFETY: the caller's promise.
    let condition = unsafe { &*condition };
    condition.sequence.fetch_add(1, Ordering::Release);
    linux::futex_wake(&condition.sequence, 1);
    0
}

/// # Safety
/// As [`pthread_cond_signal`].
pub unsafe extern "C" fn pthread_cond_broadcast(condition: *mut Condition) -> c_int {
    // SAFETY: the caller's promise.
    let condition = unsafe { &*condition };
    condition.sequence.fetch_add(1, Ordering::Release);
    linux::futex_wake(&condition.sequence, i32::MAX as u32);
    0
}

/// # Safety
/// `once` must be a `pthread_once_t` and `routine` a function that takes nothing.
pub unsafe extern "C" fn pthread_once(once: *mut Once, routine: extern "C" fn()) -> c_int {
    // SAFETY: the caller's promise.
    let state = unsafe { &(*once).state };
    loop {
        match state.compare_exchange(0, ONCE_RUNNING, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) => {
                routine();
                state.store(ONCE_DONE, Ordering::Release);
                linux::futex_wake(state, i32::MAX as u32);
                return 0;
            }
            Err(ONCE_DONE) => return 0,
            Err(_) => {
                let _ = linux::futex_wait(state, ONCE_RUNNING, None);
            }
        }
    }
}

/// A libdispatch semaphore.
#[repr(C)]
#[derive(Debug)]
pub struct Semaphore {
    value: AtomicU32,
    references: AtomicU32,
}

pub extern "C" fn dispatch_semaphore_create(value: isize) -> *mut Semaphore {
    if !(0..=i32::MAX as isize).contains(&value) {
        return core::ptr::null_mut();
    }
    let semaphore = super::heap::allocate(size_of::<Semaphore>(), true).cast::<Semaphore>();
    if !semaphore.is_null() {
        // SAFETY: a new block of the semaphore's size.
        unsafe {
            semaphore.write(Semaphore {
                value: AtomicU32::new(value as u32),
                references: AtomicU32::new(1),
            })
        };
    }
    semaphore
}

/// Nanoseconds on the monotonic clock, the unit of `mach_absolute_time` and so of
/// `dispatch_time_t`.
fn now() -> u64 {
    super::process::mach_absolute_time()
}

pub extern "C" fn dispatch_time(when: u64, delta: i64) -> u64 {
    if when == TIME_FOREVER {
        return TIME_FOREVER;
    }
    let base = if when == TIME_NOW { now() } else { when };
    base.saturating_add_signed(delta).min(TIME_FOREVER - 1)
}

/// # Safety
/// `semaphore` must come from [`dispatch_semaphore_create`] and not be released.
pub unsafe extern "C" fn dispatch_semaphore_wait(semaphore: *mut Semaphore, timeout: u64) -> isize {
    // SAFETY: the caller's promise.
    let value = unsafe { &(*semaphore).value };
    loop {
        let current = value.load(Ordering::Acquire);
        if current > 0 {
            if value
                .compare_exchange_weak(current, current - 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return 0;
            }
            continue;
        }
        let remaining = match timeout {
            TIME_FOREVER => None,
            TIME_NOW => return TIMED_OUT,
            deadline => match deadline.checked_sub(now()) {
                Some(left) if left > 0 => Some(left),
                _ => return TIMED_OUT,
            },
        };
        let _ = linux::futex_wait(value, 0, remaining);
    }
}

/// # Safety
/// As [`dispatch_semaphore_wait`].
pub unsafe extern "C" fn dispatch_semaphore_signal(semaphore: *mut Semaphore) -> isize {
    // SAFETY: the caller's promise.
    let value = unsafe { &(*semaphore).value };
    value.fetch_add(1, Ordering::Release);
    linux::futex_wake(value, 1);
    0
}

/// # Safety
/// `object` must be a live dispatch object the layer made.
pub unsafe extern "C" fn dispatch_retain(object: *mut Semaphore) {
    if !object.is_null() {
        // SAFETY: the caller's promise.
        unsafe { &(*object).references }.fetch_add(1, Ordering::Relaxed);
    }
}

/// # Safety
/// As [`dispatch_retain`]; the caller gives up its reference.
pub unsafe extern "C" fn dispatch_release(object: *mut Semaphore) {
    if object.is_null() {
        return;
    }
    // SAFETY: the caller's promise.
    if unsafe { &(*object).references }.fetch_sub(1, Ordering::AcqRel) == 1 {
        super::heap::release(object.cast());
    }
}
