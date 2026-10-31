//! GDI+ (`gdiplus.dll`, D178), the flat API a program reaches through `LoadLibrary` and
//! `GetProcAddress`: startup and shutdown; bitmaps decoded from PNG and BMP files (straight
//! alpha, `PixelFormat32bppARGB`), or made over 32-bit pixels of the program's or new ones
//! (`GdipCreateBitmapFromScan0`); their size, pixel format and pixels; an `HBITMAP` of one
//! (`GdipCreateHBITMAPFromBitmap`: a top-down 32-bit DIB section whose pixels are the image
//! composited over the background colour, with premultiplied alpha, as GDI+ gives it); and a
//! graphics on a device context that draws a bitmap scaled into a rectangle, each pixel the
//! nearest source pixel blended over what is there. A bitmap and a graphics are heap blocks of
//! the layer's. Other file formats (JPEG, GIF, TIFF, icons) answer `UnknownImageFormat`, other
//! pixel formats `NotImplemented`.

use super::dib;
use super::gdi;
use super::icon::{self, Decoded};
use super::paint;
use super::{error, heap};
use crate::library::Export;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_windows_gui::surface::Rect;

mod status {
    pub const OK: u32 = 0;
    pub const GENERIC_ERROR: u32 = 1;
    pub const INVALID_PARAMETER: u32 = 2;
    pub const OUT_OF_MEMORY: u32 = 3;
    pub const NOT_IMPLEMENTED: u32 = 6;
    pub const WIN32_ERROR: u32 = 7;
    pub const FILE_NOT_FOUND: u32 = 10;
    pub const UNKNOWN_IMAGE_FORMAT: u32 = 13;
    pub const UNSUPPORTED_VERSION: u32 = 17;
    pub const NOT_INITIALIZED: u32 = 18;
}

type Status = u32;

const PIXEL_FORMAT_32BPP_RGB: u32 = 0x0002_2009;
const PIXEL_FORMAT_32BPP_ARGB: u32 = 0x0026_200a;
const PIXEL_FORMAT_32BPP_PARGB: u32 = 0x000e_200b;
const BITMAP_MAGIC: u32 = 0x4770_426d;
const GRAPHICS_MAGIC: u32 = 0x4770_4772;
const OPAQUE: u32 = 0xff00_0000;

static STARTED: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Debug)]
struct GpBitmap {
    magic: u32,
    format: u32,
    width: u32,
    height: u32,
    stride: isize,
    pixels: usize,
    owned: usize,
}

#[repr(C)]
#[derive(Debug)]
struct GpGraphics {
    magic: u32,
    hdc: usize,
}

#[repr(C)]
struct StartupInput {
    version: u32,
    debug_event_callback: usize,
    suppress_background_thread: i32,
    suppress_external_codecs: i32,
}

#[repr(C)]
struct StartupOutput {
    hook: usize,
    unhook: usize,
}

fn started() -> bool {
    STARTED.load(Ordering::Acquire) != 0
}

fn status_of(code: u32) -> Status {
    match code {
        error::FILE_NOT_FOUND | error::PATH_NOT_FOUND => status::FILE_NOT_FOUND,
        error::NOT_ENOUGH_MEMORY => status::OUT_OF_MEMORY,
        error::INVALID_DATA => status::GENERIC_ERROR,
        code => super::fail(code, status::WIN32_ERROR),
    }
}

impl GpBitmap {
    fn pixel(&self, x: u32, y: u32) -> u32 {
        let row = self.pixels as isize + y as isize * self.stride;
        let at = (row as *const u32).wrapping_add(x as usize);
        // SAFETY: the bitmap's rows hold `width` pixels each, for as long as it lives (the
        // program's own memory for `GdipCreateBitmapFromScan0`, by its contract); `x` and `y`
        // are inside.
        let value = unsafe { at.read_unaligned() };
        match self.format {
            PIXEL_FORMAT_32BPP_PARGB => dib::unpremultiplied(value),
            PIXEL_FORMAT_32BPP_RGB => value | OPAQUE,
            _ => value,
        }
    }
}

