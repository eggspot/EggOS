//! Painting (D129): device contexts on a window's pixel buffer (`BeginPaint`/`EndPaint` for the
//! region waiting to be painted, `GetDC`/`ReleaseDC` for the whole client area), what each one
//! changed shown on the desktop with one commit when it is released (a window first appears when
//! its first paint ends), and gdi32's objects, shapes and text drawn with
//! `eggos_windows_gui`.

use super::gui::{self, try_with, with};
use super::user32::WindowRect;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::gui::objects::{self, DeviceContext, Object, ObjectTable};
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
    paint: WindowRect,
    restore: Bool,
    include_update: Bool,
    reserved: [u8; 32],
}

#[repr(C)]
struct Size {
    width: i32,
    height: i32,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

fn rect_of(rect: &WindowRect) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

fn draw<R>(
    hdc: usize,
    work: impl FnOnce(Option<&mut Surface<'_>>, &mut DeviceContext, &ObjectTable) -> R,
) -> Result<R, u32> {
    try_with(|gui| {
        let mut context = *gui.contexts.get(hdc).ok_or(error::INVALID_HANDLE)?;
        let surface = gui.windows.get(context.window).and_then(|window| {
            // SAFETY: the window's pixels are its mapped buffer of `width * height` pixels,
            // which stays mapped while the window is in the table, and the table's lock is held.
            let pixels = unsafe {
                core::slice::from_raw_parts_mut(
                    window.pixels as *mut u32,
                    window.width as usize * window.height as usize,
                )
            };
            Surface::new(pixels, window.width, window.height)
        });
        let result = match surface {
            Some(mut surface) => {
                surface.clip_to(context.clip);
                let result = work(Some(&mut surface), &mut context, &gui.objects);
                context.drawn = context.drawn.union(&surface.drawn());
                result
            }
            None => work(None, &mut context, &gui.objects),
        };
        if let Some(stored) = gui.contexts.get_mut(hdc) {
            *stored = context;
        }
        Ok(result)
    })
}

fn begin(window: usize) -> Result<(usize, Rect, bool), u32> {
    let (hdc, region, erase) = try_with(|gui| {
        let target = gui.window_mut(window)?;
        let region = target.update.intersection(&target.client());
        let erase = target.erase;
        target.update = Rect::default();
        target.erase = false;
        let hdc = gui
            .contexts
            .insert(DeviceContext::new(window, true, region))
            .ok_or(error::NOT_ENOUGH_MEMORY)?;
        Ok((hdc, region, erase))
    })?;
    let unerased = erase && gui::send(window, message::ERASE_BACKGROUND, hdc, 0) == 0;
    Ok((hdc, region, unerased))
}

fn finish(hdc: usize) -> bool {
    let commit = with(|gui| {
        let context = gui.contexts.remove(hdc)?;
        let device = gui.device;
        let window = gui.windows.get_mut(context.window)?;
        let rect = match context.painting {
            true => context.clip,
            false => context.drawn,
        };
        if !window.committed {
            if !context.painting {
                return Some(None);
            }
            window.committed = true;
            return Some(Some((device?, window.server, None)));
        }
        match rect.is_empty() {
            true => Some(None),
            false => Some(Some((device?, window.server, Some(rect)))),
        }
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
            paint: WindowRect {
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
    rect: *const WindowRect,
    erase: Bool,
) -> Bool {
    let rect = match rect.is_null() {
        true => None,
        // SAFETY: the caller passes a readable `RECT` or null.
        false => Some(rect_of(unsafe { &*rect })),
    };
    let invalidated = try_with(|gui| {
        match window {
            0 => gui
                .windows
                .iter_mut()
                .for_each(|(_, target)| target.invalidate(rect, erase != 0)),
            window => gui.window_mut(window)?.invalidate(rect, erase != 0),
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

unsafe extern "efiapi" fn fill_rect(hdc: usize, rect: *const WindowRect, brush: usize) -> i32 {
    // SAFETY: the caller passes a readable `RECT`.
    let rect = rect_of(unsafe { &*rect });
    let filled = draw(hdc, |surface, _, objects| {
        if let (Some(surface), Some(colour)) = (surface, objects::brush_colour(objects, brush)) {
            surface.fill(rect, colour);
        }
    });
    match filled {
        Ok(()) => 1,
        Err(code) => fail(code, 0),
    }
}

unsafe fn units<'a>(text: *const u16, count: i32) -> &'a [u16] {
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
    rect: *mut WindowRect,
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

fn create(object: Object) -> usize {
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
    match with(|gui| gui.objects.remove(handle)).flatten() {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
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
