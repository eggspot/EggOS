//! Synchronisation primitives for kernel data shared between CPUs.

use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// A spin lock: [`SpinLock::lock`] busy-waits until the value is free, doing what
/// [`set_wait_duty`] gave it meanwhile unless `WAIT_DUTY` is false ([`LeafSpinLock`]).
///
/// It does not mask interrupts, so an interrupt handler must never take a lock that the code it
/// interrupted may hold: on a single CPU that would spin forever. Locks are held for a few
/// instructions only; a critical section must not sleep or wait for an interrupt.
pub struct SpinLock<T, const WAIT_DUTY: bool = true> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock hands out at most one reference to the value at a time (see `lock`), so
// sharing the lock between CPUs only ever moves the value's use from one to another, which
// `T: Send` allows.
unsafe impl<T: Send, const WAIT_DUTY: bool> Sync for SpinLock<T, WAIT_DUTY> {}

/// A [`SpinLock`] whose waiters skip the wait duty, so taking it stays free of calls: only for
/// a leaf lock, whose holder takes no lock but other leaf locks and never waits for another
/// CPU. A CPU waiting for it then waits only for code that finishes on its own, and does its
/// duty at its next wait for another lock or interrupt.
pub type LeafSpinLock<T> = SpinLock<T, false>;

/// A [`LeafSpinLock`]'s guard.
pub type LeafSpinLockGuard<'a, T> = SpinLockGuard<'a, T, false>;

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }
}

impl<T> LeafSpinLock<T> {
    pub const fn new_leaf(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }
}

impl<T, const WAIT_DUTY: bool> SpinLock<T, WAIT_DUTY> {
    /// Waits until the lock is free, then holds it until the guard is dropped.
    #[inline]
    pub fn lock(&self) -> SpinLockGuard<'_, T, WAIT_DUTY> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if WAIT_DUTY {
                wait_until_free(&self.locked);
            } else {
                // Laid out away from the free case, which then falls straight through.
                core::hint::cold_path();
                // Wait with plain loads, so a waiting CPU does not keep taking the cache line
                // from the holder.
                while self.locked.load(Ordering::Relaxed) {
                    core::hint::spin_loop();
                }
            }
        }
        SpinLockGuard {
            lock: self,
            _value: PhantomData,
        }
    }

    /// The value, without taking the lock: `&mut self` means nobody else can hold it.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// Takes the lock if it is free; `None` if someone holds it.
    #[inline]
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T, WAIT_DUTY>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard {
                lock: self,
                _value: PhantomData,
            })
    }
}

/// What a CPU does while it waits for a contended [`SpinLock`]: null for nothing, else a
/// `fn()`. The kernel runs with interrupts masked, so a CPU that waits for this one to answer
/// a request (a TLB shootdown) needs it answered here as well.
static WAIT_DUTY: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());

/// Has every CPU call `duty` while it waits for a contended [`SpinLock`]. `duty` must not take
/// a [`SpinLock`] itself, and must work on any CPU at any point the kernel takes a lock.
pub fn set_wait_duty(duty: fn()) {
    WAIT_DUTY.store(duty as *mut (), Ordering::Release);
}

/// Waits until `locked` reads false, with plain loads, so a waiting CPU does not keep taking
/// the cache line from the holder, doing the wait duty meanwhile. Out of line: the free case
/// falls straight through without its code around it.
#[cold]
#[inline(never)]
fn wait_until_free(locked: &AtomicBool) {
    let duty = WAIT_DUTY.load(Ordering::Acquire);
    while locked.load(Ordering::Relaxed) {
        if !duty.is_null() {
            // SAFETY: only `set_wait_duty` stores a non-null value, and it stores a `fn()`.
            let duty: fn() = unsafe { core::mem::transmute::<*mut (), fn()>(duty) };
            duty();
        }
        core::hint::spin_loop();
    }
}

impl<T, const WAIT_DUTY: bool> SpinLock<T, WAIT_DUTY> {
    /// A guard for this lock, which a guard given up with [`SpinLockGuard::leak`] left held;
    /// dropping it releases the lock. The scheduler hands its lock across a context switch this
    /// way: the thread that switches leaks the guard, the thread it resumes takes it over.
    ///
    /// # Safety
    /// The lock must be held through a leaked guard, and each leak may be taken over once: the
    /// new guard is then the only access to the value.
    #[inline]
    pub unsafe fn assume_locked(&self) -> SpinLockGuard<'_, T, WAIT_DUTY> {
        debug_assert!(self.locked.load(Ordering::Relaxed), "the lock is not held");
        SpinLockGuard {
            lock: self,
            _value: PhantomData,
        }
    }
}

impl<T, const WAIT_DUTY: bool> fmt::Debug for SpinLock<T, WAIT_DUTY> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Access to a [`SpinLock`]'s value; the lock is released when it is dropped.
pub struct SpinLockGuard<'a, T, const WAIT_DUTY: bool = true> {
    lock: &'a SpinLock<T, WAIT_DUTY>,
    /// Makes the guard `Send` and `Sync` exactly when `&mut T` is: a shared guard hands out
    /// `&T`, so sharing it between threads needs `T: Sync` (the lock alone is `Sync` for any
    /// `T: Send`, which would otherwise let two threads reach a `Cell` through one guard).
    _value: PhantomData<&'a mut T>,
}

impl<T, const WAIT_DUTY: bool> SpinLockGuard<'_, T, WAIT_DUTY> {
    /// Gives up `guard` without releasing the lock, for [`SpinLock::assume_locked`] to take
    /// over.
    #[inline]
    pub fn leak(guard: Self) {
        core::mem::forget(guard);
    }
}

impl<T, const WAIT_DUTY: bool> Deref for SpinLockGuard<'_, T, WAIT_DUTY> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard exists only while its holder owns the lock, so no other reference
        // to the value exists.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T, const WAIT_DUTY: bool> DerefMut for SpinLockGuard<'_, T, WAIT_DUTY> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`, and `&mut self` makes this the only reference through the
        // guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T, const WAIT_DUTY: bool> Drop for SpinLockGuard<'_, T, WAIT_DUTY> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T, const WAIT_DUTY: bool> fmt::Debug for SpinLockGuard<'_, T, WAIT_DUTY> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLockGuard").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_lock_fails_while_held_and_succeeds_after() {
        let lock = SpinLock::new(1);
        let mut guard = lock.lock();
        *guard += 1;
        assert!(lock.try_lock().is_none());
        drop(guard);
        assert_eq!(*lock.try_lock().unwrap(), 2);
    }

    #[test]
    fn a_leaked_guard_keeps_the_lock_until_taken_over() {
        let lock = SpinLock::new(1);
        let mut guard = lock.lock();
        *guard = 2;
        SpinLockGuard::leak(guard);
        assert!(lock.try_lock().is_none());
        // SAFETY: held through the guard just leaked, taken over once.
        let guard = unsafe { lock.assume_locked() };
        assert_eq!(*guard, 2);
        drop(guard);
        assert_eq!(*lock.try_lock().unwrap(), 2);
    }

    #[test]
    fn threads_never_lose_an_update() {
        let lock = SpinLock::new(0u64);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..10_000 {
                        *lock.lock() += 1;
                    }
                });
            }
        });
        assert_eq!(*lock.lock(), 40_000);
    }
}
