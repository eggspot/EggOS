//! The libSystem functions macOS console programs import, implemented natively on EggOS's
//! Linux system calls: each is an `extern "C"` function the program calls directly through its
//! bound pointers, with no translation in between. An import the layer lacks is bound to a stub
//! that names it and ends the program with status 127 when it is called, so a program whose
//! unused imports are missing still runs.

mod file;
mod heap;
mod process;
mod stdio;
mod string;
mod variadic;

use crate::darwin;
use crate::linux;
use core::ffi::{c_char, c_int, c_long, c_void};
use eggos_runtime::errno::Errno;

pub use process::{exit, set_process};
pub use stdio::set_up as set_up_streams;

/// The layer's process-wide state. Programs have one thread (no `pthread_create` yet), so
/// `with` needs no atomic read-modify-write; it ends the program if a call re-enters it, which
/// only a bug in the layer could do.
pub struct Exclusive<T> {
    busy: core::sync::atomic::AtomicBool,
    value: core::cell::UnsafeCell<T>,
}

// SAFETY: programs have one thread, and `busy` refuses a second reference to `value`.
unsafe impl<T: Send> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    pub const fn new(value: T) -> Self {
        Self {
            busy: core::sync::atomic::AtomicBool::new(false),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    pub fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        use core::sync::atomic::Ordering;
        if self.busy.load(Ordering::Acquire) {
            linux::exit(134);
        }
        self.busy.store(true, Ordering::Release);
        // SAFETY: `busy` was clear, so this is the only reference to the value.
        let result = work(unsafe { &mut *self.value.get() });
        self.busy.store(false, Ordering::Release);
        result
    }
}

static mut ERRNO: c_int = 0;

pub fn set_errno(value: c_int) {
    // SAFETY: the program's only thread owns `errno`.
    unsafe { ERRNO = value };
}

pub fn errno() -> c_int {
    // SAFETY: as above.
    unsafe { ERRNO }
}

/// Sets `errno` to Darwin's number for `errno` and returns `value`: how a failing function
/// ends.
pub fn errno_from<T>(errno: Errno, value: T) -> T {
    set_errno(darwin::darwin_errno(errno.0));
    value
}

extern "C" fn error_location() -> *mut c_int {
    &raw mut ERRNO
}

pub fn error_message(errno: c_int) -> &'static [u8] {
    match errno {
        0 => b"Undefined error: 0",
        1 => b"Operation not permitted",
        2 => b"No such file or directory",
        3 => b"No such process",
        4 => b"Interrupted system call",
        5 => b"Input/output error",
        9 => b"Bad file descriptor",
        12 => b"Cannot allocate memory",
        13 => b"Permission denied",
        17 => b"File exists",
        20 => b"Not a directory",
        21 => b"Is a directory",
        22 => b"Invalid argument",
        24 => b"Too many open files",
        25 => b"Inappropriate ioctl for device",
        28 => b"No space left on device",
        30 => b"Read-only file system",
        34 => b"Result too large",
        35 => b"Resource temporarily unavailable",
        63 => b"File name too long",
        66 => b"Directory not empty",
        78 => b"Function not implemented",
        _ => b"Unknown error",
    }
}

extern "C" fn strerror(errno: c_int) -> *const c_char {
    let message = error_message(errno);
    static mut BUFFER: [u8; 64] = [0; 64];
    let length = message.len().min(63);
    // SAFETY: the program's only thread owns the buffer, which `strerror` may overwrite.
    unsafe {
        let buffer = &mut *(&raw mut BUFFER);
        buffer[..length].copy_from_slice(&message[..length]);
        buffer[length] = 0;
        buffer.as_ptr().cast()
    }
}

/// The bytes of the NUL-terminated `text`, without the NUL.
///
/// # Safety
/// `text` must be a NUL-terminated string that outlives the result.
pub unsafe fn c_string<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: the caller's promise.
    unsafe { core::ffi::CStr::from_ptr(text) }.to_bytes()
}

/// The bytes of the NUL-terminated `text`, with the NUL.
///
/// # Safety
/// As [`c_string`].
pub unsafe fn c_string_with_nul<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: the caller's promise.
    unsafe { core::ffi::CStr::from_ptr(text) }.to_bytes_with_nul()
}

extern "C" fn malloc(size: usize) -> *mut c_void {
    heap::allocate(size, false).cast()
}

extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    match count.checked_mul(size) {
        Some(total) => heap::allocate(total, true).cast(),
        None => {
            set_errno(darwin::ENOMEM);
            core::ptr::null_mut()
        }
    }
}

