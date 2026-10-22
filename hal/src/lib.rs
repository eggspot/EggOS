//! EggOS hardware abstraction layer.
//!
//! The kernel core is architecture-independent: it only talks to the machine through the
//! [`Platform`] trait defined here. Each architecture lives in its own module under
//! [`arch`], compiled only for that architecture's bare-metal target, so adding an
//! architecture never touches the core (decision D4 / D6 in `docs/decisions.md`).

#![cfg_attr(not(test), no_std)]

pub mod acpi;
pub mod arch;
pub mod cpu;
mod exception;
pub mod fdt;
mod interrupt;
pub mod memory;
pub mod paging;
pub mod thread;
pub mod time;

pub use exception::{Exception, ExceptionKind};
pub use interrupt::Interrupt;
pub use memory::{MemoryMap, PhysicalRange};

/// How a run ends, as reported to whoever started the machine (in tests: the QEMU runner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Success,
    Failure,
}

/// The services every architecture module provides to the kernel core.
///
/// Implementations are zero-sized types with associated functions, so the core is generic
/// over the platform and every call is resolved and inlined at compile time.
pub trait Platform {
    /// Short architecture name, e.g. `"aarch64"`.
    const ARCH: &'static str;

    /// Writes raw bytes to the early boot console. Must work before memory management is up.
    fn console_write(bytes: &[u8]);

    /// Powers the machine off, reporting `status` to the host when the machine supports it.
    fn exit(status: ExitStatus) -> !;

    /// Parks the current CPU forever, waiting for interrupts.
    fn halt() -> !;

    /// Executes the architecture's breakpoint instruction. The exception goes to the kernel's
    /// handler (see [`Exception`]); if the handler returns, execution continues after the
    /// breakpoint instruction.
    fn breakpoint();

    /// Executes an instruction the architecture defines as permanently undefined (`udf` on
    /// AArch64, `unimp` on RISC-V, `ud2` on x86). The kernel uses it to test that a fatal
    /// exception is reported; the kernel's handler never returns from that exception.
    fn execute_illegal_instruction() -> !;

    /// This CPU's monotonic clock: ticks since an arbitrary point at or before boot. It never
    /// goes backwards and counts at [`Self::clock_frequency`].
    fn clock_ticks() -> u64;

    /// Ticks of [`Self::clock_ticks`] per second, fixed from boot on; never 0.
    fn clock_frequency() -> u64;

    /// Arms this CPU's one-shot timer: once [`Self::clock_ticks`] reaches `deadline`, the
    /// kernel's interrupt handler receives [`Interrupt::Timer`] (as soon as interrupts are
    /// unmasked). Never earlier; a deadline already passed fires at once. Replaces any
    /// deadline set before. The timer fires once per call. `u64::MAX` disarms it: from then on it
    /// neither fires nor wakes the CPU (one raised before, for a replaced deadline, may still
    /// arrive, and is not delivered).
    fn set_timer_deadline(deadline: u64);

    /// Called with interrupts masked, as the kernel runs today: sleeps until an interrupt is
    /// pending, lets the kernel handle it, and returns with interrupts masked again. An
    /// interrupt that became pending before the call is handled at once, so checking a
    /// condition and then calling this cannot miss the interrupt that changes it.
    fn wait_for_interrupt();

    /// Called with interrupts masked: unmasks them just long enough for an interrupt already
    /// pending to be handled, then masks them again. Unlike [`Self::wait_for_interrupt`] it
    /// never sleeps: with nothing pending it returns at once. The kernel's preemption points
    /// use it to take the timer tick (D37).
    fn interrupt_window();

    /// The physical memory the kernel may allocate: the machine's RAM as the firmware or
    /// loader describes it, minus everything in use (the kernel image and its boot stack, the
    /// firmware, and the boot data the kernel still reads, such as the device tree), limited
    /// to what the kernel can reach today. Built from the firmware's description on each call;
    /// the kernel calls it once at boot.
    fn memory_map() -> MemoryMap;

    /// Where the kernel reaches physical memory: free RAM at physical address `p` (from
    /// [`Self::memory_map`]) is readable and writable at virtual address `p + offset`. On every
    /// architecture this is [`Self::direct_map_offset`] from the kernel's first instruction on:
    /// the boot code maps the direct map before any Rust code runs, and the kernel's own page
    /// tables keep it where it is (D33).
    fn physical_memory_offset() -> usize;

    /// This architecture's page-table format.
    type PageTableFormat: paging::Format;

    /// Where the kernel's page tables put the direct map: physical address `p` at virtual
    /// address `p + offset`, in the high half of the address space (D29).
    fn direct_map_offset() -> usize;

    /// Where the parts of the kernel image are in physical memory, from the linker script,
    /// and the virtual address it is linked and runs at (in the top 2 GiB, D33).
    fn kernel_image() -> paging::KernelImage;

    /// All the physical memory the direct map covers: every range of RAM, whether free or
    /// reserved, and the firmware tables the HAL reads (such as ACPI's on x86_64). Device
    /// registers are not part of it; where a firmware range covers an early device region
    /// anyway (x86 loaders may report the local APIC's page as reserved), the kernel leaves
    /// those pages out of the normal direct map and maps them as device memory only. Built from
    /// the firmware's description on each call.
    fn physical_memory() -> MemoryMap;

