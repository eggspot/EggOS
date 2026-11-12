//! The system control interrupt, an ISA line the I/O APIC routes to the boot CPU: the power
//! button (D202), ACPI's fixed-feature button, whose press sets `PWRBTN_STS` in the PM1a status
//! register (QEMU's `system_powerdown` presses it), and every other ACPI event (D539), which
//! masks the line and goes to the kernel until it has looked at the event registers.

use super::console_input::io_apic;
use super::{acpi_table, message_interrupts, port, timer};
use crate::acpi::{self, PowerButton};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use eggos_drivers::interrupt::io_apic::LineSignal;

const ACPI_ENABLE_POLLS: u32 = 1_000_000;
const NO_SYSTEM_CONTROL: u32 = u32::MAX;

static PORTS: AtomicU32 = AtomicU32::new(0);

static SYSTEM_CONTROL_INPUT: AtomicU32 = AtomicU32::new(NO_SYSTEM_CONTROL);

static FORWARDS_EVENTS: AtomicBool = AtomicBool::new(false);

/// Routes the system control interrupt ([`enable_events`]) and has every cause but the power
/// button reach the kernel from now on.
pub(super) fn forward_events() -> Result<(), &'static str> {
    enable_events()?;
    FORWARDS_EVENTS.store(true, Ordering::Release);
    Ok(())
}

/// Turns the power button's event on, once the system control interrupt is routed
/// ([`enable_events`]).
pub(super) fn enable() -> Result<(), &'static str> {
    let fadt = acpi_table(b"FACP").ok_or("the firmware gives no ACPI FADT")?;
    let button: PowerButton =
        acpi::power_button(fadt).ok_or("the power button is not ACPI's fixed feature")?;
    enable_events()?;
    // SAFETY: the FADT's PM1a status port: writing the power button's bit clears a stale
    // press and leaves every other status bit as it was.
    unsafe { port::out16(button.pm1a_status_port, acpi::PM1_POWER_BUTTON) };
    // SAFETY: the FADT's PM1a enable port: reading it has no side effect.
    let enabled = unsafe { port::in16(button.pm1a_enable_port) };
    // SAFETY: the FADT's PM1a enable port: the power button's bit turns the event on, and
    // every other event stays as it was.
    unsafe { port::out16(button.pm1a_enable_port, enabled | acpi::PM1_POWER_BUTTON) };
    PORTS.store(
        u32::from(button.pm1a_status_port) | u32::from(button.pm1a_enable_port) << 16,
        Ordering::Relaxed,
    );
    Ok(())
}

/// Switches the machine to ACPI mode if it is not and routes the system control interrupt to
/// the boot CPU as [`timer::POWER_BUTTON_VECTOR`], once.
pub(super) fn enable_events() -> Result<(), &'static str> {
    if SYSTEM_CONTROL_INPUT.load(Ordering::Acquire) != NO_SYSTEM_CONTROL {
        return Ok(());
    }
    let fadt = acpi_table(b"FACP").ok_or("the firmware gives no ACPI FADT")?;
    let sci_line = acpi::sci_line(fadt).ok_or("the FADT names no system control interrupt")?;
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
    let line = acpi::madt_isa_interrupt(madt, sci_line);
    let (apic, controller) = acpi::madt_io_apics(madt)
        .map(|apic| (apic, io_apic(apic)))
        .find(|(apic, controller)| {
            line.global_interrupt
                .checked_sub(apic.first_interrupt)
                .is_some_and(|input| input < controller.lines())
        })
        .ok_or("no I/O APIC receives the system control interrupt")?;
    controller.route(
        line.global_interrupt - apic.first_interrupt,
        LineSignal {
            active_low: line.active_low,
            level_triggered: line.level_triggered,
        },
        timer::POWER_BUTTON_VECTOR,
        timer::boot_apic_id(),
    );
    SYSTEM_CONTROL_INPUT.store(line.global_interrupt, Ordering::Release);
    Ok(())
}

/// Masks or unmasks the system control interrupt's I/O APIC input.
pub(super) fn set_masked(masked: bool) {
    let global = SYSTEM_CONTROL_INPUT.load(Ordering::Acquire);
    if global != NO_SYSTEM_CONTROL {
        message_interrupts::with_input(global, |input, controller| {
            controller.set_masked(input, masked);
        });
    }
}

/// Handles the system control interrupt: a press clears its status bit, which lets the line
/// go, and reaches the kernel; any other cause masks the line and reaches the kernel as
/// [`crate::Interrupt::SystemControl`]; then the local APIC's end of interrupt.
pub(super) fn interrupt() {
    let ports = PORTS.load(Ordering::Relaxed);
    let status_port = ports as u16;
    let mut pressed = false;
    if status_port != 0 {
        // SAFETY: the PM1a status port `enable` found in the FADT: reading it, and writing the
        // power button's bit back, clears the press.
        pressed = unsafe { port::in16(status_port) } & acpi::PM1_POWER_BUTTON != 0;
        if pressed {
            // SAFETY: as above.
            unsafe { port::out16(status_port, acpi::PM1_POWER_BUTTON) };
            crate::interrupt::deliver(crate::Interrupt::PowerButton);
        }
    }
    if !pressed && FORWARDS_EVENTS.load(Ordering::Acquire) {
        set_masked(true);
        crate::interrupt::deliver(crate::Interrupt::SystemControl);
    }
    timer::end_of_interrupt();
}
