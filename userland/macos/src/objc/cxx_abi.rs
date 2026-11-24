//! The parts of libc++abi a C++ program on macOS links against beside its exceptions (D380):
//! `operator new` and `delete`, static locals' guards, `__cxa_atexit`, pure and deleted virtual
//! calls, `std::terminate`, and `std::exception` and `std::bad_alloc` with their vtables and
//! type information, plus the `type_info` vtables and fundamental types `cxx_types` keeps.
//! A `new` that finds no memory throws `std::bad_alloc` from the layer. libc++'s other
//! standard exceptions (D710): `logic_error` and `runtime_error` with their message, kept in a
//! counted block the copies share, and the classes derived from them, `bad_exception`,
//! `bad_array_new_length`, and `nested_exception` holding the exception being handled.

use super::cxx_types::{
    self, PointerRecord, SingleBaseRecord, TYPE_BAD_ALLOC, TYPE_BAD_ARRAY_NEW_LENGTH,
    TYPE_BAD_CAST, TYPE_BAD_EXCEPTION, TYPE_BAD_TYPEID, TYPE_DOMAIN_ERROR, TYPE_EXCEPTION,
    TYPE_INVALID_ARGUMENT, TYPE_LENGTH_ERROR, TYPE_LOGIC_ERROR, TYPE_NESTED_EXCEPTION,
    TYPE_OUT_OF_RANGE, TYPE_OVERFLOW_ERROR, TYPE_RANGE_ERROR, TYPE_RUNTIME_ERROR,
    TYPE_UNDERFLOW_ERROR, TypeRecord, VTABLES,
};
use super::exception::{CXX_HEADER, cxa_allocate_exception};
use crate::libsystem::{self, heap, process};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const GUARD_DONE: u8 = 1;
const GUARD_BUSY: u8 = 2;
const MOST_FINALIZERS: usize = 256;
const ALIGNMENT: usize = 16;
const EXCEPTION_WHAT: &core::ffi::CStr = c"std::exception";
const BAD_ALLOC_WHAT: &core::ffi::CStr = c"std::bad_alloc";
const BAD_CAST_WHAT: &core::ffi::CStr = c"std::bad_cast";
const BAD_TYPEID_WHAT: &core::ffi::CStr = c"std::bad_typeid";
const BAD_EXCEPTION_WHAT: &core::ffi::CStr = c"std::bad_exception";
const BAD_ARRAY_NEW_LENGTH_WHAT: &core::ffi::CStr = c"bad_array_new_length";
const NESTED_EXCEPTION_WHAT: &core::ffi::CStr = c"std::nested_exception";
const WORD: usize = core::mem::size_of::<usize>();
const SHORT_STRING_FLAG: u8 = 1;
const LONG_STRING_SIZE_AT: usize = WORD;
const LONG_STRING_DATA_AT: usize = 2 * WORD;

/// A polymorphic class's vtable as the Itanium ABI lays it out: the offset to the top, the
/// type information, then the virtual functions.
#[repr(C)]
#[derive(Debug)]
pub struct ExceptionVtable {
    offset_to_top: isize,
    type_info: *const TypeRecord,
    complete_destructor: extern "C" fn(*mut u8),
    deleting_destructor: extern "C" fn(*mut u8),
    what: extern "C" fn(*const u8) -> *const u8,
}

// SAFETY: never written; its pointers are to permanent statics and functions.
unsafe impl Sync for ExceptionVtable {}

extern "C" fn exception_destructor(_: *mut u8) {}

extern "C" fn exception_deleting_destructor(this: *mut u8) {
    operator_delete(this);
}

extern "C" fn exception_what(_: *const u8) -> *const u8 {
    EXCEPTION_WHAT.as_ptr().cast()
}

extern "C" fn bad_alloc_what(_: *const u8) -> *const u8 {
    BAD_ALLOC_WHAT.as_ptr().cast()
}

/// `std::exception`'s vtable.
pub static EXCEPTION_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: &raw const TYPE_EXCEPTION,
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: exception_what,
};

/// `std::bad_alloc`'s vtable.
pub static BAD_ALLOC_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: (&raw const TYPE_BAD_ALLOC).cast(),
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: bad_alloc_what,
};

extern "C" fn bad_cast_what(_: *const u8) -> *const u8 {
    BAD_CAST_WHAT.as_ptr().cast()
}

