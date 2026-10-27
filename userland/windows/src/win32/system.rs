//! The system: `GetSystemInfo` (the CPU architecture, the page size and allocation
//! granularity, the CPUs the process may use) and the Windows version a program sees, which is
//! Windows 11's (10.0, build 26100) whether or not its manifest asks for it.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::linux;

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

pub(super) const EXPORTS: &[Export] = exports![
    b"GetSystemInfo" => get_system_info,
    b"GetNativeSystemInfo" => get_system_info,
    b"GetVersionExW" => get_version_ex_w,
    b"GetVersion" => get_version,
];

pub(super) const NTDLL_EXPORTS: &[Export] = exports![
    b"RtlGetVersion" => rtl_get_version,
];
