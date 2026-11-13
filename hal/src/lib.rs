//! EggOS hardware abstraction layer.
//!
//! The kernel core is architecture-independent: it only talks to the machine through the
//! [`Platform`] trait defined here. Each architecture lives in its own module under
//! [`arch`], compiled only for that architecture's bare-metal target, so adding an
//! architecture never touches the core (decision D4 / D6 in `docs/decisions.md`).

#![cfg_attr(not(test), no_std)]

#[cfg(all(eggos_kernel, eggos_user))]
compile_error!(
    "a build is the kernel's (`eggos_kernel`) or a program's (`eggos_user`), never both"
);
#[cfg(all(target_os = "none", not(eggos_kernel)))]
compile_error!("a bare-metal build of the HAL needs `--cfg eggos_kernel` (.cargo/config.toml)");

pub mod acpi;
pub mod aml;
pub mod arch;
pub mod atomic;
pub mod cache;
pub mod command_line;
pub mod cpu;
mod exception;
pub mod fdt;
pub mod fp;
pub mod handoff;
mod interrupt;
pub mod memory;
pub mod paging;
pub mod random;
pub mod thread;
pub mod time;
pub mod user;

pub use exception::{Exception, ExceptionKind};
pub use interrupt::{DeviceLine, Interrupt, InterruptMessage};
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

    /// Turns on the power button's interrupt, [`Interrupt::PowerButton`] (D202), where the
    /// machine has one the HAL answers: ACPI's fixed-feature button on x86_64. An error says why
    /// not.
    fn enable_power_button() -> Result<(), &'static str> {
        Err("this machine has no power button EggOS answers yet")
    }

    /// Switches the machine to ACPI mode if it is not and routes the system control interrupt
    /// (D539): from then on any cause but the fixed power button raises
    /// [`Interrupt::SystemControl`], with the interrupt masked until
    /// [`Self::unmask_system_control`], so the kernel can look at the event registers from a
    /// thread. Fails on a machine without ACPI's fixed hardware (the default).
    fn enable_system_control() -> Result<(), &'static str> {
        Err("this machine has no ACPI system control interrupt")
    }

    /// Unmasks the system control interrupt once the kernel has handled the events it raised.
    /// Nothing by default.
    fn unmask_system_control() {}

    /// Starts the PC's 8042 keyboard controller and its PS/2 keyboard and mouse, polled, where
    /// the machine has one (D239): what answered. None by default.
    fn start_legacy_input() -> eggos_drivers::input::ps2::Found {
        eggos_drivers::input::ps2::Found::default()
    }

    /// Hands `event` what the PS/2 devices sent since the last call, at most `budget` bytes'
    /// worth, and returns how many bytes it took. Called by the kernel's input poller only, once
    /// [`Self::start_legacy_input`] found a device.
    fn poll_legacy_input(
        _budget: usize,
        _event: &mut dyn FnMut(eggos_drivers::input::RawEvent),
    ) -> usize {
        0
    }

    /// Shows the lock lights on the PS/2 keyboard; `false` when it could not take them now.
    fn set_legacy_lights(_lights: eggos_drivers::input::LockLights) -> bool {
        true
    }

    /// Routes the device interrupt `specifier` names (a device-tree interrupt of the machine's
    /// interrupt controller: a GIC shared peripheral interrupt, a PLIC source) to the boot CPU and
    /// enables it: from then on it raises [`Interrupt::Device`] with the line returned. A
    /// specifier routed before gets its line again. Called on the boot CPU once the kernel's page
    /// tables are active. Fails on a machine without device-tree interrupts (the default), for a
    /// specifier the HAL cannot route, or when every line is taken.
    fn route_device_interrupt(
        _specifier: &fdt::InterruptSpecifier,
    ) -> Result<DeviceLine, &'static str> {
        Err("this machine routes no device-tree interrupts")
    }

    /// Takes a message-signalled interrupt for a device (PCI MSI-X): the message the device
    /// writes raises [`Interrupt::Device`] with the line returned, on the boot CPU. Called on the
    /// boot CPU. Fails on a machine without message-signalled interrupts (the default) or when
    /// every line is taken.
    fn message_interrupt() -> Result<(DeviceLine, InterruptMessage), &'static str> {
        Err("this machine has no message-signalled interrupts")
    }

    /// Routes the legacy interrupt input `global` (an x86 global system interrupt, which a PCI
    /// function's INTx pin arrives on), level-triggered and active-low as PCI's are unless
    /// `edge`, to the boot CPU: it raises [`Interrupt::Device`] with the line returned and is
    /// masked until [`Self::unmask_device_interrupt`] (D413). Fails on a machine without such
    /// inputs (the default) or when every line is taken.
    fn route_legacy_interrupt(_global: u32, _edge: bool) -> Result<DeviceLine, &'static str> {
        Err("this machine routes no legacy interrupts")
    }

    /// Unmasks `line` after its driver looked at the device: a level-triggered input masked by
    /// its interrupt; nothing for other lines (D413).
    fn unmask_device_interrupt(_line: DeviceLine) {}

    /// Gives `line` back when its driver lets the device go or connecting it failed: it raises
    /// nothing more and may be handed out again (D413). Nothing by default.
    fn release_device_interrupt(_line: DeviceLine) {}

    /// Moves `line`, which the HAL routes at an interrupt controller (a GIC shared peripheral
    /// interrupt, a PLIC or APLIC source, an I/O APIC input), to the CPU with hardware id `cpu`
    /// (from [`Self::cpus`]), which must run the kernel's code (D536). An interrupt the line
    /// raised while it moved may be lost, so the caller has the driver look once after. Fails
    /// for a message-signalled line, whose device takes [`Self::message_for`]'s message
    /// instead, and on a machine that keeps device interrupts on the boot CPU (the default).
    fn steer_device_interrupt(_line: DeviceLine, _cpu: u64) -> Result<(), &'static str> {
        Err("this machine keeps device interrupts on the boot CPU")
    }

    /// The message that raises the message-signalled `line` ([`Self::message_interrupt`]) on
    /// the CPU with hardware id `cpu` (from [`Self::cpus`]) instead, for the kernel to write to
    /// the device (D536). Fails for a line routed at an interrupt controller and on a machine
    /// that keeps message-signalled interrupts on the boot CPU (the default).
    fn message_for(_line: DeviceLine, _cpu: u64) -> Result<InterruptMessage, &'static str> {
        Err("this machine keeps message-signalled interrupts on the boot CPU")
    }

    /// Powers the machine off, reporting `status` to the host when the machine supports it.
    fn exit(status: ExitStatus) -> !;

    /// Resets the machine, which boots again as from power-on (or, under QEMU's `-no-reboot`,
    /// ends QEMU). Parks the CPU if the machine offers no way to reset it.
    fn restart() -> !;

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
    /// to what the kernel's direct map covers. Built from the firmware's description on each
    /// call; the kernel calls it once at boot.
    fn memory_map() -> MemoryMap;

    /// Where the boot direct map ends: the kernel builds its page tables only with memory
    /// below this physical address, and hands out the rest of [`Self::memory_map`] once its own
    /// tables, whose direct map covers all of it, are active (D171). No limit by default.
    fn boot_direct_map_end() -> u64 {
        u64::MAX
    }

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

    /// Calls `aperture` for each range of device registers that the firmware's memory description
    /// may cover but that the kernel must only reach as device memory, through a device window
    /// (D110): the direct map leaves those pages out, so no page has two memory types (D33). On
    /// x86_64, the PCI Express configuration space the `MCFG` names, which QEMU's loader lists as
    /// reserved. The default: none.
    fn device_apertures(_aperture: &mut dyn FnMut(PhysicalRange)) {}

    /// The flattened device tree the firmware handed over (AArch64 and RISC-V), whole, for device
    /// discovery (D110); `None` on a machine without one (the default) or when it is not valid.
    /// The kernel only reads it.
    fn device_tree() -> Option<&'static [u8]> {
        None
    }

    /// Hands `line` the kernel command line the machine was started with (QEMU's `-append`):
    /// `/chosen/bootargs` of the device tree by default, on x86_64 the PVH loader's, copied at
    /// boot. Nothing when there is none. Only the host sets it; nothing in the machine can.
    fn command_line(line: &mut dyn FnMut(&[u8])) {
        if let Some(arguments) = Self::device_tree().and_then(fdt::boot_arguments) {
            line(arguments);
        }
    }

    /// The ACPI table with `signature` the firmware's root table lists (x86_64), its length and
    /// checksum validated, for device discovery (D110): `MCFG` for PCI Express. `None` on a
    /// machine without ACPI (the default) or without that table.
    fn acpi_table(_signature: &[u8; 4]) -> Option<&'static [u8]> {
        None
    }

    /// Every validated ACPI table with `signature` the root table lists, in its order: the
    /// SSDTs, for the ACPI interpreter (D538). By default the one [`Self::acpi_table`] finds.
    fn acpi_tables(signature: &[u8; 4], table: &mut dyn FnMut(&'static [u8])) {
        if let Some(found) = Self::acpi_table(signature) {
            table(found);
        }
    }

    /// What the ACPI interpreter tells the firmware's `\_PIC` about the interrupt controller
    /// the kernel uses (D538): 1 for the I/O APIC on x86_64; `None` where firmware has no
    /// `\_PIC` to tell (the default).
    fn acpi_interrupt_model() -> Option<u64> {
        None
    }

    /// Reads `width` bits (8, 16 or 32) from I/O port `port`, for the ACPI interpreter's
    /// `SystemIO` regions (D538), on a machine with an I/O port space (x86_64); `None` elsewhere
    /// (the default) or for another width. Only firmware-described ports are reached this way.
    fn read_io_port(_port: u16, _width: u32) -> Option<u32> {
        None
    }

    /// Writes `value`'s low `width` bits (8, 16 or 32) to I/O port `port`, as
    /// [`Self::read_io_port`] reads: whether it was written.
    fn write_io_port(_port: u16, _width: u32, _value: u32) -> bool {
        false
    }

    /// The configuration space of a PCI host bridge the firmware serves without ECAM (POWER's
    /// PHBs through OPAL, D312), set up for the kernel's enumeration; `None` elsewhere.
    fn firmware_pci() -> Option<&'static dyn eggos_drivers::pci::ConfigSpace> {
        None
    }

    /// The machine's channel subsystem (s390x, D322), whose subchannels the kernel enumerates
    /// for devices; `None` elsewhere.
    fn channel_subsystem() -> Option<&'static dyn eggos_drivers::channel::ChannelSubsystem> {
        None
    }

    /// Where the kernel maps device registers (D110), when the place below the kernel image's
    /// root entry does not translate: MIPS64's image is in the unmapped `CKSEG0`, below which
    /// nothing is mapped, so its window is in `XKSEG` (D313). `None` elsewhere.
    fn device_window() -> Option<core::ops::Range<u64>> {
        None
    }

    /// What a device adds to a physical address to reach it by DMA: 0 where devices see
    /// physical memory as it is, POWER's bypass window base once [`Self::firmware_pci`] set it
    /// (D312).
    fn dma_device_offset() -> u64 {
        0
    }

    /// The ACPI DSDT, the definition block the `FADT` names, validated, for the namespace walk
    /// ([`aml::devices`], D252); `None` on a machine without ACPI.
    fn acpi_dsdt() -> Option<&'static [u8]> {
        None
    }

    /// The framebuffer the firmware's graphics output left set up, when EggOS's UEFI loader
    /// started the kernel and found one it can draw into (D236), else the device tree's
    /// `simple-framebuffer` (D542); `None` otherwise.
    fn boot_framebuffer() -> Option<handoff::Framebuffer>
    where
        Self: Sized,
    {
        handoff::handoff::<Self>()
            .map(|handoff| handoff.framebuffer)
            .filter(handoff::Framebuffer::is_usable)
            .or_else(|| Self::device_tree().and_then(fdt::simple_framebuffer))
            .filter(handoff::Framebuffer::is_usable)
    }

    /// The GPT unique partition GUID of the partition EggOS's UEFI loader was read from (D236),
    /// for the installer to find the disk it runs from; `None` when unknown.
    fn boot_partition() -> Option<[u8; 16]>
    where
        Self: Sized,
    {
        handoff::handoff::<Self>()
            .filter(handoff::Handoff::knows_boot_partition)
            .map(|handoff| handoff.boot_partition)
    }

    /// A file EggOS's UEFI loader read from the boot partition and kept (D241), for the
    /// installer; `None` when the kernel was started another way.
    fn boot_file(file: handoff::BootFile) -> Option<&'static [u8]>
    where
        Self: Sized,
    {
        handoff::file::<Self>(&handoff::handoff::<Self>()?, file)
    }

    /// Calls `clock` with the platform's own real-time clock, one no bus describes, if it has
    /// one (D150): the PC's CMOS clock on x86_64, unless the ACPI FADT says there is none. The
    /// device-tree clocks of AArch64 and RISC-V are drivers of the driver model instead. The
    /// default: none. The clock calls `wait_duty` at each poll while it waits for the hardware.
    ///
    /// # Safety
    ///
    /// Every call, and every use of the clock it hands `clock`, is made under one lock the
    /// caller holds for the platform's clock, so no two calls' register accesses interleave
    /// (the CMOS clock's index and data ports are one shared pair).
    unsafe fn platform_clock(
        _wait_duty: fn(),
        _clock: &mut dyn FnMut(&dyn eggos_drivers::rtc::RealTimeClock),
    ) {
    }

    /// Whether a device-tree device that says neither `dma-coherent` nor `dma-noncoherent` has
    /// coherent DMA: the architecture's convention (Linux's), true on RISC-V and false on Arm.
    const DEVICE_TREE_DMA_COHERENT_BY_DEFAULT: bool = true;

    /// Makes the kernel mappings just added to the active tables' device window (entries that
    /// were invalid, D110) usable by this CPU from the next instruction on: on AArch64 the table
    /// writes are completed for the walker (`dsb ishst; isb`), on RISC-V cached translations are
    /// dropped (`sfence.vma`, a hart may cache invalid entries); x86 caches no invalid entries,
    /// so the default does nothing. Other CPUs must not have reached `start..start + length`
    /// before.
    fn make_kernel_mappings_visible(_start: usize, _length: usize) {}

    /// Switches this CPU from the boot page tables to the page tables at physical addresses
    /// `roots` (one per [`paging::Format::ROOTS`]). The boot tables' identity map of the kernel
    /// is gone from then on: nothing is mapped in the low half.
    ///
    /// # Safety
    /// The tables must be in [`Self::PageTableFormat`], stay unchanged (but for mappings added to
    /// the device window, see [`Self::make_kernel_mappings_visible`]) and allocated for as long
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

    /// Whether programs run with a view of the kernel half that maps only what entering and
    /// leaving the kernel needs (page-table isolation, KPTI, D502): on CPUs that let a program
    /// read kernel memory speculatively (Meltdown). `false` by default.
    fn isolates_user_view() -> bool {
        false
    }

    /// Calls `range` with each kernel range this CPU's user view maps, by virtual start and
    /// length: the entry and exit code, the interrupt tables and this CPU's block (its GDT, TSS,
    /// entry and exception stacks). Nothing by default.
    fn user_view_ranges(range: &mut dyn FnMut(u64, u64)) {
        let _ = range;
    }

    /// Gives this CPU its user view: page tables whose root, at physical `root`, maps the ranges
    /// [`Self::user_view_ranges`] named in its kernel half; [`Self::run_user`] fills its user
    /// half from the active tables' at every entry. Nothing by default.
    ///
    /// # Safety
    /// `root` must be such a root, kept for as long as the kernel runs, and this CPU's block set
    /// ([`Self::set_cpu_local`]); interrupts must be masked.
    unsafe fn set_user_view(root: u64) {
        let _ = root;
    }

    /// The user-mode test program for the boot self-tests and benchmarks (D47).
    fn user_test_program() -> user::TestProgram;

    /// What user programs may run beyond the base instruction set, as Linux's `AT_HWCAP` and
    /// `AT_HWCAP2` encode it for the architecture (D51): only what user mode really runs as
    /// the kernel sets it up. FP/SIMD, which programs get on first use (D54): FP and ASIMD on
    /// AArch64 (no SVE or SME), with the ARMv8.1 atomics when the processor has them (D151), RV64GC's letters with F and D on RISC-V (no V), and on x86_64
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
        unsafe { &*((Self::cpu_local() + OFFSET) as *const crate::atomic::AtomicU64) }
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
