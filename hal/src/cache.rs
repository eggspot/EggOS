//! Making the CPU's writes visible to devices that read memory without looking in its caches,
//! and the devices' writes visible to the CPU (non-coherent DMA, D268, D563).

#[cfg(target_arch = "x86_64")]
const CACHE_LINE: usize = 64;

/// Writes the cache lines holding the `length` bytes at `start` back to memory, for a device
/// that reads memory without snooping the CPU's caches (Intel's display engine, D268; a DMA
/// device without `dma-coherent`, D563): `clflush` on x86_64, `dc cvac` on AArch64, Zicbom's
/// `cbo.clean` on RISC-V. Does nothing on architectures whose devices see the caches. A
/// `core::sync::atomic::fence(Ordering::SeqCst)` after the last call orders the write-backs
/// before the CPU's later stores, such as the register write that hands the memory over.
///
/// # Safety
/// The `length` bytes at `start` must be mapped.
pub unsafe fn write_back_for_device(start: *const u8, length: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        let end = start as usize + length;
        let mut line = start as usize & !(CACHE_LINE - 1);
        while line < end {
            // SAFETY: `line` is in a cache line of the range the caller says is mapped;
            // `clflush` writes it back and drops it from the caches, changing no memory.
            unsafe { core::arch::x86_64::_mm_clflush(line as *const u8) };
            line += CACHE_LINE;
        }
    }
    #[cfg(all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64")))]
    each_line(start, length, |line| {
        // SAFETY: `line` is in the range the caller says is mapped, and `each_line` only runs
        // where the CPU has the instruction.
        unsafe { crate::arch::clean_data_line(line) }
    });
    #[cfg(not(any(
        target_arch = "x86_64",
        all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64"))
    )))]
    let _ = (start, length);
}

/// Drops the cache lines holding the `length` bytes at `start` (writing back what the CPU
/// changed in them first), so the CPU's next reads see what a device that does not snoop its
/// caches wrote there by DMA (D563): `clflush` on x86_64, `dc civac` on AArch64, `cbo.flush`
/// on RISC-V. Does nothing on architectures whose devices see the caches.
///
/// # Safety
/// The `length` bytes at `start` must be mapped.
pub unsafe fn discard_for_cpu(start: *const u8, length: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: guaranteed by the caller; `clflush` both writes back and drops the line.
        unsafe { write_back_for_device(start, length) };
    }
    #[cfg(all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64")))]
    each_line(start, length, |line| {
        // SAFETY: as in `write_back_for_device`.
        unsafe { crate::arch::flush_data_line(line) }
    });
    #[cfg(not(any(
        target_arch = "x86_64",
        all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64"))
    )))]
    let _ = (start, length);
}

/// Whether this CPU can keep its caches in step with a device that does not snoop them
/// ([`write_back_for_device`] and [`discard_for_cpu`] do something): x86_64 and AArch64, and
/// RISC-V with Zicbom (D563).
pub fn can_maintain_for_devices() -> bool {
    #[cfg(target_arch = "x86_64")]
    return true;
    #[cfg(all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64")))]
    return crate::arch::data_cache_line() != 0;
    #[cfg(not(any(
        target_arch = "x86_64",
        all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64"))
    )))]
    false
}

#[cfg(all(eggos_kernel, any(target_arch = "aarch64", target_arch = "riscv64")))]
fn each_line(start: *const u8, length: usize, mut each: impl FnMut(usize)) {
    let line = crate::arch::data_cache_line();
    if line == 0 || length == 0 {
        return;
    }
    let end = start as usize + length;
    let mut address = start as usize & !(line - 1);
    while address < end {
        each(address);
        address += line;
    }
    crate::arch::cache_maintenance_barrier();
}
