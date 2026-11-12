//! The boot clock: ticks from [`boot`](crate::boot), for the serial console's
//! `boot: desktop <ms>` line (D222) and, in kernels built with the `qemu-boot-clock` feature,
//! the benchmark lines `cargo xtask bench` reads (D116): `boot_to_prompt` at the first time a
//! program waits for console input, the shell at its first prompt, and `boot_to_desktop` when
//! the frame that ends the boot screen is composed, each written once.

use eggos_hal::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use eggos_hal::Platform;

/// Must match `xtask`'s benchmark marker.
pub const MARKER: &str = "EGGOS-BENCH:";

/// The benchmark's name in its line.
pub const NAME: &str = "boot_to_prompt";

/// The idle memory's name in its line (D203): the bytes of memory in use when the shell first
/// waits at its prompt.
pub const IDLE_MEMORY: &str = "idle_memory_bytes";

/// The first desktop frame's name in its line (D222): the time from the kernel's entry to the
/// frame that ends the boot screen.
pub const DESKTOP: &str = "boot_to_desktop";

/// What the serial console's desktop line starts with, the milliseconds after it (D222).
pub const DESKTOP_LINE_PREFIX: &str = "boot: desktop ";

static STARTED: AtomicU64 = AtomicU64::new(0);

/// Starts the clock: called first thing in [`boot`](crate::boot).
pub fn start<P: Platform>() {
    STARTED.store(P::clock_ticks(), Ordering::Relaxed);
}

/// The milliseconds since [`start`].
pub fn milliseconds_since_start<P: Platform>() -> u64 {
    let ticks = P::clock_ticks().saturating_sub(STARTED.load(Ordering::Relaxed));
    (u128::from(ticks) * 1000 / u128::from(P::clock_frequency().max(1))) as u64
}

#[cfg(feature = "qemu-boot-clock")]
mod lines {
    use super::{DESKTOP, IDLE_MEMORY, MARKER, NAME, STARTED};
    use alloc::format;
    use eggos_hal::atomic::AtomicU64;
    use core::sync::atomic::{AtomicBool, Ordering};
    use eggos_hal::Platform;

    static REPORTED: AtomicBool = AtomicBool::new(false);
    static DESKTOP_REPORTED: AtomicBool = AtomicBool::new(false);
    static WAITED_TICKS: AtomicU64 = AtomicU64::new(0);
    static WAITED_MEMORY: AtomicU64 = AtomicU64::new(0);

    fn since_start<P: Platform>() -> u64 {
        P::clock_ticks()
            .saturating_sub(STARTED.load(Ordering::Relaxed))
            .max(1)
    }

    fn write_first_wait<P: Platform>(ticks: u64, used: u64) {
        let memory = format!(
            "\r\n{MARKER} {IDLE_MEMORY} iterations=1 ticks={used} frequency=1000000000\r\n"
        );
        P::console_write(memory.as_bytes());
        let line = format!(
            "{MARKER} {NAME} iterations=1 ticks={ticks} frequency={}\r\n",
            P::clock_frequency()
        );
        P::console_write(line.as_bytes());
    }

    /// Writes `EGGOS-BENCH: idle_memory_bytes iterations=1 ticks=<bytes in use>
    /// frequency=1000000000` and `EGGOS-BENCH: boot_to_prompt iterations=1 ticks=<since start>
    /// frequency=<Hz>`, each on a line of its own, the first time it is called, and nothing
    /// after. While the boot screen still shows, the lines wait for [`report_desktop`], so
    /// `boot_to_prompt` stays the last line a boot-clock run writes.
    pub fn report_first_wait<P: Platform>() {
        if REPORTED.swap(true, Ordering::Relaxed) {
            return;
        }
        let ticks = since_start::<P>();
        let free = crate::memory::FRAMES
            .lock()
            .as_ref()
            .map_or(0, |frames| frames.free_frames());
        let used =
            crate::memory::total_frames().saturating_sub(free) * eggos_hal::memory::FRAME_SIZE;
        if crate::terminal::boot_screen_up() && !DESKTOP_REPORTED.load(Ordering::Acquire) {
            WAITED_MEMORY.store(used, Ordering::Relaxed);
            WAITED_TICKS.store(ticks, Ordering::Release);
            if !DESKTOP_REPORTED.load(Ordering::Acquire) {
                return;
            }
            if WAITED_TICKS.swap(0, Ordering::AcqRel) == 0 {
                return;
            }
        }
        write_first_wait::<P>(ticks, used);
    }

    /// Writes `EGGOS-BENCH: boot_to_desktop iterations=1 ticks=<since start> frequency=<Hz>`
    /// once, when the frame that ends the boot screen was composed, then the first wait's lines
    /// if they were waiting for it.
    pub fn report_desktop<P: Platform>() {
        if DESKTOP_REPORTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let line = format!(
            "\r\n{MARKER} {DESKTOP} iterations=1 ticks={} frequency={}\r\n",
            since_start::<P>(),
            P::clock_frequency()
        );
        P::console_write(line.as_bytes());
        let waited = WAITED_TICKS.swap(0, Ordering::AcqRel);
        if waited != 0 {
            write_first_wait::<P>(waited, WAITED_MEMORY.load(Ordering::Relaxed));
        }
    }
}

#[cfg(feature = "qemu-boot-clock")]
pub use lines::{report_desktop, report_first_wait};
