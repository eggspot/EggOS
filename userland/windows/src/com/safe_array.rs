//! SAFEARRAYs, Automation's arrays: a descriptor (dimension count, `FADF_*` features, element
//! size, lock count, data pointer, then one bound per dimension, stored rightmost dimension
//! first) and the elements, column-major (the leftmost index varies fastest). Index vectors,
//! as `SafeArrayPtrOfIndex` and `SafeArrayGetElement` take them, list the leftmost dimension
//! first, and dimension 1 of `SafeArrayGetUBound` is the leftmost. The layer keeps the element
//! kind in the four bytes before a descriptor it makes (`FADF_HAVEVARTYPE`). Arrays of strings,
//! objects and VARIANTs own their elements, which copying copies and destroying gives back.

use super::variant::{self, Variant, kind};
use super::{E_INVALIDARG, E_OUTOFMEMORY, E_UNEXPECTED, HResult, S_OK, bstr};
use core::ptr;

/// `DISP_E_BADINDEX`: an index or dimension outside the array.
pub const DISP_E_BADINDEX: HResult = 0x8002_000b_u32 as i32;
const DISP_E_ARRAYISLOCKED: HResult = 0x8002_000d_u32 as i32;

const FADF_AUTO: u16 = 0x1;
const FADF_STATIC: u16 = 0x2;
const FADF_EMBEDDED: u16 = 0x4;
const FADF_RECORD: u16 = 0x20;
const FADF_HAVEVARTYPE: u16 = 0x80;
const FADF_BSTR: u16 = 0x100;
const FADF_UNKNOWN: u16 = 0x200;
const FADF_DISPATCH: u16 = 0x400;
const FADF_VARIANT: u16 = 0x800;
const NOT_ALLOCATED: u16 = FADF_AUTO | FADF_STATIC | FADF_EMBEDDED;
const KIND_FEATURES: u16 = FADF_BSTR | FADF_UNKNOWN | FADF_DISPATCH | FADF_VARIANT;
const PREFIX: usize = 16;
const KIND_SIZE: usize = 4;
const BOUNDS_OFFSET: usize = 24;
const MOST_LOCKS: u32 = 0xffff;
const MOST_DIMENSIONS_ON_STACK: usize = 64;
const LARGEST_ELEMENT: usize = core::mem::size_of::<Variant>();

/// One dimension's bounds (`SAFEARRAYBOUND`): its element count and lowest index.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub elements: u32,
    pub lower: i32,
}

const NO_BOUND: Bound = Bound {
    elements: 0,
    lower: 0,
};

/// An array's descriptor (`SAFEARRAY`); its `dimensions` bounds follow it.
#[repr(C)]
#[derive(Debug)]
pub struct SafeArray {
    pub dimensions: u16,
    pub features: u16,
    pub element_size: u32,
    pub locks: u32,
    pub data: *mut u8,
}

const _: () = assert!(core::mem::size_of::<SafeArray>() == BOUNDS_OFFSET);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Elements {
    Plain,
    Strings,
    Objects,
    Variants,
}

fn elements_of(features: u16) -> Elements {
    if features & FADF_VARIANT != 0 {
        Elements::Variants
    } else if features & FADF_BSTR != 0 {
        Elements::Strings
    } else if features & (FADF_UNKNOWN | FADF_DISPATCH) != 0 {
        Elements::Objects
    } else {
        Elements::Plain
    }
}

fn kind_features(element_kind: u16) -> u16 {
    match element_kind {
        kind::BSTR => FADF_BSTR,
        kind::UNKNOWN => FADF_UNKNOWN,
        kind::DISPATCH => FADF_DISPATCH,
        kind::VARIANT => FADF_VARIANT,
        _ => 0,
    }
}

