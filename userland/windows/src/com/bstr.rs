//! BSTRs, Automation's strings: UTF-16 text with a NUL after it and its length in bytes, a
//! `u32`, in the four bytes before it. The block starts 8 bytes before the text, so the text is
//! 8-byte aligned as on 64-bit Windows; `SysAllocString` and its family make them,
//! `SysFreeString` gives the block back, and a null BSTR is the empty string.

use core::ptr;

const PREFIX: usize = 8;
const LENGTH_SIZE: usize = 4;
const NUL_SIZE: usize = 2;

/// A new BSTR of `bytes` bytes, zeroed, with its NUL after them; null without memory or for
/// more than 4 GiB.
pub fn allocate_bytes(bytes: usize) -> *mut u16 {
    let Ok(length) = u32::try_from(bytes) else {
        return ptr::null_mut();
    };
    let Some(total) = bytes.checked_add(PREFIX + NUL_SIZE) else {
        return ptr::null_mut();
    };
    let block = super::allocate(total);
    if block.is_null() {
        return ptr::null_mut();
    }
    let length_at = block.wrapping_add(PREFIX - LENGTH_SIZE).cast::<u32>();
    // SAFETY: the block is 16-byte aligned and holds `PREFIX` bytes before the text.
    unsafe { length_at.write(length) };
    block.wrapping_add(PREFIX).cast()
}

/// A new BSTR holding `units`; null without memory.
pub fn from_units(units: &[u16]) -> *mut u16 {
    let Some(bytes) = units.len().checked_mul(2) else {
        return ptr::null_mut();
    };
    let text = allocate_bytes(bytes);
    if !text.is_null() {
        // SAFETY: the new string holds `units.len()` units, apart from `units`.
        unsafe { text.copy_from_nonoverlapping(units.as_ptr(), units.len()) };
    }
    text
}

/// A new BSTR holding the ASCII text `bytes`; null without memory.
pub fn from_ascii(bytes: &[u8]) -> *mut u16 {
    let text = allocate_bytes(bytes.len() * 2);
    for (index, &byte) in bytes.iter().enumerate() {
        if text.is_null() {
            break;
        }
        // SAFETY: the new string holds `bytes.len()` units.
        unsafe { text.wrapping_add(index).write(u16::from(byte)) };
    }
    text
}

/// The length of `text` in bytes; 0 for null.
///
/// # Safety
/// `text` must be null or a BSTR.
pub unsafe fn byte_length(text: *const u16) -> u32 {
    if text.is_null() {
        return 0;
    }
    let length_at = text.cast::<u8>().wrapping_sub(LENGTH_SIZE).cast::<u32>();
    // SAFETY: the caller's duty: a BSTR's length is in the four bytes before it.
    unsafe { length_at.read_unaligned() }
}

/// The units of `text`; empty for null.
///
/// # Safety
/// `text` must be null or a BSTR that outlives the result and is not changed meanwhile.
pub unsafe fn units<'a>(text: *const u16) -> &'a [u16] {
    if text.is_null() {
        return &[];
    }
    // SAFETY: the caller's duty.
    let length = unsafe { byte_length(text) } as usize / 2;
    // SAFETY: as above: a BSTR holds its length's units.
    unsafe { core::slice::from_raw_parts(text, length) }
}

/// Gives back `text`'s block; null does nothing.
///
/// # Safety
/// `text` must be null or a BSTR these functions made that nothing uses any more.
pub unsafe fn free(text: *mut u16) {
    if !text.is_null() {
        // SAFETY: the caller's duty: the block starts `PREFIX` bytes before the text.
        unsafe { super::free(text.cast::<u8>().wrapping_sub(PREFIX)) };
    }
}

/// A copy of `text` with the same byte length (null for null); `None` without memory.
///
/// # Safety
/// `text` must be null or a BSTR.
pub unsafe fn duplicate(text: *const u16) -> Option<*mut u16> {
    if text.is_null() {
        return Some(ptr::null_mut());
    }
    // SAFETY: the caller's duty.
    let bytes = unsafe { byte_length(text) } as usize;
    let copy = allocate_bytes(bytes);
    if copy.is_null() {
        return None;
    }
    // SAFETY: both strings hold `bytes` bytes and are different blocks.
    unsafe { ptr::copy_nonoverlapping(text.cast::<u8>(), copy.cast::<u8>(), bytes) };
    Some(copy)
}

