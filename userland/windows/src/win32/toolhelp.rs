//! Toolhelp snapshots of the processes (D178): a snapshot is `/proc/ps` opened, which the
//! kernel makes whole when it is opened (D168), so the handle is its descriptor and
//! `Process32FirstW` and `Process32NextW` read it a line at a time; a name `/proc/ps` cut is
//! made whole from the process's command line. Snapshots of threads, modules and heaps are not
//! there yet.

use super::file::{fd_of, handle_of};
use super::{Bool, FALSE, INVALID_HANDLE_VALUE, TRUE, error, error_for, fail};
use crate::library::Export;
use crate::linux::{self, O_RDONLY};
use crate::process_list::{self, CUT_NAME};

const TH32CS_SNAPPROCESS: u32 = 0x2;
const ENTRY_SIZE: usize = 568;
const NAME_UNITS: usize = 260;
const LINE_CAPACITY: usize = 256;
const SEEK_SET: usize = 0;
const SEEK_CURRENT: usize = 1;
const NORMAL_PRIORITY: i32 = 8;

mod field {
    pub const PROCESS_ID: usize = 8;
    pub const THREADS: usize = 28;
    pub const PARENT_PROCESS_ID: usize = 32;
    pub const PRIORITY: usize = 36;
    pub const EXE_FILE: usize = 44;
}

extern "efiapi" fn create_toolhelp32_snapshot(flags: u32, _process: u32) -> usize {
    if flags & TH32CS_SNAPPROCESS == 0 {
        return fail(error::NOT_SUPPORTED, INVALID_HANDLE_VALUE);
    }
    match linux::open(b"/proc/ps\0", O_RDONLY) {
        Ok(fd) => handle_of(fd),
        Err(errno) => fail(error_for(errno), INVALID_HANDLE_VALUE),
    }
}

fn process_file(id: u32, file: &[u8], out: &mut [u8; 32]) -> usize {
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        out[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    put(b"/proc/");
    let mut digits = [0u8; 10];
    let mut count = 0;
    let mut value = id;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    digits[..count].reverse();
    put(&digits[..count]);
    put(b"/");
    put(file);
    put(b"\0");
    length
}

fn thread_count(id: u32) -> u32 {
    let mut path = [0u8; 32];
    let length = process_file(id, b"status", &mut path);
    let mut status = [0u8; 1024];
    let Ok(read) = linux::read_file(&path[..length], &mut status) else {
        return 1;
    };
    status[..read]
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"Threads:"))
        .and_then(|value| core::str::from_utf8(value).ok()?.trim().parse().ok())
        .unwrap_or(1)
}

fn fill_entry(entry: &mut [u8; ENTRY_SIZE], listed: &process_list::Listed<'_>) {
    entry[4..].fill(0);
    entry[field::PROCESS_ID..field::PROCESS_ID + 4].copy_from_slice(&listed.id.to_le_bytes());
    entry[field::THREADS..field::THREADS + 4]
        .copy_from_slice(&thread_count(listed.id).to_le_bytes());
    entry[field::PARENT_PROCESS_ID..field::PARENT_PROCESS_ID + 4]
        .copy_from_slice(&listed.parent.to_le_bytes());
    entry[field::PRIORITY..field::PRIORITY + 4].copy_from_slice(&NORMAL_PRIORITY.to_le_bytes());
    let mut arguments = [0u8; 1024];
    let mut name = listed.name;
    if name.len() == CUT_NAME {
        let mut path = [0u8; 32];
        let length = process_file(listed.id, b"cmdline", &mut path);
        if let Ok(read) = linux::read_file(&path[..length], &mut arguments) {
            name = process_list::whole_name(name, &arguments[..read]).unwrap_or(name);
        }
    }
    let mut units = [0u16; NAME_UNITS - 1];
    let written = crate::text::write_utf16(name, &mut units);
    for (at, unit) in units[..written].iter().enumerate() {
        let offset = field::EXE_FILE + at * 2;
        entry[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }
}

fn next_process(fd: usize, entry: &mut [u8; ENTRY_SIZE]) -> Result<(), u32> {
    loop {
        let offset = linux::lseek(fd, 0, SEEK_CURRENT).map_err(error_for)?;
        let mut line = [0u8; LINE_CAPACITY];
        let read = linux::read(fd, &mut line).map_err(error_for)?;
        if read == 0 {
            return Err(error::NO_MORE_FILES);
        }
        let end = line[..read]
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(read);
        linux::lseek(fd, (offset + end + 1) as i64, SEEK_SET).map_err(error_for)?;
        if let Some(listed) = process_list::parse_line(&line[..end]) {
            fill_entry(entry, &listed);
            return Ok(());
        }
    }
}

unsafe fn entry_of<'a>(entry: *mut u8) -> Option<&'a mut [u8; ENTRY_SIZE]> {
    if entry.is_null() {
        return None;
    }
    // SAFETY: the callers' contract: a `PROCESSENTRY32W` starts with its size.
    let size = unsafe { entry.cast::<u32>().read_unaligned() };
    // SAFETY: as above: a structure of at least its own size, which is checked first.
    (size as usize >= ENTRY_SIZE).then(|| unsafe { &mut *entry.cast::<[u8; ENTRY_SIZE]>() })
}

unsafe fn process_entry(snapshot: usize, entry: *mut u8, first: bool) -> Bool {
    let Some(fd) = fd_of(snapshot) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    // SAFETY: the callers' contract: a writable `PROCESSENTRY32W`.
    let Some(entry) = (unsafe { entry_of(entry) }) else {
        return fail(error::BAD_LENGTH, FALSE);
    };
    if first && let Err(errno) = linux::lseek(fd, 0, SEEK_SET) {
        return fail(error_for(errno), FALSE);
    }
    match next_process(fd, entry) {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn process32_first_w(snapshot: usize, entry: *mut u8) -> Bool {
    // SAFETY: `Process32FirstW`'s contract.
    unsafe { process_entry(snapshot, entry, true) }
}

unsafe extern "efiapi" fn process32_next_w(snapshot: usize, entry: *mut u8) -> Bool {
    // SAFETY: `Process32NextW`'s contract.
    unsafe { process_entry(snapshot, entry, false) }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateToolhelp32Snapshot" => create_toolhelp32_snapshot,
    b"Process32FirstW" => process32_first_w,
    b"Process32NextW" => process32_next_w,
];
