//! The PLIC's sources for the boot hart's supervisor-mode context, which the device tree names:
//! the console's receive interrupt (UART0's line) and the devices' lines the kernel routes.
//! When the device tree has a supervisor-level APLIC delivering MSIs (the Advanced Interrupt
//! Architecture, D532), the same sources go through it instead: each routed to the boot hart's
//! IMSIC with its source number as its identity, which the hart enables, and claims and clears
//! through its own CSRs (`siselect`/`sireg` and `stopei`).

use super::DIRECT_MAP_OFFSET;
use crate::interrupt::SourceLines;
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::sync::atomic::AtomicBool;
use eggos_drivers::interrupt::{Aplic, Plic, aplic, plic};
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
const NO_CONTEXT: usize = usize::MAX;

const DEVICE_PRIORITY: u32 = 1;

static CONTEXT: AtomicUsize = AtomicUsize::new(NO_CONTEXT);

static DEVICE_LINES: SourceLines = SourceLines::new();

static APLIC_BASE: AtomicUsize = AtomicUsize::new(0);
static AIA_READY: AtomicBool = AtomicBool::new(false);
const EXTERNAL_DELIVERY: u64 = 0x70;
const EXTERNAL_THRESHOLD: u64 = 0x72;
const EXTERNAL_ENABLES: u64 = 0xc0;
const IDENTITY_SHIFT: u64 = 16;
const IDENTITY_MASK: u64 = 0x7ff;
const ENABLE_BITS: u32 = 64;

/// Whether interrupts go through the AIA (D532): the device tree named a supervisor APLIC.
pub(super) fn uses_aia() -> bool {
    APLIC_BASE.load(Ordering::Relaxed) != 0
}

fn aplic() -> Option<Aplic<Mmio>> {
    let base = APLIC_BASE.load(Ordering::Relaxed);
    // SAFETY: `APLIC_BASE` is the supervisor APLIC's registers in the direct map, which
    // `device_regions` gave the kernel's page tables as device memory; only this module drives
    // the APLIC.
    (base != 0).then(|| Aplic::new(unsafe { Mmio::new(base, aplic::REGISTER_BLOCK_SIZE) }))
}

fn write_indirect(select: u64, value: u64) {
    // SAFETY: `siselect` and `sireg` reach this hart's IMSIC interrupt file, which only this
    // module drives; the register `select` names is one of its supervisor-level ones.
    unsafe {
        core::arch::asm!(
            "csrw 0x150, {select}",
            "csrw 0x151, {value}",
            select = in(reg) select,
            value = in(reg) value,
            options(nomem, nostack),
        );
    }
}

fn set_indirect_bits(select: u64, bits: u64, set: bool) {
    // SAFETY: as in `write_indirect`; only the given bits of the selected register change.
    unsafe {
        core::arch::asm!("csrw 0x150, {}", in(reg) select, options(nomem, nostack));
        match set {
            true => core::arch::asm!("csrs 0x151, {}", in(reg) bits, options(nomem, nostack)),
            false => core::arch::asm!("csrc 0x151, {}", in(reg) bits, options(nomem, nostack)),
        }
    }
}

fn enable_identity(identity: u32, enable: bool) {
    let select = EXTERNAL_ENABLES + 2 * u64::from(identity / ENABLE_BITS);
    set_indirect_bits(select, 1 << (identity % ENABLE_BITS), enable);
}

/// Claims and clears the highest-priority pending identity of this hart's IMSIC; 0 for none.
fn claim_identity() -> u32 {
    let top: u64;
    // SAFETY: swapping `stopei` with zero claims the identity it reports, which the caller
    // handles; it touches only this hart's IMSIC state.
    unsafe { core::arch::asm!("csrrw {}, 0x15c, zero", out(reg) top, options(nomem, nostack)) };
    ((top >> IDENTITY_SHIFT) & IDENTITY_MASK) as u32
}

fn aia_ready() {
    if !AIA_READY.swap(true, Ordering::Relaxed) {
        if let Some(aplic) = aplic() {
            aplic.enable_msi_delivery();
        }
        write_indirect(EXTERNAL_DELIVERY, 1);
        write_indirect(EXTERNAL_THRESHOLD, 0);
        // SAFETY: setting `sie.SEIE` only lets this hart take the external interrupts its IMSIC
        // delivers, which `interrupt` handles.
        unsafe { core::arch::asm!("csrs sie, {}", in(reg) SIE_SEIE, options(nomem, nostack)) };
    }
}

fn aia_route(source: u32, mode: aplic::SourceMode) {
    let Some(aplic) = aplic() else {
        return;
    };
    aia_ready();
    let hart = super::BOOT_HART.load(Ordering::Relaxed) as u32;
    enable_identity(source, true);
    aplic.route(source, mode, hart, source);
}

