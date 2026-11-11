//! NSScrollView, NSClipView and NSScroller (D372, docs/ux/macos-gui-v0.3.md section 10): a
//! scroll view owns a clip view that shows part of one document view, scrolled by the clip
//! view's bounds origin; the scroll view draws its border and 8-pixel scrollers itself and
//! takes the wheel, presses and drags on the scrollers, and the paging keys its document
//! leaves unhandled. Every document shows its top first.

use super::{
    NSCLIPVIEW, NSCLIPVIEW_META, NSSCROLLER, NSSCROLLER_META, NSSCROLLVIEW, NSSCROLLVIEW_META,
    NSVIEW, ask, drawing, event, store, view,
};
use crate::foundation::{class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::objc_release;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, methods, sel, send1};
use eggos_macos_gui::geometry::{Frame, Point, Size, pixel_rect, round};
use eggos_macos_gui::scroll_view::{self as geometry, Layout, Scrollers, border_lines};
use eggos_toolkit::canvas::INSET_R4;
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;

const NO_BORDER: usize = 0;
const PAGE_UP: u16 = 0x74;
const PAGE_DOWN: u16 = 0x79;
const HOME: u16 = 0x73;
const END: u16 = 0x77;
const SHIFT_FLAG: u64 = 1 << 17;
const SCROLLER_STYLE_LEGACY: isize = 0;

#[derive(Debug, Clone, Copy)]
struct Grab {
    vertical: bool,
    at: i32,
    offset: i32,
}

#[repr(C)]
#[derive(Debug)]
struct ScrollViewFields {
    view: view::ViewFields,
    clip: Id,
    border: usize,
    scrollers: Scrollers,
    draws_background: bool,
    grab: Option<Grab>,
    vertical_scroller: Id,
    horizontal_scroller: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ClipViewFields {
    view: view::ViewFields,
    document: Id,
    offset: (i32, i32),
    background: Id,
    draws_background: bool,
}

#[repr(C)]
#[derive(Debug)]
struct ScrollerFields {
    isa: usize,
    scroll_view: Id,
    vertical: bool,
}

fn scroll_view(object: Id) -> &'static mut ScrollViewFields {
    // SAFETY: the callers pass NSScrollView instances, whose memory starts with these fields;
    // the program's single event thread is the only one that reaches them.
    unsafe { &mut *object.cast::<ScrollViewFields>() }
}

fn clip_view(object: Id) -> &'static mut ClipViewFields {
    // SAFETY: the callers pass NSClipView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ClipViewFields>() }
}

fn scroller(object: Id) -> &'static mut ScrollerFields {
    // SAFETY: the callers pass NSScroller instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ScrollerFields>() }
}

fn size_of_frame(frame: Frame) -> (i32, i32) {
    (round(frame.width), round(frame.height))
}

fn document_size(clip: Id) -> (i32, i32) {
    let document = clip_view(clip).document;
    match document.is_null() {
        true => (0, 0),
        false => size_of_frame(view::fields(document).frame),
    }
}

fn is_flipped(view: Id) -> bool {
    !view.is_null() && ask(view, sel!("isFlipped"))
}

/// Where the bounds' origin of `view` is in its own coordinates as the layer draws them, y
/// growing upward: a clip view's scroll position, (0, 0) for every other view.
pub fn bounds_origin(view: Id) -> Point {
    if view.is_null() || !super::is(view, &NSCLIPVIEW) {
        return Point::default();
    }
    let fields = clip_view(view);
    let content = size_of_frame(fields.view.frame);
    let document = document_size(view);
    let y = geometry::bounds_y(false, fields.offset.1, content.1, document.1);
    Point::new(f64::from(fields.offset.0), f64::from(y))
}

/// A clip view's offset from its document's top-left and its size, or `None` for a view that
/// is not a clip view.
pub fn clip_offset(view: Id) -> Option<((i32, i32), (i32, i32))> {
    if view.is_null() || !super::is(view, &NSCLIPVIEW) {
        return None;
    }
    let fields = clip_view(view);
    Some((fields.offset, size_of_frame(fields.view.frame)))
}

