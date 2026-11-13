//! msvcp140's throwers (D230): the `_X*` functions the standard library's headers call when a
//! container grows too long, an index is out of range or a `std::function` is empty, and
//! `_Throw_C_error` and `_Throw_Cpp_error`, which `std::thread` and `std::mutex` call with a
//! failed call's result. Each throws the standard exception from the program's call. A
//! `std::system_error` carries an `std::error_code` in the generic category, whose object here
//! answers `name()`, `message()` (an `std::string` in MSVC's release layout, short strings in
//! place and longer ones on the C runtime's heap, which the program's `std::string` frees) and
//! the comparisons `std::errc` takes part in; it compares equal to the program's own
//! `std::generic_category()`, as categories compare by their address word.

use super::super::exception::{Context, StdObject, capturing_entry, throw, throw_object};
use super::super::heap;
use crate::cpp_errors::{
    CPP_ERROR_WHAT, CPP_ERRORS, INVALID_ARGUMENT, cpp_error_of, errc_of_windows_error,
    generic_message, generic_message_terminated,
};
use crate::library::Export;
use crate::std_exceptions::StdException;

const GENERIC_CATEGORY_ADDRESS: u64 = 3;
const SHORT_STRING_CAPACITY: usize = 15;

#[repr(C)]
struct Category {
    table: *const (),
    address: u64,
}

// SAFETY: the category is never written; its table is immutable.
unsafe impl Sync for Category {}

#[repr(C)]
struct CategoryTable([*const (); 7]);

// SAFETY: the table holds the layer's functions' addresses, immutable for the program's life.
unsafe impl Sync for CategoryTable {}

static CATEGORY_TABLE: CategoryTable = CategoryTable([
    core::ptr::null(),
    export_entry!(category_delete),
    export_entry!(category_name),
    export_entry!(category_message),
    export_entry!(category_default_condition),
    export_entry!(category_equivalent_code),
    export_entry!(category_equivalent_condition),
]);

static GENERIC_CATEGORY: Category = Category {
    table: (&raw const CATEGORY_TABLE.0[1]).cast(),
    address: GENERIC_CATEGORY_ADDRESS,
};

#[repr(C)]
struct ErrorCode {
    value: i32,
    category: *const Category,
}

#[repr(C)]
struct StdString {
    text: [u8; 16],
    size: usize,
    capacity: usize,
}

extern "efiapi" fn category_delete(category: u64, _flags: u32) -> u64 {
    category
}

extern "efiapi" fn category_name(_category: u64) -> *const u8 {
    c"generic".as_ptr().cast()
}

unsafe fn write_string(target: *mut StdString, text: &[u8]) {
    let mut string = StdString {
        text: [0; 16],
        size: text.len(),
        capacity: SHORT_STRING_CAPACITY,
    };
    if text.len() <= SHORT_STRING_CAPACITY {
        string.text[..text.len()].copy_from_slice(text);
    } else {
        let capacity = text.len() | SHORT_STRING_CAPACITY;
        let buffer = heap::allocate(capacity + 1);
        if !buffer.is_null() {
            // SAFETY: `buffer` holds `capacity + 1` bytes, more than the text and its NUL.
            unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len()) };
            string.text[..8].copy_from_slice(&(buffer as u64).to_le_bytes());
            string.capacity = capacity;
        } else {
            string.size = 0;
        }
    }
    // SAFETY: the caller's duty: `target` is the program's uninitialized `std::string`.
    unsafe { target.write(string) };
}

unsafe extern "efiapi" fn category_message(
    _category: u64,
    target: *mut StdString,
    value: i32,
) -> *mut StdString {
    // SAFETY: `message`'s hidden result: the program's storage for the returned string.
    unsafe { write_string(target, generic_message(value)) };
    target
}

unsafe extern "efiapi" fn category_default_condition(
    category: *const Category,
    target: *mut ErrorCode,
    value: i32,
) -> *mut ErrorCode {
    // SAFETY: `default_error_condition`'s hidden result: the program's `error_condition`, laid
    // out as an `error_code`.
    unsafe { target.write(ErrorCode { value, category }) };
    target
}

fn same_category(left: *const Category, right: *const Category) -> bool {
    if left == right {
        return true;
    }
    // SAFETY: both are `error_category` objects, whose second word is their address word.
    let left_address = unsafe { (*left).address };
    // SAFETY: as above.
    let right_address = unsafe { (*right).address };
    left_address == right_address
}

unsafe extern "efiapi" fn category_equivalent_code(
    category: *const Category,
    code: *const ErrorCode,
    value: i32,
) -> bool {
    // SAFETY: `equivalent(const error_code&, int)`'s contract.
    let code = unsafe { &*code };
    same_category(category, code.category) && code.value == value
}

unsafe extern "efiapi" fn category_equivalent_condition(
    category: *const Category,
    value: i32,
    condition: *const ErrorCode,
) -> bool {
    // SAFETY: `equivalent(int, const error_condition&)`'s contract.
    let condition = unsafe { &*condition };
    same_category(category, condition.category) && condition.value == value
}

