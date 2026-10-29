//! comdlg32's `GetOpenFileNameW` and `GetSaveFileNameW` (D145): EggOS's own file picker of
//! `docs/ux/windows-controls-v0.3.md` section 7, a window of the layer's built from the
//! system's controls in the dark palette and run by a modal loop like the message box's, which
//! dispatches the calling thread's other messages meanwhile. Its owner's window takes no input
//! while it is open. Paths are shown and returned in Windows' form (`C:\` is `/`). The picker's
//! state is a heap block its window's entry points to, reached only under the lock; folders are
//! read outside it.

use super::controls::{caret_to_end, fill_list, selected_item};
use super::dialog::{self, set_focus};
use super::directory::{for_each_entry, working_directory, write_windows_path};
use super::gui::{self, Gui, Next, device, with};
use super::user32::{create_child, def_window_proc_w, destroy_tree, dispatch, translate};
use super::window_device::{FIXED_SIZE, ROLE_DIALOG, ROLE_NORMAL};
use super::{Bool, FALSE, error_for, heap, paint, thread_environment};
use crate::gui::message;
use crate::gui::picker;
use crate::gui::queue::{Filter, Message};
use crate::gui::windows::{ControlKind, Procedure, Window};
use crate::library::Export;
use crate::linux::{self, AT_FDCWD};
use crate::text::{self, PATH_CAPACITY};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_windows_gui::controls::DARK;
use eggos_windows_gui::controls::list_box::compare_names;
use eggos_windows_gui::file_picker::{self as layout, PickerKind};

const CHROME: (i32, i32) = (2, 30);
const OPEN_FILE_NAME_SIZE: u32 = 152;
const OPEN_FILE_NAME_SIZE_400: u32 = 136;
const OFN_PATH_MUST_EXIST: u32 = 0x800;
const OFN_FILE_MUST_EXIST: u32 = 0x1000;
const CDERR_STRUCTSIZE: u32 = 0x0001;
const CDERR_INITIALIZATION: u32 = 0x0002;
const FNERR_BUFFERTOOSMALL: u32 = 0x3003;
const WS_CHILD_VISIBLE: u32 = 0x5000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_DISABLED: u32 = 0x0800_0000;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const ES_READONLY: u32 = 0x0800;
const LBS_NOTIFY: u32 = 0x1;
const ID_UP: usize = 0x501;
const ID_FOLDER: usize = 0x502;
const ID_ENTRIES: usize = 0x503;
const ID_NAME_LABEL: usize = 0x504;
const ID_NAME: usize = 0x505;
const ID_FILTER: usize = 0x506;
const BN_CLICKED: usize = 0;
const LBN_SELCHANGE: usize = 1;
const VK_BACK: usize = 0x08;
const MAX_ENTRIES: usize = 256;
const LISTING_UNITS: usize = 8192;
const NAME_UNITS: usize = 1024;
const PATTERN_BYTES: usize = 256;
const EXTENSION_BYTES: usize = 32;
const FILTER_UNITS: usize = 4096;
const TITLE_BYTES: usize = 256;

static LAST_ERROR: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct OpenFileName {
    size: u32,
    owner: usize,
    instance: usize,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    default_extension: *const u16,
    custom_data: usize,
    hook: usize,
    template_name: *const u16,
    reserved: usize,
    reserved_flags: u32,
    flags_ex: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Waiting,
    Cancelled,
    Chosen,
}

struct PickerState {
    kind: PickerKind,
    flags: u32,
    folder: [u8; PATH_CAPACITY],
    folder_length: usize,
    pattern: [u8; PATTERN_BYTES],
    pattern_length: usize,
    extension: [u8; EXTENSION_BYTES],
    extension_length: usize,
    chosen: [u8; PATH_CAPACITY],
    chosen_length: usize,
    answer: Answer,
}

struct Listing {
    count: usize,
    entries: [(u16, u16, bool); MAX_ENTRIES],
    used: usize,
    units: [u16; LISTING_UNITS],
}

fn state_of(gui: &mut Gui, window: usize) -> Option<&mut PickerState> {
    let entry = gui.windows.get(window)?;
    let Procedure::FilePicker(address) = entry.procedure else {
        return None;
    };
    // SAFETY: a picker's state is the heap block its call made and frees only after taking the
    // picker's window out of the table under the lock, which the caller holds through `gui`.
    Some(unsafe { &mut *(address as *mut PickerState) })
}

