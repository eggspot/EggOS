//! The thread environment block (TEB) and process environment block (PEB), at the offsets
//! Microsoft's public headers give (`NT_TIB` in `winnt.h`, `TEB` and `PEB` in `winternl.h`) and
//! its debugger documentation names: an x64 program finds its TEB at `gs:[0x30]` and its PEB
//! at `gs:[0x60]`, an ARM64 program its TEB in `x18` (the layer keeps it in `tpidr_el0`, D124);
//! the layout is the same on both. The last error and the TLS slots live in the TEB, so each thread has its own;
//! past the documented TEB (0x1838 bytes) the layer keeps each thread's fiber-local storage
//! slots and its thread object. Every thread gets a TEB and a copy of the image's static TLS.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::Export;
use crate::linux::{self, PROT_READ, PROT_WRITE};
use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
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
    pub const DOCUMENTED_END: usize = 0x1838;
    pub const FLS_SLOTS: usize = 0x1c00;
    pub const FLS_SLOT_COUNT: usize = 64;
    pub const THREAD_OBJECT: usize = 0x1e00;
}

pub mod peb {
    pub const IMAGE_BASE_ADDRESS: usize = 0x10;
    pub const PROCESS_HEAP: usize = 0x30;
}

const _: () = assert!(teb::TLS_SLOTS + teb::TLS_SLOT_COUNT * 8 <= teb::DOCUMENTED_END);
const _: () = assert!(teb::DOCUMENTED_END <= teb::FLS_SLOTS);
const _: () = assert!(teb::FLS_SLOTS + teb::FLS_SLOT_COUNT * 8 <= teb::THREAD_OBJECT);
const _: () = assert!(teb::THREAD_OBJECT + 8 <= TEB_SIZE);
const TLS_OUT_OF_INDEXES: u32 = u32::MAX;
const FLS_OUT_OF_INDEXES: u32 = u32::MAX;
const DLL_THREAD_ATTACH: u32 = 2;
const DLL_THREAD_DETACH: u32 = 3;

static TLS_SLOTS_IN_USE: AtomicU64 = AtomicU64::new(0);
static FLS_SLOTS_IN_USE: AtomicU64 = AtomicU64::new(0);
static FLS_CALLBACKS: [AtomicUsize; teb::FLS_SLOT_COUNT] =
    [const { AtomicUsize::new(0) }; teb::FLS_SLOT_COUNT];
static PEB: AtomicUsize = AtomicUsize::new(0);
static STATIC_TLS: AtomicPtr<StaticTls> = AtomicPtr::new(core::ptr::null_mut());

/// The image's static TLS, which every thread gets a copy of: its template, the zeros after
/// it, and its callbacks (`PIMAGE_TLS_CALLBACK`s) with the image base they get.
#[derive(Debug, Clone, Copy)]
pub struct StaticTls {
    pub template: &'static [u8],
    pub zero_fill: usize,
    pub callbacks: Option<(usize, usize, usize)>,
}

/// Records the image's static TLS for the threads to come; `None` without memory.
pub fn set_static_tls(tls: StaticTls) -> Option<()> {
    let state = super::heap_allocate(core::mem::size_of::<StaticTls>()).cast::<StaticTls>();
    if state.is_null() {
        return None;
    }
    // SAFETY: `state` is a fresh, 16-byte aligned heap block for one `StaticTls`.
    unsafe { state.write(tls) };
    STATIC_TLS.store(state, Ordering::Release);
    Some(())
}

fn static_tls() -> Option<&'static StaticTls> {
    // SAFETY: the loader publishes the record before the program runs and never changes or
    // frees it.
    unsafe { STATIC_TLS.load(Ordering::Acquire).as_ref() }
}

/// A new copy of the image's static TLS for a thread: the address of its one-slot array
/// (what `ThreadLocalStoragePointer` holds), 0 if the image has none, `None` without memory.
pub fn new_thread_local_storage() -> Option<usize> {
    let Some(tls) = static_tls() else {
        return Some(0);
    };
    let size = tls.template.len().checked_add(tls.zero_fill)?;
    let block = super::heap_allocate(size.max(1));
    let slots = super::heap_allocate(8).cast::<u64>();
    if block.is_null() || slots.is_null() {
        // SAFETY: each is null or a fresh block nothing else has seen.
        unsafe { super::heap::free(block) };
        // SAFETY: as above.
        unsafe { super::heap::free(slots.cast()) };
        return None;
    }
    // SAFETY: `block` is a fresh heap block of `size` bytes, the template's length first.
    unsafe { block.copy_from_nonoverlapping(tls.template.as_ptr(), tls.template.len()) };
    // SAFETY: `slots` is a fresh heap block of one pointer.
    unsafe { slots.write(block as u64) };
    Some(slots as usize)
}

