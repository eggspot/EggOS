//! shell32's folders (D178): `SHGetFolderPathW` and `SHGetKnownFolderPath` give Windows'
//! special folders where EggOS keeps them (`crate::known_folders`), made on request;
//! `SHGetMalloc` is the shell's allocator, a COM object over the layer's heap. The shell's
//! namespace of item lists and shell folders is not there yet: the layer makes no item lists,
//! `SHGetDesktopFolder` and `SHCreateItemFromParsingName` answer that the request is not
//! supported, and the folder picker (`SHBrowseForFolderW`) cannot pick folders yet, so it
//! returns as if cancelled.

use super::{Bool, FALSE, heap};
use crate::known_folders::{self, Folder, Place};
use crate::library::Export;
use crate::linux::{self, AT_FDCWD};
use crate::text::PATH_CAPACITY;
use core::ffi::c_void;

pub(super) const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
const E_POINTER: i32 = 0x8000_4003_u32 as i32;
const E_FAIL: i32 = 0x8000_4005_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000e_u32 as i32;
const CSIDL_FLAG_CREATE: u32 = 0x8000;
const SHGFP_TYPE_DEFAULT: u32 = 1;
const KF_FLAG_DONT_VERIFY: u32 = 0x4000;
const KF_FLAG_CREATE: u32 = 0x8000;
const MAX_PATH: usize = 260;
const DEFAULT_HOME: &[u8] = b"/home/root";
const EEXIST: u16 = 17;
const IID_IUNKNOWN: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const IID_IMALLOC: [u8; 16] = [2, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];

/// `HRESULT_FROM_WIN32(code)`.
pub(super) fn from_win32(code: u32) -> i32 {
    (0x8007_0000 | (code & 0xffff)) as i32
}

/// The user's home folder (`HOME`, else root's, D132) into `out`; its length.
pub(super) fn home(out: &mut [u8; PATH_CAPACITY]) -> usize {
    match super::environment::variable(b"HOME", out) {
        Some(length) if (1..PATH_CAPACITY).contains(&length) => length,
        _ => {
            out[..DEFAULT_HOME.len()].copy_from_slice(DEFAULT_HOME);
            DEFAULT_HOME.len()
        }
    }
}

fn folder_path(folder: Folder, out: &mut [u8; PATH_CAPACITY]) -> usize {
    let length = match folder.place() {
        Place::Absolute(path) => {
            out[..path.len()].copy_from_slice(path);
            path.len()
        }
        Place::Home(under) => {
            let mut length = home(out);
            if !under.is_empty() {
                out[length] = b'/';
                out[length + 1..length + 1 + under.len()].copy_from_slice(under);
                length += 1 + under.len();
            }
            length
        }
    };
    out[length] = 0;
    length + 1
}

/// Makes the folder at the NUL-terminated `path` and the folders above it that are missing;
/// whether it is there afterwards.
pub(super) fn make_folders(path: &[u8]) -> bool {
    let mut partial = [0u8; PATH_CAPACITY];
    let name = &path[..path.len() - 1];
    for (at, byte) in name.iter().enumerate() {
        if *byte == b'/' && at > 0 {
            partial[..at].copy_from_slice(&name[..at]);
            partial[at] = 0;
            let _ = linux::make_directory(&partial[..=at]);
        }
    }
    match linux::make_directory(path) {
        Ok(_) => true,
        Err(errno) => errno.0 == EEXIST,
    }
}

fn is_folder(path: &[u8]) -> bool {
    linux::stat_at(AT_FDCWD, path)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY)
}

fn windows_folder(
    folder: Folder,
    create: bool,
    verify: bool,
    out: &mut [u16],
) -> Result<usize, i32> {
    let mut path = [0u8; PATH_CAPACITY];
    let length = folder_path(folder, &mut path);
    let path = &path[..length];
    if create && !is_folder(path) && !make_folders(path) {
        return Err(E_FAIL);
    }
    if verify && !is_folder(path) {
        return Err(from_win32(super::error::FILE_NOT_FOUND));
    }
    super::directory::write_windows_path(&path[..length - 1], out).map_err(|_| E_FAIL)
}

unsafe extern "efiapi" fn sh_get_folder_path_w(
    _window: usize,
    csidl: u32,
    _token: usize,
    flags: u32,
    out: *mut u16,
) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let Some(folder) = known_folders::by_csidl(csidl) else {
        return E_INVALIDARG;
    };
    // SAFETY: `SHGetFolderPathW`'s contract: a buffer of `MAX_PATH` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(out, MAX_PATH) };
    let create = csidl & CSIDL_FLAG_CREATE != 0;
    match windows_folder(folder, create, flags != SHGFP_TYPE_DEFAULT, out) {
        Ok(_) => S_OK,
        Err(_) => E_FAIL,
    }
}

