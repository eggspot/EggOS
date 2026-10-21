//! AArch64.
//!
//! Supports QEMU's `virt` machine for now, booted as an ELF image with
//! `-kernel`: the loader enters `_start` at EL1 with the MMU off. Device
//! addresses are the fixed ones of that machine until the kernel reads the
//! device tree.

use core::arch::{asm, global_asm};
use core::ptr::{self, NonNull};

use eggos_drivers::Driver;
use eggos_drivers::io::Mmio;
use eggos_drivers::pl011::Pl011;

use crate::Platform;

global_asm!(include_str!("boot.S"));

/// Physical address of the `virt` machine's PL011 UART.
const PL011_BASE: usize = 0x0900_0000;
/// Frequency of the clock driving that UART (QEMU's `apb-pclk`).
const PL011_CLOCK_HZ: u32 = 24_000_000;

/// A QEMU `virt` machine, as far as the kernel knows it today: a CPU and a
/// PL011 UART.
pub struct Machine {
    console: Pl011,
}

/// Bring up the machine. `boot_info` is whatever the loader left in `x0`
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
pub fn emergency_console() -> Pl011 {
    uart()
}

fn uart() -> Pl011 {
    // SAFETY: `PL011_BASE` is the `virt` machine's UART, and with the MMU off
    // every physical address is device memory. Only one CPU runs, and the
    // `Machine` and emergency handles never write concurrently: the emergency
    // console is only used once the kernel has stopped normal work to report
    // a panic.
    let regs =
        unsafe { Mmio::new(NonNull::new(ptr::with_exposed_provenance_mut(PL011_BASE)).unwrap()) };
    Pl011::new(regs, PL011_CLOCK_HZ)
}

/// Stop this CPU for good.
pub fn halt() -> ! {
    // SAFETY: masking all exceptions only affects this CPU.
    unsafe { asm!("msr daifset, #0xf", options(nomem, nostack)) };
    loop {
        // SAFETY: waiting for an interrupt that cannot be taken only idles
        // this CPU.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}

impl Platform for Machine {
    const ARCH: &'static str = "aarch64";
    type Console = Pl011;

    fn console(&self) -> &Self::Console {
        &self.console
    }

    fn halt(&self) -> ! {
        halt()
    }
}