/// Scrolls clip view `clip` to `offset` from its document's top-left, through its scroll
/// view when it has one.
pub fn scroll_clip(clip: Id, offset: (i32, i32)) {
    if clip.is_null() || !super::is(clip, &NSCLIPVIEW) {
        return;
    }
    let scroll_view = view::fields(clip).superview;
    match !scroll_view.is_null() && super::is(scroll_view, &NSSCROLLVIEW) {
        true => set_offset(scroll_view, offset),
        false => {
            clip_view(clip).offset = offset;
            redraw(clip);
        }
    }
}

/// Lays out the scroll view around clip view `clip` again, after its document changed size.
pub fn document_resized(clip: Id) {
    if clip.is_null() || !super::is(clip, &NSCLIPVIEW) {
        return;
    }
    let scroll_view = view::fields(clip).superview;
    if !scroll_view.is_null() && super::is(scroll_view, &NSSCROLLVIEW) {
        tile(scroll_view);
        redraw(scroll_view);
    }
}

fn layout_of(this: Id) -> Layout {
    let fields = scroll_view(this);
    let (width, height) = size_of_frame(fields.view.frame);
    let document = match fields.clip.is_null() {
        true => (0, 0),
        false => document_size(fields.clip),
    };
    geometry::layout(
        Rect::new(0, 0, width, height),
        fields.border != NO_BORDER,
        fields.scrollers,
        document,
    )
}

fn tile(this: Id) {
    let laid = layout_of(this);
    let clip = scroll_view(this).clip;
    if clip.is_null() {
        return;
    }
    let height = round(scroll_view(this).view.frame.height);
    let frame = Frame::new(
        f64::from(laid.clip.x),
        f64::from(height - laid.clip.y - laid.content.1),
        f64::from(laid.content.0),
        f64::from(laid.content.1),
    );
    view::fields(clip).frame = frame;
    let document = document_size(clip);
    let fields = clip_view(clip);
    fields.offset = (
        geometry::clamp(fields.offset.0, laid.content.0, document.0),
        geometry::clamp(fields.offset.1, laid.content.1, document.1),
    );
}

fn redraw(this: Id) {
    send1(this, sel!("setNeedsDisplay:"), 1);
}

fn set_offset(this: Id, offset: (i32, i32)) {
    let clip = scroll_view(this).clip;
    if clip.is_null() {
        return;
    }
    clip_view(clip).offset = offset;
    tile(this);
    redraw(this);
}

fn offset_of(this: Id) -> (i32, i32) {
    let clip = scroll_view(this).clip;
    match clip.is_null() {
        true => (0, 0),
        false => clip_view(clip).offset,
    }
}

fn enclosing_scroll_view(view: Id) -> Id {
    let mut current = view::fields(view).superview;
    while !current.is_null() {
        if super::is(current, &NSSCROLLVIEW) {
            return current;
        }
        current = view::fields(current).superview;
    }
    core::ptr::null_mut()
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    let fields = scroll_view(this);
    fields.scrollers.autohides = true;
    fields.draws_background = true;
    let clip = view::new_with_frame(&NSCLIPVIEW, frame.at_origin());
    if !clip.is_null() {
        clip_view(clip).draws_background = true;
        send1(this, sel!("addSubview:"), clip as usize);
        objc_release(clip);
        scroll_view(this).clip = clip;
    }
    tile(this);
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = scroll_view(this);
    store(&mut fields.vertical_scroller, core::ptr::null_mut());
    store(&mut fields.horizontal_scroller, core::ptr::null_mut());
    fields.clip = core::ptr::null_mut();
    view::dispose(this);
}

extern "C" fn content_view(this: Id, _: Sel) -> Id {
    scroll_view(this).clip
}

extern "C" fn document_view(this: Id, _: Sel) -> Id {
    let clip = scroll_view(this).clip;
    match clip.is_null() {
        true => clip,
        false => clip_view(clip).document,
    }
}

extern "C" fn set_document_view(this: Id, _: Sel, document: Id) {
    let clip = scroll_view(this).clip;
    if !clip.is_null() {
        clip_set_document_view(clip, sel!("setDocumentView:"), document);
    }
    tile(this);
    redraw(this);
}

