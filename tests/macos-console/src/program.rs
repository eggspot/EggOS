use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::fmt::Write;

const PREFIX: &str = "MACOS-CONSOLE";
const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 6;
const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0x200;
const O_TRUNC: c_int = 0x400;
const S_IFMT: u16 = 0o170_000;
const S_IFREG: u16 = 0o100_000;
const S_IFDIR: u16 = 0o040_000;
const ENOENT: c_int = 2;
const LINE_BUFFER: usize = 128;
const LISTED_NAMES: usize = 16;
const NAME_LENGTH: usize = 64;
const SMALL_BLOCKS: usize = 64;
const LARGE_BLOCK: usize = 1 << 20;

#[repr(C)]
struct File {
    opaque: [u8; 0],
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
#[derive(Default, Clone, Copy)]
struct Timespec {
    seconds: i64,
    nanoseconds: c_long,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timeval {
    seconds: i64,
    microseconds: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timebase {
    numerator: u32,
    denominator: u32,
}

#[repr(C)]
struct Stat {
    device: i32,
    mode: u16,
    links: u16,
    inode: u64,
    user: u32,
    group: u32,
    special_device: i32,
    times: [Timespec; 4],
    size: i64,
    blocks: i64,
    block_size: i32,
    flags: u32,
    generation: u32,
    spare: i32,
    more_spare: [i64; 2],
}

#[link(name = "System")]
unsafe extern "C" {
    static __stdoutp: *mut File;
    static __stderrp: *mut File;
    fn printf(format: *const c_char, ...) -> c_int;
    fn snprintf(buffer: *mut c_char, size: usize, format: *const c_char, ...) -> c_int;
    fn puts(text: *const c_char) -> c_int;
    fn fputs(text: *const c_char, file: *mut File) -> c_int;
    fn fflush(file: *mut File) -> c_int;
    fn fopen(path: *const c_char, mode: *const c_char) -> *mut File;
    fn fgets(buffer: *mut c_char, size: c_int, file: *mut File) -> *mut c_char;
    fn fclose(file: *mut File) -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    #[cfg_attr(target_arch = "x86_64", link_name = "stat$INODE64")]
    fn stat(path: *const c_char, stat: *mut Stat) -> c_int;
    #[cfg_attr(target_arch = "x86_64", link_name = "opendir$INODE64")]
    fn opendir(path: *const c_char) -> *mut Directory;
    #[cfg_attr(target_arch = "x86_64", link_name = "readdir$INODE64")]
    fn readdir(directory: *mut Directory) -> *mut DirectoryEntry;
    fn closedir(directory: *mut Directory) -> c_int;
    fn getcwd(buffer: *mut c_char, size: usize) -> *mut c_char;
    fn chdir(path: *const c_char) -> c_int;
    fn __error() -> *mut c_int;
    fn getenv(name: *const c_char) -> *const c_char;
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
    fn malloc(size: usize) -> *mut u8;
    fn calloc(count: usize, size: usize) -> *mut u8;
    fn realloc(block: *mut u8, size: usize) -> *mut u8;
    fn free(block: *mut u8);
    fn strlen(text: *const c_char) -> usize;
    fn strcmp(left: *const c_char, right: *const c_char) -> c_int;
    fn strncmp(left: *const c_char, right: *const c_char, count: usize) -> c_int;
    fn strchr(text: *const c_char, character: c_int) -> *const c_char;
    fn strrchr(text: *const c_char, character: c_int) -> *const c_char;
    fn strstr(text: *const c_char, needle: *const c_char) -> *const c_char;
    fn strdup(text: *const c_char) -> *mut c_char;
    fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int;
    fn atoi(text: *const c_char) -> c_int;
    fn strtol(text: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long;
    fn qsort(
        base: *mut c_void,
        count: usize,
        size: usize,
        compare: extern "C" fn(*const c_void, *const c_void) -> c_int,
    );
    fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
    fn gettimeofday(time: *mut Timeval, zone: *mut c_void) -> c_int;
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> c_int;
    fn usleep(microseconds: u32) -> c_int;
    fn getpid() -> c_int;
    fn atexit(handler: extern "C" fn()) -> c_int;
    fn exit(status: c_int) -> !;
    fn abort() -> !;
    fn posix_spawn(
        pid: *mut c_int,
        path: *const c_char,
        actions: *const c_void,
        attributes: *const c_void,
        arguments: *const *const c_char,
        environment: *const *const c_char,
    ) -> c_int;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: `abort` takes no arguments and ends the process.
    unsafe { abort() }
}

struct Line {
    bytes: [u8; 256],
    length: usize,
}

impl Line {
    fn new() -> Self {
        let mut line = Self {
            bytes: [0; 256],
            length: 0,
        };
        let _ = write!(line, "{PREFIX} ");
        line
    }

    fn print_to(mut self, file: *mut File) {
        let _ = self.write_str("\n");
        let end = self.length.min(self.bytes.len() - 1);
        self.bytes[end] = 0;
        // SAFETY: the line ends in a NUL, and `file` is one of libSystem's standard streams.
        unsafe { fputs(self.bytes.as_ptr().cast(), file) };
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - 1 - self.length;
        let taken = text.len().min(room);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text.as_bytes()[..taken]);
        self.length += taken;
        Ok(())
    }
}

macro_rules! say {
    ($($argument:tt)*) => {{
        let mut line = Line::new();
        let _ = write!(line, $($argument)*);
        // SAFETY: libSystem set `__stdoutp` up before `main`.
        line.print_to(unsafe { __stdoutp });
    }};
}

fn text<'a>(pointer: *const c_char) -> &'a str {
    if pointer.is_null() {
        return "";
    }
    // SAFETY: the callers pass NUL-terminated strings that live as long as the program.
    let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
    core::str::from_utf8(bytes).unwrap_or("?")
}

fn errno() -> c_int {
    // SAFETY: `__error` returns the calling thread's `errno`.
    unsafe { *__error() }
}

extern "C" fn goodbye() {
    say!("atexit");
}

#[unsafe(no_mangle)]
extern "C" fn main(
    count: c_int,
    arguments: *const *const c_char,
    environment: *const *const c_char,
    apple: *const *const c_char,
) -> c_int {
    // SAFETY: the loader passes `count` valid argument pointers.
    let arguments = unsafe { core::slice::from_raw_parts(arguments, count.max(0) as usize) };
    let argument = |index: usize| arguments.get(index).copied().map(text);
    match argument(1) {
        Some("exit") => {
            let status = arguments.get(2).copied().unwrap_or(c"1".as_ptr());
            // SAFETY: `status` is a NUL-terminated argument.
            let status = unsafe { atoi(status) };
            // SAFETY: `exit` takes a number and ends the process.
            unsafe { exit(status) }
        }
        // SAFETY: `abort` takes no arguments and ends the process.
        Some("abort") => unsafe { abort() },
        Some("unimplemented") => {
            let mut pid = 0;
            // SAFETY: the layer ends the process before it reads any argument.
            let status = unsafe {
                posix_spawn(
                    &mut pid,
                    c"/bin/eggsh".as_ptr(),
                    core::ptr::null(),
                    core::ptr::null(),
                    core::ptr::null(),
                    core::ptr::null(),
                )
            };
            say!("posix_spawn returned {status}");
            return 0;
        }
        _ => {}
    }
    // SAFETY: `goodbye` is a function that takes nothing.
    unsafe { atexit(goodbye) };
    let name = argument(0).and_then(|path| path.rsplit('/').next());
    let mut line = Line::new();
    let _ = write!(line, "{} args", name.unwrap_or_default());
    for (index, argument) in arguments.iter().skip(1).map(|a| text(*a)).enumerate() {
        let separator = if index == 0 { " " } else { "|" };
        let _ = write!(line, "{separator}{argument}");
    }
    // SAFETY: libSystem set `__stdoutp` up before `main`.
    line.print_to(unsafe { __stdoutp });
    // SAFETY: `apple` is the loader's null-ended array of strings.
    let executable = text(unsafe { apple.read() });
    say!(
        "apple {}",
        if executable.starts_with("executable_path=") && executable.ends_with("macos-console") {
            "executable_path"
        } else {
            "missing"
        }
    );
    say!("env {}", environment_check(environment));
    say!("printf");
    formats();
    say!("strings {}", strings());
    say!("numbers {}", numbers());
    let mut buffer = [0; LINE_BUFFER];
    say!("file {}", first_line(c"/etc/hostname", &mut buffer));
    say!("missing {}", missing_file());
    say!("stat {}", stat_check());
    directory_listing(c"/usr/share/eggos");
    say!("cwd {}", working_directory());
    say!("malloc {}", heap());
    say!("time {}", clocks());
    // SAFETY: `getpid` takes no arguments.
    let pid = unsafe { getpid() };
    say!("pid {}", if pid > 0 { "positive" } else { "invalid" });
    // SAFETY: `fflush(NULL)` flushes every stream.
    unsafe { fflush(core::ptr::null_mut()) };
    let mut line = Line::new();
    let _ = write!(line, "stderr");
    // SAFETY: libSystem set `__stderrp` up before `main`.
    line.print_to(unsafe { __stderrp });
    0
}

fn environment_check(environment: *const *const c_char) -> &'static str {
    // SAFETY: the name is a NUL-terminated literal.
    let greeting = text(unsafe { getenv(c"GREETING".as_ptr()) });
    let mut found = false;
    let mut at = environment;
    loop {
        // SAFETY: `envp` is a null-ended array of strings.
        let entry = unsafe { at.read() };
        if entry.is_null() {
            break;
        }
        found |= text(entry) == "GREETING=hi";
        at = at.wrapping_add(1);
    }
    // SAFETY: the name and value are NUL-terminated literals.
    unsafe { setenv(c"EGGOS_MACOS".as_ptr(), c"set".as_ptr(), 1) };
    // SAFETY: as above.
    let set = text(unsafe { getenv(c"EGGOS_MACOS".as_ptr()) });
    match (greeting, found, set) {
        ("hi", true, "set") => "GREETING=hi envp setenv",
        (_, false, _) => "envp lacks GREETING",
        (_, _, "set") => "getenv lacks GREETING",
        _ => "setenv failed",
    }
}

fn formats() {
    // SAFETY: each format takes the arguments that follow it.
    unsafe {
        printf(
            c"%s ints %d %i %u %ld %lld %zu %x %X %o %c %%\n".as_ptr(),
            c"MACOS-CONSOLE".as_ptr(),
            -42 as c_int,
            7 as c_int,
            3_000_000_000u32,
            -1_234_567_890_123 as c_long,
            9_000_000_000_000_000_000u64 as i64,
            12usize,
            0xbeef as c_int,
            0xbeef as c_int,
            8 as c_int,
            b'Z' as c_int,
        );
        printf(
            c"%s widths [%5d] [%-5d] [%05d] [%+d] [%.3s] [%8s] [%-8s] [%#x] [%*d]\n".as_ptr(),
            c"MACOS-CONSOLE".as_ptr(),
            42 as c_int,
            42 as c_int,
            42 as c_int,
            42 as c_int,
            c"abcdef".as_ptr(),
            c"right".as_ptr(),
            c"left".as_ptr(),
            255 as c_int,
            4 as c_int,
            7 as c_int,
        );
        printf(
            c"%s floats %.2f %f %.0f %e %g\n".as_ptr(),
            c"MACOS-CONSOLE".as_ptr(),
            3.25f64,
            -0.5f64,
            2.5f64,
            12345.678f64,
            0.0001f64,
        );
    }
    let mut buffer = [0 as c_char; 8];
    // SAFETY: `buffer` holds the 8 bytes `snprintf` may write.
    let length = unsafe {
        snprintf(
            buffer.as_mut_ptr(),
            buffer.len(),
            c"%s-%d".as_ptr(),
            c"truncated".as_ptr(),
            12345 as c_int,
        )
    };
    say!("snprintf {} {}", length, text(buffer.as_ptr()));
    // SAFETY: the text is a NUL-terminated literal.
    unsafe { puts(c"MACOS-CONSOLE puts".as_ptr()) };
}

fn strings() -> &'static str {
    let hello = c"hello, macos".as_ptr();
    // SAFETY: every pointer is a NUL-terminated literal or a copy `strdup` made of one.
    unsafe {
        let copy = strdup(hello);
        let checks = [
            strlen(hello) == 12,
            strcmp(hello, copy) == 0,
            strcmp(c"abc".as_ptr(), c"abd".as_ptr()) < 0,
            strncmp(c"abcx".as_ptr(), c"abcy".as_ptr(), 3) == 0,
            text(strchr(hello, b'o' as c_int)) == "o, macos",
            text(strrchr(hello, b'o' as c_int)) == "os",
            text(strstr(hello, c"mac".as_ptr())) == "macos",
            strstr(hello, c"linux".as_ptr()).is_null(),
            memcmp(hello.cast(), copy.cast(), 13) == 0,
        ];
        free(copy.cast());
        if checks.iter().all(|&check| check) {
            "strlen strcmp strchr strstr strdup memcmp"
        } else {
            "a string function failed"
        }
    }
}

extern "C" fn compare(left: *const c_void, right: *const c_void) -> c_int {
    // SAFETY: `qsort` passes pointers to two of the array's `i32` elements.
    let (left, right) = unsafe { (*left.cast::<i32>(), *right.cast::<i32>()) };
    left.cmp(&right) as c_int
}

fn numbers() -> &'static str {
    let mut values = [5i32, -3, 9, 0, 2];
    // SAFETY: `values` holds 5 `i32`s and `compare` compares two of them.
    unsafe { qsort(values.as_mut_ptr().cast(), values.len(), 4, compare) };
    let mut end = core::ptr::null_mut();
    // SAFETY: the text is a NUL-terminated literal and `end` a local pointer.
    let hex = unsafe { strtol(c"  -0x1fz".as_ptr(), &mut end, 16) };
    // SAFETY: `end` points into the literal.
    let rest = text(end);
    // SAFETY: the text is a NUL-terminated literal.
    let decimal = unsafe { atoi(c"1234abc".as_ptr()) };
    match (values, hex, rest, decimal) {
        ([-3, 0, 2, 5, 9], -31, "z", 1234) => "qsort strtol atoi",
        _ => "a number function failed",
    }
}

