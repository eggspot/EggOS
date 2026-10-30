//! Streams and `printf`: a `FILE *` is one of the layer's streams (`crate::stream`) on a Linux
//! descriptor, kept in a table of at most 64 so a pointer the program did not get from
//! `fopen` is refused with `EBADF`; one lock covers the table and every operation, so threads
//! may share streams. `stdin`, `stdout` and `stderr` are descriptors 0 to 2, made on first use:
//! a GUI program's are missing where they would be the terminal, as its standard handles are;
//! a terminal's `stdout` is line buffered and `stderr` unbuffered. Paths are Windows paths
//! (`C:\tmp\a.txt` is `/tmp/a.txt`). The `printf` family is the shared `eggos-c-format` crate's
//! Windows dialect over the `va_list`, which on x64 and ARM64 alike is a pointer to 8-byte
//! slots; the wide functions format UTF-8, which `swprintf`'s family converts to UTF-16 in the
//! caller's buffer and `fwprintf`'s writes to the stream as it is (streams are byte streams).
//! For the old C runtime (`msvcrt.dll`, D189), an entry of the `FILE` array `__iob_func` returns
//! names standard input, output or error, every stream starts with the old runtime's `FILE`
//! and the critical section MinGW's `printf` enters after it, and the `printf` family is also
//! variadic.

use super::super::SpinLock;
use super::super::console::standard_handle;
use super::super::file::fd_of;
use super::super::heap as process_heap;
use super::runtime::{binary_by_default, errno, errno_for_linux, set_errno};
use crate::counted_print::{self, Convention, Fault};
use crate::library::Export;
use crate::linux;
use crate::msvcrt_layout;
use crate::stream::{Backing, Buffering, Mode, Origin, Stream};
use crate::text;
use core::cell::UnsafeCell;
use core::ptr;
use eggos_c_format::{self as c_format, Arguments, Dialect, Output};

const MAX_STREAMS: usize = 64;
const STANDARD_STREAMS: usize = 3;
const END_OF_FILE: i32 = -1;
const LINUX_EBADF: u16 = 9;
const LINUX_EACCES: u16 = 13;
const O_APPEND: usize = 0o2000;
const LEGACY_WIDE_SPECIFIERS: u64 = 0x4;
const TRUNCATE: usize = usize::MAX;

#[derive(Debug, Clone, Copy)]
struct Descriptor {
    fd: Option<usize>,
}

impl Backing for Descriptor {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, u16> {
        let fd = self.fd.ok_or(LINUX_EBADF)?;
        linux::read(fd, buffer).map_err(|errno| errno.0)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), u16> {
        let fd = self.fd.ok_or(LINUX_EBADF)?;
        linux::write_all(fd, bytes).map_err(|errno| errno.0)
    }

    fn seek(&mut self, offset: i64, origin: Origin) -> Result<u64, u16> {
        let fd = self.fd.ok_or(LINUX_EBADF)?;
        let whence = match origin {
            Origin::Start => 0,
            Origin::Current => 1,
            Origin::End => 2,
        };
        linux::lseek(fd, offset, whence)
            .map(|position| position as u64)
            .map_err(|errno| errno.0)
    }
}

type Buffered = Stream<Descriptor>;

#[repr(C)]
struct File {
    #[allow(dead_code, reason = "MinGW programs reach into it")]
    msvcrt_fields: [u64; msvcrt_layout::FILE_WITH_LOCK_WORDS],
    stream: Buffered,
}

#[repr(C, align(16))]
struct Iob(UnsafeCell<[u64; msvcrt_layout::IOB_WORDS]>);

// SAFETY: the program writes its entries (MinGW's `_lock_file` sets a flag in them); the layer
// only takes their addresses.
unsafe impl Sync for Iob {}

static IOB: Iob = Iob(UnsafeCell::new([0; msvcrt_layout::IOB_WORDS]));

struct Streams {
    open: [*mut File; MAX_STREAMS],
    last_used: usize,
    standard_closed: [bool; STANDARD_STREAMS],
}

impl Streams {
    fn holds(&mut self, stream: *mut File) -> bool {
        if stream.is_null() {
            return false;
        }
        if self.open[self.last_used] == stream {
            return true;
        }
        match self.open.iter().position(|&open| open == stream) {
            Some(index) => {
                self.last_used = index;
                true
            }
            None => false,
        }
    }

    fn resolve(&mut self, stream: *mut File) -> *mut File {
        match msvcrt_layout::iob_index(IOB.0.get() as usize, stream as usize) {
            Some(index) if index < STANDARD_STREAMS => standard_stream(self, index),
            _ => stream,
        }
    }

    fn each_open(&mut self, mut work: impl FnMut(&mut Buffered)) {
        for &stream in self.open.iter().filter(|stream| !stream.is_null()) {
            // SAFETY: an open stream of the table, used only under its lock.
            work(unsafe { &mut (*stream).stream });
        }
    }
}

// SAFETY: the table holds addresses of heap blocks, which any thread may use under its lock.
unsafe impl Send for Streams {}

static STREAMS: SpinLock<Streams> = SpinLock::new(Streams {
    open: [ptr::null_mut(); MAX_STREAMS],
    last_used: 0,
    standard_closed: [false; STANDARD_STREAMS],
});
const EXIT_LOCK_WAIT_NANOSECONDS: u64 = 100_000_000;

fn letters(text: &[u8]) -> impl Iterator<Item = u32> + Clone + '_ {
    text.iter().map(|&byte| u32::from(byte))
}

fn new_stream(stream: Buffered) -> *mut File {
    let block = process_heap::allocate_with(core::mem::size_of::<File>(), false).cast::<File>();
    if !block.is_null() {
        let file = File {
            msvcrt_fields: [0; msvcrt_layout::FILE_WITH_LOCK_WORDS],
            stream,
        };
        // SAFETY: a fresh heap block, 16-byte aligned, for one stream.
        unsafe { block.write(file) };
    }
    block
}

