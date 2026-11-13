//! Block devices (D130): the disks drivers install, read and written in sectors of
//! [`SECTOR_SIZE`] bytes, which a file system takes over when it mounts one. A driver
//! implements [`BlockDevice`] and calls [`install`]; nothing is held while no disk is there.
//! Each disk keeps the place it was installed at, its name in `/dev` (`vda` to `vdd`, D192):
//! a disk no file system took is read and written there whole ([`with_disk`]), and
//! [`disks_text`] lists every disk for `/proc/driver/disks`.

use crate::devices::Deadline;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;

/// The unit disks are addressed in.
pub const SECTOR_SIZE: usize = 512;

/// The most disks installed at once.
pub const MAX_DISKS: usize = 4;

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The disk refuses writes.
    ReadOnly,
    /// Past the end of the disk, or not a whole number of sectors.
    OutOfRange,
    /// The device failed the transfer or did not answer in time, for this reason.
    Device(&'static str),
}

/// A disk, as its driver drives it.
pub trait BlockDevice: Send {
    /// Its size in sectors.
    fn sectors(&self) -> u64;
    /// Whether it refuses writes.
    fn is_read_only(&self) -> bool;
    /// Reads the sectors from `sector` on into `buffer`, a whole number of sectors, polling the
    /// device until `deadline`.
    fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        deadline: &mut Deadline,
    ) -> Result<(), BlockError>;
    /// Writes `buffer`, a whole number of sectors, to the sectors from `sector` on, polling the
    /// device until `deadline`.
    fn write(
        &mut self,
        sector: u64,
        buffer: &[u8],
        deadline: &mut Deadline,
    ) -> Result<(), BlockError>;
    /// The disk's own logical block size in bytes, which its partition table counts in
    /// (D238): [`SECTOR_SIZE`] unless the driver says otherwise.
    fn logical_block_bytes(&self) -> usize {
        SECTOR_SIZE
    }
    /// Returns once every write before it is on stable storage, polling the device until
    /// `deadline`; a disk without a volatile cache has nothing to do.
    fn flush(&mut self, _deadline: &mut Deadline) -> Result<(), BlockError> {
        Ok(())
    }
    /// Reads every one of `reads`, each the sectors from its first one into its buffer, with as
    /// many requests in flight at once as the device takes (D220); one after the other by
    /// default.
    fn read_many(
        &mut self,
        reads: &mut [(u64, &mut [u8])],
        deadline: &mut Deadline,
    ) -> Result<(), BlockError> {
        for (sector, buffer) in reads.iter_mut() {
            self.read(*sector, buffer, deadline)?;
        }
        Ok(())
    }
}

/// The names of the disks in `/dev`, by the place they were installed at.
pub const DISK_NAMES: [&str; MAX_DISKS] = ["vda", "vdb", "vdc", "vdd"];

/// What is known of the disk at one place, whether or not a file system holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskFacts {
    /// Whether a disk was installed there.
    pub present: bool,
    /// Its size in sectors.
    pub sectors: u64,
    /// Whether it refuses writes.
    pub read_only: bool,
    /// Where a file system holding it is mounted, `None` while none does.
    pub mounted_on: Option<&'static str>,
}

const NO_DISK: DiskFacts = DiskFacts {
    present: false,
    sectors: 0,
    read_only: false,
    mounted_on: None,
};

static DISKS: LeafSpinLock<[Option<Box<dyn BlockDevice>>; MAX_DISKS]> =
    LeafSpinLock::new_leaf([const { None }; MAX_DISKS]);

static FACTS: LeafSpinLock<[DiskFacts; MAX_DISKS]> = LeafSpinLock::new_leaf([NO_DISK; MAX_DISKS]);

type NewDeadline = fn(u64) -> Deadline;

static NEW_DEADLINE: LeafSpinLock<Option<NewDeadline>> = LeafSpinLock::new_leaf(None);

/// Installs `disk` for a file system to take, at the first free place; gives it back when
/// [`MAX_DISKS`] are installed.
pub fn install(disk: Box<dyn BlockDevice>) -> Result<(), Box<dyn BlockDevice>> {
    let installed = DiskFacts {
        present: true,
        sectors: disk.sectors(),
        read_only: disk.is_read_only(),
        mounted_on: None,
    };
    let mut facts = FACTS.lock();
    let mut disks = DISKS.lock();
    let free = (0..MAX_DISKS).find(|&index| !facts[index].present && disks[index].is_none());
    match free {
        Some(index) => {
            facts[index] = installed;
            disks[index] = Some(disk);
            Ok(())
        }
        None => Err(disk),
    }
}

/// Gives back disk `index` [`take_all`] took, which no file system kept, to its place.
pub fn put_back(index: usize, disk: Box<dyn BlockDevice>) {
    if let Some(slot) = DISKS.lock().get_mut(index) {
        *slot = Some(disk);
    }
}

/// Records that the file system mounted on `place` holds disk `index`, which [`take_all`]
/// took.
pub fn mark_mounted(index: usize, place: &'static str) {
    if let Some(facts) = FACTS.lock().get_mut(index) {
        facts.mounted_on = Some(place);
    }
}

