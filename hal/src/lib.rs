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
pub mod fp;
mod interrupt;
pub mod memory;
pub mod paging;
pub mod random;
pub mod thread;
pub mod time;
pub mod user;

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
/// over the platform and every call is resolved and inlined at compile time. `'static`, as a
/// type with no data is: kernel objects that live on their own (a process its threads share)
/// are generic over it.
pub trait Platform: 'static {
    /// Short architecture name, e.g. `"aarch64"`.
    const ARCH: &'static str;

    /// Writes raw bytes to the early boot console. Must work before memory management is up.
    fn console_write(bytes: &[u8]);

    /// Takes bytes the early boot console's UART received into `buffer`, until it is full or
    /// nothing is waiting, and returns how many. Never waits.
    fn console_read(buffer: &mut [u8]) -> usize;

    /// Routes the early boot console's receive interrupt to the boot CPU and turns it on: from
    /// then on, received bytes raise [`Interrupt::ConsoleInput`] there. Bytes received before
    /// may be lost. Called once, on the boot CPU, once the kernel's page tables are active. Fails
    /// when the machine's description gives no way to route it.
    fn enable_console_input() -> Result<(), &'static str>;

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

    /// Switches this CPU's low half, user space, to the page tables at physical addresses
    /// `roots` (one per [`paging::Format::ROOTS`]), whose kernel half is the active tables' own:
    /// on AArch64 `roots[1]` is the active `TTBR1_EL1` root and only `TTBR0_EL1` changes; with
    /// one root, the root's kernel-half entries are the active root's. Translations of the low
    /// half cached from the tables active before are dropped; the kernel's are kept where the
    /// architecture tags them apart (AArch64). Instructions [`Self::make_instructions_visible`]
    /// made visible on any CPU before the switch are visible to this CPU's instruction fetch
    /// after it (on RISC-V the switch runs `fence.i`), so a program loaded on one CPU runs on
    /// another once that CPU switches to its address space (D48). Every store this CPU made
    /// before the switch is visible to other CPUs before the new tables' first walk reads an
    /// entry (on RISC-V the switch starts with a full fence), so a CPU that changes an entry,
    /// fences, then finds no record of this switch knows this CPU walks the new entry (D94).
    ///
    /// # Safety
    /// Everything [`Self::activate_page_tables`] requires (interrupts masked, the tables in
    /// [`Self::PageTableFormat`], unchanged and allocated while active), and the kernel half
    /// must be exactly the active one. Nothing may hold a pointer into the low half the tables
    /// replace.
    unsafe fn switch_address_space(roots: &[u64]);

    /// Runs `access` with the kernel allowed to read and write user mappings
    /// ([`paging::Attributes::user`]) of the active address space: on RISC-V, S-mode reaches
    /// U-mode pages only with `sstatus.SUM` set, which this sets for `access` alone. The kernel
    /// touches user memory nowhere else, so a stray kernel pointer into user space faults
    /// instead of reading what a program put there. (On AArch64 and x86_64 the kernel does not
    /// turn PAN and SMAP on yet; when it does, they are lifted here too.) `access` must not
    /// block or switch threads.
    fn with_user_access<R>(access: impl FnOnce() -> R) -> R;

    /// Runs the user program whose registers are in `context`, in user mode with the address
    /// space active on this CPU, until a system call, an interrupt or an exception takes the CPU
    /// back to the kernel; saves the program's registers in `context` and says why (D47). The
    /// program runs with interrupts unmasked; the kernel continues with them masked. Values in
    /// `context` that would let the program out of user mode are not used: the HAL enters
    /// user mode with its own privilege, interrupt and segment settings, and on x86_64 a
    /// program counter outside user space is refused (as an [`ExceptionKind::InstructionFault`])
    /// without entering, since `sysretq` and `iretq` would fault in the kernel.
    ///
    /// # Safety
    /// Interrupts must be masked (as the kernel runs outside [`Self::wait_for_interrupt`]): the
    /// way in and out passes through code no handler may interrupt. The active page tables'
    /// kernel half must be the kernel's own, as [`Self::switch_address_space`] keeps it, and
    /// nothing may switch this CPU's address space or threads until the call returns.
    unsafe fn run_user(context: &mut user::UserContext) -> user::UserExit;

    /// The user-mode test program for the boot self-tests and benchmarks (D47).
    fn user_test_program() -> user::TestProgram;

    /// What user programs may run beyond the base instruction set, as Linux's `AT_HWCAP` and
    /// `AT_HWCAP2` encode it for the architecture (D51): only what user mode really runs as
    /// the kernel sets it up. FP/SIMD, which programs get on first use (D54): FP and ASIMD on
    /// AArch64 (no SVE or SME), RV64GC's letters with F and D on RISC-V (no V), and on x86_64
    /// `CPUID.1:EDX`, with `AT_HWCAP2` 0 (its `FSGSBASE` bit would let programs set their GS
    /// base, D49's trigger).
    fn linux_hardware_capabilities() -> [u64; 2];

    /// Hands the kernel's random number generator (D56) the seed material the machine offers,
    /// each piece to `seed` with its source: the device tree's `/chosen/rng-seed` and
    /// `kaslr-seed` on AArch64 and RISC-V, 256 bits from `RDSEED` (or `RDRAND`) on x86_64.
    /// Called on the boot CPU before any other CPU starts: at boot, and again by a boot
    /// self-test that checks the device tree's seeds are gone. A device-tree seed is
    /// removed from the tree once handed over (the whole property, value included, overwritten
    /// with `FDT_NOP` tokens), so nothing that reads the tree later sees it, and CPU samples are
    /// wiped after the call: the kernel must not keep `seed`'s bytes either, beyond hashing
    /// them. Nothing at all on a machine with no source.
    fn random_seeds(seed: &mut dyn FnMut(random::SeedSource, &[u8]));

    /// Makes the instructions just written as data at kernel virtual addresses
    /// `start..start + length` visible to instruction fetch on this CPU, at any virtual address
    /// that maps them: on AArch64 it cleans the data cache to the point of unification and
    /// invalidates the instruction caches of every CPU in the inner shareable domain, on RISC-V
    /// it runs `fence.i`; x86 keeps the two coherent itself. Another CPU sees them once it
    /// switches address space ([`Self::switch_address_space`]): on RISC-V, `fence.i` is this
    /// hart's alone, and the switch runs it there (D48).
    fn make_instructions_visible(start: usize, length: usize);

    /// Drops what this CPU's TLB cached of user pages `start..start + length` (4 KiB-aligned)
    /// in the active address space, after making the page-table writes before the call visible
    /// to its table walks: one page at a time up to
    /// [`paging::USER_PAGES_DROPPED_ONE_BY_ONE`] pages, every user translation beyond. Needed
    /// after a user mapping is added on RISC-V (a hart may cache an invalid entry, unless it has
    /// Svvptc) and AArch64 (the barrier), and on every architecture after a mapping is removed
    /// or its permissions change. The kernel's own translations are kept where the architecture
    /// tags them apart. Other CPUs are not told: the kernel sees to them.
    fn drop_user_translations(start: u64, length: u64);

    /// Whether an exception of `kind` with raw cause `code` ([`Exception::code`]) that a
    /// program took is a page fault, a translation the tables did not give (no valid entry, or
    /// one that does not allow the access), and if so which access made it. Alignment faults,
    /// external aborts, access faults (RISC-V PMP) and the like are not: retrying them would
    /// fault again. The kernel asks only when a program faults (demand paging, D52), so the
    /// trap path decodes nothing more.
    fn page_fault(kind: ExceptionKind, code: u64) -> Option<paging::PageFault>;

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

    /// Turns the FP/SIMD registers on for user mode on this CPU, or off, so that a program's
    /// first FP/SIMD instruction traps as an illegal instruction (D54, see [`fp`]). While they
    /// are off for user mode, x86_64 and RISC-V turn them off for the kernel too, so
    /// [`Self::save_fp`] and [`Self::restore_fp`] need them on. Each CPU starts with them off.
    ///
    /// # Safety
    /// Interrupts must be masked. Turning them on lets the next program run with whatever the
    /// registers hold: the caller first loads that program's own ([`Self::restore_fp`]), or
    /// knows they are still its own. Turning them off still leaves the values where some CPUs
    /// read them speculatively (LazyFP): before a program runs with them off, they must hold
    /// no other program's.
    unsafe fn set_user_fp(on: bool);

    /// Saves this CPU's FP/SIMD registers in `*state`. On RISC-V, only if a program changed
    /// them since they were last saved or loaded (`sstatus.FS` is Dirty): otherwise `*state`
    /// already holds them.
    ///
    /// # Safety
    /// Interrupts must be masked, the registers must be on ([`Self::set_user_fp`]), and
    /// `state` must be valid for a write. On RISC-V, `*state` must be where they were last
    /// saved or loaded, as the skip above relies on.
    unsafe fn save_fp(state: *mut fp::FpState);

    /// Loads this CPU's FP/SIMD registers from `*state`.
    ///
    /// # Safety
    /// Interrupts must be masked, the registers must be on ([`Self::set_user_fp`]), and
    /// `state` must be valid for a read and hold what [`Self::save_fp`] or
    /// [`fp::FpState::new`] wrote, changed only through [`fp::FpState`]'s methods.
    unsafe fn restore_fp(state: *const fp::FpState);

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
    /// On x86_64 it also loads this CPU's own GDT and TSS from the block's HAL part and sets
    /// `syscall` up, which [`Self::run_user`] needs (D47).
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
    /// 8-byte aligned `AtomicU64` in the block, or of an 8-byte aligned word inside an
    /// `UnsafeCell` that nothing writes while it is read (say, one guarded by a lock the
    /// caller holds).
    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        // SAFETY: the caller guarantees an aligned `AtomicU64` at `OFFSET` in the block, which
        // lives as long as the kernel runs, or an aligned word in an `UnsafeCell` (so a shared
        // atomic view of it is allowed) that nothing writes meanwhile.
        unsafe { &*((Self::cpu_local() + OFFSET) as *const core::sync::atomic::AtomicU64) }
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Raises [`Interrupt::Wakeup`] on the CPU with hardware id `cpu` (from [`Self::cpus`]),
    /// which must run the kernel's code (its [`cpu::CpuStart::entry`] was called, or it is the
    /// boot CPU; D41), the calling CPU included, which takes it once it unmasks interrupts
    /// (D95). Whatever the caller wrote to memory before the call is visible to that CPU once it
    /// takes the interrupt. Returns once the interrupt is sent, which may be before it is taken.
    /// Interrupts must be masked, as the kernel runs.
    fn wake_cpu(cpu: u64);
}
