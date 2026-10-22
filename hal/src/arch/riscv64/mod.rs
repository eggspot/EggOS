//! RISC-V 64-bit (RV64GC).
//!
//! Boot contract: the SBI firmware (OpenSBI, as bundled with QEMU) enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device-tree address, MMU off.
//!
//! Board addresses are QEMU `virt`'s for now; device-tree discovery replaces them in M5.

mod exception;

use crate::{ExitStatus, Platform};
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

    // FPU off: the kernel uses no FP registers and the trap entry does not save them, so any
    // FP instruction traps as illegal. Then the trap vector, before any Rust code runs.
2:  li      t0, 3 << 13
    csrc    sstatus, t0
    la      t0, exception_entry
    csrw    stvec, t0
    call    kernel_main
3:  wfi
    j       3b
"#
);

/// QEMU `virt`: NS16550A UART0, registers 1 byte apart.
const UART0_BASE: usize = 0x1000_0000;
const UART0_REG_SHIFT: u32 = 0;

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
}
