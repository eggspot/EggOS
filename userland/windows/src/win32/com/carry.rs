//! VARIANTs turned into the values that travel between a program and the server of a class it
//! uses out of process, and back (D400, D743), shared by both ends: numbers, decimals, texts,
//! objects (each end says how its objects travel) and arrays of any of them, nested through
//! VARIANT elements; and the arguments passed by reference, which the serving end holds in
//! cells of its own and the using end writes back into the program's places.

use crate::com::safe_array::{self, Bound, SafeArray};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Value};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, HResult, bstr};
use alloc::vec::Vec;

const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
const DECIMAL_BYTES: usize = 16;
const WORD_BYTES: usize = 8;

/// How one end's objects travel.
pub(super) trait Objects {
    /// The value a live, non-null object of `kind` travels as.
    fn object_value(&mut self, kind: u16, object: usize) -> Result<Value, HResult>;
    /// The object, with a reference for its new holder, that a non-null object or peer
    /// object value stands for.
    fn object_of(&mut self, value: &Value) -> Result<usize, HResult>;
}

fn read_word(at: *const u8, size: usize) -> u64 {
    let mut bytes = [0u8; WORD_BYTES];
    let taken = size.min(WORD_BYTES);
    // SAFETY: the callers pass an element of `size` readable bytes.
    unsafe { core::ptr::copy_nonoverlapping(at, bytes.as_mut_ptr(), taken) };
    u64::from_le_bytes(bytes)
}

fn element_value(
    objects: &mut impl Objects,
    element: u16,
    at: *const u8,
    size: usize,
    depth: usize,
) -> Result<Value, HResult> {
    Ok(match element {
        kind::VARIANT => {
            // SAFETY: an array of VARIANTs holds one here.
            let held = unsafe { at.cast::<Variant>().read_unaligned() };
            nested_value(objects, &held, depth + 1)?
        }
        kind::BSTR => match read_word(at, WORD_BYTES) {
            0 => Value::Text(None),
            // SAFETY: an array of BSTRs holds a live one here.
            text => Value::Text(Some(unsafe { bstr::units(text as *const u16) }.to_vec())),
        },
        kind::DISPATCH | kind::UNKNOWN => match read_word(at, WORD_BYTES) {
            0 => Value::Object {
                kind: element,
                id: 0,
            },
            object => objects.object_value(element, object as usize)?,
        },
        kind::DECIMAL => {
            let mut bytes = [0u8; DECIMAL_BYTES];
            // SAFETY: an array of DECIMALs holds sixteen bytes here.
            unsafe { core::ptr::copy_nonoverlapping(at, bytes.as_mut_ptr(), DECIMAL_BYTES) };
            Value::Decimal(bytes)
        }
        other => Value::Plain {
            kind: other,
            word: read_word(at, size),
        },
    })
}

fn array_value(
    objects: &mut impl Objects,
    array_kind: u16,
    array: usize,
    depth: usize,
) -> Result<Value, HResult> {
    if array == 0 {
        return Ok(Value::Plain {
            kind: array_kind,
            word: 0,
        });
    }
    if depth >= wire::MOST_NESTING {
        return Err(DISP_E_TYPEMISMATCH);
    }
    // SAFETY: the VARIANT holds a live SAFEARRAY.
    let (element, bounds, data, size) = unsafe { safe_array::shape(array as *const SafeArray) };
    let total = bounds
        .iter()
        .try_fold(1usize, |product, bound| {
            product.checked_mul(bound.elements as usize)
        })
        .filter(|&total| total <= wire::MOST_ELEMENTS && bounds.len() <= wire::MOST_DIMENSIONS)
        .ok_or(DISP_E_TYPEMISMATCH)?;
    if total > 0 && data.is_null() {
        return Err(E_INVALIDARG);
    }
    let items = (0..total)
        .map(|index| {
            let at = data.wrapping_add(index * size);
            element_value(objects, element, at, size, depth)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Array {
        element,
        bounds: bounds
            .iter()
            .map(|bound| (bound.elements, bound.lower))
            .collect(),
        items,
    })
}

fn nested_value(
    objects: &mut impl Objects,
    held: &Variant,
    depth: usize,
) -> Result<Value, HResult> {
    // SAFETY: a valid VARIANT of the program's.
    let held = unsafe { variant::view(held) }?;
    if held.kind & kind::ARRAY != 0 {
        return array_value(objects, held.kind, held.pointer(), depth);
    }
    Ok(match held.kind {
        kind::DECIMAL => Value::Decimal(variant::decimal_bytes(&held)),
        kind::BSTR => match held.pointer() {
            0 => Value::Text(None),
            // SAFETY: the VARIANT holds a live BSTR.
            text => Value::Text(Some(unsafe { bstr::units(text as *const u16) }.to_vec())),
        },
        kind::DISPATCH | kind::UNKNOWN => match held.pointer() {
            0 => Value::Object {
                kind: held.kind,
                id: 0,
            },
            object => objects.object_value(held.kind, object)?,
        },
        other => Value::Plain {
            kind: other,
            word: held.data[0],
        },
    })
}

/// The value the VARIANT `held` (or, by reference, the value it points at) travels as.
pub(super) fn value_of(objects: &mut impl Objects, held: &Variant) -> Result<Value, HResult> {
    nested_value(objects, held, 0)
}

fn array_variant(
    objects: &mut impl Objects,
    (element, bounds, items): (u16, &[(u32, i32)], &[Value]),
    depth: usize,
) -> Result<Variant, HResult> {
    if depth >= wire::MOST_NESTING {
        return Err(DISP_E_TYPEMISMATCH);
    }
    let bounds: Vec<Bound> = bounds
        .iter()
        .map(|&(elements, lower)| Bound { elements, lower })
        .collect();
    let array = safe_array::create(element, &bounds);
    if array.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the array just made, which nothing else holds.
    let (_, _, data, size) = unsafe { safe_array::shape(array) };
    for (index, item) in items.iter().enumerate() {
        let made = nested_variant(objects, item, depth + 1);
        let value = match made {
            Ok(value) => value,
            Err(status) => {
                // SAFETY: the array made above, with the elements written so far.
                unsafe { safe_array::destroy(array) };
                return Err(status);
            }
        };
        let at = data.wrapping_add(index * size);
        // SAFETY: element `index` of the new array, `size` bytes, which now owns the value.
        unsafe {
            match element {
                kind::VARIANT => at.cast::<Variant>().write_unaligned(value),
                kind::DECIMAL => {
                    let bytes = variant::decimal_bytes(&value);
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), at, DECIMAL_BYTES);
                }
                _ => {
                    let bytes = value.data[0].to_le_bytes();
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), at, size.min(WORD_BYTES));
                }
            }
        }
    }
    Ok(Variant::new(kind::ARRAY | element, array as u64))
}

