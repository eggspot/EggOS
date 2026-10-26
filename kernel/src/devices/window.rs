//! Device windows (D110): device registers mapped as device memory at addresses of their own in
//! the kernel's device window ([`crate::paging::device_window`]), handed out from its start and
//! never reused. A window aligned to a large leaf's size where its physical address allows, so
//! a large one (PCI Express configuration space is 256 MiB) costs a few entries.

use crate::memory::{FRAMES, Memory};
use crate::paging::{self, MapError};
use crate::sync::LeafSpinLock;
use eggos_hal::paging::{Format, leaf_size};
use eggos_hal::{PhysicalRange, Platform};

static NEXT: LeafSpinLock<u64> = LeafSpinLock::new_leaf(0);

/// Maps the registers at `region` and returns the virtual address of `region.start`. Must run on
/// the boot CPU before any other starts ([`eggos_hal::Platform::make_kernel_mappings_visible`]).
pub fn map<P: Platform>(memory: &mut Memory<P>, region: PhysicalRange) -> Result<usize, MapError> {
    if region.is_empty() {
        return Err(MapError::InvalidAddress);
    }
    let frames = region.covering_frames();
    let window = paging::device_window::<P>();
    let largest = <P::PageTableFormat as Format>::MAX_LEAF_LEVEL
        .min(<P::PageTableFormat as Format>::LEVELS - 2);
    let alignment = (0..=largest)
        .rev()
        .map(leaf_size)
        .find(|&size| frames.start.is_multiple_of(size) && frames.len() >= size)
        .unwrap_or(leaf_size(0));
    let mut next = NEXT.lock();
    if *next == 0 {
        *next = window.start;
    }
    let virt = next
        .checked_next_multiple_of(alignment)
        .ok_or(MapError::OutsideDeviceWindow)?;
    let mut allocator = FRAMES.lock();
    let allocator = allocator.as_mut().ok_or(MapError::OutOfMemory)?;
    *next = virt.saturating_add(frames.len());
    memory
        .page_tables
        .map_device_window(&window, virt, frames.start, frames.len(), allocator)?;
    P::make_kernel_mappings_visible(virt as usize, frames.len() as usize);
    Ok((virt + (region.start - frames.start)) as usize)
}