fn with_state<R>(window: usize, work: impl FnOnce(&mut PickerState) -> R) -> Option<R> {
    with(|gui| state_of(gui, window).map(work)).flatten()
}

fn child(window: usize, id: usize) -> usize {
    dialog::dialog_item(window, id as i32).unwrap_or(0)
}

fn units_of(text: &str, out: &mut [u16]) -> usize {
    let mut length = 0;
    for (slot, unit) in out.iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
        length += 1;
    }
    length
}

fn set_text(window: usize, units: &[u16]) {
    let mut terminated = [0u16; NAME_UNITS + 1];
    let length = units.len().min(NAME_UNITS);
    terminated[..length].copy_from_slice(&units[..length]);
    gui::send(window, message::SET_TEXT, 0, terminated.as_ptr() as usize);
}

fn read_listing(folder: &[u8], pattern: &[u8], listing: &mut Listing) {
    listing.count = 0;
    listing.used = 0;
    let mut path = [0u8; PATH_CAPACITY + 1];
    path[..folder.len()].copy_from_slice(folder);
    for_each_entry(&path[..=folder.len()], &mut |name, folder| {
        if picker::is_hidden(name) || (!folder && !picker::pattern_matches(pattern, name)) {
            return;
        }
        let length = text::utf16_length(name);
        if listing.count == MAX_ENTRIES || listing.used + length > LISTING_UNITS {
            return;
        }
        let written = text::write_utf16(
            name,
            &mut listing.units[listing.used..listing.used + length],
        );
        listing.entries[listing.count] = (listing.used as u16, written as u16, folder);
        listing.count += 1;
        listing.used += written;
    });
    let (entries, units) = (&mut listing.entries[..listing.count], &listing.units);
    let name = |entry: &(u16, u16, bool)| {
        let start = usize::from(entry.0);
        &units[start..start + usize::from(entry.1)]
    };
    entries.sort_unstable_by(|a, b| b.2.cmp(&a.2).then_with(|| compare_names(name(a), name(b))));
}

fn show_folder(window: usize) {
    let mut folder = [0u8; PATH_CAPACITY];
    let mut pattern = [0u8; PATTERN_BYTES];
    let Some((folder_length, pattern_length)) = with_state(window, |state| {
        folder[..state.folder_length].copy_from_slice(&state.folder[..state.folder_length]);
        pattern[..state.pattern_length].copy_from_slice(&state.pattern[..state.pattern_length]);
        (state.folder_length, state.pattern_length)
    }) else {
        return;
    };
    let listing = heap::allocate(core::mem::size_of::<Listing>()).cast::<Listing>();
    if !listing.is_null() {
        // SAFETY: a fresh, zeroed, 16-byte aligned heap block for one `Listing`, whose fields
        // are integers, booleans and arrays of them, for which zeros are valid.
        let listing = unsafe { &mut *listing };
        read_listing(
            &folder[..folder_length],
            &pattern[..pattern_length],
            listing,
        );
        let units = &listing.units;
        let mut entries =
            listing.entries[..listing.count]
                .iter()
                .map(|&(start, length, folder)| {
                    let start = usize::from(start);
                    (&units[start..start + usize::from(length)], folder)
                });
        fill_list(child(window, ID_ENTRIES), &mut entries);
    }
    if !listing.is_null() {
        // SAFETY: the block was made above and nothing else holds it.
        unsafe { heap::free(listing.cast()) };
    }
    let mut shown = [0u16; NAME_UNITS];
    let length = write_windows_path(&folder[..folder_length], &mut shown).unwrap_or(0);
    set_text(child(window, ID_FOLDER), &shown[..length]);
    let _ = dialog::enable(child(window, ID_UP), folder_length > 1);
}

fn enter(window: usize, path: &[u8]) {
    with_state(window, |state| {
        let length = path.len().min(PATH_CAPACITY);
        state.folder[..length].copy_from_slice(&path[..length]);
        state.folder_length = length;
    });
    show_folder(window);
}

