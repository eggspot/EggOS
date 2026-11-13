//! NSTrackingArea (D382): a rectangle of a view whose owner hears `mouseEntered:` and
//! `mouseExited:` as the pointer crosses its edge and `mouseMoved:` while it moves inside, as
//! its options ask. The areas are kept in one table; each pointer move in a window checks the
//! areas of that window's views, and the pointer leaving the window exits every area it was
//! in.

use super::{NSTRACKINGAREA, NSTRACKINGAREA_META, NSVIEW, event, store, view};
use crate::foundation::class_pointer;
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class, methods, sel, send1};
use eggos_macos_gui::geometry::{Frame, Point};

const ENTERED_AND_EXITED: usize = 0x01;
const MOUSE_MOVED: usize = 0x02;
const IN_VISIBLE_RECT: usize = 0x200;
const MOUSE_ENTERED_EVENT: usize = 8;
const MOUSE_EXITED_EVENT: usize = 9;

#[repr(C)]
#[derive(Debug)]
struct AreaFields {
    isa: usize,
    rect: Frame,
    options: usize,
    owner: Id,
    user_info: Id,
    view: Id,
    inside: bool,
}

static AREAS: Exclusive<Buffer<Id>> = Exclusive::new(Buffer::new());

fn area(object: Id) -> &'static mut AreaFields {
    // SAFETY: the callers pass NSTrackingArea instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<AreaFields>() }
}

fn each_area(work: &mut dyn FnMut(Id)) {
    let snapshot = AREAS.with(|areas| {
        let mut copy: Buffer<Id> = Buffer::new();
        for &each in areas.as_slice() {
            copy.push(objc_retain(each));
        }
        copy
    });
    for &each in snapshot.as_slice() {
        work(each);
        objc_release(each);
    }
    let mut snapshot = snapshot;
    snapshot.free();
}

fn rect_in_window(each: Id) -> Option<Frame> {
    let fields = area(each);
    if fields.view.is_null() || view::fields(fields.view).window.is_null() {
        return None;
    }
    let origin = view::origin_in_window(fields.view);
    let local = match fields.options & IN_VISIBLE_RECT {
        0 => fields.rect,
        _ => view::fields(fields.view).frame.at_origin(),
    };
    Some(local.offset(origin.x, origin.y))
}

fn tell(each: Id, kind: usize, location: Point, time_ms: u32) {
    let fields = area(each);
    let (owner, window) = (fields.owner, view::fields(fields.view).window);
    if owner.is_null() {
        return;
    }
    let sel = match kind {
        MOUSE_ENTERED_EVENT => sel!("mouseEntered:"),
        MOUSE_EXITED_EVENT => sel!("mouseExited:"),
        _ => sel!("mouseMoved:"),
    };
    let event = event::mouse(kind, window, location, 0, 0.0, time_ms);
    if !event.is_null() {
        send1(owner, sel, event as usize);
    }
}

/// The pointer moved to `location` in `window`: the areas of its views it entered, left or
/// moved inside hear of it.
pub fn pointer_moved(window: Id, location: Point, time_ms: u32) {
    each_area(&mut |each| {
        let fields = area(each);
        if fields.view.is_null() || view::fields(fields.view).window != window {
            return;
        }
        let Some(rect) = rect_in_window(each) else {
            return;
        };
        let inside = rect.contains(location);
        let was = area(each).inside;
        area(each).inside = inside;
        let options = area(each).options;
        if options & ENTERED_AND_EXITED != 0 && inside != was {
            let kind = match inside {
                true => MOUSE_ENTERED_EVENT,
                false => MOUSE_EXITED_EVENT,
            };
            tell(each, kind, location, time_ms);
        }
        if options & MOUSE_MOVED != 0 && inside {
            tell(each, event::kind::MOUSE_MOVED, location, time_ms);
        }
    });
}