fn standard_stream(streams: &mut Streams, index: usize) -> *mut File {
    if streams.open[index].is_null() && !streams.standard_closed[index] {
        let fd = fd_of(standard_handle(index));
        let (letter, buffering) = match index {
            0 => (b"r", Buffering::Full),
            1 if fd.is_some_and(linux::is_terminal) => (b"w", Buffering::Line),
            1 => (b"w", Buffering::Full),
            _ => (b"w", Buffering::None),
        };
        let Some(mode) = Mode::parse(letters(letter)) else {
            return ptr::null_mut();
        };
        streams.open[index] = new_stream(Stream::new(Descriptor { fd }, mode, buffering));
    }
    streams.open[index]
}

fn with_stream<R>(stream: *mut File, refused: R, work: impl FnOnce(&mut Buffered) -> R) -> R {
    STREAMS.with(|streams| {
        let stream = streams.resolve(stream);
        if !streams.holds(stream) {
            set_errno(if stream.is_null() {
                errno::EINVAL
            } else {
                errno::EBADF
            });
            return refused;
        }
        // SAFETY: an open stream of the table, used only under its lock.
        work(unsafe { &mut (*stream).stream })
    })
}

/// Writes out every stream's buffer and gives back what reading streams read ahead, as `exit`
/// and `ExitProcess` do, unless another thread keeps the streams' lock for 100 ms (blocked
/// reading `stdin`, say): the process ends then rather than waiting for it.
pub fn flush_all_at_exit() {
    let deadline =
        linux::clock_nanoseconds(linux::CLOCK_MONOTONIC).saturating_add(EXIT_LOCK_WAIT_NANOSECONDS);
    let past_deadline = || linux::clock_nanoseconds(linux::CLOCK_MONOTONIC) >= deadline;
    let _ = STREAMS.try_with(past_deadline, |streams| {
        streams.each_open(|file| {
            let _ = file.flush();
        })
    });
}

fn flush_output_streams() {
    STREAMS.with(|streams| {
        streams.each_open(|file| {
            let _ = file.flush_output();
        })
    });
}

fn open_flags(mode: Mode) -> usize {
    let access = match (mode.read, mode.write) {
        (true, true) => linux::O_RDWR,
        (false, true) => linux::O_WRONLY,
        _ => linux::O_RDONLY,
    };
    let flag = |wanted: bool, flag: usize| if wanted { flag } else { 0 };
    access
        | flag(mode.create, linux::O_CREAT)
        | flag(mode.truncate, linux::O_TRUNC)
        | flag(mode.append, O_APPEND)
        | flag(mode.exclusive, linux::O_EXCL)
}

fn refuse_linux(errno: u16) -> *mut File {
    set_errno(errno_for_linux(errno));
    ptr::null_mut()
}

fn open(
    path: impl Iterator<Item = char>,
    mode_letters: impl Iterator<Item = u32> + Clone,
) -> *mut File {
    let Some(mut mode) = Mode::parse(mode_letters.clone()) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    let explicit = mode_letters
        .take_while(|letter| *letter != u32::from(b','))
        .any(|letter| letter == u32::from(b'b') || letter == u32::from(b't'));
    if !explicit && binary_by_default() {
        mode.text = false;
    }
    let mut eggos_path = [0u8; text::PATH_CAPACITY];
    let Some(length) = text::eggos_path(path, &mut eggos_path) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    let fd = match linux::open(&eggos_path[..length], open_flags(mode)) {
        Ok(fd) => fd,
        Err(error) => return refuse_linux(error.0),
    };
    let directory = linux::fstat(fd)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY);
    if directory {
        let _ = linux::close(fd);
        return refuse_linux(LINUX_EACCES);
    }
    if mode.temporary
        && let Err(error) = linux::remove(&eggos_path[..length], 0)
    {
        let _ = linux::close(fd);
        return refuse_linux(error.0);
    }
    let block = new_stream(Stream::new(
        Descriptor { fd: Some(fd) },
        mode,
        Buffering::Full,
    ));
    if block.is_null() {
        let _ = linux::close(fd);
        set_errno(errno::ENOMEM);
        return block;
    }
    let placed = STREAMS.with(|streams| {
        let free = streams.open[STANDARD_STREAMS..]
            .iter_mut()
            .find(|slot| slot.is_null())?;
        *free = block;
        Some(())
    });
    if placed.is_none() {
        let _ = linux::close(fd);
        // SAFETY: the block this call allocated, which nothing else holds.
        unsafe { process_heap::free(block.cast()) };
        set_errno(errno::EMFILE);
        return ptr::null_mut();
    }
    block
}

unsafe fn narrow<'a>(text: *const u8) -> Option<&'a [u8]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!text.is_null()).then(|| unsafe { core::ffi::CStr::from_ptr(text.cast()) }.to_bytes())
}

unsafe fn wide<'a>(text: *const u16) -> Option<&'a [u16]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!text.is_null()).then(|| unsafe { super::super::file::wide_string(text) })
}

unsafe extern "efiapi" fn fopen(path: *const u8, mode: *const u8) -> *mut File {
    // SAFETY: `fopen`'s contract: a NUL-terminated path.
    let path = unsafe { narrow(path) };
    // SAFETY: and a NUL-terminated mode.
    let mode = unsafe { narrow(mode) };
    let (Some(path), Some(mode)) = (path, mode) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    open(text::utf8_chars(path), letters(mode))
}

