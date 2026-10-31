//! The power button (D202): ACPI's fixed-feature button, whose press sets `PWRBTN_STS` in the
//! PM1a status register and raises the system control interrupt, an ISA line the I/O APIC
//! routes to the boot CPU. QEMU's `system_powerdown` presses it.

use super::console_input::io_apic;
use super::{acpi_table, port, timer};
use crate::acpi::{self, PowerButton};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_drivers::interrupt::io_apic::LineSignal;

const ACPI_ENABLE_POLLS: u32 = 1_000_000;

/// The PM1a status port and enable port, packed, once the button is on; 0 before.
static PORTS: AtomicU32 = AtomicU32::new(0);

/// Switches the machine to ACPI mode if it is not, turns the power button's event on and routes
/// the system control interrupt to the boot CPU as [`timer::POWER_BUTTON_VECTOR`].
pub(super) fn enable() -> Result<(), &'static str> {
    let fadt = acpi_table(b"FACP").ok_or("the firmware gives no ACPI FADT")?;
    let button: PowerButton =
        acpi::power_button(fadt).ok_or("the power button is not ACPI's fixed feature")?;
    let control = acpi::parse_fadt(fadt).ok_or("the FADT is too short")?;
    let (Ok(pm1a_control), Ok(smi_command)) = (
        u16::try_from(control.pm1a_control_port),
        u16::try_from(control.smi_command_port),
    ) else {
        return Err("the PM1 control port is not in I/O space");
    };
    // SAFETY: the FADT's PM1a control port, read only.
    let in_legacy_mode = unsafe { port::in16(pm1a_control) } & acpi::PM1_CONTROL_SCI_EN == 0;
    if in_legacy_mode && smi_command != 0 && control.acpi_enable != 0 {
        // SAFETY: writing the FADT's ACPI_ENABLE value to its SMI command port is the spec's
        // handshake for switching to ACPI mode; the kernel then owns the fixed hardware.
        unsafe { port::out8(smi_command, control.acpi_enable) };
        for _ in 0..ACPI_ENABLE_POLLS {
            // SAFETY: as above, a read.
            if unsafe { port::in16(pm1a_control) } & acpi::PM1_CONTROL_SCI_EN != 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
    let madt = acpi_table(b"APIC").ok_or("the firmware gives no ACPI MADT")?;
    let line = acpi::madt_isa_interrupt(madt, button.sci_line);
    let (apic, controller) = acpi::madt_io_apics(madt)
        .map(|apic| (apic, io_apic(apic)))
        .find(|(apic, controller)| {
            line.global_interrupt
                .checked_sub(apic.first_interrupt)
                .is_some_and(|input| input < controller.lines())
        })
        .ok_or("no I/O APIC receives the system control interrupt")?;
    // SAFETY: the FADT's PM1a status and enable ports: writing the power button's bit to the
    // status register clears a stale press, and to the enable register turns the event on;
    // every other event stays as the firmware left it, off.
    unsafe {
        port::out16(button.pm1a_status_port, acpi::PM1_POWER_BUTTON);
        let enabled = port::in16(button.pm1a_enable_port);
        port::out16(button.pm1a_enable_port, enabled | acpi::PM1_POWER_BUTTON);
    }
    PORTS.store(
        u32::from(button.pm1a_status_port) | u32::from(button.pm1a_enable_port) << 16,
        Ordering::Relaxed,
    );
    controller.route(
        line.global_interrupt - apic.first_interrupt,
        LineSignal {
            active_low: line.active_low,
            level_triggered: line.level_triggered,
        },
        timer::POWER_BUTTON_VECTOR,
        timer::boot_apic_id(),
    );
    Ok(())
}

/// Handles the system control interrupt: a press clears its status bit, which lets the line
/// go, and reaches the kernel; then the local APIC's end of interrupt.
pub(super) fn interrupt() {
    let ports = PORTS.load(Ordering::Relaxed);
    let status_port = ports as u16;
    if status_port != 0 {
        // SAFETY: the PM1a status port `enable` found in the FADT: reading it, and writing the
        // power button's bit back, clears the press.
        let pressed = unsafe { port::in16(status_port) } & acpi::PM1_POWER_BUTTON != 0;
        if pressed {
            // SAFETY: as above.
            unsafe { port::out16(status_port, acpi::PM1_POWER_BUTTON) };
            crate::interrupt::deliver(crate::Interrupt::PowerButton);
        }
    }
    timer::end_of_interrupt();
}
