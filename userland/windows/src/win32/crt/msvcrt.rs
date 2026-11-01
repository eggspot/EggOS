//! The old C runtime, `msvcrt.dll`, which MinGW-w64 programs import (D189): the Universal C
//! Runtime's functions where the two agree, under the old names where they differ. The startup
//! MinGW links into the program asks for `argc`, `argv` and the environment through
//! `__getmainargs`, registers its exit work with `atexit` and `_onexit`, and reads `_fmode`,
//! `_commode` and `__initenv` as data; its own `printf` locks the streams of the `FILE` array
//! (`__iob_func`, in `stdio.rs`) through `_lock` and `_unlock`, and reads `localeconv`.

use super::super::sync::{enter_critical_section, leave_critical_section};
use super::runtime::{self, end_abnormally};
use crate::error_messages;
use crate::library::Export;
use crate::msvcrt_layout;
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

const RUNTIME_ERROR_STATUS: u32 = 255;
const FIRST_RUNTIME_ERROR: i32 = 6000;
const C_LOCALE_CODE_PAGE: u32 = 0;
const C_LOCALE_LONGEST_CHARACTER: i32 = 1;
const CHAR_MAX: u8 = 127;
const CRITICAL_SECTION_WORDS: usize = msvcrt_layout::FILE_LOCK_BYTES / 8;

static INITIAL_ENVIRONMENT: AtomicPtr<*mut u8> = AtomicPtr::new(ptr::null_mut());
static INITIAL_WIDE_ENVIRONMENT: AtomicPtr<*mut u16> = AtomicPtr::new(ptr::null_mut());

#[repr(C)]
struct StartupInfo {
    new_mode: i32,
}

unsafe fn give_main_arguments<T>(
    count_out: *mut i32,
    arguments_out: *mut *mut *mut T,
    environment_out: *mut *mut *mut T,
    startup: *const StartupInfo,
    arguments: *mut *mut T,
    environment: *mut *mut T,
) {
    if !startup.is_null() {
        // SAFETY: `__getmainargs`'s contract: null or the startup's `_startupinfo`.
        runtime::set_new_mode(unsafe { startup.read() }.new_mode);
    }
    // SAFETY: `__getmainargs`'s contract: each out pointer is the startup's writable variable.
    unsafe { count_out.write(runtime::ARGUMENT_COUNT.load(Ordering::Acquire)) };
    // SAFETY: as above.
    unsafe { arguments_out.write(arguments) };
    // SAFETY: as above.
    unsafe { environment_out.write(environment) };
}

fn argument_mode(expand_wildcards: i32) -> i32 {
    match expand_wildcards {
        0 => 0,
        _ => runtime::EXPANDED_ARGUMENTS,
    }
}

unsafe extern "efiapi" fn get_main_arguments(
    count: *mut i32,
    arguments: *mut *mut *mut u8,
    environment: *mut *mut *mut u8,
    expand_wildcards: i32,
    startup: *const StartupInfo,
) -> i32 {
    let list = runtime::narrow_environment();
    if runtime::configure_narrow_argv(argument_mode(expand_wildcards)) != 0 || list.is_null() {
        return -1;
    }
    INITIAL_ENVIRONMENT.store(list, Ordering::Release);
    let argv = runtime::NARROW_ARGUMENTS.load(Ordering::Acquire);
    // SAFETY: `__getmainargs`'s contract, which the caller keeps.
    unsafe { give_main_arguments(count, arguments, environment, startup, argv, list) };
    0
}

unsafe extern "efiapi" fn get_wide_main_arguments(
    count: *mut i32,
    arguments: *mut *mut *mut u16,
    environment: *mut *mut *mut u16,
    expand_wildcards: i32,
    startup: *const StartupInfo,
) -> i32 {
    let list = runtime::wide_environment();
    if runtime::configure_wide_argv(argument_mode(expand_wildcards)) != 0 || list.is_null() {
        return -1;
    }
    INITIAL_WIDE_ENVIRONMENT.store(list, Ordering::Release);
    let argv = runtime::WIDE_ARGUMENTS.load(Ordering::Acquire);
    // SAFETY: `__wgetmainargs`'s contract, which the caller keeps.
    unsafe { give_main_arguments(count, arguments, environment, startup, argv, list) };
    0
}

extern "efiapi" fn on_exit(function: usize) -> usize {
    match runtime::crt_atexit(function) {
        0 => function,
        _ => 0,
    }
}

extern "efiapi" fn runtime_error_exit(code: i32) -> ! {
    let mut text = *b"runtime error R0000000000";
    let prefix = b"runtime error R".len();
    let mut number = FIRST_RUNTIME_ERROR.saturating_add(code).unsigned_abs();
    let mut digits = 0;
    let mut reversed = [0u8; 10];
    loop {
        reversed[digits] = b'0' + (number % 10) as u8;
        digits += 1;
        number /= 10;
        if number == 0 {
            break;
        }
    }
    for (at, digit) in reversed[..digits].iter().rev().enumerate() {
        text[prefix + at] = *digit;
    }
    end_abnormally(&text[..prefix + digits], RUNTIME_ERROR_STATUS)
}

extern "efiapi" fn initialize_locale_conventions() -> i32 {
    0
}

