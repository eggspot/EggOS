//! 64-bit little-endian PowerPC on POWER9 bare metal (QEMU `powernv9` with skiboot's OPAL
//! firmware; D297, D304).
//!
//! Boot contract: skiboot copies the image to memory and enters `_start` in hypervisor real
//! mode, big-endian, with `r3` the device tree's physical address, `r8` OPAL's base and `r9`
//! its entry point. The entry code turns the CPU little-endian (Linux's `FIXUP_ENDIAN`), moves
//! the image to the physical address it is linked for if it is elsewhere, builds the boot
//! radix tree (the first 512 GiB of physical memory at `0xc000...`, 1 GiB pages), the partition
//! and process tables, sets `LPCR` for radix with relocation-on interrupts at `0xc000...4000`
//! (`AIL` 3), copies the interrupt vectors to physical `0x4000` and turns translation on with
//! `rfid`.
//!
//! The kernel is linked at `0xc000_0000_2001_0000`, where the direct map puts its physical
//! address: kernel and direct map are one (quadrant 3 of the radix tree), as on Linux. The
//! per-CPU block is in `r13`, which the ABI reserves; each CPU's exception scratch area is
//! in `HSPRG0`. The console, power and the start of other CPUs are OPAL calls.

mod exception;
mod fp;
mod opal;
mod pci;
mod thread;
mod timer;
mod xive;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

