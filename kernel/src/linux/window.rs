//! `/dev/window` for Linux programs (D127, docs/windows.md): opening the device connects a
//! client of the window server ([`crate::window`]); `ioctl` makes and changes its windows,
//! `mmap` maps a window's pixels, `read` takes its event records (waiting for one unless
//! `O_NONBLOCK`), `poll` reports `POLLIN` while one waits, `fstat` reports the character
//! device, and closing the last descriptor destroys the client's windows. `write` is `EINVAL`
//! and `lseek` `ESPIPE`, as on a Linux character device without them.

use super::descriptors::{FileKind, OpenFile};
use super::poll::event;
use super::signals::{self, Interrupted};
use super::terminal::{Waited, wait_for_input};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, iovec};
use crate::process::{LentFrames, Placement, Process};
use crate::regions::RegionError;
use crate::terminal::{
    desktop_profile, open_app, reload_catalogue, send_to_terminal, set_desktop_profile,
    type_in_terminal, with_windows,
};
use crate::window::{
    self, Connection, PixelBuffer, QueryAnswer, QueryRequest, ServerError, copy_content_rows,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use eggos_drivers::desktop::protocol;
use eggos_drivers::desktop::protocol::event as record_kind;
use eggos_drivers::desktop::protocol::{
    ELEMENT_TEXT_CAPACITY, EVENT_SIZE, EventRecord, EventSink, MAX_COMMIT_RECTS, MAX_TREE_BYTES,
    MENU_POOL_CAPACITY, QUERY_ANSWER_MS, Request, TITLE_CAPACITY, WindowError, act_request,
    capture_reply, create_reply, desktop_action, element_action, get_clipboard_reply, get_form_reply,
    get_menu_reply, get_tree_reply, input_outcome, keyboard_flags, keyboard_layout_reply,
    list_desktops_reply, list_reply, placement_reply, put_double_word, put_word, query_kind,
    read_query_reply, request, send_input_reply, set_size_reply, size, tree_answer,
};
use eggos_drivers::desktop::{Desktop, Injected, InjectedInput, ListedWindow, Profile, WindowId};
use eggos_drivers::display::Damage;
use eggos_hal::Platform;
use eggos_hal::paging::Attributes;
use eggos_hal::user::UserContext;

const TRAY_PIXELS: usize = protocol::TRAY_ICON_SIDE * protocol::TRAY_ICON_SIDE;

const READ_RECORDS: usize = 16;

const SCALE: u32 = 1;

fn errno(error: ServerError) -> Errno {
    match error {
        ServerError::Refused(WindowError::Invalid) => Errno::EINVAL,
        ServerError::Refused(WindowError::NotAllowed) => Errno::EPERM,
        ServerError::Refused(WindowError::TableFull) => Errno::ENOSPC,
        ServerError::Refused(WindowError::Unsupported) => Errno::EOPNOTSUPP,
        ServerError::Refused(WindowError::PersonViewOnly) => Errno::EACCES,
        ServerError::Refused(WindowError::NoSuchWindow) => Errno::ENOENT,
        ServerError::Refused(WindowError::Busy) => Errno::EBUSY,
        ServerError::Refused(WindowError::QueueFull) => Errno::EAGAIN,
        ServerError::Refused(WindowError::NotResponding) => Errno::ENOTCONN,
        ServerError::Refused(WindowError::FixedByForm) => Errno::EROFS,
        ServerError::NoServer => Errno::ENXIO,
        ServerError::NoClientSlot => Errno::ENFILE,
        ServerError::OutOfMemory => Errno::ENOMEM,
    }
}

fn refused(error: WindowError) -> Errno {
    errno(ServerError::Refused(error))
}

/// Opens the window server's device for process `process`: a new client. `ENXIO` without a
/// display, `ENFILE` when the server serves as many clients as it may.
pub fn open<P: Platform>(process: u32) -> Result<FileKind, Errno> {
    let connection = window::connect(window::disconnect::<P>, process).map_err(errno)?;
    let named = super::process_table::TABLE
        .lock()
        .list()
        .find(|listed| listed.id == process)
        .map(|listed| listed.name);
    if named.is_some_and(|name| serves_linux_apps(&name)) {
        let client = connection.client().id();
        let _ = with_windows::<P, _>(|desktop, _, _, _| desktop.set_linux_apps_client(client));
    }
    Ok(FileKind::Window(connection))
}

const LINUX_APPS_SERVER: &[u8] = b"eggway";

fn serves_linux_apps(name: &super::process_table::Name) -> bool {
    name.split(|&byte| byte == 0).next() == Some(LINUX_APPS_SERVER)
}

/// `ioctl(fd, request, argument)` on `/dev/window`: the requests of docs/windows.md, their
/// argument read from and written back to `argument`. `ENOTTY` for a request it does not
/// answer, `EFAULT` for an argument outside the program's memory.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    connection: &Connection,
    number: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let number = number as u32;
    let length = Request::argument_size(number).ok_or(Errno::ENOTTY)?;
    let mut bytes = [0u8; size::LARGEST];
    let bytes = &mut bytes[..length];
    if number != request::GET_WORK_AREA
        && number != request::GET_PROFILE
        && number != request::GET_PALETTE
        && number != request::GET_REDUCE_MOTION
        && number != request::GET_FORM
        && number != request::GET_KEYBOARD_LAYOUT
        && number != request::APPS_CHANGED
    {
        process
            .read_user_memory(argument, bytes)
            .map_err(|_| Errno::EFAULT)?;
    }
    let request = Request::parse(number, bytes).ok_or(Errno::ENOTTY)?;
    let client = connection.client().id();
    let manages = matches!(
        request,
        Request::ListWindow { .. }
            | Request::ManageWindow { .. }
            | Request::GetMenu { .. }
            | Request::DesktopAction { .. }
    );
    if manages && !is_privileged(process) {
        return Err(Errno::EPERM);
    }
    let changes_settings = matches!(
        request,
        Request::SetProfile { .. }
            | Request::SetPalette { .. }
            | Request::SetReduceMotion { .. }
            | Request::SetKeyboardLayout { .. }
            | Request::SetKeyboardLayouts { .. }
            | Request::SetInputPreferences { .. }
            | Request::SetKeyAccessibility { .. }
    );
    if changes_settings && !may_change_desktop_settings(process) {
        return Err(Errno::EPERM);
    }
    if matches!(request, Request::OpenApp { .. } | Request::GetLaunch { .. }) && !may_open_apps(process) {
        return Err(Errno::EPERM);
    }
    if matches!(request, Request::TypeInTerminal { .. }) && !may_type_in_terminal(process) {
        return Err(Errno::EPERM);
    }
    let reads = matches!(
        request,
        Request::CaptureWindow { .. } | Request::GetTree { .. }
    );
    if reads && !may_capture_windows(process) {
        return Err(Errno::EPERM);
    }
    let injects = matches!(
        request,
        Request::SendInput { .. } | Request::ChooseMenu { .. } | Request::ActOnElement { .. }
    );
    if injects && !may_send_input(process) {
        return Err(Errno::EPERM);
    }
    match request {
        Request::Create {
            role,
            parent,
            width,
            height,
            flags,
        } => {
            let (window, offset, width, height) =
                create::<P>(connection, role, parent, width, height, flags)?;
            put_word(bytes, create_reply::WIDTH, width);
            put_word(bytes, create_reply::HEIGHT, height);
            put_word(bytes, create_reply::WINDOW, u32::from(window));
            put_double_word(bytes, create_reply::OFFSET, offset);
            if process.write_user_memory(argument, bytes).is_err() {
                on_windows::<P, _>(|desktop, buffers, damage, events| {
                    desktop.destroy(client, u32::from(window), damage, events)?;
                    buffers.release_unused(desktop);
                    Ok(())
                })?;
                return Err(Errno::EFAULT);
            }
        }
        Request::SetTitle {
            window,
            length,
            title,
        } => {
            let title = title.get(..length as usize).ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, _| {
                desktop.set_title(client, window, title, damage)
            })?;
        }
        Request::SetMenu {
            window,
            length,
            pool,
        } => {
            let pool = pool.get(..length as usize).ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, _| {
                desktop.set_menu(client, window, pool, damage)
            })?;
        }
        Request::SetSize {
            window,
            width,
            height,
        } => {
            let (offset, width, height) = set_size::<P>(connection, window, width, height)?;
            put_word(bytes, set_size_reply::WIDTH, width);
            put_word(bytes, set_size_reply::HEIGHT, height);
            put_double_word(bytes, set_size_reply::OFFSET, offset);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetMinimumSize {
            window,
            width,
            height,
        } => on_windows::<P, _>(|desktop, _, _, _| {
            desktop.set_minimum_size(client, window, width, height)
        })?,
        Request::SetPosition { window, x, y } => on_windows::<P, _>(|desktop, _, damage, _| {
            desktop.set_position(client, window, x, y, damage)
        })?,
        Request::SetState { window, state } => on_windows::<P, _>(|desktop, _, damage, events| {
            desktop.set_state(client, window, state, damage, events)
        })?,
        Request::ChangeFlags {
            window,
            mask,
            values,
        } => on_windows::<P, _>(|desktop, _, _, _| {
            desktop.change_flags(client, window, mask, values)
        })?,
        Request::SetPointer { window, shape } => on_windows::<P, _>(|desktop, _, damage, _| {
            desktop.set_pointer(client, window, shape, damage)
        })?,
        Request::Commit {
            window,
            count,
            rects,
        } => {
            let rects = rects
                .get(..count as usize)
                .filter(|_| count as usize <= MAX_COMMIT_RECTS)
                .ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.commit(client, window, rects, damage, events)
            })?;
        }
        Request::StartMoveOrResize {
            window,
            kind,
            edges,
        } => on_windows::<P, _>(|desktop, _, damage, events| {
            desktop.start_move_or_resize(client, window, kind, edges, damage, events)
        })?,
        Request::Destroy { window } => on_windows::<P, _>(|desktop, buffers, damage, events| {
            desktop.destroy(client, window, damage, events)?;
            buffers.release_unused(desktop);
            Ok(())
        })?,
        Request::GetWorkArea => {
            let mut area = None;
            on_windows::<P, _>(|desktop, _, _, _| {
                area = Some(desktop.work_area());
                Ok(())
            })?;
            let area = area.ok_or(Errno::ENXIO)?;
            for (index, value) in [area.x, area.y, area.width, area.height, SCALE]
                .into_iter()
                .enumerate()
            {
                put_word(bytes, index * 4, value);
            }
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ListWindow { index } => {
            let info =
                on_windows::<P, _>(|desktop, _, _, _| desktop.listed_window(index as usize))?;
            bytes.fill(0);
            for (offset, value) in [
                (list_reply::WINDOW, u32::from(info.id)),
                (list_reply::STATE, listed_state(&info)),
                (list_reply::X, info.rect.x as u32),
                (list_reply::Y, info.rect.y),
                (list_reply::WIDTH, info.rect.width),
                (list_reply::HEIGHT, info.rect.height),
                (list_reply::CLIENT, u32::from(info.client)),
                (list_reply::TITLE_LENGTH, info.title_length as u32),
            ] {
                put_word(bytes, offset, value);
            }
            bytes[list_reply::TITLE..list_reply::TITLE + info.title.len()]
                .copy_from_slice(&info.title);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ManageWindow {
            window,
            action,
            a,
            b,
        } => on_windows::<P, _>(|desktop, _, damage, events| {
            desktop.manage(window, action, a, b, damage, events)
        })?,
        Request::SetKeyAccessibility {
            sticky_keys,
            slow_keys,
            slow_keys_ms,
        } => {
            let slow = (slow_keys != 0).then_some(slow_keys_ms);
            let was_on = crate::input::sticky_keys();
            if !crate::input::set_key_accessibility(sticky_keys != 0, slow) {
                return Err(Errno::EINVAL);
            }
            if was_on != (sticky_keys != 0) {
                crate::terminal::sticky_keys_turned::<P>(sticky_keys != 0);
            }
        }
        Request::ListDesktops => {
            let (count, current, windows) =
                on_windows::<P, _>(|desktop, _, _, _| Ok(desktop.desktops()))?;
            bytes[list_desktops_reply::COUNT] = count;
            bytes[list_desktops_reply::CURRENT] = current + 1;
            bytes[list_desktops_reply::WINDOWS..list_desktops_reply::WINDOWS + windows.len()]
                .copy_from_slice(&windows);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::DesktopAction {
            action,
            desktop,
            window,
        } => {
            let report = on_windows::<P, _>(|windows, _, damage, events| {
                windows.desktop_action(action, desktop, window, damage, events)
            })?;
            put_word(bytes, desktop_action::REPORT, report);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::GetProfile => {
            put_word(bytes, 0, desktop_profile().number());
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetProfile { profile } => {
            let profile = Profile::from_number(profile).ok_or(Errno::EINVAL)?;
            set_desktop_profile::<P>(profile);
        }
        Request::GetForm => {
            let (form, setting, inputs) = crate::terminal::form_in_use();
            let form_number = get_form_reply::FORMS
                .iter()
                .position(|each| *each == form)
                .unwrap_or(0) as u32;
            let setting_number = eggos_drivers::desktop::form::FORM_SETTING_NAMES
                .iter()
                .position(|name| *name == setting.name())
                .unwrap_or(0) as u32;
            let mut kinds = 0;
            for (present, bit) in [
                (inputs.touch, get_form_reply::TOUCH),
                (inputs.keyboard, get_form_reply::KEYBOARD),
                (inputs.pointer, get_form_reply::POINTER),
                (inputs.remote, get_form_reply::REMOTE),
            ] {
                if present {
                    kinds |= bit;
                }
            }
            put_word(bytes, get_form_reply::FORM, form_number);
            put_word(bytes, get_form_reply::SETTING, setting_number);
            put_word(bytes, get_form_reply::INPUTS, kinds);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetForm { setting } => {
            if !crate::terminal::set_form_setting::<P>(setting) {
                return Err(Errno::EINVAL);
            }
        }
        Request::SetLayered {
            window,
            alpha,
            key,
            flags,
        } => {
            let alpha = (flags & protocol::layered_flags::ALPHA != 0).then(|| alpha.min(255) as u8);
            let key = (flags & protocol::layered_flags::COLOUR_KEY != 0).then_some(key);
            let pixel_alpha = flags & protocol::layered_flags::PIXEL_ALPHA != 0;
            on_windows::<P, _>(|desktop, _, damage, _| {
                desktop.set_layered(client, window, (alpha, key, pixel_alpha), damage)
            })?;
        }
        Request::StartDrag {
            window,
            kind,
            length,
            payload,
            text_length,
            text,
        } => {
            let length = usize::try_from(length)
                .ok()
                .filter(|&length| length <= crate::drag_payload::CAPACITY)
                .ok_or(Errno::E2BIG)?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|_| Errno::ENOMEM)?;
            bytes.resize(length, 0);
            process
                .read_user_memory(payload, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let serial = crate::drag_payload::set(kind, bytes);
            let words = text.get(..text_length as usize).ok_or(Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.start_carry(client, window, (kind, serial), words, damage, events)
            })?;
        }
        Request::RegisterHotKey {
            window,
            hot_key,
            code,
            modifiers,
        } => {
            let remove = modifiers & protocol::HOT_KEY_REMOVE != 0;
            let code = u16::try_from(code).map_err(|_| Errno::EINVAL)?;
            on_windows::<P, _>(|desktop, _, _, _| {
                desktop.register_hot_key(client, window, (hot_key, code, modifiers as u8), remove)
            })?;
        }
        Request::GetDrag {
            buffer,
            length,
            offset,
        } => {
            use eggos_drivers::desktop::protocol::get_drag_reply;
            let (payload, kind, serial) = crate::drag_payload::get();
            let payload = payload.as_deref().map_or(&[][..], Vec::as_slice);
            let rest = payload.get(offset as usize..).unwrap_or_default();
            let copied = rest.len().min(length as usize);
            if copied > 0 {
                process
                    .write_user_memory(buffer, &rest[..copied])
                    .map_err(|_| Errno::EFAULT)?;
            }
            put_word(bytes, get_drag_reply::LENGTH, payload.len() as u32);
            put_word(bytes, get_drag_reply::KIND, kind);
            put_word(bytes, get_drag_reply::SERIAL, serial);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::RaiseWindow { window } => {
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.raise_window(client, window, damage, events)
            })?;
        }
        Request::ShowShellMenu {
            window,
            length,
            entries,
        } => {
            let length = usize::try_from(length)
                .ok()
                .filter(|&length| length <= protocol::SHELL_MENU_CAPACITY)
                .ok_or(Errno::EINVAL)?;
            let mut bytes = [0u8; protocol::SHELL_MENU_CAPACITY];
            process
                .read_user_memory(entries, &mut bytes[..length])
                .map_err(|_| Errno::EFAULT)?;
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.show_shell_menu(client, window, &bytes[..length], damage, events)
            })?;
        }
        Request::SetTrayIcon {
            window,
            icon,
            flags,
            tip_length,
            tip,
            pixels,
        } => {
            let tip_length = usize::try_from(tip_length)
                .ok()
                .filter(|&length| length <= protocol::TRAY_TIP_CAPACITY)
                .ok_or(Errno::EINVAL)?;
            let mut text = [0u8; protocol::TRAY_TIP_CAPACITY];
            process
                .read_user_memory(tip, &mut text[..tip_length])
                .map_err(|_| Errno::EFAULT)?;
            let mut image = [0u32; TRAY_PIXELS];
            let given = match pixels {
                0 => None,
                address => {
                    let mut bytes = [0u8; TRAY_PIXELS * 4];
                    process
                        .read_user_memory(address, &mut bytes)
                        .map_err(|_| Errno::EFAULT)?;
                    for (pixel, word) in image.iter_mut().zip(bytes.chunks_exact(4)) {
                        *pixel = u32::from_ne_bytes([word[0], word[1], word[2], word[3]]);
                    }
                    Some(&image)
                }
            };
            let hidden = flags & protocol::tray_flags::HIDDEN != 0;
            let remove = flags & protocol::tray_flags::REMOVE != 0;
            if flags & protocol::tray_flags::BALLOON != 0 {
                let text = &text[..tip_length];
                on_windows::<P, _>(|desktop, _, damage, events| {
                    desktop.show_tray_balloon(client, window, icon, text, damage, events)
                })?;
                return Ok(0);
            }
            on_windows::<P, _>(|desktop, _, damage, _| {
                desktop.set_tray_icon(
                    client,
                    window,
                    icon,
                    (hidden, remove),
                    &text[..tip_length],
                    given,
                    damage,
                )
            })?;
        }
        Request::SetRotation { setting } => match crate::terminal::set_rotation_setting::<P>(setting) {
            Ok(()) => {}
            Err(crate::terminal::RotationRefused::NoSuchValue) => return Err(Errno::EINVAL),
            Err(crate::terminal::RotationRefused::NoMemory) => return Err(Errno::ENOMEM),
        },
        Request::BackUnused { window } => {
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.back_unused(client, window, damage, events);
                Ok::<(), WindowError>(())
            })?;
        }
        Request::SetTextFocus {
            window,
            kind,
            enter,
            cause,
        } => {
            use eggos_drivers::desktop::{EnterWord, FieldKind, FocusCause};
            on_windows::<P, _>(|desktop, _, damage, events| {
                if desktop.owner_of(window)? != client {
                    return Err(WindowError::NoSuchWindow);
                }
                desktop.set_text_focus(
                    window as u16,
                    FieldKind::from_number(kind),
                    EnterWord::from_number(enter),
                    FocusCause::from_number(cause),
                    damage,
                    events,
                );
                Ok(())
            })?;
        }
        Request::OnScreenKeyboard { action } => {
            use eggos_drivers::desktop::protocol::on_screen_keyboard;
            if action >= on_screen_keyboard::SET_MODE {
                if action - on_screen_keyboard::SET_MODE > 2 {
                    return Err(Errno::EINVAL);
                }
                crate::terminal::set_on_screen_keyboard_mode(action - on_screen_keyboard::SET_MODE);
            }
            let panel = on_windows::<P, _>(|desktop, _, damage, events| {
                match action {
                    on_screen_keyboard::SHOW => desktop
                        .show_on_screen_keyboard_by_request(damage, events)
                        .map_err(|()| WindowError::Unsupported)?,
                    on_screen_keyboard::HIDE => desktop.hide_on_screen_keyboard(damage, events),
                    _ => {}
                }
                Ok::<_, WindowError>(desktop.on_screen_keyboard_rect())
            })?;
            put_word(bytes, on_screen_keyboard::SHOWN, u32::from(panel.is_some()));
            let panel = panel.unwrap_or_default();
            for (index, value) in [panel.x, panel.y, panel.width, panel.height].into_iter().enumerate() {
                put_word(bytes, on_screen_keyboard::PANEL + 4 * index, value);
            }
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ShellNavigate { to } => {
            if !crate::terminal::shell_navigate::<P>(to) {
                return Err(Errno::EINVAL);
            }
        }
        Request::GetPalette => {
            put_word(bytes, 0, crate::terminal::desktop_palette());
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetPalette { palette } => {
            if palette >= crate::terminal::PALETTES {
                return Err(Errno::EINVAL);
            }
            crate::terminal::set_desktop_palette::<P>(palette);
        }
        Request::GetReduceMotion => {
            put_word(bytes, 0, u32::from(crate::terminal::reduce_motion()));
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetReduceMotion { reduce } => {
            if reduce > 1 {
                return Err(Errno::EINVAL);
            }
            crate::terminal::set_reduce_motion::<P>(reduce == 1);
        }
        Request::GetKeyboardLayout => {
            let flags = [
                (crate::input::caps_lock(), keyboard_flags::CAPS_LOCK),
                (
                    crate::input::summary().keyboards > 0,
                    keyboard_flags::KEYBOARD,
                ),
                (crate::input::num_lock(), keyboard_flags::NUM_LOCK),
                (crate::input::scroll_lock(), keyboard_flags::SCROLL_LOCK),
            ]
            .into_iter()
            .filter(|(set, _)| *set)
            .fold(0, |flags, (_, bit)| flags | bit);
            put_word(
                bytes,
                keyboard_layout_reply::LAYOUT,
                crate::input::layout_number() as u32,
            );
            put_word(bytes, keyboard_layout_reply::FLAGS, flags);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SetKeyboardLayout { layout } => {
            if !crate::input::set_layout(layout as usize) {
                return Err(Errno::EINVAL);
            }
            crate::terminal::keyboard_layouts_changed::<P>();
        }
        Request::SetKeyboardLayouts {
            count,
            layouts,
            switch_key,
        } => {
            let numbers = layouts.map(usize::from);
            let layouts = numbers
                .get(..count as usize)
                .and_then(eggos_settings_file::KeyboardLayoutSet::of)
                .ok_or(Errno::EINVAL)?;
            let switch_key = eggos_settings_file::SwitchKey::ALL
                .get(switch_key as usize)
                .copied()
                .ok_or(Errno::EINVAL)?;
            crate::input::set_layouts(layouts, switch_key);
            crate::terminal::keyboard_layouts_changed::<P>();
        }
        Request::SetInputPreferences {
            left_handed,
            natural_scrolling,
            pointer_speed,
            repeat_delay_ms,
            repeat_rate,
        } => {
            let preferences = crate::input::Preferences {
                left_handed: left_handed != 0,
                natural_scrolling: natural_scrolling != 0,
                pointer_speed,
                repeat_delay_ms,
                repeat_rate,
            };
            if !crate::input::set_preferences(preferences) {
                return Err(Errno::EINVAL);
            }
        }
        Request::SetClipboard { buffer, length } => {
            let length = usize::try_from(length)
                .ok()
                .filter(|&length| length <= crate::clipboard::CAPACITY)
                .ok_or(Errno::E2BIG)?;
            let mut text = Vec::new();
            text.try_reserve_exact(length).map_err(|_| Errno::ENOMEM)?;
            text.resize(length, 0);
            process
                .read_user_memory(buffer, &mut text)
                .map_err(|_| Errno::EFAULT)?;
            if core::str::from_utf8(&text).is_err() {
                return Err(Errno::EINVAL);
            }
            crate::clipboard::set(text);
        }
        Request::GetClipboard {
            buffer,
            length,
            offset,
        } => {
            let (text, serial) = crate::clipboard::get();
            let text = text.as_deref().map_or(&[][..], Vec::as_slice);
            let rest = usize::try_from(offset)
                .ok()
                .and_then(|offset| text.get(offset..))
                .unwrap_or_default();
            let copied = rest
                .len()
                .min(usize::try_from(length).unwrap_or(usize::MAX));
            if copied > 0 {
                process
                    .write_user_memory(buffer, &rest[..copied])
                    .map_err(|_| Errno::EFAULT)?;
            }
            put_word(bytes, get_clipboard_reply::LENGTH, text.len() as u32);
            put_word(bytes, get_clipboard_reply::SERIAL, serial);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::AppsChanged => reload_catalogue::<P>(),
        Request::OpenApp { length, name } => {
            let name = name.get(..length as usize).ok_or(Errno::EINVAL)?;
            if !open_app::<P>(name) {
                return Err(Errno::ENOENT);
            }
        }
        Request::GetLaunch { length, name } => {
            use eggos_drivers::desktop::Launch;
            use eggos_drivers::desktop::protocol::get_launch_reply;
            let name = name.get(..length as usize).ok_or(Errno::EINVAL)?;
            if !crate::terminal::has_interactive_desktop() {
                return Err(Errno::ENXIO);
            }
            let launch = crate::terminal::launch_of::<P>(name).ok_or(Errno::ENOENT)?;
            let (kind, command) = match &launch {
                Launch::Start(command) => (get_launch_reply::START, command.as_bytes()),
                Launch::Type(line) => (get_launch_reply::TYPE, line.as_bytes()),
            };
            let mut reply = [0u8; eggos_drivers::desktop::protocol::size::GET_LAUNCH];
            put_word(&mut reply, get_launch_reply::KIND, kind);
            put_word(&mut reply, get_launch_reply::LENGTH, command.len() as u32);
            reply[get_launch_reply::COMMAND..get_launch_reply::COMMAND + command.len()].copy_from_slice(command);
            process
                .write_user_memory(argument, &reply)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::TypeInTerminal { length, line } => {
            let line = line.get(..length as usize).ok_or(Errno::EINVAL)?;
            if line.iter().any(|&byte| byte < b' ' || byte == 0x7f) {
                return Err(Errno::EINVAL);
            }
            if !type_in_terminal::<P>(line) {
                return Err(Errno::ENXIO);
            }
        }
        Request::CaptureWindow {
            window,
            first_row,
            row_count,
            buffer,
            length,
        } => {
            let rows = capture::<P>(process, window, first_row, row_count, buffer, length)?;
            put_word(bytes, capture_reply::WIDTH, rows.width);
            put_word(bytes, capture_reply::HEIGHT, rows.height);
            put_word(bytes, capture_reply::ROWS, rows.copied);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::SendInput {
            window,
            kind,
            x,
            y,
            code,
            value,
        } => {
            let input = InjectedInput::from_request(kind, x, y, code, value).map_err(refused)?;
            if crate::input::person_holds_input() || crate::input::has_pending_events() {
                return Err(Errno::EBUSY);
            }
            let injected = on_windows::<P, _>(|desktop, _, damage, events| {
                let injected = desktop.send_input(window, input, events)?;
                match injected {
                    Injected::QueuedForDialog { dialog } => {
                        desktop.note_agent_input(dialog, input, damage)
                    }
                    Injected::DialogOpen { .. } => {}
                    Injected::Queued | Injected::ForTerminal(_) => {
                        desktop.note_agent_input(window, input, damage)
                    }
                }
                Ok(injected)
            })?;
            let (receiver, outcome) = match injected {
                Injected::Queued => (window, input_outcome::SENT),
                Injected::ForTerminal(input) => {
                    send_to_terminal::<P>(input);
                    (window, input_outcome::SENT)
                }
                Injected::QueuedForDialog { dialog } => (dialog, input_outcome::SENT_TO_DIALOG),
                Injected::DialogOpen { dialog } => (dialog, input_outcome::DIALOG_OPEN),
            };
            bytes.fill(0);
            put_word(bytes, send_input_reply::RECEIVER, receiver);
            put_word(bytes, send_input_reply::OUTCOME, outcome);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::GetMenu { window } => {
            let mut pool = [0u8; MENU_POOL_CAPACITY];
            let length = on_windows::<P, _>(|desktop, _, _, _| {
                desktop.refuse_if_not_responding(window)?;
                let menu = desktop.menu_pool(window)?;
                pool[..menu.len()].copy_from_slice(menu);
                Ok(menu.len())
            })?;
            bytes.fill(0);
            put_word(bytes, 0, window);
            put_word(bytes, get_menu_reply::LENGTH, length as u32);
            bytes[get_menu_reply::POOL..get_menu_reply::POOL + length]
                .copy_from_slice(&pool[..length]);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ChooseMenu {
            window,
            menu,
            entry,
            label_length,
            label,
        } => {
            let label = label.get(..label_length as usize).ok_or(Errno::EINVAL)?;
            if crate::input::person_holds_input() || crate::input::has_pending_events() {
                return Err(Errno::EBUSY);
            }
            on_windows::<P, _>(|desktop, _, damage, events| {
                desktop.refuse_if_not_responding(window)?;
                desktop.choose_menu(window, menu, entry, label, damage, events)?;
                desktop.note_agent_action(damage);
                Ok(())
            })?;
        }
        Request::GetTree {
            window,
            buffer,
            length,
        } => {
            if length < MAX_TREE_BYTES as u64 {
                return Err(Errno::EINVAL);
            }
            buffer
                .checked_add(MAX_TREE_BYTES as u64)
                .ok_or(Errno::EFAULT)?;
            let unresponsive =
                on_windows::<P, _>(|desktop, _, _, _| Ok(desktop.not_responding_window(window)))?;
            let tree = match unresponsive {
                Some(listed) => not_responding_tree(&listed),
                None => ask::<P>(process, thread, query(window, query_kind::TREE))?
                    .filter(|answer| answer.outcome == tree_answer::TREE)
                    .map(|answer| answer.tree)
                    .unwrap_or_default(),
            };
            if !tree.is_empty() {
                process
                    .write_user_memory(buffer, &tree)
                    .map_err(|_| Errno::EFAULT)?;
            }
            let outcome = match tree.is_empty() {
                true => tree_answer::PIXELS_ONLY,
                false => tree_answer::TREE,
            };
            put_word(bytes, get_tree_reply::ANSWER, outcome);
            put_word(bytes, get_tree_reply::LENGTH, tree.len() as u32);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ActOnElement {
            window,
            element,
            action,
            text_length,
            text,
        } => {
            let text = text.get(..text_length as usize).ok_or(Errno::EINVAL)?;
            let readable =
                core::str::from_utf8(text).is_ok_and(|text| !text.chars().any(char::is_control));
            let known = (element_action::INVOKE..=element_action::SELECT).contains(&action);
            if !known || !readable || element == 0 {
                return Err(Errno::EINVAL);
            }
            on_windows::<P, _>(|desktop, _, _, _| desktop.refuse_if_not_responding(window))?;
            if crate::input::person_holds_input() || crate::input::has_pending_events() {
                return Err(Errno::EBUSY);
            }
            let mut request = query(window, query_kind::ACT);
            request.element = element;
            request.action = action;
            request.text_length = text.len();
            request.text[..text.len()].copy_from_slice(text);
            let answer = ask::<P>(process, thread, request)?.ok_or(Errno::ETIMEDOUT)?;
            put_word(bytes, act_request::OUTCOME, answer.outcome);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::ReadQuery { serial } => {
            let query = window::read_query(client, serial).ok_or(Errno::ENOENT)?;
            bytes.fill(0);
            for (offset, value) in [
                (0, serial),
                (read_query_reply::KIND, query.kind),
                (read_query_reply::WINDOW, query.window),
                (read_query_reply::ELEMENT, query.element),
                (read_query_reply::ACTION, query.action),
                (read_query_reply::TEXT_LENGTH, query.text_length as u32),
            ] {
                put_word(bytes, offset, value);
            }
            bytes[read_query_reply::TEXT..read_query_reply::TEXT + query.text_length]
                .copy_from_slice(&query.text[..query.text_length]);
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
        Request::AnswerQuery {
            serial,
            outcome,
            buffer,
            length,
        } => {
            let length = usize::try_from(length)
                .ok()
                .filter(|&length| length <= MAX_TREE_BYTES)
                .ok_or(Errno::EINVAL)?;
            window::read_query(client, serial).ok_or(Errno::ENOENT)?;
            let mut tree = Vec::new();
            if !crate::heap::may_grow_for_user(length) || tree.try_reserve_exact(length).is_err() {
                return Err(Errno::ENOMEM);
            }
            tree.resize(length, 0);
            if length > 0 {
                process
                    .read_user_memory(buffer, &mut tree)
                    .map_err(|_| Errno::EFAULT)?;
            }
            if !window::answer_query(client, serial, QueryAnswer { outcome, tree }) {
                return Err(Errno::ENOENT);
            }
            crate::console::wake_input_waiters::<P>();
        }
        Request::GetPlacement { window } => {
            let rect = on_windows::<P, _>(|desktop, _, _, _| desktop.placement(client, window))?;
            for (offset, value) in [
                (placement_reply::X, rect.x as u32),
                (placement_reply::Y, rect.y),
                (placement_reply::WIDTH, rect.width),
                (placement_reply::HEIGHT, rect.height),
            ] {
                put_word(bytes, offset, value);
            }
            process
                .write_user_memory(argument, bytes)
                .map_err(|_| Errno::EFAULT)?;
        }
    }
    Ok(0)
}

fn listed_state(info: &ListedWindow) -> u32 {
    match info.responding {
        true => info.state.value(),
        false => info.state.value() | list_reply::NOT_RESPONDING,
    }
}

fn not_responding_tree(listed: &ListedWindow) -> Vec<u8> {
    use eggos_element_tree::{Bounds, Element, HEADER_BYTES, Role, TreeWriter, flags};
    let mut bytes = [0u8; HEADER_BYTES + TITLE_CAPACITY];
    let mut writer = TreeWriter::new(&mut bytes);
    let bounds = Bounds::new(0, 0, listed.rect.width as i32, listed.rect.height as i32);
    let root = Element::new(1, 0, Role::Window, bounds)
        .named(&listed.title[..listed.title_length])
        .flagged(flags::NOT_RESPONDING);
    writer.push(&root);
    writer.bytes().to_vec()
}

fn query(window: u32, kind: u32) -> QueryRequest {
    QueryRequest {
        window,
        kind,
        element: 0,
        action: 0,
        text_length: 0,
        text: [0; ELEMENT_TEXT_CAPACITY],
    }
}

fn ask<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    request: QueryRequest,
) -> Result<Option<QueryAnswer>, Errno> {
    let serial = on_windows::<P, _>(|desktop, _, damage, events| {
        let owner = desktop.owner_of(request.window)?;
        let serial = window::start_query(owner, request).ok_or(WindowError::QueueFull)?;
        let record = EventRecord::new(
            record_kind::QUERY,
            0,
            serial as i32,
            request.kind as i32,
            request.element,
            request.action,
        );
        if let Err(error) = desktop.ask_owner(request.window, record, damage, events) {
            window::finish_query(serial);
            return Err(error);
        }
        Ok(serial)
    })?;
    let wait_ticks = u64::from(QUERY_ANSWER_MS) * P::clock_frequency() / 1000;
    let deadline = P::clock_ticks().saturating_add(wait_ticks);
    let waited = wait_for_input(process, thread, deadline, || window::query_answered(serial));
    let answer = window::finish_query(serial);
    match waited {
        ControlFlow::Continue(Waited::Signalled) | ControlFlow::Break(()) if answer.is_none() => {
            Err(Errno::EINTR)
        }
        _ => Ok(answer),
    }
}

struct CapturedRows {
    width: u32,
    height: u32,
    copied: u32,
}

fn capture<P: Platform>(
    process: &Process<P>,
    window: u32,
    first_row: u32,
    row_count: u32,
    buffer: u64,
    length: u64,
) -> Result<CapturedRows, Errno> {
    let (layout, pixels, view_only_turns) = on_windows::<P, _>(|desktop, buffers, damage, _| {
        let layout = desktop.content_layout(window)?;
        let turns = desktop.person_view_only_turns(window)?;
        let id = WindowId::try_from(window).map_err(|_| WindowError::NoSuchWindow)?;
        desktop.note_agent_action(damage);
        Ok((layout, buffers.shared(id), turns))
    })?;
    let rows = layout
        .height
        .checked_sub(first_row)
        .ok_or(Errno::EINVAL)?
        .min(row_count);
    let needed = u64::from(rows)
        .checked_mul(u64::from(layout.width) * 4)
        .filter(|&needed| needed <= length)
        .ok_or(Errno::EINVAL)?;
    buffer.checked_add(needed).ok_or(Errno::EFAULT)?;
    copy_content_rows(
        &layout,
        pixels.as_deref(),
        first_row,
        rows,
        &mut |offset, piece| {
            let at = buffer.checked_add(offset as u64).ok_or(Errno::EFAULT)?;
            process
                .write_user_memory(at, piece)
                .map_err(|_| Errno::EFAULT)
        },
    )?;
    let turns_after =
        on_windows::<P, _>(|desktop, _, _, _| desktop.person_view_only_turns(window))?;
    if turns_after != view_only_turns {
        return Err(Errno::EACCES);
    }
    Ok(CapturedRows {
        width: layout.width,
        height: layout.height,
        copied: rows,
    })
}

fn is_privileged<P: Platform>(process: &Process<P>) -> bool {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .is_privileged()
}

fn may_change_desktop_settings<P: Platform>(process: &Process<P>) -> bool {
    is_privileged(process)
}

fn may_open_apps<P: Platform>(process: &Process<P>) -> bool {
    is_privileged(process)
}

fn may_type_in_terminal<P: Platform>(process: &Process<P>) -> bool {
    is_privileged(process)
}

fn may_capture_windows<P: Platform>(process: &Process<P>) -> bool {
    is_privileged(process)
}

fn may_send_input<P: Platform>(process: &Process<P>) -> bool {
    is_privileged(process)
}

fn on_windows<P: Platform, R>(
    change: impl FnOnce(
        &mut Desktop,
        &mut window::Buffers,
        &mut Damage,
        &mut dyn EventSink,
    ) -> Result<R, WindowError>,
) -> Result<R, Errno> {
    with_windows::<P, _>(change)
        .ok_or(Errno::ENXIO)?
        .map_err(refused)
}

const SIZE_ATTEMPTS: usize = 4;

fn create<P: Platform>(
    connection: &Connection,
    role: u32,
    parent: u32,
    width: u32,
    height: u32,
    flags: u32,
) -> Result<(WindowId, u64, u32, u32), Errno> {
    let client = connection.client().id();
    let size = with_windows::<P, _>(|desktop, _, _, _| desktop.created_size(width, height))
        .ok_or(Errno::ENXIO)?;
    let buffer = PixelBuffer::new(size.0, size.1, connection.client()).ok_or(Errno::ENOMEM)?;
    with_windows::<P, _>(|desktop, buffers, _, _| {
        if desktop.created_size(width, height) != buffer.size() {
            return Err(WindowError::Invalid);
        }
        let created = desktop.create(client, role, parent, width, height, flags)?;
        let offset = buffers.attach(created.window, buffer);
        Ok((created.window, offset, created.width, created.height))
    })
    .ok_or(Errno::ENXIO)?
    .map_err(refused)
}

fn set_size<P: Platform>(
    connection: &Connection,
    window: u32,
    width: u32,
    height: u32,
) -> Result<(u64, u32, u32), Errno> {
    let client = connection.client().id();
    let id = WindowId::try_from(window).map_err(|_| Errno::EINVAL)?;
    for _ in 0..SIZE_ATTEMPTS {
        let size = with_windows::<P, _>(|desktop, _, _, _| {
            desktop.size_for(client, window, width, height)
        })
        .ok_or(Errno::ENXIO)?
        .map_err(refused)?;
        let buffer = PixelBuffer::new(size.0, size.1, connection.client()).ok_or(Errno::ENOMEM)?;
        let attached = with_windows::<P, _>(|desktop, buffers, damage, events| {
            if desktop.size_for(client, window, width, height)? != buffer.size() {
                return Ok(None);
            }
            let resized = desktop.set_size(client, window, width, height, damage, events)?;
            Ok(Some((buffers.attach(id, buffer), resized.0, resized.1)))
        })
        .ok_or(Errno::ENXIO)?
        .map_err(refused)?;
        if let Some(attached) = attached {
            return Ok(attached);
        }
    }
    Err(Errno::EAGAIN)
}

/// The `poll` events `connection` reports of `requested`: `POLLIN` while an event waits.
pub fn events(connection: &Connection, requested: u16) -> u16 {
    match connection.client().has_events() {
        true => requested & (event::IN | event::RDNORM),
        false => 0,
    }
}

/// `read` or `readv` of `buffers` from `/dev/window` through `file`: whole records, as many as
/// fit (`EINVAL` if not one does), waiting for one unless `O_NONBLOCK` (`EAGAIN`); sets the
/// call's result and delivers the signals due after it. A record whose copy to the program
/// fails is lost, as on Linux's input devices.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    connection: &Connection,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match read_or_wait(process, thread, file, connection, buffers)? {
            Some(result) => break result,
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn read_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    connection: &Connection,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let room = buffers.total() / EVENT_SIZE as u64;
    if room == 0 {
        return ControlFlow::Continue(Some(Err(Errno::EINVAL)));
    }
    let client = connection.client();
    loop {
        let mut records = [EventRecord::default(); READ_RECORDS];
        let wanted = room.min(READ_RECORDS as u64) as usize;
        let taken = client.take_events(&mut records[..wanted], window::time_ms::<P>());
        if taken > 0 {
            if client.seen_not_responding() {
                let (id, since) = (client.id(), client.unread_since());
                with_windows::<P, _>(|desktop, _, damage, _| {
                    desktop.set_unread_since(id, since, damage);
                    client.set_seen_not_responding(desktop.client_not_responding(id));
                });
            }
            if records[..taken]
                .iter()
                .any(|record| record.kind == record_kind::CLOSE_REQUEST)
            {
                let id = client.id();
                with_windows::<P, _>(|desktop, _, damage, _| desktop.close_answered(id, damage));
            }
            return ControlFlow::Continue(Some(copy_out(process, buffers, &records[..taken])));
        }
        if file.is_nonblocking() {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        if wait_for_input(process, thread, u64::MAX, || client.has_events())? == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
    }
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    records: &[EventRecord],
) -> Result<u64, Errno> {
    let mut bytes = [0u8; READ_RECORDS * EVENT_SIZE];
    for (record, out) in records.iter().zip(bytes.as_chunks_mut::<EVENT_SIZE>().0) {
        out.copy_from_slice(&record.to_bytes());
    }
    let bytes = &bytes[..records.len() * EVENT_SIZE];
    let mut copied = 0;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let part = (length as usize).min(bytes.len() - copied);
        if part > 0
            && process
                .write_user_memory(base, &bytes[copied..copied + part])
                .is_err()
        {
            return Err(Errno::EFAULT);
        }
        copied += part;
        if copied == bytes.len() {
            break;
        }
    }
    Ok(copied as u64)
}

/// `mmap` of `length` bytes of `/dev/window` at `offset` with `attributes` where `placement`
/// says: the pixels of the caller's window whose buffer `offset` names, shared with the
/// server. `EINVAL` for an offset that names none of the caller's buffers or a length past
/// the buffer.
pub fn map<P: Platform>(
    process: &Process<P>,
    connection: &Connection,
    placement: Placement,
    length: u64,
    attributes: Attributes,
    offset: u64,
) -> Result<u64, Errno> {
    let client = connection.client().id();
    let buffer = with_windows::<P, _>(|desktop, buffers, _, _| {
        let (window, buffer) = buffers.at_offset(offset)?;
        desktop.owns(client, window).then_some(buffer)
    })
    .ok_or(Errno::ENXIO)?
    .ok_or(Errno::EINVAL)?;
    if length > buffer.length() {
        return Err(Errno::EINVAL);
    }
    process
        .map_lent_frames(placement, length, attributes, buffer)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid => Errno::EINVAL,
            RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

#[cfg(test)]
mod tests {
    use super::super::process_table::name_of;
    use super::*;

    #[test]
    fn only_eggway_serves_every_linux_window_app() {
        assert!(serves_linux_apps(&name_of(b"eggway")));
        for other in [&b"eggwayx"[..], b"egg", b"settings", b""] {
            assert!(!serves_linux_apps(&name_of(other)), "{other:?}");
        }
    }
}
