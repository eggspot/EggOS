//! The `IStream` `CreateStreamOnHGlobal` makes: a stream over a memory block (an `HGLOBAL`,
//! which in the layer is the block itself), with a seek pointer of its own. Writing past the
//! end grows the stream; when the block is too small, the bytes move to a larger block the
//! stream owns (the old one is freed if the stream owned it). `Clone` gives a stream over the
//! same memory with its own seek pointer. The memory is not transacted: `Commit` and `Revert`
//! do nothing, and region locking is not supported (`STG_E_INVALIDFUNCTION`).

use super::guid::{self, Guid};
use super::{E_OUTOFMEMORY, E_POINTER, HResult, S_OK};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

const STG_E_INVALIDFUNCTION: HResult = 0x8003_0001_u32 as i32;
const STG_E_INVALIDPOINTER: HResult = 0x8003_0009_u32 as i32;
const STREAM_SEEK_SET: u32 = 0;
const STREAM_SEEK_CUR: u32 = 1;
const STREAM_SEEK_END: u32 = 2;
const STGTY_STREAM: u32 = 2;
const STGM_READWRITE: u32 = 2;
const SEQUENTIAL_WRITE_SLOT: usize = 4;
const COPY_CHUNK: usize = 4096;
const SMALLEST_GROWTH: usize = 256;

#[derive(Debug)]
struct Memory {
    references: AtomicU32,
    block: *mut u8,
    capacity: usize,
    size: usize,
    owns_block: bool,
}

/// A stream: its table, its reference count, its seek pointer and the memory it shares with
/// its clones.
#[repr(C)]
#[derive(Debug)]
pub struct Stream {
    table: usize,
    references: AtomicU32,
    position: u64,
    memory: *mut Memory,
}

/// `STATSTG`, what `IStream::Stat` reports.
#[repr(C)]
#[derive(Debug, Default)]
#[allow(dead_code, reason = "the program reads it")]
pub struct StatStg {
    pub name: usize,
    pub kind: u32,
    pub size: u64,
    pub modified: u64,
    pub created: u64,
    pub accessed: u64,
    pub mode: u32,
    pub locks_supported: u32,
    pub class: Guid,
    pub state_bits: u32,
    pub reserved: u32,
}

const _: () = assert!(core::mem::size_of::<StatStg>() == 80);

fn new_stream(table: usize, memory: *mut Memory, position: u64) -> *mut Stream {
    let object = super::allocate(core::mem::size_of::<Stream>()).cast::<Stream>();
    if !object.is_null() {
        let stream = Stream {
            table,
            references: AtomicU32::new(1),
            position,
            memory,
        };
        // SAFETY: a fresh, aligned block for one stream.
        unsafe { object.write(stream) };
    }
    object
}

/// A new stream with `table` over `size` bytes at `block` (an `HGLOBAL` the layer's heap made),
/// which it frees when the last stream goes if `owns_block`; null without memory.
pub fn create(table: usize, block: *mut u8, size: usize, owns_block: bool) -> *mut Stream {
    let memory = super::allocate(core::mem::size_of::<Memory>()).cast::<Memory>();
    if memory.is_null() {
        return ptr::null_mut();
    }
    let shared = Memory {
        references: AtomicU32::new(1),
        block,
        capacity: size,
        size,
        owns_block,
    };
    // SAFETY: a fresh, aligned block for the shared memory record.
    unsafe { memory.write(shared) };
    let stream = new_stream(table, memory, 0);
    if stream.is_null() {
        // SAFETY: the record is this function's and unused.
        unsafe { super::free(memory.cast()) };
    }
    stream
}

unsafe fn parts<'a>(this: *mut Stream) -> (&'a mut Stream, &'a mut Memory) {
    // SAFETY: the caller's duty.
    let stream = unsafe { &mut *this };
    // SAFETY: a live stream's memory record lives while any stream uses it.
    let memory = unsafe { &mut *stream.memory };
    (stream, memory)
}

