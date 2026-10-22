//! Building the boot memory map, shared by the architecture modules: the kernel image's extent
//! from the linker script and the memory a device tree describes.

use crate::arch::KERNEL_VIRTUAL_OFFSET;
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use crate::fdt::{self, Fdt};
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use crate::memory::MemoryMap;
use crate::memory::PhysicalRange;
use crate::paging::KernelImage;
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use crate::{Platform, arch::CurrentPlatform};

/// Largest device tree read, against a corrupt size field (QEMU's are 1 MiB at most).
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(crate) const MAX_DEVICE_TREE_SIZE: usize = 16 << 20;

unsafe extern "C" {
    /// First byte of the kernel image (linker script).
    static __kernel_start: u8;
    /// Where the code, the constants and the data start (linker script).
    static __text_start: u8;
    static __rodata_start: u8;
    static __data_start: u8;
    /// The page below the boot stack, and the stack's lowest byte (linker script).
    static __stack_guard: u8;
    static __stack_bottom: u8;
    /// End of the kernel image, its `.bss` and boot stack included (linker script).
    static __kernel_end: u8;
}

// The HAL's kernel virtual offset, for `linker.ld` to check its own against.
core::arch::global_asm!(
    ".globl __kernel_virtual_offset_hal",
    ".set __kernel_virtual_offset_hal, {offset}",
    offset = const KERNEL_VIRTUAL_OFFSET,
);

/// The physical address of the kernel image's byte at virtual address `virt`.
fn physical(virt: *const u8) -> u64 {
    (virt as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET)
}

/// The physical memory the kernel image occupies, `.bss` and boot stack included. The kernel
/// is linked, and runs, [`KERNEL_VIRTUAL_OFFSET`] above it (D33).
pub(crate) fn kernel_image() -> PhysicalRange {
    PhysicalRange {
        start: physical(&raw const __kernel_start),
        end: physical(&raw const __kernel_end),
    }
}

/// The parts of the kernel image, for its page-table permissions.
pub(crate) fn kernel_image_parts() -> KernelImage {
    KernelImage {
        virtual_offset: KERNEL_VIRTUAL_OFFSET,
        start: physical(&raw const __kernel_start),
        text_start: physical(&raw const __text_start),
        rodata_start: physical(&raw const __rodata_start),
        data_start: physical(&raw const __data_start),
        stack_guard_start: physical(&raw const __stack_guard),
        stack_start: physical(&raw const __stack_bottom),
        end: physical(&raw const __kernel_end),
    }
}

/// The device tree at physical `address`, read through the physical memory offset; `None` if
/// there is no valid tree there.
///
/// # Safety
/// `address` must be 0, or readable (at `address` + the physical memory offset) for
/// [`fdt::HEADER_LENGTH`] bytes and, if those hold an FDT header, for as many bytes as it
/// declares (up to [`MAX_DEVICE_TREE_SIZE`]): the firmware's device tree, which nothing writes
/// while the kernel runs.
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
unsafe fn device_tree(address: usize) -> Option<(Fdt<'static>, usize)> {
    if address == 0 || !address.is_multiple_of(8) {
        return None;
    }
    let virt = address.wrapping_add(CurrentPlatform::physical_memory_offset());
    // SAFETY: the caller guarantees `address` is readable for a header's length.
    let header = unsafe { core::slice::from_raw_parts(virt as *const u8, fdt::HEADER_LENGTH) };
    let size = fdt::total_size(header).ok()?;
    if size > MAX_DEVICE_TREE_SIZE {
        return None;
    }
    // SAFETY: the header is valid and declares `size` bytes (bounded above), which the caller
    // guarantees are readable and not written while the kernel runs.
    let blob = unsafe { core::slice::from_raw_parts(virt as *const u8, size) };
    Some((Fdt::new(blob).ok()?, size))
}

/// The free memory the device tree at `address` describes: its RAM minus its reservations,
/// the tree itself (the kernel reads it again for device discovery, M5) and the kernel image.
/// Empty if there is no valid tree at `address`.
///
/// # Safety
/// As for [`device_tree`].
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(crate) unsafe fn device_tree_memory_map(address: usize) -> MemoryMap {
    let mut map = MemoryMap::new();
    // SAFETY: guaranteed by the caller.
    let Some((tree, size)) = (unsafe { device_tree(address) }) else {
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

/// All the RAM the device tree at `address` describes, reserved parts included: what the direct
/// map covers. Empty if there is no valid tree at `address`.
///
/// # Safety
/// As for [`device_tree`].
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub(crate) unsafe fn device_tree_ram(address: usize) -> MemoryMap {
    let mut map = MemoryMap::new();
    // SAFETY: guaranteed by the caller.
    let Some((tree, _)) = (unsafe { device_tree(address) }) else {
        return map;
    };
    if tree
        .memory(&mut |ram| map.add_ram(ram), &mut |_| {})
        .is_err()
    {
        return MemoryMap::new();
    }
    map
}