core::arch::global_asm!(
    r#"
    .macro FIXUP_ENDIAN
    tdi         0, 0, 0x48
    b           191f
    .long       0xa600607d
    .long       0x01006b69
    .long       0x00004039
    .long       0x6401417d
    .long       0x05009f42
    .long       0xa602487d
    .long       0x14004a39
    .long       0xa6035a7d
    .long       0xa6037b7d
    .long       0x2400004c
191:
    .endm

    .macro LOAD64 reg, value
    lis         \reg, ((\value) >> 48) & 0xffff
    ori         \reg, \reg, ((\value) >> 32) & 0xffff
    sldi        \reg, \reg, 32
    oris        \reg, \reg, ((\value) >> 16) & 0xffff
    ori         \reg, \reg, (\value) & 0xffff
    .endm

    .macro PHYS reg, anchor, sym
    addis       \reg, %r27, (\sym - \anchor)@ha
    addi        \reg, \reg, (\sym - \anchor)@l
    .endm

    .section .text.boot, "ax"
    .global _start
_start:
    FIXUP_ENDIAN
    mr          %r31, %r3
    mr          %r30, %r8
    mr          %r29, %r9
    bcl         20, 31, ppc64_boot_anchor
ppc64_boot_anchor:
    mflr        %r28
    ld          %r27, (ppc64_boot_linked - ppc64_boot_anchor)(%r28)
    LOAD64      %r26, {offset}
    sub         %r27, %r27, %r26
    cmpd        %r27, %r28
    beq         3f

    // Not where it is linked: copy the loaded image there, flush it, and go on there.
    addi        %r4, %r28, (_start - ppc64_boot_anchor)
    addi        %r5, %r27, (_start - ppc64_boot_anchor)
    ld          %r6, (ppc64_boot_size - ppc64_boot_anchor)(%r28)
    cmpld       %r5, %r4
    bgt         5f
    li          %r7, 0
4:  cmpld       %r7, %r6
    bge         6f
    ldx         %r8, %r4, %r7
    stdx        %r8, %r5, %r7
    addi        %r7, %r7, 8
    b           4b
5:  mr          %r7, %r6
7:  cmpdi       %r7, 0
    ble         6f
    addi        %r7, %r7, -8
    ldx         %r8, %r4, %r7
    stdx        %r8, %r5, %r7
    b           7b
6:  mr          %r3, %r5
    mr          %r4, %r6
    bl          ppc64_flush_physical
    addi        %r8, %r27, (3f - ppc64_boot_anchor)
    mtctr       %r8
    bctr

    // From here on the code runs at its linked physical address; r27 is that of the anchor.
3:  PHYS        %r4, ppc64_boot_anchor, __bss_start
    PHYS        %r5, ppc64_boot_anchor, __bss_end
    li          %r6, 0
8:  cmpld       %r4, %r5
    bge         9f
    std         %r6, 0(%r4)
    addi        %r4, %r4, 8
    b           8b

9:  PHYS        %r4, ppc64_boot_anchor, {device_tree}
    std         %r31, 0(%r4)
    PHYS        %r4, ppc64_boot_anchor, {opal_base}
    std         %r30, 0(%r4)
    PHYS        %r4, ppc64_boot_anchor, {opal_entry}
    std         %r29, 0(%r4)

    // The interrupt vectors, to physical 0x4000, where AIL 3 takes interrupts at 0xc000...4000.
    PHYS        %r4, ppc64_boot_anchor, ppc64_vectors_start
    li          %r5, {vectors_physical}
    li          %r6, 0
    li          %r7, {vectors_size}
10: cmpld       %r6, %r7
    bge         11f
    ldx         %r8, %r4, %r6
    stdx        %r8, %r5, %r6
    addi        %r6, %r6, 8
    b           10b
11: mr          %r3, %r5
    mr          %r4, %r7
    bl          ppc64_flush_physical

    // The boot radix tree, entries big-endian: the root's entry 0 to the level-3 table, its
    // entry 0 to the level-2 table, whose 512 entries map 1 GiB each, read, write, execute.
    PHYS        %r4, ppc64_boot_anchor, {boot_root}
    PHYS        %r5, ppc64_boot_anchor, {boot_l3}
    PHYS        %r6, ppc64_boot_anchor, {boot_l2}
    LOAD64      %r7, {directory}
    or          %r8, %r7, %r5
    stdbrx      %r8, 0, %r4
    or          %r8, %r7, %r6
    stdbrx      %r8, 0, %r5
    LOAD64      %r7, {gigabyte_leaf}
    li          %r8, 1
    sldi        %r8, %r8, 30
    li          %r9, 512
    mtctr       %r9
12: stdbrx      %r7, 0, %r6
    add         %r7, %r7, %r8
    addi        %r6, %r6, 8
    bdnz        12b

    // Partition table entry 0: this tree, host radix; the process table. Process table entry
    // 0: the kernel's tree, for quadrant 3.
    PHYS        %r10, ppc64_boot_anchor, {partition_table}
    PHYS        %r11, ppc64_boot_anchor, {process_table}
    LOAD64      %r7, {partition_entry}
    or          %r8, %r7, %r4
    stdbrx      %r8, 0, %r10
    li          %r9, 8
    stdbrx      %r11, %r10, %r9
    LOAD64      %r7, {process_entry}
    or          %r8, %r7, %r4
    stdbrx      %r8, 0, %r11

    bl          ppc64_radix_on
    ld          %r9, (ppc64_boot_virtual - ppc64_boot_anchor)(%r27)
    mtspr       26, %r9
    LOAD64      %r9, {msr_kernel}
    mtspr       27, %r9
    rfid

    // Translation on: the kernel's TOC and stack, this CPU's scratch area, then the kernel.
ppc64_boot_continue:
    bcl         20, 31, 13f
13: mflr        %r11
    ld          %r2, (ppc64_boot_toc - 13b)(%r11)
    ld          %r1, (ppc64_boot_stack - 13b)(%r11)
    li          %r0, 0
    stdu        %r0, -32(%r1)
    bl          ppc64_scratch_setup
    bl          {init_timer}
    nop
    bl          kernel_main
    nop
14: b           14b

    // Sets PTCR, LPCR (host radix, process tables in use, AIL 3, external interrupts to the
    // hypervisor, no hypervisor decrementer) and PIDR 0, and drops every translation. r10:
    // the partition table's physical address. Clobbers r9, r10, r12.
ppc64_radix_on:
    mtspr       464, %r10
    mfspr       %r12, 318
    LOAD64      %r9, {lpcr_set}
    or          %r12, %r12, %r9
    li          %r9, {lpcr_clear}
    andc        %r12, %r12, %r9
    mtspr       318, %r12
    li          %r9, 0
    mtspr       48, %r9
    mtspr       319, %r9
    isync
    li          %r9, 0xc00
    li          %r10, 0
    ptesync
    tlbie       %r9, %r10, 2, 1, 1
    eieio
    tlbsync
    ptesync
    blr

    // Writes back and invalidates the caches over the r4 bytes at physical r3. Clobbers r5, r6.
ppc64_flush_physical:
    li          %r5, 0
1:  cmpld       %r5, %r4
    bge         2f
    add         %r6, %r3, %r5
    dcbst       0, %r6
    addi        %r5, %r5, 128
    b           1b
2:  sync
    li          %r5, 0
3:  cmpld       %r5, %r4
    bge         4f
    add         %r6, %r3, %r5
    icbi        0, %r6
    addi        %r5, %r5, 128
    b           3b
4:  sync
    isync
    blr

    // HSPRG0 = this CPU's exception scratch area (by PIR), holding the common entry's address
    // and the kernel's TOC. Clobbers r9 to r12.
ppc64_scratch_setup:
    mflr        %r10
    bcl         20, 31, 1f
1:  mflr        %r11
    mtlr        %r10
    ld          %r12, (ppc64_boot_scratch - 1b)(%r11)
    mfspr       %r9, 1023
    andi.       %r9, %r9, {scratch_mask}
    sldi        %r9, %r9, {scratch_shift}
    add         %r12, %r12, %r9
    mtspr       304, %r12
    ld          %r9, (ppc64_boot_common - 1b)(%r11)
    std         %r9, 16(%r12)
    std         %r2, 32(%r12)
    blr

    // A secondary CPU, started by OPAL_START_CPU in hypervisor real mode, big-endian, with r3
    // its PIR; SECONDARY_STARTS[PIR] holds the kernel virtual address of its `CpuStart`.
    .global eggos_secondary_start
eggos_secondary_start:
    FIXUP_ENDIAN
    mr          %r31, %r3
    bcl         20, 31, ppc64_secondary_anchor
ppc64_secondary_anchor:
    mflr        %r27
    PHYS        %r4, ppc64_secondary_anchor, {secondary_starts}
    andi.       %r5, %r31, {scratch_mask}
    sldi        %r5, %r5, 3
    ldx         %r30, %r4, %r5
    PHYS        %r10, ppc64_secondary_anchor, {partition_table}
    bl          ppc64_radix_on
    ld          %r9, (ppc64_secondary_virtual - ppc64_secondary_anchor)(%r27)
    mtspr       26, %r9
    LOAD64      %r9, {msr_kernel}
    mtspr       27, %r9
    rfid
ppc64_secondary_continue:
    bcl         20, 31, 1f
1:  mflr        %r11
    ld          %r2, (ppc64_boot_toc - 1b)(%r11)
    ld          %r1, {start_stack_top}(%r30)
    li          %r0, 0
    stdu        %r0, -32(%r1)
    bl          ppc64_scratch_setup
    mr          %r3, %r30
    bl          {secondary_main}
    nop
2:  b           2b


    .pushsection .text.opal, "ax"
    .balign 4
    .global ppc64_opal_call
ppc64_opal_call:
    mflr        %r0
    std         %r0, 16(%r1)
    std         %r2, 24(%r1)
    stdu        %r1, -64(%r1)
    std         %r30, 32(%r1)
    std         %r31, 40(%r1)
    mr          %r0, %r3
    mr          %r3, %r4
    mr          %r4, %r5
    mr          %r5, %r6
    mr          %r6, %r7
    mr          %r7, %r8
    mr          %r8, %r9
    mr          %r9, %r10
    mfmsr       %r31
    bcl         20, 31, 1f
1:  mflr        %r10
    ld          %r30, (2f - 1b)(%r10)
    addi        %r12, %r10, (ppc64_opal_return - 1b)
    LOAD64      %r11, {offset}
    sub         %r12, %r12, %r11
    mtlr        %r12
    addis       %r12, %r2, {opal_entry}@toc@ha
    ld          %r12, {opal_entry}@toc@l(%r12)
    mtspr       314, %r12
    LOAD64      %r12, {msr_opal}
    mtspr       315, %r12
    addis       %r11, %r2, {opal_base}@toc@ha
    ld          %r2, {opal_base}@toc@l(%r11)
    hrfid
ppc64_opal_return:
    FIXUP_ENDIAN
    mtspr       314, %r30
    mtspr       315, %r31
    hrfid
    .balign 8
2:  .quad       3f
3:  ld          %r30, 32(%r1)
    ld          %r31, 40(%r1)
    addi        %r1, %r1, 64
    ld          %r2, 24(%r1)
    ld          %r0, 16(%r1)
    mtlr        %r0
    blr
    .popsection

    .section .text.boot, "ax"
    .balign 8
ppc64_boot_linked:      .quad ppc64_boot_anchor
ppc64_boot_size:        .quad __file_end - _start
ppc64_boot_virtual:     .quad ppc64_boot_continue
ppc64_secondary_virtual: .quad ppc64_secondary_continue
ppc64_boot_toc:         .quad .TOC.
ppc64_boot_stack:       .quad __stack_top
ppc64_boot_scratch:     .quad {scratch}
ppc64_boot_common:      .quad ppc64_common_entry
"#,
    offset = const KERNEL_VIRTUAL_OFFSET,
    device_tree = sym DEVICE_TREE,
    opal_base = sym opal::OPAL_BASE,
    opal_entry = sym opal::OPAL_ENTRY,
    vectors_physical = const exception::VECTORS_PHYSICAL,
    vectors_size = const exception::VECTORS_SIZE,
    boot_root = sym BOOT_ROOT,
    boot_l3 = sym BOOT_L3,
    boot_l2 = sym BOOT_L2,
    directory = const DIRECTORY_ENTRY,
    gigabyte_leaf = const BOOT_GIGABYTE_LEAF,
    partition_table = sym PARTITION_TABLE,
    process_table = sym PROCESS_TABLE,
    partition_entry = const PARTITION_ENTRY,
    process_entry = const PROCESS_ENTRY,
    msr_kernel = const msr::KERNEL,
    msr_opal = const msr::OPAL,
    lpcr_set = const lpcr::HOST_RADIX | lpcr::PROCESS_TABLES | lpcr::AIL_3 | lpcr::HYPERVISOR_VIRTUALIZATION_INTERRUPTS,
    lpcr_clear = const lpcr::EXTERNAL_TO_GUEST | lpcr::HYPERVISOR_DECREMENTER,
    init_timer = sym timer::init,
    scratch = sym exception::SCRATCH,
    scratch_mask = const exception::MAX_CPUS - 1,
    scratch_shift = const exception::SCRATCH_SHIFT,
    secondary_starts = sym SECONDARY_STARTS,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    secondary_main = sym secondary_main,
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xc000_0000_2001_0000`, physical `0x2001_0000`, in quadrant 3 with the direct map.
/// `linker.ld` uses the same value and checks it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xc000_0000_0000_0000;

/// The direct map: physical address `p` at `DIRECT_MAP_OFFSET + p`, the kernel's own place.
const DIRECT_MAP_OFFSET: usize = KERNEL_VIRTUAL_OFFSET as usize;
/// The physical memory the boot direct map covers: 512 GiB.
const BOOT_DIRECT_MAP_END: u64 = 512 << 30;
/// Below it: the interrupt vectors (at 0x4000) and what the firmware keeps.
const LOW_RESERVED: u64 = 0x1_0000;
/// The room OPAL keeps at its base, which the device tree also reserves.
const OPAL_RESERVED: u64 = 32 << 20;
/// QEMU `powernv`'s RAM without a device tree: the first 1 GiB.
const DEFAULT_RAM_END: u64 = 1 << 30;

/// `MSR` values (Power ISA 3.0B, Book III, 4.2.1).
pub(super) mod msr {
    pub const SIXTY_FOUR_BIT: u64 = 1 << 63;
    pub const HYPERVISOR: u64 = 1 << 60;
    pub const VECTOR: u64 = 1 << 25;
    pub const VSX: u64 = 1 << 23;
    pub const EXTERNAL_ENABLE: u64 = 1 << 15;
    pub const PROBLEM_STATE: u64 = 1 << 14;
    pub const FLOATING_POINT: u64 = 1 << 13;
    pub const MACHINE_CHECK: u64 = 1 << 12;
    pub const INSTRUCTION_RELOCATE: u64 = 1 << 5;
    pub const DATA_RELOCATE: u64 = 1 << 4;
    pub const RECOVERABLE: u64 = 1 << 1;
    pub const LITTLE_ENDIAN: u64 = 1 << 0;
    /// The kernel: hypervisor, translation on, little-endian, interrupts masked.
    pub const KERNEL: u64 = SIXTY_FOUR_BIT
        | HYPERVISOR
        | MACHINE_CHECK
        | INSTRUCTION_RELOCATE
        | DATA_RELOCATE
        | RECOVERABLE
        | LITTLE_ENDIAN;
    /// OPAL: hypervisor real mode, big-endian, interrupts masked.
    pub const OPAL: u64 = SIXTY_FOUR_BIT | HYPERVISOR | MACHINE_CHECK;
    /// A program: problem state, translation on, interrupts enabled.
    pub const USER: u64 = SIXTY_FOUR_BIT
        | EXTERNAL_ENABLE
        | PROBLEM_STATE
        | MACHINE_CHECK
        | INSTRUCTION_RELOCATE
        | DATA_RELOCATE
        | RECOVERABLE
        | LITTLE_ENDIAN;
    /// The facilities a program gets once it used floating point or vectors (D54).
    pub const USER_FP: u64 = FLOATING_POINT | VECTOR | VSX;
}

/// `LPCR` bits the HAL sets or clears.
mod lpcr {
    pub const HOST_RADIX: u64 = 1 << 20;
    pub const PROCESS_TABLES: u64 = 1 << 22;
    pub const AIL_3: u64 = 3 << 23;
    pub const EXTERNAL_TO_GUEST: u64 = 1 << 3;
    pub const HYPERVISOR_DECREMENTER: u64 = 1 << 0;
    /// `HVICE`: XIVE's physical interrupts come as hypervisor virtualization interrupts (0xea0).
    pub const HYPERVISOR_VIRTUALIZATION_INTERRUPTS: u64 = 1 << 1;
}

/// A radix directory entry's flags: valid, 9 index bits at the next level.
const DIRECTORY_ENTRY: u64 = (1 << 63) | 9;
/// A boot leaf of 1 GiB at physical 0: valid, leaf, read, write, execute, privileged,
/// referenced and changed.
const BOOT_GIGABYTE_LEAF: u64 = (1 << 63) | (1 << 62) | 0x18f;
/// Partition table entry 0's first doubleword but the root: host radix, 52 bits, 4 root bits.
const PARTITION_ENTRY: u64 = (1 << 63) | paging::powerpc64::RADIX_TREE_SIZE | paging::powerpc64::ROOT_INDEX_BITS;
/// A process table entry's first doubleword but the root.
const PROCESS_ENTRY: u64 = paging::powerpc64::RADIX_TREE_SIZE | paging::powerpc64::ROOT_INDEX_BITS;

#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

/// The partition table (one entry used) and the process table: entry 0 the kernel's tree,
/// entry `1 + cpu` the program running on that CPU (D304).
#[repr(C, align(4096))]
pub(super) struct ProcessTable(pub(super) [[u64; 2]; 256]);

/// Built by `_start` before any Rust code runs.
static mut BOOT_ROOT: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L3: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L2: BootTable = BootTable([0; paging::ENTRIES]);
static mut PARTITION_TABLE: ProcessTable = ProcessTable([[0; 2]; 256]);
pub(super) static mut PROCESS_TABLE: ProcessTable = ProcessTable([[0; 2]; 256]);

/// The device tree's physical address, from `r3` at entry; 0 for none.
static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);

/// Each secondary CPU's `CpuStart`, by PIR, until it runs.
static SECONDARY_STARTS: [AtomicU64; exception::MAX_CPUS] =
    [const { AtomicU64::new(0) }; exception::MAX_CPUS];

/// Called by `eggos_secondary_start` on the `CpuStart`'s stack, translated with the boot tree:
/// switches to the kernel's tables, sets this CPU's timer up (D41), then calls the entry.
extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now; interrupts are masked (`MSR.EE` is
    // clear since the entry).
    unsafe { Powerpc64::activate_page_tables(&start.page_table_roots[..2]) };
    timer::init();
    (start.entry)(start.argument)
}

/// The device tree's physical address, or 0 when there is none or it lies beyond the boot
/// direct map.
fn device_tree() -> usize {
    let address = DEVICE_TREE.load(Ordering::Relaxed);
    if address as u64 > BOOT_DIRECT_MAP_END - boot_memory::MAX_DEVICE_TREE_SIZE as u64 {
        return 0;
    }
    address
}

/// This CPU's processor id register.
pub(super) fn pir() -> u64 {
    let value: u64;
    // SAFETY: reading `PIR` has no side effects.
    unsafe { core::arch::asm!("mfspr {0}, 1023", out(reg) value, options(nomem, nostack)) };
    value
}

/// This CPU's process id for programs: `1 + PIR`, its own process table entry.
pub(super) fn program_pid() -> u64 {
    1 + (pir() & (exception::MAX_CPUS as u64 - 1))
}

#[derive(Debug)]
pub struct Powerpc64;

impl Platform for Powerpc64 {
    const ARCH: &'static str = "ppc64le";

    #[inline]
    fn console_write(bytes: &[u8]) {
        opal::console_write(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        opal::console_read(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        Err("console input is polled through OPAL (D312)")
    }

    fn route_device_interrupt(
        _specifier: &crate::fdt::InterruptSpecifier,
    ) -> Result<crate::DeviceLine, &'static str> {
        Err("POWER devices use message interrupts (D312)")
    }

    fn message_interrupt() -> Result<(crate::DeviceLine, crate::interrupt::InterruptMessage), &'static str> {
        xive::message()
    }

    fn firmware_pci() -> Option<&'static dyn eggos_drivers::pci::ConfigSpace> {
        pci::firmware_pci(Self::device_tree())
    }

    fn dma_device_offset() -> u64 {
        pci::dma_offset()
    }

    fn exit(status: ExitStatus) -> ! {
        let _ = status;
        opal::power_down();
        Self::halt()
    }

    fn restart() -> ! {
        opal::reboot();
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            timer::stop();
        }
    }

    fn breakpoint() {
        // SAFETY: `tw 31, 0, 0` always traps to the program interrupt, whose entry saves and
        // restores every register and resumes after it. No `nomem`/`nostack`: the kernel's
        // handler may read and write any memory, and its frame is pushed below the stack
        // pointer's red zone.
        unsafe { core::arch::asm!("tw 31, 0, 0") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: the all-zero word is no instruction; it traps to the entry installed at boot,
        // and the kernel never resumes from that exception.
        unsafe { core::arch::asm!(".long 0", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::FREQUENCY
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        timer::stop();
        Self::interrupt_window();
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: `mtmsrd` with L=1 changes only `EE` and `RI`: interrupts are taken between
        // the two, through the entry installed at boot, which restores every register.
        unsafe {
            core::arch::asm!(
                "li {value}, {enable}",
                "mtmsrd {value}, 1",
                "li {value}, {recoverable}",
                "mtmsrd {value}, 1",
                value = out(reg) _,
                enable = const msr::EXTERNAL_ENABLE | msr::RECOVERABLE,
                recoverable = const msr::RECOVERABLE,
            )
        };
    }

    fn memory_map() -> MemoryMap {
        let mut map = match device_tree() {
            // SAFETY: the address came from skiboot in r3; its device tree stays readable and
            // unwritten in RAM inside the boot direct map (`device_tree` checks).
            address @ 1.. => unsafe { boot_memory::device_tree_memory_map(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange {
                    start: 0,
                    end: DEFAULT_RAM_END,
                });
                map.reserve(boot_memory::kernel_image());
                map
            }
        };
        map.reserve(PhysicalRange {
            start: 0,
            end: LOW_RESERVED,
        });
        let base = opal::base();
        if base != 0 {
            map.reserve(PhysicalRange::from_start_length(base, OPAL_RESERVED));
        }
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    type PageTableFormat = paging::powerpc64::Powerpc64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        let mut map = match device_tree() {
            // SAFETY: as in `memory_map`.
            address @ 1.. => unsafe { boot_memory::device_tree_ram(address) },
            0 => {
                let mut map = MemoryMap::new();
                map.add_ram(PhysicalRange {
                    start: 0,
                    end: DEFAULT_RAM_END,
                });
                map
            }
        };
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        xive::prepare(Self::device_tree(), region);
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(device_tree()) }
    }

    fn command_line(line: &mut dyn FnMut(&[u8])) {
        if let Some(arguments) = Self::device_tree().and_then(crate::fdt::boot_arguments) {
            line(arguments);
        }
    }

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: `ptesync` orders the table writes before later walks, and `tlbie` with IS 2,
        // RIC 2, PRS 1, R 1 drops every process-scoped translation and the page walk cache of
        // this partition; none changes memory contents.
        unsafe {
            core::arch::asm!(
                "ptesync",
                "tlbie {rb}, {rs}, 2, 1, 1",
                "eieio",
                "tlbsync",
                "ptesync",
                rb = in(reg) 0x800u64,
                rs = in(reg) 0u64,
                options(nostack),
            )
        };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("POWER9 radix page tables have two roots");
        };
        // SAFETY: entry 0 is the kernel's tree, read by this CPU's walks only after the
        // invalidation below; the other entries belong to the CPUs' programs.
        unsafe {
            let table = &raw mut PROCESS_TABLE;
            (*table).0[0][0] = paging::powerpc64::table_entry(*high);
            (*table).0[program_pid() as usize][0] = paging::powerpc64::table_entry(*low);
        }
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on
        // at the addresses it uses now; the invalidation drops what the boot tree left (and
        // the process table entries cached), and `PIDR` picks this CPU's entry for programs.
        unsafe {
            core::arch::asm!(
                "ptesync",
                "tlbie {rb}, {rs}, 2, 1, 1",
                "eieio",
                "tlbsync",
                "ptesync",
                "mtspr 48, {pid}",
                "isync",
                rb = in(reg) 0x800u64,
                rs = in(reg) 0u64,
                pid = in(reg) program_pid(),
                options(nostack),
            )
        };
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let Some(low) = roots.first() else {
            panic!("POWER9 radix page tables have two roots");
        };
        let pid = program_pid();
        // SAFETY: only this CPU uses process table entry `pid`; the invalidation by PID with
        // RIC 2 drops its translations, page walk cache and the cached entry, so the next
        // program walk reads the new root.
        unsafe {
            let table = &raw mut PROCESS_TABLE;
            (*table).0[pid as usize][0] = paging::powerpc64::table_entry(*low);
            core::arch::asm!(
                "ptesync",
                "tlbie {rb}, {rs}, 2, 1, 1",
                "eieio",
                "tlbsync",
                "ptesync",
                "isync",
                rb = in(reg) 0x400u64,
                rs = in(reg) pid << 32,
                options(nostack),
            )
        };
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        access()
    }

    unsafe fn run_user(context: &mut crate::user::UserContext) -> crate::user::UserExit {
        // SAFETY: the caller's guarantees are `run_user`'s.
        unsafe { exception::run_user(context) }
    }

    fn user_test_program() -> crate::user::TestProgram {
        exception::user_test_program()
    }

    fn linux_hardware_capabilities() -> [u64; 2] {
        const TRUE_LITTLE_ENDIAN: u64 = 0x0000_0002;
        const VSX: u64 = 0x0000_0080;
        const MMU: u64 = 0x0400_0000;
        const FPU: u64 = 0x0800_0000;
        const ALTIVEC: u64 = 0x1000_0000;
        const SIXTY_FOUR_BIT: u64 = 0x4000_0000;
        const ARCH_2_07: u64 = 0x8000_0000;
        const ARCH_3_00: u64 = 0x0080_0000;
        [
            TRUE_LITTLE_ENDIAN | VSX | MMU | FPU | ALTIVEC | SIXTY_FOUR_BIT,
            ARCH_2_07 | ARCH_3_00,
        ]
    }

    fn make_instructions_visible(start: usize, length: usize) {
        const LINE: usize = 128;
        let first = start & !(LINE - 1);
        let end = start + length;
        let mut line = first;
        while line < end {
            // SAFETY: `dcbst` writes the line back; it changes no contents.
            unsafe { core::arch::asm!("dcbst 0, {}", in(reg) line, options(nostack)) };
            line += LINE;
        }
        // SAFETY: a barrier only.
        unsafe { core::arch::asm!("sync", options(nostack)) };
        line = first;
        while line < end {
            // SAFETY: `icbi` drops the line from the instruction cache; it changes no contents.
            unsafe { core::arch::asm!("icbi 0, {}", in(reg) line, options(nostack)) };
            line += LINE;
        }
        // SAFETY: barriers only.
        unsafe { core::arch::asm!("sync", "isync", options(nostack)) };
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(code)
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pid = program_pid();
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: drops every translation of this CPU's program PID; no memory changes.
            unsafe {
                core::arch::asm!(
                    "ptesync",
                    "tlbie {rb}, {rs}, 0, 1, 1",
                    "eieio",
                    "tlbsync",
                    "ptesync",
                    rb = in(reg) 0x400u64,
                    rs = in(reg) pid << 32,
                    options(nostack),
                )
            };
            return;
        }
        // SAFETY: a barrier only, ordering the caller's table writes before the invalidations.
        unsafe { core::arch::asm!("ptesync", options(nostack)) };
        for page in 0..pages {
            let address = start + page * crate::memory::FRAME_SIZE;
            // SAFETY: drops one 4 KiB translation (IS 0, AP 0) of this CPU's program PID.
            unsafe {
                core::arch::asm!(
                    "tlbie {rb}, {rs}, 0, 1, 1",
                    rb = in(reg) address,
                    rs = in(reg) pid << 32,
                    options(nostack),
                )
            };
        }
        // SAFETY: barriers only.
        unsafe { core::arch::asm!("eieio", "tlbsync", "ptesync", options(nostack)) };
    }

    unsafe fn prepare_thread(
        stack_top: usize,
        entry: extern "C" fn(usize) -> !,
        argument: usize,
    ) -> usize {
        // SAFETY: the frame, with the rounding down to 16 bytes, fits in `INITIAL_FRAME_MAX`
        // bytes, which the caller guarantees are writable and unused below `stack_top`.
        unsafe {
            crate::thread::write_initial_frame(stack_top, &thread::initial_frame(entry, argument))
        }
    }

    #[inline]
    unsafe fn switch_thread(save: *mut usize, resume: usize) {
        // SAFETY: the caller guarantees `save` is writable and `resume` a saved context whose
        // stack is allocated and not running; the switch saves and restores every register the
        // calling convention requires a call to preserve.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        if device_tree() != 0 {
            // SAFETY: as in `memory_map`; called once at boot, on the boot CPU.
            unsafe { boot_memory::device_tree_take_seeds(device_tree(), seed) };
        }
    }

    unsafe fn set_user_fp(on: bool) {
        // SAFETY: the caller's contract is `set_user`'s.
        unsafe { fp::set_user(on) }
    }

    unsafe fn save_fp(state: *mut crate::fp::FpState) {
        // SAFETY: the caller's contract is `save`'s.
        unsafe { fp::save(state) }
    }

    unsafe fn restore_fp(state: *const crate::fp::FpState) {
        // SAFETY: the caller's contract is `restore`'s.
        unsafe { fp::restore(state) }
    }

    fn cpus(cpu: &mut dyn FnMut(u64)) {
        match device_tree() {
            0 => cpu(Self::boot_cpu()),
            // SAFETY: as in `memory_map`.
            address => unsafe { boot_memory::device_tree_cpus(address, cpu) },
        }
    }

    fn boot_cpu() -> u64 {
        pir()
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            /// The secondary entry point in the boot code above.
            fn eggos_secondary_start();
        }
        if cpu >= exception::MAX_CPUS as u64 {
            return Err(StartCpuError::InvalidCpu);
        }
        SECONDARY_STARTS[cpu as usize].store(core::ptr::from_ref(start) as u64, Ordering::SeqCst);
        let entry = (eggos_secondary_start as *const () as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET);
        match opal::start_cpu(cpu, entry) {
            true => Ok(()),
            false => Err(StartCpuError::InvalidCpu),
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        exception::set_kernel_r13(block);
        // SAFETY: `r13` is reserved by the ABI, so compiled code never touches it; the entry
        // from user mode puts the kernel's value back (D304).
        unsafe { core::arch::asm!("mr 13, {}", in(reg) block, options(nomem, nostack)) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading `r13` has no side effects.
        unsafe { core::arch::asm!("mr {}, 13", out(reg) block, options(nomem, nostack)) };
        block
    }

    #[inline]
    unsafe fn cpu_local_u64<const OFFSET: usize>() -> u64 {
        let value: u64;
        // SAFETY: the caller guarantees `r13` holds this CPU's block, with an aligned word at
        // `OFFSET`: one aligned 8-byte load, which never tears.
        unsafe {
            core::arch::asm!(
                "ld {}, {offset}(13)",
                out(reg) value,
                offset = const OFFSET,
                options(nostack, readonly, preserves_flags),
            );
        }
        value
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
}

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]) in 128-byte lines.
///
/// # Safety
/// As for [`crate::memory::zero`].
#[inline]
pub(crate) unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    unsafe { crate::memory::zero_by_lines(start, length) }
}

#[inline]
pub(crate) fn doorbell_barrier() {
    // SAFETY: a barrier: it touches no memory or register, only orders stores.
    unsafe { core::arch::asm!("sync", options(nostack, preserves_flags)) };
}

/// The C library's memory functions, which the Linux target's prebuilt `core` calls and which
/// no C library provides in the kernel (D304). Byte loops: the kernel never uses FP or vector
/// registers, so these must not either.
mod memory_functions {
    /// # Safety
    /// As C's `memcpy`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcpy(destination: *mut u8, source: *const u8, length: usize) -> *mut u8 {
        let mut index = 0;
        while index < length {
            // SAFETY: the caller guarantees both ranges are valid for `length` bytes.
            unsafe { destination.add(index).write_volatile(source.add(index).read()) };
            index += 1;
        }
        destination
    }

    /// # Safety
    /// As C's `memmove`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memmove(destination: *mut u8, source: *const u8, length: usize) -> *mut u8 {
        if (destination as usize) <= (source as usize) {
            // SAFETY: forward copy is safe when the destination is below the source.
            return unsafe { memcpy(destination, source, length) };
        }
        let mut index = length;
        while index > 0 {
            index -= 1;
            // SAFETY: the caller guarantees both ranges are valid for `length` bytes.
            unsafe { destination.add(index).write_volatile(source.add(index).read()) };
        }
        destination
    }

    /// # Safety
    /// As C's `memset`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memset(destination: *mut u8, value: i32, length: usize) -> *mut u8 {
        let mut index = 0;
        while index < length {
            // SAFETY: the caller guarantees the range is valid for `length` bytes.
            unsafe { destination.add(index).write_volatile(value as u8) };
            index += 1;
        }
        destination
    }

    /// # Safety
    /// As C's `memcmp`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcmp(first: *const u8, second: *const u8, length: usize) -> i32 {
        let mut index = 0;
        while index < length {
            // SAFETY: the caller guarantees both ranges are valid for `length` bytes.
            let (a, b) = unsafe { (first.add(index).read(), second.add(index).read()) };
            if a != b {
                return i32::from(a) - i32::from(b);
            }
            index += 1;
        }
        0
    }

    /// # Safety
    /// As C's `bcmp`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn bcmp(first: *const u8, second: *const u8, length: usize) -> i32 {
        // SAFETY: as `memcmp`.
        unsafe { memcmp(first, second, length) }
    }
}