    /// Calls `region` for each block of device registers the HAL itself uses before the driver
    /// model exists (the early console, the interrupt controller, the timer), which the HAL
    /// reaches at physical address + [`Self::direct_map_offset`] and the kernel's page tables
    /// map there as device memory (D33). Call it after boot
    /// set up the timer (as `kernel_main` runs).
    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange));

    /// Switches this CPU from the boot page tables to the page tables at physical addresses
    /// `roots` (one per [`paging::Format::ROOTS`]). The boot tables' identity map of the kernel
    /// is gone from then on: nothing is mapped in the low half.
    ///
    /// # Safety
    /// The tables must be in [`Self::PageTableFormat`], stay unchanged and allocated for as long
    /// as they are active, and map at least: the kernel image at its virtual address (code
    /// executable, data and stack writable), every range of [`Self::physical_memory`] at
    /// [`Self::direct_map_offset`], and every [`Self::early_device_regions`] region at the same
    /// offset as device memory. Nothing may hold a pointer into memory those tables leave
    /// unmapped. Interrupts must be masked (as the kernel runs outside
    /// [`Self::wait_for_interrupt`]): the switch passes through code no handler may interrupt.
    unsafe fn activate_page_tables(roots: &[u64]);

    /// Lays out a new kernel thread on the stack whose highest address is `stack_top` and
    /// returns its saved context: the first [`Self::switch_thread`] to that context calls
    /// `entry(argument)` on that stack, with interrupts masked (D35). `entry` must never return.
    ///
    /// # Safety
    /// The [`thread::INITIAL_FRAME_MAX`] bytes below `stack_top` must be writable memory that
    /// the caller owns, and the whole stack must stay allocated and otherwise unused for as long
    /// as the thread may run.
    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize;

    /// Saves the running thread's context in `*save` and resumes the thread whose saved
    /// context is `resume` (from [`Self::prepare_thread`], or saved by an earlier switch). The
    /// call returns once another switch resumes the context saved in `*save`.
    ///
    /// # Safety
    /// Interrupts must be masked (as the kernel runs outside [`Self::wait_for_interrupt`]).
    /// `save` must be valid for a write. `resume` must be a context saved and not resumed
    /// since, whose stack is still allocated: resuming a context twice runs two threads on one
    /// stack.
    unsafe fn switch_thread(save: *mut usize, resume: usize);

    /// Calls `cpu` with the hardware id of every CPU the firmware describes as usable, the
    /// boot CPU included, in the firmware's order (D40). Built from the firmware's description
    /// on each call.
    fn cpus(cpu: &mut dyn FnMut(u64));

    /// The hardware id of the CPU that booted the kernel.
    fn boot_cpu() -> u64;

    /// Starts the stopped CPU with hardware id `cpu` (from [`Self::cpus`]): it switches to the
    /// page tables in `start` and calls `start.entry(start.argument)` on `start.stack_top`,
    /// with interrupts masked and the exception vectors installed. Returns once the firmware or
    /// the machine has accepted the request, which may be before the CPU runs.
    ///
    /// # Safety
    /// `start`'s page tables must be the kernel's own, active on this CPU, and meet
    /// [`Self::activate_page_tables`]' requirements for as long as the CPU may run. `start` must
    /// stay allocated and unchanged until the CPU has called its entry, and its stack must be as
    /// [`cpu::CpuStart::stack_top`] says. Calls must not overlap (x86_64 starts CPUs through one
    /// trampoline).
    unsafe fn start_cpu(cpu: u64, start: &cpu::CpuStart) -> Result<(), cpu::StartCpuError>;

    /// Sets this CPU's per-CPU block pointer, which [`Self::cpu_local`] returns from then on.
    ///
    /// # Safety
    /// `block` must be the address of a block that lives for as long as the kernel runs, whose
    /// first word holds `block` itself (x86_64 reads it back through `gs:0`), followed at
    /// [`cpu::HAL_CPU_LOCAL_OFFSET`] by a [`cpu::HalCpuLocal`] made with
    /// [`cpu::HalCpuLocal::new`] that only the HAL uses. Must run before this CPU arms its timer.
    unsafe fn set_cpu_local(block: usize);

    /// This CPU's per-CPU block pointer. Only valid once [`Self::set_cpu_local`] has run on
    /// this CPU; before that, x86_64 faults.
    fn cpu_local() -> usize;

    /// The 64-bit word at byte `OFFSET` of this CPU's per-CPU block, with one load where the
    /// architecture allows (GS-relative on x86_64), so that hot paths reach per-CPU data as
    /// cheaply as a global. A relaxed atomic load: it never tears, even while an interrupt
    /// handler on this CPU changes the word.
    ///
    /// # Safety
    /// [`Self::set_cpu_local`] must have run on this CPU, and `OFFSET` must be the offset of an
    /// 8-byte aligned `AtomicU64` in the block.
    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        // SAFETY: the caller guarantees an aligned `AtomicU64` at `OFFSET` in the block, which
        // lives as long as the kernel runs.
        unsafe { &*((Self::cpu_local() + OFFSET) as *const core::sync::atomic::AtomicU64) }
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Raises [`Interrupt::Wakeup`] on the CPU with hardware id `cpu` (from [`Self::cpus`]),
    /// which must run the kernel's code (its [`cpu::CpuStart::entry`] was called, or it is the
    /// boot CPU) and not be the calling CPU (D41). Whatever the caller wrote to memory before
    /// the call is visible to that CPU once it takes the interrupt. Returns once the interrupt
    /// is sent, which may be before it is taken. Interrupts must be masked, as the kernel runs.
    fn wake_cpu(cpu: u64);
}
