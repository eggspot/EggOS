//! `/bin/files [<folder>]`, a name of the Settings program (D135): opens the Files window at
//! `folder` (home by default) and does what it asks of the system: typing a line in the
//! Terminal window (`/dev/window`'s `TYPE_IN_TERMINAL`), opening an installed app (`OPEN_APP`)
//! and starting the installer's window, `/usr/libexec/app-window` (D134), as a child process.

use crate::files_window::{DEFAULT_SIZE, FilesWindow, LINE_CAPACITY, MINIMUM_SIZE, Request, TITLE};
use core::cell::UnsafeCell;
use core::ffi::CStr;
use eggos_app::files::PATH_CAPACITY;
use eggos_app::linux::{self, LinuxFiles, STANDARD_ERROR, WINDOW_PROGRAM, write_all};
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{self, CLONE_VFORK, CLONE_VM};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, View};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::input::Input;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::window::Window;

const TYPE_IN_TERMINAL: usize = 0x4104_7711;
const WINDOW_DEVICE: &CStr = c"/dev/window";
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CLOEXEC: usize = 0o2 | 0o2_000_000;
const CLOCK_REALTIME: usize = 0;
const SIGCHLD: usize = 17;
const WNOHANG: usize = 1;
const CHILD_STACK_SIZE: usize = 16 * 1024;
const MAX_VARIABLES: usize = 32;
const NOT_RUN: i32 = 127;
const PROFILE_WINDOWS: u32 = 0;
const FAILED: i32 = 1;

#[repr(C, align(16))]
struct ChildStack(UnsafeCell<[u8; CHILD_STACK_SIZE]>);

// SAFETY: only a child started by `start_installer` uses the stack, while this program's only
// thread sleeps in its `CLONE_VFORK` clone, one child at a time.
unsafe impl Sync for ChildStack {}

static CHILD_STACK: ChildStack = ChildStack(UnsafeCell::new([0; CHILD_STACK_SIZE]));

#[repr(C)]
struct Spawn {
    program: usize,
    arguments: usize,
    environment: usize,
}

