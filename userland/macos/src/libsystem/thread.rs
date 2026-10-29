//! Threads and thread-local storage. Each thread has a control block (`Thread`) that its
//! thread pointer (`tpidr_el0` on aarch64, the FS base on x86_64, neither of which Darwin
//! programs use) points at: its `errno`, its thread-local variables, its `pthread_key_t`
//! values and its `_tlv_atexit` destructors. `pthread_create` maps a stack with a guard page
//! below it and the control block above it and starts a Linux thread of the process there.
//! Thread-local variables follow Mach-O's descriptors: every descriptor's thunk is bound to
//! `_tlv_bootstrap`, which the layer implements as the accessor itself, returning the calling
//! thread's copy of the `__thread_data` and `__thread_bss` template plus the descriptor's
//! offset.

use super::heap;
use crate::darwin::EINVAL;
use crate::linux::{self, PROT_READ, PROT_WRITE};
use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// How many `pthread_key_t` keys a program may create, Darwin's `PTHREAD_KEYS_MAX`.
pub const KEYS: usize = 512;
const DEFAULT_STACK: usize = 512 << 10;
const MINIMUM_STACK: usize = 16 << 10;
const MAXIMUM_STACK: usize = 1 << 30;
const MAIN_STACK: usize = 8 << 20;
const GUARD: usize = linux::PAGE_SIZE;
const JOINABLE: u32 = 0;
const DETACHED: u32 = 1;
const FINISHED: u32 = 2;
const CREATE_JOINABLE: c_int = 1;
const CREATE_DETACHED: c_int = 2;
const ATTRIBUTES_SIGNATURE: u64 = 0x5448_4441;
const EAGAIN: c_int = 35;
const ESRCH: c_int = 3;
const EDEADLK: c_int = 11;
const KEY_DESTRUCTOR_ROUNDS: usize = 4;
const BURIED_THREADS: usize = 64;

/// A thread's control block. The accessor `_tlv_bootstrap` reads `thread_locals` at offset 0,
/// and x86_64 finds the block through `this` at offset 8.
#[repr(C)]
#[derive(Debug)]
pub struct Thread {
    thread_locals: usize,
    this: usize,
    errno: c_int,
    id: AtomicU32,
    id_word: AtomicU32,
    state: AtomicU32,
    mapping: usize,
    mapping_length: usize,
    stack_top: usize,
    stack_size: usize,
    start: usize,
    argument: usize,
    result: usize,
    exit_handlers: usize,
    exit_handler_count: usize,
    exit_handler_capacity: usize,
    pub alternate_stack: [usize; 3],
    pub autorelease_pools: usize,
    values: [usize; KEYS],
}

impl Thread {
    #[allow(clippy::declare_interior_mutable_const)]
    const EMPTY: Self = Self {
        thread_locals: 0,
        this: 0,
        errno: 0,
        id: AtomicU32::new(0),
        id_word: AtomicU32::new(0),
        state: AtomicU32::new(JOINABLE),
        mapping: 0,
        mapping_length: 0,
        stack_top: 0,
        stack_size: 0,
        start: 0,
        argument: 0,
        result: 0,
        exit_handlers: 0,
        exit_handler_count: 0,
        exit_handler_capacity: 0,
        alternate_stack: [0; 3],
        autorelease_pools: 0,
        values: [0; KEYS],
    };
}

static mut MAIN: Thread = Thread::EMPTY;
static mut TEMPLATE: [usize; 3] = [0; 3];
static KEY_IN_USE: [AtomicU32; KEYS] = [const { AtomicU32::new(0) }; KEYS];
static KEY_DESTRUCTORS: [AtomicUsize; KEYS] = [const { AtomicUsize::new(0) }; KEYS];
static BURIED: super::Exclusive<[usize; BURIED_THREADS]> =
    super::Exclusive::new([0; BURIED_THREADS]);

