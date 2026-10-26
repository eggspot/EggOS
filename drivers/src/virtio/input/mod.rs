//! The virtio input device (virtio 1.2, 5.8), polled: keyboards, mice and tablets. The device
//! writes one event (type, code, value: the Linux input event codes) into each buffer of its
//! event queue; the driver keeps every buffer posted and takes the events back as they come.
//! What the device reports is read from its configuration ([`Capabilities`]). The status queue
//! (keyboard LEDs) is not set up.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;
use crate::input::{
    AxisRange, Capabilities, RawEvent, absolute, button, event_type, key, relative,
};

#[cfg(test)]
mod tests;

/// Entries of the event queue the driver sets up, at most: QEMU's devices offer 64.
pub const QUEUE_SIZE: u16 = 64;
/// The DMA memory the driver needs for its queue and event buffers.
pub const MEMORY_BYTES: usize = 4096;
/// Bytes of one event: type and code (16 bits each), value (32 bits), little-endian.
pub const EVENT_BYTES: usize = 8;

const EVENT_QUEUE: u16 = 0;
const EVENTS: usize = 2048;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= EVENTS);
const _: () = assert!(EVENTS + QUEUE_SIZE as usize * EVENT_BYTES <= MEMORY_BYTES);

/// The device configuration's layout (`virtio_input_config`) and what `select` asks for.
pub mod config {
    /// Where the driver writes what it asks for.
    pub const SELECT_OFFSET: usize = 0;
    /// Where the driver writes the detail it asks for (an event type, an axis).
    pub const SUBSELECT_OFFSET: usize = 1;
    /// Where the device says how many bytes of the answer are valid.
    pub const SIZE_OFFSET: usize = 2;
    /// Where the answer starts: a bitmap, a string or an axis's information.
    pub const DATA_OFFSET: usize = 8;
    /// The longest answer.
    pub const DATA_BYTES: usize = 128;
    /// `VIRTIO_INPUT_CFG_EV_BITS`: the codes an event type (the subselect) reports, a bitmap.
    pub const EVENT_BITS: u8 = 0x11;
    /// `VIRTIO_INPUT_CFG_ABS_INFO`: an absolute axis's (the subselect) minimum and maximum, at
    /// the start of the answer, 32 bits each.
    pub const ABSOLUTE_INFO: u8 = 0x12;
}

/// A virtio input device, set up and polled.
#[derive(Debug)]
pub struct VirtioInput<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queue: SplitQueue,
    capabilities: Capabilities,
    buffer_of_head: [u8; QUEUE_SIZE as usize],
}

