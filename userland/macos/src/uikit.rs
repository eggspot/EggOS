//! UIKit for iOS apps distributed outside the App Store, part 1 (M15, D473): an iOS app is an
//! arm64 Mach-O like a Mac app, so the macOS layer runs it natively (D17), and UIKit's first
//! classes stand on AppKit's (D147): `UIView` is `NSView`, `UIWindow` `NSWindow`, `UILabel` a
//! text field, `UIViewController` `NSViewController`, and so on, each class symbol bound to its
//! AppKit class. Where UIKit names a method differently, a send that finds no method under the
//! UIKit selector tries AppKit's ([`appkit_selector`]); `-[UIWindow initWithFrame:]` gets a
//! window of that frame. `UIApplicationMain` makes the shared application, makes the app
//! delegate of the class it names, tells it the app finished launching, and runs the app.
//!
//! Part 2 (D501): an app whose `Info.plist` names a scene delegate
//! (`UIApplicationSceneManifest`) gets a window scene and its session, the delegate of that
//! class connected to them (`scene:willConnectToSession:options:`, then foreground and active),
//! and `-[UIWindow initWithWindowScene:]` makes a phone-sized window. Swift class names
//! (`MyApp.SceneDelegate`) are found by their Objective-C names. A view whose class answers
//! `touchesBegan:withEvent:` and its siblings gets the mouse's presses, moves and releases as a
//! set of one `UITouch` (its place in any view, its phase, its view and window), and a control's
//! `addTarget:action:forControlEvents:` for a touch up inside or a primary action becomes its
//! target and action.
//!
//! Part 3 (D505): `UITapGestureRecognizer` (and its base `UIGestureRecognizer`) is a class of
//! the layer's: `initWithTarget:action:` and `addTarget:action:` keep its target and action,
//! `addGestureRecognizer:` attaches it to a view, and a press released over the view it was
//! pressed on fires it after the view's own touches, its `state` then ended and its place in
//! any view the release's.

use crate::appkit::{
    NSAPPLICATION, NSAPPLICATION_META, NSBUTTON, NSBUTTON_META, NSCOLOR, NSCOLOR_META,
    NSCONTROL, NSCONTROL_META, NSEVENT, NSEVENT_META, NSFONT, NSFONT_META, NSIMAGEVIEW,
    NSIMAGEVIEW_META, NSRESPONDER, NSRESPONDER_META, NSSCREEN, NSSCREEN_META, NSTEXTFIELD,
    NSTEXTFIELD_META, NSVIEW, NSVIEW_META, NSVIEWCONTROLLER, NSVIEWCONTROLLER_META, NSWINDOW,
    NSWINDOW_META,
};
use crate::foundation::{self, Definition, NSSET, class_pointer, new_instance};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class_named, message, methods, sel, send, send1, send2, send_bool};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::geometry::{Frame, Point};

const NO_OBJECT: usize = 0;
const WINDOW_STYLE: u64 = 1 | 2 | 4 | 8;
const BUFFERED: u64 = 2;
const PHONE_FRAME: Frame = Frame {
    x: 0.0,
    y: 0.0,
    width: 390.0,
    height: 844.0,
};
const TOUCH_UP_INSIDE: u64 = 1 << 6;
const PRIMARY_ACTION_TRIGGERED: u64 = 1 << 13;
const LONGEST_CLASS_NAME: usize = 256;

/// `UITouchPhase`.
mod phase {
    pub const BEGAN: u64 = 0;
    pub const MOVED: u64 = 1;
    pub const ENDED: u64 = 3;
}

/// UIKit's touch class, defined by the layer (D501).
pub static UITOUCH: ClassObject = ClassObject::EMPTY;
/// Its metaclass.
pub static UITOUCH_META: ClassObject = ClassObject::EMPTY;
/// UIKit's gesture recognizers' base class and the tap recognizer, defined by the layer (D505).
pub static UIGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UIGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UITAPGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UITAPGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;

const MOST_RECOGNIZERS: usize = 128;
const STATE_POSSIBLE: u64 = 0;
const STATE_ENDED: u64 = 3;

/// Which recognizer is attached to which view: pairs of addresses, a pair's view 0 when free.
static ATTACHED: [[AtomicUsize; 2]; MOST_RECOGNIZERS] =
    [const { [AtomicUsize::new(0), AtomicUsize::new(0)] }; MOST_RECOGNIZERS];

static SCENE: AtomicUsize = AtomicUsize::new(0);

