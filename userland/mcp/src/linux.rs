//! `eggmcpd`'s system calls: the agent channel's loop, the grants from `/proc/cmdline` and
//! the agents' grants file, running a CLI twin with its input given and its output captured,
//! in a process group of its own that is stopped whole, the checks of what is at a path,
//! `wm_capture`'s file in `/run`, the audit log on `/var/log` and the clocks.

use crate::arguments::{MAX_CONTENT_BYTES, WORDS_BYTES};
use crate::audit::{GENERATIONS, LOG_FOLDER, LOG_PATH, RECORD_BYTES, ROTATE_BYTES, ROTATED_PATHS};
use crate::framing::{Line, LineReader};
use crate::grant_prompt::{ANSWER_BYTES, PROMPT_OPTION, PROMPT_PROGRAM, PromptAnswer};
use crate::grants::{self, COMMAND_LINE_PATH, ChannelAgent, Grant, Grants, STORE_PATH};
use crate::paths::{HOME, MAX_PATH_BYTES};
use crate::server::{
    ARGUMENTS_BYTES, ERROR_OUTPUT_LIMIT, OUTPUT_LIMIT, RESPONSE_BYTES, Scratch, Server,
};
use crate::system::{
    CaptureFile, Ended, FileState, Invocation, MAX_COMMAND_WORDS, RunOutcome, System,
};
use core::ffi::CStr;
use eggos_commands::tools::NO_SYMLINKS_ENVIRONMENT;
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{ChildStack, Spawn, spawn};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

pub mod network;

/// The longest message taken, in bytes; a longer line is dropped and answered with an error.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// Where every command starts.
pub const WORKING_DIRECTORY: &CStr = c"/home";
/// How long the server waits for a stopped command's end before it goes on without it.
pub const REAP_GRACE_MS: u64 = 1000;

const CHANNEL: &CStr = c"/dev/agent";
const WINDOW_DEVICE: &CStr = c"/dev/window";
const RESTARTED: &[u8] = b"--restarted";
const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY: usize = 0;
const O_RDWR: usize = 2;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0o100;
const O_EXCL: usize = 0o200;
#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: usize = 0o100_000;
#[cfg(not(target_arch = "aarch64"))]
const O_NOFOLLOW: usize = 0o400_000;
const CAPTURE_MODE: usize = 0o600;
const AT_EMPTY_PATH: usize = 0x1000;
const O_APPEND: usize = 0o2000;
const O_NONBLOCK: usize = 0o4000;
const O_CLOEXEC: usize = 0o2_000_000;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const STATX_TYPE: usize = 1;
const STATX_SIZE: usize = 0x200;
const STATX_BYTES: usize = 256;
const STATX_MODE: usize = 28;
const STATX_SIZE_OFFSET: usize = 40;
const S_IFMT: u16 = 0o170_000;
const S_IFLNK: u16 = 0o120_000;
const S_IFREG: u16 = 0o100_000;
const S_IFDIR: u16 = 0o040_000;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLHUP: i16 = 0x10;
const POLLERR: i16 = 8;
const SIGKILL: usize = 9;
const WNOHANG: usize = 1;
const F_SETFL: usize = 4;
const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;
const CONSOLE: usize = 2;
const ENVIRONMENT: [&CStr; 3] = [c"PATH=/bin", c"HOME=/home/root", c"TERM=dumb"];
const MAX_WORD_BYTES: usize = WORDS_BYTES + MAX_PATH_BYTES + 64;
const RETRY_PAUSE_MS: u64 = 100;
const REAP_PAUSE_MS: u64 = 10;
const COMMAND_LINE_BYTES: usize = 1024;
const SIGPIPE: usize = 13;
const SIG_IGN: u64 = 1;
const SIGNAL_SET_BYTES: usize = 8;
const HOME_MODE: usize = 0o755;