/// The calling thread's control block.
#[inline(always)]
pub fn current() -> *mut Thread {
    let address: usize;
    #[cfg(target_arch = "aarch64")]
    // SAFETY: reading `tpidr_el0` touches no memory.
    unsafe {
        core::arch::asm!(
            "mrs {}, tpidr_el0",
            out(reg) address,
            options(nomem, nostack, preserves_flags)
        );
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: every thread's FS base points at its control block, whose `this` is at 8.
    unsafe {
        core::arch::asm!(
            "mov {}, fs:[8]",
            out(reg) address,
            options(readonly, nostack, preserves_flags)
        );
    }
    address as *mut Thread
}

/// The calling thread's Linux thread id.
pub fn current_id() -> u32 {
    // SAFETY: the control block lives as long as its thread.
    unsafe { &(*current()).id }.load(Ordering::Relaxed)
}

/// Where the calling thread's `errno` is.
pub fn errno_location() -> *mut c_int {
    // SAFETY: as above; only `errno`'s address is taken.
    unsafe { &raw mut (*current()).errno }
}

/// Makes the layer's own thread the program's main thread: its control block, its thread
/// pointer, and its stack (`stack_pointer` is near the top).
///
/// # Safety
/// Called once, first, on the layer's only thread.
pub unsafe fn set_up_main_thread(stack_pointer: usize) {
    let main = &raw mut MAIN;
    // SAFETY: the caller's promise: nothing else uses the block yet.
    let block = unsafe { &mut *main };
    block.this = main as usize;
    block.id.store(linux::thread_id(), Ordering::Relaxed);
    block.stack_top = stack_pointer.next_multiple_of(linux::PAGE_SIZE);
    block.stack_size = MAIN_STACK;
    // SAFETY: the layer never uses the thread pointer otherwise.
    unsafe { linux::set_thread_pointer(main as usize) };
}

/// Records the program's thread-local template (`length` bytes at `address`, aligned to
/// `alignment`) and gives the main thread its copy; `false` if memory ran out.
///
/// # Safety
/// Called once, before the program runs; the template must stay mapped and unchanged.
pub unsafe fn set_thread_local_template(address: usize, length: usize, alignment: usize) -> bool {
    // SAFETY: the caller's promise.
    unsafe { TEMPLATE = [address, length, alignment] };
    let Some(block) = new_thread_locals() else {
        return false;
    };
    // SAFETY: as above.
    unsafe { (*current()).thread_locals = block };
    true
}

fn new_thread_locals() -> Option<usize> {
    // SAFETY: written once before the program runs.
    let [address, length, alignment] = unsafe { TEMPLATE };
    if address == 0 {
        return Some(0);
    }
    let block = heap::allocate_aligned(alignment.max(16), length.max(16));
    if block.is_null() {
        return None;
    }
    // SAFETY: the template is `length` readable bytes and the block `length` writable ones.
    unsafe { core::ptr::copy_nonoverlapping(address as *const u8, block, length) };
    Some(block as usize)
}

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_macos_tlv_get_address",
    ".hidden eggos_macos_tlv_get_address",
    "eggos_macos_tlv_get_address:",
    "mrs x16, tpidr_el0",
    "ldr x16, [x16]",
    "ldr x0, [x0, #16]",
    "add x0, x16, x0",
    "ret",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_macos_tlv_get_address",
    ".hidden eggos_macos_tlv_get_address",
    "eggos_macos_tlv_get_address:",
    "mov rax, [rdi + 16]",
    "add rax, fs:[0]",
    "ret",
);

unsafe extern "C" {
    #[link_name = "eggos_macos_tlv_get_address"]
    pub fn tlv_get_address();
}

/// # Safety
/// `destructor` must be a function that takes `object`.
pub unsafe extern "C" fn tlv_atexit(destructor: usize, object: usize) {
    // SAFETY: the control block lives as long as its thread, which is the caller.
    let thread = unsafe { &mut *current() };
    if thread.exit_handler_count == thread.exit_handler_capacity {
        let capacity = (thread.exit_handler_capacity * 2).max(8);
        let grown = heap::reallocate(thread.exit_handlers as *mut u8, capacity * 16);
        if grown.is_null() {
            return;
        }
        thread.exit_handlers = grown as usize;
        thread.exit_handler_capacity = capacity;
    }
    let slot = (thread.exit_handlers as *mut [usize; 2]).wrapping_add(thread.exit_handler_count);
    // SAFETY: the array holds `exit_handler_capacity` pairs.
    unsafe { slot.write([destructor, object]) };
    thread.exit_handler_count += 1;
}

