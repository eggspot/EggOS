//! GPIO pin controllers (D789): AMD's (`AMDI0030`) and Intel's from Cannon Lake on, found on
//! the ACPI bus before the devices whose interrupts come through their pins. Each controller's
//! own interrupt (the global system interrupt its `_CRS` names) wakes a thread of its own, which
//! finds the armed pins that fired, acknowledges them and raises each one's device events; a
//! device arms its pin with [`bind`]. Intel's communities are the controller's memory ranges in
//! order, each counted as the pads its range holds past `PADBAR`.

use super::{DeviceId, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::devices::interrupts::{self, DeviceEvents, Handler};
use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_drivers::gpio::amd::AmdGpio;
use eggos_drivers::gpio::intel::{Community, IntelGpio};
use eggos_drivers::gpio::{PinController, Trigger};
use eggos_drivers::io::{Mmio, RegisterIo};
use eggos_hal::Platform;

const MOST_CONTROLLERS: usize = 2;
const MOST_INTEL_PADS: u16 = 512;
const INTEL_PAD_BYTES: u64 = 16;
const INTEL_PAD_BASE: usize = 0x00c;
const SERVICE_WAIT_MS: u64 = 1000;

/// GPIO pin controllers on the ACPI bus.
pub static GPIO_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "gpio",
    pass: ProbePass::Bus,
    matches: &[
        Match::AcpiHid(b"AMDI0030"),
        Match::AcpiHid(b"AMD0030"),
        Match::AcpiHid(b"INT34BB"),
        Match::AcpiHid(b"INT3450"),
        Match::AcpiHid(b"INT3455"),
        Match::AcpiHid(b"INT34C5"),
        Match::AcpiHid(b"INT34C6"),
        Match::AcpiHid(b"INTC1055"),
        Match::AcpiHid(b"INTC1056"),
        Match::AcpiHid(b"INTC1057"),
        Match::AcpiHid(b"INTC1085"),
    ],
    probe: probe_gpio,
};

struct Controller {
    path: [u8; super::acpi_bus::PATH_BYTES],
    pins: Box<dyn PinController + Send>,
    bound: Vec<(u16, &'static DeviceEvents)>,
    events: Option<&'static DeviceEvents>,
}

static CONTROLLERS: SpinLock<Vec<Controller>> = SpinLock::new(Vec::new());
static CONTROLLER_EVENTS: [DeviceEvents; MOST_CONTROLLERS] =
    [DeviceEvents::new(), DeviceEvents::new()];

fn acknowledge_nothing(_: usize) {}

fn probe_gpio(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Acpi {
        hid,
        path,
        interrupt,
        ..
    } = node.identity
    else {
        return Err(ProbeError::Refused("not an ACPI device"));
    };
    let index = CONTROLLERS.lock().len();
    if index >= MOST_CONTROLLERS {
        return Err(ProbeError::Refused("no room for another GPIO controller"));
    }
    let regions: Vec<_> = node
        .regions
        .iter()
        .copied()
        .filter(|region| region.len() != 0)
        .collect();
    let first = *regions
        .first()
        .ok_or(ProbeError::Refused("the GPIO controller names no registers"))?;
    let amd = super::acpi_bus::text(&hid).starts_with(b"AMD");
    let pins: Box<dyn PinController + Send> = match amd {
        true => {
            let base = probe.map_window(first)?;
            // SAFETY: the window maps the controller's registers as device memory, for good.
            Box::new(AmdGpio::new(unsafe { Mmio::new(base, first.len() as usize) }))
        }
        false => {
            let mut communities = Vec::new();
            for region in regions {
                let base = probe.map_window(region)?;
                // SAFETY: the window maps one community's registers as device memory, for good.
                let regs = unsafe { Mmio::new(base, region.len() as usize) };
                let pad_base = u64::from(regs.read32(INTEL_PAD_BASE));
                let pads = (region.len().saturating_sub(pad_base) / INTEL_PAD_BYTES)
                    .min(u64::from(MOST_INTEL_PADS)) as u16;
                communities.push(Community { regs, pads });
            }
            Box::new(IntelGpio::new(communities))
        }
    };
    let events = &CONTROLLER_EVENTS[index];
    let mut connected = None;
    if let Some(global) = interrupt {
        let line = probe.legacy_interrupt(global)?;
        events.set_masked_line(line);
        let handler = Handler {
            acknowledge: acknowledge_nothing,
            context: 0,
            events,
        };
        match interrupts::connect(line, handler) {
            Ok(()) => {
                probe.interrupt_connected(line);
                connected = Some(events);
            }
            Err(_) => probe.release_interrupt(line),
        }
    }
    CONTROLLERS.lock().push(Controller {
        path,
        pins,
        bound: Vec::new(),
        events: connected,
    });
    Ok(())
}

/// Arms pin `pin` of the controller at namespace path `controller` as `trigger` says and has
/// its interrupt raise `events`; false when no such controller or pin answers.
pub fn bind(
    controller: &[u8; super::acpi_bus::PATH_BYTES],
    pin: u16,
    trigger: Trigger,
    events: &'static DeviceEvents,
) -> bool {
    let mut controllers = CONTROLLERS.lock();
    let Some(found) = controllers.iter_mut().find(|each| each.path == *controller) else {
        return false;
    };
    if found.events.is_none() || !found.pins.arm(pin, trigger) {
        return false;
    }
    found.bound.push((pin, events));
    true
}

fn service<P: Platform>(index: usize) {
    let mut fired: Vec<&'static DeviceEvents> = Vec::new();
    let mut controllers = CONTROLLERS.lock();
    let Some(controller) = controllers.get_mut(index) else {
        return;
    };
    for &(pin, events) in &controller.bound {
        if controller.pins.pending(pin) {
            controller.pins.acknowledge(pin);
            fired.push(events);
        }
    }
    controller.pins.end_of_interrupt();
    drop(controllers);
    for events in fired {
        events.raise::<P>();
    }
}

/// Starts each connected controller's thread, which serves its interrupt.
pub fn start<P: Platform>() {
    let count = CONTROLLERS.lock().len();
    for index in 0..count {
        let events = CONTROLLERS.lock()[index].events;
        let Some(events) = events else {
            continue;
        };
        let _ = crate::thread::spawn::<P, _>(move || {
            let mut seen = events.raised();
            loop {
                seen = events.wait::<P>(seen, crate::net::ticks_after::<P>(SERVICE_WAIT_MS));
                service::<P>(index);
            }
        });
    }
}