unsafe extern "efiapi" fn sh_get_known_folder_path(
    id: *const [u8; 16],
    flags: u32,
    _token: usize,
    path: *mut *mut u16,
) -> i32 {
    if path.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller passes a writable `PWSTR`.
    unsafe { path.write(core::ptr::null_mut()) };
    if id.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `SHGetKnownFolderPath`'s contract: a readable `KNOWNFOLDERID`.
    let Some(folder) = known_folders::by_known_folder_id(&unsafe { id.read_unaligned() }) else {
        return E_INVALIDARG;
    };
    let mut units = [0u16; PATH_CAPACITY];
    let verify = flags & KF_FLAG_DONT_VERIFY == 0;
    let length = match windows_folder(folder, flags & KF_FLAG_CREATE != 0, verify, &mut units) {
        Ok(length) => length,
        Err(result) => return result,
    };
    let block = heap::allocate((length + 1) * 2).cast::<u16>();
    if block.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh, zeroed block of `length + 1` units: the path and its NUL.
    unsafe { block.copy_from_nonoverlapping(units.as_ptr(), length) };
    // SAFETY: as above: the caller frees the block with `CoTaskMemFree`.
    unsafe { path.write(block) };
    S_OK
}

extern "efiapi" fn sh_get_path_from_id_list_w(_list: usize, _path: *mut u16) -> Bool {
    FALSE
}

unsafe extern "efiapi" fn sh_get_desktop_folder(folder: *mut usize) -> i32 {
    if folder.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable `IShellFolder*`.
    unsafe { folder.write(0) };
    from_win32(super::error::NOT_SUPPORTED)
}

unsafe extern "efiapi" fn sh_create_item_from_parsing_name(
    _path: *const u16,
    _context: usize,
    _interface: *const [u8; 16],
    item: *mut usize,
) -> i32 {
    if item.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { item.write(0) };
    from_win32(super::error::NOT_SUPPORTED)
}

extern "efiapi" fn sh_browse_for_folder_w(_request: usize) -> usize {
    0
}

#[allow(dead_code, reason = "the program reads it")]
#[repr(transparent)]
struct Methods([*const (); 9]);

// SAFETY: the table holds functions' addresses, fixed for the program's life.
unsafe impl Sync for Methods {}

unsafe extern "efiapi" fn malloc_query_interface(
    this: *mut c_void,
    interface: *const [u8; 16],
    object: *mut *mut c_void,
) -> i32 {
    if object.is_null() || interface.is_null() {
        return E_POINTER;
    }
    // SAFETY: `QueryInterface`'s contract: a readable interface id.
    let interface = unsafe { interface.read_unaligned() };
    let known = interface == IID_IUNKNOWN || interface == IID_IMALLOC;
    let answer = if known { this } else { core::ptr::null_mut() };
    // SAFETY: as above: a writable interface pointer.
    unsafe { object.write(answer) };
    if known { S_OK } else { E_NOINTERFACE }
}

extern "efiapi" fn malloc_add_ref(_this: *mut c_void) -> u32 {
    1
}

extern "efiapi" fn malloc_release(_this: *mut c_void) -> u32 {
    1
}

extern "efiapi" fn malloc_alloc(_this: *mut c_void, size: usize) -> *mut u8 {
    super::heap::allocate_with(size, false)
}

unsafe extern "efiapi" fn malloc_realloc(
    _this: *mut c_void,
    block: *mut u8,
    size: usize,
) -> *mut u8 {
    if block.is_null() {
        return super::heap::allocate_with(size, false);
    }
    if size == 0 {
        // SAFETY: `IMalloc::Realloc`'s contract: a block this allocator handed out.
        unsafe { heap::free(block) };
        return core::ptr::null_mut();
    }
    // SAFETY: as above.
    unsafe { super::heap::heap_realloc(0, 0, block, size) }
}

unsafe extern "efiapi" fn malloc_free(_this: *mut c_void, block: *mut u8) {
    // SAFETY: `IMalloc::Free`'s contract: a block this allocator handed out, or null.
    unsafe { heap::free(block) };
}

unsafe extern "efiapi" fn malloc_get_size(_this: *mut c_void, block: *mut u8) -> usize {
    if block.is_null() {
        return usize::MAX;
    }
    // SAFETY: `IMalloc::GetSize`'s contract: a block this allocator handed out.
    unsafe { super::heap::heap_size(0, 0, block) }
}

extern "efiapi" fn malloc_did_alloc(_this: *mut c_void, _block: *mut u8) -> i32 {
    -1
}

extern "efiapi" fn malloc_heap_minimize(_this: *mut c_void) {}

static MALLOC_METHODS: Methods = Methods([
    export_entry!(malloc_query_interface),
    export_entry!(malloc_add_ref),
    export_entry!(malloc_release),
    export_entry!(malloc_alloc),
    export_entry!(malloc_realloc),
    export_entry!(malloc_free),
    export_entry!(malloc_get_size),
    export_entry!(malloc_did_alloc),
    export_entry!(malloc_heap_minimize),
]);

static MALLOC: &Methods = &MALLOC_METHODS;

unsafe extern "efiapi" fn sh_get_malloc(malloc: *mut usize) -> i32 {
    if malloc.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller passes a writable `IMalloc*`: the object is the address of a pointer
    // to its table of methods.
    unsafe { malloc.write(&raw const MALLOC as usize) };
    S_OK
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SHGetFolderPathW" => sh_get_folder_path_w,
    b"SHGetKnownFolderPath" => sh_get_known_folder_path,
    b"SHGetPathFromIDListW" => sh_get_path_from_id_list_w,
    b"SHGetDesktopFolder" => sh_get_desktop_folder,
    b"SHCreateItemFromParsingName" => sh_create_item_from_parsing_name,
    b"SHBrowseForFolderW" => sh_browse_for_folder_w,
    b"SHGetMalloc" => sh_get_malloc,
];
