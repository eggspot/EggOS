//! Drives and volumes (D178): EggOS has one drive, `C:`, which is `/` (D118), so
//! `GetDriveTypeW` calls it fixed and every other letter missing; free space and a volume's
//! file system, longest name and serial number come from `statfs` on the path a program
//! names, whose file system may be any mounted under `/`. Volumes have no labels, there are no
//! DOS device names, and no device takes `DeviceIoControl` requests yet.

use super::{Bool, FALSE, TRUE, error, error_for, fail};
use crate::library::Export;
use crate::linux::{self, STATFS_WORDS};
use crate::text::PATH_CAPACITY;

const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_FIXED: u32 = 3;
const BYTES_PER_SECTOR: u64 = 512;
const FILE_CASE_SENSITIVE_SEARCH: u32 = 0x1;
const FILE_CASE_PRESERVED_NAMES: u32 = 0x2;
const FILE_UNICODE_ON_DISK: u32 = 0x4;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
const MOUNTED_READ_ONLY: u64 = 0x1;

mod word {
    pub const TYPE: usize = 0;
    pub const BLOCK_SIZE: usize = 1;
    pub const BLOCKS: usize = 2;
    pub const FREE: usize = 3;
    pub const AVAILABLE: usize = 4;
    pub const ID: usize = 7;
    pub const LONGEST_NAME: usize = 8;
    pub const FLAGS: usize = 10;
}

fn file_system_name(kind: u64) -> &'static str {
    match kind {
        0x4567_6746 => "EggFS",
        0x4d44 => "FAT",
        0x0102_1994 => "tmpfs",
        0x8584_58f6 => "rootfs",
        0x9fa0 => "proc",
        _ => "unknown",
    }
}

unsafe fn volume_of(root: *const u16) -> Result<[u64; STATFS_WORDS], u32> {
    let mut path = [0u8; PATH_CAPACITY];
    let length = match root.is_null() {
        true => {
            path[0] = b'.';
            2
        }
        // SAFETY: the callers' contract: a NUL-terminated path.
        false => unsafe { super::directory::eggos_path(root, &mut path) }
            .ok_or(error::INVALID_PARAMETER)?,
    };
    if length == 1 {
        path[..2].copy_from_slice(b"/\0");
    }
    let path = &path[..length.max(2)];
    linux::file_system(path).map_err(|errno| super::directory::path_error(path, errno))
}

fn block_size(words: &[u64; STATFS_WORDS]) -> u64 {
    words[word::BLOCK_SIZE].max(1)
}

