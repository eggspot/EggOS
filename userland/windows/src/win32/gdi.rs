//! gdi32 for programs that draw off screen (D174): memory device contexts and bitmaps
//! (`CreateCompatibleDC`, `CreateCompatibleBitmap`, `BitBlt` and `PatBlt` with every raster
//! operation), polygons and polylines, rectangular clips and regions, fonts that are all Egg
//! Mono with its honest metrics (`GetTextMetrics`, `docs/ux/windows-controls-v0.3.md` section
//! 11.3), `ExtTextOutW` and the ANSI text functions, the device's capabilities, and printing,
//! which fails because EggOS has no printers yet. D178 adds DIB sections (`CreateDIBSection`,
//! `GetDIBits`, `GetObjectW`): a top-down 32-bit section's bits are the bitmap's own pixels,
//! any other format keeps the program's bits beside them and the two are brought together
//! around every drawing call that reaches the bitmap; pattern brushes, tiled from the brush
//! origin by `FillRect`, `PatBlt` and `BitBlt` (a shape filled with one uses its first pixel);
//! regions made from polygons, ellipses and rounded rectangles, kept as their bounding
//! rectangles, as every region is one rectangle; `EnumFontFamiliesExW`, which names Egg Mono
//! under any face asked for, as `CreateFontW` stands it in for any; and a screen device
//! context from `CreateDCW("DISPLAY")`.

use super::dib::{self, Layout};
use super::gui::{self, try_with, with};
use super::paint::{self, Point, Raster, Size, create, draw, raster_of, rect_of, units};
use super::user32::Win32Rect;
use super::{Bool, FALSE, SpinLock, TRUE, call_program, error, fail, heap};
use crate::gui::objects::{self, Bitmap, DEFAULT_BITMAP, DeviceContext, Object};
use crate::library::Export;
use crate::text as layer_text;
use core::sync::atomic::{AtomicUsize, Ordering};
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
const FIXED_PITCH: u8 = 0x1;
const DEFAULT_CHARACTER: u8 = b'?';
const ANSI_CHARSET: u8 = 0;
const DEFAULT_CHARSET: u8 = 1;
const RASTER_FONTTYPE: usize = 1;
const FACE_UNITS: usize = 32;
const CLR_INVALID: u32 = u32::MAX;
const DIB_PAL_COLORS: u32 = 1;
const BS_SOLID: u32 = 0;
const BS_NULL: u32 = 1;
const BS_PATTERN: u32 = 3;
const PS_SOLID: u32 = 0;
const PS_NULL: u32 = 5;
const MAX_SECTIONS: usize = 128;
const MAX_PATTERNS: usize = 32;
const EGG_MONO: &str = "Egg Mono";
const DISPLAY: &[u8] = b"DISPLAY";
const DISPLAY_DEVICE: &[u8] = b"\\\\.\\DISPLAY";
const MONOCHROME_PALETTE: [u8; 8] = [0, 0, 0, 0, 0xff, 0xff, 0xff, 0];

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

#[derive(Debug, Clone, Copy)]
struct Section {
    handle: usize,
    pixels: usize,
    bits: usize,
    palette: usize,
    layout: Layout,
}

impl Section {
    const EMPTY: Section = Section {
        handle: 0,
        pixels: 0,
        bits: 0,
        palette: 0,
        layout: Layout::EMPTY,
    };

    fn mirrored(&self) -> bool {
        self.bits != self.pixels
    }

    unsafe fn parts<'a>(&self) -> (&'a mut [u32], &'a mut [u8], &'a [u8]) {
        let count = self.layout.width as usize * self.layout.height as usize;
        let palette: &[u8] = match self.palette {
            0 => &[],
            // SAFETY: the caller's duty: the table block holds `colours` entries of 4 bytes.
            address => unsafe {
                core::slice::from_raw_parts(address as *const u8, self.layout.colours * 4)
            },
        };
        // SAFETY: the caller's duty: a live block of `count` pixels, distinct from the bits'.
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.pixels as *mut u32, count) };
        let size = self.layout.bits_size();
        // SAFETY: the caller's duty: a live block of the layout's rows.
        let bits = unsafe { core::slice::from_raw_parts_mut(self.bits as *mut u8, size) };
        (pixels, bits, palette)
    }

    unsafe fn free(&self) {
        let mut blocks = [self.pixels, 0, 0];
        if self.mirrored() {
            blocks[1] = self.bits;
            blocks[2] = self.palette;
        }
        for block in blocks.into_iter().filter(|block| *block != 0) {
            // SAFETY: the caller's duty; each is a heap block this module allocated.
            unsafe { heap::free(block as *mut u8) };
        }
    }
}

struct Sections {
    count: usize,
    entries: [Section; MAX_SECTIONS],
}

static SECTIONS: SpinLock<Sections> = SpinLock::new(Sections {
    count: 0,
    entries: [Section::EMPTY; MAX_SECTIONS],
});
static MIRRORED: AtomicUsize = AtomicUsize::new(0);

fn section_of(handle: usize) -> Option<Section> {
    SECTIONS.with(|sections| {
        sections.entries[..sections.count]
            .iter()
            .find(|section| section.handle == handle)
            .copied()
    })
}

fn take_section(handle: usize) -> Option<Section> {
    let taken = SECTIONS.with(|sections| {
        let index = sections.entries[..sections.count]
            .iter()
            .position(|section| section.handle == handle)?;
        sections.count -= 1;
        let last = sections.count;
        sections.entries.swap(index, last);
        Some(core::mem::replace(
            &mut sections.entries[last],
            Section::EMPTY,
        ))
    })?;
    if taken.mirrored() {
        MIRRORED.fetch_sub(1, Ordering::AcqRel);
    }
    Some(taken)
}

