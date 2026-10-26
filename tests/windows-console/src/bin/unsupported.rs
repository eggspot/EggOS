//! A Windows console program that imports a function the Windows layer does not provide
//! (`CreateProcessW`), so the layer must refuse it with a clear message and a non-zero status
//! before any of its code runs (D118).

#![cfg_attr(windows, no_std, no_main)]

#[cfg(windows)]
mod program {
    use core::ffi::c_void;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn ExitProcess(code: u32) -> !;
        fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *mut c_void,
            thread_attributes: *mut c_void,
            inherit_handles: i32,
            flags: u32,
            environment: *mut c_void,
            directory: *const u16,
            startup: *mut c_void,
            information: *mut c_void,
        ) -> i32;
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: `ExitProcess` ends the process.
        unsafe { ExitProcess(101) }
    }

    #[unsafe(no_mangle)]
    extern "system" fn mainCRTStartup() -> u32 {
        let null = core::ptr::null_mut::<c_void>();
        // SAFETY: never reached: the layer refuses the program before its entry point runs.
        unsafe {
            CreateProcessW(
                null.cast(),
                null.cast(),
                null,
                null,
                0,
                0,
                null,
                null.cast(),
                null,
                null,
            ) as u32
        }
    }
}

#[cfg(not(windows))]
fn main() {}