/// Gives back a thread's copy of the static TLS that [`new_thread_local_storage`] made.
///
/// # Safety
/// `slots` must be 0 or what `new_thread_local_storage` returned, for a thread that no longer
/// runs.
pub unsafe fn free_thread_local_storage(slots: usize) {
    if slots == 0 {
        return;
    }
    // SAFETY: `slots` is the one-slot array `new_thread_local_storage` returned, which holds
    // its block's address; the thread that used them has ended.
    let block = unsafe { (slots as *const u64).read() } as *mut u8;
    // SAFETY: as above: both are this heap's blocks, which nothing uses any more.
    unsafe { super::heap::free(block) };
    // SAFETY: as above.
    unsafe { super::heap::free(slots as *mut u8) };
}

/// Calls the image's TLS callbacks with `reason` (`DLL_PROCESS_ATTACH` and the others).
pub fn run_tls_callbacks(reason: u32) {
    let Some((base, mut at, end)) = static_tls().and_then(|tls| tls.callbacks) else {
        return;
    };
    while at + 8 <= end {
        // SAFETY: the TLS directory's callback array, checked to start inside the image, is a
        // null-terminated array of function addresses the image's relocations fixed.
        let callback = unsafe { (at as *const u64).read_unaligned() };
        if callback == 0 {
            return;
        }
        // SAFETY: as above; Windows calls each callback as `PIMAGE_TLS_CALLBACK`.
        unsafe { super::call_program(callback as usize, [base, reason as usize, 0, 0]) };
        at += 8;
    }
}

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
    PEB.store(peb, Ordering::Release);
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

/// Maps and fills a new thread's TEB, which shares the PEB, for the stack between
/// `stack_limit` and `stack_base`, the static TLS copy `slots` and the thread's object; the
/// thread fills in its own id when it starts.
pub fn create_for_thread(
    stack_base: usize,
    stack_limit: usize,
    slots: usize,
    object: usize,
) -> Result<usize, Errno> {
    let teb = linux::map_anonymous(0, TEB_SIZE, PROT_READ | PROT_WRITE, false)?;
    let put = |offset: usize, value: u64| {
        // SAFETY: every offset written is inside the new mapping, which nothing else uses yet,
        // and 8-byte aligned.
        unsafe { ((teb + offset) as *mut u64).write(value) }
    };
    put(teb::STACK_BASE, stack_base as u64);
    put(teb::STACK_LIMIT, stack_limit as u64);
    put(teb::SELF, teb as u64);
    put(teb::PROCESS_ID, u64::from(linux::process_id()));
    put(teb::THREAD_LOCAL_STORAGE_POINTER, slots as u64);
    put(
        teb::PROCESS_ENVIRONMENT_BLOCK,
        PEB.load(Ordering::Acquire) as u64,
    );
    put(teb::THREAD_OBJECT, object as u64);
    Ok(teb)
}

/// Starts the calling thread on its TEB: points the thread's TEB register at it, records the thread's id in it and
/// runs the TLS callbacks with `DLL_THREAD_ATTACH`.
///
/// # Safety
/// `teb` must be the TEB [`create_for_thread`] made for the calling thread.
pub unsafe fn enter(teb: usize) {
    // SAFETY: the caller's duty: the TEB register gets this thread's own TEB.
    let _ = unsafe { linux::set_thread_environment_block(teb) };
    // SAFETY: the TEB is mapped and writable, the field 8-byte aligned, and only this thread
    // writes it.
    unsafe { ((teb + teb::THREAD_ID) as *mut u64).write(u64::from(linux::thread_id())) };
    run_tls_callbacks(DLL_THREAD_ATTACH);
}

/// Ends the calling thread's Win32 life: its fiber-local storage callbacks run for its
/// non-null values, then the TLS callbacks with `DLL_THREAD_DETACH`.
pub fn leave() {
    for (index, callback) in FLS_CALLBACKS.iter().enumerate() {
        let callback = callback.load(Ordering::Acquire);
        let Some(slot) = fls_slot(index as u32) else {
            continue;
        };
        // SAFETY: `slot` is inside the current thread's TEB.
        let value = unsafe { slot.read() };
        if callback != 0 && value != 0 && FLS_SLOTS_IN_USE.load(Ordering::Acquire) & 1 << index != 0
        {
            // SAFETY: as above.
            unsafe { slot.write(0) };
            // SAFETY: `FlsAlloc` was given this callback, a `PFLS_CALLBACK_FUNCTION`.
            unsafe { super::call_program(callback, [value as usize, 0, 0, 0]) };
        }
    }
    run_tls_callbacks(DLL_THREAD_DETACH);
}

/// The calling thread's object (0 for the first thread, which has none).
pub fn thread_object() -> usize {
    // SAFETY: the field is inside the current thread's TEB, 8-byte aligned.
    unsafe { ((current() + teb::THREAD_OBJECT) as *const usize).read() }
}

/// The calling thread's id, from its TEB.
pub fn thread_id() -> u32 {
    // SAFETY: the field is inside the current thread's TEB, 8-byte aligned; the thread wrote
    // it when it started.
    unsafe { ((current() + teb::THREAD_ID) as *const u64).read() as u32 }
}