/// The element `indices` (one per dimension, leftmost first) names, as its number in the
/// column-major data, given `bounds` stored rightmost first; `None` outside them.
pub fn element_number(bounds: &[Bound], indices: &[i32]) -> Option<usize> {
    if indices.len() != bounds.len() {
        return None;
    }
    let mut number = 0usize;
    let mut stride = 1usize;
    for (&index, bound) in indices.iter().zip(bounds.iter().rev()) {
        let offset = i64::from(index) - i64::from(bound.lower);
        if offset < 0 || offset >= i64::from(bound.elements) {
            return None;
        }
        number = number.checked_add((offset as usize).checked_mul(stride)?)?;
        stride = stride.checked_mul(bound.elements as usize)?;
    }
    Some(number)
}

fn element_count(bounds: &[Bound]) -> Option<usize> {
    bounds.iter().try_fold(1usize, |count, bound| {
        count.checked_mul(bound.elements as usize)
    })
}

fn data_size(stored: &[Bound], element_size: u32) -> Option<usize> {
    element_count(stored)?.checked_mul(element_size as usize)
}

fn bounds_of(array: *mut SafeArray) -> *mut Bound {
    array.cast::<u8>().wrapping_add(BOUNDS_OFFSET).cast()
}

unsafe fn descriptor<'a>(array: *const SafeArray) -> &'a SafeArray {
    // SAFETY: the caller's duty.
    unsafe { &*array }
}

unsafe fn stored_bounds<'a>(array: *const SafeArray) -> &'a [Bound] {
    // SAFETY: the caller's duty.
    let dimensions = usize::from(unsafe { descriptor(array) }.dimensions);
    // SAFETY: the caller's duty: the descriptor holds that many bounds from its fourth word.
    unsafe { core::slice::from_raw_parts(bounds_of(array.cast_mut()), dimensions) }
}

fn build(element_kind: u16, features: u16, element_size: u32, stored: &[Bound]) -> *mut SafeArray {
    let Ok(dimensions) = u16::try_from(stored.len()) else {
        return ptr::null_mut();
    };
    let Some(bytes) = data_size(stored, element_size) else {
        return ptr::null_mut();
    };
    let header_size = BOUNDS_OFFSET + core::mem::size_of::<Bound>() * stored.len().max(1);
    let block = super::allocate(PREFIX + header_size);
    if block.is_null() {
        return ptr::null_mut();
    }
    let data = match bytes {
        0 => ptr::null_mut(),
        _ => super::allocate(bytes),
    };
    if bytes != 0 && data.is_null() {
        // SAFETY: the block is fresh and unused.
        unsafe { super::free(block) };
        return ptr::null_mut();
    }
    let kind_at = block.wrapping_add(PREFIX - KIND_SIZE).cast::<u32>();
    // SAFETY: the block holds `PREFIX` bytes before the descriptor, 16-byte aligned.
    unsafe { kind_at.write(u32::from(element_kind)) };
    let array = block.wrapping_add(PREFIX).cast::<SafeArray>();
    let header = SafeArray {
        dimensions,
        features: features | FADF_HAVEVARTYPE,
        element_size,
        locks: 0,
        data,
    };
    // SAFETY: the block holds the descriptor and its bounds after the prefix.
    unsafe { array.write(header) };
    for (index, bound) in stored.iter().enumerate() {
        // SAFETY: as above: all the bounds fit.
        unsafe { bounds_of(array).wrapping_add(index).write(*bound) };
    }
    array
}

/// A new array of `element_kind` with `bounds` (leftmost dimension first), its elements
/// zeroed, as `SafeArrayCreate` makes it; null for a kind no array holds, no dimensions, or
/// without memory.
pub fn create(element_kind: u16, bounds: &[Bound]) -> *mut SafeArray {
    let Some(size) = variant::value_size(element_kind) else {
        return ptr::null_mut();
    };
    if bounds.is_empty() {
        return ptr::null_mut();
    }
    let features = kind_features(element_kind);
    let mut on_stack = [NO_BOUND; MOST_DIMENSIONS_ON_STACK];
    let Some(stored) = on_stack.get_mut(..bounds.len()) else {
        return create_many(element_kind, features, size as u32, bounds);
    };
    for (slot, bound) in stored.iter_mut().zip(bounds.iter().rev()) {
        *slot = *bound;
    }
    build(element_kind, features, size as u32, stored)
}