unsafe extern "efiapi" fn get_disk_free_space_ex_w(
    root: *const u16,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> Bool {
    // SAFETY: `GetDiskFreeSpaceExW`'s contract: a NUL-terminated folder, or null.
    let words = match unsafe { volume_of(root) } {
        Ok(words) => words,
        Err(code) => return fail(code, FALSE),
    };
    let size = block_size(&words);
    for (out, blocks) in [
        (available, words[word::AVAILABLE]),
        (total, words[word::BLOCKS]),
        (free, words[word::FREE]),
    ] {
        if !out.is_null() {
            // SAFETY: the caller passes writable `ULARGE_INTEGER`s, or nulls.
            unsafe { out.write_unaligned(blocks.saturating_mul(size)) };
        }
    }
    TRUE
}

unsafe extern "efiapi" fn get_disk_free_space_w(
    root: *const u16,
    sectors_per_cluster: *mut u32,
    bytes_per_sector: *mut u32,
    free_clusters: *mut u32,
    clusters: *mut u32,
) -> Bool {
    // SAFETY: `GetDiskFreeSpaceW`'s contract: a NUL-terminated root, or null.
    let words = match unsafe { volume_of(root) } {
        Ok(words) => words,
        Err(code) => return fail(code, FALSE),
    };
    let sectors = (block_size(&words) / BYTES_PER_SECTOR).max(1);
    let clamp = |value: u64| value.min(u64::from(u32::MAX)) as u32;
    for (out, value) in [
        (sectors_per_cluster, clamp(sectors)),
        (bytes_per_sector, BYTES_PER_SECTOR as u32),
        (free_clusters, clamp(words[word::AVAILABLE])),
        (clusters, clamp(words[word::BLOCKS])),
    ] {
        if !out.is_null() {
            // SAFETY: the caller passes writable `DWORD`s, or nulls.
            unsafe { out.write(value) };
        }
    }
    TRUE
}

unsafe extern "efiapi" fn get_drive_type_w(root: *const u16) -> u32 {
    if root.is_null() {
        return DRIVE_FIXED;
    }
    // SAFETY: `GetDriveTypeW`'s contract: a NUL-terminated root.
    let root = unsafe { super::file::wide_string(root) };
    let is_letter = |unit: u16, letter: u8| {
        unit == u16::from(letter) || unit == u16::from(letter.to_ascii_lowercase())
    };
    let separator = |unit: u16| unit == u16::from(b'\\') || unit == u16::from(b'/');
    match root {
        [drive, colon] | [drive, colon, _]
            if is_letter(*drive, b'C') && *colon == u16::from(b':') =>
        {
            match root.get(2) {
                Some(unit) if !separator(*unit) => DRIVE_NO_ROOT_DIR,
                _ => DRIVE_FIXED,
            }
        }
        [slash] if separator(*slash) => DRIVE_FIXED,
        _ => DRIVE_NO_ROOT_DIR,
    }
}

unsafe fn write_name(name: &str, buffer: *mut u16, size: u32) -> Result<(), u32> {
    if buffer.is_null() {
        return Ok(());
    }
    let mut units = [0u16; 16];
    let mut length = 0;
    for unit in name.encode_utf16() {
        units[length] = unit;
        length += 1;
    }
    // SAFETY: the callers' contract: `buffer` holds `size` writable units.
    let out = unsafe { super::file::wide_buffer(buffer, size as usize) };
    super::file::copy_terminated(&units[..length], out)
        .map(|_| ())
        .map_err(|_| error::INSUFFICIENT_BUFFER)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn get_volume_information_w(
    root: *const u16,
    name: *mut u16,
    name_size: u32,
    serial: *mut u32,
    longest_component: *mut u32,
    flags: *mut u32,
    file_system: *mut u16,
    file_system_size: u32,
) -> Bool {
    // SAFETY: `GetVolumeInformationW`'s contract: a NUL-terminated root, or null.
    let words = match unsafe { volume_of(root) } {
        Ok(words) => words,
        Err(code) => return fail(code, FALSE),
    };
    let mut volume_flags =
        FILE_CASE_SENSITIVE_SEARCH | FILE_CASE_PRESERVED_NAMES | FILE_UNICODE_ON_DISK;
    if words[word::FLAGS] & MOUNTED_READ_ONLY != 0 {
        volume_flags |= FILE_READ_ONLY_VOLUME;
    }
    for (out, value) in [
        (serial, words[word::ID] as u32),
        (longest_component, words[word::LONGEST_NAME] as u32),
        (flags, volume_flags),
    ] {
        if !out.is_null() {
            // SAFETY: the caller passes writable `DWORD`s, or nulls.
            unsafe { out.write(value) };
        }
    }
    // SAFETY: the caller passes a buffer of the size it gives, or null.
    let named = unsafe { write_name("", name, name_size) };
    // SAFETY: as above.
    let written = named.and_then(|()| unsafe {
        write_name(
            file_system_name(words[word::TYPE]),
            file_system,
            file_system_size,
        )
    });
    match written {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn set_volume_label_w(_root: *const u16, _label: *const u16) -> Bool {
    fail(error::NOT_SUPPORTED, FALSE)
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn device_io_control(
    device: usize,
    _code: u32,
    _input: usize,
    _input_size: u32,
    _output: usize,
    _output_size: u32,
    _returned: usize,
    _overlapped: usize,
) -> Bool {
    match super::file::fd_of(device) {
        Some(fd) => match linux::fstat(fd) {
            Ok(_) => fail(error::INVALID_FUNCTION, FALSE),
            Err(errno) => fail(error_for(errno), FALSE),
        },
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn query_dos_device_w(_name: *const u16, _buffer: *mut u16, _size: u32) -> u32 {
    fail(error::NOT_SUPPORTED, 0)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetDiskFreeSpaceExW" => get_disk_free_space_ex_w,
    b"GetDiskFreeSpaceW" => get_disk_free_space_w,
    b"GetDriveTypeW" => get_drive_type_w,
    b"GetVolumeInformationW" => get_volume_information_w,
    b"SetVolumeLabelW" => set_volume_label_w,
    b"DeviceIoControl" => device_io_control,
    b"QueryDosDeviceW" => query_dos_device_w,
];
