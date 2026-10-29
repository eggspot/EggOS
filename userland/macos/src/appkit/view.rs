//! NSResponder and NSView: a view's frame in its superview, its subviews drawn back to front
//! after it, its window, and the walks the window makes over the tree: drawing each view's
//! `drawRect:` clipped to its frame, finding the view under a point, and the text fields in the
//! order Tab visits them.

use super::{
    NSCONTROL, NSRESPONDER, NSRESPONDER_META, NSTEXTFIELD, NSVIEW, NSVIEW_META, ask, control,
    drawing, send_frame, server, window,
};
use crate::foundation::{NSARRAY, class_pointer};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send2};
use eggos_macos_gui::geometry::{Frame, Point, Size, pixel_rect};
use eggos_macos_gui::paint;
use eggos_toolkit::geometry::Rect;

/// A view's fields, which every subclass's start with.
#[repr(C)]
#[derive(Debug)]
pub struct ViewFields {
    isa: usize,
    pub frame: Frame,
    pub superview: Id,
    pub window: Id,
    pub subviews: Buffer<Id>,
    pub hidden: bool,
}

/// The fields of `view`.
pub fn fields(view: Id) -> &'static mut ViewFields {
    // SAFETY: the callers pass NSView instances (or a subclass's), whose memory starts with
    // these fields; the program's single event thread is the only one that reaches them.
    unsafe { &mut *view.cast::<ViewFields>() }
}

/// A new view of `class` (NSView or a subclass), `initWithFrame:` `frame`, counted once.
pub fn new_with_frame(class: &'static ClassObject, frame: Frame) -> Id {
    new_with_frame_of(class_pointer(class).cast(), frame)
}

/// A new view of the class object `class`, `initWithFrame:` `frame`, counted once.
pub fn new_with_frame_of(class: Id, frame: Frame) -> Id {
    let object = send(class, sel!("alloc"));
    if object.is_null() {
        return object;
    }
    let init: extern "C" fn(Id, Sel, Frame) -> Id = message::msg_send_as();
    init(object, sel!("initWithFrame:"), frame)
}

/// Where `view`'s bounds' origin is in its window's coordinates.
pub fn origin_in_window(view: Id) -> Point {
    let mut origin = Point::default();
    let mut current = view;
    while !current.is_null() {
        let fields = fields(current);
        origin.x += fields.frame.x;
        origin.y += fields.frame.y;
        current = fields.superview;
    }
    origin
}

fn set_window(view: Id, window: Id) {
    fields(view).window = window;
    let mut index = 0;
    while let Some(&child) = fields(view).subviews.as_slice().get(index) {
        set_window(child, window);
        index += 1;
    }
}

fn changed(view: Id) {
    let window = fields(view).window;
    if !window.is_null() {
        window::mark_dirty(window);
    }
}

/// Makes `view` the content view of `window`, or nil's, without a superview.
pub fn attach_to_window(view: Id, window: Id) {
    fields(view).superview = core::ptr::null_mut();
    set_window(view, window);
    changed(view);
}

/// Whether `view` (nil or not) is `root` or one of its descendants, found walking down from
/// `root` so that `view` itself is never read.
pub fn holds(root: Id, view: Id) -> bool {
    if view.is_null() {
        return false;
    }
    if root == view {
        return true;
    }
    let mut index = 0;
    while let Some(&child) = fields(root).subviews.as_slice().get(index) {
        if holds(child, view) {
            return true;
        }
        index += 1;
    }
    false
}

fn detach(view: Id) {
    let parent = fields(view).superview;
    if parent.is_null() {
        return;
    }
    changed(view);
    let siblings = &mut fields(parent).subviews;
    let mut kept = Buffer::new();
    for &sibling in siblings.as_slice() {
        if sibling != view {
            kept.push(sibling);
        }
    }
    siblings.free();
    *siblings = kept;
    let window = fields(view).window;
    if !window.is_null() {
        window::forget_tree(window, view);
    }
    fields(view).superview = core::ptr::null_mut();
    set_window(view, core::ptr::null_mut());
    objc_release(view);
}

