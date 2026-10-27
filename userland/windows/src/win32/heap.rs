//! The process heap (`HeapAlloc` and its family): blocks of 32 bytes to 64 KiB in power-of-two
//! size classes, each class a free list, carved from 1 MiB chunks; larger blocks are mappings
//! of their own. Every block starts with a 16-byte header holding its class and the size asked
//! for, so blocks stay 16-byte aligned as Windows' are, and a pointer the heap did not hand out
//! is refused instead of corrupting it. No system call on the common path.

use super::{Bool, FALSE, SpinLock, TRUE, error, fail};
use crate::library::Export;
use crate::linux::{self, PROT_READ, PROT_WRITE};
use core::ptr;

const HEADER_SIZE: usize = 16;
const SMALLEST_BLOCK_SHIFT: u32 = 5;
const CLASS_COUNT: usize = 12;
const LARGEST_BLOCK: usize = 1 << (SMALLEST_BLOCK_SHIFT as usize + CLASS_COUNT - 1);
const CHUNK_SIZE: usize = 1 << 20;
const PAGE_SIZE: usize = 4096;
const MAGIC: u32 = 0xe660_5eed;
const LARGE: u32 = u32::MAX;
const HEAP_ZERO_MEMORY: u32 = 0x8;
const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;

const _: () = assert!(LARGEST_BLOCK == 64 << 10);

struct State {
    free: [*mut u8; CLASS_COUNT],
    next: usize,
    end: usize,
}

struct FreeLists(State);

// SAFETY: the free lists hold addresses of heap blocks, which any thread may use.
unsafe impl Send for FreeLists {}

static HEAP: SpinLock<FreeLists> = SpinLock::new(FreeLists(State {
    free: [ptr::null_mut(); CLASS_COUNT],
    next: 0,
    end: 0,
}));

/// The handle `GetProcessHeap` returns.
pub fn process_heap() -> usize {
    &raw const HEAP as usize
}

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

/// Allocates `size` bytes, zeroed; null if there is no memory.
pub fn allocate(size: usize) -> *mut u8 {
    allocate_with(size, true)
}

fn allocate_with(size: usize, zeroed: bool) -> *mut u8 {
    let block = match class_of(size) {
        Some(class) => {
            let (block, recycled) = HEAP.with(|lists| take(&mut lists.0, class));
            if block.is_null() {
                return block;
            }
            write_header(block, class as u32, size);
            if zeroed && recycled {
                // SAFETY: the block holds `block_size(class)` bytes, the header's 16 first.
                unsafe { block.wrapping_add(HEADER_SIZE).write_bytes(0, size) };
            }
            block
        }
        None => {
            let Some(length) = large_length(size) else {
                return ptr::null_mut();
            };
            let Ok(address) = linux::map_anonymous(0, length, PROT_READ | PROT_WRITE, false) else {
                return ptr::null_mut();
            };
            let block = address as *mut u8;
            write_header(block, LARGE, size);
            block
        }
    };
    block.wrapping_add(HEADER_SIZE)
}

fn large_length(size: usize) -> Option<usize> {
    size.checked_add(HEADER_SIZE + PAGE_SIZE - 1)
        .map(|length| length & !(PAGE_SIZE - 1))
}

fn take(state: &mut State, class: usize) -> (*mut u8, bool) {
    let head = state.free[class];
    if !head.is_null() {
        // SAFETY: a free block's first word, inside its header, links to the next free block.
        state.free[class] = unsafe { head.cast::<*mut u8>().read() };
        return (head, true);
    }
    let size = block_size(class);
    if state.end - state.next < size {
        let Ok(chunk) = linux::map_anonymous(0, CHUNK_SIZE, PROT_READ | PROT_WRITE, false) else {
            return (ptr::null_mut(), false);
        };
        state.next = chunk;
        state.end = chunk + CHUNK_SIZE;
    }
    let block = state.next as *mut u8;
    state.next += size;
    (block, false)
}

fn write_header(block: *mut u8, class: u32, size: usize) {
    // SAFETY: the block is the caller's, 16-byte aligned and at least 16 bytes long.
    unsafe {
        block.cast::<[u32; 2]>().write([MAGIC, class]);
    }
    // SAFETY: as above.
    unsafe { block.wrapping_add(8).cast::<usize>().write(size) };
}

struct Block {
    start: *mut u8,
    class: u32,
    size: usize,
}

