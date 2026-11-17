//! A GICv3 Interrupt Translation Service (Arm's GICv3 and GICv4 architecture specification,
//! chapters 5 and 6; D618): it turns a device's message write (its device id and an event id)
//! into an LPI at a redistributor. The service reads commands from a ring in memory and keeps
//! its device and collection tables in memory the kernel gives it.

use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use crate::storage::write64;
use core::sync::atomic::{Ordering, fence};

/// The device tree's `compatible` for an ITS.
pub const COMPATIBLE: &[u8] = b"arm,gic-v3-its";
/// Its register frames: the control frame and the translation frame.
pub const REGISTER_BYTES: usize = 0x2_0000;
/// Where in its registers a device writes its message: what an MSI's address is.
pub const TRANSLATER: usize = 0x1_0040;
/// The command ring's size.
pub const COMMAND_QUEUE_BYTES: usize = 0x1_0000;
/// The first LPI interrupt id.
pub const FIRST_LPI: u32 = 8192;

const GITS_CTLR: usize = 0x0000;
const GITS_TYPER: usize = 0x0008;
const GITS_CBASER: usize = 0x0080;
const GITS_CWRITER: usize = 0x0088;
const GITS_CREADR: usize = 0x0090;
const GITS_BASER: usize = 0x0100;
const TABLE_REGISTERS: usize = 8;

const CTLR_ENABLED: u32 = 1;
const CTLR_QUIESCENT: u32 = 1 << 31;
const TYPER_PHYSICAL_TARGETS: u64 = 1 << 19;
const TYPER_ITT_ENTRY_SHIFT: u32 = 4;
const TYPER_DEVICE_BITS_SHIFT: u32 = 13;
const VALID: u64 = 1 << 63;
const INNER_WRITE_BACK: u64 = 0b111 << 59;
const INNER_SHAREABLE: u64 = 0b01 << 10;
const BASER_TYPE_SHIFT: u32 = 56;
const BASER_ENTRY_SHIFT: u32 = 48;
const BASER_KEPT: u64 = (0b111 << BASER_TYPE_SHIFT) | (0b1_1111 << BASER_ENTRY_SHIFT);
const TABLE_DEVICES: u64 = 1;
const TABLE_COLLECTIONS: u64 = 4;
const TABLE_PAGE: usize = 4096;
const MOST_TABLE_PAGES: usize = 256;
const MOST_DEVICE_BITS: u32 = 16;
const COLLECTIONS: usize = 16;
const COMMAND_BYTES: usize = 32;
const QUEUE_PAGES: u64 = (COMMAND_QUEUE_BYTES / TABLE_PAGE) as u64;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const ITT_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_ff00;

const MAPD: u64 = 0x08;
const MAPC: u64 = 0x09;
const MAPTI: u64 = 0x0a;
const INV: u64 = 0x0c;
const SYNC: u64 = 0x05;

/// One command, as the ring holds it: four double words.
pub type Command = [u64; 4];

/// `MAPD`: device `device`'s events go through the interrupt translation table at `table`,
/// which holds `2^event_bits` events.
pub const fn map_device(device: u32, table: u64, event_bits: u32) -> Command {
    [
        MAPD | ((device as u64) << 32),
        (event_bits.saturating_sub(1) & 0x1f) as u64,
        VALID | (table & ITT_ADDRESS_MASK),
        0,
    ]
}

/// `MAPC`: collection `collection` delivers to redistributor `target` (a physical address
/// shifted down 16 bits, or a processor number, as the ITS's `GITS_TYPER.PTA` says).
pub const fn map_collection(collection: u16, target: u64) -> Command {
    [MAPC, 0, VALID | (target << 16) | collection as u64, 0]
}

/// `MAPTI`: device `device`'s event `event` is LPI `interrupt`, in collection `collection`.
pub const fn map_translation(device: u32, event: u32, interrupt: u32, collection: u16) -> Command {
    [
        MAPTI | ((device as u64) << 32),
        event as u64 | ((interrupt as u64) << 32),
        collection as u64,
        0,
    ]
}

/// `INV`: the redistributor reads the configuration of device `device`'s event `event` again.
pub const fn invalidate(device: u32, event: u32) -> Command {
    [INV | ((device as u64) << 32), event as u64, 0, 0]
}

/// `SYNC`: every command before it has taken effect at redistributor `target`.
pub const fn synchronise(target: u64) -> Command {
    [SYNC, 0, target << 16, 0]
}

/// Why the service could not be set up or a command not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TimedOut,
    NoMemory,
}

fn read64<R: RegisterIo>(regs: &R, offset: usize) -> u64 {
    u64::from(regs.read32(offset)) | (u64::from(regs.read32(offset + 4)) << 32)
}

fn write64_register<R: RegisterIo>(regs: &R, offset: usize, value: u64) {
    regs.write32(offset, value as u32);
    regs.write32(offset + 4, (value >> 32) as u32);
}

/// An ITS with its command ring.
#[derive(Debug)]
pub struct Its<R: RegisterIo, M: DmaMemory> {
    regs: R,
    queue: M,
    write: usize,
    typer: u64,
}

