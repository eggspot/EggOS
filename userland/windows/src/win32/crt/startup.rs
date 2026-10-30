//! What the startup code MSVC links into every program needs besides the runtime: kernel32's
//! `IsDebuggerPresent`, `IsProcessorFeaturePresent`, `InitializeSListHead` and
//! `GetStartupInfoW` (no `STARTF_USESHOWWINDOW`, so `wWinMain` gets `SW_SHOWDEFAULT`), and
//! vcruntime's `__std_type_info_destroy_list` and `_purecall`. The unhandled-exception filter,
//! the unwinding functions and the language handlers are exception handling's (D177,
//! `win32/exception`).

use super::super::{Bool, FALSE, TRUE};
use crate::library::Export;
use core::sync::atomic::{AtomicU64, Ordering};

const STARTUP_INFO_SIZE: usize = 104;
const SINGLE_LIST_HEADER_SIZE: usize = 16;
static PROCESSOR_FEATURES: AtomicU64 = AtomicU64::new(0);

extern "efiapi" fn is_debugger_present() -> Bool {
    FALSE
}

/// Sets the features `IsProcessorFeaturePresent` answers for, one bit per feature number.
pub fn set_processor_features(features: u64) {
    PROCESSOR_FEATURES.store(features, Ordering::Relaxed);
}

extern "efiapi" fn is_processor_feature_present(feature: u32) -> Bool {
    let features = PROCESSOR_FEATURES.load(Ordering::Relaxed);
    match crate::user_shared_data::present(features, feature) {
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
];

pub(super) const VCRUNTIME_EXPORTS: &[Export] = exports![
    b"__std_type_info_destroy_list" => destroy_type_info_list,
    b"_purecall" => pure_call,
];
