//! Objective-C exceptions (D175): `@throw`, `-[NSException raise]`, `@catch`, `@finally` and
//! `objc_exception_rethrow`, unwound as Apple's runtime does, in two phases over the program's
//! frames. A throw saves the caller's callee-saved registers in an assembly stub; the search
//! phase steps from frame to frame with the image's compact unwind information
//! ([`unwind_info`]) and asks each frame's LSDA ([`lsda`]) whether a `@catch` takes the object;
//! the cleanup phase walks the same frames again and jumps to each cleanup's landing pad
//! (`@finally`, ARC releases) until the handler's, whose pad gets the selector of the catch
//! that matched. A frame outside the program's image (the layer's own code) or one only DWARF
//! describes ends the search: the exception is uncaught, and the program ends with its name and
//! reason, as on macOS.

use super::abi::{ClassObject, Id};
use super::lsda::{self, Found};
use super::reference_count::{objc_autorelease, objc_retain};
use super::root;
use super::unwind_info::{self, FunctionInfo};
use crate::libsystem::{heap, thread};
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "aarch64")]
pub use super::unwind_info::Arm64Registers as Registers;
#[cfg(target_arch = "x86_64")]
pub use super::unwind_info::X86_64Registers as Registers;

const MAX_FRAMES: usize = 4096;
const STACK_WINDOW: u64 = 256 << 20;

static IMAGE_START: AtomicUsize = AtomicUsize::new(0);
static IMAGE_END: AtomicUsize = AtomicUsize::new(0);
static UNWIND_INFO: AtomicUsize = AtomicUsize::new(0);
static UNWIND_INFO_SIZE: AtomicUsize = AtomicUsize::new(0);
static UNCAUGHT_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Records where the program's image is and its `__unwind_info` section (slid), if it has one.
pub fn set_image(start: usize, size: usize, unwind_info: Option<(usize, usize)>) {
    IMAGE_START.store(start, Ordering::Relaxed);
    IMAGE_END.store(start.saturating_add(size), Ordering::Relaxed);
    let (address, length) = unwind_info.unwrap_or((0, 0));
    UNWIND_INFO.store(address, Ordering::Relaxed);
    UNWIND_INFO_SIZE.store(length, Ordering::Release);
}

/// Sets the function called with an exception nothing catches, returning the previous one
/// (`objc_setUncaughtExceptionHandler`, `NSSetUncaughtExceptionHandler`).
pub fn swap_uncaught_handler(handler: usize) -> usize {
    UNCAUGHT_HANDLER.swap(handler, Ordering::AcqRel)
}

/// The function called with an exception nothing catches, 0 for none.
pub fn uncaught_handler() -> usize {
    UNCAUGHT_HANDLER.load(Ordering::Acquire)
}

#[repr(C)]
#[derive(Debug)]
struct Raised {
    object: Id,
    handler_frame: u64,
    rethrown: bool,
    next: *mut Raised,
}

/// `objc_typeinfo`: what a `@catch` clause names in the LSDA's type table.
#[repr(C)]
#[derive(Debug)]
pub struct TypeInfo {
    vtable: *const usize,
    _name: *const u8,
    class: *const ClassObject,
}

// SAFETY: the records are never written; their pointers are to permanent statics.
unsafe impl Sync for TypeInfo {}

static EHTYPE_VTABLE: [usize; 3] = [0; 3];

const fn vtable_pointer() -> *const usize {
    (&raw const EHTYPE_VTABLE).cast::<usize>().wrapping_add(2)
}

/// `@catch (id)`: any object.
pub static OBJC_EHTYPE_ID: TypeInfo = TypeInfo {
    vtable: vtable_pointer(),
    _name: b"id\0".as_ptr(),
    class: core::ptr::null(),
};

/// `@catch (NSException *)`.
pub static OBJC_EHTYPE_NSEXCEPTION: TypeInfo = TypeInfo {
    vtable: vtable_pointer(),
    _name: b"NSException\0".as_ptr(),
    class: &raw const crate::foundation::NSEXCEPTION,
};

fn image_start() -> u64 {
    IMAGE_START.load(Ordering::Relaxed) as u64
}

fn in_image(address: u64, length: u64) -> bool {
    let start = image_start();
    let end = IMAGE_END.load(Ordering::Relaxed) as u64;
    address >= start && address.checked_add(length).is_some_and(|last| last <= end)
}

fn image_byte(address: u64) -> Option<u8> {
    if !in_image(address, 1) {
        return None;
    }
    // SAFETY: a byte of the mapped image, which stays mapped while the program runs.
    Some(unsafe { (address as *const u8).read() })
}