extern "C" fn bad_typeid_what(_: *const u8) -> *const u8 {
    BAD_TYPEID_WHAT.as_ptr().cast()
}

/// `std::bad_cast`'s vtable.
pub static BAD_CAST_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: (&raw const TYPE_BAD_CAST).cast(),
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: bad_cast_what,
};

/// `std::bad_typeid`'s vtable.
pub static BAD_TYPEID_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: (&raw const TYPE_BAD_TYPEID).cast(),
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: bad_typeid_what,
};

extern "C" fn bad_exception_what(_: *const u8) -> *const u8 {
    BAD_EXCEPTION_WHAT.as_ptr().cast()
}

extern "C" fn bad_array_new_length_what(_: *const u8) -> *const u8 {
    BAD_ARRAY_NEW_LENGTH_WHAT.as_ptr().cast()
}

/// The message of a `logic_error` or `runtime_error` at `this`: its second word points at the
/// text, after the count of objects sharing it.
extern "C" fn message_what(this: *const u8) -> *const u8 {
    // SAFETY: the object's second word, which its constructor set to the text.
    unsafe { this.cast::<*const u8>().add(1).read() }
}

fn message_count(text: *const u8) -> *mut usize {
    text.cast_mut().cast::<usize>().wrapping_sub(1)
}

/// A shared copy of `bytes`, NUL-terminated, after a count of one: the text's address.
fn new_message(bytes: &[u8]) -> *const u8 {
    let block = heap::allocate(WORD + bytes.len() + 1, false);
    if block.is_null() {
        throw_bad_alloc();
    }
    // SAFETY: a new block of the count's word, the bytes and their NUL.
    unsafe {
        block.cast::<usize>().write_unaligned(1);
        let text = block.add(WORD);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), text, bytes.len());
        text.add(bytes.len()).write(0);
        text
    }
}

fn share_message(text: *const u8) {
    if !text.is_null() {
        // SAFETY: a text [`new_message`] made, its count before it.
        unsafe { *message_count(text) += 1 };
    }
}

fn release_message(text: *const u8) {
    if text.is_null() {
        return;
    }
    let count = message_count(text);
    // SAFETY: as above.
    unsafe {
        *count -= 1;
        if *count == 0 {
            heap::release(count.cast());
        }
    }
}

fn set_message(this: *mut usize, vtable: &'static ExceptionVtable, text: *const u8) {
    if this.is_null() {
        return;
    }
    // SAFETY: the caller's object of two words: its vtable pointer and its message.
    unsafe {
        this.write(address_point(vtable));
        this.add(1).write(text as usize);
    }
}

/// The bytes of a libc++ `std::string` at `string` (D710): a short string keeps its length
/// doubled in its first byte and its bytes after it; a long one has the low bit of its first
/// word set, its length in the second word and its bytes behind the third.
fn libcxx_string<'a>(string: *const u8) -> &'a [u8] {
    if string.is_null() {
        return &[];
    }
    // SAFETY: the program's `std::string`, laid out as libc++'s standard layout says.
    unsafe {
        let first = string.read();
        if first & SHORT_STRING_FLAG == 0 {
            core::slice::from_raw_parts(string.add(1), usize::from(first >> 1))
        } else {
            let length = string.add(LONG_STRING_SIZE_AT).cast::<usize>().read_unaligned();
            let data = string.add(LONG_STRING_DATA_AT).cast::<*const u8>().read_unaligned();
            core::slice::from_raw_parts(data, length)
        }
    }
}

macro_rules! message_exceptions {
    ($($vtable:ident = $record:ident;)*) => {
        $(
            pub static $vtable: ExceptionVtable = ExceptionVtable {
                offset_to_top: 0,
                type_info: (&raw const $record).cast(),
                complete_destructor: message_destructor,
                deleting_destructor: message_deleting_destructor,
                what: message_what,
            };
        )*
    };
}