extern "C" fn nothing_with(_: Id, _: Sel, _: Id) {}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn yes(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn no_with(_: Id, _: Sel, _: Id) -> u8 {
    0
}

extern "C" fn nil(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    fields(this).frame = frame;
    this
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    let init: extern "C" fn(Id, Sel, Frame) -> Id = message::msg_send_as();
    init(this, sel!("initWithFrame:"), Frame::default())
}

extern "C" fn frame(this: Id, _: Sel) -> Frame {
    fields(this).frame
}

extern "C" fn bounds(this: Id, _: Sel) -> Frame {
    fields(this).frame.at_origin()
}

extern "C" fn set_frame(this: Id, _: Sel, frame: Frame) {
    changed(this);
    fields(this).frame = frame;
    changed(this);
}

extern "C" fn set_frame_origin(this: Id, _: Sel, origin: Point) {
    let frame = fields(this).frame;
    set_frame(
        this,
        core::ptr::null(),
        Frame::new(origin.x, origin.y, frame.width, frame.height),
    );
}

extern "C" fn set_frame_size(this: Id, _: Sel, size: Size) {
    let frame = fields(this).frame;
    set_frame(
        this,
        core::ptr::null(),
        Frame::new(frame.x, frame.y, size.width, size.height),
    );
}

/// Frees `view` as `-[NSView dealloc]` does: its subviews are let go of (each loses its
/// superview and window), then its memory. A subclass's `dealloc` releases its own fields first.
pub fn dispose(view: Id) {
    let mut subviews = core::mem::replace(&mut fields(view).subviews, Buffer::new());
    for &child in subviews.as_slice() {
        fields(child).superview = core::ptr::null_mut();
        set_window(child, core::ptr::null_mut());
        objc_release(child);
    }
    subviews.free();
    crate::objc::reference_count::dispose(view);
}

extern "C" fn dealloc(this: Id, _: Sel) {
    dispose(this);
}

extern "C" fn add_subview(this: Id, _: Sel, view: Id) {
    if view.is_null() || !super::is(view, &NSVIEW) || holds(view, this) {
        return;
    }
    objc_retain(view);
    detach(view);
    fields(this).subviews.push(view);
    fields(view).superview = this;
    set_window(view, fields(this).window);
    changed(view);
}

extern "C" fn remove_from_superview(this: Id, _: Sel) {
    detach(this);
}

extern "C" fn subviews(this: Id, _: Sel) -> Id {
    let views = fields(this).subviews.as_slice();
    send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        views.as_ptr() as usize,
        views.len(),
    )
}

extern "C" fn superview(this: Id, _: Sel) -> Id {
    fields(this).superview
}

extern "C" fn window_of(this: Id, _: Sel) -> Id {
    fields(this).window
}

extern "C" fn set_needs_display(this: Id, _: Sel, flag: u8) {
    if flag != 0 {
        changed(this);
    }
}

extern "C" fn set_needs_display_in_rect(this: Id, _: Sel, _: Frame) {
    changed(this);
}

extern "C" fn needs_display(this: Id, _: Sel) -> u8 {
    let window = fields(this).window;
    u8::from(!window.is_null() && window::is_dirty(window))
}

extern "C" fn display(this: Id, _: Sel) {
    let window = fields(this).window;
    if !window.is_null() {
        window::mark_dirty(window);
        window::display_if_needed(window);
    }
}

extern "C" fn draw_rect(_: Id, _: Sel, _: Frame) {}

extern "C" fn is_hidden(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).hidden)
}

extern "C" fn set_hidden(this: Id, _: Sel, hidden: u8) {
    fields(this).hidden = hidden != 0;
    changed(this);
}

extern "C" fn convert_point_from(this: Id, _: Sel, point: Point, view: Id) -> Point {
    let from = if view.is_null() {
        Point::default()
    } else {
        origin_in_window(view)
    };
    let to = origin_in_window(this);
    Point::new(point.x + from.x - to.x, point.y + from.y - to.y)
}

extern "C" fn convert_point_to(this: Id, _: Sel, point: Point, view: Id) -> Point {
    let from = origin_in_window(this);
    let to = if view.is_null() {
        Point::default()
    } else {
        origin_in_window(view)
    };
    Point::new(point.x + from.x - to.x, point.y + from.y - to.y)
}

extern "C" fn convert_rect_from(this: Id, sel: Sel, rect: Frame, view: Id) -> Frame {
    let at = convert_point_from(this, sel, Point::new(rect.x, rect.y), view);
    Frame::new(at.x, at.y, rect.width, rect.height)
}

extern "C" fn convert_rect_to(this: Id, sel: Sel, rect: Frame, view: Id) -> Frame {
    let at = convert_point_to(this, sel, Point::new(rect.x, rect.y), view);
    Frame::new(at.x, at.y, rect.width, rect.height)
}