impl<T: Transport, M: DmaMemory> VirtioInput<T, M> {
    /// Sets the input device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more) for
    /// its event queue and buffers, reads what it reports, and posts every buffer, polling while
    /// `keep_waiting` says to. On an error the transport comes back, and the memory too unless
    /// the device may still use it (a reset that did not read back 0): then it is leaked.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::INPUT {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        if let Err(error) = super::initialize(&transport, 0, keep_waiting) {
            return Err((error, transport, Some(memory)));
        }
        let capabilities = read_capabilities(&transport);
        let max = transport.max_queue_size(EVENT_QUEUE).min(QUEUE_SIZE);
        if max == 0 {
            super::fail(&transport);
            return Err((Error::QueueUnavailable, transport, Some(memory)));
        }
        let size = 1 << max.ilog2();
        // SAFETY: `memory` holds at least `MEMORY_BYTES`, more than the queue's
        // `memory_size(QUEUE_SIZE)` before `EVENTS`, 4096-aligned, and owned by this driver,
        // which keeps it for as long as the queue lives; the device reaches it at its DMA address.
        let queue = unsafe { SplitQueue::new(memory.as_mut_ptr(), memory.dma_addr(), size) };
        let mut queue = match queue {
            Ok(queue) => queue,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        queue.disable_used_notifications();
        if let Err(error) = transport.set_up_queue(EVENT_QUEUE, size, &queue.addresses()) {
            super::fail(&transport);
            let memory = match super::reset(&transport, keep_waiting) {
                Ok(_) => Some(memory),
                Err(_) => {
                    core::mem::forget(memory);
                    None
                }
            };
            return Err((error, transport, memory));
        }
        let mut input = Self {
            transport,
            memory,
            queue,
            capabilities,
            buffer_of_head: [0; QUEUE_SIZE as usize],
        };
        if let Err(error) = (0..size as u8).try_for_each(|buffer| input.post(buffer)) {
            super::fail(&input.transport);
            return match input.reset(keep_waiting) {
                Ok((transport, memory, _)) => Err((error, transport, Some(memory))),
                Err((_, transport)) => Err((error, transport, None)),
            };
        }
        super::driver_ok(&input.transport);
        input.notify_if_asked();
        Ok(input)
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// What the device reports.
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Hands `event` each event the device has written, oldest first, at most `budget` of them,
    /// and posts their buffers again; returns how many buffers came back. With nothing new, it
    /// costs one load of the used ring's index.
    pub fn poll(&mut self, budget: usize, event: &mut impl FnMut(RawEvent)) -> usize {
        let mut taken = 0;
        while taken < budget {
            let Some(used) = self.queue.pop_used() else {
                break;
            };
            taken += 1;
            let Some(&buffer) = self.buffer_of_head.get(usize::from(used.head)) else {
                continue;
            };
            if used.len as usize >= EVENT_BYTES {
                event(self.read_event(buffer));
            }
            let _ = self.post(buffer);
        }
        if taken > 0 {
            self.notify_if_asked();
        }
        taken
    }

    /// Resets the device and hands back the transport and the memory, with the proof that the
    /// device no longer uses it. If the reset does not read back 0, the memory is leaked and only
    /// the transport comes back.
    pub fn reset(
        self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(T, M, ResetDone), (Error, T)> {
        let Self {
            transport, memory, ..
        } = self;
        match super::reset(&transport, keep_waiting) {
            Ok(done) => Ok((transport, memory, done)),
            Err(error) => {
                core::mem::forget(memory);
                Err((error, transport))
            }
        }
    }

    fn post(&mut self, buffer: u8) -> Result<(), Error> {
        let offset = EVENTS + usize::from(buffer) * EVENT_BYTES;
        let addr = self
            .memory
            .dma_addr()
            .checked_add(offset as u64)
            .ok_or(Error::MemoryTooSmall)?;
        let head = self.queue.submit(&[Buffer {
            addr,
            len: EVENT_BYTES as u32,
            device_writes: true,
        }])?;
        if let Some(slot) = self.buffer_of_head.get_mut(usize::from(head)) {
            *slot = buffer;
        }
        Ok(())
    }

    fn notify_if_asked(&self) {
        if self.queue.should_notify() {
            self.transport.notify(EVENT_QUEUE);
        }
    }

    fn read_event(&self, buffer: u8) -> RawEvent {
        let event = self
            .memory
            .as_mut_ptr()
            .wrapping_add(EVENTS + usize::from(buffer) * EVENT_BYTES);
        // SAFETY: the type field of an event buffer inside the driver's memory, 8-byte aligned,
        // which the device has handed back.
        let kind = u16::from_le(unsafe { event.cast::<u16>().read_volatile() });
        // SAFETY: the code field of the same event.
        let code = u16::from_le(unsafe { event.wrapping_add(2).cast::<u16>().read_volatile() });
        // SAFETY: the value field of the same event.
        let value = u32::from_le(unsafe { event.wrapping_add(4).cast::<u32>().read_volatile() });
        RawEvent::new(kind, code, value as i32)
    }
}

/// What the device behind `transport` reports, from its configuration.
pub fn read_capabilities<T: Transport + ?Sized>(transport: &T) -> Capabilities {
    let keys = select(transport, config::EVENT_BITS, event_type::KEY as u8);
    let keyboard = (key::ESC..=key::F12).any(|code| has_bit(transport, keys, code));
    let buttons = has_bit(transport, keys, button::LEFT);
    let relatives = select(transport, config::EVENT_BITS, event_type::REL as u8);
    let relative =
        has_bit(transport, relatives, relative::X) && has_bit(transport, relatives, relative::Y);
    let wheel = has_bit(transport, relatives, relative::WHEEL);
    let absolutes = select(transport, config::EVENT_BITS, event_type::ABS as u8);
    let absolute = (has_bit(transport, absolutes, absolute::X)
        && has_bit(transport, absolutes, absolute::Y))
    .then(|| {
        (
            axis_range(transport, absolute::X),
            axis_range(transport, absolute::Y),
        )
    });
    Capabilities {
        keyboard,
        buttons,
        relative,
        wheel,
        absolute,
    }
}

fn select<T: Transport + ?Sized>(transport: &T, select: u8, subselect: u8) -> usize {
    transport.write_config8(config::SELECT_OFFSET, select);
    transport.write_config8(config::SUBSELECT_OFFSET, subselect);
    usize::from(transport.read_config8(config::SIZE_OFFSET)).min(config::DATA_BYTES)
}

fn has_bit<T: Transport + ?Sized>(transport: &T, size: usize, bit: u16) -> bool {
    let byte = usize::from(bit / 8);
    byte < size && transport.read_config8(config::DATA_OFFSET + byte) & (1 << (bit % 8)) != 0
}

fn axis_range<T: Transport + ?Sized>(transport: &T, axis: u16) -> AxisRange {
    if select(transport, config::ABSOLUTE_INFO, axis as u8) < 8 {
        return AxisRange::default();
    }
    AxisRange {
        min: transport.read_config32(config::DATA_OFFSET) as i32,
        max: transport.read_config32(config::DATA_OFFSET + 4) as i32,
    }
}