message_exceptions! {
    LOGIC_ERROR_VTABLE = TYPE_LOGIC_ERROR;
    RUNTIME_ERROR_VTABLE = TYPE_RUNTIME_ERROR;
    DOMAIN_ERROR_VTABLE = TYPE_DOMAIN_ERROR;
    INVALID_ARGUMENT_VTABLE = TYPE_INVALID_ARGUMENT;
    LENGTH_ERROR_VTABLE = TYPE_LENGTH_ERROR;
    OUT_OF_RANGE_VTABLE = TYPE_OUT_OF_RANGE;
    RANGE_ERROR_VTABLE = TYPE_RANGE_ERROR;
    OVERFLOW_ERROR_VTABLE = TYPE_OVERFLOW_ERROR;
    UNDERFLOW_ERROR_VTABLE = TYPE_UNDERFLOW_ERROR;
}

extern "C" fn message_destructor(this: *mut u8) {
    if !this.is_null() {
        release_message(message_what(this));
    }
}

extern "C" fn message_deleting_destructor(this: *mut u8) {
    message_destructor(this);
    operator_delete(this);
}

extern "C" fn logic_error_from_text(this: *mut usize, text: *const u8) {
    // SAFETY: the program's NUL-terminated message.
    let bytes = unsafe { libsystem::c_string(text.cast()) };
    set_message(this, &LOGIC_ERROR_VTABLE, new_message(bytes));
}

extern "C" fn logic_error_from_string(this: *mut usize, string: *const u8) {
    set_message(this, &LOGIC_ERROR_VTABLE, new_message(libcxx_string(string)));
}

extern "C" fn runtime_error_from_text(this: *mut usize, text: *const u8) {
    // SAFETY: as above.
    let bytes = unsafe { libsystem::c_string(text.cast()) };
    set_message(this, &RUNTIME_ERROR_VTABLE, new_message(bytes));
}

extern "C" fn runtime_error_from_string(this: *mut usize, string: *const u8) {
    set_message(this, &RUNTIME_ERROR_VTABLE, new_message(libcxx_string(string)));
}

fn copy_message(this: *mut usize, vtable: &'static ExceptionVtable, other: *const u8) {
    let text = message_what(other);
    share_message(text);
    set_message(this, vtable, text);
}

extern "C" fn logic_error_copy(this: *mut usize, other: *const u8) {
    copy_message(this, &LOGIC_ERROR_VTABLE, other);
}

extern "C" fn runtime_error_copy(this: *mut usize, other: *const u8) {
    copy_message(this, &RUNTIME_ERROR_VTABLE, other);
}

/// `operator=` of either: the message is shared, the object's own vtable kept.
extern "C" fn message_assign(this: *mut usize, other: *const u8) -> *mut usize {
    if this.is_null() || core::ptr::eq(this.cast::<u8>(), other) {
        return this;
    }
    let text = message_what(other);
    share_message(text);
    release_message(message_what(this.cast()));
    // SAFETY: the object's second word.
    unsafe { this.add(1).write(text as usize) };
    this
}

/// `std::bad_exception`'s vtable.
pub static BAD_EXCEPTION_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: (&raw const TYPE_BAD_EXCEPTION).cast(),
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: bad_exception_what,
};

/// `std::bad_array_new_length`'s vtable.
pub static BAD_ARRAY_NEW_LENGTH_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: (&raw const TYPE_BAD_ARRAY_NEW_LENGTH).cast(),
    complete_destructor: exception_destructor,
    deleting_destructor: exception_deleting_destructor,
    what: bad_array_new_length_what,
};

extern "C" fn bad_exception_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: the caller's object, its first word the vtable pointer.
        unsafe { this.write(address_point(&BAD_EXCEPTION_VTABLE)) };
    }
}

extern "C" fn bad_array_new_length_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: as above.
        unsafe { this.write(address_point(&BAD_ARRAY_NEW_LENGTH_VTABLE)) };
    }
}

extern "C" fn throw_bad_array_new_length() -> ! {
    throw_standard(&BAD_ARRAY_NEW_LENGTH_VTABLE, &raw const TYPE_BAD_ARRAY_NEW_LENGTH)
}

/// `std::nested_exception`'s vtable: its destructors and a `what` the class does not have,
/// as no slot after the destructors is called through it.
pub static NESTED_EXCEPTION_VTABLE: ExceptionVtable = ExceptionVtable {
    offset_to_top: 0,
    type_info: &raw const TYPE_NESTED_EXCEPTION,
    complete_destructor: nested_exception_destructor,
    deleting_destructor: nested_exception_deleting_destructor,
    what: nested_exception_what,
};

extern "C" fn nested_exception_what(_: *const u8) -> *const u8 {
    NESTED_EXCEPTION_WHAT.as_ptr().cast()
}