macro_rules! flag_accessors {
    ($get:ident, $set:ident, $($field:tt)+) => {
        extern "C" fn $get(this: Id, _: Sel) -> u8 {
            u8::from(scroll_view(this).$($field)+)
        }

        extern "C" fn $set(this: Id, _: Sel, value: u8) {
            scroll_view(this).$($field)+ = value != 0;
            tile(this);
            redraw(this);
        }
    };
}

flag_accessors!(has_vertical_scroller, set_has_vertical_scroller, scrollers.vertical);
flag_accessors!(has_horizontal_scroller, set_has_horizontal_scroller, scrollers.horizontal);
flag_accessors!(autohides_scrollers, set_autohides_scrollers, scrollers.autohides);

extern "C" fn draws_background(this: Id, _: Sel) -> u8 {
    u8::from(scroll_view(this).draws_background)
}

extern "C" fn set_draws_background(this: Id, _: Sel, value: u8) {
    scroll_view(this).draws_background = value != 0;
    let clip = scroll_view(this).clip;
    if !clip.is_null() {
        clip_view(clip).draws_background = value != 0;
    }
    redraw(this);
}

extern "C" fn background_colour(this: Id, _: Sel) -> Id {
    let clip = scroll_view(this).clip;
    match clip.is_null() {
        true => clip,
        false => clip_view(clip).background,
    }
}

extern "C" fn set_background_colour(this: Id, _: Sel, colour: Id) {
    let clip = scroll_view(this).clip;
    if !clip.is_null() {
        store(&mut clip_view(clip).background, colour);
    }
    redraw(this);
}

extern "C" fn border_type(this: Id, _: Sel) -> usize {
    scroll_view(this).border
}

extern "C" fn set_border_type(this: Id, _: Sel, border: usize) {
    scroll_view(this).border = border;
    tile(this);
    redraw(this);
}

extern "C" fn content_size(this: Id, _: Sel) -> Size {
    let laid = layout_of(this);
    Size {
        width: f64::from(laid.content.0),
        height: f64::from(laid.content.1),
    }
}

extern "C" fn document_visible_rect(this: Id, _: Sel) -> Frame {
    let clip = scroll_view(this).clip;
    match clip.is_null() {
        true => Frame::default(),
        false => clip_document_visible_rect(clip, sel!("documentVisibleRect")),
    }
}

extern "C" fn tile_method(this: Id, _: Sel) {
    tile(this);
    redraw(this);
}

extern "C" fn reflect_scrolled_clip_view(this: Id, _: Sel, _: Id) {
    tile(this);
    redraw(this);
}

fn scroller_object(this: Id, vertical: bool) -> Id {
    let fields = scroll_view(this);
    let slot = match vertical {
        true => &mut fields.vertical_scroller,
        false => &mut fields.horizontal_scroller,
    };
    if slot.is_null() {
        let object = new_instance(class_pointer(&NSSCROLLER));
        if !object.is_null() {
            scroller(object).scroll_view = this;
            scroller(object).vertical = vertical;
            *slot = object;
        }
    }
    *slot
}

extern "C" fn vertical_scroller(this: Id, _: Sel) -> Id {
    scroller_object(this, true)
}

extern "C" fn horizontal_scroller(this: Id, _: Sel) -> Id {
    scroller_object(this, false)
}

fn axis(this: Id, vertical: bool) -> (i32, i32, i32) {
    let laid = layout_of(this);
    let document = match scroll_view(this).clip.is_null() {
        true => (0, 0),
        false => document_size(scroll_view(this).clip),
    };
    let offset = offset_of(this);
    match vertical {
        true => (laid.content.1, document.1, offset.1),
        false => (laid.content.0, document.0, offset.0),
    }
}

fn scroll_axis(this: Id, vertical: bool, to: i32) {
    let (mut x, mut y) = offset_of(this);
    match vertical {
        true => y = to,
        false => x = to,
    }
    set_offset(this, (x, y));
}