fn first_line<'a>(path: &CStr, buffer: &'a mut [c_char; LINE_BUFFER]) -> &'a str {
    // SAFETY: the path and the mode are NUL-terminated.
    let file = unsafe { fopen(path.as_ptr(), c"r".as_ptr()) };
    if file.is_null() {
        return "error";
    }
    // SAFETY: the buffer holds the `LINE_BUFFER` bytes `fgets` may write, and `file` is open.
    let read = unsafe { fgets(buffer.as_mut_ptr(), LINE_BUFFER as c_int, file) };
    // SAFETY: `file` is the stream `fopen` returned.
    unsafe { fclose(file) };
    if read.is_null() {
        return "empty";
    }
    text(read).trim_end()
}

fn missing_file() -> &'static str {
    // SAFETY: the path is NUL-terminated.
    let fd = unsafe { open(c"/etc/no-such-file".as_ptr(), O_RDONLY) };
    let missing = fd == -1 && errno() == ENOENT;
    // SAFETY: the path is NUL-terminated and the mode follows the flags.
    let fd = unsafe { open(c"/hello.txt".as_ptr(), O_WRONLY | O_CREAT | O_TRUNC, 0o644) };
    let refused = fd == -1 && errno() != 0;
    // SAFETY: the path is NUL-terminated.
    let fd = unsafe { open(c"/etc/hostname".as_ptr(), O_RDONLY) };
    let mut bytes = [0u8; 5];
    // SAFETY: `bytes` holds the 5 bytes `read` may write.
    let read = unsafe { read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
    // SAFETY: `fd` is the descriptor `open` returned.
    unsafe { close(fd) };
    match (missing, refused, read, &bytes) {
        (true, true, 5, b"eggos") => "ENOENT, read-only, open read close",
        (false, _, _, _) => "no ENOENT",
        (_, false, _, _) => "a write to the read-only file system worked",
        _ => "open and read failed",
    }
}

