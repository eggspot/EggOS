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
//!
//! Part 4 (D515): `UILongPressGestureRecognizer` fires (began, then ended) when a press held
//! on the view for its `minimumPressDuration` (0.5 s) is released without moving far, and
//! `UIPanGestureRecognizer` fires as the press moves (began, changed) and when it is released
//! (ended), its `translationInView:` how far the press has moved since it began.
//!
//! Part 5 (D517): `UISwipeGestureRecognizer` fires (ended) when a press released within
//! half a second has moved at least 50 points, mostly along one axis, in a direction its
//! `direction` mask allows (right 1, left 2, up 4, down 8; right by default).
//!
//! Part 6 (D768): several touches at once (a second finger's `UITouch` delivered to the view
//! under it), `UIPinchGestureRecognizer` (`scale`, `velocity`) from the window server's
//! pinches and `UIRotationGestureRecognizer` (`rotation`, `velocity`) from the angle between
//! two fingers, a pan's `velocityInView:`, taps counted for `numberOfTapsRequired`,
//! `requireGestureRecognizerToFail:` (a recognizer waits for a multi-tap one to fail before it
//! fires), `safeAreaInsets` for the phone's status bar and home indicator, `UIScreen`'s
//! `mainScreen`, `bounds`, `scale` and `nativeBounds`, and `traitCollection` with the size
//! classes and idiom of a phone in portrait.

use crate::appkit::{
    NSAPPLICATION, NSAPPLICATION_META, NSBUTTON, NSBUTTON_META, NSCOLOR, NSCOLOR_META, NSCONTROL,
    NSCONTROL_META, NSEVENT, NSEVENT_META, NSFONT, NSFONT_META, NSIMAGEVIEW, NSIMAGEVIEW_META,
    NSLAYOUTANCHOR, NSLAYOUTANCHOR_META, NSLAYOUTCONSTRAINT, NSLAYOUTCONSTRAINT_META, NSRESPONDER,
    NSRESPONDER_META, NSSCREEN, NSSCREEN_META, NSSTACKVIEW, NSSTACKVIEW_META, NSTEXTFIELD,
    NSTEXTFIELD_META, NSVIEW, NSVIEW_META, NSVIEWCONTROLLER, NSVIEWCONTROLLER_META, NSWINDOW,
    NSWINDOW_META,
};
use crate::foundation::{self, Definition, NSSET, class_pointer, new_instance};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, class_named, message, methods, sel, send, send_bool, send1, send2};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::geometry::{Frame, Point};
use eggos_toolkit::input::TouchPhase;

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
pub static UILONGPRESSGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UILONGPRESSGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UIPANGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UIPANGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UISWIPEGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UISWIPEGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UIPINCHGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UIPINCHGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UIROTATIONGESTURERECOGNIZER: ClassObject = ClassObject::EMPTY;
pub static UIROTATIONGESTURERECOGNIZER_META: ClassObject = ClassObject::EMPTY;
pub static UITRAITCOLLECTION: ClassObject = ClassObject::EMPTY;
pub static UITRAITCOLLECTION_META: ClassObject = ClassObject::EMPTY;

const DOUBLE_TAP_SECONDS: f64 = 0.3;
const DOUBLE_TAP_MS: u64 = 300;
const SAFE_TOP: f64 = 47.0;
const SAFE_BOTTOM: f64 = 34.0;
const PHONE_SCALE: f64 = 3.0;
const SIZE_CLASS_COMPACT: i64 = 1;
const SIZE_CLASS_REGULAR: i64 = 2;
const IDIOM_PHONE: i64 = 0;

static TAPS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
static FINGER: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static DEFERRED: [[AtomicUsize; 2]; MOST_RECOGNIZERS] =
    [const { [AtomicUsize::new(0), AtomicUsize::new(0)] }; MOST_RECOGNIZERS];

const SWIPE_POINTS: f64 = 50.0;
const SWIPE_SECONDS: f64 = 0.5;
const SWIPE_RIGHT: u64 = 1;
const SWIPE_LEFT: u64 = 2;
const SWIPE_UP: u64 = 4;
const SWIPE_DOWN: u64 = 8;

