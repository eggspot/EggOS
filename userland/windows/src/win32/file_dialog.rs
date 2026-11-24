//! The common item dialogs (D742): `FileOpenDialog` and `FileSaveDialog`, whose `IFileDialog`
//! keeps what the program sets (title, OK label, file name, default extension, file types and
//! the chosen one, options, folder and default folder, a save-as item) and whose `Show` runs
//! the system picker (`/bin/pick open|save|folder`, D392) as the file dialogs and the folder
//! browser do: the owner disabled and the thread's messages served meanwhile, several files
//! when `FOS_ALLOWMULTISELECT` is set, the layer's own picker when `pick` cannot run. The
//! chosen items come back as `IShellItem`s (`GetResult`) and an `IShellItemArray`
//! (`GetResults`); an advised `IFileDialogEvents` is asked `OnFileOk` and can send the user
//! back to the picker. Customization, places, property stores and filters of items are
//! accepted and not shown.

use super::file_picker::{self, Fallback};
use super::shell_item_array::new_array;
use super::shell_namespace::{
    clone_list, file_system_path, list_bytes, list_of_new_path, list_of_object, new_shell_item,
    task_text,
};
use super::{heap, pick_process};
use crate::com::guid::{self, Guid};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK};
use crate::text::{self, PATH_CAPACITY};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

/// `CLSID_FileOpenDialog`.
pub(super) const FILE_OPEN_DIALOG: Guid = Guid::new(
    0xdc1c_5a9c,
    0xe88a,
    0x4dde,
    [0xa5, 0xa1, 0x60, 0xf8, 0x2a, 0x20, 0xae, 0xf7],
);
/// `CLSID_FileSaveDialog`.
pub(super) const FILE_SAVE_DIALOG: Guid = Guid::new(
    0xc0b4_e2f3,
    0xba21,
    0x4773,
    [0x8d, 0xba, 0x33, 0x5e, 0xc9, 0x46, 0xeb, 0x8b],
);
const MODAL_WINDOW: Guid = Guid::new(
    0xb4db_1657,
    0x70d7,
    0x485e,
    [0x8e, 0x3e, 0x6f, 0xcb, 0x5a, 0x5c, 0x18, 0x02],
);
const FILE_DIALOG: Guid = Guid::new(
    0x42f8_5136,
    0xdb7e,
    0x439c,
    [0x85, 0xf1, 0xe4, 0x07, 0x5d, 0x13, 0x5f, 0xc8],
);
const OPEN_DIALOG_INTERFACE: Guid = Guid::new(
    0xd57c_7288,
    0xd4ad,
    0x4768,
    [0xbe, 0x02, 0x9d, 0x96, 0x95, 0x32, 0xd9, 0x60],
);
const SAVE_DIALOG_INTERFACE: Guid = Guid::new(
    0x84bc_cd23,
    0x5fde,
    0x4cdb,
    [0xae, 0xa4, 0xaf, 0x64, 0xb8, 0x3d, 0x78, 0xab],
);

const CANCELLED: HResult = 0x8007_04c7_u32 as i32;
const E_UNEXPECTED: HResult = 0x8000_ffff_u32 as i32;
const E_FAIL: HResult = 0x8000_4005_u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110_u32 as i32;
const FOS_OVERWRITEPROMPT: u32 = 0x2;
const FOS_NOCHANGEDIR: u32 = 0x8;
const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_ALLOWMULTISELECT: u32 = 0x200;
const FOS_PATHMUSTEXIST: u32 = 0x800;
const FOS_FILEMUSTEXIST: u32 = 0x1000;
const FOS_NOREADONLYRETURN: u32 = 0x8000;
const OPEN_DEFAULTS: u32 = FOS_PATHMUSTEXIST | FOS_FILEMUSTEXIST | FOS_NOCHANGEDIR;
const SAVE_DEFAULTS: u32 =
    FOS_OVERWRITEPROMPT | FOS_NOREADONLYRETURN | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR;
const ON_FILE_OK: usize = 3;
const MOST_FILE_OK_ROUNDS: usize = 8;
const MOST_RESULTS: usize = 256;
const OUTPUT_BYTES: usize = 64 * 1024;
const TEXT_BYTES: usize = 1024;
const HOME: &[u8] = b"/home/root";

