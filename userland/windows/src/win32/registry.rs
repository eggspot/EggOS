//! advapi32's registry functions (D231) over `eggos_windows_registry`: the user's hive below the
//! home folder and the machine's in `/var/lib/eggos/windows`, kept by the crate's
//! [`FileStore`], which Settings uses too. Opened keys are handles that name their key's path,
//! so a key another process deleted reads as missing; the handle table's spin lock is held only
//! to look a handle up, never across a file's reads and writes, which run on a registry taken
//! out of its slot (or a fresh one while another thread has it). The `A` functions take and
//! give ANSI text, which is UTF-8 on EggOS, string data included.

use alloc::vec::Vec;

use eggos_runtime::startup::Startup;
use eggos_windows_registry::Error as RegistryError;
use eggos_windows_registry::MAX_KEY_NAME_UNITS;
use eggos_windows_registry::command::{self, CODED_ERRORS_ENVIRONMENT, Console};
use eggos_windows_registry::linux::LinuxHiveFiles;
use eggos_windows_registry::path::{KeyPath, Root};
use eggos_windows_registry::registry::{Registry, is_machine_key};
use eggos_windows_registry::store::FileStore;
use eggos_windows_registry::value::{
    REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_NONE, REG_QWORD, REG_SZ,
};

use super::{SpinLock, error};
use crate::library::Export;
use crate::linux;
use crate::text::PATH_CAPACITY;

const ERROR_SUCCESS: u32 = 0;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_BADDB: u32 = 1009;
const ERROR_CANTREAD: u32 = 1012;
const ERROR_CANTWRITE: u32 = 1013;
const ERROR_DATATYPE_MISMATCH: u32 = 1629;
const ERROR_UNSUPPORTED_TYPE: u32 = 1630;
const FIRST_HANDLE: usize = 0x5000_0000;
const HANDLE_STEP: usize = 4;
const MAX_OPEN_KEYS: usize = 16_384;
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;
const RRF_RT_REG_NONE: u32 = 0x01;
const RRF_RT_REG_SZ: u32 = 0x02;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x04;
const RRF_RT_REG_BINARY: u32 = 0x08;
const RRF_RT_REG_DWORD: u32 = 0x10;
const RRF_RT_REG_MULTI_SZ: u32 = 0x20;
const RRF_RT_REG_QWORD: u32 = 0x40;
const RRF_RT_DWORD: u32 = RRF_RT_REG_BINARY | RRF_RT_REG_DWORD;
const RRF_RT_QWORD: u32 = RRF_RT_REG_BINARY | RRF_RT_REG_QWORD;
const RRF_RT_ANY: u32 = 0x0000_ffff;
const RRF_NOEXPAND: u32 = 0x1000_0000;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_CREATE_SUB_KEY: u32 = 0x0004;
const KEY_CREATE_LINK: u32 = 0x0020;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const WRITING_ACCESS: u32 = KEY_SET_VALUE
    | KEY_CREATE_SUB_KEY
    | KEY_CREATE_LINK
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;
const PERCENT: u16 = b'%' as u16;

type LayerStore = FileStore<LinuxHiveFiles>;

fn layer_store() -> LayerStore {
    let mut home = [0u8; PATH_CAPACITY];
    let length = super::shell_folders::home(&mut home);
    FileStore::new(LinuxHiveFiles, &home[..length])
}

static OPENED_KEYS: SpinLock<Vec<Option<KeyPath>>> = SpinLock::new(Vec::new());
static IDLE_REGISTRY: SpinLock<Option<Registry<LayerStore>>> = SpinLock::new(None);

fn with_idle<T, R>(
    idle: &SpinLock<Option<T>>,
    make: impl FnOnce() -> T,
    work: impl FnOnce(&mut T) -> R,
) -> R {
    let mut taken = idle.with(Option::take).unwrap_or_else(make);
    let result = work(&mut taken);
    drop(idle.with(|slot| slot.replace(taken)));
    result
}

fn with_registry<R>(work: impl FnOnce(&mut Registry<LayerStore>) -> R) -> R {
    with_idle(&IDLE_REGISTRY, || Registry::new(layer_store()), work)
}

struct TerminalConsole {
    coded_errors: bool,
}

impl Console for TerminalConsole {
    fn out(&mut self, bytes: &[u8]) {
        let _ = linux::write_all(1, bytes);
    }

    fn err(&mut self, bytes: &[u8]) {
        let _ = linux::write_all(2, bytes);
    }

    fn confirm(&mut self, question: &str) -> Option<bool> {
        if !linux::is_terminal(0) {
            return None;
        }
        let _ = linux::write_all(2, question.as_bytes());
        let mut answer = [0u8; 8];
        let mut length = 0;
        let mut byte = [0u8; 1];
        while let Ok(1) = linux::read(0, &mut byte) {
            if byte[0] == b'\n' {
                break;
            }
            if length < answer.len() {
                answer[length] = byte[0];
                length += 1;
            }
        }
        Some(matches!(
            &answer[..length],
            b"y" | b"Y" | b"yes" | b"Yes" | b"YES"
        ))
    }

    fn wants_coded_errors(&self) -> bool {
        self.coded_errors
    }
}

