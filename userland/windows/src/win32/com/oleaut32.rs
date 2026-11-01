//! oleaut32's exports: BSTRs, VARIANTs and their conversions, SAFEARRAYs (`crate::com`),
//! `GetActiveObject` and `OleLoadPicture`, by name and by the ordinals of Microsoft's
//! `oleaut32.dll`, which is how programs import them. There is no running object table, so
//! `GetActiveObject` finds nothing (`MK_E_UNAVAILABLE`); `OleLoadPicture` has no picture object
//! to make and reports the data as no picture it can load (`CTL_E_INVALIDPICTURE`).

use crate::com::guid::Guid;
use crate::com::{HResult, bstr, conversion, safe_array, variant};
use crate::library::Export;

const MK_E_UNAVAILABLE: HResult = 0x8004_01e3_u32 as i32;
const CTL_E_INVALIDPICTURE: HResult = 0x800a_01e1_u32 as i32;

const ORDINALS: [(u16, &[u8]); 30] = [
    (2, b"SysAllocString"),
    (3, b"SysReAllocString"),
    (4, b"SysAllocStringLen"),
    (5, b"SysReAllocStringLen"),
    (6, b"SysFreeString"),
    (7, b"SysStringLen"),
    (8, b"VariantInit"),
    (9, b"VariantClear"),
    (10, b"VariantCopy"),
    (11, b"VariantCopyInd"),
    (12, b"VariantChangeType"),
    (15, b"SafeArrayCreate"),
    (16, b"SafeArrayDestroy"),
    (17, b"SafeArrayGetDim"),
    (18, b"SafeArrayGetElemsize"),
    (19, b"SafeArrayGetUBound"),
    (20, b"SafeArrayGetLBound"),
    (21, b"SafeArrayLock"),
    (22, b"SafeArrayUnlock"),
    (23, b"SafeArrayAccessData"),
    (24, b"SafeArrayUnaccessData"),
    (25, b"SafeArrayGetElement"),
    (26, b"SafeArrayPutElement"),
    (27, b"SafeArrayCopy"),
    (35, b"GetActiveObject"),
    (147, b"VariantChangeTypeEx"),
    (148, b"SafeArrayPtrOfIndex"),
    (149, b"SysStringByteLen"),
    (150, b"SysAllocStringByteLen"),
    (418, b"OleLoadPicture"),
];

unsafe extern "efiapi" fn get_active_object(
    _class: *const Guid,
    _reserved: usize,
    result: *mut usize,
) -> HResult {
    if !result.is_null() {
        // SAFETY: `GetActiveObject`'s contract: a writable pointer.
        unsafe { result.write_unaligned(0) };
    }
    MK_E_UNAVAILABLE
}

unsafe extern "efiapi" fn ole_load_picture(
    _stream: usize,
    _size: i32,
    _keep_original_format: i32,
    _interface: *const Guid,
    result: *mut usize,
) -> HResult {
    if !result.is_null() {
        // SAFETY: `OleLoadPicture`'s contract: a writable pointer.
        unsafe { result.write_unaligned(0) };
    }
    CTL_E_INVALIDPICTURE
}

pub(in crate::win32) const EXPORTS: &[Export] = exports![
    b"SysAllocString" => bstr::sys_alloc_string,
    b"SysReAllocString" => bstr::sys_re_alloc_string,
    b"SysAllocStringLen" => bstr::sys_alloc_string_len,
    b"SysReAllocStringLen" => bstr::sys_re_alloc_string_len,
    b"SysFreeString" => bstr::sys_free_string,
    b"SysStringLen" => bstr::sys_string_len,
    b"SysStringByteLen" => bstr::sys_string_byte_len,
    b"SysAllocStringByteLen" => bstr::sys_alloc_string_byte_len,
    b"VariantInit" => variant::variant_init,
    b"VariantClear" => variant::variant_clear,
    b"VariantCopy" => variant::variant_copy,
    b"VariantCopyInd" => variant::variant_copy_ind,
    b"VariantChangeType" => conversion::variant_change_type,
    b"VariantChangeTypeEx" => conversion::variant_change_type_ex,
    b"SafeArrayCreate" => safe_array::safe_array_create,
    b"SafeArrayDestroy" => safe_array::safe_array_destroy,
    b"SafeArrayGetDim" => safe_array::safe_array_get_dim,
    b"SafeArrayGetElemsize" => safe_array::safe_array_get_elemsize,
    b"SafeArrayGetUBound" => safe_array::safe_array_get_ubound,
    b"SafeArrayGetLBound" => safe_array::safe_array_get_lbound,
    b"SafeArrayLock" => safe_array::safe_array_lock,
    b"SafeArrayUnlock" => safe_array::safe_array_unlock,
    b"SafeArrayAccessData" => safe_array::safe_array_access_data,
    b"SafeArrayUnaccessData" => safe_array::safe_array_unaccess_data,
    b"SafeArrayGetElement" => safe_array::safe_array_get_element,
    b"SafeArrayPutElement" => safe_array::safe_array_put_element,
    b"SafeArrayCopy" => safe_array::safe_array_copy,
    b"SafeArrayPtrOfIndex" => safe_array::safe_array_ptr_of_index,
    b"GetActiveObject" => get_active_object,
    b"OleLoadPicture" => ole_load_picture,
];

/// The function oleaut32 exports as `ordinal`.
pub(in crate::win32) fn by_ordinal(ordinal: u16) -> Option<*const ()> {
    let &(_, name) = ORDINALS.iter().find(|&&(number, _)| number == ordinal)?;
    crate::library::find(&[EXPORTS], name)
}
