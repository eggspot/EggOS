//! `malloc` and its family, on the process heap (`HeapAlloc`'s), so a block `malloc` returns is
//! 16-byte aligned and `_msize` knows its size; a failed allocation sets `errno` to `ENOMEM`.

use super::super::heap as process_heap;
use super::runtime::{errno, set_errno};
use crate::library::Export;
use core::ptr;

fn out_of_memory(block: *mut u8) -> *mut u8 {
    if block.is_null() {
        set_errno(errno::ENOMEM);
    }
    block
}

extern "efiapi" fn malloc(size: usize) -> *mut u8 {
    out_of_memory(process_heap::allocate_with(size, false))
}

extern "efiapi" fn calloc(count: usize, size: usize) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => out_of_memory(process_heap::allocate(total)),
        None => out_of_memory(ptr::null_mut()),
    }
}

unsafe extern "efiapi" fn free(block: *mut u8) {
    // SAFETY: `free`'s contract: null or a block this heap handed out.
    unsafe { process_heap::free(block) };
}

unsafe extern "efiapi" fn realloc(block: *mut u8, size: usize) -> *mut u8 {
    if block.is_null() {
        return malloc(size);
    }
    if size == 0 {
        // SAFETY: `realloc`'s contract: a block this heap handed out.
        unsafe { process_heap::free(block) };
        return ptr::null_mut();
    }
    // SAFETY: as above.
    out_of_memory(unsafe { process_heap::heap_realloc(0, 0, block, size) })
}

unsafe extern "efiapi" fn recalloc(block: *mut u8, count: usize, size: usize) -> *mut u8 {
    const HEAP_ZERO_MEMORY: u32 = 0x8;
    let Some(total) = count.checked_mul(size) else {
        return out_of_memory(ptr::null_mut());
    };
    if block.is_null() {
        return out_of_memory(process_heap::allocate(total));
    }
    // SAFETY: `_recalloc`'s contract: a block this heap handed out.
    out_of_memory(unsafe { process_heap::heap_realloc(0, HEAP_ZERO_MEMORY, block, total) })
}

unsafe extern "efiapi" fn block_size(block: *mut u8) -> usize {
    if block.is_null() {
        set_errno(errno::EINVAL);
        return usize::MAX;
    }
    // SAFETY: `_msize`'s contract: a block this heap handed out.
    unsafe { process_heap::heap_size(0, 0, block) }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"malloc" => malloc,
    b"calloc" => calloc,
    b"free" => free,
    b"realloc" => realloc,
    b"_recalloc" => recalloc,
    b"_msize" => block_size,
];
