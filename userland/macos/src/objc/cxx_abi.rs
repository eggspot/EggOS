//! The parts of libc++abi a C++ program on macOS links against beside its exceptions (D380):
//! `operator new` and `delete`, static locals' guards, `__cxa_atexit`, pure and deleted virtual
//! calls, `std::terminate`, and `std::exception` and `std::bad_alloc` with their vtables and
//! type information, plus the `type_info` vtables and fundamental types `cxx_types` keeps.
//! A `new` that finds no memory throws `std::bad_alloc` from the layer.

use super::cxx_types::{
    self, PointerRecord, SingleBaseRecord, TYPE_BAD_ALLOC, TYPE_EXCEPTION, TypeRecord, VTABLES,
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

fn throw_bad_alloc() -> ! {
    let object = cxa_allocate_exception(core::mem::size_of::<usize>()).cast::<usize>();
    bad_alloc_construct(object);
    let throw: extern "C" fn(*mut usize, *const SingleBaseRecord, usize) -> ! =
        // SAFETY: `eggos_cxa_throw` is `__cxa_throw`'s entry, which takes the object, its type
        // and its destructor and never returns.
        unsafe { core::mem::transmute(eggos_cxa_throw as *const ()) };
    throw(object, &raw const TYPE_BAD_ALLOC, exception_destructor as usize)
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
    if !FINALIZERS_REGISTERED.swap(true, Ordering::AcqRel)
        && process::atexit(run_finalizers) != 0
    {
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
        b"__ZNSt9exceptionD1Ev" | b"__ZNSt9exceptionD2Ev" | b"__ZNSt9bad_allocD1Ev"
        | b"__ZNSt9bad_allocD2Ev" => exception_destructor as *const () as usize,
        b"__ZNSt9exceptionD0Ev" | b"__ZNSt9bad_allocD0Ev" => {
            exception_deleting_destructor as *const () as usize
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
        b"__ZTVN10__cxxabiv121__vmi_class_type_infoE" => {
            &raw const VTABLES.multiple_bases as usize
        }
        b"__ZTVN10__cxxabiv119__pointer_type_infoE" => &raw const VTABLES.pointer as usize,
        b"__ZTVN10__cxxabiv123__fundamental_type_infoE" => {
            &raw const VTABLES.fundamental as usize
        }
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

const _: () = assert!(CXX_HEADER % ALIGNMENT == 0);
