//! The UEFI loader's handoff (D236): the address the architecture entry code stores before any
//! Rust code runs, and what the kernel reads from it through the boot direct map. A machine
//! started another way (QEMU's PVH or device-tree loaders) has none.

use crate::Platform;
use crate::memory::PhysicalRange;
use core::sync::atomic::{AtomicU64, Ordering};
pub use eggos_boot_handoff::{
    Framebuffer, HANDOFF_BYTES, Handoff, MEMORY_RANGE_BYTES, MemoryRange, memory_kind,
    pixel_format,
};

/// Physical address of the handoff, 0 for none; written by the architecture's UEFI entry code.
pub(crate) static HANDOFF_ADDRESS: AtomicU64 = AtomicU64::new(0);

/// The handoff the loader passed, if the kernel was started by EggOS's UEFI loader.
pub fn handoff<P: Platform>() -> Option<Handoff> {
    let address = HANDOFF_ADDRESS.load(Ordering::Relaxed);
    if address == 0 || address.saturating_add(HANDOFF_BYTES as u64) > P::boot_direct_map_end() {
        return None;
    }
    // SAFETY: the loader's handoff lies in its boot data, which the boot direct map covers
    // (checked above), which the kernel's direct map covers too (it is part of the memory map
    // the handoff describes), and which nothing writes after the loader.
    let bytes = unsafe { physical_bytes::<P>(address, HANDOFF_BYTES) };
    Handoff::read(bytes)
}

/// # Safety
/// `address..address + length` must be memory the direct map reaches and nothing writes.
unsafe fn physical_bytes<P: Platform>(address: u64, length: usize) -> &'static [u8] {
    let virt = (address as usize).wrapping_add(P::physical_memory_offset());
    // SAFETY: guaranteed by the caller.
    unsafe { core::slice::from_raw_parts(virt as *const u8, length) }
}

/// The memory map the handoff carries.
pub fn memory_ranges<P: Platform>(handoff: &Handoff) -> impl Iterator<Item = MemoryRange> {
    let entries = handoff.memory_map_entries as usize;
    let length = entries * MEMORY_RANGE_BYTES;
    let reachable = handoff.memory_map != 0
        && handoff.memory_map.saturating_add(length as u64) <= handoff.boot_data_end
        && handoff.memory_map >= handoff.boot_data_start;
    let bytes: &'static [u8] = if reachable {
        // SAFETY: the map lies inside the loader's boot data (checked), which the direct map
        // reaches and nothing writes.
        unsafe { physical_bytes::<P>(handoff.memory_map, length) }
    } else {
        &[]
    };
    eggos_boot_handoff::memory_ranges(bytes, entries)
}

/// The command line the handoff carries, empty for none.
pub fn command_line<P: Platform>(handoff: &Handoff) -> &'static [u8] {
    let length = handoff.command_line_length as usize;
    let inside = handoff.command_line >= handoff.boot_data_start
        && handoff.command_line.saturating_add(length as u64) <= handoff.boot_data_end;
    if handoff.command_line == 0 || length == 0 || !inside {
        return &[];
    }
    // SAFETY: inside the loader's boot data (checked), which the direct map reaches and nothing
    // writes.
    unsafe { physical_bytes::<P>(handoff.command_line, length) }
}

/// The loader's boot data, which the kernel keeps out of its allocator.
pub fn boot_data(handoff: &Handoff) -> PhysicalRange {
    PhysicalRange {
        start: handoff.boot_data_start,
        end: handoff.boot_data_end.max(handoff.boot_data_start),
    }
}

/// A file the UEFI loader read from the boot partition and kept for the installer (D241).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootFile {
    /// `\EGGOS\KERNEL.ELF`.
    Kernel,
    /// `\EFI\BOOT\BOOTX64.EFI` or `BOOTAA64.EFI`.
    Loader,
}

/// The largest boot file read, against a corrupt length.
const MAX_BOOT_FILE_BYTES: u64 = 256 << 20;

/// The bytes of `file`, if the loader kept it; read through the direct map, which covers the
/// loader's boot data once the kernel's page tables are active.
pub fn file<P: Platform>(handoff: &Handoff, file: BootFile) -> Option<&'static [u8]> {
    let (address, length) = match file {
        BootFile::Kernel => (handoff.kernel_file, handoff.kernel_file_bytes),
        BootFile::Loader => (handoff.loader_file, handoff.loader_file_bytes),
    };
    if address == 0 || length == 0 || length > MAX_BOOT_FILE_BYTES {
        return None;
    }
    let inside = memory_ranges::<P>(handoff).any(|range| {
        range.kind == memory_kind::BOOT_DATA && range.start <= address && address + length <= range.end()
    });
    // SAFETY: the file lies inside a range of the loader's boot data (checked), which the
    // kernel's direct map covers and nothing writes.
    inside.then(|| unsafe { physical_bytes::<P>(address, length as usize) })
}
