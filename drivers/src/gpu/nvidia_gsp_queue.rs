//! GSP-RM's message queues (D523), for GSP firmware 535.113.01: the command queue the driver
//! writes and the status queue GSP-RM writes, each a ring of 4 KiB entries after a page of
//! headers in the shared memory [`super::nvidia_gsp_boot`] laid out. A message is a 48-byte
//! element header (a checksum that makes the element's 64-bit words fold to zero, a sequence
//! number and the entries it spans) and a remote call: a 32-byte header (version 3.0, `VRPC`,
//! its length, its function and the result GSP-RM fills in) and its payload. Each side moves
//! its own write pointer and the other side's read pointer; a new command is announced through
//! GSP's queue doorbell. The first calls a driver sends describe the system and set GSP-RM's
//! registry; GSP-RM answers with events, among them "init done" once it runs, and may ask the
//! driver to run a sequence of register operations (the CPU sequencer). Layouts are NVIDIA's
//! r535 ABI (the MIT-licensed open GPU kernel modules, `NVIDIA-NOTICE.md`).

use super::nvidia_gsp_boot::{PAGE_BYTES, QUEUE_BYTES, SharedMemory};
use crate::io::RegisterIo;
use alloc::vec;
use alloc::vec::Vec;

/// The bytes of a queue element's header.
pub const ELEMENT_HEADER_BYTES: usize = 48;
/// The bytes of a remote call's header.
pub const CALL_HEADER_BYTES: usize = 32;
/// The bytes of the system information payload.
pub const SYSTEM_INFO_BYTES: usize = 664;
/// The registry every boot sets: PCIe resets allowed, the configuration space saved, and the
/// device id check skipped.
pub const DEFAULT_REGISTRY: [(&str, u32); 3] = [
    ("RMSecBusResetEnable", 1),
    ("RMForcePcieConfigSave", 1),
    ("RMDevidCheckIgnore", 1),
];

/// The remote calls the driver sends.
pub mod function {
    /// Describes the system GSP-RM runs in.
    pub const SET_SYSTEM_INFO: u32 = 72;
    /// Sets GSP-RM's registry keys.
    pub const SET_REGISTRY: u32 = 73;
}

/// The events GSP-RM sends.
pub mod event {
    /// GSP-RM finished starting.
    pub const INIT_DONE: u32 = 0x1001;
    /// GSP-RM asks the driver to run register operations.
    pub const RUN_CPU_SEQUENCER: u32 = 0x1002;
    /// GSP-RM reports an error.
    pub const OS_ERROR_LOG: u32 = 0x1006;
}

const PAGE: usize = PAGE_BYTES as usize;
const ENTRIES: usize = ((QUEUE_BYTES - PAGE_BYTES) / PAGE_BYTES) as usize;
const CALL_VERSION: u32 = 0x0300_0000;
const CALL_SIGNATURE: u32 = u32::from_le_bytes(*b"VRPC");
const NO_RESULT: u32 = 0xffff_ffff;
const CHECKSUM: usize = 32;
const SEQUENCE: usize = 36;
const ELEMENT_COUNT: usize = 40;
const CALL_LENGTH: usize = 8;
const CALL_FUNCTION: usize = 12;
const CALL_RESULT: usize = 16;
const CALL_RESULT_PRIVATE: usize = 20;
const WRITE_POINTER: usize = 16;
const READ_POINTER: usize = 32;
const REGISTRY_HEADER_BYTES: usize = 8;
const REGISTRY_ENTRY_BYTES: usize = 16;
const REGISTRY_DWORD: u8 = 1;
const SEQUENCER_COMMANDS: usize = 40;

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// The element carrying remote call `function` with `payload`, numbered `sequence`: whole
/// entries, its checksum set.
pub fn element(function: u32, payload: &[u8], sequence: u32) -> Vec<u8> {
    let call_bytes = (CALL_HEADER_BYTES + payload.len()).next_multiple_of(8);
    let bytes = (ELEMENT_HEADER_BYTES + call_bytes).next_multiple_of(PAGE);
    let mut element = vec![0u8; bytes];
    put(&mut element, SEQUENCE, sequence);
    put(&mut element, ELEMENT_COUNT, (bytes / PAGE) as u32);
    let call = ELEMENT_HEADER_BYTES;
    put(&mut element, call, CALL_VERSION);
    put(&mut element, call + 4, CALL_SIGNATURE);
    put(
        &mut element,
        call + CALL_LENGTH,
        (CALL_HEADER_BYTES + payload.len()) as u32,
    );
    put(&mut element, call + CALL_FUNCTION, function);
    put(&mut element, call + CALL_RESULT, NO_RESULT);
    put(&mut element, call + CALL_RESULT_PRIVATE, NO_RESULT);
    element[call + CALL_HEADER_BYTES..][..payload.len()].copy_from_slice(payload);
    let folded = element.chunks_exact(8).fold(0u64, |folded, chunk| {
        let mut word = [0u8; 8];
        word.copy_from_slice(chunk);
        folded ^ u64::from_le_bytes(word)
    });
    put(
        &mut element,
        CHECKSUM,
        (folded >> 32) as u32 ^ folded as u32,
    );
    element
}

