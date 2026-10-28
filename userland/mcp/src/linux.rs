//! `eggmcpd`'s system calls: the agent channel's loop, running a CLI twin with its output
//! captured, the symbolic-link check, the audit log on `/var/log` and the clock.

use crate::audit::{LOG_FOLDER, LOG_PATH, RECORD_BYTES, ROTATE_BYTES, ROTATED_PATH};
use crate::framing::{Line, LineReader};
use crate::paths::MAX_PATH_BYTES;
use crate::server::{
    ARGUMENTS_BYTES, ERROR_OUTPUT_LIMIT, OUTPUT_LIMIT, RESPONSE_BYTES, Scratch, Server,
};
use crate::system::{Ended, Invocation, RunOutcome, System};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{ChildStack, Spawn, spawn};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

/// The longest message taken, in bytes; a longer line is dropped and answered with an error.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// How long a CLI twin may run before it is stopped.
pub const TWIN_TIME_LIMIT_MS: u64 = 10_000;

const CHANNEL: &CStr = c"/dev/agent";
const RESTARTED: &[u8] = b"--restarted";
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR: usize = 2;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0o100;
const O_APPEND: usize = 0o2000;
const O_CLOEXEC: usize = 0o2_000_000;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const STATX_TYPE: usize = 1;
const STATX_SIZE: usize = 0x200;
const STATX_BYTES: usize = 256;
const STATX_MODE: usize = 28;
const STATX_SIZE_OFFSET: usize = 40;
const S_IFMT: u16 = 0o170_000;
const S_IFLNK: u16 = 0o120_000;
const POLLIN: i16 = 1;
const POLLHUP: i16 = 0x10;
const POLLERR: i16 = 8;
const SIGKILL: usize = 9;
const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;
const CONSOLE: usize = 2;
const ENVIRONMENT: [&CStr; 3] = [c"PATH=/bin", c"HOME=/home/root", c"TERM=dumb"];
const MAX_WORDS: usize = 8;
const RETRY_PAUSE_MS: u64 = 100;

static CHILD_STACK: ChildStack = ChildStack::new();

struct Buffers {
    input: [u8; MAX_MESSAGE_BYTES],
    response: [u8; RESPONSE_BYTES],
    stdout: [u8; OUTPUT_LIMIT],
    stderr: [u8; ERROR_OUTPUT_LIMIT],
    decoded: [u8; MAX_PATH_BYTES + 1],
    resolved: [u8; MAX_PATH_BYTES + 1],
    record: [u8; RECORD_BYTES],
    arguments: [u8; ARGUMENTS_BYTES],
}

static mut BUFFERS: Buffers = Buffers {
    input: [0; MAX_MESSAGE_BYTES],
    response: [0; RESPONSE_BYTES],
    stdout: [0; OUTPUT_LIMIT],
    stderr: [0; ERROR_OUTPUT_LIMIT],
    decoded: [0; MAX_PATH_BYTES + 1],
    resolved: [0; MAX_PATH_BYTES + 1],
    record: [0; RECORD_BYTES],
    arguments: [0; ARGUMENTS_BYTES],
};

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every call here passes only buffers this program owns and borrows for the call,
    // sized as the call reads or writes them, and none maps, unmaps or protects memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn say(parts: &[&[u8]]) {
    for part in parts {
        syscall::write_all(CONSOLE, part);
    }
}

fn close(descriptor: usize) {
    let _ = call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
}

fn open(path: &CStr, flags: usize, mode: usize) -> Result<usize, Errno> {
    call(
        number::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags | O_CLOEXEC, mode, 0, 0],
    )
}

fn pause(milliseconds: u64) {
    let time = [
        (milliseconds / 1000) as i64,
        (milliseconds % 1000 * 1_000_000) as i64,
    ];
    let _ = call(
        number::CLOCK_NANOSLEEP,
        [CLOCK_MONOTONIC, 0, time.as_ptr() as usize, 0, 0, 0],
    );
}

