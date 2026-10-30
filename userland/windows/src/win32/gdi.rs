//! gdi32 for programs that draw off screen (D174): memory device contexts and bitmaps
//! (`CreateCompatibleDC`, `CreateCompatibleBitmap`, `BitBlt` and `PatBlt` with every raster
//! operation), polygons and polylines, rectangular clips and regions, fonts that are all Egg
//! Mono with its honest metrics (`GetTextMetrics`, `docs/ux/windows-controls-v0.3.md` section
//! 11.3), `ExtTextOutW` and the ANSI text functions, the device's capabilities, and printing,
//! which fails because EggOS has no printers yet.

use super::gui::{self, try_with, with};
use super::paint::{self, Point, Raster, Size, create, draw, raster_of, rect_of, units};
use super::user32::Win32Rect;
use super::{Bool, FALSE, TRUE, error, fail, heap};
use crate::gui::objects::{Bitmap, DeviceContext, Object};
use crate::library::Export;
use crate::text as layer_text;
use eggos_windows_gui::blit;
use eggos_windows_gui::colour;
use eggos_windows_gui::shapes::{self, MAX_POLYGON_POINTS};
use eggos_windows_gui::surface::Rect;
use eggos_windows_gui::text::{self, ASCENT, CELL_HEIGHT, CELL_WIDTH, DESCENT};

const LARGEST_BITMAP_PIXELS: usize = 16 * 1024 * 1024;
const LARGEST_BITMAP_SIDE: i32 = 16_384;
const COPY_CHUNK: usize = 512;
const NULL_REGION: i32 = 1;
const SIMPLE_REGION: i32 = 2;
const REGION_ERROR: i32 = 0;
const MM_TEXT: i32 = 1;
const ETO_OPAQUE: u32 = 0x2;
const ETO_CLIPPED: u32 = 0x4;
const SP_ERROR: i32 = -1;
const NARROW_TEXT_UNITS: usize = 1024;
const DOTS_PER_INCH: i32 = 96;
const MILLIMETRES_PER_INCH_TENTHS: i32 = 254;
const FW_NORMAL: i32 = 400;
const FF_MODERN: u8 = 0x30;
const DEFAULT_CHARACTER: u8 = b'?';

mod capability {
    pub const TECHNOLOGY: i32 = 2;
    pub const HORIZONTAL_SIZE: i32 = 4;
    pub const VERTICAL_SIZE: i32 = 6;
    pub const HORIZONTAL_RESOLUTION: i32 = 8;
    pub const VERTICAL_RESOLUTION: i32 = 10;
    pub const BITS_PER_PIXEL: i32 = 12;
    pub const PLANES: i32 = 14;
    pub const NUMBER_OF_COLOURS: i32 = 24;
    pub const RASTER_CAPABILITIES: i32 = 38;
    pub const PIXELS_PER_INCH_ACROSS: i32 = 88;
    pub const PIXELS_PER_INCH_DOWN: i32 = 90;
}

const RASTER_DISPLAY: i32 = 1;
const RC_BITBLT: i32 = 0x1;
const RC_BITMAP64: i32 = 0x8;
const RC_DI_BITMAP: i32 = 0x80;

fn with_context<R>(hdc: usize, work: impl FnOnce(&mut DeviceContext) -> R) -> Result<R, u32> {
    try_with(|gui| {
        let context = gui.contexts.get_mut(hdc).ok_or(error::INVALID_HANDLE)?;
        Ok(work(context))
    })
}

