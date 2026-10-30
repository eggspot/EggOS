//! The runtime's startup and exit, as the startup code MSVC links into every program calls
//! them: the initializer tables (`_initterm`, `_initterm_e`), `argc` and `argv` (the layer's
//! own arguments, which are the program's), the environment, the line `WinMain` gets, the
//! modes the startup sets, the `atexit` tables and `exit`; and `errno`, which lives in a slot of
//! each thread's TEB past the documented part.

use super::super::directory;
use super::super::heap as process_heap;
use super::super::process::process;
use super::super::{SpinLock, call_program, thread_environment};
use super::stdio;
use crate::command_line;
use crate::find;
use crate::library::Export;
use crate::linux;
use crate::text;
use core::cell::UnsafeCell;
use core::ffi::CStr;
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};

/// The C runtime's `errno` values (`errno.h`) the layer sets by name.
pub mod errno {
    pub const EBADF: i32 = 9;
    pub const ENOMEM: i32 = 12;
    pub const EINVAL: i32 = 22;
    pub const EMFILE: i32 = 24;
    pub const ERANGE: i32 = 34;
}

const LAST_LINUX_ERRNO_SHARED_WITH_WINDOWS: u16 = 34;
const DISABLE_PER_THREAD_LOCALE: i32 = 2;
const TEXT_MODE: i32 = 0x4000;
const BINARY_MODE: i32 = 0x8000;
const DLL_PROCESS_DETACH: usize = 0;
const FIRST_EXIT_TABLE_CAPACITY: usize = 32;
const ABORT_STATUS: u32 = 3;
const STATUS_INVALID_CRUNTIME_PARAMETER: u32 = 0xc000_0417;
const POINTER_SIZE: usize = core::mem::size_of::<usize>();

fn errno_slot() -> *mut i32 {
    (thread_environment::current() + thread_environment::teb::ERRNO) as *mut i32
}

/// Sets the calling thread's `errno`.
pub fn set_errno(value: i32) {
    // SAFETY: the slot is the calling thread's own, inside its TEB, which only it writes.
    unsafe { errno_slot().write(value) };
}

/// The calling thread's `errno`.
pub fn errno_value() -> i32 {
    // SAFETY: the slot is the calling thread's own, inside its TEB.
    unsafe { errno_slot().read() }
}

/// The `errno` value for a Linux error number: the same up to `ERANGE`, where Windows and
/// Linux agree, and `EINVAL` past it.
pub fn errno_for_linux(linux: u16) -> i32 {
    match linux {
        1..=LAST_LINUX_ERRNO_SHARED_WITH_WINDOWS => i32::from(linux),
        _ => errno::EINVAL,
    }
}

/// Ends the program because the C runtime cannot go on, as Windows' runtime does: a line on
/// standard error and `status`.
pub fn end_abnormally(reason: &[u8], status: u32) -> ! {
    let _ = linux::write_all(2, b"windows: ");
    let _ = linux::write_all(2, reason);
    let _ = linux::write_all(2, b"\n");
    linux::exit(status)
}

#[repr(C)]
struct OnExitTable {
    first: *mut usize,
    last: *mut usize,
    end: *mut usize,
}

impl OnExitTable {
    const EMPTY: OnExitTable = OnExitTable {
        first: ptr::null_mut(),
        last: ptr::null_mut(),
        end: ptr::null_mut(),
    };
}

struct ProcessTable(UnsafeCell<OnExitTable>);

// SAFETY: the table is only reached under `TABLES`.
unsafe impl Sync for ProcessTable {}

static TABLES: SpinLock<()> = SpinLock::new(());
static AT_EXIT: ProcessTable = ProcessTable(UnsafeCell::new(OnExitTable::EMPTY));
static AT_QUICK_EXIT: ProcessTable = ProcessTable(UnsafeCell::new(OnExitTable::EMPTY));
static THREAD_LOCAL_EXIT_CALLBACK: AtomicUsize = AtomicUsize::new(0);