type Pair = (&'static [u8], &'static ClassObject, &'static ClassObject);

const CLASSES: [Pair; 20] = [
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
    (b"UIControl", &NSCONTROL, &NSCONTROL_META),
    (b"UIImageView", &NSIMAGEVIEW, &NSIMAGEVIEW_META),
    (b"UIEvent", &NSEVENT, &NSEVENT_META),
    (b"UITouch", &UITOUCH, &UITOUCH_META),
    (b"UIScene", &NSRESPONDER, &NSRESPONDER_META),
    (b"UIWindowScene", &NSRESPONDER, &NSRESPONDER_META),
    (b"UISceneSession", &NSRESPONDER, &NSRESPONDER_META),
    (b"UISceneConfiguration", &NSRESPONDER, &NSRESPONDER_META),
    (b"UIGestureRecognizer", &UIGESTURERECOGNIZER, &UIGESTURERECOGNIZER_META),
    (b"UITapGestureRecognizer", &UITAPGESTURERECOGNIZER, &UITAPGESTURERECOGNIZER_META),
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
        b"initWithWindowScene:" => Some(window_init_with_scene as *const () as usize),
        b"addTarget:action:forControlEvents:" => Some(add_target as *const () as usize),
        b"windowScene" => Some(window_scene as *const () as usize),
        b"addGestureRecognizer:" => Some(add_gesture_recognizer as *const () as usize),
        b"removeGestureRecognizer:" => Some(remove_gesture_recognizer as *const () as usize),
        _ => None,
    }
}

#[repr(C)]
struct RecognizerFields {
    isa: usize,
    target: Id,
    action: Sel,
    view: Id,
    location: Point,
    state: u64,
    enabled: u8,
}

fn recognizer(object: Id) -> &'static mut RecognizerFields {
    // SAFETY: the callers pass gesture recognizer instances, whose memory starts with these
    // fields.
    unsafe { &mut *object.cast::<RecognizerFields>() }
}

extern "C" fn recognizer_init_with_target(this: Id, _: Sel, target: Id, action: Sel) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = recognizer(this);
    fields.target = target;
    fields.action = action;
    fields.enabled = 1;
    this
}

extern "C" fn recognizer_add_target(this: Id, _: Sel, target: Id, action: Sel) {
    let fields = recognizer(this);
    fields.target = target;
    fields.action = action;
}

extern "C" fn recognizer_view(this: Id, _: Sel) -> Id {
    recognizer(this).view
}

extern "C" fn recognizer_state(this: Id, _: Sel) -> u64 {
    recognizer(this).state
}

extern "C" fn recognizer_location_in_view(this: Id, _: Sel, view: Id) -> Point {
    let fields = recognizer(this);
    if view.is_null() {
        return fields.location;
    }
    let convert: extern "C" fn(Id, Sel, Point, Id) -> Point = message::msg_send_as();
    convert(view, sel!("convertPoint:fromView:"), fields.location, core::ptr::null_mut())
}

extern "C" fn recognizer_is_enabled(this: Id, _: Sel) -> u8 {
    recognizer(this).enabled
}

extern "C" fn recognizer_set_enabled(this: Id, _: Sel, enabled: u8) {
    recognizer(this).enabled = enabled;
}

extern "C" fn add_gesture_recognizer(this: Id, _: Sel, gesture: Id) {
    if gesture.is_null() {
        return;
    }
    for pair in &ATTACHED {
        if pair[0]
            .compare_exchange(0, this as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            pair[1].store(gesture as usize, Ordering::Release);
            recognizer(gesture).view = this;
            return;
        }
    }
}

extern "C" fn remove_gesture_recognizer(this: Id, _: Sel, gesture: Id) {
    for pair in &ATTACHED {
        if pair[0].load(Ordering::Acquire) == this as usize
            && pair[1].load(Ordering::Acquire) == gesture as usize
        {
            pair[1].store(0, Ordering::Release);
            pair[0].store(0, Ordering::Release);
        }
    }
}

fn fire_taps(view: Id, location: Point) {
    for pair in &ATTACHED {
        if pair[0].load(Ordering::Acquire) != view as usize {
            continue;
        }
        let gesture = pair[1].load(Ordering::Acquire) as Id;
        if gesture.is_null() {
            continue;
        }
        let fields = recognizer(gesture);
        if fields.enabled == 0 || fields.target.is_null() || fields.action.is_null() {
            continue;
        }
        fields.location = location;
        fields.state = STATE_ENDED;
        send1(fields.target, fields.action, gesture as usize);
        fields.state = STATE_POSSIBLE;
    }
}