fn mirror(handle: usize) -> Option<Section> {
    if MIRRORED.load(Ordering::Acquire) == 0 {
        return None;
    }
    section_of(handle).filter(Section::mirrored)
}

/// Brings bitmap `handle`'s pixels up to date with the bits the program wrote, when it is a
/// DIB section whose bits are not its pixels; called under the state's lock, before the layer
/// reads or draws on it.
pub(super) fn load_section(handle: usize) {
    if let Some(section) = mirror(handle) {
        // SAFETY: the section is mirrored and its bitmap is in the table, under the state's
        // lock, so its blocks are alive and nothing else touches them.
        let (pixels, bits, palette) = unsafe { section.parts() };
        dib::decode(&section.layout, palette, bits, pixels);
    }
}

/// Writes rows `rows.top..rows.bottom` of bitmap `handle`'s pixels back to the program's bits,
/// when it is a DIB section whose bits are not its pixels; called under the state's lock,
/// after the layer drew on it.
pub(super) fn store_section(handle: usize, rows: Rect) {
    if rows.is_empty() {
        return;
    }
    if let Some(section) = mirror(handle) {
        // SAFETY: as in `load_section`.
        let (pixels, bits, palette) = unsafe { section.parts() };
        let range = rows.top.max(0) as usize..rows.bottom.max(0) as usize;
        dib::encode(&section.layout, palette, pixels, bits, range);
    }
}

/// A pattern brush's tile: a copy of the bitmap it was made from, in a heap block.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pattern {
    brush: usize,
    bitmap: usize,
    pixels: usize,
    width: u32,
    height: u32,
}

impl Pattern {
    const EMPTY: Pattern = Pattern {
        brush: 0,
        bitmap: 0,
        pixels: 0,
        width: 0,
        height: 0,
    };

    /// The tile's pixel at (`x`, `y`) of a context whose brush origin is `origin`; called
    /// under the state's lock, while the brush is in the table.
    pub(super) fn at(&self, (x, y): (i32, i32), origin: (i32, i32)) -> u32 {
        let column = x.wrapping_sub(origin.0).rem_euclid(self.width as i32) as usize;
        let row = y.wrapping_sub(origin.1).rem_euclid(self.height as i32) as usize;
        let at = (self.pixels as *const u32).wrapping_add(row * self.width as usize + column);
        // SAFETY: the tile holds `width * height` pixels and is freed only after its brush
        // left the table, which takes the state's lock this runs under.
        let pixel = unsafe { at.read() };
        pixel & 0x00ff_ffff
    }
}

struct Patterns {
    count: usize,
    entries: [Pattern; MAX_PATTERNS],
}

static PATTERNS: SpinLock<Patterns> = SpinLock::new(Patterns {
    count: 0,
    entries: [Pattern::EMPTY; MAX_PATTERNS],
});
static PATTERN_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The tile of pattern brush `brush`, if it is one.
pub(super) fn pattern(brush: usize) -> Option<Pattern> {
    if PATTERN_COUNT.load(Ordering::Acquire) == 0 {
        return None;
    }
    PATTERNS.with(|patterns| {
        patterns.entries[..patterns.count]
            .iter()
            .find(|pattern| pattern.brush == brush)
            .copied()
    })
}

fn take_pattern(brush: usize) -> Option<Pattern> {
    let taken = PATTERNS.with(|patterns| {
        let index = patterns.entries[..patterns.count]
            .iter()
            .position(|pattern| pattern.brush == brush)?;
        patterns.count -= 1;
        let last = patterns.count;
        patterns.entries.swap(index, last);
        Some(core::mem::replace(
            &mut patterns.entries[last],
            Pattern::EMPTY,
        ))
    })?;
    PATTERN_COUNT.fetch_sub(1, Ordering::AcqRel);
    Some(taken)
}

/// Frees what object `handle` (just taken out of the table by `DeleteObject`) owned: a
/// bitmap's pixels (and a DIB section's bits), a pattern brush's tile.
pub(super) fn release(handle: usize, object: Object) {
    match object {
        Object::Bitmap(bitmap) => match take_section(handle) {
            // SAFETY: the bitmap left the table and no context holds it.
            Some(section) => unsafe { section.free() },
            None if bitmap.address != 0 => {
                // SAFETY: a bitmap's pixels are a heap block of this layer's; the bitmap left
                // the table and no context holds it, so nothing reaches them.
                unsafe { heap::free(bitmap.address as *mut u8) }
            }
            None => {}
        },
        Object::Brush(_) => {
            if let Some(pattern) = take_pattern(handle) {
                // SAFETY: the brush left the table, so nothing reads its tile any more.
                unsafe { heap::free(pattern.pixels as *mut u8) };
            }
        }
        _ => {}
    }
}

