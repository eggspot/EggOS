//! A macOS console program that links a library other than libSystem (`libz`), so the macOS
//! layer must refuse it with a clear message naming the library and status 126 before any of
//! its code runs (D120).

#![cfg_attr(all(target_os = "macos", not(test)), no_std, no_main)]

#[cfg(all(target_os = "macos", not(test)))]
mod program {
    use core::ffi::{c_char, c_int};

    #[link(name = "System")]
    unsafe extern "C" {
        fn puts(text: *const c_char) -> c_int;
        fn abort() -> !;
    }

    #[link(name = "z")]
    unsafe extern "C" {
        fn zlibVersion() -> *const c_char;
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: `abort` takes no arguments and ends the process.
        unsafe { abort() }
    }

    #[unsafe(no_mangle)]
    extern "C" fn main() -> c_int {
        // SAFETY: `zlibVersion` takes no arguments.
        let version = unsafe { zlibVersion() };
        // SAFETY: it returns a NUL-terminated string.
        unsafe { puts(version) };
        0
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn main() {}
