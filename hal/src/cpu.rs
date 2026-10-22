//! Starting the other CPUs, the same way on every architecture (D40).
//!
//! The firmware describes the machine's CPUs by hardware id: the MPIDR affinity on AArch64, the
//! hart id on RISC-V, the local APIC id on x86. The kernel numbers them itself, 0 for the CPU
//! that booted, and starts each other one with [`crate::Platform::start_cpu`], passing a
//! [`CpuStart`]. The architecture's secondary entry code turns translation on with the boot
//! page tables, which stay in the image for this, moves to the kernel's virtual address,
//! switches to the stack in the [`CpuStart`], installs the exception vectors, switches to the
//! kernel's page tables and calls the entry function, with interrupts masked, as a thread
//! starts (D35). The entry code reads nothing the boot CPU wrote before translation and the
//! caches are on: on AArch64, memory read with the MMU off is not coherent with the boot CPU's
//! caches.
//!
//! Each CPU has a per-CPU block pointer ([`crate::Platform::set_cpu_local`]) in a register only
//! the kernel uses: `TPIDR_EL1` on AArch64, `tp` on RISC-V (while in the kernel; `sscratch`
//! stays free for the user-mode trap entry), the GS base on x86_64.

/// What a secondary CPU starts with. The kernel keeps it allocated and unchanged until the CPU
/// has called [`CpuStart::entry`]; the architecture's entry code reads it through the direct
/// map.
#[derive(Debug)]
#[repr(C)]
pub struct CpuStart {
    /// The highest address of the stack the CPU starts on, a kernel virtual address. The stack
    /// must be writable, unused by anything else and at least 16-byte aligned.
    pub stack_top: usize,
    /// Called on that stack with [`CpuStart::argument`]; must never return.
    pub entry: extern "C" fn(usize) -> !,
    pub argument: usize,
    /// The physical addresses of the kernel's page-table roots, the first
    /// [`crate::paging::Format::ROOTS`] of them used: the tables the CPU switches to.
    pub page_table_roots: [u64; MAX_PAGE_TABLE_ROOTS],
}

/// Most page-table roots any architecture has (AArch64's two, one per half).
pub const MAX_PAGE_TABLE_ROOTS: usize = 2;

/// Why [`crate::Platform::start_cpu`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartCpuError {
    /// The firmware does not know the CPU, or it cannot run (disabled, failed).
    InvalidCpu,
    /// The CPU is already running or being started.
    AlreadyRunning,
    /// The firmware or the machine refused for another reason, with its error code.
    Refused(i64),
}
