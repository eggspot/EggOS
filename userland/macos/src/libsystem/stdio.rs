//! `<stdio.h>`: streams over file descriptors with a buffer each (standard output is
//! line-buffered on a terminal and fully buffered otherwise, standard error unbuffered),
//! `__stdinp`, `__stdoutp` and `__stderrp`, the `printf` family and the character and line
//! functions.

use super::variadic::{RawVaList, VaList, variadic};
use super::{c_string, errno_from, file, heap, set_errno};
use crate::darwin::{self, EBADF, EINVAL};
use crate::format::{self, Output};
use crate::linux;
use crate::stream_buffer::StreamBuffer;
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;

const BUFFER_SIZE: usize = 4096;
const EOF: c_int = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Buffering {
    Full,
    Line,
    None,
}

/// A `FILE`: programs see only pointers to it.
#[derive(Debug)]
pub struct Stream {
    fd: c_int,
    buffering: Buffering,
    buffer: StreamBuffer,
    written: usize,
    read_at: usize,
    read_end: usize,
    unread: Option<u8>,
    error: bool,
    end: bool,
    readable: bool,
    writable: bool,
    owned: bool,
    next: *mut Stream,
}

impl Stream {
    const fn standard(fd: c_int, buffering: Buffering) -> Self {
        Self {
            fd,
            buffering,
            buffer: StreamBuffer::none(),
            written: 0,
            read_at: 0,
            read_end: 0,
            unread: None,
            error: false,
            end: false,
            readable: fd == 0,
            writable: fd != 0,
            owned: false,
            next: ptr::null_mut(),
        }
    }
}

static mut STDIN: Stream = Stream::standard(0, Buffering::Line);
static mut STDOUT: Stream = Stream::standard(1, Buffering::Full);
static mut STDERR: Stream = Stream::standard(2, Buffering::None);
static mut STDIN_BUFFER: [u8; BUFFER_SIZE] = [0; BUFFER_SIZE];
static mut STDOUT_BUFFER: [u8; BUFFER_SIZE] = [0; BUFFER_SIZE];
static mut OPEN_STREAMS: *mut Stream = ptr::null_mut();

/// `__stdinp`, `__stdoutp` and `__stderrp`.
pub static mut STDIN_POINTER: *mut Stream = ptr::null_mut();
pub static mut STDOUT_POINTER: *mut Stream = ptr::null_mut();
pub static mut STDERR_POINTER: *mut Stream = ptr::null_mut();

/// Sets up the standard streams.
///
/// # Safety
/// Called once, before the program runs.
pub unsafe fn set_up() {
    let input = standard(&raw mut STDIN);
    input.buffer = StreamBuffer::borrowed((&raw mut STDIN_BUFFER).cast(), BUFFER_SIZE);
    let output = standard(&raw mut STDOUT);
    output.buffer = StreamBuffer::borrowed((&raw mut STDOUT_BUFFER).cast(), BUFFER_SIZE);
    if linux::is_terminal(1) {
        output.buffering = Buffering::Line;
    }
    // SAFETY: the program has not started, so nothing else uses the pointers yet.
    unsafe { STDIN_POINTER = &raw mut STDIN };
    // SAFETY: as above.
    unsafe { STDOUT_POINTER = &raw mut STDOUT };
    // SAFETY: as above.
    unsafe { STDERR_POINTER = &raw mut STDERR };
}

fn standard(stream: *mut Stream) -> &'static mut Stream {
    // SAFETY: one of the three standard streams, used by the program's only thread, one call
    // at a time.
    unsafe { &mut *stream }
}

fn standard_output() -> *mut Stream {
    // SAFETY: the program's only thread reads the pointer it may have changed.
    unsafe { STDOUT_POINTER }
}

fn standard_input() -> *mut Stream {
    // SAFETY: as above.
    unsafe { STDIN_POINTER }
}