fn stat_check() -> &'static str {
    // SAFETY: `Stat` is plain data, and zero is a valid value for it.
    let mut file: Stat = unsafe { core::mem::zeroed() };
    // SAFETY: as above.
    let mut directory: Stat = unsafe { core::mem::zeroed() };
    // SAFETY: the paths are NUL-terminated and each `Stat` a local one.
    let results = unsafe {
        [
            stat(c"/etc/hostname".as_ptr(), &mut file),
            stat(c"/usr/share/eggos".as_ptr(), &mut directory),
        ]
    };
    let file_ok = file.mode & S_IFMT == S_IFREG && file.size == 6 && file.links >= 1;
    let directory_ok = directory.mode & S_IFMT == S_IFDIR;
    match (results, file_ok, directory_ok) {
        ([0, 0], true, true) => "regular 6 bytes, directory",
        _ => "wrong",
    }
}

fn directory_listing(path: &CStr) {
    let mut names = [[0u8; NAME_LENGTH]; LISTED_NAMES];
    let mut count = 0;
    let mut lengths_match = true;
    // SAFETY: the path is NUL-terminated.
    let directory = unsafe { opendir(path.as_ptr()) };
    if directory.is_null() {
        say!("dir error");
        return;
    }
    loop {
        // SAFETY: `directory` is the stream `opendir` returned.
        let entry = unsafe { readdir(directory) };
        if entry.is_null() {
            break;
        }
        // SAFETY: `readdir` returned an entry whose name is NUL-terminated.
        let name = text(unsafe { (*entry).name.as_ptr() });
        // SAFETY: as above.
        lengths_match &= usize::from(unsafe { (*entry).name_length }) == name.len();
        if name == "." || name == ".." || count == LISTED_NAMES {
            continue;
        }
        let length = name.len().min(NAME_LENGTH);
        names[count][..length].copy_from_slice(&name.as_bytes()[..length]);
        count += 1;
    }
    // SAFETY: `directory` is the stream `opendir` returned.
    unsafe { closedir(directory) };
    let names = &mut names[..count];
    names.sort_unstable();
    let mut line = Line::new();
    let _ = write!(line, "dir");
    for name in names.iter() {
        let end = name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(NAME_LENGTH);
        let _ = write!(
            line,
            " {}",
            core::str::from_utf8(&name[..end]).unwrap_or("?")
        );
    }
    if !lengths_match {
        let _ = write!(line, " (d_namlen wrong)");
    }
    // SAFETY: libSystem set `__stdoutp` up before `main`.
    line.print_to(unsafe { __stdoutp });
}

