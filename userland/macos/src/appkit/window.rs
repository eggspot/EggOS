//! NSWindow: a window of the window server (`docs/ux/macos-gui-v0.3.md` section 1): its content
//! size, title and content view, shown by its first `makeKeyAndOrderFront:`, drawn whole into
//! its pixels when anything in it changed, and the routing of its events: a click to the view
//! under it (which becomes the first responder if it accepts), the press's drags and release to
//! the same view, keys to the first responder after Tab and the default button's Enter.

use super::event::{self, kind};
use super::{
    NSRESPONDER, NSWINDOW, NSWINDOW_META, application, ask, control, drawing, responds, server,
    store, store_copy, string, view, with_text,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::{Runtime, message, methods, sel, send_bool, send1};
use eggos_macos_gui::colour;
use eggos_macos_gui::geometry::{Frame, Point, Size, round};
use eggos_macos_gui::keys::modifier;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::key;

/// `NSWindowStyleMaskResizable`.
const RESIZABLE: u64 = 1 << 3;

/// A window's fields.
#[repr(C)]
#[derive(Debug)]
pub struct WindowFields {
    isa: usize,
    pub content: Size,
    pub style: u64,
    pub title: Id,
    pub content_view: Id,
    pub delegate: Id,
    pub first_responder: Id,
    pub initial_first_responder: Id,
    pub tracking: Id,
    pub background: Id,
    pub buffer: server::Buffer,
    pub shown: bool,
    pub key: bool,
    pub dirty: bool,
    pub pointer_shape: u32,
}

/// The fields of `window`.
pub fn fields(window: Id) -> &'static mut WindowFields {
    // SAFETY: the callers pass NSWindow instances, reached only from the event thread.
    unsafe { &mut *window.cast::<WindowFields>() }
}

/// Whether anything in `window` waits to be drawn.
pub fn is_dirty(window: Id) -> bool {
    fields(window).dirty
}

/// Marks `window` to be drawn again before the next event is read.
pub fn mark_dirty(window: Id) {
    fields(window).dirty = true;
}

/// Whether `window` has the keyboard.
pub fn is_key(window: Id) -> bool {
    fields(window).key
}

/// `window`'s first responder (nil when it is the window itself).
pub fn first_responder(window: Id) -> Id {
    fields(window).first_responder
}

/// The window server's id of `window`, 0 when it is not shown.
pub fn server_id(window: Id) -> u32 {
    fields(window).buffer.window
}

/// Notes that `view` left `window`'s tree.
pub fn forget_view(window: Id, view: Id) {
    let fields = fields(window);
    if fields.first_responder == view {
        fields.first_responder = core::ptr::null_mut();
    }
    if fields.tracking == view {
        fields.tracking = core::ptr::null_mut();
    }
    if fields.initial_first_responder == view {
        fields.initial_first_responder = core::ptr::null_mut();
    }
}

/// Makes `responder` (a view of `window`, or nil for the window) the first responder, as
/// `makeFirstResponder:` does: the old one must resign and the new one accept.
pub fn make_first_responder(window: Id, responder: Id) -> bool {
    let current = fields(window).first_responder;
    if current == responder {
        return true;
    }
    if !current.is_null() && !ask(current, sel!("resignFirstResponder")) {
        return false;
    }
    if !responder.is_null() && !ask(responder, sel!("becomeFirstResponder")) {
        fields(window).first_responder = core::ptr::null_mut();
        mark_dirty(window);
        return false;
    }
    fields(window).first_responder = responder;
    mark_dirty(window);
    true
}

/// Gives `window` the keyboard (`key`) or takes it away.
pub fn set_key(window: Id, key: bool) {
    if fields(window).key != key {
        fields(window).key = key;
        mark_dirty(window);
    }
}

