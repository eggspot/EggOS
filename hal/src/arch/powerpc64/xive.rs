//! XIVE, POWER9's interrupt controller, through OPAL in exploitation mode (D312): an event
//! queue at priority 7 for the boot CPU, which every source is routed to at first, and one for
//! each CPU started after it, which the kernel then spreads the sources over (D571); the hypervisor ring of
//! the thread interrupt management area (TIMA) acknowledges, and each source's event state
//! buffer (ESB) page is unmasked and EOIed by loads and stores at fixed offsets. The PHB's MSIs
//! are XIVE sources, so a device's MSI-X entry writes the address and data OPAL gives for its
//! source.

use super::{DIRECT_MAP_OFFSET, opal, pir};
use crate::atomic::AtomicU64;
use crate::interrupt::{DeviceLine, InterruptMessage, SourceLines};
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const PRIORITY: u64 = 7;
const QUEUE_ORDER: u64 = 12;
const QUEUE_ENTRIES: usize = 1 << (QUEUE_ORDER - 2);
const QUEUE_ENABLED: u64 = 1;
const QUEUE_ALWAYS_NOTIFY: u64 = 2;
const ACK_HV: usize = 0x830;
const CPPR_HV_PHYS: usize = 0x31;
const ACCEPT_ALL: u8 = 0xff;
const PENDING_PHYSICAL: u16 = 1;
const ESB_LOAD_EOI: usize = 0x000;
const ESB_STORE_EOI: usize = 0x400;
const ESB_SET_PQ_00: usize = 0xc00;
const IRQ_STORE_EOI: u64 = 2;
/// The MSIs prepared at boot, whose ESB pages the kernel maps.
pub(super) const PREPARED_MSIS: usize = 32;

#[repr(C, align(4096))]
struct Queue([u32; QUEUE_ENTRIES]);

/// The CPUs that get an event queue, the boot CPU's first.
const QUEUED_CPUS: usize = 8;
const NO_PIR: u64 = u64::MAX;

static mut QUEUES: [Queue; QUEUED_CPUS] = [const { Queue([0; QUEUE_ENTRIES]) }; QUEUED_CPUS];
static QUEUE_INDEX: [AtomicU32; QUEUED_CPUS] = [const { AtomicU32::new(0) }; QUEUED_CPUS];
static QUEUE_TOGGLE: [AtomicU32; QUEUED_CPUS] = [const { AtomicU32::new(0) }; QUEUED_CPUS];
static QUEUE_PIRS: [AtomicU64; QUEUED_CPUS] = [const { AtomicU64::new(NO_PIR) }; QUEUED_CPUS];
static NEXT_QUEUE: AtomicU32 = AtomicU32::new(1);
static TIMA: AtomicU64 = AtomicU64::new(0);
static TIMA_LENGTH: AtomicU64 = AtomicU64::new(0);
static ESB_SIZE: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static CPPR_SET: AtomicBool = AtomicBool::new(false);
static DEVICE_LINES: SourceLines = SourceLines::new();
static NEXT_MSI: AtomicU32 = AtomicU32::new(0);
static MSI_SOURCES: [AtomicU64; PREPARED_MSIS] = [const { AtomicU64::new(0) }; PREPARED_MSIS];
static MSI_PAGES: [AtomicU64; PREPARED_MSIS] = [const { AtomicU64::new(0) }; PREPARED_MSIS];
static MSI_FLAGS: [AtomicU64; PREPARED_MSIS] = [const { AtomicU64::new(0) }; PREPARED_MSIS];
static BOOT_PIR: AtomicU64 = AtomicU64::new(0);

/// Resets XIVE to exploitation mode and gives the boot CPU its queue, once, at boot; then, on
/// every call, reports the TIMA and the prepared MSIs' ESB pages, which the kernel maps as device
/// memory in the direct map (D33).
pub(super) fn prepare(blob: Option<&[u8]>, region: &mut dyn FnMut(PhysicalRange)) {
    if !STARTED.swap(true, Ordering::AcqRel)
        && let Some(blob) = blob
    {
        start(blob);
    }
    let tima = TIMA.load(Ordering::Relaxed);
    if tima != 0 {
        region(PhysicalRange::from_start_length(
            tima,
            TIMA_LENGTH.load(Ordering::Relaxed),
        ));
    }
    let size = ESB_SIZE.load(Ordering::Relaxed);
    for page in &MSI_PAGES {
        match page.load(Ordering::Relaxed) {
            0 => {}
            page => region(PhysicalRange::from_start_length(page, size)),
        }
    }
}

