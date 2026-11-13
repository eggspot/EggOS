//! `/bin/text-editor [<path>]`, a name of the Settings program (D149): opens the Text Editor's
//! window on the file at `path` (relative to the working folder), or on an untitled document,
//! and serves it until it closes. Its memory (the text, the line index, the history and the
//! clipboard) is mapped when it starts and costs only the pages the document touches.

use crate::editor::{Editor, MEMORY_SIZES, Memory};
use crate::editor_window::{DEFAULT_SIZE, EditorWindow, MINIMUM_SIZE, PickRequest};
use core::ffi::CStr;
use eggos_app::files::{PATH_CAPACITY, Path};
use eggos_app::linux::{LinuxFiles, STANDARD_ERROR, write_all};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, View};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::elements::{Action, Outcome, TreeWriter};
use eggos_toolkit::input::Input;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::window::{Window, WindowServer, now_ms};

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
    server: Option<WindowServer>,
    clipboard_serial: Option<u32>,
}

impl Program<'_> {
    fn take_shared_clipboard(&mut self) {
        let Some(server) = self.server.as_ref() else {
            return;
        };
        let Ok((_, serial)) = server.clipboard(0, &mut []) else {
            return;
        };
        if self.clipboard_serial == Some(serial) {
            return;
        }
        self.clipboard_serial = Some(serial);
        self.window.take_shared_clipboard(|room| {
            let (length, _) = server.clipboard(0, room).ok()?;
            Some(length)
        });
    }

    fn give_copied(&mut self) {
        let (Some(server), Some(copied)) = (self.server.as_ref(), self.window.take_copied()) else {
            return;
        };
        if let Ok(text) = core::str::from_utf8(copied)
            && server.set_clipboard(text).is_ok()
        {
            self.clipboard_serial = server.clipboard(0, &mut []).ok().map(|(_, serial)| serial);
        }
    }
}

const PICK_PROGRAM: &[u8] = b"/bin/pick";
const AT_FDCWD: usize = -100isize as usize;
const PICK_OUTPUT_BYTES: usize = 4096;
const TEXT_FILTER: &[u8] = b"Text files|*.txt;*.md;*.log;*.csv;*.json;*.toml";
const ALL_FILTER: &[u8] = b"All files|*";

fn picker_available() -> bool {
    let mut status = [0u8; 256];
    // SAFETY: `newfstatat` writes at most one `struct stat` (under 256 bytes) into the
    // borrowed buffer and reads the NUL-terminated path.
    let found = unsafe {
        syscall::call(
            number::NEWFSTATAT,
            [
                AT_FDCWD,
                c"/bin/pick".as_ptr() as usize,
                status.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    };
    found == 0
}

fn ask_picker(request: &PickRequest) -> Option<Path> {
    let mode: &[u8] = if request.save { b"save" } else { b"open" };
    let title: &[u8] = if request.save { b"Save As" } else { b"Open" };
    let folder = request.folder;
    let name = request.name;
    let mut all: [&[u8]; 16] = [b""; 16];
    let fixed: [&[u8]; 10] = [
        b"pick",
        mode,
        b"--title",
        title,
        b"--filter",
        TEXT_FILTER,
        b"--filter",
        ALL_FILTER,
        b"--folder",
        folder.as_bytes(),
    ];
    all[..fixed.len()].copy_from_slice(&fixed);
    let mut count = fixed.len();
    if request.save {
        let more: [&[u8]; 4] = [b"--name", name.as_bytes(), b"--extension", b"txt"];
        all[count..count + more.len()].copy_from_slice(&more);
        count += more.len();
    }
    let mut output = [0u8; PICK_OUTPUT_BYTES];
    let (length, status) =
        eggos_app::linux::run_with_output(PICK_PROGRAM, &all[..count], &mut output).ok()?;
    if status != 0 {
        return None;
    }
    let line = output[..length].split(|&byte| byte == b'\n').next()?;
    Path::from(line)
}

const DROPPED_PATHS: u32 = 1;
const DROPPED_BYTES: usize = 8192;

impl Program<'_> {
    fn open_dropped(&mut self) {
        let Some(server) = self.server.as_ref() else {
            return;
        };
        let mut payload = [0u8; DROPPED_BYTES];
        let Ok((length, _, _)) = server.drag_payload(0, &mut payload) else {
            return;
        };
        let first = payload[..length.min(DROPPED_BYTES)]
            .split(|&byte| byte == b'\n')
            .find(|line| line.starts_with(b"/"));
        let Some(path) = first.and_then(Path::from) else {
            return;
        };
        let request = PickRequest {
            save: false,
            folder: Path::new(),
            name: Path::new(),
        };
        self.window.picked(&mut self.files, request, Some(path));
    }

    fn serve_picks(&mut self) {
        while let Some(request) = self.window.take_pick() {
            let chosen = ask_picker(&request);
            self.window.picked(&mut self.files, request, chosen);
        }
    }
}

impl View for Program<'_> {
    fn resize(&mut self, width: i32, height: i32) {
        self.window.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        if let Input::Drop {
            kind: DROPPED_PATHS,
            ..
        } = input
        {
            self.open_dropped();
            return true;
        }
        if matches!(
            input,
            Input::Key { pressed: true, .. } | Input::Button { pressed: true, .. }
        ) {
            self.take_shared_clipboard();
        }
        let handled = self.window.handle(&mut self.files, input, time_ms);
        self.give_copied();
        self.serve_picks();
        handled
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

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.window.describe(tree);
        true
    }

    fn act(&mut self, element: u32, action: Action, text: &str, _time_ms: u32) -> Outcome {
        let outcome = self.window.act(element, action, text);
        self.serve_picks();
        outcome
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
    let mut view = EditorWindow::new(
        Editor::new(memory),
        &mut files,
        path.as_ref().map(Path::as_bytes),
        now_ms(),
    );
    view.set_picker_available(picker_available());
    let (width, height) = DEFAULT_SIZE;
    let mut window = match Window::create(view.title(), width, height, MINIMUM_SIZE) {
        Ok(window) => window,
        Err(error) => return complain(b"cannot open its window: ", error),
    };
    let mut program = Program {
        window: view,
        files,
        server: WindowServer::open().ok(),
        clipboard_serial: None,
    };
    match application::run_view(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => complain(b"its window: ", error),
    }
}