/// Draws `window` whole and shows it, if anything in it changed.
pub fn display_if_needed(window: Id) {
    let fields = fields(window);
    if !fields.dirty || !fields.shown || fields.buffer.window == 0 {
        return;
    }
    fields.dirty = false;
    let buffer = fields.buffer;
    let content = fields.content_view;
    let background = drawing::colour_of(fields.background)
        .filter(|colour| colour.visible)
        .unwrap_or(colour::WINDOW_BACKGROUND);
    let whole = Rect::new(0, 0, buffer.width as i32, buffer.height as i32);
    drawing::begin(buffer, Point::default(), whole);
    drawing::with_canvas(|canvas, _, _, _, _| canvas.fill(whole, background.pixel));
    if !content.is_null() {
        view::draw_tree(content, Point::default(), whole, buffer);
    }
    drawing::end();
    server::commit(buffer.window);
}

fn title_text(window: Id, work: impl FnOnce(&str)) {
    let title = fields(window).title;
    with_text(title, |text| match text.is_empty() {
        true => work(super::app_name()),
        false => work(text),
    });
}

fn initial_first_responder(window: Id) {
    if !fields(window).first_responder.is_null() {
        return;
    }
    let wanted = fields(window).initial_first_responder;
    if !wanted.is_null() {
        make_first_responder(window, wanted);
        return;
    }
    let content = fields(window).content_view;
    if content.is_null() {
        return;
    }
    let mut found = Buffer::new();
    view::editable_fields(content, &mut found);
    if let Some(&first) = found.as_slice().first() {
        make_first_responder(window, first);
    }
    found.free();
}

fn set_content_frame(window: Id) {
    let content = fields(window).content_view;
    if !content.is_null() {
        let size = fields(window).content;
        view::fields(content).frame = Frame::new(0.0, 0.0, size.width, size.height);
    }
}

/// Shows `window` (its first show makes its server window), gives it the keyboard and draws it.
pub fn show(window: Id) {
    if fields(window).buffer.window == 0 {
        let size = fields(window).content;
        let width = round(size.width).max(1) as u32;
        let height = round(size.height).max(1) as u32;
        let flags = match fields(window).style & RESIZABLE {
            0 => server::FIXED_SIZE,
            _ => 0,
        };
        let buffer = match server::create(server::ROLE_NORMAL, 0, width, height, flags) {
            Ok(buffer) => buffer,
            Err(_) => {
                application::report_no_window_server();
                return;
            }
        };
        let fields = fields(window);
        fields.buffer = buffer;
        fields.content = Size {
            width: f64::from(buffer.width),
            height: f64::from(buffer.height),
        };
        set_content_frame(window);
        title_text(window, |title| server::set_title(buffer.window, title));
        application::window_shown(window);
    }
    fields(window).shown = true;
    application::make_key(window);
    initial_first_responder(window);
    mark_dirty(window);
    display_if_needed(window);
}

/// Hides and forgets `window`'s server window; with `closing`, tells its delegate and lets the
/// app end when it was the last.
pub fn close(window: Id, closing: bool) {
    if fields(window).buffer.window == 0 {
        fields(window).shown = false;
        return;
    }
    let delegate = fields(window).delegate;
    let will_close = sel!("windowWillClose:");
    if closing && responds(delegate, will_close) {
        send1(delegate, will_close, 0);
    }
    let fields = fields(window);
    let buffer = fields.buffer;
    fields.buffer = server::Buffer {
        window: 0,
        width: 0,
        height: 0,
        pixels: 0,
    };
    fields.shown = false;
    fields.key = false;
    fields.tracking = core::ptr::null_mut();
    // SAFETY: the window's buffer is no longer in its fields, so nothing draws on it.
    unsafe { server::destroy(&buffer) };
    application::window_closed(window, closing);
}

/// Takes the content size the server gave `window`: a new buffer, drawn whole.
pub fn resize(window: Id, width: u32, height: u32) {
    let buffer = fields(window).buffer;
    if buffer.window == 0 {
        return;
    }
    // SAFETY: drawing happens only in `display_if_needed`, on this thread, not now.
    let Ok(resized) = (unsafe { server::resize(&buffer, width, height) }) else {
        return;
    };
    let fields = fields(window);
    fields.buffer = resized;
    fields.content = Size {
        width: f64::from(resized.width),
        height: f64::from(resized.height),
    };
    set_content_frame(window);
    mark_dirty(window);
}

fn hit(window: Id, location: Point) -> Option<Id> {
    let content = fields(window).content_view;
    if content.is_null() {
        return None;
    }
    view::hit_test(content, location)
}

