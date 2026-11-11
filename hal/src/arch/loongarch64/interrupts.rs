//! LoongArch64's interrupt controllers on QEMU `virt` (D291), as Loongson's 7A bridge and
//! 3A5000 manuals describe them: the extended I/O interrupt controller (EIOINTC), reached
//! through IOCSR, gathers 256 vectors and raises this CPU's hardware interrupt 1 for them; the
//! platform controller hub's interrupt controller (PCH-PIC) turns wired device lines into EIOINTC
//! vectors; and its MSI controller (PCH-MSI) turns a device's write of a vector number to its
//! doorbell into that vector. Every vector goes to the boot CPU.

use super::{UNCACHED_WINDOW, iocsr_read32, iocsr_write32};
use crate::interrupt::{DeviceLine, InterruptMessage, SourceLines};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const EIOINTC_IPMAP: u64 = 0x14c0;
const EIOINTC_ENABLE: u64 = 0x1600;
const EIOINTC_BOUNCE: u64 = 0x1680;
const EIOINTC_ISR: u64 = 0x1800;
const EIOINTC_ROUTE: u64 = 0x1c00;
const VECTORS: u64 = 256;
/// The CPU interrupt pin EIOINTC raises: hardware interrupt 1, `ESTAT` bit 3.
const PIN: u32 = 1;
/// `ESTAT`'s and `ECFG.LIE`'s bit for that pin.
pub(super) const PIN_INTERRUPT: u64 = 1 << (2 + PIN);

/// QEMU `virt`'s PCH-PIC registers and the line its UART uses.
const PCH_PIC_BASE: usize = 0x1000_0000;
const PCH_PIC_MASK: usize = 0x20;
const PCH_PIC_EDGE: usize = 0x60;
const PCH_PIC_ROUTE: usize = 0x100;
const PCH_PIC_VECTOR: usize = 0x200;
const PCH_PIC_POLARITY: usize = 0x3e0;
const PCH_PIC_LINES: u32 = 64;
pub(super) const UART_LINE: u32 = 2;

/// QEMU `virt`'s PCH-MSI doorbell and the vectors it may raise.
const PCH_MSI_DOORBELL: u64 = 0x2ff0_0000;
const MSI_FIRST: u32 = 32;
const MSI_LAST: u32 = 255;

static STARTED: AtomicBool = AtomicBool::new(false);
static DEVICE_LINES: SourceLines = SourceLines::new();
static NEXT_MSI: AtomicU32 = AtomicU32::new(MSI_FIRST);
static CONSOLE_VECTOR: AtomicU32 = AtomicU32::new(u32::MAX);

fn iocsr_read64(address: u64) -> u64 {
    let value: u64;
    // SAFETY: reading this core's EIOINTC registers has no side effects.
    unsafe { core::arch::asm!("iocsrrd.d {0}, {1}", out(reg) value, in(reg) address, options(nostack)) };
    value
}

fn iocsr_write64(address: u64, value: u64) {
    // SAFETY: the callers write EIOINTC registers as the controller defines them.
    unsafe { core::arch::asm!("iocsrwr.d {0}, {1}", in(reg) value, in(reg) address, options(nostack)) };
}

fn pch_pic(offset: usize) -> *mut u8 {
    (UNCACHED_WINDOW + PCH_PIC_BASE + offset) as *mut u8
}

/// Routes every vector to the boot CPU's pin, enables them all at EIOINTC and the pin in `ECFG`;
/// once, on the boot CPU.
pub(super) fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let pin = 1u32 << PIN;
    let every_byte = |byte: u32| byte | byte << 8 | byte << 16 | byte << 24;
    for group in 0..VECTORS / 32 {
        iocsr_write32(EIOINTC_IPMAP + group * 4, every_byte(pin));
    }
    let boot_core = 1u32 << (super::Loongarch64::boot_cpu_index() & 3);
    for word in 0..VECTORS / 4 {
        iocsr_write32(EIOINTC_ROUTE + word * 4, every_byte(boot_core));
    }
    for word in 0..VECTORS / 32 {
        iocsr_write32(EIOINTC_ENABLE + word * 4, u32::MAX);
        iocsr_write32(EIOINTC_BOUNCE + word * 4, 0);
    }
    // SAFETY: setting `ECFG.LIE`'s bit for the pin only lets EIOINTC's interrupt reach the
    // entry, which is installed, once `CRMD.IE` is set.
    unsafe {
        core::arch::asm!(
            "csrxchg {value}, {mask}, 0x4",
            value = inout(reg) PIN_INTERRUPT => _,
            mask = in(reg) PIN_INTERRUPT,
            options(nomem, nostack),
        )
    };
}