unsafe fn bitmap<'a>(image: *const GpBitmap) -> Option<&'a GpBitmap> {
    // SAFETY: the caller's duty: null or one of this layer's blocks, whose magic says which.
    let image = unsafe { image.as_ref()? };
    (image.magic == BITMAP_MAGIC).then_some(image)
}

fn new_bitmap(value: GpBitmap) -> Result<*mut GpBitmap, Status> {
    let block = heap::allocate(core::mem::size_of::<GpBitmap>()).cast::<GpBitmap>();
    if block.is_null() {
        return Err(status::OUT_OF_MEMORY);
    }
    // SAFETY: a fresh block, 16-byte aligned, of a `GpBitmap`'s size.
    unsafe { block.write(value) };
    Ok(block)
}

fn bitmap_of(image: Decoded) -> Result<*mut GpBitmap, Status> {
    let (width, height) = (image.width, image.height);
    let pixels = image.pixels.into_raw() as usize;
    let made = new_bitmap(GpBitmap {
        magic: BITMAP_MAGIC,
        format: PIXEL_FORMAT_32BPP_ARGB,
        width,
        height,
        stride: width as isize * 4,
        pixels,
        owned: pixels,
    });
    if made.is_err() {
        // SAFETY: the pixels' block, which no bitmap holds.
        unsafe { heap::free(pixels as *mut u8) };
    }
    made
}

fn decode_file(bytes: &[u8]) -> Result<Decoded, Status> {
    if dib::png_size(bytes).is_some() {
        return icon::decode_png(bytes).map_err(status_of);
    }
    let packed = dib::bmp_file(bytes).ok_or(status::UNKNOWN_IMAGE_FORMAT)?;
    let mut image = icon::decode_packed(&packed).map_err(status_of)?;
    if !packed.layout.alpha_declared {
        for pixel in image.pixels.pixels_mut() {
            *pixel |= OPAQUE;
        }
    }
    Ok(image)
}

fn finish<T>(made: Result<*mut T, Status>, out: *mut *mut T) -> Status {
    match made {
        Ok(made) => {
            // SAFETY: the caller passes a writable pointer for the result.
            unsafe { out.write(made) };
            status::OK
        }
        Err(status) => status,
    }
}

unsafe extern "efiapi" fn gdiplus_startup(
    token: *mut usize,
    input: *const StartupInput,
    output: *mut StartupOutput,
) -> Status {
    if token.is_null() || input.is_null() {
        return status::INVALID_PARAMETER;
    }
    // SAFETY: the caller passes a readable `GdiplusStartupInput`.
    let input = unsafe { &*input };
    if !(1..=3).contains(&input.version) {
        return status::UNSUPPORTED_VERSION;
    }
    if input.suppress_background_thread != 0 && !output.is_null() {
        let hooks = StartupOutput {
            hook: export_entry!(notification_hook) as usize,
            unhook: export_entry!(notification_unhook) as usize,
        };
        // SAFETY: the caller passes a writable `GdiplusStartupOutput` with that flag.
        unsafe { output.write(hooks) };
    }
    let count = STARTED.fetch_add(1, Ordering::AcqRel) + 1;
    // SAFETY: the caller passes a writable token.
    unsafe { token.write(count) };
    status::OK
}

unsafe extern "efiapi" fn notification_hook(token: *mut usize) -> Status {
    if token.is_null() {
        return status::INVALID_PARAMETER;
    }
    // SAFETY: the caller passes a writable token.
    unsafe { token.write(1) };
    status::OK
}

extern "efiapi" fn notification_unhook(_token: usize) {}

extern "efiapi" fn gdiplus_shutdown(_token: usize) {
    let _ = STARTED.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        count.checked_sub(1)
    });
}

