//! AppKit (D147, `docs/ux/macos-gui-v0.3.md`): the classes a programmatic Objective-C app uses
//! to open windows on the window server and draw and take input in them, as classes of the
//! layer's own Objective-C runtime, set up only for a program that links AppKit (D18).
//! `NSApplication` runs the event loop over `/dev/window`'s records, `NSWindow` owns a server
//! window and its pixels, `NSView` and its subclasses draw into them through `eggos-macos-gui`
//! (`NSButton`, `NSTextField` as the toolkit's controls), `NSAlert` runs a dialog of its own,
//! and `NSMenu` is shown by the server in the top bar.

mod alert;
mod application;
mod control;
mod drawing;
mod event;
mod menu;
mod server;
mod view;
mod window;

use crate::foundation::{self, Definition};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::{RUNTIME, class, message, root};

macro_rules! class_pairs {
    ($($class:ident, $meta:ident;)*) => {
        $(
            pub static $class: ClassObject = ClassObject::EMPTY;
            pub static $meta: ClassObject = ClassObject::EMPTY;
        )*
    };
}

class_pairs! {
    NSRESPONDER, NSRESPONDER_META;
    NSVIEW, NSVIEW_META;
    NSCONTROL, NSCONTROL_META;
    NSBUTTON, NSBUTTON_META;
    NSTEXTFIELD, NSTEXTFIELD_META;
    NSWINDOW, NSWINDOW_META;
    NSAPPLICATION, NSAPPLICATION_META;
    NSEVENT, NSEVENT_META;
    NSCOLOR, NSCOLOR_META;
    NSFONT, NSFONT_META;
    NSBEZIERPATH, NSBEZIERPATH_META;
    NSALERT, NSALERT_META;
    NSMENU, NSMENU_META;
    NSMENUITEM, NSMENUITEM_META;
}

/// Sends `sel` with one `NSRect` argument.
pub fn send_frame(receiver: Id, sel: Sel, frame: eggos_macos_gui::geometry::Frame) {
    let function: extern "C" fn(Id, Sel, eggos_macos_gui::geometry::Frame) =
        message::msg_send_as();
    function(receiver, sel, frame);
}

/// Sends `sel` with no argument and reads a `BOOL` back.
pub fn ask(receiver: Id, sel: Sel) -> bool {
    let function: extern "C" fn(Id, Sel) -> u8 = message::msg_send_as();
    function(receiver, sel) != 0
}

/// Whether `object` (not nil) answers `sel`.
pub fn responds(object: Id, sel: Sel) -> bool {
    !object.is_null() && root::responds(class::class_of(object), sel)
}

/// Whether `object` is an instance of `class` or of a subclass.
pub fn is(object: Id, class: &'static ClassObject) -> bool {
    root::is_kind_of(object, class)
}

/// Stores `value` in `slot`, retaining it and releasing what the slot held.
pub fn store(slot: &mut Id, value: Id) {
    let old = *slot;
    *slot = if value.is_null() {
        value
    } else {
        objc_retain(value)
    };
    if !old.is_null() {
        objc_release(old);
    }
}

/// Stores a copy of the string `value` in `slot` (`copy` properties), releasing what it held.
pub fn store_copy(slot: &mut Id, value: Id) {
    let copy = if value.is_null() {
        value
    } else {
        crate::objc::send(value, crate::objc::sel!("copy"))
    };
    let old = *slot;
    *slot = copy;
    if !old.is_null() {
        objc_release(old);
    }
}

/// Runs `work` with the text of the string `object` (empty for nil or text that is not UTF-8).
pub fn with_text<R>(object: Id, work: impl FnOnce(&str) -> R) -> R {
    if object.is_null() {
        return work("");
    }
    foundation::with_utf8(object, |bytes| work(core::str::from_utf8(bytes).unwrap_or("")))
}

/// A new autoreleased string of `text`.
pub fn string(text: &str) -> Id {
    foundation::string_from_utf8(text.as_bytes())
}

