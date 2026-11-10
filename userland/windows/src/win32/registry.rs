//! advapi32's registry functions (D231) over `eggos_windows_registry`: the user's hive below the
//! home folder and the machine's in `/var/lib/eggos/windows`, both written through a temporary
//! file and a rename. Opened keys are handles that name their key's path, so a key another
//! process deleted reads as missing. The `A` functions take and give ANSI text, which is UTF-8
//! on EggOS, string data included.

use alloc::vec::Vec;

use eggos_runtime::startup::Startup;
use eggos_windows_registry::Error as RegistryError;
use eggos_windows_registry::command::{self, CODED_ERRORS_ENVIRONMENT, Console};
use eggos_windows_registry::hive::Value;
use eggos_windows_registry::path::{KeyPath, Root};
use eggos_windows_registry::registry::{
    DAMAGED_SUFFIX, HiveFile, MACHINE_HIVE_PATH, Registry, Store, StoreError, TEMPORARY_SUFFIX,
    USER_HIVE_PATH,
};
use eggos_windows_registry::value::{
    REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_NONE, REG_QWORD, REG_SZ,
};
use eggos_windows_registry::{MAX_HIVE_BYTES, MAX_KEY_NAME_UNITS};

use super::{SpinLock, error};
use crate::library::Export;
use crate::linux;
use crate::text::PATH_CAPACITY;

const ERROR_SUCCESS: u32 = 0;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_BADDB: u32 = 1009;
const ERROR_CANTREAD: u32 = 1012;
const ERROR_CANTWRITE: u32 = 1013;
const ERROR_UNSUPPORTED_TYPE: u32 = 1630;
const FIRST_HANDLE: usize = 0x5000_0000;
const HANDLE_STEP: usize = 4;
const MAX_OPEN_KEYS: usize = 16_384;
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;
const RRF_RT_ANY: u32 = 0x0000_ffff;
const FILETIME_EPOCH_INTERVALS: u64 = 11_644_473_600 * 10_000_000;

struct LayerStore;

fn hive_path(file: HiveFile, suffix: &str) -> Vec<u8> {
    let mut path = Vec::new();
    match file {
        HiveFile::User => {
            let mut home = [0u8; PATH_CAPACITY];
            let length = super::shell_folders::home(&mut home);
            path.extend_from_slice(&home[..length]);
            path.push(b'/');
            path.extend_from_slice(USER_HIVE_PATH.as_bytes());
        }
        HiveFile::Machine => path.extend_from_slice(MACHINE_HIVE_PATH.as_bytes()),
    }
    path.extend_from_slice(suffix.as_bytes());
    path.push(0);
    path
}

fn store_error(errno: eggos_runtime::errno::Errno) -> StoreError {
    match errno {
        eggos_runtime::errno::Errno::ENOSPC => StoreError::DiskFull,
        _ => StoreError::Failed,
    }
}

impl Store for LayerStore {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        let fd = match linux::open(&hive_path(file, ""), linux::O_RDONLY) {
            Ok(fd) => fd,
            Err(eggos_runtime::errno::Errno::ENOENT) => return Ok(None),
            Err(_) => return Err(StoreError::Failed),
        };
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        let result = loop {
            match linux::read(fd, &mut chunk) {
                Ok(0) => break Ok(Some(bytes)),
                Ok(count) if bytes.len() + count <= MAX_HIVE_BYTES + 1 => {
                    bytes.extend_from_slice(&chunk[..count.min(chunk.len())]);
                }
                Ok(_) => break Ok(Some(bytes)),
                Err(_) => break Err(StoreError::Failed),
            }
        };
        let _ = linux::close(fd);
        result
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        let words = linux::stat_at(linux::AT_FDCWD, &hive_path(file, "")).ok()?;
        Some(
            words[linux::STAT_MODIFY_TIME_WORD]
                .wrapping_mul(1_000_000_007)
                .wrapping_add(words[linux::STAT_MODIFY_TIME_WORD + 1])
                .wrapping_add(words[linux::STAT_SIZE_WORD].rotate_left(24))
                .wrapping_add(words[linux::STAT_INODE_WORD].rotate_left(44)),
        )
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        let path = hive_path(file, TEMPORARY_SUFFIX);
        for (index, byte) in path.iter().enumerate().skip(1) {
            if *byte == b'/' {
                let mut folder = path[..index].to_vec();
                folder.push(0);
                let _ = linux::make_directory(&folder);
            }
        }
        let fd = linux::open(&path, linux::O_WRONLY | linux::O_CREAT | linux::O_TRUNC)
            .map_err(store_error)?;
        let written = linux::write_all(fd, bytes).and_then(|()| match linux::sync_file(fd) {
            Ok(_) | Err(eggos_runtime::errno::Errno::ENOSYS) => Ok(()),
            Err(errno) => Err(errno),
        });
        let _ = linux::close(fd);
        written.map_err(store_error)
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        linux::rename(
            &hive_path(file, TEMPORARY_SUFFIX),
            &hive_path(file, ""),
            false,
        )
        .map(|_| ())
        .map_err(store_error)
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        match linux::rename(
            &hive_path(file, ""),
            &hive_path(file, DAMAGED_SUFFIX),
            false,
        ) {
            Ok(_) | Err(eggos_runtime::errno::Errno::ENOENT) => Ok(()),
            Err(errno) => Err(store_error(errno)),
        }
    }

    fn user_id(&mut self) -> u32 {
        linux::user_id()
    }

    fn now(&mut self) -> u64 {
        linux::clock_nanoseconds(linux::CLOCK_REALTIME) / 100 + FILETIME_EPOCH_INTERVALS
    }
}

