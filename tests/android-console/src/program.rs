use core::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use core::fmt::Write;

const PREFIX: &str = "ANDROID-CONSOLE";
const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 1;
const AT_PAGESZ: c_ulong = 6;
const PROPERTY_VALUE_MAX: usize = 92;
const THREADS: usize = 4;
const LINE_BUFFER: usize = 128;
const LISTED_NAMES: usize = 16;
const NAME_LENGTH: usize = 64;
const SMALL_BLOCKS: usize = 64;
const LARGE_BLOCK: usize = 1 << 20;
const AF_UNIX: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOCK_CLOEXEC: c_int = 0o2_000_000;
const LOG_SOCKET: &[u8] = b"/dev/socket/logdw";
const LOG_ID_MAIN: u8 = 0;
const LOG_INFO: u8 = 4;
const LOG_HEADER: usize = 11;
const LOG_TAG: &CStr = c"eggos-test";
const LOG_MESSAGE: &CStr = c"hello from android-console";

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
    offset: i64,
    record_length: u16,
    kind: u8,
    name: [c_char; 256],
}

#[repr(C)]
struct UnixAddress {
    family: u16,
    path: [u8; 108],
}

#[repr(C)]
struct IoVector {
    base: *const c_void,
    length: usize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timespec {
    seconds: i64,
    nanoseconds: c_long,
}

#[link(name = "c")]
unsafe extern "C" {
    static stdout: *mut File;
    static stderr: *mut File;
    fn printf(format: *const c_char, ...) -> c_int;
    fn fputs(text: *const c_char, file: *mut File) -> c_int;
    fn fflush(file: *mut File) -> c_int;
    fn fopen(path: *const c_char, mode: *const c_char) -> *mut File;
    fn fgets(buffer: *mut c_char, size: c_int, file: *mut File) -> *mut c_char;
    fn fclose(file: *mut File) -> c_int;
    fn opendir(path: *const c_char) -> *mut Directory;
    fn readdir(directory: *mut Directory) -> *mut DirectoryEntry;
    fn closedir(directory: *mut Directory) -> c_int;
    fn getenv(name: *const c_char) -> *const c_char;
    fn malloc(size: usize) -> *mut u8;
    fn calloc(count: usize, size: usize) -> *mut u8;
    fn realloc(block: *mut u8, size: usize) -> *mut u8;
    fn free(block: *mut u8);
    fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
    fn usleep(microseconds: u32) -> c_int;
    fn pthread_create(
        thread: *mut c_long,
        attributes: *const c_void,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        argument: *mut c_void,
    ) -> c_int;
    fn pthread_join(thread: c_long, result: *mut *mut c_void) -> c_int;
    fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    fn getauxval(kind: c_ulong) -> c_ulong;
    fn arc4random_buf(buffer: *mut c_void, length: usize);
    fn system(command: *const c_char) -> c_int;
    fn atoi(text: *const c_char) -> c_int;
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn connect(socket: c_int, address: *const UnixAddress, length: u32) -> c_int;
    fn writev(file: c_int, vectors: *const IoVector, count: c_int) -> isize;
    fn close(file: c_int) -> c_int;
    fn gettid() -> c_int;
    fn exit(status: c_int) -> !;
    fn abort() -> !;
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
        // SAFETY: the line ends in a NUL, and `file` is one of bionic's standard streams.
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
        // SAFETY: bionic set `stdout` up before `main`.
        line.print_to(unsafe { stdout });
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

#[unsafe(no_mangle)]
extern "C" fn main(count: c_int, arguments: *const *const c_char) -> c_int {
    // SAFETY: bionic passes `count` valid argument pointers.
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
        _ => {}
    }
    let name = argument(0).and_then(|path| path.rsplit('/').next());
    let mut line = Line::new();
    let _ = write!(line, "{} args", name.unwrap_or_default());
    for (index, argument) in arguments.iter().skip(1).map(|a| text(*a)).enumerate() {
        let separator = if index == 0 { " " } else { "|" };
        let _ = write!(line, "{separator}{argument}");
    }
    // SAFETY: bionic set `stdout` up before `main`.
    line.print_to(unsafe { stdout });
    // SAFETY: the name is a NUL-terminated literal.
    let greeting = unsafe { getenv(c"GREETING".as_ptr()) };
    let greeting = if greeting.is_null() {
        "unset"
    } else {
        text(greeting)
    };
    say!("env GREETING={greeting}");
    let mut buffer = [0; LINE_BUFFER];
    say!("file {}", first_line(c"/etc/hostname", &mut buffer));
    directory_listing(c"/usr/share/eggos");
    say!("malloc {}", heap());
    say!("time {}", clocks());
    say!("threads {}", threads());
    say_property(c"ro.build.version.sdk");
    say_property(c"eggos.test.greeting");
    say!("log {}", log_entry());
    // SAFETY: `getauxval` takes a number.
    let page_size = unsafe { getauxval(AT_PAGESZ) };
    // SAFETY: the format takes one string and one unsigned long, which follow it.
    unsafe {
        printf(
            c"%s pagesize %lu\n".as_ptr(),
            c"ANDROID-CONSOLE".as_ptr(),
            page_size,
        )
    };
    say!("random {}", random());
    // SAFETY: `fflush(NULL)` flushes every stream.
    unsafe { fflush(core::ptr::null_mut()) };
    // SAFETY: the command is a NUL-terminated literal.
    let status = unsafe { system(c"echo ANDROID-CONSOLE shell".as_ptr()) };
    say!("system status {}", status >> 8 & 0xff);
    let mut line = Line::new();
    let _ = write!(line, "stderr");
    // SAFETY: bionic set `stderr` up before `main`.
    line.print_to(unsafe { stderr });
    0
}

fn first_line<'a>(path: &CStr, buffer: &'a mut [c_char; LINE_BUFFER]) -> &'a str {
    // SAFETY: the path and the mode are NUL-terminated.
    let file = unsafe { fopen(path.as_ptr(), c"re".as_ptr()) };
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

