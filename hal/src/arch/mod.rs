//! One module per CPU architecture. Each is compiled only for its own bare-metal target
//! (`target_os = "none"`), so host builds and tests never see architecture code.
//!
//! An architecture module provides the boot entry point (`_start`, which sets up a stack,
//! zeroes `.bss` and calls the kernel's `kernel_main`), its linker script (`linker.ld`, used
//! by the kernel's build script) and a type implementing [`crate::Platform`], re-exported
//! here as `CurrentPlatform`.

#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub mod aarch64;
#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub use aarch64::Aarch64 as CurrentPlatform;

#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub mod riscv64;
#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub use riscv64::Riscv64 as CurrentPlatform;

#[cfg(all(
    target_os = "none",
    not(any(target_arch = "aarch64", target_arch = "riscv64"))
))]
compile_error!("EggOS has no HAL module for this architecture yet (see docs/roadmap.md)");