fn nested_variant(
    objects: &mut impl Objects,
    value: &Value,
    depth: usize,
) -> Result<Variant, HResult> {
    Ok(match value {
        Value::Plain { kind, word } => Variant::new(*kind, *word),
        Value::Decimal(bytes) => variant::decimal_variant(bytes),
        Value::Text(None) => Variant::new(kind::BSTR, 0),
        Value::Text(Some(units)) => {
            let text = bstr::from_units(units);
            if text.is_null() {
                return Err(E_OUTOFMEMORY);
            }
            Variant::new(kind::BSTR, text as u64)
        }
        Value::Object { kind, id: 0 } | Value::Peer { kind, id: 0 } => Variant::new(*kind, 0),
        Value::Object { kind, .. } | Value::Peer { kind, .. } => {
            Variant::new(*kind, objects.object_of(value)? as u64)
        }
        Value::Array {
            element,
            bounds,
            items,
        } => array_variant(objects, (*element, bounds, items), depth)?,
    })
}

/// A VARIANT, owning what it holds, for the travelled `value`.
pub(super) fn variant_of(objects: &mut impl Objects, value: &Value) -> Result<Variant, HResult> {
    nested_variant(objects, value, 0)
}

/// The place a `VT_BYREF` VARIANT of base kind `base` points at in `cell`, a VARIANT that
/// holds the value: the cell itself for VARIANTs and decimals, else its value's bytes.
pub(super) fn cell_place(base: u16, cell: &mut Variant) -> usize {
    match base {
        kind::VARIANT | kind::DECIMAL => cell as *mut Variant as usize,
        _ => cell.data.as_mut_ptr() as usize,
    }
}

/// The value `cell` holds after a call that may have changed it through [`cell_place`].
pub(super) fn cell_value(base: u16, cell: &Variant) -> Variant {
    match base {
        kind::VARIANT => *cell,
        kind::DECIMAL => variant::decimal_variant(&variant::decimal_bytes(cell)),
        _ => Variant::new(base, cell.data[0]),
    }
}

/// Writes `value` into the program's place `target` that a `VT_BYREF` of base kind `base`
/// points at, giving back what was there.
///
/// # Safety
/// `target` must be the program's writable place of a value of `base`.
pub(super) unsafe fn write_back(target: usize, base: u16, value: Variant) {
    if base == kind::VARIANT {
        // SAFETY: the caller's duty: a VARIANT the program owns.
        unsafe { variant::replace(target as *mut Variant, value) };
        return;
    }
    if base == kind::DECIMAL {
        let bytes = variant::decimal_bytes(&value);
        let place = target as *mut u8;
        // SAFETY: the caller's duty: sixteen writable bytes.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), place, DECIMAL_BYTES) };
        return;
    }
    let size = match base & kind::ARRAY {
        0 => variant::value_size(base).unwrap_or(WORD_BYTES),
        _ => WORD_BYTES,
    };
    let old = read_word(target as *const u8, size);
    // SAFETY: the caller's duty: the place held a value of `base`, which it owned.
    unsafe {
        if base & kind::ARRAY != 0 {
            safe_array::destroy(old as *mut SafeArray);
        } else if base == kind::BSTR {
            bstr::free(old as *mut u16);
        } else if base == kind::DISPATCH || base == kind::UNKNOWN {
            crate::com::release(old as usize);
        }
        let bytes = value.data[0].to_le_bytes();
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, size.min(WORD_BYTES));
    }
}