fn go_up(window: usize) {
    let mut parent = [0u8; PATH_CAPACITY];
    let length = with_state(window, |state| {
        text::absolute_path(&state.folder[..state.folder_length], b"..", &mut parent)
    })
    .flatten();
    if let Some(length) = length {
        enter(window, &parent[..length]);
    }
}

fn is_folder(path: &[u8]) -> Option<bool> {
    let mut terminated = [0u8; PATH_CAPACITY + 1];
    terminated[..path.len()].copy_from_slice(path);
    let words = linux::stat_at(AT_FDCWD, &terminated[..=path.len()]).ok()?;
    Some(linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY)
}

fn resolve(window: usize, typed: &[u16], out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let mut eggos = [0u8; PATH_CAPACITY];
    let length = text::eggos_path(text::utf16_chars(typed), &mut eggos)? - 1;
    let slash = |unit: u16| unit == u16::from(b'\\') || unit == u16::from(b'/');
    let verbatim = typed.len() >= 4
        && slash(typed[0])
        && slash(typed[1])
        && typed[2] == u16::from(b'?')
        && slash(typed[3]);
    let bare = if verbatim { &typed[4..] } else { typed };
    let drive = bare.len() >= 2 && bare[1] == u16::from(b':');
    if drive && !matches!(bare[0], 0x43 | 0x63) {
        return None;
    }
    with_state(window, |state| {
        text::absolute_path(&state.folder[..state.folder_length], &eggos[..length], out)
    })
    .flatten()
}

fn accept(window: usize) {
    let list = child(window, ID_ENTRIES);
    let focused_list = with(|gui| gui.key_target(window) == list).unwrap_or(false);
    let mut item = [0u16; NAME_UNITS];
    if focused_list && let Some((length, true)) = selected_item(list, &mut item) {
        let mut path = [0u8; PATH_CAPACITY];
        if let Some(length) = resolve(window, &item[..length], &mut path) {
            enter(window, &path[..length]);
        }
        return;
    }
    let name = child(window, ID_NAME);
    let mut typed = [0u16; NAME_UNITS + 1];
    let length = gui::send(
        name,
        message::GET_TEXT,
        typed.len(),
        typed.as_mut_ptr() as usize,
    );
    let typed = trim(&typed[..length.min(NAME_UNITS)]);
    if typed.is_empty() {
        return;
    }
    let mut path = [0u8; PATH_CAPACITY];
    let Some(mut length) = resolve(window, typed, &mut path) else {
        return;
    };
    if is_folder(&path[..length]) == Some(true) {
        enter(window, &path[..length]);
        set_text(name, &[]);
        return;
    }
    let Some((kind, flags, extension, extension_length)) = with_state(window, |state| {
        (
            state.kind,
            state.flags,
            state.extension,
            state.extension_length,
        )
    }) else {
        return;
    };
    if kind == PickerKind::Save && extension_length > 0 && !picker::has_extension(&path[..length]) {
        let end = length + 1 + extension_length;
        if end > PATH_CAPACITY {
            return;
        }
        path[length] = b'.';
        path[length + 1..end].copy_from_slice(&extension[..extension_length]);
        length = end;
    }
    let exists = is_folder(&path[..length]).is_some();
    if kind == PickerKind::Open && flags & OFN_FILE_MUST_EXIST != 0 && !exists {
        return;
    }
    let parent_end = path[..length]
        .iter()
        .rposition(|byte| *byte == b'/')
        .unwrap_or(0)
        .max(1);
    if flags & OFN_PATH_MUST_EXIST != 0 && is_folder(&path[..parent_end]) != Some(true) {
        return;
    }
    with_state(window, |state| {
        state.chosen[..length].copy_from_slice(&path[..length]);
        state.chosen_length = length;
        state.answer = Answer::Chosen;
    });
    gui::wake();
}

fn trim(units: &[u16]) -> &[u16] {
    let start = units
        .iter()
        .position(|unit| *unit != 0x20)
        .unwrap_or(units.len());
    let end = units
        .iter()
        .rposition(|unit| *unit != 0x20)
        .map_or(start, |at| at + 1);
    &units[start..end]
}

fn cancel(window: usize) {
    with_state(window, |state| state.answer = Answer::Cancelled);
    gui::wake();
}