fn run_exit_handlers(thread: *mut Thread) {
    loop {
        // SAFETY: the calling thread's own control block.
        let block = unsafe { &mut *thread };
        if block.exit_handler_count == 0 {
            break;
        }
        block.exit_handler_count -= 1;
        let slot =
            (block.exit_handlers as *const [usize; 2]).wrapping_add(block.exit_handler_count);
        // SAFETY: a pair `tlv_atexit` stored.
        let [destructor, object] = unsafe { slot.read() };
        // SAFETY: `tlv_atexit`'s contract.
        let destructor: extern "C" fn(usize) = unsafe { core::mem::transmute(destructor) };
        destructor(object);
    }
    for round in 0..KEY_DESTRUCTOR_ROUNDS {
        let mut ran = false;
        for (key, destructor) in KEY_DESTRUCTORS.iter().enumerate() {
            let destructor = destructor.load(Ordering::Acquire);
            // SAFETY: as above.
            let value = unsafe { &mut (*thread).values[key] };
            if destructor != 0 && *value != 0 {
                let argument = core::mem::take(value);
                // SAFETY: `pthread_key_create`'s contract: a function that takes the value.
                let destructor: extern "C" fn(usize) = unsafe { core::mem::transmute(destructor) };
                destructor(argument);
                ran = true;
            }
        }
        if !ran || round + 1 == KEY_DESTRUCTOR_ROUNDS {
            break;
        }
    }
    // SAFETY: as above.
    let block = unsafe { &mut *thread };
    heap::release(core::mem::take(&mut block.exit_handlers) as *mut u8);
    block.exit_handler_capacity = 0;
}

/// Runs the calling thread's thread-local and key destructors, as Darwin does at `exit`.
pub fn run_destructors() {
    run_exit_handlers(current());
}

fn bury(thread: *mut Thread) {
    BURIED.with(|buried| {
        if let Some(slot) = buried.iter_mut().find(|slot| **slot == 0) {
            *slot = thread as usize;
        }
    });
}

fn reap() {
    BURIED.with(|buried| {
        for slot in buried.iter_mut().filter(|slot| **slot != 0) {
            let thread = *slot as *const Thread;
            // SAFETY: a buried thread's mapping stays until it is reaped here.
            let block = unsafe { &*thread };
            if block.id_word.load(Ordering::Acquire) == 0 {
                let (mapping, length) = (block.mapping, block.mapping_length);
                // SAFETY: the kernel cleared the id word, so the thread no longer runs there.
                let _ = unsafe { linux::unmap(mapping, length) };
                *slot = 0;
            }
        }
    });
}