extern "C" fn hit_test_method(this: Id, _: Sel, point: Point) -> Id {
    hit_test(this, point).unwrap_or(core::ptr::null_mut())
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

/// The deepest shown view of the tree at `view` whose frame holds `point`, in `view`'s
/// superview's coordinates; later subviews are on top.
pub fn hit_test(view: Id, point: Point) -> Option<Id> {
    let fields = fields(view);
    if fields.hidden || !fields.frame.contains(point) {
        return None;
    }
    let inner = Point::new(point.x - fields.frame.x, point.y - fields.frame.y);
    for &child in fields.subviews.as_slice().iter().rev() {
        if let Some(found) = hit_test(child, inner) {
            return Some(found);
        }
    }
    Some(view)
}

/// Whether `view` is a text field the user can type in.
pub fn is_editable_field(view: Id) -> bool {
    super::is(view, &NSTEXTFIELD) && control::is_editable(view)
}

/// Appends the editable text fields of the tree at `view` to `found`, in the order added.
pub fn editable_fields(view: Id, found: &mut Buffer<Id>) {
    if fields(view).hidden {
        return;
    }
    if is_editable_field(view) {
        found.push(view);
    }
    let mut index = 0;
    while let Some(&child) = fields(view).subviews.as_slice().get(index) {
        editable_fields(child, found);
        index += 1;
    }
}

fn drawing_area(view: Id, frame: Frame, height: i32) -> Rect {
    let area = pixel_rect(frame, height);
    match super::is(view, &NSCONTROL) {
        true => area.union(&paint::control_rect(frame, height)),
        false => area,
    }
}

/// Draws the tree at `view` into `buffer`: `drawRect:` of each shown view with its bounds,
/// clipped to its frame inside `clip`, its superview's origin at `origin` in the window.
pub fn draw_tree(view: Id, origin: Point, clip: Rect, buffer: server::Buffer) {
    if fields(view).hidden {
        return;
    }
    let frame = fields(view).frame.offset(origin.x, origin.y);
    let area = drawing_area(view, frame, buffer.height as i32).intersection(&clip);
    if area.is_empty() {
        return;
    }
    drawing::begin(buffer, Point::new(frame.x, frame.y), area);
    send_frame(view, sel!("drawRect:"), fields(view).frame.at_origin());
    let mut index = 0;
    while let Some(&child) = fields(view).subviews.as_slice().get(index) {
        draw_tree(child, Point::new(frame.x, frame.y), area, buffer);
        index += 1;
    }
}

/// Whether `view` takes the keyboard when clicked.
pub fn accepts_first_responder(view: Id) -> bool {
    ask(view, sel!("acceptsFirstResponder"))
}

pub fn define(runtime: &mut Runtime) -> bool {
    let responder = methods![
        "mouseDown:" => nothing_with,
        "mouseUp:" => nothing_with,
        "mouseDragged:" => nothing_with,
        "mouseMoved:" => nothing_with,
        "rightMouseDown:" => nothing_with,
        "rightMouseUp:" => nothing_with,
        "otherMouseDown:" => nothing_with,
        "otherMouseUp:" => nothing_with,
        "scrollWheel:" => nothing_with,
        "keyDown:" => nothing_with,
        "keyUp:" => nothing_with,
        "flagsChanged:" => nothing_with,
        "acceptsFirstResponder" => no,
        "becomeFirstResponder" => yes,
        "resignFirstResponder" => yes,
        "nextResponder" => nil,
        "performKeyEquivalent:" => no_with,
    ];
    let view = methods![
        "initWithFrame:" => init_with_frame,
        "init" => init,
        "dealloc" => dealloc,
        "frame" => frame,
        "bounds" => bounds,
        "setFrame:" => set_frame,
        "setFrameOrigin:" => set_frame_origin,
        "setFrameSize:" => set_frame_size,
        "addSubview:" => add_subview,
        "removeFromSuperview" => remove_from_superview,
        "subviews" => subviews,
        "superview" => superview,
        "window" => window_of,
        "nextResponder" => superview,
        "setNeedsDisplay:" => set_needs_display,
        "setNeedsDisplayInRect:" => set_needs_display_in_rect,
        "needsDisplay" => needs_display,
        "display" => display,
        "drawRect:" => draw_rect,
        "isFlipped" => no,
        "isHidden" => is_hidden,
        "setHidden:" => set_hidden,
        "convertPoint:fromView:" => convert_point_from,
        "convertPoint:toView:" => convert_point_to,
        "convertRect:fromView:" => convert_rect_from,
        "convertRect:toView:" => convert_rect_to,
        "hitTest:" => hit_test_method,
        "setWantsLayer:" => ignore_word,
        "setAutoresizingMask:" => ignore_word,
        "setTranslatesAutoresizingMaskIntoConstraints:" => ignore_word,
        "viewDidMoveToWindow" => nothing,
        "viewWillMoveToWindow:" => nothing_with,
    ];
    super::define(
        runtime,
        b"NSResponder\0",
        (&NSRESPONDER, &NSRESPONDER_META),
        &NSOBJECT,
        core::mem::size_of::<usize>(),
        &responder,
        &[],
    ) && super::define(
        runtime,
        b"NSView\0",
        (&NSVIEW, &NSVIEW_META),
        &NSRESPONDER,
        core::mem::size_of::<ViewFields>(),
        &view,
        &[],
    )
}

extern "C" fn nothing(_: Id, _: Sel) {}
