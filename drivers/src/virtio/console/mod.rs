//! The virtio console device (virtio 1.2, 5.3) with multiple ports, polled, driving one named
//! data port: port 1, which QEMU's `virtserialport` occupies (port 0 is reserved for its
//! `virtconsole`, whose output QEMU drops when the host does not read fast enough). The control
//! queues carry the ports' life cycle: the driver says it is ready, the device adds the port,
//! the driver readies it, the device names it and says whether the host side is open, and the
//! driver opens it. Bytes then flow through the port's receive and transmit queues; the device
//! holds a transmit buffer until the host has taken all of it.

use super::queue::{Buffer, SplitQueue};
use super::{Error, Transport, device_type};
use crate::dma::{DmaAddr, DmaMemory};

#[cfg(test)]
mod tests;

/// Feature bits of the console device (virtio 1.2, 5.3.3).
pub mod feature {
    /// The device has several ports, managed through the control queues.
    pub const MULTIPORT: u64 = 1 << 1;
}

/// Control message events (virtio 1.2, 5.3.6.2).
pub mod event {
    /// The driver is ready to manage ports.
    pub const DEVICE_READY: u16 = 0;
    /// The device added a port.
    pub const PORT_ADD: u16 = 1;
    /// The device removed a port.
    pub const PORT_REMOVE: u16 = 2;
    /// The driver set a port up (value 1) or could not (value 0).
    pub const PORT_READY: u16 = 3;
    /// The port is a console.
    pub const CONSOLE_PORT: u16 = 4;
    /// The port's other side opened (value 1) or closed (value 0) it.
    pub const PORT_OPEN: u16 = 6;
    /// The port's name follows the message.
    pub const PORT_NAME: u16 = 7;
}

/// The port the driver drives.
pub const DATA_PORT: u32 = 1;
/// Entries of each queue the driver sets up, at most.
pub const QUEUE_SIZE: u16 = 8;
/// The bytes each queue takes in the driver's DMA memory, from its start.
pub const QUEUE_BYTES: usize = 1024;
/// Where the control receive buffers start.
pub const CONTROL_BUFFERS_OFFSET: usize = 4096;
/// Each control receive buffer's size.
pub const CONTROL_BUFFER_BYTES: usize = 128;
/// How many control receive buffers stay posted.
pub const CONTROL_BUFFERS: usize = 4;
/// Where the control message the driver sends is written.
pub const CONTROL_SEND_OFFSET: usize = CONTROL_BUFFERS_OFFSET + CONTROL_BUFFERS * CONTROL_BUFFER_BYTES;
/// Where the port's receive buffers start.
pub const RECEIVE_OFFSET: usize = 8192;
/// Each receive buffer's size.
pub const RECEIVE_BUFFER_BYTES: usize = 4096;
/// How many receive buffers stay posted.
pub const RECEIVE_BUFFERS: usize = 4;
/// Where the transmit buffer starts.
pub const TRANSMIT_OFFSET: usize = RECEIVE_OFFSET + RECEIVE_BUFFERS * RECEIVE_BUFFER_BYTES;
/// The most bytes one transmission carries.
pub const TRANSMIT_BYTES: usize = 8192;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = TRANSMIT_OFFSET + TRANSMIT_BYTES;
/// The longest port name kept; a longer one is cut.
pub const MAX_NAME_BYTES: usize = 64;

const CONTROL_MESSAGE_BYTES: usize = 8;
const CONTROL_RECEIVE_QUEUE: u16 = 2;
const CONTROL_TRANSMIT_QUEUE: u16 = 3;
const PORT_RECEIVE_QUEUE: u16 = 4;
const PORT_TRANSMIT_QUEUE: u16 = 5;
const QUEUES: [u16; 4] = [
    CONTROL_RECEIVE_QUEUE,
    CONTROL_TRANSMIT_QUEUE,
    PORT_RECEIVE_QUEUE,
    PORT_TRANSMIT_QUEUE,
];

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= QUEUE_BYTES);
const _: () = assert!(QUEUES.len() * QUEUE_BYTES <= CONTROL_BUFFERS_OFFSET);
const _: () = assert!(CONTROL_SEND_OFFSET + CONTROL_MESSAGE_BYTES <= RECEIVE_OFFSET);
const _: () = assert!(CONTROL_BUFFERS <= QUEUE_SIZE as usize);
const _: () = assert!(RECEIVE_BUFFERS <= QUEUE_SIZE as usize);