fn now_ms() -> u64 {
    let mut time = [0i64; 2];
    let _ = call(
        number::CLOCK_GETTIME,
        [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
    );
    time[0] as u64 * 1000 + time[1] as u64 / 1_000_000
}

/// Opens the channel, records the start and answers messages until the machine stops.
pub fn run(startup: &Startup) -> i32 {
    let restarted = startup.arguments().any(|argument| argument.to_bytes() == RESTARTED);
    let channel = match open(CHANNEL, O_RDWR, 0) {
        Ok(channel) => channel,
        Err(error) => {
            say(&[b"eggmcpd: cannot open /dev/agent: ", error.message(), b"\n"]);
            return 1;
        }
    };
    let mut folder = [0u8; 16];
    if let Some(folder) = c_path(LOG_FOLDER, &mut folder) {
        let _ = call(
            number::MKDIRAT,
            [AT_FDCWD, folder.as_ptr() as usize, 0o755, 0, 0, 0],
        );
    }
    // SAFETY: the program has one thread and takes the buffers once, here, so no other
    // reference to them exists while these live.
    let buffers = unsafe { &mut *(&raw mut BUFFERS) };
    let Buffers {
        input,
        response,
        stdout,
        stderr,
        decoded,
        resolved,
        record,
        arguments,
    } = buffers;
    let mut scratch = Scratch {
        stdout,
        stderr,
        decoded,
        resolved,
        record,
        arguments,
    };
    let mut system = Linux;
    let mut server = match restarted {
        true => Server::restarted(),
        false => Server::new(),
    };
    server.start(restarted, &mut system, &mut scratch);
    let mut reader = LineReader::new(input);
    loop {
        let space = reader.free_space();
        let read = call(
            number::READ,
            [channel, space.as_mut_ptr() as usize, space.len(), 0, 0, 0],
        );
        match read {
            Ok(0) => {
                reader.end_of_input();
                server.disconnected(&mut system, &mut scratch);
                continue;
            }
            Ok(count) => reader.filled(count),
            Err(Errno::EINTR) => continue,
            Err(error) => {
                say(&[b"eggmcpd: reading /dev/agent failed: ", error.message(), b"\n"]);
                pause(RETRY_PAUSE_MS);
                continue;
            }
        }
        while let Some(line) = reader.next_line() {
            let length = match line {
                Line::Message(message) => {
                    server.handle_line(message, &mut system, &mut scratch, response)
                }
                Line::TooLong => Server::line_too_long(response),
            };
            if length == 0 {
                continue;
            }
            if let Some(answer) = response.get_mut(..=length) {
                if let Some(end) = answer.last_mut() {
                    *end = b'\n';
                }
                if !syscall::write_all(channel, answer) {
                    say(&[b"eggmcpd: an answer could not be sent\n"]);
                }
            }
        }
    }
}

struct Linux;

#[repr(C)]
struct PollDescriptor {
    descriptor: i32,
    events: i16,
    returned_events: i16,
}

fn pipe() -> Result<[usize; 2], Errno> {
    let mut ends = [0i32; 2];
    call(
        number::PIPE2,
        [ends.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0],
    )?;
    Ok([ends[0] as usize, ends[1] as usize])
}

fn statx(path: &CStr, mask: usize, flags: usize) -> Result<[u8; STATX_BYTES], Errno> {
    let mut status = [0u8; STATX_BYTES];
    call(
        number::STATX,
        [
            AT_FDCWD,
            path.as_ptr() as usize,
            flags,
            mask,
            status.as_mut_ptr() as usize,
            0,
        ],
    )?;
    Ok(status)
}

fn c_path<'b>(path: &str, buffer: &'b mut [u8]) -> Option<&'b CStr> {
    let place = buffer.get_mut(..path.len() + 1)?;
    place[..path.len()].copy_from_slice(path.as_bytes());
    place[path.len()] = 0;
    CStr::from_bytes_with_nul(place).ok()
}

fn wait_for(child: usize) -> Ended {
    loop {
        let mut status = 0i32;
        match call(
            number::WAIT4,
            [child, &raw mut status as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => {
                let signal = status & 0x7f;
                return match signal {
                    0 => Ended::Exited((status >> 8) & 0xff),
                    _ => Ended::Signalled(signal),
                };
            }
            Err(Errno::EINTR) => continue,
            Err(error) => return Ended::NotStarted(error),
        }
    }
}

struct Capture<'b> {
    descriptor: usize,
    buffer: &'b mut [u8],
    kept: usize,
    open: bool,
    overflowed: bool,
}

impl Capture<'_> {
    fn take(&mut self) {
        let mut spill = [0u8; 512];
        let room = self.buffer.len() - self.kept;
        let (target, length) = match room {
            0 => (spill.as_mut_ptr(), spill.len()),
            _ => (self.buffer[self.kept..].as_mut_ptr(), room),
        };
        match call(number::READ, [self.descriptor, target as usize, length, 0, 0, 0]) {
            Ok(0) | Err(_) => self.open = false,
            Ok(count) if room == 0 => self.overflowed |= count > 0,
            Ok(count) => self.kept += count,
        }
    }
}