/// The app's name, as its menu and untitled windows show it.
pub fn app_name() -> &'static str {
    core::str::from_utf8(foundation::app_name()).unwrap_or("")
}

/// One of AppKit's classes to define.
fn define(
    runtime: &mut crate::objc::Runtime,
    name: &'static [u8],
    pair: (&'static ClassObject, &'static ClassObject),
    superclass: &'static ClassObject,
    instance_size: usize,
    instance_methods: &[(&'static [u8], usize)],
    class_methods: &[(&'static [u8], usize)],
) -> bool {
    foundation::define(
        runtime,
        &Definition {
            name,
            class: pair.0,
            meta: pair.1,
            superclass,
            instance_size,
            flags: 0,
            instance_methods,
            class_methods,
        },
    )
}

/// Defines AppKit's classes and its additions to NSString; false if there is no memory.
pub fn set_up() -> bool {
    let defined = RUNTIME.with(|runtime| {
        view::define(runtime)
            && control::define(runtime)
            && window::define(runtime)
            && application::define(runtime)
            && event::define(runtime)
            && drawing::define(runtime)
            && alert::define(runtime)
            && menu::define(runtime)
    });
    defined && drawing::set_up()
}

/// The address of the AppKit function, variable or class `name`, if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let class = |object: &'static ClassObject| Some(object as *const ClassObject as u64);
    match name {
        b"_OBJC_CLASS_$_NSResponder" => class(&NSRESPONDER),
        b"_OBJC_METACLASS_$_NSResponder" => class(&NSRESPONDER_META),
        b"_OBJC_CLASS_$_NSView" => class(&NSVIEW),
        b"_OBJC_METACLASS_$_NSView" => class(&NSVIEW_META),
        b"_OBJC_CLASS_$_NSControl" => class(&NSCONTROL),
        b"_OBJC_METACLASS_$_NSControl" => class(&NSCONTROL_META),
        b"_OBJC_CLASS_$_NSButton" => class(&NSBUTTON),
        b"_OBJC_METACLASS_$_NSButton" => class(&NSBUTTON_META),
        b"_OBJC_CLASS_$_NSTextField" => class(&NSTEXTFIELD),
        b"_OBJC_METACLASS_$_NSTextField" => class(&NSTEXTFIELD_META),
        b"_OBJC_CLASS_$_NSWindow" => class(&NSWINDOW),
        b"_OBJC_METACLASS_$_NSWindow" => class(&NSWINDOW_META),
        b"_OBJC_CLASS_$_NSApplication" => class(&NSAPPLICATION),
        b"_OBJC_METACLASS_$_NSApplication" => class(&NSAPPLICATION_META),
        b"_OBJC_CLASS_$_NSEvent" => class(&NSEVENT),
        b"_OBJC_METACLASS_$_NSEvent" => class(&NSEVENT_META),
        b"_OBJC_CLASS_$_NSColor" => class(&NSCOLOR),
        b"_OBJC_METACLASS_$_NSColor" => class(&NSCOLOR_META),
        b"_OBJC_CLASS_$_NSFont" => class(&NSFONT),
        b"_OBJC_METACLASS_$_NSFont" => class(&NSFONT_META),
        b"_OBJC_CLASS_$_NSBezierPath" => class(&NSBEZIERPATH),
        b"_OBJC_METACLASS_$_NSBezierPath" => class(&NSBEZIERPATH_META),
        b"_OBJC_CLASS_$_NSAlert" => class(&NSALERT),
        b"_OBJC_METACLASS_$_NSAlert" => class(&NSALERT_META),
        b"_OBJC_CLASS_$_NSMenu" => class(&NSMENU),
        b"_OBJC_METACLASS_$_NSMenu" => class(&NSMENU_META),
        b"_OBJC_CLASS_$_NSMenuItem" => class(&NSMENUITEM),
        b"_OBJC_METACLASS_$_NSMenuItem" => class(&NSMENUITEM_META),
        _ => application::resolve(name).or_else(|| drawing::resolve(name)),
    }
}