/// Throws the `std::system_error` `_Throw_Cpp_error(code)` throws.
pub fn throw_cpp_error(code: i32, context: *mut Context) -> ! {
    let index = usize::try_from(code)
        .ok()
        .filter(|index| *index < CPP_ERRORS.len())
        .unwrap_or(INVALID_ARGUMENT as usize);
    let mut object = StdObject::new(StdException::SystemError, CPP_ERROR_WHAT[index].as_ptr());
    object.code = CPP_ERRORS[index].0;
    object.category = (&raw const GENERIC_CATEGORY).cast();
    throw_object(StdException::SystemError, object, context)
}

extern "efiapi" fn syserror_map(value: i32) -> *const u8 {
    generic_message_terminated(value).as_ptr()
}

capturing_entry!(length_error_entry => length_error);
capturing_entry!(out_of_range_entry => out_of_range);
capturing_entry!(invalid_argument_entry => invalid_argument);
capturing_entry!(runtime_error_entry => runtime_error);
capturing_entry!(overflow_error_entry => overflow_error);
capturing_entry!(bad_alloc_entry => bad_alloc);
capturing_entry!(bad_function_call_entry => bad_function_call);
capturing_entry!(throw_c_error_entry => throw_c_error);
capturing_entry!(throw_cpp_error_entry => throw_cpp_error_call);

extern "C" fn length_error(message: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::LengthError, message as *const u8, context)
}

extern "C" fn out_of_range(message: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::OutOfRange, message as *const u8, context)
}

extern "C" fn invalid_argument(message: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::InvalidArgument, message as *const u8, context)
}

extern "C" fn runtime_error(message: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::RuntimeError, message as *const u8, context)
}

extern "C" fn overflow_error(message: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::OverflowError, message as *const u8, context)
}

extern "C" fn bad_alloc(_: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::BadAlloc, core::ptr::null(), context)
}

extern "C" fn bad_function_call(_: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw(StdException::BadFunctionCall, core::ptr::null(), context)
}

extern "C" fn throw_c_error(result: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw_cpp_error(cpp_error_of(result as i32), context)
}

extern "C" fn throw_cpp_error_call(code: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    throw_cpp_error(code as i32, context)
}

extern "efiapi" fn winerror_map(code: i32) -> i32 {
    errc_of_windows_error(code as u32)
}

unsafe extern "efiapi" fn winerror_message(code: u32, narrow: *mut u8, size: u32) -> u32 {
    let Some(message) = crate::errors::message(code) else {
        return 0;
    };
    let bytes = message.as_bytes();
    if narrow.is_null() || bytes.len() >= size as usize {
        return 0;
    }
    // SAFETY: the caller's `size` bytes, more than the message and its NUL need.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), narrow, bytes.len());
        narrow.add(bytes.len()).write(0);
    }
    bytes.len() as u32
}

unsafe extern "efiapi" fn system_error_allocate_message(code: u32, out: *mut *mut u8) -> usize {
    let message = crate::errors::message(code).map_or(&[][..], str::as_bytes);
    let block = match message.is_empty() {
        true => core::ptr::null_mut(),
        false => heap::allocate(message.len() + 1),
    };
    if !block.is_null() {
        // SAFETY: a new block of the message's length and its NUL.
        unsafe {
            core::ptr::copy_nonoverlapping(message.as_ptr(), block, message.len());
            block.add(message.len()).write(0);
        }
    }
    // SAFETY: the caller's word.
    unsafe { out.write_unaligned(block) };
    match block.is_null() {
        true => 0,
        false => message.len(),
    }
}

unsafe extern "efiapi" fn system_error_deallocate_message(message: *mut u8) {
    if !message.is_null() {
        // SAFETY: the caller's contract.
        unsafe { heap::free(message) };
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"?_Xlength_error@std@@YAXPEBD@Z" => length_error_entry [direct],
    b"?_Xout_of_range@std@@YAXPEBD@Z" => out_of_range_entry [direct],
    b"?_Xinvalid_argument@std@@YAXPEBD@Z" => invalid_argument_entry [direct],
    b"?_Xruntime_error@std@@YAXPEBD@Z" => runtime_error_entry [direct],
    b"?_Xoverflow_error@std@@YAXPEBD@Z" => overflow_error_entry [direct],
    b"?_Xbad_alloc@std@@YAXXZ" => bad_alloc_entry [direct],
    b"?_Xbad_function_call@std@@YAXXZ" => bad_function_call_entry [direct],
    b"?_Throw_C_error@std@@YAXH@Z" => throw_c_error_entry [direct],
    b"?_Throw_Cpp_error@std@@YAXH@Z" => throw_cpp_error_entry [direct],
    b"?_Syserror_map@std@@YAPEBDH@Z" => syserror_map,
    b"?_Winerror_map@std@@YAHH@Z" => winerror_map,
    b"?_Winerror_message@std@@YAKKPEADK@Z" => winerror_message,
    b"__std_system_error_allocate_message" => system_error_allocate_message,
    b"__std_system_error_deallocate_message" => system_error_deallocate_message,
];
