//! One module per CPU architecture. Each is compiled only for its own bare-metal target
//! (`target_os = "none"`), so host builds and tests never see architecture code.
//!
//! An architecture module provides the boot entry point (`_start`, or `_start32` on x86_64,
//! which zeroes `.bss`, builds the boot page tables and jumps to the kernel's virtual address
//! in the top 2 GiB (D33), then sets up a stack, installs the exception vectors, sets up the
//! timer and its interrupt controller, and calls the kernel's `kernel_main`), the secondary
//! CPUs' entry point (`eggos_secondary_start`, or the real-mode trampoline on x86_64, see
//! [`crate::cpu`]), the exception
//! vectors (which hand every exception to the kernel's `kernel_exception`, see
//! [`crate::Exception`], and every timer interrupt to its `kernel_interrupt`, see
//! [`crate::Interrupt`]), the kernel thread switch and trampoline (`thread`, see
//! [`crate::thread`]), its linker script (`linker.ld`, used by the kernel's build script)
//! and a type implementing [`crate::Platform`], re-exported here as `CurrentPlatform`, which
//! also reports the free physical memory (see [`crate::MemoryMap`];
//! the shared parts of building it are in `boot_memory`).

#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub mod aarch64;
#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub use aarch64::Aarch64 as CurrentPlatform;
#[cfg(all(target_os = "none", target_arch = "aarch64"))]
pub(crate) use aarch64::KERNEL_VIRTUAL_OFFSET;

#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub mod riscv64;
#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub(crate) use riscv64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(target_os = "none", target_arch = "riscv64"))]
pub use riscv64::Riscv64 as CurrentPlatform;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod x86_64;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) use x86_64::KERNEL_VIRTUAL_OFFSET;
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
