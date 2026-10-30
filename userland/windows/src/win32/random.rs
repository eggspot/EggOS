//! `bcryptprimitives.dll`'s `ProcessPrng`, the random bytes Rust's standard library seeds its
//! hash maps with (D189): the kernel's generator (`getrandom`). Like Windows', it never fails.

use super::{Bool, TRUE};
use crate::library::Export;
use crate::linux;

unsafe extern "efiapi" fn process_prng(buffer: *mut u8, length: usize) -> Bool {
    if buffer.is_null() || length == 0 {
        return TRUE;
    }
    // SAFETY: `ProcessPrng`'s contract: `length` writable bytes at `buffer`.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, length) };
    while linux::random_bytes(buffer).is_err() {
        linux::yield_now();
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"ProcessPrng" => process_prng,
];