/// A control message: the port it is about, the event and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlMessage {
    /// The port.
    pub id: u32,
    /// What happened (see [`event`]).
    pub event: u16,
    /// The event's value.
    pub value: u16,
}

impl ControlMessage {
    /// The message's bytes as the device reads them.
    pub fn to_bytes(self) -> [u8; CONTROL_MESSAGE_BYTES] {
        let mut bytes = [0; CONTROL_MESSAGE_BYTES];
        bytes[..4].copy_from_slice(&self.id.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.event.to_le_bytes());
        bytes[6..].copy_from_slice(&self.value.to_le_bytes());
        bytes
    }

    /// The message at the start of `bytes`, `None` if they are too short for one.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let id = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
        let event = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?);
        let value = u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?);
        Some(Self { id, event, value })
    }
}

#[derive(Debug)]
struct Queues {
    control_receive: SplitQueue,
    control_transmit: SplitQueue,
    port_receive: SplitQueue,
    port_transmit: SplitQueue,
}

#[derive(Debug, Clone, Copy)]
struct Unread {
    buffer: usize,
    offset: usize,
    len: usize,
}

/// A virtio console device with its data port set up, polled.
#[derive(Debug)]
pub struct VirtioConsole<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queues: Queues,
    name: [u8; MAX_NAME_BYTES],
    name_len: usize,
    host_connected: bool,
    unread: Option<Unread>,
    staged: usize,
    sending: bool,
    control_heads: [u16; CONTROL_BUFFERS],
    receive_heads: [u16; RECEIVE_BUFFERS],
}

