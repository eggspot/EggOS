//! NSEvent: a mouse or key event made from one of the window server's records, with AppKit's
//! type, location in the window, modifier flags, characters and key code
//! (`eggos_macos_gui::keys`). The Linux key code and character stay with it for the layer's
//! own controls, which edit text as the toolkit does.

use super::{NSEVENT, NSEVENT_META, string};
use crate::foundation::{autoreleased, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods};
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_macos_gui::geometry::Point;
use eggos_macos_gui::keys;

/// `NSEventType`s the layer makes.
pub mod kind {
    pub const LEFT_MOUSE_DOWN: usize = 1;
    pub const LEFT_MOUSE_UP: usize = 2;
    pub const RIGHT_MOUSE_DOWN: usize = 3;
    pub const RIGHT_MOUSE_UP: usize = 4;
    pub const MOUSE_MOVED: usize = 5;
    pub const LEFT_MOUSE_DRAGGED: usize = 6;
    pub const KEY_DOWN: usize = 10;
    pub const KEY_UP: usize = 11;
    pub const FLAGS_CHANGED: usize = 12;
    pub const SCROLL_WHEEL: usize = 22;
}

static MODIFIER_FLAGS: AtomicU64 = AtomicU64::new(0);

/// An event's fields.
#[repr(C)]
#[derive(Debug)]
pub struct EventFields {
    isa: usize,
    pub kind: usize,
    pub location: Point,
    pub flags: u64,
    pub window: Id,
    pub characters: Id,
    pub ignoring: Id,
    pub key_code: u16,
    pub linux_code: u16,
    pub character: u32,
    pub button: isize,
    pub delta: f64,
    pub time_ms: u32,
}

/// The fields of `event`.
pub fn fields(event: Id) -> &'static mut EventFields {
    // SAFETY: the callers pass NSEvent instances, which the layer makes.
    unsafe { &mut *event.cast::<EventFields>() }
}

fn new_event(kind: usize, window: Id, location: Point, time_ms: u32) -> Id {
    let event = new_instance(class_pointer(&NSEVENT));
    if event.is_null() {
        return event;
    }
    let fields = fields(event);
    fields.kind = kind;
    fields.window = window;
    fields.location = location;
    fields.flags = MODIFIER_FLAGS.load(Ordering::Relaxed);
    fields.time_ms = time_ms;
    autoreleased(event)
}

/// A new autoreleased mouse event of `kind` at `location` in `window`, button `button`, the
/// wheel's `delta`.
pub fn mouse(
    kind: usize,
    window: Id,
    location: Point,
    button: isize,
    delta: f64,
    time_ms: u32,
) -> Id {
    let event = new_event(kind, window, location, time_ms);
    if !event.is_null() {
        fields(event).button = button;
        fields(event).delta = delta;
    }
    event
}

fn characters_string(character: Option<u32>) -> Id {
    let mut encoded = [0u8; 4];
    let text = match character.and_then(char::from_u32) {
        Some(character) => &*character.encode_utf8(&mut encoded),
        None => "",
    };
    objc_retain(string(text))
}

/// Whether Linux key `code` is a modifier key, which makes `flagsChanged:` events.
pub fn is_modifier_key(code: u16) -> bool {
    matches!(code, 29 | 42 | 54 | 56 | 58 | 97 | 100 | 125 | 126)
}

/// A new autoreleased key event for Linux key `code` going down (`pressed`) or up in `window`,
/// with the record's modifier bits and character.
pub fn key(window: Id, code: u16, pressed: bool, modifiers: u32, character: u32, time_ms: u32) -> Id {
    let flags = keys::modifier_flags(modifiers);
    MODIFIER_FLAGS.store(flags, Ordering::Relaxed);
    let kind = match (is_modifier_key(code), pressed) {
        (true, _) => kind::FLAGS_CHANGED,
        (false, true) => kind::KEY_DOWN,
        (false, false) => kind::KEY_UP,
    };
    let event = new_event(kind, window, Point::default(), time_ms);
    if event.is_null() {
        return event;
    }
    let fields = fields(event);
    fields.flags = flags;
    fields.key_code = keys::key_code(code).unwrap_or(0xffff);
    fields.linux_code = code;
    fields.character = character;
    fields.characters = characters_string(keys::characters(code, character));
    fields.ignoring = characters_string(keys::characters_ignoring_modifiers(code, character, flags));
    event
}

/// The first character of `event`'s `charactersIgnoringModifiers`, 0 for none.
pub fn typed(event: Id) -> u32 {
    let fields = fields(event);
    keys::characters_ignoring_modifiers(fields.linux_code, fields.character, fields.flags)
        .unwrap_or(0)
}

extern "C" fn event_type(this: Id, _: Sel) -> usize {
    fields(this).kind
}

extern "C" fn location_in_window(this: Id, _: Sel) -> Point {
    fields(this).location
}

extern "C" fn modifier_flags(this: Id, _: Sel) -> u64 {
    fields(this).flags
}

extern "C" fn characters(this: Id, _: Sel) -> Id {
    match fields(this).characters {
        none if none.is_null() => string(""),
        characters => characters,
    }
}

extern "C" fn characters_ignoring_modifiers(this: Id, _: Sel) -> Id {
    match fields(this).ignoring {
        none if none.is_null() => string(""),
        characters => characters,
    }
}

extern "C" fn key_code(this: Id, _: Sel) -> usize {
    usize::from(fields(this).key_code)
}

extern "C" fn click_count(this: Id, _: Sel) -> isize {
    match fields(this).kind {
        kind::LEFT_MOUSE_DOWN | kind::LEFT_MOUSE_UP | kind::RIGHT_MOUSE_DOWN
        | kind::RIGHT_MOUSE_UP => 1,
        _ => 0,
    }
}

extern "C" fn button_number(this: Id, _: Sel) -> isize {
    fields(this).button
}

extern "C" fn window(this: Id, _: Sel) -> Id {
    fields(this).window
}

extern "C" fn timestamp(this: Id, _: Sel) -> f64 {
    f64::from(fields(this).time_ms) / 1000.0
}

extern "C" fn delta(this: Id, _: Sel) -> f64 {
    fields(this).delta
}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn current_modifier_flags(_: Id, _: Sel) -> u64 {
    MODIFIER_FLAGS.load(Ordering::Relaxed)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    for string in [fields.characters, fields.ignoring] {
        if !string.is_null() {
            objc_release(string);
        }
    }
    dispose(this);
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "type" => event_type,
        "locationInWindow" => location_in_window,
        "modifierFlags" => modifier_flags,
        "characters" => characters,
        "charactersIgnoringModifiers" => characters_ignoring_modifiers,
        "keyCode" => key_code,
        "clickCount" => click_count,
        "buttonNumber" => button_number,
        "window" => window,
        "timestamp" => timestamp,
        "deltaY" => delta,
        "scrollingDeltaY" => delta,
        "isARepeat" => no,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "modifierFlags" => current_modifier_flags,
    ];
    super::define(
        runtime,
        b"NSEvent\0",
        (&NSEVENT, &NSEVENT_META),
        &NSOBJECT,
        core::mem::size_of::<EventFields>(),
        &instance,
        &class,
    )
}
