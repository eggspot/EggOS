use crate::agent_server::{AGENT_CHANNEL, AGENT_SERVER, AfterEnd, AgentServer, RESTARTED_ARGUMENT};
use crate::backoff::Backoff;
use crate::config::{self, Command, Config, LoginExit, MAX_WORDS};
use crate::display_server::{DISPLAY_DEVICE, DISPLAY_SERVER};
use core::ptr;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

const BANNER: &[u8] = concat!("EggOS ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();

const CONFIG_PATH: &[u8] = b"/etc/eggos/init.conf\0";

const CONSOLE: usize = 1;
const AT_FDCWD: usize = -100isize as usize;
const HOME_DIRECTORY: &[u8] = b"/home/root\0";
const HOME_DIRECTORY_MODE: usize = 0o755;
const O_RDONLY_CLOEXEC: usize = 0o2_000_000;
const EINTR: isize = 4;
const ECHILD: isize = 10;
const WNOHANG: usize = 1;
const SIGCHLD: usize = 17;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
const SIGNAL_SET_SIZE: usize = 8;
const IGNORED_SIGNALS: [usize; 10] = [1, 2, 3, 10, 12, 13, 15, 20, 21, 22];
const TIOCSCTTY: usize = 0x540e;
const TIOCSPGRP: usize = 0x5410;
const STEAL_THE_CONSOLE: usize = 1;
const CLOCK_MONOTONIC: usize = 1;
const COMMAND_NOT_RUN: i32 = 127;
const CONFIG_CAPACITY: usize = 1024;

const REBOOT_MAGIC1: usize = 0xfee1_dead;
const REBOOT_MAGIC2: usize = 0x2812_1969;
const REBOOT_POWER_OFF: usize = 0x4321_fedc;
const REBOOT_RESTART: usize = 0x0123_4567;
const REBOOT_HALT: usize = 0xcdef_0123;

const ENVIRONMENT: [&[u8]; 5] = [
    b"HOME=/home/root\0",
    b"TERM=linux\0",
    b"PATH=/bin\0",
    b"XDG_RUNTIME_DIR=/run/user/0\0",
    b"WAYLAND_DISPLAY=wayland-0\0",
];

#[derive(Default)]
struct Services {
    agent: AgentServer,
    display: AgentServer,
}

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
        Self {
            handler,
            flags: 0,
            #[cfg(not(target_arch = "riscv64"))]
            restorer: 0,
            mask: 0,
        }
    }
}

#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

fn call(number: usize, arguments: [usize; 6]) -> isize {
    // SAFETY: every call init makes passes only memory it owns for the call's length, and none
    // maps, unmaps or protects memory.
    unsafe { syscall::call(number, arguments) }
}

fn say(parts: &[&[u8]]) {
    for part in parts {
        syscall::write_all(CONSOLE, part);
    }
}

fn decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut start = buffer.len();
    loop {
        start -= 1;
        buffer[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &buffer[start..];
        }
    }
}

pub fn run(_startup: &Startup) -> i32 {
    set_signal_actions(SIG_IGN);
    say(&[BANNER]);
    let mut services = Services::default();
    if exists(AGENT_CHANNEL) && exists(AGENT_SERVER) {
        start_agent_server(&mut services.agent, false);
    }
    if exists(DISPLAY_SERVER) && exists(DISPLAY_DEVICE) {
        start_display_server(&mut services.display);
    }
    call(
        number::MKDIRAT,
        [
            AT_FDCWD,
            HOME_DIRECTORY.as_ptr() as usize,
            HOME_DIRECTORY_MODE,
            0,
            0,
            0,
        ],
    );
    let config = read_config();
    let name = config.login.program_name();
    let mut backoff = Backoff::default();
    loop {
        if !exists(config.login.program()) {
            say(&[
                b"init: no login program: ",
                name,
                b" is missing; init only reaps children\n",
            ]);
            reap_forever(&mut services);
        }
        say(&[b"init: starting ", name, b"\n"]);
        let started = now_ms();
        match spawn(&config.login).and_then(|child| wait_for(child, &mut services)) {
            Some(status) => report(name, status),
            None => say(&[b"init: could not start ", name, b"\n"]),
        }
        if let Some(command) = reboot_command(config.login_exit) {
            call(
                number::REBOOT,
                [REBOOT_MAGIC1, REBOOT_MAGIC2, command, 0, 0, 0],
            );
            say(&[b"init: reboot failed; starting the login program again\n"]);
        }
        let delay = backoff.delay_after(now_ms().saturating_sub(started));
        if delay > 0 {
            let mut digits = [0; 20];
            say(&[
                b"init: starting ",
                name,
                b" again in ",
                decimal(delay, &mut digits),
                b" s\n",
            ]);
            sleep_reaping(delay, &mut services);
        }
    }
}