/// The pointer moved to `location`: the I-beam over an editable text field, else the arrow.
pub fn pointer_moved(window: Id, location: Point) {
    let over_field = hit(window, location).is_some_and(view::is_editable_field);
    let shape = match over_field {
        true => server::POINTER_I_BEAM,
        false => server::POINTER_ARROW,
    };
    let fields = fields(window);
    if fields.pointer_shape != shape && fields.buffer.window != 0 {
        fields.pointer_shape = shape;
        server::set_pointer(fields.buffer.window, shape);
    }
}

fn default_button_in(view: Id) -> Option<Id> {
    let mut found = None;
    let mut index = 0;
    while let Some(&child) = view::fields(view).subviews.as_slice().get(index) {
        if let Some(button) = default_button_in(child) {
            found = Some(button);
        }
        index += 1;
    }
    if control::is_default_button(view) && found.is_none() {
        found = Some(view);
    }
    found
}

/// The window's default button: the last one whose key equivalent is Return.
pub fn default_button(window: Id) -> Option<Id> {
    let content = fields(window).content_view;
    if content.is_null() {
        return None;
    }
    default_button_in(content)
}

fn tab(window: Id, backwards: bool) {
    let content = fields(window).content_view;
    if content.is_null() {
        return;
    }
    let mut found = Buffer::new();
    view::editable_fields(content, &mut found);
    let count = found.len();
    if count > 0 {
        let current = fields(window).first_responder;
        let next = match found.as_slice().iter().position(|&field| field == current) {
            Some(at) if backwards => (at + count - 1) % count,
            Some(at) => (at + 1) % count,
            None if backwards => count - 1,
            None => 0,
        };
        make_first_responder(window, found.as_slice()[next]);
    }
    found.free();
}

fn key_down(window: Id, event: Id) {
    let fields_of_event = event::fields(event);
    let code = fields_of_event.linux_code;
    let flags = fields_of_event.flags;
    let plain = flags & (modifier::COMMAND | modifier::CONTROL | modifier::OPTION) == 0;
    if code == key::TAB && plain {
        tab(window, flags & modifier::SHIFT != 0);
        return;
    }
    let responder = fields(window).first_responder;
    let typing = !responder.is_null() && view::is_editable_field(responder);
    if code == key::ENTER && plain && !typing {
        if let Some(button) = default_button(window) {
            control::perform_click(button);
            return;
        }
    }
    if !responder.is_null() {
        send1(responder, sel!("keyDown:"), event as usize);
    }
}

fn to_responder(window: Id, sel: Sel, event: Id) {
    let responder = fields(window).first_responder;
    if !responder.is_null() {
        send1(responder, sel, event as usize);
    }
}

extern "C" fn send_event(this: Id, _: Sel, event: Id) {
    if event.is_null() {
        return;
    }
    let location = event::fields(event).location;
    match event::fields(event).kind {
        kind::LEFT_MOUSE_DOWN => {
            let Some(view) = hit(this, location) else {
                return;
            };
            if view::accepts_first_responder(view) {
                make_first_responder(this, view);
            }
            fields(this).tracking = view;
            send1(view, sel!("mouseDown:"), event as usize);
        }
        kind::LEFT_MOUSE_UP => {
            let tracking = core::mem::replace(&mut fields(this).tracking, core::ptr::null_mut());
            let view = match tracking.is_null() {
                true => hit(this, location),
                false => Some(tracking),
            };
            if let Some(view) = view {
                send1(view, sel!("mouseUp:"), event as usize);
            }
        }
        kind::LEFT_MOUSE_DRAGGED => {
            let tracking = fields(this).tracking;
            if !tracking.is_null() {
                send1(tracking, sel!("mouseDragged:"), event as usize);
            }
        }
        kind::RIGHT_MOUSE_DOWN | kind::RIGHT_MOUSE_UP | kind::SCROLL_WHEEL => {
            let sel = match event::fields(event).kind {
                kind::RIGHT_MOUSE_DOWN => sel!("rightMouseDown:"),
                kind::RIGHT_MOUSE_UP => sel!("rightMouseUp:"),
                _ => sel!("scrollWheel:"),
            };
            if let Some(view) = hit(this, location) {
                send1(view, sel, event as usize);
            }
        }
        kind::MOUSE_MOVED => pointer_moved(this, location),
        kind::KEY_DOWN => key_down(this, event),
        kind::KEY_UP => to_responder(this, sel!("keyUp:"), event),
        kind::FLAGS_CHANGED => to_responder(this, sel!("flagsChanged:"), event),
        _ => {}
    }
}

