//! AHCI SATA controllers (Serial ATA AHCI 1.3.1; D238): the controller taken from the firmware,
//! each port with an ATA disk set up, and one command at a time per disk, polled: IDENTIFY
//! DEVICE, READ and WRITE DMA EXT, FLUSH CACHE EXT.

use super::{Covering, Error, SECTOR_SIZE, copy_in, copy_out, covering, write32, zero};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use core::sync::atomic::{Ordering, fence};

const CAP: usize = 0x00;
const GHC: usize = 0x04;
const PORTS_IMPLEMENTED: usize = 0x0c;
const CAP2: usize = 0x24;
const BOHC: usize = 0x28;
const CAP_64_BIT: u32 = 1 << 31;
const GHC_AHCI_ENABLE: u32 = 1 << 31;
const GHC_INTERRUPT_ENABLE: u32 = 1 << 1;
const INTERRUPT_STATUS: usize = 0x08;
const PORT_INTERRUPTS_WANTED: u32 = 1 | (1 << 3) | (1 << 30);
const CAP2_BIOS_HANDOFF: u32 = 1;
const BOHC_BIOS_OWNED: u32 = 1;
const BOHC_OS_OWNED: u32 = 1 << 1;

/// Where port `n`'s registers start in the controller's block, and how long they are.
pub const PORT_REGISTERS: usize = 0x100;
pub const PORT_REGISTER_BYTES: usize = 0x80;
pub const MAX_PORTS: usize = 32;

const PORT_COMMAND_LIST: usize = 0x00;
const PORT_RECEIVED_FIS: usize = 0x08;
const PORT_INTERRUPT_STATUS: usize = 0x10;
const PORT_INTERRUPT_ENABLE: usize = 0x14;
const PORT_COMMAND: usize = 0x18;
const PORT_TASK_FILE: usize = 0x20;
const PORT_SIGNATURE: usize = 0x24;
const PORT_SATA_STATUS: usize = 0x28;
const PORT_SATA_ERROR: usize = 0x30;
const PORT_COMMAND_ISSUE: usize = 0x38;

const COMMAND_START: u32 = 1;
const COMMAND_FIS_RECEIVE: u32 = 1 << 4;
const COMMAND_FIS_RUNNING: u32 = 1 << 14;
const COMMAND_LIST_RUNNING: u32 = 1 << 15;
const TASK_FILE_ERROR: u32 = 1;
const TASK_FILE_DATA_REQUEST: u32 = 1 << 3;
const TASK_FILE_BUSY: u32 = 1 << 7;
const INTERRUPT_TASK_FILE_ERROR: u32 = 1 << 30;
const DEVICE_PRESENT_WITH_LINK: u32 = 3;
const LINK_ACTIVE: u32 = 1;
pub const SIGNATURE_ATA: u32 = 0x0000_0101;

const FIS_REGISTER_HOST_TO_DEVICE: u8 = 0x27;
const FIS_COMMAND: u8 = 0x80;
const DEVICE_LBA_MODE: u8 = 1 << 6;
const ATA_IDENTIFY_DEVICE: u8 = 0xec;
const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_CACHE_EXT: u8 = 0xea;

const COMMAND_LIST: usize = 0x0000;
const RECEIVED_FIS: usize = 0x0400;
const COMMAND_TABLE: usize = 0x0800;
const PRDT: usize = COMMAND_TABLE + 0x80;
const PRDT_ENTRIES: usize = 16;
const IDENTIFY: usize = 0x1000;
const DATA: usize = 0x2000;
pub const MEMORY_BYTES: usize = 0x2_0000;
pub const DATA_BYTES: usize = MEMORY_BYTES - DATA;
const MAX_SECTORS_PER_COMMAND: usize = 0xffff;
const PRD_MAX_BYTES: usize = 0x40_0000;

const _: () = assert!(PRDT + PRDT_ENTRIES * 16 <= IDENTIFY);
const _: () = assert!(DATA_BYTES.div_ceil(PRD_MAX_BYTES) <= PRDT_ENTRIES);

/// What the controller-wide setup found: which ports hold an ATA disk, and whether the
/// controller reaches 64-bit addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controller {
    pub disk_ports: u32,
    pub addresses_64_bit: bool,
}

