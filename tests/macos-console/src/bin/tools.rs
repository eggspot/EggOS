//! A small macOS console tool built from our own source for the `*-apple-darwin` targets and
//! linked by Apple's linker against libSystem, that the macOS programs boot test runs from
//! `eggsh` (D123). It parses its options with `getopt_long`, lists the directory `--list`
//! names with `opendir` and `readdir`, formats times with `gmtime_r`, `localtime_r`, `timegm`
//! and `strftime`, sleeps with `nanosleep`, reads `errno` through `__error` and `strerror`,
//! and asks the C locale through `setlocale`, `nl_langinfo` and `localeconv`. An unknown
//! option ends it with status 2 after `getopt`'s message.

#![cfg_attr(all(target_os = "macos", not(test)), no_std, no_main)]

#[cfg(all(target_os = "macos", not(test)))]
mod program {
    use core::ffi::{CStr, c_char, c_int, c_long, c_void};

    const NO_ARGUMENT: c_int = 0;
    const REQUIRED_ARGUMENT: c_int = 1;
    const LONG_LIST: c_int = 0x100;
    const LONG_NAME: c_int = 0x101;
    const LC_ALL: c_int = 0;
    const CODESET: c_int = 0;
    const O_RDONLY: c_int = 0;
    const CLOCK_MONOTONIC: c_int = 6;
    const MAX_NAMES: usize = 16;

    #[repr(C)]
    struct LongOption {
        name: *const c_char,
        has_arg: c_int,
        flag: *mut c_int,
        value: c_int,
    }

    #[repr(C)]
    struct Directory {
        opaque: [u8; 0],
    }