impl<T: Transport, M: DmaMemory> VirtioConsole<T, M> {
    /// Sets the console device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more):
    /// negotiates [`feature::MULTIPORT`], sets the four queues up, posts the receive buffers,
    /// then runs the control handshake for [`DATA_PORT`] until the device has named it, polling
    /// while `keep_waiting` says to. Other ports the device adds are refused. On an error the
    /// transport comes back, and the memory too unless the device may still use it (a reset
    /// that did not read back 0): then it is leaked.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::CONSOLE {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        let accepted = match super::initialize(&transport, feature::MULTIPORT, keep_waiting) {
            Ok(accepted) => accepted,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        if accepted & feature::MULTIPORT == 0 {
            super::fail(&transport);
            return Err((Error::NoPort, transport, Some(memory)));
        }
        let queues = match set_up_queues(&transport, &memory) {
            Ok(queues) => queues,
            Err(error) => return Err(give_up(error, transport, memory, keep_waiting)),
        };
        let mut console = Self {
            transport,
            memory,
            queues,
            name: [0; MAX_NAME_BYTES],
            name_len: 0,
            host_connected: false,
            unread: None,
            staged: 0,
            sending: false,
            control_heads: [u16::MAX; CONTROL_BUFFERS],
            receive_heads: [u16::MAX; RECEIVE_BUFFERS],
        };
        if let Err(error) = console.post_receive_buffers() {
            let Self {
                transport, memory, ..
            } = console;
            return Err(give_up(error, transport, memory, keep_waiting));
        }
        super::driver_ok(&console.transport);
        if let Err(error) = console.handshake(keep_waiting) {
            let Self {
                transport, memory, ..
            } = console;
            return Err(give_up(error, transport, memory, keep_waiting));
        }
        Ok(console)
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The data port's name, as the device gave it (cut at [`MAX_NAME_BYTES`]).
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }

    /// Whether the port's host side is open, as the device last said.
    pub fn host_connected(&self) -> bool {
        self.host_connected
    }

    fn post_receive_buffers(&mut self) -> Result<(), Error> {
        for buffer in 0..CONTROL_BUFFERS {
            self.post_control_buffer(buffer)?;
        }
        for buffer in 0..RECEIVE_BUFFERS {
            self.post_port_buffer(buffer)?;
        }
        Ok(())
    }

    fn handshake(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.send_control(
            ControlMessage {
                id: 0,
                event: event::DEVICE_READY,
                value: 1,
            },
            keep_waiting,
        )?;
        let mut port_ready = false;
        loop {
            while let Some((message, name)) = self.next_control_message() {
                match message.event {
                    event::PORT_ADD => {
                        let ready = message.id == DATA_PORT;
                        port_ready |= ready;
                        self.send_control(
                            ControlMessage {
                                id: message.id,
                                event: event::PORT_READY,
                                value: u16::from(ready),
                            },
                            keep_waiting,
                        )?;
                    }
                    event::PORT_NAME if message.id == DATA_PORT => self.set_name(&name),
                    event::PORT_OPEN if message.id == DATA_PORT => {
                        self.host_connected = message.value != 0
                    }
                    _ => {}
                }
            }
            if port_ready && self.name_len > 0 {
                return self.send_control(
                    ControlMessage {
                        id: DATA_PORT,
                        event: event::PORT_OPEN,
                        value: 1,
                    },
                    keep_waiting,
                );
            }
            if !keep_waiting() {
                return Err(Error::NoPort);
            }
        }
    }

    fn set_name(&mut self, name: &NameBytes) {
        let len = name.len.min(MAX_NAME_BYTES);
        self.name[..len].copy_from_slice(&name.bytes[..len]);
        self.name_len = len;
    }

    /// Takes the control messages the device sent since the last call: the data port's host
    /// side opening or closing changes [`Self::host_connected`]; a port added later is
    /// refused when the control transmit queue is free, else left unanswered. Returns whether
    /// the host side changed.
    pub fn poll_control(&mut self) -> bool {
        let was = self.host_connected;
        while let Some((message, _)) = self.next_control_message() {
            match message.event {
                event::PORT_OPEN if message.id == DATA_PORT => {
                    self.host_connected = message.value != 0
                }
                event::PORT_REMOVE if message.id == DATA_PORT => self.host_connected = false,
                event::PORT_ADD if message.id != DATA_PORT => {
                    let refusal = ControlMessage {
                        id: message.id,
                        event: event::PORT_READY,
                        value: 0,
                    };
                    let _ = self.send_control(refusal, &mut || false);
                }
                _ => {}
            }
        }
        was != self.host_connected
    }

    fn next_control_message(&mut self) -> Option<(ControlMessage, NameBytes)> {
        loop {
            let used = self.queues.control_receive.pop_used()?;
            let Some(buffer) = self.control_heads.iter().position(|&head| head == used.head) else {
                continue;
            };
            let len = (used.len as usize).min(CONTROL_BUFFER_BYTES);
            let mut bytes = [0u8; CONTROL_BUFFER_BYTES];
            copy_from_dma(
                &self.memory,
                CONTROL_BUFFERS_OFFSET + buffer * CONTROL_BUFFER_BYTES,
                &mut bytes[..len],
            );
            let _ = self.post_control_buffer(buffer);
            let Some(message) = ControlMessage::from_bytes(&bytes[..len]) else {
                continue;
            };
            let mut name = NameBytes {
                bytes: [0; MAX_NAME_BYTES],
                len: 0,
            };
            if message.event == event::PORT_NAME {
                let text = &bytes[CONTROL_MESSAGE_BYTES..len];
                let text = text.split(|&byte| byte == 0).next().unwrap_or(&[]);
                name.len = text.len().min(MAX_NAME_BYTES);
                name.bytes[..name.len].copy_from_slice(&text[..name.len]);
            }
            return Some((message, name));
        }
    }

    fn send_control(
        &mut self,
        message: ControlMessage,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        copy_to_dma(&self.memory, CONTROL_SEND_OFFSET, &message.to_bytes());
        let addr = dma_at(&self.memory, CONTROL_SEND_OFFSET)?;
        self.queues.control_transmit.submit(&[Buffer {
            addr,
            len: CONTROL_MESSAGE_BYTES as u32,
            device_writes: false,
        }])?;
        if self.queues.control_transmit.should_notify() {
            self.transport.notify(CONTROL_TRANSMIT_QUEUE);
        }
        loop {
            if self.queues.control_transmit.pop_used().is_some() {
                return Ok(());
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
        }
    }

    fn post_control_buffer(&mut self, buffer: usize) -> Result<(), Error> {
        let addr = dma_at(
            &self.memory,
            CONTROL_BUFFERS_OFFSET + buffer * CONTROL_BUFFER_BYTES,
        )?;
        let head = self.queues.control_receive.submit(&[Buffer {
            addr,
            len: CONTROL_BUFFER_BYTES as u32,
            device_writes: true,
        }])?;
        self.control_heads[buffer] = head;
        if self.queues.control_receive.should_notify() {
            self.transport.notify(CONTROL_RECEIVE_QUEUE);
        }
        Ok(())
    }

    fn post_port_buffer(&mut self, buffer: usize) -> Result<(), Error> {
        let addr = dma_at(&self.memory, RECEIVE_OFFSET + buffer * RECEIVE_BUFFER_BYTES)?;
        let head = self.queues.port_receive.submit(&[Buffer {
            addr,
            len: RECEIVE_BUFFER_BYTES as u32,
            device_writes: true,
        }])?;
        self.receive_heads[buffer] = head;
        if self.queues.port_receive.should_notify() {
            self.transport.notify(PORT_RECEIVE_QUEUE);
        }
        Ok(())
    }

    /// Whether bytes from the host wait to be read.
    pub fn has_input(&mut self) -> bool {
        self.next_unread().is_some()
    }

    fn next_unread(&mut self) -> Option<Unread> {
        loop {
            if let Some(unread) = self.unread {
                return Some(unread);
            }
            let used = self.queues.port_receive.pop_used()?;
            let Some(buffer) = self.receive_heads.iter().position(|&head| head == used.head) else {
                continue;
            };
            let unread = Unread {
                buffer,
                offset: 0,
                len: (used.len as usize).min(RECEIVE_BUFFER_BYTES),
            };
            if unread.len == 0 {
                let _ = self.post_port_buffer(unread.buffer);
                continue;
            }
            self.unread = Some(unread);
        }
    }

    /// Copies the bytes the host sent into the start of `bytes`, oldest first, and returns how
    /// many; 0 when none wait. Each receive buffer emptied goes back to the device.
    pub fn receive(&mut self, bytes: &mut [u8]) -> usize {
        let mut done = 0;
        while done < bytes.len() {
            let Some(mut unread) = self.next_unread() else {
                break;
            };
            let count = (unread.len - unread.offset).min(bytes.len() - done);
            copy_from_dma(
                &self.memory,
                RECEIVE_OFFSET + unread.buffer * RECEIVE_BUFFER_BYTES + unread.offset,
                &mut bytes[done..done + count],
            );
            done += count;
            unread.offset += count;
            if unread.offset == unread.len {
                self.unread = None;
                let _ = self.post_port_buffer(unread.buffer);
            } else {
                self.unread = Some(unread);
            }
        }
        done
    }

    /// Whether a transmission is still with the device (the host has not taken all of it).
    pub fn is_sending(&mut self) -> bool {
        if self.sending && self.queues.port_transmit.pop_used().is_some() {
            self.sending = false;
        }
        self.sending
    }

    /// Appends the start of `bytes` to the next transmission and returns how many fitted: 0
    /// while one is with the device or the buffer is full.
    pub fn stage(&mut self, bytes: &[u8]) -> usize {
        if self.is_sending() {
            return 0;
        }
        let count = bytes.len().min(TRANSMIT_BYTES - self.staged);
        copy_to_dma(
            &self.memory,
            TRANSMIT_OFFSET + self.staged,
            &bytes[..count],
        );
        self.staged += count;
        count
    }

    /// The bytes staged for the next transmission.
    pub fn staged(&self) -> usize {
        self.staged
    }

    /// Hands the staged bytes to the device; [`Self::is_sending`] says when the host took
    /// them all. Nothing happens when nothing is staged.
    pub fn send_staged(&mut self) -> Result<(), Error> {
        if self.staged == 0 || self.sending {
            return Ok(());
        }
        let addr = dma_at(&self.memory, TRANSMIT_OFFSET)?;
        self.queues.port_transmit.submit(&[Buffer {
            addr,
            len: self.staged as u32,
            device_writes: false,
        }])?;
        self.staged = 0;
        self.sending = true;
        if self.queues.port_transmit.should_notify() {
            self.transport.notify(PORT_TRANSMIT_QUEUE);
        }
        Ok(())
    }
}

