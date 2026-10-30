//! COM and OLE Automation for the Windows layer (D178), the parts pure enough to test on the
//! host: GUIDs and their text (`guid`), BSTRs (`bstr`), VARIANTs (`variant`) and the
//! conversions between their kinds (`conversion`, with dates in `date`), SAFEARRAYs
//! (`safe_array`), the `Scripting.Dictionary` class (`dictionary`) with its enumerator
//! (`enumerator`), and the memory stream `CreateStreamOnHGlobal` makes (`memory_stream`), all
//! laid out as COM's binary standard and Microsoft's headers give them for 64-bit Windows. An
//! object is a heap block whose first word points at its interface's table of methods;
//! `win32/com` builds the tables, whose entries keep the program's `x18` on aarch64, and exports
//! the functions. Objects follow the apartment model: one thread uses an object at a time, so
//! only reference counts are atomic. In host tests blocks come from Rust's allocator and calls
//! into objects are direct.

pub mod bstr;
pub mod conversion;
pub mod date;
pub mod dictionary;
pub mod enumerator;
pub mod guid;
pub mod memory_stream;
pub mod safe_array;
pub mod variant;

use core::sync::atomic::{AtomicU32, Ordering};
use guid::Guid;

/// A COM status code (`HRESULT`): negative for a failure.
pub type HResult = i32;

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003_u32 as i32;
pub const E_UNEXPECTED: HResult = 0x8000_ffff_u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000e_u32 as i32;

const TABLE_QUERY_INTERFACE: usize = 0;
const TABLE_ADD_REFERENCE: usize = 1;
const TABLE_RELEASE: usize = 2;

/// A new block of `size` bytes, zeroed and 16-byte aligned, from the layer's heap; null without
/// memory.
#[cfg(runs_windows_programs)]
pub fn allocate(size: usize) -> *mut u8 {
    crate::win32::heap_allocate(size)
}

/// Gives back a block [`allocate`] returned; null does nothing.
///
/// # Safety
/// `block` must be null or a block [`allocate`] returned that nothing uses any more.
#[cfg(runs_windows_programs)]
pub unsafe fn free(block: *mut u8) {
    // SAFETY: the caller's duty; the layer's heap made the block.
    unsafe { crate::win32::heap_free(block) }
}

#[cfg(runs_windows_programs)]
unsafe fn call(address: usize, arguments: [usize; 4]) -> usize {
    // SAFETY: the caller's duty: a method of an object, with arguments it accepts.
    unsafe { crate::win32::call_program(address, arguments) }
}

#[cfg(not(runs_windows_programs))]
const HOST_HEADER: usize = 16;

/// A new block of `size` bytes, zeroed and 16-byte aligned; null without memory.
#[cfg(not(runs_windows_programs))]
pub fn allocate(size: usize) -> *mut u8 {
    let Some(layout) = size
        .checked_add(HOST_HEADER)
        .and_then(|total| std::alloc::Layout::from_size_align(total, HOST_HEADER).ok())
    else {
        return core::ptr::null_mut();
    };
    // SAFETY: the layout is at least 16 bytes long.
    let block = unsafe { std::alloc::alloc_zeroed(layout) };
    if block.is_null() {
        return block;
    }
    // SAFETY: the block starts with 16 bytes of its own, aligned for a `usize`.
    unsafe { block.cast::<usize>().write(size) };
    block.wrapping_add(HOST_HEADER)
}

/// Gives back a block [`allocate`] returned; null does nothing.
///
/// # Safety
/// `block` must be null or a block [`allocate`] returned that nothing uses any more.
#[cfg(not(runs_windows_programs))]
pub unsafe fn free(block: *mut u8) {
    if block.is_null() {
        return;
    }
    let start = block.wrapping_sub(HOST_HEADER);
    // SAFETY: the caller's duty: the block's size is in the 16 bytes before it.
    let size = unsafe { start.cast::<usize>().read() };
    let Ok(layout) = std::alloc::Layout::from_size_align(size + HOST_HEADER, HOST_HEADER) else {
        return;
    };
    // SAFETY: `start` came from `alloc_zeroed` with this layout.
    unsafe { std::alloc::dealloc(start, layout) };
}

#[cfg(not(runs_windows_programs))]
unsafe fn call(address: usize, arguments: [usize; 4]) -> usize {
    // SAFETY: the caller's duty: `address` is a method that takes at most four arguments.
    let method: unsafe extern "efiapi" fn(usize, usize, usize, usize) -> usize =
        unsafe { core::mem::transmute(address) };
    // SAFETY: as above; a method ignores the arguments it does not take.
    unsafe { method(arguments[0], arguments[1], arguments[2], arguments[3]) }
}

/// Calls method number `slot` of `object`'s table with `object` and `arguments` (a method that
/// takes fewer ignores the rest), and returns its result register.
///
/// # Safety
/// `object` must be a live COM object whose table has at least `slot + 1` methods, the one in
/// `slot` taking at most four arguments that accept these values.
pub unsafe fn call_method(object: usize, slot: usize, arguments: [usize; 3]) -> usize {
    // SAFETY: the caller's duty: an object's first word is its table's address.
    let table = unsafe { (object as *const *const usize).read() };
    // SAFETY: as above; the table holds the slot.
    let method = unsafe { table.wrapping_add(slot).read() };
    // SAFETY: the caller's duty.
    unsafe { call(method, [object, arguments[0], arguments[1], arguments[2]]) }
}