fn standard_error() -> *mut Stream {
    // SAFETY: as above.
    unsafe { STDERR_POINTER }
}

fn open_streams() -> *mut *mut Stream {
    &raw mut OPEN_STREAMS
}

/// Flushes every stream's buffered output: at `exit`, and for `fflush(NULL)`.
pub fn flush_all() {
    flush(standard(&raw mut STDOUT));
    flush(standard(&raw mut STDERR));
    // SAFETY: the open-stream list is the layer's, used by the program's only thread.
    let mut stream = unsafe { open_streams().read() };
    // SAFETY: each stream on the list is a live one `new_stream` made.
    while let Some(open) = unsafe { stream.as_mut() } {
        flush(open);
        stream = open.next;
    }
}

fn flush(stream: &mut Stream) -> c_int {
    if stream.written == 0 {
        return 0;
    }
    // SAFETY: the buffer holds `written` bytes of output.
    let bytes = unsafe { core::slice::from_raw_parts(stream.buffer.start, stream.written) };
    stream.written = 0;
    match linux::write_all(stream.fd as usize, bytes) {
        Ok(()) => 0,
        Err(errno) => {
            stream.error = true;
            errno_from(errno, EOF)
        }
    }
}

fn write_bytes(stream: &mut Stream, bytes: &[u8]) -> bool {
    if !stream.writable {
        stream.error = true;
        set_errno(EBADF);
        return false;
    }
    stream.read_at = 0;
    stream.read_end = 0;
    if stream.buffering == Buffering::None || stream.buffer.capacity == 0 {
        return match linux::write_all(stream.fd as usize, bytes) {
            Ok(()) => true,
            Err(errno) => {
                stream.error = true;
                errno_from(errno, false)
            }
        };
    }
    let mut rest = bytes;
    while !rest.is_empty() {
        if stream.written == stream.buffer.capacity && flush(stream) != 0 {
            return false;
        }
        let taken = rest.len().min(stream.buffer.capacity - stream.written);
        // SAFETY: the buffer has room for `taken` more bytes.
        unsafe {
            stream
                .buffer
                .start
                .wrapping_add(stream.written)
                .copy_from_nonoverlapping(rest.as_ptr(), taken)
        };
        stream.written += taken;
        rest = &rest[taken..];
    }
    if stream.buffering == Buffering::Line && bytes.contains(&b'\n') {
        return flush(stream) == 0;
    }
    true
}

fn fill(stream: &mut Stream) -> bool {
    if stream.read_at < stream.read_end {
        return true;
    }
    if !stream.readable || stream.buffer.capacity == 0 {
        stream.error |= !stream.readable;
        return false;
    }
    if stream.fd == 0 {
        flush(standard(&raw mut STDOUT));
    }
    // SAFETY: the buffer holds `capacity` bytes.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(stream.buffer.start, stream.buffer.capacity) };
    match linux::read(stream.fd as usize, buffer) {
        Ok(0) => {
            stream.end = true;
            false
        }
        Ok(read) => {
            stream.read_at = 0;
            stream.read_end = read;
            true
        }
        Err(errno) => {
            stream.error = true;
            errno_from(errno, false)
        }
    }
}

fn read_byte(stream: &mut Stream) -> Option<u8> {
    if let Some(byte) = stream.unread.take() {
        return Some(byte);
    }
    if stream.written > 0 && flush(stream) != 0 {
        return None;
    }
    if !fill(stream) {
        return None;
    }
    // SAFETY: `fill` left at least one unread byte in the buffer.
    let byte = unsafe { stream.buffer.start.wrapping_add(stream.read_at).read() };
    stream.read_at += 1;
    Some(byte)
}

unsafe fn stream<'a>(stream: *mut Stream) -> Option<&'a mut Stream> {
    // SAFETY: the caller's promise.
    unsafe { stream.as_mut() }
}

struct StreamOutput<'a>(&'a mut Stream, bool);