extern "C" fn init_with_content_rect(
    this: Id,
    _: Sel,
    rect: Frame,
    style: u64,
    _: u64,
    _: u8,
) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = fields(this);
    fields.content = Size {
        width: rect.width,
        height: rect.height,
    };
    fields.style = style;
    let content = view::new_with_frame(&super::NSVIEW, rect.at_origin());
    fields.content_view = content;
    if !content.is_null() {
        view::attach_to_window(content, this);
    }
    this
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    let init: extern "C" fn(Id, Sel, Frame, u64, u64, u8) -> Id = message::msg_send_as();
    init(
        this,
        sel!("initWithContentRect:styleMask:backing:defer:"),
        Frame::default(),
        0,
        2,
        0,
    )
}

extern "C" fn title(this: Id, _: Sel) -> Id {
    match fields(this).title {
        none if none.is_null() => string(""),
        title => title,
    }
}

extern "C" fn set_title(this: Id, _: Sel, title: Id) {
    store_copy(&mut fields(this).title, title);
    let id = fields(this).buffer.window;
    if id != 0 {
        title_text(this, |text| server::set_title(id, text));
    }
}

extern "C" fn content_view(this: Id, _: Sel) -> Id {
    fields(this).content_view
}

extern "C" fn set_content_view(this: Id, _: Sel, content: Id) {
    let old = fields(this).content_view;
    if old == content {
        return;
    }
    if !old.is_null() {
        view::attach_to_window(old, core::ptr::null_mut());
    }
    fields(this).first_responder = core::ptr::null_mut();
    fields(this).tracking = core::ptr::null_mut();
    store(&mut fields(this).content_view, content);
    if !content.is_null() {
        view::attach_to_window(content, this);
        set_content_frame(this);
    }
    mark_dirty(this);
}

extern "C" fn make_key_and_order_front(this: Id, _: Sel, _: Id) {
    show(this);
}

extern "C" fn order_front(this: Id, _: Sel) {
    show(this);
}

extern "C" fn order_out(this: Id, _: Sel, _: Id) {
    close(this, false);
}

extern "C" fn close_method(this: Id, _: Sel) {
    close(this, true);
}

extern "C" fn perform_close(this: Id, _: Sel, _: Id) {
    let delegate = fields(this).delegate;
    let should_close = sel!("windowShouldClose:");
    if responds(delegate, should_close) && !send_bool(delegate, should_close, this as usize) {
        return;
    }
    close(this, true);
}

extern "C" fn is_visible(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).shown)
}

extern "C" fn is_key_window(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).key)
}

extern "C" fn delegate(this: Id, _: Sel) -> Id {
    fields(this).delegate
}

extern "C" fn set_delegate(this: Id, _: Sel, delegate: Id) {
    store(&mut fields(this).delegate, delegate);
}

extern "C" fn first_responder_method(this: Id, _: Sel) -> Id {
    match fields(this).first_responder {
        none if none.is_null() => this,
        responder => responder,
    }
}

extern "C" fn make_first_responder_method(this: Id, _: Sel, responder: Id) -> u8 {
    let responder = if responder == this {
        core::ptr::null_mut()
    } else {
        responder
    };
    u8::from(make_first_responder(this, responder))
}

extern "C" fn initial_first_responder_method(this: Id, _: Sel) -> Id {
    fields(this).initial_first_responder
}

extern "C" fn set_initial_first_responder(this: Id, _: Sel, view: Id) {
    fields(this).initial_first_responder = view;
}

extern "C" fn background_colour(this: Id, _: Sel) -> Id {
    match fields(this).background {
        none if none.is_null() => drawing::new_colour(colour::WINDOW_BACKGROUND, 1.0),
        background => background,
    }
}

