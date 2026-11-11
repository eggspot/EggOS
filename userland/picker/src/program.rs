//! `/bin/pick <mode> [options]` (D391, docs/ux/file-picker-v0.3.md section 15), a name of the
//! Settings program so the toolkit is in the image once: the command line read, the window
//! shown in the app's current folder, and the chosen paths printed one per line. Exit 0 with
//! the paths, 1 for Cancel, 2 for a usage error, 3 when no window can show.

use crate::options::Options;
use crate::picker_window::{DEFAULT_SIZE, Ending, MINIMUM_SIZE, PERSON_INPUT_ONLY, PickerWindow};
use crate::state::{Picker, output};
use core::ffi::CStr;
use eggos_app::files::PATH_CAPACITY;
use eggos_app::linux::{LinuxFiles, STANDARD_ERROR, STANDARD_OUTPUT, write_all};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_toolkit::application;
use eggos_toolkit::window::Window;

const CHOSEN: i32 = 0;
const CANCELLED: i32 = 1;
const USAGE: i32 = 2;
const NO_WINDOW: i32 = 3;

fn working_folder(out: &mut [u8; PATH_CAPACITY]) -> &[u8] {
    // SAFETY: `getcwd` writes at most the buffer's length into it.
    let written = unsafe {
        syscall::call(number::GETCWD, [out.as_mut_ptr() as usize, out.len(), 0, 0, 0, 0])
    };
    match written > 0 {
        true => {
            let end = out.iter().position(|&byte| byte == 0).unwrap_or(out.len());
            &out[..end]
        }
        false => b"/",
    }
}

/// The program's main function: see the module documentation.
pub fn run(startup: &Startup) -> i32 {
    let arguments = startup.arguments().skip(1).map(CStr::to_bytes);
    let options = match Options::parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            write_all(STANDARD_ERROR, error.message().as_bytes());
            return USAGE;
        }
    };
    let (width, height) = DEFAULT_SIZE;
    let title = options.title.clone();
    let mut window = match Window::create_with_flags(&title, width, height, MINIMUM_SIZE, PERSON_INPUT_ONLY) {
        Ok(window) => window,
        Err(_) => {
            write_all(STANDARD_ERROR, b"pick: cannot show a window\n");
            return NO_WINDOW;
        }
    };
    let mut folder = [0u8; PATH_CAPACITY];
    let working = working_folder(&mut folder);
    let mut files = LinuxFiles::new();
    let picker = Picker::new(&mut files, options, working);
    let mut view = PickerWindow::new(picker, files, (width as i32, height as i32));
    if application::run_view(&mut window, &mut view).is_err() {
        write_all(STANDARD_ERROR, b"pick: its window closed\n");
        return CANCELLED;
    }
    match view.ending() {
        Some(Ending::Chosen(paths)) => {
            write_all(STANDARD_OUTPUT, &output(paths));
            CHOSEN
        }
        _ => CANCELLED,
    }
}