impl Output for StreamOutput<'_> {
    fn write(&mut self, bytes: &[u8]) {
        self.1 &= write_bytes(self.0, bytes);
    }
}

struct BufferOutput {
    buffer: *mut u8,
    room: usize,
    written: usize,
}

impl Output for BufferOutput {
    fn write(&mut self, bytes: &[u8]) {
        let taken = bytes.len().min(self.room.saturating_sub(self.written + 1));
        if taken > 0 {
            // SAFETY: the buffer holds `room` bytes; this keeps one for the NUL.
            unsafe {
                self.buffer
                    .wrapping_add(self.written)
                    .copy_from_nonoverlapping(bytes.as_ptr(), taken)
            };
            self.written += taken;
        }
    }
}

fn count_result(count: usize) -> c_int {
    c_int::try_from(count).unwrap_or_else(|_| {
        set_errno(darwin::ERANGE);
        -1
    })
}

/// # Safety
/// `file` must be a stream, `format` a NUL-terminated string and `arguments` its `va_list`.
pub unsafe extern "C" fn vfprintf(
    file: *mut Stream,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        set_errno(EBADF);
        return -1;
    };
    let mut output = StreamOutput(stream, true);
    let count = formatted(format, arguments, &mut output);
    if output.1 { count_result(count) } else { -1 }
}

fn formatted(format: *const c_char, arguments: RawVaList, output: &mut impl Output) -> usize {
    // SAFETY: the public callers' callers promise a NUL-terminated format.
    let format = unsafe { c_string(format) };
    // SAFETY: and the `va_list` for its arguments.
    let mut arguments = unsafe { VaList::new(arguments) };
    // SAFETY: as above: `%s` arguments are the strings the format says.
    unsafe { format::format(format, &mut arguments, output) }
}

/// # Safety
/// As `vfprintf`, to standard output.
pub unsafe extern "C" fn vprintf(format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { vfprintf(standard_output(), format, arguments) }
}

/// # Safety
/// `buffer` must hold `size` bytes, `format` be a NUL-terminated string and `arguments` its
/// `va_list`.
pub unsafe extern "C" fn vsnprintf(
    buffer: *mut c_char,
    size: usize,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    let mut output = BufferOutput {
        buffer: buffer.cast(),
        room: if buffer.is_null() { 0 } else { size },
        written: 0,
    };
    let count = formatted(format, arguments, &mut output);
    if output.room > 0 {
        // SAFETY: `written` is below `room`, so the NUL fits.
        unsafe { output.buffer.wrapping_add(output.written).write(0) };
    }
    count_result(count)
}

/// # Safety
/// As `vsnprintf`, with a buffer large enough for the result.
pub unsafe extern "C" fn vsprintf(
    buffer: *mut c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { vsnprintf(buffer, usize::MAX / 2, format, arguments) }
}

/// # Safety
/// As `vfprintf`, to the file descriptor `fd`.
pub unsafe extern "C" fn vdprintf(fd: c_int, format: *const c_char, arguments: RawVaList) -> c_int {
    let mut stream = Stream::standard(fd, Buffering::None);
    stream.writable = true;
    let mut output = StreamOutput(&mut stream, true);
    let count = formatted(format, arguments, &mut output);
    if output.1 { count_result(count) } else { -1 }
}

/// # Safety
/// `result` must be writable; otherwise as `vsnprintf`.
pub unsafe extern "C" fn vasprintf(
    result: *mut *mut c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    let mut first = [0u8; 256];
    // SAFETY: `arguments` is read twice: the program's `va_list` is copied first, as C's
    // `va_copy` would, because reading it advances it.
    let copy = unsafe { copy_va_list(arguments) };
    // SAFETY: the caller's promise; `first` holds 256 bytes.
    let length = unsafe { vsnprintf(first.as_mut_ptr().cast(), first.len(), format, copy.raw()) };
    if length < 0 {
        return -1;
    }
    let size = length as usize + 1;
    let block = heap::allocate(size, false).cast::<c_char>();
    if block.is_null() {
        return -1;
    }
    // SAFETY: `block` holds `size` bytes.
    unsafe { vsnprintf(block, size, format, arguments) };
    // SAFETY: the caller's promise.
    unsafe { result.write(block) };
    length
}

