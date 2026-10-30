//! psapi (D178): `GetProcessImageFileNameW` names the caller's own program, in the Windows
//! form the layer gives every path (`C:\` is `/`): EggOS has no NT device names
//! (`\Device\...`). Other processes' programs are `ERROR_ACCESS_DENIED`.

use super::{error, fail};
use crate::library::Export;
use crate::linux;

unsafe extern "efiapi" fn get_process_image_file_name_w(
    process: usize,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    match super::processes::process_id_of(process) {
        None => return fail(error::INVALID_HANDLE, 0),
        Some(id) if id != linux::process_id() => return fail(error::ACCESS_DENIED, 0),
        Some(_) => {}
    }
    // SAFETY: `GetProcessImageFileNameW`'s contract: `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    match super::file::copy_terminated(super::process::process().program_windows, out) {
        Ok(written) => written as u32,
        Err(_) => fail(error::INSUFFICIENT_BUFFER, 0),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetProcessImageFileNameW" => get_process_image_file_name_w,
];
