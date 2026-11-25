//! Carbon's global hot keys (D754): `RegisterEventHotKey` registers a key and its modifiers
//! with the window server's global hot keys (D398) on one of the app's windows, waiting for
//! the first window when the app has none; `InstallEventHandler` keeps the handlers an app
//! installs on the application (or dispatcher) target, and a pressed hot key reaches every
//! handler for `kEventClassKeyboard` / `kEventHotKeyPressed` as an event whose direct object
//! is the hot key's `EventHotKeyID`, read with `GetEventParameter`.

use super::server;
use crate::libsystem::Exclusive;
use eggos_macos_gui::keys::{linux_code, server_modifiers_of_carbon};

const NO_ERROR: i32 = 0;
const PARAMETER_ERROR: i32 = -50;
const NOT_HANDLED: i32 = -9874;
const PARAMETER_NOT_FOUND: i32 = -9870;
const HOT_KEY_EXISTS: i32 = -9878;
const HOT_KEY_INVALID: i32 = -9879;
const CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const MOST_HOT_KEYS: usize = 16;
const MOST_HANDLERS: usize = 16;
const MOST_TYPES: usize = 8;
const APPLICATION_TARGET: usize = 0x4361_7070;
const DISPATCHER_TARGET: usize = 0x4364_7370;
const HOT_KEY_RECORD: u16 = 23;

type Handler = extern "C" fn(usize, *const HotKeyEvent, usize) -> i32;

#[derive(Debug, Clone, Copy)]
struct HotKey {
    identifier: u64,
    code: u16,
    modifiers: u32,
    window: u32,
}

#[derive(Debug, Clone, Copy)]
struct Installed {
    handler: Handler,
    user_data: usize,
    types: [(u32, u32); MOST_TYPES],
    type_count: usize,
}

/// The event a handler receives (its `EventRef`).
#[repr(C)]
#[derive(Debug)]
pub struct HotKeyEvent {
    class: u32,
    kind: u32,
    identifier: u64,
}

struct Tables {
    hot_keys: [Option<HotKey>; MOST_HOT_KEYS],
    handlers: [Option<Installed>; MOST_HANDLERS],
}

static TABLES: Exclusive<Tables> = Exclusive::new(Tables {
    hot_keys: [None; MOST_HOT_KEYS],
    handlers: [None; MOST_HANDLERS],
});

fn some_window() -> u32 {
    super::application::shown_server_window().unwrap_or(0)
}

fn send_to_server(slot: usize, hot_key: HotKey, remove: bool) {
    if hot_key.window != 0 {
        server::register_hot_key(
            hot_key.window,
            slot as u32 + 1,
            hot_key.code,
            hot_key.modifiers,
            remove,
        );
    }
}

extern "C" fn register_event_hot_key(
    mac_code: u32,
    carbon_modifiers: u32,
    identifier: u64,
    _target: usize,
    _options: u32,
    out: *mut usize,
) -> i32 {
    let Some(code) = u16::try_from(mac_code).ok().and_then(linux_code) else {
        return HOT_KEY_INVALID;
    };
    let modifiers = server_modifiers_of_carbon(carbon_modifiers);
    let window = some_window();
    let placed = TABLES.with(|tables| {
        let taken = tables
            .hot_keys
            .iter()
            .flatten()
            .any(|existing| existing.code == code && existing.modifiers == modifiers);
        if taken {
            return Err(HOT_KEY_EXISTS);
        }
        let slot = tables
            .hot_keys
            .iter()
            .position(Option::is_none)
            .ok_or(PARAMETER_ERROR)?;
        let hot_key = HotKey {
            identifier,
            code,
            modifiers,
            window,
        };
        tables.hot_keys[slot] = Some(hot_key);
        Ok((slot, hot_key))
    });
    match placed {
        Ok((slot, hot_key)) => {
            send_to_server(slot, hot_key, false);
            if !out.is_null() {
                // SAFETY: the program passes room for its `EventHotKeyRef`.
                unsafe { out.write(slot + 1) };
            }
            NO_ERROR
        }
        Err(status) => status,
    }
}

extern "C" fn unregister_event_hot_key(reference: usize) -> i32 {
    let slot = reference.wrapping_sub(1);
    let removed = TABLES.with(|tables| tables.hot_keys.get_mut(slot).and_then(Option::take));
    match removed {
        Some(hot_key) => {
            send_to_server(slot, hot_key, true);
            NO_ERROR
        }
        None => PARAMETER_ERROR,
    }
}

/// Registers the hot keys still waiting for a window on `window`, the app's first one shown.
pub fn window_shown(window: u32) {
    let waiting = TABLES.with(|tables| {
        let mut waiting = [None; MOST_HOT_KEYS];
        for (slot, entry) in tables.hot_keys.iter_mut().enumerate() {
            if let Some(hot_key) = entry
                && hot_key.window == 0
            {
                hot_key.window = window;
                waiting[slot] = Some(*hot_key);
            }
        }
        waiting
    });
    for (slot, hot_key) in waiting.iter().enumerate() {
        if let Some(hot_key) = hot_key {
            send_to_server(slot, *hot_key, false);
        }
    }
}

/// Moves the hot keys registered on `window`, which is going away, to another shown window.
pub fn window_closed(window: u32) {
    let others = some_window();
    let moved = TABLES.with(|tables| {
        let mut moved = [None; MOST_HOT_KEYS];
        for (slot, entry) in tables.hot_keys.iter_mut().enumerate() {
            if let Some(hot_key) = entry
                && hot_key.window == window
            {
                hot_key.window = match others == window {
                    true => 0,
                    false => others,
                };
                moved[slot] = Some(*hot_key);
            }
        }
        moved
    });
    for (slot, hot_key) in moved.iter().enumerate() {
        if let Some(hot_key) = hot_key {
            send_to_server(slot, *hot_key, false);
        }
    }
}