#[cfg(target_arch = "aarch64")]
struct VaCopy(RawVaList);

#[cfg(target_arch = "aarch64")]
impl VaCopy {
    fn raw(&self) -> RawVaList {
        self.0
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn copy_va_list(raw: RawVaList) -> VaCopy {
    VaCopy(raw)
}

#[cfg(target_arch = "x86_64")]
struct VaCopy([u64; 3]);

#[cfg(target_arch = "x86_64")]
impl VaCopy {
    fn raw(&self) -> RawVaList {
        self.0.as_ptr().cast_mut().cast()
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn copy_va_list(raw: RawVaList) -> VaCopy {
    // SAFETY: a System V `va_list` is 24 bytes, which the caller's pointer names.
    VaCopy(unsafe { raw.cast::<[u64; 3]>().read_unaligned() })
}

unsafe extern "C" fn printf_with(format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: the variadic entry passes the program's format and arguments.
    unsafe { vprintf(format, arguments) }
}

unsafe extern "C" fn fprintf_with(
    file: *mut Stream,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vfprintf(file, format, arguments) }
}

unsafe extern "C" fn dprintf_with(fd: c_int, format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: as above.
    unsafe { vdprintf(fd, format, arguments) }
}

unsafe extern "C" fn sprintf_with(
    buffer: *mut c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vsprintf(buffer, format, arguments) }
}

unsafe extern "C" fn snprintf_with(
    buffer: *mut c_char,
    size: usize,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vsnprintf(buffer, size, format, arguments) }
}

unsafe extern "C" fn asprintf_with(
    result: *mut *mut c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vasprintf(result, format, arguments) }
}

unsafe extern "C" fn sprintf_chk_with(
    buffer: *mut c_char,
    _flag: c_int,
    size: usize,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above; the result is checked against the buffer's size.
    let length = unsafe { vsnprintf(buffer, size, format, arguments) };
    if length >= 0 && length as usize >= size {
        super::process::fail(b"detected buffer overflow");
    }
    length
}

unsafe extern "C" fn snprintf_chk_with(
    buffer: *mut c_char,
    count: usize,
    _flag: c_int,
    size: usize,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    if count > size {
        super::process::fail(b"detected buffer overflow");
    }
    // SAFETY: as above.
    unsafe { vsnprintf(buffer, count, format, arguments) }
}

/// # Safety
/// As `vsnprintf`, with `size` the buffer's real size.
pub unsafe extern "C" fn vsnprintf_chk(
    buffer: *mut c_char,
    count: usize,
    _flag: c_int,
    size: usize,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    if count > size {
        super::process::fail(b"detected buffer overflow");
    }
    // SAFETY: the caller's promise.
    unsafe { vsnprintf(buffer, count, format, arguments) }
}

variadic!(eggos_macos_printf => printf_with, 1, "x1", "rsi");
variadic!(eggos_macos_fprintf => fprintf_with, 2, "x2", "rdx");
variadic!(eggos_macos_dprintf => dprintf_with, 2, "x2", "rdx");
variadic!(eggos_macos_sprintf => sprintf_with, 2, "x2", "rdx");
variadic!(eggos_macos_snprintf => snprintf_with, 3, "x3", "rcx");
variadic!(eggos_macos_asprintf => asprintf_with, 2, "x2", "rdx");
variadic!(eggos_macos_sprintf_chk => sprintf_chk_with, 4, "x4", "r8");
variadic!(eggos_macos_snprintf_chk => snprintf_chk_with, 5, "x5", "r9");

