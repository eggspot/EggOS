//! The layer's own C++ throws (D230): a standard exception object (`std_exceptions.rs`) made on
//! the throwing frame of the layer, which stays on the stack until the catch block that takes
//! it ends, raised as `_CxxThrowException` raises the program's with the layer's type table as
//! the throw's image. A function that always throws is a capturing entry, so the dispatch
//! starts at the program's call; one that throws only sometimes (`dynamic_cast` to a
//! reference, `typeid`) returns through a `throwing_entry!` wrapper, which checks the thread's
//! owed throw after the call and, with the program's registers as they were at the call,
//! jumps to the capturing entry that raises it.

use super::super::heap;
use super::super::thread_environment::{self, teb};
use super::context::{Context, capturing_entry};
use super::dispatch::{self, Origin};
use super::{ExceptionRecord, cxx, flag, frames};
use crate::cxx_data::{CXX_EXCEPTION, CXX_MAGIC};
use crate::std_exceptions::{StdException, TypeTable};
use crate::unwind::Image;

static TYPES: TypeTable = TypeTable::new((&raw const cxx::TYPE_INFO_TABLE).cast());

const DELETE_MEMORY: u32 = 1;
const NO_THROW_OWED: u64 = 0;

#[repr(C)]
struct VirtualTable(#[allow(dead_code, reason = "the program reads it")] [*const (); 3]);

// SAFETY: the tables hold addresses of the layer's functions and table, immutable for the
// program's life.
unsafe impl Sync for VirtualTable {}

macro_rules! virtual_tables {
    ($($kind:ident),* $(,)?) => {
        [$(VirtualTable([
            TYPES.locator(StdException::$kind),
            export_entry!(delete_exception),
            export_entry!(what),
        ])),*]
    };
}

static VIRTUAL_TABLES: [VirtualTable; StdException::ALL.len()] = virtual_tables![
    Exception,
    BadAlloc,
    BadArrayNewLength,
    BadCast,
    BadTypeid,
    NonRttiObject,
    BadFunctionCall,
    BadException,
    LogicError,
    LengthError,
    OutOfRange,
    InvalidArgument,
    RuntimeError,
    OverflowError,
    SystemErrorBase,
    SystemError,
];

/// An object of one of the layer's exception classes: `std::exception`'s virtual function
/// table pointer and data, then `std::system_error`'s `std::error_code` (its value and its
/// category), which the other classes do not have.
#[repr(C)]
pub struct StdObject {
    #[allow(dead_code, reason = "the program reads it")]
    table: *const (),
    message: *const u8,
    owns_message: u64,
    pub code: i32,
    pub category: *const (),
}

impl StdObject {
    /// An object of `kind` saying `message` (NUL-terminated, never freed), or the class's own
    /// words when `message` is null.
    pub fn new(kind: StdException, message: *const u8) -> Self {
        Self {
            table: (&raw const VIRTUAL_TABLES[kind.index()].0[1]).cast(),
            message: match message.is_null() {
                true => kind.default_message().as_ptr(),
                false => message,
            },
            owns_message: 0,
            code: 0,
            category: core::ptr::null(),
        }
    }
}

unsafe extern "efiapi" fn what(object: *const StdObject) -> *const u8 {
    // SAFETY: the virtual function's contract: `object` is one of these objects or a copy.
    let message = unsafe { (*object).message };
    match message.is_null() {
        true => StdException::Exception.default_message().as_ptr(),
        false => message,
    }
}

unsafe extern "efiapi" fn delete_exception(object: *mut StdObject, flags: u32) -> *mut StdObject {
    // SAFETY: the scalar deleting destructor's contract: `object` is one of these objects.
    let owned = unsafe { &mut *object };
    if owned.owns_message != 0 {
        // SAFETY: an owned message came from this heap (`__std_exception_copy`).
        unsafe { heap::free(owned.message.cast_mut()) };
        owned.message = core::ptr::null();
        owned.owns_message = 0;
    }
    if flags & DELETE_MEMORY != 0 {
        // SAFETY: a deleted object was allocated by `operator new`, which is this heap's.
        unsafe { heap::free(object.cast()) };
    }
    object
}

/// The image whose `ThrowInfo` describes the object of the C++ exception `record`: the layer's
/// type table when the record names it as the throw's image, the program's otherwise.
pub fn image_of(record: &ExceptionRecord) -> Image<'static> {
    let table = TYPES.image();
    match record.parameters().get(3) {
        Some(&base) if base == table.base => table,
        _ => frames::image(),
    }
}