fn allocate_pixels(width: u32, height: u32) -> Result<*mut u32, u32> {
    let pixels = width as usize * height as usize;
    if width == 0 || height == 0 {
        return Err(error::INVALID_PARAMETER);
    }
    let side = LARGEST_BITMAP_SIDE as u32;
    if width > side || height > side || pixels > LARGEST_BITMAP_PIXELS {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    let block = heap::allocate(pixels * 4);
    match block.is_null() {
        true => Err(error::NOT_ENOUGH_MEMORY),
        false => Ok(block.cast()),
    }
}

fn insert_bitmap(bitmap: Bitmap) -> Result<usize, u32> {
    with(|gui| gui.objects.insert(Object::Bitmap(bitmap)))
        .flatten()
        .ok_or(error::NOT_ENOUGH_MEMORY)
}

fn remove_object(handle: usize) {
    with(|gui| gui.objects.remove(handle));
}

fn make_section(layout: Layout, palette: &[u8]) -> Result<(usize, usize), u32> {
    let pixels = allocate_pixels(layout.width, layout.height)? as usize;
    let mut section = Section {
        pixels,
        bits: pixels,
        layout,
        ..Section::EMPTY
    };
    if !layout.is_native() {
        let bits = heap::allocate(layout.bits_size());
        let table = match palette.is_empty() {
            true => core::ptr::null_mut(),
            false => heap::allocate(palette.len()),
        };
        if bits.is_null() || (table.is_null() && !palette.is_empty()) {
            for block in [pixels as *mut u8, bits, table] {
                if !block.is_null() {
                    // SAFETY: a block this call allocated, which nothing else knows.
                    unsafe { heap::free(block) };
                }
            }
            return Err(error::NOT_ENOUGH_MEMORY);
        }
        if !table.is_null() {
            // SAFETY: `table` is a fresh block of `palette.len()` bytes.
            unsafe { core::ptr::copy_nonoverlapping(palette.as_ptr(), table, palette.len()) };
        }
        section.bits = bits as usize;
        section.palette = table as usize;
    }
    let bitmap = Bitmap {
        address: pixels,
        width: layout.width,
        height: layout.height,
    };
    let handle = match insert_bitmap(bitmap) {
        Ok(handle) => handle,
        Err(code) => {
            // SAFETY: blocks this call allocated, which no table holds.
            unsafe { section.free() };
            return Err(code);
        }
    };
    section.handle = handle;
    let added = SECTIONS.with(|sections| {
        let slot = sections.entries.get_mut(sections.count)?;
        *slot = section;
        sections.count += 1;
        Some(())
    });
    if added.is_none() {
        remove_object(handle);
        // SAFETY: the bitmap left the table before any context could select it.
        unsafe { section.free() };
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    if section.mirrored() {
        MIRRORED.fetch_add(1, Ordering::AcqRel);
    }
    Ok((handle, section.bits))
}

/// A new bitmap of `width` by `height` zeroed pixels, a top-down 32-bit DIB section when
/// `section` is set: its handle and its pixels, which the caller may fill before handing the
/// handle out.
pub(super) fn new_bitmap(width: u32, height: u32, section: bool) -> Result<(usize, *mut u32), u32> {
    if section {
        let (handle, bits) = make_section(Layout::native(width, height), &[])?;
        return Ok((handle, bits as *mut u32));
    }
    let pixels = allocate_pixels(width, height)?;
    let bitmap = Bitmap {
        address: pixels as usize,
        width,
        height,
    };
    match insert_bitmap(bitmap) {
        Ok(handle) => Ok((handle, pixels)),
        Err(code) => {
            // SAFETY: the block this call allocated, which no table holds.
            unsafe { heap::free(pixels.cast()) };
            Err(code)
        }
    }
}

/// Deletes bitmap `handle`, as `DeleteObject` does: not while a memory context holds it.
pub(super) fn delete_bitmap(handle: usize) {
    let removed = with(|gui| {
        let selected = gui
            .contexts
            .iter()
            .any(|(_, context)| context.memory && context.bitmap == handle);
        let bitmap = matches!(gui.objects.get(handle), Some(Object::Bitmap(_)));
        match bitmap && !selected {
            true => gui.objects.remove(handle),
            false => None,
        }
    });
    if let Some(object) = removed.flatten() {
        release(handle, object);
    }
}

/// Runs `work` on bitmap `handle` and its pixels (a DIB section's brought up to date with its
/// bits) under the state's lock.
pub(super) fn read_bitmap<R>(
    handle: usize,
    work: impl FnOnce(Bitmap, &[u32]) -> R,
) -> Result<R, u32> {
    try_with(|gui| {
        let Some(Object::Bitmap(bitmap)) = objects::object(&gui.objects, handle) else {
            return Err(error::INVALID_HANDLE);
        };
        load_section(handle);
        let pixels: &[u32] = match bitmap.address {
            0 => &[0],
            // SAFETY: a bitmap's block holds `width * height` pixels while it is in the table,
            // and the table's lock is held.
            address => unsafe {
                core::slice::from_raw_parts(
                    address as *const u32,
                    bitmap.width as usize * bitmap.height as usize,
                )
            },
        };
        Ok(work(bitmap, pixels))
    })
}

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
    match new_bitmap(width as u32, height as u32, false) {
        Ok((handle, _)) => handle,
        Err(code) => fail(code, 0),
    }
}

fn whole_buffer(raster: &Raster) -> Rect {
    Rect::new(0, 0, raster.width as i32, raster.height as i32)
        .offset(-raster.origin.0, -raster.origin.1)
}

#[derive(Debug, Clone, Copy)]
enum Fill {
    Solid(u32),
    Tile(Pattern, (i32, i32)),
}

impl Fill {
    fn at(&self, point: (i32, i32)) -> u32 {
        match self {
            Fill::Solid(colour) => *colour,
            Fill::Tile(pattern, origin) => pattern.at(point, *origin),
        }
    }
}

unsafe fn copy(
    to: &Raster,
    landed: Rect,
    from: Option<&Raster>,
    start: (i32, i32),
    operation: u32,
    fill: Fill,
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
                // buffer.
                let source_start = unsafe { (from.address as *const u32).add(read_at) };
                // SAFETY: as above; they are copied out before any pixel of this chunk is
                // written.
                unsafe { core::ptr::copy(source_start, chunk_buffer.as_mut_ptr(), length) };
            }
            let write_at = target_row + destination_column + offset;
            // SAFETY: the caller's duty: these `length` destination pixels lie in `to`'s buffer.
            let target_start = unsafe { (to.address as *mut u32).add(write_at) };
            // SAFETY: as above.
            let target = unsafe { core::slice::from_raw_parts_mut(target_start, length) };
            let pixels = target.iter_mut().zip(chunk_buffer.iter());
            match fill {
                Fill::Solid(pattern) => {
                    for (pixel, source) in pixels {
                        *pixel = blit::combine(operation, pattern, *source, *pixel);
                    }
                }
                Fill::Tile(..) => {
                    let first_x = landed.left + offset as i32;
                    for (index, (pixel, source)) in pixels.enumerate() {
                        let pattern = fill.at((first_x + index as i32, landed.top + row));
                        *pixel = blit::combine(operation, pattern, *source, *pixel);
                    }
                }
            }
        }
    }
}