pub use self::{
    eggos_macos_asprintf as asprintf, eggos_macos_dprintf as dprintf,
    eggos_macos_fprintf as fprintf, eggos_macos_printf as printf, eggos_macos_snprintf as snprintf,
    eggos_macos_snprintf_chk as snprintf_chk, eggos_macos_sprintf as sprintf,
    eggos_macos_sprintf_chk as sprintf_chk,
};

/// # Safety
/// `file` must be a stream and `text` a NUL-terminated string.
pub unsafe extern "C" fn fputs(text: *const c_char, file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return EOF;
    };
    // SAFETY: as above.
    if write_bytes(stream, unsafe { c_string(text) }) {
        0
    } else {
        EOF
    }
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn puts(text: *const c_char) -> c_int {
    // SAFETY: the program keeps `__stdoutp` a stream.
    let Some(stream) = (unsafe { stream(standard_output()) }) else {
        return EOF;
    };
    // SAFETY: the caller's promise.
    if write_bytes(stream, unsafe { c_string(text) }) && write_bytes(stream, b"\n") {
        0
    } else {
        EOF
    }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn fputc(character: c_int, file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return EOF;
    };
    if write_bytes(stream, &[character as u8]) {
        character & 0xff
    } else {
        EOF
    }
}

pub extern "C" fn putchar(character: c_int) -> c_int {
    // SAFETY: standard output is a stream.
    unsafe { fputc(character, standard_output()) }
}

/// # Safety
/// `file` must be a stream and `data` readable for `size * count` bytes.
pub unsafe extern "C" fn fwrite(
    data: *const c_void,
    size: usize,
    count: usize,
    file: *mut Stream,
) -> usize {
    let Some(total) = size.checked_mul(count).filter(|&total| total > 0) else {
        return 0;
    };
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return 0;
    };
    // SAFETY: as above.
    let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), total) };
    if write_bytes(stream, bytes) { count } else { 0 }
}

/// # Safety
/// `file` must be a stream and `data` writable for `size * count` bytes.
pub unsafe extern "C" fn fread(
    data: *mut c_void,
    size: usize,
    count: usize,
    file: *mut Stream,
) -> usize {
    let Some(total) = size.checked_mul(count).filter(|&total| total > 0) else {
        return 0;
    };
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return 0;
    };
    let out = data.cast::<u8>();
    let mut filled = 0;
    while filled < total {
        if let Some(byte) = stream.unread.take() {
            // SAFETY: `filled` is below the caller's `total` bytes.
            unsafe { out.wrapping_add(filled).write(byte) };
            filled += 1;
            continue;
        }
        if stream.written > 0 && flush(stream) != 0 {
            break;
        }
        if !fill(stream) {
            break;
        }
        let taken = (stream.read_end - stream.read_at).min(total - filled);
        // SAFETY: `taken` buffered bytes into the rest of the caller's buffer.
        unsafe {
            out.wrapping_add(filled)
                .copy_from_nonoverlapping(stream.buffer.start.wrapping_add(stream.read_at), taken)
        };
        stream.read_at += taken;
        filled += taken;
    }
    filled / size
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn fgetc(file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stream(file) }
        .and_then(read_byte)
        .map_or(EOF, c_int::from)
}

pub extern "C" fn getchar() -> c_int {
    // SAFETY: standard input is a stream.
    unsafe { fgetc(standard_input()) }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn ungetc(character: c_int, file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { stream(file) } {
        Some(stream) if character != EOF && stream.unread.is_none() => {
            stream.unread = Some(character as u8);
            stream.end = false;
            character & 0xff
        }
        _ => EOF,
    }
}

/// # Safety
/// `file` must be a stream and `buffer` writable for `size` bytes.
pub unsafe extern "C" fn fgets(buffer: *mut c_char, size: c_int, file: *mut Stream) -> *mut c_char {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return ptr::null_mut();
    };
    if size <= 0 {
        return ptr::null_mut();
    }
    let mut length = 0;
    while length + 1 < size as usize {
        let Some(byte) = read_byte(stream) else { break };
        // SAFETY: `length` is below `size - 1`.
        unsafe { buffer.wrapping_add(length).write(byte as c_char) };
        length += 1;
        if byte == b'\n' {
            break;
        }
    }
    if length == 0 {
        return ptr::null_mut();
    }
    // SAFETY: `length` is below `size`.
    unsafe { buffer.wrapping_add(length).write(0) };
    buffer
}