/// Runs the layer as the `windows` command (`/bin/windows registry ...`); its exit status.
pub fn run_command(startup: &Startup) -> i32 {
    let words: Option<Vec<&str>> = startup
        .arguments()
        .skip(1)
        .map(|word| word.to_str().ok())
        .collect();
    let Some(words) = words else {
        let _ = linux::write_all(2, b"windows: the arguments are not UTF-8\n");
        return 2;
    };
    let coded_errors = startup
        .environment()
        .any(|variable| variable == CODED_ERRORS_ENVIRONMENT);
    let mut registry = Registry::new(layer_store());
    command::run(&mut registry, &mut TerminalConsole { coded_errors }, &words)
}

/// The string data of the value `name` (empty for the key's default) of the key at `path`
/// (`HKEY_CLASSES_ROOT\...`), UTF-16 without its terminating NULs; `None` when there is no
/// such string.
pub(super) fn string_value(path: &str, name: &[u16]) -> Option<Vec<u16>> {
    let path = KeyPath::parse(path).ok()?;
    let value = with_registry(|registry| registry.value(&path, name)).ok()?;
    if !is_string(value.kind) {
        return None;
    }
    let mut units = units_of(&value.data);
    while units.last() == Some(&0) {
        units.pop();
    }
    Some(units)
}

fn key_of(handle: usize) -> Option<KeyPath> {
    let high = handle >> 32;
    if (high == 0 || high == 0xffff_ffff)
        && let Some(root) = Root::from_handle(handle as u32)
    {
        return Some(KeyPath::root(root));
    }
    let index = handle.checked_sub(FIRST_HANDLE)?;
    if index % HANDLE_STEP != 0 {
        return None;
    }
    OPENED_KEYS.with(|keys| keys.get(index / HANDLE_STEP).cloned().flatten())
}

fn is_predefined(handle: usize) -> bool {
    let high = handle >> 32;
    (high == 0 || high == 0xffff_ffff) && (0x8000_0000..=0x8000_0007).contains(&(handle as u32))
}

fn insert_key(path: KeyPath) -> Option<usize> {
    OPENED_KEYS.with(|keys| {
        let index = match keys.iter().position(Option::is_none) {
            Some(index) => index,
            None if keys.len() < MAX_OPEN_KEYS => {
                keys.push(None);
                keys.len() - 1
            }
            None => return None,
        };
        keys[index] = Some(path);
        Some(FIRST_HANDLE + index * HANDLE_STEP)
    })
}

fn denied_by_access(path: &KeyPath, access: u32, may_change_machine: bool) -> bool {
    access & WRITING_ACCESS != 0 && is_machine_key(path) && !may_change_machine
}

fn access_allowed(path: &KeyPath, access: u32) -> bool {
    if access & WRITING_ACCESS == 0 || !is_machine_key(path) {
        return true;
    }
    let may_change_machine = with_registry(|registry| registry.may_change_machine());
    !denied_by_access(path, access, may_change_machine)
}

