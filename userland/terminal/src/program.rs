//! `/bin/terminal`, a name of the Settings program (D187, docs/ux/terminal-windows-v0.3.md
//! section 8): opens a terminal window running the shell and returns at once. The program forks
//! first (a process with a mapped window cannot), the parent ending with 0 so the prompt that
//! started it comes back; the child leaves the caller's session, opens a pseudo-terminal
//! (`/dev/ptmx`, unlocked, its program side `/dev/pts/<n>` opened without taking it), starts
//! `/bin/eggsh` on it as the leader of a session whose controlling terminal it is, with
//! `TERM=xterm`, the caller's environment and folder (the home folder when started from `/`),
//! then serves the window until the shell ends or the window closes; its end closes the
//! controlling side, which hangs the shell's session up. `SIGTERM`, which init's shutdown sends
//! (D183), ends it as it ends any program, with the same effect. Exits 1 with `terminal: no
//! display` when there is no window server, 2 for an argument.
//!
//! The engine allocates its grid, the program its picture of the window: memory comes from the
//! kernel a mapping at a time ([`Pages`]), so the program pays for what it shows.

use crate::window::{DEFAULT_CONTENT, MINIMUM_CONTENT, Shell, ShellOutput, TerminalWindow};
use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::CStr;
use core::fmt::Write;
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{self, ChildStack, Spawn};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, View};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::elements::{Action, Outcome, TreeWriter};
use eggos_toolkit::input::Input;
use eggos_toolkit::window::{TAKES_CTRL_KEYS, Window, WindowServer, now_ms};

const SHELL: &CStr = c"/bin/eggsh";
const SHELL_NAME: &CStr = c"eggsh";
const TERMINAL: &CStr = c"/bin/terminal";
const TERMINAL_NAME: &CStr = c"terminal";
const PSEUDO_TERMINAL_MAKER: &CStr = c"/dev/ptmx";
const TERM: &CStr = c"TERM=xterm";
const HOME_PREFIX: &[u8] = b"HOME=";
const TERM_PREFIX: &[u8] = b"TERM=";

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR: usize = 0o2;
const O_NOCTTY: usize = 0o400;
const O_NONBLOCK: usize = 0o4000;
const O_CLOEXEC: usize = 0o2_000_000;
const TIOCSWINSZ: usize = 0x5414;
const TIOCGPGRP: usize = 0x540f;
const TIOCGPTN: usize = 0x8004_5430;
const TIOCSPTLCK: usize = 0x4004_5431;
const SIGCHLD: usize = 17;
const PROT_READ_WRITE: usize = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x02 | 0x20;
const NO_DESCRIPTOR: usize = usize::MAX;
const PAGE: usize = 4096;
const MAX_ENVIRONMENT: usize = 126;
const PATH_CAPACITY: usize = 4096;

const NO_DISPLAY_STATUS: i32 = 1;
const USAGE_STATUS: i32 = 2;
const FAILED_STATUS: i32 = 1;

const STANDARD_ERROR: usize = 2;

/// The program's memory: each allocation a private anonymous mapping of whole pages, given
/// back when freed. The engine and the window's picture allocate a few large blocks, rarely,
/// so a mapping each costs less than any heap would.
#[derive(Debug)]
pub struct Pages;

fn whole_pages(size: usize) -> usize {
    size.div_ceil(PAGE) * PAGE
}

// SAFETY: `alloc` returns null or a new mapping of at least `layout.size()` bytes, aligned to a
// page, which meets every alignment up to a page (larger ones get null), and no other block
// overlaps it; `dealloc` unmaps exactly the pages `alloc` mapped for the same layout.
unsafe impl GlobalAlloc for Pages {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > PAGE {
            return core::ptr::null_mut();
        }
        let length = whole_pages(layout.size().max(1));
        // SAFETY: a new private anonymous mapping changes no memory the program uses.
        let at = unsafe {
            syscall::call(
                number::MMAP,
                [
                    0,
                    length,
                    PROT_READ_WRITE,
                    MAP_PRIVATE_ANONYMOUS,
                    NO_DESCRIPTOR,
                    0,
                ],
            )
        };
        match Errno::result(at) {
            Ok(at) => at as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let length = whole_pages(layout.size().max(1));
        // SAFETY: `pointer` and `length` are the mapping `alloc` made for this layout, which
        // the caller gives back and no longer uses.
        unsafe { syscall::call(number::MUNMAP, [pointer as usize, length, 0, 0, 0, 0]) };
    }
}

#[global_allocator]
static MEMORY: Pages = Pages;

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as the
    // call reads or writes them, or plain numbers.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn write_all(descriptor: usize, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match call(
            number::WRITE,
            [descriptor, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
        ) {
            Ok(0) | Err(_) => return,
            Ok(written) => bytes = &bytes[written.min(bytes.len())..],
        }
    }
}

