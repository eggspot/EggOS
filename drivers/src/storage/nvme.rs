//! NVM Express controllers (NVMe 1.4 base specification; D238): the admin queue, one I/O queue
//! pair, the first active namespace, read, write and flush, polled.

use super::{Covering, Error, SECTOR_SIZE, copy_in, copy_out, covering, read32, write32, write64, zero};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use core::sync::atomic::{Ordering, fence};

const CAP: usize = 0x00;
const CC: usize = 0x14;
const CSTS: usize = 0x1c;
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;
const DOORBELLS: usize = 0x1000;

const CC_ENABLE: u32 = 1;
const CC_IO_SUBMISSION_ENTRY_64_BYTES: u32 = 6 << 16;
const CC_IO_COMPLETION_ENTRY_16_BYTES: u32 = 4 << 20;
const CC_SHUTDOWN_NORMAL: u32 = 1 << 14;
const CSTS_READY: u32 = 1;
const CSTS_FATAL: u32 = 1 << 1;
const CSTS_SHUTDOWN_COMPLETE: u32 = 0b10 << 2;
const CSTS_SHUTDOWN_MASK: u32 = 0b11 << 2;

const ADMIN_DELETE_SUBMISSION_QUEUE: u8 = 0x00;
const ADMIN_CREATE_SUBMISSION_QUEUE: u8 = 0x01;
const ADMIN_DELETE_COMPLETION_QUEUE: u8 = 0x04;
const ADMIN_CREATE_COMPLETION_QUEUE: u8 = 0x05;
const ADMIN_IDENTIFY: u8 = 0x06;
const IO_FLUSH: u8 = 0x00;
const IO_WRITE: u8 = 0x01;
const IO_READ: u8 = 0x02;

const IDENTIFY_NAMESPACE: u32 = 0;
const IDENTIFY_CONTROLLER: u32 = 1;
const IDENTIFY_ACTIVE_NAMESPACES: u32 = 2;

const PAGE: usize = 4096;
const SUBMISSION_ENTRY: usize = 64;
const COMPLETION_ENTRY: usize = 16;
pub const QUEUE_ENTRIES: usize = 64;
const ADMIN_SQ: usize = 0x0000;
const ADMIN_CQ: usize = 0x1000;
const IO_SQ: usize = 0x2000;
const IO_CQ: usize = 0x3000;
const IDENTIFY: usize = 0x4000;
const PRP_LIST: usize = 0x5000;
const DATA: usize = 0x8000;
pub const MEMORY_BYTES: usize = 0x4_0000;
pub const DATA_BYTES: usize = MEMORY_BYTES - DATA;

const _: () = assert!(QUEUE_ENTRIES * SUBMISSION_ENTRY <= PAGE);
const _: () = assert!(DATA_BYTES / PAGE <= PAGE / 8);

/// A submission queue entry being built: sixteen command dwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Command(pub [u32; 16]);

impl Command {
    pub fn new(opcode: u8) -> Self {
        let mut command = Self::default();
        command.0[0] = u32::from(opcode);
        command
    }

    pub fn namespace(mut self, namespace: u32) -> Self {
        self.0[1] = namespace;
        self
    }

    pub fn data(mut self, first: u64, second: u64) -> Self {
        self.0[6] = first as u32;
        self.0[7] = (first >> 32) as u32;
        self.0[8] = second as u32;
        self.0[9] = (second >> 32) as u32;
        self
    }

    pub fn dword(mut self, index: usize, value: u32) -> Self {
        self.0[index] = value;
        self
    }

    fn with_id(mut self, id: u16) -> Self {
        self.0[0] = (self.0[0] & 0xffff) | (u32::from(id) << 16);
        self
    }
}

/// The two PRP entries of a transfer of `bytes` from DMA address `data` (page-aligned), and
/// the PRP list it needs at `list`, as `(prp1, prp2, list entries)`.
pub fn prp_entries(data: u64, bytes: usize, list: u64) -> (u64, u64, usize) {
    let pages = bytes.div_ceil(PAGE).max(1);
    match pages {
        1 => (data, 0, 0),
        2 => (data, data + PAGE as u64, 0),
        _ => (data, list, pages - 1),
    }
}