unsafe extern "efiapi" fn wfopen(path: *const u16, mode: *const u16) -> *mut File {
    // SAFETY: `_wfopen`'s contract: a NUL-terminated path.
    let path = unsafe { wide(path) };
    // SAFETY: and a NUL-terminated mode.
    let mode = unsafe { wide(mode) };
    let (Some(path), Some(mode)) = (path, mode) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    open(
        text::utf16_chars(path),
        mode.iter().map(|&unit| u32::from(unit)),
    )
}

unsafe fn opened(out: *mut *mut File, stream: *mut File) -> i32 {
    // SAFETY: the caller checked `out`: a writable `FILE *`.
    unsafe { out.write(stream) };
    if stream.is_null() {
        super::runtime::errno_value()
    } else {
        0
    }
}

unsafe extern "efiapi" fn fopen_s(out: *mut *mut File, path: *const u8, mode: *const u8) -> i32 {
    if out.is_null() {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    // SAFETY: `fopen_s`'s contract: NUL-terminated strings.
    let stream = unsafe { fopen(path, mode) };
    // SAFETY: and a writable `FILE *`.
    unsafe { opened(out, stream) }
}

unsafe extern "efiapi" fn wfopen_s(out: *mut *mut File, path: *const u16, mode: *const u16) -> i32 {
    if out.is_null() {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    // SAFETY: `_wfopen_s`'s contract: NUL-terminated strings.
    let stream = unsafe { wfopen(path, mode) };
    // SAFETY: and a writable `FILE *`.
    unsafe { opened(out, stream) }
}

unsafe extern "efiapi" fn fclose(stream: *mut File) -> i32 {
    let closed = STREAMS.with(|streams| {
        let stream = streams.resolve(stream);
        let index = streams
            .open
            .iter()
            .position(|&open| open == stream && !stream.is_null())?;
        streams.open[index] = ptr::null_mut();
        if let Some(closed) = streams.standard_closed.get_mut(index) {
            *closed = true;
        }
        // SAFETY: an open stream of the table, which is now out of it.
        let file = unsafe { &mut (*stream).stream };
        let flushed = file.flush();
        let descriptor = *file.backing();
        Some((flushed, descriptor, stream))
    });
    let Some((flushed, descriptor, stream)) = closed else {
        set_errno(errno::EBADF);
        return END_OF_FILE;
    };
    // SAFETY: the stream's block, out of the table, which nothing else holds.
    unsafe { process_heap::free(stream.cast()) };
    let closed = descriptor.fd.map_or(Ok(0), linux::close);
    match (flushed, closed) {
        (Ok(()), Ok(_)) => 0,
        (Err(error), _) | (_, Err(eggos_runtime::errno::Errno(error))) => {
            set_errno(errno_for_linux(error));
            END_OF_FILE
        }
    }
}

fn byte_count(size: usize, count: usize) -> Option<usize> {
    let total = size.checked_mul(count);
    if total.is_none() {
        set_errno(errno::EINVAL);
    }
    total
}

unsafe extern "efiapi" fn fread(
    buffer: *mut u8,
    size: usize,
    count: usize,
    stream: *mut File,
) -> usize {
    let Some(total) = byte_count(size, count).filter(|total| *total > 0) else {
        return 0;
    };
    if buffer.is_null() || total > isize::MAX as usize {
        set_errno(errno::EINVAL);
        return 0;
    }
    // SAFETY: `fread`'s contract: `size * count` writable bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, total) };
    with_stream(stream, 0, |file| file.read(out) / size)
}

unsafe extern "efiapi" fn fwrite(
    buffer: *const u8,
    size: usize,
    count: usize,
    stream: *mut File,
) -> usize {
    let Some(total) = byte_count(size, count).filter(|total| *total > 0) else {
        return 0;
    };
    if buffer.is_null() || total > isize::MAX as usize {
        set_errno(errno::EINVAL);
        return 0;
    }
    // SAFETY: `fwrite`'s contract: `size * count` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(buffer, total) };
    with_stream(stream, 0, |file| file.write(bytes) / size)
}

unsafe extern "efiapi" fn fgetc(stream: *mut File) -> i32 {
    with_stream(stream, END_OF_FILE, |file| {
        file.get_byte().map_or(END_OF_FILE, i32::from)
    })
}

unsafe extern "efiapi" fn fputc(character: i32, stream: *mut File) -> i32 {
    let byte = character as u8;
    with_stream(stream, END_OF_FILE, |file| match file.write(&[byte]) {
        1 => i32::from(byte),
        _ => END_OF_FILE,
    })
}

unsafe extern "efiapi" fn ungetc(character: i32, stream: *mut File) -> i32 {
    if character == END_OF_FILE {
        return END_OF_FILE;
    }
    let byte = character as u8;
    with_stream(stream, END_OF_FILE, |file| match file.unget(byte) {
        true => i32::from(byte),
        false => END_OF_FILE,
    })
}

unsafe extern "efiapi" fn fgets(buffer: *mut u8, size: i32, stream: *mut File) -> *mut u8 {
    let Ok(size) = usize::try_from(size) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    if buffer.is_null() || size == 0 {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    }
    // SAFETY: `fgets`'s contract: `size` writable bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size) };
    let filled = with_stream(stream, None, |file| {
        let mut filled = 0;
        while filled + 1 < out.len() {
            let Some(byte) = file.get_byte() else {
                break;
            };
            out[filled] = byte;
            filled += 1;
            if byte == b'\n' {
                break;
            }
        }
        Some(filled)
    });
    match filled {
        Some(filled) if filled > 0 || size == 1 => {
            out[filled] = 0;
            buffer
        }
        _ => ptr::null_mut(),
    }
}

unsafe fn put_text(text: *const u8, stream: *mut File, newline: bool) -> i32 {
    // SAFETY: the caller's duty: a NUL-terminated string.
    let Some(text) = (unsafe { narrow(text) }) else {
        set_errno(errno::EINVAL);
        return END_OF_FILE;
    };
    with_stream(stream, END_OF_FILE, |file| {
        let mut whole = file.write(text) == text.len();
        if newline {
            whole &= file.write(b"\n") == 1;
        }
        if whole { 0 } else { END_OF_FILE }
    })
}

unsafe extern "efiapi" fn fputs(text: *const u8, stream: *mut File) -> i32 {
    // SAFETY: `fputs`'s contract.
    unsafe { put_text(text, stream, false) }
}

unsafe extern "efiapi" fn puts(text: *const u8) -> i32 {
    // SAFETY: `puts`'s contract; `stdout` is an open stream.
    unsafe { put_text(text, standard(1), true) }
}

unsafe extern "efiapi" fn fflush(stream: *mut File) -> i32 {
    if stream.is_null() {
        flush_output_streams();
        return 0;
    }
    with_stream(stream, END_OF_FILE, |file| match file.flush() {
        Ok(()) => 0,
        Err(error) => {
            set_errno(errno_for_linux(error));
            END_OF_FILE
        }
    })
}

fn origin(origin: i32) -> Option<Origin> {
    match origin {
        0 => Some(Origin::Start),
        1 => Some(Origin::Current),
        2 => Some(Origin::End),
        _ => None,
    }
}

fn seek(stream: *mut File, offset: i64, from: i32) -> i32 {
    let Some(from) = origin(from) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    with_stream(stream, -1, |file| match file.seek(offset, from) {
        Ok(()) => 0,
        Err(error) => {
            set_errno(errno_for_linux(error));
            -1
        }
    })
}

fn position(stream: *mut File) -> i64 {
    with_stream(stream, -1, |file| match file.position() {
        Ok(position) => position as i64,
        Err(error) => {
            set_errno(errno_for_linux(error));
            -1
        }
    })
}

unsafe extern "efiapi" fn fseek(stream: *mut File, offset: i32, from: i32) -> i32 {
    seek(stream, i64::from(offset), from)
}

unsafe extern "efiapi" fn fseeki64(stream: *mut File, offset: i64, from: i32) -> i32 {
    seek(stream, offset, from)
}

unsafe extern "efiapi" fn ftell(stream: *mut File) -> i32 {
    match i32::try_from(position(stream)) {
        Ok(position) => position,
        Err(_) => {
            set_errno(errno::EINVAL);
            -1
        }
    }
}

unsafe extern "efiapi" fn ftelli64(stream: *mut File) -> i64 {
    position(stream)
}

unsafe extern "efiapi" fn rewind(stream: *mut File) {
    with_stream(stream, (), |file| {
        let _ = file.seek(0, Origin::Start);
        file.clear_indicators();
    })
}

unsafe extern "efiapi" fn fgetpos(stream: *mut File, out: *mut i64) -> i32 {
    if out.is_null() {
        set_errno(errno::EINVAL);
        return -1;
    }
    let position = position(stream);
    if position < 0 {
        return -1;
    }
    // SAFETY: `fgetpos`'s contract: a writable `fpos_t`, a 64-bit offset on Windows.
    unsafe { out.write_unaligned(position) };
    0
}

unsafe extern "efiapi" fn fsetpos(stream: *mut File, at: *const i64) -> i32 {
    if at.is_null() {
        set_errno(errno::EINVAL);
        return -1;
    }
    // SAFETY: `fsetpos`'s contract: a readable `fpos_t` from `fgetpos`.
    let offset = unsafe { at.read_unaligned() };
    seek(stream, offset, 0)
}

unsafe extern "efiapi" fn feof(stream: *mut File) -> i32 {
    with_stream(stream, 0, |file| i32::from(file.end_of_file()))
}

unsafe extern "efiapi" fn ferror(stream: *mut File) -> i32 {
    with_stream(stream, 0, |file| i32::from(file.error()))
}

unsafe extern "efiapi" fn clearerr(stream: *mut File) {
    with_stream(stream, (), |file| file.clear_indicators())
}

unsafe extern "efiapi" fn fileno(stream: *mut File) -> i32 {
    with_stream(stream, -1, |file| {
        file.backing().fd.map_or(-1, |fd| fd as i32)
    })
}

fn standard(index: usize) -> *mut File {
    STREAMS.with(|streams| standard_stream(streams, index))
}

extern "efiapi" fn standard_stream_at(index: u32) -> *mut File {
    match index as usize {
        index if index < STANDARD_STREAMS => standard(index),
        _ => {
            set_errno(errno::EINVAL);
            ptr::null_mut()
        }
    }
}

fn remove_path(path: impl Iterator<Item = char>) -> i32 {
    let mut eggos_path = [0u8; text::PATH_CAPACITY];
    let Some(length) = text::eggos_path(path, &mut eggos_path) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    match linux::remove(&eggos_path[..length], 0) {
        Ok(_) => 0,
        Err(error) => {
            set_errno(errno_for_linux(error.0));
            -1
        }
    }
}

unsafe extern "efiapi" fn remove(path: *const u8) -> i32 {
    // SAFETY: `remove`'s contract: a NUL-terminated path.
    let Some(path) = (unsafe { narrow(path) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    remove_path(text::utf8_chars(path))
}

unsafe extern "efiapi" fn wremove(path: *const u16) -> i32 {
    // SAFETY: `_wremove`'s contract: a NUL-terminated path.
    let Some(path) = (unsafe { wide(path) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    remove_path(text::utf16_chars(path))
}

struct VaList(*const u64);

impl VaList {
    unsafe fn new(list: *const u64) -> Self {
        Self(list)
    }
}

impl Arguments for VaList {
    fn integer(&mut self) -> u64 {
        // SAFETY: `new`'s promise: a slot for each argument the format reads.
        let value = unsafe { self.0.read_unaligned() };
        self.0 = self.0.wrapping_add(1);
        value
    }

    fn double(&mut self) -> f64 {
        f64::from_bits(self.integer())
    }
}

struct StreamOutput<'a> {
    file: &'a mut Buffered,
    failed: bool,
    units: usize,
}

impl Output for StreamOutput<'_> {
    fn write(&mut self, bytes: &[u8]) {
        self.failed |= self.file.write(bytes) != bytes.len();
        self.units += bytes
            .iter()
            .map(|&byte| match byte {
                0x80..=0xbf => 0,
                0xf0..=0xff => 2,
                _ => 1,
            })
            .sum::<usize>();
    }
}

struct BufferOutput<T> {
    start: *mut T,
    capacity: usize,
    length: usize,
    decoder: text::Utf8Decoder,
}

impl<T: Copy + Default> BufferOutput<T> {
    fn new(start: *mut T, capacity: usize) -> Self {
        Self {
            start,
            capacity: if start.is_null() { 0 } else { capacity },
            length: 0,
            decoder: text::Utf8Decoder::default(),
        }
    }

    fn push(&mut self, unit: T) {
        if self.length < self.capacity {
            // SAFETY: the caller of the formatting function passed `capacity` writable units.
            unsafe { self.start.wrapping_add(self.length).write(unit) };
        }
        self.length += 1;
    }

    fn terminate_at(&mut self, at: usize) {
        if at < self.capacity {
            // SAFETY: as above.
            unsafe { self.start.wrapping_add(at).write(T::default()) };
        }
    }
}

impl Output for BufferOutput<u8> {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }
}

impl BufferOutput<u16> {
    fn push_character(&mut self, character: char) {
        let mut units = [0u16; 2];
        for &unit in character.encode_utf16(&mut units).iter() {
            self.push(unit);
        }
    }
}

impl Output for BufferOutput<u16> {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            for character in self.decoder.push(byte).into_iter().flatten() {
                self.push_character(character);
            }
        }
    }
}

trait FinishedOutput: Output {
    fn finish(&mut self);
}

impl FinishedOutput for BufferOutput<u8> {
    fn finish(&mut self) {}
}

impl FinishedOutput for BufferOutput<u16> {
    fn finish(&mut self) {
        if let Some(replacement) = self.decoder.finish() {
            self.push_character(replacement);
        }
    }
}

struct Utf8Format {
    block: *mut u8,
    length: usize,
}

impl Utf8Format {
    unsafe fn of(format: *const u16) -> Option<Utf8Format> {
        // SAFETY: the caller's duty: null or a NUL-terminated string.
        let units = unsafe { wide(format) }?;
        let length = text::utf8_length(units);
        let block = process_heap::allocate_with(length.max(1), false);
        if block.is_null() {
            set_errno(errno::ENOMEM);
            return None;
        }
        // SAFETY: a fresh block of at least `length` bytes, used only here.
        let out = unsafe { core::slice::from_raw_parts_mut(block, length) };
        let (written, _) = text::write_utf8(units, out);
        Some(Utf8Format {
            block,
            length: written,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the block holds `length` bytes written by `of`.
        unsafe { core::slice::from_raw_parts(self.block, self.length) }
    }
}

impl Drop for Utf8Format {
    fn drop(&mut self) {
        // SAFETY: the block `of` allocated, which nothing else holds.
        unsafe { process_heap::free(self.block) };
    }
}

fn narrow_dialect() -> Dialect {
    Dialect::Windows {
        wide_strings: false,
    }
}

fn wide_dialect(options: u64) -> Dialect {
    Dialect::Windows {
        wide_strings: options & LEGACY_WIDE_SPECIFIERS != 0,
    }
}

unsafe fn render(
    dialect: Dialect,
    format: &[u8],
    arguments: *const u64,
    output: &mut impl Output,
) -> usize {
    // SAFETY: the caller's duty: a `va_list` with an argument for each conversion.
    let mut arguments = unsafe { VaList::new(arguments) };
    // SAFETY: as above; string arguments are what their conversions say.
    unsafe { c_format::format_in(dialect, format, &mut arguments, output) }
}

unsafe fn print_to_stream(
    stream: *mut File,
    dialect: Dialect,
    format: &[u8],
    arguments: *const u64,
    count_units: bool,
) -> i32 {
    with_stream(stream, -1, |file| {
        let mut output = StreamOutput {
            file,
            failed: false,
            units: 0,
        };
        // SAFETY: the caller's duty: a `va_list` with an argument for each conversion.
        let bytes = unsafe { render(dialect, format, arguments, &mut output) };
        match output.failed {
            true => -1,
            false if count_units => output.units as i32,
            false => bytes as i32,
        }
    })
}

unsafe extern "efiapi" fn vfprintf(
    _options: u64,
    stream: *mut File,
    format: *const u8,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vfprintf`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { narrow(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    // SAFETY: its contract: an open stream and a matching `va_list`.
    unsafe { print_to_stream(stream, narrow_dialect(), format, arguments, false) }
}

unsafe extern "efiapi" fn vfwprintf(
    options: u64,
    stream: *mut File,
    format: *const u16,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vfwprintf`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { Utf8Format::of(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    // SAFETY: its contract: an open stream and a matching `va_list`.
    unsafe {
        print_to_stream(
            stream,
            wide_dialect(options),
            format.bytes(),
            arguments,
            true,
        )
    }
}

unsafe fn format_into<T: Copy + Default>(
    dialect: Dialect,
    format: &[u8],
    arguments: *const u64,
    buffer: *mut T,
    capacity: usize,
) -> BufferOutput<T>
where
    BufferOutput<T>: FinishedOutput,
{
    let mut output = BufferOutput::new(buffer, capacity);
    // SAFETY: the caller's duty: `capacity` writable units and a matching `va_list`.
    unsafe { render(dialect, format, arguments, &mut output) };
    output.finish();
    output
}

fn counted_result<T: Copy + Default>(
    output: &mut BufferOutput<T>,
    count: usize,
    options: u64,
) -> i32 {
    let ending = counted_print::ending(
        Convention::of_options(options),
        output.length,
        count,
        !output.start.is_null(),
    );
    if let Some(at) = ending.terminator_at {
        output.terminate_at(at);
    }
    match ending.fault {
        Some(Fault::InvalidParameter) => set_errno(errno::EINVAL),
        Some(Fault::Range) => set_errno(errno::ERANGE),
        None => {}
    }
    ending.result
}

fn checked_result<T: Copy + Default>(
    output: &mut BufferOutput<T>,
    size: usize,
    limit: usize,
) -> i32 {
    let produced = output.length;
    if output.capacity == 0 || size == 0 {
        set_errno(errno::EINVAL);
        return -1;
    }
    let room = size - 1;
    if limit > room && produced > room {
        output.terminate_at(0);
        set_errno(errno::ERANGE);
        return -1;
    }
    let kept = produced.min(limit).min(room);
    output.terminate_at(kept);
    if produced > kept { -1 } else { produced as i32 }
}

unsafe extern "efiapi" fn vsprintf(
    options: u64,
    buffer: *mut u8,
    count: usize,
    format: *const u8,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vsprintf`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { narrow(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    // SAFETY: its contract: `count` writable bytes and a matching `va_list`.
    let mut output = unsafe { format_into(narrow_dialect(), format, arguments, buffer, count) };
    counted_result(&mut output, count, options)
}

unsafe extern "efiapi" fn vsprintf_s(
    _options: u64,
    buffer: *mut u8,
    size: usize,
    format: *const u8,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vsprintf_s`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { narrow(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    // SAFETY: its contract: `size` writable bytes and a matching `va_list`.
    let mut output = unsafe { format_into(narrow_dialect(), format, arguments, buffer, size) };
    checked_result(&mut output, size, usize::MAX)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn vsnprintf_s(
    _options: u64,
    buffer: *mut u8,
    size: usize,
    limit: usize,
    format: *const u8,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vsnprintf_s`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { narrow(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    // SAFETY: its contract: `size` writable bytes and a matching `va_list`.
    let mut output = unsafe { format_into(narrow_dialect(), format, arguments, buffer, size) };
    let limit = if limit == TRUNCATE {
        size.saturating_sub(1)
    } else {
        limit
    };
    checked_result(&mut output, size, limit)
}

unsafe extern "efiapi" fn vswprintf(
    options: u64,
    buffer: *mut u16,
    count: usize,
    format: *const u16,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vswprintf`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { Utf8Format::of(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    let dialect = wide_dialect(options);
    // SAFETY: its contract: `count` writable units and a matching `va_list`.
    let mut output = unsafe { format_into(dialect, format.bytes(), arguments, buffer, count) };
    counted_result(&mut output, count, options)
}

unsafe extern "efiapi" fn vswprintf_s(
    options: u64,
    buffer: *mut u16,
    size: usize,
    format: *const u16,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vswprintf_s`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { Utf8Format::of(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    let dialect = wide_dialect(options);
    // SAFETY: its contract: `size` writable units and a matching `va_list`.
    let mut output = unsafe { format_into(dialect, format.bytes(), arguments, buffer, size) };
    checked_result(&mut output, size, usize::MAX)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn vsnwprintf_s(
    options: u64,
    buffer: *mut u16,
    size: usize,
    limit: usize,
    format: *const u16,
    _locale: usize,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `__stdio_common_vsnwprintf_s`'s contract: a NUL-terminated format.
    let Some(format) = (unsafe { Utf8Format::of(format) }) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    let dialect = wide_dialect(options);
    // SAFETY: its contract: `size` writable units and a matching `va_list`.
    let mut output = unsafe { format_into(dialect, format.bytes(), arguments, buffer, size) };
    let limit = if limit == TRUNCATE {
        size.saturating_sub(1)
    } else {
        limit
    };
    checked_result(&mut output, size, limit)
}

unsafe extern "efiapi" fn fdopen(fd: i32, mode: *const u8) -> *mut File {
    // SAFETY: `_fdopen`'s contract: a NUL-terminated mode.
    let Some(mode_letters) = (unsafe { narrow(mode) }) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    let Some(mut parsed) = Mode::parse(letters(mode_letters)) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    let explicit = mode_letters
        .iter()
        .take_while(|letter| **letter != b',')
        .any(|letter| *letter == b'b' || *letter == b't');
    if !explicit && binary_by_default() {
        parsed.text = false;
    }
    let Ok(fd) = usize::try_from(fd) else {
        set_errno(errno::EBADF);
        return ptr::null_mut();
    };
    let block = new_stream(Stream::new(
        Descriptor { fd: Some(fd) },
        parsed,
        Buffering::Full,
    ));
    if block.is_null() {
        set_errno(errno::ENOMEM);
        return block;
    }
    let placed = STREAMS.with(|streams| {
        let free = streams.open[STANDARD_STREAMS..]
            .iter_mut()
            .find(|slot| slot.is_null())?;
        *free = block;
        Some(())
    });
    if placed.is_none() {
        // SAFETY: the block this call allocated, which nothing else holds.
        unsafe { process_heap::free(block.cast()) };
        set_errno(errno::EMFILE);
        return ptr::null_mut();
    }
    block
}

extern "efiapi" fn open_os_handle(handle: isize, _flags: i32) -> i32 {
    match fd_of(handle as usize) {
        Some(fd) => fd as i32,
        None => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

const SECURE_SCANF: u64 = 0x1;

fn write_integer(target: usize, value: u64, size: crate::scan::Size) {
    use crate::scan::Size;
    // SAFETY: the program's argument for the conversion points at an integer of its size.
    unsafe {
        match size {
            Size::Byte => (target as *mut u8).write_unaligned(value as u8),
            Size::Short => (target as *mut u16).write_unaligned(value as u16),
            Size::Int => (target as *mut u32).write_unaligned(value as u32),
            Size::Wide => (target as *mut u64).write_unaligned(value),
        }
    }
}

fn store_field(
    field: crate::scan::Field<'_>,
    take: &mut dyn FnMut() -> usize,
    secure: bool,
) -> bool {
    use crate::scan::Field;
    let target = take();
    if target == 0 {
        return false;
    }
    match field {
        Field::Signed(value, size) => write_integer(target, value as u64, size),
        Field::Unsigned(value, size) => write_integer(target, value, size),
        Field::Count(count, size) => write_integer(target, count as u64, size),
        // SAFETY: the argument for `%lf` and its family points at a `double`.
        Field::Float(value, true) => unsafe { (target as *mut f64).write_unaligned(value) },
        // SAFETY: the argument for `%f` and its family points at a `float`.
        Field::Float(value, false) => unsafe { (target as *mut f32).write_unaligned(value as f32) },
        Field::Characters(bytes) => {
            if secure && (take() as u32 as usize) < bytes.len() {
                return false;
            }
            // SAFETY: the argument for `%c` points at room for its characters.
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len()) };
        }
        Field::Text(bytes) => {
            if secure {
                let room = take() as u32 as usize;
                if room <= bytes.len() {
                    if room > 0 {
                        // SAFETY: the buffer holds `room` bytes, at least one.
                        unsafe { (target as *mut u8).write(0) };
                    }
                    return false;
                }
            }
            // SAFETY: the argument for `%s` and `%[` points at room for the text and a NUL.
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
                (target as *mut u8).add(bytes.len()).write(0);
            }
        }
    }
    true
}

unsafe extern "efiapi" fn stdio_common_vsscanf(
    options: u64,
    buffer: *const u8,
    count: usize,
    format: *const u8,
    _locale: usize,
    arguments: *const usize,
) -> i32 {
    if buffer.is_null() || format.is_null() {
        set_errno(errno::EINVAL);
        return -1;
    }
    let input = match count {
        // SAFETY: `__stdio_common_vsscanf`'s contract: with no count, a NUL-terminated buffer.
        usize::MAX => unsafe { core::ffi::CStr::from_ptr(buffer.cast()) }.to_bytes(),
        count => {
            // SAFETY: with a count, `count` readable bytes.
            let bytes = unsafe { core::slice::from_raw_parts(buffer, count) };
            let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(count);
            &bytes[..end]
        }
    };
    // SAFETY: and a NUL-terminated format.
    let format = unsafe { core::ffi::CStr::from_ptr(format.cast()) }.to_bytes();
    let secure = options & SECURE_SCANF != 0;
    let mut next = 0usize;
    let mut take = || {
        // SAFETY: the `va_list` is a pointer to 8-byte slots, one per argument the format
        // names; the scan reads only as many as its conversions take.
        let value = unsafe { arguments.add(next).read_unaligned() };
        next += 1;
        value
    };
    crate::scan::scan(input, format, &mut |field| {
        store_field(field, &mut take, secure)
    })
}

const STANDARD_OUTPUT: usize = 1;
const FULL_BUFFERING: i32 = 0x0000;
const LINE_BUFFERING: i32 = 0x0040;
const NO_BUFFERING: i32 = 0x0004;

unsafe extern "efiapi" fn putchar(character: i32) -> i32 {
    // SAFETY: standard output is the table's own stream, or null, which `fputc` refuses.
    unsafe { fputc(character, standard(STANDARD_OUTPUT)) }
}

unsafe extern "efiapi" fn setvbuf(
    stream: *mut File,
    _buffer: *mut u8,
    mode: i32,
    size: usize,
) -> i32 {
    let buffering = match mode {
        FULL_BUFFERING | LINE_BUFFERING => Buffering::Full,
        NO_BUFFERING => Buffering::None,
        _ => {
            set_errno(errno::EINVAL);
            return -1;
        }
    };
    if buffering == Buffering::Full && !(2..=i32::MAX as usize).contains(&size) {
        set_errno(errno::EINVAL);
        return -1;
    }
    with_stream(stream, -1, |file| match file.set_buffering(buffering) {
        Ok(()) => 0,
        Err(error) => {
            set_errno(errno_for_linux(error));
            -1
        }
    })
}

extern "efiapi" fn iob_func() -> *mut u64 {
    IOB.0.get().cast()
}

unsafe extern "efiapi" fn print(format: *const u8, arguments: *const u64) -> i32 {
    // SAFETY: `printf`'s contract: a NUL-terminated format and an argument for each conversion.
    unsafe { vfprintf(0, standard(STANDARD_OUTPUT), format, 0, arguments) }
}

unsafe extern "efiapi" fn print_to_file(
    stream: *mut File,
    format: *const u8,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `fprintf`'s contract: an open stream, a NUL-terminated format and an argument for
    // each conversion.
    unsafe { vfprintf(0, stream, format, 0, arguments) }
}

unsafe extern "efiapi" fn print_into(
    buffer: *mut u8,
    format: *const u8,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `sprintf`'s contract: room for the whole output, a NUL-terminated format and an
    // argument for each conversion.
    unsafe {
        vsprintf(
            counted_print::LEGACY_VSPRINTF_NULL_TERMINATION,
            buffer,
            usize::MAX,
            format,
            0,
            arguments,
        )
    }
}

unsafe extern "efiapi" fn print_into_counted_legacy(
    buffer: *mut u8,
    count: usize,
    format: *const u8,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `_snprintf`'s contract: `count` writable bytes, a NUL-terminated format and an
    // argument for each conversion.
    unsafe {
        vsprintf(
            counted_print::LEGACY_VSPRINTF_NULL_TERMINATION,
            buffer,
            count,
            format,
            0,
            arguments,
        )
    }
}

unsafe extern "efiapi" fn print_into_counted(
    buffer: *mut u8,
    count: usize,
    format: *const u8,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `snprintf`'s contract: `count` writable bytes, a NUL-terminated format and an
    // argument for each conversion.
    unsafe {
        vsprintf(
            counted_print::STANDARD_SNPRINTF_BEHAVIOR,
            buffer,
            count,
            format,
            0,
            arguments,
        )
    }
}

unsafe extern "efiapi" fn count_print(format: *const u8, arguments: *const u64) -> i32 {
    // SAFETY: `_scprintf`'s contract: a NUL-terminated format and an argument for each
    // conversion; nothing is written.
    unsafe {
        vsprintf(
            counted_print::STANDARD_SNPRINTF_BEHAVIOR,
            ptr::null_mut(),
            0,
            format,
            0,
            arguments,
        )
    }
}

unsafe extern "efiapi" fn print_wide(format: *const u16, arguments: *const u64) -> i32 {
    // SAFETY: `wprintf`'s contract: a NUL-terminated format and an argument for each conversion.
    unsafe {
        vfwprintf(
            LEGACY_WIDE_SPECIFIERS,
            standard(STANDARD_OUTPUT),
            format,
            0,
            arguments,
        )
    }
}

unsafe extern "efiapi" fn print_wide_to_file(
    stream: *mut File,
    format: *const u16,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `fwprintf`'s contract: an open stream, a NUL-terminated format and an argument for
    // each conversion.
    unsafe { vfwprintf(LEGACY_WIDE_SPECIFIERS, stream, format, 0, arguments) }
}

unsafe extern "efiapi" fn print_wide_into_counted_legacy(
    buffer: *mut u16,
    count: usize,
    format: *const u16,
    arguments: *const u64,
) -> i32 {
    // SAFETY: `_snwprintf`'s contract: `count` writable units, a NUL-terminated format and an
    // argument for each conversion.
    unsafe {
        vswprintf(
            LEGACY_WIDE_SPECIFIERS | counted_print::LEGACY_VSPRINTF_NULL_TERMINATION,
            buffer,
            count,
            format,
            0,
            arguments,
        )
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"_fdopen" => fdopen,
    b"putchar" => putchar,
    b"setvbuf" => setvbuf,
    b"_open_osfhandle" => open_os_handle,
    b"__stdio_common_vsscanf" => stdio_common_vsscanf,
    b"__acrt_iob_func" => standard_stream_at,
    b"fopen" => fopen,
    b"_wfopen" => wfopen,
    b"fopen_s" => fopen_s,
    b"_wfopen_s" => wfopen_s,
    b"fclose" => fclose,
    b"fread" => fread,
    b"fwrite" => fwrite,
    b"fgetc" => fgetc,
    b"getc" => fgetc,
    b"fputc" => fputc,
    b"putc" => fputc,
    b"ungetc" => ungetc,
    b"fgets" => fgets,
    b"fputs" => fputs,
    b"puts" => puts,
    b"fflush" => fflush,
    b"fseek" => fseek,
    b"_fseeki64" => fseeki64,
    b"ftell" => ftell,
    b"_ftelli64" => ftelli64,
    b"rewind" => rewind,
    b"fgetpos" => fgetpos,
    b"fsetpos" => fsetpos,
    b"feof" => feof,
    b"ferror" => ferror,
    b"clearerr" => clearerr,
    b"_fileno" => fileno,
    b"remove" => remove,
    b"_wremove" => wremove,
    b"__stdio_common_vfprintf" => vfprintf,
    b"__stdio_common_vfprintf_s" => vfprintf,
    b"__stdio_common_vfwprintf" => vfwprintf,
    b"__stdio_common_vfwprintf_s" => vfwprintf,
    b"__stdio_common_vsprintf" => vsprintf,
    b"__stdio_common_vsprintf_s" => vsprintf_s,
    b"__stdio_common_vsnprintf_s" => vsnprintf_s,
    b"__stdio_common_vswprintf" => vswprintf,
    b"__stdio_common_vswprintf_s" => vswprintf_s,
    b"__stdio_common_vsnwprintf_s" => vsnwprintf_s,
];

/// The old C runtime's (`msvcrt.dll`, D189) stream functions: its `FILE` array, and the
/// `printf` family as functions of their own, variadic or taking a `va_list`, where the
/// Universal C Runtime has `__stdio_common_*`.
pub(super) const MSVCRT_EXPORTS: &[Export] = exports![
    b"__iob_func" => iob_func,
    b"printf" => print [variadic_after_one],
    b"vprintf" => print,
    b"fprintf" => print_to_file [variadic_after_two],
    b"vfprintf" => print_to_file,
    b"sprintf" => print_into [variadic_after_two],
    b"vsprintf" => print_into,
    b"_snprintf" => print_into_counted_legacy [variadic_after_three],
    b"_vsnprintf" => print_into_counted_legacy,
    b"snprintf" => print_into_counted [variadic_after_three],
    b"vsnprintf" => print_into_counted,
    b"_scprintf" => count_print [variadic_after_one],
    b"_vscprintf" => count_print,
    b"wprintf" => print_wide [variadic_after_one],
    b"vwprintf" => print_wide,
    b"fwprintf" => print_wide_to_file [variadic_after_two],
    b"vfwprintf" => print_wide_to_file,
    b"_snwprintf" => print_wide_into_counted_legacy [variadic_after_three],
    b"_vsnwprintf" => print_wide_into_counted_legacy,
];
