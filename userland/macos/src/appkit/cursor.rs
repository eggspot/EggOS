//! NSCursor (`docs/ux/macos-gui-v0.3.md` section 13, D233): AppKit's class cursors on the window
//! server's pointer shapes, `set`, `push` and `pop`, `hide` and `unhide`, and the views' cursor
//! rects (`addCursorRect:cursor:`, `resetCursorRects`). The shape a window shows is decided by
//! `eggos_macos_gui::cursor` on every pointer move and every change, and sent to the server only
//! when it differs from the last one sent.

use super::{NSCURSOR, NSCURSOR_META, NSVIEW, NSWINDOW, view, window};
use crate::foundation::{self, class_pointer};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::{Runtime, class, methods, root, sel, send};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::cursor::{ARROW, CLASS_CURSORS, CursorRects, Cursors, I_BEAM};
use eggos_macos_gui::geometry::{Frame, Point};

const RECT_TABLE: usize = 64;
const LONGEST_SELECTOR: usize = 64;

#[repr(C)]
#[derive(Debug)]
struct CursorFields {
    isa: usize,
    shape: u32,
}

#[derive(Debug, Clone, Copy)]
struct Pointer {
    window: usize,
    location: Point,
}

static STATE: Exclusive<Cursors> = Exclusive::new(Cursors::new());
static RECTS: Exclusive<CursorRects<RECT_TABLE>> = Exclusive::new(CursorRects::new());
static POINTER: Exclusive<Pointer> = Exclusive::new(Pointer {
    window: 0,
    location: Point::new(0.0, 0.0),
});
static SHARED: [AtomicUsize; CLASS_CURSORS.len()] =
    [const { AtomicUsize::new(0) }; CLASS_CURSORS.len()];

fn fields(object: Id) -> &'static mut CursorFields {
    // SAFETY: the callers pass NSCursor instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<CursorFields>() }
}

fn shape_of_cursor(cursor: Id) -> u32 {
    match cursor.is_null() || !root::is_kind_of(cursor, &NSCURSOR) {
        true => ARROW,
        false => fields(cursor).shape,
    }
}

fn selector_name(sel: Sel) -> &'static str {
    let mut length = 0;
    // SAFETY: a selector is the address of its NUL-terminated name, which lives as long as the
    // runtime; the scan stops at the NUL or at the longest class cursor's name.
    while length < LONGEST_SELECTOR && unsafe { *sel.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: as above, the first `length` bytes are the name.
    let bytes = unsafe { core::slice::from_raw_parts(sel, length) };
    core::str::from_utf8(bytes).unwrap_or("")
}

fn shared_cursor(index: usize) -> Id {
    let existing = SHARED[index].load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = foundation::new_instance(class_pointer(&NSCURSOR));
    if made.is_null() {
        return made;
    }
    fields(made).shape = CLASS_CURSORS[index].1;
    match SHARED[index].compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => {
            crate::objc::reference_count::objc_release(made);
            winner as Id
        }
    }
}

fn zone(window: Id, location: Point) -> Option<u32> {
    let view = window::view_at(window, location)?;
    let origin = view::origin_in_window(view);
    let local = Point::new(location.x - origin.x, location.y - origin.y);
    let bounds = view::fields(view).frame.at_origin();
    let own = RECTS.with(|rects| match rects.is_empty() {
        true => None,
        false => rects.find(view as usize, bounds, local),
    });
    own.or_else(|| view::is_editable_field(view).then_some(I_BEAM))
}

fn apply(window: Id, location: Point, moved: bool) {
    let zone = zone(window, location);
    let shape = STATE.with(|state| state.resolve(zone, moved));
    window::show_pointer(window, shape);
}

/// The pointer moved to `location` in `window`'s content: the shape it shows there.
pub fn pointer_moved(window: Id, location: Point) {
    POINTER.with(|pointer| {
        *pointer = Pointer {
            window: window as usize,
            location,
        }
    });
    apply(window, location, true);
}

/// The pointer left `window`, or the window went away.
pub fn pointer_left(window: Id) {
    POINTER.with(|pointer| {
        if pointer.window == window as usize {
            pointer.window = 0;
        }
    });
}

fn refresh() {
    let pointer = POINTER.with(|pointer| *pointer);
    if pointer.window != 0 {
        apply(pointer.window as Id, pointer.location, false);
    }
}

/// Lets go of `view`'s cursor rects (`discardCursorRects`, and a view being freed).
pub fn discard_rects(view: Id) {
    RECTS.with(|rects| rects.discard(view as usize));
}

/// Asks every view of `root`'s tree for its cursor rects again, as AppKit does when a window
/// becomes key: each view's rects are discarded, then it is sent `resetCursorRects`.
pub fn reset_tree(root: Id) {
    if root.is_null() {
        return;
    }
    view::each_view(root, &mut |each| {
        discard_rects(each);
        send(each, sel!("resetCursorRects"));
    });
    refresh();
}

extern "C" fn class_cursor(_: Id, sel: Sel) -> Id {
    let name = selector_name(sel);
    match CLASS_CURSORS.iter().position(|&(each, _)| each == name) {
        Some(index) => shared_cursor(index),
        None => shared_cursor(0),
    }
}