impl Memory {
    fn bytes(&mut self) -> &mut [u8] {
        match self.block.is_null() || self.size == 0 {
            true => &mut [],
            // SAFETY: the block holds `capacity` >= `size` bytes.
            false => unsafe { core::slice::from_raw_parts_mut(self.block, self.size) },
        }
    }

    fn resize(&mut self, size: usize) -> Result<(), HResult> {
        if size > self.capacity {
            let capacity = size
                .max(self.capacity.saturating_mul(2))
                .max(SMALLEST_GROWTH);
            let block = super::allocate(capacity);
            if block.is_null() {
                return Err(E_OUTOFMEMORY);
            }
            if self.size != 0 {
                // SAFETY: both blocks hold at least `size` bytes and are different blocks.
                unsafe { ptr::copy_nonoverlapping(self.block, block, self.size) };
            }
            if self.owns_block {
                // SAFETY: the stream owned the old block, replaced now.
                unsafe { super::free(self.block) };
            }
            self.block = block;
            self.capacity = capacity;
            self.owns_block = true;
        } else if size > self.size {
            // SAFETY: the block holds `capacity` >= `size` bytes.
            unsafe {
                self.block
                    .wrapping_add(self.size)
                    .write_bytes(0, size - self.size)
            };
        }
        self.size = size;
        Ok(())
    }
}

/// `IUnknown::QueryInterface`: `IUnknown`, `ISequentialStream` and `IStream`.
///
/// # Safety
/// `this` must be a live stream; the others as `QueryInterface` says.
pub unsafe extern "efiapi" fn query_interface(
    this: *mut Stream,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    let interfaces = [guid::UNKNOWN, guid::SEQUENTIAL_STREAM, guid::STREAM];
    // SAFETY: the caller's duty.
    unsafe { super::answer_query(this as usize, references, interface, result, &interfaces) }
}

/// `IUnknown::AddRef`.
///
/// # Safety
/// `this` must be a live stream.
pub unsafe extern "efiapi" fn add_reference(this: *mut Stream) -> u32 {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    references.fetch_add(1, Ordering::Relaxed) + 1
}

/// `IUnknown::Release`: the last reference to the last stream over the memory frees it, and
/// the block when the stream owns it.
///
/// # Safety
/// `this` must be a live stream the caller holds a reference to.
pub unsafe extern "efiapi" fn release(this: *mut Stream) -> u32 {
    // SAFETY: the caller's duty.
    let (left, last) = super::drop_reference(unsafe { &(*this).references });
    if !last {
        return left;
    }
    // SAFETY: no reference is left: the stream is this function's.
    let (stream, memory) = unsafe { parts(this) };
    let record = stream.memory;
    if super::drop_reference(&memory.references).1 {
        if memory.owns_block {
            // SAFETY: the stream owned the block, which nothing uses now.
            unsafe { super::free(memory.block) };
        }
        // SAFETY: the record's last stream goes.
        unsafe { super::free(record.cast()) };
    }
    // SAFETY: as above.
    unsafe { super::free(this.cast()) };
    left
}