fn create_many(element_kind: u16, features: u16, size: u32, bounds: &[Bound]) -> *mut SafeArray {
    let Some(bytes) = bounds.len().checked_mul(core::mem::size_of::<Bound>()) else {
        return ptr::null_mut();
    };
    let reversed = super::allocate(bytes).cast::<Bound>();
    if reversed.is_null() {
        return ptr::null_mut();
    }
    for (index, bound) in bounds.iter().rev().enumerate() {
        // SAFETY: the block holds `bounds.len()` bounds.
        unsafe { reversed.wrapping_add(index).write(*bound) };
    }
    // SAFETY: as above, all written.
    let stored = unsafe { core::slice::from_raw_parts(reversed, bounds.len()) };
    let array = build(element_kind, features, size, stored);
    // SAFETY: the block is this function's, unused now.
    unsafe { super::free(reversed.cast()) };
    array
}

/// The VARIANTs of an array of VARIANTs, in data order.
///
/// # Safety
/// `array` must be a valid descriptor of VARIANT elements, used by no one else meanwhile.
pub unsafe fn variants<'a>(array: *mut SafeArray) -> &'a mut [Variant] {
    // SAFETY: the caller's duty.
    let count = element_count(unsafe { stored_bounds(array) }).unwrap_or(0);
    // SAFETY: as above.
    let data = unsafe { descriptor(array) }.data;
    if data.is_null() || count == 0 {
        return &mut [];
    }
    // SAFETY: as above: the data holds `count` VARIANTs.
    unsafe { core::slice::from_raw_parts_mut(data.cast::<Variant>(), count) }
}

unsafe fn release_element(elements: Elements, at: *mut u8) {
    match elements {
        Elements::Plain => {}
        Elements::Strings => {
            // SAFETY: the caller's duty: the element is a BSTR.
            let text = unsafe { at.cast::<*mut u16>().read_unaligned() };
            // SAFETY: as above: the array owns it.
            unsafe { bstr::free(text) };
        }
        Elements::Objects => {
            // SAFETY: the caller's duty: the element is an interface pointer.
            let object = unsafe { at.cast::<usize>().read_unaligned() };
            // SAFETY: as above: the array holds a reference.
            unsafe { super::release(object) };
        }
        Elements::Variants => {
            // SAFETY: the caller's duty: the element is a VARIANT the array owns.
            unsafe { variant::clear(at.cast()) };
        }
    }
}

unsafe fn release_elements(array: *mut SafeArray) {
    // SAFETY: the caller's duty.
    let header = unsafe { descriptor(array) };
    let elements = elements_of(header.features);
    let (size, data) = (header.element_size, header.data);
    if elements == Elements::Plain || data.is_null() {
        return;
    }
    // SAFETY: the caller's duty.
    let count = element_count(unsafe { stored_bounds(array) }).unwrap_or(0);
    for number in 0..count {
        let at = data.wrapping_add(number * size as usize);
        // SAFETY: the data holds `count` elements of `size` bytes, which the array owns.
        unsafe { release_element(elements, at) };
    }
}

/// `SafeArrayDestroy`'s work: refuses a locked array, else gives back its elements and, unless
/// the program made it (`FADF_AUTO`, `FADF_STATIC`, `FADF_EMBEDDED`), its memory.
///
/// # Safety
/// `array` must be null or a valid descriptor that nothing uses after it is destroyed.
pub unsafe fn destroy(array: *mut SafeArray) -> HResult {
    if array.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let header = unsafe { descriptor(array) };
    let (locks, features, data) = (header.locks, header.features, header.data);
    if locks != 0 {
        return DISP_E_ARRAYISLOCKED;
    }
    // SAFETY: the caller's duty.
    unsafe { release_elements(array) };
    if features & NOT_ALLOCATED == 0 {
        // SAFETY: the layer made the data and the descriptor, which go now.
        unsafe { super::free(data) };
        // SAFETY: as above; the descriptor's block starts `PREFIX` bytes before it.
        unsafe { super::free(array.cast::<u8>().wrapping_sub(PREFIX)) };
    }
    S_OK
}

