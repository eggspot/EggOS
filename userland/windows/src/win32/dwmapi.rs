//! dwmapi (D178): `DwmGetWindowAttribute`'s `DWMWA_EXTENDED_FRAME_BOUNDS`, a top-level window's
//! rectangle on the screen as the window server places it (`placement.rs`; EggOS draws no
//! invisible resize borders, so it is the window's own rectangle). The window server composes
//! the desktop; the other attributes are not offered.

use super::gui::try_with;
use super::user32::Win32Rect;
use crate::library::Export;

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_HANDLE: i32 = 0x8007_0006_u32 as i32;
const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;

unsafe extern "efiapi" fn dwm_get_window_attribute(
    window: usize,
    attribute: u32,
    out: *mut Win32Rect,
    size: u32,
) -> i32 {
    if try_with(|gui| gui.window(window).map(|_| ())).is_err() {
        return E_HANDLE;
    }
    if attribute != DWMWA_EXTENDED_FRAME_BOUNDS
        || out.is_null()
        || (size as usize) < core::mem::size_of::<Win32Rect>()
    {
        return E_INVALIDARG;
    }
    let Ok(rect) = super::placement::screen_rect(window) else {
        return E_HANDLE;
    };
    // SAFETY: the caller passes `size` writable bytes, at least a `RECT`'s.
    unsafe {
        out.write_unaligned(Win32Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    };
    S_OK
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DwmGetWindowAttribute" => dwm_get_window_attribute,
];