const STATE_BEGAN: u64 = 1;
const STATE_CHANGED: u64 = 2;
const LONG_PRESS_SECONDS: f64 = 0.5;
const TAP_SLOP: f64 = 10.0;

static PRESS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static PRIMARY: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

const MOST_RECOGNIZERS: usize = 128;
const STATE_POSSIBLE: u64 = 0;
const STATE_ENDED: u64 = 3;

static ATTACHED: [[AtomicUsize; 2]; MOST_RECOGNIZERS] =
    [const { [AtomicUsize::new(0), AtomicUsize::new(0)] }; MOST_RECOGNIZERS];

static SCENE: AtomicUsize = AtomicUsize::new(0);

type Pair = (&'static [u8], &'static ClassObject, &'static ClassObject);

const CLASSES: [Pair; 29] = [
    (
        b"UIPinchGestureRecognizer",
        &UIPINCHGESTURERECOGNIZER,
        &UIPINCHGESTURERECOGNIZER_META,
    ),
    (
        b"UIRotationGestureRecognizer",
        &UIROTATIONGESTURERECOGNIZER,
        &UIROTATIONGESTURERECOGNIZER_META,
    ),
    (b"UITraitCollection", &UITRAITCOLLECTION, &UITRAITCOLLECTION_META),
    (b"UIApplication", &NSAPPLICATION, &NSAPPLICATION_META),
    (b"UIResponder", &NSRESPONDER, &NSRESPONDER_META),
    (b"UIView", &NSVIEW, &NSVIEW_META),
    (b"UIWindow", &NSWINDOW, &NSWINDOW_META),
    (
        b"UIViewController",
        &NSVIEWCONTROLLER,
        &NSVIEWCONTROLLER_META,
    ),
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
    (
        b"UIGestureRecognizer",
        &UIGESTURERECOGNIZER,
        &UIGESTURERECOGNIZER_META,
    ),
    (
        b"UITapGestureRecognizer",
        &UITAPGESTURERECOGNIZER,
        &UITAPGESTURERECOGNIZER_META,
    ),
    (
        b"NSLayoutConstraint",
        &NSLAYOUTCONSTRAINT,
        &NSLAYOUTCONSTRAINT_META,
    ),
    (b"NSLayoutAnchor", &NSLAYOUTANCHOR, &NSLAYOUTANCHOR_META),
    (b"UIStackView", &NSSTACKVIEW, &NSSTACKVIEW_META),
    (
        b"UILongPressGestureRecognizer",
        &UILONGPRESSGESTURERECOGNIZER,
        &UILONGPRESSGESTURERECOGNIZER_META,
    ),
    (
        b"UIPanGestureRecognizer",
        &UIPANGESTURERECOGNIZER,
        &UIPANGESTURERECOGNIZER_META,
    ),
    (
        b"UISwipeGestureRecognizer",
        &UISWIPEGESTURERECOGNIZER,
        &UISWIPEGESTURERECOGNIZER_META,
    ),
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
        b"safeAreaInsets" => Some(safe_area_insets as *const () as usize),
        b"traitCollection" => Some(trait_collection as *const () as usize),
        b"scale" => Some(screen_scale as *const () as usize),
        b"nativeScale" => Some(screen_scale as *const () as usize),
        b"nativeBounds" => Some(screen_native_bounds as *const () as usize),
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
    start: Point,
    direction: u64,
    taps_required: u64,
    touches_required: u64,
    requires: Id,
    previous: Point,
    previous_time: f64,
    velocity: Point,
    value: f64,
    start_value: f64,
    rate: f64,
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
    fields.taps_required = 1;
    fields.touches_required = 1;
    fields.value = match kind_of(this) {
        Kind::Pinch => 1.0,
        _ => 0.0,
    };
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
    convert(
        view,
        sel!("convertPoint:fromView:"),
        fields.location,
        core::ptr::null_mut(),
    )
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tap,
    LongPress,
    Pan,
    Swipe,
    Pinch,
    Rotation,
}

fn kind_of(gesture: Id) -> Kind {
    let is = |class: &ClassObject| {
        send_bool(
            gesture,
            sel!("isKindOfClass:"),
            class as *const ClassObject as usize,
        )
    };
    if is(&UIPINCHGESTURERECOGNIZER) {
        Kind::Pinch
    } else if is(&UIROTATIONGESTURERECOGNIZER) {
        Kind::Rotation
    } else if is(&UIPANGESTURERECOGNIZER) {
        Kind::Pan
    } else if is(&UISWIPEGESTURERECOGNIZER) {
        Kind::Swipe
    } else if is(&UILONGPRESSGESTURERECOGNIZER) {
        Kind::LongPress
    } else {
        Kind::Tap
    }
}

fn each_recognizer(view: Id, mut each: impl FnMut(Id, &'static mut RecognizerFields, Kind)) {
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
        each(gesture, fields, kind_of(gesture));
    }
}

fn fire(gesture: Id, fields: &mut RecognizerFields, state: u64) {
    fields.state = state;
    send1(fields.target, fields.action, gesture as usize);
}

fn press_started(location: Point, time: f64) {
    store_point(&PRIMARY, location);
    PRESS[0].store(location.x.to_bits() as usize, Ordering::Relaxed);
    PRESS[1].store(location.y.to_bits() as usize, Ordering::Relaxed);
    PRESS[2].store(time.to_bits() as usize, Ordering::Relaxed);
}

fn press() -> (Point, f64) {
    let read = |index: usize| f64::from_bits(PRESS[index].load(Ordering::Relaxed) as u64);
    (Point::new(read(0), read(1)), read(2))
}

fn store_point(slots: &[AtomicUsize], point: Point) {
    slots[0].store(point.x.to_bits() as usize, Ordering::Relaxed);
    slots[1].store(point.y.to_bits() as usize, Ordering::Relaxed);
}

fn load_point(slots: &[AtomicUsize]) -> Point {
    let read = |index: usize| f64::from_bits(slots[index].load(Ordering::Relaxed) as u64);
    Point::new(read(0), read(1))
}

fn moved(view: Id, location: Point, time: f64) {
    let (start, _) = press();
    store_point(&PRIMARY, location);
    each_recognizer(view, |gesture, fields, kind| {
        if kind != Kind::Pan {
            return;
        }
        let elapsed = time - fields.previous_time;
        if fields.previous_time > 0.0 && elapsed > 0.0 {
            fields.velocity = Point::new(
                (location.x - fields.previous.x) / elapsed,
                (location.y - fields.previous.y) / elapsed,
            );
        }
        fields.previous = location;
        fields.previous_time = time;
        fields.location = location;
        let state = match fields.state {
            STATE_BEGAN | STATE_CHANGED => STATE_CHANGED,
            _ => {
                fields.start = start;
                STATE_BEGAN
            }
        };
        fire(gesture, fields, state);
    });
}

/// Counts a tap released at `location` at `time`: one more when it follows the last within
/// [`DOUBLE_TAP_SECONDS`] near it, else one; and a serial that every tap moves on.
fn count_tap(location: Point, time: f64) -> (u64, usize) {
    let last = load_point(&TAPS[0..2]);
    let last_time = f64::from_bits(TAPS[2].load(Ordering::Relaxed) as u64);
    let near = (location.x - last.x).abs() <= TAP_SLOP && (location.y - last.y).abs() <= TAP_SLOP;
    let count = match near && time - last_time <= DOUBLE_TAP_SECONDS {
        true => TAPS[3].load(Ordering::Relaxed) as u64 + 1,
        false => 1,
    };
    store_point(&TAPS[0..2], location);
    TAPS[2].store(time.to_bits() as usize, Ordering::Relaxed);
    TAPS[3].store(count as usize, Ordering::Relaxed);
    let serial = TAPS[4].fetch_add(1, Ordering::Relaxed) + 1;
    (count, serial)
}

fn waits_for(gesture: Id, taps: u64) -> bool {
    let requires = recognizer(gesture).requires;
    if requires.is_null() || recognizer(requires).enabled == 0 {
        return false;
    }
    kind_of(requires) == Kind::Tap && recognizer(requires).taps_required > taps
}

extern "C" fn fire_deferred(slot: usize) {
    let Some(pair) = DEFERRED.get(slot) else {
        return;
    };
    let gesture = pair[0].swap(0, Ordering::AcqRel) as Id;
    let serial = pair[1].swap(0, Ordering::AcqRel);
    if gesture.is_null() || TAPS[4].load(Ordering::Relaxed) != serial {
        return;
    }
    let fields = recognizer(gesture);
    fire(gesture, fields, STATE_ENDED);
    fields.state = STATE_POSSIBLE;
}

fn defer(gesture: Id, serial: usize) {
    for (slot, pair) in DEFERRED.iter().enumerate() {
        if pair[0]
            .compare_exchange(0, gesture as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            pair[1].store(serial, Ordering::Release);
            crate::libsystem::dispatch::after_ms(DOUBLE_TAP_MS, slot, fire_deferred);
            return;
        }
    }
}

fn released(view: Id, location: Point, time: f64) {
    let (start, began) = press();
    let near = |a: f64, b: f64| (a - b).abs() <= TAP_SLOP;
    let still = near(location.x, start.x) && near(location.y, start.y);
    let held = time - began >= LONG_PRESS_SECONDS;
    let swiped = swipe_direction(start, location, time - began);
    let (taps, serial) = match still && !held {
        true => count_tap(location, time),
        false => (0, 0),
    };
    let mut fired = [NO_OBJECT; MOST_RECOGNIZERS];
    let mut fired_count = 0;
    each_recognizer(view, |gesture, fields, kind| {
        let tapped = kind == Kind::Tap && still && !held && taps == fields.taps_required.max(1);
        if tapped && fired_count < MOST_RECOGNIZERS {
            fired[fired_count] = gesture as usize;
            fired_count += 1;
        }
    });
    let fired = &fired[..fired_count];
    each_recognizer(view, |gesture, fields, kind| {
        fields.location = location;
        fields.previous_time = 0.0;
        let tap_now = kind == Kind::Tap && fired.contains(&(gesture as usize));
        let blocked = !fields.requires.is_null() && fired.contains(&(fields.requires as usize));
        match kind {
            Kind::Tap if tap_now && blocked => {}
            Kind::Tap if tap_now && waits_for(gesture, taps) => defer(gesture, serial),
            Kind::Tap if tap_now => fire(gesture, fields, STATE_ENDED),
            Kind::LongPress if still && held => {
                fire(gesture, fields, STATE_BEGAN);
                fire(gesture, fields, STATE_ENDED);
            }
            Kind::Pan if matches!(fields.state, STATE_BEGAN | STATE_CHANGED) => {
                fire(gesture, fields, STATE_ENDED)
            }
            Kind::Swipe if swiped & swipe_mask(fields.direction) != 0 => {
                fire(gesture, fields, STATE_ENDED)
            }
            _ => {}
        }
        fields.state = STATE_POSSIBLE;
    });
}

fn swipe_direction(start: Point, end: Point, seconds: f64) -> u64 {
    let (across, along) = (end.x - start.x, end.y - start.y);
    if seconds > SWIPE_SECONDS {
        return 0;
    }
    match across.abs() >= along.abs() {
        true if across >= SWIPE_POINTS => SWIPE_RIGHT,
        true if across <= -SWIPE_POINTS => SWIPE_LEFT,
        false if along >= SWIPE_POINTS => SWIPE_UP,
        false if along <= -SWIPE_POINTS => SWIPE_DOWN,
        _ => 0,
    }
}

fn swipe_mask(direction: u64) -> u64 {
    match direction {
        0 => SWIPE_RIGHT,
        mask => mask,
    }
}

extern "C" fn swipe_set_direction(this: Id, _: Sel, direction: u64) {
    recognizer(this).direction = direction;
}

extern "C" fn swipe_get_direction(this: Id, _: Sel) -> u64 {
    swipe_mask(recognizer(this).direction)
}

extern "C" fn pan_velocity(this: Id, _: Sel, _view: Id) -> Point {
    recognizer(this).velocity
}

extern "C" fn taps_required(this: Id, _: Sel) -> u64 {
    recognizer(this).taps_required.max(1)
}

extern "C" fn set_taps_required(this: Id, _: Sel, taps: u64) {
    recognizer(this).taps_required = taps.max(1);
}

extern "C" fn touches_required(this: Id, _: Sel) -> u64 {
    recognizer(this).touches_required.max(1)
}

extern "C" fn set_touches_required(this: Id, _: Sel, touches: u64) {
    recognizer(this).touches_required = touches.max(1);
}

extern "C" fn require_to_fail(this: Id, _: Sel, other: Id) {
    recognizer(this).requires = other;
}

extern "C" fn gesture_value(this: Id, _: Sel) -> f64 {
    recognizer(this).value
}

extern "C" fn set_gesture_value(this: Id, _: Sel, value: f64) {
    recognizer(this).value = value;
}

extern "C" fn gesture_rate(this: Id, _: Sel) -> f64 {
    recognizer(this).rate
}

extern "C" fn touch_count(_: Id, _: Sel) -> u64 {
    match FINGER[2].load(Ordering::Relaxed) {
        0 => 1,
        _ => 2,
    }
}

/// Moves pinch or rotation recognizers of `view` to `value` at `time` (D768): began on the
/// first change, changed after, with the change's rate.
fn two_finger(view: Id, wanted: Kind, value: f64, time: f64, ending: bool) {
    each_recognizer(view, |gesture, fields, kind| {
        if kind != wanted {
            return;
        }
        if ending {
            if matches!(fields.state, STATE_BEGAN | STATE_CHANGED) {
                fire(gesture, fields, STATE_ENDED);
            }
            fields.state = STATE_POSSIBLE;
            fields.previous_time = 0.0;
            return;
        }
        let began = !matches!(fields.state, STATE_BEGAN | STATE_CHANGED);
        if began {
            fields.start_value = value;
            fields.previous_time = time;
        }
        let relative = match kind {
            Kind::Pinch => value / fields.start_value.max(f64::MIN_POSITIVE),
            _ => value - fields.start_value,
        };
        let elapsed = time - fields.previous_time;
        if elapsed > 0.0 {
            fields.rate = (relative - fields.value) / elapsed;
        }
        fields.previous_time = time;
        fields.value = relative;
        let state = if began { STATE_BEGAN } else { STATE_CHANGED };
        fire(gesture, fields, state);
    });
}

fn view_under(window: Id, location: Point) -> Id {
    let content = send(window, sel!("contentView"));
    match content.is_null() {
        true => content,
        false => crate::appkit::view::hit_test(content, location).unwrap_or(content),
    }
}

/// A second finger touched, moved or lifted in `window` at `location` (D768): its `UITouch`
/// for the view under it, and the rotation between the two fingers.
pub fn second_finger(window: Id, location: Point, touch: TouchPhase, time: f64) {
    let phase = match touch {
        TouchPhase::Down => phase::BEGAN,
        TouchPhase::Move => phase::MOVED,
        TouchPhase::Up => phase::ENDED,
    };
    let view = view_under(window, location);
    store_point(&FINGER[0..2], location);
    FINGER[2].store(usize::from(phase != phase::ENDED), Ordering::Relaxed);
    let selector = match phase {
        phase::BEGAN => sel!("touchesBegan:withEvent:"),
        phase::MOVED => sel!("touchesMoved:withEvent:"),
        _ => sel!("touchesEnded:withEvent:"),
    };
    deliver_at(view, location, phase, selector);
    let primary = load_point(&PRIMARY);
    let angle = eggos_math::atan2(location.y - primary.y, location.x - primary.x);
    two_finger(view, Kind::Rotation, -angle, time, phase == phase::ENDED);
    if phase == phase::ENDED {
        two_finger(view, Kind::Pinch, 1.0, time, true);
    }
}

/// The window server reported two fingers `scale` thousandths as far apart as when they
/// started, around `location` (D768).
pub fn pinched(window: Id, location: Point, scale: u32, time: f64) {
    let view = view_under(window, location);
    two_finger(view, Kind::Pinch, f64::from(scale) / 1000.0, time, false);
}

/// Edge insets as UIKit lays them out: top, left, bottom, right.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

extern "C" fn safe_area_insets(this: Id, _: Sel) -> EdgeInsets {
    let window = send(this, sel!("window"));
    let at_edge = |edge: f64| match window.is_null() {
        true => 0.0,
        false => edge,
    };
    EdgeInsets {
        top: at_edge(SAFE_TOP),
        left: 0.0,
        bottom: at_edge(SAFE_BOTTOM),
        right: 0.0,
    }
}

extern "C" fn screen_scale(_: Id, _: Sel) -> f64 {
    PHONE_SCALE
}

extern "C" fn screen_native_bounds(_: Id, _: Sel) -> Frame {
    Frame::new(
        0.0,
        0.0,
        PHONE_FRAME.width * PHONE_SCALE,
        PHONE_FRAME.height * PHONE_SCALE,
    )
}

extern "C" fn trait_collection(_: Id, _: Sel) -> Id {
    let made = new_instance((&raw const UITRAITCOLLECTION).cast_mut());
    crate::foundation::autoreleased(made)
}

extern "C" fn horizontal_size_class(_: Id, _: Sel) -> i64 {
    match PHONE_FRAME.width < PHONE_FRAME.height {
        true => SIZE_CLASS_COMPACT,
        false => SIZE_CLASS_REGULAR,
    }
}

extern "C" fn vertical_size_class(_: Id, _: Sel) -> i64 {
    match PHONE_FRAME.width < PHONE_FRAME.height {
        true => SIZE_CLASS_REGULAR,
        false => SIZE_CLASS_COMPACT,
    }
}

extern "C" fn interface_idiom(_: Id, _: Sel) -> i64 {
    IDIOM_PHONE
}

extern "C" fn pan_translation(this: Id, _: Sel, _view: Id) -> Point {
    let fields = recognizer(this);
    Point::new(
        fields.location.x - fields.start.x,
        fields.location.y - fields.start.y,
    )
}

extern "C" fn minimum_press_duration(_: Id, _: Sel) -> f64 {
    LONG_PRESS_SECONDS
}

extern "C" fn ignore_duration(_: Id, _: Sel, _: f64) {}

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
    convert(
        view,
        sel!("convertPoint:fromView:"),
        fields.location,
        core::ptr::null_mut(),
    )
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
        "numberOfTouches" => touch_count,
        "requireGestureRecognizerToFail:" => require_to_fail,
    ];
    let tap = methods![
        "numberOfTapsRequired" => taps_required,
        "setNumberOfTapsRequired:" => set_taps_required,
        "numberOfTouchesRequired" => touches_required,
        "setNumberOfTouchesRequired:" => set_touches_required,
    ];
    let pinch = methods![
        "scale" => gesture_value,
        "setScale:" => set_gesture_value,
        "velocity" => gesture_rate,
    ];
    let rotation = methods![
        "rotation" => gesture_value,
        "setRotation:" => set_gesture_value,
        "velocity" => gesture_rate,
    ];
    let traits = methods![
        "horizontalSizeClass" => horizontal_size_class,
        "verticalSizeClass" => vertical_size_class,
        "userInterfaceIdiom" => interface_idiom,
        "displayScale" => screen_scale,
    ];
    let long_press = methods![
        "minimumPressDuration" => minimum_press_duration,
        "setMinimumPressDuration:" => ignore_duration,
    ];
    let pan = methods![
        "translationInView:" => pan_translation,
        "velocityInView:" => pan_velocity,
    ];
    let swipe = methods![
        "direction" => swipe_get_direction,
        "setDirection:" => swipe_set_direction,
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
    ) && define(
        runtime,
        Definition {
            name: b"UILongPressGestureRecognizer\0",
            class: &UILONGPRESSGESTURERECOGNIZER,
            meta: &UILONGPRESSGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &long_press,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UIPanGestureRecognizer\0",
            class: &UIPANGESTURERECOGNIZER,
            meta: &UIPANGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &pan,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UISwipeGestureRecognizer\0",
            class: &UISWIPEGESTURERECOGNIZER,
            meta: &UISWIPEGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &swipe,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UIPinchGestureRecognizer\0",
            class: &UIPINCHGESTURERECOGNIZER,
            meta: &UIPINCHGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &pinch,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UIRotationGestureRecognizer\0",
            class: &UIROTATIONGESTURERECOGNIZER,
            meta: &UIROTATIONGESTURERECOGNIZER_META,
            superclass: &UIGESTURERECOGNIZER,
            instance_size: core::mem::size_of::<RecognizerFields>(),
            flags: 0,
            instance_methods: &rotation,
            class_methods: &[],
        },
    ) && define(
        runtime,
        Definition {
            name: b"UITraitCollection\0",
            class: &UITRAITCOLLECTION,
            meta: &UITRAITCOLLECTION_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<usize>(),
            flags: 0,
            instance_methods: &traits,
            class_methods: &[],
        },
    )
}

