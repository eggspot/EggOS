//! Virtual memory (`VirtualAlloc`, `VirtualFree`, `VirtualProtect`) on Linux's `mmap`,
//! `mprotect`, `madvise` and `munmap`, `VirtualQuery` from `/proc/self/maps` (D189),
//! `FlushInstructionCache` for code a program writes, and
//! the memory functions and stack probe (`__chkstk`)
//! that ntdll exports and compilers call.

use super::heap as process_heap;
use super::{Bool, FALSE, SpinLock, TRUE, error, error_for, fail};
use crate::library::Export;
use crate::linux::{self, PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE};
use crate::memory_regions::{self, Region};
use core::ptr;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_SIZE: usize = 4096;
const PAGE_READWRITE: u32 = 0x04;
const MAX_RESERVATIONS: usize = 256;
const MAPS_PATH: &[u8] = b"/proc/self/maps\0";
const FIRST_MAPS_BYTES: usize = 64 * 1024;
const LARGEST_MAPS_BYTES: usize = 4 * 1024 * 1024;
const MEMORY_BASIC_INFORMATION_BYTES: usize = 48;
const END_OF_USER_SPACE: usize = 0x7fff_ffff_0000;

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

pub(super) extern "efiapi" fn virtual_alloc(
    address: usize,
    size: usize,
    kind: u32,
    protect: u32,
) -> usize {
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

pub(super) extern "efiapi" fn virtual_free(address: usize, size: usize, kind: u32) -> Bool {
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

unsafe extern "efiapi" fn virtual_protect(
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

fn region_of(address: usize) -> Result<Region, u32> {
    let mut capacity = FIRST_MAPS_BYTES;
    loop {
        let block = process_heap::allocate(capacity);
        if block.is_null() {
            return Err(error::NOT_ENOUGH_MEMORY);
        }
        // SAFETY: a fresh, zeroed block of `capacity` bytes, used only through this slice.
        let text = unsafe { core::slice::from_raw_parts_mut(block, capacity) };
        let outcome = match linux::read_file(MAPS_PATH, text) {
            Ok(length) if length == capacity && capacity < LARGEST_MAPS_BYTES => None,
            Ok(length) => Some(Ok(memory_regions::region_at(
                &text[..length],
                address,
                END_OF_USER_SPACE,
            ))),
            Err(errno) => Some(Err(error_for(errno))),
        };
        // SAFETY: the block this call allocated, which nothing holds any more.
        unsafe { process_heap::free(block) };
        match outcome {
            Some(outcome) => return outcome,
            None => capacity *= 2,
        }
    }
}

unsafe extern "efiapi" fn virtual_query(address: usize, out: *mut u8, length: usize) -> usize {
    if out.is_null() || length < MEMORY_BASIC_INFORMATION_BYTES {
        return fail(error::BAD_LENGTH, 0);
    }
    if address >= END_OF_USER_SPACE {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let region = match region_of(address) {
        Ok(region) => region,
        Err(code) => return fail(code, 0),
    };
    let mut information = [0u8; MEMORY_BASIC_INFORMATION_BYTES];
    information[0..8].copy_from_slice(&(region.base as u64).to_le_bytes());
    information[8..16].copy_from_slice(&(region.allocation_base as u64).to_le_bytes());
    information[16..20].copy_from_slice(&region.allocation_protect.to_le_bytes());
    information[24..32].copy_from_slice(&(region.size as u64).to_le_bytes());
    information[32..36].copy_from_slice(&region.state.to_le_bytes());
    information[36..40].copy_from_slice(&region.protect.to_le_bytes());
    information[40..44].copy_from_slice(&region.kind.to_le_bytes());
    // SAFETY: `VirtualQuery`'s contract: `length` writable bytes, at least one
    // `MEMORY_BASIC_INFORMATION`.
    unsafe { ptr::copy_nonoverlapping(information.as_ptr(), out, information.len()) };
    MEMORY_BASIC_INFORMATION_BYTES
}

unsafe extern "efiapi" fn memcpy(
    destination: *mut u8,
    source: *const u8,
    length: usize,
) -> *mut u8 {
    // SAFETY: the caller passes `length` readable and writable bytes, as `memcpy`'s contract
    // says; copying as `memmove` also covers callers that overlap them.
    unsafe { ptr::copy(source, destination, length) };
    destination
}

unsafe extern "efiapi" fn memset(destination: *mut u8, value: i32, length: usize) -> *mut u8 {
    // SAFETY: the caller passes `length` writable bytes.
    unsafe { destination.write_bytes(value as u8, length) };
    destination
}

unsafe extern "efiapi" fn memcmp(a: *const u8, b: *const u8, length: usize) -> i32 {
    // SAFETY: the caller passes `length` readable bytes at each.
    let a = unsafe { core::slice::from_raw_parts(a, length) };
    // SAFETY: as above.
    let b = unsafe { core::slice::from_raw_parts(b, length) };
    a.iter()
        .zip(b)
        .find(|(x, y)| x != y)
        .map_or(0, |(x, y)| i32::from(*x) - i32::from(*y))
}

unsafe extern "efiapi" fn strlen(text: *const u8) -> usize {
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe { core::ffi::CStr::from_ptr(text.cast()) }.count_bytes()
}

unsafe extern "C" {
    fn eggos_windows_chkstk();
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".globl eggos_windows_chkstk",
    "eggos_windows_chkstk:",
    "mov x16, sp",
    "subs x17, x16, x15, lsl #4",
    "csel x17, xzr, x17, lo",
    "and x16, x16, #-4096",
    "2:",
    "cmp x16, x17",
    "b.ls 3f",
    "sub x16, x16, #4096",
    "ldrb wzr, [x16]",
    "b 2b",
    "3:",
    "ret",
);

extern "efiapi" fn flush_instruction_cache(_process: usize, _address: usize, _size: usize) -> Bool {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: barriers touch no memory; they order the program's earlier writes of code
        // before the instructions that follow, which is all a fresh mapping needs.
        unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"VirtualAlloc" => virtual_alloc,
    b"VirtualFree" => virtual_free,
    b"VirtualProtect" => virtual_protect,
    b"VirtualQuery" => virtual_query,
    b"FlushInstructionCache" => flush_instruction_cache,
];

pub(super) const NTDLL_EXPORTS: &[Export] = exports![
    b"memcpy" => memcpy,
    b"memmove" => memcpy,
    b"memset" => memset,
    b"memcmp" => memcmp,
    b"strlen" => strlen,
    b"__chkstk" => eggos_windows_chkstk [direct],
];
