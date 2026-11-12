//! AFK and EPIC (D545), the ring-buffer protocol on top of RTKit that the display coprocessor's
//! services use: an application endpoint asks for one shared buffer, places a transmit and a
//! receive ring in it, and from then on each side writes entries (a 16-byte header with the
//! magic `IOP `, the size, the channel and the kind, then the data, padded to 64-byte blocks)
//! and moves its write pointer, telling the other side through a mailbox message. EPIC
//! messages in those entries carry a header and a sub-header; a command names buffers the
//! service reads its input from and writes its output to, and its reply gives the result. A
//! service announces itself on a channel, by name or by a property list whose
//! `EPICProviderClass` names it.

use super::rtkit::RtkitError;
use alloc::vec;
use alloc::vec::Vec;

const MESSAGE_TYPE_SHIFT: u32 = 48;
const INIT: u64 = 0x80;
const INIT_ACK: u64 = 0xa0;
const GET_BUFFER: u64 = 0x89;
const GET_BUFFER_ACK: u64 = 0xa1;
const INIT_TRANSMIT: u64 = 0x8a;
const INIT_RECEIVE: u64 = 0x8b;
const START: u64 = 0xa3;
const START_ACK: u64 = 0x86;
const SEND: u64 = 0xa2;
const RECEIVE: u64 = 0x85;
const SHUTDOWN: u64 = 0xc0;
const SHUTDOWN_ACK: u64 = 0xc1;
const BLOCK: usize = 64;
const BLOCK_SHIFT: u32 = 6;
const FIELD_16: u64 = 0xffff;
const ADDRESS_48: u64 = (1 << 48) - 1;

const RING_HEADER_BYTES: usize = 3 * BLOCK;
const RING_SIZE_AT: usize = 0;
const RING_READ_AT: usize = BLOCK;
const RING_WRITE_AT: usize = 2 * BLOCK;
const ENTRY_MAGIC: u32 = u32::from_le_bytes(*b"IOP ");
const ENTRY_HEADER_BYTES: usize = 16;

/// What an entry carries.
pub mod entry_kind {
    pub const NOTIFY: u32 = 0;
    pub const COMMAND: u32 = 3;
    pub const REPLY: u32 = 4;
    pub const NOTIFY_ACK: u32 = 8;
}

const CATEGORY_REPORT: u8 = 0x00;
const CATEGORY_REPLY: u8 = 0x20;
const CATEGORY_COMMAND: u8 = 0x30;
const SUBTYPE_ANNOUNCE: u16 = 0x30;
const EPIC_VERSION: u8 = 2;
const SUB_VERSION: u8 = 4;
const EPIC_HEADER_BYTES: usize = 16;
const SUB_HEADER_BYTES: usize = 24;
const COMMAND_BYTES: usize = 30;
const ANNOUNCE_NAME_BYTES: usize = 32;
const ANNOUNCE_PROPERTIES_MINIMUM: usize = 36;

/// Shared memory both sides reach: `bytes` at the CPU's `base`, at the coprocessor's
/// `device_address`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedBuffer {
    pub base: usize,
    pub bytes: usize,
    pub device_address: u64,
}

