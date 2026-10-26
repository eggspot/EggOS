use crate::path::{Files, Kind, PATH_CAPACITY};
use crate::shell::{Errno, MAX_POSITIONAL, Read, Shell, System};
use crate::status::USAGE;
use crate::syntax::MAX_WORDS;
use crate::variables::MAX_VARIABLES;
use core::cell::UnsafeCell;
use core::ffi::CStr;
use core::ptr;
use core::sync::atomic::{AtomicIsize, Ordering};
use eggos_runtime::process::{self, CLONE_VFORK, CLONE_VM};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

const TERMINAL: usize = 0;
const STANDARD_ERROR: usize = 2;
const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY_CLOEXEC: usize = 0o2_000_000;
const EINTR: isize = 4;
const ENOENT: isize = 2;
const SIGINT: usize = 2;
const SIGCHLD: usize = 17;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
#[cfg(target_arch = "x86_64")]
const SA_RESTORER: usize = 0x0400_0000;
const SIGNAL_SET_SIZE: usize = 8;
const JOB_CONTROL_SIGNALS: [usize; 4] = [3, 20, 21, 22];
const SIG_BLOCK: usize = 0;
const SIG_SETMASK: usize = 2;
const INTERRUPT_MASK: u64 = 1 << (SIGINT - 1);
const POLLIN: i16 = 1;
const TCGETS: usize = 0x5401;
const TIOCSPGRP: usize = 0x5410;
const TERMIOS_SIZE: usize = 64;
const STAT_SIZE: usize = 144;
const MODE_OFFSET: usize = if cfg!(target_arch = "x86_64") { 24 } else { 16 };
const FILE_TYPE: u32 = 0o170_000;
const DIRECTORY: u32 = 0o040_000;
const REGULAR_FILE: u32 = 0o100_000;
const ANY_EXECUTE: u32 = 0o111;
const CHILD_STACK_SIZE: usize = 16 * 1024;
const COMMAND_NOT_RUN: i32 = 127;
const NOT_EXECUTABLE: i32 = 126;

#[cfg(not(target_arch = "riscv64"))]
#[repr(C)]
struct SignalAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: u64,
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
struct SignalAction {
    handler: usize,
    flags: usize,
    mask: u64,
}

impl SignalAction {
    fn with_handler(handler: usize) -> Self {
        #[cfg(target_arch = "x86_64")]
        let (flags, restorer) = match handler {
            SIG_DFL | SIG_IGN => (0, 0),
            _ => (
                SA_RESTORER,
                process::restore_after_signal as *const () as usize,
            ),
        };
        #[cfg(target_arch = "aarch64")]
        let (flags, restorer) = (0, 0);
        #[cfg(target_arch = "riscv64")]
        let flags = 0;
        Self {
            handler,
            flags,
            #[cfg(not(target_arch = "riscv64"))]
            restorer,
            mask: 0,
        }
    }
}

#[repr(C, align(16))]
struct ChildStack(UnsafeCell<[u8; CHILD_STACK_SIZE]>);

// SAFETY: only a child started by `Linux::run` uses the stack, while the shell's only thread
// sleeps in its `CLONE_VFORK` clone, and one child at a time.
unsafe impl Sync for ChildStack {}

static CHILD_STACK: ChildStack = ChildStack(UnsafeCell::new([0; CHILD_STACK_SIZE]));

#[repr(C)]
struct PollDescriptor {
    descriptor: i32,
    events: i16,
    returned_events: i16,
}

struct Request {
    program: *const u8,
    arguments: *const *const u8,
    environment: *const *const u8,
    job_control: bool,
    error: AtomicIsize,
}

fn call(number: usize, arguments: [usize; 6]) -> isize {
    // SAFETY: every call the shell makes passes only memory it owns for the call's length,
    // and none maps, unmaps or protects memory.
    unsafe { syscall::call(number, arguments) }
}

fn set_signal_action(signal: usize, handler: usize) {
    let action = SignalAction::with_handler(handler);
    call(
        number::RT_SIGACTION,
        [signal, &raw const action as usize, 0, SIGNAL_SET_SIZE, 0, 0],
    );
}

extern "C" fn interrupted(_: i32) {}

fn set_foreground(group: i32) {
    call(
        number::IOCTL,
        [TERMINAL, TIOCSPGRP, &raw const group as usize, 0, 0, 0],
    );
}

