//! The boot-to-prompt clock that `cargo xtask bench` reads (D116), only in kernels built with
//! the `qemu-boot-clock` feature: the clock ticks from [`boot`](crate::boot) to the first time
//! a program waits for console input, the shell at its first prompt, written once as a
//! benchmark line.

use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use eggos_hal::Platform;

/// Must match `xtask`'s benchmark marker.
pub const MARKER: &str = "EGGOS-BENCH:";

/// The benchmark's name in its line.
pub const NAME: &str = "boot_to_prompt";

static STARTED: AtomicU64 = AtomicU64::new(0);

static REPORTED: AtomicBool = AtomicBool::new(false);

/// Starts the clock: called first thing in [`boot`](crate::boot).
pub fn start<P: Platform>() {
    STARTED.store(P::clock_ticks(), Ordering::Relaxed);
}

/// Writes `EGGOS-BENCH: boot_to_prompt iterations=1 ticks=<since start> frequency=<Hz>` on a
/// line of its own the first time it is called, and nothing after.
pub fn report_first_wait<P: Platform>() {
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let ticks = P::clock_ticks().saturating_sub(STARTED.load(Ordering::Relaxed));
    let line = format!(
        "\r\n{MARKER} {NAME} iterations=1 ticks={ticks} frequency={}\r\n",
        P::clock_frequency()
    );
    P::console_write(line.as_bytes());
}