/// # Safety
/// `file` must be null or a stream.
pub unsafe extern "C" fn fflush(file: *mut Stream) -> c_int {
    if file.is_null() {
        flush_all();
        return 0;
    }
    // SAFETY: the caller's promise.
    unsafe { stream(file) }.map_or(EOF, flush)
}

fn mode_flags(mode: &[u8]) -> Option<(u32, bool, bool)> {
    let plus = mode.contains(&b'+');
    let base = match mode.first()? {
        b'r' => 0,
        b'w' => darwin::open::CREAT | darwin::open::TRUNC,
        b'a' => darwin::open::CREAT | darwin::open::APPEND,
        _ => return None,
    };
    let access = match (mode[0], plus) {
        (_, true) => 2,
        (b'r', false) => 0,
        _ => 1,
    };
    let exclusive = if mode.contains(&b'x') {
        darwin::open::EXCL
    } else {
        0
    };
    let cloexec = if mode.contains(&b'e') {
        darwin::open::CLOEXEC
    } else {
        0
    };
    Some((
        base | access | exclusive | cloexec,
        access != 1,
        access != 0,
    ))
}

fn new_stream(fd: c_int, readable: bool, writable: bool) -> *mut Stream {
    let buffer = heap::allocate(BUFFER_SIZE, false);
    let stream = heap::allocate(size_of::<Stream>(), false).cast::<Stream>();
    if buffer.is_null() || stream.is_null() {
        heap::release(buffer);
        heap::release(stream.cast());
        return ptr::null_mut();
    }
    let mut value = Stream::standard(fd, Buffering::Full);
    value.buffer = StreamBuffer::heap(buffer, BUFFER_SIZE);
    value.readable = readable;
    value.writable = writable;
    value.owned = true;
    // SAFETY: the open-stream list is the layer's, used by the program's only thread.
    value.next = unsafe { open_streams().read() };
    // SAFETY: a fresh block of a stream's size.
    unsafe { stream.write(value) };
    // SAFETY: as above.
    unsafe { open_streams().write(stream) };
    stream
}

/// # Safety
/// `path` and `mode` must be NUL-terminated strings.
pub unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut Stream {
    // SAFETY: the caller's promise.
    let Some((flags, readable, writable)) = mode_flags(unsafe { c_string(mode) }) else {
        set_errno(EINVAL);
        return ptr::null_mut();
    };
    // SAFETY: as above.
    let fd = unsafe { file::open_with_mode(path, flags as c_int, 0o666) };
    if fd < 0 {
        return ptr::null_mut();
    }
    let stream = new_stream(fd, readable, writable);
    if stream.is_null() {
        let _ = linux::close(fd as usize);
    }
    stream
}

/// # Safety
/// `mode` must be a NUL-terminated string.
pub unsafe extern "C" fn fdopen(fd: c_int, mode: *const c_char) -> *mut Stream {
    // SAFETY: the caller's promise.
    let Some((_, readable, writable)) = mode_flags(unsafe { c_string(mode) }) else {
        set_errno(EINVAL);
        return ptr::null_mut();
    };
    new_stream(fd, readable, writable)
}

