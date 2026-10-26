//! Virtual memory (`VirtualAlloc`, `VirtualFree`, `VirtualProtect`) on Linux's `mmap`,
//! `mprotect`, `madvise` and `munmap`, and the memory functions and stack probe (`__chkstk`)
//! that ntdll exports and compilers call.

use super::{Bool, FALSE, SpinLock, TRUE, error, error_for, fail};
use crate::library::Export;
use crate::linux::{self, PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE};
use core::ptr;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_SIZE: usize = 4096;
const PAGE_READWRITE: u32 = 0x04;
const MAX_RESERVATIONS: usize = 256;

static RESERVATIONS: SpinLock<[(usize, usize); MAX_RESERVATIONS]> =
    SpinLock::new([(0, 0); MAX_RESERVATIONS]);

fn protection(protect: u32) -> Option<usize> {
    Some(match protect & 0xff {
        0x01 => PROT_NONE,
        0x02 => PROT_READ,
        0x04 | 0x08 => PROT_READ | PROT_WRITE,
        0x10 | 0x20 => PROT_READ | PROT_EXEC,
        0x40 | 0x80 => PROT_READ | PROT_WRITE | PROT_EXEC,
        _ => return None,
    })
}

fn page_range(address: usize, size: usize) -> Option<(usize, usize)> {
    let start = address & !(PAGE_SIZE - 1);
    let end = address
        .checked_add(size)?
        .checked_next_multiple_of(PAGE_SIZE)?;
    Some((start, end - start))
}

extern "win64" fn virtual_alloc(address: usize, size: usize, kind: u32, protect: u32) -> usize {
    let (Some(prot), Some((start, length))) = (protection(protect), page_range(address, size))
    else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if size == 0 || kind & !(MEM_COMMIT | MEM_RESERVE) != 0 || kind == 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    if kind & MEM_RESERVE == 0 && address != 0 {
        // SAFETY: committing pages of a reservation the program made only changes what it may
        // do with them.
        return match unsafe { linux::protect(start, length, prot) } {
            Ok(_) => start,
            Err(errno) => fail(error_for(errno), 0),
        };
    }
    let prot = if kind & MEM_COMMIT != 0 {
        prot
    } else {
        PROT_NONE
    };
    let mapped = match linux::map_anonymous(start, length, prot, address != 0) {
        Ok(mapped) => mapped,
        Err(_) => return fail(error::INVALID_ADDRESS, 0),
    };
    let recorded = RESERVATIONS.with(|table| {
        let free = table.iter_mut().find(|entry| entry.1 == 0)?;
        *free = (mapped, length);
        Some(())
    });
    if recorded.is_none() {
        // SAFETY: the mapping was just made and handed to no one.
        let _ = unsafe { linux::unmap(mapped, length) };
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    mapped
}

extern "win64" fn virtual_free(address: usize, size: usize, kind: u32) -> Bool {
    match kind {
        MEM_RELEASE if size == 0 => {
            let length = RESERVATIONS.with(|table| {
                let entry = table
                    .iter_mut()
                    .find(|entry| entry.0 == address && entry.1 != 0)?;
                Some(core::mem::take(entry).1)
            });
            let Some(length) = length else {
                return fail(error::INVALID_ADDRESS, FALSE);
            };
            // SAFETY: the reservation is the program's, which gives it back whole.
            let _ = unsafe { linux::unmap(address, length) };
            TRUE
        }
        MEM_DECOMMIT => {
            let Some((start, length)) = page_range(address, size.max(1)) else {
                return fail(error::INVALID_PARAMETER, FALSE);
            };
            // SAFETY: the program decommits its own pages: their contents go.
            let discarded = unsafe { linux::discard(start, length) }
                // SAFETY: as above; the pages become inaccessible until committed again.
                .and_then(|_| unsafe { linux::protect(start, length, PROT_NONE) });
            match discarded {
                Ok(_) => TRUE,
                Err(errno) => fail(error_for(errno), FALSE),
            }
        }
        _ => fail(error::INVALID_PARAMETER, FALSE),
    }
}

unsafe extern "win64" fn virtual_protect(
    address: usize,
    size: usize,
    protect: u32,
    old: *mut u32,
) -> Bool {
    let (Some(prot), Some((start, length))) = (protection(protect), page_range(address, size))
    else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    if old.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the program changes its own pages' protection, as `VirtualProtect` does.
    match unsafe { linux::protect(start, length, prot) } {
        Ok(_) => {
            // SAFETY: the caller passes a writable `DWORD` for the old protection.
            unsafe { old.write(PAGE_READWRITE) };
            TRUE
        }
        Err(errno) => fail(error_for(errno), FALSE),
    }
}

unsafe extern "win64" fn memcpy(destination: *mut u8, source: *const u8, length: usize) -> *mut u8 {
    // SAFETY: the caller passes `length` readable and writable bytes, as `memcpy`'s contract
    // says; copying as `memmove` also covers callers that overlap them.
    unsafe { ptr::copy(source, destination, length) };
    destination
}

unsafe extern "win64" fn memset(destination: *mut u8, value: i32, length: usize) -> *mut u8 {
    // SAFETY: the caller passes `length` writable bytes.
    unsafe { destination.write_bytes(value as u8, length) };
    destination
}

unsafe extern "win64" fn memcmp(a: *const u8, b: *const u8, length: usize) -> i32 {
    // SAFETY: the caller passes `length` readable bytes at each.
    let a = unsafe { core::slice::from_raw_parts(a, length) };
    // SAFETY: as above.
    let b = unsafe { core::slice::from_raw_parts(b, length) };
    a.iter()
        .zip(b)
        .find(|(x, y)| x != y)
        .map_or(0, |(x, y)| i32::from(*x) - i32::from(*y))
}

unsafe extern "win64" fn strlen(text: *const u8) -> usize {
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe { core::ffi::CStr::from_ptr(text.cast()) }.count_bytes()
}

unsafe extern "C" {
    fn eggos_windows_chkstk();
}

core::arch::global_asm!(
    ".globl eggos_windows_chkstk",
    "eggos_windows_chkstk:",
    "push rcx",
    "push rax",
    "lea rcx, [rsp + 24]",
    "sub rcx, rax",
    "jae 2f",
    "xor ecx, ecx",
    "2:",
    "mov rax, rsp",
    "and rax, -4096",
    "3:",
    "cmp rax, rcx",
    "jbe 4f",
    "sub rax, 4096",
    "test byte ptr [rax], 0",
    "jmp 3b",
    "4:",
    "pop rax",
    "pop rcx",
    "ret",
);

pub(super) const EXPORTS: &[Export] = exports![
    b"VirtualAlloc" => virtual_alloc,
    b"VirtualFree" => virtual_free,
    b"VirtualProtect" => virtual_protect,
];

pub(super) const NTDLL_EXPORTS: &[Export] = exports![
    b"memcpy" => memcpy,
    b"memmove" => memcpy,
    b"memset" => memset,
    b"memcmp" => memcmp,
    b"strlen" => strlen,
    b"__chkstk" => eggos_windows_chkstk,
];
