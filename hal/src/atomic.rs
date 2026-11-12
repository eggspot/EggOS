//! 64-bit atomics on every target (D483). Where the target has them, [`AtomicU64`] is
//! `core`'s; RV32's `A` extension has none, so there it is a word guarded by a spin lock with
//! the same methods. The kernel takes interrupts only where it holds no lock (D37), so no
//! handler can find the lock held by the code it interrupted.

#[cfg(target_has_atomic = "64")]
pub use core::sync::atomic::AtomicU64;

#[cfg(not(target_has_atomic = "64"))]
pub use emulated::AtomicU64;

#[cfg(not(target_has_atomic = "64"))]
mod emulated {
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// A 64-bit word read and changed under a spin lock, with `core`'s `AtomicU64` methods; each
    /// operation is sequentially consistent whatever ordering it is given.
    #[derive(Default)]
    pub struct AtomicU64 {
        locked: AtomicBool,
        value: UnsafeCell<u64>,
    }

    // SAFETY: every access to `value` holds `locked`, which makes the accesses exclusive.
    unsafe impl Sync for AtomicU64 {}

    impl core::fmt::Debug for AtomicU64 {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            self.load(Ordering::SeqCst).fmt(formatter)
        }
    }

    impl AtomicU64 {
        pub const fn new(value: u64) -> Self {
            Self {
                locked: AtomicBool::new(false),
                value: UnsafeCell::new(value),
            }
        }

        fn with<R>(&self, change: impl FnOnce(&mut u64) -> R) -> R {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
            // SAFETY: the lock is held, so no other access to the value runs.
            let result = change(unsafe { &mut *self.value.get() });
            self.locked.store(false, Ordering::Release);
            result
        }

        pub fn load(&self, _: Ordering) -> u64 {
            self.with(|value| *value)
        }

        pub fn store(&self, new: u64, _: Ordering) {
            self.with(|value| *value = new);
        }

        pub fn swap(&self, new: u64, _: Ordering) -> u64 {
            self.with(|value| core::mem::replace(value, new))
        }

        pub fn compare_exchange(
            &self,
            current: u64,
            new: u64,
            _: Ordering,
            _: Ordering,
        ) -> Result<u64, u64> {
            self.with(|value| match *value == current {
                true => {
                    *value = new;
                    Ok(current)
                }
                false => Err(*value),
            })
        }

        pub fn compare_exchange_weak(
            &self,
            current: u64,
            new: u64,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u64, u64> {
            self.compare_exchange(current, new, success, failure)
        }

        fn fetch(&self, change: impl FnOnce(u64) -> u64) -> u64 {
            self.with(|value| {
                let before = *value;
                *value = change(before);
                before
            })
        }

        pub fn fetch_add(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value.wrapping_add(operand))
        }

        pub fn fetch_sub(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value.wrapping_sub(operand))
        }

        pub fn fetch_and(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value & operand)
        }

        pub fn fetch_or(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value | operand)
        }

        pub fn fetch_xor(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value ^ operand)
        }

        pub fn fetch_max(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value.max(operand))
        }

        pub fn fetch_min(&self, operand: u64, _: Ordering) -> u64 {
            self.fetch(|value| value.min(operand))
        }

        pub fn get_mut(&mut self) -> &mut u64 {
            self.value.get_mut()
        }

        pub fn into_inner(self) -> u64 {
            self.value.into_inner()
        }
    }
}
