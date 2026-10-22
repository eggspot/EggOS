//! Interrupts the kernel handles, described the same way on every architecture.
//!
//! Each architecture module acknowledges an interrupt at its interrupt controller, quiets the
//! source when needed (a fired one-shot timer), signals the end of the interrupt, and then
//! passes an [`Interrupt`] to the kernel's `kernel_interrupt` function. When that returns, the
//! interrupted code resumes. Spurious interrupts are dropped in the HAL; an interrupt from a
//! source the kernel has not asked for is reported as an [`crate::Exception`] of kind
//! [`crate::ExceptionKind::Interrupt`].

/// An interrupt delivered to the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// This CPU's timer reached the deadline set with [`crate::Platform::set_timer_deadline`].
    /// The timer is disarmed until the next `set_timer_deadline`.
    Timer,
    /// Another CPU called [`crate::Platform::wake_cpu`] for this one. It carries nothing: the
    /// kernel looks at its own shared state to see why (D41). Wakes a CPU waiting in
    /// [`crate::Platform::wait_for_interrupt`]. Wakes sent while one is pending may merge into
    /// one.
    Wakeup,
}

// The kernel binary defines `kernel_interrupt` with exactly this signature (Rust ABI), which is
// what makes declaring it `safe` sound.
#[cfg(target_os = "none")]
unsafe extern "Rust" {
    /// Defined by the kernel binary. Returns to resume the interrupted code.
    safe fn kernel_interrupt(interrupt: Interrupt);
}

/// Hands `interrupt` to the kernel.
#[cfg(target_os = "none")]
pub(crate) fn deliver(interrupt: Interrupt) {
    kernel_interrupt(interrupt);
}
