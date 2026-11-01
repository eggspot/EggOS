//! The picture objects `OleLoadPicture` makes (D230): an `IPicture` over a bitmap (BMP, JPEG,
//! GIF's first frame, TIFF, PNG) as a top-down 32-bit DIB section with premultiplied alpha, or
//! over an icon (`.ico`, `.cur`) as an `HICON`, sized in HIMETRIC at 96 pixels per inch, which
//! `Render` draws scaled into a device context, blending its straight-alpha pixels over what is
//! there, and `SaveAsFile` writes back as the bytes it was loaded from. `SelectPicture` is not
//! supported (`E_NOTIMPL`).

use super::super::icon::{self, Decoded};
use super::super::{dib, gdi, heap, paint};
use crate::com::guid::{self, Guid};
use crate::com::picture::{Axis, himetric, source_pixel};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_image::{Error as ImageError, Format};
use eggos_windows_gui::surface::Rect;

const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const E_FAIL: HResult = 0x8000_4005_u32 as i32;
const CTL_E_INVALIDPICTURE: HResult = 0x800a_01e1_u32 as i32;
const PICTYPE_BITMAP: i16 = 1;
const PICTYPE_ICON: i16 = 3;
const PICTURE_TRANSPARENT: u32 = 0x2;
const WRITE_SLOT: usize = 4;

/// A picture: its table, its reference count, what it is, its handle, its pixels and the bytes
/// it was loaded from.
#[repr(C)]
#[derive(Debug)]
pub struct Picture {
    table: usize,
    references: AtomicU32,
    kind: i16,
    keep_original: i32,
    handle: usize,
    width: u32,
    height: u32,
    pixels: *mut u32,
    original: *mut u8,
    original_length: usize,
}

impl Picture {
    fn pixel(&self, x: u32, y: u32) -> u32 {
        let at = y as usize * self.width as usize + x as usize;
        // SAFETY: the picture's `width * height` pixels live as long as it; `x` and `y` are
        // inside.
        unsafe { self.pixels.wrapping_add(at).read() }
    }
}

fn hresult_of(code: u32) -> HResult {
    match code {
        super::super::error::NOT_ENOUGH_MEMORY => E_OUTOFMEMORY,
        _ => E_FAIL,
    }
}

fn picture_handle(kind: i16, image: &Decoded) -> Result<usize, HResult> {
    if kind == PICTYPE_ICON {
        return icon::icon_from_pixels(image).map_err(hresult_of);
    }
    let (handle, pixels) = gdi::new_bitmap(image.width, image.height, true).map_err(hresult_of)?;
    let count = image.width as usize * image.height as usize;
    // SAFETY: the new section's `width * height` pixels, which nothing else uses yet.
    let target = unsafe { core::slice::from_raw_parts_mut(pixels, count) };
    for (into, &pixel) in target.iter_mut().zip(image.pixels.pixels()) {
        *into = dib::premultiplied_over(pixel, 0);
    }
    Ok(handle)
}

fn release_handle(kind: i16, handle: usize) {
    match kind {
        PICTYPE_ICON => {
            let _ = icon::destroy(handle);
        }
        _ => gdi::delete_bitmap(handle),
    }
}