/// `SysAllocString`: a BSTR copy of the NUL-terminated `text`; null for null.
///
/// # Safety
/// `text` must be null or NUL-terminated.
pub unsafe extern "efiapi" fn sys_alloc_string(text: *const u16) -> *mut u16 {
    if text.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller's duty.
    from_units(unsafe { super::wide_text(text) })
}

/// `SysAllocStringLen`: a BSTR of `length` units, copied from `text` unless it is null.
///
/// # Safety
/// `text` must be null or hold `length` readable units.
pub unsafe extern "efiapi" fn sys_alloc_string_len(text: *const u16, length: u32) -> *mut u16 {
    let units = length as usize;
    let string = allocate_bytes(units * 2);
    if !string.is_null() && !text.is_null() {
        // SAFETY: the caller's duty; the new string holds `units` units in its own block.
        unsafe { string.copy_from_nonoverlapping(text, units) };
    }
    string
}

/// `SysAllocStringByteLen`: a BSTR of `length` bytes, copied from `bytes` unless it is null.
///
/// # Safety
/// `bytes` must be null or hold `length` readable bytes.
pub unsafe extern "efiapi" fn sys_alloc_string_byte_len(bytes: *const u8, length: u32) -> *mut u16 {
    let string = allocate_bytes(length as usize);
    if !string.is_null() && !bytes.is_null() {
        // SAFETY: the caller's duty; the new string holds `length` bytes in its own block.
        unsafe { ptr::copy_nonoverlapping(bytes, string.cast::<u8>(), length as usize) };
    }
    string
}

unsafe fn replace(string: *mut *mut u16, new: *mut u16) -> i32 {
    if new.is_null() {
        return 0;
    }
    // SAFETY: the caller's duty: `string` holds a BSTR or null.
    let old = unsafe { string.replace(new) };
    // SAFETY: the old string was the caller's, which gives it up here.
    unsafe { free(old) };
    1
}

/// `SysReAllocString`: replaces `*string` with a copy of `text` (empty for null); FALSE
/// without memory, leaving it.
///
/// # Safety
/// `string` must be null or hold a BSTR (or null); `text` must be null or NUL-terminated.
pub unsafe extern "efiapi" fn sys_re_alloc_string(string: *mut *mut u16, text: *const u16) -> i32 {
    if string.is_null() {
        return 0;
    }
    // SAFETY: the caller's duty.
    let new = from_units(unsafe { super::wide_text(text) });
    // SAFETY: the caller's duty.
    unsafe { replace(string, new) }
}

/// `SysReAllocStringLen`: replaces `*string` with `length` units copied from `text` (zeros for
/// null); FALSE without memory, leaving it.
///
/// # Safety
/// `string` must be null or hold a BSTR (or null); `text` must be null or hold `length` units.
pub unsafe extern "efiapi" fn sys_re_alloc_string_len(
    string: *mut *mut u16,
    text: *const u16,
    length: u32,
) -> i32 {
    if string.is_null() {
        return 0;
    }
    // SAFETY: the caller's duty.
    let new = unsafe { sys_alloc_string_len(text, length) };
    // SAFETY: the caller's duty.
    unsafe { replace(string, new) }
}

/// `SysFreeString`.
///
/// # Safety
/// `text` must be null or a BSTR that nothing uses any more.
pub unsafe extern "efiapi" fn sys_free_string(text: *mut u16) {
    // SAFETY: the caller's duty.
    unsafe { free(text) }
}

/// `SysStringLen`: the length of `text` in units; 0 for null.
///
/// # Safety
/// `text` must be null or a BSTR.
pub unsafe extern "efiapi" fn sys_string_len(text: *const u16) -> u32 {
    // SAFETY: the caller's duty.
    let bytes = unsafe { byte_length(text) };
    bytes / 2
}