    #[repr(C)]
    struct DirectoryEntry {
        inode: u64,
        seek_offset: u64,
        record_length: u16,
        name_length: u16,
        kind: u8,
        name: [c_char; 1024],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Tm {
        fields: [c_int; 9],
        offset: c_long,
        zone: *const c_char,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Timespec {
        seconds: i64,
        nanoseconds: c_long,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        seconds: i64,
        microseconds: i32,
    }

    #[link(name = "System")]
    unsafe extern "C" {
        static mut optarg: *mut c_char;
        static mut optind: c_int;
        fn getopt_long(
            count: c_int,
            arguments: *const *mut c_char,
            spec: *const c_char,
            long: *const LongOption,
            index: *mut c_int,
        ) -> c_int;
        fn printf(format: *const c_char, ...) -> c_int;
        #[cfg_attr(target_arch = "x86_64", link_name = "opendir$INODE64")]
        fn opendir(path: *const c_char) -> *mut Directory;
        #[cfg_attr(target_arch = "x86_64", link_name = "readdir$INODE64")]
        fn readdir(directory: *mut Directory) -> *mut DirectoryEntry;
        fn closedir(directory: *mut Directory) -> c_int;
        fn strcmp(left: *const c_char, right: *const c_char) -> c_int;
        fn strdup(text: *const c_char) -> *mut c_char;
        fn qsort(
            base: *mut c_void,
            count: usize,
            size: usize,
            compare: extern "C" fn(*const c_void, *const c_void) -> c_int,
        );
        fn gmtime_r(seconds: *const i64, out: *mut Tm) -> *mut Tm;
        fn localtime_r(seconds: *const i64, out: *mut Tm) -> *mut Tm;
        fn timegm(time: *mut Tm) -> i64;
        fn strftime(
            buffer: *mut c_char,
            size: usize,
            format: *const c_char,
            time: *const Tm,
        ) -> usize;
        fn time(out: *mut i64) -> i64;
        fn gettimeofday(time: *mut Timeval, zone: *mut c_void) -> c_int;
        fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
        fn nanosleep(request: *const Timespec, remaining: *mut Timespec) -> c_int;
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn __error() -> *mut c_int;
        fn strerror(errno: c_int) -> *const c_char;
        fn setlocale(category: c_int, locale: *const c_char) -> *const c_char;
        fn nl_langinfo(item: c_int) -> *const c_char;
        fn localeconv() -> *const *const c_char;
        fn abort() -> !;
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: `abort` takes no arguments and ends the process.
        unsafe { abort() }
    }

    extern "C" fn by_name(left: *const c_void, right: *const c_void) -> c_int {
        // SAFETY: `qsort` passes pointers to two of the array's string pointers.
        let left = unsafe { *left.cast::<*const c_char>() };
        // SAFETY: as above.
        let right = unsafe { *right.cast::<*const c_char>() };
        // SAFETY: both are NUL-terminated copies `strdup` made.
        unsafe { strcmp(left, right) }
    }

    fn list(path: *const c_char) {
        // SAFETY: a NUL-terminated path.
        let directory = unsafe { opendir(path) };
        if directory.is_null() {
            // SAFETY: a format and a NUL-terminated path.
            unsafe { printf(c"MACOS-TOOLS dir %s: cannot open\n".as_ptr(), path) };
            return;
        }
        let mut names = [core::ptr::null_mut::<c_char>(); MAX_NAMES];
        let mut count = 0;
        loop {
            // SAFETY: an open directory.
            let entry = unsafe { readdir(directory) };
            if entry.is_null() {
                break;
            }
            // SAFETY: `readdir` returns a valid entry with a NUL-terminated name.
            let name = unsafe { (*entry).name.as_ptr() };
            // SAFETY: as above.
            let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
            if bytes != b"." && bytes != b".." && count < MAX_NAMES {
                // SAFETY: as above.
                names[count] = unsafe { strdup(name) };
                count += 1;
            }
        }
        // SAFETY: the directory `opendir` opened.
        unsafe { closedir(directory) };
        // SAFETY: `names` holds `count` string pointers.
        unsafe {
            qsort(
                names.as_mut_ptr().cast(),
                count,
                size_of::<*mut c_char>(),
                by_name,
            )
        };
        // SAFETY: a format and a NUL-terminated path.
        unsafe { printf(c"MACOS-TOOLS dir %s:".as_ptr(), path) };
        for name in &names[..count] {
            // SAFETY: a format and a NUL-terminated copy.
            unsafe { printf(c" %s".as_ptr(), *name) };
        }
        // SAFETY: a format.
        unsafe { printf(c"\n".as_ptr()) };
    }

    fn times() {
        let mut buffer = [0 as c_char; 96];
        let format = c"%Y-%m-%d %H:%M:%S %a %b %j %Z".as_ptr();
        let mut tm = Tm {
            fields: [0; 9],
            offset: 0,
            zone: core::ptr::null(),
        };
        let epoch = 0i64;
        // SAFETY: a readable time and a writable `struct tm`.
        unsafe { localtime_r(&epoch, &mut tm) };
        // SAFETY: a writable buffer of its size, a format and a `struct tm`.
        unsafe { strftime(buffer.as_mut_ptr(), buffer.len(), format, &tm) };
        // SAFETY: a format and the NUL-terminated result.
        unsafe { printf(c"MACOS-TOOLS time %s".as_ptr(), buffer.as_ptr()) };
        let later = 1_700_000_000i64;
        // SAFETY: as above.
        unsafe { gmtime_r(&later, &mut tm) };
        // SAFETY: as above.
        unsafe { strftime(buffer.as_mut_ptr(), buffer.len(), c"%F %T %A".as_ptr(), &tm) };
        // SAFETY: as above.
        unsafe { printf(c" | %s".as_ptr(), buffer.as_ptr()) };
        tm.fields[4] += 2;
        // SAFETY: a `struct tm` to normalize.
        let shifted = unsafe { timegm(&mut tm) };
        let round_trip = shifted - later == 61 * 86_400;
        let mut before = Timespec::default();
        let mut after = Timespec::default();
        let pause = Timespec {
            seconds: 0,
            nanoseconds: 2_000_000,
        };
        let mut day = Timeval::default();
        // SAFETY: a writable `struct timespec`.
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut before) };
        // SAFETY: a readable request.
        let slept = unsafe { nanosleep(&pause, core::ptr::null_mut()) };
        // SAFETY: as above.
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut after) };
        // SAFETY: a writable `struct timeval`.
        unsafe { gettimeofday(&mut day, core::ptr::null_mut()) };
        let elapsed = (after.seconds - before.seconds) * 1_000_000_000
            + (after.nanoseconds - before.nanoseconds);
        // SAFETY: `time` accepts a null pointer.
        let now = unsafe { time(core::ptr::null_mut()) };
        let checks = [
            (round_trip, c"timegm"),
            (slept == 0 && elapsed >= 2_000_000, c"nanosleep"),
            ((now - day.seconds).abs() <= 1, c"now"),
        ];
        for (passed, name) in checks {
            let shown = if passed { name } else { c"FAILED" };
            // SAFETY: a format and a NUL-terminated name.
            unsafe { printf(c" %s".as_ptr(), shown.as_ptr()) };
        }
        // SAFETY: a format.
        unsafe { printf(c"\n".as_ptr()) };
    }

    fn errors() {
        // SAFETY: a NUL-terminated path.
        let fd = unsafe { open(c"/no/such/file".as_ptr(), O_RDONLY) };
        // SAFETY: `__error` returns the calling thread's `errno`.
        let location = unsafe { __error() };
        // SAFETY: it points to the calling thread's `errno`.
        let errno = unsafe { *location };
        // SAFETY: `strerror` takes any number.
        let message = unsafe { strerror(errno) };
        // SAFETY: a format, numbers and a NUL-terminated message.
        unsafe { printf(c"MACOS-TOOLS errno %d %d %s\n".as_ptr(), fd, errno, message) };
    }

    fn locale() {
        // SAFETY: an empty locale name asks for the environment's.
        let name = unsafe { setlocale(LC_ALL, c"".as_ptr()) };
        // SAFETY: a locale that does not exist.
        let other = unsafe { setlocale(LC_ALL, c"tlh_KL.UTF-8".as_ptr()) };
        // SAFETY: `localeconv` returns `struct lconv`, whose first field is `decimal_point`.
        let conventions = unsafe { localeconv() };
        // SAFETY: as above.
        let decimal_point = unsafe { *conventions };
        // SAFETY: `nl_langinfo` takes any item.
        let codeset = unsafe { nl_langinfo(CODESET) };
        let refused = if other.is_null() {
            c"refused"
        } else {
            c"accepted"
        };
        // SAFETY: a format and NUL-terminated strings.
        unsafe {
            printf(
                c"MACOS-TOOLS locale %s %s %s other %s\n".as_ptr(),
                name,
                codeset,
                decimal_point,
                refused.as_ptr(),
            )
        };
    }

    #[unsafe(no_mangle)]
    extern "C" fn main(count: c_int, arguments: *const *mut c_char) -> c_int {
        let long = [
            LongOption {
                name: c"list".as_ptr(),
                has_arg: REQUIRED_ARGUMENT,
                flag: core::ptr::null_mut(),
                value: LONG_LIST,
            },
            LongOption {
                name: c"name".as_ptr(),
                has_arg: REQUIRED_ARGUMENT,
                flag: core::ptr::null_mut(),
                value: LONG_NAME,
            },
            LongOption {
                name: c"verbose".as_ptr(),
                has_arg: NO_ARGUMENT,
                flag: core::ptr::null_mut(),
                value: b'v' as c_int,
            },
            LongOption {
                name: core::ptr::null(),
                has_arg: 0,
                flag: core::ptr::null_mut(),
                value: 0,
            },
        ];
        let mut verbose = 0;
        let mut number: *const c_char = c"-".as_ptr();
        let mut directory: *const c_char = core::ptr::null();
        let mut name: *const c_char = c"-".as_ptr();
        loop {
            // SAFETY: the program's arguments, a NUL-terminated spec and a terminated table.
            let option = unsafe {
                getopt_long(
                    count,
                    arguments,
                    c"vn:".as_ptr(),
                    long.as_ptr(),
                    core::ptr::null_mut(),
                )
            };
            // SAFETY: `getopt_long` set `optarg` for options that take an argument.
            let value = unsafe { optarg };
            match option {
                -1 => break,
                0x76 => verbose += 1,
                0x6e => number = value,
                LONG_LIST => directory = value,
                LONG_NAME => name = value,
                _ => return 2,
            }
        }
        // SAFETY: `getopt_long` leaves `optind` at the first operand.
        let first = unsafe { optind };
        let rest = if first < count {
            // SAFETY: an index below `count`.
            let slot = unsafe { arguments.add(first as usize) };
            // SAFETY: the loader's argument vector holds `count` pointers.
            unsafe { *slot }
        } else {
            c"-".as_ptr().cast_mut()
        };
        // SAFETY: a format, numbers and NUL-terminated strings.
        unsafe {
            printf(
                c"MACOS-TOOLS options v=%d n=%s name=%s optind=%d rest=%s\n".as_ptr(),
                verbose,
                number,
                name,
                first,
                rest,
            )
        };
        if !directory.is_null() {
            list(directory);
        }
        times();
        errors();
        locale();
        0
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn main() {}
