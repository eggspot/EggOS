//! What the startup code MSVC links into every program needs besides the runtime: kernel32's
//! `IsDebuggerPresent`, `IsProcessorFeaturePresent`, `InitializeSListHead`, `GetStartupInfoW`
//! (no `STARTF_USESHOWWINDOW`, so `wWinMain` gets `SW_SHOWDEFAULT`), the unhandled-exception
//! filter, and the unwinding functions its failure path asks, which find no function entry, so
//! that path goes straight to ending the process; and vcruntime's exception plumbing, which
//! only the exception dispatch EggOS does not do yet would call.

use super::super::{Bool, FALSE, TRUE};
use crate::library::Export;
use core::sync::atomic::{AtomicUsize, Ordering};

const STARTUP_INFO_SIZE: usize = 104;
const SINGLE_LIST_HEADER_SIZE: usize = 16;
const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
const EXCEPTION_DISPOSITION_CONTINUE_SEARCH: i32 = 1;
#[cfg(target_arch = "x86_64")]
const FEATURES_PRESENT: &[u32] = &[6, 8, 10, 12];
#[cfg(target_arch = "aarch64")]
const FEATURES_PRESENT: &[u32] = &[12, 19, 29];

static UNHANDLED_EXCEPTION_FILTER: AtomicUsize = AtomicUsize::new(0);
static CURRENT_EXCEPTION: AtomicUsize = AtomicUsize::new(0);
static CURRENT_EXCEPTION_CONTEXT: AtomicUsize = AtomicUsize::new(0);

extern "efiapi" fn is_debugger_present() -> Bool {
    FALSE
}

extern "efiapi" fn is_processor_feature_present(feature: u32) -> Bool {
    match FEATURES_PRESENT.contains(&feature) {
        true => TRUE,
        false => FALSE,
    }
}

unsafe extern "efiapi" fn initialize_single_list_head(head: *mut u8) {
    // SAFETY: `InitializeSListHead`'s contract: a writable `SLIST_HEADER`.
    unsafe { head.write_bytes(0, SINGLE_LIST_HEADER_SIZE) };
}

unsafe extern "efiapi" fn get_startup_info(info: *mut u8) {
    // SAFETY: `GetStartupInfoW`'s contract: a writable `STARTUPINFOW`.
    unsafe { info.write_bytes(0, STARTUP_INFO_SIZE) };
    // SAFETY: as above; `cb`, its size, comes first.
    unsafe { info.cast::<u32>().write_unaligned(STARTUP_INFO_SIZE as u32) };
}

extern "efiapi" fn set_unhandled_exception_filter(filter: usize) -> usize {
    UNHANDLED_EXCEPTION_FILTER.swap(filter, Ordering::AcqRel)
}

extern "efiapi" fn unhandled_exception_filter(_pointers: usize) -> i32 {
    EXCEPTION_EXECUTE_HANDLER
}

extern "efiapi" fn capture_context(_context: usize) {}

extern "efiapi" fn lookup_function_entry(_pc: usize, _image_base: usize, _history: usize) -> usize {
    0
}

#[allow(clippy::too_many_arguments)]
extern "efiapi" fn virtual_unwind(
    _kind: u32,
    _image_base: usize,
    _pc: usize,
    _function: usize,
    _context: usize,
    _handler_data: usize,
    _establisher_frame: usize,
    _context_pointers: usize,
) -> usize {
    0
}

extern "efiapi" fn c_specific_handler(
    _record: usize,
    _frame: usize,
    _context: usize,
    _dispatcher: usize,
) -> i32 {
    EXCEPTION_DISPOSITION_CONTINUE_SEARCH
}

extern "efiapi" fn current_exception() -> *mut usize {
    CURRENT_EXCEPTION.as_ptr()
}

extern "efiapi" fn current_exception_context() -> *mut usize {
    CURRENT_EXCEPTION_CONTEXT.as_ptr()
}

extern "efiapi" fn destroy_type_info_list(_list: usize) {}

extern "efiapi" fn pure_call() -> ! {
    super::runtime::end_abnormally(b"the program called a pure virtual function", 3)
}

pub(super) const KERNEL32_EXPORTS: &[Export] = exports![
    b"IsDebuggerPresent" => is_debugger_present,
    b"IsProcessorFeaturePresent" => is_processor_feature_present,
    b"InitializeSListHead" => initialize_single_list_head,
    b"GetStartupInfoW" => get_startup_info,
    b"GetStartupInfoA" => get_startup_info,
    b"SetUnhandledExceptionFilter" => set_unhandled_exception_filter,
    b"UnhandledExceptionFilter" => unhandled_exception_filter,
    b"RtlCaptureContext" => capture_context,
    b"RtlLookupFunctionEntry" => lookup_function_entry,
    b"RtlVirtualUnwind" => virtual_unwind,
];

pub(super) const VCRUNTIME_EXPORTS: &[Export] = exports![
    b"__C_specific_handler" => c_specific_handler,
    b"__current_exception" => current_exception,
    b"__current_exception_context" => current_exception_context,
    b"__std_type_info_destroy_list" => destroy_type_info_list,
    b"_purecall" => pure_call,
];