fn working_directory() -> &'static str {
    let mut buffer = [0 as c_char; 64];
    // SAFETY: the path is NUL-terminated.
    let changed = unsafe { chdir(c"/etc".as_ptr()) } == 0;
    // SAFETY: `buffer` holds the 64 bytes `getcwd` may write.
    let cwd = text(unsafe { getcwd(buffer.as_mut_ptr(), buffer.len()) });
    let mut line = [0; LINE_BUFFER];
    let relative = first_line(c"hostname", &mut line) == "eggos";
    match (changed, cwd, relative) {
        (true, "/etc", true) => "chdir getcwd relative",
        _ => "wrong",
    }
}

fn heap() -> &'static str {
    let mut blocks = [core::ptr::null_mut::<u8>(); SMALL_BLOCKS];
    for (index, block) in blocks.iter_mut().enumerate() {
        // SAFETY: `malloc` takes a size.
        *block = unsafe { malloc(16 + index * 8) };
        if block.is_null() || *block as usize % 16 != 0 {
            return "small blocks failed";
        }
        // SAFETY: the block holds at least 16 bytes.
        unsafe { block.write_bytes(index as u8, 16) };
    }
    let kept = blocks
        .iter()
        .enumerate()
        // SAFETY: each block holds the 16 bytes written above.
        .all(|(index, block)| unsafe { block.wrapping_add(15).read() } == index as u8);
    for block in blocks {
        // SAFETY: each block came from `malloc` and is freed once.
        unsafe { free(block) };
    }
    // SAFETY: `calloc` takes a count and a size.
    let large = unsafe { calloc(1, LARGE_BLOCK) };
    if large.is_null() {
        return "large block failed";
    }
    // SAFETY: `calloc` zeroed the block's `LARGE_BLOCK` bytes.
    let zeroed = unsafe { core::slice::from_raw_parts(large, LARGE_BLOCK) }
        .iter()
        .all(|&byte| byte == 0);
    // SAFETY: `large` came from `calloc`.
    unsafe { large.write(7) };
    // SAFETY: `large` came from `calloc`, and `realloc` takes it over.
    let grown = unsafe { realloc(large, 2 * LARGE_BLOCK) };
    if grown.is_null() {
        return "realloc failed";
    }
    // SAFETY: `realloc` kept the first byte.
    let moved = unsafe { grown.read() } == 7;
    // SAFETY: `grown` came from `realloc`.
    unsafe { free(grown) };
    match (kept, zeroed, moved) {
        (true, true, true) => "small large realloc",
        (false, _, _) => "a small block changed",
        (_, false, _) => "calloc did not zero",
        (_, _, false) => "realloc lost the data",
    }
}