impl SharedBuffer {
    fn read_u32(&self, at: usize) -> u32 {
        let mut bytes = [0; 4];
        self.read(at, &mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn write_u32(&self, at: usize, value: u32) {
        self.write(at, &value.to_le_bytes());
    }

    fn read(&self, at: usize, out: &mut [u8]) {
        assert!(at + out.len() <= self.bytes, "outside the shared buffer");
        for (index, byte) in out.iter_mut().enumerate() {
            let place = (self.base + at + index) as *const u8;
            // SAFETY: `Link::allocate`'s contract makes `base..base + bytes` shared memory
            // that stays valid while the endpoint lives, and the access lies inside it
            // (asserted above).
            *byte = unsafe { place.read_volatile() };
        }
    }

    fn write(&self, at: usize, bytes: &[u8]) {
        assert!(at + bytes.len() <= self.bytes, "outside the shared buffer");
        for (index, &byte) in bytes.iter().enumerate() {
            let place = (self.base + at + index) as *mut u8;
            // SAFETY: as in `read`.
            unsafe { place.write_volatile(byte) };
        }
    }

    fn part(&self, offset: usize, bytes: usize) -> Option<Self> {
        (offset.checked_add(bytes)? <= self.bytes).then_some(Self {
            base: self.base + offset,
            bytes,
            device_address: self.device_address + offset as u64,
        })
    }
}

/// What the EPIC endpoint asks of the coprocessor's driver: RTKit's messages and shared memory.
///
/// # Safety
/// Every [`SharedBuffer`] [`Self::allocate`] gives must be memory only this endpoint and the
/// coprocessor use, mapped for the CPU at `base` and for the coprocessor at `device_address`,
/// coherent, for as long as the endpoint lives.
pub unsafe trait Link {
    /// Sends `payload` to `endpoint` through RTKit.
    fn send(&mut self, endpoint: u8, payload: u64) -> Result<(), RtkitError>;
    /// The next message RTKit received for `endpoint`, polling it; `None` when none came
    /// within the link's patience.
    fn next(&mut self, endpoint: u8) -> Result<Option<u64>, RtkitError>;
    /// Shared memory of at least `bytes` bytes.
    fn allocate(&mut self, bytes: usize) -> Option<SharedBuffer>;
}

/// Why an EPIC operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpicError {
    Rtkit(RtkitError),
    /// The coprocessor did not answer in time.
    NoAnswer,
    /// No shared memory.
    NoMemory,
    /// A ring's header or an entry did not read as the protocol says.
    Malformed,
    /// The transmit ring has no room.
    RingFull,
    /// No service of the wanted class announced itself.
    NoService,
    /// The service answered a command with this non-zero result.
    Failed(u32),
}

impl From<RtkitError> for EpicError {
    fn from(error: RtkitError) -> Self {
        Self::Rtkit(error)
    }
}

/// One entry read from the receive ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub channel: u32,
    pub kind: u32,
    pub data: Vec<u8>,
}

/// A ring in the endpoint's shared buffer: its header, then its data.
#[derive(Debug, Clone, Copy)]
struct Ring {
    header: SharedBuffer,
    data: SharedBuffer,
}

impl Ring {
    fn new(buffer: SharedBuffer, offset: usize, bytes: usize) -> Result<Self, EpicError> {
        let whole = buffer.part(offset, bytes).ok_or(EpicError::Malformed)?;
        let header = whole.part(0, RING_HEADER_BYTES).ok_or(EpicError::Malformed)?;
        let size = header.read_u32(RING_SIZE_AT) as usize;
        if size + RING_HEADER_BYTES != bytes {
            return Err(EpicError::Malformed);
        }
        let data = whole
            .part(RING_HEADER_BYTES, size)
            .ok_or(EpicError::Malformed)?;
        Ok(Self { header, data })
    }

    fn has_entry(&self) -> bool {
        self.header.read_u32(RING_READ_AT) != self.header.read_u32(RING_WRITE_AT)
    }

    /// The entry at the read pointer, moving to the ring's start when it would not fit before
    /// the end, and the read pointer it leaves.
    fn read_entry(&self) -> Result<Entry, EpicError> {
        let mut read = self.header.read_u32(RING_READ_AT) as usize;
        let size = self.entry_size(read)?;
        if read + size > self.data.bytes {
            read = 0;
            self.header.write_u32(RING_READ_AT, 0);
        }
        let size = self.entry_size(read)?;
        let channel = self.data.read_u32(read + 8);
        let kind = self.data.read_u32(read + 12);
        let mut data = vec![0; size];
        let start = read + ENTRY_HEADER_BYTES;
        if start + size > self.data.bytes {
            return Err(EpicError::Malformed);
        }
        self.data.read(start, &mut data);
        let next = (start + size).next_multiple_of(BLOCK);
        let next = if next >= self.data.bytes { 0 } else { next };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.header.write_u32(RING_READ_AT, next as u32);
        Ok(Entry {
            channel,
            kind,
            data,
        })
    }