extern "C" fn realloc(block: *mut c_void, size: usize) -> *mut c_void {
    heap::reallocate(block.cast(), size).cast()
}

extern "C" fn reallocf(block: *mut c_void, size: usize) -> *mut c_void {
    let grown = heap::reallocate(block.cast(), size);
    if grown.is_null() {
        heap::release(block.cast());
    }
    grown.cast()
}

extern "C" fn free(block: *mut c_void) {
    heap::release(block.cast());
}

extern "C" fn malloc_size(block: *const c_void) -> usize {
    heap::size_of(block.cast_mut().cast())
}

extern "C" fn posix_memalign(out: *mut *mut c_void, alignment: usize, size: usize) -> c_int {
    if !alignment.is_power_of_two() || alignment < size_of::<usize>() {
        return darwin::EINVAL;
    }
    let block = heap::allocate_aligned(alignment, size);
    if block.is_null() {
        return darwin::ENOMEM;
    }
    // SAFETY: the program passes a writable pointer.
    unsafe { out.write(block.cast()) };
    0
}

extern "C" fn aligned_alloc(alignment: usize, size: usize) -> *mut c_void {
    if !alignment.is_power_of_two() {
        set_errno(darwin::EINVAL);
        return core::ptr::null_mut();
    }
    heap::allocate_aligned(alignment, size).cast()
}

extern "C" fn getpagesize() -> c_int {
    linux::PAGE_SIZE as c_int
}

extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        29 => linux::PAGE_SIZE as c_long,
        57 | 58 => 1,
        _ => {
            set_errno(darwin::EINVAL);
            -1
        }
    }
}

extern "C" fn image_count() -> u32 {
    1
}

extern "C" fn image_header(index: u32) -> *const c_void {
    if index == 0 {
        // SAFETY: written once before the program runs.
        unsafe { IMAGE_HEADER as *const c_void }
    } else {
        core::ptr::null()
    }
}

extern "C" fn image_slide(index: u32) -> isize {
    if index == 0 {
        // SAFETY: as above.
        unsafe { IMAGE_SLIDE }
    } else {
        0
    }
}

static mut IMAGE_HEADER: usize = 0;
static mut IMAGE_SLIDE: isize = 0;

/// Records where the program's image is, for `_dyld_get_image_header` and its kin.
///
/// # Safety
/// Called once, before the program runs.
pub unsafe fn set_image(header: usize, slide: isize) {
    // SAFETY: the caller's promise.
    unsafe {
        IMAGE_HEADER = header;
        IMAGE_SLIDE = slide;
    }
}

macro_rules! functions {
    ($name:expr; $($symbol:literal => $function:path),* $(,)?) => {
        match $name {
            $($symbol => Some($function as *const () as usize),)*
            _ => None,
        }
    };
}