/// Makes PCH-PIC line `line` raise EIOINTC vector `line`, level-triggered, active high, and
/// unmasks it.
fn route_wired(line: u32) -> Result<u32, &'static str> {
    if line >= PCH_PIC_LINES {
        return Err("no such PCH-PIC line");
    }
    start();
    let index = line as usize;
    // SAFETY: QEMU `virt`'s PCH-PIC registers through the uncached window; each line has a byte
    // of route and of vector, and one bit of edge, polarity and mask in 64-bit words.
    unsafe {
        core::ptr::write_volatile(pch_pic(PCH_PIC_VECTOR + index), line as u8);
        core::ptr::write_volatile(pch_pic(PCH_PIC_ROUTE + index), 1);
        let bit = 1u64 << line;
        let edge = pch_pic(PCH_PIC_EDGE) as *mut u64;
        core::ptr::write_volatile(edge, core::ptr::read_volatile(edge) & !bit);
        let polarity = pch_pic(PCH_PIC_POLARITY) as *mut u64;
        core::ptr::write_volatile(polarity, core::ptr::read_volatile(polarity) & !bit);
        let mask = pch_pic(PCH_PIC_MASK) as *mut u64;
        core::ptr::write_volatile(mask, core::ptr::read_volatile(mask) & !bit);
    }
    Ok(line)
}

/// Turns the UART's line on as the console's input interrupt.
pub(super) fn enable_console() -> Result<(), &'static str> {
    let vector = route_wired(UART_LINE)?;
    CONSOLE_VECTOR.store(vector, Ordering::Relaxed);
    Ok(())
}

/// [`crate::Platform::route_device_interrupt`]: a PCH-PIC specifier's first cell is its line.
pub(super) fn route_device(cells: &[u32]) -> Result<DeviceLine, &'static str> {
    let line = *cells.first().ok_or("an empty interrupt specifier")?;
    let vector = route_wired(line)?;
    DEVICE_LINES.claim(vector).map(|(line, _)| line).ok_or("every device line is taken")
}

/// [`crate::Platform::message_interrupt`]: the next free vector of PCH-MSI's, raised by writing
/// it to the doorbell.
pub(super) fn message() -> Result<(DeviceLine, InterruptMessage), &'static str> {
    start();
    let vector = NEXT_MSI.fetch_add(1, Ordering::Relaxed);
    if vector > MSI_LAST {
        return Err("every MSI vector is taken");
    }
    let (line, _) = DEVICE_LINES.claim(vector).ok_or("every device line is taken")?;
    Ok((
        line,
        InterruptMessage {
            address: PCH_MSI_DOORBELL,
            data: vector,
        },
    ))
}

/// Takes EIOINTC's pending vectors, clears them and hands each to the kernel: the console's
/// input, a routed device's line, or, for one the kernel never asked for, an exception report.
pub(super) fn handle(pc: u64) {
    for word in 0..VECTORS / 64 {
        let address = EIOINTC_ISR + word * 8;
        let mut pending = iocsr_read64(address);
        if pending == 0 {
            continue;
        }
        iocsr_write64(address, pending);
        while pending != 0 {
            let bit = pending.trailing_zeros();
            pending &= pending - 1;
            let vector = (word * 64) as u32 + bit;
            let delivered = if vector == CONSOLE_VECTOR.load(Ordering::Relaxed) {
                Some(crate::Interrupt::ConsoleInput)
            } else {
                DEVICE_LINES.line_of(vector).map(crate::Interrupt::Device)
            };
            match delivered {
                Some(interrupt) => crate::interrupt::deliver(interrupt),
                None => crate::exception::deliver(&crate::Exception {
                    kind: crate::ExceptionKind::Interrupt,
                    name: "EIOINTC vector nobody asked for",
                    code: u64::from(vector),
                    pc,
                    address: None,
                }),
            }
        }
    }
}

#[allow(dead_code)]
fn enabled_word(word: u64) -> u32 {
    iocsr_read32(EIOINTC_ENABLE + word * 4)
}