impl Linux {
    fn run_captured(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> Result<RunOutcome, Errno> {
        let mut path_storage = [0u8; MAX_PATH_BYTES + 1];
        let mut pointers = [core::ptr::null::<u8>(); MAX_WORDS + 4];
        let mut program = [0u8; 64];
        let program = c_path(invocation.program, &mut program).ok_or(Errno::E2BIG)?;
        pointers[0] = program.as_ptr().cast();
        let mut count = 1;
        let mut word_storage = [[0u8; 32]; MAX_WORDS];
        for (word, storage) in invocation.words.iter().zip(word_storage.iter_mut()) {
            pointers[count] = c_path(word, storage).ok_or(Errno::E2BIG)?.as_ptr().cast();
            count += 1;
        }
        if let Some(path) = invocation.path {
            pointers[count] = c"--".as_ptr().cast();
            count += 1;
            pointers[count] = c_path(path, &mut path_storage).ok_or(Errno::E2BIG)?.as_ptr().cast();
            count += 1;
        }
        pointers[count] = core::ptr::null();
        let mut environment = [core::ptr::null::<u8>(); ENVIRONMENT.len() + 1];
        for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
            *slot = variable.as_ptr().cast();
        }
        let input = pipe()?;
        close(input[1]);
        let output = pipe().inspect_err(|_| close(input[0]))?;
        let errors = pipe().inspect_err(|_| {
            close(input[0]);
            close(output[0]);
            close(output[1]);
        })?;
        let request = Spawn::new(
            program,
            &pointers[..=count],
            &environment,
            [input[0], output[1], errors[1]],
        );
        let started = spawn(&CHILD_STACK, &request);
        for descriptor in [input[0], output[1], errors[1]] {
            close(descriptor);
        }
        let child = match started {
            Ok(child) => child,
            Err(error) => {
                close(output[0]);
                close(errors[0]);
                return Err(error);
            }
        };
        let mut out = Capture {
            descriptor: output[0],
            buffer: stdout,
            kept: 0,
            open: true,
            overflowed: false,
        };
        let mut err = Capture {
            descriptor: errors[0],
            buffer: stderr,
            kept: 0,
            open: true,
            overflowed: false,
        };
        let deadline = now_ms() + TWIN_TIME_LIMIT_MS;
        let mut timed_out = false;
        while out.open || err.open {
            if out.overflowed {
                break;
            }
            let left = deadline.saturating_sub(now_ms());
            if left == 0 {
                timed_out = true;
                break;
            }
            let mut watched = [
                PollDescriptor {
                    descriptor: if out.open { out.descriptor as i32 } else { -1 },
                    events: POLLIN,
                    returned_events: 0,
                },
                PollDescriptor {
                    descriptor: if err.open { err.descriptor as i32 } else { -1 },
                    events: POLLIN,
                    returned_events: 0,
                },
            ];
            let timeout = [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64];
            match call(
                number::PPOLL,
                [watched.as_mut_ptr() as usize, 2, timeout.as_ptr() as usize, 0, 8, 0],
            ) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(_) => break,
            }
            let ready = |entry: &PollDescriptor| entry.returned_events & (POLLIN | POLLHUP | POLLERR) != 0;
            if ready(&watched[0]) {
                out.take();
            }
            if ready(&watched[1]) {
                err.take();
            }
        }
        let stopped = timed_out || out.overflowed;
        if stopped {
            let _ = call(number::KILL, [child, SIGKILL, 0, 0, 0, 0]);
        }
        close(output[0]);
        close(errors[0]);
        let ended = match (timed_out, wait_for(child)) {
            (true, _) => Ended::TimedOut,
            (false, ended) => ended,
        };
        Ok(RunOutcome {
            ended,
            stdout: out.kept,
            stdout_cut: out.overflowed,
            stderr: err.kept,
        })
    }
}

impl System for Linux {
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> RunOutcome {
        self.run_captured(invocation, stdout, stderr)
            .unwrap_or_else(|error| RunOutcome {
                ended: Ended::NotStarted(error),
                stdout: 0,
                stdout_cut: false,
                stderr: 0,
            })
    }

    fn is_link(&mut self, path: &str) -> bool {
        let mut buffer = [0u8; MAX_PATH_BYTES + 1];
        let Some(path) = c_path(path, &mut buffer) else {
            return false;
        };
        statx(path, STATX_TYPE, AT_SYMLINK_NOFOLLOW).is_ok_and(|status| {
            let mode = u16::from_ne_bytes([status[STATX_MODE], status[STATX_MODE + 1]]);
            mode & S_IFMT == S_IFLNK
        })
    }

    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno> {
        let mut buffer = [0u8; 64];
        let log = c_path(LOG_PATH, &mut buffer).ok_or(Errno::EINVAL)?;
        if let Ok(status) = statx(log, STATX_SIZE, 0) {
            let mut size = [0u8; 8];
            size.copy_from_slice(&status[STATX_SIZE_OFFSET..STATX_SIZE_OFFSET + 8]);
            if u64::from_ne_bytes(size) + record.len() as u64 > ROTATE_BYTES {
                let mut rotated = [0u8; 64];
                let rotated = c_path(ROTATED_PATH, &mut rotated).ok_or(Errno::EINVAL)?;
                let _ = call(
                    number::RENAMEAT2,
                    [AT_FDCWD, log.as_ptr() as usize, AT_FDCWD, rotated.as_ptr() as usize, 0, 0],
                );
            }
        }
        let file = open(log, O_WRONLY | O_APPEND | O_CREAT, 0o640)?;
        let written = syscall::write_all(file, record);
        let synced = call(number::FSYNC, [file, 0, 0, 0, 0, 0]);
        close(file);
        match (written, synced) {
            (true, Ok(_)) => Ok(()),
            (false, _) => Err(Errno::EIO),
            (true, Err(error)) => Err(error),
        }
    }

    fn now(&mut self) -> u64 {
        let mut time = [0i64; 2];
        let _ = call(
            number::CLOCK_GETTIME,
            [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        );
        time[0].max(0) as u64
    }

    fn report(&mut self, message: &[u8]) {
        say(&[message, b"\n"]);
    }
}
