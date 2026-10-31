//! The libSystem functions macOS console programs import, implemented natively on EggOS's
//! Linux system calls: each is an `extern "C"` function the program calls directly through its
//! bound pointers, with no translation in between. An import the layer lacks is bound to a stub
//! that names it and ends the program with status 127 when it is called, so a program whose
//! unused imports are missing still runs.

mod children;
pub mod dynamic;
mod file;
pub mod heap;
mod locale;
mod memory;
mod multibyte;
mod options;
pub mod process;
mod scanf;
mod signal;
pub mod stdio;
mod string;
pub mod sync;
pub mod thread;
mod time;
pub mod variadic;

use crate::darwin;
use crate::linux;
use core::ffi::{c_char, c_int, c_long, c_void};
use eggos_runtime::errno::Errno;

pub use dynamic::set_image;
pub use process::{abort, exit, set_process};
pub use stdio::set_up as set_up_streams;
pub use thread::{set_thread_local_template, set_up_main_thread};

/// The layer's process-wide state, behind a lock that any of the program's threads may take;
/// `with` ends the program if the thread that holds it re-enters it, which only a bug in the
/// layer could do.
pub struct Exclusive<T> {
    lock: core::sync::atomic::AtomicU32,
    owner: core::sync::atomic::AtomicUsize,
    value: core::cell::UnsafeCell<T>,
}

// SAFETY: the lock gives one thread at a time a reference to `value`.
unsafe impl<T: Send> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    pub const fn new(value: T) -> Self {
        Self {
            lock: core::sync::atomic::AtomicU32::new(0),
            owner: core::sync::atomic::AtomicUsize::new(0),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    pub fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        use core::sync::atomic::Ordering;
        let me = thread::current() as usize;
        if self.owner.load(Ordering::Relaxed) == me {
            linux::exit(134);
        }
        sync::lock(&self.lock);
        self.owner.store(me, Ordering::Relaxed);
        // SAFETY: the lock is held, so this is the only reference to the value.
        let result = work(unsafe { &mut *self.value.get() });
        self.owner.store(0, Ordering::Relaxed);
        sync::unlock(&self.lock);
        result
    }
}

/// Writes `bytes` to standard error, unbuffered.
pub fn write_error(bytes: &[u8]) {
    let _ = linux::write_all(2, bytes);
}

pub fn set_errno(value: c_int) {
    // SAFETY: each thread's `errno` is its own.
    unsafe { *thread::errno_location() = value };
}

pub fn errno() -> c_int {
    // SAFETY: as above.
    unsafe { *thread::errno_location() }
}

/// Sets `errno` to Darwin's number for `errno` and returns `value`: how a failing function
/// ends.
pub fn errno_from<T>(errno: Errno, value: T) -> T {
    set_errno(darwin::darwin_errno(errno.0));
    value
}

extern "C" fn error_location() -> *mut c_int {
    thread::errno_location()
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
        11 => b"Resource deadlock avoided",
        16 => b"Resource busy",
        34 => b"Result too large",
        35 => b"Resource temporarily unavailable",
        60 => b"Operation timed out",
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
    let buffer = unsafe { core::slice::from_raw_parts_mut((&raw mut BUFFER).cast::<u8>(), 64) };
    buffer[..length].copy_from_slice(&message[..length]);
    buffer[length] = 0;
    buffer.as_ptr().cast()
}

/// The CPU features Linux's auxiliary vector reports, which compiler-builtins' aarch64 atomics
/// ask for: none, so they use load-exclusive and store-exclusive pairs, which every Armv8 CPU
/// has.
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
pub extern "C" fn getauxval(_: core::ffi::c_ulong) -> core::ffi::c_ulong {
    0
}

