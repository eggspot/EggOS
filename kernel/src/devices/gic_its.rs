//! A GICv3 Interrupt Translation Service as the machine's MSI controller (D618): found in the
//! device tree, given its command ring, its tables and the LPI tables, and asked for an MSI per
//! PCI function, which it maps to an LPI the boot CPU's redistributor raises as a device line.

use super::dma::DmaRegion;
use super::{DeviceId, DriverDescriptor, Match, Probe, ProbeError, ProbePass};
use crate::sync::LeafSpinLock;
use alloc::vec::Vec;
use eggos_drivers::dma::DmaMemory;
use eggos_drivers::interrupt::gic_its::{self, Its};
use eggos_drivers::io::Mmio;
use eggos_hal::{DeviceLine, InterruptMessage};

const START_TIMEOUT_MS: u64 = 1_000;
const COMMAND_TIMEOUT_MS: u64 = 100;
const ID_BITS: u32 = 14;
const LPIS: u32 = (1 << ID_BITS) - gic_its::FIRST_LPI;
const PENDING_TABLE_BYTES: usize = 0x1_0000;
const LPI_PRIORITY_ENABLED: u8 = 0xa0 | 1;
const EVENT_BITS: u32 = 5;
const EVENTS_PER_DEVICE: u32 = 1 << EVENT_BITS;
const ITT_ALIGNMENT: usize = 256;
const COLLECTION: u16 = 0;

/// The ITS of an Arm machine with a GICv3.
pub static ITS_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "gic-its",
    pass: ProbePass::InterruptController,
    matches: &[Match::Compatible(gic_its::COMPATIBLE)],
    probe: probe_its,
};

struct Device {
    id: u32,
    next_event: u32,
    _table: DmaRegion,
}

struct Service {
    its: Its<Mmio, DmaRegion>,
    translater: u64,
    target: u64,
    properties: DmaRegion,
    _pending: DmaRegion,
    _tables: Vec<DmaRegion>,
    devices: Vec<Device>,
    next_lpi: u32,
}

static SERVICE: LeafSpinLock<Option<Service>> = LeafSpinLock::new_leaf(None);

fn refused(error: gic_its::Error) -> ProbeError {
    ProbeError::Refused(match error {
        gic_its::Error::TimedOut => "the interrupt translation service did not answer in time",
        gic_its::Error::NoMemory => "no memory for the interrupt translation service",
    })
}

fn allocate(bytes: usize) -> Result<DmaRegion, ProbeError> {
    DmaRegion::allocate(bytes)
        .map_err(|_| ProbeError::Refused("no memory for the interrupt translation service"))
}

fn probe_its(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    if SERVICE.lock().is_some() {
        return Err(ProbeError::Refused("an interrupt translation service is in use"));
    }
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let region = node
        .regions()
        .next()
        .ok_or(ProbeError::Refused("the service has no registers"))?;
    if region.len() < gic_its::REGISTER_BYTES as u64 {
        return Err(ProbeError::Refused("the service's registers are too short"));
    }
    let physical = region.start;
    let base = probe.map_window(region)?;
    // SAFETY: `map_window` mapped the service's two register frames as device memory for good,
    // and only this driver reaches them, under `SERVICE`'s lock once probed.
    let regs = unsafe { Mmio::new(base, gic_its::REGISTER_BYTES) };
    let queue = allocate(gic_its::COMMAND_QUEUE_BYTES)?;
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let mut its = Its::new(regs, queue, &mut || deadline.keep_waiting()).map_err(
        |(error, queue)| {
            // SAFETY: the service was never given the ring.
            unsafe { queue.free() };
            refused(error)
        },
    )?;
    let mut tables = Vec::new();
    its.set_tables(&mut |bytes| {
        let table = DmaRegion::allocate(bytes).ok()?;
        let address = table.dma_addr().get();
        tables.push(table);
        Some(address)
    })
    .map_err(refused)?;
    let properties = allocate(LPIS as usize)?;
    let pending = allocate(PENDING_TABLE_BYTES)?;
    let (processor, redistributor) = probe.enable_lpis(
        properties.dma_addr().get(),
        pending.dma_addr().get(),
        ID_BITS,
    )?;
    let target = its.target(processor, redistributor);
    its.enable();
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let mut keep_waiting = || deadline.keep_waiting();
    its.run(gic_its::map_collection(COLLECTION, target), &mut keep_waiting)
        .map_err(refused)?;
    its.run(gic_its::synchronise(target), &mut keep_waiting)
        .map_err(refused)?;
    *SERVICE.lock() = Some(Service {
        its,
        translater: physical + gic_its::TRANSLATER as u64,
        target,
        properties,
        _pending: pending,
        _tables: tables,
        devices: Vec::new(),
        next_lpi: gic_its::FIRST_LPI,
    });
    Ok(())
}