fn start(blob: &[u8]) {
    if !opal::xive_reset() {
        return;
    }
    let Some((tima, length)) = tima_window(blob) else {
        return;
    };
    TIMA.store(tima, Ordering::Relaxed);
    TIMA_LENGTH.store(length, Ordering::Relaxed);
    BOOT_PIR.store(pir(), Ordering::Relaxed);
    give_queue(0, pir());
    let Some((msi_base, msi_count)) = super::pci::first_phb_msis(blob) else {
        return;
    };
    for index in 0..PREPARED_MSIS.min(msi_count as usize) {
        let source = msi_base + index as u64;
        let Some(info) = opal::xive_irq_info(source) else {
            continue;
        };
        ESB_SIZE.store(1u64 << info.esb_shift, Ordering::Relaxed);
        MSI_SOURCES[index].store(source, Ordering::Relaxed);
        MSI_PAGES[index].store(info.eoi_page, Ordering::Relaxed);
        MSI_FLAGS[index].store(info.flags, Ordering::Relaxed);
    }
}

fn give_queue(slot: usize, pir: u64) {
    // SAFETY: only the address of queue `slot` is taken; XIVE writes it, its CPU reads it.
    let queue = unsafe { (&raw const QUEUES[slot]) as u64 } - super::KERNEL_VIRTUAL_OFFSET;
    QUEUE_PIRS[slot].store(pir, Ordering::Release);
    opal::xive_set_queue(
        pir,
        PRIORITY,
        queue,
        QUEUE_ORDER,
        QUEUE_ENABLED | QUEUE_ALWAYS_NOTIFY,
    );
}

fn slot_of(pir: u64) -> Option<usize> {
    QUEUE_PIRS
        .iter()
        .position(|slot| slot.load(Ordering::Acquire) == pir)
}

/// Gives the calling (secondary) CPU its own event queue and lets it take every priority
/// (D571), so sources can be steered to it.
pub(super) fn prepare_cpu() {
    if TIMA.load(Ordering::Relaxed) == 0 {
        return;
    }
    let slot = NEXT_QUEUE.fetch_add(1, Ordering::Relaxed) as usize;
    if slot >= QUEUED_CPUS {
        return;
    }
    give_queue(slot, pir());
    // SAFETY: as in `accept_all`, for this CPU's own thread context.
    unsafe { core::ptr::write_volatile(tima(CPPR_HV_PHYS) as *mut u8, ACCEPT_ALL) };
}

/// [`crate::Platform::steer_device_interrupt`] (D571): line `line`'s source goes to the queue
/// of the CPU with PIR `cpu`, when that CPU has one.
pub(super) fn steer_device(line: DeviceLine, cpu: u64) -> Result<(), &'static str> {
    let source = DEVICE_LINES.source_of(line).ok_or("the line is not routed")?;
    slot_of(cpu).ok_or("the CPU has no event queue")?;
    match opal::xive_set_irq(u64::from(source), cpu, PRIORITY, u64::from(source)) {
        true => Ok(()),
        false => Err("OPAL refused to move the source"),
    }
}

fn tima_window(blob: &[u8]) -> Option<(u64, u64)> {
    let tree = crate::fdt::Fdt::new(blob).ok()?;
    let node = tree
        .nodes()
        .map_while(Result::ok)
        .find(|node| node.is_compatible(b"ibm,opal-xive-pe"))?;
    node.reg_entries().nth(1)
}

fn tima(offset: usize) -> usize {
    DIRECT_MAP_OFFSET + TIMA.load(Ordering::Relaxed) as usize + offset
}

fn esb(index: usize, offset: usize) -> usize {
    DIRECT_MAP_OFFSET + MSI_PAGES[index].load(Ordering::Relaxed) as usize + offset
}

