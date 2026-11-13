//! RV32's PLIC (D569): the console's receive interrupt and the devices' lines, delivered to each
//! hart's supervisor-mode context (`2·hart + 1` on QEMU `virt`), mapped at its physical address
//! by a boot-table megapage, as the UART is.

use crate::interrupt::SourceLines;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::interrupt::{Plic, plic};
use eggos_drivers::io::Mmio;

/// The PLIC's physical address on QEMU `virt`; its first megapage holds the sources, the
/// enables and the first 512 contexts.
pub const PLIC: u32 = 0x0c00_0000;
const UART0_SOURCE: u32 = 10;
const CONSOLE_PRIORITY: u32 = 1;
const DEVICE_PRIORITY: u32 = 1;
const MAPPED_CONTEXTS: usize = 512;
const EXTERNAL_ENABLE: usize = 1 << 9;

static DEVICE_LINES: SourceLines = SourceLines::new();
static SOURCE_CONTEXTS: [AtomicUsize; crate::DeviceLine::COUNT] =
    [const { AtomicUsize::new(usize::MAX) }; crate::DeviceLine::COUNT];

/// Hart `hart`'s supervisor context.
pub fn context_of(hart: usize) -> usize {
    2 * hart + 1
}

fn plic(context: usize) -> Option<Plic<Mmio>> {
    if context >= MAPPED_CONTEXTS {
        return None;
    }
    let base = PLIC as usize;
    // SAFETY: the boot table maps the PLIC's first megapage at its physical address as device
    // memory; `context` is below the contexts it holds, and only this module drives the PLIC.
    let sources = unsafe { Mmio::new(base, plic::sources_and_enables_size(context)) };
    // SAFETY: as above, for the context's threshold and claim page.
    let registers =
        unsafe { Mmio::new(base + plic::context_offset(context), plic::CONTEXT_BLOCK_SIZE) };
    Some(Plic::new(sources, registers, context))
}

/// Lets this hart take external interrupts at every priority.
pub fn prepare_hart(hart: usize) {
    if let Some(plic) = plic(context_of(hart)) {
        plic.set_threshold(0);
    }
    // SAFETY: setting `sie.SEIE` only lets external interrupts reach the installed vector.
    unsafe { core::arch::asm!("csrs sie, {}", in(reg) EXTERNAL_ENABLE) };
}

/// Routes the console's receive interrupt to `hart`.
pub fn enable_console(hart: usize) -> Result<(), &'static str> {
    let plic = plic(context_of(hart)).ok_or("the hart has no PLIC context the kernel maps")?;
    plic.enable(UART0_SOURCE, CONSOLE_PRIORITY);
    prepare_hart(hart);
    Ok(())
}

/// Routes device source `source` to `hart` as a device line.
pub fn route_device(hart: usize, source: u32) -> Result<crate::DeviceLine, &'static str> {
    if source == UART0_SOURCE {
        return Err("the interrupt is the console's");
    }
    let context = context_of(hart);
    let plic = plic(context).ok_or("the hart has no PLIC context the kernel maps")?;
    let (line, new) = DEVICE_LINES
        .claim(source)
        .ok_or("every device interrupt line is taken")?;
    if new {
        plic.enable(source, DEVICE_PRIORITY);
        SOURCE_CONTEXTS[line.index()].store(context, Ordering::Release);
    }
    Ok(line)
}

/// Gives line `line` back: its source no longer interrupts.
pub fn release_device(line: crate::DeviceLine) {
    let Some(source) = DEVICE_LINES.source_of(line) else {
        return;
    };
    let context = SOURCE_CONTEXTS[line.index()].swap(usize::MAX, Ordering::AcqRel);
    if let Some(plic) = plic(context) {
        plic.disable(source);
    }
    DEVICE_LINES.release(line);
}

/// Moves line `line` to hart `hart`.
pub fn steer_device(line: crate::DeviceLine, hart: usize) -> Result<(), &'static str> {
    let source = DEVICE_LINES.source_of(line).ok_or("the line is not routed")?;
    let target = context_of(hart);
    let new = plic(target).ok_or("the hart has no PLIC context the kernel maps")?;
    new.enable(source, DEVICE_PRIORITY);
    let old = SOURCE_CONTEXTS[line.index()].swap(target, Ordering::AcqRel);
    if old != target
        && let Some(old) = plic(old)
    {
        old.disable(source);
    }
    Ok(())
}

/// Claims and delivers what this hart's context has pending.
pub fn interrupt(hart: usize) {
    let Some(plic) = plic(context_of(hart)) else {
        return;
    };
    while let Some(source) = plic.claim() {
        let delivered = match source {
            UART0_SOURCE => Some(crate::Interrupt::ConsoleInput),
            _ => DEVICE_LINES.line_of(source).map(crate::Interrupt::Device),
        };
        if let Some(delivered) = delivered {
            crate::interrupt::deliver(delivered);
        }
        plic.complete(source);
    }
}