    fn entry_size(&self, at: usize) -> Result<usize, EpicError> {
        if at + ENTRY_HEADER_BYTES > self.data.bytes || self.data.read_u32(at) != ENTRY_MAGIC {
            return Err(EpicError::Malformed);
        }
        Ok(self.data.read_u32(at + 4) as usize)
    }

    /// Writes an entry of `kind` on `channel` holding `data`: the new write pointer.
    fn write_entry(&self, channel: u32, kind: u32, data: &[u8]) -> Result<u32, EpicError> {
        let size = self.data.bytes;
        let read = self.header.read_u32(RING_READ_AT) as usize;
        let mut write = self.header.read_u32(RING_WRITE_AT) as usize;
        let advance = (ENTRY_HEADER_BYTES + data.len()).next_multiple_of(BLOCK);
        let full = match write < read {
            true => advance >= read - write,
            false => {
                let fits_above = advance < size - write || (advance == size - write && read != 0);
                !fits_above && advance >= read
            }
        };
        if full {
            return Err(EpicError::RingFull);
        }
        let mut header = [0u8; ENTRY_HEADER_BYTES];
        header[0..4].copy_from_slice(&ENTRY_MAGIC.to_le_bytes());
        header[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        header[8..12].copy_from_slice(&channel.to_le_bytes());
        header[12..16].copy_from_slice(&kind.to_le_bytes());
        let mut at = write;
        if data.len() > size - (write + ENTRY_HEADER_BYTES) {
            at = 0;
        }
        self.data.write(at, &header);
        if at != write {
            self.data.write(write, &header);
        }
        write = at + ENTRY_HEADER_BYTES;
        self.data.write(write, data);
        write = (write + data.len()).next_multiple_of(BLOCK);
        if write >= size {
            write = 0;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.header.write_u32(RING_WRITE_AT, write as u32);
        Ok(write as u32)
    }
}

/// One EPIC endpoint.
#[derive(Debug)]
pub struct Endpoint {
    endpoint: u8,
    buffer: Option<SharedBuffer>,
    tag: u64,
    transmit: Option<Ring>,
    receive: Option<Ring>,
    started: bool,
    sequence: u16,
    command_input: Option<SharedBuffer>,
    command_output: Option<SharedBuffer>,
}

impl Endpoint {
    pub const fn new(endpoint: u8) -> Self {
        Self {
            endpoint,
            buffer: None,
            tag: 0,
            transmit: None,
            receive: None,
            started: false,
            sequence: 0,
            command_input: None,
            command_output: None,
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Sends `INIT` (RTKit started the endpoint already) and answers the coprocessor until the
    /// rings are set up and it acknowledged `START`.
    pub fn start(&mut self, link: &mut dyn Link) -> Result<(), EpicError> {
        link.send(self.endpoint, INIT << MESSAGE_TYPE_SHIFT)?;
        while !self.started {
            let payload = link.next(self.endpoint)?.ok_or(EpicError::NoAnswer)?;
            self.handle(link, payload)?;
        }
        Ok(())
    }

    /// Handles one mailbox message for the endpoint: whether it said the receive ring has
    /// entries.
    pub fn handle(&mut self, link: &mut dyn Link, payload: u64) -> Result<bool, EpicError> {
        match payload >> MESSAGE_TYPE_SHIFT {
            INIT_ACK => {}
            GET_BUFFER => {
                let blocks = (payload >> 16 & FIELD_16) as usize;
                self.tag = payload & FIELD_16;
                let buffer = link
                    .allocate(blocks << BLOCK_SHIFT)
                    .ok_or(EpicError::NoMemory)?;
                self.buffer = Some(buffer);
                let reply =
                    GET_BUFFER_ACK << MESSAGE_TYPE_SHIFT | buffer.device_address & ADDRESS_48;
                link.send(self.endpoint, reply)?;
            }
            kind @ (INIT_TRANSMIT | INIT_RECEIVE) => {
                let offset = ((payload >> 32 & FIELD_16) as usize) << BLOCK_SHIFT;
                let bytes = ((payload >> 16 & FIELD_16) as usize) << BLOCK_SHIFT;
                if payload & FIELD_16 != self.tag {
                    return Err(EpicError::Malformed);
                }
                let buffer = self.buffer.ok_or(EpicError::Malformed)?;
                let ring = Ring::new(buffer, offset, bytes)?;
                match kind {
                    INIT_TRANSMIT => self.transmit = Some(ring),
                    _ => self.receive = Some(ring),
                }
                if self.transmit.is_some() && self.receive.is_some() {
                    link.send(self.endpoint, START << MESSAGE_TYPE_SHIFT)?;
                }
            }
            START_ACK => self.started = true,
            SHUTDOWN_ACK => self.started = false,
            RECEIVE => return Ok(self.receive.is_some_and(|ring| ring.has_entry())),
            _ => {}
        }
        Ok(false)
    }

    fn has_entry(&self) -> bool {
        self.receive.is_some_and(|ring| ring.has_entry())
    }

    /// The next entry of the receive ring, waiting for the coprocessor's word when there is
    /// none yet.
    pub fn next_entry(&mut self, link: &mut dyn Link) -> Result<Entry, EpicError> {
        while !self.has_entry() {
            let payload = link.next(self.endpoint)?.ok_or(EpicError::NoAnswer)?;
            self.handle(link, payload)?;
        }
        self.receive.ok_or(EpicError::Malformed)?.read_entry()
    }

    /// Writes an entry and tells the coprocessor the new write pointer.
    pub fn transmit(
        &mut self,
        link: &mut dyn Link,
        channel: u32,
        kind: u32,
        data: &[u8],
    ) -> Result<(), EpicError> {
        let ring = self.transmit.ok_or(EpicError::Malformed)?;
        let write = ring.write_entry(channel, kind, data)?;
        link.send(self.endpoint, SEND << MESSAGE_TYPE_SHIFT | u64::from(write))?;
        Ok(())
    }

    /// Waits for the service of class `class` to announce itself, reading at most `entries`
    /// entries, and sets up command buffers of `input_bytes` and `output_bytes`: its channel.
    pub fn find_service(
        &mut self,
        link: &mut dyn Link,
        class: &[u8],
        entries: usize,
        input_bytes: usize,
        output_bytes: usize,
    ) -> Result<u32, EpicError> {
        let mut channel = None;
        for _ in 0..entries {
            let entry = self.next_entry(link)?;
            if let Some((category, subtype, body)) = sub_message(&entry.data)
                && category == CATEGORY_REPORT
                && subtype == SUBTYPE_ANNOUNCE
                && announced_class(body).is_some_and(|announced| announced == class)
            {
                channel = Some(entry.channel);
                break;
            }
        }
        let channel = channel.ok_or(EpicError::NoService)?;
        self.command_input = Some(link.allocate(input_bytes).ok_or(EpicError::NoMemory)?);
        self.command_output = Some(link.allocate(output_bytes).ok_or(EpicError::NoMemory)?);
        Ok(channel)
    }

    /// Runs command `subtype` of the service on `channel` with `input`: its output, as long as
    /// the service wrote it. Announcements and notifications that come first are read and
    /// passed over.
    pub fn command(
        &mut self,
        link: &mut dyn Link,
        channel: u32,
        subtype: u16,
        input: &[u8],
    ) -> Result<Vec<u8>, EpicError> {
        let input_buffer = self.command_input.ok_or(EpicError::NoService)?;
        let output_buffer = self.command_output.ok_or(EpicError::NoService)?;
        if input.len() > input_buffer.bytes {
            return Err(EpicError::Malformed);
        }
        input_buffer.write(0, input);
        let mut message = self.headers(CATEGORY_COMMAND, subtype, COMMAND_BYTES);
        message.extend_from_slice(&0u32.to_le_bytes());
        message.extend_from_slice(&output_buffer.device_address.to_le_bytes());
        message.extend_from_slice(&input_buffer.device_address.to_le_bytes());
        message.extend_from_slice(&(output_buffer.bytes as u32).to_le_bytes());
        message.extend_from_slice(&(input.len() as u32).to_le_bytes());
        message.extend_from_slice(&[0, 0]);
        self.transmit(link, channel, entry_kind::COMMAND, &message)?;
        loop {
            let entry = self.next_entry(link)?;
            if entry.kind != entry_kind::REPLY && entry.kind != entry_kind::NOTIFY {
                continue;
            }
            let Some((category, replied, body)) = sub_message(&entry.data) else {
                continue;
            };
            if category != CATEGORY_REPLY || replied != subtype {
                continue;
            }
            let field = |at: usize| -> Option<u32> {
                Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
            };
            let result = field(0).ok_or(EpicError::Malformed)?;
            if result != 0 {
                return Err(EpicError::Failed(result));
            }
            let length = (field(20).ok_or(EpicError::Malformed)? as usize).min(output_buffer.bytes);
            let mut output = vec![0; length];
            output_buffer.read(0, &mut output);
            return Ok(output);
        }
    }

    fn headers(&mut self, category: u8, subtype: u16, length: usize) -> Vec<u8> {
        let mut message = Vec::with_capacity(EPIC_HEADER_BYTES + SUB_HEADER_BYTES + length);
        message.push(EPIC_VERSION);
        message.extend_from_slice(&self.sequence.to_le_bytes());
        self.sequence = self.sequence.wrapping_add(1);
        message.resize(EPIC_HEADER_BYTES, 0);
        message.extend_from_slice(&(length as u32).to_le_bytes());
        message.push(SUB_VERSION);
        message.push(category);
        message.extend_from_slice(&subtype.to_le_bytes());
        message.resize(EPIC_HEADER_BYTES + SUB_HEADER_BYTES, 0);
        message
    }

    /// Asks the coprocessor to shut the endpoint down and waits for its word.
    pub fn shut_down(&mut self, link: &mut dyn Link) -> Result<(), EpicError> {
        link.send(self.endpoint, SHUTDOWN << MESSAGE_TYPE_SHIFT)?;
        while self.started {
            let payload = link.next(self.endpoint)?.ok_or(EpicError::NoAnswer)?;
            self.handle(link, payload)?;
        }
        Ok(())
    }
}

/// An EPIC message's sub-header category and type, and what follows the sub-header.
fn sub_message(data: &[u8]) -> Option<(u8, u16, &[u8])> {
    let sub = data.get(EPIC_HEADER_BYTES..EPIC_HEADER_BYTES + SUB_HEADER_BYTES)?;
    let subtype = u16::from_le_bytes([sub[6], sub[7]]);
    Some((sub[5], subtype, &data[EPIC_HEADER_BYTES + SUB_HEADER_BYTES..]))
}

/// The class an announcement names: its `EPICProviderClass` when it carries properties, else
/// its name.
fn announced_class(body: &[u8]) -> Option<&[u8]> {
    let name = body.get(..ANNOUNCE_NAME_BYTES)?;
    let properties = &body[ANNOUNCE_NAME_BYTES..];
    if properties.len() > ANNOUNCE_PROPERTIES_MINIMUM {
        return super::properties::string_value(properties, b"EPICProviderClass");
    }
    let end = name.iter().position(|&byte| byte == 0).unwrap_or(name.len());
    Some(&name[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{Layout, alloc_zeroed};
    use std::collections::VecDeque;

    fn host_buffer(bytes: usize) -> SharedBuffer {
        let layout = Layout::from_size_align(bytes, 4096).unwrap();
        // SAFETY: the layout's size is non-zero.
        let base = unsafe { alloc_zeroed(layout) } as usize;
        SharedBuffer {
            base,
            bytes,
            device_address: 0x1_0000,
        }
    }

    #[test]
    fn entries_written_to_a_ring_read_back_in_order_and_wrap() {
        let buffer = host_buffer(RING_HEADER_BYTES + 4 * BLOCK);
        buffer.write_u32(RING_SIZE_AT, 4 * BLOCK as u32);
        let ring = Ring::new(buffer, 0, buffer.bytes).unwrap();
        assert_eq!(ring.write_entry(2, entry_kind::COMMAND, b"abc"), Ok(64));
        assert_eq!(ring.write_entry(2, entry_kind::NOTIFY, &[7; 70]), Ok(192));
        let first = ring.read_entry().unwrap();
        assert_eq!((first.channel, first.kind, first.data), (2, 3, b"abc".to_vec()));
        let second = ring.read_entry().unwrap();
        assert_eq!(second.data, vec![7; 70]);
        assert!(!ring.has_entry());
        assert_eq!(ring.write_entry(1, entry_kind::REPLY, &[1; 40]), Ok(0));
        assert_eq!(ring.read_entry().unwrap().data, vec![1; 40]);
    }

    #[test]
    fn a_full_ring_refuses_an_entry() {
        let buffer = host_buffer(RING_HEADER_BYTES + 2 * BLOCK);
        buffer.write_u32(RING_SIZE_AT, 2 * BLOCK as u32);
        let ring = Ring::new(buffer, 0, buffer.bytes).unwrap();
        assert_eq!(ring.write_entry(0, 0, &[0; 40]), Ok(64));
        assert_eq!(ring.write_entry(0, 0, &[0; 40]), Err(EpicError::RingFull));
    }

    struct FakeLink {
        inbox: VecDeque<u64>,
        sent: Vec<u64>,
        buffer: SharedBuffer,
    }

    // SAFETY: the buffer is leaked host memory, valid for the whole test.
    unsafe impl Link for FakeLink {
        fn send(&mut self, _endpoint: u8, payload: u64) -> Result<(), RtkitError> {
            self.sent.push(payload);
            Ok(())
        }
        fn next(&mut self, _endpoint: u8) -> Result<Option<u64>, RtkitError> {
            Ok(self.inbox.pop_front())
        }
        fn allocate(&mut self, bytes: usize) -> Option<SharedBuffer> {
            (bytes <= self.buffer.bytes).then_some(self.buffer)
        }
    }

    #[test]
    fn the_endpoint_asks_for_its_buffer_sets_its_rings_and_starts() {
        let buffer = host_buffer(0x1000);
        buffer.write_u32(RING_SIZE_AT, 0x800 - RING_HEADER_BYTES as u32);
        buffer.write_u32(0x800 + RING_SIZE_AT, 0x800 - RING_HEADER_BYTES as u32);
        let mut link = FakeLink {
            inbox: VecDeque::from([
                INIT_ACK << 48,
                GET_BUFFER << 48 | 0x40 << 16 | 0x7,
                INIT_TRANSMIT << 48 | 0x20 << 16 | 0x7,
                INIT_RECEIVE << 48 | 0x20 << 32 | 0x20 << 16 | 0x7,
                START_ACK << 48,
            ]),
            sent: Vec::new(),
            buffer,
        };
        let mut endpoint = Endpoint::new(0x23);
        endpoint.start(&mut link).unwrap();
        assert!(endpoint.is_started());
        assert_eq!(
            link.sent,
            vec![INIT << 48, GET_BUFFER_ACK << 48 | 0x1_0000, START << 48]
        );
    }

    #[test]
    fn an_announcement_names_its_service_by_name_when_it_has_no_properties() {
        let mut body = vec![0u8; ANNOUNCE_NAME_BYTES];
        body[..13].copy_from_slice(b"disp0-service");
        assert_eq!(announced_class(&body), Some(&b"disp0-service"[..]));
    }
}