fn selection_changed(window: usize) {
    let mut item = [0u16; NAME_UNITS];
    if let Some((length, false)) = selected_item(child(window, ID_ENTRIES), &mut item) {
        let name = child(window, ID_NAME);
        set_text(name, &item[..length]);
        caret_to_end(name);
    }
}

fn paint_surface(window: usize) {
    let Ok((hdc, _, _)) = paint::begin(window) else {
        return;
    };
    let _ = paint::draw_in(hdc, |surface, context, _| {
        if let Some(surface) = surface {
            surface.fill(context.clip, DARK.surface);
        }
    });
    paint::finish(hdc);
}

/// The picker window's procedure: its background, its controls' notifications, and the close
/// request as Cancel.
pub fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::PAINT => {
            paint_surface(window);
            0
        }
        message::ERASE_BACKGROUND => 1,
        message::COMMAND => {
            let (id, notification) = (wparam & 0xffff, (wparam >> 16) & 0xffff);
            match (id, notification) {
                (ID_UP, BN_CLICKED) => go_up(window),
                (message::ID_OK, _) => accept(window),
                (message::ID_CANCEL, _) => cancel(window),
                (ID_ENTRIES, LBN_SELCHANGE) => selection_changed(window),
                _ => {}
            }
            0
        }
        message::SYS_COMMAND if wparam & 0xfff0 == message::SC_CLOSE => {
            cancel(window);
            0
        }
        message::CLOSE => {
            cancel(window);
            0
        }
        _ => def_window_proc_w(window, number, wparam, lparam),
    }
}

fn answer_of(gui: &Gui, window: usize) -> Answer {
    let Some(Procedure::FilePicker(address)) = gui.windows.get(window).map(|entry| entry.procedure)
    else {
        return Answer::Cancelled;
    };
    // SAFETY: as `state_of`: the caller holds the lock and the window is in the table.
    unsafe { (*(address as *const PickerState)).answer }
}

fn run(window: usize) -> Answer {
    loop {
        match with(|gui| answer_of(gui, window)).unwrap_or(Answer::Cancelled) {
            Answer::Waiting => {}
            answer => return answer,
        }
        let waiting = |gui: &Gui| answer_of(gui, window) != Answer::Waiting;
        match gui::wait_for(Filter::default(), true, true, &waiting) {
            Next::Done => {}
            Next::Nothing => return Answer::Cancelled,
            Next::Message(next) if next.message == message::QUIT => {
                with(|gui| gui.queue.quit(next.thread, next.wparam));
                gui::wake();
                return Answer::Cancelled;
            }
            Next::Message(next) => handle_message(window, &next),
        }
    }
}

fn handle_message(window: usize, next: &Message) {
    if next.message == message::KEY_DOWN
        && next.wparam == VK_BACK
        && next.window == child(window, ID_ENTRIES)
    {
        go_up(window);
        return;
    }
    if !dialog::dialog_message(window, next) {
        translate(next);
        dispatch(next);
    }
}

fn filter_of(
    request: &OpenFileName,
    description: &mut [u16],
    pattern: &mut [u8],
) -> (usize, usize) {
    if request.filter.is_null() {
        return (0, 0);
    }
    let mut filters = [0u16; FILTER_UNITS];
    let mut length = 0;
    while length + 1 < FILTER_UNITS {
        // SAFETY: `lpstrFilter` is pairs of NUL-terminated strings ended by an empty one; the
        // read stops at that empty string (two NULs in a row) or at the local's end.
        let unit = unsafe { request.filter.wrapping_add(length).read() };
        filters[length] = unit;
        length += 1;
        if unit == 0 && length >= 2 && filters[length - 2] == 0 {
            break;
        }
        if unit == 0 && length == 1 {
            break;
        }
    }
    let Some((shown, applied)) = picker::filter_pair(&filters[..length], request.filter_index)
    else {
        return (0, 0);
    };
    let shown_length = shown.len().min(description.len());
    description[..shown_length].copy_from_slice(&filters[shown.start..shown.start + shown_length]);
    let (written, _) = text::write_utf8(&filters[applied], pattern);
    (shown_length, written)
}

