//! Message-signalled device interrupts (PCI MSI-X): each line is a vector of the boot CPU's
//! local APIC, from [`FIRST_DEVICE_VECTOR`] on, which a device raises by writing the vector to
//! the local APIC's message address (Intel SDM vol. 3A, 11.11: fixed delivery, edge-triggered,
//! physical destination).

use super::timer;
use crate::{DeviceLine, Interrupt, InterruptMessage};
use core::sync::atomic::{AtomicUsize, Ordering};

/// The first device line's vector, right after the console input's.
pub(super) const FIRST_DEVICE_VECTOR: u8 = 0x33;
/// The last device line's vector.
pub(super) const LAST_DEVICE_VECTOR: u8 = FIRST_DEVICE_VECTOR + DeviceLine::COUNT as u8 - 1;

const MESSAGE_ADDRESS: u64 = 0xfee0_0000;
const DESTINATION_ID_SHIFT: u32 = 12;

static LINES_TAKEN: AtomicUsize = AtomicUsize::new(0);

/// Takes the next device line and the message that raises it on the boot CPU.
pub(super) fn take() -> Result<(DeviceLine, InterruptMessage), &'static str> {
    let index = LINES_TAKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |taken| {
            (taken < DeviceLine::COUNT).then_some(taken + 1)
        })
        .map_err(|_| "every device interrupt line is taken")?;
    let line = DeviceLine::from_index(index).ok_or("every device interrupt line is taken")?;
    let destination = u64::from(timer::boot_apic_id()) << DESTINATION_ID_SHIFT;
    Ok((
        line,
        InterruptMessage {
            address: MESSAGE_ADDRESS | destination,
            data: u32::from(FIRST_DEVICE_VECTOR) + index as u32,
        },
    ))
}

/// Hands the device line of `vector` to the kernel, which quiets the device, then ends the
/// interrupt at the local APIC. Out of line, so the trap paths keep their length.
#[cold]
#[inline(never)]
pub(super) fn interrupt(vector: u64) {
    let index = vector.wrapping_sub(u64::from(FIRST_DEVICE_VECTOR)) as usize;
    if index < LINES_TAKEN.load(Ordering::Relaxed)
        && let Some(line) = DeviceLine::from_index(index)
    {
        crate::interrupt::deliver(Interrupt::Device(line));
    }
    timer::end_of_interrupt();
}