/// Why a queue could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// The command queue has no room for the element yet.
    Full,
    /// The element is larger than the queue.
    TooLarge,
    /// A pointer or a length makes no sense.
    Corrupt,
}

impl QueueError {
    pub fn description(self) -> &'static str {
        match self {
            Self::Full => "GSP-RM's command queue is full",
            Self::TooLarge => "the call is larger than GSP-RM's queue",
            Self::Corrupt => "GSP-RM's status queue makes no sense",
        }
    }
}

/// A message GSP-RM sent: its function or event, its result and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub function: u32,
    pub result: u32,
    pub payload: Vec<u8>,
}

/// The queues in the shared memory behind `memory` (its CPU mapping), laid out as `layout`.
pub struct Queues<R: RegisterIo> {
    memory: R,
    layout: SharedMemory,
    sequence: u32,
}

impl<R: RegisterIo> Queues<R> {
    pub fn new(memory: R, layout: SharedMemory) -> Self {
        Self {
            memory,
            layout,
            sequence: 0,
        }
    }

    fn command(&self) -> usize {
        self.layout.command_queue as usize
    }

    fn status(&self) -> usize {
        self.layout.status_queue as usize
    }

    fn write_page(&self, at: usize, bytes: &[u8]) {
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            self.memory.write32(at + 4 * index, value);
        }
    }

    fn read_page(&self, at: usize, bytes: &mut [u8]) {
        for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
            chunk.copy_from_slice(&self.memory.read32(at + 4 * index).to_le_bytes());
        }
    }

    /// Writes remote call `function` with `payload` into the command queue and moves its write
    /// pointer; the caller rings GSP's doorbell.
    pub fn send(&mut self, function: u32, payload: &[u8]) -> Result<(), QueueError> {
        let element = element(function, payload, self.sequence);
        let pages = element.len() / PAGE;
        if pages >= ENTRIES {
            return Err(QueueError::TooLarge);
        }
        let write = self.memory.read32(self.command() + WRITE_POINTER) as usize;
        let read = self.memory.read32(self.status() + READ_POINTER) as usize;
        if write >= ENTRIES || read >= ENTRIES {
            return Err(QueueError::Corrupt);
        }
        if (read + ENTRIES - write - 1) % ENTRIES < pages {
            return Err(QueueError::Full);
        }
        for (index, page) in element.chunks_exact(PAGE).enumerate() {
            let slot = (write + index) % ENTRIES;
            self.write_page(self.command() + PAGE + slot * PAGE, page);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.memory.write32(
            self.command() + WRITE_POINTER,
            ((write + pages) % ENTRIES) as u32,
        );
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }

    /// The next message in the status queue, its entries handed back; `None` while there is
    /// none, or it is not whole yet.
    pub fn receive(&mut self) -> Result<Option<Message>, QueueError> {
        let read = self.memory.read32(self.command() + READ_POINTER) as usize;
        let write = self.memory.read32(self.status() + WRITE_POINTER) as usize;
        if read >= ENTRIES || write >= ENTRIES {
            return Err(QueueError::Corrupt);
        }
        if read == write {
            return Ok(None);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let entry = |slot: usize| self.status() + PAGE + (slot % ENTRIES) * PAGE;
        let length_at = entry(read) + ELEMENT_HEADER_BYTES + CALL_LENGTH;
        let length = self.memory.read32(length_at) as usize;
        if length < CALL_HEADER_BYTES || ELEMENT_HEADER_BYTES + length > (ENTRIES - 1) * PAGE {
            return Err(QueueError::Corrupt);
        }
        let pages = (ELEMENT_HEADER_BYTES + length).div_ceil(PAGE);
        if (write + ENTRIES - read) % ENTRIES < pages {
            return Ok(None);
        }
        let mut bytes = vec![0u8; pages * PAGE];
        for (page, chunk) in bytes.chunks_exact_mut(PAGE).enumerate() {
            self.read_page(entry(read + page), chunk);
        }
        let call = &bytes[ELEMENT_HEADER_BYTES..ELEMENT_HEADER_BYTES + length];
        let message = Message {
            function: word(call, CALL_FUNCTION).ok_or(QueueError::Corrupt)?,
            result: word(call, CALL_RESULT).ok_or(QueueError::Corrupt)?,
            payload: call[CALL_HEADER_BYTES..].to_vec(),
        };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.memory.write32(
            self.command() + READ_POINTER,
            ((read + pages) % ENTRIES) as u32,
        );
        Ok(Some(message))
    }
}

/// What GSP-RM is told about the system: the device addresses of BAR 0, the framebuffer BAR
/// and the instance memory BAR, the PCI function's id (bus, device, function), the top of a
/// process's address space, and where the GPU mirrors its PCI configuration space in BAR 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemInfo {
    pub registers: u64,
    pub framebuffer: u64,
    pub instance_memory: u64,
    pub pci_id: u64,
    pub user_address_top: u64,
    pub config_mirror: (u32, u32),
}