fn function(name: &[u8]) -> Option<usize> {
    functions![name;
        b"___error" => error_location,
        b"_strerror" => strerror,
        b"_malloc" => malloc,
        b"_calloc" => calloc,
        b"_realloc" => realloc,
        b"_reallocf" => reallocf,
        b"_free" => free,
        b"_malloc_size" => malloc_size,
        b"_posix_memalign" => posix_memalign,
        b"_aligned_alloc" => aligned_alloc,
        b"_valloc" => malloc,
        b"_getpagesize" => getpagesize,
        b"_sysconf" => sysconf,
        b"__dyld_image_count" => image_count,
        b"__dyld_get_image_header" => image_header,
        b"__dyld_get_image_vmaddr_slide" => image_slide,
        b"_strlen" => string::strlen,
        b"_strnlen" => string::strnlen,
        b"_strcmp" => string::strcmp,
        b"_strncmp" => string::strncmp,
        b"_strcasecmp" => string::strcasecmp,
        b"_strncasecmp" => string::strncasecmp,
        b"_strcpy" => string::strcpy,
        b"_strncpy" => string::strncpy,
        b"_strcat" => string::strcat,
        b"_strncat" => string::strncat,
        b"_strlcpy" => string::strlcpy,
        b"_strchr" => string::strchr,
        b"_strrchr" => string::strrchr,
        b"_strstr" => string::strstr,
        b"_strdup" => string::strdup,
        b"_strndup" => string::strndup,
        b"_memchr" => string::memchr,
        b"_memcmp" => string::memcmp,
        b"_bcmp" => string::memcmp,
        b"_memcpy" => string::memcpy,
        b"_memmove" => string::memmove,
        b"_memset" => string::memset,
        b"_bzero" => string::bzero,
        b"___bzero" => string::bzero,
        b"___memcpy_chk" => string::memcpy_chk,
        b"___memmove_chk" => string::memmove_chk,
        b"___memset_chk" => string::memset_chk,
        b"___strcpy_chk" => string::strcpy_chk,
        b"___strncpy_chk" => string::strncpy_chk,
        b"___strcat_chk" => string::strcat_chk,
        b"_printf" => stdio::printf,
        b"_fprintf" => stdio::fprintf,
        b"_dprintf" => stdio::dprintf,
        b"_sprintf" => stdio::sprintf,
        b"_snprintf" => stdio::snprintf,
        b"_asprintf" => stdio::asprintf,
        b"___sprintf_chk" => stdio::sprintf_chk,
        b"___snprintf_chk" => stdio::snprintf_chk,
        b"_vprintf" => stdio::vprintf,
        b"_vfprintf" => stdio::vfprintf,
        b"_vdprintf" => stdio::vdprintf,
        b"_vsprintf" => stdio::vsprintf,
        b"_vsnprintf" => stdio::vsnprintf,
        b"_vasprintf" => stdio::vasprintf,
        b"___vsnprintf_chk" => stdio::vsnprintf_chk,
        b"_puts" => stdio::puts,
        b"_fputs" => stdio::fputs,
        b"_fputc" => stdio::fputc,
        b"_putc" => stdio::fputc,
        b"_putchar" => stdio::putchar,
        b"_fwrite" => stdio::fwrite,
        b"_fread" => stdio::fread,
        b"_fgetc" => stdio::fgetc,
        b"_getc" => stdio::fgetc,
        b"_getchar" => stdio::getchar,
        b"_ungetc" => stdio::ungetc,
        b"_fgets" => stdio::fgets,
        b"_fflush" => stdio::fflush,
        b"_fopen" => stdio::fopen,
        b"_fdopen" => stdio::fdopen,
        b"_fclose" => stdio::fclose,
        b"_fileno" => stdio::fileno,
        b"_feof" => stdio::feof,
        b"_ferror" => stdio::ferror,
        b"_clearerr" => stdio::clearerr,
        b"_fseek" => stdio::fseek,
        b"_fseeko" => stdio::fseeko,
        b"_ftell" => stdio::ftell,
        b"_ftello" => stdio::ftello,
        b"_rewind" => stdio::rewind,
        b"_setvbuf" => stdio::setvbuf,
        b"_perror" => stdio::perror,
        b"_open" => file::open,
        b"_close" => file::close,
        b"_read" => file::read,
        b"_write" => file::write,
        b"_lseek" => file::lseek,
        b"_stat" => file::stat,
        b"_lstat" => file::lstat,
        b"_fstat" => file::fstat,
        b"_isatty" => file::isatty,
        b"_access" => file::access,
        b"_getcwd" => file::getcwd,
        b"_chdir" => file::chdir,
        b"_unlink" => file::unlink,
        b"_rmdir" => file::rmdir,
        b"_mkdir" => file::mkdir,
        b"_opendir" => file::opendir,
        b"_readdir" => file::readdir,
        b"_closedir" => file::closedir,
        b"_dirfd" => file::dirfd,
        b"_exit" => process::exit,
        b"__exit" => process::exit_now,
        b"__Exit" => process::exit_now,
        b"_abort" => process::abort,
        b"_atexit" => process::atexit,
        b"___stack_chk_fail" => process::stack_chk_fail,
        b"_raise" => process::raise,
        b"_kill" => process::kill,
        b"_getpid" => process::getpid,
        b"_getppid" => process::getppid,
        b"_getuid" => process::getuid,
        b"_geteuid" => process::getuid,
        b"_getenv" => process::getenv,
        b"_setenv" => process::setenv,
        b"_putenv" => process::putenv,
        b"_unsetenv" => process::unsetenv,
        b"__NSGetArgc" => process::ns_get_argc,
        b"__NSGetArgv" => process::ns_get_argv,
        b"__NSGetEnviron" => process::ns_get_environ,
        b"__NSGetProgname" => process::ns_get_progname,
        b"_clock_gettime" => process::clock_gettime,
        b"_clock_gettime_nsec_np" => process::clock_gettime_nsec_np,
        b"_gettimeofday" => process::gettimeofday,
        b"_time" => process::time,
        b"_mach_absolute_time" => process::mach_absolute_time,
        b"_mach_continuous_time" => process::mach_absolute_time,
        b"_mach_timebase_info" => process::mach_timebase_info,
        b"_nanosleep" => process::nanosleep,
        b"_usleep" => process::usleep,
        b"_sleep" => process::sleep,
        b"_atoi" => process::atoi,
        b"_atol" => process::atol,
        b"_atoll" => process::atol,
        b"_strtol" => process::strtol,
        b"_strtoll" => process::strtoll,
        b"_strtoul" => process::strtoul,
        b"_strtoull" => process::strtoull,
        b"_abs" => process::abs,
        b"_labs" => process::labs,
        b"_llabs" => process::labs,
        b"_rand" => process::rand,
        b"_srand" => process::srand,
        b"_arc4random" => process::arc4random,
        b"_arc4random_buf" => process::arc4random_buf,
        b"_arc4random_uniform" => process::arc4random_uniform,
        b"_qsort" => process::qsort,
        b"_bsearch" => process::bsearch,
    ]
}

