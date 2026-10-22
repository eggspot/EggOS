//! RISC-V 64-bit (RV64GC).
//!
//! Boot contract: the SBI firmware (OpenSBI, as bundled with QEMU) enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device-tree address, MMU off. The device tree describes the
//! RAM and the firmware's own reservation (see [`Platform::memory_map`]).
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;
mod timer;

use crate::arch::boot_memory;
use crate::memory::PhysicalRange;
use crate::paging::{self, KernelImage};
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Ns16550, Uart};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    // SBI starts only the boot hart; others stay stopped until started through SBI HSM.
    la      sp, __stack_top

    // Zero .bss (8-byte aligned and sized by the linker script).
    la      t0, __bss_start
    la      t1, __bss_end
1:  bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b

    // Keep the device-tree address (after zeroing .bss, where it is stored).
2:  la      t0, {device_tree}
    sd      a1, 0(t0)

    // FPU off: the kernel uses no FP registers and the trap entry does not save them, so any
    // FP instruction traps as illegal. Then the trap vector, before any Rust code runs, and
    // the timer.
    li      t0, 3 << 13
    csrc    sstatus, t0
    la      t0, exception_entry
    csrw    stvec, t0
    call    {init_timer}
    call    kernel_main
3:  wfi
    j       3b
"#,
    init_timer = sym timer::init,
    device_tree = sym DEVICE_TREE,
);

/// Physical address of the device tree, passed by the SBI firmware in `a1` and stored by the
/// entry code.
static DEVICE_TREE: AtomicUsize = AtomicUsize::new(0);

/// QEMU `virt`: NS16550A UART0, registers 1 byte apart.
const UART0_BASE: usize = 0x1000_0000;
const UART0_REG_SHIFT: u32 = 0;

/// The direct map: the start of Sv39's high half (D29), which holds 256 GiB.
const DIRECT_MAP_OFFSET: usize = 0xffff_ffc0_0000_0000;

/// SBI System Reset extension (SBI spec v2.0, chapter 10).
const SBI_EXT_SRST: usize = 0x5352_5354;
const SBI_SRST_SHUTDOWN: usize = 0;
const SBI_SRST_REASON_NONE: usize = 0;
const SBI_SRST_REASON_FAILURE: usize = 1;

#[derive(Debug)]
pub struct Riscv64;

impl Platform for Riscv64 {
    const ARCH: &'static str = "riscv64";

    #[inline]
    fn console_write(bytes: &[u8]) {
        // SAFETY: on QEMU `virt` UART0's register block is at UART0_BASE, accessible with the
        // MMU off; the early console is its only user.
        let regs = unsafe {
            Mmio::new(
                UART0_BASE,
                Ns16550::<Mmio>::register_block_size(UART0_REG_SHIFT),
            )
        };
        Ns16550::new(regs, UART0_REG_SHIFT).write_bytes(bytes);
    }

    fn exit(status: ExitStatus) -> ! {
        let reason = match status {
            ExitStatus::Success => SBI_SRST_REASON_NONE,
            ExitStatus::Failure => SBI_SRST_REASON_FAILURE,
        };
        // SAFETY: an SBI call per the SBI calling convention: a7 = extension, a6 = function,
        // a0/a1 = arguments, a0/a1 = returned error/value. It does not return on success; on
        // failure (no SRST) we fall through and halt.
        unsafe {
            core::arch::asm!(
                "ecall",
                inout("a0") SBI_SRST_SHUTDOWN => _,
                inout("a1") reason => _,
                in("a6") 0usize,
                in("a7") SBI_EXT_SRST,
                options(nostack),
            );
        }
        Self::halt()
    }

    fn halt() -> ! {
        loop {
            // SAFETY: `wfi` only pauses the hart until an interrupt is pending.
            unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
        }
    }

    fn breakpoint() {
        // SAFETY: `ebreak` traps to the vector installed at boot, whose handler saves and
        // restores every general-purpose register and resumes after the `ebreak`.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // exception frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("ebreak") };
    }

    fn execute_illegal_instruction() -> ! {
        // SAFETY: `unimp` is defined to be illegal; it traps to the vector installed at boot,
        // and the kernel never resumes from that exception.
        unsafe { core::arch::asm!("unimp", options(noreturn)) }
    }

    #[inline]
    fn clock_ticks() -> u64 {
        timer::now()
    }

    #[inline]
    fn clock_frequency() -> u64 {
        timer::TIMEBASE_FREQUENCY
    }

    fn set_timer_deadline(deadline: u64) {
        timer::set_deadline(deadline);
    }

    fn wait_for_interrupt() {
        // SAFETY: `wfi` wakes once an interrupt enabled in `sie` is pending, even with
        // `sstatus.SIE` clear; setting SIE then takes it through the trap vector installed at
        // boot, which restores every register, and clearing SIE masks interrupts again.
        // No `nomem`/`nostack`: the kernel's handler may read and write any memory, and the
        // trap frame is pushed below the stack pointer.
        unsafe { core::arch::asm!("wfi", "csrsi sstatus, 2", "csrci sstatus, 2") };
    }

    fn memory_map() -> MemoryMap {
        // SAFETY: the SBI firmware passes the address of its device tree in `a1` (the RISC-V
        // boot convention, which OpenSBI follows), which stays readable and unwritten; it is in
        // RAM, reachable through the physical memory offset (the MMU is off, or the direct map
        // covers it).
        unsafe { boot_memory::device_tree_memory_map(DEVICE_TREE.load(Ordering::Relaxed)) }
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        boot_memory::physical_memory_offset()
    }

    type PageTableFormat = paging::riscv64::Riscv64Sv39;

    fn direct_map_offset() -> usize {
        DIRECT_MAP_OFFSET
    }

    fn kernel_image() -> KernelImage {
        boot_memory::kernel_image_parts()
    }

    fn physical_memory() -> MemoryMap {
        // SAFETY: as in `memory_map`.
        unsafe { boot_memory::device_tree_ram(DEVICE_TREE.load(Ordering::Relaxed)) }
    }

    fn early_device_regions(region: &mut dyn FnMut(PhysicalRange)) {
        region(PhysicalRange::from_start_length(
            UART0_BASE as u64,
            Ns16550::<Mmio>::register_block_size(UART0_REG_SHIFT) as u64,
        ));
    }

    unsafe fn activate_page_tables(roots: &[u64]) {
        let [root] = roots else {
            panic!("Sv39 page tables have one root");
        };
        let satp = paging::riscv64::SATP_MODE_SV39 | root >> 12;
        // SAFETY: the caller guarantees the tables map everything this hart uses from here on
        // at the addresses it uses now: the code runs on at its physical address, the stack and
        // data likewise, and the UART too. The first `sfence.vma` orders the table writes
        // before the walker reads them, the second drops translations cached before the
        // switch. No `nomem`: memory accesses must not move across the switch.
        unsafe {
            core::arch::asm!(
                "sfence.vma",
                "csrw satp, {satp}",
                "sfence.vma",
                satp = in(reg) satp,
                options(nostack, preserves_flags),
            );
        }
        boot_memory::set_physical_memory_offset(DIRECT_MAP_OFFSET);
    }
}
