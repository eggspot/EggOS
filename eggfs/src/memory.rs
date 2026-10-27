//! A disk in memory, for tests, benchmarks and checking disk images on a host. It keeps what
//! a power cut would leave apart from what it would lose: writes since the last flush are
//! volatile, and [`MemoryDisk::after_power_cut`] makes the disk a cut leaves, keeping any
//! chosen subset of those writes' sectors. [`MemoryDisk::cut_power_after`] makes every write
//! and flush after a chosen number of writes fail, as a disk that lost its power does.

use crate::layout::BLOCK_SIZE;
use crate::{Disk, DiskError};
use alloc::vec::Vec;

/// The unit a power cut tears writes into.
pub const SECTOR_SIZE: usize = 512;

/// A disk in memory: see the module documentation.
#[derive(Debug, Clone)]
pub struct MemoryDisk {
    current: Vec<u8>,
    durable: Vec<u8>,
    pending: Vec<(usize, Vec<u8>)>,
    writes: u64,
    flushes: u64,
    power_left: Option<u64>,
}

impl MemoryDisk {
    /// A disk of `blocks` blocks of zeros.
    pub fn new(blocks: usize) -> Self {
        Self::from_image(alloc::vec![0; blocks * BLOCK_SIZE])
    }

    /// A disk holding `image`, cut to whole blocks, all of it stable.
    pub fn from_image(mut image: Vec<u8>) -> Self {
        image.truncate(image.len() / BLOCK_SIZE * BLOCK_SIZE);
        Self {
            durable: image.clone(),
            current: image,
            pending: Vec::new(),
            writes: 0,
            flushes: 0,
            power_left: None,
        }
    }

    /// What reads see now.
    pub fn image(&self) -> &[u8] {
        &self.current
    }

    /// What reads see now, to change behind the file system's back.
    pub fn image_mut(&mut self) -> &mut [u8] {
        &mut self.current
    }

    /// Writes made so far.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Flushes made so far.
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Lets `writes` more writes through, then fails every write and flush.
    pub fn cut_power_after(&mut self, writes: u64) {
        self.power_left = Some(writes);
    }

    /// Writes since the last flush, each as (byte offset, length).
    pub fn volatile_writes(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.pending
            .iter()
            .map(|(offset, bytes)| (*offset, bytes.len()))
    }

    /// The disk a power cut leaves now: what was flushed, plus each sector of the writes since
    /// that `keep` (write index, sector index within it) chooses, applied in order.
    pub fn after_power_cut(&self, mut keep: impl FnMut(usize, usize) -> bool) -> MemoryDisk {
        let mut image = self.durable.clone();
        for (index, (offset, bytes)) in self.pending.iter().enumerate() {
            for (sector, data) in bytes.chunks(SECTOR_SIZE).enumerate() {
                if keep(index, sector) {
                    let at = offset + sector * SECTOR_SIZE;
                    image[at..at + data.len()].copy_from_slice(data);
                }
            }
        }
        MemoryDisk::from_image(image)
    }

    fn take_power(&mut self) -> Result<(), DiskError> {
        match &mut self.power_left {
            Some(0) => Err(DiskError),
            Some(left) => {
                *left -= 1;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl Disk for MemoryDisk {
    fn blocks(&self) -> u64 {
        (self.current.len() / BLOCK_SIZE) as u64
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError> {
        let start = usize::try_from(block).map_err(|_| DiskError)? * BLOCK_SIZE;
        let end = start.checked_add(buffer.len()).ok_or(DiskError)?;
        if !buffer.len().is_multiple_of(BLOCK_SIZE) || end > self.current.len() {
            return Err(DiskError);
        }
        buffer.copy_from_slice(&self.current[start..end]);
        Ok(())
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError> {
        let start = usize::try_from(block).map_err(|_| DiskError)? * BLOCK_SIZE;
        let end = start.checked_add(buffer.len()).ok_or(DiskError)?;
        if !buffer.len().is_multiple_of(BLOCK_SIZE) || end > self.current.len() {
            return Err(DiskError);
        }
        self.take_power()?;
        self.writes += 1;
        self.current[start..end].copy_from_slice(buffer);
        self.pending.push((start, buffer.to_vec()));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        if self.power_left == Some(0) {
            return Err(DiskError);
        }
        self.flushes += 1;
        for (offset, bytes) in &self.pending {
            self.durable[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        self.pending.clear();
        Ok(())
    }
}