struct Buffers {
    input: [u8; MAX_MESSAGE_BYTES],
    response: [u8; RESPONSE_BYTES],
    stdout: [u8; OUTPUT_LIMIT],
    stderr: [u8; ERROR_OUTPUT_LIMIT],
    decoded: [u8; MAX_PATH_BYTES + 1],
    resolved: [u8; MAX_PATH_BYTES + 1],
    record: [u8; RECORD_BYTES],
    arguments: [u8; ARGUMENTS_BYTES],
    content: [u8; MAX_CONTENT_BYTES],
    words: [u8; WORDS_BYTES],
    held_path: [u8; MAX_PATH_BYTES + 1],
    held_arguments: [u8; ARGUMENTS_BYTES],
    command_words: [u8; MAX_WORD_BYTES],
    child_stack: ChildStack,
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
    content: [0; MAX_CONTENT_BYTES],
    words: [0; WORDS_BYTES],
    held_path: [0; MAX_PATH_BYTES + 1],
    held_arguments: [0; ARGUMENTS_BYTES],
    command_words: [0; MAX_WORD_BYTES],
    child_stack: ChildStack::new(),
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
        [
            AT_FDCWD,
            path.as_ptr() as usize,
            flags | O_CLOEXEC,
            mode,
            0,
            0,
        ],
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

fn read_grants(server: &mut Server) {
    let mut path = [0u8; 32];
    let Some(path) = c_path(COMMAND_LINE_PATH, &mut path) else {
        return;
    };
    let Ok(file) = open(path, O_RDONLY, 0) else {
        return;
    };
    let mut line = [0u8; COMMAND_LINE_BYTES];
    let mut length = 0;
    while let Some(free) = line.get_mut(length..).filter(|free| !free.is_empty()) {
        match call(
            number::READ,
            [file, free.as_mut_ptr() as usize, free.len(), 0, 0, 0],
        ) {
            Ok(0) => break,
            Ok(count) => length += count,
            Err(Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
    close(file);
    let words = line.get(..length).unwrap_or_default();
    let (grants, unknown) = grants::from_command_line(words);
    match grants::agent_from_command_line(words) {
        ChannelAgent::Local => server.set_grants(grants, unknown),
        ChannelAgent::Pinned(agent) => {
            server.set_grants(grants, unknown);
            server.set_agent(agent);
        }
        ChannelAgent::Invalid => server.set_grants(Grants::NONE, unknown),
    }
}

enum Woke {
    Channel,
    Prompt,
    Timeout,
}

fn wait_for_channel(channel: usize, prompt: Option<usize>, deadline_ms: u64) -> Woke {
    let left = deadline_ms.saturating_sub(now_ms());
    if left == 0 {
        return Woke::Timeout;
    }
    let mut watched = [channel, prompt.unwrap_or(channel)].map(|descriptor| PollDescriptor {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned_events: 0,
    });
    let count = match prompt {
        Some(_) => 2,
        None => 1,
    };
    let timeout = [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64];
    let polled = call(
        number::PPOLL,
        [
            watched.as_mut_ptr() as usize,
            count,
            timeout.as_ptr() as usize,
            0,
            8,
            0,
        ],
    );
    let prompt_ready = prompt.is_some()
        && watched
            .get(1)
            .is_some_and(|ready| ready.returned_events != 0);
    match polled {
        Ok(0) => Woke::Timeout,
        Ok(_) if prompt_ready => Woke::Prompt,
        Ok(_) | Err(_) => Woke::Channel,
    }
}

fn ignore_broken_pipes() {
    let action = [SIG_IGN, 0, 0, 0];
    let _ = call(
        number::RT_SIGACTION,
        [SIGPIPE, action.as_ptr() as usize, 0, SIGNAL_SET_BYTES, 0, 0],
    );
}

fn send(channel: usize, answer: &[u8]) {
    if !answer.is_empty() && !syscall::write_all(channel, answer) {
        say(&[b"eggmcpd: an answer could not be sent\n"]);
    }
}

/// Opens the channel, reads the grants, records the start and answers messages until the
/// machine stops.
pub fn run(startup: &Startup) -> i32 {
    if startup
        .arguments()
        .any(|argument| argument.to_bytes() == network::NETWORK_OPTION)
    {
        return network::run();
    }
    let restarted = startup
        .arguments()
        .any(|argument| argument.to_bytes() == RESTARTED);
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
    let Some(buffers) = (unsafe { (&raw mut BUFFERS).as_mut() }) else {
        return 1;
    };
    let Buffers {
        input,
        response,
        stdout,
        stderr,
        decoded,
        resolved,
        record,
        arguments,
        content,
        words,
        held_path,
        held_arguments,
        command_words,
        child_stack,
    } = buffers;
    let mut scratch = Scratch {
        stdout,
        stderr,
        decoded,
        resolved,
        record,
        arguments,
        content,
        words,
        held_path,
        held_arguments,
    };
    let mut system = Linux {
        child_stack,
        command_words,
        prompt: None,
    };
    let mut server = match restarted {
        true => Server::restarted(),
        false => Server::new(),
    };
    read_grants(&mut server);
    if server.grants().allows(Grant::Write) {
        let mut home = [0u8; 16];
        if let Some(home) = c_path(HOME, &mut home) {
            let _ = call(
                number::MKDIRAT,
                [AT_FDCWD, home.as_ptr() as usize, HOME_MODE, 0, 0, 0],
            );
        }
    }
    ignore_broken_pipes();
    server.start(restarted, &mut system, &mut scratch);
    let mut reader = LineReader::new(input);
    loop {
        if let Some(deadline) = server.waiting_until() {
            let length = match wait_for_channel(channel, system.prompt_answer(), deadline) {
                Woke::Channel => None,
                Woke::Prompt => {
                    let answer = system.read_prompt_answer();
                    Some(server.prompt_answered(answer, &mut system, &mut scratch, response))
                }
                Woke::Timeout => Some(server.expire(now_ms(), &mut system, &mut scratch, response)),
            };
            if let Some(length) = length {
                send(channel, response.get(..length).unwrap_or_default());
                continue;
            }
        }
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
                say(&[
                    b"eggmcpd: reading /dev/agent failed: ",
                    error.message(),
                    b"\n",
                ]);
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
            send(channel, response.get(..length).unwrap_or_default());
        }
    }
}

struct Linux {
    child_stack: &'static mut ChildStack,
    command_words: &'static mut [u8; MAX_WORD_BYTES],
    prompt: Option<Prompt>,
}

struct Prompt {
    child: usize,
    answer: usize,
}

impl Linux {
    fn prompt_answer(&self) -> Option<usize> {
        self.prompt.as_ref().map(|prompt| prompt.answer)
    }

    fn read_prompt_answer(&self) -> Option<PromptAnswer> {
        let prompt = self.prompt.as_ref()?;
        let mut line = [0u8; ANSWER_BYTES];
        let read = loop {
            match call(
                number::READ,
                [
                    prompt.answer,
                    line.as_mut_ptr() as usize,
                    line.len(),
                    0,
                    0,
                    0,
                ],
            ) {
                Err(Errno::EINTR) => continue,
                other => break other,
            }
        };
        PromptAnswer::from_line(line.get(..read.ok()?)?)
    }
}

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

fn statx_open(descriptor: usize, mask: usize) -> Result<[u8; STATX_BYTES], Errno> {
    let mut status = [0u8; STATX_BYTES];
    call(
        number::STATX,
        [
            descriptor,
            c"".as_ptr() as usize,
            AT_EMPTY_PATH,
            mask,
            status.as_mut_ptr() as usize,
            0,
        ],
    )?;
    Ok(status)
}

fn mode_of(status: &[u8; STATX_BYTES]) -> u16 {
    u16::from_ne_bytes([status[STATX_MODE], status[STATX_MODE + 1]])
}

fn size_of(status: &[u8; STATX_BYTES]) -> u64 {
    let mut size = [0u8; 8];
    size.copy_from_slice(&status[STATX_SIZE_OFFSET..STATX_SIZE_OFFSET + 8]);
    u64::from_ne_bytes(size)
}

fn c_path<'b>(path: &str, buffer: &'b mut [u8]) -> Option<&'b CStr> {
    let place = buffer.get_mut(..path.len() + 1)?;
    place[..path.len()].copy_from_slice(path.as_bytes());
    place[path.len()] = 0;
    CStr::from_bytes_with_nul(place).ok()
}

fn status_of(status: i32) -> Ended {
    let signal = status & 0x7f;
    match signal {
        0 => Ended::Exited((status >> 8) & 0xff),
        _ => Ended::Signalled(signal),
    }
}

fn wait_until(child: usize, deadline_ms: u64) -> Option<Ended> {
    loop {
        let mut status = 0i32;
        match call(
            number::WAIT4,
            [child, &raw mut status as usize, WNOHANG, 0, 0, 0],
        ) {
            Ok(0) => {}
            Ok(_) => return Some(status_of(status)),
            Err(Errno::EINTR) => continue,
            Err(error) => return Some(Ended::NotStarted(error)),
        }
        if now_ms() >= deadline_ms {
            return None;
        }
        pause(REAP_PAUSE_MS);
    }
}

fn stop_group(child: usize) -> Result<(), Errno> {
    let group = (child as isize).wrapping_neg() as usize;
    match call(number::KILL, [group, SIGKILL, 0, 0, 0, 0]) {
        Ok(_) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error),
    }
}

