//! The keyboard layout and hot keys (D178): `ToUnicodeEx`, `VkKeyScanExW` and
//! `MapVirtualKeyW`'s character mapping answer for the layout EggOS types with, read from
//! `/proc/sys/eggos/keyboard_layout` at each call, its dead keys and AltGr (Ctrl+Alt) included
//! (`gui::layout_keyboard`, D221); `GetKeyboardLayout`, `GetKeyboardLayoutNameW` and
//! `ActivateKeyboardLayout` still name the US English layout; `SetKeyboardState` sets the keys
//! held. `RegisterHotKey`'s hot keys work while one of the program's windows has the
//! keyboard: the window server gives a client no keys meant for other apps.

use super::gui::{HotKey, try_with, with};
use super::{Bool, FALSE, TRUE, error, fail, thread_environment};
use crate::gui::input::KEY_STATES;
use crate::gui::keyboard::{self, LAYOUT, LAYOUT_NAME, Modifiers};
use crate::gui::layout_keyboard::{self, Translated};
use crate::library::Export;
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_drivers::input::layout::Typist;

const HKL_PREV: usize = 0;
const HKL_NEXT: usize = 1;
const HELD: u8 = 0x80;
const TOGGLED: u8 = 0x01;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;
const MOD_KEYS: u32 = 0xf;
const KEEP_KEYBOARD_STATE: u32 = 0x4;
const LAYOUT_CONTROL: &[u8] = b"/proc/sys/eggos/keyboard_layout\0";
const LAYOUT_NAME_CAPACITY: usize = 32;

static TYPIST: AtomicU32 = AtomicU32::new(0);

fn layout_in_force() -> usize {
    let Ok(fd) = crate::linux::open(LAYOUT_CONTROL, crate::linux::O_RDONLY) else {
        return 0;
    };
    let mut name = [0u8; LAYOUT_NAME_CAPACITY];
    let length = crate::linux::read(fd, &mut name).unwrap_or(0);
    let _ = crate::linux::close(fd);
    let name = &name[..length.min(LAYOUT_NAME_CAPACITY)];
    let name = name.strip_suffix(b"\n").unwrap_or(name);
    eggos_settings_file::keyboard_layout_named(name).unwrap_or(0)
}

fn typist() -> Typist {
    let mut typist = Typist::from_bits(TYPIST.load(Ordering::Relaxed));
    let layout = layout_in_force();
    if typist.layout_number() != layout {
        typist.set_layout(layout);
    }
    typist
}

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
        unsafe { out.wrapping_add(index).write(unit) };
    }
    TRUE
}

unsafe extern "efiapi" fn to_unicode_ex(
    virtual_key: u32,
    _scan: u32,
    state: *const u8,
    buffer: *mut u16,
    capacity: i32,
    flags: u32,
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
    let Ok(key) = u8::try_from(virtual_key) else {
        return 0;
    };
    let mut typist = typist();
    let translated = layout_keyboard::to_unicode(&mut typist, key, modifiers);
    if flags & KEEP_KEYBOARD_STATE == 0 {
        TYPIST.store(typist.to_bits(), Ordering::Relaxed);
    }
    let (units, count, result) = match translated {
        Translated::Nothing => return 0,
        Translated::Dead(accent) => ([accent, 0, 0, 0], 1, -1),
        Translated::Typed { units, count } => (units, usize::from(count), i32::from(count)),
    };
    if buffer.is_null() || capacity < 1 {
        return 0;
    }
    let room = capacity as usize;
    for (index, &unit) in units[..count.min(room)].iter().enumerate() {
        // SAFETY: the caller passes `capacity` writable units, and `index` is below it.
        unsafe { buffer.wrapping_add(index).write(unit) };
    }
    if count < room {
        // SAFETY: as above, the unit after the characters.
        unsafe { buffer.wrapping_add(count).write(0) };
    }
    result
}

extern "efiapi" fn vk_key_scan_ex_w(character: u16, _layout: usize) -> i16 {
    layout_keyboard::key_for(&typist(), character).map_or(-1, |found| found as i16)
}

fn map_virtual_key(code: u32, kind: u32) -> u32 {
    match (kind, u8::try_from(code)) {
        (keyboard::mapping::KEY_TO_CHARACTER, Ok(key)) => {
            layout_keyboard::key_character(&typist(), key)
        }
        _ => keyboard::map_virtual_key(code, kind),
    }
}

extern "efiapi" fn map_virtual_key_w(code: u32, kind: u32) -> u32 {
    map_virtual_key(code, kind)
}

extern "efiapi" fn map_virtual_key_ex_w(code: u32, kind: u32, _layout: usize) -> u32 {
    map_virtual_key(code, kind)
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

const MOD_ALT: u32 = 0x1;
const MOD_CONTROL: u32 = 0x2;
const MOD_SHIFT: u32 = 0x4;
const MOD_WIN: u32 = 0x8;
const KEYMAP_SHIFT: u8 = 1;
const KEYMAP_ALT: u8 = 2;
const KEYMAP_CTRL: u8 = 4;
const KEYMAP_SUPER: u8 = 8;

fn linux_code_of(virtual_key: u32) -> Option<u16> {
    (1..=255u16).find(|&code| {
        crate::gui::input::virtual_key(code).is_some_and(|found| u32::from(found.virtual_key) == virtual_key)
    })
}

fn keymap_modifiers(modifiers: u32) -> u8 {
    [(MOD_SHIFT, KEYMAP_SHIFT), (MOD_ALT, KEYMAP_ALT), (MOD_CONTROL, KEYMAP_CTRL), (MOD_WIN, KEYMAP_SUPER)]
        .into_iter()
        .filter(|(flag, _)| modifiers & flag != 0)
        .fold(0, |held, (_, bit)| held | bit)
}

fn server_of(window: usize) -> Option<(super::window_device::Device, u32)> {
    with(|gui| {
        let device = gui.device?;
        let root = crate::gui::windows::root_of(&gui.windows, window)?;
        let server = gui.windows.get(root)?.server;
        (server != 0).then_some((device, server))
    })
    .flatten()
}

fn tell_server(window: usize, id: i32, modifiers: u32, key: u32, remove: bool) {
    if window == 0 {
        return;
    }
    let code = match remove {
        true => Some(0),
        false => linux_code_of(key),
    };
    let (Some((device, server)), Some(code)) = (server_of(window), code) else {
        return;
    };
    let _ = device.register_hot_key(server, id as u32, code, keymap_modifiers(modifiers), remove);
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
        Ok(()) => {
            tell_server(window, id, modifiers, key, false);
            TRUE
        }
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
        Some(()) => {
            tell_server(window, id, 0, 0, true);
            TRUE
        }
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