fn deliver(view: Id, event: Id, phase: u64, selector: Sel) {
    if event.is_null() {
        return;
    }
    let location: extern "C" fn(Id, Sel) -> Point = message::msg_send_as();
    deliver_with(view, location(event, sel!("locationInWindow")), phase, selector, event);
}

fn deliver_at(view: Id, location: Point, phase: u64, selector: Sel) {
    deliver_with(view, location, phase, selector, core::ptr::null_mut());
}

fn deliver_with(view: Id, location: Point, phase: u64, selector: Sel, event: Id) {
    if view.is_null() || !send_bool(view, sel!("respondsToSelector:"), selector as usize) {
        return;
    }
    let touch_object = new_instance((&raw const UITOUCH).cast_mut());
    if touch_object.is_null() {
        return;
    }
    let fields = touch(touch_object);
    fields.location = location;
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

fn place_and_time(event: Id) -> (Point, f64) {
    let location: extern "C" fn(Id, Sel) -> Point = message::msg_send_as();
    let time: extern "C" fn(Id, Sel) -> f64 = message::msg_send_as();
    (
        location(event, sel!("locationInWindow")),
        time(event, sel!("timestamp")),
    )
}

/// A view's `mouseDown:` (D501): `touchesBegan:withEvent:` when its class answers it.
pub extern "C" fn touches_began(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::BEGAN, sel!("touchesBegan:withEvent:"));
    if !event.is_null() {
        let (location, time) = place_and_time(event);
        press_started(location, time);
    }
}