/// A new picture with `table` of the image file in `bytes`, with one reference; its
/// `KeepOriginalFormat` starts as `keep_original`.
pub(super) fn load(table: usize, bytes: &[u8], keep_original: bool) -> Result<usize, HResult> {
    let kind = match eggos_image::sniff(bytes) {
        Some(Format::Icon | Format::Cursor) => PICTYPE_ICON,
        _ => PICTYPE_BITMAP,
    };
    let image = icon::decode_image_file(bytes).map_err(|error| match error {
        ImageError::TooLarge | ImageError::OutOfMemory => E_OUTOFMEMORY,
        _ => CTL_E_INVALIDPICTURE,
    })?;
    let original = heap::allocate(bytes.len());
    if original.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: a fresh block of `bytes.len()` bytes.
    unsafe { original.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
    let handle = match picture_handle(kind, &image) {
        Ok(handle) => handle,
        Err(status) => {
            // SAFETY: the block this call allocated, which nothing holds.
            unsafe { heap::free(original) };
            return Err(status);
        }
    };
    let object = crate::com::allocate(core::mem::size_of::<Picture>()).cast::<Picture>();
    if object.is_null() {
        release_handle(kind, handle);
        // SAFETY: as above.
        unsafe { heap::free(original) };
        return Err(E_OUTOFMEMORY);
    }
    let (width, height) = (image.width, image.height);
    let picture = Picture {
        table,
        references: AtomicU32::new(1),
        kind,
        keep_original: i32::from(keep_original),
        handle,
        width,
        height,
        pixels: image.pixels.into_raw().cast(),
        original,
        original_length: bytes.len(),
    };
    // SAFETY: a fresh, aligned block for one picture.
    unsafe { object.write(picture) };
    Ok(object as usize)
}

unsafe fn picture<'a>(this: *mut Picture) -> &'a Picture {
    // SAFETY: the caller's duty: a live picture.
    unsafe { &*this }
}

/// `IUnknown::QueryInterface`: `IUnknown` and `IPicture`.
///
/// # Safety
/// `this` must be a live picture; the others as `QueryInterface` says.
pub(super) unsafe extern "efiapi" fn query_interface(
    this: *mut Picture,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &picture(this).references };
    let interfaces = [guid::UNKNOWN, guid::PICTURE];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

/// `IUnknown::AddRef`.
///
/// # Safety
/// `this` must be a live picture.
pub(super) unsafe extern "efiapi" fn add_reference(this: *mut Picture) -> u32 {
    // SAFETY: the caller's duty.
    let references = unsafe { &picture(this).references };
    references.fetch_add(1, Ordering::Relaxed) + 1
}

/// `IUnknown::Release`: the last reference frees the picture, its handle, its pixels and its
/// bytes.
///
/// # Safety
/// `this` must be a live picture the caller holds a reference to.
pub(super) unsafe extern "efiapi" fn release(this: *mut Picture) -> u32 {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    release_handle(found.kind, found.handle);
    // SAFETY: the picture's own blocks, which nothing uses once it is gone.
    unsafe { heap::free(found.pixels.cast()) };
    // SAFETY: as above.
    unsafe { heap::free(found.original) };
    // SAFETY: the picture's block; no reference is left.
    unsafe { crate::com::free(this.cast()) };
    0
}

unsafe fn answer<T>(out: *mut T, value: T) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a writable pointer from the program.
    unsafe { out.write_unaligned(value) };
    S_OK
}

/// `IPicture::get_Handle`: the `HBITMAP` or `HICON`.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_handle(this: *mut Picture, out: *mut u32) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    // SAFETY: the caller's duty.
    unsafe { answer(out, found.handle as u32) }
}

/// `IPicture::get_hPal`: no palette.
///
/// # Safety
/// `out` must be null or writable.
pub(super) unsafe extern "efiapi" fn get_palette(_this: *mut Picture, out: *mut u32) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { answer(out, 0) }
}

/// `IPicture::get_Type`: `PICTYPE_BITMAP` or `PICTYPE_ICON`.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_type(this: *mut Picture, out: *mut i16) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    // SAFETY: the caller's duty.
    unsafe { answer(out, found.kind) }
}

/// `IPicture::get_Width`, in HIMETRIC.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_width(this: *mut Picture, out: *mut i32) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    // SAFETY: the caller's duty.
    unsafe { answer(out, himetric(found.width)) }
}

/// `IPicture::get_Height`, in HIMETRIC.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_height(this: *mut Picture, out: *mut i32) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    // SAFETY: the caller's duty.
    unsafe { answer(out, himetric(found.height)) }
}

/// `IPicture::Render`: the source rectangle (HIMETRIC, its y axis up) scaled into the
/// destination rectangle of `hdc`, each pixel the nearest source pixel blended over what is
/// there.
///
/// # Safety
/// `this` must be a live picture.
#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "efiapi" fn render(
    this: *mut Picture,
    hdc: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    source_x: i32,
    source_y: i32,
    source_width: i32,
    source_height: i32,
    _bounds: usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    if width <= 0 || height <= 0 {
        return E_INVALIDARG;
    }
    let columns = Axis {
        pixels: found.width,
        himetric: himetric(found.width),
        from_far_end: false,
    };
    let rows = Axis {
        pixels: found.height,
        himetric: himetric(found.height),
        from_far_end: true,
    };
    let drawn = paint::draw_in(hdc, |surface, _, _| {
        let Some(surface) = surface else {
            return;
        };
        let area = Rect::sized(x, y, width, height).intersection(&surface.clip());
        for row in area.top..area.bottom {
            let Some(from_y) = source_pixel(rows, row - y, height, source_y, source_height) else {
                continue;
            };
            for column in area.left..area.right {
                let from_x = source_pixel(columns, column - x, width, source_x, source_width);
                let (Some(from_x), Some(under)) = (from_x, surface.get(column, row)) else {
                    continue;
                };
                surface.pixel(column, row, dib::blend(found.pixel(from_x, from_y), under));
            }
        }
    });
    match drawn {
        Ok(()) => S_OK,
        Err(_) => E_INVALIDARG,
    }
}

