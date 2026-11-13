//! A memory allocator for EggOS's own programs: each allocation a private anonymous mapping of
//! whole pages. A program makes it its allocator with `#[global_allocator]`.

use crate::errno::Errno;
use crate::syscall::{self, number};
use core::alloc::{GlobalAlloc, Layout};

const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x02 | 0x20;
const NO_DESCRIPTOR: usize = usize::MAX;
const PAGE: usize = 4096;

/// The program's memory: each allocation a private anonymous mapping of whole pages, given
/// back when freed. For programs that allocate a few large blocks, rarely (a terminal's engine
/// and its window's picture), where a mapping each costs less than any heap would.
#[derive(Debug)]
pub struct Pages;

fn whole_pages(size: usize) -> usize {
    size.div_ceil(PAGE) * PAGE
}

// SAFETY: `alloc` returns null or a new mapping of at least `layout.size()` bytes, aligned to a
// page, which meets every alignment up to a page (larger ones get null), and no other block
// overlaps it; `dealloc` unmaps exactly the pages `alloc` mapped for the same layout.
unsafe impl GlobalAlloc for Pages {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > PAGE {
            return core::ptr::null_mut();
        }
        let length = whole_pages(layout.size().max(1));
        // SAFETY: a new private anonymous mapping changes no memory the program uses.
        let at = unsafe {
            syscall::call(
                number::MMAP,
                [
                    0,
                    length,
                    PROT_READ_WRITE,
                    MAP_PRIVATE_ANONYMOUS,
                    NO_DESCRIPTOR,
                    0,
                ],
            )
        };
        match Errno::result(at) {
            Ok(at) => at as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let length = whole_pages(layout.size().max(1));
        // SAFETY: `pointer` and `length` are the mapping `alloc` made for this layout, which
        // the caller gives back and no longer uses.
        unsafe { syscall::call(number::MUNMAP, [pointer as usize, length, 0, 0, 0, 0]) };
    }
}
