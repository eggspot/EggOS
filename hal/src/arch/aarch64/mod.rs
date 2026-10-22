//! AArch64 (Armv8-A and later).
//!
//! Boot contract: entered at `_start` in EL1 with the MMU off, as QEMU's `virt` machine does
//! for an ELF kernel. The kernel is built for `aarch64-unknown-none-softfloat` (decision D10):
//! no FP/SIMD in the kernel, and `strict-align`, which is required while the MMU is off.
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;

use crate::{ExitStatus, Platform};
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

    // Exception vectors, before any Rust code runs.
2:  adrp    x0, exception_vectors
    add     x0, x0, :lo12:exception_vectors
    msr     vbar_el1, x0
    isb
    bl      kernel_main
3:  wfe
    b       3b
"#
);

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

    fn illegal_instruction() -> ! {
        // SAFETY: `udf` is permanently undefined; it traps to the exception vectors installed
        // at boot, and the kernel never resumes from that exception.
        unsafe { core::arch::asm!("udf #0", options(noreturn)) }
    }
}
