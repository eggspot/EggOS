//! Keyboard accelerators (D178, `docs/ux/windows-controls-v0.3.md` section 12):
//! `LoadAcceleratorsW` from the program's `RT_ACCELERATOR` resource, `CreateAcceleratorTableW`
//! from `ACCEL` entries, `DestroyAcceleratorTable`, and `TranslateAcceleratorW`, which turns a
//! matching key message into `WM_COMMAND` (`HIWORD` 1) to the window, unless the window is
//! disabled or the command is a grayed item of its menu bar.

use super::gui::{self, try_with, with};
use super::resource::{self, Name, kind, name_from};
use super::user32::{Msg, read_message};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::accelerators::{ACCEL_SIZE, AcceleratorTable, Held};
use crate::gui::message;
use crate::library::Export;

const FROM_ACCELERATOR: usize = 1;

fn store(table: Option<AcceleratorTable>) -> usize {
    let Some(table) = table else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let stored = try_with(|gui| {
        let extras = gui.extras().ok_or(error::NOT_ENOUGH_MEMORY)?;
        extras
            .accelerators
            .insert(table)
            .ok_or(error::NOT_ENOUGH_MEMORY)
    });
    match stored {
        Ok(handle) => handle,
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn load_accelerators_w(module: usize, name: *const u16) -> usize {
    // SAFETY: `LoadAcceleratorsW`'s contract: a resource id or a NUL-terminated name.
    let name = unsafe { name_from(name as usize) };
    let Some((address, size)) = resource::find(module, Name::Id(kind::ACCELERATOR), name) else {
        return fail(error::RESOURCE_NAME_NOT_FOUND, 0);
    };
    // SAFETY: a resource's data is `size` bytes of the program's mapped image.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) };
    store(AcceleratorTable::from_resource(bytes))
}

unsafe extern "efiapi" fn create_accelerator_table_w(entries: *const u8, count: i32) -> usize {
    let Ok(count) = usize::try_from(count) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if entries.is_null() || count == 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes `count` readable `ACCEL`s of six bytes each.
    let bytes = unsafe { core::slice::from_raw_parts(entries, count * ACCEL_SIZE) };
    store(AcceleratorTable::from_accel(bytes))
}

extern "efiapi" fn destroy_accelerator_table(table: usize) -> Bool {
    let removed = with(|gui| {
        gui.made_extras_mut()
            .and_then(|extras| extras.accelerators.remove(table))
            .is_some()
    });
    match removed {
        Some(true) => TRUE,
        _ => fail(error::INVALID_ACCEL_HANDLE, FALSE),
    }
}

unsafe extern "efiapi" fn translate_accelerator_w(
    window: usize,
    table: usize,
    message: *const Msg,
) -> i32 {
    if message.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller passes a readable `MSG`.
    let key = unsafe { read_message(message) };
    if !matches!(
        key.message,
        message::KEY_DOWN | message::SYS_KEY_DOWN | message::CHAR | message::SYS_CHAR
    ) {
        return FALSE;
    }
    let found = try_with(|gui| {
        let target = gui.window(window)?;
        let (enabled, menu) = (target.enabled, target.menu);
        let state = gui.keyboard_state();
        let held = Held {
            shift: state[0x10] & 0x80 != 0,
            control: state[0x11] & 0x80 != 0,
            alt: state[0x12] & 0x80 != 0 || key.message == message::SYS_KEY_DOWN,
        };
        let accelerators = gui
            .made_extras()
            .and_then(|extras| extras.accelerators.get(table))
            .ok_or(error::INVALID_ACCEL_HANDLE)?;
        let Some(command) = accelerators.command_for(key.message, key.wparam, held) else {
            return Ok(None);
        };
        let grayed = menu != 0 && super::menu::command_grayed(gui, menu, usize::from(command));
        Ok(Some((command, enabled && !grayed)))
    });
    match found {
        Ok(Some((command, true))) => {
            gui::send(
                window,
                message::COMMAND,
                message::word_pair(usize::from(command), FROM_ACCELERATOR),
                0,
            );
            TRUE
        }
        Ok(Some((_, false))) => TRUE,
        Ok(None) => FALSE,
        Err(code) => fail(code, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"LoadAcceleratorsW" => load_accelerators_w,
    b"CreateAcceleratorTableW" => create_accelerator_table_w,
    b"DestroyAcceleratorTable" => destroy_accelerator_table,
    b"TranslateAcceleratorW" => translate_accelerator_w,
];