fn memory_bitmap(context: &DeviceContext) -> Option<usize> {
    context.memory.then_some(context.bitmap)
}

fn transfer(
    destination: usize,
    (x, y, width, height): (i32, i32, i32, i32),
    source: Option<(usize, i32, i32)>,
    operation: u32,
) -> Result<(), u32> {
    try_with(|gui| {
        let target = *gui.contexts.get(destination).ok_or(error::INVALID_HANDLE)?;
        let fill = match target.brush(&gui.objects) {
            Some(colour) => pattern(target.brush).map_or(Fill::Solid(colour), |tile| {
                Fill::Tile(tile, target.brush_origin)
            }),
            None => Fill::Solid(0),
        };
        let from = match source {
            Some((hdc, source_x, source_y)) => {
                let context = *gui.contexts.get(hdc).ok_or(error::INVALID_HANDLE)?;
                if let Some(bitmap) = memory_bitmap(&context) {
                    load_section(bitmap);
                }
                match raster_of(gui, &context) {
                    Some(raster) => Some((raster, source_x, source_y)),
                    None => return Ok(()),
                }
            }
            None => None,
        };
        if let Some(bitmap) = memory_bitmap(&target) {
            load_section(bitmap);
        }
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
                fill,
            )
        };
        if let Some(bitmap) = memory_bitmap(&target) {
            store_section(bitmap, landed);
        }
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
            region => match objects::object(&gui.objects, region) {
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

unsafe extern "efiapi" fn get_clip_box(hdc: usize, rect: *mut Win32Rect) -> i32 {
    let clip = try_with(|gui| {
        let context = *gui.contexts.get(hdc).ok_or(error::INVALID_HANDLE)?;
        Ok(match raster_of(gui, &context) {
            Some(raster) => raster.clip,
            None => match context.selected_bitmap(&gui.objects) {
                Some(bitmap) => bitmap.bounds().intersection(&context.clip),
                None => context.clip,
            },
        })
    });
    let clip = match clip {
        Ok(clip) if clip.is_empty() => Rect::default(),
        Ok(clip) => clip,
        Err(code) => return fail(code, REGION_ERROR),
    };
    if rect.is_null() {
        return fail(error::INVALID_PARAMETER, REGION_ERROR);
    }
    // SAFETY: the caller passes a writable `RECT`.
    unsafe {
        rect.write(Win32Rect {
            left: clip.left,
            top: clip.top,
            right: clip.right,
            bottom: clip.bottom,
        })
    };
    region_kind(clip)
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

fn bounding_box(points: &[Point]) -> Rect {
    let mut bounds = Rect::new(i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for point in points {
        bounds = Rect::new(
            bounds.left.min(point.x),
            bounds.top.min(point.y),
            bounds.right.max(point.x),
            bounds.bottom.max(point.y),
        );
    }
    bounds
}

unsafe extern "efiapi" fn create_polygon_rgn(
    points: *const Point,
    count: i32,
    _mode: i32,
) -> usize {
    let count = match usize::try_from(count) {
        Ok(count) if count >= 1 && !points.is_null() => count,
        _ => return fail(error::INVALID_PARAMETER, 0),
    };
    // SAFETY: `CreatePolygonRgn`'s contract: `count` readable `POINT`s.
    let points = unsafe { core::slice::from_raw_parts(points, count) };
    create(Object::Region(bounding_box(points)))
}

extern "efiapi" fn create_elliptic_rgn(left: i32, top: i32, right: i32, bottom: i32) -> usize {
    create_rect_rgn(left, top, right, bottom)
}

extern "efiapi" fn create_round_rect_rgn(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    _corner_width: i32,
    _corner_height: i32,
) -> usize {
    create_rect_rgn(left, top, right, bottom)
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

fn egg_mono_metrics() -> TextMetricW {
    TextMetricW {
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
        character_set: ANSI_CHARSET,
    }
}

/// `Ok` when `hdc` is a device context of the program's.
pub(super) fn known_context(hdc: usize) -> Result<(), u32> {
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
    unsafe { metrics.write(egg_mono_metrics()) };
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
            character_set: ANSI_CHARSET,
        })
    };
    TRUE
}

#[repr(C)]
struct Abc {
    before: i32,
    glyph: u32,
    after: i32,
}

unsafe extern "efiapi" fn get_char_abc_widths_w(
    hdc: usize,
    first: u32,
    last: u32,
    widths: *mut Abc,
) -> Bool {
    if let Err(code) = known_context(hdc) {
        return fail(code, FALSE);
    }
    if last < first || widths.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    for index in 0..=(last - first) as usize {
        let slot = widths.wrapping_add(index);
        // SAFETY: the caller passes `last - first + 1` writable `ABC`s.
        unsafe {
            slot.write_unaligned(Abc {
                before: 0,
                glyph: CELL_WIDTH as u32,
                after: 0,
            })
        };
    }
    TRUE
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LogFont {
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u8,
    underline: u8,
    strike_out: u8,
    character_set: u8,
    output_precision: u8,
    clip_precision: u8,
    quality: u8,
    pitch_and_family: u8,
    face: [u16; FACE_UNITS],
}

#[repr(C)]
struct EnumLogFontEx {
    font: LogFont,
    full_name: [u16; 64],
    style: [u16; 32],
    script: [u16; 32],
}

#[repr(C)]
struct NewTextMetricEx {
    metric: TextMetricW,
    flags: u32,
    size_em: u32,
    cell_height: u32,
    average_width: u32,
    signature: [u32; 6],
}

fn put_text(out: &mut [u16], text: impl Iterator<Item = u16>) {
    let room = out.len().saturating_sub(1);
    for (slot, unit) in out[..room].iter_mut().zip(text) {
        *slot = unit;
    }
}

fn egg_mono_font(face: &[u16]) -> LogFont {
    let mut font = LogFont {
        height: CELL_HEIGHT,
        width: CELL_WIDTH,
        escapement: 0,
        orientation: 0,
        weight: FW_NORMAL,
        italic: 0,
        underline: 0,
        strike_out: 0,
        character_set: ANSI_CHARSET,
        output_precision: 0,
        clip_precision: 0,
        quality: 0,
        pitch_and_family: FIXED_PITCH | FF_MODERN,
        face: [0; FACE_UNITS],
    };
    match face.is_empty() {
        true => put_text(&mut font.face, EGG_MONO.encode_utf16()),
        false => put_text(&mut font.face, face.iter().copied()),
    }
    font
}

fn face_of(font: &LogFont) -> &[u16] {
    let length = font
        .face
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(FACE_UNITS);
    &font.face[..length]
}

unsafe extern "efiapi" fn enum_font_families_ex_w(
    hdc: usize,
    wanted: *const LogFont,
    callback: usize,
    parameter: usize,
    _flags: u32,
) -> i32 {
    if let Err(code) = known_context(hdc) {
        return fail(code, 0);
    }
    if wanted.is_null() || callback == 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes a readable `LOGFONTW`.
    let wanted = unsafe { wanted.read_unaligned() };
    if !matches!(wanted.character_set, ANSI_CHARSET | DEFAULT_CHARSET) {
        return 1;
    }
    let font = egg_mono_font(face_of(&wanted));
    let mut described = EnumLogFontEx {
        font,
        full_name: [0; 64],
        style: [0; 32],
        script: [0; 32],
    };
    put_text(&mut described.full_name, face_of(&font).iter().copied());
    put_text(&mut described.style, "Regular".encode_utf16());
    put_text(&mut described.script, "Western".encode_utf16());
    let metric = NewTextMetricEx {
        metric: egg_mono_metrics(),
        flags: 0,
        size_em: CELL_HEIGHT as u32,
        cell_height: CELL_HEIGHT as u32,
        average_width: CELL_WIDTH as u32,
        signature: [0; 6],
    };
    let arguments = [
        &raw const described as usize,
        &raw const metric as usize,
        RASTER_FONTTYPE,
        parameter,
    ];
    // SAFETY: `EnumFontFamiliesExW`'s contract: `callback` is the program's
    // `EnumFontFamExProc`, which takes these four arguments; both structures live through the
    // call.
    unsafe { call_program(callback, arguments) as i32 }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BitmapHeader {
    kind: i32,
    width: i32,
    height: i32,
    width_bytes: i32,
    planes: u16,
    bits_per_pixel: u16,
    reserved: u32,
    bits: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pixels_per_metre: i32,
    y_pixels_per_metre: i32,
    colours_used: u32,
    colours_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DibSection {
    bitmap: BitmapHeader,
    header: InfoHeader,
    bit_fields: [u32; 3],
    reserved: u32,
    section: usize,
    offset: u32,
    tail: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LogBrush {
    style: u32,
    colour: u32,
    hatch: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LogPen {
    style: u32,
    width: [i32; 2],
    colour: u32,
}

impl Section {
    fn describe(&self) -> DibSection {
        let layout = self.layout;
        let height = match layout.bottom_up {
            true => layout.height as i32,
            false => -(layout.height as i32),
        };
        let bit_fields = match layout.compression {
            0 => [0; 3],
            _ => layout.masks,
        };
        DibSection {
            bitmap: BitmapHeader {
                kind: 0,
                width: layout.width as i32,
                height: layout.height as i32,
                width_bytes: layout.stride() as i32,
                planes: 1,
                bits_per_pixel: layout.bits,
                reserved: 0,
                bits: self.bits,
            },
            header: InfoHeader {
                size: dib::INFO_HEADER_SIZE as u32,
                width: layout.width as i32,
                height,
                planes: 1,
                bit_count: layout.bits,
                compression: layout.compression,
                size_image: layout.bits_size() as u32,
                x_pixels_per_metre: 0,
                y_pixels_per_metre: 0,
                colours_used: if layout.bits <= 8 {
                    layout.colours as u32
                } else {
                    0
                },
                colours_important: 0,
            },
            bit_fields,
            reserved: 0,
            section: 0,
            offset: 0,
            tail: 0,
        }
    }
}

enum Description {
    Bitmap(BitmapHeader),
    Section(DibSection),
    Brush(LogBrush),
    Pen(LogPen),
    Font(LogFont),
}

fn describe(handle: usize) -> Result<Description, u32> {
    let object =
        try_with(|gui| objects::object(&gui.objects, handle).ok_or(error::INVALID_HANDLE))?;
    Ok(match object {
        Object::Bitmap(bitmap) => match section_of(handle) {
            Some(section) => Description::Section(section.describe()),
            None => {
                let bits: u16 = if handle == DEFAULT_BITMAP { 1 } else { 32 };
                Description::Bitmap(BitmapHeader {
                    kind: 0,
                    width: bitmap.width as i32,
                    height: bitmap.height as i32,
                    width_bytes: (bitmap.width as usize * usize::from(bits)).div_ceil(16) as i32
                        * 2,
                    planes: 1,
                    bits_per_pixel: bits,
                    reserved: 0,
                    bits: 0,
                })
            }
        },
        Object::Brush(colour) => Description::Brush(match (pattern(handle), colour) {
            (Some(tile), _) => LogBrush {
                style: BS_PATTERN,
                colour: 0,
                hatch: tile.bitmap,
            },
            (None, Some(pixel)) => LogBrush {
                style: BS_SOLID,
                colour: colour::colour_ref(pixel),
                hatch: 0,
            },
            (None, None) => LogBrush {
                style: BS_NULL,
                colour: 0,
                hatch: 0,
            },
        }),
        Object::Pen(pen) => Description::Pen(match pen {
            Some(pen) => LogPen {
                style: PS_SOLID,
                width: [pen.width as i32, 0],
                colour: colour::colour_ref(pen.colour),
            },
            None => LogPen {
                style: PS_NULL,
                width: [0, 0],
                colour: 0,
            },
        }),
        Object::Font => Description::Font(egg_mono_font(&[])),
        Object::Region(_) => return Err(error::INVALID_PARAMETER),
    })
}

fn bytes_of<T>(value: &T) -> &[u8] {
    // SAFETY: `value` is a live `T` of `size_of::<T>()` bytes with no padding, read as bytes
    // for as long as it is borrowed.
    unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    }
}

unsafe extern "efiapi" fn get_object_w(handle: usize, size: i32, buffer: *mut u8) -> i32 {
    let description = match describe(handle) {
        Ok(description) => description,
        Err(code) => return fail(code, 0),
    };
    let section_size = core::mem::size_of::<DibSection>() as i32;
    let bytes = match &description {
        Description::Section(section) if buffer.is_null() || size >= section_size => {
            bytes_of(section)
        }
        Description::Section(section) => bytes_of(&section.bitmap),
        Description::Bitmap(bitmap) => bytes_of(bitmap),
        Description::Brush(brush) => bytes_of(brush),
        Description::Pen(pen) => bytes_of(pen),
        Description::Font(font) => bytes_of(font),
    };
    if buffer.is_null() {
        return bytes.len() as i32;
    }
    let count = bytes.len().min(usize::try_from(size).unwrap_or(0));
    // SAFETY: `GetObjectW`'s contract: `buffer` holds `size` writable bytes.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, count) };
    count as i32
}

unsafe fn read_info<'a>(info: *const u8) -> Option<(Layout, &'a [u8])> {
    if info.is_null() {
        return None;
    }
    // SAFETY: the caller's duty: a `BITMAPINFOHEADER` at least.
    let header = unsafe { core::slice::from_raw_parts(info, dib::INFO_HEADER_SIZE) };
    let size = Layout::info_size(header)?;
    // SAFETY: the caller's duty: the header, then the masks and colour table it announces.
    let whole = unsafe { core::slice::from_raw_parts(info, size) };
    let layout = Layout::parse(whole)?;
    Some((
        layout,
        whole.get(layout.table_offset..layout.palette_end())?,
    ))
}

unsafe extern "efiapi" fn create_dib_section(
    _hdc: usize,
    info: *const u8,
    usage: u32,
    bits: *mut usize,
    file_mapping: usize,
    _offset: u32,
) -> usize {
    if !bits.is_null() {
        // SAFETY: the caller passes a writable pointer for the bits' address, or null.
        unsafe { bits.write(0) };
    }
    if file_mapping != 0 {
        return fail(error::NOT_SUPPORTED, 0);
    }
    // SAFETY: `CreateDIBSection`'s contract: a readable `BITMAPINFO`.
    let Some((layout, palette)) = (unsafe { read_info(info) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if usage == DIB_PAL_COLORS && layout.bits <= 8 {
        return fail(error::NOT_SUPPORTED, 0);
    }
    match make_section(layout, palette) {
        Ok((handle, address)) => {
            if !bits.is_null() {
                // SAFETY: as above.
                unsafe { bits.write(address) };
            }
            handle
        }
        Err(code) => fail(code, 0),
    }
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    if let Some(field) = bytes.get_mut(at..at + 4) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

fn describe_into(header: &mut [u8], (width, height): (u32, u32)) {
    header.fill(0);
    put_u32(header, 0, dib::INFO_HEADER_SIZE as u32);
    put_u32(header, 4, width);
    put_u32(header, 8, height);
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&32u16.to_le_bytes());
    put_u32(header, 20, width * 4 * height);
}

unsafe extern "efiapi" fn get_di_bits(
    hdc: usize,
    bitmap: usize,
    start: u32,
    lines: u32,
    bits: *mut u8,
    info: *mut u8,
    usage: u32,
) -> i32 {
    if let Err(code) = known_context(hdc) {
        return fail(code, 0);
    }
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let size = match read_bitmap(bitmap, |bitmap, _| (bitmap.width, bitmap.height)) {
        Ok(size) => size,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: `GetDIBits`' contract: a readable and writable `BITMAPINFOHEADER` at least.
    let header_size = unsafe { info.cast::<u32>().read_unaligned() } as usize;
    // SAFETY: as above: `biBitCount`, inside the header.
    let bit_count = unsafe { info.wrapping_add(14).cast::<u16>().read_unaligned() };
    if header_size < dib::INFO_HEADER_SIZE {
        return fail(error::INVALID_PARAMETER, 0);
    }
    if bit_count == 0 {
        // SAFETY: as above.
        let header = unsafe { core::slice::from_raw_parts_mut(info, dib::INFO_HEADER_SIZE) };
        describe_into(header, size);
        return size.1 as i32;
    }
    // SAFETY: as above, with the masks and colour table the header announces.
    let Some(layout) = (unsafe { read_info(info) }).map(|(layout, _)| layout) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if !matches!(layout.bits, 1 | 16 | 24 | 32) || (usage == DIB_PAL_COLORS && layout.bits <= 8) {
        return fail(error::NOT_SUPPORTED, 0);
    }
    let table_size = layout.palette_end() - layout.table_offset;
    // SAFETY: the colour table `read_info` found lies in the caller's writable `BITMAPINFO`,
    // after the header.
    let table = unsafe {
        core::slice::from_raw_parts_mut(info.wrapping_add(layout.table_offset), table_size)
    };
    if layout.bits == 1 {
        let entries = table.len().min(MONOCHROME_PALETTE.len());
        table[..entries].copy_from_slice(&MONOCHROME_PALETTE[..entries]);
    }
    let size_image = info.wrapping_add(20).cast::<u32>();
    // SAFETY: `biSizeImage`, inside the caller's writable header.
    unsafe { size_image.write_unaligned(layout.bits_size() as u32) };
    if bits.is_null() {
        return layout.height as i32;
    }
    let palette: &[u8] = table;
    let stride = layout.stride();
    let copied = lines.min(layout.height.saturating_sub(start)) as usize;
    // SAFETY: `GetDIBits`' contract: `bits` holds `lines` rows of the layout's stride.
    let out = unsafe { core::slice::from_raw_parts_mut(bits, copied * stride) };
    let written = read_bitmap(bitmap, |source, pixels| {
        let width = source.width as usize;
        for (index, target) in out.chunks_exact_mut(stride).enumerate() {
            let scan = start as usize + index;
            let row = match layout.bottom_up {
                true => layout.height as usize - 1 - scan,
                false => scan,
            };
            target.fill(0);
            if let Some(from) = pixels.get(row * width..row * width + width) {
                let columns = width.min(layout.width as usize);
                dib::write_row(&layout, palette, &from[..columns], target);
            }
        }
        copied as i32
    });
    match written {
        Ok(lines) => lines,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn gdi_flush() -> Bool {
    TRUE
}

unsafe extern "efiapi" fn set_brush_org_ex(
    hdc: usize,
    x: i32,
    y: i32,
    previous: *mut Point,
) -> Bool {
    match with_context(hdc, |context| {
        core::mem::replace(&mut context.brush_origin, (x, y))
    }) {
        Ok((old_x, old_y)) => {
            if !previous.is_null() {
                // SAFETY: the caller passes a writable `POINT` or null.
                unsafe { previous.write(Point { x: old_x, y: old_y }) };
            }
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

fn copy_tile(bitmap: usize) -> Result<(Pattern, u32), u32> {
    let (width, height, first) = read_bitmap(bitmap, |source, pixels| {
        (source.width, source.height, pixels[0] & 0x00ff_ffff)
    })?;
    let tile = allocate_pixels(width, height)?;
    let copied = read_bitmap(bitmap, |source, pixels| {
        let count = (width * height) as usize;
        if source.width != width || source.height != height || pixels.len() < count {
            return false;
        }
        // SAFETY: `tile` is a fresh block of `count` pixels.
        unsafe { core::ptr::copy_nonoverlapping(pixels.as_ptr(), tile, count) };
        true
    });
    if copied != Ok(true) {
        // SAFETY: the block this call allocated, which nothing else knows.
        unsafe { heap::free(tile.cast()) };
        return Err(copied.err().unwrap_or(error::INVALID_HANDLE));
    }
    let pattern = Pattern {
        brush: 0,
        bitmap,
        pixels: tile as usize,
        width,
        height,
    };
    Ok((pattern, first))
}

extern "efiapi" fn create_pattern_brush(bitmap: usize) -> usize {
    let (mut tile, first) = match copy_tile(bitmap) {
        Ok(copied) => copied,
        Err(code) => return fail(code, 0),
    };
    let Some(brush) = with(|gui| gui.objects.insert(Object::Brush(Some(first)))).flatten() else {
        // SAFETY: the tile this call allocated, which nothing else knows.
        unsafe { heap::free(tile.pixels as *mut u8) };
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    tile.brush = brush;
    let added = PATTERNS.with(|patterns| {
        let slot = patterns.entries.get_mut(patterns.count)?;
        *slot = tile;
        patterns.count += 1;
        Some(())
    });
    if added.is_none() {
        remove_object(brush);
        // SAFETY: as above; the brush left the table before anything could use it.
        unsafe { heap::free(tile.pixels as *mut u8) };
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    PATTERN_COUNT.fetch_add(1, Ordering::AcqRel);
    brush
}

extern "efiapi" fn get_pixel(hdc: usize, x: i32, y: i32) -> u32 {
    let read = draw(hdc, |surface, _, _| {
        let surface = surface?;
        surface
            .clip()
            .contains(x, y)
            .then(|| surface.get(x, y))
            .flatten()
    });
    match read {
        Ok(Some(pixel)) => colour::colour_ref(pixel & 0x00ff_ffff),
        Ok(None) => CLR_INVALID,
        Err(code) => fail(code, CLR_INVALID),
    }
}

unsafe fn names(text: *const u16, wanted: &[u8], whole: bool) -> bool {
    if text.is_null() {
        return false;
    }
    // SAFETY: the caller's duty: a NUL-terminated string.
    let units = unsafe { super::file::wide_string(text) };
    let compared = match whole {
        true => units,
        false => &units[..units.len().min(wanted.len())],
    };
    let narrow = compared.iter().map(|&unit| u8::try_from(unit).unwrap_or(0));
    layer_text::equal_ignoring_ascii_case(narrow, wanted.iter().copied())
}

unsafe extern "efiapi" fn create_dc_w(
    driver: *const u16,
    device: *const u16,
    _port: *const u16,
    _mode: usize,
) -> usize {
    // SAFETY: `CreateDCW`'s contract: NUL-terminated strings or null.
    let display_driver = unsafe { names(driver, DISPLAY, true) };
    // SAFETY: as above.
    let display_device = driver.is_null() && unsafe { names(device, DISPLAY_DEVICE, false) };
    if !display_driver && !display_device {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let screen = gui::device()
        .map(|device| device.work_area)
        .unwrap_or_default();
    let made = try_with(|gui| {
        gui.contexts
            .insert(DeviceContext::new(0, false, screen))
            .ok_or(error::NOT_ENOUGH_MEMORY)
    });
    match made {
        Ok(hdc) => hdc,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_system_palette_entries(
    hdc: usize,
    _start: u32,
    _count: u32,
    _entries: usize,
) -> u32 {
    match known_context(hdc) {
        Ok(()) => 0,
        Err(code) => fail(code, 0),
    }
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
    b"GetClipBox" => get_clip_box,
    b"CreateRectRgn" => create_rect_rgn,
    b"CreateRectRgnIndirect" => create_rect_rgn_indirect,
    b"CreatePolygonRgn" => create_polygon_rgn,
    b"CreateEllipticRgn" => create_elliptic_rgn,
    b"CreateRoundRectRgn" => create_round_rect_rgn,
    b"CreateFontW" => create_font [six_stack_arguments],
    b"CreateFontA" => create_font [six_stack_arguments],
    b"CreateFontIndirectW" => create_font_indirect,
    b"CreateFontIndirectA" => create_font_indirect,
    b"EnumFontFamiliesExW" => enum_font_families_ex_w,
    b"GetTextMetricsW" => get_text_metrics_w,
    b"GetTextMetricsA" => get_text_metrics_a,
    b"GetCharABCWidthsW" => get_char_abc_widths_w,
    b"GetTextExtentPoint32A" => get_text_extent_point_32_a,
    b"ExtTextOutW" => ext_text_out_w,
    b"ExtTextOutA" => ext_text_out_a,
    b"TextOutA" => text_out_a,
    b"GetDeviceCaps" => get_device_caps,
    b"SetMapMode" => set_map_mode,
    b"CreateHatchBrush" => create_hatch_brush,
    b"CreatePatternBrush" => create_pattern_brush,
    b"SetBrushOrgEx" => set_brush_org_ex,
    b"GetObjectW" => get_object_w,
    b"CreateDIBSection" => create_dib_section,
    b"GetDIBits" => get_di_bits,
    b"GdiFlush" => gdi_flush,
    b"GetPixel" => get_pixel,
    b"CreateDCW" => create_dc_w,
    b"GetSystemPaletteEntries" => get_system_palette_entries,
    b"StartDocW" => start_doc,
    b"StartDocA" => start_doc,
    b"EndDoc" => end_doc,
    b"StartPage" => end_doc,
    b"EndPage" => end_doc,
    b"AbortDoc" => end_doc,
];

const _: () = assert!(core::mem::size_of::<TextMetricW>() == 60);
const _: () = assert!(core::mem::size_of::<TextMetricA>() == 56);
const _: () = assert!(core::mem::size_of::<LogFont>() == 92);
const _: () = assert!(core::mem::size_of::<EnumLogFontEx>() == 348);
const _: () = assert!(core::mem::size_of::<NewTextMetricEx>() == 100);
const _: () = assert!(core::mem::size_of::<BitmapHeader>() == 32);
const _: () = assert!(core::mem::size_of::<InfoHeader>() == 40);
const _: () = assert!(core::mem::size_of::<DibSection>() == 104);
const _: () = assert!(core::mem::size_of::<LogBrush>() == 16);
const _: () = assert!(core::mem::size_of::<LogPen>() == 16);
const _: () = assert!(core::mem::size_of::<Abc>() == 12);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_from_shapes_are_their_bounding_rectangles() {
        let points = [
            Point { x: 4, y: 9 },
            Point { x: -2, y: 3 },
            Point { x: 7, y: 5 },
        ];
        assert_eq!(bounding_box(&points), Rect::new(-2, 3, 7, 9));
    }

    #[test]
    fn a_section_describes_itself_as_win32_lays_it_out() {
        let section = Section {
            handle: 1,
            pixels: 0x1000,
            bits: 0x2000,
            palette: 0,
            layout: Layout {
                bottom_up: true,
                bits: 24,
                masks: [0; 3],
                alpha_mask: 0,
                ..Layout::native(3, 2)
            },
        };
        assert!(section.mirrored());
        let described = section.describe();
        assert_eq!(described.bitmap.width_bytes, 12);
        assert_eq!(described.bitmap.bits, 0x2000);
        assert_eq!(described.header.height, 2);
        assert_eq!(described.header.size_image, 24);
        let native = Section {
            bits: 0x1000,
            layout: Layout::native(3, 2),
            ..section
        };
        assert!(!native.mirrored());
        assert_eq!(native.describe().header.height, -2);
    }

    #[test]
    fn fonts_are_named_as_asked_and_header_fields_land_in_place() {
        let face: Vec<u16> = "Segoe UI".encode_utf16().collect();
        assert_eq!(face_of(&egg_mono_font(&face)), &face[..]);
        let default: Vec<u16> = EGG_MONO.encode_utf16().collect();
        assert_eq!(face_of(&egg_mono_font(&[])), &default[..]);
        let mut header = [0xffu8; 40];
        describe_into(&mut header, (3, 2));
        assert_eq!(dib::u32_at(&header, 0), Some(40));
        assert_eq!(dib::u32_at(&header, 8), Some(2));
        assert_eq!(dib::u16_at(&header, 14), Some(32));
        assert_eq!(dib::u32_at(&header, 20), Some(24));
        assert_eq!(dib::u32_at(&header, 16), Some(0));
    }
}