fn initial_folder(request: &OpenFileName, out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let mut working = [0u8; PATH_CAPACITY];
    let working_length = working_directory(&mut working)?;
    if request.initial_dir.is_null() {
        out[..working_length].copy_from_slice(&working[..working_length]);
        return Some(working_length);
    }
    // SAFETY: `lpstrInitialDir` is a NUL-terminated string.
    let typed = unsafe { super::file::wide_string(request.initial_dir) };
    let mut eggos = [0u8; PATH_CAPACITY];
    let length = text::eggos_path(text::utf16_chars(typed), &mut eggos)? - 1;
    let length = text::absolute_path(&working[..working_length], &eggos[..length], out)?;
    match is_folder(&out[..length]) {
        Some(true) => Some(length),
        _ => {
            out[..working_length].copy_from_slice(&working[..working_length]);
            Some(working_length)
        }
    }
}

fn new_state(request: &OpenFileName, kind: PickerKind) -> Option<*mut PickerState> {
    let state = heap::allocate(core::mem::size_of::<PickerState>()).cast::<PickerState>();
    if state.is_null() {
        return None;
    }
    // SAFETY: a fresh, zeroed, 16-byte aligned heap block for one `PickerState`; every field is
    // set below before the block is used, and zeros are valid integers and arrays meanwhile.
    let block = unsafe { &mut *state };
    block.kind = kind;
    block.flags = request.flags;
    block.answer = Answer::Waiting;
    block.chosen_length = 0;
    block.folder_length = initial_folder(request, &mut block.folder).unwrap_or(1);
    if block.folder_length == 1 {
        block.folder[0] = b'/';
    }
    let mut description = [0u16; 128];
    let (_, pattern_length) = filter_of(request, &mut description, &mut block.pattern);
    block.pattern_length = pattern_length;
    block.extension_length = 0;
    if !request.default_extension.is_null() {
        // SAFETY: `lpstrDefExt` is a NUL-terminated string.
        let extension = unsafe { super::file::wide_string(request.default_extension) };
        let (written, _) = text::write_utf8(extension, &mut block.extension);
        block.extension_length = written;
    }
    Some(state)
}

fn build(window: usize, kind: PickerKind, filter_text: &[u16]) -> Option<()> {
    let mut label = [0u16; 32];
    let controls = [
        (
            ID_UP,
            ControlKind::Button,
            WS_TABSTOP,
            layout::UP,
            layout::UP_LABEL,
        ),
        (
            ID_FOLDER,
            ControlKind::Edit,
            ES_READONLY,
            layout::FOLDER,
            "",
        ),
        (
            ID_ENTRIES,
            ControlKind::ListBox,
            WS_TABSTOP | LBS_NOTIFY,
            layout::ENTRIES,
            "",
        ),
        (
            ID_NAME_LABEL,
            ControlKind::Static,
            0,
            layout::NAME_LABEL,
            layout::NAME_LABEL_TEXT,
        ),
        (ID_NAME, ControlKind::Edit, WS_TABSTOP, layout::NAME, ""),
        (
            ID_FILTER,
            ControlKind::Static,
            WS_DISABLED,
            layout::FILTER,
            "",
        ),
        (
            message::ID_CANCEL,
            ControlKind::Button,
            WS_TABSTOP,
            layout::CANCEL,
            layout::CANCEL_LABEL,
        ),
        (
            message::ID_OK,
            ControlKind::Button,
            WS_TABSTOP | BS_DEFPUSHBUTTON,
            layout::ACCEPT,
            kind.accept_label(),
        ),
    ];
    for (id, control, style, rect, text) in controls {
        let length = units_of(text, &mut label);
        let text = match id {
            ID_FILTER => filter_text,
            _ => &label[..length],
        };
        create_child(window, control, WS_CHILD_VISIBLE | style, rect, id, text)?;
    }
    Some(())
}

fn owner_root(owner: usize) -> Option<(usize, u32)> {
    with(|gui| {
        let root = crate::gui::windows::root_of(&gui.windows, owner)?;
        let window = gui.windows.get(root)?;
        let shown = window.committed && matches!(window.procedure, Procedure::Program(_));
        shown.then_some((root, window.server))
    })
    .flatten()
}

fn set_modal(root: Option<(usize, u32)>, disabled: bool) {
    if let Some((root, _)) = root {
        with(|gui| {
            if disabled {
                super::menu::close(gui, root);
            }
            if let Some(window) = gui.windows.get_mut(root) {
                window.modal_disabled = disabled;
            }
        });
    }
}

