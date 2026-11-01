//! `malloc` and its family: blocks of up to 64 KiB in power-of-two size classes, each class a
//! free list, carved from 1 MiB chunks; larger blocks are mappings of their own. Every block
//! starts with a 16-byte header holding its class and the size asked for, so blocks are
//! 16-byte aligned as Darwin's are, `malloc_size` is exact, and a pointer the heap did not hand
//! out is refused instead of corrupting it. No system call on the common path.

use super::{Exclusive, set_errno};
use crate::darwin::ENOMEM;
use crate::linux::{self, PROT_READ, PROT_WRITE};
use core::ptr;

const HEADER_SIZE: usize = 16;
const SMALLEST_BLOCK_SHIFT: u32 = 5;
const CLASS_COUNT: usize = 12;
const LARGEST_BLOCK: usize = 1 << (SMALLEST_BLOCK_SHIFT as usize + CLASS_COUNT - 1);
const CHUNK_SIZE: usize = 1 << 20;
const MAGIC: u32 = 0xe660_5eed;
const LARGE: u32 = u32::MAX;
const ALIGNED: u32 = u32::MAX - 1;

struct State {
    free: [usize; CLASS_COUNT],
    next: usize,
    end: usize,
}

static HEAP: Exclusive<State> = Exclusive::new(State {
    free: [0; CLASS_COUNT],
    next: 0,
    end: 0,
});

fn class_of(size: usize) -> Option<usize> {
    let block = size
        .checked_add(HEADER_SIZE)?
        .max(1 << SMALLEST_BLOCK_SHIFT);
    (block <= LARGEST_BLOCK)
        .then(|| (block.next_power_of_two().trailing_zeros() - SMALLEST_BLOCK_SHIFT) as usize)
}

fn block_size(class: usize) -> usize {
    1 << (SMALLEST_BLOCK_SHIFT as usize + class)
}

fn take(state: &mut State, class: usize) -> Option<(usize, bool)> {
    let head = state.free[class];
    if head != 0 {
        // SAFETY: a free block's first word holds the next free block of its class.
        state.free[class] = unsafe { (head as *const usize).read() };
        return Some((head, true));
    }
    let size = block_size(class);
    if state.end - state.next < size {
        let chunk = linux::map_anonymous(0, CHUNK_SIZE, PROT_READ | PROT_WRITE, false).ok()?;
        state.next = chunk;
        state.end = chunk + CHUNK_SIZE;
    }
    let block = state.next;
    state.next += size;
    Some((block, false))
}

fn write_header(block: usize, class: u32, size: usize) -> *mut u8 {
    let header = block as *mut u32;
    // SAFETY: every block starts with its 16-byte header, which the heap owns.
    unsafe {
        header.write(MAGIC);
    }
    // SAFETY: as above.
    unsafe { header.wrapping_add(1).write(class) };
    // SAFETY: as above.
    unsafe { header.wrapping_add(2).cast::<u64>().write(size as u64) };
    (block + HEADER_SIZE) as *mut u8
}

fn header_of(pointer: *mut u8) -> Option<(usize, u32, usize)> {
    let block = (pointer as usize).checked_sub(HEADER_SIZE)?;
    if !(pointer as usize).is_multiple_of(16) {
        return None;
    }
    let header = block as *const u32;
    // SAFETY: a pointer the heap handed out has its header just before it; the magic word
    // refuses most others.
    if unsafe { header.read() } != MAGIC {
        return None;
    }
    // SAFETY: as above.
    let class = unsafe { header.wrapping_add(1).read() };
    // SAFETY: as above.
    let size = unsafe { header.wrapping_add(2).cast::<u64>().read() } as usize;
    Some((block, class, size))
}

fn large_length(size: usize) -> Option<usize> {
    size.checked_add(HEADER_SIZE)?
        .checked_next_multiple_of(linux::PAGE_SIZE)
}

/// Allocates `size` bytes; null (with `ENOMEM`) if there is no memory.
pub fn allocate(size: usize, zeroed: bool) -> *mut u8 {
    let pointer = match class_of(size) {
        Some(class) => match HEAP.with(|state| take(state, class)) {
            Some((block, recycled)) => {
                let pointer = write_header(block, class as u32, size);
                if zeroed && recycled {
                    // SAFETY: the block holds at least `size` bytes after its header.
                    unsafe { pointer.write_bytes(0, size) };
                }
                pointer
            }
            None => ptr::null_mut(),
        },
        None => match large_length(size)
            .and_then(|length| linux::map_anonymous(0, length, PROT_READ | PROT_WRITE, false).ok())
        {
            Some(block) => write_header(block, LARGE, size),
            None => ptr::null_mut(),
        },
    };
    if pointer.is_null() {
        set_errno(ENOMEM);
    }
    pointer
}

