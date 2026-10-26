//! AArch64 (Armv8-A and later).
//!
//! Boot contract: entered at `_start` in EL1 with the MMU off, as QEMU's `virt` machine does
//! for an ELF kernel. The kernel is built for `aarch64-unknown-none-softfloat` (decision D10):
//! no FP/SIMD in the kernel, and `strict-align`, which was required while the MMU was off. Since
//! D33 only the entry code below runs with the MMU off: it builds the boot page tables, turns
//! the MMU on and jumps to the kernel's virtual address before any Rust code runs.
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod fp;
mod thread;
mod timer;

use crate::arch::boot_memory;
use crate::cpu::{CpuStart, StartCpuError};
use crate::memory::PhysicalRange;
use crate::paging::{self, Attributes, KernelImage, MemoryType};
use crate::{ExitStatus, MemoryMap, Platform};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Pl011, Uart, pl011};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    // Only the CPU whose affinity (MPIDR_EL1 Aff0-Aff3) is all zero boots for now; park
    // every other CPU, in any cluster.
    mrs     x0, mpidr_el1
    and     x1, x0, #0xffffff
    ubfx    x2, x0, #32, #8
    orr     x0, x1, x2
    cbnz    x0, 6f

    // Use SP_EL1 at EL1, so exceptions taken to EL1 run on the boot stack set up below.
    msr     spsel, #1

    // The MMU is off, so `adrp` (PC-relative) gives physical addresses until the jump below.
    // Zero .bss (8-byte aligned and sized by the linker script); the boot page tables are there.
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
1:  cmp     x0, x1
    b.hs    2f
    str     xzr, [x0], #8
    b       1b

    // The boot page tables (D33), with 1 GiB blocks: the gigabyte holding the kernel, read,
    // write and execute, at its physical address through TTBR0 (only for the jump below and
    // the switch to the kernel's own tables) and at the kernel's virtual address through
    // TTBR1, and the boot direct map: the first {direct_map_gigabytes} GiB of physical
    // memory, never executable, at the direct map offset; device memory below the gigabyte
    // RAM starts in, normal memory from there on.
