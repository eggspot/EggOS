//! Other processes (D178): `CreateProcessW` starts a program as the layer's child the way
//! `posix_spawn` does (the kernel runs a Windows program in a layer of its own, D118, and a
//! Linux program as it is), `OpenProcess` names any process by its id, and a process handle
//! is a kernel object (`sync.rs`) that waits, `GetExitCodeProcess` and `TerminateProcess` use;
//! the child's end is learned with `wait4` and others' with `kill(id, 0)`. A process's memory is
//! only the caller's own: other processes' is `ERROR_ACCESS_DENIED`.

use super::sync::{self, Kind};
use super::{Bool, CURRENT_PROCESS, FALSE, SpinLock, TRUE, error, error_for, fail, heap};
use crate::command_line::{self, Piece};
use crate::library::Export;
use crate::linux;
use crate::text::{self, PATH_CAPACITY};
use core::ffi::CStr;
use eggos_runtime::process::{ChildStack, Spawn, spawn};

const ESRCH: u16 = 3;
const ENOENT: u16 = 2;
const ENOEXEC: u16 = 8;
const CREATE_SUSPENDED: u32 = 0x4;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
const STARTF_USESTDHANDLES: u32 = 0x100;
const STARTUP_INFO_FLAGS: usize = 60;
const STARTUP_INFO_STANDARD_HANDLES: usize = 80;
const NORMAL_PRIORITY_CLASS: u32 = 0x20;
const PRIORITY_CLASSES: [u32; 8] = [
    0x40,
    0x4000,
    0x20,
    0x8000,
    0x80,
    0x100,
    0x0010_0000,
    0x0020_0000,
];
const POINTER: usize = core::mem::size_of::<usize>();

static CHILD_STACK: SpinLock<ChildStack> = SpinLock::new(ChildStack::new());

/// What a process handle names: a process by its id, and its end once seen.
#[derive(Debug)]
pub struct ProcessObject {
    id: u32,
    ended: bool,
    exit_code: Option<u32>,
}

impl ProcessObject {
    /// Whether the process was seen to end.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Looks whether the process ended, reaping it when it is the layer's child.
    pub fn refresh(&mut self) {
        if self.ended {
            return;
        }
        match linux::reap(self.id) {
            Ok(Some(status)) => {
                self.ended = true;
                self.exit_code = self.exit_code.or(Some(exit_code_of(status)));
            }
            Ok(None) => {}
            Err(_) => {
                if linux::kill(self.id, 0).is_err_and(|errno| errno.0 == ESRCH) {
                    self.ended = true;
                }
            }
        }
    }
}

fn exit_code_of(status: u32) -> u32 {
    match status & 0x7f {
        0 => (status >> 8) & 0xff,
        signal => 128 + signal,
    }
}

fn new_process_handle(id: u32) -> usize {
    sync::insert(
        Kind::Process(ProcessObject {
            id,
            ended: false,
            exit_code: None,
        }),
        0,
    )
    .0
}

/// The id of the process `handle` names, the caller's own for its pseudo-handle.
pub(super) fn process_id_of(handle: usize) -> Option<u32> {
    if handle == CURRENT_PROCESS {
        return Some(linux::process_id());
    }
    sync::change(handle, |kind| match kind {
        Kind::Process(process) => (Some(process.id), false),
        _ => (None, false),
    })
    .flatten()
}

fn is_caller(handle: usize) -> Result<bool, u32> {
    match process_id_of(handle) {
        Some(id) => Ok(id == linux::process_id()),
        None => Err(error::INVALID_HANDLE),
    }
}