fn win32_error(failure: RegistryError) -> u32 {
    match failure {
        RegistryError::NoSuchKey | RegistryError::NoSuchValue => error::FILE_NOT_FOUND,
        RegistryError::ReadOnlyKey | RegistryError::RootKey | RegistryError::HasSubkeys => {
            error::ACCESS_DENIED
        }
        RegistryError::BadName => error::INVALID_PARAMETER,
        RegistryError::BadData => error::INVALID_DATA,
        RegistryError::ValueTooLarge | RegistryError::KeyTooDeep => error::INVALID_PARAMETER,
        RegistryError::HiveFull | RegistryError::DiskFull => error::DISK_FULL,
        RegistryError::HiveDamaged => ERROR_BADDB,
        RegistryError::ReadFailed => ERROR_CANTREAD,
        RegistryError::WriteFailed => ERROR_CANTWRITE,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Text {
    Wide,
    Ansi,
}

unsafe fn name_units(name: *const u8, text: Text) -> Vec<u16> {
    if name.is_null() {
        return Vec::new();
    }
    match text {
        // SAFETY: the caller's string is NUL-terminated UTF-16.
        Text::Wide => unsafe { super::file::wide_string(name.cast()) }.to_vec(),
        Text::Ansi => {
            let mut length = 0;
            // SAFETY: the caller's string is NUL-terminated, so every byte to the NUL reads.
            while unsafe { name.wrapping_add(length).read() } != 0 {
                length += 1;
            }
            // SAFETY: as above: `length` readable bytes.
            let bytes = unsafe { core::slice::from_raw_parts(name, length) };
            crate::text::utf8_chars(bytes)
                .flat_map(|character| {
                    let mut pair = [0u16; 2];
                    let count = character.encode_utf16(&mut pair).len();
                    pair.into_iter().take(count)
                })
                .collect()
        }
    }
}

unsafe fn below(handle: usize, subkey: *const u8, text: Text) -> Result<KeyPath, u32> {
    let parent = key_of(handle).ok_or(error::INVALID_HANDLE)?;
    // SAFETY: the caller's duty.
    let units = unsafe { name_units(subkey, text) };
    parent.join(&units).map_err(win32_error)
}

unsafe fn open_key(
    handle: usize,
    subkey: *const u8,
    access: u32,
    result: *mut usize,
    text: Text,
) -> u32 {
    if result.is_null() {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: the caller's duty.
    let path = match unsafe { below(handle, subkey, text) } {
        Ok(path) => path,
        Err(code) => return code,
    };
    if !access_allowed(&path, access) {
        return error::ACCESS_DENIED;
    }
    if path.names.is_empty() && is_predefined(handle) {
        // SAFETY: the caller passes a writable `HKEY`.
        unsafe { result.write(handle) };
        return ERROR_SUCCESS;
    }
    match with_registry(|registry| registry.key_exists(&path)) {
        Ok(true) => {}
        Ok(false) => return error::FILE_NOT_FOUND,
        Err(failure) => return win32_error(failure),
    }
    match insert_key(path) {
        Some(opened) => {
            // SAFETY: as above.
            unsafe { result.write(opened) };
            ERROR_SUCCESS
        }
        None => error::NOT_ENOUGH_MEMORY,
    }
}

unsafe fn create_key(
    handle: usize,
    subkey: *const u8,
    access: u32,
    result: *mut usize,
    disposition: *mut u32,
    text: Text,
) -> u32 {
    if result.is_null() {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: the caller's duty.
    let path = match unsafe { below(handle, subkey, text) } {
        Ok(path) => path,
        Err(code) => return code,
    };
    if !access_allowed(&path, access) {
        return error::ACCESS_DENIED;
    }
    let created = match with_registry(|registry| registry.create_key(&path)) {
        Ok(created) => created,
        Err(failure) => return win32_error(failure),
    };
    if !disposition.is_null() {
        let made = match created {
            true => REG_CREATED_NEW_KEY,
            false => REG_OPENED_EXISTING_KEY,
        };
        // SAFETY: the caller passes a writable `DWORD`, or null.
        unsafe { disposition.write(made) };
    }
    if path.names.is_empty() && is_predefined(handle) {
        // SAFETY: the caller passes a writable `HKEY`.
        unsafe { result.write(handle) };
        return ERROR_SUCCESS;
    }
    match insert_key(path) {
        Some(opened) => {
            // SAFETY: as above.
            unsafe { result.write(opened) };
            ERROR_SUCCESS
        }
        None => error::NOT_ENOUGH_MEMORY,
    }
}

fn restriction(kind: u32) -> Option<u32> {
    match kind {
        REG_NONE => Some(RRF_RT_REG_NONE),
        REG_SZ => Some(RRF_RT_REG_SZ),
        REG_EXPAND_SZ => Some(RRF_RT_REG_EXPAND_SZ),
        REG_BINARY => Some(RRF_RT_REG_BINARY),
        REG_DWORD => Some(RRF_RT_REG_DWORD),
        REG_MULTI_SZ => Some(RRF_RT_REG_MULTI_SZ),
        REG_QWORD => Some(RRF_RT_REG_QWORD),
        _ => None,
    }
}

fn is_string(kind: u32) -> bool {
    matches!(kind, REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ)
}

fn units_of(data: &[u8]) -> Vec<u16> {
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect()
}

fn ansi_data(kind: u32, data: &[u8]) -> Vec<u8> {
    if !is_string(kind) {
        return data.to_vec();
    }
    let units = units_of(data);
    let mut bytes = Vec::with_capacity(units.len());
    let mut buffer = [0u8; 4];
    for character in crate::text::utf16_chars(&units) {
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
    bytes
}

fn wide_data(kind: u32, data: &[u8]) -> Vec<u8> {
    if !is_string(kind) {
        return data.to_vec();
    }
    let mut wide = Vec::with_capacity(data.len() * 2);
    for character in crate::text::utf8_chars(data) {
        let mut pair = [0u16; 2];
        for unit in character.encode_utf16(&mut pair).iter() {
            wide.extend_from_slice(&unit.to_le_bytes());
        }
    }
    wide
}

unsafe fn give_value(
    kind: u32,
    data: &[u8],
    kind_out: *mut u32,
    out: *mut u8,
    size: *mut u32,
) -> u32 {
    if !kind_out.is_null() {
        // SAFETY: the caller's duty.
        unsafe { kind_out.write(kind) };
    }
    if size.is_null() {
        return match out.is_null() {
            true => ERROR_SUCCESS,
            false => error::INVALID_PARAMETER,
        };
    }
    // SAFETY: the caller's duty.
    let room = unsafe { size.read() } as usize;
    // SAFETY: as above.
    unsafe { size.write(data.len() as u32) };
    if out.is_null() {
        return ERROR_SUCCESS;
    }
    if data.len() > room {
        return ERROR_MORE_DATA;
    }
    // SAFETY: `out` holds `room` writable bytes, and `data` fits in them.
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len()) };
    ERROR_SUCCESS
}

unsafe fn query_value(
    handle: usize,
    name: *const u8,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    text: Text,
) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    // SAFETY: the caller's duty.
    let name = unsafe { name_units(name, text) };
    let value = match with_registry(|registry| registry.value(&path, &name)) {
        Ok(value) => value,
        Err(failure) => return win32_error(failure),
    };
    let bytes = match text {
        Text::Wide => value.data.clone(),
        Text::Ansi => ansi_data(value.kind, &value.data),
    };
    // SAFETY: the caller's duty.
    unsafe { give_value(value.kind, &bytes, kind, data, size) }
}

unsafe fn set_value(
    handle: usize,
    name: *const u8,
    kind: u32,
    data: *const u8,
    size: u32,
    text: Text,
) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    if data.is_null() && size != 0 {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: the caller's duty.
    let name = unsafe { name_units(name, text) };
    let bytes = match data.is_null() {
        true => &[][..],
        // SAFETY: the caller's duty: `size` readable bytes.
        false => unsafe { core::slice::from_raw_parts(data, size as usize) },
    };
    let stored = match text {
        Text::Wide => bytes.to_vec(),
        Text::Ansi => wide_data(kind, bytes),
    };
    with_registry(|registry| registry.set_value(&path, &name, kind, stored))
        .map_or_else(win32_error, |()| ERROR_SUCCESS)
}

unsafe fn delete_value(handle: usize, name: *const u8, text: Text) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    // SAFETY: the caller's duty.
    let name = unsafe { name_units(name, text) };
    with_registry(|registry| registry.delete_value(&path, &name))
        .map_or_else(win32_error, |()| ERROR_SUCCESS)
}

unsafe fn delete_key(handle: usize, subkey: *const u8, text: Text, tree: bool) -> u32 {
    // SAFETY: the caller's duty.
    let path = match unsafe { below(handle, subkey, text) } {
        Ok(path) => path,
        Err(code) => return code,
    };
    let deleted = with_registry(|registry| match (tree, subkey.is_null()) {
        (true, true) => registry.clear_key(&path),
        (true, false) => registry.delete_key(&path, true),
        (false, _) => registry.delete_key(&path, false),
    });
    deleted.map_or_else(win32_error, |()| ERROR_SUCCESS)
}

unsafe fn give_name(name: &[u16], out: *mut u8, size: *mut u32, text: Text) -> u32 {
    if out.is_null() || size.is_null() {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: the caller's duty.
    let room = unsafe { size.read() } as usize;
    match text {
        Text::Wide => {
            if name.len() >= room {
                return ERROR_MORE_DATA;
            }
            // SAFETY: `out` holds `room` units, more than the name's.
            unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), out.cast::<u16>(), name.len()) };
            // SAFETY: as above: the unit after the name.
            unsafe { out.cast::<u16>().wrapping_add(name.len()).write(0) };
            // SAFETY: as above.
            unsafe { size.write(name.len() as u32) };
        }
        Text::Ansi => {
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4];
            for character in crate::text::utf16_chars(name) {
                bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            if bytes.len() >= room {
                return ERROR_MORE_DATA;
            }
            // SAFETY: `out` holds `room` bytes, more than the name's.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
            // SAFETY: as above: the byte after the name.
            unsafe { out.wrapping_add(bytes.len()).write(0) };
            // SAFETY: as above.
            unsafe { size.write(bytes.len() as u32) };
        }
    }
    ERROR_SUCCESS
}

