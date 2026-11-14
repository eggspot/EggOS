//! The terminal mode's system calls (D582): the terminal's settings kept and set raw, the
//! alternate screen, the size (`TIOCGWINSZ`, else `LINES` and `COLUMNS`, else the terminal's
//! own answer), the loop that waits for keys and draws, and the screen given back at the end.

use super::session::{END, ESCAPE_WAIT_MS, Outcome, SIZE_QUERY, START, Style, TerminalEditor};
use crate::document::TEXT_CAPACITY;
use crate::editor::{Editor, MEMORY_SIZES, Memory};
use eggos_app::linux::{LinuxFiles, write_all};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number, tty};

const INPUT: usize = 0;
const OUTPUT: usize = 1;
const TERMIOS_BYTES: usize = 64;
const INPUT_FLAGS: usize = 0;
const OUTPUT_FLAGS: usize = 4;
const LOCAL_FLAGS: usize = 12;
const CONTROL_CHARACTERS: usize = 17;
const VTIME: usize = 5;
const VMIN: usize = 6;
const ICRNL: u32 = 0o400;
const IXON: u32 = 0o2000;
const OPOST: u32 = 0o1;
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const IEXTEN: u32 = 0o100_000;
const POLLIN: i16 = 1;
const POLLHUP: i16 = 0x10;
const POLLERR: i16 = 8;
const IDLE_WAIT_MS: u64 = 250;
const FRAME_BYTES: usize = 64 * 1024;
const READ_BYTES: usize = 4096;
const DEFAULT_SIZE: (usize, usize) = (24, 80);
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x02 | 0x20;
const NO_DESCRIPTOR: usize = usize::MAX;
const SIGHUP: usize = eggos_runtime::signal::SIGHUP as usize;
const SIGNAL_SET_BYTES: usize = eggos_runtime::signal::SET_BYTES;
const WORKING_FOLDER_BYTES: usize = 4096;

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as the
    // call reads or writes them; a new anonymous mapping changes no memory the program uses.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn region<T>(count: usize) -> Result<&'static mut [T], Errno> {
    let bytes = count
        .checked_mul(core::mem::size_of::<T>())
        .ok_or(Errno::ENOMEM)?;
    let at = call(
        number::MMAP,
        [0, bytes, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, NO_DESCRIPTOR, 0],
    )?;
    // SAFETY: `at` starts a new private anonymous mapping of `bytes` bytes, page-aligned and
    // zero-filled, which nothing else refers to and which the program never unmaps; it is
    // only made for `u8` and `u32`, for which zero bytes are a value.
    Ok(unsafe { core::slice::from_raw_parts_mut(at as *mut T, count) })
}

