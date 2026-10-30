//! An `IEnumVARIANT` over a list of values taken when it was made (a dictionary's keys, for
//! `_NewEnum`), held as a one-dimensional SAFEARRAY of VARIANTs: `Next` hands out copies,
//! `Skip`, `Reset`, and `Clone` gives an enumerator at the same place over its own copy.

use super::guid::{self, Guid};
use super::safe_array::{self, SafeArray};
use super::variant::{self, Variant};
use super::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_FALSE, S_OK};
use core::sync::atomic::AtomicU32;

/// An enumerator: its table, its reference count, its values and where it is.
#[repr(C)]
#[derive(Debug)]
pub struct Enumerator {
    table: usize,
    references: AtomicU32,
    position: u32,
    values: *mut SafeArray,
}

/// A new enumerator with `table` over `values` (an array of VARIANTs it now owns), at
/// `position`; null without memory, and the values are then given back.
///
/// # Safety
/// `values` must be a one-dimensional array of VARIANTs that nothing else uses.
pub unsafe fn create(table: usize, values: *mut SafeArray, position: u32) -> *mut Enumerator {
    let object = super::allocate(core::mem::size_of::<Enumerator>()).cast::<Enumerator>();
    if object.is_null() {
        // SAFETY: the caller's duty: the values were handed over.
        unsafe { safe_array::destroy(values) };
        return object;
    }
    let enumerator = Enumerator {
        table,
        references: AtomicU32::new(1),
        position,
        values,
    };
    // SAFETY: a fresh, aligned block for one enumerator.
    unsafe { object.write(enumerator) };
    object
}

unsafe fn values<'a>(this: &Enumerator) -> &'a [Variant] {
    // SAFETY: the caller's duty: the enumerator owns its array of VARIANTs.
    unsafe { safe_array::variants(this.values) }
}

/// `IUnknown::QueryInterface`: `IUnknown` and `IEnumVARIANT`.
///
/// # Safety
/// `this` must be a live enumerator; the others as `QueryInterface` says.
pub unsafe extern "efiapi" fn query_interface(
    this: *mut Enumerator,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    let interfaces = [guid::UNKNOWN, guid::ENUM_VARIANT];
    // SAFETY: the caller's duty.
    unsafe { super::answer_query(this as usize, references, interface, result, &interfaces) }
}

/// `IUnknown::AddRef`.
///
/// # Safety
/// `this` must be a live enumerator.
pub unsafe extern "efiapi" fn add_reference(this: *mut Enumerator) -> u32 {
    // SAFETY: the caller's duty.
    let references = unsafe { &(*this).references };
    references.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1
}

/// `IUnknown::Release`: the last reference gives back the values and the enumerator.
///
/// # Safety
/// `this` must be a live enumerator the caller holds a reference to.
pub unsafe extern "efiapi" fn release(this: *mut Enumerator) -> u32 {
    // SAFETY: the caller's duty.
    let (left, last) = super::drop_reference(unsafe { &(*this).references });
    if last {
        // SAFETY: no reference is left: the enumerator and its values go.
        let values = unsafe { (*this).values };
        // SAFETY: as above.
        unsafe { safe_array::destroy(values) };
        // SAFETY: as above.
        unsafe { super::free(this.cast()) };
    }
    left
}

/// `IEnumVARIANT::Next`: copies of up to `count` values into `values`; `S_FALSE` when fewer
/// were left.
///
/// # Safety
/// `this` must be a live enumerator, `values` room for `count` VARIANTs, `fetched` null or
/// writable.
pub unsafe extern "efiapi" fn next(
    this: *mut Enumerator,
    count: u32,
    values: *mut Variant,
    fetched: *mut u32,
) -> HResult {
    if values.is_null() && count != 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let enumerator = unsafe { &mut *this };
    // SAFETY: as above.
    let all = unsafe { self::values(enumerator) };
    let start = (enumerator.position as usize).min(all.len());
    let taken = (count as usize).min(all.len() - start);
    for (index, value) in all[start..start + taken].iter().enumerate() {
        // SAFETY: the enumerator's values are valid VARIANTs.
        let copy = match unsafe { variant::duplicate(value) } {
            Ok(copy) => copy,
            Err(code) => return code,
        };
        // SAFETY: the caller's duty: room for `count` VARIANTs.
        unsafe { values.wrapping_add(index).write_unaligned(copy) };
    }
    enumerator.position = (start + taken) as u32;
    if !fetched.is_null() {
        // SAFETY: the caller's duty.
        unsafe { fetched.write_unaligned(taken as u32) };
    }
    if taken == count as usize {
        S_OK
    } else {
        S_FALSE
    }
}

