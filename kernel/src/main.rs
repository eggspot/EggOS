//! The EggOS kernel image.
//!
//! On bare-metal targets this is the kernel binary: the HAL's `_start` sets up a stack and
//! calls [`kernel_main`]. On the development host it is only a stub, so `cargo build` works
//! everywhere.

#![cfg_attr(target_os = "none", no_std, no_main)]

#[cfg(target_os = "none")]
mod bare_metal {
    use core::fmt::Write;
    use eggos_hal::arch::CurrentPlatform;
    use eggos_hal::{ExitStatus, Platform};
    use eggos_kernel::console::PlatformConsole;

    /// Called by the architecture entry point once a stack is set up and `.bss` is zeroed.
    #[unsafe(no_mangle)]
    extern "C" fn kernel_main() -> ! {
        let mut console = PlatformConsole::<CurrentPlatform>::new();
        let status = eggos_kernel::boot(&mut console, CurrentPlatform::ARCH);
        finish(status)
    }

    /// With `qemu-test`, power off so the test runner sees the result; otherwise stay up.
    fn finish(status: ExitStatus) -> ! {
        if cfg!(feature = "qemu-test") {
            CurrentPlatform::exit(status)
        } else {
            CurrentPlatform::halt()
        }
    }

    #[panic_handler]
    fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
        let mut console = PlatformConsole::<CurrentPlatform>::new();
        // Nothing useful can be done if the console itself fails while panicking.
        let _ = writeln!(console, "\nkernel panic: {info}");
        let _ = writeln!(console, "{}", eggos_kernel::selftest::FAIL_MARKER);
        finish(ExitStatus::Failure)
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("eggos-kernel is a bare-metal binary: build and boot it with `cargo xtask qemu`.");
    std::process::exit(1);
}