/// `SysStringByteLen`: the length of `text` in bytes; 0 for null.
///
/// # Safety
/// `text` must be null or a BSTR.
pub unsafe extern "efiapi" fn sys_string_byte_len(text: *const u16) -> u32 {
    // SAFETY: the caller's duty.
    unsafe { byte_length(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    #[test]
    fn a_bstr_has_its_byte_length_before_it_and_a_nul_after_it() {
        let source = wide("Apples");
        // SAFETY: the text is NUL-terminated.
        let text = unsafe { sys_alloc_string(source.as_ptr()) };
        assert!(!text.is_null());
        assert_eq!(text as usize % 8, 0);
        // SAFETY: the BSTR's length is in the four bytes before it.
        let prefix = unsafe { text.cast::<u8>().wrapping_sub(4).cast::<u32>().read() };
        assert_eq!(prefix, 12);
        // SAFETY: the BSTR holds 6 units and its NUL.
        let with_nul = unsafe { core::slice::from_raw_parts(text, 7) };
        assert_eq!(with_nul, &source[..]);
        // SAFETY: a BSTR.
        assert_eq!(unsafe { sys_string_len(text) }, 6);
        // SAFETY: a BSTR.
        assert_eq!(unsafe { sys_string_byte_len(text) }, 12);
        // SAFETY: unused now.
        unsafe { sys_free_string(text) };
    }

    #[test]
    fn lengths_and_bytes_are_kept_exactly() {
        let source = wide("abcdef");
        // SAFETY: the text holds 6 units.
        let three = unsafe { sys_alloc_string_len(source.as_ptr(), 3) };
        // SAFETY: a BSTR.
        assert_eq!(unsafe { units(three) }, &source[..3]);
        // SAFETY: null text gives zeros.
        let zeros = unsafe { sys_alloc_string_len(core::ptr::null(), 4) };
        // SAFETY: a BSTR.
        assert_eq!(unsafe { units(zeros) }, &[0u16; 4]);
        // SAFETY: five readable bytes.
        let odd = unsafe { sys_alloc_string_byte_len(b"hello".as_ptr(), 5) };
        // SAFETY: a BSTR.
        assert_eq!(unsafe { sys_string_byte_len(odd) }, 5);
        // SAFETY: a BSTR.
        assert_eq!(unsafe { sys_string_len(odd) }, 2);
        // SAFETY: a BSTR.
        let copy = unsafe { duplicate(odd) }.unwrap();
        // SAFETY: a BSTR.
        assert_eq!(unsafe { byte_length(copy) }, 5);
        for text in [three, zeros, odd, copy] {
            // SAFETY: each is unused now.
            unsafe { free(text) };
        }
    }

    #[test]
    fn null_is_the_empty_string() {
        // SAFETY: null is allowed.
        assert_eq!(unsafe { sys_string_len(core::ptr::null()) }, 0);
        // SAFETY: null is allowed.
        assert!(unsafe { units(core::ptr::null()) }.is_empty());
        // SAFETY: null is allowed.
        assert!(unsafe { sys_alloc_string(core::ptr::null()) }.is_null());
        // SAFETY: null is allowed.
        assert_eq!(
            unsafe { duplicate(core::ptr::null()) },
            Some(core::ptr::null_mut())
        );
    }

    #[test]
    fn reallocation_replaces_the_string() {
        let first = wide("one");
        let second = wide("three");
        // SAFETY: NUL-terminated.
        let mut text = unsafe { sys_alloc_string(first.as_ptr()) };
        // SAFETY: `text` holds a BSTR; the new text is NUL-terminated.
        assert_eq!(
            unsafe { sys_re_alloc_string(&mut text, second.as_ptr()) },
            1
        );
        // SAFETY: a BSTR.
        assert_eq!(unsafe { units(text) }, &second[..5]);
        // SAFETY: as above, two units.
        assert_eq!(
            unsafe { sys_re_alloc_string_len(&mut text, second.as_ptr(), 2) },
            1
        );
        // SAFETY: a BSTR.
        assert_eq!(unsafe { units(text) }, &second[..2]);
        // SAFETY: unused now.
        unsafe { free(text) };
        let ascii = from_ascii(b"True");
        // SAFETY: a BSTR.
        assert_eq!(unsafe { units(ascii) }, &wide("True")[..4]);
        // SAFETY: unused now.
        unsafe { free(ascii) };
    }
}