/// The PLIC pages the kernel maps: the source priorities and the enables up to the last mapped
/// context's, then the mapped contexts' threshold and claim pages.
pub(super) fn device_regions(region: &mut dyn FnMut(PhysicalRange)) {
    let base = PLIC_BASE as u64;
    if let Some((address, bytes)) =
        super::device_tree_bytes().and_then(crate::fdt::supervisor_aplic)
    {
        let bytes = bytes.min(aplic::REGISTER_BLOCK_SIZE as u64);
        region(PhysicalRange::from_start_length(address, bytes));
        APLIC_BASE.store(DIRECT_MAP_OFFSET + address as usize, Ordering::Relaxed);
    }
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

fn context_plic(context: Option<usize>) -> Result<Plic<Mmio>, &'static str> {
    let context = context.ok_or("the device tree names no PLIC context for the boot hart")?;
    if context >= MAPPED_CONTEXTS {
        return Err("the boot hart's PLIC context is beyond the ones the kernel maps");
    }
    let plic = plic(context);
    if CONTEXT.swap(context, Ordering::Relaxed) != context {
        plic.set_threshold(EVERY_PRIORITY_ABOVE_ZERO);
        // SAFETY: setting `sie.SEIE` only lets this hart take the external interrupts its PLIC
        // context routes to it, which `interrupt` handles.
        unsafe { core::arch::asm!("csrs sie, {}", in(reg) SIE_SEIE, options(nomem, nostack)) };
    }
    Ok(plic)
}

/// Routes UART0's source to `context`, the calling hart's supervisor-mode context.
pub(super) fn enable(context: Option<usize>) -> Result<(), &'static str> {
    if uses_aia() {
        aia_route(UART0_SOURCE, aplic::SourceMode::LevelHigh);
        return Ok(());
    }
    context_plic(context)?.enable(UART0_SOURCE, UART0_PRIORITY);
    Ok(())
}

/// Routes the device source `source` to `context`, the calling hart's supervisor-mode context,
/// as a device's line; a source routed before keeps its line.
pub(super) fn route_device(
    context: Option<usize>,
    source: u32,
    mode: aplic::SourceMode,
) -> Result<crate::DeviceLine, &'static str> {
    if source == UART0_SOURCE {
        return Err("the interrupt is the console's");
    }
    let plic = match uses_aia() {
        true => None,
        false => Some(context_plic(context)?),
    };
    let (line, new) = DEVICE_LINES
        .claim(source)
        .ok_or("every device interrupt line is taken")?;
    if new {
        match &plic {
            Some(plic) => plic.enable(source, DEVICE_PRIORITY),
            None => aia_route(source, mode),
        }
    }
    Ok(line)
}

/// Gives `line` back: its source stops interrupting the boot hart's context (D413).
pub(super) fn release_device(context: Option<usize>, line: crate::DeviceLine) {
    let Some(source) = DEVICE_LINES.release(line) else {
        return;
    };
    if let Some(aplic) = aplic() {
        aplic.disable(source);
        enable_identity(source, false);
        return;
    }
    if let Ok(plic) = context_plic(context) {
        plic.disable(source);
    }
}

/// Claims the pending external interrupt and handles it: UART0's goes to the kernel as console
/// input, a routed device's as its line, and the kernel quiets the device before the claim is
/// completed; any other source is reported as an unexpected interrupt at `pc`. Out of line and
/// called last, as a tail call, so the trap paths keep their length.
#[cold]
#[inline(never)]
pub(super) fn interrupt(pc: u64) {
    if uses_aia() {
        loop {
            let identity = claim_identity();
            if identity == 0 {
                return;
            }
            deliver_source(identity, pc);
        }
    }
    let context = CONTEXT.load(Ordering::Relaxed);
    if context == NO_CONTEXT {
        return;
    }
    let plic = plic(context);
    let Some(source) = plic.claim() else {
        return;
    };
    let delivered = delivered_as(source);
    if let Some(delivered) = delivered {
        crate::interrupt::deliver(delivered);
    }
    plic.complete(source);
    if delivered.is_none() {
        report_unexpected("unexpected PLIC source", source, pc);
    }
}

fn delivered_as(source: u32) -> Option<crate::Interrupt> {
    if source == UART0_SOURCE {
        Some(crate::Interrupt::ConsoleInput)
    } else {
        DEVICE_LINES.line_of(source).map(crate::Interrupt::Device)
    }
}

fn report_unexpected(name: &'static str, source: u32, pc: u64) {
    crate::exception::deliver(&crate::Exception {
        kind: crate::ExceptionKind::Interrupt,
        name,
        code: source.into(),
        pc,
        address: None,
    });
}

fn deliver_source(identity: u32, pc: u64) {
    match delivered_as(identity) {
        Some(delivered) => crate::interrupt::deliver(delivered),
        None => report_unexpected("unexpected AIA identity", identity, pc),
    }
}