extern "C" fn window_init_with_scene(this: Id, sel: Sel, _scene: Id) -> Id {
    window_init_with_frame(this, sel, PHONE_FRAME)
}

extern "C" fn window_scene(_: Id, _: Sel) -> Id {
    SCENE.load(Ordering::Acquire) as Id
}

extern "C" fn add_target(this: Id, _: Sel, target: Id, action: Sel, events: u64) {
    if events & (TOUCH_UP_INSIDE | PRIMARY_ACTION_TRIGGERED) != 0 {
        send1(this, sel!("setTarget:"), target as usize);
        send1(this, sel!("setAction:"), action as usize);
    }
}

#[repr(C)]
struct TouchFields {
    isa: usize,
    location: Point,
    view: Id,
    window: Id,
    phase: u64,
}

fn touch(object: Id) -> &'static mut TouchFields {
    // SAFETY: the callers pass UITouch instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<TouchFields>() }
}

extern "C" fn touch_location_in_view(this: Id, _: Sel, view: Id) -> Point {
    let fields = touch(this);
    if view.is_null() {
        return fields.location;
    }
    let convert: extern "C" fn(Id, Sel, Point, Id) -> Point = message::msg_send_as();
    convert(view, sel!("convertPoint:fromView:"), fields.location, core::ptr::null_mut())
}

extern "C" fn touch_view(this: Id, _: Sel) -> Id {
    touch(this).view
}

extern "C" fn touch_window(this: Id, _: Sel) -> Id {
    touch(this).window
}

extern "C" fn touch_phase(this: Id, _: Sel) -> u64 {
    touch(this).phase
}

extern "C" fn one(_: Id, _: Sel) -> u64 {
    1
}

/// Defines UIKit's own classes (`UITouch`); false if there is no memory.
pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "locationInView:" => touch_location_in_view,
        "previousLocationInView:" => touch_location_in_view,
        "view" => touch_view,
        "window" => touch_window,
        "phase" => touch_phase,
        "tapCount" => one,
    ];
    let gesture = methods![
        "initWithTarget:action:" => recognizer_init_with_target,
        "addTarget:action:" => recognizer_add_target,
        "view" => recognizer_view,
        "state" => recognizer_state,
        "locationInView:" => recognizer_location_in_view,
        "isEnabled" => recognizer_is_enabled,
        "setEnabled:" => recognizer_set_enabled,
        "numberOfTouches" => one,
    ];
    let tap = methods![
        "numberOfTapsRequired" => one,
        "numberOfTouchesRequired" => one,
    ];
    let define = |runtime: &mut Runtime, definition: Definition<'_>| {
        foundation::define(runtime, &definition)
    };
    define(
        runtime,
        Definition {
            name: b"UITouch\0",
            class: &UITOUCH,
            meta: &UITOUCH_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<TouchFields>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UIGestureRecognizer\0",
            class: &UIGESTURERECOGNIZER,
            meta: &UIGESTURERECOGNIZER_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &gesture,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UITapGestureRecognizer\0",
            class: &UITAPGESTURERECOGNIZER,
            meta: &UITAPGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &tap,
            class_methods: &[],
        },
    )
}

fn deliver(view: Id, event: Id, phase: u64, selector: Sel) {
    if view.is_null() || !send_bool(view, sel!("respondsToSelector:"), selector as usize) {
        return;
    }
    let touch_object = new_instance((&raw const UITOUCH).cast_mut());
    if touch_object.is_null() {
        return;
    }
    let location: extern "C" fn(Id, Sel) -> Point = message::msg_send_as();
    let fields = touch(touch_object);
    fields.location = location(event, sel!("locationInWindow"));
    fields.view = view;
    fields.window = send(view, sel!("window"));
    fields.phase = phase;
    let touches = send1(
        class_pointer(&NSSET).cast(),
        sel!("setWithObject:"),
        touch_object as usize,
    );
    send2(view, selector, touches as usize, event as usize);
}

/// A view's `mouseDown:` (D501): `touchesBegan:withEvent:` when its class answers it.
pub extern "C" fn touches_began(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::BEGAN, sel!("touchesBegan:withEvent:"));
}

/// A view's `mouseDragged:`: `touchesMoved:withEvent:`.
pub extern "C" fn touches_moved(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::MOVED, sel!("touchesMoved:withEvent:"));
}