#[derive(Debug)]
#[repr(transparent)]
struct Table<const N: usize>([*const (); N]);

// SAFETY: the table holds functions' addresses, fixed for the program's life.
unsafe impl<const N: usize> Sync for Table<N> {}

impl<const N: usize> Table<N> {
    fn address(&'static self) -> usize {
        self.0.as_ptr() as usize
    }
}

#[repr(C)]
#[derive(Debug)]
struct Dialog {
    table: usize,
    references: AtomicU32,
    save: bool,
    options: u32,
    file_type: u32,
    title: Vec<u16>,
    ok_label: Vec<u16>,
    file_name: Vec<u16>,
    default_extension: Vec<u16>,
    file_types: Vec<(Vec<u16>, Vec<u16>)>,
    folder: *mut u8,
    default_folder: *mut u8,
    save_as: *mut u8,
    events: usize,
    results: Vec<*mut u8>,
}

unsafe fn dialog<'a>(this: *mut Dialog) -> &'a mut Dialog {
    // SAFETY: the caller's duty: a live dialog, used by one thread at a time.
    unsafe { &mut *this }
}

fn free_list(slot: &mut *mut u8) {
    // SAFETY: a list the dialog owns, freed once and forgotten.
    unsafe { heap::free(*slot) };
    *slot = core::ptr::null_mut();
}

fn clear_results(found: &mut Dialog) {
    for mut list in found.results.drain(..) {
        free_list(&mut list);
    }
}

fn text_of(text: *const u16) -> Vec<u16> {
    match text.is_null() {
        true => Vec::new(),
        // SAFETY: the program's NUL-terminated text.
        false => unsafe { super::file::wide_string(text) }.to_vec(),
    }
}

fn terminated(units: &[u16]) -> Vec<u16> {
    let mut copy = Vec::with_capacity(units.len() + 1);
    copy.extend_from_slice(units);
    copy.push(0);
    copy
}

fn utf8(units: &[u16], out: &mut [u8]) -> usize {
    text::write_utf8(units, out).0
}

