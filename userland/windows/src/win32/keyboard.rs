//! The keyboard layout and hot keys (D178): the layer's keyboard is the US English layout
//! (`gui::keyboard`), so `GetKeyboardLayout`, `GetKeyboardLayoutNameW`, `ActivateKeyboardLayout`,
//! `ToUnicodeEx`, `VkKeyScanExW` and `MapVirtualKeyW` answer for it; `SetKeyboardState` sets the
//! keys held. `RegisterHotKey`'s hot keys work while one of the program's windows has the
//! keyboard: the window server gives a client no keys meant for other apps.

use super::gui::{HotKey, try_with, with};
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::gui::input::KEY_STATES;
use crate::gui::keyboard::{self, LAYOUT, LAYOUT_NAME, Modifiers};
use crate::library::Export;

const HKL_PREV: usize = 0;
const HKL_NEXT: usize = 1;
const HELD: u8 = 0x80;
const TOGGLED: u8 = 0x01;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;
const MOD_KEYS: u32 = 0xf;

extern "efiapi" fn get_keyboard_layout(_thread: u32) -> usize {
    LAYOUT
}

extern "efiapi" fn activate_keyboard_layout(layout: usize, _flags: u32) -> usize {
    match layout {
        LAYOUT | HKL_PREV | HKL_NEXT => LAYOUT,
        _ => fail(error::INVALID_PARAMETER, 0),
    }
}

unsafe extern "efiapi" fn get_keyboard_layout_name_w(out: *mut u16) -> Bool {
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    for (index, unit) in LAYOUT_NAME.encode_utf16().chain([0]).enumerate() {
        // SAFETY: `GetKeyboardLayoutNameW`'s contract: room for `KL_NAMELENGTH` (9) units.
        unsafe { out.add(index).write(unit) };
    }
    TRUE
}

unsafe extern "efiapi" fn to_unicode_ex(
    virtual_key: u32,
    _scan: u32,
    state: *const u8,
    buffer: *mut u16,
    capacity: i32,
    _flags: u32,
    _layout: usize,
) -> i32 {
    if state.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `ToUnicodeEx`'s contract: a readable array of 256 key states.
    let state = unsafe { core::slice::from_raw_parts(state, KEY_STATES) };
    let held = |key: usize| state[key] & HELD != 0;
    let modifiers = Modifiers {
        shift: held(VK_SHIFT),
        control: held(VK_CONTROL),
        alt: held(VK_MENU),
        caps_lock: state[VK_CAPITAL] & TOGGLED != 0,
    };
    let Some(unit) = u8::try_from(virtual_key)
        .ok()
        .and_then(|key| keyboard::character(key, modifiers))
    else {
        return 0;
    };
    if buffer.is_null() || capacity < 1 {
        return 0;
    }
    // SAFETY: the caller passes `capacity` writable units, at least one.
    unsafe { buffer.write(unit) };
    if capacity > 1 {
        // SAFETY: as above, a second unit.
        unsafe { buffer.add(1).write(0) };
    }
    1
}

extern "efiapi" fn vk_key_scan_ex_w(character: u16, _layout: usize) -> i16 {
    keyboard::key_for(character).map_or(-1, |found| found as i16)
}

extern "efiapi" fn map_virtual_key_w(code: u32, kind: u32) -> u32 {
    keyboard::map_virtual_key(code, kind)
}

extern "efiapi" fn map_virtual_key_ex_w(code: u32, kind: u32, _layout: usize) -> u32 {
    keyboard::map_virtual_key(code, kind)
}

unsafe extern "efiapi" fn set_keyboard_state(state: *const u8) -> Bool {
    if state.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `SetKeyboardState`'s contract: a readable array of 256 key states.
    let state = unsafe { &*state.cast::<[u8; KEY_STATES]>() };
    match with(|gui| gui.input.set_keyboard_state(state)) {
        Some(()) => TRUE,
        None => fail(error::NOT_ENOUGH_MEMORY, FALSE),
    }
}

extern "efiapi" fn register_hot_key(window: usize, id: i32, modifiers: u32, key: u32) -> Bool {
    let registered = try_with(|gui| {
        let thread = match window {
            0 => thread_environment::thread_id(),
            window => gui.window(window)?.thread,
        };
        let extras = gui.extras().ok_or(error::NOT_ENOUGH_MEMORY)?;
        let taken = extras.hot_keys.iter().flatten().any(|hot_key| {
            hot_key.key == key && hot_key.modifiers & MOD_KEYS == modifiers & MOD_KEYS
        });
        if taken {
            return Err(error::HOTKEY_ALREADY_REGISTERED);
        }
        let slot = extras
            .hot_keys
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(error::NOT_ENOUGH_MEMORY)?;
        *slot = Some(HotKey {
            window,
            thread,
            id,
            modifiers,
            key,
        });
        Ok(())
    });
    match registered {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn unregister_hot_key(window: usize, id: i32) -> Bool {
    let thread = thread_environment::thread_id();
    let removed = with(|gui| {
        let slot = gui.made_extras_mut()?.hot_keys.iter_mut().find(|slot| {
            slot.is_some_and(|hot_key| {
                hot_key.window == window
                    && hot_key.id == id
                    && (window != 0 || hot_key.thread == thread)
            })
        })?;
        *slot = None;
        Some(())
    })
    .flatten();
    match removed {
        Some(()) => TRUE,
        None => fail(error::HOTKEY_NOT_REGISTERED, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetKeyboardLayout" => get_keyboard_layout,
    b"ActivateKeyboardLayout" => activate_keyboard_layout,
    b"GetKeyboardLayoutNameW" => get_keyboard_layout_name_w,
    b"ToUnicodeEx" => to_unicode_ex,
    b"VkKeyScanExW" => vk_key_scan_ex_w,
    b"MapVirtualKeyW" => map_virtual_key_w,
    b"MapVirtualKeyExW" => map_virtual_key_ex_w,
    b"SetKeyboardState" => set_keyboard_state,
    b"RegisterHotKey" => register_hot_key,
    b"UnregisterHotKey" => unregister_hot_key,
];
