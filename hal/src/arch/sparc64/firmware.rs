//! Open Firmware's client interface (IEEE 1275, 64-bit cells), used once at boot while its
//! traps are still in place (D326): the RAM it leaves available, the CPU's clock and the
//! command line, kept for the device tree the HAL writes.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The most RAM ranges kept.
pub(super) const MAX_RAM_RANGES: usize = 8;
const COMMAND_LINE_BYTES: usize = 512;
const PROPERTY_BYTES: usize = 16 * MAX_RAM_RANGES;
const NO_PHANDLE: u64 = 0;
const FAILED: u64 = u64::MAX;

static mut RAM: [(u64, u64); MAX_RAM_RANGES] = [(0, 0); MAX_RAM_RANGES];
static RAM_COUNT: AtomicUsize = AtomicUsize::new(0);
static mut COMMAND_LINE: [u8; COMMAND_LINE_BYTES] = [0; COMMAND_LINE_BYTES];
static COMMAND_LINE_LENGTH: AtomicUsize = AtomicUsize::new(0);
static CPU_FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Calls `service` with `arguments`; its first `returns` results, or `None` when it failed.
fn call<const ARGUMENTS: usize, const RETURNS: usize>(
    interface: u64,
    service: &core::ffi::CStr,
    arguments: [u64; ARGUMENTS],
) -> Option<[u64; RETURNS]> {
    let mut cells = [0u64; 3 + 8];
    cells[0] = service.as_ptr() as u64;
    cells[1] = ARGUMENTS as u64;
    cells[2] = RETURNS as u64;
    cells[3..3 + ARGUMENTS].copy_from_slice(&arguments);
    let result: u64;
    // SAFETY: Open Firmware's client interface takes the cell array in `%o0` and returns
    // nonzero in `%o0` on failure; it reads the array and the memory the arguments name (the
    // caller's buffers) and writes the results and those buffers; it may change the registers a
    // call may.
    unsafe {
        core::arch::asm!(
            "jmpl {entry}, %o7",
            "nop",
            entry = in(reg) interface,
            inlateout("o0") cells.as_mut_ptr() => result,
            clobber_abi("C"),
        )
    };
    if result != 0 {
        return None;
    }
    let mut returned = [0u64; RETURNS];
    returned.copy_from_slice(&cells[3 + ARGUMENTS..3 + ARGUMENTS + RETURNS]);
    Some(returned)
}

fn find_device(interface: u64, path: &core::ffi::CStr) -> Option<u64> {
    let [phandle] = call::<1, 1>(interface, c"finddevice", [path.as_ptr() as u64])?;
    (phandle != NO_PHANDLE && phandle != FAILED).then_some(phandle)
}

fn property(interface: u64, phandle: u64, name: &core::ffi::CStr, buffer: &mut [u8]) -> Option<usize> {
    let [length] = call::<4, 1>(
        interface,
        c"getprop",
        [phandle, name.as_ptr() as u64, buffer.as_mut_ptr() as u64, buffer.len() as u64],
    )?;
    (length != FAILED).then_some((length as usize).min(buffer.len()))
}

fn sibling(interface: u64, phandle: u64) -> Option<u64> {
    let [next] = call::<1, 1>(interface, c"peer", [phandle])?;
    (next != NO_PHANDLE && next != FAILED).then_some(next)
}

fn child(interface: u64, phandle: u64) -> Option<u64> {
    let [first] = call::<1, 1>(interface, c"child", [phandle])?;
    (first != NO_PHANDLE && first != FAILED).then_some(first)
}

fn big_endian_cells(bytes: &[u8], at: usize, cells: usize) -> u64 {
    (0..cells).fold(0, |value, cell| {
        let word = u32::from_be_bytes(bytes[at + cell * 4..at + cell * 4 + 4].try_into().unwrap());
        value << 32 | u64::from(word)
    })
}

/// Asks Open Firmware, whose client interface is at `interface`, for the RAM, the CPU's clock
/// and the command line. Called once, on the boot CPU, before the HAL takes the traps over.
pub(super) fn read(interface: u64) {
    if interface == 0 {
        return;
    }
    let mut buffer = [0u8; PROPERTY_BYTES];
    if let Some(memory) = find_device(interface, c"/memory")
        && let Some(length) = property(interface, memory, c"available", &mut buffer)
    {
        const ENTRY_BYTES: usize = 16;
        let mut count = 0;
        // SAFETY: only this function writes the table, once, before any reader.
        let ram = unsafe { &mut *(&raw mut RAM) };
        for entry in 0..(length / ENTRY_BYTES).min(MAX_RAM_RANGES) {
            let start = big_endian_cells(&buffer, entry * ENTRY_BYTES, 2);
            let size = big_endian_cells(&buffer, entry * ENTRY_BYTES + 8, 2);
            if size != 0 {
                ram[count] = (start, size);
                count += 1;
            }
        }
        RAM_COUNT.store(count, Ordering::Relaxed);
    }
    if let Some(chosen) = find_device(interface, c"/chosen") {
        // SAFETY: as for the table.
        let line = unsafe { &mut *(&raw mut COMMAND_LINE) };
        if let Some(length) = property(interface, chosen, c"bootargs", &mut line[..COMMAND_LINE_BYTES - 1]) {
            let end = line[..length].iter().position(|&byte| byte == 0).unwrap_or(length);
            COMMAND_LINE_LENGTH.store(end, Ordering::Relaxed);
        }
    }
    let mut node = find_device(interface, c"/").and_then(|root| child(interface, root));
    while let Some(phandle) = node {
        let mut kind = [0u8; 8];
        if property(interface, phandle, c"device_type", &mut kind).is_some() && kind.starts_with(b"cpu\0") {
            let mut clock = [0u8; 4];
            if property(interface, phandle, c"clock-frequency", &mut clock) == Some(4) {
                CPU_FREQUENCY.store(u64::from(u32::from_be_bytes(clock)), Ordering::Relaxed);
            }
            break;
        }
        node = sibling(interface, phandle);
    }
}

/// The RAM ranges Open Firmware left available.
pub(super) fn ram() -> impl Iterator<Item = (u64, u64)> {
    // SAFETY: written once by `read` before any reader, read-only after.
    let ram = unsafe { &*(&raw const RAM) };
    ram[..RAM_COUNT.load(Ordering::Relaxed)].iter().copied()
}

/// The CPU's clock, which `%tick` counts.
pub(super) fn cpu_frequency() -> Option<u64> {
    match CPU_FREQUENCY.load(Ordering::Relaxed) {
        0 => None,
        frequency => Some(frequency),
    }
}

/// `/chosen`'s `bootargs`.
pub(super) fn command_line() -> &'static [u8] {
    // SAFETY: written once by `read` before any reader, read-only after.
    let line = unsafe { &*(&raw const COMMAND_LINE) };
    &line[..COMMAND_LINE_LENGTH.load(Ordering::Relaxed)]
}
