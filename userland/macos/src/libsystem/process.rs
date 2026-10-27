//! The process: `exit` and `atexit`, `abort`, signals to itself, process ids, the environment
//! (`getenv`, `setenv`, `_NSGetEnviron` and the other `_NSGet*` functions crt1 once provided),
//! the stack protector's guard, clocks and sleeping, and numbers (`atoi`, `strtol`, `qsort`,
//! `rand`).

use super::{Exclusive, c_string, errno_from, heap, set_errno, stdio};
use crate::darwin::{self, EINVAL};
use crate::linux;
use core::ffi::{c_char, c_int, c_long, c_longlong, c_uint, c_ulong, c_ulonglong, c_void};
use core::ptr;

const MAX_HANDLERS: usize = 64;

static HANDLERS: Exclusive<([usize; MAX_HANDLERS], usize)> = Exclusive::new(([0; MAX_HANDLERS], 0));

/// The `environ` array: the program's `envp`, copied when `setenv` first changes it.
pub static mut ENVIRON: *mut *mut c_char = ptr::null_mut();
static mut ARGUMENT_COUNT: c_int = 0;
static mut ARGUMENTS: *mut *mut c_char = ptr::null_mut();
static mut PROGRAM_NAME: *const c_char = ptr::null();
static ENVIRONMENT_CAPACITY: Exclusive<usize> = Exclusive::new(0);

/// The stack protector's guard, `__stack_chk_guard`, set once before the program starts.
pub static mut STACK_GUARD: usize = 0;

/// Records what `_NSGetArgc`, `_NSGetArgv`, `_NSGetEnviron` and `_NSGetProgname` return.
///
/// # Safety
/// Called once, before the program runs; the arrays must stay for as long as it does.
pub unsafe fn set_process(
    count: usize,
    arguments: *mut *mut c_char,
    environment: *mut *mut c_char,
) {
    // SAFETY: the caller's promise: nothing else uses these yet.
    unsafe {
        ARGUMENT_COUNT = count as c_int;
        ARGUMENTS = arguments;
        ENVIRON = environment;
    }
    // SAFETY: `arguments` holds `count` strings.
    let program = if count > 0 {
        unsafe { arguments.read() }
    } else {
        ptr::null_mut()
    };
    if !program.is_null() {
        // SAFETY: `program` is a NUL-terminated argument.
        let bytes = unsafe { c_string(program) };
        let start = bytes
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(0, |slash| slash + 1);
        // SAFETY: as above.
        unsafe { PROGRAM_NAME = program.add(start) };
    }
    let mut guard = [0u8; 8];
    // SAFETY: `getrandom` writes 8 bytes into the borrowed buffer.
    let _ = unsafe {
        linux::syscall(
            linux::number::GETRANDOM,
            [guard.as_mut_ptr() as usize, guard.len(), 0, 0, 0, 0],
        )
    };
    // SAFETY: the program has not started yet.
    unsafe { STACK_GUARD = usize::from_le_bytes(guard) & !0xff };
}

pub extern "C" fn ns_get_argc() -> *mut c_int {
    &raw mut ARGUMENT_COUNT
}

pub extern "C" fn ns_get_argv() -> *mut *mut *mut c_char {
    &raw mut ARGUMENTS
}

pub extern "C" fn ns_get_environ() -> *mut *mut *mut c_char {
    &raw mut ENVIRON
}

pub extern "C" fn ns_get_progname() -> *mut *const c_char {
    &raw mut PROGRAM_NAME
}

pub extern "C" fn atexit(handler: extern "C" fn()) -> c_int {
    HANDLERS.with(|(handlers, count)| {
        if *count == MAX_HANDLERS {
            return -1;
        }
        handlers[*count] = handler as usize;
        *count += 1;
        0
    })
}

pub extern "C" fn exit(status: c_int) -> ! {
    loop {
        let handler = HANDLERS.with(|(handlers, count)| {
            (*count > 0).then(|| {
                *count -= 1;
                handlers[*count]
            })
        });
        let Some(handler) = handler else { break };
        // SAFETY: `atexit` stored a function that takes nothing.
        let handler: extern "C" fn() = unsafe { core::mem::transmute(handler) };
        handler();
    }
    stdio::flush_all();
    linux::exit(status)
}