unsafe extern "efiapi" fn gdip_create_bitmap_from_file(
    name: *const u16,
    out: *mut *mut GpBitmap,
) -> Status {
    if name.is_null() || out.is_null() {
        return status::INVALID_PARAMETER;
    }
    if !started() {
        return status::NOT_INITIALIZED;
    }
    // SAFETY: the caller passes a NUL-terminated file name.
    let name = unsafe { super::file::wide_string(name) };
    let made = icon::read_file(name)
        .map_err(status_of)
        .and_then(|file| decode_file(file.bytes()))
        .and_then(bitmap_of);
    finish(made, out)
}

unsafe extern "efiapi" fn gdip_create_bitmap_from_scan0(
    width: i32,
    height: i32,
    stride: i32,
    format: u32,
    scan0: *mut u8,
    out: *mut *mut GpBitmap,
) -> Status {
    if width <= 0 || height <= 0 || out.is_null() {
        return status::INVALID_PARAMETER;
    }
    if !started() {
        return status::NOT_INITIALIZED;
    }
    let known = [
        PIXEL_FORMAT_32BPP_RGB,
        PIXEL_FORMAT_32BPP_ARGB,
        PIXEL_FORMAT_32BPP_PARGB,
    ];
    if !known.contains(&format) {
        return status::NOT_IMPLEMENTED;
    }
    let (width, height) = (width as u32, height as u32);
    let bitmap = |pixels: usize, stride: isize, owned: usize| GpBitmap {
        magic: BITMAP_MAGIC,
        format,
        width,
        height,
        stride,
        pixels,
        owned,
    };
    if !scan0.is_null() {
        if stride.unsigned_abs() < width * 4 {
            return status::INVALID_PARAMETER;
        }
        return finish(new_bitmap(bitmap(scan0 as usize, stride as isize, 0)), out);
    }
    let image = match Decoded::new(width, height) {
        Ok(image) => image,
        Err(code) => return status_of(code),
    };
    let pixels = image.pixels.into_raw() as usize;
    let made = new_bitmap(bitmap(pixels, width as isize * 4, pixels));
    if made.is_err() {
        // SAFETY: the pixels' block, which no bitmap holds.
        unsafe { heap::free(pixels as *mut u8) };
    }
    finish(made, out)
}

unsafe extern "efiapi" fn gdip_dispose_image(image: *mut GpBitmap) -> Status {
    // SAFETY: the program passes an image GDI+ gave it.
    let Some(found) = (unsafe { bitmap(image) }) else {
        return status::INVALID_PARAMETER;
    };
    let owned = found.owned;
    // SAFETY: the bitmap's block is going away: nothing reads its magic as a bitmap again.
    unsafe { (*image).magic = 0 };
    if owned != 0 {
        // SAFETY: the pixels' block this layer allocated for the bitmap, which is gone.
        unsafe { heap::free(owned as *mut u8) };
    }
    // SAFETY: the bitmap's own block, which the program no longer uses.
    unsafe { heap::free(image.cast()) };
    status::OK
}

unsafe fn answer(
    image: *const GpBitmap,
    out: *mut u32,
    value: impl FnOnce(&GpBitmap) -> u32,
) -> Status {
    // SAFETY: the program passes an image GDI+ gave it.
    let Some(found) = (unsafe { bitmap(image) }) else {
        return status::INVALID_PARAMETER;
    };
    if out.is_null() {
        return status::INVALID_PARAMETER;
    }
    // SAFETY: the caller passes a writable value.
    unsafe { out.write(value(found)) };
    status::OK
}

unsafe extern "efiapi" fn gdip_get_image_width(image: *const GpBitmap, width: *mut u32) -> Status {
    // SAFETY: as `answer` asks.
    unsafe { answer(image, width, |found| found.width) }
}

unsafe extern "efiapi" fn gdip_get_image_height(
    image: *const GpBitmap,
    height: *mut u32,
) -> Status {
    // SAFETY: as `answer` asks.
    unsafe { answer(image, height, |found| found.height) }
}

