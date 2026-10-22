//! Building the boot memory map, shared by the architecture modules: the kernel image's extent
//! from the linker script, and the memory a device tree describes.

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use crate::fdt::{self, Fdt};
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use crate::memory::MemoryMap;
use crate::memory::PhysicalRange;

/// Largest device tree read, against a corrupt size field (QEMU's are 1 MiB at most).
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
const MAX_DEVICE_TREE_SIZE: usize = 16 << 20;

unsafe extern "C" {
    /// First byte of the kernel image (linker script).
    static __kernel_start: u8;
    /// End of the kernel image, its `.bss` and boot stack included (linker script).
    static __kernel_end: u8;
}

/// The physical memory the kernel image occupies, `.bss` and boot stack included. The kernel
/// runs at its physical load address on every architecture today (MMU off, or the x86_64 boot
/// identity map).
pub(crate) fn kernel_image() -> PhysicalRange {
    PhysicalRange {
        start: (&raw const __kernel_start) as u64,
        end: (&raw const __kernel_end) as u64,
    }
}

/// The free memory the device tree at `address` describes: its RAM minus its reservations,
/// the tree itself (the kernel reads it again for device discovery, M5) and the kernel image.
/// Empty if there is no valid tree at `address`.
///
/// # Safety
/// `address` must be 0, or readable for [`fdt::HEADER_LENGTH`] bytes and, if those hold an FDT
/// header, for as many bytes as it declares (up to [`MAX_DEVICE_TREE_SIZE`]): the firmware's
/// device tree, which nothing writes while the kernel runs.
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(crate) unsafe fn device_tree_memory_map(address: usize) -> MemoryMap {
    let mut map = MemoryMap::new();
    if address == 0 || !address.is_multiple_of(8) {
        return map;
    }
    // SAFETY: the caller guarantees `address` is readable for a header's length.
    let header = unsafe { core::slice::from_raw_parts(address as *const u8, fdt::HEADER_LENGTH) };
    let Ok(size) = fdt::total_size(header) else {
        return map;
    };
    if size > MAX_DEVICE_TREE_SIZE {
        return map;
    }
    // SAFETY: the header is valid and declares `size` bytes (bounded above), which the caller
    // guarantees are readable and not written while the kernel runs.
    let blob = unsafe { core::slice::from_raw_parts(address as *const u8, size) };
    let Ok(tree) = Fdt::new(blob) else {
        return map;
    };
    // All RAM first, then every reservation (a reservation only removes RAM already added).
    if tree
        .memory(&mut |ram| map.add_ram(ram), &mut |_| {})
        .is_err()
    {
        return MemoryMap::new();
    }
    if tree
        .memory(&mut |_| {}, &mut |reserved| map.reserve(reserved))
        .is_err()
    {
        return MemoryMap::new();
    }
    for reserved in tree.reservations() {
        map.reserve(reserved);
    }
    map.reserve(PhysicalRange::from_start_length(
        address as u64,
        size as u64,
    ));
    map.reserve(kernel_image());
    map
}