/// Whether `descriptor` is a terminal (`TCGETS` answers).
pub fn is_terminal(descriptor: usize) -> bool {
    let mut settings = [0u8; TERMIOS_BYTES];
    call(
        number::IOCTL,
        [descriptor, tty::TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    )
    .is_ok()
}

fn settings() -> Option<[u8; TERMIOS_BYTES]> {
    let mut settings = [0u8; TERMIOS_BYTES];
    call(
        number::IOCTL,
        [INPUT, tty::TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    )
    .ok()?;
    Some(settings)
}

fn set_settings(settings: &[u8; TERMIOS_BYTES]) {
    let _ = call(
        number::IOCTL,
        [INPUT, tty::TCSETSW, settings.as_ptr() as usize, 0, 0, 0],
    );
}

fn flags(settings: &[u8; TERMIOS_BYTES], at: usize) -> u32 {
    u32::from_ne_bytes([settings[at], settings[at + 1], settings[at + 2], settings[at + 3]])
}

fn set_flags(settings: &mut [u8; TERMIOS_BYTES], at: usize, value: u32) {
    settings[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

fn raw(mut settings: [u8; TERMIOS_BYTES]) -> [u8; TERMIOS_BYTES] {
    let input = flags(&settings, INPUT_FLAGS) & !(ICRNL | IXON);
    let output = flags(&settings, OUTPUT_FLAGS) & !OPOST;
    let local = flags(&settings, LOCAL_FLAGS) & !(ICANON | ECHO | ISIG | IEXTEN);
    set_flags(&mut settings, INPUT_FLAGS, input);
    set_flags(&mut settings, OUTPUT_FLAGS, output);
    set_flags(&mut settings, LOCAL_FLAGS, local);
    settings[CONTROL_CHARACTERS + VMIN] = 1;
    settings[CONTROL_CHARACTERS + VTIME] = 0;
    settings
}

fn window_size() -> Option<(usize, usize)> {
    let mut size = [0u16; 4];
    call(
        number::IOCTL,
        [OUTPUT, tty::TIOCGWINSZ, size.as_mut_ptr() as usize, 0, 0, 0],
    )
    .ok()?;
    (size[0] > 0 && size[1] > 0).then_some((usize::from(size[0]), usize::from(size[1])))
}

fn variable<'s>(startup: &'s Startup, name: &[u8]) -> Option<&'s [u8]> {
    startup.environment().find_map(|entry| {
        let entry = entry.to_bytes();
        entry
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(b"="))
    })
}

fn number_of(text: Option<&[u8]>) -> Option<usize> {
    core::str::from_utf8(text?).ok()?.parse().ok().filter(|&value| value > 0)
}

fn readable_within(milliseconds: u64) -> (bool, bool) {
    #[repr(C)]
    struct Watched {
        descriptor: i32,
        events: i16,
        returned: i16,
    }
    let mut watched = Watched {
        descriptor: INPUT as i32,
        events: POLLIN,
        returned: 0,
    };
    let time = [(milliseconds / 1000) as i64, (milliseconds % 1000 * 1_000_000) as i64];
    match call(
        number::PPOLL,
        [&raw mut watched as usize, 1, time.as_ptr() as usize, 0, 8, 0],
    ) {
        Ok(0) | Err(_) => (false, false),
        Ok(_) => (
            watched.returned & POLLIN != 0,
            watched.returned & (POLLHUP | POLLERR) != 0,
        ),
    }
}

fn ignore_hangups() {
    let action = eggos_runtime::signal::action(eggos_runtime::signal::IGNORE);
    let _ = call(
        number::RT_SIGACTION,
        [SIGHUP, action.as_ptr() as usize, 0, SIGNAL_SET_BYTES, 0, 0],
    );
}

fn working_folder(out: &mut [u8; WORKING_FOLDER_BYTES]) -> &[u8] {
    match call(number::GETCWD, [out.as_mut_ptr() as usize, out.len(), 0, 0, 0, 0]) {
        Ok(_) => {
            let end = out.iter().position(|&byte| byte == 0).unwrap_or(0);
            &out[..end]
        }
        Err(_) => b"/home/root",
    }
}

/// Runs the terminal mode on the process's terminal, on the file at `path` (absolute) or an
/// untitled document, until the person quits (0) or the terminal hangs up (1, the unsaved
/// document written to `<path>.save`).
pub fn run(startup: &Startup, path: Option<&[u8]>) -> i32 {
    let (text, starts, history, clipboard) = MEMORY_SIZES;
    let memory = (|| {
        Ok::<_, Errno>((
            Memory {
                text: region::<u8>(text)?,
                starts: region::<u32>(starts)?,
                history: region::<u8>(history)?,
                clipboard: region::<u8>(clipboard)?,
            },
            region::<u8>(TEXT_CAPACITY)?,
        ))
    })();
    let Ok((memory, scratch)) = memory else {
        write_all(2, b"edit: no memory for the document\n");
        return 1;
    };
    let Some(kept) = settings() else {
        write_all(2, b"edit: there is no terminal to edit in; `cat` shows a file and `tee` writes one\n");
        return 1;
    };
    let style = Style::from_environment(
        variable(startup, b"TERM"),
        variable(startup, b"NO_COLOR"),
        variable(startup, b"TERM_PROGRAM"),
    );
    let reported = window_size();
    let (rows, columns) = reported
        .or_else(|| Some((number_of(variable(startup, b"LINES"))?, number_of(variable(startup, b"COLUMNS"))?)))
        .unwrap_or(DEFAULT_SIZE);
    let mut files = LinuxFiles::new();
    let mut editor = TerminalEditor::new(Editor::new(memory), scratch, style, rows, columns);
    editor.open(&mut files, path);
    ignore_hangups();
    set_settings(&raw(kept));
    write_all(OUTPUT, START);
    if reported.is_none() {
        write_all(OUTPUT, SIZE_QUERY);
    }
    let frame = match region::<u8>(FRAME_BYTES) {
        Ok(frame) => frame,
        Err(_) => {
            write_all(OUTPUT, END);
            set_settings(&kept);
            return 1;
        }
    };
    let mut input = [0u8; READ_BYTES];
    let mut size = (rows, columns);
    let status = loop {
        let length = editor.frame(frame);
        write_all(OUTPUT, &frame[..length]);
        let wait = match editor.waits_for_escape() {
            true => ESCAPE_WAIT_MS,
            false => IDLE_WAIT_MS,
        };
        let (readable, hung_up) = readable_within(wait);
        if let Some(now) = window_size()
            && now != size
        {
            size = now;
            editor.resize(now.0, now.1);
        }
        if !readable && !hung_up {
            if editor.escape_timed_out(&mut files) == Outcome::Quit {
                break 0;
            }
            continue;
        }
        match call(
            number::READ,
            [INPUT, input.as_mut_ptr() as usize, input.len(), 0, 0, 0],
        ) {
            Ok(0) => {
                let mut folder = [0u8; WORKING_FOLDER_BYTES];
                let folder = working_folder(&mut folder);
                editor.rescue(&mut files, folder);
                break 1;
            }
            Ok(count) => {
                if editor.feed(&mut files, &input[..count]) == Outcome::Quit {
                    break 0;
                }
            }
            Err(Errno::EINTR | Errno::EAGAIN) => {}
            Err(_) => {
                let mut folder = [0u8; WORKING_FOLDER_BYTES];
                let folder = working_folder(&mut folder);
                editor.rescue(&mut files, folder);
                break 1;
            }
        }
    };
    write_all(OUTPUT, END);
    set_settings(&kept);
    status
}