/// `ISequentialStream::Read`: up to `count` bytes from the seek pointer.
///
/// # Safety
/// `this` must be a live stream; `buffer` room for `count` bytes; `read` null or writable.
pub unsafe extern "efiapi" fn read(
    this: *mut Stream,
    buffer: *mut u8,
    count: u32,
    read: *mut u32,
) -> HResult {
    if buffer.is_null() && count != 0 {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's duty.
    let (stream, memory) = unsafe { parts(this) };
    let bytes = memory.bytes();
    let start = usize::try_from(stream.position).map_or(bytes.len(), |at| at.min(bytes.len()));
    let taken = (count as usize).min(bytes.len() - start);
    if taken != 0 {
        // SAFETY: the caller's duty: `buffer` holds `count` >= `taken` bytes, apart from ours.
        unsafe { ptr::copy_nonoverlapping(bytes[start..].as_ptr(), buffer, taken) };
    }
    stream.position = (start + taken) as u64;
    if !read.is_null() {
        // SAFETY: the caller's duty.
        unsafe { read.write_unaligned(taken as u32) };
    }
    S_OK
}

/// `ISequentialStream::Write`: `count` bytes at the seek pointer, growing the stream.
///
/// # Safety
/// `this` must be a live stream; `buffer` `count` readable bytes; `written` null or writable.
pub unsafe extern "efiapi" fn write(
    this: *mut Stream,
    buffer: *const u8,
    count: u32,
    written: *mut u32,
) -> HResult {
    if buffer.is_null() && count != 0 {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's duty.
    let (stream, memory) = unsafe { parts(this) };
    let Some(end) = usize::try_from(stream.position)
        .ok()
        .and_then(|start| start.checked_add(count as usize))
    else {
        return E_OUTOFMEMORY;
    };
    let start = end - count as usize;
    if end > memory.size
        && let Err(code) = memory.resize(end)
    {
        return code;
    }
    if count != 0 {
        // SAFETY: the caller's duty: `count` readable bytes, apart from the stream's.
        unsafe {
            ptr::copy_nonoverlapping(buffer, memory.block.wrapping_add(start), count as usize)
        };
    }
    stream.position = end as u64;
    if !written.is_null() {
        // SAFETY: the caller's duty.
        unsafe { written.write_unaligned(count) };
    }
    S_OK
}

/// `IStream::Seek`: moves the seek pointer from the start, itself or the end; a pointer before
/// the start is refused.
///
/// # Safety
/// `this` must be a live stream; `position` null or writable.
pub unsafe extern "efiapi" fn seek(
    this: *mut Stream,
    offset: i64,
    origin: u32,
    position: *mut u64,
) -> HResult {
    // SAFETY: the caller's duty.
    let (stream, memory) = unsafe { parts(this) };
    let base = match origin {
        STREAM_SEEK_SET => 0,
        STREAM_SEEK_CUR => stream.position as i128,
        STREAM_SEEK_END => memory.size as i128,
        _ => return STG_E_INVALIDFUNCTION,
    };
    let target = base + i128::from(offset);
    let Ok(target) = u64::try_from(target) else {
        return STG_E_INVALIDFUNCTION;
    };
    stream.position = target;
    if !position.is_null() {
        // SAFETY: the caller's duty.
        unsafe { position.write_unaligned(target) };
    }
    S_OK
}

/// `IStream::SetSize`: the stream's new length, new bytes zeros; the seek pointer stays.
///
/// # Safety
/// `this` must be a live stream.
pub unsafe extern "efiapi" fn set_size(this: *mut Stream, size: u64) -> HResult {
    // SAFETY: the caller's duty.
    let (_, memory) = unsafe { parts(this) };
    let Ok(size) = usize::try_from(size) else {
        return E_OUTOFMEMORY;
    };
    match memory.resize(size) {
        Ok(()) => S_OK,
        Err(code) => code,
    }
}

/// `IStream::CopyTo`: up to `count` bytes from the seek pointer written to `destination`
/// through its `Write`, a chunk at a time.
///
/// # Safety
/// `this` must be a live stream, `destination` a live stream object, `read` and `written` null
/// or writable.
pub unsafe extern "efiapi" fn copy_to(
    this: *mut Stream,
    destination: usize,
    count: u64,
    read: *mut u64,
    written: *mut u64,
) -> HResult {
    if destination == 0 {
        return STG_E_INVALIDPOINTER;
    }
    let mut chunk = [0u8; COPY_CHUNK];
    let (mut total_read, mut total_written) = (0u64, 0u64);
    let mut outcome = S_OK;
    while total_read < count {
        let wanted = (count - total_read).min(COPY_CHUNK as u64) as u32;
        let mut got = 0u32;
        // SAFETY: the caller's duty; the chunk holds `wanted` bytes.
        unsafe { self::read(this, chunk.as_mut_ptr(), wanted, &mut got) };
        if got == 0 {
            break;
        }
        total_read += u64::from(got);
        let mut put = 0u32;
        let arguments = [chunk.as_ptr() as usize, got as usize, &raw mut put as usize];
        // SAFETY: the caller's duty: `Write` is an `ISequentialStream`'s fifth method.
        let code = unsafe { super::call_method(destination, SEQUENTIAL_WRITE_SLOT, arguments) };
        total_written += u64::from(put);
        if (code as u32 as i32) < 0 {
            outcome = code as u32 as i32;
            break;
        }
    }
    for (target, value) in [(read, total_read), (written, total_written)] {
        if !target.is_null() {
            // SAFETY: the caller's duty.
            unsafe { target.write_unaligned(value) };
        }
    }
    outcome
}

/// `IStream::Commit`: nothing to do, the stream is not transacted.
pub extern "efiapi" fn commit(_this: *mut Stream, _flags: u32) -> HResult {
    S_OK
}

/// `IStream::Revert`: nothing to undo, the stream is not transacted.
pub extern "efiapi" fn revert(_this: *mut Stream) -> HResult {
    S_OK
}

/// `IStream::LockRegion` and `UnlockRegion`: memory streams lock no regions.
pub extern "efiapi" fn lock_region(
    _this: *mut Stream,
    _offset: u64,
    _count: u64,
    _kind: u32,
) -> HResult {
    STG_E_INVALIDFUNCTION
}

/// `IStream::Stat`: a stream without a name, its size, read-write.
///
/// # Safety
/// `this` must be a live stream; `statistics` null or writable.
pub unsafe extern "efiapi" fn stat(
    this: *mut Stream,
    statistics: *mut StatStg,
    _flags: u32,
) -> HResult {
    if statistics.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's duty.
    let (_, memory) = unsafe { parts(this) };
    let report = StatStg {
        kind: STGTY_STREAM,
        size: memory.size as u64,
        mode: STGM_READWRITE,
        ..StatStg::default()
    };
    // SAFETY: the caller's duty.
    unsafe { statistics.write_unaligned(report) };
    S_OK
}

/// `IStream::Clone`: a stream over the same memory at the same seek pointer.
///
/// # Safety
/// `this` must be a live stream; `result` null or writable.
pub unsafe extern "efiapi" fn clone(this: *mut Stream, result: *mut usize) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    let (stream, memory) = unsafe { parts(this) };
    memory.references.fetch_add(1, Ordering::Relaxed);
    let clone = new_stream(stream.table, stream.memory, stream.position);
    if clone.is_null() {
        memory.references.fetch_sub(1, Ordering::Relaxed);
    }
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(clone as usize) };
    if clone.is_null() { E_OUTOFMEMORY } else { S_OK }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_stream_over(bytes: &[u8]) -> *mut Stream {
        let block = super::super::allocate(bytes.len());
        // SAFETY: the block holds `bytes.len()` bytes.
        unsafe { block.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        create(0, block, bytes.len(), true)
    }

    fn read_all(stream: *mut Stream) -> Vec<u8> {
        let mut buffer = [0u8; 64];
        let mut got = 0;
        // SAFETY: a live stream from here on.
        assert_eq!(
            unsafe { seek(stream, 0, STREAM_SEEK_SET, ptr::null_mut()) },
            S_OK
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { read(stream, buffer.as_mut_ptr(), 64, &mut got) },
            S_OK
        );
        buffer[..got as usize].to_vec()
    }

    #[test]
    fn reads_writes_and_seeks() {
        let stream = new_stream_over(b"hello");
        let mut buffer = [0u8; 3];
        let mut got = 0;
        // SAFETY: a live stream from here on.
        assert_eq!(
            unsafe { read(stream, buffer.as_mut_ptr(), 3, &mut got) },
            S_OK
        );
        assert_eq!((&buffer, got), (b"hel", 3));
        let mut at = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { seek(stream, -1, STREAM_SEEK_END, &mut at) }, S_OK);
        assert_eq!(at, 4);
        let mut put = 0;
        // SAFETY: as above.
        assert_eq!(
            unsafe { write(stream, b"O, world".as_ptr(), 8, &mut put) },
            S_OK
        );
        assert_eq!(put, 8);
        assert_eq!(read_all(stream), b"hellO, world");
        // SAFETY: as above.
        assert_eq!(
            unsafe { seek(stream, -20, STREAM_SEEK_CUR, &mut at) },
            STG_E_INVALIDFUNCTION
        );
        // SAFETY: as above.
        assert_eq!(unsafe { seek(stream, 3, STREAM_SEEK_END, &mut at) }, S_OK);
        // SAFETY: as above.
        assert_eq!(
            unsafe { write(stream, b"!".as_ptr(), 1, ptr::null_mut()) },
            S_OK
        );
        assert_eq!(read_all(stream), b"hellO, world\0\0\0!");
        // SAFETY: as above.
        assert_eq!(unsafe { release(stream) }, 0);
    }

    #[test]
    fn size_and_statistics() {
        let stream = new_stream_over(b"abcdef");
        // SAFETY: a live stream from here on.
        assert_eq!(unsafe { set_size(stream, 2) }, S_OK);
        assert_eq!(read_all(stream), b"ab");
        // SAFETY: as above.
        assert_eq!(unsafe { set_size(stream, 4) }, S_OK);
        assert_eq!(read_all(stream), b"ab\0\0");
        let mut statistics = StatStg::default();
        // SAFETY: as above.
        assert_eq!(unsafe { stat(stream, &mut statistics, 1) }, S_OK);
        assert_eq!(
            (statistics.kind, statistics.size, statistics.name),
            (STGTY_STREAM, 4, 0)
        );
        assert_eq!(commit(stream, 0), S_OK);
        assert_eq!(lock_region(stream, 0, 1, 0), STG_E_INVALIDFUNCTION);
        // SAFETY: as above.
        assert_eq!(unsafe { release(stream) }, 0);
    }

    #[test]
    fn clones_share_the_memory_with_their_own_pointers() {
        let stream = new_stream_over(b"shared");
        let mut copy = 0usize;
        // SAFETY: a live stream from here on.
        assert_eq!(unsafe { clone(stream, &mut copy) }, S_OK);
        let copy = copy as *mut Stream;
        // SAFETY: the clone is a live stream.
        assert_eq!(
            unsafe { write(copy, b"S".as_ptr(), 1, ptr::null_mut()) },
            S_OK
        );
        assert_eq!(read_all(stream), b"Shared");
        // SAFETY: the original's last reference; the clone keeps the memory.
        assert_eq!(unsafe { release(stream) }, 0);
        assert_eq!(read_all(copy), b"Shared");
        let mut answer = 0usize;
        // SAFETY: as above.
        let code = unsafe { query_interface(copy, &guid::SEQUENTIAL_STREAM, &mut answer) };
        assert_eq!((code, answer), (S_OK, copy as usize));
        // SAFETY: two references to the clone now.
        assert_eq!(unsafe { release(copy) }, 1);
        // SAFETY: the last one.
        assert_eq!(unsafe { release(copy) }, 0);
    }

    #[test]
    fn copy_to_writes_through_the_destination_s_write() {
        let table = [0, 0, 0, 0, write as usize];
        let source = new_stream_over(b"copy me");
        let target = new_stream_over(b"");
        // SAFETY: the target is a live stream; its table's fifth method is `Write`.
        unsafe { (*target).table = table.as_ptr() as usize };
        let (mut read, mut written) = (0u64, 0u64);
        // SAFETY: both are live streams.
        let code = unsafe { copy_to(source, target as usize, 4, &mut read, &mut written) };
        assert_eq!((code, read, written), (S_OK, 4, 4));
        assert_eq!(read_all(target), b"copy");
        for stream in [source, target] {
            // SAFETY: the last reference to each.
            assert_eq!(unsafe { release(stream) }, 0);
        }
    }
}