fn show(
    request: &OpenFileName,
    kind: PickerKind,
) -> Result<Option<(usize, [u8; PATH_CAPACITY])>, u32> {
    let device = device().map_err(|_| CDERR_INITIALIZATION)?;
    let state = new_state(request, kind).ok_or(CDERR_INITIALIZATION)?;
    let owner = owner_root(request.owner);
    let (role, parent, flags) = match owner {
        Some((_, server)) => (ROLE_DIALOG, server, 0),
        None => (ROLE_NORMAL, 0, FIXED_SIZE),
    };
    let (width, height) = layout::CONTENT;
    let buffer = match device.create(role, parent, width as u32, height as u32, flags) {
        Ok(buffer) => buffer,
        Err(errno) => {
            // SAFETY: the state was never published.
            unsafe { heap::free(state.cast()) };
            return Err(error_for(errno));
        }
    };
    let mut title = [0u8; TITLE_BYTES];
    let title_text = match request.title.is_null() {
        true => kind.title(),
        false => {
            // SAFETY: `lpstrTitle` is a NUL-terminated string.
            let units = unsafe { super::file::wide_string(request.title) };
            let (length, _) = text::write_utf8(units, &mut title);
            core::str::from_utf8(&title[..length]).unwrap_or(kind.title())
        }
    };
    let _ = device.set_title(buffer.window, title_text);
    if owner.is_none() {
        let work = device.work_area;
        let _ = device.set_position(
            buffer.window,
            work.width().saturating_sub(width + CHROME.0) / 2,
            work.height().saturating_sub(height + CHROME.1) / 2,
        );
    }
    let mut window = Window::new(
        Procedure::FilePicker(state as usize),
        buffer.window,
        buffer.pixels,
        buffer.width,
        buffer.height,
    );
    window.shown = true;
    window.owner = request.owner;
    window.thread = thread_environment::thread_id();
    window.invalidate(None, true);
    let Some(Some(handle)) = with(|gui| gui.windows.insert(window)) else {
        // SAFETY: the window never reached the table, so nothing holds its buffer.
        unsafe { device.destroy(&buffer) };
        // SAFETY: the state was never published.
        unsafe { heap::free(state.cast()) };
        return Err(CDERR_INITIALIZATION);
    };
    let mut description = [0u16; 128];
    let (description_length, _) = filter_of(request, &mut description, &mut [0u8; PATTERN_BYTES]);
    let built = build(handle, kind, &description[..description_length]);
    set_modal(owner, true);
    let mut chosen = None;
    if built.is_some() {
        show_folder(handle);
        if kind == PickerKind::Save && !request.file.is_null() {
            // SAFETY: `lpstrFile` is a NUL-terminated string (empty for none).
            let given = unsafe { super::file::wide_string(request.file) };
            let name = given
                .iter()
                .rposition(|unit| *unit == u16::from(b'\\') || *unit == u16::from(b'/'))
                .map_or(given, |at| &given[at + 1..]);
            set_text(child(handle, ID_NAME), name);
            caret_to_end(child(handle, ID_NAME));
        }
        set_focus(child(handle, ID_NAME));
        gui::wake();
        if run(handle) == Answer::Chosen {
            chosen = with_state(handle, |state| (state.chosen_length, state.chosen));
        }
    }
    set_modal(owner, false);
    destroy_tree(handle);
    // SAFETY: the picker's window is out of the table, so no thread reaches the state any
    // more, and every earlier access was under the lock `destroy_tree` took after it.
    unsafe { heap::free(state.cast()) };
    Ok(chosen)
}

