//! The bootable EggOS kernel image: joins the HAL of the target architecture
//! to the architecture-independent kernel core. Build it with
//! `cargo xtask build`; it does not build for a development host.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use eggos_hal::{Platform, arch};

/// Called by the architecture's boot code once there is a stack and `.bss` is
/// zeroed. What `boot_info` holds depends on the architecture; see its
/// `arch::init`.
#[unsafe(no_mangle)]
extern "C" fn eggos_kernel_start(boot_info: usize) -> ! {
    let machine = arch::init(boot_info);
    eggos_kernel_core::boot(&machine);
    machine.halt()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    eggos_kernel_core::report_panic(&arch::emergency_console(), info);
    arch::halt()
}