fn clocks() -> &'static str {
    let now = |clock| {
        let mut time = Timespec::default();
        // SAFETY: `time` is a local `timespec`.
        let status = unsafe { clock_gettime(clock, &mut time) };
        (status == 0).then_some(time.seconds * 1_000_000_000 + time.nanoseconds)
    };
    let mut timebase = Timebase::default();
    // SAFETY: `timebase` is a local `mach_timebase_info`.
    unsafe { mach_timebase_info(&mut timebase) };
    // SAFETY: `mach_absolute_time` takes no arguments.
    let ticks = unsafe { mach_absolute_time() };
    let started = now(CLOCK_MONOTONIC);
    // SAFETY: `usleep` takes a number.
    unsafe { usleep(1000) };
    let ended = now(CLOCK_MONOTONIC);
    // SAFETY: as above.
    let elapsed = (unsafe { mach_absolute_time() } - ticks) * u64::from(timebase.numerator)
        / u64::from(timebase.denominator.max(1));
    let monotonic = matches!((started, ended), (Some(s), Some(e)) if e - s >= 1_000_000);
    let realtime = now(CLOCK_REALTIME).is_some_and(|time| time >= 0);
    let mut day = Timeval::default();
    // SAFETY: `day` is a local `timeval`.
    let day_ok = unsafe { gettimeofday(&mut day, core::ptr::null_mut()) } == 0
        && (0..1_000_000).contains(&day.microseconds);
    match (monotonic, realtime, elapsed >= 1_000_000, day_ok) {
        (true, true, true, true) => "monotonic realtime mach gettimeofday",
        (false, ..) => "the monotonic clock went backwards",
        (_, false, ..) => "the realtime clock is before the epoch",
        (_, _, false, _) => "mach_absolute_time is wrong",
        _ => "gettimeofday failed",
    }
}