/// `nested_exception()`: holds the exception being handled, if any, in its second word.
extern "C" fn nested_exception_construct(this: *mut usize) {
    if this.is_null() {
        return;
    }
    let held = super::exception::cxa_current_primary_exception();
    // SAFETY: the caller's object of two words: its vtable pointer and its `exception_ptr`.
    unsafe {
        this.write(address_point(&NESTED_EXCEPTION_VTABLE));
        this.add(1).write(held as usize);
    }
}

extern "C" fn nested_exception_destructor(this: *mut u8) {
    if this.is_null() {
        return;
    }
    // SAFETY: as above.
    let held = unsafe { this.cast::<*mut u8>().add(1).read() };
    if !held.is_null() {
        super::exception::cxa_decrement_exception_refcount(held);
    }
}

extern "C" fn nested_exception_deleting_destructor(this: *mut u8) {
    nested_exception_destructor(this);
    operator_delete(this);
}

fn address_point(vtable: &'static ExceptionVtable) -> usize {
    (vtable as *const ExceptionVtable as usize) + 2 * core::mem::size_of::<usize>()
}

extern "C" fn bad_alloc_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: the caller's `std::bad_alloc` object, its first word the vtable pointer.
        unsafe { this.write(address_point(&BAD_ALLOC_VTABLE)) };
    }
}

extern "C" fn exception_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: the caller's `std::exception` object, its first word the vtable pointer.
        unsafe { this.write(address_point(&EXCEPTION_VTABLE)) };
    }
}

unsafe extern "C" {
    fn eggos_cxa_throw();
}

fn throw_standard(vtable: &'static ExceptionVtable, type_info: *const SingleBaseRecord) -> ! {
    let object = cxa_allocate_exception(core::mem::size_of::<usize>()).cast::<usize>();
    // SAFETY: a new exception object one word long, its first word the vtable pointer.
    unsafe { object.write(address_point(vtable)) };
    let throw: extern "C" fn(*mut usize, *const SingleBaseRecord, usize) -> ! =
        // SAFETY: `eggos_cxa_throw` is `__cxa_throw`'s entry; it takes object, type, destructor.
        unsafe { core::mem::transmute(eggos_cxa_throw as *const ()) };
    throw(object, type_info, exception_destructor as usize)
}

fn throw_bad_alloc() -> ! {
    throw_standard(&BAD_ALLOC_VTABLE, &raw const TYPE_BAD_ALLOC)
}

extern "C" fn throw_bad_cast() -> ! {
    throw_standard(&BAD_CAST_VTABLE, &raw const TYPE_BAD_CAST)
}

extern "C" fn throw_bad_typeid() -> ! {
    throw_standard(&BAD_TYPEID_VTABLE, &raw const TYPE_BAD_TYPEID)
}

extern "C" fn bad_cast_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: the caller's `std::bad_cast` object, its first word the vtable pointer.
        unsafe { this.write(address_point(&BAD_CAST_VTABLE)) };
    }
}

extern "C" fn bad_typeid_construct(this: *mut usize) {
    if !this.is_null() {
        // SAFETY: the caller's `std::bad_typeid` object, its first word the vtable pointer.
        unsafe { this.write(address_point(&BAD_TYPEID_VTABLE)) };
    }
}

extern "C" fn operator_new(size: usize) -> *mut u8 {
    let block = heap::allocate_aligned(ALIGNMENT, size.max(1));
    if block.is_null() {
        throw_bad_alloc();
    }
    block
}

extern "C" fn operator_new_nothrow(size: usize, _: *const u8) -> *mut u8 {
    heap::allocate_aligned(ALIGNMENT, size.max(1))
}

extern "C" fn operator_new_aligned(size: usize, alignment: usize) -> *mut u8 {
    let block = heap::allocate_aligned(alignment.max(ALIGNMENT), size.max(1));
    if block.is_null() {
        throw_bad_alloc();
    }
    block
}

extern "C" fn operator_delete(block: *mut u8) {
    if !block.is_null() {
        heap::release(block);
    }
}

extern "C" fn operator_delete_sized(block: *mut u8, _: usize) {
    operator_delete(block);
}