impl<R: RegisterIo, M: DmaMemory> Its<R, M> {
    /// Stops the service at `regs`, waits for it to be quiescent and gives it `queue`
    /// ([`COMMAND_QUEUE_BYTES`] or more) as its command ring.
    pub fn new(
        regs: R,
        queue: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, M)> {
        if queue.len() < COMMAND_QUEUE_BYTES {
            return Err((Error::NoMemory, queue));
        }
        let control = regs.read32(GITS_CTLR);
        regs.write32(GITS_CTLR, control & !CTLR_ENABLED);
        while regs.read32(GITS_CTLR) & CTLR_QUIESCENT == 0 {
            if !keep_waiting() {
                return Err((Error::TimedOut, queue));
            }
            core::hint::spin_loop();
        }
        let base = queue.dma_addr().get();
        write64_register(
            &regs,
            GITS_CBASER,
            VALID | INNER_WRITE_BACK | INNER_SHAREABLE | (base & ADDRESS_MASK) | (QUEUE_PAGES - 1),
        );
        write64_register(&regs, GITS_CWRITER, 0);
        let typer = read64(&regs, GITS_TYPER);
        Ok(Self {
            regs,
            queue,
            write: 0,
            typer,
        })
    }

    /// The bytes of one interrupt translation table entry.
    pub fn itt_entry_bytes(&self) -> usize {
        (((self.typer >> TYPER_ITT_ENTRY_SHIFT) & 0xf) + 1) as usize
    }

    /// The device ids it takes: `2^bits`, at most `2^16` here.
    pub fn device_bits(&self) -> u32 {
        ((((self.typer >> TYPER_DEVICE_BITS_SHIFT) & 0x1f) + 1) as u32).min(MOST_DEVICE_BITS)
    }

    /// What `MAPC` and `SYNC` name redistributor `processor` (its `GICR_TYPER` processor number),
    /// at physical address `address`, by.
    pub fn target(&self, processor: u16, address: u64) -> u64 {
        match self.typer & TYPER_PHYSICAL_TARGETS != 0 {
            true => address >> 16,
            false => u64::from(processor),
        }
    }

    /// Gives the device and collection tables their memory: `allocate(bytes)` hands back the
    /// physical address of that many zeroed bytes, 4 KiB aligned, which the service keeps.
    pub fn set_tables(
        &mut self,
        allocate: &mut dyn FnMut(usize) -> Option<u64>,
    ) -> Result<(), Error> {
        for index in 0..TABLE_REGISTERS {
            let offset = GITS_BASER + index * 8;
            let register = read64(&self.regs, offset);
            let kind = (register >> BASER_TYPE_SHIFT) & 0b111;
            let entry_bytes = (((register >> BASER_ENTRY_SHIFT) & 0b1_1111) + 1) as usize;
            let entries = match kind {
                TABLE_DEVICES => 1usize << self.device_bits(),
                TABLE_COLLECTIONS => COLLECTIONS,
                _ => continue,
            };
            let pages = (entries * entry_bytes)
                .div_ceil(TABLE_PAGE)
                .clamp(1, MOST_TABLE_PAGES);
            let table = allocate(pages * TABLE_PAGE).ok_or(Error::NoMemory)?;
            write64_register(
                &self.regs,
                offset,
                VALID
                    | INNER_WRITE_BACK
                    | INNER_SHAREABLE
                    | (register & BASER_KEPT)
                    | (table & ADDRESS_MASK)
                    | (pages as u64 - 1),
            );
        }
        Ok(())
    }

    /// Turns the service on.
    pub fn enable(&self) {
        let control = self.regs.read32(GITS_CTLR);
        self.regs.write32(GITS_CTLR, control | CTLR_ENABLED);
    }

    /// Puts `command` on the ring and waits until the service has read it.
    pub fn run(
        &mut self,
        command: Command,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        for (index, word) in command.into_iter().enumerate() {
            write64(&self.queue, self.write + index * 8, word);
        }
        fence(Ordering::SeqCst);
        self.write = (self.write + COMMAND_BYTES) % COMMAND_QUEUE_BYTES;
        write64_register(&self.regs, GITS_CWRITER, self.write as u64);
        loop {
            let read = read64(&self.regs, GITS_CREADR) as usize & (COMMAND_QUEUE_BYTES - 1);
            if read == self.write {
                return Ok(());
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
    }
}

const _: () = assert!(COMMAND_QUEUE_BYTES.is_multiple_of(COMMAND_BYTES));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_carry_their_fields_where_the_specification_puts_them() {
        let mapd = map_device(0x0010, 0x8000_1200, 5);
        assert_eq!(mapd[0], 0x0000_0010_0000_0008);
        assert_eq!(mapd[1], 4);
        assert_eq!(mapd[2], VALID | 0x8000_1200);
        let mapti = map_translation(0x0010, 3, 8195, 0);
        assert_eq!(mapti[0] & 0xff, MAPTI);
        assert_eq!(mapti[1], 3 | (8195 << 32));
        let mapc = map_collection(0, 2);
        assert_eq!(mapc[2], VALID | (2 << 16));
        assert_eq!(synchronise(2)[2], 2 << 16);
    }
}