unsafe fn enum_key(
    handle: usize,
    index: u32,
    name: *mut u8,
    size: *mut u32,
    last_write: *mut u64,
    text: Text,
) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    let subkeys = match with_registry(|registry| registry.subkeys(&path)) {
        Ok(subkeys) => subkeys,
        Err(failure) => return win32_error(failure),
    };
    let Some(subkey) = subkeys.get(index as usize) else {
        return error::NO_MORE_ITEMS;
    };
    if !last_write.is_null() {
        let time = path
            .join(subkey)
            .ok()
            .and_then(|below| with_registry(|registry| registry.info(&below)).ok())
            .map_or(0, |info| info.last_write);
        // SAFETY: the caller passes a writable `FILETIME`, or null.
        unsafe { last_write.write_unaligned(time) };
    }
    // SAFETY: the caller's duty.
    unsafe { give_name(subkey, name, size, text) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn enum_value(
    handle: usize,
    index: u32,
    name: *mut u8,
    name_size: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    text: Text,
) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    let values = match with_registry(|registry| registry.values(&path)) {
        Ok(values) => values,
        Err(failure) => return win32_error(failure),
    };
    let Some(value) = values.get(index as usize) else {
        return error::NO_MORE_ITEMS;
    };
    // SAFETY: the caller's duty.
    let named = unsafe { give_name(&value.name, name, name_size, text) };
    if named != ERROR_SUCCESS {
        return named;
    }
    let bytes = match text {
        Text::Wide => value.data.clone(),
        Text::Ansi => ansi_data(value.kind, &value.data),
    };
    // SAFETY: the caller's duty.
    unsafe { give_value(value.kind, &bytes, kind, data, size) }
}

macro_rules! both {
    ($wide:ident, $ansi:ident, $inner:ident, ($($argument:ident: $kind:ty),*)) => {
        #[allow(clippy::too_many_arguments)]
        unsafe extern "efiapi" fn $wide($($argument: $kind),*) -> u32 {
            // SAFETY: the function's contract, with UTF-16 text.
            unsafe { $inner($($argument),*, Text::Wide) }
        }

        #[allow(clippy::too_many_arguments)]
        unsafe extern "efiapi" fn $ansi($($argument: $kind),*) -> u32 {
            // SAFETY: the function's contract, with ANSI text.
            unsafe { $inner($($argument),*, Text::Ansi) }
        }
    };
}

unsafe fn open_key_ex(
    handle: usize,
    subkey: *const u8,
    _options: u32,
    access: u32,
    result: *mut usize,
    text: Text,
) -> u32 {
    // SAFETY: `RegOpenKeyEx`'s contract.
    unsafe { open_key(handle, subkey, access, result, text) }
}