unsafe fn block_of(pointer: *mut u8) -> Option<Block> {
    if pointer.is_null() || !(pointer as usize).is_multiple_of(HEADER_SIZE) {
        return None;
    }
    let start = pointer.wrapping_sub(HEADER_SIZE);
    // SAFETY: the caller's duty: `pointer` came from this heap, so its header is the 16 readable
    // bytes before it; the magic then catches a block freed already.
    let [magic, class] = unsafe { start.cast::<[u32; 2]>().read() };
    // SAFETY: as above.
    let size = unsafe { start.wrapping_add(8).cast::<usize>().read() };
    (magic == MAGIC && (class == LARGE || (class as usize) < CLASS_COUNT)).then_some(Block {
        start,
        class,
        size,
    })
}

fn release(block: Block) {
    if block.class == LARGE {
        if let Some(length) = large_length(block.size) {
            // SAFETY: a large block is a mapping of its own, which the program gave back.
            let _ = unsafe { linux::unmap(block.start as usize, length) };
        }
        return;
    }
    HEAP.with(|FreeLists(state)| {
        let class = block.class as usize;
        // SAFETY: the block is the heap's and free now: its first word, over the magic, links
        // it into its class's list, so freeing it again is refused.
        unsafe { block.start.cast::<*mut u8>().write(state.free[class]) };
        state.free[class] = block.start;
    });
}

extern "win64" fn get_process_heap() -> usize {
    process_heap()
}

extern "win64" fn heap_alloc(_heap: usize, flags: u32, size: usize) -> *mut u8 {
    allocate_with(size, flags & HEAP_ZERO_MEMORY != 0)
}

unsafe extern "win64" fn heap_free(_heap: usize, _flags: u32, pointer: *mut u8) -> Bool {
    if pointer.is_null() {
        return TRUE;
    }
    // SAFETY: `HeapFree`'s contract: a block `HeapAlloc` or `HeapReAlloc` returned.
    match unsafe { block_of(pointer) } {
        Some(block) => {
            release(block);
            TRUE
        }
        None => fail(error::INVALID_PARAMETER, FALSE),
    }
}

unsafe extern "win64" fn heap_size(_heap: usize, _flags: u32, pointer: *mut u8) -> usize {
    // SAFETY: `HeapSize`'s contract: a block `HeapAlloc` or `HeapReAlloc` returned.
    match unsafe { block_of(pointer) } {
        Some(block) => block.size,
        None => usize::MAX,
    }
}

unsafe extern "win64" fn heap_realloc(
    _heap: usize,
    flags: u32,
    pointer: *mut u8,
    size: usize,
) -> *mut u8 {
    // SAFETY: `HeapReAlloc`'s contract: a block `HeapAlloc` or `HeapReAlloc` returned.
    let Some(block) = (unsafe { block_of(pointer) }) else {
        return fail(error::INVALID_PARAMETER, ptr::null_mut());
    };
    let zero = flags & HEAP_ZERO_MEMORY != 0;
    let fits = match block.class {
        LARGE => large_length(size) == large_length(block.size),
        class => class_of(size) == Some(class as usize),
    };
    if fits {
        if zero && size > block.size {
            // SAFETY: the block holds `size` bytes after its header, as its class or length says.
            unsafe {
                pointer
                    .wrapping_add(block.size)
                    .write_bytes(0, size - block.size)
            };
        }
        // SAFETY: as above; the header's size word.
        unsafe { block.start.wrapping_add(8).cast::<usize>().write(size) };
        return pointer;
    }
    if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 {
        return fail(error::NOT_ENOUGH_MEMORY, ptr::null_mut());
    }
    let moved = allocate_with(size, zero);
    if moved.is_null() {
        return fail(error::NOT_ENOUGH_MEMORY, moved);
    }
    // SAFETY: both blocks hold at least the smaller size, and they are different blocks.
    unsafe { ptr::copy_nonoverlapping(pointer, moved, block.size.min(size)) };
    release(block);
    moved
}

extern "win64" fn heap_create(_options: u32, _initial: usize, _maximum: usize) -> usize {
    process_heap()
}

extern "win64" fn heap_destroy(_heap: usize) -> Bool {
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetProcessHeap" => get_process_heap,
    b"HeapAlloc" => heap_alloc,
    b"HeapFree" => heap_free,
    b"HeapSize" => heap_size,
    b"HeapReAlloc" => heap_realloc,
    b"HeapCreate" => heap_create,
    b"HeapDestroy" => heap_destroy,
];
