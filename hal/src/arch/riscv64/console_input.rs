//! The console's receive interrupt: UART0's line through the PLIC to the boot hart's
//! supervisor-mode context, which the device tree names.

use super::DIRECT_MAP_OFFSET;
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_drivers::interrupt::{Plic, plic};
use eggos_drivers::io::Mmio;

/// QEMU `virt`: the PLIC's registers. Device-tree discovery replaces it in M5.
const PLIC_BASE: usize = 0x0c00_0000;
/// QEMU `virt`: UART0's PLIC source.
const UART0_SOURCE: u32 = 10;
const UART0_PRIORITY: u32 = 1;
const EVERY_PRIORITY_ABOVE_ZERO: u32 = 0;
/// The PLIC contexts whose registers the kernel maps: 16 harts' worth, two contexts each.
const MAPPED_CONTEXTS: usize = 32;
/// `sie` bit 9: supervisor external interrupt enable.
const SIE_SEIE: u64 = 1 << 9;
const NONE: usize = usize::MAX;

/// The boot hart's supervisor-mode context, once [`enable`] has routed UART0 there.
static CONTEXT: AtomicUsize = AtomicUsize::new(NONE);

/// The PLIC pages the kernel maps: the source priorities and the enables up to the last mapped
/// context's, then the mapped contexts' threshold and claim pages.
pub(super) fn device_regions(region: &mut dyn FnMut(PhysicalRange)) {
    let base = PLIC_BASE as u64;
    region(PhysicalRange::from_start_length(
        base,
        plic::sources_and_enables_size(MAPPED_CONTEXTS - 1) as u64,
    ));
    region(PhysicalRange::from_start_length(
        base + plic::context_offset(0) as u64,
        (MAPPED_CONTEXTS * plic::CONTEXT_BLOCK_SIZE) as u64,
    ));
}

fn plic(context: usize) -> Plic<Mmio> {
    let base = DIRECT_MAP_OFFSET + PLIC_BASE;
    // SAFETY: `context` is below MAPPED_CONTEXTS, so the sources and its enables lie in the
    // pages `device_regions` gives the kernel's page tables, which map them as device memory at
    // the direct map offset; only this module drives the PLIC.
    let sources = unsafe { Mmio::new(base, plic::sources_and_enables_size(context)) };
    // SAFETY: as above, for the context's threshold and claim page.
    let context_regs = unsafe {
        Mmio::new(
            base + plic::context_offset(context),
            plic::CONTEXT_BLOCK_SIZE,
        )
    };
    Plic::new(sources, context_regs, context)
}

/// Routes UART0's source to the supervisor-mode context of `hart`, the calling hart, and lets
/// the hart take supervisor external interrupts.
pub(super) fn enable(hart: u64, context: Option<usize>) -> Result<(), &'static str> {
    let context = context.ok_or("the device tree names no PLIC context for the boot hart")?;
    if context >= MAPPED_CONTEXTS {
        return Err("the boot hart's PLIC context is beyond the ones the kernel maps");
    }
    let plic = plic(context);
    plic.set_threshold(EVERY_PRIORITY_ABOVE_ZERO);
    plic.enable(UART0_SOURCE, UART0_PRIORITY);
    CONTEXT.store(context, Ordering::Relaxed);
    debug_assert_eq!(hart, super::BOOT_HART.load(Ordering::Relaxed));
    // SAFETY: setting `sie.SEIE` only lets this hart take the external interrupts its PLIC
    // context now routes to it, which `interrupt` handles.
    unsafe { core::arch::asm!("csrs sie, {}", in(reg) SIE_SEIE, options(nomem, nostack)) };
    Ok(())
}

/// Claims the pending external interrupt and handles it; returns the source when it was not
/// UART0's, to be reported. The claim is completed after the kernel has read the UART. Out of
/// line, so the trap paths keep their length.
#[cold]
#[inline(never)]
pub(super) fn interrupt() -> Option<u32> {
    let context = CONTEXT.load(Ordering::Relaxed);
    if context == NONE {
        return None;
    }
    let plic = plic(context);
    let source = plic.claim()?;
    if source == UART0_SOURCE {
        crate::interrupt::deliver(crate::Interrupt::ConsoleInput);
    }
    plic.complete(source);
    (source != UART0_SOURCE).then_some(source)
}