fn data(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"___stdinp" => &raw const stdio::STDIN_POINTER as usize,
        b"___stdoutp" => &raw const stdio::STDOUT_POINTER as usize,
        b"___stderrp" => &raw const stdio::STDERR_POINTER as usize,
        b"___stack_chk_guard" => &raw const process::STACK_GUARD as usize,
        b"_environ" => &raw const process::ENVIRON as usize,
        _ => return None,
    })
}

/// The address of the libSystem function or variable `name` (a Mach-O symbol name, with its
/// leading underscore), if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let name = darwin::base_symbol(name);
    data(name)
        .or_else(|| function(name))
        .map(|address| address as u64)
}

/// How many missing functions a program may import.
pub const MISSING_STUBS: usize = 256;

static MISSING_NAMES: Exclusive<[(usize, usize); MISSING_STUBS]> =
    Exclusive::new([(0, 0); MISSING_STUBS]);

extern "C" fn missing<const INDEX: usize>() -> ! {
    let (address, length) = MISSING_NAMES.with(|names| names[INDEX]);
    // SAFETY: `bind_missing` stored a name from the program's file, which stays mapped.
    let name = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    let name = name.strip_prefix(b"_").unwrap_or(name);
    stdio::flush_all();
    let _ = linux::write_all(2, b"eggos-macos: unimplemented libSystem function ");
    let _ = linux::write_all(2, name);
    let _ = linux::write_all(2, b"\n");
    linux::exit(127)
}

macro_rules! missing_row {
    ($row:literal) => {
        [
            missing::<{ $row * 16 }> as *const () as usize,
            missing::<{ $row * 16 + 1 }> as *const () as usize,
            missing::<{ $row * 16 + 2 }> as *const () as usize,
            missing::<{ $row * 16 + 3 }> as *const () as usize,
            missing::<{ $row * 16 + 4 }> as *const () as usize,
            missing::<{ $row * 16 + 5 }> as *const () as usize,
            missing::<{ $row * 16 + 6 }> as *const () as usize,
            missing::<{ $row * 16 + 7 }> as *const () as usize,
            missing::<{ $row * 16 + 8 }> as *const () as usize,
            missing::<{ $row * 16 + 9 }> as *const () as usize,
            missing::<{ $row * 16 + 10 }> as *const () as usize,
            missing::<{ $row * 16 + 11 }> as *const () as usize,
            missing::<{ $row * 16 + 12 }> as *const () as usize,
            missing::<{ $row * 16 + 13 }> as *const () as usize,
            missing::<{ $row * 16 + 14 }> as *const () as usize,
            missing::<{ $row * 16 + 15 }> as *const () as usize,
        ]
    };
}

fn missing_stub(index: usize) -> usize {
    let rows: [[usize; 16]; 16] = [
        missing_row!(0),
        missing_row!(1),
        missing_row!(2),
        missing_row!(3),
        missing_row!(4),
        missing_row!(5),
        missing_row!(6),
        missing_row!(7),
        missing_row!(8),
        missing_row!(9),
        missing_row!(10),
        missing_row!(11),
        missing_row!(12),
        missing_row!(13),
        missing_row!(14),
        missing_row!(15),
    ];
    rows[index / 16][index % 16]
}

/// The address of stub number `index` (below [`MISSING_STUBS`]), which reports `name` as an
/// unimplemented function when the program calls it.
pub fn bind_missing(index: usize, name: &'static [u8]) -> u64 {
    MISSING_NAMES.with(|names| names[index] = (name.as_ptr() as usize, name.len()));
    missing_stub(index) as u64
}
