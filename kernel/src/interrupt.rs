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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_interrupts_are_counted() {
        let before = timer_interrupts();
        handle(Interrupt::Timer);
        // At least one more: other tests may take timer interrupts in parallel.
        assert!(timer_interrupts() > before);
    }
}