extern "C" fn scroll_wheel(this: Id, _: Sel, event: Id) {
    let fields = event::fields(event);
    let shifted = fields.flags & SHIFT_FLAG != 0;
    let vertical_notches = round(fields.delta);
    let sideways_notches = round(fields.delta_x);
    let (vertical, notches) = match (shifted, vertical_notches, sideways_notches) {
        (true, notches, _) if notches != 0 => (false, notches),
        (_, 0, notches) => (false, notches),
        (_, notches, _) => (true, notches),
    };
    let (view_length, document, offset) = axis(this, vertical);
    if notches == 0 || document <= view_length {
        let superview = view::fields(this).superview;
        if !superview.is_null() {
            send1(superview, sel!("scrollWheel:"), event as usize);
        }
        return;
    }
    scroll_axis(this, vertical, geometry::wheel(offset, notches, view_length, document));
}

fn local_pixel(this: Id, event: Id) -> (i32, i32) {
    let origin = view::origin_in_window(this);
    let location = event::fields(event).location;
    let height = view::fields(this).frame.height;
    (
        round(location.x - origin.x),
        round(height - (location.y - origin.y)) - 1,
    )
}

extern "C" fn mouse_down(this: Id, _: Sel, event: Id) {
    let (x, y) = local_pixel(this, event);
    let laid = layout_of(this);
    let pressed = [(true, laid.vertical), (false, laid.horizontal)]
        .into_iter()
        .find_map(|(vertical, rect)| rect.filter(|rect| rect.contains(x, y)).map(|rect| (vertical, rect)));
    let Some((vertical, track)) = pressed else {
        return;
    };
    let (view_length, document, offset) = axis(this, vertical);
    let (along, length) = match vertical {
        true => (y - track.y, track.height),
        false => (x - track.x, track.width),
    };
    match geometry::thumb(length, view_length, document, offset) {
        Some((start, thumb)) if along >= start && along < start + thumb => {
            scroll_view(this).grab = Some(Grab {
                vertical,
                at: along,
                offset,
            });
        }
        Some(_) => {
            let moved = geometry::track_press(along, length, view_length, document, offset);
            scroll_axis(this, vertical, moved);
        }
        None => {}
    }
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event: Id) {
    let Some(grab) = scroll_view(this).grab else {
        return;
    };
    let (x, y) = local_pixel(this, event);
    let laid = layout_of(this);
    let track = match grab.vertical {
        true => laid.vertical,
        false => laid.horizontal,
    };
    let Some(track) = track else {
        return;
    };
    let (view_length, document, _) = axis(this, grab.vertical);
    let (along, length) = match grab.vertical {
        true => (y - track.y, track.height),
        false => (x - track.x, track.width),
    };
    let moved = geometry::drag(grab.offset, along - grab.at, length, view_length, document);
    scroll_axis(this, grab.vertical, moved);
}

extern "C" fn mouse_up(this: Id, _: Sel, _: Id) {
    scroll_view(this).grab = None;
}

extern "C" fn key_down(this: Id, _: Sel, event: Id) {
    let (view_length, document, offset) = axis(this, true);
    let moved = match event::fields(event).key_code {
        PAGE_UP => geometry::page(offset, false, view_length, document),
        PAGE_DOWN => geometry::page(offset, true, view_length, document),
        HOME => 0,
        END => geometry::most(view_length, document),
        _ => {
            let superview = view::fields(this).superview;
            if !superview.is_null() {
                send1(superview, sel!("keyDown:"), event as usize);
            }
            return;
        }
    };
    scroll_axis(this, true, moved);
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    tile(this);
    let laid = layout_of(this);
    let bordered = scroll_view(this).border != NO_BORDER;
    let frame = view::fields(this).frame;
    let grabbed = scroll_view(this).grab.map(|grab| grab.vertical);
    let offset = offset_of(this);
    let document = match scroll_view(this).clip.is_null() {
        true => (0, 0),
        false => document_size(scroll_view(this).clip),
    };
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let outer = pixel_rect(Frame::new(origin.x, origin.y, frame.width, frame.height), height);
        let place = |rect: Rect| rect.offset(outer.x, outer.y);
        if bordered {
            for line in border_lines(Rect::new(0, 0, outer.width, outer.height)) {
                canvas.fill(place(line), colour::BUTTON_BORDER);
            }
        }
        if let Some(corner) = laid.corner {
            canvas.fill(place(corner), colour::WINDOW_BACKGROUND);
        }
        for (vertical, track) in [(true, laid.vertical), (false, laid.horizontal)] {
            let Some(track) = track else {
                continue;
            };
            canvas.fill(place(track), colour::PROGRESS_TRACK);
            let (view_length, document, offset) = match vertical {
                true => (laid.content.1, document.1, offset.1),
                false => (laid.content.0, document.0, offset.0),
            };
            let length = if vertical { track.height } else { track.width };
            let Some((start, thumb)) = geometry::thumb(length, view_length, document, offset) else {
                continue;
            };
            let rect = match vertical {
                true => Rect::new(track.x, track.y + start, track.width, thumb),
                false => Rect::new(track.x + start, track.y, thumb, track.height),
            };
            let fill = match grabbed == Some(vertical) {
                true => colour::BUTTON_INACTIVE,
                false => colour::BAR_PRESSED,
            };
            canvas.rounded(place(rect), &INSET_R4, fill);
        }
    });
}

