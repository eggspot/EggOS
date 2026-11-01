//! VARIANTs, Automation's tagged values: a 16-bit kind (`VARTYPE`), three reserved words and
//! the value, 24 bytes on 64-bit Windows. `VariantInit`, `VariantClear`, `VariantCopy` and
//! `VariantCopyInd`, and how a value's resources (its string, its reference to an object, its
//! array) are copied and given back. A `VT_BYREF` value points at a value of its kind
//! elsewhere, which it does not own.

use super::safe_array::{self, SafeArray};
use super::{E_INVALIDARG, E_OUTOFMEMORY, HResult, S_OK, bstr};

/// The kinds (`VARTYPE`s) of values.
pub mod kind {
    pub const EMPTY: u16 = 0;
    pub const NULL: u16 = 1;
    pub const I2: u16 = 2;
    pub const I4: u16 = 3;
    pub const R4: u16 = 4;
    pub const R8: u16 = 5;
    pub const CY: u16 = 6;
    pub const DATE: u16 = 7;
    pub const BSTR: u16 = 8;
    pub const DISPATCH: u16 = 9;
    pub const ERROR: u16 = 10;
    pub const BOOL: u16 = 11;
    pub const VARIANT: u16 = 12;
    pub const UNKNOWN: u16 = 13;
    pub const DECIMAL: u16 = 14;
    pub const I1: u16 = 16;
    pub const UI1: u16 = 17;
    pub const UI2: u16 = 18;
    pub const UI4: u16 = 19;
    pub const I8: u16 = 20;
    pub const UI8: u16 = 21;
    pub const INT: u16 = 22;
    pub const UINT: u16 = 23;
    pub const ARRAY: u16 = 0x2000;
    pub const BYREF: u16 = 0x4000;
    pub const TYPE_MASK: u16 = 0x0fff;
}

/// `DISP_E_BADVARTYPE`: a kind no VARIANT has.
pub const DISP_E_BADVARTYPE: HResult = 0x8002_0008_u32 as i32;

const DECIMAL_SIZE: usize = 16;

/// A VARIANT as Windows lays it out on 64-bit CPUs: the value is in `data`, from its first
/// byte.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Variant {
    pub kind: u16,
    pub reserved: [u16; 3],
    pub data: [u64; 2],
}

const _: () = assert!(core::mem::size_of::<Variant>() == 24);

impl Variant {
    /// `VT_EMPTY`.
    pub const EMPTY: Self = Self::new(kind::EMPTY, 0);

    /// A value of `kind` whose first eight bytes are `word`.
    pub const fn new(kind: u16, word: u64) -> Self {
        Self {
            kind,
            reserved: [0; 3],
            data: [word, 0],
        }
    }

    /// A `VT_BOOL`: `VARIANT_TRUE` (-1) or `VARIANT_FALSE` (0).
    pub const fn boolean(value: bool) -> Self {
        Self::new(kind::BOOL, if value { 0xffff } else { 0 })
    }

    /// A `VT_I4`.
    pub const fn long(value: i32) -> Self {
        Self::new(kind::I4, value as u32 as u64)
    }

    /// The value's pointer: a BSTR's, an object's, an array's or a `VT_BYREF`'s target.
    pub const fn pointer(&self) -> usize {
        self.data[0] as usize
    }
}

/// How many bytes a value of kind `base` (no `VT_ARRAY` or `VT_BYREF`) takes, for the kinds an
/// array or a reference may hold.
pub fn value_size(base: u16) -> Option<usize> {
    Some(match base {
        kind::I1 | kind::UI1 => 1,
        kind::I2 | kind::UI2 | kind::BOOL => 2,
        kind::I4 | kind::UI4 | kind::INT | kind::UINT | kind::R4 | kind::ERROR => 4,
        kind::I8 | kind::UI8 | kind::R8 | kind::CY | kind::DATE => 8,
        kind::BSTR | kind::DISPATCH | kind::UNKNOWN => core::mem::size_of::<usize>(),
        kind::DECIMAL => DECIMAL_SIZE,
        kind::VARIANT => core::mem::size_of::<Variant>(),
        _ => return None,
    })
}