fn is_terminal(descriptor: usize) -> bool {
    let mut settings = [0u8; TERMIOS_SIZE];
    call(
        number::IOCTL,
        [descriptor, TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    ) == 0
}

fn set_signal_mask(how: usize, mask: u64) {
    call(
        number::RT_SIGPROCMASK,
        [how, &raw const mask as usize, 0, SIGNAL_SET_SIZE, 0, 0],
    );
}

fn wait_for_input_or_interrupt(descriptor: usize) -> bool {
    let mut poll = PollDescriptor {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned_events: 0,
    };
    let nothing_blocked: u64 = 0;
    call(
        number::PPOLL,
        [
            &raw mut poll as usize,
            1,
            0,
            &raw const nothing_blocked as usize,
            SIGNAL_SET_SIZE,
            0,
        ],
    ) != -EINTR
}

fn take_the_terminal() -> i32 {
    set_signal_mask(SIG_BLOCK, INTERRUPT_MASK);
    set_signal_action(SIGINT, interrupted as *const () as usize);
    for signal in JOB_CONTROL_SIGNALS {
        set_signal_action(signal, SIG_IGN);
    }
    let pid = call(number::GETPID, [0; 6]);
    if call(number::GETPGID, [0; 6]) != pid {
        call(number::SETPGID, [0; 6]);
    }
    let group = call(number::GETPGID, [0; 6]) as i32;
    set_foreground(group);
    group
}

extern "C" fn start_child(request: usize) -> ! {
    // SAFETY: `Linux::run` passes its `Request`, which stays as it is while this child runs
    // on the shell's memory: the shell's thread sleeps until the child runs `execve` or ends.
    let request = unsafe { &*(request as *const Request) };
    if request.job_control {
        call(number::SETPGID, [0; 6]);
        set_foreground(call(number::GETPID, [0; 6]) as i32);
        set_signal_action(SIGINT, SIG_DFL);
        for signal in JOB_CONTROL_SIGNALS {
            set_signal_action(signal, SIG_DFL);
        }
        set_signal_mask(SIG_SETMASK, 0);
    }
    let result = call(
        number::EXECVE,
        [
            request.program as usize,
            request.arguments as usize,
            request.environment as usize,
            0,
            0,
            0,
        ],
    );
    request.error.store(-result, Ordering::Release);
    syscall::exit(if result == -ENOENT {
        COMMAND_NOT_RUN
    } else {
        NOT_EXECUTABLE
    })
}

fn wait_for(child: isize) -> Result<i32, Errno> {
    loop {
        let mut status = 0i32;
        let ended = call(
            number::WAIT4,
            [child as usize, &raw mut status as usize, 0, 0, 0, 0],
        );
        match ended {
            _ if ended == child => return Ok(status),
            _ if ended == -EINTR => {}
            _ => return Err(Errno(ended.unsigned_abs() as u16)),
        }
    }
}

#[derive(Debug)]
struct Linux {
    group: i32,
}

impl Files for Linux {
    fn kind(&mut self, path: &[u8]) -> Kind {
        let mut status = [0u8; STAT_SIZE];
        let result = call(
            number::NEWFSTATAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                status.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        );
        if result < 0 {
            return Kind::Missing;
        }
        let mut mode = [0; 4];
        mode.copy_from_slice(&status[MODE_OFFSET..MODE_OFFSET + 4]);
        let mode = u32::from_ne_bytes(mode);
        match mode & FILE_TYPE {
            DIRECTORY => Kind::Directory,
            REGULAR_FILE if mode & ANY_EXECUTE != 0 => Kind::Executable,
            _ => Kind::NotExecutable,
        }
    }
}

impl System for Linux {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) {
        syscall::write_all(descriptor, bytes);
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read {
        if descriptor == TERMINAL && self.group != 0 && !wait_for_input_or_interrupt(descriptor) {
            return Read::Interrupted;
        }
        let read = call(
            number::READ,
            [
                descriptor,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            ],
        );
        match read {
            1.. => Read::Bytes(read as usize),
            _ if read == -EINTR => Read::Interrupted,
            _ => Read::End,
        }
    }

    fn open(&mut self, path: &[u8]) -> Result<usize, Errno> {
        let descriptor = call(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, O_RDONLY_CLOEXEC, 0, 0, 0],
        );
        match descriptor {
            0.. => Ok(descriptor as usize),
            _ => Err(Errno(descriptor.unsigned_abs() as u16)),
        }
    }

    fn close(&mut self, descriptor: usize) {
        call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
    }

    fn change_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        match call(number::CHDIR, [path.as_ptr() as usize, 0, 0, 0, 0, 0]) {
            0 => Ok(()),
            error => Err(Errno(error.unsigned_abs() as u16)),
        }
    }

    fn working_directory<'b>(&mut self, buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]> {
        let result = call(
            number::GETCWD,
            [buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0, 0],
        );
        if result <= 0 {
            return None;
        }
        let end = buffer.iter().position(|&byte| byte == 0)?;
        Some(&buffer[..end])
    }

    fn run(
        &mut self,
        program: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        job_control: bool,
    ) -> Result<i32, Errno> {
        let mut argument_pointers = [ptr::null::<u8>(); MAX_WORDS + 1];
        for (slot, argument) in argument_pointers.iter_mut().zip(arguments) {
            *slot = argument.as_ptr();
        }
        let mut environment_pointers = [ptr::null::<u8>(); MAX_VARIABLES + MAX_WORDS + 1];
        for (slot, variable) in environment_pointers.iter_mut().zip(environment) {
            *slot = variable.as_ptr();
        }
        let request = Request {
            program: program.as_ptr(),
            arguments: argument_pointers.as_ptr(),
            environment: environment_pointers.as_ptr(),
            job_control,
            error: AtomicIsize::new(0),
        };
        let stack_top = CHILD_STACK.0.get() as usize + CHILD_STACK_SIZE;
        // SAFETY: the child runs `start_child` on `CHILD_STACK`, which nothing else uses, and
        // only reads `request` and the strings it names and writes its `error`, all of which
        // stay in place: this thread sleeps until the child runs `execve` or ends.
        let child = unsafe {
            process::clone_onto_stack(
                CLONE_VM | CLONE_VFORK | SIGCHLD,
                stack_top,
                start_child,
                &raw const request as usize,
            )
        };
        if child < 0 {
            return Err(Errno(child.unsigned_abs() as u16));
        }
        if job_control {
            call(
                number::SETPGID,
                [child as usize, child as usize, 0, 0, 0, 0],
            );
            set_foreground(child as i32);
        }
        let status = wait_for(child);
        if job_control {
            set_foreground(self.group);
        }
        match request.error.load(Ordering::Acquire) {
            0 => status,
            error => Err(Errno(error as u16)),
        }
    }
}