extern "C" fn scroller_width(_: Id, _: Sel) -> f64 {
    f64::from(geometry::SCROLLER_WIDTH)
}

extern "C" fn scroller_width_for(_: Id, _: Sel, _: usize, _: isize) -> f64 {
    f64::from(geometry::SCROLLER_WIDTH)
}

extern "C" fn scroller_style(_: Id, _: Sel) -> isize {
    SCROLLER_STYLE_LEGACY
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn clip_init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    clip_view(this).draws_background = true;
    this
}

extern "C" fn clip_dealloc(this: Id, _: Sel) {
    let fields = clip_view(this);
    store(&mut fields.background, core::ptr::null_mut());
    fields.document = core::ptr::null_mut();
    view::dispose(this);
}

extern "C" fn clip_document_view(this: Id, _: Sel) -> Id {
    clip_view(this).document
}

extern "C" fn clip_set_document_view(this: Id, _: Sel, document: Id) {
    let old = clip_view(this).document;
    if old == document {
        return;
    }
    if !old.is_null() {
        clip_view(this).document = core::ptr::null_mut();
        view::detach(old);
    }
    if !document.is_null() && super::is(document, &NSVIEW) {
        send1(this, sel!("addSubview:"), document as usize);
        clip_view(this).document = document;
    }
    clip_view(this).offset = (0, 0);
    let scroll_view = view::fields(this).superview;
    if !scroll_view.is_null() && super::is(scroll_view, &NSSCROLLVIEW) {
        tile(scroll_view);
    }
    redraw(this);
}

extern "C" fn clip_bounds(this: Id, _: Sel) -> Frame {
    let origin = bounds_origin(this);
    let frame = view::fields(this).frame;
    Frame::new(origin.x, origin.y, frame.width, frame.height)
}

extern "C" fn clip_document_visible_rect(this: Id, _: Sel) -> Frame {
    clip_bounds(this, sel!("bounds"))
}

extern "C" fn clip_is_flipped(this: Id, _: Sel) -> u8 {
    u8::from(is_flipped(clip_view(this).document))
}

fn clip_scroll_to(this: Id, point: Point) {
    let content = size_of_frame(view::fields(this).frame);
    let document = document_size(this);
    let flipped = is_flipped(clip_view(this).document);
    let offset = (
        geometry::clamp(round(point.x), content.0, document.0),
        geometry::offset_of_bounds_y(flipped, round(point.y), content.1, document.1),
    );
    let scroll_view = view::fields(this).superview;
    match !scroll_view.is_null() && super::is(scroll_view, &NSSCROLLVIEW) {
        true => set_offset(scroll_view, offset),
        false => {
            clip_view(this).offset = offset;
            redraw(this);
        }
    }
}

extern "C" fn clip_scroll_to_point(this: Id, _: Sel, point: Point) {
    clip_scroll_to(this, point);
}

extern "C" fn clip_background_colour(this: Id, _: Sel) -> Id {
    clip_view(this).background
}