/// Whether a VARIANT may have `value_kind`.
pub fn is_valid(value_kind: u16) -> bool {
    let base = value_kind & kind::TYPE_MASK;
    let flags = value_kind & !kind::TYPE_MASK;
    if flags & !(kind::ARRAY | kind::BYREF) != 0 {
        return false;
    }
    match base {
        kind::EMPTY | kind::NULL => flags == 0,
        kind::VARIANT => flags != 0,
        _ => value_size(base).is_some(),
    }
}

/// Gives back what `value` owns: its string, its reference to an object or its array. A
/// `VT_BYREF` owns nothing.
///
/// # Safety
/// `value` must be a valid VARIANT whose resources the caller gives up.
pub unsafe fn release(value: &Variant) -> HResult {
    if value.kind & kind::BYREF != 0 {
        return S_OK;
    }
    if value.kind & kind::ARRAY != 0 {
        if value.pointer() == 0 {
            return S_OK;
        }
        // SAFETY: the caller's duty: the value owns its array.
        return unsafe { safe_array::destroy(value.pointer() as *mut SafeArray) };
    }
    match value.kind {
        // SAFETY: the caller's duty: the value owns its string.
        kind::BSTR => unsafe { bstr::free(value.pointer() as *mut u16) },
        // SAFETY: the caller's duty: the value holds a reference.
        kind::DISPATCH | kind::UNKNOWN => unsafe { super::release(value.pointer()) },
        _ => {}
    }
    S_OK
}

/// A deep copy of `source`: its own string, a new reference to the object, its own array. A
/// `VT_BYREF` is copied as the reference it is.
///
/// # Safety
/// `source` must be a valid VARIANT.
pub unsafe fn duplicate(source: &Variant) -> Result<Variant, HResult> {
    if !is_valid(source.kind) {
        return Err(DISP_E_BADVARTYPE);
    }
    let mut copy = *source;
    if source.kind & kind::BYREF != 0 {
        return Ok(copy);
    }
    if source.kind & kind::ARRAY != 0 {
        if source.pointer() != 0 {
            // SAFETY: the caller's duty: the value holds an array.
            let array = unsafe { safe_array::copy(source.pointer() as *const SafeArray) }?;
            copy.data[0] = array as u64;
        }
        return Ok(copy);
    }
    match source.kind {
        kind::BSTR => {
            // SAFETY: the caller's duty: the value holds a BSTR.
            let text = unsafe { bstr::duplicate(source.pointer() as *const u16) };
            copy.data[0] = text.ok_or(E_OUTOFMEMORY)? as u64;
        }
        // SAFETY: the caller's duty: the value holds an object.
        kind::DISPATCH | kind::UNKNOWN => unsafe { super::add_reference(source.pointer()) },
        _ => {}
    }
    Ok(copy)
}

/// The value `source` stands for, without copying anything: itself, or for a `VT_BYREF` the
/// value it points at (one level; a reference to a reference is refused).
///
/// # Safety
/// `source` must be a valid VARIANT whose reference, if any, points at a value of its kind.
pub unsafe fn view(source: &Variant) -> Result<Variant, HResult> {
    if source.kind & kind::BYREF == 0 {
        return Ok(*source);
    }
    if !is_valid(source.kind) {
        return Err(DISP_E_BADVARTYPE);
    }
    let base = source.kind & !kind::BYREF;
    let at = source.pointer();
    if at == 0 {
        return Err(E_INVALIDARG);
    }
    if base == kind::VARIANT {
        // SAFETY: the caller's duty: a `VT_VARIANT | VT_BYREF` points at a VARIANT.
        let inner = unsafe { (at as *const Variant).read_unaligned() };
        return match inner.kind & kind::BYREF {
            0 => Ok(inner),
            _ => Err(E_INVALIDARG),
        };
    }
    let mut value = Variant::new(base, 0);
    if base == kind::DECIMAL {
        let target = (&raw mut value).cast::<u8>();
        // SAFETY: a DECIMAL fills a VARIANT's first 16 bytes, its kind's word included.
        unsafe { core::ptr::copy_nonoverlapping(at as *const u8, target, DECIMAL_SIZE) };
        value.kind = kind::DECIMAL;
        return Ok(value);
    }
    let size = match base & kind::ARRAY {
        0 => value_size(base).ok_or(DISP_E_BADVARTYPE)?,
        _ => core::mem::size_of::<usize>(),
    };
    let target = value.data.as_mut_ptr().cast::<u8>();
    // SAFETY: the caller's duty: the reference points at `size` bytes of its kind's value.
    unsafe { core::ptr::copy_nonoverlapping(at as *const u8, target, size) };
    Ok(value)
}