struct NameBytes {
    bytes: [u8; MAX_NAME_BYTES],
    len: usize,
}

fn give_up<T: Transport, M: DmaMemory>(
    error: Error,
    transport: T,
    memory: M,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> (Error, T, Option<M>) {
    super::fail(&transport);
    match super::reset(&transport, keep_waiting) {
        Ok(_) => (error, transport, Some(memory)),
        Err(_) => {
            core::mem::forget(memory);
            (error, transport, None)
        }
    }
}

fn set_up_queues<T: Transport, M: DmaMemory>(transport: &T, memory: &M) -> Result<Queues, Error> {
    let mut made = [const { None }; 4];
    for (slot, (index, &queue)) in made.iter_mut().zip(QUEUES.iter().enumerate()) {
        let max = transport.max_queue_size(queue).min(QUEUE_SIZE);
        if max == 0 {
            return Err(Error::QueueUnavailable);
        }
        let size = 1 << max.ilog2();
        if usize::from(size) < CONTROL_BUFFERS.max(RECEIVE_BUFFERS) {
            return Err(Error::QueueTooSmall);
        }
        let mut split = make_queue(memory, index * QUEUE_BYTES, size)?;
        split.disable_used_notifications();
        transport.set_up_queue(queue, size, &split.addresses())?;
        *slot = Some(split);
    }
    let [Some(control_receive), Some(control_transmit), Some(port_receive), Some(port_transmit)] =
        made
    else {
        return Err(Error::QueueUnavailable);
    };
    Ok(Queues {
        control_receive,
        control_transmit,
        port_receive,
        port_transmit,
    })
}

fn make_queue<M: DmaMemory>(memory: &M, offset: usize, size: u16) -> Result<SplitQueue, Error> {
    let dma = dma_at(memory, offset)?;
    // SAFETY: `offset` is one of the queue slots below `CONTROL_BUFFERS_OFFSET`, each
    // `QUEUE_BYTES` (at least `memory_size(QUEUE_SIZE)`) long and 1024-aligned inside memory of
    // `MEMORY_BYTES` or more that this driver owns for as long as its queues live; no other
    // queue or buffer overlaps the slot, and the device reaches it at `dma`.
    unsafe { SplitQueue::new(memory.as_mut_ptr().wrapping_add(offset), dma, size) }
}

fn dma_at<M: DmaMemory>(memory: &M, offset: usize) -> Result<DmaAddr, Error> {
    memory
        .dma_addr()
        .checked_add(offset as u64)
        .ok_or(Error::MemoryTooSmall)
}

fn copy_from_dma<M: DmaMemory>(memory: &M, offset: usize, bytes: &mut [u8]) {
    assert!(offset + bytes.len() <= memory.len());
    let from = memory.as_mut_ptr().wrapping_add(offset);
    for (index, byte) in bytes.iter_mut().enumerate() {
        // SAFETY: inside the driver's DMA memory (checked above), in a buffer the device handed
        // back with its used index read with acquire ordering and does not write until it is
        // posted again.
        *byte = unsafe { from.wrapping_add(index).read_volatile() };
    }
}

fn copy_to_dma<M: DmaMemory>(memory: &M, offset: usize, bytes: &[u8]) {
    assert!(offset + bytes.len() <= memory.len());
    let to = memory.as_mut_ptr().wrapping_add(offset);
    for (index, &byte) in bytes.iter().enumerate() {
        // SAFETY: inside the driver's DMA memory (checked above), in a buffer the device does
        // not hold: it is posted only after these writes, and the submission's release fence
        // orders them before the device can see it.
        unsafe { to.wrapping_add(index).write_volatile(byte) };
    }
}
