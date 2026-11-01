//! dyld's functions: the program's image (`_dyld_get_image_header` and its kin,
//! `_NSGetExecutablePath`) and `dlsym`, which finds the layer's own libSystem functions;
//! `dlopen` loads nothing yet.

use super::{c_string, c_string_with_nul};
use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

static mut IMAGE_HEADER: usize = 0;
static mut IMAGE_SLIDE: isize = 0;
static mut EXECUTABLE_PATH: *const c_char = core::ptr::null();
static FAILED: AtomicBool = AtomicBool::new(false);
const SYMBOL_LENGTH: usize = 256;

/// Records where the program's image is and its path (NUL-terminated, kept for as long as the
/// program runs), for `_dyld_get_image_header` and its kin.
///
/// # Safety
/// Called once, before the program runs.
pub unsafe fn set_image(header: usize, slide: isize, path: *const c_char) {
    // SAFETY: the caller's promise.
    unsafe { IMAGE_HEADER = header };
    // SAFETY: as above.
    unsafe { IMAGE_SLIDE = slide };
    // SAFETY: as above.
    unsafe { EXECUTABLE_PATH = path };
}

/// The executable's path, as the kernel gave it; null before the program starts.
pub fn executable_path() -> *const c_char {
    // SAFETY: written once before the program runs.
    unsafe { EXECUTABLE_PATH }
}

pub extern "C" fn image_count() -> u32 {
    1
}

pub extern "C" fn image_header(index: u32) -> *const c_void {
    if index == 0 {
        // SAFETY: written once before the program runs.
        unsafe { IMAGE_HEADER as *const c_void }
    } else {
        core::ptr::null()
    }
}

pub extern "C" fn image_slide(index: u32) -> isize {
    if index == 0 {
        // SAFETY: as above.
        unsafe { IMAGE_SLIDE }
    } else {
        0
    }
}

pub extern "C" fn image_name(index: u32) -> *const c_char {
    if index == 0 {
        // SAFETY: as above.
        unsafe { EXECUTABLE_PATH }
    } else {
        core::ptr::null()
    }
}

/// # Safety
/// `buffer` must be writable for `*size` bytes and `size` readable and writable.
pub unsafe extern "C" fn ns_get_executable_path(buffer: *mut c_char, size: *mut u32) -> c_int {
    // SAFETY: written once before the program runs.
    let path = unsafe { EXECUTABLE_PATH };
    // SAFETY: a NUL-terminated string that lasts as long as the program.
    let path = unsafe { c_string_with_nul(path) };
    // SAFETY: the caller's promise.
    let room = unsafe { size.read() } as usize;
    if room < path.len() {
        // SAFETY: as above.
        unsafe { size.write(path.len() as u32) };
        return -1;
    }
    // SAFETY: as above; the path and its NUL fit.
    unsafe { core::ptr::copy_nonoverlapping(path.as_ptr(), buffer.cast(), path.len()) };
    0
}

/// # Safety
/// `name` must be a NUL-terminated string.
pub unsafe extern "C" fn dlsym(_: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: the caller's promise.
    let name = unsafe { c_string(name) };
    let mut symbol = [0u8; SYMBOL_LENGTH];
    let found = (name.len() < SYMBOL_LENGTH)
        .then(|| {
            symbol[0] = b'_';
            symbol[1..=name.len()].copy_from_slice(name);
            super::resolve(&symbol[..=name.len()])
        })
        .flatten();
    FAILED.store(found.is_none(), Ordering::Relaxed);
    found.map_or(core::ptr::null_mut(), |address| address as *mut c_void)
}

/// `dyld_stub_binder`, which a lazy symbol stub reaches through its stub helper only while
/// its lazy pointer is unbound. The layer binds every lazy pointer when it loads a program
/// (D190), so reaching it means the program jumped into a stub helper by itself.
pub extern "C" fn stub_binder() -> ! {
    super::process::fail(b"dyld_stub_binder: called for a lazy pointer the layer did not bind")
}

pub extern "C" fn dlopen(_: *const c_char, _: c_int) -> *mut c_void {
    FAILED.store(true, Ordering::Relaxed);
    core::ptr::null_mut()
}

pub extern "C" fn dlclose(_: *mut c_void) -> c_int {
    0
}

pub extern "C" fn dlerror() -> *const c_char {
    if FAILED.swap(false, Ordering::Relaxed) {
        c"EggOS does not load dynamic libraries yet".as_ptr()
    } else {
        core::ptr::null()
    }
}