static FINISH_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Sets what a created thread runs first when it ends (the Objective-C runtime's pool drain).
pub fn set_finish_hook(hook: fn()) {
    FINISH_HOOK.store(hook as usize, Ordering::Release);
}

fn finish(result: usize) -> ! {
    let thread = current();
    run_exit_handlers(thread);
    let hook = FINISH_HOOK.load(Ordering::Acquire);
    if hook != 0 && thread != &raw mut MAIN {
        // SAFETY: only `set_finish_hook` stores here, a `fn()`.
        let hook: fn() = unsafe { core::mem::transmute(hook) };
        hook();
    }
    // SAFETY: the calling thread's own control block.
    let block = unsafe { &mut *thread };
    heap::release(core::mem::take(&mut block.thread_locals) as *mut u8);
    block.result = result;
    if thread != &raw mut MAIN && block.state.swap(FINISHED, Ordering::AcqRel) == DETACHED {
        bury(thread);
    }
    linux::exit_thread()
}

extern "C" fn thread_main(thread: usize) -> ! {
    let thread = thread as *mut Thread;
    // SAFETY: the block `pthread_create` made for this thread, which it no longer writes.
    let block = unsafe { &*thread };
    block.id.store(linux::thread_id(), Ordering::Relaxed);
    // SAFETY: `pthread_create`'s contract: `start` takes one pointer and returns one.
    let start: extern "C" fn(usize) -> usize = unsafe { core::mem::transmute(block.start) };
    finish(start(block.argument))
}

/// Darwin's 64-byte `pthread_attr_t`.
#[repr(C)]
#[derive(Debug)]
pub struct Attributes {
    signature: u64,
    stack_size: usize,
    detach_state: c_int,
    unused: [u32; 11],
}

/// # Safety
/// `attributes` must be writable.
pub unsafe extern "C" fn pthread_attr_init(attributes: *mut Attributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe {
        attributes.write(Attributes {
            signature: ATTRIBUTES_SIGNATURE,
            stack_size: DEFAULT_STACK,
            detach_state: CREATE_JOINABLE,
            unused: [0; 11],
        })
    };
    0
}

/// # Safety
/// As [`pthread_attr_init`].
pub unsafe extern "C" fn pthread_attr_destroy(attributes: *mut Attributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { (*attributes).signature = 0 };
    0
}

/// # Safety
/// As [`pthread_attr_init`].
pub unsafe extern "C" fn pthread_attr_setstacksize(
    attributes: *mut Attributes,
    size: usize,
) -> c_int {
    if !(MINIMUM_STACK..=MAXIMUM_STACK).contains(&size) {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe { (*attributes).stack_size = size };
    0
}

/// # Safety
/// As [`pthread_attr_init`]; `size` must be writable.
pub unsafe extern "C" fn pthread_attr_getstacksize(
    attributes: *const Attributes,
    size: *mut usize,
) -> c_int {
    // SAFETY: the caller's promise.
    let value = unsafe { (*attributes).stack_size };
    // SAFETY: as above.
    unsafe { size.write(value) };
    0
}

/// # Safety
/// As [`pthread_attr_init`].
pub unsafe extern "C" fn pthread_attr_setdetachstate(
    attributes: *mut Attributes,
    state: c_int,
) -> c_int {
    if state != CREATE_JOINABLE && state != CREATE_DETACHED {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe { (*attributes).detach_state = state };
    0
}

/// # Safety
/// As [`pthread_attr_getstacksize`].
pub unsafe extern "C" fn pthread_attr_getdetachstate(
    attributes: *const Attributes,
    state: *mut c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    let value = unsafe { (*attributes).detach_state };
    // SAFETY: as above.
    unsafe { state.write(value) };
    0
}

pub extern "C" fn pthread_attr_setguardsize(_: *mut Attributes, _: usize) -> c_int {
    0
}

/// # Safety
/// `out` must be writable, `attributes` null or initialized, and `start` a function that takes
/// `argument` and returns a pointer.
pub unsafe extern "C" fn pthread_create(
    out: *mut *mut Thread,
    attributes: *const Attributes,
    start: usize,
    argument: usize,
) -> c_int {
    reap();
    let (stack_size, detach_state) = if attributes.is_null() {
        (DEFAULT_STACK, CREATE_JOINABLE)
    } else {
        // SAFETY: the caller's promise.
        let attributes = unsafe { &*attributes };
        (attributes.stack_size, attributes.detach_state)
    };
    let stack_size = stack_size.next_multiple_of(linux::PAGE_SIZE);
    let control = size_of::<Thread>().next_multiple_of(linux::PAGE_SIZE);
    let length = GUARD + stack_size + control;
    let Ok(mapping) = linux::map_anonymous(0, length, PROT_READ | PROT_WRITE, false) else {
        return EAGAIN;
    };
    // SAFETY: the guard page is the bottom of the new mapping, which nothing uses yet.
    let _ = unsafe { linux::protect(mapping, GUARD, 0) };
    let Some(thread_locals) = new_thread_locals() else {
        // SAFETY: the new mapping is not used by any thread.
        let _ = unsafe { linux::unmap(mapping, length) };
        return EAGAIN;
    };
    let stack_top = mapping + GUARD + stack_size;
    let thread = stack_top as *mut Thread;
    // SAFETY: the control block's pages are part of the new mapping, zeroed and unused.
    let block = unsafe { &mut *thread };
    block.thread_locals = thread_locals;
    block.this = thread as usize;
    block.mapping = mapping;
    block.mapping_length = length;
    block.stack_top = stack_top;
    block.stack_size = stack_size;
    block.start = start;
    block.argument = argument;
    let detached = detach_state == CREATE_DETACHED;
    block.state.store(
        if detached { DETACHED } else { JOINABLE },
        Ordering::Relaxed,
    );
    let id_word = block.id_word.as_ptr();
    // SAFETY: the stack's top is page-aligned and only the new thread uses the stack until the
    // kernel clears `id_word`, which lives in the mapping that is unmapped only after that.
    match unsafe {
        linux::spawn_thread(
            stack_top,
            thread as usize,
            id_word,
            thread_main,
            thread as usize,
        )
    } {
        Ok(_) => {
            if !out.is_null() {
                // SAFETY: the caller's promise.
                unsafe { out.write(thread) };
            }
            0
        }
        Err(_) => {
            heap::release(thread_locals as *mut u8);
            // SAFETY: the thread never started.
            let _ = unsafe { linux::unmap(mapping, length) };
            EAGAIN
        }
    }
}

/// # Safety
/// `thread` must come from `pthread_create`, be joinable and not joined, and `result` be null
/// or writable.
pub unsafe extern "C" fn pthread_join(thread: *mut Thread, result: *mut usize) -> c_int {
    if thread.is_null() {
        return ESRCH;
    }
    if thread == current() {
        return EDEADLK;
    }
    // SAFETY: the caller's promise: the block stays mapped until this join unmaps it.
    let block = unsafe { &*thread };
    if thread == &raw mut MAIN || block.state.load(Ordering::Acquire) == DETACHED {
        return EINVAL;
    }
    loop {
        let id = block.id_word.load(Ordering::Acquire);
        if id == 0 {
            break;
        }
        linux::futex_wait_shared(&block.id_word, id);
    }
    if !result.is_null() {
        // SAFETY: the caller's promise.
        unsafe { result.write(block.result) };
    }
    let (mapping, length) = (block.mapping, block.mapping_length);
    // SAFETY: the kernel cleared the id word, so the thread no longer uses its mapping.
    let _ = unsafe { linux::unmap(mapping, length) };
    0
}

/// # Safety
/// `thread` must come from `pthread_create` and be neither joined nor detached.
pub unsafe extern "C" fn pthread_detach(thread: *mut Thread) -> c_int {
    if thread.is_null() || thread == &raw mut MAIN {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    match unsafe { &(*thread).state }.swap(DETACHED, Ordering::AcqRel) {
        FINISHED => {
            bury(thread);
            0
        }
        DETACHED => EINVAL,
        _ => 0,
    }
}

pub extern "C" fn pthread_exit(result: usize) -> ! {
    finish(result)
}

pub extern "C" fn pthread_self() -> *mut Thread {
    current()
}

pub extern "C" fn pthread_equal(left: *mut Thread, right: *mut Thread) -> c_int {
    c_int::from(left == right)
}

pub extern "C" fn pthread_main_np() -> c_int {
    c_int::from(current() == &raw mut MAIN)
}

fn thread_or_current(thread: *mut Thread) -> *mut Thread {
    if thread.is_null() { current() } else { thread }
}

/// # Safety
/// `thread` must be null or a live thread, and `id` writable.
pub unsafe extern "C" fn pthread_threadid_np(thread: *mut Thread, id: *mut u64) -> c_int {
    if id.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    let value = unsafe { id_of(thread_or_current(thread)) };
    // SAFETY: as above.
    unsafe { id.write(u64::from(value)) };
    0
}

/// # Safety
/// `thread` must be a live thread.
pub unsafe extern "C" fn pthread_mach_thread_np(thread: *mut Thread) -> u32 {
    // SAFETY: the caller's promise.
    unsafe { id_of(thread_or_current(thread)) }
}

unsafe fn id_of(thread: *mut Thread) -> u32 {
    // SAFETY: the caller's duty: a live thread's block.
    let block = unsafe { &*thread };
    match block.id.load(Ordering::Relaxed) {
        0 => block.id_word.load(Ordering::Acquire),
        id => id,
    }
}

/// # Safety
/// As [`pthread_mach_thread_np`].
pub unsafe extern "C" fn pthread_get_stackaddr_np(thread: *mut Thread) -> usize {
    // SAFETY: the caller's promise.
    unsafe { (*thread_or_current(thread)).stack_top }
}

/// # Safety
/// As [`pthread_mach_thread_np`].
pub unsafe extern "C" fn pthread_get_stacksize_np(thread: *mut Thread) -> usize {
    // SAFETY: the caller's promise.
    unsafe { (*thread_or_current(thread)).stack_size }
}

pub extern "C" fn pthread_setname_np(_: *const c_char) -> c_int {
    0
}

/// # Safety
/// `buffer` must be writable for `length` bytes.
pub unsafe extern "C" fn pthread_getname_np(
    _: *mut Thread,
    buffer: *mut c_char,
    length: usize,
) -> c_int {
    if length == 0 {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe { buffer.write(0) };
    0
}

/// # Safety
/// `key` must be writable and `destructor` null or a function that takes one pointer.
pub unsafe extern "C" fn pthread_key_create(key: *mut usize, destructor: usize) -> c_int {
    let Some(free) = KEY_IN_USE.iter().position(|used| {
        used.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }) else {
        return EAGAIN;
    };
    KEY_DESTRUCTORS[free].store(destructor, Ordering::Release);
    // SAFETY: the caller's promise.
    unsafe { key.write(free) };
    0
}

pub extern "C" fn pthread_key_delete(key: usize) -> c_int {
    match KEY_IN_USE.get(key) {
        Some(used) if used.load(Ordering::Acquire) == 1 => {
            KEY_DESTRUCTORS[key].store(0, Ordering::Release);
            used.store(0, Ordering::Release);
            0
        }
        _ => EINVAL,
    }
}

pub extern "C" fn pthread_getspecific(key: usize) -> *mut c_void {
    if key >= KEYS {
        return core::ptr::null_mut();
    }
    // SAFETY: the calling thread's own control block.
    unsafe { (*current()).values[key] as *mut c_void }
}

pub extern "C" fn pthread_setspecific(key: usize, value: *const c_void) -> c_int {
    if key >= KEYS {
        return EINVAL;
    }
    // SAFETY: as above.
    unsafe { (*current()).values[key] = value as usize };
    0
}

pub extern "C" fn sched_yield() -> c_int {
    linux::yield_now();
    0
}