unsafe fn register(table: *mut OnExitTable, function: usize) -> i32 {
    if table.is_null() {
        return -1;
    }
    TABLES.with(|_| {
        // SAFETY: the caller's table, reached only under the lock.
        let table = unsafe { &mut *table };
        if table.last == table.end {
            let length = (table.last as usize - table.first as usize) / POINTER_SIZE;
            let capacity = (length * 2).max(FIRST_EXIT_TABLE_CAPACITY);
            let grown = match table.first.is_null() {
                true => process_heap::allocate(capacity * POINTER_SIZE),
                // SAFETY: a table's array is a block of the process heap that this file
                // allocated.
                false => unsafe {
                    process_heap::heap_realloc(0, 0, table.first.cast(), capacity * POINTER_SIZE)
                },
            };
            if grown.is_null() {
                return -1;
            }
            table.first = grown.cast();
            table.last = table.first.wrapping_add(length);
            table.end = table.first.wrapping_add(capacity);
        }
        // SAFETY: `last` is inside the array, before `end`.
        unsafe { table.last.write(function) };
        table.last = table.last.wrapping_add(1);
        0
    })
}

unsafe fn execute(table: *mut OnExitTable) -> i32 {
    if table.is_null() {
        return -1;
    }
    loop {
        let (first, last) = TABLES.with(|_| {
            // SAFETY: the caller's table, reached only under the lock.
            let table = unsafe { &mut *table };
            let taken = (table.first, table.last);
            *table = OnExitTable::EMPTY;
            taken
        });
        if first.is_null() {
            return 0;
        }
        let mut at = last;
        while at != first {
            at = at.wrapping_sub(1);
            // SAFETY: `at` is inside the array the table held, which nothing else holds now.
            let function = unsafe { at.read() };
            if function != 0 {
                // SAFETY: the program registered `function`, a `void (*)(void)`.
                unsafe { call_program(function, [0; 4]) };
            }
        }
        // SAFETY: the array is a block of the process heap that nothing uses any more.
        unsafe { process_heap::free(first.cast()) };
    }
}

fn run_exit_work() {
    let callback = THREAD_LOCAL_EXIT_CALLBACK.swap(0, Ordering::AcqRel);
    if callback != 0 {
        let image_base = process().image_base as usize;
        // SAFETY: the program registered `callback`, a `PIMAGE_TLS_CALLBACK`.
        unsafe { call_program(callback, [image_base, DLL_PROCESS_DETACH, 0, 0]) };
    }
    // SAFETY: the process's own table.
    unsafe { execute(AT_EXIT.0.get()) };
    stdio::flush_all_at_exit();
}

unsafe extern "efiapi" fn initterm(first: *const usize, last: *const usize) {
    let mut at = first;
    while at < last {
        // SAFETY: `_initterm`'s contract: an array of function pointers, some null.
        let function = unsafe { at.read() };
        if function != 0 {
            // SAFETY: each is the program's `void (*)(void)`.
            unsafe { call_program(function, [0; 4]) };
        }
        at = at.wrapping_add(1);
    }
}

unsafe extern "efiapi" fn initterm_e(first: *const usize, last: *const usize) -> i32 {
    let mut at = first;
    while at < last {
        // SAFETY: `_initterm_e`'s contract: an array of function pointers, some null.
        let function = unsafe { at.read() };
        if function != 0 {
            // SAFETY: each is the program's `int (*)(void)`.
            let result = unsafe { call_program(function, [0; 4]) } as u32 as i32;
            if result != 0 {
                return result;
            }
        }
        at = at.wrapping_add(1);
    }
    0
}

