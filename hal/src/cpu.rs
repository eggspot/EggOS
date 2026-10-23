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
use core::cell::UnsafeCell;
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
    /// x86_64: this CPU's GDT and TSS, which user mode needs (D47).
    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    pub(crate) x86: X86CpuTables,
}

impl HalCpuLocal {
    pub const fn new() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            timer_deadline: AtomicU64::new(u64::MAX),
            #[cfg(target_arch = "x86_64")]
            x86: X86CpuTables(UnsafeCell::new(X86Tables {
                gdt: [0; GDT_ENTRIES],
                tss: Tss {
                    reserved0: 0,
                    rsp: [0; 3],
                    reserved1: 0,
                    ist: [0; 7],
                    reserved2: 0,
                    reserved3: 0,
                    io_map_base: 0,
                },
                user_rsp: 0,
                fs_base_id: 0,
            })),
        }
    }
}

/// x86_64: a CPU's own descriptor tables (D47). The TSS is per CPU (its `RSP0` is where an
/// interrupt from ring 3 lands, and `ltr` marks its descriptor busy), so the GDT that holds its
/// descriptor is too. Written by the CPU itself in `set_cpu_local`, then read by the CPU (the
/// GDT and TSS) and by its ring-3 entry and exit code, GS-relative.
#[cfg(target_arch = "x86_64")]
#[derive(Debug)]
#[repr(transparent)]
pub(crate) struct X86CpuTables(pub(crate) UnsafeCell<X86Tables>);

// SAFETY: only the CPU whose block this is touches it (`set_cpu_local` and its own entry and exit
// code), with interrupts masked; other CPUs never do.
#[cfg(target_arch = "x86_64")]
unsafe impl Sync for X86CpuTables {}

#[cfg(target_arch = "x86_64")]
#[derive(Debug)]
#[repr(C)]
pub(crate) struct X86Tables {
    pub(crate) gdt: [u64; GDT_ENTRIES],
    pub(crate) tss: Tss,
    /// The program's `rsp` for the moment between `syscall` and its frame being pushed.
    pub(crate) user_rsp: u64,
    /// The `fs_base_id` of the user context this CPU's FS base was last loaded for: 0, the FS
    /// base zeroed by `set_cpu_local`, until a program with one set runs here.
    pub(crate) fs_base_id: u64,
}

/// Null, kernel code and data, user data and code, and the TSS's two.
#[cfg(target_arch = "x86_64")]
pub(crate) const GDT_ENTRIES: usize = 7;

/// The 64-bit task-state segment (SDM vol. 3A, figure 8-11): only `RSP0` is used, and no I/O
/// permission bitmap (`io_map_base` past the end), so ring 3 has no port access.
#[cfg(target_arch = "x86_64")]
#[derive(Debug)]
#[repr(C, packed(4))]
pub(crate) struct Tss {
    pub(crate) reserved0: u32,
    pub(crate) rsp: [u64; 3],
    pub(crate) reserved1: u64,
    pub(crate) ist: [u64; 7],
    pub(crate) reserved2: u64,
    pub(crate) reserved3: u16,
    pub(crate) io_map_base: u16,
}

#[cfg(target_arch = "x86_64")]
const _: () = assert!(core::mem::size_of::<Tss>() == 104);

impl Default for HalCpuLocal {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a per-CPU block holds its [`HalCpuLocal`]: right after the block's own address.
pub const HAL_CPU_LOCAL_OFFSET: usize = 8;

/// Where this CPU's `TSS.RSP0` is in a per-CPU block (x86_64).
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) const TSS_RSP0_OFFSET: usize = HAL_CPU_LOCAL_OFFSET
    + core::mem::offset_of!(HalCpuLocal, x86)
    + core::mem::offset_of!(X86Tables, tss)
    + core::mem::offset_of!(Tss, rsp);

/// Where the program's `rsp` is kept on the way in from `syscall` in a per-CPU block (x86_64).
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) const USER_RSP_OFFSET: usize = HAL_CPU_LOCAL_OFFSET
    + core::mem::offset_of!(HalCpuLocal, x86)
    + core::mem::offset_of!(X86Tables, user_rsp);

/// Where the `fs_base_id` this CPU's FS base was loaded for is in a per-CPU block (x86_64).
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) const FS_BASE_ID_OFFSET: usize = HAL_CPU_LOCAL_OFFSET
    + core::mem::offset_of!(HalCpuLocal, x86)
    + core::mem::offset_of!(X86Tables, fs_base_id);

/// Where the x86_64 timer's deadline is in a per-CPU block.
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(target_os = "none"), allow(dead_code))]
pub(crate) const TIMER_DEADLINE_OFFSET: usize =
    HAL_CPU_LOCAL_OFFSET + core::mem::offset_of!(HalCpuLocal, timer_deadline);
