//! Loongson's legacy I/O interrupt controller (`liointc`) on QEMU `loongson3-virt` (D325): 32
//! level-triggered inputs (the UART on input 0, PCI Express's INTA to INTD on inputs 2 to 5),
//! each with a route byte naming the cores (low nibble) and the CPU interrupt pin, `IP2` to
//! `IP5` (high nibble), it raises. Every routed input goes to the boot core on `IP2`; the
//! interrupt entry reads the core's pending inputs and hands each to the kernel.

use super::UNCACHED_WINDOW;
use crate::interrupt::{DeviceLine, SourceLines};
use core::sync::atomic::{AtomicU32, Ordering};

const BASE: u64 = 0x3ff0_1400;
const ROUTE: u64 = 0x00;
const ENABLE_SET: u64 = 0x28;
const ENABLE_CLEAR: u64 = 0x2c;
const CORE_PENDING: u64 = 0x40;
const CORE_PENDING_STRIDE: u64 = 0x08;
const INPUTS: u32 = 32;
const PIN_IP2: u8 = 1 << 4;
/// `Cause.IP2`.
pub(super) const PENDING: u64 = 1 << 10;
const UART_INPUT: u32 = 0;

static DEVICE_LINES: SourceLines = SourceLines::new();
static BOOT_CORE: AtomicU32 = AtomicU32::new(0);
static CONSOLE_ENABLED: AtomicU32 = AtomicU32::new(0);

fn register(offset: u64) -> *mut u32 {
    (UNCACHED_WINDOW + BASE + offset) as *mut u32
}

fn route(input: u32) -> Result<(), &'static str> {
    if input >= INPUTS {
        return Err("no such liointc input");
    }
    let core = super::cpu_number() as u32 & 3;
    BOOT_CORE.store(core, Ordering::Relaxed);
    // SAFETY: the controller's registers through the uncached window: one route byte per
    // input, then the enable set register, a write of ones that leaves the other inputs as
    // they were.
    unsafe {
        let byte = (UNCACHED_WINDOW + BASE + ROUTE + u64::from(input)) as *mut u8;
        byte.write_volatile(PIN_IP2 | (1 << core));
        register(ENABLE_SET).write_volatile(1 << input);
    }
    Ok(())
}

/// Turns the UART's input on as the console's interrupt.
pub(super) fn enable_console() -> Result<(), &'static str> {
    route(UART_INPUT)?;
    CONSOLE_ENABLED.store(1, Ordering::Relaxed);
    Ok(())
}

/// [`crate::Platform::route_device_interrupt`]: a `liointc` specifier's first cell is its
/// input.
pub(super) fn route_device(cells: &[u32]) -> Result<DeviceLine, &'static str> {
    let input = *cells.first().ok_or("an empty interrupt specifier")?;
    if input == UART_INPUT {
        return Err("the UART's input is the console's");
    }
    route(input)?;
    DEVICE_LINES
        .claim(input + 1)
        .map(|(line, _)| line)
        .ok_or("every device line is taken")
}

/// Hands the boot core's pending inputs to the kernel: the console's, or a routed device's
/// line. An input nobody asked for is turned off.
pub(super) fn handle() {
    let core = u64::from(BOOT_CORE.load(Ordering::Relaxed));
    // SAFETY: the core's pending register, read only.
    let mut pending =
        unsafe { register(CORE_PENDING + core * CORE_PENDING_STRIDE).read_volatile() };
    while pending != 0 {
        let input = pending.trailing_zeros();
        pending &= pending - 1;
        let delivered = if input == UART_INPUT && CONSOLE_ENABLED.load(Ordering::Relaxed) != 0 {
            Some(crate::Interrupt::ConsoleInput)
        } else {
            DEVICE_LINES
                .line_of(input + 1)
                .map(crate::Interrupt::Device)
        };
        match delivered {
            Some(interrupt) => crate::interrupt::deliver(interrupt),
            // SAFETY: the enable clear register: turns only this input off.
            None => unsafe { register(ENABLE_CLEAR).write_volatile(1 << input) },
        }
    }
}
