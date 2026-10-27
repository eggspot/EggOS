//! The system calls `hello-window` makes, with the contract's request numbers and record
//! layouts as docs/windows.md gives them.

use crate::pattern;
use core::ffi::CStr;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};

const CREATE: usize = 0xc020_7701;
const SET_TITLE: usize = 0x4048_7702;
const COMMIT: usize = 0x4088_7708;
const DESTROY: usize = 0x4004_7709;
const GET_WORK_AREA: usize = 0x8014_770a;

const CLOSE_REQUEST: u16 = 9;
const RECORD: usize = 32;
const RECORDS_READ: usize = 8;

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CLOEXEC: usize = 0o2 | 0o2_000_000;
const PROT_READ_WRITE: usize = 3;
const MAP_SHARED: usize = 1;
const MAP_PRIVATE: usize = 2;
const EINVAL: isize = -22;
const EACCES: isize = -13;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
const STANDARD_INPUT: usize = 0;
const STANDARD_OUTPUT: usize = 1;
const STANDARD_ERROR: usize = 2;
const LONGEST_TITLE: usize = 64;

struct Options<'a> {
    title: &'a [u8],
    wait_for_input: bool,
    signal_output: bool,
}

fn options(startup: &Startup) -> Option<Options<'static>> {
    let mut options = Options {
        title: b"",
        wait_for_input: false,
        signal_output: false,
    };
    for argument in startup.arguments().skip(1).map(CStr::to_bytes) {
        match argument {
            b"-i" => options.wait_for_input = true,
            b"-o" => options.signal_output = true,
            title if title.len() <= LONGEST_TITLE => options.title = title,
            _ => return None,
        }
    }
    (!options.title.is_empty()).then_some(options)
}

fn say(title: &[u8], what: &[u8]) {
    let mut line = [0u8; 160];
    let mut length = 0;
    for part in [&b"hello-window "[..], title, b": ", what, b"\n"] {
        let part = &part[..part.len().min(line.len() - length)];
        line[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    }
    write_all(STANDARD_ERROR, &line[..length]);
}

fn fail(title: &[u8], what: &[u8]) -> i32 {
    say(title, what);
    1
}

fn ioctl(fd: usize, request: usize, argument: &mut [u8]) -> isize {
    // SAFETY: the request reads and writes `argument`, which this function borrows, and no
    // more than its size, which the request number encodes.
    unsafe {
        call(
            number::IOCTL,
            [fd, request, argument.as_mut_ptr() as usize, 0, 0, 0],
        )
    }
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn wait_for_line() {
    let mut byte = 0u8;
    loop {
        // SAFETY: `read` writes one byte into `byte`, which this function owns.
        let read = unsafe {
            call(
                number::READ,
                [STANDARD_INPUT, &raw mut byte as usize, 1, 0, 0, 0],
            )
        };
        if read <= 0 || byte == b'\n' {
            return;
        }
    }
}

/// Opens the window, draws it, and serves it until asked to close; the program's status.
pub fn run(startup: &Startup) -> i32 {
    let Some(options) = options(startup) else {
        write_all(STANDARD_ERROR, b"usage: hello-window [-i] [-o] <title>\n");
        return 2;
    };
    let title = options.title;
    if options.wait_for_input {
        wait_for_line();
    }
    let path = c"/dev/window";
    // SAFETY: `openat` reads the path, a C string that lives for the whole program.
    let fd = unsafe {
        call(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, O_RDWR_CLOEXEC, 0, 0, 0],
        )
    };
    if fd < 0 {
        return fail(title, b"cannot open /dev/window");
    }
    let fd = fd as usize;
    let mut area = [0u8; 20];
    if ioctl(fd, GET_WORK_AREA, &mut area) != 0 || word(&area, 8) == 0 || word(&area, 16) != 1 {
        return fail(title, b"no work area");
    }
    let mut create = [0u8; 32];
    put(&mut create, 8, WIDTH);
    put(&mut create, 12, HEIGHT);
    if ioctl(fd, CREATE, &mut create) != 0 {
        return fail(title, b"cannot create a window");
    }
    let (width, height, window) = (word(&create, 8), word(&create, 12), word(&create, 20));
    let offset = u64::from_ne_bytes(create[24..32].try_into().unwrap_or([0; 8])) as usize;
    if (width, height) != (WIDTH, HEIGHT) {
        return fail(title, b"the window has another size");
    }
    let length = (width * height * 4) as usize;
    // SAFETY: a new mapping, of memory nothing else in the program uses.
    let private = unsafe {
        call(
            number::MMAP,
            [0, length, PROT_READ_WRITE, MAP_PRIVATE, fd, offset],
        )
    };
    if private != EACCES {
        return fail(title, b"a private mapping of the window was not refused");
    }
    // SAFETY: as above.
    let pixels = unsafe {
        call(
            number::MMAP,
            [0, length, PROT_READ_WRITE, MAP_SHARED, fd, offset],
        )
    };
    if pixels < 0 {
        return fail(title, b"cannot map the window's pixels");
    }
    let colours = pattern::colours(title);
    for y in 0..height {
        for x in 0..width {
            let at = (pixels as usize as *mut u32).wrapping_add((y * width + x) as usize);
            // SAFETY: the mapping is `width * height` pixels, writable, and this program's.
            unsafe { at.write_volatile(pattern::pixel(colours, x, y)) };
        }
    }
    let mut set_title = [0u8; 72];
    put(&mut set_title, 0, window);
    put(&mut set_title, 4, title.len() as u32);
    set_title[8..8 + title.len()].copy_from_slice(title);
    let mut commit = [0u8; 136];
    put(&mut commit, 0, window);
    if ioctl(fd, SET_TITLE, &mut set_title) != 0 || ioctl(fd, COMMIT, &mut commit) != 0 {
        return fail(title, b"cannot show the window");
    }
    let mut small = [0u8; RECORD - 1];
    // SAFETY: `read` writes at most `small.len()` bytes into `small`, which this function owns.
    let short = unsafe {
        call(
            number::READ,
            [fd, small.as_mut_ptr() as usize, small.len(), 0, 0, 0],
        )
    };
    if short != EINVAL {
        return fail(title, b"a read shorter than a record was not refused");
    }
    say(title, b"shown");
    if options.signal_output {
        write_all(STANDARD_OUTPUT, b"shown\n");
    }
    serve(fd, window, title)
}

fn serve(fd: usize, window: u32, title: &[u8]) -> i32 {
    let mut records = [0u8; RECORD * RECORDS_READ];
    loop {
        // SAFETY: `read` writes at most `records.len()` bytes into `records`, owned here.
        let read = unsafe {
            call(
                number::READ,
                [fd, records.as_mut_ptr() as usize, records.len(), 0, 0, 0],
            )
        };
        if read <= 0 {
            return fail(title, b"cannot read events");
        }
        for record in records[..read as usize].chunks_exact(RECORD) {
            let kind = u16::from_ne_bytes([record[0], record[1]]);
            let target = u16::from_ne_bytes([record[2], record[3]]);
            if kind == CLOSE_REQUEST && u32::from(target) == window {
                say(title, b"close request");
                let mut destroy = window.to_ne_bytes();
                if ioctl(fd, DESTROY, &mut destroy) != 0 {
                    return fail(title, b"cannot destroy the window");
                }
                return 0;
            }
        }
    }
}
