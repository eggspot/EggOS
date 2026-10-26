//! The console's receive interrupt: COM1's ISA line through the I/O APIC that the ACPI MADT
//! names for it, to the boot CPU's local APIC.

use super::{DIRECT_MAP_OFFSET, acpi_table, timer};
use crate::acpi::{self, IoApic as MadtIoApic};
use crate::memory::PhysicalRange;
use eggos_drivers::interrupt::IoApic;
use eggos_drivers::interrupt::io_apic::{self, LineSignal};
use eggos_drivers::io::Mmio;

/// The PC's first serial port's ISA interrupt line.
const COM1_ISA_LINE: u8 = 4;

/// The I/O APICs the MADT lists, whose registers the kernel maps.
pub(super) fn device_regions(region: &mut dyn FnMut(PhysicalRange)) {
    if let Some(madt) = acpi_table(b"APIC") {
        for apic in acpi::madt_io_apics(madt) {
            region(PhysicalRange::from_start_length(
                apic.address,
                io_apic::REGISTER_BLOCK_SIZE as u64,
            ));
        }
    }
}

fn io_apic(apic: MadtIoApic) -> IoApic<Mmio> {
    // SAFETY: the MADT lists `apic`'s registers at this physical address, which
    // `device_regions` gives the kernel's page tables, mapped as device memory at the direct map
    // offset; only this module drives the I/O APICs.
    IoApic::new(unsafe {
        Mmio::new(
            DIRECT_MAP_OFFSET + apic.address as usize,
            io_apic::REGISTER_BLOCK_SIZE,
        )
    })
}

/// Routes COM1's line, as the MADT says it arrives, to the boot CPU as
/// [`timer::CONSOLE_INPUT_VECTOR`].
pub(super) fn enable() -> Result<(), &'static str> {
    let madt = acpi_table(b"APIC").ok_or("the firmware gives no ACPI MADT")?;
    let line = acpi::madt_isa_interrupt(madt, COM1_ISA_LINE);
    let (apic, controller) = acpi::madt_io_apics(madt)
        .map(|apic| (apic, io_apic(apic)))
        .find(|(apic, controller)| {
            line.global_interrupt
                .checked_sub(apic.first_interrupt)
                .is_some_and(|input| input < controller.lines())
        })
        .ok_or("no I/O APIC receives COM1's interrupt")?;
    controller.route(
        line.global_interrupt - apic.first_interrupt,
        LineSignal {
            active_low: line.active_low,
            level_triggered: line.level_triggered,
        },
        timer::CONSOLE_INPUT_VECTOR,
        timer::boot_apic_id(),
    );
    Ok(())
}

/// Handles COM1's interrupt: the kernel reads the UART, then the local APIC's end of interrupt
/// lets the next one in.
pub(super) fn interrupt() {
    crate::interrupt::deliver(crate::Interrupt::ConsoleInput);
    timer::end_of_interrupt();
}