/// A view's `mouseUp:`: `touchesEnded:withEvent:`, then the view's tap recognizers.
pub extern "C" fn touches_ended(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::ENDED, sel!("touchesEnded:withEvent:"));
    if !this.is_null() && !event.is_null() {
        let location: extern "C" fn(Id, Sel) -> Point = message::msg_send_as();
        fire_taps(this, location(event, sel!("locationInWindow")));
    }
}

/// The Objective-C name a Swift class `Module.Class` is registered under:
/// `_TtC<length>Module<length>Class`.
pub fn swift_class_name(name: &[u8], out: &mut [u8; LONGEST_CLASS_NAME]) -> Option<usize> {
    let dot = name.iter().position(|&byte| byte == b'.')?;
    let (module, class) = (&name[..dot], &name[dot + 1..]);
    if module.is_empty() || class.is_empty() || class.contains(&b'.') {
        return None;
    }
    let mut written = 0;
    let mut push = |bytes: &[u8]| -> Option<()> {
        out.get_mut(written..written + bytes.len())?.copy_from_slice(bytes);
        written += bytes.len();
        Some(())
    };
    push(b"_TtC")?;
    for part in [module, class] {
        let mut digits = [0u8; 20];
        let mut count = part.len();
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (count % 10) as u8;
            count /= 10;
            if count == 0 {
                break;
            }
        }
        push(&digits[at..])?;
        push(part)?;
    }
    Some(written)
}

fn class_of_name(name: &[u8]) -> Id {
    let class = class_named(name);
    if !class.is_null() {
        return class as Id;
    }
    let mut mangled = [0u8; LONGEST_CLASS_NAME];
    match swift_class_name(name, &mut mangled) {
        Some(length) => class_named(&mangled[..length]) as Id,
        None => core::ptr::null_mut(),
    }
}

fn scene_delegate_name(out: &mut [u8; LONGEST_CLASS_NAME]) -> Option<usize> {
    let document = eggos_plist::Document::parse(foundation::info_plist_bytes()).ok()?;
    let name = document
        .top()
        .get(b"UIApplicationSceneManifest")?
        .get(b"UISceneConfigurations")?
        .get(b"UIWindowSceneSessionRoleApplication")?
        .item(0)?
        .get(b"UISceneDelegateClassName")?
        .text()?;
    Some(name.write_utf8(out)?.len())
}

fn connect_scene() {
    let mut name = [0u8; LONGEST_CLASS_NAME];
    let Some(length) = scene_delegate_name(&mut name) else {
        return;
    };
    let class = class_of_name(&name[..length]);
    if class.is_null() {
        return;
    }
    let make = |class: &ClassObject| {
        let class = class as *const ClassObject as Id;
        send(send(class, sel!("alloc")), sel!("init"))
    };
    let scene = make(&NSRESPONDER);
    let session = make(&NSRESPONDER);
    SCENE.store(scene as usize, Ordering::Release);
    let delegate = send(send(class, sel!("alloc")), sel!("init"));
    let connect = sel!("scene:willConnectToSession:options:");
    if send_bool(delegate, sel!("respondsToSelector:"), connect as usize) {
        let send3: extern "C" fn(Id, Sel, Id, Id, Id) -> Id = message::msg_send_as();
        send3(delegate, connect, scene, session, core::ptr::null_mut());
    }
    for step in [sel!("sceneWillEnterForeground:"), sel!("sceneDidBecomeActive:")] {
        if send_bool(delegate, sel!("respondsToSelector:"), step as usize) {
            send1(delegate, step, scene as usize);
        }
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
        let class = class_of_name(name);
        if !class.is_null() {
            let delegate = send(send(class, sel!("alloc")), sel!("init"));
            send1(application, sel!("setDelegate:"), delegate as usize);
            let launched = sel!("application:didFinishLaunchingWithOptions:");
            if send_bool(delegate, sel!("respondsToSelector:"), launched as usize) {
                send2(delegate, launched, application as usize, NO_OBJECT);
            }
        }
    }
    connect_scene();
    send(application, sel!("run"));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_classes_are_found_by_their_mangled_names() {
        let mut out = [0u8; LONGEST_CLASS_NAME];
        let length = swift_class_name(b"MyApp.SceneDelegate", &mut out).unwrap();
        assert_eq!(&out[..length], b"_TtC5MyApp13SceneDelegate");
        assert_eq!(swift_class_name(b"SceneDelegate", &mut out), None);
        assert_eq!(swift_class_name(b"A.B.C", &mut out), None);
    }
}