extern "C" fn clip_set_background_colour(this: Id, _: Sel, colour: Id) {
    store(&mut clip_view(this).background, colour);
    redraw(this);
}

extern "C" fn clip_draws_background(this: Id, _: Sel) -> u8 {
    u8::from(clip_view(this).draws_background)
}

extern "C" fn clip_set_draws_background(this: Id, _: Sel, value: u8) {
    clip_view(this).draws_background = value != 0;
    redraw(this);
}

extern "C" fn clip_draw_rect(this: Id, _: Sel, _: Frame) {
    let fields = clip_view(this);
    if !fields.draws_background {
        return;
    }
    let fill = drawing::colour_of(fields.background)
        .map_or(colour::FIELD, |colour| colour.pixel);
    let frame = fields.view.frame;
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let rect = pixel_rect(Frame::new(origin.x, origin.y, frame.width, frame.height), height);
        canvas.fill(rect, fill);
    });
}

extern "C" fn view_scroll_point(this: Id, _: Sel, point: Point) {
    let scroll_view = enclosing_scroll_view(this);
    if scroll_view.is_null() {
        return;
    }
    let clip = self::scroll_view(scroll_view).clip;
    let at = view::convert_to(this, point, clip);
    clip_scroll_to(clip, at);
}

extern "C" fn view_scroll_rect_to_visible(this: Id, _: Sel, rect: Frame) -> u8 {
    let scroll_view = enclosing_scroll_view(this);
    if scroll_view.is_null() {
        return 0;
    }
    let clip = self::scroll_view(scroll_view).clip;
    if clip.is_null() {
        return 0;
    }
    let document = clip_view(clip).document;
    if document.is_null() {
        return 0;
    }
    let corner = view::convert_to(this, Point::new(rect.x, rect.y), document);
    let content = size_of_frame(view::fields(clip).frame);
    let size = document_size(clip);
    let top = match is_flipped(document) {
        true => round(corner.y),
        false => size.1 - round(corner.y + rect.height),
    };
    let (x, y) = clip_view(clip).offset;
    let moved = (
        geometry::reveal(x, round(corner.x), round(rect.width), content.0, size.0),
        geometry::reveal(y, top, round(rect.height), content.1, size.1),
    );
    if moved == (x, y) {
        return 0;
    }
    set_offset(scroll_view, moved);
    1
}

extern "C" fn view_enclosing_scroll_view(this: Id, _: Sel) -> Id {
    enclosing_scroll_view(this)
}

extern "C" fn scroller_float_value(this: Id, _: Sel) -> f32 {
    let fields = scroller(this);
    let (view_length, document, offset) = axis(fields.scroll_view, fields.vertical);
    let most = geometry::most(view_length, document);
    match most {
        0 => 0.0,
        most => offset as f32 / most as f32,
    }
}

extern "C" fn scroller_set_float_value(this: Id, _: Sel, value: f32) {
    let fields = scroller(this);
    let (view_length, document, _) = axis(fields.scroll_view, fields.vertical);
    let most = geometry::most(view_length, document);
    let to = (value.clamp(0.0, 1.0) * most as f32) as i32;
    scroll_axis(fields.scroll_view, fields.vertical, to);
}

extern "C" fn scroller_knob_proportion(this: Id, _: Sel) -> f32 {
    let fields = scroller(this);
    let (view_length, document, _) = axis(fields.scroll_view, fields.vertical);
    match document {
        0 => 1.0,
        document => (view_length as f32 / document as f32).min(1.0),
    }
}