unsafe fn element_kind(array: *const SafeArray) -> u16 {
    // SAFETY: the caller's duty.
    let features = unsafe { descriptor(array) }.features;
    if features & FADF_HAVEVARTYPE == 0 {
        return 0;
    }
    let kind_at = array.cast::<u8>().wrapping_sub(KIND_SIZE).cast::<u32>();
    // SAFETY: the caller's duty: with `FADF_HAVEVARTYPE` the kind is in the 4 bytes before.
    unsafe { kind_at.read_unaligned() as u16 }
}

unsafe fn copy_element(elements: Elements, from: *const u8, to: *mut u8, size: usize) -> HResult {
    match elements {
        Elements::Plain => {
            // SAFETY: the caller's duty.
            unsafe { ptr::copy_nonoverlapping(from, to, size) };
        }
        Elements::Strings => {
            // SAFETY: the caller's duty: the element is a BSTR.
            let text = unsafe { from.cast::<*const u16>().read_unaligned() };
            // SAFETY: as above.
            let Some(copy) = (unsafe { bstr::duplicate(text) }) else {
                return E_OUTOFMEMORY;
            };
            // SAFETY: the caller's duty: `to` holds a pointer.
            unsafe { to.cast::<*mut u16>().write_unaligned(copy) };
        }
        Elements::Objects => {
            // SAFETY: the caller's duty: the element is an interface pointer.
            let object = unsafe { from.cast::<usize>().read_unaligned() };
            // SAFETY: as above.
            unsafe { super::add_reference(object) };
            // SAFETY: the caller's duty: `to` holds a pointer.
            unsafe { to.cast::<usize>().write_unaligned(object) };
        }
        Elements::Variants => {
            // SAFETY: the caller's duty: the element is a VARIANT.
            let value = unsafe { from.cast::<Variant>().read_unaligned() };
            // SAFETY: as above.
            match unsafe { variant::duplicate(&value) } {
                // SAFETY: the caller's duty: `to` holds a VARIANT.
                Ok(copy) => unsafe { to.cast::<Variant>().write_unaligned(copy) },
                Err(code) => return code,
            }
        }
    }
    S_OK
}

/// `SafeArrayCopy`'s work: a new array with the same shape and kind and copies of the elements.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe fn copy(array: *const SafeArray) -> Result<*mut SafeArray, HResult> {
    if array.is_null() {
        return Err(E_INVALIDARG);
    }
    // SAFETY: the caller's duty.
    let header = unsafe { descriptor(array) };
    let (features, size, data) = (header.features, header.element_size, header.data);
    if features & FADF_RECORD != 0 {
        return Err(variant::DISP_E_BADVARTYPE);
    }
    // SAFETY: the caller's duty.
    let element_kind = unsafe { element_kind(array) };
    // SAFETY: the caller's duty.
    let stored = unsafe { stored_bounds(array) };
    let copy = build(element_kind, features & KIND_FEATURES, size, stored);
    if copy.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the new descriptor is this function's.
    let new = unsafe { &mut *copy };
    if element_kind == 0 {
        new.features &= !FADF_HAVEVARTYPE;
    }
    let target = new.data;
    if data.is_null() || target.is_null() {
        return Ok(copy);
    }
    let elements = elements_of(features);
    let count = element_count(stored).unwrap_or(0);
    let size = size as usize;
    for number in 0..count {
        let from = data.wrapping_add(number * size);
        let to = target.wrapping_add(number * size);
        // SAFETY: both hold `count` elements of `size` bytes.
        let copied = unsafe { copy_element(elements, from, to, size) };
        if copied < 0 {
            // SAFETY: the copy is this function's; elements not yet copied are zeros.
            unsafe { destroy(copy) };
            return Err(copied);
        }
    }
    Ok(copy)
}