unsafe extern "efiapi" fn gdip_get_image_pixel_format(
    image: *const GpBitmap,
    format: *mut u32,
) -> Status {
    // SAFETY: as `answer` asks.
    unsafe { answer(image, format, |found| found.format) }
}

unsafe extern "efiapi" fn gdip_bitmap_get_pixel(
    image: *const GpBitmap,
    x: i32,
    y: i32,
    colour: *mut u32,
) -> Status {
    // SAFETY: the program passes an image GDI+ gave it.
    let Some(found) = (unsafe { bitmap(image) }) else {
        return status::INVALID_PARAMETER;
    };
    let inside = (0..found.width as i64).contains(&i64::from(x))
        && (0..found.height as i64).contains(&i64::from(y));
    if !inside || colour.is_null() {
        return status::INVALID_PARAMETER;
    }
    // SAFETY: the caller passes a writable `ARGB`.
    unsafe { colour.write(found.pixel(x as u32, y as u32)) };
    status::OK
}

unsafe extern "efiapi" fn gdip_create_hbitmap_from_bitmap(
    image: *const GpBitmap,
    out: *mut usize,
    background: u32,
) -> Status {
    // SAFETY: the program passes an image GDI+ gave it.
    let Some(found) = (unsafe { bitmap(image) }) else {
        return status::INVALID_PARAMETER;
    };
    if out.is_null() {
        return status::INVALID_PARAMETER;
    }
    let (handle, pixels) = match gdi::new_bitmap(found.width, found.height, true) {
        Ok(made) => made,
        Err(code) => return status_of(code),
    };
    let width = found.width as usize;
    // SAFETY: the new section's `width * height` pixels, which nothing else uses yet.
    let target = unsafe { core::slice::from_raw_parts_mut(pixels, width * found.height as usize) };
    for (index, pixel) in target.iter_mut().enumerate() {
        let (x, y) = ((index % width) as u32, (index / width) as u32);
        *pixel = dib::premultiplied_over(found.pixel(x, y), background);
    }
    // SAFETY: the caller passes a writable `HBITMAP`.
    unsafe { out.write(handle) };
    status::OK
}

unsafe extern "efiapi" fn gdip_create_from_hdc(hdc: usize, out: *mut *mut GpGraphics) -> Status {
    if out.is_null() {
        return status::INVALID_PARAMETER;
    }
    if !started() {
        return status::NOT_INITIALIZED;
    }
    if let Err(code) = gdi::known_context(hdc) {
        return super::fail(code, status::INVALID_PARAMETER);
    }
    let block = heap::allocate(core::mem::size_of::<GpGraphics>()).cast::<GpGraphics>();
    if block.is_null() {
        return status::OUT_OF_MEMORY;
    }
    // SAFETY: a fresh block, 16-byte aligned, of a `GpGraphics`' size.
    unsafe {
        block.write(GpGraphics {
            magic: GRAPHICS_MAGIC,
            hdc,
        })
    };
    finish(Ok(block), out)
}

unsafe fn graphics_of<'a>(graphics: *const GpGraphics) -> Option<&'a GpGraphics> {
    // SAFETY: the caller's duty: null or one of this layer's blocks, whose magic says which.
    let graphics = unsafe { graphics.as_ref()? };
    (graphics.magic == GRAPHICS_MAGIC).then_some(graphics)
}

unsafe extern "efiapi" fn gdip_delete_graphics(graphics: *mut GpGraphics) -> Status {
    // SAFETY: the program passes a graphics GDI+ gave it.
    if unsafe { graphics_of(graphics) }.is_none() {
        return status::INVALID_PARAMETER;
    }
    // SAFETY: the block is going away: nothing reads its magic as a graphics again.
    unsafe { (*graphics).magic = 0 };
    // SAFETY: the graphics' own block, which the program no longer uses.
    unsafe { heap::free(graphics.cast()) };
    status::OK
}