unsafe fn write_result(request: &mut OpenFileName, path: &[u8]) -> Result<(), u32> {
    let mut windows_form = [0u16; PATH_CAPACITY];
    let length = write_windows_path(path, &mut windows_form).map_err(|_| FNERR_BUFFERTOOSMALL)?;
    let full = &windows_form[..length];
    if request.file.is_null() || (request.max_file as usize) < length + 1 {
        if !request.file.is_null() && request.max_file >= 1 {
            // SAFETY: `lpstrFile` holds `nMaxFile` writable units, at least one.
            unsafe { *request.file = (length + 1).min(u16::MAX as usize) as u16 };
        }
        return Err(FNERR_BUFFERTOOSMALL);
    }
    // SAFETY: `lpstrFile` holds `nMaxFile` writable units, at least `length + 1`.
    unsafe {
        let out = core::slice::from_raw_parts_mut(request.file, length + 1);
        out[..length].copy_from_slice(full);
        out[length] = 0;
    }
    let (offset, extension) = picker::offsets(full);
    request.file_offset = offset;
    request.file_extension = extension;
    if !request.file_title.is_null() && request.max_file_title > 0 {
        let name = &full[usize::from(offset)..];
        // SAFETY: `lpstrFileTitle` holds `nMaxFileTitle` writable units.
        unsafe {
            super::window_text::copy_out(
                name,
                request.file_title as usize,
                request.max_file_title as usize,
            )
        };
    }
    Ok(())
}

unsafe fn pick(request: *mut OpenFileName, kind: PickerKind) -> Bool {
    LAST_ERROR.store(0, Ordering::Relaxed);
    if request.is_null() {
        LAST_ERROR.store(CDERR_INITIALIZATION, Ordering::Relaxed);
        return FALSE;
    }
    // SAFETY: every `OPENFILENAMEW` starts with its size, a readable `u32`.
    let size = unsafe { request.cast::<u32>().read_unaligned() };
    if size != OPEN_FILE_NAME_SIZE && size != OPEN_FILE_NAME_SIZE_400 {
        LAST_ERROR.store(CDERR_STRUCTSIZE, Ordering::Relaxed);
        return FALSE;
    }
    let mut copy = core::mem::MaybeUninit::<OpenFileName>::zeroed();
    // SAFETY: the caller's structure holds `size` readable bytes, at most the local's size, and
    // a zeroed `OpenFileName` is valid (integers and null pointers) for the bytes not copied.
    unsafe {
        core::ptr::copy_nonoverlapping(
            request.cast::<u8>(),
            copy.as_mut_ptr().cast::<u8>(),
            size as usize,
        )
    };
    // SAFETY: as above: every byte of the local is now a valid field value.
    let mut local = unsafe { copy.assume_init() };
    let outcome = show(&local, kind).and_then(|chosen| match chosen {
        // SAFETY: the request's buffers are the caller's, as `OPENFILENAMEW` describes them.
        Some((length, path)) => unsafe { write_result(&mut local, &path[..length]) }.map(|_| true),
        None => Ok(false),
    });
    let base = request.cast::<u8>();
    let offset_field = base
        .wrapping_add(core::mem::offset_of!(OpenFileName, file_offset))
        .cast::<u16>();
    let extension_field = base
        .wrapping_add(core::mem::offset_of!(OpenFileName, file_extension))
        .cast::<u16>();
    // SAFETY: the field lies inside the version-400 structure the caller's `size` covers; only
    // the two fields the call sets are written back.
    unsafe { offset_field.write_unaligned(local.file_offset) };
    // SAFETY: as above.
    unsafe { extension_field.write_unaligned(local.file_extension) };
    match outcome {
        Ok(chosen) => Bool::from(chosen),
        Err(code) => {
            LAST_ERROR.store(code, Ordering::Relaxed);
            FALSE
        }
    }
}

unsafe extern "efiapi" fn get_open_file_name_w(request: *mut OpenFileName) -> Bool {
    // SAFETY: the caller's duty, passed on.
    unsafe { pick(request, PickerKind::Open) }
}

unsafe extern "efiapi" fn get_save_file_name_w(request: *mut OpenFileName) -> Bool {
    // SAFETY: the caller's duty, passed on.
    unsafe { pick(request, PickerKind::Save) }
}

extern "efiapi" fn comm_dlg_extended_error() -> u32 {
    LAST_ERROR.load(Ordering::Relaxed)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetOpenFileNameW" => get_open_file_name_w,
    b"GetSaveFileNameW" => get_save_file_name_w,
    b"CommDlgExtendedError" => comm_dlg_extended_error,
];

const _: () = assert!(core::mem::size_of::<OpenFileName>() == OPEN_FILE_NAME_SIZE as usize);
const _: () =
    assert!(core::mem::offset_of!(OpenFileName, reserved) == OPEN_FILE_NAME_SIZE_400 as usize);