/// `IEnumVARIANT::Skip`: passes `count` values; `S_FALSE` when fewer were left.
///
/// # Safety
/// `this` must be a live enumerator.
pub unsafe extern "efiapi" fn skip(this: *mut Enumerator, count: u32) -> HResult {
    // SAFETY: the caller's duty.
    let enumerator = unsafe { &mut *this };
    // SAFETY: as above.
    let total = unsafe { values(enumerator) }.len();
    let wanted = enumerator.position as usize + count as usize;
    enumerator.position = wanted.min(total) as u32;
    if wanted <= total { S_OK } else { S_FALSE }
}

/// `IEnumVARIANT::Reset`: back to the first value.
///
/// # Safety
/// `this` must be a live enumerator.
pub unsafe extern "efiapi" fn reset(this: *mut Enumerator) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { (*this).position = 0 };
    S_OK
}

/// `IEnumVARIANT::Clone`: a new enumerator at the same place over a copy of the values.
///
/// # Safety
/// `this` must be a live enumerator; `result` null or writable.
pub unsafe extern "efiapi" fn clone(this: *mut Enumerator, result: *mut usize) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    let enumerator = unsafe { &*this };
    // SAFETY: the enumerator owns a valid array.
    let copy = match unsafe { safe_array::copy(enumerator.values) } {
        Ok(copy) => copy,
        Err(code) => return code,
    };
    // SAFETY: the copy is this call's, handed to the new enumerator.
    let clone = unsafe { create(enumerator.table, copy, enumerator.position) };
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(clone as usize) };
    if clone.is_null() { E_OUTOFMEMORY } else { S_OK }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::safe_array::Bound;
    use crate::com::variant::kind;

    fn three_values() -> *mut Enumerator {
        let bound = [Bound {
            elements: 3,
            lower: 0,
        }];
        let array = safe_array::create(kind::VARIANT, &bound);
        // SAFETY: a new array of VARIANTs.
        let elements = unsafe { safe_array::variants(array) };
        for (index, element) in elements.iter_mut().enumerate() {
            *element = Variant::long(index as i32 + 1);
        }
        // SAFETY: the array is handed over.
        unsafe { create(0, array, 0) }
    }

    #[test]
    fn next_hands_out_copies_until_none_are_left() {
        let enumerator = three_values();
        let mut values = [Variant::EMPTY; 2];
        let mut fetched = 0;
        // SAFETY: a live enumerator and room for two values from here on.
        let code = unsafe { next(enumerator, 2, values.as_mut_ptr(), &mut fetched) };
        assert_eq!((code, fetched), (S_OK, 2));
        assert_eq!(values, [Variant::long(1), Variant::long(2)]);
        // SAFETY: as above.
        let code = unsafe { next(enumerator, 2, values.as_mut_ptr(), &mut fetched) };
        assert_eq!((code, fetched), (S_FALSE, 1));
        assert_eq!(values[0], Variant::long(3));
        // SAFETY: as above.
        assert_eq!(unsafe { reset(enumerator) }, S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { skip(enumerator, 2) }, S_OK);
        let mut copy = 0usize;
        // SAFETY: as above.
        assert_eq!(unsafe { clone(enumerator, &mut copy) }, S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { skip(enumerator, 2) }, S_FALSE);
        let copy = copy as *mut Enumerator;
        // SAFETY: the clone is a live enumerator at the third value.
        let code = unsafe { next(copy, 1, values.as_mut_ptr(), core::ptr::null_mut()) };
        assert_eq!((code, values[0]), (S_OK, Variant::long(3)));
        for object in [enumerator, copy] {
            // SAFETY: the last reference to each.
            assert_eq!(unsafe { release(object) }, 0);
        }
    }

    #[test]
    fn it_answers_only_to_its_interfaces() {
        let enumerator = three_values();
        let mut answer = 0usize;
        // SAFETY: a live enumerator.
        let code = unsafe { query_interface(enumerator, &guid::ENUM_VARIANT, &mut answer) };
        assert_eq!((code, answer), (S_OK, enumerator as usize));
        // SAFETY: as above.
        let code = unsafe { query_interface(enumerator, &guid::DISPATCH, &mut answer) };
        assert_eq!((code, answer), (super::super::E_NOINTERFACE, 0));
        // SAFETY: as above: two references now.
        assert_eq!(unsafe { release(enumerator) }, 1);
        // SAFETY: the last one.
        assert_eq!(unsafe { release(enumerator) }, 0);
    }
}