unsafe fn element_at(
    array: *mut SafeArray,
    indices: *const i32,
) -> Result<(*mut u8, Elements, usize), HResult> {
    if array.is_null() || indices.is_null() {
        return Err(E_INVALIDARG);
    }
    // SAFETY: the caller's duty.
    let stored = unsafe { stored_bounds(array) };
    // SAFETY: the caller's duty: one index per dimension.
    let indices = unsafe { core::slice::from_raw_parts(indices, stored.len()) };
    let number = element_number(stored, indices).ok_or(DISP_E_BADINDEX)?;
    // SAFETY: the caller's duty.
    let header = unsafe { descriptor(array) };
    let size = header.element_size as usize;
    if header.data.is_null() {
        return Err(DISP_E_BADINDEX);
    }
    let at = header.data.wrapping_add(number * size);
    Ok((at, elements_of(header.features), size))
}

/// `SafeArrayCreate`.
///
/// # Safety
/// `bounds` must be null or hold `dimensions` bounds, leftmost dimension first.
pub unsafe extern "efiapi" fn safe_array_create(
    element_kind: u16,
    dimensions: u32,
    bounds: *const Bound,
) -> *mut SafeArray {
    if bounds.is_null() || dimensions == 0 {
        return ptr::null_mut();
    }
    // SAFETY: the caller's duty.
    let bounds = unsafe { core::slice::from_raw_parts(bounds, dimensions as usize) };
    create(element_kind, bounds)
}

/// `SafeArrayDestroy`.
///
/// # Safety
/// `array` must be null or a valid descriptor that nothing uses afterwards.
pub unsafe extern "efiapi" fn safe_array_destroy(array: *mut SafeArray) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { destroy(array) }
}

/// `SafeArrayGetDim`: the dimension count; 0 for null.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe extern "efiapi" fn safe_array_get_dim(array: *const SafeArray) -> u32 {
    // SAFETY: the caller's duty.
    let array = unsafe { array.as_ref() };
    array.map_or(0, |array| u32::from(array.dimensions))
}

/// `SafeArrayGetElemsize`: an element's size in bytes; 0 for null.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe extern "efiapi" fn safe_array_get_elemsize(array: *const SafeArray) -> u32 {
    // SAFETY: the caller's duty.
    let array = unsafe { array.as_ref() };
    array.map_or(0, |array| array.element_size)
}

unsafe fn write_bound(
    array: *const SafeArray,
    dimension: u32,
    result: *mut i32,
    pick: fn(Bound) -> i32,
) -> HResult {
    if array.is_null() || result.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let stored = unsafe { stored_bounds(array) };
    let dimension = dimension as usize;
    if dimension == 0 || dimension > stored.len() {
        return DISP_E_BADINDEX;
    }
    let bound = stored[stored.len() - dimension];
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(pick(bound)) };
    S_OK
}

/// `SafeArrayGetUBound`: the highest index of `dimension` (1 is the leftmost).
///
/// # Safety
/// `array` must be null or a valid descriptor; `result` null or writable.
pub unsafe extern "efiapi" fn safe_array_get_ubound(
    array: *const SafeArray,
    dimension: u32,
    result: *mut i32,
) -> HResult {
    let upper = |bound: Bound| (i64::from(bound.lower) + i64::from(bound.elements) - 1) as i32;
    // SAFETY: the caller's duty.
    unsafe { write_bound(array, dimension, result, upper) }
}

/// `SafeArrayGetLBound`: the lowest index of `dimension` (1 is the leftmost).
///
/// # Safety
/// `array` must be null or a valid descriptor; `result` null or writable.
pub unsafe extern "efiapi" fn safe_array_get_lbound(
    array: *const SafeArray,
    dimension: u32,
    result: *mut i32,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { write_bound(array, dimension, result, |bound| bound.lower) }
}