fn complain(what: &[u8], error: Errno) -> i32 {
    write_all(STANDARD_ERROR, b"terminal: ");
    write_all(STANDARD_ERROR, what);
    write_all(STANDARD_ERROR, error.message());
    write_all(STANDARD_ERROR, b"\n");
    FAILED_STATUS
}

fn close(descriptor: usize) {
    let _ = call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
}

struct PathText {
    bytes: [u8; 24],
    length: usize,
}

impl Write for PathText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let room = self
            .bytes
            .get_mut(self.length..end)
            .ok_or(core::fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

type Environment = [*const u8; MAX_ENVIRONMENT + 2];

struct PseudoTerminal {
    controlling: usize,
    shell: usize,
    environment: Environment,
}

impl Drop for PseudoTerminal {
    fn drop(&mut self) {
        close(self.controlling);
    }
}

fn open_pseudo_terminal() -> Result<(usize, usize), Errno> {
    let controlling = call(
        number::OPENAT,
        [
            AT_FDCWD,
            PSEUDO_TERMINAL_MAKER.as_ptr() as usize,
            O_RDWR | O_NOCTTY | O_NONBLOCK | O_CLOEXEC,
            0,
            0,
            0,
        ],
    )?;
    match open_program_side(controlling) {
        Ok(program_side) => Ok((controlling, program_side)),
        Err(error) => {
            close(controlling);
            Err(error)
        }
    }
}

fn open_program_side(controlling: usize) -> Result<usize, Errno> {
    let mut unlocked = 0i32.to_ne_bytes();
    call(
        number::IOCTL,
        [
            controlling,
            TIOCSPTLCK,
            unlocked.as_mut_ptr() as usize,
            0,
            0,
            0,
        ],
    )?;
    let mut number = [0u8; 4];
    call(
        number::IOCTL,
        [controlling, TIOCGPTN, number.as_mut_ptr() as usize, 0, 0, 0],
    )?;
    let mut path = PathText {
        bytes: [0; 24],
        length: 0,
    };
    write!(path, "/dev/pts/{}\0", u32::from_ne_bytes(number)).map_err(|_| Errno::ENAMETOOLONG)?;
    call(
        number::OPENAT,
        [
            AT_FDCWD,
            path.bytes.as_ptr() as usize,
            O_RDWR | O_NOCTTY | O_CLOEXEC,
            0,
            0,
            0,
        ],
    )
}

fn home(startup: &Startup) -> Option<&'static CStr> {
    startup.environment().find_map(|variable| {
        let bytes = variable.to_bytes_with_nul();
        bytes
            .starts_with(HOME_PREFIX)
            .then(|| CStr::from_bytes_with_nul(&bytes[HOME_PREFIX.len()..]).ok())
            .flatten()
    })
}

fn started_at_root() -> bool {
    let mut folder = [0u8; PATH_CAPACITY];
    match call(
        number::GETCWD,
        [folder.as_mut_ptr() as usize, folder.len(), 0, 0, 0, 0],
    ) {
        Ok(_) => folder.starts_with(b"/\0"),
        Err(_) => true,
    }
}

fn environment_of(startup: &Startup) -> Environment {
    let mut environment = [core::ptr::null::<u8>(); MAX_ENVIRONMENT + 2];
    let mut count = 0;
    for variable in startup.environment() {
        if count == MAX_ENVIRONMENT {
            break;
        }
        if variable.to_bytes().starts_with(TERM_PREFIX) {
            continue;
        }
        environment[count] = variable.as_ptr().cast();
        count += 1;
    }
    environment[count] = TERM.as_ptr().cast();
    environment
}

fn start_shell(
    startup: &Startup,
    environment: &Environment,
    program_side: usize,
) -> Result<usize, Errno> {
    let arguments = [SHELL_NAME.as_ptr().cast::<u8>(), core::ptr::null()];
    let mut request = Spawn::new(
        SHELL,
        &arguments,
        environment,
        [program_side, program_side, program_side],
    )
    .with_controlling_terminal();
    if started_at_root()
        && let Some(home) = home(startup)
    {
        request = request.in_directory(home);
    }
    let mut stack = Box::new(ChildStack::new());
    process::spawn(&mut stack, &request)
}