fn reboot_command(login_exit: LoginExit) -> Option<usize> {
    match login_exit {
        LoginExit::Respawn => None,
        LoginExit::PowerOff => Some(REBOOT_POWER_OFF),
        LoginExit::Restart => Some(REBOOT_RESTART),
        LoginExit::Halt => Some(REBOOT_HALT),
    }
}

fn set_signal_actions(handler: usize) {
    let action = SignalAction::with_handler(handler);
    for signal in IGNORED_SIGNALS {
        call(
            number::RT_SIGACTION,
            [signal, &raw const action as usize, 0, SIGNAL_SET_SIZE, 0, 0],
        );
    }
}

fn open(path: &[u8]) -> Option<usize> {
    let descriptor = call(
        number::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, O_RDONLY_CLOEXEC, 0, 0, 0],
    );
    (descriptor >= 0).then_some(descriptor as usize)
}

fn exists(path: &[u8]) -> bool {
    let Some(descriptor) = open(path) else {
        return false;
    };
    call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
    true
}

fn read_config() -> Config {
    let Some(descriptor) = open(CONFIG_PATH) else {
        return Config::default();
    };
    let mut text = [0u8; CONFIG_CAPACITY];
    let mut length = 0;
    while length < text.len() {
        let read = call(
            number::READ,
            [
                descriptor,
                text[length..].as_mut_ptr() as usize,
                text.len() - length,
                0,
                0,
                0,
            ],
        );
        if read <= 0 {
            break;
        }
        length += read as usize;
    }
    let mut more = [0u8; 1];
    let cut_short = length == text.len()
        && call(
            number::READ,
            [descriptor, more.as_mut_ptr() as usize, 1, 0, 0, 0],
        ) > 0;
    call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
    if !cut_short {
        return Config::parse(&text[..length]);
    }
    say(&[b"init: /etc/eggos/init.conf is too long; only its first whole lines are read\n"]);
    Config::parse(config::whole_lines(&text[..length]))
}

fn now_ms() -> u64 {
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    call(
        number::CLOCK_GETTIME,
        [CLOCK_MONOTONIC, &raw mut time as usize, 0, 0, 0, 0],
    );
    time.seconds as u64 * 1000 + time.nanoseconds as u64 / 1_000_000
}

fn spawn(command: &Command) -> Option<usize> {
    let mut arguments = [ptr::null::<u8>(); MAX_WORDS + 1];
    for (slot, word) in arguments.iter_mut().zip(command.words()) {
        *slot = word.as_ptr();
    }
    let mut environment = [ptr::null::<u8>(); ENVIRONMENT.len() + 1];
    for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
        *slot = variable.as_ptr();
    }
    let child = call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]);
    match child {
        0 => become_login(command, &arguments, &environment),
        1.. => Some(child as usize),
        _ => None,
    }
}

fn become_login(command: &Command, arguments: &[*const u8], environment: &[*const u8]) -> ! {
    set_signal_actions(SIG_DFL);
    call(number::SETSID, [0; 6]);
    call(number::IOCTL, [0, TIOCSCTTY, STEAL_THE_CONSOLE, 0, 0, 0]);
    let group = call(number::GETPID, [0; 6]) as i32;
    call(
        number::IOCTL,
        [0, TIOCSPGRP, &raw const group as usize, 0, 0, 0],
    );
    call(
        number::EXECVE,
        [
            command.program().as_ptr() as usize,
            arguments.as_ptr() as usize,
            environment.as_ptr() as usize,
            0,
            0,
            0,
        ],
    );
    say(&[b"init: could not run ", command.program_name(), b"\n"]);
    syscall::exit(COMMAND_NOT_RUN)
}

fn wait4(options: usize, status: &mut i32) -> isize {
    call(
        number::WAIT4,
        [usize::MAX, status as *mut i32 as usize, options, 0, 0, 0],
    )
}

