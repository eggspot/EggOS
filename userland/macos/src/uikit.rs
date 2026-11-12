//! UIKit for iOS apps distributed outside the App Store, part 1 (M15, D473): an iOS app is an
//! arm64 Mach-O like a Mac app, so the macOS layer runs it natively (D17), and UIKit's first
//! classes stand on AppKit's (D147): `UIView` is `NSView`, `UIWindow` `NSWindow`, `UILabel` a
//! text field, `UIViewController` `NSViewController`, and so on, each class symbol bound to its
//! AppKit class. Where UIKit names a method differently, a send that finds no method under the
//! UIKit selector tries AppKit's ([`appkit_selector`]); `-[UIWindow initWithFrame:]` gets a
//! window of that frame. `UIApplicationMain` makes the shared application, makes the app
//! delegate of the class it names, tells it the app finished launching, and runs the app.

use crate::appkit::{
    NSAPPLICATION, NSAPPLICATION_META, NSBUTTON, NSBUTTON_META, NSCOLOR, NSCOLOR_META, NSFONT,
    NSFONT_META, NSRESPONDER, NSRESPONDER_META, NSSCREEN, NSSCREEN_META, NSTEXTFIELD,
    NSTEXTFIELD_META, NSVIEW, NSVIEW_META, NSVIEWCONTROLLER, NSVIEWCONTROLLER_META, NSWINDOW,
    NSWINDOW_META,
};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::{class_named, message, sel, send, send1, send2, send_bool};
use eggos_macos_gui::geometry::Frame;

const NO_OBJECT: usize = 0;
const WINDOW_STYLE: u64 = 1 | 2 | 4 | 8;
const BUFFERED: u64 = 2;

type Pair = (&'static [u8], &'static ClassObject, &'static ClassObject);

const CLASSES: [Pair; 10] = [
    (b"UIApplication", &NSAPPLICATION, &NSAPPLICATION_META),
    (b"UIResponder", &NSRESPONDER, &NSRESPONDER_META),
    (b"UIView", &NSVIEW, &NSVIEW_META),
    (b"UIWindow", &NSWINDOW, &NSWINDOW_META),
    (b"UIViewController", &NSVIEWCONTROLLER, &NSVIEWCONTROLLER_META),
    (b"UILabel", &NSTEXTFIELD, &NSTEXTFIELD_META),
    (b"UIButton", &NSBUTTON, &NSBUTTON_META),
    (b"UIColor", &NSCOLOR, &NSCOLOR_META),
    (b"UIFont", &NSFONT, &NSFONT_META),
    (b"UIScreen", &NSSCREEN, &NSSCREEN_META),
];

const SELECTORS: [(&[u8], &[u8]); 7] = [
    (b"setText:", b"setStringValue:"),
    (b"text", b"stringValue"),
    (b"makeKeyAndVisible", b"makeKeyAndOrderFront:"),
    (b"setRootViewController:", b"setContentViewController:"),
    (b"rootViewController", b"contentViewController"),
    (b"setTitle:forState:", b"setTitle:"),
    (b"bounds", b"frame"),
];

/// The address of UIKit symbol `name` (a class, its metaclass or `UIApplicationMain`).
pub fn resolve(name: &[u8]) -> Option<u64> {
    if name == b"_UIApplicationMain" {
        return Some(application_main as *const () as u64);
    }
    let (wanted, meta) = match (
        name.strip_prefix(b"_OBJC_CLASS_$_"),
        name.strip_prefix(b"_OBJC_METACLASS_$_"),
    ) {
        (Some(class), _) => (class, false),
        (None, Some(class)) => (class, true),
        (None, None) => return None,
    };
    let (_, class, metaclass) = CLASSES.iter().find(|(named, ..)| *named == wanted)?;
    let object = if meta { metaclass } else { class };
    Some(*object as *const ClassObject as u64)
}

/// The AppKit selector a UIKit selector `name` is answered by, when they differ.
pub fn appkit_selector(name: &[u8]) -> Option<&'static [u8]> {
    SELECTORS
        .iter()
        .find(|(uikit, _)| *uikit == name)
        .map(|(_, appkit)| *appkit)
}

/// What a send of `name` that found no method answers with on a UIKit object: the window's
/// `initWithFrame:`.
pub fn method_for(name: &[u8]) -> Option<usize> {
    match name {
        b"initWithFrame:" => Some(window_init_with_frame as *const () as usize),
        _ => None,
    }
}

extern "C" fn window_init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    let init: extern "C" fn(Id, Sel, Frame, u64, u64, u8) -> Id = message::msg_send_as();
    init(
        this,
        sel!("initWithContentRect:styleMask:backing:defer:"),
        frame,
        WINDOW_STYLE,
        BUFFERED,
        0,
    )
}

extern "C" fn application_main(_: i32, _: usize, _: Id, delegate_class: Id) -> i32 {
    let application = send(
        &NSAPPLICATION as *const ClassObject as Id,
        sel!("sharedApplication"),
    );
    if !delegate_class.is_null() {
        let name = send(delegate_class, sel!("UTF8String")) as *const u8;
        let length = (0..).take_while(|&index| {
            // SAFETY: `UTF8String` gives a NUL-terminated string kept while its string lives,
            // and the walk stops at the NUL.
            unsafe { *name.add(index) != 0 }
        });
        let length = length.count();
        // SAFETY: the `length` bytes before the NUL found above.
        let name = unsafe { core::slice::from_raw_parts(name, length) };
        let class = class_named(name);
        if !class.is_null() {
            let delegate = send(send(class as Id, sel!("alloc")), sel!("init"));
            send1(application, sel!("setDelegate:"), delegate as usize);
            let launched = sel!("application:didFinishLaunchingWithOptions:");
            if send_bool(delegate, sel!("respondsToSelector:"), launched as usize) {
                send2(delegate, launched, application as usize, NO_OBJECT);
            }
        }
    }
    send(application, sel!("run"));
    0
}