fn read_word(address: u64, floor: u64) -> Option<u64> {
    let in_stack = address >= floor
        && address < floor.saturating_add(STACK_WINDOW)
        && address.is_multiple_of(8);
    if !in_stack && !in_image(address, 8) {
        return None;
    }
    // SAFETY: a word of the image, or of the thread's stack above the frame being stepped
    // over, where the frame's unwind information says a saved register is.
    Some(unsafe { (address as *const u64).read_unaligned() })
}

fn function_at(pc: u64) -> Option<FunctionInfo> {
    let lookup = pc.checked_sub(1)?;
    if !in_image(lookup, 1) {
        return None;
    }
    let size = UNWIND_INFO_SIZE.load(Ordering::Acquire);
    let address = UNWIND_INFO.load(Ordering::Relaxed);
    if address == 0 || size == 0 {
        return None;
    }
    // SAFETY: the image's `__unwind_info` section, checked inside a mapped segment by the
    // loader, which nothing writes.
    let section = unsafe { core::slice::from_raw_parts(address as *const u8, size) };
    unwind_info::find(section, u32::try_from(lookup - image_start()).ok()?)
}

fn catches(type_info: u64, object: Id) -> bool {
    if type_info == 0 || type_info == &raw const OBJC_EHTYPE_ID as u64 {
        return true;
    }
    if !in_image(type_info, 24) && type_info != &raw const OBJC_EHTYPE_NSEXCEPTION as u64 {
        return false;
    }
    // SAFETY: a type information record of the image's or the layer's, at least three words.
    let info = unsafe { &*(type_info as *const TypeInfo) };
    if info.vtable != vtable_pointer() {
        return false;
    }
    info.class.is_null() || root::is_kind_of(object, info.class)
}

fn landing(info: &FunctionInfo, pc: u64, object: Id) -> Found {
    let Some(lsda) = info.lsda else {
        return Found::Nothing;
    };
    let image = image_start();
    let read = |address: u64| image_byte(address);
    let matches = |type_info: u64| catches(type_info, object);
    lsda::search(
        image + u64::from(lsda),
        image + u64::from(info.start),
        pc - 1,
        &read,
        &matches,
    )
    .unwrap_or(Found::Nothing)
}

#[cfg(target_arch = "aarch64")]
mod frame {
    use super::{FunctionInfo, Registers, unwind_info};

    pub fn pc(registers: &Registers) -> u64 {
        registers.pc
    }

    pub fn sp(registers: &Registers) -> u64 {
        registers.sp
    }

    pub fn land(registers: &mut Registers, pad: u64, raised: u64, selector: i64) {
        registers.pc = pad;
        registers.x[0] = raised;
        registers.x[1] = selector as u64;
    }

    pub fn step(
        info: &FunctionInfo,
        _: u64,
        registers: &mut Registers,
        read: &dyn Fn(u64) -> Option<u64>,
    ) -> bool {
        unwind_info::step_arm64(info.encoding, registers, read).is_ok()
    }
}

#[cfg(target_arch = "x86_64")]
mod frame {
    use super::{FunctionInfo, Registers, unwind_info};
    use unwind_info::x86_64::{RAX, RDX, RSP};

    pub fn pc(registers: &Registers) -> u64 {
        registers.rip
    }

    pub fn sp(registers: &Registers) -> u64 {
        registers.general[RSP]
    }

    pub fn land(registers: &mut Registers, pad: u64, raised: u64, selector: i64) {
        registers.rip = pad;
        registers.general[RAX] = raised;
        registers.general[RDX] = selector as u64;
    }

    pub fn step(
        info: &FunctionInfo,
        function_start: u64,
        registers: &mut Registers,
        read: &dyn Fn(u64) -> Option<u64>,
    ) -> bool {
        unwind_info::step_x86_64(info.encoding, function_start, registers, read).is_ok()
    }
}

fn step(info: &FunctionInfo, registers: &mut Registers) -> bool {
    let floor = frame::sp(registers);
    let read = |address: u64| read_word(address, floor);
    let start = image_start() + u64::from(info.start);
    frame::step(info, start, registers, &read)
}

fn search(start: &Registers, object: Id) -> Option<u64> {
    let mut registers = *start;
    for _ in 0..MAX_FRAMES {
        let pc = frame::pc(&registers);
        let info = function_at(pc)?;
        if let Found::Handler { .. } = landing(&info, pc, object) {
            return Some(frame::sp(&registers));
        }
        if !step(&info, &mut registers) {
            return None;
        }
    }
    None
}

unsafe extern "C" {
    fn eggos_install_context(registers: *const Registers) -> !;
}

