//! The display monitors (D178): the window server shows one screen, so there is one monitor,
//! the primary one, its rectangle the screen and its work area the server's
//! (`MonitorFromPoint`, `EnumDisplayMonitors`, `GetMonitorInfoW`).

use super::gui;
use super::user32::Win32Rect;
use super::{Bool, FALSE, TRUE, call_program, error, fail};
use crate::library::Export;
use eggos_windows_gui::surface::Rect;

/// The one monitor's handle (`HMONITOR`).
const PRIMARY: usize = 0x0001_0001;
const MONITOR_DEFAULTTONULL: u32 = 0;
const MONITORINFOF_PRIMARY: u32 = 0x1;
const DEVICE_NAME: &str = "\\\\.\\DISPLAY1";
const DEVICE_NAME_UNITS: usize = 32;

fn screen() -> Result<(Rect, Rect), u32> {
    let work = gui::device()?.work_area;
    Ok((Rect::new(0, 0, work.right, work.bottom), work))
}

fn win32(rect: Rect) -> Win32Rect {
    Win32Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

extern "efiapi" fn monitor_from_point(point: u64, flags: u32) -> usize {
    let (x, y) = (point as u32 as i32, (point >> 32) as u32 as i32);
    let inside = screen().is_ok_and(|(monitor, _)| monitor.contains(x, y));
    match inside || flags != MONITOR_DEFAULTTONULL {
        true => PRIMARY,
        false => 0,
    }
}

unsafe extern "efiapi" fn enum_display_monitors(
    context: usize,
    clip: *const Win32Rect,
    procedure: usize,
    lparam: usize,
) -> Bool {
    if procedure == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let (monitor, _) = match screen() {
        Ok(screen) => screen,
        Err(code) => return fail(code, FALSE),
    };
    let area = match clip.is_null() {
        true => monitor,
        // SAFETY: the caller passes a readable `RECT` or null.
        false => monitor.intersection(&super::paint::rect_of(unsafe { &*clip })),
    };
    if area.is_empty() {
        return TRUE;
    }
    let rect = win32(area);
    // SAFETY: the program passes a `MonitorEnumProc`, which Windows calls with the monitor, the
    // device context, the monitor's rectangle and the program's value.
    unsafe {
        call_program(
            procedure,
            [PRIMARY, context, &raw const rect as usize, lparam],
        )
    };
    TRUE
}

#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Win32Rect,
    work: Win32Rect,
    flags: u32,
}

unsafe extern "efiapi" fn get_monitor_info_w(monitor: usize, out: *mut MonitorInfo) -> Bool {
    if monitor != PRIMARY || out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `MONITORINFO` or `MONITORINFOEXW`.
    let info = unsafe { &mut *out };
    let size = info.size as usize;
    let extended = core::mem::size_of::<MonitorInfo>() + DEVICE_NAME_UNITS * 2;
    if size != core::mem::size_of::<MonitorInfo>() && size != extended {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let (whole, work) = match screen() {
        Ok(screen) => screen,
        Err(code) => return fail(code, FALSE),
    };
    info.monitor = win32(whole);
    info.work = win32(work);
    info.flags = MONITORINFOF_PRIMARY;
    if size == extended {
        let name = out
            .cast::<u8>()
            .wrapping_add(core::mem::size_of::<MonitorInfo>());
        let name = name.cast::<u16>();
        for (index, unit) in DEVICE_NAME
            .encode_utf16()
            .chain(core::iter::repeat(0))
            .take(DEVICE_NAME_UNITS)
            .enumerate()
        {
            // SAFETY: a `MONITORINFOEXW` has 32 units of device name after its `MONITORINFO`.
            unsafe { name.add(index).write_unaligned(unit) };
        }
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"MonitorFromPoint" => monitor_from_point,
    b"EnumDisplayMonitors" => enum_display_monitors,
    b"GetMonitorInfoW" => get_monitor_info_w,
];