extern "efiapi" fn create_compatible_dc(hdc: usize) -> usize {
    let made = try_with(|gui| {
        if hdc != 0 && gui.contexts.get(hdc).is_none() {
            return Err(error::INVALID_HANDLE);
        }
        gui.contexts
            .insert(DeviceContext::memory())
            .ok_or(error::NOT_ENOUGH_MEMORY)
    });
    match made {
        Ok(context) => context,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn delete_dc(hdc: usize) -> Bool {
    match with(|gui| gui.contexts.remove(hdc)).flatten() {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn create_compatible_bitmap(hdc: usize, width: i32, height: i32) -> usize {
    if with(|gui| gui.contexts.get(hdc).is_some()) != Some(true) {
        return fail(error::INVALID_HANDLE, 0);
    }
    if width <= 0 || height <= 0 {
        return create(Object::Bitmap(Bitmap::EMPTY));
    }
    let pixels = width as usize * height as usize;
    let too_large = width > LARGEST_BITMAP_SIDE || height > LARGEST_BITMAP_SIDE;
    if too_large || pixels > LARGEST_BITMAP_PIXELS {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    let block = heap::allocate(pixels * 4);
    if block.is_null() {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    let bitmap = Bitmap {
        address: block as usize,
        width: width as u32,
        height: height as u32,
    };
    match with(|gui| gui.objects.insert(Object::Bitmap(bitmap))).flatten() {
        Some(handle) => handle,
        None => {
            // SAFETY: the block this call allocated, which no table holds.
            unsafe { heap::free(block) };
            fail(error::NOT_ENOUGH_MEMORY, 0)
        }
    }
}

fn whole_buffer(raster: &Raster) -> Rect {
    Rect::new(0, 0, raster.width as i32, raster.height as i32)
        .offset(-raster.origin.0, -raster.origin.1)
}

unsafe fn copy(
    to: &Raster,
    landed: Rect,
    from: Option<&Raster>,
    start: (i32, i32),
    operation: u32,
    pattern: u32,
) {
    let width = landed.width() as usize;
    let height = landed.height();
    let destination_column = (landed.left + to.origin.0) as usize;
    let destination_row = landed.top + to.origin.1;
    let (source_column, source_row) = match from {
        Some(from) => ((start.0 + from.origin.0) as usize, start.1 + from.origin.1),
        None => (0, 0),
    };
    let same = from.is_some_and(|from| from.address == to.address);
    let upwards = same && source_row < destination_row;
    let leftwards = same && source_row == destination_row && source_column < destination_column;
    let chunks = width.div_ceil(COPY_CHUNK);
    let mut row_buffer = [0u32; COPY_CHUNK];
    for step in 0..height {
        let row = match upwards {
            true => height - 1 - step,
            false => step,
        };
        let target_row = (destination_row + row) as usize * to.width as usize;
        for chunk_step in 0..chunks {
            let chunk = match leftwards {
                true => chunks - 1 - chunk_step,
                false => chunk_step,
            };
            let offset = chunk * COPY_CHUNK;
            let length = COPY_CHUNK.min(width - offset);
            let chunk_buffer = &mut row_buffer[..length];
            if let Some(from) = from {
                let read_at =
                    (source_row + row) as usize * from.width as usize + source_column + offset;
                // SAFETY: the caller's duty: these `length` source pixels lie in `from`'s
                // buffer; they are copied out before any pixel of this chunk is written.
                unsafe {
                    core::ptr::copy(
                        (from.address as *const u32).add(read_at),
                        chunk_buffer.as_mut_ptr(),
                        length,
                    )
                };
            }
            let write_at = target_row + destination_column + offset;
            // SAFETY: the caller's duty: these `length` destination pixels lie in `to`'s buffer.
            let target = unsafe {
                core::slice::from_raw_parts_mut((to.address as *mut u32).add(write_at), length)
            };
            for (pixel, source) in target.iter_mut().zip(chunk_buffer.iter()) {
                *pixel = blit::combine(operation, pattern, *source, *pixel);
            }
        }
    }
}

fn transfer(
    destination: usize,
    (x, y, width, height): (i32, i32, i32, i32),
    source: Option<(usize, i32, i32)>,
    operation: u32,
) -> Result<(), u32> {
    try_with(|gui| {
        let target = *gui.contexts.get(destination).ok_or(error::INVALID_HANDLE)?;
        let pattern = target.brush(&gui.objects).unwrap_or(0);
        let from = match source {
            Some((hdc, source_x, source_y)) => {
                let context = *gui.contexts.get(hdc).ok_or(error::INVALID_HANDLE)?;
                match raster_of(gui, &context) {
                    Some(raster) => Some((raster, source_x, source_y)),
                    None => return Ok(()),
                }
            }
            None => None,
        };
        let Some(to) = raster_of(gui, &target) else {
            return Ok(());
        };
        let reading =
            from.map(|(raster, source_x, source_y)| ((source_x, source_y), whole_buffer(&raster)));
        let Some((landed, start)) = blit::landing((x, y, width, height), to.clip, reading) else {
            return Ok(());
        };
        // SAFETY: both rasters come from the table under its lock; `landing` keeps `landed`
        // inside the destination's clip (inside its buffer) and the source rectangle inside the
        // source's buffer.
        unsafe {
            copy(
                &to,
                landed,
                from.as_ref().map(|(raster, _, _)| raster),
                start,
                operation,
                pattern,
            )
        };
        if let Some(stored) = gui.contexts.get_mut(destination) {
            stored.drawn = stored.drawn.union(&landed);
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn bit_blt(
    destination: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    source: usize,
    source_x: i32,
    source_y: i32,
    operation: u32,
) -> Bool {
    let source = blit::uses_source(operation).then_some((source, source_x, source_y));
    match transfer(destination, (x, y, width, height), source, operation) {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn pat_blt(
    destination: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    operation: u32,
) -> Bool {
    if blit::uses_source(operation) {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match transfer(destination, (x, y, width, height), None, operation) {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe fn read_points(
    points: *const Point,
    count: i32,
    out: &mut [(i32, i32); MAX_POLYGON_POINTS],
) -> Option<usize> {
    let count = usize::try_from(count).ok()?;
    if points.is_null() || count > MAX_POLYGON_POINTS {
        return None;
    }
    // SAFETY: the caller's duty: `count` readable `POINT`s.
    let points = unsafe { core::slice::from_raw_parts(points, count) };
    for (slot, point) in out.iter_mut().zip(points) {
        *slot = (point.x, point.y);
    }
    Some(count)
}

unsafe extern "efiapi" fn polygon(hdc: usize, points: *const Point, count: i32) -> Bool {
    let mut read = [(0, 0); MAX_POLYGON_POINTS];
    // SAFETY: `Polygon`'s contract: `count` readable `POINT`s.
    let Some(count) = (unsafe { read_points(points, count, &mut read) }).filter(|&n| n >= 2) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let drawn = draw(hdc, |surface, context, objects| {
        if let Some(surface) = surface {
            let (pen, brush) = (context.pen(objects), context.brush(objects));
            shapes::polygon(surface, &read[..count], pen, brush);
        }
    });
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn polyline(hdc: usize, points: *const Point, count: i32) -> Bool {
    let mut read = [(0, 0); MAX_POLYGON_POINTS];
    // SAFETY: `Polyline`'s contract: `count` readable `POINT`s.
    let Some(count) = (unsafe { read_points(points, count, &mut read) }).filter(|&n| n >= 2) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let drawn = draw(hdc, |surface, context, objects| {
        if let (Some(surface), Some(pen)) = (surface, context.pen(objects)) {
            shapes::polyline(surface, &read[..count], pen);
        }
    });
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn region_kind(clip: Rect) -> i32 {
    match clip.is_empty() {
        true => NULL_REGION,
        false => SIMPLE_REGION,
    }
}

extern "efiapi" fn intersect_clip_rect(
    hdc: usize,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> i32 {
    let rect = Rect::new(left, top, right, bottom).normalised();
    match with_context(hdc, |context| {
        context.clip = context.clip.intersection(&rect);
        region_kind(context.clip)
    }) {
        Ok(kind) => kind,
        Err(code) => fail(code, REGION_ERROR),
    }
}

extern "efiapi" fn select_clip_rgn(hdc: usize, region: usize) -> i32 {
    let selected = try_with(|gui| {
        let rect = match region {
            0 => None,
            region => match crate::gui::objects::object(&gui.objects, region) {
                Some(Object::Region(rect)) => Some(rect),
                _ => return Err(error::INVALID_HANDLE),
            },
        };
        let context = gui.contexts.get_mut(hdc).ok_or(error::INVALID_HANDLE)?;
        context.clip = match rect {
            Some(rect) => context.base_clip.intersection(&rect),
            None => context.base_clip,
        };
        Ok(region_kind(context.clip))
    });
    match selected {
        Ok(kind) => kind,
        Err(code) => fail(code, REGION_ERROR),
    }
}

extern "efiapi" fn create_rect_rgn(left: i32, top: i32, right: i32, bottom: i32) -> usize {
    create(Object::Region(
        Rect::new(left, top, right, bottom).normalised(),
    ))
}

unsafe extern "efiapi" fn create_rect_rgn_indirect(rect: *const Win32Rect) -> usize {
    if rect.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes a readable `RECT`.
    create(Object::Region(rect_of(unsafe { &*rect }).normalised()))
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn create_font(
    _height: i32,
    _width: i32,
    _escapement: i32,
    _orientation: i32,
    _weight: i32,
    _italic: u32,
    _underline: u32,
    _strike_out: u32,
    _character_set: u32,
    _output_precision: u32,
    _clip_precision: u32,
    _quality: u32,
    _pitch_and_family: u32,
    _face: usize,
) -> usize {
    create(Object::Font)
}

extern "efiapi" fn create_font_indirect(font: usize) -> usize {
    match font {
        0 => fail(error::INVALID_PARAMETER, 0),
        _ => create(Object::Font),
    }
}

#[repr(C)]
struct TextMetricW {
    height: i32,
    ascent: i32,
    descent: i32,
    internal_leading: i32,
    external_leading: i32,
    average_width: i32,
    maximum_width: i32,
    weight: i32,
    overhang: i32,
    aspect_x: i32,
    aspect_y: i32,
    first: u16,
    last: u16,
    default: u16,
    break_character: u16,
    italic: u8,
    underlined: u8,
    struck_out: u8,
    pitch_and_family: u8,
    character_set: u8,
}

#[repr(C)]
struct TextMetricA {
    height: i32,
    ascent: i32,
    descent: i32,
    internal_leading: i32,
    external_leading: i32,
    average_width: i32,
    maximum_width: i32,
    weight: i32,
    overhang: i32,
    aspect_x: i32,
    aspect_y: i32,
    first: u8,
    last: u8,
    default: u8,
    break_character: u8,
    italic: u8,
    underlined: u8,
    struck_out: u8,
    pitch_and_family: u8,
    character_set: u8,
}

fn known_context(hdc: usize) -> Result<(), u32> {
    try_with(|gui| {
        gui.contexts
            .get(hdc)
            .map(|_| ())
            .ok_or(error::INVALID_HANDLE)
    })
}

unsafe extern "efiapi" fn get_text_metrics_w(hdc: usize, metrics: *mut TextMetricW) -> Bool {
    if let Err(code) = known_context(hdc) {
        return fail(code, FALSE);
    }
    // SAFETY: the caller passes a writable `TEXTMETRICW`.
    unsafe {
        metrics.write(TextMetricW {
            height: CELL_HEIGHT,
            ascent: ASCENT,
            descent: DESCENT,
            internal_leading: 0,
            external_leading: 0,
            average_width: CELL_WIDTH,
            maximum_width: CELL_WIDTH,
            weight: FW_NORMAL,
            overhang: 0,
            aspect_x: DOTS_PER_INCH,
            aspect_y: DOTS_PER_INCH,
            first: u16::from(b' '),
            last: 0xfffd,
            default: u16::from(DEFAULT_CHARACTER),
            break_character: u16::from(b' '),
            italic: 0,
            underlined: 0,
            struck_out: 0,
            pitch_and_family: FF_MODERN,
            character_set: 0,
        })
    };
    TRUE
}

unsafe extern "efiapi" fn get_text_metrics_a(hdc: usize, metrics: *mut TextMetricA) -> Bool {
    if let Err(code) = known_context(hdc) {
        return fail(code, FALSE);
    }
    // SAFETY: the caller passes a writable `TEXTMETRICA`.
    unsafe {
        metrics.write(TextMetricA {
            height: CELL_HEIGHT,
            ascent: ASCENT,
            descent: DESCENT,
            internal_leading: 0,
            external_leading: 0,
            average_width: CELL_WIDTH,
            maximum_width: CELL_WIDTH,
            weight: FW_NORMAL,
            overhang: 0,
            aspect_x: DOTS_PER_INCH,
            aspect_y: DOTS_PER_INCH,
            first: b' ',
            last: 0xff,
            default: DEFAULT_CHARACTER,
            break_character: b' ',
            italic: 0,
            underlined: 0,
            struck_out: 0,
            pitch_and_family: FF_MODERN,
            character_set: 0,
        })
    };
    TRUE
}

unsafe fn narrow_text<'a>(text: *const u8, count: i32) -> &'a [u8] {
    if text.is_null() || count <= 0 {
        return &[];
    }
    // SAFETY: the caller's duty: `count` readable bytes.
    unsafe { core::slice::from_raw_parts(text, count as usize) }
}

unsafe extern "efiapi" fn get_text_extent_point_32_a(
    hdc: usize,
    text: *const u8,
    count: i32,
    size: *mut Size,
) -> Bool {
    if count < 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if let Err(code) = known_context(hdc) {
        return fail(code, FALSE);
    }
    // SAFETY: `GetTextExtentPoint32A`'s contract: `count` readable bytes.
    let characters = layer_text::utf8_chars(unsafe { narrow_text(text, count) }).count();
    // SAFETY: the caller passes a writable `SIZE`.
    unsafe {
        size.write(Size {
            width: (characters as i32).saturating_mul(CELL_WIDTH),
            height: CELL_HEIGHT,
        })
    };
    TRUE
}

fn widen_into<'a>(bytes: &[u8], out: &'a mut [u16; NARROW_TEXT_UNITS]) -> &'a [u16] {
    let written = layer_text::write_utf16(bytes, out);
    &out[..written]
}

fn text_out_in(
    hdc: usize,
    x: i32,
    y: i32,
    units: &[u16],
    options: u32,
    rect: Option<Rect>,
) -> Bool {
    let drawn = draw(hdc, |surface, context, _| {
        let Some(surface) = surface else {
            return;
        };
        let saved = surface.clip();
        if let Some(rect) = rect {
            if options & ETO_OPAQUE != 0 {
                surface.fill(rect, colour::pixel(context.background_colour));
            }
            if options & ETO_CLIPPED != 0 {
                surface.clip_to(rect);
            }
        }
        let foreground = colour::pixel(context.text_colour);
        text::text_out(surface, x, y, units, foreground, context.text_background());
        surface.set_clip(saved);
    });
    match drawn {
        Ok(()) => {
            let (width, height) = text::extent(units);
            paint::remember_drawn(hdc, Rect::sized(x, y, width, height), units);
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn ext_text_out_w(
    hdc: usize,
    x: i32,
    y: i32,
    options: u32,
    rect: *const Win32Rect,
    text: *const u16,
    count: u32,
    _spacing: *const i32,
) -> Bool {
    let rect = match rect.is_null() {
        true => None,
        // SAFETY: the caller passes a readable `RECT` or null.
        false => Some(rect_of(unsafe { &*rect })),
    };
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    // SAFETY: `ExtTextOutW`'s contract: `count` readable units.
    let units = unsafe { units(text, count) };
    text_out_in(hdc, x, y, units, options, rect)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn ext_text_out_a(
    hdc: usize,
    x: i32,
    y: i32,
    options: u32,
    rect: *const Win32Rect,
    text: *const u8,
    count: u32,
    _spacing: *const i32,
) -> Bool {
    let rect = match rect.is_null() {
        true => None,
        // SAFETY: the caller passes a readable `RECT` or null.
        false => Some(rect_of(unsafe { &*rect })),
    };
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    let mut wide = [0u16; NARROW_TEXT_UNITS];
    // SAFETY: `ExtTextOutA`'s contract: `count` readable bytes.
    let units = widen_into(unsafe { narrow_text(text, count) }, &mut wide);
    text_out_in(hdc, x, y, units, options, rect)
}

unsafe extern "efiapi" fn text_out_a(
    hdc: usize,
    x: i32,
    y: i32,
    text: *const u8,
    count: i32,
) -> Bool {
    if count < 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let mut wide = [0u16; NARROW_TEXT_UNITS];
    // SAFETY: `TextOutA`'s contract: `count` readable bytes.
    let units = widen_into(unsafe { narrow_text(text, count) }, &mut wide);
    text_out_in(hdc, x, y, units, 0, None)
}

extern "efiapi" fn get_device_caps(hdc: usize, index: i32) -> i32 {
    if known_context(hdc).is_err() {
        return fail(error::INVALID_HANDLE, 0);
    }
    let screen = gui::device()
        .map(|device| device.work_area)
        .unwrap_or_default();
    let millimetres = |pixels: i32| pixels * MILLIMETRES_PER_INCH_TENTHS / (DOTS_PER_INCH * 10);
    match index {
        capability::TECHNOLOGY => RASTER_DISPLAY,
        capability::HORIZONTAL_SIZE => millimetres(screen.right),
        capability::VERTICAL_SIZE => millimetres(screen.bottom),
        capability::HORIZONTAL_RESOLUTION => screen.right,
        capability::VERTICAL_RESOLUTION => screen.bottom,
        capability::BITS_PER_PIXEL => 32,
        capability::PLANES => 1,
        capability::NUMBER_OF_COLOURS => -1,
        capability::RASTER_CAPABILITIES => RC_BITBLT | RC_BITMAP64 | RC_DI_BITMAP,
        capability::PIXELS_PER_INCH_ACROSS | capability::PIXELS_PER_INCH_DOWN => DOTS_PER_INCH,
        _ => 0,
    }
}

extern "efiapi" fn set_map_mode(hdc: usize, mode: i32) -> i32 {
    if let Err(code) = known_context(hdc) {
        return fail(code, 0);
    }
    match mode {
        MM_TEXT => MM_TEXT,
        _ => fail(error::NOT_SUPPORTED, 0),
    }
}

extern "efiapi" fn create_hatch_brush(_style: i32, colour_ref: u32) -> usize {
    create(Object::Brush(Some(colour::pixel(colour_ref))))
}

extern "efiapi" fn start_doc(_hdc: usize, _document: usize) -> i32 {
    fail(error::NOT_SUPPORTED, SP_ERROR)
}

extern "efiapi" fn end_doc(_hdc: usize) -> i32 {
    fail(error::NOT_SUPPORTED, SP_ERROR)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateCompatibleDC" => create_compatible_dc,
    b"DeleteDC" => delete_dc,
    b"CreateCompatibleBitmap" => create_compatible_bitmap,
    b"BitBlt" => bit_blt [two_stack_arguments],
    b"PatBlt" => pat_blt,
    b"Polygon" => polygon,
    b"Polyline" => polyline,
    b"IntersectClipRect" => intersect_clip_rect,
    b"SelectClipRgn" => select_clip_rgn,
    b"CreateRectRgn" => create_rect_rgn,
    b"CreateRectRgnIndirect" => create_rect_rgn_indirect,
    b"CreateFontW" => create_font [six_stack_arguments],
    b"CreateFontA" => create_font [six_stack_arguments],
    b"CreateFontIndirectW" => create_font_indirect,
    b"CreateFontIndirectA" => create_font_indirect,
    b"GetTextMetricsW" => get_text_metrics_w,
    b"GetTextMetricsA" => get_text_metrics_a,
    b"GetTextExtentPoint32A" => get_text_extent_point_32_a,
    b"ExtTextOutW" => ext_text_out_w,
    b"ExtTextOutA" => ext_text_out_a,
    b"TextOutA" => text_out_a,
    b"GetDeviceCaps" => get_device_caps,
    b"SetMapMode" => set_map_mode,
    b"CreateHatchBrush" => create_hatch_brush,
    b"StartDocW" => start_doc,
    b"StartDocA" => start_doc,
    b"EndDoc" => end_doc,
    b"StartPage" => end_doc,
    b"EndPage" => end_doc,
    b"AbortDoc" => end_doc,
];

const _: () = assert!(core::mem::size_of::<TextMetricW>() == 60);
const _: () = assert!(core::mem::size_of::<TextMetricA>() == 56);