/// The shell's main function: see the crate's documentation.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = startup.arguments();
    let name = arguments.next().map_or(&b"eggsh"[..], CStr::to_bytes);
    let mut positional: [&[u8]; MAX_POSITIONAL] = [b""; MAX_POSITIONAL];
    let first = arguments.next();
    let mut text = None;
    let mut file = None;
    positional[0] = match first.map(CStr::to_bytes) {
        Some(b"-c") => {
            let Some(command) = arguments.next() else {
                syscall::write_all(STANDARD_ERROR, b"eggsh: -c: option requires an argument\n");
                return USAGE;
            };
            text = Some(command.to_bytes());
            arguments.next().map_or(name, CStr::to_bytes)
        }
        Some(path) => {
            file = first;
            path
        }
        None => name,
    };
    let mut count = 1;
    for argument in arguments {
        if count < MAX_POSITIONAL {
            positional[count] = argument.to_bytes();
        }
        count += 1;
    }
    let interactive = text.is_none() && file.is_none() && is_terminal(0) && is_terminal(2);
    let group = if interactive { take_the_terminal() } else { 0 };
    let pid = call(number::GETPID, [0; 6]) as u32;
    let root = call(number::GETEUID, [0; 6]) == 0;
    let kept = count.min(MAX_POSITIONAL);
    let mut shell = Shell::new(
        Linux { group },
        pid,
        &positional[..kept],
        count,
        interactive,
        root,
    );
    for variable in startup.environment() {
        shell.import(variable.to_bytes());
    }
    shell.set_working_directory_variable();
    if let Some(text) = text {
        shell.run_text(text);
    } else if let Some(file) = file {
        let descriptor = match shell.system.open(file.to_bytes_with_nul()) {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                for part in [
                    &b"eggsh: "[..],
                    file.to_bytes(),
                    b": ",
                    errno.message(),
                    b"\n",
                ] {
                    syscall::write_all(STANDARD_ERROR, part);
                }
                return COMMAND_NOT_RUN;
            }
        };
        shell.run_descriptor(descriptor, false);
    } else {
        shell.run_descriptor(TERMINAL, interactive);
    }
    shell.exit_status()
}
