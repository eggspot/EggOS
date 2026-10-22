//! What the kernel does with an interrupt, on every architecture.
//!
//! The HAL acknowledges each interrupt at its controller and hands it to the kernel binary,
//! which calls [`handle`]. For now the kernel only counts timer interrupts; the scheduler
//! (M4) will preempt on them.

use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Interrupt;

/// Timer interrupts taken since boot.
static TIMER_INTERRUPTS: AtomicU64 = AtomicU64::new(0);

/// Handles `interrupt`; the interrupted code resumes afterwards.
pub fn handle(interrupt: Interrupt) {
    match interrupt {
        Interrupt::Timer => {
            TIMER_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many timer interrupts [`handle`] has taken since boot.
pub fn timer_interrupts() -> u64 {
    TIMER_INTERRUPTS.load(Ordering::Relaxed)
}

/// Held by every host unit test that raises or counts timer interrupts. The counter is
/// global and tests run in parallel, so without it one test's interrupts satisfy (or fail)
/// another's wait.
#[cfg(test)]
pub(crate) fn lock_timer_interrupts_for_test() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A test that panicked while holding the lock leaves the counter as usable as before.
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_interrupts_are_counted() {
        let _lock = lock_timer_interrupts_for_test();
        let before = timer_interrupts();
        handle(Interrupt::Timer);
        assert_eq!(timer_interrupts(), before + 1);
    }
}
