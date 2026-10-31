//! A Windows console program that imports a function the Windows layer does not provide
//! (`CreateMailslotW`), so the layer must refuse it with a clear message and a non-zero status
//! before any of its code runs (D118).

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
mod program {
    use core::ffi::c_void;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn ExitProcess(code: u32) -> !;
        fn CreateMailslotW(
            name: *const u16,
            largest_message: u32,
            read_timeout: u32,
            security: *mut c_void,
        ) -> *mut c_void;
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: `ExitProcess` ends the process.
        unsafe { ExitProcess(101) }
    }

    #[unsafe(no_mangle)]
    extern "system" fn mainCRTStartup() -> u32 {
        // SAFETY: never reached: the layer refuses the program before its entry point runs.
        unsafe { CreateMailslotW(core::ptr::null(), 0, 0, core::ptr::null_mut()) as usize as u32 }
    }
}

#[cfg(any(not(windows), test))]
fn main() {}
