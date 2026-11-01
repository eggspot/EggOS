//! The system: `GetSystemInfo` (the CPU architecture, the page size and allocation
//! granularity, the CPUs the process may use) and the Windows version a program sees, which is
//! Windows 11's (10.0, build 26100) whether or not its manifest asks for it. The computer's
//! name is the host name in capitals, the Windows and system folders are `/usr/lib/eggos`,
//! where the layer lives, pointers are encoded with a secret of the process's (D178), no
//! debugger listens to `OutputDebugStringW` and there is no beeper for `Beep`.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::linux;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[cfg(target_arch = "x86_64")]
const PROCESSOR_ARCHITECTURE: u16 = 9;
#[cfg(target_arch = "x86_64")]
const PROCESSOR_TYPE: u32 = 8664;
#[cfg(target_arch = "aarch64")]
const PROCESSOR_ARCHITECTURE: u16 = 12;
#[cfg(target_arch = "aarch64")]
const PROCESSOR_TYPE: u32 = 0;
const ALLOCATION_GRANULARITY: u32 = 64 << 10;
const MINIMUM_APPLICATION_ADDRESS: u64 = 0x1_0000;
const MAXIMUM_APPLICATION_ADDRESS: u64 = 0x7fff_fffe_ffff;
const SYSTEM_INFO_SIZE: usize = 48;
const MAJOR_VERSION: u32 = 10;
const MINOR_VERSION: u32 = 0;
const BUILD_NUMBER: u32 = 26100;
const VER_PLATFORM_WIN32_NT: u32 = 2;
const VER_NT_WORKSTATION: u8 = 1;
const VERSION_INFO_SIZE: u32 = 276;
const VERSION_INFO_EX_SIZE: u32 = 284;
const STATUS_SUCCESS: i32 = 0;
const STATUS_INVALID_PARAMETER: i32 = 0xc000_000du32 as i32;

unsafe extern "efiapi" fn get_system_info(info: *mut u8) {
    let mut out = [0u8; SYSTEM_INFO_SIZE];
    let cpus = linux::cpu_count();
    out[0..2].copy_from_slice(&PROCESSOR_ARCHITECTURE.to_le_bytes());
    out[4..8].copy_from_slice(&(linux::PAGE_SIZE as u32).to_le_bytes());
    out[8..16].copy_from_slice(&MINIMUM_APPLICATION_ADDRESS.to_le_bytes());
    out[16..24].copy_from_slice(&MAXIMUM_APPLICATION_ADDRESS.to_le_bytes());
    let mask = if cpus >= 64 {
        u64::MAX
    } else {
        (1u64 << cpus) - 1
    };
    out[24..32].copy_from_slice(&mask.to_le_bytes());
    out[32..36].copy_from_slice(&cpus.to_le_bytes());
    out[36..40].copy_from_slice(&PROCESSOR_TYPE.to_le_bytes());
    out[40..44].copy_from_slice(&ALLOCATION_GRANULARITY.to_le_bytes());
    out[44..46].copy_from_slice(&6u16.to_le_bytes());
    // SAFETY: `GetSystemInfo`'s contract: a writable `SYSTEM_INFO`.
    unsafe { info.copy_from_nonoverlapping(out.as_ptr(), SYSTEM_INFO_SIZE) };
}

unsafe fn fill_version(info: *mut u8) -> bool {
    // SAFETY: the caller's duty: the size field comes first.
    let size = unsafe { info.cast::<u32>().read_unaligned() };
    if size != VERSION_INFO_SIZE && size != VERSION_INFO_EX_SIZE {
        return false;
    }
    let mut out = [0u8; VERSION_INFO_EX_SIZE as usize];
    out[0..4].copy_from_slice(&size.to_le_bytes());
    out[4..8].copy_from_slice(&MAJOR_VERSION.to_le_bytes());
    out[8..12].copy_from_slice(&MINOR_VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&BUILD_NUMBER.to_le_bytes());
    out[16..20].copy_from_slice(&VER_PLATFORM_WIN32_NT.to_le_bytes());
    out[282] = VER_NT_WORKSTATION;
    // SAFETY: as above: `size` writable bytes.
    unsafe { info.copy_from_nonoverlapping(out.as_ptr(), size as usize) };
    true
}

unsafe extern "efiapi" fn get_version_ex_w(info: *mut u8) -> Bool {
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `GetVersionExW`'s contract.
    match unsafe { fill_version(info) } {
        true => TRUE,
        false => fail(error::INSUFFICIENT_BUFFER, FALSE),
    }
}

unsafe extern "efiapi" fn rtl_get_version(info: *mut u8) -> i32 {
    if info.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: `RtlGetVersion`'s contract.
    match unsafe { fill_version(info) } {
        true => STATUS_SUCCESS,
        false => STATUS_INVALID_PARAMETER,
    }
}

extern "efiapi" fn get_version() -> u32 {
    BUILD_NUMBER << 16 | MINOR_VERSION << 8 | MAJOR_VERSION
}

/// `MulDiv`: `number * numerator / denominator` with a 64-bit product, rounded half away from
/// zero; -1 for a zero denominator or a result past 32 bits (D174).
pub fn multiply_divide(number: i32, numerator: i32, denominator: i32) -> i32 {
    if denominator == 0 {
        return -1;
    }
    let product = i64::from(number) * i64::from(numerator);
    let denominator = i64::from(denominator);
    let half = denominator.abs() / 2;
    let rounded = match (product < 0) == (denominator < 0) {
        true => (product.abs() + half) / denominator.abs(),
        false => -((product.abs() + half) / denominator.abs()),
    };
    i32::try_from(rounded).unwrap_or(-1)
}

extern "efiapi" fn mul_div(number: i32, numerator: i32, denominator: i32) -> i32 {
    multiply_divide(number, numerator, denominator)
}

