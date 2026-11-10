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
    };
    command::run(
        &mut LinuxFiles::new(),
        &mut Terminal,
        context,
        &arguments[..count],
    )
}