/// Whether an ITS was found and set up.
pub fn is_present() -> bool {
    SERVICE.lock().is_some()
}

/// An MSI for the PCI function whose requester id is `requester` (bus, device and function,
/// as the ITS sees them for a single PCI segment): a new event of that device mapped to a new
/// LPI, enabled at priority `0xa0` and raised at the boot CPU; `route` makes the LPI a device
/// line. `None` without an ITS.
pub fn message_for(
    requester: u32,
    route: &mut dyn FnMut(u32) -> Result<DeviceLine, &'static str>,
    new_deadline: &mut dyn FnMut(u64) -> super::Deadline,
) -> Option<Result<(DeviceLine, InterruptMessage), ProbeError>> {
    let mut guard = SERVICE.lock();
    let service = guard.as_mut()?;
    Some(take_message(service, requester, route, new_deadline))
}

fn take_message(
    service: &mut Service,
    requester: u32,
    route: &mut dyn FnMut(u32) -> Result<DeviceLine, &'static str>,
    new_deadline: &mut dyn FnMut(u64) -> super::Deadline,
) -> Result<(DeviceLine, InterruptMessage), ProbeError> {
    if service.next_lpi >= gic_its::FIRST_LPI + LPIS {
        return Err(ProbeError::Refused("every LPI is taken"));
    }
    let mut deadline = new_deadline(COMMAND_TIMEOUT_MS);
    let mut keep_waiting = || deadline.keep_waiting();
    let index = match service.devices.iter().position(|device| device.id == requester) {
        Some(index) => index,
        None => {
            let bytes = (service.its.itt_entry_bytes() * EVENTS_PER_DEVICE as usize)
                .next_multiple_of(ITT_ALIGNMENT);
            let table = allocate(bytes)?;
            let command = gic_its::map_device(requester, table.dma_addr().get(), EVENT_BITS);
            service.its.run(command, &mut keep_waiting).map_err(refused)?;
            service.devices.push(Device {
                id: requester,
                next_event: 0,
                _table: table,
            });
            service.devices.len() - 1
        }
    };
    let event = service.devices[index].next_event;
    if event >= EVENTS_PER_DEVICE {
        return Err(ProbeError::Refused("the device has every event it may have"));
    }
    let lpi = service.next_lpi;
    let line = route(lpi).map_err(ProbeError::Refused)?;
    let configuration = service.properties.as_mut_ptr();
    // SAFETY: the configuration table holds one byte per LPI from the first; `lpi` is below
    // the last one (checked above), and the table is this service's alone.
    unsafe {
        configuration
            .add((lpi - gic_its::FIRST_LPI) as usize)
            .write_volatile(LPI_PRIORITY_ENABLED)
    };
    let commands = [
        gic_its::map_translation(requester, event, lpi, COLLECTION),
        gic_its::invalidate(requester, event),
        gic_its::synchronise(service.target),
    ];
    for command in commands {
        service.its.run(command, &mut keep_waiting).map_err(refused)?;
    }
    service.devices[index].next_event = event + 1;
    service.next_lpi = lpi + 1;
    Ok((
        line,
        InterruptMessage {
            address: service.translater,
            data: event,
        },
    ))
}