/// Creates the dialog `class` if it is one of these, answering `interface` through `result`.
///
/// # Safety
/// `interface` must point at a GUID and `result` be writable.
pub(super) unsafe fn create_instance(
    class: &Guid,
    outer: usize,
    interface: *const Guid,
    result: *mut usize,
) -> Option<HResult> {
    let save = if *class == FILE_OPEN_DIALOG {
        false
    } else if *class == FILE_SAVE_DIALOG {
        true
    } else {
        return None;
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let object = crate::com::allocate(core::mem::size_of::<Dialog>()).cast::<Dialog>();
    if object.is_null() {
        return Some(E_OUTOFMEMORY);
    }
    let table = match save {
        true => SAVE_TABLE.address(),
        false => OPEN_TABLE.address(),
    };
    // SAFETY: a fresh block, sized and aligned for a dialog.
    unsafe {
        object.write(Dialog {
            table,
            references: AtomicU32::new(1),
            save,
            options: if save { SAVE_DEFAULTS } else { OPEN_DEFAULTS },
            file_type: 1,
            title: Vec::new(),
            ok_label: Vec::new(),
            file_name: Vec::new(),
            default_extension: Vec::new(),
            file_types: Vec::new(),
            folder: core::ptr::null_mut(),
            default_folder: core::ptr::null_mut(),
            save_as: core::ptr::null_mut(),
            events: 0,
            results: Vec::new(),
        })
    };
    // SAFETY: the dialog just made; the caller's interface and result.
    let answer = unsafe { query_interface(object, interface, result) };
    // SAFETY: the creation's reference goes; the caller's, if any, keeps it.
    unsafe { release(object) };
    Some(answer)
}

unsafe extern "efiapi" fn query_interface(
    this: *mut Dialog,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    let own = match found.save {
        true => SAVE_DIALOG_INTERFACE,
        false => OPEN_DIALOG_INTERFACE,
    };
    let interfaces = [guid::UNKNOWN, MODAL_WINDOW, FILE_DIALOG, own];
    let references = &found.references;
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn add_reference(this: *mut Dialog) -> u32 {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "efiapi" fn release(this: *mut Dialog) -> u32 {
    // SAFETY: the caller's duty: a live dialog it holds a reference to.
    let found = unsafe { dialog(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    clear_results(found);
    free_list(&mut found.folder);
    free_list(&mut found.default_folder);
    free_list(&mut found.save_as);
    // SAFETY: the advised events object, whose reference the dialog held.
    unsafe { crate::com::release(found.events) };
    // SAFETY: the dialog's fields, then its block; no reference is left.
    unsafe {
        core::ptr::drop_in_place(this);
        crate::com::free(this.cast());
    }
    0
}

fn push_text(arguments: &mut pick_process::Arguments, option: &[u8], units: &[u16]) {
    if units.is_empty() {
        return;
    }
    let mut bytes = [0u8; TEXT_BYTES];
    let length = utf8(units, &mut bytes);
    let _ = arguments.option(option, &bytes[..length]);
}

fn push_number(arguments: &mut pick_process::Arguments, option: &[u8], value: u32) {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut rest = value;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let _ = arguments.option(option, &digits[at..]);
}

fn list_path(list: *mut u8, out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    // SAFETY: a list the dialog owns, or null.
    let bytes = unsafe { list_bytes(list) }?;
    file_system_path(bytes, out)
}

fn split_path(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => (&path[..1], &path[1..]),
        Some(at) => (&path[..at], &path[at + 1..]),
        None => (&[], path),
    }
}

fn start_folder(found: &Dialog, out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    if let Some(length) = list_path(found.folder, out) {
        return Some(length);
    }
    let mut saved = [0u8; PATH_CAPACITY];
    if let Some(length) = list_path(found.save_as, &mut saved) {
        let (folder, _) = split_path(&saved[..length]);
        out[..folder.len()].copy_from_slice(folder);
        return Some(folder.len());
    }
    list_path(found.default_folder, out)
}

fn suggested_name(found: &Dialog, out: &mut [u8]) -> usize {
    if !found.file_name.is_empty() {
        return utf8(&found.file_name, out);
    }
    let mut saved = [0u8; PATH_CAPACITY];
    match list_path(found.save_as, &mut saved) {
        Some(length) => {
            let (_, name) = split_path(&saved[..length]);
            let taken = name.len().min(out.len());
            out[..taken].copy_from_slice(&name[..taken]);
            taken
        }
        None => 0,
    }
}

fn arguments_for(found: &Dialog) -> pick_process::Arguments {
    let folders = found.options & FOS_PICKFOLDERS != 0;
    let mode: &[u8] = match (folders, found.save) {
        (true, _) => b"folder",
        (false, true) => b"save",
        (false, false) => b"open",
    };
    let mut arguments = pick_process::Arguments::new(mode);
    push_text(&mut arguments, b"--title", &found.title);
    push_text(&mut arguments, b"--button", &found.ok_label);
    let mut folder = [0u8; PATH_CAPACITY];
    if let Some(length) = start_folder(found, &mut folder) {
        let _ = arguments.option(b"--folder", &folder[..length]);
    }
    if folders {
        return arguments;
    }
    let mut any = false;
    for (name, spec) in &found.file_types {
        let mut joined = [0u8; TEXT_BYTES * 2];
        let written = utf8(name, &mut joined[..TEXT_BYTES]);
        joined[written] = b'|';
        let more = utf8(spec, &mut joined[written + 1..]);
        any |= arguments.option(b"--filter", &joined[..written + 1 + more]);
    }
    if any && found.file_type > 0 {
        push_number(&mut arguments, b"--filter-index", found.file_type);
    }
    match found.save {
        true => {
            let mut name = [0u8; TEXT_BYTES];
            let length = suggested_name(found, &mut name);
            if length > 0 {
                let _ = arguments.option(b"--name", &name[..length]);
            }
            push_text(&mut arguments, b"--extension", &found.default_extension);
            if found.options & FOS_OVERWRITEPROMPT == 0 {
                let _ = arguments.push(b"--overwrite");
            }
        }
        false => {
            if found.options & FOS_ALLOWMULTISELECT != 0 {
                let _ = arguments.push(b"--multiple");
            }
        }
    }
    arguments
}

fn chosen_lists(found: &Dialog, output: &[u8]) -> Vec<*mut u8> {
    let mut lists = Vec::new();
    for line in output.split(|&byte| byte == b'\n') {
        let usable = line.starts_with(b"/") && line.len() < PATH_CAPACITY;
        if !usable || lists.len() == MOST_RESULTS {
            continue;
        }
        if let Ok(list) = list_of_new_path(line) {
            lists.push(list);
        }
        if !found.save && found.options & FOS_ALLOWMULTISELECT == 0 {
            break;
        }
    }
    lists
}

fn fallback(found: &Dialog, owner: usize) -> Result<Vec<*mut u8>, u32> {
    let title = terminated(&found.title);
    let extension = terminated(&found.default_extension);
    let mut folder_bytes = [0u8; PATH_CAPACITY];
    let mut folder_units = [0u16; PATH_CAPACITY];
    let folder_length = start_folder(found, &mut folder_bytes)
        .and_then(|length| {
            super::directory::write_windows_path(&folder_bytes[..length], &mut folder_units).ok()
        })
        .unwrap_or(0);
    let folder = terminated(&folder_units[..folder_length]);
    let mut name_bytes = [0u8; TEXT_BYTES];
    let name_length = suggested_name(found, &mut name_bytes);
    let mut name_units = [0u16; TEXT_BYTES];
    let name_written = text::write_utf16(&name_bytes[..name_length], &mut name_units);
    let name = terminated(&name_units[..name_written]);
    let mut filter = Vec::new();
    for (description, spec) in &found.file_types {
        filter.extend_from_slice(description);
        filter.push(0);
        filter.extend_from_slice(spec);
        filter.push(0);
    }
    filter.push(0);
    filter.push(0);
    let request = Fallback {
        save: found.save,
        owner,
        title: if found.title.is_empty() { core::ptr::null() } else { title.as_ptr() },
        folder: if folder_length == 0 { core::ptr::null() } else { folder.as_ptr() },
        filter: if found.file_types.is_empty() { core::ptr::null() } else { filter.as_ptr() },
        filter_index: found.file_type,
        default_extension: match found.default_extension.is_empty() {
            true => core::ptr::null(),
            false => extension.as_ptr(),
        },
        name: name.as_ptr(),
        overwrite_prompt: found.options & FOS_OVERWRITEPROMPT != 0,
    };
    Ok(match file_picker::layer_picker(&request)? {
        Some((length, path)) => list_of_new_path(&path[..length]).into_iter().collect(),
        None => Vec::new(),
    })
}

fn pick(found: &Dialog, owner: usize) -> Result<Vec<*mut u8>, u32> {
    let arguments = arguments_for(found);
    let modal = file_picker::owner_root(owner);
    file_picker::set_modal(modal, true);
    let mut output = alloc::vec![0u8; OUTPUT_BYTES];
    let picked = pick_process::run(&arguments, &mut output);
    file_picker::set_modal(modal, false);
    match picked {
        Ok(pick_process::Picked::Cancelled) => Ok(Vec::new()),
        Ok(pick_process::Picked::Chosen(length)) => Ok(chosen_lists(found, &output[..length])),
        Err(_) if found.options & FOS_PICKFOLDERS == 0 => fallback(found, owner),
        Err(code) => Err(code),
    }
}

unsafe extern "efiapi" fn show(this: *mut Dialog, owner: usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    for _ in 0..MOST_FILE_OK_ROUNDS {
        clear_results(found);
        let chosen = match pick(found, owner) {
            Ok(chosen) => chosen,
            Err(code) => return super::shell_folders::from_win32(code),
        };
        if chosen.is_empty() {
            return CANCELLED;
        }
        found.results = chosen;
        if found.events == 0 {
            return S_OK;
        }
        let dialog_pointer = [this as usize, 0, 0];
        // SAFETY: the advised `IFileDialogEvents`, whose `OnFileOk` takes the dialog.
        let answer = unsafe { crate::com::call_method(found.events, ON_FILE_OK, dialog_pointer) };
        if answer as u32 == 0 {
            return S_OK;
        }
    }
    clear_results(found);
    CANCELLED
}

unsafe extern "efiapi" fn set_file_types(
    this: *mut Dialog,
    count: u32,
    specs: *const [*const u16; 2],
) -> HResult {
    if specs.is_null() && count != 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    found.file_types.clear();
    for index in 0..count as usize {
        // SAFETY: `count` readable `COMDLG_FILTERSPEC`s.
        let [name, spec] = unsafe { specs.add(index).read_unaligned() };
        found.file_types.push((text_of(name), text_of(spec)));
    }
    S_OK
}

unsafe extern "efiapi" fn set_file_type_index(this: *mut Dialog, index: u32) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.file_type = index.max(1);
    S_OK
}

unsafe extern "efiapi" fn get_file_type_index(this: *mut Dialog, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live dialog and a writable index.
    unsafe { out.write(dialog(this).file_type) };
    S_OK
}

unsafe extern "efiapi" fn advise(this: *mut Dialog, events: usize, cookie: *mut u32) -> HResult {
    if events == 0 || cookie.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: a live dialog and a live events object.
    unsafe {
        let found = dialog(this);
        crate::com::add_reference(events);
        crate::com::release(found.events);
        found.events = events;
        cookie.write(1);
    }
    S_OK
}

unsafe extern "efiapi" fn unadvise(this: *mut Dialog, _cookie: u32) -> HResult {
    // SAFETY: the caller's duty: a live dialog; the events object's reference is its own.
    unsafe {
        let found = dialog(this);
        crate::com::release(found.events);
        found.events = 0;
    }
    S_OK
}

unsafe extern "efiapi" fn set_options(this: *mut Dialog, options: u32) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.options = options;
    S_OK
}

unsafe extern "efiapi" fn get_options(this: *mut Dialog, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live dialog and writable options.
    unsafe { out.write(dialog(this).options) };
    S_OK
}

fn replace_with_item(slot: &mut *mut u8, item: usize) -> HResult {
    // SAFETY: the program's shell item, or null.
    let Some(list) = (unsafe { list_of_object(item) }) else {
        return E_INVALIDARG;
    };
    if list.is_null() {
        return E_OUTOFMEMORY;
    }
    free_list(slot);
    *slot = list;
    S_OK
}

unsafe extern "efiapi" fn set_default_folder(this: *mut Dialog, item: usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    replace_with_item(&mut unsafe { dialog(this) }.default_folder, item)
}

unsafe extern "efiapi" fn set_folder(this: *mut Dialog, item: usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    replace_with_item(&mut unsafe { dialog(this) }.folder, item)
}

unsafe fn hand_out_item(list: *mut u8, out: *mut usize) -> HResult {
    if out.is_null() {
        // SAFETY: a list made for the item, unused.
        unsafe { heap::free(list) };
        return E_POINTER;
    }
    let made = new_shell_item(list);
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

fn copy_of(list: *mut u8) -> Option<*mut u8> {
    // SAFETY: a list the dialog owns, or null.
    unsafe { list_bytes(list) }.map(clone_list)
}

unsafe extern "efiapi" fn get_folder(this: *mut Dialog, out: *mut usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    let list = copy_of(found.folder)
        .or_else(|| copy_of(found.default_folder))
        .or_else(|| list_of_new_path(HOME).ok());
    match list {
        // SAFETY: the caller's interface pointer.
        Some(list) => unsafe { hand_out_item(list, out) },
        None => E_FAIL,
    }
}

unsafe extern "efiapi" fn get_current_selection(this: *mut Dialog, out: *mut usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    let first = unsafe { dialog(this) }.results.first().copied();
    match first.and_then(copy_of) {
        // SAFETY: the caller's interface pointer.
        Some(list) => unsafe { hand_out_item(list, out) },
        None => E_FAIL,
    }
}

unsafe extern "efiapi" fn set_file_name(this: *mut Dialog, name: *const u16) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.file_name = text_of(name);
    S_OK
}

unsafe extern "efiapi" fn get_file_name(this: *mut Dialog, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    let mut units = [0u16; PATH_CAPACITY];
    let mut path = [0u8; PATH_CAPACITY];
    let length = match found.results.first().and_then(|&list| list_path(list, &mut path)) {
        Some(length) => text::write_utf16(split_path(&path[..length]).1, &mut units),
        None => {
            let taken = found.file_name.len().min(PATH_CAPACITY);
            units[..taken].copy_from_slice(&found.file_name[..taken]);
            taken
        }
    };
    let block = task_text(&units[..length]);
    // SAFETY: the caller passes a writable string pointer.
    unsafe { out.write(block) };
    match block.is_null() {
        true => E_OUTOFMEMORY,
        false => S_OK,
    }
}

unsafe extern "efiapi" fn set_title(this: *mut Dialog, title: *const u16) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.title = text_of(title);
    S_OK
}

unsafe extern "efiapi" fn set_ok_button_label(this: *mut Dialog, label: *const u16) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    unsafe { dialog(this) }.ok_label = text_of(label);
    S_OK
}