fn guard_byte(guard: *mut u64) -> &'static AtomicU8 {
    // SAFETY: the program's 64-bit guard word of a static local, which lives as long as the
    // program; its first byte says whether the local is made.
    unsafe { AtomicU8::from_ptr(guard.cast::<u8>()) }
}

extern "C" fn guard_acquire(guard: *mut u64) -> i32 {
    let state = guard_byte(guard);
    loop {
        match state.compare_exchange(0, GUARD_BUSY, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) => return 1,
            Err(GUARD_DONE) => return 0,
            Err(_) => crate::linux::yield_now(),
        }
    }
}

extern "C" fn guard_release(guard: *mut u64) {
    guard_byte(guard).store(GUARD_DONE, Ordering::Release);
}

extern "C" fn guard_abort(guard: *mut u64) {
    guard_byte(guard).store(0, Ordering::Release);
}

static FINALIZERS: libsystem::Exclusive<([(usize, usize); MOST_FINALIZERS], usize)> =
    libsystem::Exclusive::new(([(0, 0); MOST_FINALIZERS], 0));
static FINALIZERS_REGISTERED: AtomicBool = AtomicBool::new(false);

extern "C" fn run_finalizers() {
    loop {
        let next = FINALIZERS.with(|(entries, count)| {
            (*count > 0).then(|| {
                *count -= 1;
                entries[*count]
            })
        });
        let Some((function, argument)) = next else {
            break;
        };
        let function = crate::callback::native_callable(function);
        // SAFETY: `__cxa_atexit` stored a function that takes one pointer.
        let function: extern "C" fn(usize) = unsafe { core::mem::transmute(function) };
        function(argument);
    }
}

extern "C" fn cxa_atexit(function: usize, argument: usize, _: usize) -> i32 {
    if !FINALIZERS_REGISTERED.swap(true, Ordering::AcqRel) && process::atexit(run_finalizers) != 0 {
        return -1;
    }
    FINALIZERS.with(|(entries, count)| {
        if *count == MOST_FINALIZERS {
            return -1;
        }
        entries[*count] = (function, argument);
        *count += 1;
        0
    })
}

fn write_type_name(type_info: u64) {
    let mut buffer = [0u8; 128];
    let mut length = 0;
    while length < buffer.len() {
        match cxx_types::name_byte_of(type_info, length as u64) {
            Some(0) | None => break,
            Some(byte) => buffer[length] = byte,
        }
        length += 1;
    }
    libsystem::write_error(&buffer[..length]);
}

/// Ends the program for a C++ exception of type `type_info` nothing caught, as libc++abi
/// reports it.
pub fn terminate_uncaught(type_info: u64) -> ! {
    libsystem::stdio::flush_all();
    libsystem::write_error(b"libc++abi: terminating due to uncaught exception of type ");
    write_type_name(type_info);
    libsystem::write_error(b"\n");
    libsystem::abort()
}

extern "C" fn terminate() -> ! {
    libsystem::stdio::flush_all();
    libsystem::write_error(b"libc++abi: terminating\n");
    libsystem::abort()
}

extern "C" fn pure_virtual() -> ! {
    libsystem::write_error(b"libc++abi: Pure virtual function called!\n");
    libsystem::abort()
}

extern "C" fn deleted_virtual() -> ! {
    libsystem::write_error(b"libc++abi: Deleted virtual function called!\n");
    libsystem::abort()
}

fn record(record: &'static TypeRecord) -> usize {
    record as *const TypeRecord as usize
}

fn pointer_record(record: &'static PointerRecord) -> usize {
    record as *const PointerRecord as usize
}

extern "C" fn dynamic_cast(sub: u64, _source: u64, wanted: u64, _hint: i64) -> u64 {
    super::cxx_types::dynamic_cast(sub, wanted)
}