extern "C" fn set_background_colour(this: Id, _: Sel, colour: Id) {
    store(&mut fields(this).background, colour);
    mark_dirty(this);
}

extern "C" fn frame(this: Id, _: Sel) -> Frame {
    let size = fields(this).content;
    Frame::new(0.0, 0.0, size.width, size.height)
}

extern "C" fn same_rect(_: Id, _: Sel, rect: Frame) -> Frame {
    rect
}

extern "C" fn set_content_size(this: Id, _: Sel, size: Size) {
    let shown = fields(this).buffer.window != 0;
    match shown {
        true => resize(
            this,
            round(size.width).max(1) as u32,
            round(size.height).max(1) as u32,
        ),
        false => {
            fields(this).content = size;
            set_content_frame(this);
        }
    }
}

extern "C" fn display(this: Id, _: Sel) {
    mark_dirty(this);
    display_if_needed(this);
}

extern "C" fn display_if_needed_method(this: Id, _: Sel) {
    display_if_needed(this);
}

extern "C" fn style_mask(this: Id, _: Sel) -> u64 {
    fields(this).style
}

extern "C" fn window_number(this: Id, _: Sel) -> isize {
    fields(this).buffer.window as isize
}

extern "C" fn nothing(_: Id, _: Sel) {}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn nil_for(_: Id, _: Sel, _: usize) -> Id {
    core::ptr::null_mut()
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithContentRect:styleMask:backing:defer:" => init_with_content_rect,
        "init" => init,
        "title" => title,
        "setTitle:" => set_title,
        "contentView" => content_view,
        "setContentView:" => set_content_view,
        "makeKeyAndOrderFront:" => make_key_and_order_front,
        "orderFront:" => make_key_and_order_front,
        "orderFrontRegardless" => order_front,
        "makeKeyWindow" => order_front,
        "makeMainWindow" => nothing,
        "orderOut:" => order_out,
        "close" => close_method,
        "performClose:" => perform_close,
        "isVisible" => is_visible,
        "isKeyWindow" => is_key_window,
        "isMainWindow" => is_key_window,
        "delegate" => delegate,
        "setDelegate:" => set_delegate,
        "firstResponder" => first_responder_method,
        "makeFirstResponder:" => make_first_responder_method,
        "initialFirstResponder" => initial_first_responder_method,
        "setInitialFirstResponder:" => set_initial_first_responder,
        "backgroundColor" => background_colour,
        "setBackgroundColor:" => set_background_colour,
        "frame" => frame,
        "contentRectForFrameRect:" => same_rect,
        "frameRectForContentRect:" => same_rect,
        "setContentSize:" => set_content_size,
        "display" => display,
        "displayIfNeeded" => display_if_needed_method,
        "styleMask" => style_mask,
        "windowNumber" => window_number,
        "sendEvent:" => send_event,
        "center" => nothing,
        "update" => nothing,
        "setReleasedWhenClosed:" => ignore_word,
        "setAcceptsMouseMovedEvents:" => ignore_word,
        "setLevel:" => ignore_word,
        "setOpaque:" => ignore_word,
        "setHasShadow:" => ignore_word,
        "setMovableByWindowBackground:" => ignore_word,
        "setCollectionBehavior:" => ignore_word,
        "setTabbingMode:" => ignore_word,
        "setRestorable:" => ignore_word,
        "setTitleVisibility:" => ignore_word,
        "setTitlebarAppearsTransparent:" => ignore_word,
        "setContentMinSize:" => ignore_size,
        "setContentMaxSize:" => ignore_size,
        "setFrameOrigin:" => ignore_point,
        "setFrameTopLeftPoint:" => ignore_point,
        "standardWindowButton:" => nil_for,
    ];
    super::define(
        runtime,
        b"NSWindow\0",
        (&NSWINDOW, &NSWINDOW_META),
        &NSRESPONDER,
        core::mem::size_of::<WindowFields>(),
        &instance,
        &[],
    )
}

extern "C" fn ignore_size(_: Id, _: Sel, _: Size) {}

extern "C" fn ignore_point(_: Id, _: Sel, _: Point) {}