unsafe extern "C" fn strerror_r(errno: c_int, buffer: *mut c_char, length: usize) -> c_int {
    let message = error_message(errno);
    if length <= message.len() {
        return darwin::ERANGE;
    }
    // SAFETY: the caller's promise; the message and its NUL fit.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), message.len() + 1) };
    out[..message.len()].copy_from_slice(message);
    out[message.len()] = 0;
    0
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

extern "C" fn valloc(size: usize) -> *mut c_void {
    heap::allocate_aligned(linux::PAGE_SIZE, size).cast()
}

extern "C" fn getpagesize() -> c_int {
    linux::PAGE_SIZE as c_int
}

extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        29 => linux::PAGE_SIZE as c_long,
        1 => 1 << 20,
        3 => 100,
        5 => 256,
        57 | 58 => linux::cpu_count() as c_long,
        _ => {
            set_errno(darwin::EINVAL);
            -1
        }
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
        b"_valloc" => valloc,
        b"_getpagesize" => getpagesize,
        b"_sysconf" => sysconf,
        b"__dyld_image_count" => dynamic::image_count,
        b"__dyld_get_image_header" => dynamic::image_header,
        b"__dyld_get_image_vmaddr_slide" => dynamic::image_slide,
        b"__dyld_get_image_name" => dynamic::image_name,
        b"__NSGetExecutablePath" => dynamic::ns_get_executable_path,
        b"_dlsym" => dynamic::dlsym,
        b"_dlopen" => dynamic::dlopen,
        b"_dlclose" => dynamic::dlclose,
        b"_dlerror" => dynamic::dlerror,
        b"__tlv_bootstrap" => thread::tlv_get_address,
        b"__tlv_atexit" => thread::tlv_atexit,
        b"_pthread_create" => thread::pthread_create,
        b"_pthread_join" => thread::pthread_join,
        b"_pthread_detach" => thread::pthread_detach,
        b"_pthread_exit" => thread::pthread_exit,
        b"_pthread_self" => thread::pthread_self,
        b"_pthread_equal" => thread::pthread_equal,
        b"_pthread_main_np" => thread::pthread_main_np,
        b"_pthread_threadid_np" => thread::pthread_threadid_np,
        b"_pthread_mach_thread_np" => thread::pthread_mach_thread_np,
        b"_pthread_get_stackaddr_np" => thread::pthread_get_stackaddr_np,
        b"_pthread_get_stacksize_np" => thread::pthread_get_stacksize_np,
        b"_pthread_setname_np" => thread::pthread_setname_np,
        b"_pthread_getname_np" => thread::pthread_getname_np,
        b"_pthread_attr_init" => thread::pthread_attr_init,
        b"_pthread_attr_destroy" => thread::pthread_attr_destroy,
        b"_pthread_attr_setstacksize" => thread::pthread_attr_setstacksize,
        b"_pthread_attr_getstacksize" => thread::pthread_attr_getstacksize,
        b"_pthread_attr_setdetachstate" => thread::pthread_attr_setdetachstate,
        b"_pthread_attr_getdetachstate" => thread::pthread_attr_getdetachstate,
        b"_pthread_attr_setguardsize" => thread::pthread_attr_setguardsize,
        b"_pthread_key_create" => thread::pthread_key_create,
        b"_pthread_key_delete" => thread::pthread_key_delete,
        b"_pthread_getspecific" => thread::pthread_getspecific,
        b"_pthread_setspecific" => thread::pthread_setspecific,
        b"_sched_yield" => thread::sched_yield,
        b"_pthread_mutexattr_init" => sync::pthread_mutexattr_init,
        b"_pthread_mutexattr_destroy" => sync::pthread_mutexattr_destroy,
        b"_pthread_mutexattr_settype" => sync::pthread_mutexattr_settype,
        b"_pthread_mutex_init" => sync::pthread_mutex_init,
        b"_pthread_mutex_destroy" => sync::pthread_mutex_destroy,
        b"_pthread_mutex_lock" => sync::pthread_mutex_lock,
        b"_pthread_mutex_trylock" => sync::pthread_mutex_trylock,
        b"_pthread_mutex_unlock" => sync::pthread_mutex_unlock,
        b"_pthread_cond_init" => sync::pthread_cond_init,
        b"_pthread_cond_destroy" => sync::pthread_cond_destroy,
        b"_pthread_cond_wait" => sync::pthread_cond_wait,
        b"_pthread_cond_timedwait" => sync::pthread_cond_timedwait,
        b"_pthread_cond_signal" => sync::pthread_cond_signal,
        b"_pthread_cond_broadcast" => sync::pthread_cond_broadcast,
        b"_pthread_once" => sync::pthread_once,
        b"_dispatch_semaphore_create" => sync::dispatch_semaphore_create,
        b"_dispatch_semaphore_wait" => sync::dispatch_semaphore_wait,
        b"_dispatch_semaphore_signal" => sync::dispatch_semaphore_signal,
        b"_dispatch_retain" => sync::dispatch_retain,
        b"_dispatch_release" => sync::dispatch_release,
        b"_dispatch_time" => sync::dispatch_time,
        b"_sigaction" => signal::sigaction,
        b"_signal" => signal::signal,
        b"_sigaltstack" => signal::sigaltstack,
        b"_sigprocmask" => signal::sigprocmask,
        b"_pthread_sigmask" => signal::pthread_sigmask,
        b"___sigaction" => signal::trampoline_sigaction,
        b"_sigsuspend" => signal::sigsuspend,
        b"_sigpending" => signal::sigpending,
        b"_sigemptyset" => signal::sigemptyset,
        b"_sigfillset" => signal::sigfillset,
        b"_sigaddset" => signal::sigaddset,
        b"_sigdelset" => signal::sigdelset,
        b"_sigismember" => signal::sigismember,
        b"_pause" => signal::pause,
        b"_mmap" => memory::mmap,
        b"_munmap" => memory::munmap,
        b"_mprotect" => memory::mprotect,
        b"_madvise" => memory::madvise,
        b"_getopt" => options::getopt,
        b"_getopt_long" => options::getopt_long,
        b"_getopt_long_only" => options::getopt_long_only,
        b"dyld_stub_binder" => dynamic::stub_binder,
        b"_setlocale" => locale::setlocale,
        b"_nl_langinfo" => locale::nl_langinfo,
        b"_localeconv" => locale::localeconv,
        b"____mb_cur_max" => locale::mb_cur_max,
        b"___maskrune" => locale::maskrune,
        b"___runetype" => locale::runetype,
        b"___toupper" => locale::to_upper,
        b"___tolower" => locale::to_lower,
        b"_toupper" => locale::to_upper,
        b"_tolower" => locale::to_lower,
        b"_towupper" => locale::to_upper,
        b"_towlower" => locale::to_lower,
        b"_wctype" => locale::wctype,
        b"_mbrtowc" => multibyte::mbrtowc,
        b"_mbrlen" => multibyte::mbrlen,
        b"_mbsinit" => multibyte::mbsinit,
        b"_mbtowc" => multibyte::mbtowc,
        b"_mblen" => multibyte::mblen,
        b"_wcrtomb" => multibyte::wcrtomb,
        b"_wctomb" => multibyte::wctomb,
        b"_btowc" => multibyte::btowc,
        b"_wctob" => multibyte::wctob,
        b"_mbsrtowcs" => multibyte::mbsrtowcs,
        b"_wcsrtombs" => multibyte::wcsrtombs,
        b"_mbstowcs" => multibyte::mbstowcs,
        b"_wcstombs" => multibyte::wcstombs,
        b"_gmtime_r" => time::gmtime_r,
        b"_localtime_r" => time::localtime_r,
        b"_gmtime" => time::gmtime,
        b"_localtime" => time::localtime,
        b"_timegm" => time::timegm,
        b"_mktime" => time::mktime,
        b"_strftime" => time::strftime,
        b"_strptime" => time::strptime,
        b"_tzset" => time::tzset,
        b"_strerror_r" => strerror_r,
        b"_dup" => file::dup,
        b"_dup2" => file::dup2,
        b"_pipe" => file::pipe,
        b"_fcntl" => file::fcntl,
        b"_writev" => file::writev,
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
        b"_strspn" => string::strspn,
        b"_strcspn" => string::strcspn,
        b"_strpbrk" => string::strpbrk,
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
        b"_scanf" => scanf::scanf,
        b"_fscanf" => scanf::fscanf,
        b"_sscanf" => scanf::sscanf,
        b"_vscanf" => scanf::vscanf,
        b"_vfscanf" => scanf::vfscanf,
        b"_vsscanf" => scanf::vsscanf,
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
        b"___chkstk_darwin" => process::eggos_macos_chkstk_darwin,
        b"____chkstk_darwin" => process::eggos_macos_chkstk_darwin,
        b"_raise" => process::raise,
        b"_kill" => process::kill,
        b"_killpg" => process::killpg,
        b"_fork" => children::fork,
        b"_vfork" => children::fork,
        b"_execve" => children::execve,
        b"_execv" => children::execv,
        b"_execvp" => children::execvp,
        b"_execvP" => children::execv_searching,
        b"_execl" => children::execl,
        b"_execle" => children::execle,
        b"_execlp" => children::execlp,
        b"_posix_spawn" => children::posix_spawn,
        b"_posix_spawnp" => children::posix_spawnp,
        b"_posix_spawn_file_actions_init" => children::file_actions_init,
        b"_posix_spawn_file_actions_destroy" => children::file_actions_destroy,
        b"_posix_spawn_file_actions_addopen" => children::file_actions_add_open,
        b"_posix_spawn_file_actions_addclose" => children::file_actions_add_close,
        b"_posix_spawn_file_actions_adddup2" => children::file_actions_add_dup2,
        b"_posix_spawn_file_actions_addinherit_np" => children::file_actions_add_inherit,
        b"_posix_spawn_file_actions_addchdir_np" => children::file_actions_add_chdir,
        b"_posix_spawn_file_actions_addfchdir_np" => children::file_actions_add_fchdir,
        b"_posix_spawn_file_actions_addchdir" => children::file_actions_add_chdir,
        b"_posix_spawn_file_actions_addfchdir" => children::file_actions_add_fchdir,
        b"_posix_spawnattr_init" => children::attributes_init,
        b"_posix_spawnattr_destroy" => children::attributes_destroy,
        b"_posix_spawnattr_setflags" => children::attributes_set_flags,
        b"_posix_spawnattr_getflags" => children::attributes_get_flags,
        b"_posix_spawnattr_setpgroup" => children::attributes_set_process_group,
        b"_posix_spawnattr_getpgroup" => children::attributes_get_process_group,
        b"_posix_spawnattr_setsigmask" => children::attributes_set_mask,
        b"_posix_spawnattr_getsigmask" => children::attributes_get_mask,
        b"_posix_spawnattr_setsigdefault" => children::attributes_set_default_signals,
        b"_posix_spawnattr_getsigdefault" => children::attributes_get_default_signals,
        b"_wait" => children::wait,
        b"_waitpid" => children::waitpid,
        b"_wait3" => children::wait3,
        b"_wait4" => children::wait4,
        b"_popen" => children::popen,
        b"_pclose" => children::pclose,
        b"_system" => children::system,
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
        b"_optarg" => &raw const options::OPTARG as usize,
        b"_optind" => &raw const options::OPTIND as usize,
        b"_opterr" => &raw const options::OPTERR as usize,
        b"_optopt" => &raw const options::OPTOPT as usize,
        b"_optreset" => &raw const options::OPTRESET as usize,
        b"_environ" => &raw const process::ENVIRON as usize,
        b"___mb_cur_max" => &raw const locale::MB_CUR_MAX as usize,
        b"__DefaultRuneLocale" => &raw const locale::DEFAULT_RUNE_LOCALE as usize,
        _ => return None,
    })
}

/// The address of the libSystem function or variable `name` (a Mach-O symbol name, with its
/// leading underscore), if the layer has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let name = darwin::bound_symbol(name, cfg!(target_arch = "x86_64"))?;
    data(name)
        .or_else(|| function(name))
        .map(|address| address as u64)
}

/// How many missing functions a program may import.
pub const MISSING_STUBS: usize = 256;

static MISSING_NAMES: Exclusive<[(&'static [u8], &'static [u8]); MISSING_STUBS]> =
    Exclusive::new([(b"", b""); MISSING_STUBS]);

extern "C" fn missing<const INDEX: usize>() -> ! {
    let (library, name) = MISSING_NAMES.with(|names| names[INDEX]);
    let name = name.strip_prefix(b"_").unwrap_or(name);
    stdio::flush_all();
    let _ = linux::write_all(2, b"eggos-macos: unimplemented ");
    let _ = linux::write_all(2, library);
    let _ = linux::write_all(2, b" function ");
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
/// unimplemented function of `library` (its short name) when the program calls it.
pub fn bind_missing(index: usize, library: &'static [u8], name: &'static [u8]) -> u64 {
    MISSING_NAMES.with(|names| names[index] = (library, name));
    missing_stub(index) as u64
}
