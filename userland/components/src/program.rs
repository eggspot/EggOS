//! `component` on EggOS: the command over the system's files, its standard streams and the
//! terminal.

use crate::command::{self, Console, Context, USAGE, USAGE_STATUS};
use core::ffi::CStr;
use eggos_app::linux::{self, LinuxFiles};
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};

const TCGETS: usize = 0x5401;
const TERMIOS_SIZE: usize = 64;
const STANDARD_INPUT: usize = 0;
const STANDARD_OUTPUT: usize = 1;
const STANDARD_ERROR: usize = 2;
const MAX_ARGUMENTS: usize = 8;

fn syscall(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes buffers it owns and borrows for the call, sized as the call
    // reads or writes them, or plain numbers; no call here maps or unmaps memory.
    Errno::result(unsafe { call(number, arguments) })
}

fn is_terminal(descriptor: usize) -> bool {
    let mut settings = [0u8; TERMIOS_SIZE];
    syscall(
        number::IOCTL,
        [descriptor, TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    )
    .is_ok()
}

/// The command's standard input, output and error.
#[derive(Debug)]
pub struct Terminal;

impl Console for Terminal {
    fn write_output(&mut self, bytes: &[u8]) {
        write_all(STANDARD_OUTPUT, bytes);
    }

    fn write_error(&mut self, bytes: &[u8]) {
        write_all(STANDARD_ERROR, bytes);
    }

    fn is_terminal(&mut self) -> bool {
        is_terminal(STANDARD_INPUT) && is_terminal(STANDARD_ERROR)
    }

    fn read_answer(&mut self, buffer: &mut [u8]) -> usize {
        let mut length = 0;
        while length < buffer.len() {
            let rest = &mut buffer[length..];
            match syscall(
                number::READ,
                [
                    STANDARD_INPUT,
                    rest.as_mut_ptr() as usize,
                    rest.len(),
                    0,
                    0,
                    0,
                ],
            ) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    length += read;
                    if buffer[..length].contains(&b'\n') {
                        break;
                    }
                }
            }
        }
        length
    }
}

/// Runs `component` with the program's arguments; the exit status.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = [""; MAX_ARGUMENTS];
    let mut count = 0;
    for argument in startup.arguments().skip(1).map(CStr::to_bytes) {
        let Ok(argument) = core::str::from_utf8(argument) else {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        };
        if count == arguments.len() {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        }
        arguments[count] = argument;
        count += 1;
    }
    let coded = startup
        .environment()
        .any(|variable| variable == eggos_commands::tools::CODED_ERRORS_ENVIRONMENT);
    let context = Context {
        arch: linux::machine().name(),
        pid: syscall(number::GETPID, [0; 6]).unwrap_or(0) as u32,
        coded,
        download: Some(download_with_fetch),
        download_range: Some(download_range_with_fetch),
        now_s: wall_clock_s(),
    };
    command::run(
        &mut LinuxFiles::new(),
        &mut Terminal,
        context,
        &arguments[..count],
    )
}

fn wall_clock_s() -> u64 {
    const CLOCK_REALTIME: usize = 0;
    let mut time = [0i64; 2];
    match syscall(number::CLOCK_GETTIME, [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0]) {
        Ok(_) => time[0].max(0) as u64,
        Err(_) => 0,
    }
}

const FETCH: &CStr = c"/bin/fetch";
const QUIET: &CStr = c"-q";
const OUTPUT: &CStr = c"-o";
const RANGE: &CStr = c"--range";
const SIGCHLD: usize = 17;
const PATH_BYTES: usize = 512;

fn c_string(text: &str, out: &mut [u8; PATH_BYTES]) -> Option<usize> {
    if text.len() >= PATH_BYTES || text.as_bytes().contains(&0) {
        return None;
    }
    out[..text.len()].copy_from_slice(text.as_bytes());
    out[text.len()] = 0;
    Some(out.as_ptr() as usize)
}

/// Runs `fetch -q --range <first>-<last> -o <path> <url>` (D271) and waits for it: whether it
/// ended with status 0.
fn download_range_with_fetch(url: &str, path: &str, first: u64, last: u64) -> bool {
    let range = alloc::format!("{first}-{last}");
    run_fetch(url, path, Some(&range))
}

/// Runs `fetch -q -o <path> <url>` (D257) and waits for it: whether it ended with status 0.
/// The folder is made first; the download shows in the Downloads window as any `fetch` does.
fn download_with_fetch(url: &str, path: &str) -> bool {
    run_fetch(url, path, None)
}

fn run_fetch(url: &str, path: &str, range: Option<&str>) -> bool {
    let mut folder = [0u8; PATH_BYTES];
    if let Some(folder) = c_string(crate::manager::DOWNLOAD_FOLDER, &mut folder) {
        let _ = syscall(number::MKDIRAT, [-100isize as usize, folder, 0o755, 0, 0, 0]);
    }
    let (mut url_bytes, mut path_bytes, mut range_bytes) = ([0u8; PATH_BYTES], [0u8; PATH_BYTES], [0u8; PATH_BYTES]);
    let (Some(url), Some(path)) = (c_string(url, &mut url_bytes), c_string(path, &mut path_bytes)) else {
        return false;
    };
    let (quiet, output) = (QUIET.as_ptr() as usize, OUTPUT.as_ptr() as usize);
    let arguments = match range {
        None => [FETCH.as_ptr() as usize, quiet, output, path, url, 0, 0, 0],
        Some(range) => {
            let Some(range) = c_string(range, &mut range_bytes) else {
                return false;
            };
            [FETCH.as_ptr() as usize, quiet, RANGE.as_ptr() as usize, range, output, path, url, 0]
        }
    };
    let environment = [0usize];
    let Ok(child) = syscall(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]) else {
        return false;
    };
    if child == 0 {
        let _ = syscall(
            number::EXECVE,
            [FETCH.as_ptr() as usize, arguments.as_ptr() as usize, environment.as_ptr() as usize, 0, 0, 0],
        );
        let _ = syscall(number::EXIT_GROUP, [127, 0, 0, 0, 0, 0]);
        return false;
    }
    let mut status = 0i32;
    loop {
        match syscall(number::WAIT4, [child, &raw mut status as usize, 0, 0, 0, 0]) {
            Err(Errno::EINTR) => continue,
            Ok(_) => return status & 0x7f == 0 && (status >> 8) & 0xff == 0,
            Err(_) => return false,
        }
    }
}
