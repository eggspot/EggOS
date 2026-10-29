//! `/bin/text-editor [<path>]`, a name of the Settings program (D149): opens the Text Editor's
//! window on the file at `path` (relative to the working folder), or on an untitled document,
//! and serves it until it closes. Its memory (the text, the line index, the history and the
//! clipboard) is mapped when it starts and costs only the pages the document touches.

use crate::editor::{Editor, MEMORY_SIZES, Memory};
use crate::editor_window::{DEFAULT_SIZE, EditorWindow, MINIMUM_SIZE};
use core::ffi::CStr;
use eggos_app::files::{PATH_CAPACITY, Path};
use eggos_app::linux::{LinuxFiles, STANDARD_ERROR, write_all};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, View};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::input::Input;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::window::{Window, now_ms};

const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x02 | 0x20;
const NO_DESCRIPTOR: usize = usize::MAX;
const FAILED: i32 = 1;

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
        [
            0,
            bytes,
            PROT_READ_WRITE,
            MAP_PRIVATE_ANONYMOUS,
            NO_DESCRIPTOR,
            0,
        ],
    )?;
    // SAFETY: `at` starts a new private anonymous mapping of `bytes` bytes, page-aligned and
    // zero-filled, which nothing else refers to and which the program never unmaps; it is
    // only made for `u8` and `u32`, for which zero bytes are a value.
    Ok(unsafe { core::slice::from_raw_parts_mut(at as *mut T, count) })
}

fn memory() -> Result<Memory<'static>, Errno> {
    let (text, starts, history, clipboard) = MEMORY_SIZES;
    Ok(Memory {
        text: region::<u8>(text)?,
        starts: region::<u32>(starts)?,
        history: region::<u8>(history)?,
        clipboard: region::<u8>(clipboard)?,
    })
}

/// `path` made absolute against the working folder.
pub fn absolute(path: &[u8]) -> Option<Path> {
    if path.starts_with(b"/") {
        return Path::from(path);
    }
    let mut folder = [0u8; PATH_CAPACITY + 1];
    call(
        number::GETCWD,
        [folder.as_mut_ptr() as usize, folder.len(), 0, 0, 0, 0],
    )
    .ok()?;
    let end = folder.iter().position(|&byte| byte == 0)?;
    let folder = Path::from(&folder[..end])?;
    let relative = path.strip_prefix(b"./").unwrap_or(path);
    match folder.as_bytes() {
        b"/" => {
            let mut joined = Path::new();
            (joined.append(b"/") && joined.append(relative)).then_some(joined)
        }
        _ => folder.join(relative),
    }
}

struct Program<'a> {
    window: EditorWindow<'a>,
    files: LinuxFiles,
}

impl View for Program<'_> {
    fn resize(&mut self, width: i32, height: i32) {
        self.window.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        self.window.handle(&mut self.files, input, time_ms)
    }

    fn take_damage(&mut self) -> Damage {
        self.window.take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.window.paint(canvas);
    }

    fn pointer_shape(&mut self) -> PointerShape {
        self.window.pointer_shape()
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        self.window.deadline(now_ms)
    }

    fn tick(&mut self, now_ms: u32) {
        self.window.tick(now_ms);
    }

    fn close_requested(&mut self, _time_ms: u32) -> bool {
        self.window.close_requested()
    }

    fn take_title(&mut self) -> Option<&str> {
        self.window.take_title()
    }
}

fn complain(what: &[u8], error: Errno) -> i32 {
    write_all(STANDARD_ERROR, b"text-editor: ");
    write_all(STANDARD_ERROR, what);
    write_all(STANDARD_ERROR, error.message());
    write_all(STANDARD_ERROR, b"\n");
    FAILED
}

/// The program's main function: see the module documentation.
pub fn run(startup: &Startup) -> i32 {
    let given = startup.arguments().nth(1).map(CStr::to_bytes);
    let path = match given {
        Some(given) => match absolute(given) {
            Some(path) => Some(path),
            None => return complain(b"the path is too long: ", Errno::ENAMETOOLONG),
        },
        None => None,
    };
    let memory = match memory() {
        Ok(memory) => memory,
        Err(error) => return complain(b"no memory for the document: ", error),
    };
    let mut files = LinuxFiles::new();
    let view = EditorWindow::new(
        Editor::new(memory),
        &mut files,
        path.as_ref().map(Path::as_bytes),
        now_ms(),
    );
    let (width, height) = DEFAULT_SIZE;
    let mut window = match Window::create(view.title(), width, height, MINIMUM_SIZE) {
        Ok(window) => window,
        Err(error) => return complain(b"cannot open its window: ", error),
    };
    let mut program = Program {
        window: view,
        files,
    };
    match application::run_view(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => complain(b"its window: ", error),
    }
}