unsafe extern "efiapi" fn gdip_draw_image_rect_i(
    graphics: *const GpGraphics,
    image: *const GpBitmap,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Status {
    // SAFETY: the program passes a graphics GDI+ gave it.
    let graphics = unsafe { graphics_of(graphics) };
    // SAFETY: the program passes an image GDI+ gave it.
    let found = unsafe { bitmap(image) };
    let (Some(graphics), Some(found)) = (graphics, found) else {
        return status::INVALID_PARAMETER;
    };
    if width < 0 || height < 0 {
        return status::NOT_IMPLEMENTED;
    }
    if width == 0 || height == 0 {
        return status::OK;
    }
    let drawn = paint::draw_in(graphics.hdc, |surface, _, _| {
        let Some(surface) = surface else {
            return;
        };
        let area = Rect::sized(x, y, width, height).intersection(&surface.clip());
        for row in area.top..area.bottom {
            let source_y =
                (i64::from(row - y) * i64::from(found.height) / i64::from(height)) as u32;
            for column in area.left..area.right {
                let source_x =
                    (i64::from(column - x) * i64::from(found.width) / i64::from(width)) as u32;
                let Some(under) = surface.get(column, row) else {
                    continue;
                };
                surface.pixel(
                    column,
                    row,
                    dib::blend(found.pixel(source_x, source_y), under),
                );
            }
        }
    });
    match drawn {
        Ok(()) => status::OK,
        Err(code) => super::fail(code, status::WIN32_ERROR),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GdiplusStartup" => gdiplus_startup,
    b"GdiplusShutdown" => gdiplus_shutdown,
    b"GdipCreateBitmapFromFile" => gdip_create_bitmap_from_file,
    b"GdipCreateBitmapFromScan0" => gdip_create_bitmap_from_scan0,
    b"GdipDisposeImage" => gdip_dispose_image,
    b"GdipGetImageWidth" => gdip_get_image_width,
    b"GdipGetImageHeight" => gdip_get_image_height,
    b"GdipGetImagePixelFormat" => gdip_get_image_pixel_format,
    b"GdipBitmapGetPixel" => gdip_bitmap_get_pixel,
    b"GdipCreateHBITMAPFromBitmap" => gdip_create_hbitmap_from_bitmap,
    b"GdipCreateFromHDC" => gdip_create_from_hdc,
    b"GdipDeleteGraphics" => gdip_delete_graphics,
    b"GdipDrawImageRectI" => gdip_draw_image_rect_i,
];

const _: () = assert!(core::mem::size_of::<StartupInput>() == 24);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_read_as_straight_alpha_in_every_format() {
        let mut rows = [0x8080_8080u32, 0, 0x0012_3456, 0];
        let mut image = GpBitmap {
            magic: BITMAP_MAGIC,
            format: PIXEL_FORMAT_32BPP_PARGB,
            width: 1,
            height: 2,
            stride: 8,
            pixels: rows.as_mut_ptr() as usize,
            owned: 0,
        };
        assert_eq!(image.pixel(0, 0), 0x80ff_ffff);
        image.format = PIXEL_FORMAT_32BPP_RGB;
        assert_eq!(image.pixel(0, 1), 0xff12_3456);
        image.format = PIXEL_FORMAT_32BPP_ARGB;
        assert_eq!(image.pixel(0, 1), 0x0012_3456);
        image.pixels = rows[2..].as_mut_ptr() as usize;
        image.stride = -8;
        assert_eq!(image.pixel(0, 1), 0x8080_8080);
        assert_eq!(status_of(error::FILE_NOT_FOUND), status::FILE_NOT_FOUND);
        assert_eq!(status_of(error::NOT_ENOUGH_MEMORY), status::OUT_OF_MEMORY);
    }

    #[test]
    fn files_neither_png_nor_bmp_are_an_unknown_format() {
        let refused = decode_file(b"GIF89a").err();
        assert_eq!(refused, Some(status::UNKNOWN_IMAGE_FORMAT));
    }
}