fn start_agent_server(agent: &mut AgentServer, restarted: bool) {
    let program = AGENT_SERVER.as_ptr();
    let mut arguments = [ptr::null::<u8>(); 3];
    arguments[0] = program;
    if restarted {
        arguments[1] = RESTARTED_ARGUMENT.as_ptr();
    }
    let mut environment = [ptr::null::<u8>(); ENVIRONMENT.len() + 1];
    for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
        *slot = variable.as_ptr();
    }
    let child = call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]);
    match child {
        0 => {
            set_signal_actions(SIG_DFL);
            call(number::SETSID, [0; 6]);
            call(
                number::EXECVE,
                [
                    program as usize,
                    arguments.as_ptr() as usize,
                    environment.as_ptr() as usize,
                    0,
                    0,
                    0,
                ],
            );
            syscall::exit(COMMAND_NOT_RUN)
        }
        1.. => {
            agent.started(child as usize, now_ms());
            say(&[b"init: started the MCP server on /dev/agent\n"]);
        }
        _ => say(&[b"init: could not start the MCP server\n"]),
    }
}

fn start_display_server(display: &mut AgentServer) {
    let program = DISPLAY_SERVER.as_ptr();
    let arguments = [program, ptr::null()];
    let mut environment = [ptr::null::<u8>(); ENVIRONMENT.len() + 1];
    for (slot, variable) in environment.iter_mut().zip(ENVIRONMENT) {
        *slot = variable.as_ptr();
    }
    let child = call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]);
    match child {
        0 => {
            set_signal_actions(SIG_DFL);
            call(number::SETSID, [0; 6]);
            call(
                number::EXECVE,
                [
                    program as usize,
                    arguments.as_ptr() as usize,
                    environment.as_ptr() as usize,
                    0,
                    0,
                    0,
                ],
            );
            syscall::exit(COMMAND_NOT_RUN)
        }
        1.. => display.started(child as usize, now_ms()),
        _ => say(&[b"init: could not start eggway\n"]),
    }
}

fn notice_end(services: &mut Services, ended: isize) {
    if ended <= 0 {
        return;
    }
    match services.agent.ended(ended as usize, now_ms()) {
        Some(AfterEnd::Restart) => start_agent_server(&mut services.agent, true),
        Some(AfterEnd::GiveUp) => {
            say(&[b"init: eggmcpd keeps ending; the agent channel is closed\n"])
        }
        None => {}
    }
    match services.display.ended(ended as usize, now_ms()) {
        Some(AfterEnd::Restart) => start_display_server(&mut services.display),
        Some(AfterEnd::GiveUp) => {
            say(&[b"init: eggway keeps ending; Wayland programs cannot open windows\n"])
        }
        None => {}
    }
}

fn wait_for(child: usize, services: &mut Services) -> Option<i32> {
    loop {
        let mut status = 0;
        match wait4(0, &mut status) {
            ended if ended == child as isize => return Some(status),
            error if error == -ECHILD => return None,
            ended => notice_end(services, ended),
        }
    }
}

fn reap_ended(services: &mut Services) {
    let mut status = 0;
    loop {
        let ended = wait4(WNOHANG, &mut status);
        if ended <= 0 {
            return;
        }
        notice_end(services, ended);
    }
}

fn reap_forever(services: &mut Services) -> ! {
    loop {
        let mut status = 0;
        let ended = wait4(0, &mut status);
        notice_end(services, ended);
        if ended == -ECHILD {
            let nothing_blocked: u64 = 0;
            call(
                number::RT_SIGSUSPEND,
                [
                    &raw const nothing_blocked as usize,
                    SIGNAL_SET_SIZE,
                    0,
                    0,
                    0,
                    0,
                ],
            );
        }
    }
}

fn sleep_reaping(seconds: u64, services: &mut Services) {
    reap_ended(services);
    let time = Timespec {
        seconds: seconds as i64,
        nanoseconds: 0,
    };
    while call(
        number::CLOCK_NANOSLEEP,
        [CLOCK_MONOTONIC, 0, &raw const time as usize, 0, 0, 0],
    ) == -EINTR
    {}
    reap_ended(services);
}

fn report(name: &[u8], status: i32) {
    let mut digits = [0; 20];
    let signal = status & 0x7f;
    let (how, value) = match signal {
        0 => (&b" exited with status "[..], (status >> 8) & 0xff),
        _ => (&b" was killed by signal "[..], signal),
    };
    say(&[
        b"init: ",
        name,
        how,
        decimal(value as u64, &mut digits),
        b"\n",
    ]);
}
