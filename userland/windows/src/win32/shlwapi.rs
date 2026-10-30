//! shlwapi (D178): `StrCmpLogicalW`, the order Explorer sorts names in: runs of digits compare
//! as numbers, the rest ignoring case (`crate::text::compare_logical`).

use crate::library::Export;
use core::cmp::Ordering;

unsafe fn terminated<'a>(text: *const u16) -> &'a [u16] {
    match text.is_null() {
        true => &[],
        // SAFETY: the caller's duty: a NUL-terminated string, or null for an empty one.
        false => unsafe { super::file::wide_string(text) },
    }
}

unsafe extern "efiapi" fn str_cmp_logical_w(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `StrCmpLogicalW`'s contract: NUL-terminated strings.
    let a = unsafe { terminated(a) };
    // SAFETY: as above.
    let b = unsafe { terminated(b) };
    match crate::text::compare_logical(a, b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"StrCmpLogicalW" => str_cmp_logical_w,
];