/// A view's `mouseDragged:`: `touchesMoved:withEvent:`, then its pan recognizers.
pub extern "C" fn touches_moved(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::MOVED, sel!("touchesMoved:withEvent:"));
    if !this.is_null() && !event.is_null() {
        let (location, time) = place_and_time(event);
        moved(this, location, time);
    }
}

/// A view's `mouseUp:`: `touchesEnded:withEvent:`, then its gesture recognizers.
pub extern "C" fn touches_ended(this: Id, _: Sel, event: Id) {
    deliver(this, event, phase::ENDED, sel!("touchesEnded:withEvent:"));
    if !this.is_null() && !event.is_null() {
        let (location, time) = place_and_time(event);
        released(this, location, time);
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
        out.get_mut(written..written + bytes.len())?
            .copy_from_slice(bytes);
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
    for step in [
        sel!("sceneWillEnterForeground:"),
        sel!("sceneDidBecomeActive:"),
    ] {
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
    fn a_quick_long_move_along_one_axis_is_a_swipe_that_way() {
        let start = Point::new(100.0, 100.0);
        assert_eq!(
            swipe_direction(start, Point::new(180.0, 110.0), 0.2),
            SWIPE_RIGHT
        );
        assert_eq!(
            swipe_direction(start, Point::new(100.0, 30.0), 0.2),
            SWIPE_DOWN
        );
        assert_eq!(swipe_direction(start, Point::new(180.0, 110.0), 0.9), 0);
        assert_eq!(swipe_direction(start, Point::new(120.0, 110.0), 0.2), 0);
    }

    #[test]
    fn swift_classes_are_found_by_their_mangled_names() {
        let mut out = [0u8; LONGEST_CLASS_NAME];
        let length = swift_class_name(b"MyApp.SceneDelegate", &mut out).unwrap();
        assert_eq!(&out[..length], b"_TtC5MyApp13SceneDelegate");
        assert_eq!(swift_class_name(b"SceneDelegate", &mut out), None);
        assert_eq!(swift_class_name(b"A.B.C", &mut out), None);
    }
}
