//! The thread environment block (TEB) and process environment block (PEB), at the offsets
//! Microsoft's public headers give (`NT_TIB` in `winnt.h`, `TEB` and `PEB` in `winternl.h`) and
//! its debugger documentation names: a program finds its TEB at `gs:[0x30]` and its PEB at
//! `gs:[0x60]`. The last error and the TLS slots live in the TEB, so each thread has its own.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::linux::{self, PROT_READ, PROT_WRITE};
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_runtime::errno::Errno;

pub const TEB_SIZE: usize = 0x2000;
pub const PEB_SIZE: usize = 0x1000;

pub mod teb {
    pub const STACK_BASE: usize = 0x08;
    pub const STACK_LIMIT: usize = 0x10;
    pub const SELF: usize = 0x30;
    pub const PROCESS_ID: usize = 0x40;
    pub const THREAD_ID: usize = 0x48;
    pub const THREAD_LOCAL_STORAGE_POINTER: usize = 0x58;
    pub const PROCESS_ENVIRONMENT_BLOCK: usize = 0x60;
    pub const LAST_ERROR: usize = 0x68;
    pub const TLS_SLOTS: usize = 0x1480;
    pub const TLS_SLOT_COUNT: usize = 64;
}

pub mod peb {
    pub const IMAGE_BASE_ADDRESS: usize = 0x10;
    pub const PROCESS_HEAP: usize = 0x30;
}

const _: () = assert!(teb::TLS_SLOTS + teb::TLS_SLOT_COUNT * 8 <= TEB_SIZE);
const TLS_OUT_OF_INDEXES: u32 = u32::MAX;

static TLS_SLOTS_IN_USE: AtomicU64 = AtomicU64::new(0);

/// What the first thread's blocks hold.
#[derive(Debug, Clone, Copy)]
pub struct Blocks {
    pub image_base: u64,
    pub heap: usize,
    pub stack_base: usize,
    pub stack_limit: usize,
}

/// Maps and fills the first thread's TEB and the PEB; returns the TEB's address.
pub fn create(blocks: Blocks) -> Result<usize, Errno> {
    let teb = linux::map_anonymous(0, TEB_SIZE + PEB_SIZE, PROT_READ | PROT_WRITE, false)?;
    let peb = teb + TEB_SIZE;
    let put = |address: usize, value: u64| {
        // SAFETY: every address written is inside the new mapping, which nothing else uses yet,
        // at an 8-byte aligned offset.
        unsafe { (address as *mut u64).write(value) }
    };
    put(teb + teb::STACK_BASE, blocks.stack_base as u64);
    put(teb + teb::STACK_LIMIT, blocks.stack_limit as u64);
    put(teb + teb::SELF, teb as u64);
    put(teb + teb::PROCESS_ID, u64::from(linux::process_id()));
    put(teb + teb::THREAD_ID, u64::from(linux::thread_id()));
    put(teb + teb::PROCESS_ENVIRONMENT_BLOCK, peb as u64);
    put(peb + peb::IMAGE_BASE_ADDRESS, blocks.image_base);
    put(peb + peb::PROCESS_HEAP, blocks.heap as u64);
    Ok(teb)
}

/// Points the TEB's `ThreadLocalStoragePointer` at `slots`, the array of the image's TLS blocks.
///
/// # Safety
/// `teb` must be a TEB [`create`] returned.
pub unsafe fn set_thread_local_storage(teb: usize, slots: usize) {
    // SAFETY: the caller's TEB is mapped and writable; the field is 8-byte aligned.
    unsafe { ((teb + teb::THREAD_LOCAL_STORAGE_POINTER) as *mut u64).write(slots as u64) }
}

fn current() -> usize {
    let teb: usize;
    // SAFETY: the loader sets GS to the TEB before the program runs, and the TEB's `Self` field
    // holds its own address.
    unsafe {
        core::arch::asm!(
            "mov {teb}, gs:[{offset}]",
            teb = out(reg) teb,
            offset = const teb::SELF,
            options(nostack, readonly, preserves_flags),
        )
    };
    teb
}

pub fn set_last_error(code: u32) {
    // SAFETY: the last error is the current thread's TEB field, which only this thread writes.
    unsafe {
        core::arch::asm!(
            "mov dword ptr gs:[{offset}], {code:e}",
            code = in(reg) code,
            offset = const teb::LAST_ERROR,
            options(nostack, preserves_flags),
        )
    };
}

extern "win64" fn get_last_error() -> u32 {
    let code: u32;
    // SAFETY: as in `set_last_error`; reading the current thread's TEB field.
    unsafe {
        core::arch::asm!(
            "mov {code:e}, dword ptr gs:[{offset}]",
            code = out(reg) code,
            offset = const teb::LAST_ERROR,
            options(nostack, readonly, preserves_flags),
        )
    };
    code
}

extern "win64" fn set_last_error_export(code: u32) {
    set_last_error(code);
}

fn slot(index: u32) -> Option<*mut u64> {
    let index = index as usize;
    (index < teb::TLS_SLOT_COUNT).then(|| (current() + teb::TLS_SLOTS + index * 8) as *mut u64)
}

extern "win64" fn tls_alloc() -> u32 {
    let mut used = TLS_SLOTS_IN_USE.load(Ordering::Relaxed);
    loop {
        let free = (!used).trailing_zeros();
        if free >= teb::TLS_SLOT_COUNT as u32 {
            return fail(error::NOT_ENOUGH_MEMORY, TLS_OUT_OF_INDEXES);
        }
        match TLS_SLOTS_IN_USE.compare_exchange_weak(
            used,
            used | 1 << free,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return free,
            Err(now) => used = now,
        }
    }
}

extern "win64" fn tls_free(index: u32) -> Bool {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    TLS_SLOTS_IN_USE.fetch_and(!(1 << index), Ordering::Relaxed);
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.write(0) };
    TRUE
}

extern "win64" fn tls_get_value(index: u32) -> u64 {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    set_last_error(error::SUCCESS);
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.read() }
}

extern "win64" fn tls_set_value(index: u32, value: u64) -> Bool {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.write(value) };
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetLastError" => get_last_error,
    b"SetLastError" => set_last_error_export,
    b"TlsAlloc" => tls_alloc,
    b"TlsFree" => tls_free,
    b"TlsGetValue" => tls_get_value,
    b"TlsSetValue" => tls_set_value,
];