extern "C" fn current_cursor(_: Id, _: Sel) -> Id {
    let last = STATE.with(|state| state.last());
    let index = CLASS_CURSORS
        .iter()
        .position(|&(_, shape)| shape == last)
        .unwrap_or(0);
    shared_cursor(index)
}

extern "C" fn init_with_image(this: Id, _: Sel, _: Id, _: Point) -> Id {
    if !this.is_null() {
        fields(this).shape = ARROW;
    }
    this
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    if !this.is_null() {
        fields(this).shape = ARROW;
    }
    this
}

extern "C" fn set(this: Id, _: Sel) {
    let shape = fields(this).shape;
    STATE.with(|state| state.set(shape));
    refresh();
}

extern "C" fn push(this: Id, _: Sel) {
    let shape = fields(this).shape;
    STATE.with(|state| state.push(shape));
    refresh();
}

extern "C" fn pop(_: Id, _: Sel) {
    STATE.with(Cursors::pop);
    refresh();
}

extern "C" fn hide(_: Id, _: Sel) {
    STATE.with(Cursors::hide);
    refresh();
}

extern "C" fn unhide(_: Id, _: Sel) {
    STATE.with(Cursors::unhide);
    refresh();
}

extern "C" fn set_hidden_until_mouse_moves(_: Id, _: Sel, hidden: u8) {
    STATE.with(|state| state.set_hidden_until_move(hidden != 0));
    refresh();
}

extern "C" fn add_cursor_rect(this: Id, _: Sel, rect: Frame, cursor: Id) {
    let shape = shape_of_cursor(cursor);
    RECTS.with(|rects| rects.add(this as usize, rect, cursor as usize, shape));
    refresh();
}

extern "C" fn remove_cursor_rect(this: Id, _: Sel, rect: Frame, cursor: Id) {
    RECTS.with(|rects| rects.remove(this as usize, rect, cursor as usize));
    refresh();
}

extern "C" fn discard_cursor_rects(this: Id, _: Sel) {
    discard_rects(this);
    refresh();
}

extern "C" fn reset_cursor_rects(_: Id, _: Sel) {}

extern "C" fn invalidate_cursor_rects_for_view(_: Id, _: Sel, view: Id) {
    if view.is_null() {
        return;
    }
    discard_rects(view);
    send(view, sel!("resetCursorRects"));
    refresh();
}

extern "C" fn window_reset_cursor_rects(this: Id, _: Sel) {
    reset_tree(window::content_of(this));
}

extern "C" fn window_discard_cursor_rects(this: Id, _: Sel) {
    let content = window::content_of(this);
    if !content.is_null() {
        view::each_view(content, &mut discard_rects);
    }
    refresh();
}

/// Adds NSView's and NSWindow's cursor-rect methods to their classes.
pub fn extend_views(runtime: &mut Runtime) -> bool {
    let view_methods = methods![
        "addCursorRect:cursor:" => add_cursor_rect,
        "removeCursorRect:cursor:" => remove_cursor_rect,
        "discardCursorRects" => discard_cursor_rects,
        "resetCursorRects" => reset_cursor_rects,
    ];
    let window_methods = methods![
        "invalidateCursorRectsForView:" => invalidate_cursor_rects_for_view,
        "resetCursorRects" => window_reset_cursor_rects,
        "discardCursorRects" => window_discard_cursor_rects,
    ];
    class::extend(runtime, &NSVIEW, &view_methods, &[])
        && class::extend(runtime, &NSWINDOW, &window_methods, &[])
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithImage:hotSpot:" => init_with_image,
        "set" => set,
        "push" => push,
        "pop" => pop,
    ];
    let class = methods![
        "arrowCursor" => class_cursor,
        "IBeamCursor" => class_cursor,
        "IBeamCursorForVerticalLayout" => class_cursor,
        "pointingHandCursor" => class_cursor,
        "resizeLeftRightCursor" => class_cursor,
        "resizeLeftCursor" => class_cursor,
        "resizeRightCursor" => class_cursor,
        "columnResizeCursor" => class_cursor,
        "resizeUpDownCursor" => class_cursor,
        "resizeUpCursor" => class_cursor,
        "resizeDownCursor" => class_cursor,
        "rowResizeCursor" => class_cursor,
        "crosshairCursor" => class_cursor,
        "openHandCursor" => class_cursor,
        "closedHandCursor" => class_cursor,
        "operationNotAllowedCursor" => class_cursor,
        "dragCopyCursor" => class_cursor,
        "dragLinkCursor" => class_cursor,
        "disappearingItemCursor" => class_cursor,
        "contextualMenuCursor" => class_cursor,
        "currentCursor" => current_cursor,
        "currentSystemCursor" => current_cursor,
        "hide" => hide,
        "unhide" => unhide,
        "setHiddenUntilMouseMoves:" => set_hidden_until_mouse_moves,
        "pop" => pop,
    ];
    super::define(
        runtime,
        b"NSCursor\0",
        (&NSCURSOR, &NSCURSOR_META),
        &root::NSOBJECT,
        core::mem::size_of::<CursorFields>(),
        &instance,
        &class,
    )
}
