//! Block devices (D130): the disks drivers install, read and written in sectors of
//! [`SECTOR_SIZE`] bytes, which a file system takes over when it mounts one. A driver
//! implements [`BlockDevice`] and calls [`install`]; nothing is held while no disk is there.

use crate::devices::Deadline;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;

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
    /// Returns once every write before it is on stable storage, polling the device until
    /// `deadline`; a disk without a volatile cache has nothing to do.
    fn flush(&mut self, _deadline: &mut Deadline) -> Result<(), BlockError> {
        Ok(())
    }
}

static DISKS: LeafSpinLock<[Option<Box<dyn BlockDevice>>; MAX_DISKS]> =
    LeafSpinLock::new_leaf([const { None }; MAX_DISKS]);

/// Installs `disk` for a file system to take; gives it back when [`MAX_DISKS`] are installed.
pub fn install(disk: Box<dyn BlockDevice>) -> Result<(), Box<dyn BlockDevice>> {
    let mut disks = DISKS.lock();
    match disks.iter_mut().find(|slot| slot.is_none()) {
        Some(slot) => {
            *slot = Some(disk);
            Ok(())
        }
        None => Err(disk),
    }
}

/// Whether another disk can be installed.
pub fn has_room() -> bool {
    DISKS.lock().iter().any(Option::is_none)
}

/// Takes every disk installed, in the order they were installed, for file systems to own;
/// those they do not take go back with [`install`].
pub fn take_all() -> [Option<Box<dyn BlockDevice>>; MAX_DISKS] {
    core::mem::take(&mut *DISKS.lock())
}