fn directory_listing(path: &CStr) {
    let mut names = [[0u8; NAME_LENGTH]; LISTED_NAMES];
    let mut count = 0;
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
    // SAFETY: bionic set `stdout` up before `main`.
    line.print_to(unsafe { stdout });
}

fn heap() -> &'static str {
    let mut blocks = [core::ptr::null_mut::<u8>(); SMALL_BLOCKS];
    for (index, block) in blocks.iter_mut().enumerate() {
        // SAFETY: `malloc` takes a size.
        *block = unsafe { malloc(16 + index * 8) };
        if block.is_null() {
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
    let started = now(CLOCK_MONOTONIC);
    // SAFETY: `usleep` takes a number.
    unsafe { usleep(1000) };
    let ended = now(CLOCK_MONOTONIC);
    let monotonic = matches!((started, ended), (Some(s), Some(e)) if e - s >= 1_000_000);
    let realtime = now(CLOCK_REALTIME).is_some_and(|time| time >= 0);
    match (monotonic, realtime) {
        (true, true) => "monotonic realtime",
        (false, _) => "the monotonic clock went backwards",
        (true, false) => "the realtime clock is before the epoch",
    }
}

extern "C" fn square(argument: *mut c_void) -> *mut c_void {
    let index = argument as usize;
    (index * index) as *mut c_void
}

fn threads() -> &'static str {
    let mut handles = [0 as c_long; THREADS];
    for (index, handle) in handles.iter_mut().enumerate() {
        let argument = (index + 1) as *mut c_void;
        // SAFETY: `handle` is a local `pthread_t`, the attributes are the defaults, and
        // `square` reads only its argument.
        if unsafe { pthread_create(handle, core::ptr::null(), square, argument) } != 0 {
            return "pthread_create failed";
        }
    }
    let mut sum = 0;
    for handle in handles {
        let mut result = core::ptr::null_mut();
        // SAFETY: `handle` is a thread created above and joined once.
        if unsafe { pthread_join(handle, &mut result) } != 0 {
            return "pthread_join failed";
        }
        sum += result as usize;
    }
    if sum == 30 {
        "4 joined sum 30"
    } else {
        "a thread returned the wrong square"
    }
}

fn say_property(name: &CStr) {
    let mut value = [0 as c_char; PROPERTY_VALUE_MAX];
    // SAFETY: the name is NUL-terminated, and `value` holds `PROP_VALUE_MAX` bytes.
    let length = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    let value = match length {
        0 => "unset",
        _ => text(value.as_ptr()),
    };
    say!("property {}={value}", text(name.as_ptr()));
}

fn random() -> &'static str {
    let mut first = [0u8; 16];
    let mut second = [0u8; 16];
    // SAFETY: each buffer is a local array of the length given.
    unsafe {
        arc4random_buf(first.as_mut_ptr().cast(), first.len());
    }
    // SAFETY: each buffer is a local array of the length given.
    unsafe {
        arc4random_buf(second.as_mut_ptr().cast(), second.len());
    }
    if first != second {
        "differs"
    } else {
        "repeats"
    }
}

fn log_entry() -> &'static str {
    // SAFETY: `socket` takes numbers.
    let file = unsafe { socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if file < 0 {
        return "socket failed";
    }
    let mut address = UnixAddress {
        family: AF_UNIX as u16,
        path: [0; 108],
    };
    address.path[..LOG_SOCKET.len()].copy_from_slice(LOG_SOCKET);
    let length = core::mem::size_of::<UnixAddress>() as u32;
    // SAFETY: `address` is a local `sockaddr_un` of `length` bytes.
    let connected = unsafe { connect(file, &address, length) } == 0;
    let mut time = Timespec::default();
    // SAFETY: `time` is a local `timespec`.
    unsafe { clock_gettime(CLOCK_REALTIME, &mut time) };
    // SAFETY: `gettid` takes no arguments.
    let thread = unsafe { gettid() };
    let mut header = [0u8; LOG_HEADER];
    header[0] = LOG_ID_MAIN;
    header[1..3].copy_from_slice(&(thread as u16).to_le_bytes());
    header[3..7].copy_from_slice(&(time.seconds as u32).to_le_bytes());
    header[7..11].copy_from_slice(&(time.nanoseconds as u32).to_le_bytes());
    let priority = [LOG_INFO];
    let tag = LOG_TAG.to_bytes_with_nul();
    let message = LOG_MESSAGE.to_bytes_with_nul();
    let vectors = [&header[..], &priority[..], tag, message].map(|part| IoVector {
        base: part.as_ptr().cast(),
        length: part.len(),
    });
    // SAFETY: each vector points at a local or static buffer of its length.
    let written = unsafe { writev(file, vectors.as_ptr(), vectors.len() as c_int) };
    // SAFETY: `file` is the socket opened above, closed once.
    unsafe { close(file) };
    let whole = LOG_HEADER + priority.len() + tag.len() + message.len();
    match (connected, usize::try_from(written)) {
        (false, _) => "connect failed",
        (true, Ok(written)) if written == whole => "sent",
        (true, _) => "writev failed",
    }
}