/// `VariantClear`'s work: gives back what `*variant` owns and leaves it `VT_EMPTY`.
///
/// # Safety
/// `variant` must be null or point at a VARIANT.
pub unsafe fn clear(variant: *mut Variant) -> HResult {
    if variant.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let value = unsafe { variant.read_unaligned() };
    if !is_valid(value.kind) {
        return DISP_E_BADVARTYPE;
    }
    // SAFETY: the variant owns its value, which it gives up now.
    let released = unsafe { release(&value) };
    if released < 0 {
        return released;
    }
    // SAFETY: the caller's duty.
    unsafe { variant.write_unaligned(Variant::EMPTY) };
    S_OK
}

/// Clears `*destination`, then stores `value` in it; on failure `value` is given back.
///
/// # Safety
/// `destination` must point at a VARIANT; `value` must be owned by the caller.
pub unsafe fn replace(destination: *mut Variant, value: Variant) -> HResult {
    // SAFETY: the caller's duty.
    let cleared = unsafe { clear(destination) };
    if cleared < 0 {
        // SAFETY: the value is the caller's, which gives it up.
        unsafe { release(&value) };
        return cleared;
    }
    // SAFETY: the caller's duty.
    unsafe { destination.write_unaligned(value) };
    S_OK
}

/// `VariantInit`.
///
/// # Safety
/// `variant` must be null or point at a writable VARIANT.
pub unsafe extern "efiapi" fn variant_init(variant: *mut Variant) {
    if !variant.is_null() {
        // SAFETY: the caller's duty.
        unsafe { variant.write_unaligned(Variant::EMPTY) };
    }
}

/// `VariantClear`.
///
/// # Safety
/// `variant` must be null or point at a VARIANT.
pub unsafe extern "efiapi" fn variant_clear(variant: *mut Variant) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { clear(variant) }
}

/// `VariantCopy`: a deep copy of `*source` in `*destination`, which is cleared first.
///
/// # Safety
/// Each must be null or point at a VARIANT.
pub unsafe extern "efiapi" fn variant_copy(
    destination: *mut Variant,
    source: *const Variant,
) -> HResult {
    if destination.is_null() || source.is_null() {
        return E_INVALIDARG;
    }
    if core::ptr::eq(destination, source) {
        return S_OK;
    }
    // SAFETY: the caller's duty.
    let original = unsafe { source.read_unaligned() };
    // SAFETY: as above.
    match unsafe { duplicate(&original) } {
        // SAFETY: the caller's duty; the copy is this call's own.
        Ok(copy) => unsafe { replace(destination, copy) },
        Err(code) => code,
    }
}