struct Capture<'b> {
    descriptor: usize,
    buffer: &'b mut [u8],
    kept: usize,
    open: bool,
    overflowed: bool,
    discard: bool,
}

impl Capture<'_> {
    fn take(&mut self) {
        let mut spill = [0u8; 512];
        let room = match self.discard {
            true => 0,
            false => self.buffer.len() - self.kept,
        };
        let (target, length) = match room {
            0 => (spill.as_mut_ptr(), spill.len()),
            _ => (self.buffer[self.kept..].as_mut_ptr(), room),
        };
        match call(
            number::READ,
            [self.descriptor, target as usize, length, 0, 0, 0],
        ) {
            Ok(0) | Err(_) => self.open = false,
            Ok(_) if self.discard => {}
            Ok(count) if room == 0 => self.overflowed |= count > 0,
            Ok(count) => self.kept += count,
        }
    }
}

struct Feed<'b> {
    descriptor: Option<usize>,
    bytes: &'b [u8],
    taken: usize,
}

impl Feed<'_> {
    fn give(&mut self) {
        let Some(descriptor) = self.descriptor else {
            return;
        };
        let rest = self.bytes.get(self.taken..).unwrap_or_default();
        let result = match rest.is_empty() {
            true => Err(Errno::EPIPE),
            false => call(
                number::WRITE,
                [descriptor, rest.as_ptr() as usize, rest.len(), 0, 0, 0],
            ),
        };
        match result {
            Ok(count) => self.taken += count,
            Err(Errno::EAGAIN | Errno::EINTR) => {}
            Err(_) => self.finish(),
        }
        if self.taken >= self.bytes.len() {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if let Some(descriptor) = self.descriptor.take() {
            close(descriptor);
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
        let mut pointers = [core::ptr::null::<u8>(); MAX_COMMAND_WORDS + 4];
        let mut count = 0;
        let mut used = 0;
        let storage = &mut self.command_words[..];
        let mut push = |word: &str, count: &mut usize, used: &mut usize| -> Result<(), Errno> {
            let place = storage
                .get_mut(*used..*used + word.len() + 1)
                .ok_or(Errno::E2BIG)?;
            place[..word.len()].copy_from_slice(word.as_bytes());
            place[word.len()] = 0;
            let slot = pointers.get_mut(*count).ok_or(Errno::E2BIG)?;
            *slot = place.as_ptr();
            *count += 1;
            *used += word.len() + 1;
            Ok(())
        };
        if invocation.program.contains('\0')
            || invocation.words.iter().any(|word| word.contains('\0'))
        {
            return Err(Errno::EINVAL);
        }
        push(invocation.program, &mut count, &mut used)?;
        for word in invocation.words {
            push(word, &mut count, &mut used)?;
        }
        if let Some(operand) = invocation.operand {
            if operand.contains('\0') {
                return Err(Errno::EINVAL);
            }
            push("--", &mut count, &mut used)?;
            push(operand, &mut count, &mut used)?;
        }
        let program = CStr::from_bytes_until_nul(storage_prefix(
            &self.command_words[..],
            invocation.program.len() + 1,
        ))
        .map_err(|_| Errno::EINVAL)?;
        if count >= pointers.len() {
            return Err(Errno::E2BIG);
        }
        pointers[count] = core::ptr::null();
        let mut environment = [core::ptr::null::<u8>(); ENVIRONMENT.len() + 2];
        for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
            *slot = variable.as_ptr().cast();
        }
        if invocation.no_symlinks {
            environment[ENVIRONMENT.len()] = NO_SYMLINKS_ENVIRONMENT.as_ptr().cast();
        }
        let input = pipe()?;
        let output = pipe().inspect_err(|_| {
            close(input[0]);
            close(input[1]);
        })?;
        let errors = pipe().inspect_err(|_| {
            close(input[0]);
            close(input[1]);
            close(output[0]);
            close(output[1]);
        })?;
        let request = Spawn::new(
            program,
            &pointers[..=count],
            &environment,
            [input[0], output[1], errors[1]],
        )
        .in_directory(WORKING_DIRECTORY)
        .in_own_process_group();
        let started = spawn(self.child_stack, &request);
        for descriptor in [input[0], output[1], errors[1]] {
            close(descriptor);
        }
        let child = match started {
            Ok(child) => child,
            Err(error) => {
                close(input[1]);
                close(output[0]);
                close(errors[0]);
                return Err(error);
            }
        };
        let mut feed = Feed {
            descriptor: Some(input[1]),
            bytes: invocation.stdin.unwrap_or_default(),
            taken: 0,
        };
        match feed.bytes.is_empty() {
            true => feed.finish(),
            false => {
                let _ = call(number::FCNTL, [input[1], F_SETFL, O_NONBLOCK, 0, 0, 0]);
            }
        }
        let mut out = Capture {
            descriptor: output[0],
            buffer: stdout,
            kept: 0,
            open: true,
            overflowed: false,
            discard: invocation.discard_output,
        };
        let mut err = Capture {
            descriptor: errors[0],
            buffer: stderr,
            kept: 0,
            open: true,
            overflowed: false,
            discard: false,
        };
        let deadline = now_ms() + invocation.time_limit_ms;
        let mut timed_out = false;
        while out.open || err.open || feed.descriptor.is_some() {
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
                PollDescriptor {
                    descriptor: feed.descriptor.map_or(-1, |descriptor| descriptor as i32),
                    events: POLLOUT,
                    returned_events: 0,
                },
            ];
            let timeout = [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64];
            match call(
                number::PPOLL,
                [
                    watched.as_mut_ptr() as usize,
                    3,
                    timeout.as_ptr() as usize,
                    0,
                    8,
                    0,
                ],
            ) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(_) => break,
            }
            let ready =
                |entry: &PollDescriptor| entry.returned_events & (POLLIN | POLLHUP | POLLERR) != 0;
            if ready(&watched[0]) {
                out.take();
            }
            if ready(&watched[1]) {
                err.take();
            }
            if watched[2].returned_events & (POLLOUT | POLLHUP | POLLERR) != 0 {
                feed.give();
            }
        }
        feed.finish();
        let mut stop_failed = None;
        let mut ended = None;
        if !(timed_out || out.overflowed) {
            ended = wait_until(child, deadline);
            timed_out = ended.is_none();
        }
        if ended.is_none() {
            stop_failed = stop_group(child).err();
            ended = wait_until(child, now_ms() + REAP_GRACE_MS);
            if ended.is_none() && stop_failed.is_none() {
                stop_failed = Some(Errno::EBUSY);
            }
        }
        close(output[0]);
        close(errors[0]);
        let ended = match (timed_out, ended) {
            (true, _) => Ended::TimedOut,
            (false, Some(ended)) => ended,
            (false, None) => Ended::TimedOut,
        };
        Ok(RunOutcome {
            ended,
            stdout: out.kept,
            stdout_cut: out.overflowed,
            stderr: err.kept,
            stdin_taken: feed.taken,
            stop_failed,
        })
    }

    fn rotate(&self, log: &CStr) -> Result<(), Errno> {
        let mut from = [0u8; 64];
        let mut to = [0u8; 64];
        for generation in (1..GENERATIONS).rev() {
            let (Some(older), Some(newer)) = (
                ROTATED_PATHS.get(generation),
                ROTATED_PATHS.get(generation - 1),
            ) else {
                continue;
            };
            let (Some(to), Some(from)) = (c_path(older, &mut to), c_path(newer, &mut from)) else {
                return Err(Errno::EINVAL);
            };
            rename(from, to)?;
        }
        let first = ROTATED_PATHS.first().ok_or(Errno::EINVAL)?;
        let first = c_path(first, &mut to).ok_or(Errno::EINVAL)?;
        rename(log, first)
    }
}

