//! Synchronisation primitives for kernel data shared between CPUs.

use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spin lock: [`SpinLock::lock`] busy-waits until the value is free.
///
/// It does not mask interrupts, so an interrupt handler must never take a lock that the code it
/// interrupted may hold: on a single CPU that would spin forever. Locks are held for a few
/// instructions only; a critical section must not sleep or wait for an interrupt.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock hands out at most one reference to the value at a time (see `lock`), so
// sharing the lock between CPUs only ever moves the value's use from one to another, which
// `T: Send` allows.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Waits until the lock is free, then holds it until the guard is dropped.
    #[inline]
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Wait with plain loads, so a waiting CPU does not keep taking the cache line from
            // the holder.
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinLockGuard {
            lock: self,
            _value: PhantomData,
        }
    }

    /// Takes the lock if it is free; `None` if someone holds it.
    #[inline]
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard {
                lock: self,
                _value: PhantomData,
            })
    }
}

impl<T> SpinLock<T> {
    /// A guard for this lock, which a guard given up with [`SpinLockGuard::leak`] left held;
    /// dropping it releases the lock. The scheduler hands its lock across a context switch this
    /// way: the thread that switches leaks the guard, the thread it resumes takes it over.
    ///
    /// # Safety
    /// The lock must be held through a leaked guard, and each leak may be taken over once: the
    /// new guard is then the only access to the value.
    #[inline]
    pub unsafe fn assume_locked(&self) -> SpinLockGuard<'_, T> {
        debug_assert!(self.locked.load(Ordering::Relaxed), "the lock is not held");
        SpinLockGuard {
            lock: self,
            _value: PhantomData,
        }
    }
}

impl<T> fmt::Debug for SpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Access to a [`SpinLock`]'s value; the lock is released when it is dropped.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    /// Makes the guard `Send` and `Sync` exactly when `&mut T` is: a shared guard hands out
    /// `&T`, so sharing it between threads needs `T: Sync` (the lock alone is `Sync` for any
    /// `T: Send`, which would otherwise let two threads reach a `Cell` through one guard).
    _value: PhantomData<&'a mut T>,
}

impl<T> SpinLockGuard<'_, T> {
    /// Gives up `guard` without releasing the lock, for [`SpinLock::assume_locked`] to take
    /// over.
    #[inline]
    pub fn leak(guard: Self) {
        core::mem::forget(guard);
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard exists only while its holder owns the lock, so no other reference
        // to the value exists.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`, and `&mut self` makes this the only reference through the
        // guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T> fmt::Debug for SpinLockGuard<'_, T> {
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