/// `IPicture::set_hPal`: accepted; the picture's pixels are true colour.
pub(super) extern "efiapi" fn set_palette(_this: *mut Picture, _palette: u32) -> HResult {
    S_OK
}

/// `IPicture::get_CurDC`: the picture is selected into no device context.
///
/// # Safety
/// `out` must be null or writable.
pub(super) unsafe extern "efiapi" fn get_current_dc(
    _this: *mut Picture,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { answer(out, 0) }
}

/// `IPicture::SelectPicture`: not supported.
pub(super) extern "efiapi" fn select_picture(
    _this: *mut Picture,
    _hdc: usize,
    _previous: usize,
    _bitmap: usize,
) -> HResult {
    E_NOTIMPL
}

/// `IPicture::get_KeepOriginalFormat`.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_keep_original_format(
    this: *mut Picture,
    out: *mut i32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    // SAFETY: the caller's duty.
    unsafe { answer(out, found.keep_original) }
}

/// `IPicture::put_KeepOriginalFormat`.
///
/// # Safety
/// `this` must be a live picture.
pub(super) unsafe extern "efiapi" fn put_keep_original_format(
    this: *mut Picture,
    keep: i32,
) -> HResult {
    // SAFETY: the caller's duty; one thread uses an object at a time (the apartment model).
    unsafe { (*this).keep_original = i32::from(keep != 0) };
    S_OK
}

/// `IPicture::PictureChanged`.
pub(super) extern "efiapi" fn picture_changed(_this: *mut Picture) -> HResult {
    S_OK
}

/// `IPicture::SaveAsFile`: writes the bytes the picture was loaded from to `stream`, and their
/// count to `size` when it is not null.
///
/// # Safety
/// `this` must be a live picture, `stream` a live `IStream` and `size` null or writable.
pub(super) unsafe extern "efiapi" fn save_as_file(
    this: *mut Picture,
    stream: usize,
    _save_copy: i32,
    size: *mut i32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    if stream == 0 {
        return E_POINTER;
    }
    let Ok(length) = u32::try_from(found.original_length) else {
        return E_FAIL;
    };
    let mut written = 0u32;
    let arguments = [found.original as usize, length as usize, &raw mut written as usize];
    // SAFETY: the caller's duty; `Write` is a stream's fifth method, reading `length` bytes.
    let status = unsafe { crate::com::call_method(stream, WRITE_SLOT, arguments) } as u32 as i32;
    if status < 0 {
        return status;
    }
    if !size.is_null() {
        // SAFETY: the caller's duty.
        unsafe { size.write_unaligned(i32::try_from(written).unwrap_or(i32::MAX)) };
    }
    S_OK
}

/// `IPicture::get_Attributes`: an icon is transparent.
///
/// # Safety
/// `this` must be a live picture and `out` null or writable.
pub(super) unsafe extern "efiapi" fn get_attributes(this: *mut Picture, out: *mut u32) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { picture(this) };
    let attributes = match found.kind {
        PICTYPE_ICON => PICTURE_TRANSPARENT,
        _ => 0,
    };
    // SAFETY: the caller's duty.
    unsafe { answer(out, attributes) }
}

/// Hands picture `object` (one reference, which this takes over) to the program as `interface`
/// through `result`.
///
/// # Safety
/// `object` must be a live picture `load` made; `interface` and `result` as `QueryInterface`
/// says.
pub(super) unsafe fn hand_out(
    object: usize,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    let this = object as *mut Picture;
    // SAFETY: the caller's duty.
    let status = unsafe { query_interface(this, interface, result) };
    // SAFETY: the reference `load` gave, which the program's (if any) now replaces.
    unsafe { release(this) };
    status
}
