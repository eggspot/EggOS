//! Disk controllers on PCI (D238): NVMe and AHCI (SATA). Each driver keeps its queues and a
//! bounce buffer in one block of DMA memory, polls its controller for completions and moves
//! data in 512-byte sectors whatever the disk's own logical block size.

pub mod ahci;
pub mod nvme;

use crate::dma::DmaMemory;

/// The unit the kernel addresses disks in.
pub const SECTOR_SIZE: usize = 512;

/// Why a transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The controller did not answer before the deadline.
    TimedOut,
    /// Past the end of the disk, or not a whole number of sectors.
    OutOfRange,
    /// The disk refuses writes.
    ReadOnly,
    /// The controller reported a fatal state.
    ControllerFailed,
    /// A command ended with an error status.
    CommandFailed,
    /// The device is not one the driver takes, for this reason.
    Unsupported(&'static str),
}

impl Error {
    pub fn description(self) -> &'static str {
        match self {
            Error::TimedOut => "the disk controller did not answer in time",
            Error::OutOfRange => "the transfer lies outside the disk",
            Error::ReadOnly => "the disk is read-only",
            Error::ControllerFailed => "the disk controller reported a fatal error",
            Error::CommandFailed => "the disk refused the command",
            Error::Unsupported(reason) => reason,
        }
    }
}

/// The sectors of a request as whole logical blocks: the first block, how many, and how many
/// bytes into the first block the request starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Covering {
    pub first_block: u64,
    pub blocks: u64,
    pub skip_bytes: usize,
}

/// The whole blocks of `block_bytes` that hold `sectors` sectors from `sector` on.
pub fn covering(sector: u64, sectors: u64, block_bytes: usize) -> Covering {
    let per_block = (block_bytes / SECTOR_SIZE).max(1) as u64;
    let first_block = sector / per_block;
    let end_block = (sector + sectors).div_ceil(per_block);
    Covering {
        first_block,
        blocks: end_block - first_block,
        skip_bytes: ((sector % per_block) as usize) * SECTOR_SIZE,
    }
}

pub(crate) fn write32<M: DmaMemory>(memory: &M, offset: usize, value: u32) {
    assert!(offset + 4 <= memory.len() && offset.is_multiple_of(4));
    // SAFETY: inside the DMA memory (checked) and 4-aligned, which `DmaMemory` makes writable.
    unsafe { core::ptr::write_volatile(memory.as_mut_ptr().add(offset).cast::<u32>(), value) }
}

pub(crate) fn read32<M: DmaMemory>(memory: &M, offset: usize) -> u32 {
    assert!(offset + 4 <= memory.len() && offset.is_multiple_of(4));
    // SAFETY: inside the DMA memory (checked) and 4-aligned, which `DmaMemory` makes readable.
    unsafe { core::ptr::read_volatile(memory.as_mut_ptr().add(offset).cast::<u32>()) }
}

pub(crate) fn write64<M: DmaMemory>(memory: &M, offset: usize, value: u64) {
    write32(memory, offset, value as u32);
    write32(memory, offset + 4, (value >> 32) as u32);
}

pub(crate) fn zero<M: DmaMemory>(memory: &M, offset: usize, length: usize) {
    assert!(offset + length <= memory.len());
    // SAFETY: inside the DMA memory (checked), which `DmaMemory` makes writable.
    unsafe { memory.as_mut_ptr().add(offset).write_bytes(0, length) }
}

pub(crate) fn copy_in<M: DmaMemory>(memory: &M, offset: usize, bytes: &[u8]) {
    assert!(offset + bytes.len() <= memory.len());
    // SAFETY: inside the DMA memory (checked), which does not overlap `bytes` (kernel memory the
    // caller owns).
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.as_mut_ptr().add(offset), bytes.len())
    }
}

pub(crate) fn copy_out<M: DmaMemory>(memory: &M, offset: usize, bytes: &mut [u8]) {
    assert!(offset + bytes.len() <= memory.len());
    // SAFETY: as in `copy_in`, the other way.
    unsafe {
        core::ptr::copy_nonoverlapping(
            memory.as_mut_ptr().add(offset),
            bytes.as_mut_ptr(),
            bytes.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sectors_on_a_4k_disk_cover_whole_blocks() {
        assert_eq!(
            covering(9, 2, 4096),
            Covering {
                first_block: 1,
                blocks: 1,
                skip_bytes: 512
            }
        );
        assert_eq!(
            covering(7, 2, 4096),
            Covering {
                first_block: 0,
                blocks: 2,
                skip_bytes: 3584
            }
        );
        assert_eq!(
            covering(5, 3, 512),
            Covering {
                first_block: 5,
                blocks: 3,
                skip_bytes: 0
            }
        );
    }
}