pub(super) static ARGUMENT_COUNT: AtomicI32 = AtomicI32::new(0);
pub(super) static NARROW_ARGUMENTS: AtomicPtr<*mut u8> = AtomicPtr::new(ptr::null_mut());
pub(super) static WIDE_ARGUMENTS: AtomicPtr<*mut u16> = AtomicPtr::new(ptr::null_mut());
pub(super) static NARROW_ENVIRONMENT: AtomicPtr<*mut u8> = AtomicPtr::new(ptr::null_mut());
pub(super) static WIDE_ENVIRONMENT: AtomicPtr<*mut u16> = AtomicPtr::new(ptr::null_mut());

fn program_arguments() -> impl Iterator<Item = &'static CStr> {
    process().startup.arguments().skip(2)
}

fn environment_strings() -> impl Iterator<Item = &'static CStr> {
    process().startup.environment()
}

fn new_list<T>(count: usize) -> *mut *mut T {
    process_heap::allocate((count + 1) * POINTER_SIZE).cast()
}

fn narrow_list(strings: impl Iterator<Item = &'static CStr>, count: usize) -> *mut *mut u8 {
    let list = new_list::<u8>(count);
    if list.is_null() {
        return list;
    }
    for (index, string) in strings.take(count).enumerate() {
        // SAFETY: a fresh, zeroed block of `count + 1` pointers; the strings are the process's
        // own, on its first stack, writable and kept for its life.
        unsafe {
            list.wrapping_add(index)
                .write(string.as_ptr().cast_mut().cast())
        };
    }
    list
}

fn wide_copy(bytes: &[u8]) -> *mut u16 {
    let units = text::utf16_length(bytes) + 1;
    let copy = process_heap::allocate(units * 2).cast::<u16>();
    if copy.is_null() {
        return copy;
    }
    // SAFETY: a fresh, zeroed block of `units` units, used only through this slice here.
    let out = unsafe { core::slice::from_raw_parts_mut(copy, units) };
    text::write_utf16(bytes, out);
    copy
}

fn wide_list(strings: impl Iterator<Item = &'static CStr>, count: usize) -> *mut *mut u16 {
    let list = new_list::<u16>(count);
    if list.is_null() {
        return list;
    }
    for (index, string) in strings.take(count).enumerate() {
        let copy = wide_copy(string.to_bytes());
        if copy.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: a fresh, zeroed block of `count + 1` pointers.
        unsafe { list.wrapping_add(index).write(copy) };
    }
    list
}

fn configured<T>(list: *mut *mut T, count: usize, into: &AtomicPtr<*mut T>) -> i32 {
    if list.is_null() {
        set_errno(errno::ENOMEM);
        return errno::ENOMEM;
    }
    into.store(list, Ordering::Release);
    ARGUMENT_COUNT.store(count as i32, Ordering::Release);
    0
}

/// `_crt_argv_expanded_arguments`: the mode of `_configure_narrow_argv` and
/// `_configure_wide_argv` that expands wildcards in the arguments (D189).
pub(super) const EXPANDED_ARGUMENTS: i32 = 2;

enum Expanded<'a> {
    Argument { folder: &'a [u8], name: &'a [u8] },
    End,
}