pub fn define(runtime: &mut Runtime) -> bool {
    let scroll_view_methods = methods![
        "initWithFrame:" => init_with_frame,
        "dealloc" => dealloc,
        "contentView" => content_view,
        "documentView" => document_view,
        "setDocumentView:" => set_document_view,
        "hasVerticalScroller" => has_vertical_scroller,
        "setHasVerticalScroller:" => set_has_vertical_scroller,
        "hasHorizontalScroller" => has_horizontal_scroller,
        "setHasHorizontalScroller:" => set_has_horizontal_scroller,
        "autohidesScrollers" => autohides_scrollers,
        "setAutohidesScrollers:" => set_autohides_scrollers,
        "drawsBackground" => draws_background,
        "setDrawsBackground:" => set_draws_background,
        "backgroundColor" => background_colour,
        "setBackgroundColor:" => set_background_colour,
        "borderType" => border_type,
        "setBorderType:" => set_border_type,
        "contentSize" => content_size,
        "documentVisibleRect" => document_visible_rect,
        "tile" => tile_method,
        "reflectScrolledClipView:" => reflect_scrolled_clip_view,
        "verticalScroller" => vertical_scroller,
        "horizontalScroller" => horizontal_scroller,
        "scrollerStyle" => scroller_style,
        "setScrollerStyle:" => ignore_word,
        "setScrollerKnobStyle:" => ignore_word,
        "setHasVerticalRuler:" => ignore_word,
        "setHasHorizontalRuler:" => ignore_word,
        "setAllowsMagnification:" => ignore_word,
        "setVerticalScrollElasticity:" => ignore_word,
        "setHorizontalScrollElasticity:" => ignore_word,
        "setAutomaticallyAdjustsContentInsets:" => ignore_word,
        "scrollWheel:" => scroll_wheel,
        "mouseDown:" => mouse_down,
        "mouseDragged:" => mouse_dragged,
        "mouseUp:" => mouse_up,
        "keyDown:" => key_down,
        "drawRect:" => draw_rect,
    ];
    let scroll_view_class_methods = methods![
        "scrollerWidth" => scroller_width,
    ];
    let clip_view_methods = methods![
        "initWithFrame:" => clip_init_with_frame,
        "dealloc" => clip_dealloc,
        "documentView" => clip_document_view,
        "setDocumentView:" => clip_set_document_view,
        "bounds" => clip_bounds,
        "documentVisibleRect" => clip_document_visible_rect,
        "isFlipped" => clip_is_flipped,
        "scrollToPoint:" => clip_scroll_to_point,
        "setBoundsOrigin:" => clip_scroll_to_point,
        "backgroundColor" => clip_background_colour,
        "setBackgroundColor:" => clip_set_background_colour,
        "drawsBackground" => clip_draws_background,
        "setDrawsBackground:" => clip_set_draws_background,
        "drawRect:" => clip_draw_rect,
    ];
    let scroller_methods = methods![
        "floatValue" => scroller_float_value,
        "doubleValue" => scroller_float_value,
        "setFloatValue:" => scroller_set_float_value,
        "knobProportion" => scroller_knob_proportion,
        "scrollerStyle" => scroller_style,
        "setScrollerStyle:" => ignore_word,
        "setKnobStyle:" => ignore_word,
    ];
    let scroller_class_methods = methods![
        "scrollerWidth" => scroller_width,
        "scrollerWidthForControlSize:scrollerStyle:" => scroller_width_for,
        "preferredScrollerStyle" => scroller_style,
    ];
    super::define(
        runtime,
        b"NSScrollView\0",
        (&NSSCROLLVIEW, &NSSCROLLVIEW_META),
        &NSVIEW,
        core::mem::size_of::<ScrollViewFields>(),
        &scroll_view_methods,
        &scroll_view_class_methods,
    ) && super::define(
        runtime,
        b"NSClipView\0",
        (&NSCLIPVIEW, &NSCLIPVIEW_META),
        &NSVIEW,
        core::mem::size_of::<ClipViewFields>(),
        &clip_view_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSScroller\0",
        (&NSSCROLLER, &NSSCROLLER_META),
        &NSOBJECT,
        core::mem::size_of::<ScrollerFields>(),
        &scroller_methods,
        &scroller_class_methods,
    )
}

/// NSView's additions that reach the scroll view around a view: `scrollPoint:`,
/// `scrollRectToVisible:` and `enclosingScrollView`.
pub fn extend_views(runtime: &mut Runtime) -> bool {
    let added = methods![
        "scrollPoint:" => view_scroll_point,
        "scrollRectToVisible:" => view_scroll_rect_to_visible,
        "enclosingScrollView" => view_enclosing_scroll_view,
    ];
    class::extend(runtime, &NSVIEW, &added, &[])
}
