//! `/bin/downloads`, a name of the Settings program (D225): opens the Downloads window and
//! serves it until it closes. The downloads are the `fetch` processes it starts, which outlive
//! it; while it runs it reaps those that end.

use crate::downloads_window::{DEFAULT_SIZE, DownloadSystem, DownloadsWindow, MINIMUM_SIZE, TITLE};
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_app::files::Files;
use eggos_app::linux::{LinuxFiles, STANDARD_ERROR, write_all};
use eggos_download_list::{FOLDER, id_of};
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{self, ChildStack, Spawn};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application::{self, View, answer_clipboard};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::elements::{Action, Outcome, TreeWriter};
use eggos_toolkit::input::Input;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::window::{Window, WindowServer, now_ms};

const PROCESSES_PATH: &[u8] = b"/proc/ps";
const FETCH_PROGRAM: &core::ffi::CStr = c"/bin/fetch";
const QUIET_OPTION: &core::ffi::CStr = c"-q";
const NO_CLOBBER_OPTION: &core::ffi::CStr = c"--no-clobber";
const NULL_DEVICE: &core::ffi::CStr = c"/dev/null";
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CLOEXEC: usize = 0o2_000_002;
const ANY_CHILD: usize = usize::MAX;
const WNOHANG: usize = 1;
const FAILED: i32 = 1;
const RECORD_PATH_BYTES: usize = 64;

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as the
    // call reads or writes them; none of these calls maps, unmaps or changes memory the
    // program uses.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn record_path(id: u32) -> Text<RECORD_PATH_BYTES> {
    let mut path = Text::new();
    let _ = write!(path, "{FOLDER}/{id}");
    path
}

fn with_nul(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    bytes
}

struct LinuxDownloads {
    files: LinuxFiles,
}

impl LinuxDownloads {
    fn read_whole(&mut self, path: &[u8], text: &mut [u8]) -> Option<usize> {
        let file = self.files.open(path).ok()?;
        let mut length = 0;
        let read = loop {
            if length == text.len() {
                break Some(length);
            }
            match self.files.read(file, &mut text[length..]) {
                Ok(0) => break Some(length),
                Ok(count) => length += count,
                Err(Errno::EINTR) => {}
                Err(_) => break None,
            }
        };
        self.files.close(file);
        read
    }

    fn reap_ended_fetches(&mut self) {
        let mut status = 0i32;
        while let Ok(pid) = call(
            number::WAIT4,
            [ANY_CHILD, &raw mut status as usize, WNOHANG, 0, 0, 0],
        ) {
            if pid == 0 {
                break;
            }
        }
    }
}

impl DownloadSystem for LinuxDownloads {
    fn record_ids(&mut self, ids: &mut Vec<u32>) -> Result<(), Errno> {
        let mut name = [0u8; 256];
        for index in 0.. {
            match self.files.entry(FOLDER.as_bytes(), index, &mut name) {
                Ok(Some(length)) => ids.extend(id_of(&name[..length])),
                Ok(None) | Err(Errno::ENOENT) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn read_record(&mut self, id: u32, text: &mut [u8]) -> Option<usize> {
        let path = record_path(id);
        self.read_whole(path.as_str().as_bytes(), text)
    }

    fn read_processes(&mut self, text: &mut [u8]) -> Option<usize> {
        let room = text.len();
        self.read_whole(PROCESSES_PATH, text)
            .filter(|&length| length < room)
    }

    fn send_signal(&mut self, pid: u32, signal: u32) -> Result<(), i32> {
        call(number::KILL, [pid as usize, signal as usize, 0, 0, 0, 0])
            .map(|_| ())
            .map_err(|error| i32::from(error.0))
    }

    fn remove_record(&mut self, id: u32) -> Result<(), Errno> {
        let path = record_path(id);
        self.files.remove_file(path.as_str().as_bytes())
    }

    fn exists(&mut self, path: &str) -> bool {
        !matches!(self.files.status(path.as_bytes()), Err(Errno::ENOENT))
    }

    fn make_folder(&mut self, path: &str) -> Result<(), Errno> {
        self.files.make_directory(path.as_bytes())
    }

    fn start_fetch(&mut self, url: &str, path: &str) -> Result<(), Errno> {
        let url = with_nul(url);
        let path = with_nul(path);
        let quiet = call(
            number::OPENAT,
            [
                AT_FDCWD,
                NULL_DEVICE.as_ptr() as usize,
                O_RDWR_CLOEXEC,
                0,
                0,
                0,
            ],
        );
        let streams = match quiet {
            Ok(descriptor) => [descriptor; 3],
            Err(_) => [0, 1, 2],
        };
        let arguments = [
            c"fetch".as_ptr().cast::<u8>(),
            QUIET_OPTION.as_ptr().cast::<u8>(),
            NO_CLOBBER_OPTION.as_ptr().cast::<u8>(),
            url.as_ptr(),
            path.as_ptr(),
            core::ptr::null(),
        ];
        let environment = [core::ptr::null()];
        let request = Spawn::new(FETCH_PROGRAM, &arguments, &environment, streams).in_own_session();
        let mut stack = alloc::boxed::Box::new(ChildStack::new());
        let started = process::spawn(&mut stack, &request).map(|_| ());
        if let Ok(descriptor) = quiet {
            let _ = call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
        }
        started
    }
}

struct Program {
    window: DownloadsWindow,
    system: LinuxDownloads,
    server: Option<WindowServer>,
}

impl View for Program {
    fn resize(&mut self, width: i32, height: i32) {
        self.window.resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        let response = self.window.input(input, time_ms);
        let response =
            answer_clipboard(self.window.tree(), self.server.as_ref(), response, time_ms);
        self.window.respond(&mut self.system, response, time_ms);
        true
    }

    fn take_damage(&mut self) -> Damage {
        self.window.tree().take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.window.tree().paint(canvas);
    }

    fn pointer_shape(&mut self) -> PointerShape {
        self.window.tree().pointer_shape()
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        self.window.deadline(now_ms)
    }

    fn tick(&mut self, now_ms: u32) {
        self.system.reap_ended_fetches();
        self.window.tick(&mut self.system, now_ms);
    }

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.window.tree().describe(tree);
        true
    }

    fn act(&mut self, element: u32, action: Action, text: &str, time_ms: u32) -> Outcome {
        self.window
            .act(&mut self.system, element, action, text, time_ms)
    }

    fn palette_changed(&mut self, index: u32) {
        self.window.tree().set_palette(theme::palette_at(index));
    }

    fn motion_changed(&mut self, reduce: bool) {
        self.window.tree().set_reduce_motion(reduce);
    }
}

fn complain(what: &[u8], error: Errno) -> i32 {
    write_all(STANDARD_ERROR, b"downloads: ");
    write_all(STANDARD_ERROR, what);
    write_all(STANDARD_ERROR, error.message());
    write_all(STANDARD_ERROR, b"\n");
    FAILED
}

/// The program's main function: see the module documentation.
pub fn run(_startup: &Startup) -> i32 {
    let mut system = LinuxDownloads {
        files: LinuxFiles::new(),
    };
    let view = DownloadsWindow::new(&mut system, now_ms());
    let (width, height) = DEFAULT_SIZE;
    let mut window = match Window::create(TITLE, width, height, MINIMUM_SIZE) {
        Ok(window) => window,
        Err(error) => return complain(b"cannot open its window: ", error),
    };
    let mut program = Program {
        window: view,
        system,
        server: WindowServer::open().ok(),
    };
    match application::run_view(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => complain(b"its window: ", error),
    }
}