/// The payload of [`function::SET_SYSTEM_INFO`] for `info`, the rest zero.
pub fn system_info(info: &SystemInfo) -> [u8; SYSTEM_INFO_BYTES] {
    let mut payload = [0u8; SYSTEM_INFO_BYTES];
    put64(&mut payload, 0, info.registers);
    put64(&mut payload, 8, info.framebuffer);
    put64(&mut payload, 16, info.instance_memory);
    put64(&mut payload, 24, info.pci_id);
    put64(&mut payload, 56, info.user_address_top);
    put(&mut payload, 64, info.config_mirror.0);
    put(&mut payload, 68, info.config_mirror.1);
    payload
}

/// The payload of [`function::SET_REGISTRY`] setting each 32-bit key of `entries`: the
/// table's bytes and entry count, one entry per key, then the keys' names.
pub fn registry(entries: &[(&str, u32)]) -> Vec<u8> {
    let names_start = REGISTRY_HEADER_BYTES + REGISTRY_ENTRY_BYTES * entries.len();
    let names: usize = entries.iter().map(|(name, _)| name.len() + 1).sum();
    let mut table = vec![0u8; names_start + names];
    let total = table.len() as u32;
    put(&mut table, 0, total);
    put(&mut table, 4, entries.len() as u32);
    let mut name_at = names_start;
    for (index, (name, value)) in entries.iter().enumerate() {
        let at = REGISTRY_HEADER_BYTES + index * REGISTRY_ENTRY_BYTES;
        put(&mut table, at, name_at as u32);
        table[at + 4] = REGISTRY_DWORD;
        put(&mut table, at + 8, *value);
        put(&mut table, at + 12, 4);
        table[name_at..name_at + name.len()].copy_from_slice(name.as_bytes());
        name_at += name.len() + 1;
    }
    table
}

/// One operation of the CPU sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequencerStep {
    Write {
        address: u32,
        value: u32,
    },
    Modify {
        address: u32,
        mask: u32,
        value: u32,
    },
    Poll {
        address: u32,
        mask: u32,
        value: u32,
        timeout_us: u32,
    },
    Delay {
        microseconds: u32,
    },
    Store {
        address: u32,
        slot: u32,
    },
    CoreReset,
    CoreStart,
    CoreWaitForHalt,
    CoreResume,
}