fn install(mut registers: Registers, pad: u64, raised: *mut Raised, selector: i64) -> ! {
    frame::land(&mut registers, pad, raised as u64, selector);
    // SAFETY: the registers are a frame of the program the unwinder walked to, the pad one of
    // its landing pads, which take the exception and selector in the first two registers.
    unsafe { eggos_install_context(&registers) }
}

fn clean_up_to_handler(raised: *mut Raised, start: Registers) -> ! {
    // SAFETY: an exception record made by `new_raised`, alive until its catch ends.
    let (object, handler) = unsafe { ((*raised).object, (*raised).handler_frame) };
    let mut registers = start;
    for _ in 0..MAX_FRAMES {
        let pc = frame::pc(&registers);
        let Some(info) = function_at(pc) else {
            break;
        };
        let found = landing(&info, pc, object);
        if frame::sp(&registers) == handler {
            if let Found::Handler {
                landing_pad,
                selector,
            } = found
            {
                install(registers, landing_pad, raised, selector);
            }
            break;
        }
        if let Found::Cleanup(pad) = found {
            install(registers, pad, raised, 0);
        }
        if !step(&info, &mut registers) {
            break;
        }
    }
    report_uncaught(object)
}

fn raise(raised: *mut Raised, start: Registers) -> ! {
    // SAFETY: as in `clean_up_to_handler`.
    let object = unsafe { (*raised).object };
    match search(&start, object) {
        Some(frame) => {
            // SAFETY: as above.
            unsafe { (*raised).handler_frame = frame };
            clean_up_to_handler(raised, start)
        }
        None => report_uncaught(object),
    }
}

fn new_raised(object: Id) -> *mut Raised {
    let raised = heap::allocate(core::mem::size_of::<Raised>(), true).cast::<Raised>();
    if raised.is_null() {
        report_uncaught(object);
    }
    objc_retain(object);
    // SAFETY: a new block of the record's size.
    unsafe {
        raised.write(Raised {
            object,
            handler_frame: 0,
            rethrown: false,
            next: core::ptr::null_mut(),
        })
    };
    raised
}

/// Throws `object` from the frame whose registers an exception stub saved at `registers`.
pub fn throw_from(object: Id, registers: *const Registers) -> ! {
    if object.is_null() {
        report_uncaught(object);
    }
    // SAFETY: the stub's saved registers, on its stack below the caller of this function.
    let start = unsafe { *registers };
    raise(new_raised(object), start)
}

extern "C" fn throw(object: Id, registers: *const Registers) -> ! {
    throw_from(object, registers)
}

extern "C" fn resume(raised: *mut Raised, registers: *const Registers) -> ! {
    if raised.is_null() {
        crate::libsystem::abort()
    }
    // SAFETY: as in `throw_from`.
    let start = unsafe { *registers };
    clean_up_to_handler(raised, start)
}

fn caught() -> *mut Raised {
    // SAFETY: the calling thread's own control block.
    unsafe { (*thread::current()).caught_exceptions as *mut Raised }
}

fn set_caught(raised: *mut Raised) {
    // SAFETY: as above.
    unsafe { (*thread::current()).caught_exceptions = raised as usize };
}

extern "C" fn rethrow(_: usize, registers: *const Registers) -> ! {
    let top = caught();
    if top.is_null() {
        crate::libsystem::abort()
    }
    // SAFETY: the record of the exception the innermost `@catch` is handling.
    unsafe {
        (*top).rethrown = true;
        (*top).handler_frame = 0;
    }
    // SAFETY: as in `throw_from`.
    let start = unsafe { *registers };
    raise(top, start)
}

extern "C" fn begin_catch(raised: *mut Raised) -> Id {
    if raised.is_null() {
        return core::ptr::null_mut();
    }
    let top = caught();
    // SAFETY: a record a landing pad got from the unwinder.
    unsafe {
        (*raised).rethrown = false;
        if top != raised {
            (*raised).next = top;
        }
    }
    set_caught(raised);
    // SAFETY: as above.
    unsafe { (*raised).object }
}

extern "C" fn end_catch() {
    let top = caught();
    if top.is_null() {
        return;
    }
    // SAFETY: the record the innermost `@catch` began with.
    let (next, rethrown, object) = unsafe { ((*top).next, (*top).rethrown, (*top).object) };
    set_caught(next);
    if !rethrown {
        objc_autorelease(object);
        heap::release(top.cast());
    }
}

fn report_uncaught(object: Id) -> ! {
    let handler = uncaught_handler();
    if handler != 0 && !object.is_null() {
        // SAFETY: the program set it to a function that takes the exception.
        let handler: extern "C" fn(Id) = unsafe { core::mem::transmute(handler) };
        handler(object);
    }
    crate::foundation::terminate_with(object)
}