/// The size asked for when `pointer` was allocated; 0 for null or a pointer the heap did not
/// hand out.
pub fn size_of(pointer: *mut u8) -> usize {
    match header_of(pointer) {
        Some((_, ALIGNED, distance)) => {
            let inner = (pointer as usize - distance) as *mut u8;
            header_of(inner).map_or(0, |(_, _, size)| size - distance)
        }
        Some((_, _, size)) => size,
        None => 0,
    }
}

/// Frees `pointer`; null, and pointers the heap did not hand out, are ignored.
pub fn release(pointer: *mut u8) {
    let Some((block, class, size)) = header_of(pointer) else {
        return;
    };
    match class {
        ALIGNED => release((pointer as usize - size) as *mut u8),
        LARGE => {
            if let Some(length) = large_length(size) {
                // SAFETY: a large block is a mapping of its own, which nothing uses once freed.
                let _ = unsafe { linux::unmap(block, length) };
            }
        }
        class if (class as usize) < CLASS_COUNT => {
            // SAFETY: the header is the heap's; a zero magic refuses a second free.
            unsafe { (block as *mut u32).write(0) };
            HEAP.with(|state| {
                // SAFETY: the freed block's first word links it into its class's free list.
                unsafe { (block as *mut usize).write(state.free[class as usize]) };
                state.free[class as usize] = block;
            });
        }
        _ => {}
    }
}

/// `realloc`: a new block of `size` bytes with the old one's contents.
pub fn reallocate(pointer: *mut u8, size: usize) -> *mut u8 {
    if pointer.is_null() {
        return allocate(size, false);
    }
    let old = size_of(pointer);
    if let Some((block, class, _)) = header_of(pointer)
        && (class as usize) < CLASS_COUNT
        && class_of(size) == Some(class as usize)
    {
        write_header(block, class, size);
        return pointer;
    }
    let new = allocate(size, false);
    if !new.is_null() {
        // SAFETY: both blocks hold at least the smaller size.
        unsafe { new.copy_from_nonoverlapping(pointer, old.min(size)) };
        release(pointer);
    }
    new
}

/// `posix_memalign` and `aligned_alloc`: `size` bytes at a multiple of `alignment`, a power of
/// two.
pub fn allocate_aligned(alignment: usize, size: usize) -> *mut u8 {
    if alignment <= 16 {
        return allocate(size, false);
    }
    let Some(total) = size
        .checked_add(alignment)
        .and_then(|total| total.checked_add(HEADER_SIZE))
    else {
        set_errno(ENOMEM);
        return ptr::null_mut();
    };
    let inner = allocate(total, false);
    if inner.is_null() {
        return inner;
    }
    let aligned = (inner as usize + HEADER_SIZE).next_multiple_of(alignment);
    write_header(aligned - HEADER_SIZE, ALIGNED, aligned - inner as usize)
}

#[cfg(target_arch = "aarch64")]
mod layer_allocator {
    use core::alloc::{GlobalAlloc, Layout};

    const BLOCK_ALIGNMENT: usize = 16;

    struct ProcessHeap;

    // SAFETY: every block comes from the process heap, aligned to at least the layout's
    // alignment, and is handed out once until it is given back.
    unsafe impl GlobalAlloc for ProcessHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            match layout.align() <= BLOCK_ALIGNMENT {
                true => super::allocate(layout.size(), false),
                false => super::allocate_aligned(layout.align(), layout.size()),
            }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= BLOCK_ALIGNMENT {
                return super::allocate(layout.size(), true);
            }
            let pointer = super::allocate_aligned(layout.align(), layout.size());
            if !pointer.is_null() {
                // SAFETY: the block holds at least `layout.size()` bytes.
                unsafe { pointer.write_bytes(0, layout.size()) };
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            super::release(pointer)
        }
    }

    #[global_allocator]
    static LAYER_HEAP: ProcessHeap = ProcessHeap;
}