/// The folder Windows' and its system folder are on EggOS: where the layer's own libraries
/// live (D178).
pub(super) const WINDOWS_FOLDER: &[u8] = b"/usr/lib/eggos";
const HOST_NAME_FILES: [&[u8]; 2] = [b"/proc/sys/kernel/hostname\0", b"/etc/hostname\0"];
const MAX_COMPUTER_NAME: usize = 15;
pub(super) const HOST_NAME_CAPACITY: usize = 64;

/// The machine's host name (`uname`'s node name) into `out`; its length, 0 if it has none.
pub(super) fn host_name(out: &mut [u8; HOST_NAME_CAPACITY]) -> usize {
    for file in HOST_NAME_FILES {
        let Ok(read) = linux::read_file(file, out) else {
            continue;
        };
        let name = out[..read]
            .split(|byte| byte.is_ascii_whitespace())
            .next()
            .unwrap_or(&[]);
        let length = name.len();
        if length > 0 {
            return length;
        }
    }
    0
}

unsafe extern "efiapi" fn get_computer_name_w(buffer: *mut u16, size: *mut u32) -> Bool {
    if size.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let mut name = [0u8; HOST_NAME_CAPACITY];
    let length = host_name(&mut name).min(MAX_COMPUTER_NAME);
    let mut units = [0u16; MAX_COMPUTER_NAME];
    for (unit, byte) in units.iter_mut().zip(&name[..length]) {
        *unit = u16::from(byte.to_ascii_uppercase());
    }
    // SAFETY: the caller passes a readable and writable `DWORD` for the size.
    let capacity = unsafe { size.read() };
    // SAFETY: `GetComputerNameW`'s contract: `buffer` holds that many writable units.
    let out = unsafe { super::file::wide_buffer(buffer, capacity as usize) };
    let (result, count) = match super::file::copy_terminated(&units[..length], out) {
        Ok(written) => (TRUE, written),
        Err(needed) => (fail(error::BUFFER_OVERFLOW, FALSE), needed),
    };
    // SAFETY: as above.
    unsafe { size.write(count as u32) };
    result
}

fn windows_folder_units(out: &mut [u16; 32]) -> usize {
    let mut length = 0;
    crate::text::windows_path(WINDOWS_FOLDER, &mut |c| {
        length += c.encode_utf16(&mut out[length..]).len();
    });
    length
}

unsafe extern "efiapi" fn get_system_windows_directory_w(buffer: *mut u16, size: u32) -> u32 {
    let mut units = [0u16; 32];
    let length = windows_folder_units(&mut units);
    // SAFETY: the caller passes `size` writable units, or null.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    match super::file::copy_terminated(&units[..length], out) {
        Ok(written) | Err(written) => written as u32,
    }
}

unsafe extern "efiapi" fn get_system_directory_a(buffer: *mut u8, size: u32) -> u32 {
    let mut units = [0u16; 32];
    let length = windows_folder_units(&mut units);
    if length >= size as usize || buffer.is_null() {
        return length as u32 + 1;
    }
    for (at, unit) in units[..length].iter().enumerate() {
        // SAFETY: the caller passes `size` writable bytes, more than the ASCII path takes.
        unsafe { buffer.wrapping_add(at).write(*unit as u8) };
    }
    // SAFETY: as above.
    unsafe { buffer.wrapping_add(length).write(0) };
    length as u32
}

extern "efiapi" fn output_debug_string_w(_text: *const u16) {}

extern "efiapi" fn beep(_frequency: u32, _duration: u32) -> Bool {
    fail(error::NOT_SUPPORTED, FALSE)
}

static ERROR_MODE: AtomicU32 = AtomicU32::new(0);

extern "efiapi" fn set_error_mode(mode: u32) -> u32 {
    ERROR_MODE.swap(mode, Ordering::Relaxed)
}

static POINTER_SECRET: AtomicUsize = AtomicUsize::new(0);

fn pointer_secret() -> usize {
    match POINTER_SECRET.load(Ordering::Relaxed) {
        0 => {
            let stack = 0u8;
            let fresh = (linux::clock_nanoseconds(linux::CLOCK_MONOTONIC) as usize
                ^ (&raw const stack as usize).rotate_left(17))
                | 1;
            match POINTER_SECRET.compare_exchange(0, fresh, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => fresh,
                Err(current) => current,
            }
        }
        secret => secret,
    }
}

extern "efiapi" fn encode_pointer(pointer: usize) -> usize {
    let secret = pointer_secret();
    (pointer ^ secret).rotate_right(secret as u32 % usize::BITS)
}

extern "efiapi" fn decode_pointer(pointer: usize) -> usize {
    let secret = pointer_secret();
    pointer.rotate_left(secret as u32 % usize::BITS) ^ secret
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetComputerNameW" => get_computer_name_w,
    b"GetSystemWindowsDirectoryW" => get_system_windows_directory_w,
    b"GetSystemDirectoryW" => get_system_windows_directory_w,
    b"GetWindowsDirectoryW" => get_system_windows_directory_w,
    b"GetSystemDirectoryA" => get_system_directory_a,
    b"OutputDebugStringW" => output_debug_string_w,
    b"Beep" => beep,
    b"SetErrorMode" => set_error_mode,
    b"EncodePointer" => encode_pointer,
    b"DecodePointer" => decode_pointer,
    b"MulDiv" => mul_div,
    b"GetSystemInfo" => get_system_info,
    b"GetNativeSystemInfo" => get_system_info,
    b"GetVersionExW" => get_version_ex_w,
    b"GetVersion" => get_version,
];

pub(super) const NTDLL_EXPORTS: &[Export] = exports![
    b"RtlGetVersion" => rtl_get_version,
];