/// Sets how requests to a disk outside a file system make their deadline: polling until
/// `new_deadline(milliseconds)`.
pub fn set_deadline(new_deadline: fn(u64) -> Deadline) {
    *NEW_DEADLINE.lock() = Some(new_deadline);
}

/// What is known of disk `index`.
pub fn facts(index: usize) -> DiskFacts {
    FACTS.lock().get(index).copied().unwrap_or(NO_DISK)
}

/// Runs `transfer` on disk `index` with a deadline [`set_deadline`] made `milliseconds` ahead,
/// holding the disk alone meanwhile: `None` while a file system holds it, another transfer
/// has it, or no disk is there.
pub fn with_disk<R>(
    index: usize,
    milliseconds: u64,
    transfer: impl FnOnce(&mut dyn BlockDevice, &mut Deadline) -> R,
) -> Option<R> {
    let new_deadline = (*NEW_DEADLINE.lock())?;
    let mut disk = DISKS.lock().get_mut(index)?.take()?;
    let mut deadline = new_deadline(milliseconds);
    let result = transfer(&mut *disk, &mut deadline);
    put_back(index, disk);
    Some(result)
}

/// How long one whole-disk transfer may take before it fails.
pub const WHOLE_DISK_TIMEOUT_MS: u64 = 2000;

/// The most bytes one whole-disk read or write moves.
pub const WHOLE_DISK_CHUNK: usize = 60 * 1024;

/// Why a whole-disk read or write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WholeDiskError {
    /// A file system holds the disk, another transfer has it, or no disk is there.
    Unavailable,
    /// The disk refuses writes.
    ReadOnly,
    /// The write starts at or past the disk's end.
    Full,
    /// Memory for the transfer ran out.
    NoMemory,
    /// The device failed the transfer.
    Device,
}

/// Reads disk `index` from byte `offset` into `buffer`: how many bytes, at most
/// [`WHOLE_DISK_CHUNK`], 0 at the disk's end.
pub fn read_at(index: usize, offset: u64, buffer: &mut [u8]) -> Result<usize, WholeDiskError> {
    let size = facts(index).sectors.saturating_mul(SECTOR_SIZE as u64);
    let Some(left) = size.checked_sub(offset).filter(|&left| left > 0) else {
        return Ok(0);
    };
    let length = buffer
        .len()
        .min(WHOLE_DISK_CHUNK)
        .min(usize::try_from(left).unwrap_or(usize::MAX));
    let span = Span::of(offset, length);
    let mut bounce = span.buffer()?;
    with_disk(index, WHOLE_DISK_TIMEOUT_MS, |disk, deadline| {
        disk.read(span.first, &mut bounce, deadline)
    })
    .ok_or(WholeDiskError::Unavailable)?
    .map_err(|_| WholeDiskError::Device)?;
    buffer[..length].copy_from_slice(&bounce[span.skip..span.skip + length]);
    Ok(length)
}

/// Writes `bytes` to disk `index` from byte `offset`: how many, at most [`WHOLE_DISK_CHUNK`]
/// and never past the disk's end.
pub fn write_at(index: usize, offset: u64, bytes: &[u8]) -> Result<usize, WholeDiskError> {
    let facts = facts(index);
    if facts.read_only {
        return Err(WholeDiskError::ReadOnly);
    }
    let size = facts.sectors.saturating_mul(SECTOR_SIZE as u64);
    let Some(left) = size.checked_sub(offset).filter(|&left| left > 0) else {
        return Err(WholeDiskError::Full);
    };
    let length = bytes
        .len()
        .min(WHOLE_DISK_CHUNK)
        .min(usize::try_from(left).unwrap_or(usize::MAX));
    let span = Span::of(offset, length);
    let mut bounce = span.buffer()?;
    let partial = span.skip != 0 || !(span.skip + length).is_multiple_of(SECTOR_SIZE);
    with_disk(index, WHOLE_DISK_TIMEOUT_MS, |disk, deadline| {
        if partial {
            disk.read(span.first, &mut bounce, deadline)?;
        }
        bounce[span.skip..span.skip + length].copy_from_slice(&bytes[..length]);
        disk.write(span.first, &bounce, deadline)
    })
    .ok_or(WholeDiskError::Unavailable)?
    .map_err(|_| WholeDiskError::Device)?;
    Ok(length)
}

/// Waits until every write to disk `index` is on stable storage.
pub fn flush(index: usize) -> Result<(), WholeDiskError> {
    with_disk(index, WHOLE_DISK_TIMEOUT_MS, |disk, deadline| {
        disk.flush(deadline)
    })
    .ok_or(WholeDiskError::Unavailable)?
    .map_err(|_| WholeDiskError::Device)
}

#[derive(Debug, Clone, Copy)]
struct Span {
    first: u64,
    skip: usize,
    sectors: usize,
}

