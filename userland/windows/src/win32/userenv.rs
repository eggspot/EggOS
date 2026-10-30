//! `userenv.dll`'s `GetUserProfileDirectoryW` (D189): the user's profile folder is their home
//! folder (`HOME`), in Windows' form, as `SHGetFolderPathW` gives it.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::text::PATH_CAPACITY;

unsafe extern "efiapi" fn get_user_profile_directory_w(
    _token: usize,
    buffer: *mut u16,
    size: *mut u32,
) -> Bool {
    if size.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let mut home = [0u8; PATH_CAPACITY];
    let length = super::shell_folders::home(&mut home);
    // SAFETY: `GetUserProfileDirectoryW`'s contract: a readable and writable size.
    let capacity = unsafe { size.read() } as usize;
    // SAFETY: the same contract: `capacity` writable units at `buffer`, or null.
    let out = unsafe { super::file::wide_buffer(buffer, capacity) };
    match super::directory::write_windows_path(&home[..length], out) {
        Ok(written) => {
            // SAFETY: as above.
            unsafe { size.write(written as u32 + 1) };
            TRUE
        }
        Err(needed) => {
            // SAFETY: as above.
            unsafe { size.write(needed as u32) };
            fail(error::INSUFFICIENT_BUFFER, FALSE)
        }
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetUserProfileDirectoryW" => get_user_profile_directory_w,
];
