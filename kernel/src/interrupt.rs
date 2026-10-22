//! What the kernel does with an interrupt, on every architecture.
//!
//! The HAL acknowledges each interrupt at its controller and hands it to the kernel binary,
//! which calls [`handle`]. For now the kernel only counts interrupts, per CPU, in the CPU's
//! block ([`crate::cpu::Cpu`], D41). The count of timer interrupts the scheduler has not seen
//! yet is also how its preemption points see the timer tick (D37), so preemption adds nothing to
//! the interrupt path; a wake-up only has to end the CPU's wait for an interrupt.

use crate::cpu;
use eggos_hal::{Interrupt, Platform};

/// Handles `interrupt`, which this CPU has taken; the interrupted code resumes afterwards.
pub fn handle<P: Platform>(interrupt: Interrupt) {
    cpu::current::<P>().count(interrupt);
}

/// How many timer interrupts this CPU has taken since boot.
pub fn timer_interrupts<P: Platform>() -> u64 {
    cpu::current::<P>().timer_interrupts()
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
    use crate::tests::TestPlatform as P;

    #[test]
    fn interrupts_are_counted_on_the_cpu_that_took_them() {
        let _lock = lock_timer_interrupts_for_test();
        // No scheduler test may move the counts meanwhile.
        let _scheduler = crate::thread::host::lock_kernel_state_for_test();
        let this = cpu::current::<P>();
        let (timer, wakeups) = (timer_interrupts::<P>(), this.wakeups());
        handle::<P>(Interrupt::Timer);
        assert_eq!(timer_interrupts::<P>(), timer + 1);
        assert_eq!(this.timer_interrupts(), timer + 1);
        handle::<P>(Interrupt::Wakeup);
        assert_eq!(this.wakeups(), wakeups + 1);
        assert_eq!(timer_interrupts::<P>(), timer + 1);
    }
}