#[cfg(target_arch = "x86_64")]
extern "efiapi" fn reset_floating_point() {
    const X87_CONTROL: u16 = 0x027f;
    const SSE_CONTROL: u32 = 0x1f80;
    let x87 = X87_CONTROL;
    let sse = SSE_CONTROL;
    // SAFETY: loads Windows' default floating-point control words for the calling thread,
    // which only change how later floating-point instructions round and trap.
    unsafe {
        core::arch::asm!(
            "fninit",
            "fldcw word ptr [{x87}]",
            "ldmxcsr dword ptr [{sse}]",
            x87 = in(reg) &raw const x87,
            sse = in(reg) &raw const sse,
            options(nostack),
        )
    };
}

#[cfg(target_arch = "aarch64")]
extern "efiapi" fn reset_floating_point() {
    // SAFETY: clears the floating-point control register to Windows' default for the calling
    // thread, which only changes how later floating-point instructions round and trap.
    unsafe { core::arch::asm!("msr fpcr, xzr", options(nomem, nostack)) };
}

extern "efiapi" fn locale_code_page() -> u32 {
    C_LOCALE_CODE_PAGE
}

extern "efiapi" fn longest_character() -> i32 {
    C_LOCALE_LONGEST_CHARACTER
}

#[repr(C)]
struct LocaleConventions {
    texts: [*const u8; 10],
    numbers: [u8; 8],
    wide_texts: [*const u16; 8],
}

// SAFETY: the conventions are read-only for the program's life.
unsafe impl Sync for LocaleConventions {}

const EMPTY: *const u8 = c"".as_ptr().cast();
const WIDE_DECIMAL_POINT: &[u16] = &[b'.' as u16, 0];
const WIDE_EMPTY_TEXT: &[u16] = &[0];
const WIDE_EMPTY: *const u16 = WIDE_EMPTY_TEXT.as_ptr();

static C_LOCALE_CONVENTIONS: LocaleConventions = LocaleConventions {
    texts: [
        c".".as_ptr().cast(),
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
        EMPTY,
    ],
    numbers: [CHAR_MAX; 8],
    wide_texts: [
        WIDE_DECIMAL_POINT.as_ptr(),
        WIDE_EMPTY,
        WIDE_EMPTY,
        WIDE_EMPTY,
        WIDE_EMPTY,
        WIDE_EMPTY,
        WIDE_EMPTY,
        WIDE_EMPTY,
    ],
};

extern "efiapi" fn locale_conventions() -> *const LocaleConventions {
    &raw const C_LOCALE_CONVENTIONS
}

extern "efiapi" fn error_text(number: i32) -> *const u8 {
    error_messages::message(number).as_ptr()
}

struct RuntimeLock(UnsafeCell<[u64; CRITICAL_SECTION_WORDS]>);

// SAFETY: each lock is a critical section, which the sync functions reach atomically.
unsafe impl Sync for RuntimeLock {}

static LOCKS: [RuntimeLock; msvcrt_layout::LOCKS] =
    [const { RuntimeLock(UnsafeCell::new([0; CRITICAL_SECTION_WORDS])) }; msvcrt_layout::LOCKS];

extern "efiapi" fn lock(number: i32) {
    let Some(slot) = msvcrt_layout::lock_slot(number) else {
        return;
    };
    // SAFETY: a zeroed critical section, which is an initialized one, kept for the process's
    // life.
    unsafe { enter_critical_section(LOCKS[slot].0.get().cast()) };
}

extern "efiapi" fn unlock(number: i32) {
    let Some(slot) = msvcrt_layout::lock_slot(number) else {
        return;
    };
    // SAFETY: the critical section `_lock` entered on this thread.
    unsafe { leave_critical_section(LOCKS[slot].0.get().cast()) };
}

/// The functions the old C runtime has under names of its own.
pub(super) const EXPORTS: &[Export] = exports![
    b"__getmainargs" => get_main_arguments,
    b"__wgetmainargs" => get_wide_main_arguments,
    b"__set_app_type" => runtime::set_app_type,
    b"_onexit" => on_exit,
    b"atexit" => runtime::crt_atexit,
    b"_amsg_exit" => runtime_error_exit,
    b"__lconv_init" => initialize_locale_conventions,
    b"_fpreset" => reset_floating_point,
    b"___lc_codepage_func" => locale_code_page,
    b"___mb_cur_max_func" => longest_character,
    b"localeconv" => locale_conventions,
    b"strerror" => error_text,
    b"_lock" => lock,
    b"_unlock" => unlock,
];

static DATA_EXPORTS: [Export; 9] = [
    Export {
        name: b"_fmode",
        address: &raw const runtime::FILE_MODE as *const (),
    },
    Export {
        name: b"_commode",
        address: &raw const runtime::COMMIT_MODE as *const (),
    },
    Export {
        name: b"__argc",
        address: &raw const runtime::ARGUMENT_COUNT as *const (),
    },
    Export {
        name: b"__argv",
        address: &raw const runtime::NARROW_ARGUMENTS as *const (),
    },
    Export {
        name: b"__wargv",
        address: &raw const runtime::WIDE_ARGUMENTS as *const (),
    },
    Export {
        name: b"_environ",
        address: &raw const runtime::NARROW_ENVIRONMENT as *const (),
    },
    Export {
        name: b"_wenviron",
        address: &raw const runtime::WIDE_ENVIRONMENT as *const (),
    },
    Export {
        name: b"__initenv",
        address: &raw const INITIAL_ENVIRONMENT as *const (),
    },
    Export {
        name: b"__winitenv",
        address: &raw const INITIAL_WIDE_ENVIRONMENT as *const (),
    },
];

/// The old C runtime's variables, which programs import as data: the address of each.
pub(super) const DATA: &[Export] = &DATA_EXPORTS;