/// Takes the controller at `regs` from the firmware (the BIOS handoff, where it has one),
/// turns AHCI mode on and lists the ports with an ATA disk on an active link.
pub fn take_controller<R: RegisterIo>(
    regs: &R,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Controller {
    if regs.read32(CAP2) & CAP2_BIOS_HANDOFF != 0 {
        let handoff = regs.read32(BOHC);
        regs.write32(BOHC, handoff | BOHC_OS_OWNED);
        while regs.read32(BOHC) & BOHC_BIOS_OWNED != 0 && keep_waiting() {
            core::hint::spin_loop();
        }
    }
    let control = regs.read32(GHC);
    regs.write32(GHC, control | GHC_AHCI_ENABLE);
    let implemented = regs.read32(PORTS_IMPLEMENTED);
    let mut disk_ports = 0;
    for port in 0..MAX_PORTS {
        if implemented & (1 << port) == 0 {
            continue;
        }
        let at = PORT_REGISTERS + port * PORT_REGISTER_BYTES;
        let status = regs.read32(at + PORT_SATA_STATUS);
        let detected = status & 0xf;
        let power = (status >> 8) & 0xf;
        if detected == DEVICE_PRESENT_WITH_LINK
            && power == LINK_ACTIVE
            && regs.read32(at + PORT_SIGNATURE) == SIGNATURE_ATA
        {
            disk_ports |= 1 << port;
        }
    }
    Controller {
        disk_ports,
        addresses_64_bit: regs.read32(CAP) & CAP_64_BIT != 0,
    }
}

/// Turns the controller's interrupt on (D262), once a message interrupt is connected; each disk
/// then asks for its port's.
pub fn enable_controller_interrupt<R: RegisterIo>(regs: &R) {
    let control = regs.read32(GHC);
    regs.write32(GHC, control | GHC_INTERRUPT_ENABLE);
}

/// Clears the controller-wide interrupt status bit of `port` (`IS.IPS`), so its next event
/// sends a new message.
pub fn clear_port_interrupt<R: RegisterIo>(regs: &R, port: usize) {
    regs.write32(INTERRUPT_STATUS, 1 << port);
}

/// The host-to-device register FIS for ATA `command` on `lba` over `count` sectors.
pub fn register_fis(command: u8, lba: u64, count: u16) -> [u8; 20] {
    let mut fis = [0u8; 20];
    fis[0] = FIS_REGISTER_HOST_TO_DEVICE;
    fis[1] = FIS_COMMAND;
    fis[2] = command;
    fis[4] = lba as u8;
    fis[5] = (lba >> 8) as u8;
    fis[6] = (lba >> 16) as u8;
    fis[7] = DEVICE_LBA_MODE;
    fis[8] = (lba >> 24) as u8;
    fis[9] = (lba >> 32) as u8;
    fis[10] = (lba >> 40) as u8;
    fis[12] = count as u8;
    fis[13] = (count >> 8) as u8;
    fis
}

/// A disk's size in logical blocks and its logical block size, from IDENTIFY DEVICE's words.
pub fn identified_size(words: &[u16; 256]) -> Option<(u64, usize)> {
    const SUPPORTS_48_BIT: u16 = 1 << 10;
    const SECTOR_SIZE_VALID: u16 = 0b01 << 14;
    const LARGE_LOGICAL_SECTORS: u16 = 1 << 12;
    if words[83] & SUPPORTS_48_BIT == 0 {
        return None;
    }
    let blocks = u64::from(words[100])
        | (u64::from(words[101]) << 16)
        | (u64::from(words[102]) << 32)
        | (u64::from(words[103]) << 48);
    let size_word = words[106];
    let block_bytes =
        if size_word & 0xc000 == SECTOR_SIZE_VALID && size_word & LARGE_LOGICAL_SECTORS != 0 {
            let size_in_words = u32::from(words[117]) | (u32::from(words[118]) << 16);
            size_in_words as usize * 2
        } else {
            SECTOR_SIZE
        };
    (blocks != 0 && block_bytes.is_power_of_two() && block_bytes >= SECTOR_SIZE)
        .then_some((blocks, block_bytes))
}

/// One SATA disk on one port, its port's registers at `regs`.
#[derive(Debug)]
pub struct AhciDisk<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    blocks: u64,
    block_bytes: usize,
}

