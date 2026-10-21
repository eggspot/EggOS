//! The EggOS hardware abstraction layer.
//!
//! All architecture-specific code lives here, one module per architecture
//! under [`arch`]; the kernel core sees only the architecture-independent
//! [`Platform`] trait. Each architecture module provides:
//!
//! - the boot entry point (`_start`, in assembly), which sets up a stack, zeroes
//!   `.bss` and calls `eggos_kernel_start(boot_info: usize) -> !`, a symbol the
//!   kernel image defines;
//! - its linker script, `link.ld`, next to its source;
//! - `init(boot_info)`, returning that architecture's `Machine`, which
//!   implements [`Platform`];
//! - `emergency_console()` and `halt()`, for the panic handler.
//!
//! The architecture modules only build for bare-metal targets
//! (`target_os = "none"`); on a development host this crate is just the traits.

#![cfg_attr(not(test), no_std)]

pub use eggos_drivers::{CharDevice, Driver, DriverError};

#[cfg(target_os = "none")]
pub mod arch;

/// What the kernel core needs from the machine it runs on.
pub trait Platform {
    /// Name of the CPU architecture, e.g. `"x86_64"`.
    const ARCH: &'static str;

    /// The device the kernel writes its log to.
    type Console: CharDevice;

    /// The kernel log device.
    fn console(&self) -> &Self::Console;

    /// Stop this CPU for good, with interrupts masked.
    fn halt(&self) -> !;
}
