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
use crate::fdt;
use crate::{ExitStatus, MemoryMap, Platform};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::io::Mmio;
use eggos_drivers::serial::{Pl011, Uart, pl011};

core::arch::global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
_start:
    mov     x19, x0
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

    // Keep x0 (a device-tree address, if the loader passed one) after zeroing .bss.
2:  adrp    x1, {boot_x0}
    str     x19, [x1, :lo12:{boot_x0}]

    // Exception vectors, before any Rust code runs, then the timer and interrupt controller.
    adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    bl      {init_timer}
    bl      kernel_main
3:  wfe
    b       3b
"#,
    init_timer = sym timer::init,
    boot_x0 = sym BOOT_X0,
);

/// `x0` at entry. A loader following the Linux arm64 boot protocol passes the device tree
/// there; QEMU passes it only to Linux images, and places it at the start of RAM otherwise.
static BOOT_X0: AtomicUsize = AtomicUsize::new(0);

/// QEMU `virt`: start of RAM, where QEMU places the device tree for a non-Linux kernel.
const RAM_BASE: usize = 0x4000_0000;

/// QEMU `virt`: PL011 UART0.
const UART0_BASE: usize = 0x0900_0000;

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
        let address = [BOOT_X0.load(Ordering::Relaxed), RAM_BASE]
            .into_iter()
            .find(|&address| is_device_tree(address))
            .unwrap_or(0);
        // SAFETY: `address` is 0 or holds a device-tree header (checked by `is_device_tree`)
        // left by the loader, whose blob stays readable and unwritten; with the MMU off
        // physical addresses are accessible directly.
        unsafe { boot_memory::device_tree_memory_map(address) }
    }

    #[inline]
    fn physical_memory_offset() -> usize {
        0
    }
}

/// Whether a device-tree header is at `address`: `x0` from the loader or QEMU `virt`'s start
/// of RAM, both inside RAM (or 0, rejected).
fn is_device_tree(address: usize) -> bool {
    if address < RAM_BASE || !address.is_multiple_of(8) {
        return false;
    }
    // SAFETY: `address` is 8-byte aligned and at or above the start of RAM, which is readable
    // with the MMU off (QEMU `virt` maps at least 128 MiB there, and the loader passes an
    // address inside RAM); a header is 40 bytes.
    let header = unsafe { core::slice::from_raw_parts(address as *const u8, fdt::HEADER_LENGTH) };
    fdt::total_size(header).is_ok()
}
