//! shell32's desktop calls (D178): files dropped on a window come as a `DROPFILES` block (the
//! list's offset, the drop point, whether it fell outside the client area, whether the names
//! are UTF-16, then the names each ended by a NUL and the list by another), which
//! `DragQueryFileW` and `DragQueryPoint` read and `DragFinish` frees; EggOS does not drop files
//! on Windows programs' windows yet, so no such block comes. Icons are not read from files yet
//! (`ExtractIconW` finds none) and there is no notification area for `Shell_NotifyIconW`.

use super::{Bool, FALSE, error, fail, heap};
use crate::library::Export;
use crate::text;

const FILES_OFFSET: usize = 0;
const POINT: usize = 4;
const OUTSIDE_CLIENT: usize = 12;
const WIDE: usize = 16;
const COUNT_FILES: u32 = u32::MAX;

unsafe fn read<T>(drop: *const u8, offset: usize) -> T {
    // SAFETY: the callers' contract: a `DROPFILES` block holding a `T` at `offset`.
    unsafe { drop.wrapping_add(offset).cast::<T>().read_unaligned() }
}

/// Calls `visit` with each name the drop holds, in UTF-16, until it says to stop.
unsafe fn for_each_name(drop: *const u8, visit: &mut dyn FnMut(&[u16]) -> bool) {
    // SAFETY: the callers' contract: a `DROPFILES` block.
    let offset: u32 = unsafe { read(drop, FILES_OFFSET) };
    // SAFETY: as above.
    let wide: Bool = unsafe { read(drop, WIDE) };
    let names = drop.wrapping_add(offset as usize);
    if wide != FALSE {
        // SAFETY: as above: UTF-16 names each ended by a NUL, the list by another.
        let units = unsafe { super::processes::listed(names.cast::<u16>()) };
        for name in units
            .split(|unit| *unit == 0)
            .filter(|name| !name.is_empty())
        {
            if !visit(name) {
                return;
            }
        }
        return;
    }
    // SAFETY: as above, in bytes (the layer's ANSI code page is UTF-8).
    let bytes = unsafe { super::processes::listed(names) };
    for name in bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let mut units = [0u16; text::PATH_CAPACITY];
        let length = text::write_utf16(name, &mut units);
        if !visit(&units[..length]) {
            return;
        }
    }
}

unsafe extern "efiapi" fn drag_query_file_w(
    drop: *const u8,
    index: u32,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    if drop.is_null() {
        return 0;
    }
    let measure = buffer.is_null();
    // SAFETY: the caller passes `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    let mut seen = 0u32;
    let mut answer = 0u32;
    let mut visit = |name: &[u16]| {
        if index == COUNT_FILES || seen < index {
            seen += 1;
            return true;
        }
        let copied = name.len().min(out.len().saturating_sub(1));
        answer = match measure {
            true => name.len() as u32,
            false => copied as u32,
        };
        if !measure && !out.is_empty() {
            out[..copied].copy_from_slice(&name[..copied]);
            out[copied] = 0;
        }
        false
    };
    // SAFETY: the caller passes an `HDROP`, its `DROPFILES` block.
    unsafe { for_each_name(drop, &mut visit) };
    match index {
        COUNT_FILES => seen,
        _ => answer,
    }
}

unsafe extern "efiapi" fn drag_query_point(drop: *const u8, point: *mut [i32; 2]) -> Bool {
    if drop.is_null() || point.is_null() {
        return FALSE;
    }
    // SAFETY: the caller passes an `HDROP`, its `DROPFILES` block.
    let at: [i32; 2] = unsafe { read(drop, POINT) };
    // SAFETY: as above.
    let outside: Bool = unsafe { read(drop, OUTSIDE_CLIENT) };
    // SAFETY: the caller passes a writable `POINT`.
    unsafe { point.write_unaligned(at) };
    Bool::from(outside == FALSE)
}

unsafe extern "efiapi" fn drag_finish(drop: *mut u8) {
    // SAFETY: an `HDROP` is a global memory block of the layer's heap, which the heap checks.
    unsafe { heap::free(drop) };
}

extern "efiapi" fn extract_icon_w(_instance: usize, _file: *const u16, _index: u32) -> usize {
    0
}

extern "efiapi" fn shell_notify_icon_w(_message: u32, _data: *const u8) -> Bool {
    fail(error::NOT_SUPPORTED, FALSE)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"DragQueryFileW" => drag_query_file_w,
    b"DragQueryPoint" => drag_query_point,
    b"DragFinish" => drag_finish,
    b"ExtractIconW" => extract_icon_w,
    b"Shell_NotifyIconW" => shell_notify_icon_w,
];