2:  adrp    x0, {l0_low}
    adrp    x1, {l1_low}
    adrp    x2, {l0_high}
    adrp    x3, {l1_direct}
    adrp    x4, {l1_kernel}
    adrp    x9, __kernel_start      // physical
    ldr     x13, =__kernel_start    // virtual
    ubfx    x10, x9, #39, #9
    orr     x6, x1, #{table}
    str     x6, [x0, x10, lsl #3]
    lsr     x10, x9, #30
    lsl     x11, x10, #30
    ldr     x12, ={kernel_block}
    orr     x11, x11, x12
    and     x10, x10, #511
    str     x11, [x1, x10, lsl #3]  // identity
    ubfx    x10, x13, #39, #9
    orr     x6, x4, #{table}
    str     x6, [x2, x10, lsl #3]
    ubfx    x10, x13, #30, #9
    str     x11, [x4, x10, lsl #3]  // the kernel's virtual address
    mov     x10, #{direct_map_l0_index}
    orr     x6, x3, #{table}
    str     x6, [x2, x10, lsl #3]
    ldr     x12, ={device_block}
    ldr     x14, ={normal_block}
    mov     x10, #0
3:  cmp     x10, #{ram_gigabyte}
    csel    x6, x12, x14, lo
    orr     x6, x6, x10, lsl #30
    str     x6, [x3, x10, lsl #3]   // the direct map
    add     x10, x10, #1
    cmp     x10, #{direct_map_gigabytes}
    b.lo    3b

    // Translation on, then continue at the kernel's virtual address.
    bl      boot_translation_on
    ldr     x0, =4f
    br      x0

    // From here on the kernel runs at its virtual address, and `adrp` gives virtual addresses.
4:  adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    mov     x0, #{cpacr_fp_el1_only}
    msr     cpacr_el1, x0
    msr     cntkctl_el1, xzr
    msr     tpidrro_el0, xzr

    // Exception vectors, before any Rust code runs, then the timer and interrupt controller.
    adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    bl      {init_timer}
    bl      kernel_main
6:  wfe
    b       6b

    // Turns translation on with the boot page tables, from code running at its physical
    // address with the MMU off: memory attributes, address sizes (IPS: the CPU's physical
    // address size, at most 48 bits), both roots, then the MMU and the caches. The TLB is
    // invalidated first, as its contents are unknown on a CPU that just started. Returns to
    // the caller's physical address, mapped by the boot tables' identity gigabyte. Clobbers
    // x0-x2.
boot_translation_on:
    ldr     x0, ={mair}
    msr     mair_el1, x0
    mrs     x1, id_aa64mmfr0_el1
    and     x1, x1, #0xf
    mov     x2, #{ips_max}
    cmp     x1, x2
    csel    x1, x1, x2, lo
    ldr     x0, ={tcr}
    orr     x0, x0, x1, lsl #{ips_shift}
    msr     tcr_el1, x0
    adrp    x0, {l0_low}
    msr     ttbr0_el1, x0
    adrp    x0, {l0_high}
    msr     ttbr1_el1, x0
    dsb     ish
    tlbi    vmalle1
    dsb     ish
    isb
    mrs     x0, sctlr_el1
    ldr     x1, ={sctlr_enable}
    orr     x0, x0, x1
    msr     sctlr_el1, x0
    isb
    ret

    // A secondary CPU, started by PSCI CPU_ON (D40): EL1, MMU off, interrupts masked, and
    // x0 = the context id, the kernel virtual address of the `CpuStart`. The boot page tables
    // the boot CPU built are still in .bss, and map the `CpuStart` and its stack (the kernel
    // image and the direct map).
    .global eggos_secondary_start
eggos_secondary_start:
    mov     x19, x0
    msr     spsel, #1
    bl      boot_translation_on
    ldr     x0, =7f
    br      x0
7:  ldr     x0, [x19, #{start_stack_top}]
    mov     sp, x0
    mov     x0, #{cpacr_fp_el1_only}
    msr     cpacr_el1, x0
    msr     cntkctl_el1, xzr
    msr     tpidrro_el0, xzr
    adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    mov     x0, x19
    bl      {secondary_main}
8:  wfe
    b       8b
    .ltorg
"#,
    secondary_main = sym secondary_main,
    start_stack_top = const core::mem::offset_of!(CpuStart, stack_top),
    cpacr_fp_el1_only = const fp::CPACR_FP_EL1_ONLY,
    init_timer = sym timer::init,
    l0_low = sym BOOT_L0_LOW,
    l1_low = sym BOOT_L1_LOW,
    l0_high = sym BOOT_L0_HIGH,
    l1_direct = sym BOOT_L1_DIRECT,
    l1_kernel = sym BOOT_L1_KERNEL,
    table = const paging::aarch64::table(0),
    kernel_block = const paging::aarch64::leaf(0, 2, BOOT_KERNEL),
    device_block = const paging::aarch64::leaf(0, 2, Attributes::DEVICE),
    normal_block = const paging::aarch64::leaf(0, 2, Attributes::READ_WRITE),
    direct_map_l0_index = const paging::index(DIRECT_MAP_OFFSET as u64, 3),
    ram_gigabyte = const RAM_BASE >> 30,
    direct_map_gigabytes = const BOOT_DIRECT_MAP_END >> 30,
    mair = const paging::aarch64::MAIR,
    tcr = const TCR,
    ips_max = const TCR_IPS_MAX,
    ips_shift = const TCR_IPS_SHIFT,
    sctlr_enable = const SCTLR_MMU_AND_CACHES,
);

/// The image's virtual address minus its physical address: the kernel links at
/// `0xffff_ffff_8020_0000`, in the top 2 GiB (D33). `linker.ld` uses the same value and checks
/// it against this one.
pub(crate) const KERNEL_VIRTUAL_OFFSET: u64 = 0xffff_ffff_4000_0000;

/// Zeroes the `length` bytes at `start` ([`crate::memory::zero`]): in 64-byte lines of word
/// stores, faster here than the compiler's `memset`, a loop of single word stores (D56).
///
/// # Safety
/// As for [`crate::memory::zero`].
#[inline]
pub(crate) unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    unsafe { crate::memory::zero_by_lines(start, length) }
}

/// The physical memory the boot direct map covers, and so the most the kernel reaches before
/// the driver model (M5): 256 GiB, all of one level-1 table's first half. QEMU `virt` puts
/// at most 255 GiB of RAM from 1 GiB on, below its high device region.
const BOOT_DIRECT_MAP_END: u64 = 256 << 30;

/// The kernel's boot mapping: read, write and execute until its own page tables are active.
const BOOT_KERNEL: Attributes = Attributes {
    writable: true,
    executable: true,
    memory_type: MemoryType::Normal,
    user: false,
};

/// A boot page table (see `_start`).
#[repr(C, align(4096))]
struct BootTable([u64; paging::ENTRIES]);

// Built by `_start` before any Rust code runs. Once the kernel's own page tables are active,
// only secondary CPUs use them, to turn translation on (D40); nothing writes them again.
static mut BOOT_L0_LOW: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_LOW: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L0_HIGH: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_DIRECT: BootTable = BootTable([0; paging::ENTRIES]);
static mut BOOT_L1_KERNEL: BootTable = BootTable([0; paging::ENTRIES]);

/// QEMU `virt`: start of RAM, where QEMU places the device tree for a non-Linux kernel (it
/// passes the tree's address in `x0` only to Linux images). The kernel links far enough above
/// it for QEMU's 1 MiB tree to fit (see `linker.ld`).
const RAM_BASE: usize = 0x4000_0000;

/// QEMU `virt`: PL011 UART0.
const UART0_BASE: usize = 0x0900_0000;

/// The direct map: the start of the high half, translated through `TTBR1_EL1` (D29).
const DIRECT_MAP_OFFSET: usize = 0xffff_0000_0000_0000;

/// `TCR_EL1` (Arm ARM D19.2.139) for 48-bit halves with the 4 KiB granule, walks through
/// inner shareable, write-back cached memory; `IPS` (the physical address size) is added from
/// `ID_AA64MMFR0_EL1` at run time.
const TCR: u64 = {
    let t0sz = 64 - 48;
    let t1sz = (64 - 48) << 16;
    let walks0 = (0b01 << 8) | (0b01 << 10) | (0b11 << 12); // IRGN0, ORGN0 write-back; SH0 inner
    let walks1 = (0b01 << 24) | (0b01 << 26) | (0b11 << 28); // likewise for TTBR1
    let granule1 = 0b10 << 30; // TG1: 4 KiB (TG0 = 0b00 is 4 KiB too)
    t0sz | t1sz | walks0 | walks1 | granule1
};
const TCR_IPS_SHIFT: u32 = 32;
/// The largest `IPS` the 4 KiB granule takes without FEAT_LPA2: 48 bits.
const TCR_IPS_MAX: u64 = 0b101;
/// `SCTLR_EL1`: M (MMU on), C (data cache), I (instruction cache).
const SCTLR_MMU_AND_CACHES: u64 = (1 << 0) | (1 << 2) | (1 << 12);

/// PSCI 0.2+ `SYSTEM_OFF` and 64-bit `CPU_ON` function IDs (Arm DEN 0022, 5.1).
const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;
const PSCI_SYSTEM_RESET: u64 = 0x8400_0009;
const PSCI_CPU_ON: u64 = 0xc400_0003;
/// PSCI return codes `CPU_ON` gives (Arm DEN 0022, 5.2.2).
const PSCI_SUCCESS: i64 = 0;
const PSCI_INVALID_PARAMETERS: i64 = -2;
const PSCI_ALREADY_ON: i64 = -4;
const PSCI_ON_PENDING: i64 = -5;

/// Called by `eggos_secondary_start` on the `CpuStart`'s stack, at the kernel's virtual address
/// with the boot page tables: switches to the kernel's page tables, sets this CPU's timer and
/// interrupts up (D41), then calls the entry.
extern "C" fn secondary_main(start: &CpuStart) -> ! {
    // SAFETY: `start_cpu`'s caller guarantees the tables are the kernel's own, which map
    // everything this CPU uses at the addresses it uses now, as they did for the boot CPU;
    // interrupts are masked (PSCI CPU_ON starts the CPU with them masked).
    unsafe { Aarch64::activate_page_tables(&start.page_table_roots) };
    timer::init_this_cpu();
    (start.entry)(start.argument)
}

/// The early console, UART0.
#[inline]
fn uart0() -> Pl011<Mmio> {
    // SAFETY: on QEMU `virt` UART0's register block is at UART0_BASE, which the boot page
    // tables and the kernel's own map as device memory at the direct map offset; only the early
    // console's transmit and receive paths use it. Turning receive interrupts on changes registers
    // transmit relies on, which is sound because the boot CPU does it once, before any other
    // CPU starts.
    Pl011::new(unsafe { Mmio::new(DIRECT_MAP_OFFSET + UART0_BASE, pl011::REGISTER_BLOCK_SIZE) })
}

fn psci_system_call(function: u64) {
    // SAFETY: SYSTEM_OFF and SYSTEM_RESET take no arguments, touch none of our memory and do
    // not return on success; QEMU `virt` provides PSCI over HVC when EL2 is not emulated.
    // SMCCC v1.0 lets the callee corrupt x0-x17, so all of them are declared clobbered.
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") function => _,
            out("x1") _, out("x2") _, out("x3") _, out("x4") _, out("x5") _,
            out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _,
            out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nomem, nostack),
        );
    }
}

#[derive(Debug)]
pub struct Aarch64;

impl Platform for Aarch64 {
    const ARCH: &'static str = "aarch64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        uart0().write_bytes(bytes);
    }

    fn console_read(buffer: &mut [u8]) -> usize {
        uart0().read_bytes(buffer)
    }

    fn enable_console_input() -> Result<(), &'static str> {
        uart0().enable_receive_interrupt();
        timer::route_console_input();
        Ok(())
    }

    fn exit(_status: ExitStatus) -> ! {
        psci_system_call(PSCI_SYSTEM_OFF);
        Self::halt()
    }

    fn restart() -> ! {
        psci_system_call(PSCI_SYSTEM_RESET);
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `wfe` only pauses the CPU until an event or interrupt.
            unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `brk` traps to the exception vectors installed at boot, whose handler saves
        // and restores every general-purpose register and resumes after the `brk`.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("brk #0") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `udf` is permanently undefined; it traps to the exception vectors installed
        // at boot, and the kernel never resumes from that exception.
        unsafe { core::arch::asm!("udf #0", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::frequency()
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        // SAFETY: `wfi` wakes once an interrupt is pending, even with `PSTATE.I` set; clearing
        // I then takes it through the vectors installed at boot, which restore every register,
        // and setting I masks interrupts again. No `nomem`/`nostack`: the kernel's handler may
        // read and write any memory, and the exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("wfi", "msr daifclr, #2", "isb", "msr daifset, #2") };
    }

    #[inline]
    fn interrupt_window() {
        // SAFETY: as in `wait_for_interrupt`, without the `wfi`: the `isb` makes the unmasking
        // take effect, so an interrupt already pending is taken before I is set again.
        unsafe { core::arch::asm!("msr daifclr, #2", "isb", "msr daifset, #2") };
    }

    fn memory_map() -> MemoryMap {
        // SAFETY: on QEMU `virt` the start of RAM holds the device tree QEMU loaded there,
        // which nothing writes; the direct map covers it. If QEMU loaded none, the header check
        // fails and the map is empty.
        let mut map = unsafe { boot_memory::device_tree_memory_map(RAM_BASE) };
        // The kernel's page tables are built through the boot direct map.
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

    type PageTableFormat = paging::aarch64::Aarch64;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        // SAFETY: as in `memory_map`.
        let mut map = unsafe { boot_memory::device_tree_ram(RAM_BASE) };
        // RAM beyond the boot direct map is never handed out (see `memory_map`), so it is left
        // unmapped.
        map.retain_within(PhysicalRange {
            start: 0,
            end: BOOT_DIRECT_MAP_END,
        });
        map
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(
            UART0_BASE as u64,
            pl011::REGISTER_BLOCK_SIZE as u64,
        ));
        timer::DEVICE_REGIONS.into_iter().for_each(region);
    }

    fn device_tree() -> Option<&'static [u8]> {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_blob(RAM_BASE) }
    }

    const DEVICE_TREE_DMA_COHERENT_BY_DEFAULT: bool = false;

    fn make_kernel_mappings_visible(_start: usize, _length: usize) {
        // SAFETY: barriers change no memory contents: the table writes complete before any walk,
        // and the next instructions see them. Arm caches no invalid entries, so nothing is
        // invalidated.
        unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on
        // at the addresses it uses now: the kernel image at its virtual address (TTBR1), the
        // direct map and the devices in it; the low half (`low`) is empty. `MAIR` and `TCR`
        // stay as the boot code set them. The tables replace the boot tables without two live
        // translations of one address ever meeting in the TLB (the Arm ARM's break-before-make
        // rule, D8.16): the code branches to the kernel's identity alias in the boot TTBR0,
        // points TTBR1 at the empty `low` root and invalidates the TLB, then at the new tables,
        // branches back to the kernel's virtual address, and finally replaces the boot TTBR0
        // with `low` and invalidates again. Nothing in between touches memory, and interrupts
        // are masked. The first barrier completes the table writes before any walk. No
        // `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "adr {tmp}, 2f",
                "sub {tmp}, {tmp}, {offset}",
                "br {tmp}",
                "2:",
                "msr ttbr1_el1, {low}",
                "isb",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                "msr ttbr1_el1, {high}",
                "isb",
                "adr {tmp}, 3f",
                "add {tmp}, {tmp}, {offset}",
                "br {tmp}",
                "3:",
                "msr ttbr0_el1, {low}",
                "isb",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                low = in(reg) *low,
                high = in(reg) *high,
                offset = in(reg) KERNEL_VIRTUAL_OFFSET,
                tmp = out(reg) _,
                options(nostack, preserves_flags),
            );
        }
    }

    unsafe fn switch_address_space(roots: &[u64]) {
        let [low, _] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        // SAFETY: the caller guarantees `low` translates the low half alone, in the active
        // format, with the kernel half (`TTBR1_EL1`) unchanged, and that nothing uses the low
        // half meanwhile. The first barrier completes the table writes before any walk; every
        // translation runs with ASID 0 (`TCR_EL1.A1` = 0, ASID 0 in `TTBR0_EL1`), user ones
        // tagged with it as not global, so `TLBI ASIDE1` with ASID 0 drops the old low half's
        // and keeps the kernel's global ones. Local: each CPU drops what its own TLB cached when
        // it switches. No `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "msr ttbr0_el1, {low}",
                "isb",
                "tlbi aside1, xzr",
                "dsb nsh",
                "isb",
                low = in(reg) *low,
                options(nostack, preserves_flags),
            );
        }
    }

    fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
        // Without PAN (not turned on yet), EL1 reaches EL0 pages directly.
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
        const HWCAP_FP: u64 = 1 << 0;
        const HWCAP_ASIMD: u64 = 1 << 1;
        let features: u64;
        // SAFETY: reading an ID register has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, id_aa64pfr0_el1", out(reg) features, options(nomem, nostack))
        };
        let fp = if features >> 16 & 0xf != 0xf {
            HWCAP_FP
        } else {
            0
        };
        let asimd = if features >> 20 & 0xf != 0xf {
            HWCAP_ASIMD
        } else {
            0
        };
        [fp | asimd, 0]
    }

    fn make_instructions_visible(start: usize, length: usize) {
        let ctr: u64;
        // SAFETY: reading the cache type register has no side effects.
        unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
        // CTR_EL0.DminLine: log2 of the smallest data cache line, in words.
        let line = 4usize << (ctr >> 16 & 0xf);
        let mut address = start & !(line - 1);
        while address < start + length {
            // SAFETY: cleaning a line to the point of unification writes back what the caller
            // wrote and changes no memory contents; `address` is in the caller's mapped range.
            unsafe { core::arch::asm!("dc cvau, {}", in(reg) address, options(nostack)) };
            address += line;
        }
        // SAFETY: barriers and instruction cache invalidation change no memory contents. The
        // whole instruction cache is invalidated, not by address, so that no alias of the
        // lines (another virtual address of the same frames, as user space has) stays stale;
        // on every CPU in the inner shareable domain (`ialluis`), as the cleaning by address
        // reaches them too, so a program loaded here runs on any CPU (D48).
        unsafe { core::arch::asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack)) };
    }

    fn page_fault(_kind: crate::ExceptionKind, code: u64) -> Option<paging::PageFault> {
        exception::page_fault(code)
    }

    fn drop_user_translations(start: u64, length: u64) {
        let pages = length / crate::memory::FRAME_SIZE;
        if pages > crate::paging::USER_PAGES_DROPPED_ONE_BY_ONE {
            // SAFETY: barriers and TLB invalidation change no memory contents. User
            // translations carry ASID 0 and are not global (see `switch_address_space`), so
            // `TLBI ASIDE1` with ASID 0 drops them all and keeps the kernel's. Local: the kernel
            // sees to other CPUs.
            unsafe {
                core::arch::asm!(
                    "dsb ishst",
                    "tlbi aside1, xzr",
                    "dsb nsh",
                    "isb",
                    options(nostack, preserves_flags)
                )
            };
            return;
        }
        // SAFETY: as above: the first barrier completes the caller's table writes before any
        // walk, then each page's translations (any level, ASID 0: the operand's top bits are 0
        // as user addresses are below 2^48) are dropped on this CPU.
        unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
        for page in 0..pages {
            let operand = (start + page * crate::memory::FRAME_SIZE) >> 12;
            // SAFETY: as above.
            unsafe {
                core::arch::asm!("tlbi vae1, {}", in(reg) operand, options(nostack, preserves_flags))
            };
        }
        // SAFETY: as above: completes the invalidations before the next instruction.
        unsafe { core::arch::asm!("dsb nsh", "isb", options(nostack, preserves_flags)) };
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
        // calling convention requires a call to preserve, so to each thread the call is an
        // ordinary function call.
        unsafe { thread::eggos_switch_thread(save, resume) }
    }

    fn random_seeds(seed: &mut dyn FnMut(crate::random::SeedSource, &[u8])) {
        // SAFETY: as in `memory_map`; the kernel calls this once at boot, on the boot CPU
        // before any other starts and between the HAL's other reads of the tree, which the
        // direct map keeps writable (RAM is mapped read-write in it, reserved or not).
        unsafe { boot_memory::device_tree_take_seeds(RAM_BASE, seed) };
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
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_cpus(RAM_BASE, cpu) };
    }

    fn boot_cpu() -> u64 {
        // `_start` lets only the CPU whose affinity is all zero boot.
        0
    }

    unsafe fn start_cpu(cpu: u64, start: &CpuStart) -> Result<(), StartCpuError> {
        unsafe extern "C" {
            /// The secondary entry point in the boot code above.
            fn eggos_secondary_start();
        }
        let entry = (eggos_secondary_start as *const () as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET);
        let context = core::ptr::from_ref(start) as u64;
        let result: i64;
        // SAFETY: CPU_ON starts CPU `cpu` at the physical address `entry`, the secondary entry
        // code, with `context` in x0; the caller guarantees what that code relies on (the
        // kernel's tables, `start` and its stack). The new CPU reads `*start`, so the asm may
        // read memory (no `nomem`: the compiler keeps the writes to it before the call), and
        // `dsb ish` completes those writes for the other CPUs before the call: an HVC is not
        // a barrier. SMCCC v1.0 lets the callee corrupt x0-x17, so all are declared clobbered.
        unsafe {
            core::arch::asm!(
                "dsb ish",
                "hvc #0",
                inout("x0") PSCI_CPU_ON => result,
                inout("x1") cpu => _,
                inout("x2") entry => _,
                inout("x3") context => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _,
                out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _,
                out("x15") _, out("x16") _, out("x17") _,
                options(nostack),
            );
        }
        match result {
            PSCI_SUCCESS => Ok(()),
            PSCI_INVALID_PARAMETERS => Err(StartCpuError::InvalidCpu),
            PSCI_ALREADY_ON | PSCI_ON_PENDING => Err(StartCpuError::AlreadyRunning),
            error => Err(StartCpuError::Refused(error)),
        }
    }

    unsafe fn set_cpu_local(block: usize) {
        // SAFETY: TPIDR_EL1 is a scratch register for the kernel's own use; nothing else
        // reads it.
        unsafe { core::arch::asm!("msr tpidr_el1, {}", in(reg) block, options(nomem, nostack)) };
    }

    #[inline]
    fn cpu_local() -> usize {
        let block: usize;
        // SAFETY: reading TPIDR_EL1 has no side effects.
        unsafe { core::arch::asm!("mrs {}, tpidr_el1", out(reg) block, options(nomem, nostack)) };
        block
    }

    fn wake_cpu(cpu: u64) {
        timer::wake(cpu);
    }
}
