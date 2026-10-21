//! x86_64 PCs.
//!
//! Boots through the PVH entry point (Xen's public `arch-x86/hvm/start_info.h`
//! boot ABI), which QEMU's `-kernel` supports for ELF64 images: the loader
//! enters `_start32` in 32-bit protected mode with paging off and `ebx`
//! pointing at an `hvm_start_info` structure. `boot.S` switches to long mode
//! with the first 1 GiB identity-mapped and passes that pointer on as
//! `boot_info`.

use core::arch::{asm, global_asm};

use eggos_drivers::Driver;
use eggos_drivers::io::ByteRegisters;
use eggos_drivers::uart16550::Uart16550;

use crate::Platform;

global_asm!(include_str!("boot.S"), options(att_syntax));

/// I/O port base of COM1, the first PC serial port.
const COM1: u16 = 0x3f8;

/// A bank of registers in the x86 I/O port space.
pub struct PortIo {
    base: u16,
}

impl PortIo {
    /// Create a register bank starting at I/O port `base`.
    ///
    /// # Safety
    ///
    /// The ports from `base` up to the highest offset a driver will use must
    /// belong to one device, and no other code may drive that device while
    /// this value lives.
    pub const unsafe fn new(base: u16) -> Self {
        Self { base }
    }

    fn port(&self, offset: usize) -> u16 {
        self.base + offset as u16
    }
}

impl ByteRegisters for PortIo {
    fn read8(&self, offset: usize) -> u8 {
        let value: u8;
        // SAFETY: `PortIo::new`'s contract makes this port ours; `in` touches
        // no memory and no flags.
        unsafe {
            asm!("in al, dx", out("al") value, in("dx") self.port(offset),
                options(nomem, nostack, preserves_flags));
        }
        value
    }

    fn write8(&self, offset: usize, value: u8) {
        // SAFETY: as in `read8`.
        unsafe {
            asm!("out dx, al", in("dx") self.port(offset), in("al") value,
                options(nomem, nostack, preserves_flags));
        }
    }
}

/// A PC, as far as the kernel knows it today: a CPU and COM1.
pub struct Machine {
    console: Uart16550<PortIo>,
}

/// Bring up the machine. `boot_info` is the physical address of the PVH
/// `hvm_start_info` structure (not used yet).
pub fn init(_boot_info: usize) -> Machine {
    let mut console = com1();
    // If COM1 is missing there is nowhere to report it; the kernel runs on and
    // its log is lost.
    let _ = console.init();
    Machine { console }
}

/// COM1, for code that cannot reach the `Machine`, such as the panic handler.
/// The UART is left as `init` configured it.
pub fn emergency_console() -> Uart16550<PortIo> {
    com1()
}

fn com1() -> Uart16550<PortIo> {
    // SAFETY: COM1's eight ports are the PC's first serial port by platform
    // convention. Only one CPU runs, and the `Machine` and emergency handles
    // never write concurrently: the emergency console is only used once the
    // kernel has stopped normal work to report a panic.
    Uart16550::new(unsafe { PortIo::new(COM1) })
}

/// Stop this CPU for good.
pub fn halt() -> ! {
    loop {
        // SAFETY: masking interrupts and halting only stops this CPU.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

impl Platform for Machine {
    const ARCH: &'static str = "x86_64";
    type Console = Uart16550<PortIo>;

    fn console(&self) -> &Self::Console {
        &self.console
    }

    fn halt(&self) -> ! {
        halt()
    }
}