/// The address of the libc++abi function or variable `name`, if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"__Znwm" | b"__Znam" => operator_new as *const () as usize,
        b"__ZnwmRKSt9nothrow_t" | b"__ZnamRKSt9nothrow_t" => {
            operator_new_nothrow as *const () as usize
        }
        b"__ZnwmSt11align_val_t" | b"__ZnamSt11align_val_t" => {
            operator_new_aligned as *const () as usize
        }
        b"__ZdlPv" | b"__ZdaPv" | b"__ZdlPvSt11align_val_t" | b"__ZdaPvSt11align_val_t" => {
            operator_delete as *const () as usize
        }
        b"__ZdlPvm" | b"__ZdaPvm" => operator_delete_sized as *const () as usize,
        b"___cxa_guard_acquire" => guard_acquire as *const () as usize,
        b"___cxa_guard_release" => guard_release as *const () as usize,
        b"___cxa_guard_abort" => guard_abort as *const () as usize,
        b"___cxa_atexit" => cxa_atexit as *const () as usize,
        b"___cxa_pure_virtual" => pure_virtual as *const () as usize,
        b"___cxa_deleted_virtual" => deleted_virtual as *const () as usize,
        b"__ZSt9terminatev" | b"___cxa_call_unexpected" => terminate as *const () as usize,
        b"__ZNSt9exceptionD1Ev"
        | b"__ZNSt9exceptionD2Ev"
        | b"__ZNSt9bad_allocD1Ev"
        | b"__ZNSt9bad_allocD2Ev" => exception_destructor as *const () as usize,
        b"__ZNSt9exceptionD0Ev" | b"__ZNSt9bad_allocD0Ev" => {
            exception_deleting_destructor as *const () as usize
        }
        b"___dynamic_cast" => dynamic_cast as *const () as usize,
        b"___cxa_bad_cast" => throw_bad_cast as *const () as usize,
        b"___cxa_bad_typeid" => throw_bad_typeid as *const () as usize,
        b"__ZTVSt8bad_cast" => &raw const BAD_CAST_VTABLE as usize,
        b"__ZTVSt10bad_typeid" => &raw const BAD_TYPEID_VTABLE as usize,
        b"__ZTISt8bad_cast" => &raw const TYPE_BAD_CAST as usize,
        b"__ZTISt10bad_typeid" => &raw const TYPE_BAD_TYPEID as usize,
        b"__ZNKSt8bad_cast4whatEv" => bad_cast_what as *const () as usize,
        b"__ZNKSt10bad_typeid4whatEv" => bad_typeid_what as *const () as usize,
        b"__ZNSt8bad_castC1Ev" | b"__ZNSt8bad_castC2Ev" => bad_cast_construct as *const () as usize,
        b"__ZNSt10bad_typeidC1Ev" | b"__ZNSt10bad_typeidC2Ev" => {
            bad_typeid_construct as *const () as usize
        }
        b"__ZNSt8bad_castD1Ev"
        | b"__ZNSt8bad_castD2Ev"
        | b"__ZNSt10bad_typeidD1Ev"
        | b"__ZNSt10bad_typeidD2Ev" => exception_destructor as *const () as usize,
        b"__ZNSt8bad_castD0Ev" | b"__ZNSt10bad_typeidD0Ev" => {
            exception_deleting_destructor as *const () as usize
        }
        b"__ZNSt11logic_errorC1EPKc" | b"__ZNSt11logic_errorC2EPKc" => {
            logic_error_from_text as *const () as usize
        }
        b"__ZNSt11logic_errorC1ERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEE"
        | b"__ZNSt11logic_errorC2ERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEE" => {
            logic_error_from_string as *const () as usize
        }
        b"__ZNSt13runtime_errorC1EPKc" | b"__ZNSt13runtime_errorC2EPKc" => {
            runtime_error_from_text as *const () as usize
        }
        b"__ZNSt13runtime_errorC1ERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEE"
        | b"__ZNSt13runtime_errorC2ERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEE" => {
            runtime_error_from_string as *const () as usize
        }
        b"__ZNSt11logic_errorC1ERKS_" | b"__ZNSt11logic_errorC2ERKS_" => {
            logic_error_copy as *const () as usize
        }
        b"__ZNSt13runtime_errorC1ERKS_" | b"__ZNSt13runtime_errorC2ERKS_" => {
            runtime_error_copy as *const () as usize
        }
        b"__ZNSt11logic_erroraSERKS_" | b"__ZNSt13runtime_erroraSERKS_" => {
            message_assign as *const () as usize
        }
        b"__ZNKSt11logic_error4whatEv" | b"__ZNKSt13runtime_error4whatEv" => {
            message_what as *const () as usize
        }
        name if message_destructor_names(name) => message_destructor as *const () as usize,
        name if message_deleting_destructor_names(name) => {
            message_deleting_destructor as *const () as usize
        }
        b"__ZTVSt11logic_error" => &raw const LOGIC_ERROR_VTABLE as usize,
        b"__ZTVSt13runtime_error" => &raw const RUNTIME_ERROR_VTABLE as usize,
        b"__ZTVSt12domain_error" => &raw const DOMAIN_ERROR_VTABLE as usize,
        b"__ZTVSt16invalid_argument" => &raw const INVALID_ARGUMENT_VTABLE as usize,
        b"__ZTVSt12length_error" => &raw const LENGTH_ERROR_VTABLE as usize,
        b"__ZTVSt12out_of_range" => &raw const OUT_OF_RANGE_VTABLE as usize,
        b"__ZTVSt11range_error" => &raw const RANGE_ERROR_VTABLE as usize,
        b"__ZTVSt14overflow_error" => &raw const OVERFLOW_ERROR_VTABLE as usize,
        b"__ZTVSt15underflow_error" => &raw const UNDERFLOW_ERROR_VTABLE as usize,
        b"__ZTVSt13bad_exception" => &raw const BAD_EXCEPTION_VTABLE as usize,
        b"__ZTVSt20bad_array_new_length" => &raw const BAD_ARRAY_NEW_LENGTH_VTABLE as usize,
        b"__ZTVSt16nested_exception" => &raw const NESTED_EXCEPTION_VTABLE as usize,
        b"__ZTISt11logic_error" => &raw const TYPE_LOGIC_ERROR as usize,
        b"__ZTISt13runtime_error" => &raw const TYPE_RUNTIME_ERROR as usize,
        b"__ZTISt12domain_error" => &raw const TYPE_DOMAIN_ERROR as usize,
        b"__ZTISt16invalid_argument" => &raw const TYPE_INVALID_ARGUMENT as usize,
        b"__ZTISt12length_error" => &raw const TYPE_LENGTH_ERROR as usize,
        b"__ZTISt12out_of_range" => &raw const TYPE_OUT_OF_RANGE as usize,
        b"__ZTISt11range_error" => &raw const TYPE_RANGE_ERROR as usize,
        b"__ZTISt14overflow_error" => &raw const TYPE_OVERFLOW_ERROR as usize,
        b"__ZTISt15underflow_error" => &raw const TYPE_UNDERFLOW_ERROR as usize,
        b"__ZTISt13bad_exception" => &raw const TYPE_BAD_EXCEPTION as usize,
        b"__ZTISt20bad_array_new_length" => &raw const TYPE_BAD_ARRAY_NEW_LENGTH as usize,
        b"__ZTISt16nested_exception" => record(&TYPE_NESTED_EXCEPTION),
        b"__ZNSt13bad_exceptionC1Ev" | b"__ZNSt13bad_exceptionC2Ev" => {
            bad_exception_construct as *const () as usize
        }
        b"__ZNSt20bad_array_new_lengthC1Ev" | b"__ZNSt20bad_array_new_lengthC2Ev" => {
            bad_array_new_length_construct as *const () as usize
        }
        b"___cxa_throw_bad_array_new_length" => throw_bad_array_new_length as *const () as usize,
        b"__ZNKSt13bad_exception4whatEv" => bad_exception_what as *const () as usize,
        b"__ZNKSt20bad_array_new_length4whatEv" => bad_array_new_length_what as *const () as usize,
        b"__ZNSt13bad_exceptionD1Ev"
        | b"__ZNSt13bad_exceptionD2Ev"
        | b"__ZNSt20bad_array_new_lengthD1Ev"
        | b"__ZNSt20bad_array_new_lengthD2Ev" => exception_destructor as *const () as usize,
        b"__ZNSt13bad_exceptionD0Ev" | b"__ZNSt20bad_array_new_lengthD0Ev" => {
            exception_deleting_destructor as *const () as usize
        }
        b"__ZNSt16nested_exceptionC1Ev" | b"__ZNSt16nested_exceptionC2Ev" => {
            nested_exception_construct as *const () as usize
        }
        b"__ZNSt16nested_exceptionD1Ev" | b"__ZNSt16nested_exceptionD2Ev" => {
            nested_exception_destructor as *const () as usize
        }
        b"__ZNSt16nested_exceptionD0Ev" => nested_exception_deleting_destructor as *const () as usize,
        b"__ZNKSt16nested_exception14rethrow_nestedEv" => {
            super::exception::rethrow_nested_entry()
        }
        b"__ZNKSt9exception4whatEv" => exception_what as *const () as usize,
        b"__ZNKSt9bad_alloc4whatEv" => bad_alloc_what as *const () as usize,
        b"__ZNSt9bad_allocC1Ev" | b"__ZNSt9bad_allocC2Ev" => {
            bad_alloc_construct as *const () as usize
        }
        b"__ZNSt9exceptionC1Ev" | b"__ZNSt9exceptionC2Ev" => {
            exception_construct as *const () as usize
        }
        b"__ZTVSt9exception" => &raw const EXCEPTION_VTABLE as usize,
        b"__ZTVSt9bad_alloc" => &raw const BAD_ALLOC_VTABLE as usize,
        b"__ZTISt9exception" => record(&TYPE_EXCEPTION),
        b"__ZTISt9bad_alloc" => &raw const TYPE_BAD_ALLOC as usize,
        b"__ZTVN10__cxxabiv117__class_type_infoE" => &raw const VTABLES.class as usize,
        b"__ZTVN10__cxxabiv120__si_class_type_infoE" => &raw const VTABLES.single_base as usize,
        b"__ZTVN10__cxxabiv121__vmi_class_type_infoE" => &raw const VTABLES.multiple_bases as usize,
        b"__ZTVN10__cxxabiv119__pointer_type_infoE" => &raw const VTABLES.pointer as usize,
        b"__ZTVN10__cxxabiv123__fundamental_type_infoE" => &raw const VTABLES.fundamental as usize,
        b"__ZTIi" => record(&cxx_types::TYPE_INT),
        b"__ZTIj" => record(&cxx_types::TYPE_UNSIGNED_INT),
        b"__ZTIl" => record(&cxx_types::TYPE_LONG),
        b"__ZTIm" => record(&cxx_types::TYPE_UNSIGNED_LONG),
        b"__ZTIx" => record(&cxx_types::TYPE_LONG_LONG),
        b"__ZTIy" => record(&cxx_types::TYPE_UNSIGNED_LONG_LONG),
        b"__ZTIc" => record(&cxx_types::TYPE_CHAR),
        b"__ZTIa" => record(&cxx_types::TYPE_SIGNED_CHAR),
        b"__ZTIh" => record(&cxx_types::TYPE_UNSIGNED_CHAR),
        b"__ZTIs" => record(&cxx_types::TYPE_SHORT),
        b"__ZTIt" => record(&cxx_types::TYPE_UNSIGNED_SHORT),
        b"__ZTIb" => record(&cxx_types::TYPE_BOOL),
        b"__ZTIf" => record(&cxx_types::TYPE_FLOAT),
        b"__ZTId" => record(&cxx_types::TYPE_DOUBLE),
        b"__ZTIv" => record(&cxx_types::TYPE_VOID),
        b"__ZTIPKc" => pointer_record(&cxx_types::TYPE_CONST_CHAR_POINTER),
        b"__ZTIPc" => pointer_record(&cxx_types::TYPE_CHAR_POINTER),
        b"__ZTIPv" => pointer_record(&cxx_types::TYPE_VOID_POINTER),
        b"__ZTIPKv" => pointer_record(&cxx_types::TYPE_CONST_VOID_POINTER),
        _ => return None,
    };
    Some(address as u64)
}

const MESSAGE_CLASSES: [&[u8]; 9] = [
    b"11logic_error",
    b"13runtime_error",
    b"12domain_error",
    b"16invalid_argument",
    b"12length_error",
    b"12out_of_range",
    b"11range_error",
    b"14overflow_error",
    b"15underflow_error",
];

fn message_class_destructor(name: &[u8], kinds: &[&[u8]]) -> bool {
    let Some(rest) = name.strip_prefix(b"__ZNSt") else {
        return false;
    };
    MESSAGE_CLASSES.iter().any(|class| {
        rest.strip_prefix(*class)
            .is_some_and(|kind| kinds.contains(&kind))
    })
}

fn message_destructor_names(name: &[u8]) -> bool {
    message_class_destructor(name, &[b"D1Ev", b"D2Ev"])
}

fn message_deleting_destructor_names(name: &[u8]) -> bool {
    message_class_destructor(name, &[b"D0Ev"])
}

const _: () = assert!(CXX_HEADER % ALIGNMENT == 0);
