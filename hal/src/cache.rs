//! Making the CPU's writes visible to devices that read memory without looking in its caches.

#[cfg(target_arch = "x86_64")]
const CACHE_LINE: usize = 64;

/// Writes the cache lines holding the `length` bytes at `start` back to memory, for a device
/// that reads memory without snooping the CPU's caches (Intel's display engine, D268). Does
/// nothing on architectures whose devices see the caches. A
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
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (start, length);
}