/// Points the TEB's `ThreadLocalStoragePointer` at `slots`, the array of the image's TLS blocks.
///
/// # Safety
/// `teb` must be a TEB [`create`] returned.
pub unsafe fn set_thread_local_storage(teb: usize, slots: usize) {
    // SAFETY: the caller's TEB is mapped and writable; the field is 8-byte aligned.
    unsafe { ((teb + teb::THREAD_LOCAL_STORAGE_POINTER) as *mut u64).write(slots as u64) }
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "aarch64")]
fn current() -> usize {
    let teb: usize;
    // SAFETY: the loader and each thread's start set `tpidr_el0` to the thread's TEB before the
    // program runs; reading it touches no memory.
    unsafe {
        core::arch::asm!(
            "mrs {teb}, tpidr_el0",
            teb = out(reg) teb,
            options(nostack, nomem, preserves_flags),
        )
    };
    teb
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "aarch64")]
pub fn set_last_error(code: u32) {
    // SAFETY: the last error is the current thread's TEB field, 4-byte aligned, which only this
    // thread writes.
    unsafe { ((current() + teb::LAST_ERROR) as *mut u32).write(code) };
}

#[cfg(target_arch = "x86_64")]
extern "efiapi" fn get_last_error() -> u32 {
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

#[cfg(target_arch = "aarch64")]
extern "efiapi" fn get_last_error() -> u32 {
    // SAFETY: as in `set_last_error`; reading the current thread's TEB field.
    unsafe { ((current() + teb::LAST_ERROR) as *const u32).read() }
}

extern "efiapi" fn set_last_error_export(code: u32) {
    set_last_error(code);
}

fn slot(index: u32) -> Option<*mut u64> {
    let index = index as usize;
    (index < teb::TLS_SLOT_COUNT).then(|| (current() + teb::TLS_SLOTS + index * 8) as *mut u64)
}

extern "efiapi" fn tls_alloc() -> u32 {
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

extern "efiapi" fn tls_free(index: u32) -> Bool {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    TLS_SLOTS_IN_USE.fetch_and(!(1 << index), Ordering::Relaxed);
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.write(0) };
    TRUE
}

extern "efiapi" fn tls_get_value(index: u32) -> u64 {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    set_last_error(error::SUCCESS);
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.read() }
}

extern "efiapi" fn tls_set_value(index: u32, value: u64) -> Bool {
    let Some(slot) = slot(index) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.write(value) };
    TRUE
}

fn fls_slot(index: u32) -> Option<*mut u64> {
    let index = index as usize;
    (index < teb::FLS_SLOT_COUNT).then(|| (current() + teb::FLS_SLOTS + index * 8) as *mut u64)
}

extern "efiapi" fn fls_alloc(callback: usize) -> u32 {
    let mut used = FLS_SLOTS_IN_USE.load(Ordering::Relaxed);
    loop {
        let free = (!used).trailing_zeros();
        if free >= teb::FLS_SLOT_COUNT as u32 {
            return fail(error::NOT_ENOUGH_MEMORY, FLS_OUT_OF_INDEXES);
        }
        match FLS_SLOTS_IN_USE.compare_exchange_weak(
            used,
            used | 1 << free,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                FLS_CALLBACKS[free as usize].store(callback, Ordering::Release);
                return free;
            }
            Err(now) => used = now,
        }
    }
}

extern "efiapi" fn fls_free(index: u32) -> Bool {
    let Some(slot) = fls_slot(index) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    if FLS_SLOTS_IN_USE.load(Ordering::Acquire) & 1 << index == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let callback = FLS_CALLBACKS[index as usize].swap(0, Ordering::AcqRel);
    if FLS_SLOTS_IN_USE.fetch_and(!(1 << index), Ordering::AcqRel) & 1 << index == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `slot` is inside the current thread's TEB.
    let value = unsafe { slot.read() };
    // SAFETY: as above.
    unsafe { slot.write(0) };
    if callback != 0 && value != 0 {
        // SAFETY: `FlsAlloc` was given this callback, a `PFLS_CALLBACK_FUNCTION`.
        unsafe { super::call_program(callback, [value as usize, 0, 0, 0]) };
    }
    TRUE
}

extern "efiapi" fn fls_get_value(index: u32) -> u64 {
    let Some(slot) = fls_slot(index) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    set_last_error(error::SUCCESS);
    // SAFETY: `slot` is inside the current thread's TEB.
    unsafe { slot.read() }
}

extern "efiapi" fn fls_set_value(index: u32, value: u64) -> Bool {
    let Some(slot) = fls_slot(index) else {
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
    b"FlsAlloc" => fls_alloc,
    b"FlsFree" => fls_free,
    b"FlsGetValue" => fls_get_value,
    b"FlsSetValue" => fls_set_value,
];