/// `SafeArrayLock`: one more lock; an array holds at most 65535.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe extern "efiapi" fn safe_array_lock(array: *mut SafeArray) -> HResult {
    // SAFETY: the caller's duty.
    let Some(array) = (unsafe { array.as_mut() }) else {
        return E_INVALIDARG;
    };
    if array.locks >= MOST_LOCKS {
        return E_UNEXPECTED;
    }
    array.locks += 1;
    S_OK
}

/// `SafeArrayUnlock`: one lock fewer; `E_UNEXPECTED` when it holds none.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe extern "efiapi" fn safe_array_unlock(array: *mut SafeArray) -> HResult {
    // SAFETY: the caller's duty.
    let Some(array) = (unsafe { array.as_mut() }) else {
        return E_INVALIDARG;
    };
    if array.locks == 0 {
        return E_UNEXPECTED;
    }
    array.locks -= 1;
    S_OK
}

/// `SafeArrayAccessData`: locks the array and gives its data's address.
///
/// # Safety
/// `array` must be null or a valid descriptor; `data` null or writable.
pub unsafe extern "efiapi" fn safe_array_access_data(
    array: *mut SafeArray,
    data: *mut *mut u8,
) -> HResult {
    if data.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let locked = unsafe { safe_array_lock(array) };
    if locked < 0 {
        return locked;
    }
    // SAFETY: the caller's duty: the array is valid, as locking showed.
    let address = unsafe { descriptor(array) }.data;
    // SAFETY: the caller's duty.
    unsafe { data.write_unaligned(address) };
    S_OK
}

/// `SafeArrayUnaccessData`: the lock `SafeArrayAccessData` took goes.
///
/// # Safety
/// `array` must be null or a valid descriptor.
pub unsafe extern "efiapi" fn safe_array_unaccess_data(array: *mut SafeArray) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { safe_array_unlock(array) }
}

/// `SafeArrayPtrOfIndex`: the address of the element `indices` name (leftmost dimension first).
///
/// # Safety
/// `array` must be null or a valid descriptor; `indices` null or one index per dimension;
/// `result` null or writable.
pub unsafe extern "efiapi" fn safe_array_ptr_of_index(
    array: *mut SafeArray,
    indices: *const i32,
    result: *mut *mut u8,
) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    match unsafe { element_at(array, indices) } {
        Ok((at, _, _)) => {
            // SAFETY: the caller's duty.
            unsafe { result.write_unaligned(at) };
            S_OK
        }
        Err(code) => code,
    }
}

/// `SafeArrayGetElement`: a copy of an element in `*value` (a new BSTR, a new reference, a deep
/// copy of a VARIANT).
///
/// # Safety
/// `array` must be null or a valid descriptor; `indices` null or one index per dimension;
/// `value` null or room for one element.
pub unsafe extern "efiapi" fn safe_array_get_element(
    array: *mut SafeArray,
    indices: *const i32,
    value: *mut u8,
) -> HResult {
    if value.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    match unsafe { element_at(array, indices) } {
        // SAFETY: `at` is the element; `value` the caller's room for one.
        Ok((at, elements, size)) => unsafe { copy_element(elements, at, value, size) },
        Err(code) => code,
    }
}