/// The pointer left `window`: every area of its views it was in hears `mouseExited:`.
pub fn pointer_left(window: Id, time_ms: u32) {
    each_area(&mut |each| {
        let fields = area(each);
        if !fields.inside || fields.view.is_null() || view::fields(fields.view).window != window {
            return;
        }
        area(each).inside = false;
        if area(each).options & ENTERED_AND_EXITED != 0 {
            tell(each, MOUSE_EXITED_EVENT, Point::default(), time_ms);
        }
    });
}

/// Forgets the areas of `gone`, a view being freed.
pub fn forget_view(gone: Id) {
    let mut removed: Buffer<Id> = Buffer::new();
    AREAS.with(|areas| {
        let mut kept: Buffer<Id> = Buffer::new();
        for &each in areas.as_slice() {
            match area(each).view == gone {
                true => {
                    removed.push(each);
                }
                false => {
                    kept.push(each);
                }
            }
        }
        let mut old = core::mem::replace(areas, kept);
        old.free();
    });
    for &each in removed.as_slice() {
        area(each).view = core::ptr::null_mut();
        objc_release(each);
    }
    removed.free();
}

extern "C" fn init_with_rect(
    this: Id,
    _: Sel,
    rect: Frame,
    options: usize,
    owner: Id,
    user_info: Id,
) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = area(this);
    fields.rect = rect;
    fields.options = options;
    fields.owner = owner;
    store(&mut fields.user_info, user_info);
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    store(&mut area(this).user_info, core::ptr::null_mut());
    dispose(this);
}

extern "C" fn rect(this: Id, _: Sel) -> Frame {
    area(this).rect
}

extern "C" fn options(this: Id, _: Sel) -> usize {
    area(this).options
}

extern "C" fn owner(this: Id, _: Sel) -> Id {
    area(this).owner
}

extern "C" fn user_info(this: Id, _: Sel) -> Id {
    area(this).user_info
}

extern "C" fn add_tracking_area(this: Id, _: Sel, added: Id) {
    if added.is_null() || !area(added).view.is_null() {
        return;
    }
    area(added).view = this;
    area(added).inside = false;
    AREAS.with(|areas| {
        areas.push(objc_retain(added));
    });
}

extern "C" fn remove_tracking_area(this: Id, _: Sel, removed: Id) {
    if removed.is_null() || area(removed).view != this {
        return;
    }
    let found = AREAS.with(|areas| {
        let at = areas.as_slice().iter().position(|&each| each == removed)?;
        let mut kept: Buffer<Id> = Buffer::new();
        for (index, &each) in areas.as_slice().iter().enumerate() {
            if index != at {
                kept.push(each);
            }
        }
        let mut old = core::mem::replace(areas, kept);
        old.free();
        Some(())
    });
    if found.is_some() {
        area(removed).view = core::ptr::null_mut();
        objc_release(removed);
    }
}

extern "C" fn tracking_areas(this: Id, _: Sel) -> Id {
    let mut mine: Buffer<Id> = Buffer::new();
    AREAS.with(|areas| {
        for &each in areas.as_slice() {
            if area(each).view == this {
                mine.push(each);
            }
        }
    });
    let array = crate::objc::send2(
        class_pointer(&crate::foundation::NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        mine.as_slice().as_ptr() as usize,
        mine.len(),
    );
    mine.free();
    array
}

extern "C" fn update_tracking_areas(_: Id, _: Sel) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithRect:options:owner:userInfo:" => init_with_rect,
        "dealloc" => dealloc,
        "rect" => rect,
        "options" => options,
        "owner" => owner,
        "userInfo" => user_info,
    ];
    let view_methods = methods![
        "addTrackingArea:" => add_tracking_area,
        "removeTrackingArea:" => remove_tracking_area,
        "trackingAreas" => tracking_areas,
        "updateTrackingAreas" => update_tracking_areas,
    ];
    super::define(
        runtime,
        b"NSTrackingArea\0",
        (&NSTRACKINGAREA, &NSTRACKINGAREA_META),
        &NSOBJECT,
        core::mem::size_of::<AreaFields>(),
        &instance,
        &[],
    ) && class::extend(runtime, &NSVIEW, &view_methods, &[])
}