fn accept_all() {
    if TIMA.load(Ordering::Relaxed) == 0 || CPPR_SET.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the TIMA's hypervisor window is mapped as device memory since boot; writing the
    // physical thread's CPPR only lets interrupts of every priority through.
    unsafe { core::ptr::write_volatile(tima(CPPR_HV_PHYS) as *mut u8, ACCEPT_ALL) };
}

/// [`crate::Platform::message_interrupt`]: the next prepared PHB MSI, routed to the boot CPU's
/// queue with its source number as the queue's data, unmasked; OPAL gives its address and data.
pub(super) fn message() -> Result<(DeviceLine, InterruptMessage), &'static str> {
    accept_all();
    let index = NEXT_MSI.fetch_add(1, Ordering::Relaxed) as usize;
    if index >= PREPARED_MSIS || MSI_PAGES[index].load(Ordering::Relaxed) == 0 {
        return Err("no prepared MSI left");
    }
    let source = MSI_SOURCES[index].load(Ordering::Relaxed);
    let (address, data) =
        super::pci::msi_message(index as u64).ok_or("OPAL gave no MSI address")?;
    if !opal::xive_set_irq(source, BOOT_PIR.load(Ordering::Relaxed), PRIORITY, source) {
        return Err("OPAL refused to route the MSI");
    }
    let (line, _) = DEVICE_LINES
        .claim(source as u32)
        .ok_or("every device line is taken")?;
    // SAFETY: the source's ESB page is mapped as device memory since boot; this load sets its
    // PQ bits to 00, unmasking it.
    unsafe { core::ptr::read_volatile(esb(index, ESB_SET_PQ_00) as *const u64) };
    Ok((line, InterruptMessage { address, data }))
}

/// Takes the pending physical interrupts: drains the queue, hands each source's line to the
/// kernel and EOIs it, until the TIMA says nothing more is pending.
pub(super) fn handle() {
    if TIMA.load(Ordering::Relaxed) == 0 {
        return;
    }
    let Some(slot) = slot_of(pir()) else {
        return;
    };
    loop {
        // SAFETY: reading the hypervisor acknowledge register acknowledges the pending
        // interrupt, which this loop then handles.
        let ack = u16::from_be(unsafe { core::ptr::read_volatile(tima(ACK_HV) as *const u16) });
        if (ack >> 14) != PENDING_PHYSICAL {
            break;
        }
        drain(slot);
        // SAFETY: as in `accept_all`.
        unsafe { core::ptr::write_volatile(tima(CPPR_HV_PHYS) as *mut u8, ACCEPT_ALL) };
    }
}

fn drain(slot: usize) {
    loop {
        let index = QUEUE_INDEX[slot].load(Ordering::Relaxed) as usize;
        // SAFETY: queue `slot` is this module's, written by XIVE and read by its CPU alone.
        let entry =
            u32::from_be(unsafe { core::ptr::read_volatile(&raw const QUEUES[slot].0[index]) });
        let toggle = QUEUE_TOGGLE[slot].load(Ordering::Relaxed);
        if entry >> 31 == toggle {
            return;
        }
        let next = (index + 1) % QUEUE_ENTRIES;
        QUEUE_INDEX[slot].store(next as u32, Ordering::Relaxed);
        if next == 0 {
            QUEUE_TOGGLE[slot].store(toggle ^ 1, Ordering::Relaxed);
        }
        let source = entry & 0x7fff_ffff;
        if let Some(line) = DEVICE_LINES.line_of(source) {
            crate::interrupt::deliver(crate::Interrupt::Device(line));
        }
        end_of_interrupt(u64::from(source));
    }
}

fn end_of_interrupt(source: u64) {
    let Some(index) =
        (0..PREPARED_MSIS).find(|&index| MSI_SOURCES[index].load(Ordering::Relaxed) == source)
    else {
        return;
    };
    // SAFETY: the source's ESB page is mapped as device memory; an EOI load or store only
    // re-arms the source.
    unsafe {
        match MSI_FLAGS[index].load(Ordering::Relaxed) & IRQ_STORE_EOI {
            0 => {
                core::ptr::read_volatile(esb(index, ESB_LOAD_EOI) as *const u64);
            }
            _ => core::ptr::write_volatile(esb(index, ESB_STORE_EOI) as *mut u64, 0),
        }
    }
}
