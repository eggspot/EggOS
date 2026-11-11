//! OPAL, skiboot's runtime firmware interface (D304): calls in hypervisor real mode, big-endian,
//! with the token in `r0`, the arguments in `r3` on, `r2` OPAL's base, and physical addresses
//! for every pointer. The call switches the CPU to that mode and back; what it passes by
//! pointer lives in this module's statics, which are in the kernel image, so their physical
//! address is their virtual one less the kernel's offset.

use super::KERNEL_VIRTUAL_OFFSET;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

unsafe extern "C" {
    /// Calls OPAL `token` with up to five arguments; its return value.
    fn ppc64_opal_call(token: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64;
}

/// OPAL's base and entry point, from `r8` and `r9` at boot.
pub(super) static OPAL_BASE: AtomicU64 = AtomicU64::new(0);
pub(super) static OPAL_ENTRY: AtomicU64 = AtomicU64::new(0);

const CONSOLE_WRITE: u64 = 1;
const CONSOLE_READ: u64 = 2;
const CEC_POWER_DOWN: u64 = 5;
const CEC_REBOOT: u64 = 6;
const POLL_EVENTS: u64 = 10;
const START_CPU: u64 = 41;
const SUCCESS: i64 = 0;
const BUSY_EVENT: i64 = -12;
const CONSOLE_TERMINAL: u64 = 0;
const BUFFER_BYTES: usize = 256;

/// The console's buffer and the length word OPAL reads and writes, big-endian.
static mut BUFFER: [u8; BUFFER_BYTES] = [0; BUFFER_BYTES];
static mut LENGTH: u64 = 0;
static CONSOLE_HELD: AtomicBool = AtomicBool::new(false);

fn physical<T>(pointer: *const T) -> u64 {
    (pointer as u64).wrapping_sub(KERNEL_VIRTUAL_OFFSET)
}

fn call(token: u64, arguments: [u64; 5]) -> i64 {
    if OPAL_ENTRY.load(Ordering::Relaxed) == 0 {
        return -1;
    }
    let [a, b, c, d, e] = arguments;
    // SAFETY: OPAL was handed to the kernel at boot and is reserved in the memory map; the call
    // runs it in the mode it expects and comes back with every register the ABI preserves.
    unsafe { ppc64_opal_call(token, a, b, c, d, e) }
}

/// OPAL's base, 0 before boot set it.
pub(super) fn base() -> u64 {
    OPAL_BASE.load(Ordering::Relaxed)
}

fn hold_console() {
    while CONSOLE_HELD
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn release_console() {
    CONSOLE_HELD.store(false, Ordering::Release);
}

/// Writes `bytes` to OPAL's console 0, polling OPAL while it is busy.
pub(super) fn console_write(bytes: &[u8]) {
    hold_console();
    for chunk in bytes.chunks(BUFFER_BYTES) {
        let mut written = 0;
        while written < chunk.len() {
            let rest = &chunk[written..];
            // SAFETY: the console lock is held, so only this CPU uses the buffer and the length.
            let (buffer, length) = unsafe {
                let buffer = &raw mut BUFFER;
                (*buffer)[..rest.len()].copy_from_slice(rest);
                let length = &raw mut LENGTH;
                *length = (rest.len() as u64).to_be();
                (buffer as *const u8, length as *const u64)
            };
            let status = call(CONSOLE_WRITE, [CONSOLE_TERMINAL, physical(length), physical(buffer), 0, 0]);
            // SAFETY: as above; OPAL wrote the count it took back.
            let taken = u64::from_be(unsafe { *(&raw const LENGTH) }) as usize;
            if taken > 0 {
                written += taken.min(rest.len());
            } else if status == SUCCESS || status == BUSY_EVENT {
                call(POLL_EVENTS, [0; 5]);
            } else {
                break;
            }
        }
    }
    release_console();
}

/// Reads what OPAL's console 0 has into `buffer`: how many bytes.
pub(super) fn console_read(buffer: &mut [u8]) -> usize {
    hold_console();
    let wanted = buffer.len().min(BUFFER_BYTES);
    // SAFETY: the console lock is held.
    let (source, length) = unsafe {
        let length = &raw mut LENGTH;
        *length = (wanted as u64).to_be();
        (&raw const BUFFER as *const u8, length as *const u64)
    };
    let status = call(CONSOLE_READ, [CONSOLE_TERMINAL, physical(length), physical(source), 0, 0]);
    // SAFETY: as above; OPAL wrote the count it read back.
    let read = match status {
        SUCCESS => (u64::from_be(unsafe { *(&raw const LENGTH) }) as usize).min(wanted),
        _ => 0,
    };
    // SAFETY: as above.
    buffer[..read].copy_from_slice(unsafe { &(*(&raw const BUFFER))[..read] });
    release_console();
    read
}

/// Asks OPAL to power the machine off.
pub(super) fn power_down() {
    call(CEC_POWER_DOWN, [0; 5]);
}

/// Asks OPAL to reset the machine.
pub(super) fn reboot() {
    call(CEC_REBOOT, [0; 5]);
}

/// Asks OPAL to start the thread whose server number is `cpu` at physical `entry`; whether it
/// took the request.
pub(super) fn start_cpu(cpu: u64, entry: u64) -> bool {
    call(START_CPU, [cpu, entry, 0, 0, 0]) == SUCCESS
}