impl<R: RegisterIo, M: DmaMemory> AhciDisk<R, M> {
    /// Sets the port up with its command list, received-FIS area and command table in `memory`
    /// ([`MEMORY_BYTES`] or more, below 4 GiB unless `addresses_64_bit`) and identifies the disk.
    /// On failure the port is stopped where it could be, and `memory` handed back only then.
    pub fn new(
        regs: R,
        memory: M,
        addresses_64_bit: bool,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((
                Error::Unsupported("too little DMA memory for a SATA port"),
                Some(memory),
            ));
        }
        let end = memory.dma_addr().get().saturating_add(memory.len() as u64);
        if !addresses_64_bit && end > u64::from(u32::MAX) {
            return Err((
                Error::Unsupported("the controller reaches only 32-bit addresses"),
                Some(memory),
            ));
        }
        let mut disk = Self {
            regs,
            memory,
            blocks: 0,
            block_bytes: SECTOR_SIZE,
        };
        if !disk.stop(keep_waiting) {
            return Err((Error::TimedOut, Some(disk.memory)));
        }
        match disk.start(keep_waiting) {
            Ok(()) => Ok(disk),
            Err(error) => {
                let stopped = disk.stop(keep_waiting);
                Err((error, stopped.then_some(disk.memory)))
            }
        }
    }

    fn stop(&self, keep_waiting: &mut dyn FnMut() -> bool) -> bool {
        let command = self.regs.read32(PORT_COMMAND);
        self.regs.write32(PORT_COMMAND, command & !COMMAND_START);
        while self.regs.read32(PORT_COMMAND) & COMMAND_LIST_RUNNING != 0 {
            if !keep_waiting() {
                return false;
            }
            core::hint::spin_loop();
        }
        let command = self.regs.read32(PORT_COMMAND);
        self.regs
            .write32(PORT_COMMAND, command & !COMMAND_FIS_RECEIVE);
        while self.regs.read32(PORT_COMMAND) & COMMAND_FIS_RUNNING != 0 {
            if !keep_waiting() {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    fn start(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        zero(&self.memory, 0, DATA);
        let base = self.memory.dma_addr().get();
        let list = base + COMMAND_LIST as u64;
        let received = base + RECEIVED_FIS as u64;
        self.regs.write32(PORT_COMMAND_LIST, list as u32);
        self.regs
            .write32(PORT_COMMAND_LIST + 4, (list >> 32) as u32);
        self.regs.write32(PORT_RECEIVED_FIS, received as u32);
        self.regs
            .write32(PORT_RECEIVED_FIS + 4, (received >> 32) as u32);
        self.regs.write32(PORT_SATA_ERROR, u32::MAX);
        self.regs.write32(PORT_INTERRUPT_STATUS, u32::MAX);
        self.regs.write32(PORT_INTERRUPT_ENABLE, 0);
        let command = self.regs.read32(PORT_COMMAND);
        self.regs
            .write32(PORT_COMMAND, command | COMMAND_FIS_RECEIVE);
        while self.regs.read32(PORT_TASK_FILE) & (TASK_FILE_BUSY | TASK_FILE_DATA_REQUEST) != 0 {
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
        let command = self.regs.read32(PORT_COMMAND);
        self.regs.write32(PORT_COMMAND, command | COMMAND_START);

        self.issue(
            ATA_IDENTIFY_DEVICE,
            0,
            0,
            IDENTIFY,
            SECTOR_SIZE,
            false,
            keep_waiting,
        )?;
        let mut bytes = [0u8; SECTOR_SIZE];
        copy_out(&self.memory, IDENTIFY, &mut bytes);
        let mut words = [0u16; 256];
        for (word, pair) in words.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *word = u16::from_le_bytes(*pair);
        }
        let (blocks, block_bytes) = identified_size(&words)
            .ok_or(Error::Unsupported("the disk has no 48-bit addressing"))?;
        self.blocks = blocks;
        self.block_bytes = block_bytes;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn issue(
        &mut self,
        command: u8,
        lba: u64,
        count: u16,
        data_offset: usize,
        data_bytes: usize,
        write: bool,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let base = self.memory.dma_addr().get();
        zero(
            &self.memory,
            COMMAND_TABLE,
            PRDT + PRDT_ENTRIES * 16 - COMMAND_TABLE,
        );
        copy_in(
            &self.memory,
            COMMAND_TABLE,
            &register_fis(command, lba, count),
        );
        let mut entries = 0;
        let mut offset = 0;
        while offset < data_bytes {
            let length = (data_bytes - offset).min(PRD_MAX_BYTES);
            let address = base + (data_offset + offset) as u64;
            let entry = PRDT + entries * 16;
            write32(&self.memory, entry, address as u32);
            write32(&self.memory, entry + 4, (address >> 32) as u32);
            write32(&self.memory, entry + 12, (length - 1) as u32);
            entries += 1;
            offset += length;
        }
        let flags = 5 | if write { 1 << 6 } else { 0 } | ((entries as u32) << 16);
        let table = base + COMMAND_TABLE as u64;
        write32(&self.memory, COMMAND_LIST, flags);
        write32(&self.memory, COMMAND_LIST + 4, 0);
        write32(&self.memory, COMMAND_LIST + 8, table as u32);
        write32(&self.memory, COMMAND_LIST + 12, (table >> 32) as u32);
        fence(Ordering::SeqCst);
        self.regs.write32(PORT_INTERRUPT_STATUS, u32::MAX);
        self.regs.write32(PORT_COMMAND_ISSUE, 1);
        loop {
            if self.regs.read32(PORT_COMMAND_ISSUE) & 1 == 0 {
                fence(Ordering::SeqCst);
                break;
            }
            if self.regs.read32(PORT_INTERRUPT_STATUS) & INTERRUPT_TASK_FILE_ERROR != 0 {
                return Err(Error::CommandFailed);
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
        if self.regs.read32(PORT_TASK_FILE) & TASK_FILE_ERROR != 0 {
            return Err(Error::CommandFailed);
        }
        Ok(())
    }

    /// Asks the port for an interrupt when a command completes or fails (D262).
    pub fn enable_interrupts(&mut self) {
        self.regs.write32(PORT_INTERRUPT_STATUS, u32::MAX);
        self.regs
            .write32(PORT_INTERRUPT_ENABLE, PORT_INTERRUPTS_WANTED);
    }

    pub fn sectors(&self) -> u64 {
        self.blocks
            .saturating_mul((self.block_bytes / SECTOR_SIZE) as u64)
    }

    pub fn block_bytes(&self) -> usize {
        self.block_bytes
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
        let blocks = (DATA_BYTES / self.block_bytes).clamp(2, MAX_SECTORS_PER_COMMAND) as u64 - 1;
        blocks * per_block
    }

    fn transfer(
        &mut self,
        write: bool,
        covering: Covering,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let command = if write {
            ATA_WRITE_DMA_EXT
        } else {
            ATA_READ_DMA_EXT
        };
        let bytes = covering.blocks as usize * self.block_bytes;
        self.issue(
            command,
            covering.first_block,
            covering.blocks as u16,
            DATA,
            bytes,
            write,
            keep_waiting,
        )
    }

    pub fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let sectors = self.check(sector, buffer.len())?;
        let chunk = self.chunk_sectors();
        let mut done = 0;
        while done < sectors {
            let count = chunk.min(sectors - done);
            let span = covering(sector + done, count, self.block_bytes);
            self.transfer(false, span, keep_waiting)?;
            let start = done as usize * SECTOR_SIZE;
            let length = count as usize * SECTOR_SIZE;
            copy_out(
                &self.memory,
                DATA + span.skip_bytes,
                &mut buffer[start..start + length],
            );
            done += count;
        }
        Ok(())
    }

    pub fn write(
        &mut self,
        sector: u64,
        buffer: &[u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let sectors = self.check(sector, buffer.len())?;
        let chunk = self.chunk_sectors();
        let mut done = 0;
        while done < sectors {
            let count = chunk.min(sectors - done);
            let span = covering(sector + done, count, self.block_bytes);
            let length = count as usize * SECTOR_SIZE;
            if span.skip_bytes != 0 || length != span.blocks as usize * self.block_bytes {
                self.transfer(false, span, keep_waiting)?;
            }
            let start = done as usize * SECTOR_SIZE;
            copy_in(
                &self.memory,
                DATA + span.skip_bytes,
                &buffer[start..start + length],
            );
            self.transfer(true, span, keep_waiting)?;
            done += count;
        }
        Ok(())
    }

    pub fn flush(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.issue(ATA_FLUSH_CACHE_EXT, 0, 0, DATA, 0, false, keep_waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn the_register_fis_carries_a_48_bit_address_and_count() {
        let fis = register_fis(ATA_READ_DMA_EXT, 0x0102_0304_0506, 0x0708);
        assert_eq!(fis[0], 0x27);
        assert_eq!(fis[1], 0x80);
        assert_eq!(fis[2], ATA_READ_DMA_EXT);
        assert_eq!(&fis[4..7], &[0x06, 0x05, 0x04]);
        assert_eq!(fis[7], 0x40);
        assert_eq!(&fis[8..11], &[0x03, 0x02, 0x01]);
        assert_eq!(&fis[12..14], &[0x08, 0x07]);
    }

    #[test]
    fn identify_gives_the_48_bit_size_and_a_large_sector() {
        let mut words = [0u16; 256];
        words[83] = 1 << 10;
        words[100] = 0x5000;
        words[101] = 0x0001;
        assert_eq!(identified_size(&words), Some((0x1_5000, 512)));
        words[106] = (0b01 << 14) | (1 << 12);
        words[117] = 2048;
        assert_eq!(identified_size(&words), Some((0x1_5000, 4096)));
        words[83] = 0;
        assert_eq!(identified_size(&words), None);
    }

    #[test]
    fn only_ports_with_an_active_ata_disk_are_listed() {
        let regs = FakeRegs::default();
        regs.script_reads(PORTS_IMPLEMENTED, &[0b11]);
        let port = |n: usize| PORT_REGISTERS + n * PORT_REGISTER_BYTES;
        regs.script_reads(port(0) + PORT_SATA_STATUS, &[0x113]);
        regs.script_reads(port(0) + PORT_SIGNATURE, &[SIGNATURE_ATA]);
        regs.script_reads(port(1) + PORT_SATA_STATUS, &[0x113]);
        regs.script_reads(port(1) + PORT_SIGNATURE, &[0xeb14_0101]);
        let controller = take_controller(&regs, &mut || false);
        assert_eq!(controller.disk_ports, 0b01);
    }
}