/// The image base a throw with the `ThrowInfo` at `info` names.
pub fn image_base_of_throw_info(info: u64) -> u64 {
    match TYPES.holds(info) {
        true => TYPES.image().base,
        false => frames::image().base,
    }
}

/// Raises the C++ exception of `object` described by the `ThrowInfo` at `info` in the image at
/// `image_base`, as thrown by the program at `context`.
pub fn raise(object: u64, info: u64, image_base: u64, context: *mut Context) -> ! {
    cxx::count_thrown();
    // SAFETY: the capturing entry's `CONTEXT`.
    let pc = unsafe { (*context).pc() };
    let parameters = [CXX_MAGIC, object, info, image_base];
    let mut record = ExceptionRecord::new(CXX_EXCEPTION, flag::NONCONTINUABLE, pc, &parameters);
    dispatch::dispatch(&raw mut record, context, Origin::Raised)
}

/// Throws `object` of `kind` as the program at `context` would. The object lives in this
/// frame, which no unwind leaves before the catch block that takes it has ended.
pub fn throw_object(kind: StdException, object: StdObject, context: *mut Context) -> ! {
    let image_base = TYPES.image().base;
    raise(
        &raw const object as u64,
        TYPES.throw_info(kind),
        image_base,
        context,
    )
}

/// Throws a `kind` saying `message` (null for the class's own words).
pub fn throw(kind: StdException, message: *const u8, context: *mut Context) -> ! {
    throw_object(kind, StdObject::new(kind, message), context)
}

fn owed_slot() -> *mut u64 {
    (thread_environment::current() + teb::PENDING_THROW) as *mut u64
}

/// Asks the `throwing_entry!` wrapper of the running export to throw a `kind` once the export
/// returns.
pub fn owe(kind: StdException) {
    // SAFETY: this thread's TEB word.
    unsafe { owed_slot().write(kind.index() as u64 + 1) };
}

capturing_entry!(pub(super) owed_throw_entry => throw_owed);

extern "C" fn throw_owed(_: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    // SAFETY: this thread's TEB word.
    let owed = unsafe { owed_slot().replace(NO_THROW_OWED) };
    let kind = usize::try_from(owed)
        .ok()
        .and_then(|owed| StdException::ALL.get(owed.wrapping_sub(1)).copied())
        .unwrap_or(StdException::BadException);
    throw(kind, kind.runtime_message().as_ptr(), context)
}

/// Wraps an export that may owe the program a throw (`owe`): the wrapper calls it with its
/// arguments (on x64 the fifth, from the caller's stack, too) and returns its result, or, when
/// the thread owes a throw, leaves the stack as the program's call found it and jumps to
/// `owed_throw_entry`, which raises it from the program's call.
#[cfg(target_arch = "x86_64")]
macro_rules! throwing_entry {
    ($function:path) => {{
        #[unsafe(naked)]
        unsafe extern "efiapi" fn entry() {
            core::arch::naked_asm!(
                "sub rsp, 56",
                "mov rax, [rsp + 96]",
                "mov [rsp + 32], rax",
                "call {function}",
                "add rsp, 56",
                "mov r10, gs:[{teb_self}]",
                "cmp qword ptr [r10 + {owed}], 0",
                "jne {throw}",
                "ret",
                function = sym $function,
                teb_self = const crate::win32::thread_environment::teb::SELF,
                owed = const crate::win32::thread_environment::teb::PENDING_THROW,
                throw = sym crate::win32::exception::thrown::owed_throw_entry,
            )
        }
        entry as *const ()
    }};
}

#[cfg(target_arch = "aarch64")]
macro_rules! throwing_entry {
    ($function:path) => {{
        #[unsafe(naked)]
        unsafe extern "efiapi" fn entry() {
            core::arch::naked_asm!(
                "stp x18, x30, [sp, #-16]!",
                "bl {function}",
                "ldp x18, x30, [sp], #16",
                "ldr x9, [x18, #{owed}]",
                "cbz x9, 1f",
                "b {throw}",
                "1:",
                "ret",
                function = sym $function,
                owed = const crate::win32::thread_environment::teb::PENDING_THROW,
                throw = sym crate::win32::exception::thrown::owed_throw_entry,
            )
        }
        entry as *const ()
    }};
}

pub(super) use throwing_entry;