fn rename(from: &CStr, to: &CStr) -> Result<(), Errno> {
    match call(
        number::RENAMEAT2,
        [
            AT_FDCWD,
            from.as_ptr() as usize,
            AT_FDCWD,
            to.as_ptr() as usize,
            0,
            0,
        ],
    ) {
        Ok(_) | Err(Errno::ENOENT) => Ok(()),
        Err(error) => Err(error),
    }
}

fn storage_prefix(storage: &[u8], length: usize) -> &[u8] {
    storage.get(..length).unwrap_or_default()
}

impl System for Linux {
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> RunOutcome {
        self.run_captured(invocation, stdout, stderr)
            .unwrap_or_else(RunOutcome::not_started)
    }

    fn is_link(&mut self, path: &str) -> bool {
        let mut buffer = [0u8; MAX_PATH_BYTES + 1];
        let Some(path) = c_path(path, &mut buffer) else {
            return false;
        };
        statx(path, STATX_TYPE, AT_SYMLINK_NOFOLLOW)
            .is_ok_and(|status| mode_of(&status) & S_IFMT == S_IFLNK)
    }

    fn file_state(&mut self, path: &str) -> FileState {
        let mut buffer = [0u8; MAX_PATH_BYTES + 1];
        let Some(path) = c_path(path, &mut buffer) else {
            return FileState::Missing;
        };
        match statx(path, STATX_TYPE | STATX_SIZE, AT_SYMLINK_NOFOLLOW) {
            Err(_) => FileState::Missing,
            Ok(status) => match mode_of(&status) & S_IFMT {
                S_IFREG => FileState::File(size_of(&status)),
                S_IFDIR => FileState::Directory,
                _ => FileState::Other,
            },
        }
    }