/// Handles a window-server record when it is a hot key of ours: whether it was.
pub fn hot_key_record(record: &[u8]) -> bool {
    if record.len() < 12 || u16::from_ne_bytes([record[0], record[1]]) != HOT_KEY_RECORD {
        return false;
    }
    let number = u32::from_ne_bytes([record[8], record[9], record[10], record[11]]) as usize;
    let found = TABLES.with(|tables| {
        tables
            .hot_keys
            .get(number.wrapping_sub(1))
            .copied()
            .flatten()
    });
    let Some(hot_key) = found else {
        return true;
    };
    let event = HotKeyEvent {
        class: CLASS_KEYBOARD,
        kind: HOT_KEY_PRESSED,
        identifier: hot_key.identifier,
    };
    let handlers = TABLES.with(|tables| tables.handlers);
    for installed in handlers.iter().flatten() {
        let wanted = installed.types[..installed.type_count]
            .iter()
            .any(|&(class, kind)| class == event.class && kind == event.kind);
        if wanted && (installed.handler)(0, &event, installed.user_data) == NO_ERROR {
            break;
        }
    }
    true
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

unsafe extern "C" fn install_event_handler(
    _target: usize,
    handler: Option<Handler>,
    count: u32,
    types: *const EventTypeSpec,
    user_data: usize,
    out: *mut usize,
) -> i32 {
    let Some(handler) = handler else {
        return PARAMETER_ERROR;
    };
    let count = (count as usize).min(MOST_TYPES);
    let mut kept = [(0, 0); MOST_TYPES];
    if count > 0 && !types.is_null() {
        // SAFETY: the program passes `count` type specs.
        let given = unsafe { core::slice::from_raw_parts(types, count) };
        for (slot, spec) in kept.iter_mut().zip(given) {
            *slot = (spec.class, spec.kind);
        }
    }
    let installed = Installed {
        handler,
        user_data,
        types: kept,
        type_count: count,
    };
    let slot = TABLES.with(|tables| {
        let slot = tables.handlers.iter().position(Option::is_none)?;
        tables.handlers[slot] = Some(installed);
        Some(slot)
    });
    match slot {
        Some(slot) => {
            if !out.is_null() {
                // SAFETY: the program passes room for its `EventHandlerRef`, or null.
                unsafe { out.write(slot + 1) };
            }
            NO_ERROR
        }
        None => PARAMETER_ERROR,
    }
}

extern "C" fn remove_event_handler(reference: usize) -> i32 {
    let slot = reference.wrapping_sub(1);
    let removed = TABLES.with(|tables| tables.handlers.get_mut(slot).and_then(Option::take));
    match removed {
        Some(_) => NO_ERROR,
        None => PARAMETER_ERROR,
    }
}

extern "C" fn application_target() -> usize {
    APPLICATION_TARGET
}

extern "C" fn dispatcher_target() -> usize {
    DISPATCHER_TARGET
}

extern "C" fn call_next_handler(_next: usize, _event: *const HotKeyEvent) -> i32 {
    NOT_HANDLED
}

extern "C" fn event_class(event: *const HotKeyEvent) -> u32 {
    match event.is_null() {
        true => 0,
        // SAFETY: an `EventRef` the layer handed a handler, alive while it runs.
        false => unsafe { (*event).class },
    }
}

extern "C" fn event_kind(event: *const HotKeyEvent) -> u32 {
    match event.is_null() {
        true => 0,
        // SAFETY: as above.
        false => unsafe { (*event).kind },
    }
}

unsafe extern "C" fn event_parameter(
    event: *const HotKeyEvent,
    name: u32,
    desired: u32,
    actual_type: *mut u32,
    size: usize,
    actual_size: *mut usize,
    data: *mut u8,
) -> i32 {
    if event.is_null() || name != DIRECT_OBJECT || desired != TYPE_HOT_KEY_ID {
        return PARAMETER_NOT_FOUND;
    }
    // SAFETY: an `EventRef` the layer handed a handler, alive while it runs.
    let identifier = unsafe { (*event).identifier }.to_ne_bytes();
    if !actual_type.is_null() {
        // SAFETY: the program passes room for the type, or null.
        unsafe { actual_type.write(TYPE_HOT_KEY_ID) };
    }
    if !actual_size.is_null() {
        // SAFETY: the program passes room for the size, or null.
        unsafe { actual_size.write(identifier.len()) };
    }
    if !data.is_null() {
        let count = size.min(identifier.len());
        // SAFETY: the program passes `size` writable bytes.
        unsafe { data.copy_from_nonoverlapping(identifier.as_ptr(), count) };
    }
    NO_ERROR
}

/// The address of Carbon function `name`, if it is one of these.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_RegisterEventHotKey" => register_event_hot_key as *const () as usize,
        b"_UnregisterEventHotKey" => unregister_event_hot_key as *const () as usize,
        b"_InstallEventHandler" => install_event_handler as *const () as usize,
        b"_RemoveEventHandler" => remove_event_handler as *const () as usize,
        b"_GetApplicationEventTarget" => application_target as *const () as usize,
        b"_GetEventDispatcherTarget" => dispatcher_target as *const () as usize,
        b"_CallNextEventHandler" => call_next_handler as *const () as usize,
        b"_GetEventClass" => event_class as *const () as usize,
        b"_GetEventKind" => event_kind as *const () as usize,
        b"_GetEventParameter" => event_parameter as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}