unsafe extern "efiapi" fn get_result(this: *mut Dialog, out: *mut usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    let first = unsafe { dialog(this) }.results.first().copied();
    match first.and_then(copy_of) {
        // SAFETY: the caller's interface pointer.
        Some(list) => unsafe { hand_out_item(list, out) },
        None => E_UNEXPECTED,
    }
}

unsafe extern "efiapi" fn set_default_extension(
    this: *mut Dialog,
    extension: *const u16,
) -> HResult {
    let mut units = text_of(extension);
    if units.first() == Some(&u16::from(b'.')) {
        units.remove(0);
    }
    // SAFETY: as above.
    unsafe { dialog(this) }.default_extension = units;
    S_OK
}

unsafe extern "efiapi" fn accepted(_this: *mut Dialog) -> HResult {
    S_OK
}

unsafe extern "efiapi" fn get_results(this: *mut Dialog, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live dialog.
    let found = unsafe { dialog(this) };
    if found.results.is_empty() {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return E_UNEXPECTED;
    }
    let copies: Vec<*mut u8> = found
        .results
        .iter()
        .map(|&list| copy_of(list).unwrap_or(core::ptr::null_mut()))
        .collect();
    let made = new_array(&copies);
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

unsafe extern "efiapi" fn set_save_as_item(this: *mut Dialog, item: usize) -> HResult {
    // SAFETY: the caller's duty: a live dialog.
    replace_with_item(&mut unsafe { dialog(this) }.save_as, item)
}

unsafe extern "efiapi" fn get_properties(_this: *mut Dialog, out: *mut usize) -> HResult {
    if !out.is_null() {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
    }
    E_UNEXPECTED
}

static OPEN_TABLE: Table<29> = Table([
    export_entry!(query_interface),
    export_entry!(add_reference),
    export_entry!(release),
    export_entry!(show),
    export_entry!(set_file_types),
    export_entry!(set_file_type_index),
    export_entry!(get_file_type_index),
    export_entry!(advise),
    export_entry!(unadvise),
    export_entry!(set_options),
    export_entry!(get_options),
    export_entry!(set_default_folder),
    export_entry!(set_folder),
    export_entry!(get_folder),
    export_entry!(get_current_selection),
    export_entry!(set_file_name),
    export_entry!(get_file_name),
    export_entry!(set_title),
    export_entry!(set_ok_button_label),
    export_entry!(accepted),
    export_entry!(get_result),
    export_entry!(accepted),
    export_entry!(set_default_extension),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(get_results),
    export_entry!(get_results),
]);

static SAVE_TABLE: Table<32> = Table([
    export_entry!(query_interface),
    export_entry!(add_reference),
    export_entry!(release),
    export_entry!(show),
    export_entry!(set_file_types),
    export_entry!(set_file_type_index),
    export_entry!(get_file_type_index),
    export_entry!(advise),
    export_entry!(unadvise),
    export_entry!(set_options),
    export_entry!(get_options),
    export_entry!(set_default_folder),
    export_entry!(set_folder),
    export_entry!(get_folder),
    export_entry!(get_current_selection),
    export_entry!(set_file_name),
    export_entry!(get_file_name),
    export_entry!(set_title),
    export_entry!(set_ok_button_label),
    export_entry!(accepted),
    export_entry!(get_result),
    export_entry!(accepted),
    export_entry!(set_default_extension),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(set_save_as_item),
    export_entry!(accepted),
    export_entry!(accepted),
    export_entry!(get_properties),
    export_entry!(accepted),
]);