fn expand_pattern(argument: &[u8], start: usize, visit: &mut dyn FnMut(Expanded<'_>)) -> usize {
    let (folder, pattern) = argument.split_at(start);
    let Ok(folder_text) = core::str::from_utf8(folder) else {
        return 0;
    };
    let mut path = [0u8; text::PATH_CAPACITY];
    let length = match text::eggos_path(folder_text.chars(), &mut path) {
        Some(length) if length > 1 => length,
        _ => {
            path[..2].copy_from_slice(b".\0");
            2
        }
    };
    let mut matched = 0;
    directory::for_each_entry(&path[..length], &mut |name, _| {
        if find::matches(pattern, name) {
            visit(Expanded::Argument { folder, name });
            matched += 1;
        }
    });
    matched
}

/// Visits the program's arguments as the C runtime expands them: an argument after the
/// program's name whose last component holds `*` or `?` is each name in its folder that the
/// pattern matches (`.` and `..` never), behind the folder as written, or itself when none
/// does; every argument is followed by [`Expanded::End`].
fn expand_arguments(visit: &mut dyn FnMut(Expanded<'_>)) {
    for (position, argument) in program_arguments().enumerate() {
        let argument = argument.to_bytes();
        let matched = match find::argument_pattern_start(argument) {
            Some(start) if position > 0 => expand_pattern(argument, start, visit),
            _ => 0,
        };
        if matched == 0 {
            visit(Expanded::Argument {
                folder: argument,
                name: b"",
            });
        }
        visit(Expanded::End);
    }
}

/// The expanded arguments ([`expand_arguments`]) as a list of NUL-terminated strings in one
/// heap block kept for the process's life, each argument's names sorted byte-wise; the list
/// and its length, or `None` without memory.
fn expanded_list() -> Option<(*mut *mut u8, usize)> {
    let mut bytes = 0;
    let mut count = 0;
    expand_arguments(&mut |piece| {
        if let Expanded::Argument { folder, name } = piece {
            bytes += folder.len() + name.len() + 1;
            count += 1;
        }
    });
    let block = process_heap::allocate(bytes.max(1));
    let list = new_list::<u8>(count);
    if block.is_null() || list.is_null() {
        return None;
    }
    let mut at = 0;
    let mut index = 0;
    let mut run = 0;
    expand_arguments(&mut |piece| match piece {
        Expanded::Argument { folder, name } => {
            let size = folder.len() + name.len() + 1;
            if index == count || at + size > bytes {
                return;
            }
            // SAFETY: the block holds `bytes` bytes, and this string's `size` fit after `at`.
            let out = unsafe { core::slice::from_raw_parts_mut(block.wrapping_add(at), size) };
            out[..folder.len()].copy_from_slice(folder);
            out[folder.len()..size - 1].copy_from_slice(name);
            out[size - 1] = 0;
            // SAFETY: the list holds `count + 1` pointers and `index` is below `count`.
            unsafe { list.wrapping_add(index).write(block.wrapping_add(at)) };
            at += size;
            index += 1;
        }
        Expanded::End => {
            // SAFETY: the list's entries `run..index` were written above.
            let names =
                unsafe { core::slice::from_raw_parts_mut(list.wrapping_add(run), index - run) };
            names.sort_unstable_by(|a, b| {
                // SAFETY: each entry is a NUL-terminated string in the block.
                let (a, b) = unsafe {
                    (
                        CStr::from_ptr(a.cast_const().cast()),
                        CStr::from_ptr(b.cast_const().cast()),
                    )
                };
                a.cmp(b)
            });
            run = index;
        }
    });
    Some((list, index))
}

pub(super) extern "efiapi" fn configure_narrow_argv(mode: i32) -> i32 {
    if mode == EXPANDED_ARGUMENTS {
        let (list, count) = expanded_list().unwrap_or((ptr::null_mut(), 0));
        return configured(list, count, &NARROW_ARGUMENTS);
    }
    let count = program_arguments().count();
    let list = narrow_list(program_arguments(), count);
    configured(list, count, &NARROW_ARGUMENTS)
}

pub(super) extern "efiapi" fn configure_wide_argv(mode: i32) -> i32 {
    if mode == EXPANDED_ARGUMENTS {
        let (list, count) = match expanded_list() {
            Some((narrow, count)) => {
                // SAFETY: `expanded_list` wrote `count` NUL-terminated strings kept for the
                // process's life.
                let strings = (0..count).map(|index| unsafe {
                    CStr::from_ptr(narrow.wrapping_add(index).read().cast_const().cast())
                });
                (wide_list(strings, count), count)
            }
            None => (ptr::null_mut(), 0),
        };
        return configured(list, count, &WIDE_ARGUMENTS);
    }
    let count = program_arguments().count();
    let list = wide_list(program_arguments(), count);
    configured(list, count, &WIDE_ARGUMENTS)
}

extern "efiapi" fn argument_count() -> *mut i32 {
    ARGUMENT_COUNT.as_ptr()
}

extern "efiapi" fn narrow_arguments() -> *mut *mut *mut u8 {
    NARROW_ARGUMENTS.as_ptr()
}

extern "efiapi" fn wide_arguments() -> *mut *mut *mut u16 {
    WIDE_ARGUMENTS.as_ptr()
}

pub(super) fn narrow_environment() -> *mut *mut u8 {
    let current = NARROW_ENVIRONMENT.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    let list = narrow_list(environment_strings(), environment_strings().count());
    NARROW_ENVIRONMENT.store(list, Ordering::Release);
    list
}

pub(super) fn wide_environment() -> *mut *mut u16 {
    let current = WIDE_ENVIRONMENT.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    let list = wide_list(environment_strings(), environment_strings().count());
    WIDE_ENVIRONMENT.store(list, Ordering::Release);
    list
}

extern "efiapi" fn initialize_narrow_environment() -> i32 {
    if narrow_environment().is_null() {
        -1
    } else {
        0
    }
}

extern "efiapi" fn initialize_wide_environment() -> i32 {
    if wide_environment().is_null() { -1 } else { 0 }
}

extern "efiapi" fn get_initial_narrow_environment() -> *mut *mut u8 {
    narrow_environment()
}

extern "efiapi" fn get_initial_wide_environment() -> *mut *mut u16 {
    wide_environment()
}

extern "efiapi" fn narrow_environment_pointer() -> *mut *mut *mut u8 {
    narrow_environment();
    NARROW_ENVIRONMENT.as_ptr()
}

extern "efiapi" fn wide_environment_pointer() -> *mut *mut *mut u16 {
    wide_environment();
    WIDE_ENVIRONMENT.as_ptr()
}

fn after_program_name<T: Copy + Into<u32> + PartialEq + Default>(line: &'static [T]) -> *mut T {
    let length = line
        .iter()
        .position(|unit| *unit == T::default())
        .unwrap_or(line.len());
    let start = command_line::arguments_start(&line[..length]);
    line[start..].as_ptr().cast_mut()
}

extern "efiapi" fn get_narrow_winmain_command_line() -> *mut u8 {
    after_program_name(process().command_line)
}

extern "efiapi" fn get_wide_winmain_command_line() -> *mut u16 {
    after_program_name(process().command_line_wide)
}

pub(super) extern "efiapi" fn set_app_type(_kind: i32) {}

extern "efiapi" fn set_user_math_error(_handler: usize) {}

extern "efiapi" fn configure_thread_locale(_flag: i32) -> i32 {
    DISABLE_PER_THREAD_LOCALE
}

static NEW_MODE: AtomicI32 = AtomicI32::new(0);
pub(super) static COMMIT_MODE: AtomicI32 = AtomicI32::new(0);
pub(super) static FILE_MODE: AtomicI32 = AtomicI32::new(TEXT_MODE);

pub(super) extern "efiapi" fn set_new_mode(mode: i32) -> i32 {
    NEW_MODE.swap(mode, Ordering::AcqRel)
}

extern "efiapi" fn query_new_mode() -> i32 {
    NEW_MODE.load(Ordering::Acquire)
}

extern "efiapi" fn commit_mode() -> *mut i32 {
    COMMIT_MODE.as_ptr()
}

extern "efiapi" fn file_mode() -> *mut i32 {
    FILE_MODE.as_ptr()
}

extern "efiapi" fn set_file_mode(mode: i32) -> i32 {
    if mode != TEXT_MODE && mode != BINARY_MODE {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    FILE_MODE.store(mode, Ordering::Release);
    0
}

unsafe extern "efiapi" fn get_file_mode(out: *mut i32) -> i32 {
    if out.is_null() {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    // SAFETY: `_get_fmode`'s contract: a writable `int`.
    unsafe { out.write(FILE_MODE.load(Ordering::Acquire)) };
    0
}

/// Whether streams opened without `t` or `b` are binary: `_fmode` is `_O_BINARY`.
pub fn binary_by_default() -> bool {
    FILE_MODE.load(Ordering::Acquire) == BINARY_MODE
}

extern "efiapi" fn register_thread_local_exe_atexit_callback(callback: usize) {
    THREAD_LOCAL_EXIT_CALLBACK.store(callback, Ordering::Release);
}

pub(super) extern "efiapi" fn crt_atexit(function: usize) -> i32 {
    // SAFETY: the process's own table.
    unsafe { register(AT_EXIT.0.get(), function) }
}

extern "efiapi" fn crt_at_quick_exit(function: usize) -> i32 {
    // SAFETY: the process's own table.
    unsafe { register(AT_QUICK_EXIT.0.get(), function) }
}

unsafe extern "efiapi" fn initialize_onexit_table(table: *mut OnExitTable) -> i32 {
    if table.is_null() {
        return -1;
    }
    TABLES.with(|_| {
        // SAFETY: `_initialize_onexit_table`'s contract: the program's writable table.
        let table = unsafe { &mut *table };
        if table.first.is_null() {
            *table = OnExitTable::EMPTY;
        }
    });
    0
}

unsafe extern "efiapi" fn register_onexit_function(
    table: *mut OnExitTable,
    function: usize,
) -> i32 {
    // SAFETY: `_register_onexit_function`'s contract: a table `_initialize_onexit_table` set.
    unsafe { register(table, function) }
}

unsafe extern "efiapi" fn execute_onexit_table(table: *mut OnExitTable) -> i32 {
    // SAFETY: `_execute_onexit_table`'s contract: a table `_initialize_onexit_table` set.
    unsafe { execute(table) }
}

extern "efiapi" fn exit(code: i32) -> ! {
    run_exit_work();
    linux::exit(code as u32)
}

extern "efiapi" fn quick_exit(code: i32) -> ! {
    // SAFETY: the process's own table.
    unsafe { execute(AT_QUICK_EXIT.0.get()) };
    linux::exit(code as u32)
}

extern "efiapi" fn exit_now(code: i32) -> ! {
    linux::exit(code as u32)
}

extern "efiapi" fn cexit() {
    run_exit_work();
}

extern "efiapi" fn c_exit() {}

extern "efiapi" fn abort() -> ! {
    end_abnormally(b"the program called abort", ABORT_STATUS)
}

extern "efiapi" fn invalid_parameter() -> ! {
    end_abnormally(
        b"the program gave the C runtime an invalid parameter",
        STATUS_INVALID_CRUNTIME_PARAMETER,
    )
}

extern "efiapi" fn errno_location() -> *mut i32 {
    errno_slot()
}

unsafe extern "efiapi" fn get_errno(out: *mut i32) -> i32 {
    if out.is_null() {
        return errno::EINVAL;
    }
    // SAFETY: the caller passes a writable `int`.
    unsafe { out.write(errno_value()) };
    0
}

extern "efiapi" fn set_errno_value(value: i32) -> i32 {
    set_errno(value);
    0
}

extern "efiapi" fn dos_errno_location() -> *mut u32 {
    (thread_environment::current() + thread_environment::teb::DOS_ERRNO) as *mut u32
}

unsafe extern "efiapi" fn getenv(name: *const u8) -> *mut u8 {
    if name.is_null() {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    }
    // SAFETY: `getenv`'s contract: a NUL-terminated name.
    let name = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
    let list = narrow_environment();
    if name.is_empty() || name.contains(&b'=') || list.is_null() {
        return ptr::null_mut();
    }
    for index in 0.. {
        // SAFETY: the environment list is NULL-terminated and lives for the program's life.
        let entry = unsafe { list.add(index).read() };
        if entry.is_null() {
            break;
        }
        // SAFETY: each entry is a NUL-terminated `NAME=value` string of the list.
        let bytes = unsafe { CStr::from_ptr(entry.cast()) }.to_bytes();
        let named = bytes.len() > name.len()
            && bytes[name.len()] == b'='
            && bytes[..name.len()].eq_ignore_ascii_case(name);
        if named {
            return entry.wrapping_add(name.len() + 1);
        }
    }
    ptr::null_mut()
}

const ASSERTION_MESSAGE_BYTES: usize = 512;

unsafe extern "efiapi" fn wassert(message: *const u16, file: *const u16, line: u32) {
    let mut text = [0u8; ASSERTION_MESSAGE_BYTES];
    let mut length = 0;
    let mut push = |bytes: &[u8]| {
        let room = text.len() - length;
        let taken = bytes.len().min(room);
        text[length..length + taken].copy_from_slice(&bytes[..taken]);
        length += taken;
    };
    let push_wide = |units: *const u16, push: &mut dyn FnMut(&[u8])| {
        if units.is_null() {
            return;
        }
        // SAFETY: `_wassert`'s contract: NUL-terminated wide strings.
        let units = unsafe { super::super::file::wide_string(units) };
        let mut bytes = [0u8; ASSERTION_MESSAGE_BYTES];
        let (written, _) = crate::text::write_utf8(units, &mut bytes);
        push(&bytes[..written]);
    };
    push(b"Assertion failed: ");
    push_wide(message, &mut push);
    push(b", file ");
    push_wide(file, &mut push);
    push(b", line ");
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut rest = line;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    push(&digits[at..]);
    end_abnormally(&text[..length], ABORT_STATUS)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"getenv" => getenv,
    b"_wassert" => wassert,
    b"_initterm" => initterm,
    b"_initterm_e" => initterm_e,
    b"_configure_narrow_argv" => configure_narrow_argv,
    b"_configure_wide_argv" => configure_wide_argv,
    b"__p___argc" => argument_count,
    b"__p___argv" => narrow_arguments,
    b"__p___wargv" => wide_arguments,
    b"_initialize_narrow_environment" => initialize_narrow_environment,
    b"_initialize_wide_environment" => initialize_wide_environment,
    b"_get_initial_narrow_environment" => get_initial_narrow_environment,
    b"_get_initial_wide_environment" => get_initial_wide_environment,
    b"__p__environ" => narrow_environment_pointer,
    b"__p__wenviron" => wide_environment_pointer,
    b"_get_narrow_winmain_command_line" => get_narrow_winmain_command_line,
    b"_get_wide_winmain_command_line" => get_wide_winmain_command_line,
    b"_set_app_type" => set_app_type,
    b"__setusermatherr" => set_user_math_error,
    b"_configthreadlocale" => configure_thread_locale,
    b"_set_new_mode" => set_new_mode,
    b"_query_new_mode" => query_new_mode,
    b"__p__commode" => commit_mode,
    b"__p__fmode" => file_mode,
    b"_set_fmode" => set_file_mode,
    b"_get_fmode" => get_file_mode,
    b"_register_thread_local_exe_atexit_callback" => register_thread_local_exe_atexit_callback,
    b"_crt_atexit" => crt_atexit,
    b"_crt_at_quick_exit" => crt_at_quick_exit,
    b"_initialize_onexit_table" => initialize_onexit_table,
    b"_register_onexit_function" => register_onexit_function,
    b"_execute_onexit_table" => execute_onexit_table,
    b"exit" => exit,
    b"quick_exit" => quick_exit,
    b"_exit" => exit_now,
    b"_Exit" => exit_now,
    b"_cexit" => cexit,
    b"_c_exit" => c_exit,
    b"abort" => abort,
    b"_invalid_parameter_noinfo" => invalid_parameter,
    b"_invalid_parameter_noinfo_noreturn" => invalid_parameter,
    b"_errno" => errno_location,
    b"_get_errno" => get_errno,
    b"_set_errno" => set_errno_value,
    b"__doserrno" => dos_errno_location,
];