/// The operations in a [`event::RUN_CPU_SEQUENCER`] payload, in order; `None` for an unknown
/// operation or one cut short.
pub fn sequencer_steps(payload: &[u8]) -> Option<Vec<SequencerStep>> {
    let used = word(payload, 4)? as usize;
    let dword = |index: usize| {
        let at = SEQUENCER_COMMANDS.checked_add(index.checked_mul(4)?)?;
        word(payload, at)
    };
    let mut steps = Vec::new();
    let mut at = 0;
    while at < used {
        let operation = dword(at)?;
        let argument = |index: usize| dword(at + 1 + index);
        let (step, arguments) = match operation {
            0 => (
                SequencerStep::Write {
                    address: argument(0)?,
                    value: argument(1)?,
                },
                2,
            ),
            1 => (
                SequencerStep::Modify {
                    address: argument(0)?,
                    mask: argument(1)?,
                    value: argument(2)?,
                },
                3,
            ),
            2 => (
                SequencerStep::Poll {
                    address: argument(0)?,
                    mask: argument(1)?,
                    value: argument(2)?,
                    timeout_us: argument(3)?,
                },
                5,
            ),
            3 => (
                SequencerStep::Delay {
                    microseconds: argument(0)?,
                },
                1,
            ),
            4 => (
                SequencerStep::Store {
                    address: argument(0)?,
                    slot: argument(1)?,
                },
                2,
            ),
            5 => (SequencerStep::CoreReset, 0),
            6 => (SequencerStep::CoreStart, 0),
            7 => (SequencerStep::CoreWaitForHalt, 0),
            8 => (SequencerStep::CoreResume, 0),
            _ => return None,
        };
        steps.push(step);
        at += 1 + arguments;
    }
    Some(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;

    struct Memory(RefCell<Vec<u8>>);

    impl RegisterIo for &Memory {
        fn read8(&self, offset: usize) -> u8 {
            self.0.borrow()[offset]
        }
        fn write8(&self, offset: usize, value: u8) {
            self.0.borrow_mut()[offset] = value;
        }
        fn read16(&self, offset: usize) -> u16 {
            u16::from_le_bytes(self.0.borrow()[offset..offset + 2].try_into().unwrap())
        }
        fn write16(&self, offset: usize, value: u16) {
            self.0.borrow_mut()[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        fn read32(&self, offset: usize) -> u32 {
            u32::from_le_bytes(self.0.borrow()[offset..offset + 4].try_into().unwrap())
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut()[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn an_element_folds_to_zero_and_names_its_call() {
        let element = element(function::SET_REGISTRY, &[1, 2, 3], 7);
        assert_eq!(element.len(), PAGE);
        let folded = element.chunks_exact(8).fold(0u64, |folded, chunk| {
            folded ^ u64::from_le_bytes(chunk.try_into().unwrap())
        });
        assert_eq!((folded >> 32) as u32 ^ folded as u32, 0);
        assert_eq!(
            word(&element, ELEMENT_HEADER_BYTES + 4),
            Some(CALL_SIGNATURE)
        );
        assert_eq!(word(&element, ELEMENT_HEADER_BYTES + CALL_LENGTH), Some(35));
        assert_eq!(word(&element, SEQUENCE), Some(7));
    }

    #[test]
    fn a_sent_call_lands_at_the_write_pointer_and_a_status_message_comes_back() {
        let layout = SharedMemory::new();
        let memory = Memory(RefCell::new(vec![0u8; layout.bytes as usize]));
        let mut queues = Queues::new(&memory, layout);
        queues.send(function::SET_SYSTEM_INFO, &[9; 16]).unwrap();
        let command = layout.command_queue as usize;
        assert_eq!((&memory).read32(command + WRITE_POINTER), 1);
        assert_eq!(
            (&memory).read32(command + PAGE + ELEMENT_HEADER_BYTES + CALL_FUNCTION),
            72
        );
        let status = layout.status_queue as usize;
        let reply = element(event::INIT_DONE, &[5, 6], 0);
        for (index, chunk) in reply.chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes(chunk.try_into().unwrap());
            (&memory).write32(status + PAGE + 4 * index, value);
        }
        assert_eq!(queues.receive(), Ok(None));
        (&memory).write32(status + WRITE_POINTER, 1);
        let message = queues.receive().unwrap().unwrap();
        assert_eq!(message.function, event::INIT_DONE);
        assert_eq!(message.payload, vec![5, 6]);
        assert_eq!((&memory).read32(command + READ_POINTER), 1);
        assert_eq!(queues.receive(), Ok(None));
    }

    #[test]
    fn a_full_command_queue_refuses_more() {
        let layout = SharedMemory::new();
        let memory = Memory(RefCell::new(vec![0u8; layout.bytes as usize]));
        (&memory).write32(layout.command_queue as usize + WRITE_POINTER, 4);
        (&memory).write32(layout.status_queue as usize + READ_POINTER, 5);
        let mut queues = Queues::new(&memory, layout);
        assert_eq!(
            queues.send(function::SET_REGISTRY, &[]),
            Err(QueueError::Full)
        );
    }

    #[test]
    fn the_registry_packs_its_entries_then_their_names() {
        let table = registry(&DEFAULT_REGISTRY[..1]);
        assert_eq!(word(&table, 0), Some(table.len() as u32));
        assert_eq!(word(&table, 8), Some(24));
        assert_eq!(&table[24..24 + 19], b"RMSecBusResetEnable");
        assert_eq!(word(&table, 16), Some(1));
    }

    #[test]
    fn the_sequencer_reads_writes_polls_and_core_steps() {
        let mut payload = vec![0u8; SEQUENCER_COMMANDS];
        let commands = [0u32, 0x100, 7, 2, 0x200, 1, 1, 50, 0, 6];
        put(&mut payload, 4, commands.len() as u32);
        for value in commands {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            sequencer_steps(&payload),
            Some(vec![
                SequencerStep::Write {
                    address: 0x100,
                    value: 7
                },
                SequencerStep::Poll {
                    address: 0x200,
                    mask: 1,
                    value: 1,
                    timeout_us: 50
                },
                SequencerStep::CoreStart,
            ])
        );
        put(&mut payload, SEQUENCER_COMMANDS, 9);
        assert_eq!(sequencer_steps(&payload), None);
    }
}