unsafe fn open_key_plain(handle: usize, subkey: *const u8, result: *mut usize, text: Text) -> u32 {
    // SAFETY: `RegOpenKey`'s contract: the access Windows grants it is what the caller may have.
    unsafe { open_key(handle, subkey, 0, result, text) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_key_ex(
    handle: usize,
    subkey: *const u8,
    _reserved: u32,
    _class: *const u8,
    _options: u32,
    access: u32,
    _security: usize,
    result: *mut usize,
    disposition: *mut u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegCreateKeyEx`'s contract.
    unsafe { create_key(handle, subkey, access, result, disposition, text) }
}

unsafe fn create_key_plain(
    handle: usize,
    subkey: *const u8,
    result: *mut usize,
    text: Text,
) -> u32 {
    // SAFETY: `RegCreateKey`'s contract.
    unsafe { create_key(handle, subkey, 0, result, core::ptr::null_mut(), text) }
}

unsafe fn query_value_ex(
    handle: usize,
    name: *const u8,
    _reserved: usize,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegQueryValueEx`'s contract.
    unsafe { query_value(handle, name, kind, data, size, text) }
}

unsafe fn set_value_ex(
    handle: usize,
    name: *const u8,
    _reserved: u32,
    kind: u32,
    data: *const u8,
    size: u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegSetValueEx`'s contract.
    unsafe { set_value(handle, name, kind, data, size, text) }
}

unsafe fn set_key_value(
    handle: usize,
    subkey: *const u8,
    name: *const u8,
    kind: u32,
    data: *const u8,
    size: u32,
    text: Text,
) -> u32 {
    let mut opened = 0usize;
    // SAFETY: `RegSetKeyValue`'s contract: the subkey as `RegCreateKeyEx` takes it.
    let code = unsafe { create_key(handle, subkey, 0, &mut opened, core::ptr::null_mut(), text) };
    if code != ERROR_SUCCESS {
        return code;
    }
    // SAFETY: as above: the value's name and data.
    let code = unsafe { set_value(opened, name, kind, data, size, text) };
    close_key(opened);
    code
}

unsafe fn delete_key_only(handle: usize, subkey: *const u8, text: Text) -> u32 {
    if subkey.is_null() {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: `RegDeleteKey`'s contract.
    unsafe { delete_key(handle, subkey, text, false) }
}

unsafe fn delete_key_ex(
    handle: usize,
    subkey: *const u8,
    _access: u32,
    _reserved: u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegDeleteKeyEx`'s contract.
    unsafe { delete_key_only(handle, subkey, text) }
}

unsafe fn delete_tree(handle: usize, subkey: *const u8, text: Text) -> u32 {
    // SAFETY: `RegDeleteTree`'s contract.
    unsafe { delete_key(handle, subkey, text, true) }
}

unsafe fn delete_value_plain(handle: usize, name: *const u8, text: Text) -> u32 {
    // SAFETY: `RegDeleteValue`'s contract.
    unsafe { delete_value(handle, name, text) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn enum_key_ex(
    handle: usize,
    index: u32,
    name: *mut u8,
    size: *mut u32,
    _reserved: usize,
    _class: *mut u8,
    _class_size: *mut u32,
    last_write: *mut u64,
    text: Text,
) -> u32 {
    // SAFETY: `RegEnumKeyEx`'s contract.
    unsafe { enum_key(handle, index, name, size, last_write, text) }
}

unsafe fn enum_key_plain(
    handle: usize,
    index: u32,
    name: *mut u8,
    capacity: u32,
    text: Text,
) -> u32 {
    let mut size = capacity;
    // SAFETY: `RegEnumKey`'s contract: `capacity` characters at `name`.
    unsafe { enum_key(handle, index, name, &mut size, core::ptr::null_mut(), text) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn enum_value_ex(
    handle: usize,
    index: u32,
    name: *mut u8,
    name_size: *mut u32,
    _reserved: usize,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegEnumValue`'s contract.
    unsafe { enum_value(handle, index, name, name_size, kind, data, size, text) }
}

fn expand_variables(units: &[u16], mut lookup: impl FnMut(&[u8]) -> Option<Vec<u8>>) -> Vec<u16> {
    let mut out = Vec::with_capacity(units.len());
    let mut rest = units;
    while let Some(start) = rest.iter().position(|unit| *unit == PERCENT) {
        out.extend_from_slice(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(length) = after.iter().position(|unit| *unit == PERCENT) else {
            out.extend_from_slice(&rest[start..]);
            return out;
        };
        let name = &after[..length];
        let mut name_bytes = Vec::with_capacity(name.len());
        let mut buffer = [0u8; 4];
        for character in crate::text::utf16_chars(name) {
            name_bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
        match (!name.is_empty()).then(|| lookup(&name_bytes)).flatten() {
            Some(value) => {
                for character in crate::text::utf8_chars(&value) {
                    let mut pair = [0u16; 2];
                    out.extend_from_slice(character.encode_utf16(&mut pair));
                }
            }
            None => {
                out.push(PERCENT);
                out.extend_from_slice(name);
                out.push(PERCENT);
            }
        }
        rest = &after[length + 1..];
    }
    out.extend_from_slice(rest);
    out
}

fn environment_value(name: &[u8]) -> Option<Vec<u8>> {
    let length = super::environment::variable(name, &mut [])?;
    let mut value = alloc::vec![0u8; length];
    let copied = super::environment::variable(name, &mut value)?;
    value.truncate(copied.min(length));
    Some(value)
}

fn get_value_flags_valid(flags: u32) -> bool {
    let expands_expand_sz = flags & RRF_RT_REG_EXPAND_SZ != 0 && flags & RRF_NOEXPAND == 0;
    !expands_expand_sz || flags & RRF_RT_ANY == RRF_RT_ANY
}

fn restricted_value(
    flags: u32,
    kind: u32,
    data: &[u8],
    lookup: impl FnMut(&[u8]) -> Option<Vec<u8>>,
) -> Result<(u32, Vec<u8>), u32> {
    let (kind, data) = match kind == REG_EXPAND_SZ && flags & RRF_NOEXPAND == 0 {
        true => {
            let units = units_of(data);
            let end = units
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(units.len());
            let mut bytes = Vec::new();
            for unit in expand_variables(&units[..end], lookup).iter().chain([&0]) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            (REG_SZ, bytes)
        }
        false => (kind, data.to_vec()),
    };
    let allowed = match restriction(kind) {
        Some(bit) => flags & bit != 0,
        None => flags & RRF_RT_ANY == RRF_RT_ANY,
    };
    if !allowed {
        return Err(ERROR_UNSUPPORTED_TYPE);
    }
    let size_needed = match (kind, flags & RRF_RT_ANY) {
        (REG_BINARY, RRF_RT_DWORD) => Some(4),
        (REG_BINARY, RRF_RT_QWORD) => Some(8),
        _ => None,
    };
    if size_needed.is_some_and(|bytes| data.len() != bytes) {
        return Err(ERROR_DATATYPE_MISMATCH);
    }
    Ok((kind, data))
}

#[allow(clippy::too_many_arguments)]
unsafe fn get_value(
    handle: usize,
    subkey: *const u8,
    name: *const u8,
    flags: u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
    text: Text,
) -> u32 {
    if !get_value_flags_valid(flags) {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: `RegGetValue`'s contract.
    let path = match unsafe { below(handle, subkey, text) } {
        Ok(path) => path,
        Err(code) => return code,
    };
    // SAFETY: as above.
    let name = unsafe { name_units(name, text) };
    let value = match with_registry(|registry| registry.value(&path, &name)) {
        Ok(value) => value,
        Err(failure) => return win32_error(failure),
    };
    let (value_kind, wide) =
        match restricted_value(flags, value.kind, &value.data, environment_value) {
            Ok(restricted) => restricted,
            Err(code) => return code,
        };
    let mut bytes = match text {
        Text::Wide => wide,
        Text::Ansi => ansi_data(value_kind, &wide),
    };
    let terminator = match text {
        Text::Wide => 2,
        Text::Ansi => 1,
    };
    if is_string(value_kind) && !bytes.ends_with(&[0, 0][..terminator]) {
        bytes.extend(core::iter::repeat_n(0, terminator));
    }
    // SAFETY: the caller's duty.
    unsafe { give_value(value_kind, &bytes, kind, data, size) }
}

extern "efiapi" fn close_key(handle: usize) -> u32 {
    if is_predefined(handle) {
        return ERROR_SUCCESS;
    }
    let Some(index) = handle
        .checked_sub(FIRST_HANDLE)
        .filter(|index| index % HANDLE_STEP == 0)
        .map(|index| index / HANDLE_STEP)
    else {
        return error::INVALID_HANDLE;
    };
    OPENED_KEYS.with(|keys| match keys.get_mut(index) {
        Some(slot @ Some(_)) => {
            *slot = None;
            ERROR_SUCCESS
        }
        _ => error::INVALID_HANDLE,
    })
}

extern "efiapi" fn flush_key(handle: usize) -> u32 {
    match key_of(handle) {
        Some(_) => ERROR_SUCCESS,
        None => error::INVALID_HANDLE,
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn reg_query_info_key_w(
    handle: usize,
    class: *mut u16,
    class_size: *mut u32,
    _reserved: usize,
    subkeys: *mut u32,
    longest_subkey: *mut u32,
    longest_class: *mut u32,
    values: *mut u32,
    longest_value_name: *mut u32,
    longest_value: *mut u32,
    security_descriptor: *mut u32,
    last_write: *mut u64,
) -> u32 {
    let Some(path) = key_of(handle) else {
        return error::INVALID_HANDLE;
    };
    let info = match with_registry(|registry| registry.info(&path)) {
        Ok(info) => info,
        Err(failure) => return win32_error(failure),
    };
    if !class.is_null() {
        // SAFETY: the caller passes the class buffer's size with it.
        if class_size.is_null() || unsafe { class_size.read() } == 0 {
            return error::INVALID_PARAMETER;
        }
        // SAFETY: as above: at least one writable unit.
        unsafe { class.write(0) };
    }
    for (count, value) in [
        (class_size, 0),
        (subkeys, info.subkeys),
        (longest_subkey, info.longest_subkey.min(MAX_KEY_NAME_UNITS)),
        (longest_class, 0),
        (values, info.values),
        (longest_value_name, info.longest_value_name),
        (longest_value, info.longest_data),
        (security_descriptor, 0),
    ] {
        if !count.is_null() {
            // SAFETY: the caller passes writable `DWORD`s, or nulls.
            unsafe { count.write(value as u32) };
        }
    }
    if !last_write.is_null() {
        // SAFETY: the caller passes a writable `FILETIME`, or null.
        unsafe { last_write.write_unaligned(info.last_write) };
    }
    ERROR_SUCCESS
}

both!(reg_open_key_ex_w, reg_open_key_ex_a, open_key_ex, (
    handle: usize, subkey: *const u8, options: u32, access: u32, result: *mut usize
));
both!(reg_open_key_w, reg_open_key_a, open_key_plain, (
    handle: usize, subkey: *const u8, result: *mut usize
));
both!(reg_create_key_ex_w, reg_create_key_ex_a, create_key_ex, (
    handle: usize, subkey: *const u8, reserved: u32, class: *const u8, options: u32,
    access: u32, security: usize, result: *mut usize, disposition: *mut u32
));
both!(reg_create_key_w, reg_create_key_a, create_key_plain, (
    handle: usize, subkey: *const u8, result: *mut usize
));
both!(reg_query_value_ex_w, reg_query_value_ex_a, query_value_ex, (
    handle: usize, name: *const u8, reserved: usize, kind: *mut u32, data: *mut u8,
    size: *mut u32
));
both!(reg_set_value_ex_w, reg_set_value_ex_a, set_value_ex, (
    handle: usize, name: *const u8, reserved: u32, kind: u32, data: *const u8, size: u32
));
both!(reg_set_key_value_w, reg_set_key_value_a, set_key_value, (
    handle: usize, subkey: *const u8, name: *const u8, kind: u32, data: *const u8, size: u32
));
both!(reg_delete_value_w, reg_delete_value_a, delete_value_plain, (
    handle: usize, name: *const u8
));
both!(reg_delete_key_w, reg_delete_key_a, delete_key_only, (
    handle: usize, subkey: *const u8
));
both!(reg_delete_key_ex_w, reg_delete_key_ex_a, delete_key_ex, (
    handle: usize, subkey: *const u8, access: u32, reserved: u32
));
both!(reg_delete_tree_w, reg_delete_tree_a, delete_tree, (
    handle: usize, subkey: *const u8
));
both!(reg_enum_key_ex_w, reg_enum_key_ex_a, enum_key_ex, (
    handle: usize, index: u32, name: *mut u8, size: *mut u32, reserved: usize,
    class: *mut u8, class_size: *mut u32, last_write: *mut u64
));
both!(reg_enum_key_w, reg_enum_key_a, enum_key_plain, (
    handle: usize, index: u32, name: *mut u8, capacity: u32
));
both!(reg_enum_value_w, reg_enum_value_a, enum_value_ex, (
    handle: usize, index: u32, name: *mut u8, name_size: *mut u32, reserved: usize,
    kind: *mut u32, data: *mut u8, size: *mut u32
));
both!(reg_get_value_w, reg_get_value_a, get_value, (
    handle: usize, subkey: *const u8, name: *const u8, flags: u32, kind: *mut u32,
    data: *mut u8, size: *mut u32
));

pub(super) const EXPORTS: &[Export] = exports![
    b"RegOpenKeyExW" => reg_open_key_ex_w,
    b"RegOpenKeyExA" => reg_open_key_ex_a,
    b"RegOpenKeyW" => reg_open_key_w,
    b"RegOpenKeyA" => reg_open_key_a,
    b"RegCreateKeyExW" => reg_create_key_ex_w [two_stack_arguments],
    b"RegCreateKeyExA" => reg_create_key_ex_a [two_stack_arguments],
    b"RegCreateKeyW" => reg_create_key_w,
    b"RegCreateKeyA" => reg_create_key_a,
    b"RegQueryValueExW" => reg_query_value_ex_w,
    b"RegQueryValueExA" => reg_query_value_ex_a,
    b"RegSetValueExW" => reg_set_value_ex_w,
    b"RegSetValueExA" => reg_set_value_ex_a,
    b"RegSetKeyValueW" => reg_set_key_value_w,
    b"RegSetKeyValueA" => reg_set_key_value_a,
    b"RegDeleteValueW" => reg_delete_value_w,
    b"RegDeleteValueA" => reg_delete_value_a,
    b"RegDeleteKeyW" => reg_delete_key_w,
    b"RegDeleteKeyA" => reg_delete_key_a,
    b"RegDeleteKeyExW" => reg_delete_key_ex_w,
    b"RegDeleteKeyExA" => reg_delete_key_ex_a,
    b"RegDeleteTreeW" => reg_delete_tree_w,
    b"RegDeleteTreeA" => reg_delete_tree_a,
    b"RegEnumKeyExW" => reg_enum_key_ex_w,
    b"RegEnumKeyExA" => reg_enum_key_ex_a,
    b"RegEnumKeyW" => reg_enum_key_w,
    b"RegEnumKeyA" => reg_enum_key_a,
    b"RegEnumValueW" => reg_enum_value_w,
    b"RegEnumValueA" => reg_enum_value_a,
    b"RegGetValueW" => reg_get_value_w,
    b"RegGetValueA" => reg_get_value_a,
    b"RegQueryInfoKeyW" => reg_query_info_key_w [four_stack_arguments],
    b"RegCloseKey" => close_key,
    b"RegFlushKey" => flush_key,
];

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_READ: u32 = 0x0002_0019;
    const KEY_WRITE: u32 = 0x0002_0006;

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn home(name: &[u8]) -> Option<Vec<u8>> {
        (name == b"HOME").then(|| b"/home/ada".to_vec())
    }

    fn expanded(text: &str) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        String::from_utf16(&expand_variables(&units, home)).unwrap()
    }

    #[test]
    fn no_spin_lock_is_held_while_the_registry_works() {
        static IDLE: SpinLock<Option<u32>> = SpinLock::new(None);
        let path = KeyPath::parse("HKCU\\Software\\Locks").unwrap();
        let handle = with_idle(&IDLE, || 1, |_| insert_key(path.clone())).unwrap();
        assert_eq!(with_idle(&IDLE, || 1, |_| key_of(handle)), Some(path));
        let made = with_idle(
            &IDLE,
            || 1,
            |outer| {
                *outer = 5;
                with_idle(&IDLE, || 2, |inner| *inner)
            },
        );
        assert_eq!(made, 2);
        assert_eq!(with_idle(&IDLE, || 3, |kept| *kept), 5);
        assert_eq!(close_key(handle), ERROR_SUCCESS);
    }

    #[test]
    fn variables_expand_and_unknown_ones_stay_as_written() {
        assert_eq!(expanded("%HOME%\\x"), "/home/ada\\x");
        assert_eq!(expanded("a%home%b"), "a%home%b");
        assert_eq!(expanded("%NOPE%%HOME%"), "%NOPE%/home/ada");
        assert_eq!(expanded("100%"), "100%");
        assert_eq!(expanded("%%"), "%%");
        assert_eq!(expanded("plain"), "plain");
    }

    #[test]
    fn reg_get_value_expands_expand_sz_into_sz_unless_told_not_to() {
        let stored = wide("%HOME%\\x");
        assert!(!get_value_flags_valid(RRF_RT_REG_EXPAND_SZ));
        assert!(!get_value_flags_valid(RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ));
        assert!(get_value_flags_valid(RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND));
        assert!(get_value_flags_valid(RRF_RT_ANY));
        assert!(get_value_flags_valid(RRF_RT_REG_SZ));
        assert_eq!(
            restricted_value(RRF_RT_REG_SZ, REG_EXPAND_SZ, &stored, home),
            Ok((REG_SZ, wide("/home/ada\\x")))
        );
        assert_eq!(
            restricted_value(RRF_RT_ANY, REG_EXPAND_SZ, &stored, home),
            Ok((REG_SZ, wide("/home/ada\\x")))
        );
        assert_eq!(
            restricted_value(RRF_RT_REG_SZ | RRF_NOEXPAND, REG_EXPAND_SZ, &stored, home),
            Err(ERROR_UNSUPPORTED_TYPE)
        );
        assert_eq!(
            restricted_value(
                RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
                REG_EXPAND_SZ,
                &stored,
                home
            ),
            Ok((REG_EXPAND_SZ, stored.clone()))
        );
        assert_eq!(
            restricted_value(RRF_RT_REG_SZ, REG_SZ, &wide("x"), home),
            Ok((REG_SZ, wide("x")))
        );
    }

    #[test]
    fn rrf_rt_dword_and_qword_take_binary_data_of_their_size_only() {
        let binary = |length: usize| vec![7u8; length];
        assert_eq!(
            restricted_value(RRF_RT_DWORD, REG_BINARY, &binary(4), home),
            Ok((REG_BINARY, binary(4)))
        );
        assert_eq!(
            restricted_value(RRF_RT_DWORD, REG_BINARY, &binary(3), home),
            Err(ERROR_DATATYPE_MISMATCH)
        );
        assert_eq!(
            restricted_value(RRF_RT_QWORD, REG_BINARY, &binary(8), home),
            Ok((REG_BINARY, binary(8)))
        );
        assert_eq!(
            restricted_value(RRF_RT_QWORD, REG_BINARY, &binary(4), home),
            Err(ERROR_DATATYPE_MISMATCH)
        );
        assert_eq!(
            restricted_value(RRF_RT_REG_DWORD, REG_BINARY, &binary(4), home),
            Err(ERROR_UNSUPPORTED_TYPE)
        );
        assert_eq!(
            restricted_value(RRF_RT_ANY, REG_BINARY, &binary(3), home),
            Ok((REG_BINARY, binary(3)))
        );
        assert_eq!(
            restricted_value(RRF_RT_DWORD, REG_DWORD, &binary(4), home),
            Ok((REG_DWORD, binary(4)))
        );
    }

    #[test]
    fn opening_the_machine_s_keys_to_write_is_refused_to_who_cannot_change_them() {
        let machine = KeyPath::parse("HKLM\\Software\\AutoHotkey").unwrap();
        let config = KeyPath::parse("HKCC\\Software").unwrap();
        let user = KeyPath::parse("HKCU\\Software\\AutoHotkey").unwrap();
        let classes = KeyPath::parse("HKCR\\.ahk").unwrap();
        for access in [KEY_WRITE, KEY_SET_VALUE, GENERIC_WRITE, DELETE, 0x000f_003f] {
            assert!(denied_by_access(&machine, access, false), "{access:#x}");
            assert!(denied_by_access(&config, access, false), "{access:#x}");
            assert!(!denied_by_access(&machine, access, true), "{access:#x}");
            assert!(!denied_by_access(&user, access, false), "{access:#x}");
            assert!(!denied_by_access(&classes, access, false), "{access:#x}");
        }
        assert!(!denied_by_access(&machine, KEY_READ, false));
        assert!(!denied_by_access(&machine, 0, false));
    }
}