extern "C" fn start_child(spawn: usize) -> ! {
    // SAFETY: `start_installer` passes its `Spawn`, which stays as it is while this child runs
    // on the program's memory: the program's thread sleeps until the child runs `execve` or
    // ends.
    let spawn = unsafe { &*(spawn as *const Spawn) };
    // SAFETY: `execve` reads the C strings and the null-ended pointer arrays `spawn` names,
    // which stay in place while the parent sleeps.
    unsafe {
        syscall::call(
            number::EXECVE,
            [spawn.program, spawn.arguments, spawn.environment, 0, 0, 0],
        )
    };
    syscall::exit(NOT_RUN)
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as the
    // call reads or writes them, and no call here maps, unmaps or changes the program's memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn clock() -> u64 {
    let mut time = [0u64; 2];
    match call(
        number::CLOCK_GETTIME,
        [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
    ) {
        Ok(_) => time[0],
        Err(_) => 0,
    }
}

fn type_in_terminal(line: &[u8]) -> Result<(), Errno> {
    let mut argument = [0u8; 4 + LINE_CAPACITY];
    let length = line.len().min(LINE_CAPACITY);
    argument[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    argument[4..4 + length].copy_from_slice(&line[..length]);
    let fd = call(
        number::OPENAT,
        [
            AT_FDCWD,
            WINDOW_DEVICE.as_ptr() as usize,
            O_RDWR_CLOEXEC,
            0,
            0,
            0,
        ],
    )?;
    let asked = call(
        number::IOCTL,
        [fd, TYPE_IN_TERMINAL, argument.as_mut_ptr() as usize, 0, 0, 0],
    );
    let _ = call(number::CLOSE, [fd, 0, 0, 0, 0, 0]);
    asked.map(|_| ())
}

fn c_string(bytes: &[u8]) -> Option<[u8; PATH_CAPACITY + 1]> {
    if bytes.len() > PATH_CAPACITY || bytes.contains(&0) {
        return None;
    }
    let mut text = [0u8; PATH_CAPACITY + 1];
    text[..bytes.len()].copy_from_slice(bytes);
    Some(text)
}

struct Program {
    window: FilesWindow,
    files: LinuxFiles,
    environment: [usize; MAX_VARIABLES + 1],
}

impl Program {
    fn start_installer(&self, verb: &CStr, subject: &[u8]) {
        let Some(subject) = c_string(subject) else {
            return;
        };
        let arguments = [
            WINDOW_PROGRAM.as_ptr() as usize,
            verb.as_ptr() as usize,
            subject.as_ptr() as usize,
            0,
        ];
        let spawn = Spawn {
            program: WINDOW_PROGRAM.as_ptr() as usize,
            arguments: arguments.as_ptr() as usize,
            environment: self.environment.as_ptr() as usize,
        };
        let stack_top = CHILD_STACK.0.get() as usize + CHILD_STACK_SIZE;
        // SAFETY: the child runs `start_child` on `CHILD_STACK`, which nothing else uses, and
        // only reads `spawn` and the strings it names, all of which stay in place: this thread
        // sleeps until the child runs `execve` or ends.
        let child = unsafe {
            process::clone_onto_stack(
                CLONE_VM | CLONE_VFORK | SIGCHLD,
                stack_top,
                start_child,
                &raw const spawn as usize,
            )
        };
        if child < 0 {
            write_all(STANDARD_ERROR, b"files: could not start the installer's window\n");
        }
    }

    fn serve(&mut self) {
        while let Some(request) = self.window.take_request() {
            match request {
                Request::TypeInTerminal(line) => {
                    let _ = type_in_terminal(line.as_bytes());
                }
                Request::OpenApp(name) => {
                    let _ = linux::open_app(name.as_bytes());
                }
                Request::Install(path) => self.start_installer(c"install", path.as_bytes()),
                Request::Uninstall(name) => self.start_installer(c"uninstall", name.as_bytes()),
            }
        }
        while let Ok(child) = call(number::WAIT4, [usize::MAX, 0, WNOHANG, 0, 0, 0]) {
            if child == 0 {
                break;
            }
        }
    }
}

impl View for Program {
    fn resize(&mut self, width: i32, height: i32) {
        self.window.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        let open = self
            .window
            .handle(&mut self.files, input, time_ms, clock());
        self.serve();
        open
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
        self.window.tick(&mut self.files, now_ms, clock());
        self.serve();
    }
}

/// The program's main function: see the module documentation.
pub fn run(startup: &Startup) -> i32 {
    let start = startup.arguments().nth(1).map(CStr::to_bytes);
    let (width, height) = DEFAULT_SIZE;
    let mut window = match Window::create(TITLE, width, height, MINIMUM_SIZE) {
        Ok(window) => window,
        Err(error) => {
            write_all(STANDARD_ERROR, b"files: cannot open its window: ");
            write_all(STANDARD_ERROR, error.message());
            write_all(STANDARD_ERROR, b"\n");
            return FAILED;
        }
    };
    let windows = window.server().desktop_profile() == Ok(PROFILE_WINDOWS);
    let mut environment = [0usize; MAX_VARIABLES + 1];
    for (slot, variable) in environment[..MAX_VARIABLES]
        .iter_mut()
        .zip(startup.environment())
    {
        *slot = variable.as_ptr() as usize;
    }
    let mut files = LinuxFiles::new();
    let view = FilesWindow::new(&mut files, start, linux::machine(), windows, clock());
    let mut program = Program {
        window: view,
        files,
        environment,
    };
    match application::run_view(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => {
            write_all(STANDARD_ERROR, b"files: its window: ");
            write_all(STANDARD_ERROR, error.message());
            write_all(STANDARD_ERROR, b"\n");
            FAILED
        }
    }
}
