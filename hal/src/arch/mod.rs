//! One module per CPU architecture. Each is compiled only for its own bare-metal target
//! (`target_os = "none"`), so host builds and tests never see architecture code.
//!
//! An architecture module provides the boot entry point (`_start`, or `_start32` on x86_64,
//! which sets up a stack, zeroes `.bss`, installs the exception vectors, sets up the timer and
//! its interrupt controller, and calls the kernel's `kernel_main`), the exception vectors (which
//! hand every exception to the kernel's `kernel_exception`, see [`crate::Exception`], and every
//! timer interrupt to its `kernel_interrupt`, see [`crate::Interrupt`]), its linker script (`linker.ld`, used by the
//! kernel's build script) and a type implementing [`crate::Platform`], re-exported here as
//! `CurrentPlatform`, which also reports the free physical memory (see [`crate::MemoryMap`];
//! the shared parts of building it are in `boot_memory`).

#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub mod aarch64;
#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub use aarch64::Aarch64 as CurrentPlatform;

#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub mod riscv64;
#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub use riscv64::Riscv64 as CurrentPlatform;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod x86_64;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub use x86_64::X86_64 as CurrentPlatform;

#[cfg(all(
    target_os = "none",
    not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    ))
))]
compile_error!("EggOS has no HAL module for this architecture yet (see docs/roadmap.md)");

#[cfg(target_os = "none")]
pub(crate) mod boot_memory;
