//! Device interrupt lines: each line is a vector of a local APIC, the boot CPU's until the
//! kernel spreads the lines over the CPUs (D536), from [`FIRST_DEVICE_VECTOR`] on, raised either
//! by a message a PCI function writes to the local APIC's address (MSI-X or MSI; Intel SDM vol.
//! 3A, 11.11: fixed delivery, edge-triggered, physical destination) or by an I/O APIC input, a
//! PCI function's legacy INTx pin (D413). Every CPU shares the vectors, so a line keeps its
//! vector wherever it goes. A line is taken from a bitmap and given back when its driver lets
//! the device go. An I/O APIC
//! line is level-triggered and its driver quiets the device only when its thread looks, so the
//! input is masked when its interrupt arrives and unmasked when the driver waits again.

use super::{acpi_table, console_input, timer};
use crate::acpi;
use crate::{DeviceLine, Interrupt, InterruptMessage};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_drivers::interrupt::io_apic::LineSignal;

/// The first device line's vector, right after the console input's.
pub(super) const FIRST_DEVICE_VECTOR: u8 = 0x33;
/// The last device line's vector.
pub(super) const LAST_DEVICE_VECTOR: u8 = FIRST_DEVICE_VECTOR + DeviceLine::COUNT as u8 - 1;

const MESSAGE_ADDRESS: u64 = 0xfee0_0000;
const DESTINATION_ID_SHIFT: u32 = 12;
const NO_PIN: u32 = u32::MAX;

static TAKEN: AtomicU32 = AtomicU32::new(0);
static PINS: [AtomicU32; DeviceLine::COUNT] =
    [const { AtomicU32::new(NO_PIN) }; DeviceLine::COUNT];

fn take_line() -> Result<DeviceLine, &'static str> {
    let taken = TAKEN
        .try_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
            (taken != u32::MAX).then(|| taken | 1 << (!taken).trailing_zeros())
        })
        .map_err(|_| "every device interrupt line is taken")?;
    let index = (!taken).trailing_zeros() as usize;
    DeviceLine::from_index(index).ok_or("every device interrupt line is taken")
}

fn vector_of(line: DeviceLine) -> u8 {
    FIRST_DEVICE_VECTOR + line.index() as u8
}

fn message(line: DeviceLine, apic_id: u8) -> InterruptMessage {
    InterruptMessage {
        address: MESSAGE_ADDRESS | u64::from(apic_id) << DESTINATION_ID_SHIFT,
        data: u32::from(vector_of(line)),
    }
}

fn apic_id_of(cpu: u64) -> Result<u8, &'static str> {
    u8::try_from(cpu).map_err(|_| "messages and the I/O APIC name APIC IDs up to 255 only")
}

/// Takes a free device line and the message that raises it on the boot CPU.
pub(super) fn take() -> Result<(DeviceLine, InterruptMessage), &'static str> {
    let line = take_line()?;
    Ok((line, message(line, timer::boot_apic_id())))
}

/// The message that raises the taken message line `line` on the CPU whose APIC ID is `cpu`
/// (D536).
pub(super) fn message_for(line: DeviceLine, cpu: u64) -> Result<InterruptMessage, &'static str> {
    if TAKEN.load(Ordering::Acquire) & 1 << line.index() == 0 {
        return Err("the line is not taken");
    }
    if PINS[line.index()].load(Ordering::Acquire) != NO_PIN {
        return Err("the line is an I/O APIC input");
    }
    Ok(message(line, apic_id_of(cpu)?))
}

/// Sends the I/O APIC input behind `line` to the CPU whose APIC ID is `cpu` (D536).
pub(super) fn steer(line: DeviceLine, cpu: u64) -> Result<(), &'static str> {
    let global = PINS[line.index()].load(Ordering::Acquire);
    if global == NO_PIN {
        return Err("the line is message-signalled: its device takes a new message");
    }
    let apic_id = apic_id_of(cpu)?;
    with_input(global, |input, controller| controller.set_destination(input, apic_id))
        .ok_or("no I/O APIC receives that interrupt")
}

type Controller = eggos_drivers::interrupt::IoApic<eggos_drivers::io::Mmio>;

pub(super) fn with_input<R>(
    global: u32,
    work: impl FnOnce(u32, &Controller) -> R,
) -> Option<R> {
    let madt = acpi_table(b"APIC")?;
    let (apic, controller) = acpi::madt_io_apics(madt)
        .map(|apic| (apic, console_input::io_apic(apic)))
        .find(|(apic, controller)| {
            global
                .checked_sub(apic.first_interrupt)
                .is_some_and(|input| input < controller.lines())
        })?;
    Some(work(global - apic.first_interrupt, &controller))
}

/// Takes a free device line for the I/O APIC input `global` (a global system interrupt, a PCI
/// INTx pin's), signalling as `signal` says, and routes it to the boot CPU.
pub(super) fn take_pin(global: u32, signal: LineSignal) -> Result<DeviceLine, &'static str> {
    let line = take_line()?;
    let routed = with_input(global, |input, controller| {
        controller.route(input, signal, vector_of(line), timer::boot_apic_id());
    });
    if routed.is_none() {
        release(line);
        return Err("no I/O APIC receives that interrupt");
    }
    PINS[line.index()].store(global, Ordering::Release);
    Ok(line)
}

/// Gives `line` back: an I/O APIC input is masked first, so the line raises nothing more.
pub(super) fn release(line: DeviceLine) {
    let global = PINS[line.index()].swap(NO_PIN, Ordering::AcqRel);
    if global != NO_PIN {
        with_input(global, |input, controller| controller.set_masked(input, true));
    }
    TAKEN.fetch_and(!(1 << line.index()), Ordering::AcqRel);
}

/// Unmasks `line`'s I/O APIC input, once its driver has looked at the device; nothing for a
/// message line.
pub(super) fn unmask(line: DeviceLine) {
    let global = PINS[line.index()].load(Ordering::Acquire);
    if global != NO_PIN {
        with_input(global, |input, controller| controller.set_masked(input, false));
    }
}

/// Hands the device line of `vector` to the kernel, which quiets the device or leaves it to its
/// driver, then ends the interrupt at the local APIC; an I/O APIC input stays masked until its
/// driver waits again. Out of line, so the trap paths keep their length.
#[cold]
#[inline(never)]
pub(super) fn interrupt(vector: u64) {
    let index = vector.wrapping_sub(u64::from(FIRST_DEVICE_VECTOR)) as usize;
    if index < DeviceLine::COUNT
        && TAKEN.load(Ordering::Acquire) & 1 << index != 0
        && let Some(line) = DeviceLine::from_index(index)
    {
        let global = PINS[index].load(Ordering::Acquire);
        if global != NO_PIN {
            with_input(global, |input, controller| controller.set_masked(input, true));
        }
        crate::interrupt::deliver(Interrupt::Device(line));
    }
    timer::end_of_interrupt();
}