struct Opened {
    registry: Option<Registry<LayerStore>>,
    keys: Vec<Option<KeyPath>>,
}

static OPENED: SpinLock<Opened> = SpinLock::new(Opened {
    registry: None,
    keys: Vec::new(),
});

fn with_registry<R>(work: impl FnOnce(&mut Registry<LayerStore>) -> R) -> R {
    OPENED.with(|opened| {
        work(
            opened
                .registry
                .get_or_insert_with(|| Registry::new(LayerStore)),
        )
    })
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
    let mut registry = Registry::new(LayerStore);
    command::run(&mut registry, &mut TerminalConsole { coded_errors }, &words)
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
    OPENED.with(|opened| opened.keys.get(index / HANDLE_STEP).cloned().flatten())
}

fn is_predefined(handle: usize) -> bool {
    let high = handle >> 32;
    (high == 0 || high == 0xffff_ffff) && (0x8000_0000..=0x8000_0007).contains(&(handle as u32))
}

fn insert_key(path: KeyPath) -> Option<usize> {
    OPENED.with(|opened| {
        let index = match opened.keys.iter().position(Option::is_none) {
            Some(index) => index,
            None if opened.keys.len() < MAX_OPEN_KEYS => {
                opened.keys.push(None);
                opened.keys.len() - 1
            }
            None => return None,
        };
        opened.keys[index] = Some(path);
        Some(FIRST_HANDLE + index * HANDLE_STEP)
    })
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

unsafe fn open_key(handle: usize, subkey: *const u8, result: *mut usize, text: Text) -> u32 {
    if result.is_null() {
        return error::INVALID_PARAMETER;
    }
    // SAFETY: the caller's duty.
    let path = match unsafe { below(handle, subkey, text) } {
        Ok(path) => path,
        Err(code) => return code,
    };
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
        REG_NONE => Some(0x01),
        REG_SZ => Some(0x02),
        REG_EXPAND_SZ => Some(0x04),
        REG_BINARY => Some(0x08),
        REG_DWORD => Some(0x10),
        REG_MULTI_SZ => Some(0x20),
        REG_QWORD => Some(0x40),
        _ => None,
    }
}

fn is_string(kind: u32) -> bool {
    matches!(kind, REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ)
}

fn ansi_data(value: &Value) -> Vec<u8> {
    if !is_string(value.kind) {
        return value.data.clone();
    }
    let units: Vec<u16> = value
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
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
        Text::Ansi => ansi_data(&value),
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
        Text::Ansi => ansi_data(value),
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
    _access: u32,
    result: *mut usize,
    text: Text,
) -> u32 {
    // SAFETY: `RegOpenKeyEx`'s contract.
    unsafe { open_key(handle, subkey, result, text) }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_key_ex(
    handle: usize,
    subkey: *const u8,
    _reserved: u32,
    _class: *const u8,
    _options: u32,
    _access: u32,
    _security: usize,
    result: *mut usize,
    disposition: *mut u32,
    text: Text,
) -> u32 {
    // SAFETY: `RegCreateKeyEx`'s contract.
    unsafe { create_key(handle, subkey, result, disposition, text) }
}

unsafe fn create_key_plain(
    handle: usize,
    subkey: *const u8,
    result: *mut usize,
    text: Text,
) -> u32 {
    // SAFETY: `RegCreateKey`'s contract.
    unsafe { create_key(handle, subkey, result, core::ptr::null_mut(), text) }
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
    let code = unsafe { create_key(handle, subkey, &mut opened, core::ptr::null_mut(), text) };
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
    let allowed = match restriction(value.kind) {
        Some(bit) => flags & bit != 0,
        None => flags & RRF_RT_ANY == RRF_RT_ANY,
    };
    if !allowed {
        return ERROR_UNSUPPORTED_TYPE;
    }
    let mut bytes = match text {
        Text::Wide => value.data.clone(),
        Text::Ansi => ansi_data(&value),
    };
    let terminator = match text {
        Text::Wide => 2,
        Text::Ansi => 1,
    };
    if is_string(value.kind) && !bytes.ends_with(&[0, 0][..terminator]) {
        bytes.extend(core::iter::repeat_n(0, terminator));
    }
    // SAFETY: the caller's duty.
    unsafe { give_value(value.kind, &bytes, kind, data, size) }
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
    OPENED.with(|opened| match opened.keys.get_mut(index) {
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
both!(reg_open_key_w, reg_open_key_a, open_key, (
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
