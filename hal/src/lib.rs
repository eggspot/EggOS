//! EggOS hardware abstraction layer.
//!
//! The kernel core is architecture-independent: it only talks to the machine through the
//! [`Platform`] trait defined here. Each architecture lives in its own module under
//! [`arch`], compiled only for that architecture's bare-metal target, so adding an
//! architecture never touches the core (decision D4 / D6 in `docs/decisions.md`).

#![cfg_attr(not(test), no_std)]

pub mod acpi;
pub mod arch;
mod exception;
pub mod fdt;
mod interrupt;
pub mod memory;
pub mod paging;
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
    /// deadline set before. The timer fires once per call.
    fn set_timer_deadline(deadline: u64);

    /// Called with interrupts masked, as the kernel runs today: sleeps until an interrupt is
    /// pending, lets the kernel handle it, and returns with interrupts masked again. An
    /// interrupt that became pending before the call is handled at once, so checking a
    /// condition and then calling this cannot miss the interrupt that changes it.
    fn wait_for_interrupt();

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
    /// registers are not part of it. Built from the firmware's description on each call.
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
}