/// # Safety
/// `file` must be a stream, not used after this.
pub unsafe extern "C" fn fclose(file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return EOF;
    };
    let flushed = flush(stream);
    let closed = linux::close(stream.fd as usize).is_ok();
    if let Some(block) = stream.buffer.take_owned() {
        heap::release(block);
    }
    if stream.owned {
        let mut link = open_streams();
        loop {
            // SAFETY: `link` is the list's head or a live stream's `next`; `file` leaves the
            // list before it is freed.
            let current = unsafe { link.read() };
            if current.is_null() {
                break;
            }
            if current == file {
                // SAFETY: as above.
                unsafe { link.write(stream.next) };
                break;
            }
            // SAFETY: `current` is a live stream on the list.
            link = unsafe { &raw mut (*current).next };
        }
        heap::release(file.cast());
    }
    if flushed == 0 && closed { 0 } else { EOF }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn fileno(file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stream(file) }.map_or(-1, |stream| stream.fd)
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn feof(file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stream(file) }.map_or(0, |stream| c_int::from(stream.end))
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn ferror(file: *mut Stream) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stream(file) }.map_or(0, |stream| c_int::from(stream.error))
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn clearerr(file: *mut Stream) {
    // SAFETY: the caller's promise.
    if let Some(stream) = unsafe { stream(file) } {
        stream.error = false;
        stream.end = false;
    }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn fseeko(file: *mut Stream, offset: i64, whence: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return -1;
    };
    if flush(stream) != 0 {
        return -1;
    }
    let buffered = (stream.read_end - stream.read_at) as i64 + i64::from(stream.unread.is_some());
    let offset = if whence == 1 {
        offset - buffered
    } else {
        offset
    };
    stream.read_at = 0;
    stream.read_end = 0;
    stream.unread = None;
    stream.end = false;
    match linux::lseek(stream.fd as usize, offset, whence as usize) {
        Ok(_) => 0,
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn fseek(file: *mut Stream, offset: c_long, whence: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { fseeko(file, offset, whence) }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn ftello(file: *mut Stream) -> i64 {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return -1;
    };
    match linux::lseek(stream.fd as usize, 0, 1) {
        Ok(position) => {
            position as i64 + stream.written as i64
                - (stream.read_end - stream.read_at) as i64
                - i64::from(stream.unread.is_some())
        }
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn ftell(file: *mut Stream) -> c_long {
    // SAFETY: the caller's promise.
    unsafe { ftello(file) }
}

/// # Safety
/// `file` must be a stream.
pub unsafe extern "C" fn rewind(file: *mut Stream) {
    // SAFETY: the caller's promise.
    unsafe { fseeko(file, 0, 0) };
    // SAFETY: as above.
    unsafe { clearerr(file) };
}

/// # Safety
/// `file` must be a stream; `buffer` null or `size` bytes that stay while the stream is used.
pub unsafe extern "C" fn setvbuf(
    file: *mut Stream,
    buffer: *mut c_char,
    mode: c_int,
    size: usize,
) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        return -1;
    };
    stream.buffering = match mode {
        0 => Buffering::Full,
        1 => Buffering::Line,
        2 => Buffering::None,
        _ => return -1,
    };
    if !buffer.is_null() && size > 0 {
        if flush(stream) != 0 {
            return -1;
        }
        if let Some(block) = stream.buffer.replace_with_borrowed(buffer.cast(), size) {
            heap::release(block);
        }
        stream.read_at = 0;
        stream.read_end = 0;
    }
    0
}

/// # Safety
/// `text` must be null or a NUL-terminated string.
pub unsafe extern "C" fn perror(text: *const c_char) {
    // SAFETY: the program keeps `__stderrp` a stream.
    let Some(stream) = (unsafe { stream(standard_error()) }) else {
        return;
    };
    if !text.is_null() {
        // SAFETY: as above.
        let prefix = unsafe { c_string(text) };
        if !prefix.is_empty() {
            write_bytes(stream, prefix);
            write_bytes(stream, b": ");
        }
    }
    write_bytes(stream, super::error_message(super::errno()));
    write_bytes(stream, b"\n");
}
