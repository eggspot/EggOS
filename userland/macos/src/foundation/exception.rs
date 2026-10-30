//! NSException (D175): a name, a reason and a user info dictionary; `-raise` and the class's
//! `raise:format:` throw it through the runtime's unwinder (`objc::exception`), from assembly
//! entries that save the caller's registers first, so a `@catch` in the program takes it. An
//! exception nothing catches ends the program with Apple's two lines. Also
//! `NSSetUncaughtExceptionHandler`.

use super::string::format_with;
use super::{
    Definition, NSEXCEPTION, NSEXCEPTION_META, autoreleased, define as define_class, new_instance,
    string_from_utf8, with_utf8,
};
use crate::libsystem::variadic::RawVaList;
use crate::objc::abi::{Id, Sel};
use crate::objc::exception::{self, Registers, capture, eggos_objc_exception_throw};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods, terminate_uncaught};

#[repr(C)]
#[derive(Debug)]
struct ExceptionObject {
    isa: usize,
    name: Id,
    reason: Id,
    user_info: Id,
}

fn fields(object: Id) -> &'static mut ExceptionObject {
    // SAFETY: the callers pass NSException instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ExceptionObject>() }
}

fn copied(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => crate::objc::send(object, crate::objc::sel!("copy")),
    }
}

extern "C" fn init_with_name(this: Id, _: Sel, name: Id, reason: Id, user_info: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = fields(this);
    fields.name = copied(name);
    fields.reason = copied(reason);
    fields.user_info = match user_info.is_null() {
        true => user_info,
        false => objc_retain(user_info),
    };
    this
}

fn new_exception(class: Id, name: Id, reason: Id, user_info: Id) -> Id {
    let object = new_instance(class.cast());
    if object.is_null() {
        return object;
    }
    init_with_name(object, core::ptr::null(), name, reason, user_info)
}

extern "C" fn exception_with_name(this: Id, _: Sel, name: Id, reason: Id, user_info: Id) -> Id {
    autoreleased(new_exception(this, name, reason, user_info))
}

extern "C" fn name(this: Id, _: Sel) -> Id {
    fields(this).name
}

extern "C" fn reason(this: Id, _: Sel) -> Id {
    fields(this).reason
}

extern "C" fn user_info(this: Id, _: Sel) -> Id {
    fields(this).user_info
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    match fields(this).reason {
        none if none.is_null() => fields(this).name,
        reason => reason,
    }
}

extern "C" fn empty_array(_: Id, _: Sel) -> Id {
    super::array::empty_address() as usize as Id
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    for object in [fields.name, fields.reason, fields.user_info] {
        if !object.is_null() {
            objc_release(object);
        }
    }
    dispose(this);
}

extern "C" fn raise_with_format(
    this: Id,
    _: Sel,
    name: Id,
    format: Id,
    arguments: RawVaList,
    registers: *const Registers,
) -> ! {
    let mut formatted = format_with(format, arguments);
    let reason = string_from_utf8(formatted.0.as_slice());
    formatted.0.free();
    let object = autoreleased(new_exception(this, name, reason, core::ptr::null_mut()));
    exception::throw_from(object, registers)
}

capture!(eggos_exception_raise_arguments => raise_with_format, "x5", "r9");

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".p2align 2",
    ".globl eggos_exception_raise_format",
    ".hidden eggos_exception_raise_format",
    "eggos_exception_raise_format:",
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
    "mov x4, x9",
    "mov x5, sp",
    "bl {target}",
    "brk #1",
    target = sym raise_with_format,
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl eggos_exception_raise_format",
    ".hidden eggos_exception_raise_format",
    "eggos_exception_raise_format:",
    "sub rsp, 360",
    "mov [rsp + 144], rdi",
    "mov [rsp + 152], rsi",
    "mov [rsp + 160], rdx",
    "mov [rsp + 168], rcx",
    "mov [rsp + 176], r8",
    "mov [rsp + 184], r9",
    "movaps [rsp + 192], xmm0",
    "movaps [rsp + 208], xmm1",
    "movaps [rsp + 224], xmm2",
    "movaps [rsp + 240], xmm3",
    "movaps [rsp + 256], xmm4",
    "movaps [rsp + 272], xmm5",
    "movaps [rsp + 288], xmm6",
    "movaps [rsp + 304], xmm7",
    "mov dword ptr [rsp + 320], 32",
    "mov dword ptr [rsp + 324], 48",
    "lea rax, [rsp + 368]",
    "mov [rsp + 328], rax",
    "lea rax, [rsp + 144]",
    "mov [rsp + 336], rax",
    "mov [rsp + 24], rbx",
    "mov [rsp + 48], rbp",
    "lea rax, [rsp + 368]",
    "mov [rsp + 56], rax",
    "mov [rsp + 96], r12",
    "mov [rsp + 104], r13",
    "mov [rsp + 112], r14",
    "mov [rsp + 120], r15",
    "mov rax, [rsp + 360]",
    "mov [rsp + 128], rax",
    "lea r8, [rsp + 320]",
    "mov r9, rsp",
    "call {target}",
    "ud2",
    target = sym raise_with_format,
);

unsafe extern "C" {
    fn eggos_exception_raise_format();
}

/// Ends the program because nothing caught `object`, as macOS does: an NSException's name and
/// reason, any other object's class.
pub fn terminate_with(object: Id) -> ! {
    if !object.is_null() && root::is_kind_of(object, &NSEXCEPTION) {
        let fields = fields(object);
        let (name, reason) = (fields.name, fields.reason);
        with_utf8(name, |name| {
            with_utf8(reason, |reason| terminate_uncaught(name, &[reason]));
        });
    }
    crate::libsystem::stdio::flush_all();
    crate::libsystem::write_error(b"*** Terminating app due to uncaught exception of class '");
    crate::libsystem::write_error(root::class_name(object));
    crate::libsystem::write_error(b"'\nlibc++abi: terminating due to uncaught exception of type ");
    crate::libsystem::write_error(root::class_name(object));
    crate::libsystem::write_error(b"\n");
    crate::libsystem::abort()
}

extern "C" fn set_uncaught_handler(handler: usize) {
    exception::swap_uncaught_handler(handler);
}

extern "C" fn get_uncaught_handler() -> usize {
    exception::uncaught_handler()
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_NSSetUncaughtExceptionHandler" => set_uncaught_handler as *const () as usize,
        b"_NSGetUncaughtExceptionHandler" => get_uncaught_handler as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithName:reason:userInfo:" => init_with_name,
        "name" => name,
        "reason" => reason,
        "userInfo" => user_info,
        "description" => description,
        "raise" => eggos_objc_exception_throw,
        "callStackSymbols" => empty_array,
        "callStackReturnAddresses" => empty_array,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "exceptionWithName:reason:userInfo:" => exception_with_name,
        "raise:format:" => eggos_exception_raise_format,
        "raise:format:arguments:" => eggos_exception_raise_arguments,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSException\0",
            class: &NSEXCEPTION,
            meta: &NSEXCEPTION_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<ExceptionObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}