/// `SafeArrayPutElement`: stores a copy of `value` in an element, giving back what it held. For
/// arrays of BSTRs and objects `value` is the string or interface pointer itself; otherwise it
/// points at the value.
///
/// # Safety
/// `array` must be null or a valid descriptor; `indices` null or one index per dimension;
/// `value` as above.
pub unsafe extern "efiapi" fn safe_array_put_element(
    array: *mut SafeArray,
    indices: *const i32,
    value: *const u8,
) -> HResult {
    // SAFETY: the caller's duty.
    let (at, elements, size) = match unsafe { element_at(array, indices) } {
        Ok(found) => found,
        Err(code) => return code,
    };
    let mut staged = [0u8; LARGEST_ELEMENT];
    let source = match elements {
        Elements::Strings | Elements::Objects => {
            let pointer = (value as usize).to_ne_bytes();
            staged[..pointer.len()].copy_from_slice(&pointer);
            staged.as_ptr()
        }
        _ if value.is_null() => return E_INVALIDARG,
        _ => value,
    };
    if elements == Elements::Plain {
        // SAFETY: `source` holds one element; `at` is the element.
        return unsafe { copy_element(elements, source, at, size) };
    }
    let size = match elements {
        Elements::Variants => core::mem::size_of::<Variant>(),
        _ => core::mem::size_of::<usize>(),
    };
    let mut copied = [0u8; LARGEST_ELEMENT];
    // SAFETY: `source` holds the value, `copied` room for one element.
    let result = unsafe { copy_element(elements, source, copied.as_mut_ptr(), size) };
    if result < 0 {
        return result;
    }
    // SAFETY: the element is the array's, which gives up what it held.
    unsafe { release_element(elements, at) };
    // SAFETY: both hold one element of `size` bytes.
    unsafe { ptr::copy_nonoverlapping(copied.as_ptr(), at, size) };
    S_OK
}