extern "C" fn personality() -> ! {
    crate::libsystem::abort()
}

extern "C" fn objc_terminate() -> ! {
    crate::libsystem::abort()
}

extern "C" fn set_uncaught_exception_handler(handler: usize) -> usize {
    swap_uncaught_handler(handler)
}

macro_rules! capture {
    ($entry:ident => $target:path, $arm64:literal, $x86_64:literal) => {
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 2",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub sp, sp, #336",
            "stp x19, x20, [sp, #152]",
            "stp x21, x22, [sp, #168]",
            "stp x23, x24, [sp, #184]",
            "stp x25, x26, [sp, #200]",
            "stp x27, x28, [sp, #216]",
            "stp x29, x30, [sp, #232]",
            "add x9, sp, #336",
            "stp x9, x30, [sp, #248]",
            "stp d8, d9, [sp, #264]",
            "stp d10, d11, [sp, #280]",
            "stp d12, d13, [sp, #296]",
            "stp d14, d15, [sp, #312]",
            concat!("mov ", $arm64, ", sp"),
            "bl {target}",
            "brk #1",
            target = sym $target,
        );
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".text",
            ".p2align 4",
            concat!(".globl ", stringify!($entry)),
            concat!(".hidden ", stringify!($entry)),
            concat!(stringify!($entry), ":"),
            "sub rsp, 152",
            "mov [rsp + 24], rbx",
            "mov [rsp + 48], rbp",
            "lea rax, [rsp + 160]",
            "mov [rsp + 56], rax",
            "mov [rsp + 96], r12",
            "mov [rsp + 104], r13",
            "mov [rsp + 112], r14",
            "mov [rsp + 120], r15",
            "mov rax, [rsp + 152]",
            "mov [rsp + 128], rax",
            concat!("mov ", $x86_64, ", rsp"),
            "call {target}",
            "ud2",
            target = sym $target,
        );
        unsafe extern "C" {
            pub fn $entry();
        }
    };
}

pub(crate) use capture;

capture!(eggos_objc_exception_throw => throw, "x1", "rsi");
capture!(eggos_objc_exception_rethrow => rethrow, "x1", "rsi");
capture!(eggos_unwind_resume => resume, "x1", "rsi");

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_install_context",
    ".hidden eggos_install_context",
    "eggos_install_context:",
    "ldp x19, x20, [x0, #152]",
    "ldp x21, x22, [x0, #168]",
    "ldp x23, x24, [x0, #184]",
    "ldp x25, x26, [x0, #200]",
    "ldp x27, x28, [x0, #216]",
    "ldp x29, x30, [x0, #232]",
    "ldp d8, d9, [x0, #264]",
    "ldp d10, d11, [x0, #280]",
    "ldp d12, d13, [x0, #296]",
    "ldp d14, d15, [x0, #312]",
    "ldr x9, [x0, #248]",
    "ldr x16, [x0, #256]",
    "ldr x1, [x0, #8]",
    "ldr x0, [x0, #0]",
    "mov sp, x9",
    "br x16",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_install_context",
    ".hidden eggos_install_context",
    "eggos_install_context:",
    "mov rbx, [rdi + 24]",
    "mov rbp, [rdi + 48]",
    "mov r12, [rdi + 96]",
    "mov r13, [rdi + 104]",
    "mov r14, [rdi + 112]",
    "mov r15, [rdi + 120]",
    "mov rax, [rdi + 0]",
    "mov rdx, [rdi + 8]",
    "mov rcx, [rdi + 128]",
    "mov rsp, [rdi + 56]",
    "jmp rcx",
);

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_exception_throw" => eggos_objc_exception_throw as *const () as usize,
        b"_objc_exception_rethrow" => eggos_objc_exception_rethrow as *const () as usize,
        b"__Unwind_Resume" => eggos_unwind_resume as *const () as usize,
        b"_objc_begin_catch" => begin_catch as *const () as usize,
        b"_objc_end_catch" => end_catch as *const () as usize,
        b"___objc_personality_v0" => personality as *const () as usize,
        b"_objc_terminate" => objc_terminate as *const () as usize,
        b"_objc_setUncaughtExceptionHandler" => {
            set_uncaught_exception_handler as *const () as usize
        }
        b"_objc_ehtype_vtable" => &raw const EHTYPE_VTABLE as usize,
        b"_OBJC_EHTYPE_id" => &raw const OBJC_EHTYPE_ID as usize,
        b"_OBJC_EHTYPE_$_NSException" => &raw const OBJC_EHTYPE_NSEXCEPTION as usize,
        _ => return None,
    };
    Some(address as u64)
}
