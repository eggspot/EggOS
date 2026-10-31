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

/// The idle memory's name in its line (D203): the bytes of memory in use when the shell first
/// waits at its prompt.
pub const IDLE_MEMORY: &str = "idle_memory_bytes";

static STARTED: AtomicU64 = AtomicU64::new(0);

static REPORTED: AtomicBool = AtomicBool::new(false);

/// Starts the clock: called first thing in [`boot`](crate::boot).
pub fn start<P: Platform>() {
    STARTED.store(P::clock_ticks(), Ordering::Relaxed);
}

/// Writes `EGGOS-BENCH: idle_memory_bytes iterations=1 ticks=<bytes in use> frequency=1000000000`
/// and `EGGOS-BENCH: boot_to_prompt iterations=1 ticks=<since start> frequency=<Hz>`, each on a
/// line of its own, the first time it is called, and nothing after.
pub fn report_first_wait<P: Platform>() {
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let ticks = P::clock_ticks().saturating_sub(STARTED.load(Ordering::Relaxed));
    let free = crate::memory::FRAMES
        .lock()
        .as_ref()
        .map_or(0, |frames| frames.free_frames());
    let used = crate::memory::total_frames().saturating_sub(free) * eggos_hal::memory::FRAME_SIZE;
    let memory =
        format!("\r\n{MARKER} {IDLE_MEMORY} iterations=1 ticks={used} frequency=1000000000\r\n");
    P::console_write(memory.as_bytes());
    let line = format!(
        "{MARKER} {NAME} iterations=1 ticks={ticks} frequency={}\r\n",
        P::clock_frequency()
    );
    P::console_write(line.as_bytes());
}
