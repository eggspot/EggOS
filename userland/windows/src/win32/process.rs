//! The process: its command line (`GetCommandLineW`/`A`, built once from the arguments the
//! layer was started with), its ids and pseudo-handles, and its end (`ExitProcess`, whose
//! code's low 8 bits are the status the shell sees).

use super::heap;
use crate::command_line;
use crate::library::Export;
use crate::linux;
use crate::text;
use core::sync::atomic::{AtomicPtr, Ordering};
use eggos_runtime::startup::Startup;

use super::{CURRENT_PROCESS, CURRENT_THREAD};

/// What the process's Win32 functions answer from, set once before the program's entry runs.
#[derive(Debug)]
pub struct ProcessState {
    pub image_base: u64,
    pub program_windows: &'static [u16],
    pub command_line: &'static [u8],
    pub command_line_wide: &'static [u16],
    pub startup: Startup,
}

static PROCESS: AtomicPtr<ProcessState> = AtomicPtr::new(core::ptr::null_mut());

/// Builds the process's state from `startup` (the layer's arguments: the program's path, then
/// the program's own arguments, its name first) and publishes it; `None` without memory.
pub fn set_process(startup: Startup, image_base: u64) -> Option<()> {
    let program = startup.arguments().nth(1)?.to_bytes();
    let arguments = || {
        startup
            .arguments()
            .skip(2)
            .map(|argument| argument.to_bytes())
    };
    let mut length = 0;
    command_line::write(arguments(), &mut |_| length += 1);
    let command_line = new_slice::<u8>(length + 1)?;
    let mut at = 0;
    command_line::write(arguments(), &mut |byte| {
        command_line[at] = byte;
        at += 1;
    });
    let command_line_wide = wide_copy(&command_line[..length])?;
    let program_windows = windows_program_path(program)?;
    let state = heap::allocate(core::mem::size_of::<ProcessState>()).cast::<ProcessState>();
    if state.is_null() {
        return None;
    }
    // SAFETY: `state` is a fresh, 16-byte aligned heap block for one `ProcessState`.
    unsafe {
        state.write(ProcessState {
            image_base,
            program_windows,
            command_line,
            command_line_wide,
            startup,
        })
    };
    PROCESS.store(state, Ordering::Release);
    Some(())
}

/// The process's state.
pub fn process() -> &'static ProcessState {
    // SAFETY: the loader publishes the state before the program's code runs and never changes
    // or frees it, and every Win32 call comes from the program's code.
    unsafe { &*PROCESS.load(Ordering::Acquire) }
}

fn new_slice<T: Copy>(length: usize) -> Option<&'static mut [T]> {
    let bytes = length.checked_mul(core::mem::size_of::<T>())?;
    let pointer = heap::allocate(bytes).cast::<T>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: a fresh, zeroed, 16-byte aligned heap block of `length` `T`s that is never freed;
    // `T` is a byte or a UTF-16 unit, for which zeros are valid.
    Some(unsafe { core::slice::from_raw_parts_mut(pointer, length) })
}

fn windows_program_path(program: &[u8]) -> Option<&'static [u16]> {
    let mut directory = [0u8; text::PATH_CAPACITY];
    let length = linux::working_directory(&mut directory).ok()?;
    let mut absolute = [0u8; text::PATH_CAPACITY];
    let directory = &directory[..length.saturating_sub(1)];
    let length = text::absolute_path(directory, program, &mut absolute)?;
    let mut units = 0;
    text::windows_path(&absolute[..length], &mut |c| units += c.len_utf16());
    let wide = new_slice::<u16>(units + 1)?;
    let mut at = 0;
    text::windows_path(&absolute[..length], &mut |c| {
        at += c.encode_utf16(&mut wide[at..]).len();
    });
    Some(&wide[..units])
}

fn wide_copy(bytes: &[u8]) -> Option<&'static [u16]> {
    let wide = new_slice::<u16>(text::utf16_length(bytes) + 1)?;
    text::write_utf16(bytes, wide);
    Some(wide)
}

extern "efiapi" fn get_command_line_w() -> *const u16 {
    process().command_line_wide.as_ptr()
}

extern "efiapi" fn get_command_line_a() -> *const u8 {
    process().command_line.as_ptr()
}

extern "efiapi" fn exit_process(code: u32) -> ! {
    super::crt::flush_streams_at_process_exit();
    linux::exit(code)
}

extern "efiapi" fn terminate_process(process: usize, code: u32) -> super::Bool {
    if process == CURRENT_PROCESS {
        linux::exit(code);
    }
    super::fail(super::error::ACCESS_DENIED, super::FALSE)
}

extern "efiapi" fn get_current_process() -> usize {
    CURRENT_PROCESS
}

extern "efiapi" fn get_current_thread() -> usize {
    CURRENT_THREAD
}

extern "efiapi" fn get_current_process_id() -> u32 {
    linux::process_id()
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetCommandLineW" => get_command_line_w,
    b"GetCommandLineA" => get_command_line_a,
    b"ExitProcess" => exit_process,
    b"TerminateProcess" => terminate_process,
    b"GetCurrentProcess" => get_current_process,
    b"GetCurrentThread" => get_current_thread,
    b"GetCurrentProcessId" => get_current_process_id,
];
