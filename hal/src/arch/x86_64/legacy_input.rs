//! The PC's 8042 keyboard controller (D239), at I/O ports 60h and 64h, polled: whichever of a
//! PS/2 keyboard and mouse answers. Machines whose `FADT` says they have no 8042 are left alone.

use super::port::PortIo;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_drivers::input::ps2::{self, Found, I8042};
use eggos_drivers::input::{LockLights, RawEvent};

const CONTROLLER_PORT: u16 = 0x60;
const FADT_IA_PC_BOOT_ARCH: usize = 109;
const BOOT_ARCH_8042: u16 = 1 << 1;
const FADT_REVISION: usize = 8;

struct ControllerSlot {
    busy: AtomicBool,
    controller: UnsafeCell<Option<I8042<PortIo>>>,
}

// SAFETY: the controller is reached only inside `with`, which `busy` makes exclusive.
unsafe impl Sync for ControllerSlot {}

impl ControllerSlot {
    fn with<R>(&self, use_it: impl FnOnce(&mut Option<I8042<PortIo>>) -> R) -> R {
        while self
            .busy
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: `busy` is held, so no other reference to the controller exists.
        let result = use_it(unsafe { &mut *self.controller.get() });
        self.busy.store(false, Ordering::Release);
        result
    }
}

static CONTROLLER: ControllerSlot = ControllerSlot {
    busy: AtomicBool::new(false),
    controller: UnsafeCell::new(None),
};

fn has_8042() -> bool {
    let Some(fadt) = super::acpi_table(b"FACP") else {
        return true;
    };
    let revision = fadt.get(FADT_REVISION).copied().unwrap_or(0);
    let flags = fadt
        .get(FADT_IA_PC_BOOT_ARCH..FADT_IA_PC_BOOT_ARCH + 2)
        .map_or(BOOT_ARCH_8042, |bytes| {
            u16::from_le_bytes([bytes[0], bytes[1]])
        });
    revision < 3 || flags & BOOT_ARCH_8042 != 0
}

pub(super) fn start() -> Found {
    if !has_8042() {
        return Found::default();
    }
    // SAFETY: ports 60h-64h are the 8042's data and status/command ports, which only this
    // module drives; the controller's interrupts are turned off by `start`, so the firmware's
    // handlers cannot race it.
    let ports = unsafe { PortIo::new(CONTROLLER_PORT, ps2::REGISTER_BLOCK_SIZE) };
    let mut controller = I8042::new(ports);
    let found = controller.start();
    if found.keyboard || found.mouse {
        CONTROLLER.with(|slot| *slot = Some(controller));
    }
    found
}

pub(super) fn poll(budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
    CONTROLLER.with(|slot| {
        slot.as_mut()
            .map_or(0, |controller| controller.poll(budget, event))
    })
}

pub(super) fn set_lights(lights: LockLights) -> bool {
    CONTROLLER.with(|slot| {
        slot.as_mut()
            .is_none_or(|controller| controller.set_lights(lights))
    })
}
