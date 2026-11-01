//! Painting (D129): device contexts on a window's pixel buffer (`BeginPaint`/`EndPaint` for the
//! region waiting to be painted, `GetDC`/`ReleaseDC` for the whole client area), what each one
//! changed shown on the desktop with one commit when it is released (a window first appears when
//! its first paint ends), and gdi32's objects, shapes and text drawn with
//! `eggos_windows_gui`. A child window (D145) draws on its top-level window's buffer from its
//! own origin, clipped to its client area and its ancestors'; what a window of a tree draws is
//! committed once no paint of the same thread is left to hand out in that tree, so a window
//! never shows with its controls half painted, and an open drop-down is painted over it first.
//! A DIB section whose bits are not its pixels (D178, `gdi.rs`) is read from the program's bits
//! before each drawing call on it and the rows drawn are written back after.

use super::gui::{self, Gui, try_with, with};
use super::thread_environment;
use super::user32::Win32Rect;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::gui::objects::{self, DeviceContext, Object, ObjectTable};
use crate::gui::windows;
use crate::library::Export;
use eggos_windows_gui::colour;
use eggos_windows_gui::shapes::{self, Pen};
use eggos_windows_gui::surface::{Rect, Surface};
use eggos_windows_gui::text::{self, Cell};

const CLR_INVALID: u32 = u32::MAX;
const PS_NULL: i32 = 5;
const TEXT_CELLS: usize = 2048;

#[repr(C)]
struct PaintStruct {
    context: usize,
    erase: Bool,
    paint: Win32Rect,
    restore: Bool,
    include_update: Bool,
    reserved: [u8; 32],
}

#[repr(C)]
pub(super) struct Size {
    pub(super) width: i32,
    pub(super) height: i32,
}

#[repr(C)]
pub(super) struct Point {
    pub(super) x: i32,
    pub(super) y: i32,
}

pub(super) fn rect_of(rect: &Win32Rect) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

/// Where a device context's drawing lands: a buffer of `width` by `height` pixels at
/// `address` (a top-level window's mapped buffer, or a memory context's bitmap), the point of
/// it the context's (0, 0) is at, and the part the context may draw on, in its coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raster {
    pub address: usize,
    pub width: u32,
    pub height: u32,
    pub origin: (i32, i32),
    pub clip: Rect,
}

/// Where `context` draws, under the state's lock; `None` when it draws nowhere.
pub fn raster_of(gui: &Gui, context: &DeviceContext) -> Option<Raster> {
    if context.memory {
        let bitmap = context.selected_bitmap(&gui.objects)?;
        return (bitmap.address != 0).then(|| Raster {
            address: bitmap.address,
            width: bitmap.width,
            height: bitmap.height,
            origin: (0, 0),
            clip: bitmap.bounds().intersection(&context.clip),
        });
    }
    let placement = windows::client_placement(&gui.windows, context.window)?;
    let root = gui.windows.get(placement.root)?;
    if root.pixels == 0 {
        return None;
    }
    let (x, y) = placement.origin;
    Some(Raster {
        address: root.pixels,
        width: root.buffer_width,
        height: root.buffer_height,
        origin: placement.origin,
        clip: placement.clip.offset(-x, -y).intersection(&context.clip),
    })
}