impl Span {
    fn of(offset: u64, length: usize) -> Self {
        let skip = (offset % SECTOR_SIZE as u64) as usize;
        Self {
            first: offset / SECTOR_SIZE as u64,
            skip,
            sectors: (skip + length).div_ceil(SECTOR_SIZE),
        }
    }

    fn buffer(self) -> Result<Vec<u8>, WholeDiskError> {
        let bytes = self.sectors * SECTOR_SIZE;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(bytes)
            .map_err(|_| WholeDiskError::NoMemory)?;
        buffer.resize(bytes, 0);
        Ok(buffer)
    }
}

/// `/proc/driver/disks`' text (D192): a line per disk, `<name> <sectors> <ro|rw> <place>`, the
/// place a file system holding it is mounted on, `-` while none does.
pub fn disks_text() -> Option<Vec<u8>> {
    let facts = *FACTS.lock();
    let mut text = Text(Vec::new());
    for (name, facts) in DISK_NAMES
        .iter()
        .zip(facts)
        .filter(|(_, facts)| facts.present)
    {
        let access = if facts.read_only { "ro" } else { "rw" };
        let place = facts.mounted_on.unwrap_or("-");
        writeln!(text, "{name} {} {access} {place}", facts.sectors).ok()?;
    }
    Some(text.0)
}

struct Text(Vec<u8>);

impl Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0
            .try_reserve(text.len())
            .map_err(|_| core::fmt::Error)?;
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// Whether another disk can be installed.
pub fn has_room() -> bool {
    FACTS.lock().iter().any(|facts| !facts.present)
}

/// Takes every disk installed, in the order they were installed, for file systems to own;
/// those they do not take go back with [`install`].
pub fn take_all() -> [Option<Box<dyn BlockDevice>>; MAX_DISKS] {
    core::mem::take(&mut *DISKS.lock())
}

const MAX_ENTRY_BYTES: usize = 128 * 1024;

/// Where EggOS's data partition is on a disk with a GUID partition table (D238), in sectors:
/// its first sector and how many; `None` when the disk has no valid table, or no such partition.
pub fn eggos_data_partition(
    disk: &mut dyn BlockDevice,
    deadline: &mut Deadline,
) -> Option<(u64, u64)> {
    let block_bytes = disk.logical_block_bytes();
    if !block_bytes.is_multiple_of(SECTOR_SIZE) || block_bytes == 0 {
        return None;
    }
    let per_block = (block_bytes / SECTOR_SIZE) as u64;
    let mut block = Vec::new();
    block.try_reserve_exact(block_bytes).ok()?;
    block.resize(block_bytes, 0);
    disk.read(per_block, &mut block, deadline).ok()?;
    let header = eggos_gpt::read_header(&block)?;
    let entry_bytes = header.entry_count as usize * header.entry_bytes as usize;
    if entry_bytes > MAX_ENTRY_BYTES {
        return None;
    }
    let mut entries = Vec::new();
    let length = entry_bytes.next_multiple_of(block_bytes);
    entries.try_reserve_exact(length).ok()?;
    entries.resize(length, 0);
    disk.read(
        header.entries_at.checked_mul(per_block)?,
        &mut entries,
        deadline,
    )
    .ok()?;
    let partition = eggos_gpt::partitions(&header, &entries)?
        .find(|partition| partition.kind == eggos_gpt::EGGOS_DATA_PARTITION)?;
    let first = partition.first.checked_mul(per_block)?;
    let sectors = partition.blocks().checked_mul(per_block)?;
    (first.checked_add(sectors)? <= disk.sectors()).then_some((first, sectors))
}

/// A partition of a disk, read and written as a disk of its own (D238).
pub struct Partition {
    disk: Box<dyn BlockDevice>,
    first: u64,
    sectors: u64,
}

impl Partition {
    pub fn new(disk: Box<dyn BlockDevice>, first: u64, sectors: u64) -> Self {
        Self {
            disk,
            first,
            sectors,
        }
    }

    fn place(&self, sector: u64, bytes: usize) -> Result<u64, BlockError> {
        let count = (bytes / SECTOR_SIZE) as u64;
        sector
            .checked_add(count)
            .filter(|&end| end <= self.sectors && bytes.is_multiple_of(SECTOR_SIZE))
            .ok_or(BlockError::OutOfRange)?;
        Ok(self.first + sector)
    }
}

impl BlockDevice for Partition {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn is_read_only(&self) -> bool {
        self.disk.is_read_only()
    }
    fn logical_block_bytes(&self) -> usize {
        self.disk.logical_block_bytes()
    }
    fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        deadline: &mut Deadline,
    ) -> Result<(), BlockError> {
        let at = self.place(sector, buffer.len())?;
        self.disk.read(at, buffer, deadline)
    }
    fn write(
        &mut self,
        sector: u64,
        buffer: &[u8],
        deadline: &mut Deadline,
    ) -> Result<(), BlockError> {
        let at = self.place(sector, buffer.len())?;
        self.disk.write(at, buffer, deadline)
    }
    fn flush(&mut self, deadline: &mut Deadline) -> Result<(), BlockError> {
        self.disk.flush(deadline)
    }
}