#[derive(Debug, Clone, Copy)]
struct Queue {
    submission: usize,
    completion: usize,
    entries: usize,
    tail: usize,
    head: usize,
    phase: bool,
    submission_doorbell: usize,
    completion_doorbell: usize,
}

impl Queue {
    fn new(id: usize, submission: usize, completion: usize, entries: usize, stride: usize) -> Self {
        Self {
            submission,
            completion,
            entries,
            tail: 0,
            head: 0,
            phase: true,
            submission_doorbell: DOORBELLS + 2 * id * stride,
            completion_doorbell: DOORBELLS + (2 * id + 1) * stride,
        }
    }
}

/// An NVMe controller with its first active namespace.
#[derive(Debug)]
pub struct Nvme<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    admin: Queue,
    io: Queue,
    next_id: u16,
    namespace: u32,
    block_bytes: usize,
    blocks: u64,
    max_transfer: usize,
    volatile_write_cache: bool,
}

fn read64<R: RegisterIo>(regs: &R, offset: usize) -> u64 {
    u64::from(regs.read32(offset)) | (u64::from(regs.read32(offset + 4)) << 32)
}

impl<R: RegisterIo, M: DmaMemory> Nvme<R, M> {
    /// Resets the controller at `regs`, sets its queues up in `memory` ([`MEMORY_BYTES`] or
    /// more) and finds its first namespace. On failure the controller is left disabled where it
    /// could be, and `memory` is handed back only then (`None`: the device may still use it).
    pub fn new(
        regs: R,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::Unsupported("too little DMA memory for an NVMe controller"), Some(memory)));
        }
        let capabilities = read64(&regs, CAP);
        let max_entries = (capabilities & 0xffff) as usize + 1;
        let stride = 4usize << ((capabilities >> 32) & 0xf);
        let supports_nvm = capabilities >> 37 & 1 == 1;
        let minimum_page = 1usize << (12 + ((capabilities >> 48) & 0xf));
        if !supports_nvm || minimum_page > PAGE {
            return Err((Error::Unsupported("the controller has no NVM command set with 4 KiB pages"), Some(memory)));
        }
        let entries = QUEUE_ENTRIES.min(max_entries).max(2);
        let mut controller = Self {
            regs,
            memory,
            admin: Queue::new(0, ADMIN_SQ, ADMIN_CQ, entries, stride),
            io: Queue::new(1, IO_SQ, IO_CQ, entries, stride),
            next_id: 0,
            namespace: 0,
            block_bytes: SECTOR_SIZE,
            blocks: 0,
            max_transfer: DATA_BYTES,
            volatile_write_cache: false,
        };
        match controller.start(keep_waiting) {
            Ok(()) => Ok(controller),
            Err(error) => {
                let disabled = controller.disable(keep_waiting);
                Err((error, disabled.then_some(controller.memory)))
            }
        }
    }

    fn wait_status(&self, mask: u32, wanted: u32, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        loop {
            let status = self.regs.read32(CSTS);
            if status == u32::MAX {
                return Err(Error::ControllerFailed);
            }
            if status & mask == wanted {
                return Ok(());
            }
            if mask & CSTS_READY != 0 && wanted & CSTS_READY != 0 && status & CSTS_FATAL != 0 {
                return Err(Error::ControllerFailed);
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
    }

    fn disable(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> bool {
        let configuration = self.regs.read32(CC);
        self.regs.write32(CC, configuration & !CC_ENABLE);
        self.wait_status(CSTS_READY, 0, keep_waiting).is_ok()
    }

    fn start(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if !self.disable(keep_waiting) {
            return Err(Error::TimedOut);
        }
        zero(&self.memory, 0, DATA);
        let base = self.memory.dma_addr().get();
        let entries = self.admin.entries as u32;
        self.regs.write32(AQA, ((entries - 1) << 16) | (entries - 1));
        self.regs.write32(ASQ, (base + ADMIN_SQ as u64) as u32);
        self.regs.write32(ASQ + 4, ((base + ADMIN_SQ as u64) >> 32) as u32);
        self.regs.write32(ACQ, (base + ADMIN_CQ as u64) as u32);
        self.regs.write32(ACQ + 4, ((base + ADMIN_CQ as u64) >> 32) as u32);
        self.regs.write32(
            CC,
            CC_ENABLE | CC_IO_SUBMISSION_ENTRY_64_BYTES | CC_IO_COMPLETION_ENTRY_16_BYTES,
        );
        self.wait_status(CSTS_READY, CSTS_READY, keep_waiting)?;

        let identify = base + IDENTIFY as u64;
        self.admin_command(
            Command::new(ADMIN_IDENTIFY).data(identify, 0).dword(10, IDENTIFY_CONTROLLER),
            keep_waiting,
        )?;
        let mut controller = [0u8; 1024];
        copy_out(&self.memory, IDENTIFY, &mut controller);
        let transfer_exponent = controller[77];
        if transfer_exponent != 0 {
            let limit = PAGE << transfer_exponent.min(16);
            self.max_transfer = DATA_BYTES.min(limit);
        }
        self.volatile_write_cache = controller[525] & 1 != 0;

        self.admin_command(
            Command::new(ADMIN_IDENTIFY).data(identify, 0).dword(10, IDENTIFY_ACTIVE_NAMESPACES),
            keep_waiting,
        )?;
        let namespace = read32(&self.memory, IDENTIFY);
        if namespace == 0 {
            return Err(Error::Unsupported("the controller has no active namespace"));
        }
        self.namespace = namespace;
        self.admin_command(
            Command::new(ADMIN_IDENTIFY)
                .namespace(namespace)
                .data(identify, 0)
                .dword(10, IDENTIFY_NAMESPACE),
            keep_waiting,
        )?;
        let mut description = [0u8; 256];
        copy_out(&self.memory, IDENTIFY, &mut description);
        let (blocks, block_bytes) = namespace_size(&description)
            .ok_or(Error::Unsupported("the namespace's block size is not one the kernel takes"))?;
        self.blocks = blocks;
        self.block_bytes = block_bytes;

        let entries = self.io.entries as u32;
        self.admin_command(
            Command::new(ADMIN_CREATE_COMPLETION_QUEUE)
                .data(base + IO_CQ as u64, 0)
                .dword(10, ((entries - 1) << 16) | 1)
                .dword(11, 1),
            keep_waiting,
        )?;
        self.admin_command(
            Command::new(ADMIN_CREATE_SUBMISSION_QUEUE)
                .data(base + IO_SQ as u64, 0)
                .dword(10, ((entries - 1) << 16) | 1)
                .dword(11, (1 << 16) | 1),
            keep_waiting,
        )?;
        Ok(())
    }

    fn submit(&mut self, io: bool, command: Command, keep_waiting: &mut dyn FnMut() -> bool) -> Result<u32, Error> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let command = command.with_id(id);
        let queue = if io { &mut self.io } else { &mut self.admin };
        let slot = queue.submission + queue.tail * SUBMISSION_ENTRY;
        for (index, word) in command.0.iter().enumerate() {
            write32(&self.memory, slot + index * 4, *word);
        }
        queue.tail = (queue.tail + 1) % queue.entries;
        let tail = queue.tail as u32;
        let submission_doorbell = queue.submission_doorbell;
        fence(Ordering::SeqCst);
        self.regs.write32(submission_doorbell, tail);
        loop {
            let queue = if io { &mut self.io } else { &mut self.admin };
            let entry = queue.completion + queue.head * COMPLETION_ENTRY;
            let status_word = read32(&self.memory, entry + 12);
            let phase = status_word >> 16 & 1 == 1;
            if phase == queue.phase {
                fence(Ordering::SeqCst);
                let result = read32(&self.memory, entry);
                let completed_id = status_word as u16;
                let status = (status_word >> 17) & 0x7fff;
                queue.head = (queue.head + 1) % queue.entries;
                if queue.head == 0 {
                    queue.phase = !queue.phase;
                }
                let head = queue.head as u32;
                let completion_doorbell = queue.completion_doorbell;
                self.regs.write32(completion_doorbell, head);
                if completed_id != id {
                    continue;
                }
                return if status == 0 {
                    Ok(result)
                } else {
                    Err(Error::CommandFailed)
                };
            }
            if self.regs.read32(CSTS) & CSTS_FATAL != 0 {
                return Err(Error::ControllerFailed);
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
    }

    fn admin_command(&mut self, command: Command, keep_waiting: &mut dyn FnMut() -> bool) -> Result<u32, Error> {
        self.submit(false, command, keep_waiting)
    }

    /// The disk's size in 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.blocks
            .saturating_mul((self.block_bytes / SECTOR_SIZE) as u64)
    }

    /// The namespace's logical block size in bytes.
    pub fn block_bytes(&self) -> usize {
        self.block_bytes
    }

    pub fn has_write_cache(&self) -> bool {
        self.volatile_write_cache
    }

    fn transfer_blocks(&mut self, opcode: u8, first: u64, blocks: u64, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let bytes = blocks as usize * self.block_bytes;
        let base = self.memory.dma_addr().get();
        let data = base + DATA as u64;
        let (first_entry, second_entry, list_entries) = prp_entries(data, bytes, base + PRP_LIST as u64);
        for index in 0..list_entries {
            write64(&self.memory, PRP_LIST + index * 8, data + ((index + 1) * PAGE) as u64);
        }
        let command = Command::new(opcode)
            .namespace(self.namespace)
            .data(first_entry, second_entry)
            .dword(10, first as u32)
            .dword(11, (first >> 32) as u32)
            .dword(12, (blocks - 1) as u32);
        self.submit(true, command, keep_waiting).map(|_| ())
    }

    fn check(&self, sector: u64, bytes: usize) -> Result<u64, Error> {
        if !bytes.is_multiple_of(SECTOR_SIZE) {
            return Err(Error::OutOfRange);
        }
        let sectors = (bytes / SECTOR_SIZE) as u64;
        sector
            .checked_add(sectors)
            .filter(|&end| end <= self.sectors())
            .ok_or(Error::OutOfRange)?;
        Ok(sectors)
    }

    fn chunk_sectors(&self) -> u64 {
        let per_block = (self.block_bytes / SECTOR_SIZE) as u64;
        let blocks = (self.max_transfer / self.block_bytes).max(2) as u64 - 1;
        blocks * per_block
    }

    /// Reads the sectors from `sector` on into `buffer`, a whole number of sectors.
    pub fn read(&mut self, sector: u64, buffer: &mut [u8], keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let sectors = self.check(sector, buffer.len())?;
        let chunk = self.chunk_sectors();
        let mut done = 0u64;
        while done < sectors {
            let count = chunk.min(sectors - done);
            let Covering { first_block, blocks, skip_bytes } = covering(sector + done, count, self.block_bytes);
            self.transfer_blocks(IO_READ, first_block, blocks, keep_waiting)?;
            let start = done as usize * SECTOR_SIZE;
            let length = count as usize * SECTOR_SIZE;
            copy_out(&self.memory, DATA + skip_bytes, &mut buffer[start..start + length]);
            done += count;
        }
        Ok(())
    }

    /// Writes `buffer`, a whole number of sectors, to the sectors from `sector` on; a partial
    /// logical block is read first and written whole.
    pub fn write(&mut self, sector: u64, buffer: &[u8], keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let sectors = self.check(sector, buffer.len())?;
        let chunk = self.chunk_sectors();
        let mut done = 0u64;
        while done < sectors {
            let count = chunk.min(sectors - done);
            let Covering { first_block, blocks, skip_bytes } = covering(sector + done, count, self.block_bytes);
            let length = count as usize * SECTOR_SIZE;
            if skip_bytes != 0 || length != blocks as usize * self.block_bytes {
                self.transfer_blocks(IO_READ, first_block, blocks, keep_waiting)?;
            }
            let start = done as usize * SECTOR_SIZE;
            copy_in(&self.memory, DATA + skip_bytes, &buffer[start..start + length]);
            fence(Ordering::SeqCst);
            self.transfer_blocks(IO_WRITE, first_block, blocks, keep_waiting)?;
            done += count;
        }
        Ok(())
    }

    /// Returns once every write before it is on stable storage.
    pub fn flush(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if !self.volatile_write_cache {
            return Ok(());
        }
        let command = Command::new(IO_FLUSH).namespace(self.namespace);
        self.submit(true, command, keep_waiting).map(|_| ())
    }

    /// Deletes the I/O queues, shuts the controller down normally and disables it, handing the
    /// registers and the memory back when the controller no longer uses it.
    pub fn shut_down(mut self, keep_waiting: &mut dyn FnMut() -> bool) -> (R, Option<M>) {
        let _ = self.admin_command(Command::new(ADMIN_DELETE_SUBMISSION_QUEUE).dword(10, 1), keep_waiting);
        let _ = self.admin_command(Command::new(ADMIN_DELETE_COMPLETION_QUEUE).dword(10, 1), keep_waiting);
        let configuration = self.regs.read32(CC);
        self.regs.write32(CC, configuration | CC_SHUTDOWN_NORMAL);
        let _ = self.wait_status(CSTS_SHUTDOWN_MASK, CSTS_SHUTDOWN_COMPLETE, keep_waiting);
        let disabled = self.disable(keep_waiting);
        (self.regs, disabled.then_some(self.memory))
    }
}

/// A namespace's size in logical blocks and its block size, from Identify Namespace's first
/// 256 bytes; block sizes from 512 bytes to 64 KiB, powers of two.
pub fn namespace_size(description: &[u8]) -> Option<(u64, usize)> {
    let blocks = u64::from_le_bytes(description.get(..8)?.try_into().ok()?);
    let format = usize::from(description.get(26)? & 0xf);
    let entry = description.get(128 + format * 4..132 + format * 4)?;
    let exponent = entry[2];
    if !(9..=16).contains(&exponent) || blocks == 0 {
        return None;
    }
    Some((blocks, 1usize << exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_transfers_take_one_or_two_prp_entries_and_large_ones_a_list() {
        assert_eq!(prp_entries(0x10_0000, 512, 0x9000), (0x10_0000, 0, 0));
        assert_eq!(prp_entries(0x10_0000, 8192, 0x9000), (0x10_0000, 0x10_1000, 0));
        assert_eq!(prp_entries(0x10_0000, 16384, 0x9000), (0x10_0000, 0x9000, 3));
    }

    #[test]
    fn a_command_carries_its_id_namespace_and_data_pointers() {
        let command = Command::new(IO_READ)
            .namespace(1)
            .data(0x1234_5678_9000, 0)
            .dword(12, 7)
            .with_id(0x42);
        assert_eq!(command.0[0], 0x0042_0002);
        assert_eq!(command.0[1], 1);
        assert_eq!(command.0[6], 0x5678_9000);
        assert_eq!(command.0[7], 0x1234);
        assert_eq!(command.0[12], 7);
    }

    #[test]
    fn the_namespace_size_comes_from_its_formatted_block_size() {
        let mut description = [0u8; 256];
        description[..8].copy_from_slice(&1_000_000u64.to_le_bytes());
        description[26] = 1;
        description[132 + 2] = 12;
        assert_eq!(namespace_size(&description), Some((1_000_000, 4096)));
        description[132 + 2] = 3;
        assert_eq!(namespace_size(&description), None);
    }
}