impl Shell for PseudoTerminal {
    fn read(&mut self, buffer: &mut [u8]) -> ShellOutput {
        match call(
            number::READ,
            [
                self.controlling,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            ],
        ) {
            Ok(0) | Err(Errno::EIO) => ShellOutput::Ended,
            Ok(count) => ShellOutput::Bytes(count),
            Err(_) => ShellOutput::Nothing,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        write_all(self.controlling, bytes);
    }

    fn set_window_size(&mut self, columns: u16, rows: u16) {
        let cell_width = eggos_font::GLYPH_WIDTH as u16;
        let cell_height = eggos_font::GLYPH_HEIGHT as u16;
        let mut size = [0u8; 8];
        for (field, value) in size.as_chunks_mut::<2>().0.iter_mut().zip([
            rows,
            columns,
            columns.saturating_mul(cell_width),
            rows.saturating_mul(cell_height),
        ]) {
            *field = value.to_ne_bytes();
        }
        let _ = call(
            number::IOCTL,
            [
                self.controlling,
                TIOCSWINSZ,
                size.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        );
    }

    fn foreground_is_shell(&self) -> bool {
        let mut group = [0u8; 4];
        let asked = call(
            number::IOCTL,
            [
                self.controlling,
                TIOCGPGRP,
                group.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        );
        asked.is_err() || i32::from_ne_bytes(group) as usize == self.shell
    }

    fn open_new_window(&mut self) {
        let arguments = [TERMINAL_NAME.as_ptr().cast::<u8>(), core::ptr::null()];
        let request = Spawn::new(TERMINAL, &arguments, &self.environment, [0, 1, 2]);
        let mut stack = Box::new(ChildStack::new());
        if let Ok(child) = process::spawn(&mut stack, &request) {
            let mut status = 0i32;
            let _ = call(number::WAIT4, [child, &raw mut status as usize, 0, 0, 0, 0]);
        }
    }
}

struct Program {
    window: TerminalWindow<PseudoTerminal>,
}

impl View for Program {
    fn resize(&mut self, width: i32, height: i32) {
        self.window.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        self.window.handle(input, time_ms)
    }

    fn take_damage(&mut self) -> Damage {
        self.window.take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.window.paint(canvas);
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        self.window.deadline(now_ms)
    }

    fn tick(&mut self, now_ms: u32) {
        self.window.tick(now_ms);
    }

    fn close_requested(&mut self, time_ms: u32) -> bool {
        self.window.close_requested(time_ms)
    }

    fn take_title(&mut self) -> Option<&str> {
        self.window.take_title()
    }

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.window.describe(tree);
        true
    }

    fn act(&mut self, element: u32, _action: Action, _text: &str, _time_ms: u32) -> Outcome {
        self.window.act(element)
    }

    fn watched_descriptor(&self) -> Option<usize> {
        Some(self.window.shell().controlling)
    }

    fn descriptor_ready(&mut self, time_ms: u32) -> bool {
        self.window.read_shell(time_ms)
    }
}

fn serve(startup: &Startup) -> i32 {
    // SAFETY: `setsid` touches no memory.
    unsafe { syscall::call(number::SETSID, [0, 0, 0, 0, 0, 0]) };
    let (controlling, program_side) = match open_pseudo_terminal() {
        Ok(sides) => sides,
        Err(error) => return complain(b"cannot open a pseudo-terminal: ", error),
    };
    let environment = environment_of(startup);
    let started = start_shell(startup, &environment, program_side);
    close(program_side);
    let shell = match started {
        Ok(shell) => shell,
        Err(error) => {
            close(controlling);
            return complain(b"cannot start the shell: ", error);
        }
    };
    let pseudo_terminal = PseudoTerminal {
        controlling,
        shell,
        environment,
    };
    let (width, height) = DEFAULT_CONTENT;
    let mut window = match Window::create_with_flags(
        crate::title::DEFAULT_TITLE,
        width,
        height,
        MINIMUM_CONTENT,
        TAKES_CTRL_KEYS,
    ) {
        Ok(window) => window,
        Err(error) => return complain(b"cannot open its window: ", error),
    };
    let Some(view) = TerminalWindow::new(pseudo_terminal, width as i32, height as i32, now_ms())
    else {
        return complain(b"no memory for the screen: ", Errno::ENOMEM);
    };
    let mut program = Program { window: view };
    match application::run_view(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => complain(b"its window: ", error),
    }
}

/// The program's main function: see the module documentation.
pub fn run(startup: &Startup) -> i32 {
    if startup.argument_count() > 1 {
        write_all(STANDARD_ERROR, b"usage: terminal\n");
        return USAGE_STATUS;
    }
    if WindowServer::open().is_err() {
        write_all(STANDARD_ERROR, b"terminal: no display\n");
        return NO_DISPLAY_STATUS;
    }
    match call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]) {
        Ok(0) => syscall::exit(serve(startup)),
        Ok(_) => 0,
        Err(error) => complain(b"cannot start: ", error),
    }
}