pub extern "C" fn exit_now(status: c_int) -> ! {
    linux::exit(status)
}

pub extern "C" fn abort() -> ! {
    stdio::flush_all();
    let _ = kill_self(linux::SIGABRT);
    linux::exit(134)
}

fn kill_self(signal: usize) -> Result<usize, eggos_runtime::errno::Errno> {
    // SAFETY: `kill` touches no memory.
    unsafe {
        linux::syscall(
            linux::number::KILL,
            [linux::process_id(), signal, 0, 0, 0, 0],
        )
    }
}

/// Ends the program as Darwin's libSystem does on a fatal check: a message, then `abort`.
pub fn fail(message: &[u8]) -> ! {
    let _ = linux::write_all(2, message);
    let _ = linux::write_all(2, b"\n");
    abort()
}

pub extern "C" fn stack_chk_fail() -> ! {
    fail(b"stack buffer overflow detected")
}

pub extern "C" fn raise(signal: c_int) -> c_int {
    let Some(linux_signal) = darwin::linux_signal(signal) else {
        set_errno(EINVAL);
        return -1;
    };
    stdio::flush_all();
    kill_self(linux_signal).map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

pub extern "C" fn kill(pid: c_int, signal: c_int) -> c_int {
    let Some(linux_signal) = darwin::linux_signal(signal) else {
        set_errno(EINVAL);
        return -1;
    };
    // SAFETY: `kill` touches no memory.
    unsafe {
        linux::syscall(
            linux::number::KILL,
            [pid as usize, linux_signal, 0, 0, 0, 0],
        )
    }
    .map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

pub extern "C" fn getpid() -> c_int {
    linux::process_id() as c_int
}

pub extern "C" fn getppid() -> c_int {
    // SAFETY: `getppid` touches no memory.
    unsafe { linux::syscall(linux::number::GETPPID, [0; 6]) }.unwrap_or(0) as c_int
}

pub extern "C" fn getuid() -> c_uint {
    // SAFETY: `getuid` touches no memory.
    unsafe { linux::syscall(linux::number::GETUID, [0; 6]) }.unwrap_or(0) as c_uint
}

fn find_variable(name: &[u8]) -> Option<(usize, *mut c_char)> {
    // SAFETY: `ENVIRON` is a null-ended array of `NAME=value` strings.
    let environment = unsafe { ENVIRON };
    if environment.is_null() {
        return None;
    }
    let mut index = 0;
    loop {
        // SAFETY: as above; the walk stops at the null entry.
        let entry = unsafe { environment.add(index).read() };
        if entry.is_null() {
            return None;
        }
        // SAFETY: each entry is NUL-terminated.
        let bytes = unsafe { c_string(entry) };
        if bytes.len() > name.len() && bytes.starts_with(name) && bytes[name.len()] == b'=' {
            return Some((index, entry));
        }
        index += 1;
    }
}

/// # Safety
/// `name` must be a NUL-terminated string.
pub unsafe extern "C" fn getenv(name: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let name = unsafe { c_string(name) };
    find_variable(name).map_or(ptr::null_mut(), |(_, entry)| {
        entry.wrapping_add(name.len() + 1)
    })
}

fn environment_length() -> usize {
    // SAFETY: as in `find_variable`.
    let environment = unsafe { ENVIRON };
    let mut length = 0;
    // SAFETY: as above.
    while !environment.is_null() && !unsafe { environment.add(length).read() }.is_null() {
        length += 1;
    }
    length
}

fn put_variable(entry: *mut c_char, name: &[u8]) -> c_int {
    if let Some((index, _)) = find_variable(name) {
        // SAFETY: `index` is inside the array.
        unsafe { ENVIRON.add(index).write(entry) };
        return 0;
    }
    let length = environment_length();
    let mut capacity = ENVIRONMENT_CAPACITY.with(|capacity| *capacity);
    if length + 1 >= capacity {
        let grown = (length + 2).max(16) * 2;
        let array = heap::allocate(grown * size_of::<usize>(), true).cast::<*mut c_char>();
        if array.is_null() {
            return -1;
        }
        // SAFETY: the old array holds `length` entries, the new one `grown`.
        unsafe {
            if !ENVIRON.is_null() {
                array.copy_from_nonoverlapping(ENVIRON, length);
            }
        }
        if capacity != 0 {
            // SAFETY: an array this function allocated earlier, which nothing else keeps.
            heap::release(unsafe { ENVIRON }.cast());
        }
        // SAFETY: the new array replaces the old one.
        unsafe { ENVIRON = array };
        capacity = grown;
        ENVIRONMENT_CAPACITY.with(|stored| *stored = capacity);
    }
    // SAFETY: the array has room for the entry and the null after it.
    unsafe {
        ENVIRON.add(length).write(entry);
    }
    // SAFETY: as above.
    unsafe { ENVIRON.add(length + 1).write(ptr::null_mut()) };
    0
}

fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && !name.contains(&b'=')
}

/// # Safety
/// `name` and `value` must be NUL-terminated strings.
pub unsafe extern "C" fn setenv(
    name: *const c_char,
    value: *const c_char,
    overwrite: c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    let (name, value) = unsafe { (c_string(name), c_string(value)) };
    if !valid_name(name) {
        set_errno(EINVAL);
        return -1;
    }
    if overwrite == 0 && find_variable(name).is_some() {
        return 0;
    }
    let entry = heap::allocate(name.len() + value.len() + 2, false);
    if entry.is_null() {
        return -1;
    }
    // SAFETY: `entry` holds the name, `=`, the value and the NUL.
    unsafe {
        entry.copy_from_nonoverlapping(name.as_ptr(), name.len());
        entry.add(name.len()).write(b'=');
        entry
            .add(name.len() + 1)
            .copy_from_nonoverlapping(value.as_ptr(), value.len());
        entry.add(name.len() + 1 + value.len()).write(0);
    }
    put_variable(entry.cast(), name)
}

/// # Safety
/// `entry` must be a NUL-terminated `NAME=value` string that stays.
pub unsafe extern "C" fn putenv(entry: *mut c_char) -> c_int {
    // SAFETY: the caller's promise.
    let bytes = unsafe { c_string(entry) };
    let Some(equals) = bytes.iter().position(|&b| b == b'=') else {
        set_errno(EINVAL);
        return -1;
    };
    put_variable(entry, &bytes[..equals])
}

/// # Safety
/// `name` must be a NUL-terminated string.
pub unsafe extern "C" fn unsetenv(name: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let name = unsafe { c_string(name) };
    if !valid_name(name) {
        set_errno(EINVAL);
        return -1;
    }
    while let Some((index, _)) = find_variable(name) {
        let length = environment_length();
        // SAFETY: the entries after `index`, and the null, move down one.
        unsafe {
            ENVIRON
                .add(index)
                .copy_from(ENVIRON.add(index + 1), length - index)
        };
    }
    0
}

#[repr(C)]
pub struct Timespec {
    seconds: i64,
    nanoseconds: c_long,
}

#[repr(C)]
pub struct Timeval {
    seconds: i64,
    microseconds: i32,
}

#[repr(C)]
pub struct Timebase {
    numerator: u32,
    denominator: u32,
}

/// # Safety
/// `time` must be null or writable.
pub unsafe extern "C" fn clock_gettime(clock: c_uint, time: *mut Timespec) -> c_int {
    let Some(clock) = darwin::linux_clock(clock) else {
        set_errno(EINVAL);
        return -1;
    };
    match linux::clock_time(clock) {
        Ok([seconds, nanoseconds]) => {
            if !time.is_null() {
                // SAFETY: the caller's promise.
                unsafe {
                    time.write(Timespec {
                        seconds,
                        nanoseconds,
                    })
                };
            }
            0
        }
        Err(errno) => errno_from(errno, -1),
    }
}

pub extern "C" fn clock_gettime_nsec_np(clock: c_uint) -> u64 {
    darwin::linux_clock(clock)
        .and_then(|clock| linux::clock_time(clock).ok())
        .map_or(0, |[seconds, nanoseconds]| {
            (seconds as u64)
                .wrapping_mul(1_000_000_000)
                .wrapping_add(nanoseconds as u64)
        })
}

/// # Safety
/// `time` must be null or writable.
pub unsafe extern "C" fn gettimeofday(time: *mut Timeval, _zone: *mut c_void) -> c_int {
    let [seconds, nanoseconds] = linux::clock_time(0).unwrap_or([0, 0]);
    if !time.is_null() {
        // SAFETY: the caller's promise.
        unsafe {
            time.write(Timeval {
                seconds,
                microseconds: (nanoseconds / 1000) as i32,
            })
        };
    }
    0
}

/// # Safety
/// `out` must be null or writable.
pub unsafe extern "C" fn time(out: *mut i64) -> i64 {
    let [seconds, _] = linux::clock_time(0).unwrap_or([0, 0]);
    if !out.is_null() {
        // SAFETY: the caller's promise.
        unsafe { out.write(seconds) };
    }
    seconds
}

pub extern "C" fn mach_absolute_time() -> u64 {
    clock_gettime_nsec_np(6)
}

/// # Safety
/// `info` must be writable.
pub unsafe extern "C" fn mach_timebase_info(info: *mut Timebase) -> c_int {
    // SAFETY: the caller's promise.
    unsafe {
        info.write(Timebase {
            numerator: 1,
            denominator: 1,
        })
    };
    0
}

/// # Safety
/// `request` must be readable and `remaining` null or writable.
pub unsafe extern "C" fn nanosleep(request: *const Timespec, remaining: *mut Timespec) -> c_int {
    // SAFETY: the caller's promise.
    let request = unsafe { &*request };
    if request.nanoseconds < 0 || request.nanoseconds >= 1_000_000_000 || request.seconds < 0 {
        set_errno(EINVAL);
        return -1;
    }
    if !remaining.is_null() {
        // SAFETY: the caller's promise.
        unsafe {
            remaining.write(Timespec {
                seconds: 0,
                nanoseconds: 0,
            })
        };
    }
    linux::sleep([request.seconds, request.nanoseconds])
        .map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

pub extern "C" fn usleep(microseconds: c_uint) -> c_int {
    let microseconds = i64::from(microseconds);
    linux::sleep([microseconds / 1_000_000, microseconds % 1_000_000 * 1000])
        .map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

pub extern "C" fn sleep(seconds: c_uint) -> c_uint {
    let _ = linux::sleep([i64::from(seconds), 0]);
    0
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'Z' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

/// `strtoull`'s parsing: the value's magnitude, its sign, whether it overflowed and where the
/// number ended.
///
/// # Safety
/// `text` must be a NUL-terminated string.
unsafe fn parse_integer(text: *const c_char, base: c_int) -> (u64, bool, bool, *const c_char) {
    // SAFETY: the caller's promise.
    let bytes = unsafe { c_string(text) };
    let mut at = 0;
    while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
        at += 1;
    }
    let negative = bytes.get(at) == Some(&b'-');
    if matches!(bytes.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let mut base = base as u32;
    let hex_prefix = matches!(bytes.get(at..at + 2), Some([b'0', b'x' | b'X']))
        && bytes
            .get(at + 2)
            .and_then(|&b| digit_value(b))
            .is_some_and(|d| d < 16);
    if (base == 0 || base == 16) && hex_prefix {
        base = 16;
        at += 2;
    } else if base == 0 && bytes.get(at) == Some(&b'0') {
        base = 8;
    } else if base == 0 {
        base = 10;
    }
    if !(2..=36).contains(&base) {
        set_errno(EINVAL);
        return (0, false, false, text);
    }
    let start = at;
    let mut value: u64 = 0;
    let mut overflow = false;
    while let Some(digit) = bytes
        .get(at)
        .and_then(|&b| digit_value(b))
        .filter(|&d| d < base)
    {
        match value
            .checked_mul(u64::from(base))
            .and_then(|v| v.checked_add(u64::from(digit)))
        {
            Some(next) => value = next,
            None => overflow = true,
        }
        at += 1;
    }
    let end = if at == start {
        text
    } else {
        text.wrapping_add(at)
    };
    (value, negative, overflow, end)
}

/// # Safety
/// `text` must be a NUL-terminated string and `end` null or writable.
pub unsafe extern "C" fn strtoll(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_longlong {
    // SAFETY: the caller's promise.
    let (magnitude, negative, overflow, stop) = unsafe { parse_integer(text, base) };
    if !end.is_null() {
        // SAFETY: the caller's promise.
        unsafe { end.write(stop.cast_mut()) };
    }
    let limit = if negative {
        1u64 << 63
    } else {
        i64::MAX as u64
    };
    if overflow || magnitude > limit {
        set_errno(darwin::ERANGE);
        return if negative { i64::MIN } else { i64::MAX };
    }
    if negative {
        (magnitude as i64).wrapping_neg()
    } else {
        magnitude as i64
    }
}

/// # Safety
/// As `strtoll`.
pub unsafe extern "C" fn strtoull(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_ulonglong {
    // SAFETY: the caller's promise.
    let (magnitude, negative, overflow, stop) = unsafe { parse_integer(text, base) };
    if !end.is_null() {
        // SAFETY: the caller's promise.
        unsafe { end.write(stop.cast_mut()) };
    }
    if overflow {
        set_errno(darwin::ERANGE);
        return u64::MAX;
    }
    if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

/// # Safety
/// As `strtoll`.
pub unsafe extern "C" fn strtol(text: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long {
    // SAFETY: the caller's promise.
    unsafe { strtoll(text, end, base) }
}

/// # Safety
/// As `strtoll`.
pub unsafe extern "C" fn strtoul(
    text: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_ulong {
    // SAFETY: the caller's promise.
    unsafe { strtoull(text, end, base) }
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn atoi(text: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { strtoll(text, ptr::null_mut(), 10) as c_int }
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn atol(text: *const c_char) -> c_long {
    // SAFETY: the caller's promise.
    unsafe { strtoll(text, ptr::null_mut(), 10) }
}

pub extern "C" fn abs(value: c_int) -> c_int {
    value.wrapping_abs()
}

pub extern "C" fn labs(value: c_long) -> c_long {
    value.wrapping_abs()
}

static RANDOM: Exclusive<u64> = Exclusive::new(1);

pub extern "C" fn srand(seed: c_uint) {
    RANDOM.with(|state| *state = u64::from(seed));
}

pub extern "C" fn rand() -> c_int {
    RANDOM.with(|state| {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*state >> 33) & 0x7fff_ffff) as c_int
    })
}

/// # Safety
/// `buffer` must be writable for `length` bytes.
pub unsafe extern "C" fn arc4random_buf(buffer: *mut c_void, length: usize) {
    let mut filled = 0;
    while filled < length {
        // SAFETY: `getrandom` writes at most the rest of the caller's buffer.
        match unsafe {
            linux::syscall(
                linux::number::GETRANDOM,
                [buffer as usize + filled, length - filled, 0, 0, 0, 0],
            )
        } {
            Ok(0) | Err(_) => break,
            Ok(written) => filled += written,
        }
    }
}

pub extern "C" fn arc4random() -> u32 {
    let mut value = 0u32;
    // SAFETY: `value` is 4 writable bytes.
    unsafe { arc4random_buf((&raw mut value).cast(), 4) };
    value
}

pub extern "C" fn arc4random_uniform(bound: u32) -> u32 {
    if bound < 2 {
        return 0;
    }
    let floor = bound.wrapping_neg() % bound;
    loop {
        let value = arc4random();
        if value >= floor {
            return value % bound;
        }
    }
}

type Compare = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;

/// # Safety
/// `base` must hold `count` elements of `size` bytes, and `compare` must compare two of them.
pub unsafe extern "C" fn qsort(base: *mut c_void, count: usize, size: usize, compare: Compare) {
    if size == 0 {
        return;
    }
    let base = base.cast::<u8>();
    for sorted in 1..count {
        let mut at = sorted;
        while at > 0 {
            let (left, right) = (
                base.wrapping_add((at - 1) * size),
                base.wrapping_add(at * size),
            );
            // SAFETY: both are elements of the caller's array.
            if unsafe { compare(left.cast(), right.cast()) } <= 0 {
                break;
            }
            // SAFETY: two distinct elements of the array, swapped in place.
            unsafe { ptr::swap_nonoverlapping(left, right, size) };
            at -= 1;
        }
    }
}

/// # Safety
/// As `qsort`, with `key` an element to look for in the sorted array.
pub unsafe extern "C" fn bsearch(
    key: *const c_void,
    base: *const c_void,
    count: usize,
    size: usize,
    compare: Compare,
) -> *mut c_void {
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        let element = base.wrapping_byte_add(middle * size);
        // SAFETY: the caller's promise.
        match unsafe { compare(key, element) } {
            0 => return element.cast_mut(),
            order if order < 0 => high = middle,
            _ => low = middle + 1,
        }
    }
    ptr::null_mut()
}
