//! AArch64 (Armv8-A and later).
//!
//! Boot contract: entered at `_start` in EL1 with the MMU off, as QEMU's `virt` machine does
//! for an ELF kernel. The kernel is built for `aarch64-unknown-none-softfloat` (decision D10):
//! no FP/SIMD in the kernel, and `strict-align`, which is required while the MMU is off.
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod timer;

use crate::arch::boot_memory;
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
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
    cbnz    x0, 3f

    // Use SP_EL1 at EL1, so exceptions taken to EL1 run on the boot stack set up below.
    msr     spsel, #1
    adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    // Zero .bss (8-byte aligned and sized by the linker script).
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
1:  cmp     x0, x1
    b.hs    2f
    str     xzr, [x0], #8
    b       1b

    // Exception vectors, before any Rust code runs, then the timer and interrupt controller.
2:  adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    bl      {init_timer}
    bl      kernel_main
3:  wfe
    b       3b
"#,
    init_timer = sym timer::init,
);

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

/// PSCI 0.2+ `SYSTEM_OFF` function ID (Arm DEN 0022).
const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;

#[derive(Debug)]
pub struct Aarch64;

impl Platform for Aarch64 {
    const ARCH: &'static str = "aarch64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        // SAFETY: on QEMU `virt` UART0's register block is at UART0_BASE, accessible with the
        // MMU off; the early console is its only user.
        let regs = unsafe { Mmio::new(UART0_BASE, pl011::REGISTER_BLOCK_SIZE) };
        Pl011::new(regs).write_bytes(bytes);
    }

    fn exit(_status: ExitStatus) -> ! {
        // PSCI has no exit status; the QEMU runner reads the verdict from the console.
        // SAFETY: SYSTEM_OFF takes no arguments, touches none of our memory and does not
        // return on success; QEMU `virt` provides PSCI over HVC when EL2 is not emulated.
        // SMCCC v1.0 lets the callee corrupt x0-x17, so all of them are declared clobbered.
        // If PSCI returns an error we fall through and halt.
        unsafe {
            core::arch::asm!(
                "hvc #0",
                inout("x0") PSCI_SYSTEM_OFF => _,
                out("x1") _, out("x2") _, out("x3") _, out("x4") _, out("x5") _,
                out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _,
                out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _,
                out("x16") _, out("x17") _,
                options(nomem, nostack),
            );
        }
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

    fn memory_map() -> MemoryMap {
        // SAFETY: on QEMU `virt` the start of RAM holds the device tree QEMU loaded there,
        // which nothing writes; RAM is reachable through the physical memory offset (the MMU is
        // off, or the direct map covers it). If QEMU loaded none, the header check fails and
        // the map is empty.
        unsafe { boot_memory::device_tree_memory_map(RAM_BASE) }
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        boot_memory::physical_memory_offset()
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
        unsafe { boot_memory::device_tree_ram(RAM_BASE) }
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(
            UART0_BASE as u64,
            pl011::REGISTER_BLOCK_SIZE as u64,
        ));
        timer::DEVICE_REGIONS.into_iter().for_each(region);
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [low, high] = roots else {
            panic!("AArch64 page tables have two roots");
        };
        let physical_address_size: u64;
        // SAFETY: reading an ID register has no side effects.
        unsafe {
            core::arch::asm!(
                "mrs {}, id_aa64mmfr0_el1",
                out(reg) physical_address_size,
                options(nomem, nostack, preserves_flags),
            );
        }
        let tcr = TCR | (physical_address_size & 0xf).min(TCR_IPS_MAX) << TCR_IPS_SHIFT;
        // SAFETY: the caller guarantees the tables map everything this CPU uses from here on
        // at the addresses it uses now: the code runs on at its physical address, the stack and
        // data likewise, and the devices too. `MAIR` matches the attribute indexes the format
        // encodes. The barriers complete the table writes before the walker reads them, the
        // TLB is invalidated before the MMU is turned on, and `isb` makes each change take
        // effect before the next instruction. No `nomem`: memory accesses must not move across
        // the switch.
        unsafe {
            core::arch::asm!(
                "msr mair_el1, {mair}",
                "msr tcr_el1, {tcr}",
                "msr ttbr0_el1, {low}",
                "msr ttbr1_el1, {high}",
                "dsb ishst",
                "tlbi vmalle1",
                "dsb ish",
                "isb",
                "mrs {sctlr}, sctlr_el1",
                "orr {sctlr}, {sctlr}, {enable}",
                "msr sctlr_el1, {sctlr}",
                "isb",
                mair = in(reg) paging::aarch64::MAIR,
                tcr = in(reg) tcr,
                low = in(reg) *low,
                high = in(reg) *high,
                enable = in(reg) SCTLR_MMU_AND_CACHES,
                sctlr = out(reg) _,
                options(nostack, preserves_flags),
            );
        }
        boot_memory::set_physical_memory_offset(DIRECT_MAP_OFFSET);
    }
}
