//! comctl32 (D174): `InitCommonControls`, which programs import by its ordinal 17, and
//! `InitCommonControlsEx`. The common control classes (list and tree views, toolbars, status
//! bars, ...) are not in the layer yet; the standard classes (`BUTTON`, `EDIT`, `STATIC`,
//! `LISTBOX`, `COMBOBOX`) always are. So `InitCommonControls` has nothing to register and
//! `InitCommonControlsEx` succeeds only for the standard classes.

use super::{Bool, FALSE};
use crate::library::Export;

const INIT_COMMON_CONTROLS_ORDINAL: u16 = 17;
const ICC_STANDARD_CLASSES: u32 = 0x4000;

#[repr(C)]
struct InitCommonControlsEx {
    size: u32,
    classes: u32,
}

extern "efiapi" fn init_common_controls() {}

unsafe extern "efiapi" fn init_common_controls_ex(request: *const InitCommonControlsEx) -> Bool {
    if request.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes a readable `INITCOMMONCONTROLSEX`.
    let request = unsafe { &*request };
    let known = request.size as usize == core::mem::size_of::<InitCommonControlsEx>();
    Bool::from(known && request.classes & !ICC_STANDARD_CLASSES == 0)
}

/// comctl32's functions by name.
pub(super) const EXPORTS: &[Export] = exports![
    b"InitCommonControls" => init_common_controls,
    b"InitCommonControlsEx" => init_common_controls_ex,
];

/// comctl32's function at `ordinal`, if the layer has it.
pub(super) fn by_ordinal(ordinal: u16) -> Option<*const ()> {
    match ordinal {
        INIT_COMMON_CONTROLS_ORDINAL => crate::library::find(&[EXPORTS], b"InitCommonControls"),
        _ => None,
    }
}
