//! The EggOS kernel image.
//!
//! On bare-metal targets this is the kernel binary: the HAL's `_start` sets up a stack and
//! calls [`kernel_main`]. On the development host it is only a stub, so `cargo build` works
//! everywhere.

#![cfg_attr(target_os = "none", no_std, no_main)]

#[cfg(target_os = "none")]
mod bare_metal {
    use core::fmt::Write;
    use core::sync::atomic::{AtomicBool, Ordering};
    use eggos_hal::arch::CurrentPlatform;
    use eggos_hal::{Exception, ExitStatus, Interrupt, Platform};
    use eggos_kernel::console::PlatformConsole;
    use eggos_kernel::exception::{self, Outcome};

    /// Called by the architecture entry point once a stack is set up, `.bss` is zeroed and the
    /// exception vectors are installed.
    #[unsafe(no_mangle)]
    extern "C" fn kernel_main() -> ! {
        let mut console = PlatformConsole::<CurrentPlatform>::new();
        let status = eggos_kernel::boot::<CurrentPlatform, _>(&mut console);
        if cfg!(feature = "qemu-test-fatal-exception") && status == ExitStatus::Success {
            // Tests the fatal path and write protection (D29): writing the kernel's read-only
            // data must fault, and the handler must report the fault at that address and fail
            // the run. The QEMU runner checks the address.
            let target = CurrentPlatform::kernel_image().rodata_start;
            let _ = writeln!(
                console,
                "{}{target:#x}",
                eggos_kernel::selftest::WRITE_PROTECT_PROBE
            );
            // SAFETY: `target` is the first word of `.rodata`, which the kernel's page tables
            // map read-only, so the store faults and the fatal exception handler never returns.
            // Should write protection be broken, the store lands and the kernel stops at once
            // with an illegal instruction, which the runner rejects: no fault names `target`.
            unsafe { (target as *mut u64).write_volatile(0) };
            CurrentPlatform::execute_illegal_instruction();
        }
        finish(status)
    }

    /// Set once a fatal exception is being reported.
    static IN_FATAL_EXCEPTION: AtomicBool = AtomicBool::new(false);

    /// Called by the HAL's exception vectors (declared in `eggos_hal`). Returning resumes the
    /// interrupted code.
    #[unsafe(no_mangle)]
    fn kernel_exception(exception: &Exception) {
        if exception::handle(exception) == Outcome::Resume {
            return;
        }
        // A second fatal exception while reporting the first (say, a broken console) would
        // otherwise recurse until the stack overflows; stop this CPU instead.
        if IN_FATAL_EXCEPTION.swap(true, Ordering::Relaxed) {
            CurrentPlatform::halt();
        }
        let mut console = PlatformConsole::<CurrentPlatform>::new();
        // Nothing useful can be done if the console itself fails here.
        let _ = writeln!(console);
        let _ = exception::report(&mut console, exception);
        let _ = writeln!(console, "{}", eggos_kernel::selftest::FAIL_MARKER);
        finish(ExitStatus::Failure)
    }

    /// Called by the HAL's interrupt handling (declared in `eggos_hal`). Returning resumes the
    /// interrupted code.
    #[unsafe(no_mangle)]
    fn kernel_interrupt(interrupt: Interrupt) {
        eggos_kernel::interrupt::handle(interrupt);
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