/// `TerminateProcess` for a process object: kills it with `SIGKILL`; its exit code is `code`.
pub(super) fn terminate(handle: usize, code: u32) -> Bool {
    let killed = sync::change(handle, |kind| match kind {
        Kind::Process(process) if process.id == linux::process_id() => (Some(Ok(true)), false),
        Kind::Process(process) => {
            process.refresh();
            if process.ended {
                return (Some(Err(error::ACCESS_DENIED)), false);
            }
            match linux::kill(process.id, linux::SIGKILL) {
                Ok(_) => {
                    process.exit_code = Some(code);
                    (Some(Ok(false)), true)
                }
                Err(errno) => (Some(Err(error_for(errno))), false),
            }
        }
        _ => (None, false),
    })
    .flatten();
    match killed {
        Some(Ok(true)) => {
            super::crt::flush_streams_at_process_exit();
            linux::exit(code)
        }
        Some(Ok(false)) => TRUE,
        Some(Err(error_code)) => fail(error_code, FALSE),
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn open_process(_access: u32, _inherit: Bool, id: u32) -> usize {
    if id == 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    match linux::kill(id, 0) {
        Ok(_) => new_process_handle(id),
        Err(errno) if errno.0 == ESRCH => fail(error::INVALID_PARAMETER, 0),
        Err(errno) => fail(error_for(errno), 0),
    }
}

extern "efiapi" fn get_process_id(handle: usize) -> u32 {
    match process_id_of(handle) {
        Some(id) => id,
        None => fail(error::INVALID_HANDLE, 0),
    }
}

unsafe extern "efiapi" fn get_exit_code_process(handle: usize, code: *mut u32) -> Bool {
    if code.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let state = match handle {
        CURRENT_PROCESS => Some(Some(sync::STILL_ACTIVE)),
        _ => sync::change(handle, |kind| match kind {
            Kind::Process(process) => {
                let was_running = !process.ended;
                process.refresh();
                let state = match process.ended {
                    true => process.exit_code,
                    false => Some(sync::STILL_ACTIVE),
                };
                (Some(state), was_running && process.ended)
            }
            _ => (None, false),
        })
        .flatten(),
    };
    match state {
        Some(Some(value)) => {
            // SAFETY: the caller passes a writable `DWORD`.
            unsafe { code.write(value) };
            TRUE
        }
        Some(None) => fail(error::NOT_SUPPORTED, FALSE),
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn set_priority_class(handle: usize, class: u32) -> Bool {
    if let Err(code) = is_caller(handle) {
        return fail(code, FALSE);
    }
    match class {
        NORMAL_PRIORITY_CLASS => TRUE,
        _ if PRIORITY_CLASSES.contains(&class) => fail(error::NOT_SUPPORTED, FALSE),
        _ => fail(error::INVALID_PARAMETER, FALSE),
    }
}

unsafe extern "efiapi" fn is_wow64_process(handle: usize, wow64: *mut Bool) -> Bool {
    if wow64.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if let Err(code) = is_caller(handle) {
        return fail(code, FALSE);
    }
    // SAFETY: the caller passes a writable `BOOL`.
    unsafe { wow64.write(FALSE) };
    TRUE
}

fn own_memory(handle: usize, address: usize) -> Result<(), u32> {
    match is_caller(handle)? {
        false => Err(error::ACCESS_DENIED),
        true if address == 0 => Err(error::INVALID_ADDRESS),
        true => Ok(()),
    }
}

unsafe extern "efiapi" fn read_process_memory(
    handle: usize,
    address: *const u8,
    buffer: *mut u8,
    size: usize,
    read: *mut usize,
) -> Bool {
    if let Err(code) = own_memory(handle, address as usize) {
        return fail(code, FALSE);
    }
    // SAFETY: `ReadProcessMemory`'s contract on the caller's own memory: `size` readable bytes
    // at `address` and `size` writable ones at `buffer`, which may overlap.
    unsafe { core::ptr::copy(address, buffer, size) };
    if !read.is_null() {
        // SAFETY: the caller passes a writable `SIZE_T`.
        unsafe { read.write(size) };
    }
    TRUE
}

unsafe extern "efiapi" fn write_process_memory(
    handle: usize,
    address: *mut u8,
    buffer: *const u8,
    size: usize,
    written: *mut usize,
) -> Bool {
    if let Err(code) = own_memory(handle, address as usize) {
        return fail(code, FALSE);
    }
    // SAFETY: `WriteProcessMemory`'s contract on the caller's own memory: `size` writable bytes
    // at `address` and `size` readable ones at `buffer`, which may overlap.
    unsafe { core::ptr::copy(buffer, address, size) };
    if !written.is_null() {
        // SAFETY: the caller passes a writable `SIZE_T`.
        unsafe { written.write(size) };
    }
    TRUE
}

extern "efiapi" fn virtual_alloc_ex(
    handle: usize,
    address: usize,
    size: usize,
    kind: u32,
    protect: u32,
) -> usize {
    match is_caller(handle) {
        Ok(true) => super::memory::virtual_alloc(address, size, kind, protect),
        Ok(false) => fail(error::ACCESS_DENIED, 0),
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn virtual_free_ex(handle: usize, address: usize, size: usize, kind: u32) -> Bool {
    match is_caller(handle) {
        Ok(true) => super::memory::virtual_free(address, size, kind),
        Ok(false) => fail(error::ACCESS_DENIED, FALSE),
        Err(code) => fail(code, FALSE),
    }
}

/// A heap block of pointers to NUL-terminated strings, null-ended, the strings after them.
struct StringTable {
    block: *mut u8,
    count: usize,
}

impl StringTable {
    fn pointers(&self) -> &[*const u8] {
        // SAFETY: the block starts with `count + 1` pointers, the last null.
        unsafe { core::slice::from_raw_parts(self.block.cast::<*const u8>(), self.count + 1) }
    }
}

impl Drop for StringTable {
    fn drop(&mut self) {
        // SAFETY: the block is this table's alone.
        unsafe { heap::free(self.block) };
    }
}

/// The table of `strings`, each UTF-16 and ended by a NUL, as UTF-8 C strings.
fn table_of_wide<'a>(strings: impl Iterator<Item = &'a [u16]> + Clone) -> Option<StringTable> {
    let count = strings.clone().count();
    let table = (count + 1) * POINTER;
    let bytes: usize = strings
        .clone()
        .map(|string| text::utf8_length(string) + 1)
        .sum();
    let block = heap::allocate(table + bytes);
    if block.is_null() {
        return None;
    }
    let mut at = table;
    for (index, string) in strings.enumerate() {
        let length = text::utf8_length(string);
        let string_at = block.wrapping_add(at);
        // SAFETY: the zeroed block holds the pointers, then each string's bytes and NUL.
        let out = unsafe { core::slice::from_raw_parts_mut(string_at, length) };
        text::write_utf8(string, out);
        // SAFETY: as above: `index` is below `count`.
        unsafe {
            block
                .cast::<usize>()
                .wrapping_add(index)
                .write(string_at as usize)
        };
        at += length + 1;
    }
    Some(StringTable { block, count })
}

fn table_of_bytes<'a>(strings: impl Iterator<Item = &'a [u8]> + Clone) -> Option<StringTable> {
    let count = strings.clone().count();
    let table = (count + 1) * POINTER;
    let bytes: usize = strings.clone().map(|string| string.len() + 1).sum();
    let block = heap::allocate(table + bytes);
    if block.is_null() {
        return None;
    }
    let mut at = table;
    for (index, string) in strings.enumerate() {
        let string_at = block.wrapping_add(at);
        // SAFETY: the zeroed block holds the pointers, then each string's bytes and NUL.
        unsafe { string_at.copy_from_nonoverlapping(string.as_ptr(), string.len()) };
        // SAFETY: as above: `index` is below `count`.
        unsafe {
            block
                .cast::<usize>()
                .wrapping_add(index)
                .write(string_at as usize)
        };
        at += string.len() + 1;
    }
    Some(StringTable { block, count })
}

/// The arguments of the Windows command line `line`, split as `CommandLineToArgvW` does, each
/// ended by a NUL, in one heap block of UTF-16 units; its length.
fn split_arguments(line: &[u16]) -> Option<(*mut u16, usize)> {
    let mut units = 0;
    command_line::split(line, &mut |_| units += 1);
    let block = heap::allocate(units.max(1) * 2).cast::<u16>();
    if block.is_null() {
        return None;
    }
    let mut at = 0;
    command_line::split(line, &mut |piece| {
        let unit = match piece {
            Piece::Unit(unit) => unit,
            Piece::End => 0,
        };
        // SAFETY: the block holds the `units` pieces the first split counted.
        unsafe { block.wrapping_add(at).write(unit) };
        at += 1;
    });
    Some((block, units))
}

fn is_regular_file(path: &[u8]) -> bool {
    linux::stat_at(linux::AT_FDCWD, path)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::REGULAR_FILE)
}

fn joined(
    directory: &[u8],
    name: &[u8],
    extension: &[u8],
    out: &mut [u8; PATH_CAPACITY],
) -> Option<usize> {
    let separator = !directory.is_empty() && !directory.ends_with(b"/");
    let length = directory.len() + usize::from(separator) + name.len() + extension.len();
    if length + 1 > out.len() {
        return None;
    }
    out[..directory.len()].copy_from_slice(directory);
    let mut at = directory.len();
    if separator {
        out[at] = b'/';
        at += 1;
    }
    out[at..at + name.len()].copy_from_slice(name);
    at += name.len();
    out[at..at + extension.len()].copy_from_slice(extension);
    out[length] = 0;
    Some(length + 1)
}

/// Finds the program the Windows name `name` (a path, or a file name searched for in the
/// program's folder, the working folder and `PATH`, with `.exe` added when it has no
/// extension), writing its NUL-terminated EggOS path into `out`; the path's length with the
/// NUL.
pub(super) fn find_program(name: &[u16], out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    let mut path = [0u8; PATH_CAPACITY];
    let length = text::eggos_path(text::utf16_chars(name), &mut path)? - 1;
    let path = &path[..length];
    if path.is_empty() {
        return None;
    }
    let last = path.rsplit(|byte| *byte == b'/').next().unwrap_or(path);
    let extensions: &[&[u8]] = match last.contains(&b'.') {
        true => &[&b""[..]],
        false => &[&b""[..], &b".exe"[..]],
    };
    let try_in = |directory: &[u8], out: &mut [u8; PATH_CAPACITY]| {
        extensions.iter().find_map(|extension| {
            let length = joined(directory, path, extension, out)?;
            is_regular_file(&out[..length]).then_some(length)
        })
    };
    if path.contains(&b'/') {
        return try_in(b"", out);
    }
    let mut program = [0u8; PATH_CAPACITY];
    let program_folder = text::eggos_path(
        text::utf16_chars(super::process::process().program_windows),
        &mut program,
    )
    .map(|length| {
        let file = &program[..length - 1];
        file.iter().rposition(|byte| *byte == b'/').unwrap_or(0)
    });
    if let Some(end) = program_folder
        && end > 0
        && let Some(found) = try_in(&program[..end], out)
    {
        return Some(found);
    }
    if let Some(found) = try_in(b"", out) {
        return Some(found);
    }
    let mut variable = [0u8; PATH_CAPACITY];
    let length = super::environment::variable(b"PATH", &mut variable)?.min(PATH_CAPACITY);
    let paths = &variable[..length];
    let separator = if paths.contains(&b';') { b';' } else { b':' };
    paths
        .split(|byte| *byte == separator)
        .filter(|entry| !entry.is_empty())
        .find_map(|entry| {
            let mut folder = [0u8; PATH_CAPACITY];
            let length = text::eggos_path(text::utf8_chars(entry), &mut folder)? - 1;
            try_in(&folder[..length], out)
        })
}

/// Whether the file at the NUL-terminated EggOS path `path` is a program: a Windows program
/// (`.exe` or `.com`) or a file anyone may run.
pub(super) fn is_program(path: &[u8]) -> bool {
    let name = &path[..path.len().saturating_sub(1)];
    let windows = [&b".exe"[..], &b".com"[..]].iter().any(|extension| {
        name.len() >= extension.len()
            && name[name.len() - extension.len()..].eq_ignore_ascii_case(extension)
    });
    windows
        || linux::stat_at(linux::AT_FDCWD, path)
            .is_ok_and(|words| linux::file_mode(&words) & 0o111 != 0)
}

/// The environment a child starts with.
#[derive(Debug, Clone, Copy)]
pub(super) enum Environment {
    Inherited,
    Wide(*const u16),
    Narrow(*const u8),
}

/// What [`launch`] starts.
#[derive(Debug)]
pub(super) struct Launch<'a> {
    pub application: Option<&'a [u16]>,
    pub command_line: &'a [u16],
    pub directory: Option<&'a [u16]>,
    pub environment: Environment,
    pub streams: [usize; 3],
    pub own_group: bool,
}

/// The strings at `block`, each ended by a zero, without the zero that ends the list.
///
/// # Safety
/// `block` must hold such strings, the list ended by another zero, which outlive the result's
/// use.
pub(super) unsafe fn listed<T: Copy + PartialEq + Default>(block: *const T) -> &'static [T] {
    let mut length = 0;
    loop {
        // SAFETY: the caller's duty: entries each ended by a zero, the list by another.
        let here = unsafe { block.wrapping_add(length).read() };
        // SAFETY: as above: a zero is followed by another value, the list's end at last.
        let next = unsafe { block.wrapping_add(length + 1).read() };
        if here == T::default() && next == T::default() {
            break;
        }
        length += 1;
    }
    // SAFETY: as above: `length + 1` readable values.
    unsafe { core::slice::from_raw_parts(block, length + 1) }
}

fn environment_table(environment: Environment) -> Option<StringTable> {
    match environment {
        Environment::Inherited => {
            let block = super::environment::exec_block();
            if block.is_null() {
                return None;
            }
            let mut count = 0;
            // SAFETY: the block starts with a null-ended array of pointers.
            while unsafe { block.cast::<usize>().wrapping_add(count).read() } != 0 {
                count += 1;
            }
            Some(StringTable { block, count })
        }
        Environment::Wide(block) => {
            // SAFETY: `CREATE_UNICODE_ENVIRONMENT`'s contract: `NAME=value` strings, each
            // ended by a NUL, the block by another.
            let units = unsafe { listed(block) };
            table_of_wide(
                units
                    .split(|unit| *unit == 0)
                    .filter(|entry| !entry.is_empty()),
            )
        }
        Environment::Narrow(block) => {
            // SAFETY: as above, in bytes.
            let bytes = unsafe { listed(block) };
            table_of_bytes(
                bytes
                    .split(|byte| *byte == 0)
                    .filter(|entry| !entry.is_empty()),
            )
        }
    }
}

/// Starts the program `launch` describes as a child: its process handle and id.
pub(super) fn launch(launch: &Launch<'_>) -> Result<(usize, u32), u32> {
    let (flat, units) = split_arguments(launch.command_line).ok_or(error::NOT_ENOUGH_MEMORY)?;
    // SAFETY: `split_arguments` filled `units` units of the block.
    let arguments = unsafe { core::slice::from_raw_parts(flat, units) };
    let first = arguments.split(|unit| *unit == 0).next().unwrap_or(&[]);
    let mut program = [0u8; PATH_CAPACITY];
    let found = find_program(launch.application.unwrap_or(first), &mut program);
    let table = table_of_wide(
        arguments
            .split(|unit| *unit == 0)
            .take(arguments.iter().filter(|unit| **unit == 0).count()),
    );
    // SAFETY: the block is the one `split_arguments` made, now copied into `table`.
    unsafe { heap::free(flat.cast()) };
    let length = found.ok_or(error::FILE_NOT_FOUND)?;
    let arguments = table.ok_or(error::NOT_ENOUGH_MEMORY)?;
    let environment = environment_table(launch.environment).ok_or(error::NOT_ENOUGH_MEMORY)?;
    let mut folder = [0u8; PATH_CAPACITY];
    let folder_length = match launch.directory {
        Some(directory) => Some(
            text::eggos_path(text::utf16_chars(directory), &mut folder)
                .ok_or(error::INVALID_PARAMETER)?,
        ),
        None => None,
    };
    let program =
        CStr::from_bytes_with_nul(&program[..length]).map_err(|_| error::INVALID_PARAMETER)?;
    let mut request = Spawn::new(
        program,
        arguments.pointers(),
        environment.pointers(),
        launch.streams,
    );
    if let Some(length) = folder_length {
        let directory =
            CStr::from_bytes_with_nul(&folder[..length]).map_err(|_| error::INVALID_PARAMETER)?;
        request = request.in_directory(directory);
    }
    if launch.own_group {
        request = request.in_own_process_group();
    }
    let id = CHILD_STACK
        .with(|stack| spawn(stack, &request))
        .map_err(|errno| match errno.0 {
            ENOENT => error::FILE_NOT_FOUND,
            ENOEXEC => error::BAD_EXE_FORMAT,
            _ => error_for(errno),
        })? as u32;
    match new_process_handle(id) {
        0 => Err(error::NOT_ENOUGH_MEMORY),
        handle => Ok((handle, id)),
    }
}

/// The descriptors of the standard handles a `STARTUPINFOW` gives, or the caller's own.
///
/// # Safety
/// `startup` must be null or a readable `STARTUPINFOW`.
unsafe fn standard_streams(startup: *const u8, inherit: bool) -> [usize; 3] {
    let mut streams = [0, 1, 2];
    if startup.is_null() || !inherit {
        return streams;
    }
    // SAFETY: the caller's duty: the flags and the three handles lie inside the structure.
    let flags = unsafe {
        startup
            .wrapping_add(STARTUP_INFO_FLAGS)
            .cast::<u32>()
            .read_unaligned()
    };
    if flags & STARTF_USESTDHANDLES == 0 {
        return streams;
    }
    for (index, stream) in streams.iter_mut().enumerate() {
        // SAFETY: as above.
        let handle = unsafe {
            startup
                .wrapping_add(STARTUP_INFO_STANDARD_HANDLES + index * POINTER)
                .cast::<usize>()
                .read_unaligned()
        };
        if let Some(fd) = super::file::fd_of(handle) {
            *stream = fd;
        }
    }
    streams
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn create_process_w(
    application: *const u16,
    command_line: *mut u16,
    _process_attributes: usize,
    _thread_attributes: usize,
    inherit_handles: Bool,
    flags: u32,
    environment: *const u8,
    directory: *const u16,
    startup: *const u8,
    information: *mut u8,
) -> Bool {
    if information.is_null() || (application.is_null() && command_line.is_null()) {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if flags & CREATE_SUSPENDED != 0 {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    let string = |text: *const u16| match text.is_null() {
        true => None,
        // SAFETY: `CreateProcessW`'s contract: NUL-terminated strings.
        false => Some(unsafe { super::file::wide_string(text) }),
    };
    let application = string(application);
    let line = string(command_line.cast_const())
        .or(application)
        .unwrap_or(&[]);
    let environment = match (
        environment.is_null(),
        flags & CREATE_UNICODE_ENVIRONMENT != 0,
    ) {
        (true, _) => Environment::Inherited,
        (false, true) => Environment::Wide(environment.cast()),
        (false, false) => Environment::Narrow(environment),
    };
    let request = Launch {
        application,
        command_line: line,
        directory: string(directory),
        environment,
        // SAFETY: `CreateProcessW`'s contract: a readable `STARTUPINFOW`.
        streams: unsafe { standard_streams(startup, inherit_handles != FALSE) },
        own_group: flags & CREATE_NEW_PROCESS_GROUP != 0,
    };
    let (handle, id) = match launch(&request) {
        Ok(started) => started,
        Err(code) => return fail(code, FALSE),
    };
    let thread = sync::duplicate(handle).unwrap_or(0);
    let fields = [(0, handle), (POINTER, thread)];
    for (offset, value) in fields {
        // SAFETY: the caller passes a writable `PROCESS_INFORMATION`: the process and thread
        // handles, then their ids.
        unsafe {
            information
                .wrapping_add(offset)
                .cast::<usize>()
                .write_unaligned(value)
        };
    }
    for offset in [2 * POINTER, 2 * POINTER + 4] {
        // SAFETY: as above: the process's id, then its main thread's, which Linux numbers
        // alike.
        unsafe {
            information
                .wrapping_add(offset)
                .cast::<u32>()
                .write_unaligned(id)
        };
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreateProcessW" => create_process_w [two_stack_arguments],
    b"OpenProcess" => open_process,
    b"GetProcessId" => get_process_id,
    b"GetExitCodeProcess" => get_exit_code_process,
    b"SetPriorityClass" => set_priority_class,
    b"IsWow64Process" => is_wow64_process,
    b"ReadProcessMemory" => read_process_memory,
    b"WriteProcessMemory" => write_process_memory,
    b"VirtualAllocEx" => virtual_alloc_ex,
    b"VirtualFreeEx" => virtual_free_ex,
];
