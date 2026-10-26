//! Interrupts the kernel handles, described the same way on every architecture.
//!
//! Each architecture module acknowledges an interrupt at its interrupt controller, quiets the
//! source when needed (a fired one-shot timer), signals the end of the interrupt, and then
//! passes an [`Interrupt`] to the kernel's `kernel_interrupt` function; a device's interrupt
//! ([`Interrupt::ConsoleInput`]) is ended after the kernel has quieted the device instead. When
//! that returns, the interrupted code resumes. Spurious interrupts are dropped in the HAL; an interrupt from a
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
    /// The console's UART received bytes, once [`crate::Platform::enable_console_input`] turned
    /// its interrupt on. The kernel takes them with [`crate::Platform::console_read`] before it
    /// returns: the HAL ends the interrupt at its controller only then, so the UART stops
    /// asserting it first.
    ConsoleInput,
}

#[cfg(target_os = "none")]
unsafe extern "Rust" {
    /// Defined by the kernel binary with exactly this signature (Rust ABI), which makes declaring
    /// it `safe` sound. Returns to resume the interrupted code.
    safe fn kernel_interrupt(interrupt: Interrupt);
}

/// Hands `interrupt` to the kernel.
#[cfg(target_os = "none")]
pub(crate) fn deliver(interrupt: Interrupt) {
    kernel_interrupt(interrupt);
}
