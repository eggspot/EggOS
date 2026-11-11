//! One module per CPU architecture. Each is compiled only for its own bare-metal target
//! (`cfg(eggos_kernel)`), so host builds and tests never see architecture code.
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

#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub mod aarch64;
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub use aarch64::Aarch64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub(crate) use aarch64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub(crate) use aarch64::kernel_virtual_offset;
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub(crate) use aarch64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub(crate) use aarch64::zero;

#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub mod loongarch64;
#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub(crate) use loongarch64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub use loongarch64::Loongarch64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub(crate) use loongarch64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub(crate) use loongarch64::zero;

#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub mod mips64;
#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub(crate) use mips64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub use mips64::Mips64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub(crate) use mips64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub(crate) use mips64::zero;

#[cfg(all(eggos_kernel, target_arch = "powerpc64"))]
pub mod powerpc64;
#[cfg(all(eggos_kernel, target_arch = "powerpc64"))]
pub(crate) use powerpc64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "powerpc64"))]
pub use powerpc64::Powerpc64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "powerpc64"))]
pub(crate) use powerpc64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "powerpc64"))]
pub(crate) use powerpc64::zero;

#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub mod s390x;
#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub(crate) use s390x::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub use s390x::S390x as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub(crate) use s390x::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub(crate) use s390x::zero;

#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub mod riscv64;
#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub(crate) use riscv64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub use riscv64::Riscv64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub(crate) use riscv64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub(crate) use riscv64::zero;

#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub mod sparc64;
#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub(crate) use sparc64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub use sparc64::Sparc64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub(crate) use sparc64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub(crate) use sparc64::zero;

#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub mod x86_64;
#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub(crate) use x86_64::KERNEL_VIRTUAL_OFFSET;
#[cfg(all(
    eggos_kernel,
    any(
        target_arch = "loongarch64",
        target_arch = "mips64",
        target_arch = "powerpc64",
        target_arch = "riscv64",
        target_arch = "s390x",
        target_arch = "sparc64",
        target_arch = "x86_64"
    )
))]
pub(crate) const fn kernel_virtual_offset() -> u64 {
    KERNEL_VIRTUAL_OFFSET
}
#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub use x86_64::X86_64 as CurrentPlatform;
#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub(crate) use x86_64::doorbell_barrier;
#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub(crate) use x86_64::zero;

#[cfg(all(
    eggos_kernel,
    not(any(
        target_arch = "aarch64",
        target_arch = "loongarch64",
        target_arch = "mips64",
        target_arch = "powerpc64",
        target_arch = "riscv64",
        target_arch = "s390x",
        target_arch = "sparc64",
        target_arch = "x86_64"
    ))
))]
compile_error!("EggOS has no HAL module for this architecture yet (see docs/roadmap.md)");

/// The QEMU machines the kernel's self-tests boot on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestMachine {
    /// AArch64's `virt` with a GICv2 and a PL011.
    ArmVirt,
    /// RISC-V's `virt` with a PLIC.
    RiscvVirt,
    /// x86_64's `q35`.
    Q35,
    /// LoongArch64's `virt` (D290).
    LoongarchVirt,
    /// MIPS64's `loongson3-virt` (D313).
    Loongson3Virt,
    /// s390x's `s390-ccw-virtio` (D321).
    S390CcwVirtio,
    /// SPARC64's `sun4u` (D326).
    Sun4u,
}

/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "aarch64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::ArmVirt);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "riscv64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::RiscvVirt);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "loongarch64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::LoongarchVirt);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "mips64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::Loongson3Virt);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "s390x"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::S390CcwVirtio);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "sparc64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::Sun4u);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(all(eggos_kernel, target_arch = "x86_64"))]
pub const TEST_MACHINE: Option<TestMachine> = Some(TestMachine::Q35);
/// The machine this kernel's self-tests expect; `None` in host builds.
#[cfg(not(eggos_kernel))]
pub const TEST_MACHINE: Option<TestMachine> = None;

#[cfg(eggos_kernel)]
pub(crate) mod boot_memory;