/// Runs `work` on context `hdc`'s surface (none when it draws nowhere): its window's, translated
/// to the window's client area and clipped to it and to the context, or its bitmap's for a
/// memory context, under the state's lock.
pub fn draw_in<R>(
    hdc: usize,
    work: impl FnOnce(Option<&mut Surface<'_>>, &mut DeviceContext, &Gui) -> R,
) -> Result<R, u32> {
    try_with(|gui| {
        let mut context = *gui.contexts.get(hdc).ok_or(error::INVALID_HANDLE)?;
        let section = context.memory.then_some(context.bitmap);
        if let Some(bitmap) = section {
            super::gdi::load_section(bitmap);
        }
        let surface = raster_of(gui, &context).and_then(|raster| {
            // SAFETY: a raster's buffer is a top-level window's mapped buffer, mapped while the
            // window is in the table, or a bitmap's heap block, freed only once no context holds
            // it; both are `width * height` pixels, and the table's lock is held.
            let pixels = unsafe {
                core::slice::from_raw_parts_mut(
                    raster.address as *mut u32,
                    raster.width as usize * raster.height as usize,
                )
            };
            let mut surface = Surface::new(pixels, raster.width, raster.height)?;
            surface.translate(raster.origin.0, raster.origin.1);
            surface.clip_to(raster.clip);
            Some(surface)
        });
        let result = match surface {
            Some(mut surface) => {
                let result = work(Some(&mut surface), &mut context, gui);
                let drawn = surface.drawn();
                context.drawn = context.drawn.union(&drawn);
                if let Some(bitmap) = section {
                    super::gdi::store_section(bitmap, drawn);
                }
                result
            }
            None => work(None, &mut context, gui),
        };
        if let Some(stored) = gui.contexts.get_mut(hdc) {
            *stored = context;
        }
        Ok(result)
    })
}

pub(super) fn draw<R>(
    hdc: usize,
    work: impl FnOnce(Option<&mut Surface<'_>>, &mut DeviceContext, &ObjectTable) -> R,
) -> Result<R, u32> {
    draw_in(hdc, |surface, context, gui| {
        work(surface, context, &gui.objects)
    })
}

pub(super) fn remember_drawn(hdc: usize, rect: Rect, units: &[u16]) {
    with(|gui| {
        let window = gui.contexts.get(hdc).map(|context| context.window);
        if let Some(window) = window.filter(|window| *window != 0) {
            gui.drawn.record(window, rect, units);
        }
    });
}

/// `BeginPaint`'s work: a painting context on `window`'s region waiting to be painted (now
/// validated), the region, and whether the background is still to erase. The text drawn in
/// the region before is forgotten (D162): the paint draws what is there now.
pub fn begin(window: usize) -> Result<(usize, Rect, bool), u32> {
    let (hdc, region, erase) = try_with(|gui| {
        let target = gui.window_mut(window)?;
        let region = target.update.intersection(&target.client());
        let erase = target.erase;
        target.update = Rect::default();
        target.erase = false;
        gui.drawn.forget_in(window, region);
        let hdc = gui
            .contexts
            .insert(DeviceContext::new(window, true, region))
            .ok_or(error::NOT_ENOUGH_MEMORY)?;
        Ok((hdc, region, erase))
    })?;
    let unerased = erase && gui::send(window, message::ERASE_BACKGROUND, hdc, 0) == 0;
    Ok((hdc, region, unerased))
}

/// `EndPaint`'s and `ReleaseDC`'s work: frees context `hdc` and adds what it painted or drew to
/// its top-level window's region to show, which is committed when nothing of the tree waits to
/// be painted; whether there was such a context.
pub fn finish(hdc: usize) -> bool {
    let thread = thread_environment::thread_id();
    let commit = with(|gui| {
        let context = gui.contexts.remove(hdc)?;
        let Some(placement) = windows::client_placement(&gui.windows, context.window) else {
            return Some(None);
        };
        let local = match context.painting {
            true => context.clip,
            false => context.drawn,
        };
        let rect = local
            .offset(placement.origin.0, placement.origin.1)
            .intersection(&placement.clip);
        let root = gui.windows.get_mut(placement.root)?;
        root.pending = root.pending.union(&rect);
        if !root.shown || windows::paint_outstanding(&gui.windows, placement.root, thread) {
            return Some(None);
        }
        Some(show_pending(gui, placement.root))
    })
    .flatten();
    match commit {
        None => false,
        Some(None) => true,
        Some(Some((device, server, rect))) => {
            let _ = device.commit(server, rect);
            true
        }
    }
}

/// Takes `root`'s region to show (all of it at its first commit, with the menu bar), with an
/// open drop-down painted over it: the commit to make, if any.
pub fn show_pending(
    gui: &mut Gui,
    root: usize,
) -> Option<(super::window_device::Device, u32, Option<Rect>)> {
    let device = gui.device?;
    let window = gui.windows.get_mut(root)?;
    let first = !window.committed;
    window.committed = true;
    let pending = core::mem::take(&mut window.pending);
    let server = window.server;
    windows::forget_sent_paints(&mut gui.windows, root);
    if first {
        super::menu::paint_bar(gui, root);
    }
    let dropped = super::menu::paint_open_drop_down(gui, root);
    let listed = super::combo_box::paint_open_list(gui, root);
    match first {
        true => Some((device, server, None)),
        false => {
            let area = [dropped, listed]
                .into_iter()
                .flatten()
                .fold(pending, |area, over| area.union(&over));
            (!area.is_empty()).then_some((device, server, Some(area)))
        }
    }
}

/// `DefWindowProcW`'s `WM_PAINT`: paints nothing but the background and shows the window.
pub fn validate(window: usize) {
    if let Ok((hdc, _, _)) = begin(window) {
        finish(hdc);
    }
}

/// `DefWindowProcW`'s `WM_ERASEBKGND`: fills context `hdc`'s region with `window`'s class
/// background; whether there was one.
pub fn erase_background(window: usize, hdc: usize) -> bool {
    let background = with(|gui| {
        let brush = gui.windows.get(window)?.background;
        objects::brush_colour(&gui.objects, brush)
    })
    .flatten();
    let Some(background) = background else {
        return false;
    };
    draw(hdc, |surface, context, _| {
        if let Some(surface) = surface {
            surface.fill(context.clip, background);
        }
    })
    .is_ok()
}

/// Fills context `hdc`'s clip with `colour`, as `DefDlgProc`'s `WM_ERASEBKGND` does with the
/// dialog's grey (D174); whether there was such a context.
pub fn fill_clip(hdc: usize, colour: u32) -> bool {
    draw(hdc, |surface, context, _| {
        if let Some(surface) = surface {
            surface.fill(context.clip, colour);
        }
    })
    .is_ok()
}

unsafe extern "efiapi" fn begin_paint(window: usize, paint: *mut PaintStruct) -> usize {
    let (hdc, region, unerased) = match begin(window) {
        Ok(begun) => begun,
        Err(code) => return fail(code, 0),
    };
    // SAFETY: the caller passes a writable `PAINTSTRUCT`.
    unsafe {
        paint.write(PaintStruct {
            context: hdc,
            erase: Bool::from(unerased),
            paint: Win32Rect {
                left: region.left,
                top: region.top,
                right: region.right,
                bottom: region.bottom,
            },
            restore: FALSE,
            include_update: FALSE,
            reserved: [0; 32],
        })
    };
    hdc
}

unsafe extern "efiapi" fn end_paint(_window: usize, paint: *const PaintStruct) -> Bool {
    // SAFETY: the caller passes the `PAINTSTRUCT` `BeginPaint` filled.
    let hdc = unsafe { (*paint).context };
    finish(hdc);
    TRUE
}

extern "efiapi" fn get_dc(window: usize) -> usize {
    let made = try_with(|gui| {
        let clip = match window {
            0 => Rect::default(),
            window => gui.window(window)?.client(),
        };
        gui.contexts
            .insert(DeviceContext::new(window, false, clip))
            .ok_or(error::NOT_ENOUGH_MEMORY)
    });
    match made {
        Ok(hdc) => hdc,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn release_dc(_window: usize, hdc: usize) -> i32 {
    i32::from(finish(hdc))
}

unsafe extern "efiapi" fn invalidate_rect(
    window: usize,
    rect: *const Win32Rect,
    erase: Bool,
) -> Bool {
    let rect = match rect.is_null() {
        true => None,
        // SAFETY: the caller passes a readable `RECT` or null.
        false => Some(rect_of(unsafe { &*rect })),
    };
    let invalidated = try_with(|gui| {
        match window {
            0 => {
                let mut roots = [0; windows::MAX_WINDOWS];
                let mut count = 0;
                for (handle, target) in gui.windows.iter() {
                    if !target.is_child() {
                        roots[count] = handle;
                        count += 1;
                    }
                }
                for &root in &roots[..count] {
                    windows::invalidate_tree(&mut gui.windows, root, rect, erase != 0);
                }
            }
            window => {
                gui.window(window)?;
                windows::invalidate_tree(&mut gui.windows, window, rect, erase != 0);
            }
        }
        Ok(())
    });
    match invalidated {
        Ok(()) => {
            gui::wake();
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn fill_rect(hdc: usize, rect: *const Win32Rect, brush: usize) -> i32 {
    // SAFETY: the caller passes a readable `RECT`.
    let rect = rect_of(unsafe { &*rect });
    let filled = draw(hdc, |surface, context, objects| {
        let (Some(surface), Some(colour)) = (surface, objects::brush_colour(objects, brush)) else {
            return;
        };
        match super::gdi::pattern(brush) {
            Some(tile) => {
                let area = rect.intersection(&surface.clip());
                for y in area.top..area.bottom {
                    for x in area.left..area.right {
                        surface.pixel(x, y, tile.at((x, y), context.brush_origin));
                    }
                }
            }
            None => surface.fill(rect, colour),
        }
    });
    match filled {
        Ok(()) => 1,
        Err(code) => fail(code, 0),
    }
}

pub(super) unsafe fn units<'a>(text: *const u16, count: i32) -> &'a [u16] {
    if text.is_null() || count == 0 {
        return &[];
    }
    match count {
        // SAFETY: the caller's duty: with a negative count, a NUL-terminated string.
        ..0 => unsafe { super::file::wide_string(text) },
        // SAFETY: the caller's duty: `count` readable UTF-16 units.
        count => unsafe { core::slice::from_raw_parts(text, count as usize) },
    }
}

unsafe extern "efiapi" fn draw_text_w(
    hdc: usize,
    text: *const u16,
    count: i32,
    rect: *mut Win32Rect,
    format: u32,
) -> i32 {
    // SAFETY: the caller passes `count` units, or a NUL-terminated string when it is -1.
    let units = unsafe { units(text, count) };
    let mut cells = [Cell::Space; TEXT_CELLS];
    let filled = text::decode(units, text::reading(format), &mut cells);
    let cells = &cells[..filled];
    // SAFETY: the caller passes a readable and writable `RECT`.
    let rect = unsafe { &mut *rect };
    let bounds = rect_of(rect);
    if format & text::format::CALCRECT != 0 {
        let measured = text::measure(cells, bounds, format);
        rect.right = measured.right;
        rect.bottom = measured.bottom;
        return measured.height();
    }
    let drawn = draw(hdc, |surface, context, _| {
        let foreground = colour::pixel(context.text_colour);
        let background = context.text_background();
        match surface {
            Some(surface) => {
                text::draw_text(surface, cells, bounds, format, foreground, background)
            }
            None => text::measure(cells, bounds, format).height(),
        }
    });
    if drawn.is_ok() {
        remember_drawn(hdc, text::measure(cells, bounds, format), units);
    }
    match drawn {
        Ok(height) => height,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn get_sys_color(index: i32) -> u32 {
    colour::system_colour(index as u32).map_or(0, colour::colour_ref)
}

extern "efiapi" fn get_sys_color_brush(index: i32) -> usize {
    objects::system_brush_handle(index as u32).unwrap_or(0)
}

pub(super) fn create(object: Object) -> usize {
    match with(|gui| gui.objects.insert(object)).flatten() {
        Some(handle) => handle,
        None => fail(error::NOT_ENOUGH_MEMORY, 0),
    }
}

extern "efiapi" fn create_solid_brush(colour_ref: u32) -> usize {
    create(Object::Brush(Some(colour::pixel(colour_ref))))
}

extern "efiapi" fn create_pen(style: i32, width: i32, colour_ref: u32) -> usize {
    create(Object::Pen(match style {
        PS_NULL => None,
        _ => Some(Pen {
            colour: colour::pixel(colour_ref),
            width: width.max(1) as u32,
        }),
    }))
}

extern "efiapi" fn get_stock_object(index: i32) -> usize {
    objects::stock_handle(index as u32).unwrap_or(0)
}

extern "efiapi" fn select_object(hdc: usize, handle: usize) -> usize {
    let selected = try_with(|gui| {
        let object = objects::object(&gui.objects, handle).ok_or(error::INVALID_HANDLE)?;
        let context = gui.contexts.get_mut(hdc).ok_or(error::INVALID_HANDLE)?;
        Ok(context.select(handle, object))
    });
    match selected {
        Ok(previous) => previous,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn delete_object(handle: usize) -> Bool {
    if objects::is_permanent(handle) {
        return TRUE;
    }
    let removed = with(|gui| {
        let selected = gui
            .contexts
            .iter()
            .any(|(_, context)| context.memory && context.bitmap == handle);
        match selected {
            true => Err(error::BUSY),
            false => gui.objects.remove(handle).ok_or(error::INVALID_HANDLE),
        }
    });
    match removed {
        Some(Ok(object)) => {
            super::gdi::release(handle, object);
            TRUE
        }
        Some(Err(code)) => fail(code, FALSE),
        None => fail(error::NOT_ENOUGH_MEMORY, FALSE),
    }
}

fn change_context(hdc: usize, work: impl FnOnce(&mut DeviceContext) -> u32, failed: u32) -> u32 {
    match draw(hdc, |_, context, _| work(context)) {
        Ok(value) => value,
        Err(code) => fail(code, failed),
    }
}

extern "efiapi" fn set_text_color(hdc: usize, colour_ref: u32) -> u32 {
    change_context(
        hdc,
        |context| core::mem::replace(&mut context.text_colour, colour_ref & 0x00ff_ffff),
        CLR_INVALID,
    )
}

extern "efiapi" fn set_bk_color(hdc: usize, colour_ref: u32) -> u32 {
    change_context(
        hdc,
        |context| core::mem::replace(&mut context.background_colour, colour_ref & 0x00ff_ffff),
        CLR_INVALID,
    )
}

extern "efiapi" fn set_bk_mode(hdc: usize, mode: i32) -> i32 {
    if !matches!(mode as u32, objects::TRANSPARENT | objects::OPAQUE) {
        return fail(error::INVALID_PARAMETER, 0);
    }
    change_context(
        hdc,
        |context| core::mem::replace(&mut context.background_mode, mode as u32),
        0,
    ) as i32
}

unsafe extern "efiapi" fn move_to_ex(hdc: usize, x: i32, y: i32, previous: *mut Point) -> Bool {
    let moved = draw(hdc, |_, context, _| {
        core::mem::replace(&mut context.position, (x, y))
    });
    match moved {
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

extern "efiapi" fn line_to(hdc: usize, x: i32, y: i32) -> Bool {
    let drawn = draw(hdc, |surface, context, objects| {
        let from = core::mem::replace(&mut context.position, (x, y));
        if let (Some(surface), Some(pen)) = (surface, context.pen(objects)) {
            shapes::line(surface, from, (x, y), pen);
        }
    });
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn shape(
    hdc: usize,
    rect: Rect,
    draw_shape: fn(&mut Surface<'_>, Rect, Option<Pen>, Option<u32>),
) -> Bool {
    let drawn = draw(hdc, |surface, context, objects| {
        if let Some(surface) = surface {
            draw_shape(surface, rect, context.pen(objects), context.brush(objects));
        }
    });
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn rectangle(hdc: usize, left: i32, top: i32, right: i32, bottom: i32) -> Bool {
    shape(hdc, Rect::new(left, top, right, bottom), shapes::rectangle)
}

extern "efiapi" fn ellipse(hdc: usize, left: i32, top: i32, right: i32, bottom: i32) -> Bool {
    shape(hdc, Rect::new(left, top, right, bottom), shapes::ellipse)
}

extern "efiapi" fn set_pixel(hdc: usize, x: i32, y: i32, colour_ref: u32) -> u32 {
    let set = draw(hdc, |surface, _, _| {
        let surface = surface?;
        surface
            .clip()
            .contains(x, y)
            .then(|| surface.pixel(x, y, colour::pixel(colour_ref)))
    });
    match set {
        Ok(Some(())) => colour_ref & 0x00ff_ffff,
        Ok(None) => CLR_INVALID,
        Err(code) => fail(code, CLR_INVALID),
    }
}

unsafe extern "efiapi" fn text_out_w(
    hdc: usize,
    x: i32,
    y: i32,
    text: *const u16,
    count: i32,
) -> Bool {
    if count < 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes `count` readable UTF-16 units.
    let units = unsafe { units(text, count) };
    let drawn = draw(hdc, |surface, context, _| {
        if let Some(surface) = surface {
            let foreground = colour::pixel(context.text_colour);
            text::text_out(surface, x, y, units, foreground, context.text_background());
        }
    });
    if drawn.is_ok() {
        let (width, height) = text::extent(units);
        remember_drawn(hdc, Rect::sized(x, y, width, height), units);
    }
    match drawn {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_text_extent_point_32_w(
    hdc: usize,
    text: *const u16,
    count: i32,
    size: *mut Size,
) -> Bool {
    if count < 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let known = try_with(|gui| {
        gui.contexts
            .get(hdc)
            .map(|_| ())
            .ok_or(error::INVALID_HANDLE)
    });
    if let Err(code) = known {
        return fail(code, FALSE);
    }
    // SAFETY: the caller passes `count` readable UTF-16 units.
    let (width, height) = text::extent(unsafe { units(text, count) });
    // SAFETY: the caller passes a writable `SIZE`.
    unsafe { size.write(Size { width, height }) };
    TRUE
}

pub(super) const USER32_EXPORTS: &[Export] = exports![
    b"BeginPaint" => begin_paint,
    b"EndPaint" => end_paint,
    b"GetDC" => get_dc,
    b"ReleaseDC" => release_dc,
    b"InvalidateRect" => invalidate_rect,
    b"FillRect" => fill_rect,
    b"DrawTextW" => draw_text_w,
    b"GetSysColor" => get_sys_color,
    b"GetSysColorBrush" => get_sys_color_brush,
];

pub(super) const GDI32_EXPORTS: &[Export] = exports![
    b"CreateSolidBrush" => create_solid_brush,
    b"CreatePen" => create_pen,
    b"GetStockObject" => get_stock_object,
    b"SelectObject" => select_object,
    b"DeleteObject" => delete_object,
    b"SetTextColor" => set_text_color,
    b"SetBkColor" => set_bk_color,
    b"SetBkMode" => set_bk_mode,
    b"MoveToEx" => move_to_ex,
    b"LineTo" => line_to,
    b"Rectangle" => rectangle,
    b"Ellipse" => ellipse,
    b"SetPixel" => set_pixel,
    b"TextOutW" => text_out_w,
    b"GetTextExtentPoint32W" => get_text_extent_point_32_w,
];