/// `VariantCopyInd`: as `VariantCopy`, with a `VT_BYREF` source replaced by a copy of the value
/// it points at.
///
/// # Safety
/// Each must be null or point at a VARIANT.
pub unsafe extern "efiapi" fn variant_copy_ind(
    destination: *mut Variant,
    source: *const Variant,
) -> HResult {
    if destination.is_null() || source.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let original = unsafe { source.read_unaligned() };
    // SAFETY: as above.
    let value = match unsafe { view(&original) } {
        Ok(value) => value,
        Err(code) => return code,
    };
    // SAFETY: the value is valid, as `view` read it.
    match unsafe { duplicate(&value) } {
        // SAFETY: the caller's duty; the copy is this call's own.
        Ok(copy) => unsafe { replace(destination, copy) },
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_starts_at_the_eighth_byte() {
        assert_eq!(core::mem::size_of::<Variant>(), 24);
        assert_eq!(core::mem::offset_of!(Variant, data), 8);
        let value = Variant::long(-2);
        // SAFETY: a VARIANT is 24 plain bytes.
        let bytes: [u8; 24] = unsafe { core::mem::transmute(value) };
        assert_eq!(&bytes[..2], &3u16.to_le_bytes());
        assert_eq!(&bytes[8..12], &(-2i32).to_le_bytes());
        assert_eq!(Variant::boolean(true).data[0], 0xffff);
    }

    #[test]
    fn kinds_are_checked() {
        assert!(is_valid(kind::EMPTY));
        assert!(is_valid(kind::BSTR | kind::BYREF));
        assert!(is_valid(kind::VARIANT | kind::ARRAY));
        assert!(!is_valid(kind::VARIANT));
        assert!(!is_valid(kind::EMPTY | kind::BYREF));
        assert!(!is_valid(15));
        assert!(!is_valid(kind::I4 | 0x1000));
    }

    #[test]
    fn copies_own_their_strings() {
        let text = bstr::from_ascii(b"Bananas");
        let original = Variant::new(kind::BSTR, text as u64);
        let mut copy = Variant::EMPTY;
        // SAFETY: both are VARIANTs.
        assert_eq!(unsafe { variant_copy(&mut copy, &original) }, S_OK);
        assert_eq!(copy.kind, kind::BSTR);
        assert_ne!(copy.pointer(), original.pointer());
        // SAFETY: the copy holds a BSTR.
        let copied = unsafe { bstr::units(copy.pointer() as *const u16) };
        // SAFETY: a BSTR.
        assert_eq!(copied, unsafe { bstr::units(text) });
        let mut original = original;
        // SAFETY: each owns its string.
        assert_eq!(unsafe { variant_clear(&mut original) }, S_OK);
        assert_eq!(original.kind, kind::EMPTY);
        // SAFETY: as above.
        assert_eq!(unsafe { variant_clear(&mut copy) }, S_OK);
    }

    #[test]
    fn references_are_followed_by_copy_ind() {
        let mut target = 7i32;
        let reference = Variant::new(kind::I4 | kind::BYREF, &raw mut target as u64);
        let mut copy = Variant::EMPTY;
        // SAFETY: the reference points at an `i32`.
        assert_eq!(unsafe { variant_copy_ind(&mut copy, &reference) }, S_OK);
        assert_eq!(copy, Variant::long(7));
        let inner = Variant::long(11);
        let to_variant = Variant::new(kind::VARIANT | kind::BYREF, &raw const inner as u64);
        // SAFETY: the reference points at a VARIANT.
        assert_eq!(unsafe { view(&to_variant) }, Ok(inner));
        let in_place = &raw mut copy;
        // SAFETY: the same VARIANT on both sides is allowed.
        assert_eq!(unsafe { variant_copy_ind(in_place, in_place) }, S_OK);
        assert_eq!(copy, Variant::long(7));
        let null_reference = Variant::new(kind::I4 | kind::BYREF, 0);
        // SAFETY: a null reference is refused before it is read.
        assert_eq!(unsafe { view(&null_reference) }, Err(E_INVALIDARG));
    }

    #[test]
    fn a_bad_kind_is_refused() {
        let mut bad = Variant::new(15, 0);
        // SAFETY: a VARIANT.
        assert_eq!(unsafe { variant_clear(&mut bad) }, DISP_E_BADVARTYPE);
        let mut empty = Variant::long(1);
        // SAFETY: a VARIANT.
        unsafe { variant_init(&mut empty) };
        assert_eq!(empty, Variant::EMPTY);
    }
}