    fn remove_capture(&mut self, path: &str) {
        let mut buffer = [0u8; 64];
        if let Some(path) = c_path(path, &mut buffer) {
            let _ = call(
                number::UNLINKAT,
                [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0],
            );
        }
    }

    fn create_capture(&mut self, path: &str) -> Result<(), Errno> {
        let mut buffer = [0u8; 64];
        let path = c_path(path, &mut buffer).ok_or(Errno::EINVAL)?;
        let file = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, CAPTURE_MODE)?;
        close(file);
        Ok(())
    }

    fn open_capture(&mut self, path: &str) -> Result<CaptureFile, Errno> {
        let mut buffer = [0u8; 64];
        let path = c_path(path, &mut buffer).ok_or(Errno::EINVAL)?;
        let file = open(path, O_RDONLY | O_NOFOLLOW, 0)?;
        let status = match statx_open(file, STATX_TYPE | STATX_SIZE) {
            Ok(status) if mode_of(&status) & S_IFMT == S_IFREG => status,
            Ok(_) => {
                close(file);
                return Err(Errno::EINVAL);
            }
            Err(error) => {
                close(file);
                return Err(error);
            }
        };
        Ok(CaptureFile {
            descriptor: file,
            size: size_of(&status),
        })
    }

    fn read_capture(&mut self, file: &CaptureFile, out: &mut [u8]) -> Result<usize, Errno> {
        loop {
            match call(
                number::READ,
                [
                    file.descriptor,
                    out.as_mut_ptr() as usize,
                    out.len(),
                    0,
                    0,
                    0,
                ],
            ) {
                Err(Errno::EINTR) => continue,
                result => return result,
            }
        }
    }

    fn close_capture(&mut self, file: CaptureFile) {
        close(file.descriptor);
    }

    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno> {
        let mut buffer = [0u8; 64];
        let log = c_path(LOG_PATH, &mut buffer).ok_or(Errno::EINVAL)?;
        let size = statx(log, STATX_SIZE, 0).map_or(0, |status| size_of(&status));
        if size + record.len() as u64 > ROTATE_BYTES {
            self.rotate(log)?;
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

    fn start_grant_prompt(&mut self, question: &[u8]) -> Result<(), Errno> {
        self.end_grant_prompt();
        close(open(WINDOW_DEVICE, O_RDWR | O_CLOEXEC, 0)?);
        let mut program = [0u8; 32];
        let program = c_path(PROMPT_PROGRAM, &mut program).ok_or(Errno::EINVAL)?;
        let mut option = [0u8; 32];
        let option = c_path(PROMPT_OPTION, &mut option).ok_or(Errno::EINVAL)?;
        let arguments = [
            program.as_ptr().cast::<u8>(),
            option.as_ptr().cast(),
            core::ptr::null(),
        ];
        let mut environment = [core::ptr::null::<u8>(); ENVIRONMENT.len() + 1];
        for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
            *slot = variable.as_ptr().cast();
        }
        let input = pipe()?;
        let output = pipe().inspect_err(|_| {
            close(input[0]);
            close(input[1]);
        })?;
        let request = Spawn::new(
            program,
            &arguments,
            &environment,
            [input[0], output[1], CONSOLE],
        )
        .in_directory(WORKING_DIRECTORY)
        .in_own_process_group();
        let started = spawn(self.child_stack, &request);
        close(input[0]);
        close(output[1]);
        let child = started.inspect_err(|_| {
            close(input[1]);
            close(output[0]);
        })?;
        let written = syscall::write_all(input[1], question);
        close(input[1]);
        self.prompt = Some(Prompt {
            child,
            answer: output[0],
        });
        if !written {
            self.end_grant_prompt();
            return Err(Errno::EPIPE);
        }
        Ok(())
    }

    fn end_grant_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        close(prompt.answer);
        if wait_until(prompt.child, now_ms()).is_none() {
            let _ = stop_group(prompt.child);
            let _ = wait_until(prompt.child, now_ms().saturating_add(REAP_GRACE_MS));
        }
    }

    fn read_grant_store(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        let mut buffer = [0u8; 64];
        let path = c_path(STORE_PATH, &mut buffer).ok_or(Errno::EINVAL)?;
        let file = open(path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK, 0)?;
        let mut length = 0;
        let mut result = Ok(());
        while let Some(free) = out.get_mut(length..).filter(|free| !free.is_empty()) {
            match call(
                number::READ,
                [file, free.as_mut_ptr() as usize, free.len(), 0, 0, 0],
            ) {
                Ok(0) => break,
                Ok(count) => length += count,
                Err(Errno::EINTR) => continue,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        close(file);
        result.map(|()| length)
    }

    fn now(&mut self) -> u64 {
        let mut time = [0i64; 2];
        let _ = call(
            number::CLOCK_GETTIME,
            [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
        );
        time[0].max(0) as u64
    }

    fn monotonic_ms(&mut self) -> u64 {
        now_ms()
    }

    fn report(&mut self, message: &[u8]) {
        say(&[message, b"\n"]);
    }
}