/// `SafeArrayCopy`.
///
/// # Safety
/// `array` must be null or a valid descriptor; `result` null or writable.
pub unsafe extern "efiapi" fn safe_array_copy(
    array: *const SafeArray,
    result: *mut *mut SafeArray,
) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let (code, copy) = match unsafe { copy(array) } {
        Ok(copy) => (S_OK, copy),
        Err(code) => (code, ptr::null_mut()),
    };
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(copy) };
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(elements: u32, lower: i32) -> Bound {
        Bound { elements, lower }
    }

    #[test]
    fn indices_name_elements_column_major() {
        let stored = [bound(3, 0), bound(2, 1)];
        assert_eq!(element_number(&stored, &[1, 0]), Some(0));
        assert_eq!(element_number(&stored, &[2, 0]), Some(1));
        assert_eq!(element_number(&stored, &[1, 2]), Some(4));
        assert_eq!(element_number(&stored, &[2, 2]), Some(5));
        assert_eq!(element_number(&stored, &[3, 0]), None);
        assert_eq!(element_number(&stored, &[0, 0]), None);
        assert_eq!(element_number(&stored, &[1, 3]), None);
        assert_eq!(element_number(&stored, &[1]), None);
    }

    #[test]
    fn bounds_are_reported_per_dimension() {
        let bounds = [bound(2, 1), bound(3, -1)];
        // SAFETY: two bounds.
        let array = unsafe { safe_array_create(kind::I4, 2, bounds.as_ptr()) };
        assert!(!array.is_null());
        // SAFETY: a valid descriptor from here on.
        assert_eq!(unsafe { safe_array_get_dim(array) }, 2);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_get_elemsize(array) }, 4);
        let mut found = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_get_lbound(array, 1, &mut found) }, S_OK);
        assert_eq!(found, 1);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_get_ubound(array, 1, &mut found) }, S_OK);
        assert_eq!(found, 2);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_get_lbound(array, 2, &mut found) }, S_OK);
        assert_eq!(found, -1);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_get_ubound(array, 2, &mut found) }, S_OK);
        assert_eq!(found, 1);
        // SAFETY: as above.
        let outside = unsafe { safe_array_get_ubound(array, 3, &mut found) };
        assert_eq!(outside, DISP_E_BADINDEX);
        // SAFETY: as above.
        assert_eq!(unsafe { stored_bounds(array) }, &[bounds[1], bounds[0]]);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_destroy(array) }, S_OK);
    }

    #[test]
    fn elements_are_put_read_and_addressed() {
        let array = create(kind::I4, &[bound(2, 0), bound(2, 0)]);
        let value = 42i32;
        let value_at = (&raw const value).cast::<u8>();
        let second = [1, 0];
        // SAFETY: a valid descriptor and two indices from here on.
        let put = unsafe { safe_array_put_element(array, second.as_ptr(), value_at) };
        assert_eq!(put, S_OK);
        let mut data = ptr::null_mut();
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_access_data(array, &mut data) }, S_OK);
        // SAFETY: the data holds four `i32`s.
        let numbers = unsafe { core::slice::from_raw_parts(data.cast::<i32>(), 4) };
        assert_eq!(numbers, &[0, 42, 0, 0]);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_destroy(array) }, DISP_E_ARRAYISLOCKED);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_unaccess_data(array) }, S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_unlock(array) }, E_UNEXPECTED);
        let mut address = ptr::null_mut();
        let last = [1, 1];
        // SAFETY: as above.
        let found = unsafe { safe_array_ptr_of_index(array, last.as_ptr(), &mut address) };
        assert_eq!(found, S_OK);
        assert_eq!(address, data.wrapping_add(12));
        let mut read = 0i32;
        let read_at = (&raw mut read).cast::<u8>();
        // SAFETY: as above.
        let got = unsafe { safe_array_get_element(array, second.as_ptr(), read_at) };
        assert_eq!((got, read), (S_OK, 42));
        let outside = [2, 0];
        // SAFETY: as above.
        let refused = unsafe { safe_array_get_element(array, outside.as_ptr(), read_at) };
        assert_eq!(refused, DISP_E_BADINDEX);
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_destroy(array) }, S_OK);
    }

    #[test]
    fn arrays_of_strings_and_variants_copy_deeply() {
        let one_dimension = [bound(2, 0)];
        let strings = create(kind::BSTR, &one_dimension);
        let text = bstr::from_ascii(b"Cherries");
        let second = [1];
        // SAFETY: a valid descriptor; for BSTR arrays the value is the string itself.
        let put = unsafe { safe_array_put_element(strings, second.as_ptr(), text.cast()) };
        assert_eq!(put, S_OK);
        // SAFETY: the string was copied into the array.
        unsafe { bstr::free(text) };
        let mut copy = ptr::null_mut();
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_copy(strings, &mut copy) }, S_OK);
        let mut read: *mut u16 = ptr::null_mut();
        let read_at = (&raw mut read).cast::<u8>();
        // SAFETY: the copy is a valid descriptor.
        let got = unsafe { safe_array_get_element(copy, second.as_ptr(), read_at) };
        assert_eq!(got, S_OK);
        // SAFETY: a BSTR.
        assert_eq!(unsafe { bstr::units(read) }.len(), 8);
        // SAFETY: the element read is the caller's.
        unsafe { bstr::free(read) };
        for array in [strings, copy] {
            // SAFETY: each is a valid descriptor, unused afterwards.
            assert_eq!(unsafe { safe_array_destroy(array) }, S_OK);
        }
        let values = create(kind::VARIANT, &one_dimension);
        // SAFETY: an array of VARIANTs.
        let elements = unsafe { variants(values) };
        assert_eq!(elements.len(), 2);
        elements[0] = Variant::long(5);
        let mut value = Variant::EMPTY;
        let value_at = (&raw mut value).cast::<u8>();
        let first = [0];
        // SAFETY: a valid descriptor.
        let got = unsafe { safe_array_get_element(values, first.as_ptr(), value_at) };
        assert_eq!((got, value), (S_OK, Variant::long(5)));
        // SAFETY: as above.
        assert_eq!(unsafe { safe_array_destroy(values) }, S_OK);
    }

    #[test]
    fn kinds_no_array_holds_are_refused() {
        let one = [bound(1, 0)];
        assert!(create(kind::EMPTY, &one).is_null());
        assert!(create(kind::NULL, &one).is_null());
        assert!(create(kind::I4, &[]).is_null());
        let empty = create(kind::I2, &[bound(0, 0)]);
        assert!(!empty.is_null());
        // SAFETY: a valid descriptor.
        assert_eq!(unsafe { safe_array_destroy(empty) }, S_OK);
    }
}
