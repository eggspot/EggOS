//! RISC-V 64.
//!
//! Supports QEMU's `virt` machine for now, booted by OpenSBI (`-bios default`)
//! with the kernel as an ELF image: SBI firmware enters `_start` in S-mode
//! with `a0` = hart id and `a1` = device tree address. Device addresses are
//! the fixed ones of that machine until the kernel reads the device tree.

use core::arch::{asm, global_asm};
use core::ptr::{self, NonNull};

use eggos_drivers::Driver;
use eggos_drivers::io::Mmio;
use eggos_drivers::uart16550::Uart16550;

use crate::Platform;

global_asm!(include_str!("boot.S"));

/// Physical address of the `virt` machine's 16550 UART.
const UART_BASE: usize = 0x1000_0000;

/// A QEMU `virt` machine, as far as the kernel knows it today: a hart and a
/// 16550 UART.
pub struct Machine {
    console: Uart16550<Mmio>,
}

/// Bring up the machine. `boot_info` is the device tree's physical address
/// (not used yet).
pub fn init(_boot_info: usize) -> Machine {
    let mut console = uart();
    // If the UART is missing there is nowhere to report it; the kernel runs
    // on and its log is lost.
    let _ = console.init();
    Machine { console }
}

/// The UART, for code that cannot reach the `Machine`, such as the panic
/// handler. The UART is left as `init` configured it.
pub fn emergency_console() -> Uart16550<Mmio> {
    uart()
}

fn uart() -> Uart16550<Mmio> {
    // SAFETY: `UART_BASE` is the `virt` machine's UART, and with paging off
    // physical addresses are used as is. Only one hart runs, and the
    // `Machine` and emergency handles never write concurrently: the emergency
    // console is only used once the kernel has stopped normal work to report
    // a panic.
    let regs =
        unsafe { Mmio::new(NonNull::new(ptr::with_exposed_provenance_mut(UART_BASE)).unwrap()) };
    Uart16550::new(regs)
}

/// Stop this hart for good.
pub fn halt() -> ! {
    // SAFETY: clearing `sstatus.SIE` masks interrupts on this hart only.
    unsafe { asm!("csrci sstatus, 2", options(nomem, nostack)) };
    loop {
        // SAFETY: waiting for an interrupt only idles this hart.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}

impl Platform for Machine {
    const ARCH: &'static str = "riscv64";
    type Console = Uart16550<Mmio>;

    fn console(&self) -> &Self::Console {
        &self.console
    }

    fn halt(&self) -> ! {
        halt()
    }
}
