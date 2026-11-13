//! NSPanel and the About panel in the app's window list (D407): NSPanel is a window that can
//! float and become key only when needed; the standard About panel, which the layer draws
//! itself (`about.rs`), stands in `[NSApp windows]` as an NSPanel of a private subclass whose
//! title, visibility, frame and window number are the panel's, and whose `close`,
//! `orderOut:` and `makeKeyAndOrderFront:` close and show it, so an app that walks its windows
//! (to close them all, or to find the About panel) sees it as on macOS.

use super::{NSABOUTPANEL, NSABOUTPANEL_META, NSPANEL, NSPANEL_META, NSWINDOW, about, window};
use crate::foundation::{class_pointer, new_instance, string_from_utf8};
use crate::objc::abi::{Id, Sel};
use crate::objc::{Runtime, methods};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::geometry::Frame;

static STAND_IN: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Debug)]
struct PanelFields {
    window: window::WindowFields,
    floating: bool,
    key_only_if_needed: bool,
    works_when_modal: bool,
}

fn fields(object: Id) -> &'static mut PanelFields {
    // SAFETY: the callers pass NSPanel instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<PanelFields>() }
}

extern "C" fn floating(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).floating)
}

extern "C" fn set_floating(this: Id, _: Sel, value: u8) {
    fields(this).floating = value != 0;
}

extern "C" fn key_only_if_needed(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).key_only_if_needed)
}

extern "C" fn set_key_only_if_needed(this: Id, _: Sel, value: u8) {
    fields(this).key_only_if_needed = value != 0;
}

extern "C" fn works_when_modal(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).works_when_modal)
}

extern "C" fn set_works_when_modal(this: Id, _: Sel, value: u8) {
    fields(this).works_when_modal = value != 0;
}

/// The About panel's stand-in, while the panel is shown.
pub fn about_stand_in() -> Option<Id> {
    if about::server_id() == 0 {
        return None;
    }
    let existing = STAND_IN.load(Ordering::Acquire) as Id;
    if !existing.is_null() {
        return Some(existing);
    }
    let made = new_instance(class_pointer(&NSABOUTPANEL));
    if made.is_null() {
        return None;
    }
    match STAND_IN.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(made),
        Err(winner) => {
            crate::objc::reference_count::objc_release(made);
            Some(winner as Id)
        }
    }
}

extern "C" fn about_title(_: Id, _: Sel) -> Id {
    string_from_utf8(about::title().as_str().as_bytes())
}

extern "C" fn about_visible(_: Id, _: Sel) -> u8 {
    u8::from(about::server_id() != 0)
}

extern "C" fn about_close(_: Id, _: Sel) {
    about::close();
}

extern "C" fn about_close_from(_: Id, _: Sel, _sender: Id) {
    about::close();
}

extern "C" fn about_show(_: Id, _: Sel, _sender: Id) {
    about::show(core::ptr::null_mut());
}

extern "C" fn about_number(_: Id, _: Sel) -> i64 {
    i64::from(about::server_id())
}

extern "C" fn about_frame(_: Id, _: Sel) -> Frame {
    let (width, height) = about::size();
    Frame::new(0.0, 0.0, f64::from(width), f64::from(height))
}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn yes(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn nothing_view(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

pub fn define(runtime: &mut Runtime) -> bool {
    let panel = methods![
        "isFloatingPanel" => floating,
        "setFloatingPanel:" => set_floating,
        "becomesKeyOnlyIfNeeded" => key_only_if_needed,
        "setBecomesKeyOnlyIfNeeded:" => set_key_only_if_needed,
        "worksWhenModal" => works_when_modal,
        "setWorksWhenModal:" => set_works_when_modal,
    ];
    let stand_in = methods![
        "title" => about_title,
        "isVisible" => about_visible,
        "close" => about_close,
        "performClose:" => about_close_from,
        "orderOut:" => about_close_from,
        "makeKeyAndOrderFront:" => about_show,
        "orderFront:" => about_show,
        "windowNumber" => about_number,
        "frame" => about_frame,
        "contentView" => nothing_view,
        "isMiniaturized" => no,
        "isZoomed" => no,
        "isReleasedWhenClosed" => no,
        "isFloatingPanel" => yes,
        "canBecomeMainWindow" => no,
    ];
    super::define(
        runtime,
        b"NSPanel\0",
        (&NSPANEL, &NSPANEL_META),
        &NSWINDOW,
        core::mem::size_of::<PanelFields>(),
        &panel,
        &[],
    ) && super::define(
        runtime,
        b"_EggOSAboutPanel\0",
        (&NSABOUTPANEL, &NSABOUTPANEL_META),
        &NSPANEL,
        core::mem::size_of::<PanelFields>(),
        &stand_in,
        &[],
    )
}
