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
//! stays free for the user-mode trap entry), the GS base on x86_64. The block starts with its own
//! address, then the HAL's part ([`HalCpuLocal`], at [`HAL_CPU_LOCAL_OFFSET`]); the rest is the
//! kernel's.
//!
//! Each CPU sets its own interrupt controller and timer up before it runs the kernel's code
//! (D41): the boot CPU's entry code before `kernel_main`, a secondary CPU's entry code before
//! [`CpuStart::entry`]. From then on its timer works ([`crate::Platform::set_timer_deadline`]
//! arms this CPU's) and other CPUs can wake it ([`crate::Platform::wake_cpu`]).

#[cfg(target_arch = "x86_64")]
use core::sync::atomic::AtomicU64;

/// What a secondary CPU starts with. The kernel keeps it allocated and unchanged until the CPU
/// has called [`CpuStart::entry`]; the architecture's entry code reads it through the boot
/// page tables, at the address the kernel passed (on x86_64 that address is in the
/// trampoline's slot, which the CPU reads through the boot tables' identity map).
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

/// The part of every per-CPU block the HAL keeps for itself, right after the block's own address
/// ([`crate::Platform::set_cpu_local`]). The kernel embeds one, made with [`HalCpuLocal::new`],
/// and never touches it.
#[derive(Debug)]
#[repr(C)]
pub struct HalCpuLocal {
    /// x86_64: the deadline this CPU's local APIC timer is armed for, in TSC ticks; `u64::MAX`
    /// for none. Reached by offset only, GS-relative. The other architectures keep nothing, so
    /// their blocks do not pay for it (D24).
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    timer_deadline: AtomicU64,
}

impl HalCpuLocal {
    pub const fn new() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            timer_deadline: AtomicU64::new(u64::MAX),
        }
    }
}

impl Default for HalCpuLocal {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a per-CPU block holds its [`HalCpuLocal`]: right after the block's own address.
pub const HAL_CPU_LOCAL_OFFSET: usize = 8;

/// Where the x86_64 timer's deadline is in a per-CPU block.
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) const TIMER_DEADLINE_OFFSET: usize =
    HAL_CPU_LOCAL_OFFSET + core::mem::offset_of!(HalCpuLocal, timer_deadline);
