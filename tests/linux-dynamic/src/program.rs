use core::ffi::{CStr, c_char, c_int};

#[cfg(target_env = "gnu")]
const LIBRARY: &CStr = c"glibc";
#[cfg(target_env = "musl")]
const LIBRARY: &CStr = c"musl";
#[cfg(not(any(target_env = "gnu", target_env = "musl")))]
const LIBRARY: &CStr = c"a C library";

#[link(name = "c")]
unsafe extern "C" {
    fn printf(format: *const c_char, ...) -> c_int;
    fn abort() -> !;
}

#[unsafe(no_mangle)]
extern "C" fn main(argc: c_int, _argv: *const *const c_char) -> c_int {
    let format = c"LINUX-DYNAMIC %s hello, %d arguments\n";
    // SAFETY: a NUL-terminated format whose two conversions take the two arguments given, a
    // NUL-terminated string and an `int`.
    let printed = unsafe { printf(format.as_ptr(), LIBRARY.as_ptr(), argc) };
    c_int::from(printed < 0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ends the program at once; nothing it holds needs anything more.
    unsafe { abort() }
}