/// Adds a reference to `object` (`IUnknown::AddRef`); null does nothing.
///
/// # Safety
/// `object` must be null or a live COM object.
pub unsafe fn add_reference(object: usize) {
    if object != 0 {
        // SAFETY: the caller's duty; `AddRef` is every table's second method.
        unsafe { call_method(object, TABLE_ADD_REFERENCE, [0; 3]) };
    }
}

/// Gives back a reference to `object` (`IUnknown::Release`); null does nothing.
///
/// # Safety
/// `object` must be null or a live COM object the caller holds a reference to.
pub unsafe fn release(object: usize) {
    if object != 0 {
        // SAFETY: the caller's duty; `Release` is every table's third method.
        unsafe { call_method(object, TABLE_RELEASE, [0; 3]) };
    }
}

/// Asks `object` for `interface` (`IUnknown::QueryInterface`): its status, and through
/// `result` the interface's pointer, with a reference, or null.
///
/// # Safety
/// `object` must be a live COM object and `result` writable.
pub unsafe fn query_interface(object: usize, interface: &Guid, result: *mut usize) -> HResult {
    let arguments = [interface as *const Guid as usize, result as usize, 0];
    // SAFETY: the caller's duty; `QueryInterface` is every table's first method.
    unsafe { call_method(object, TABLE_QUERY_INTERFACE, arguments) as u32 as i32 }
}

/// Answers `QueryInterface` for one of the layer's objects, `this`, whose interfaces are
/// `interfaces` (all at `this`): `S_OK` with a new reference in `references`, `E_NOINTERFACE`,
/// or `E_POINTER` when `result` is null.
///
/// # Safety
/// `interface` must be null or point at a GUID, and `result` must be null or writable.
pub unsafe fn answer_query(
    this: usize,
    references: &AtomicU32,
    interface: *const Guid,
    result: *mut usize,
    interfaces: &[Guid],
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    let wanted = unsafe { Guid::read(interface) };
    match wanted.filter(|wanted| interfaces.contains(wanted)) {
        Some(_) => {
            references.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the caller's duty.
            unsafe { result.write(this) };
            S_OK
        }
        None => {
            // SAFETY: as above.
            unsafe { result.write(0) };
            E_NOINTERFACE
        }
    }
}

/// Takes one reference from `references` and says whether it was the last, so the object
/// goes; returns the count left, as `Release` does.
pub fn drop_reference(references: &AtomicU32) -> (u32, bool) {
    let left = references.fetch_sub(1, Ordering::AcqRel).wrapping_sub(1);
    (left, left == 0)
}

/// The UTF-16 text at `text` up to its NUL, without it; empty for null.
///
/// # Safety
/// `text` must be null or point at NUL-terminated UTF-16 text that outlives the result.
pub unsafe fn wide_text<'a>(text: *const u16) -> &'a [u16] {
    if text.is_null() {
        return &[];
    }
    let mut length = 0;
    // SAFETY: the caller's duty: the text ends with a NUL.
    while unsafe { text.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: as above: `length` units precede the NUL.
    unsafe { core::slice::from_raw_parts(text, length) }
}

/// A short ASCII text: a number's or a date's, before it becomes a BSTR.
#[derive(Debug)]
pub struct AsciiText {
    bytes: [u8; ASCII_TEXT_CAPACITY],
    length: usize,
}

const ASCII_TEXT_CAPACITY: usize = 64;

impl AsciiText {
    pub const fn new() -> Self {
        Self {
            bytes: [0; ASCII_TEXT_CAPACITY],
            length: 0,
        }
    }

    /// Appends `bytes`, as many as fit.
    pub fn push(&mut self, bytes: &[u8]) {
        let taken = bytes.len().min(self.bytes.len() - self.length);
        self.bytes[self.length..self.length + taken].copy_from_slice(&bytes[..taken]);
        self.length += taken;
    }

    /// Appends `value` in decimal, at least `width` digits.
    pub fn number(&mut self, value: u128, width: usize) {
        let mut digits = [b'0'; 40];
        let mut start = digits.len();
        let mut left = value;
        loop {
            start -= 1;
            digits[start] = b'0' + (left % 10) as u8;
            left /= 10;
            if left == 0 {
                break;
            }
        }
        start = start.min(digits.len().saturating_sub(width));
        self.push(&digits[start..]);
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Default for AsciiText {
    fn default() -> Self {
        Self::new()
    }
}

impl eggos_c_format::Output for AsciiText {
    fn write(&mut self, bytes: &[u8]) {
        self.push(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_come_zeroed_and_aligned() {
        let block = allocate(40);
        assert!(!block.is_null());
        assert_eq!(block as usize % 16, 0);
        // SAFETY: the block holds 40 bytes.
        let bytes = unsafe { core::slice::from_raw_parts(block, 40) };
        assert!(bytes.iter().all(|&byte| byte == 0));
        // SAFETY: the block is unused.
        unsafe { free(block) };
    }

    #[test]
    fn wide_text_stops_at_the_nul() {
        let text = [b'h' as u16, b'i' as u16, 0, b'x' as u16];
        // SAFETY: the array has a NUL.
        assert_eq!(unsafe { wide_text(text.as_ptr()) }, &text[..2]);
        // SAFETY: null is allowed.
        assert!(unsafe { wide_text(core::ptr::null()) }.is_empty());
    }

    #[test]
    fn the_last_reference_says_so() {
        let references = AtomicU32::new(2);
        assert_eq!(drop_reference(&references), (1, false));
        assert_eq!(drop_reference(&references), (0, true));
    }
}
