//! Given [`READ_THE_CONSOLE`], the program reads the console through its line discipline
//! (D100) as a C library's terminal calls do, asking the QEMU runner to type each script by
//! writing one of the [`request`] lines: canonical lines with their editing, an end of file,
//! `^C` interrupting a `ppoll` and a `read`, and raw bytes. It writes [`PASSED_LINE`] and exits
//! with 0 if every answer was Linux's, or names the first that was not and exits with 1.

use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The argument that runs [`read_the_console`] instead of the checks.
pub const READ_THE_CONSOLE: &str = "read-the-console";

/// The line written when every console check passed.
pub const PASSED_LINE: &str =
    "EGGOS-USER: a std program read the console through its line discipline";

/// The lines that ask the QEMU runner to type a script. Must match the runner's.
pub mod request {
    /// Lines edited with erase, word erase and kill, then an end of file.
    pub const LINES: &str = "EGGOS-CONSOLE: type lines";
    /// A line ended by `^C`, while the program waits in `ppoll`.
    pub const INTERRUPT: &str = "EGGOS-CONSOLE: type an interrupt";
    /// A line ended by `^C`, then another, while the program waits in `read`.
    pub const RESTART: &str = "EGGOS-CONSOLE: type an interrupt and a line";
    /// Bytes for a non-canonical read.
    pub const RAW: &str = "EGGOS-CONSOLE: type raw bytes";
}

/// What a `read` of the lines script returns, in order, with the most each read asks for.
const LINES_READ: [(&[u8], usize); 9] = [
    (b"hello\n", 64),
    (b"ac\n", 64),
    (b"right\n", 64),
    (b"one\n", 64),
    (b"kept\n", 64),
    (b"partial", 64),
    (b"", 64),
    (b"la", 2),
    (b"st\n", 64),
];

const RAW_READ: &[u8] = b"xy\x7fz\n";
const RESTARTED_READ: &[u8] = b"after\n";

const SIGINT: i32 = 2;
const SIG_BLOCK: i32 = 0;
const SIG_SETMASK: i32 = 2;
const EINTR: i32 = 4;
const POLLIN: i16 = 1;
const TCSANOW: i32 = 0;
const TCSAFLUSH: i32 = 2;
const FIONREAD: i32 = 0x541b;
const ICRNL: u32 = 0o400;
const OPOST: u32 = 0o1;
const ONLCR: u32 = 0o4;
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const ECHOE: u32 = 0o20;
const VINTR: usize = 0;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const KERNEL_CONTROL_CHARACTERS: usize = 19;

/// musl's `struct termios`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Termios {
    input: u32,
    output: u32,
    control: u32,
    local: u32,
    line: u8,
    control_characters: [u8; 32],
    input_speed: u32,
    output_speed: u32,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    requested: i16,
    returned: i16,
}

unsafe extern "C" {
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, when: i32, termios: *const Termios) -> i32;
    fn read(fd: i32, buffer: *mut std::ffi::c_void, count: usize) -> isize;
    fn poll(fds: *mut PollFd, count: u64, timeout: i32) -> i32;
    fn ppoll(
        fds: *mut PollFd,
        count: u64,
        timeout: *const super::Timespec,
        mask: *const u64,
    ) -> i32;
    fn ioctl(fd: i32, request: i32, ...) -> i32;
    fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
    fn siginterrupt(signal: i32, interrupt: i32) -> i32;
    fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32;
}

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

extern "C" fn count_interrupt(_: i32) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}

/// Runs the console checks and exits: see the module documentation.
pub fn read_the_console() -> ! {
    let mut output = std::io::stdout();
    match checks() {
        Ok(()) => {
            let _ = writeln!(output, "{PASSED_LINE}");
            let _ = output.flush();
            std::process::exit(0)
        }
        Err(failure) => {
            let _ = writeln!(output, "EGGOS-USER: the console check failed: {failure}");
            let _ = output.flush();
            std::process::exit(1)
        }
    }
}

fn checks() -> Result<(), &'static str> {
    let original = settings()?;
    check(
        original.local & (ICANON | ECHO | ECHOE | ISIG) == ICANON | ECHO | ECHOE | ISIG
            && original.input & ICRNL != 0
            && original.output & (OPOST | ONLCR) == OPOST | ONLCR,
        "the console does not start in canonical mode with echo and signals",
    )?;
    let characters = original.control_characters;
    check(
        [VINTR, VERASE, VKILL, VEOF, VTIME, VMIN].map(|index| characters[index])
            == [0x03, 0x7f, 0x15, 0x04, 0, 1],
        "the console's control characters are not a Linux console's",
    )?;
    check(
        poll_input(0) == 0,
        "poll found input before anything was typed",
    )?;
    check(
        pending() == Some(0),
        "FIONREAD found input before anything was typed",
    )?;
    lines()?;
    interrupt_a_poll()?;
    interrupt_and_restart_a_read()?;
    raw_bytes(original)?;
    // SAFETY: `original` is a `struct termios` that `tcgetattr` filled.
    let restored = unsafe { tcsetattr(0, TCSAFLUSH, &original) };
    check(restored == 0, "tcsetattr could not restore the settings")?;
    let now = settings()?;
    check(
        now.local == original.local
            && now.input == original.input
            && now.control_characters[..KERNEL_CONTROL_CHARACTERS]
                == original.control_characters[..KERNEL_CONTROL_CHARACTERS],
        "the settings read back are not the ones restored",
    )
}

fn check(holds: bool, failure: &'static str) -> Result<(), &'static str> {
    match holds {
        true => Ok(()),
        false => Err(failure),
    }
}

fn settings() -> Result<Termios, &'static str> {
    let mut termios = Termios::default();
    // SAFETY: `termios` is a `struct termios` of musl's layout, which the call fills.
    let result = unsafe { tcgetattr(0, &mut termios) };
    check(result == 0, "tcgetattr failed on the console")?;
    Ok(termios)
}

fn ask_for(script: &str) {
    let mut output = std::io::stdout();
    let _ = writeln!(output, "{script}");
    let _ = output.flush();
}

/// `poll` of descriptor 0 for input with `timeout` milliseconds: the events it reports, or
/// -1 if the call failed.
fn poll_input(timeout: i32) -> i32 {
    let mut entry = PollFd {
        fd: 0,
        requested: POLLIN,
        returned: 0,
    };
    // SAFETY: one `struct pollfd` this function owns.
    match unsafe { poll(&mut entry, 1, timeout) } {
        0 => 0,
        1 => i32::from(entry.returned),
        _ => -1,
    }
}

fn pending() -> Option<i32> {
    let mut count = -1i32;
    // SAFETY: `FIONREAD` writes one `int`.
    let result = unsafe { ioctl(0, FIONREAD, &mut count as *mut i32) };
    (result == 0).then_some(count)
}

fn read_console(count: usize) -> Result<Vec<u8>, i32> {
    let mut buffer = vec![0u8; count];
    // SAFETY: `buffer` holds `count` writable bytes.
    let result = unsafe { read(0, buffer.as_mut_ptr().cast(), count) };
    match usize::try_from(result) {
        Ok(length) => {
            buffer.truncate(length);
            Ok(buffer)
        }
        Err(_) => Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
    }
}

fn lines() -> Result<(), &'static str> {
    ask_for(request::LINES);
    check(
        poll_input(-1) == i32::from(POLLIN),
        "poll did not wait for a line and report it",
    )?;
    for (expected, count) in LINES_READ {
        let line = read_console(count).map_err(|_| "a read of the lines failed")?;
        check(
            line == expected,
            "a read returned other than the line typed, as edited",
        )?;
    }
    check(
        poll_input(0) == 0 && pending() == Some(0),
        "input was left after the last line",
    )
}

fn interrupt_a_poll() -> Result<(), &'static str> {
    // SAFETY: `count_interrupt` only adds to an atomic.
    unsafe { signal(SIGINT, count_interrupt) };
    // SAFETY: a plain libc call.
    unsafe { siginterrupt(SIGINT, 1) };
    let mut sigint = [0u64; super::SIGSET_WORDS];
    sigint[0] = 1 << (SIGINT - 1);
    let mut before = [0u64; super::SIGSET_WORDS];
    // SAFETY: two signal sets of musl's size.
    unsafe { pthread_sigmask(SIG_BLOCK, sigint.as_ptr(), before.as_mut_ptr()) };
    ask_for(request::INTERRUPT);
    let mut entry = PollFd {
        fd: 0,
        requested: POLLIN,
        returned: 0,
    };
    // SAFETY: one `struct pollfd`, no timeout, and a signal set of musl's size.
    let result = unsafe { ppoll(&mut entry, 1, std::ptr::null(), before.as_ptr()) };
    let error = std::io::Error::last_os_error().raw_os_error();
    // SAFETY: as above.
    unsafe { pthread_sigmask(SIG_SETMASK, before.as_ptr(), std::ptr::null_mut()) };
    check(
        result == -1 && error == Some(EINTR),
        "^C did not interrupt ppoll with EINTR",
    )?;
    check(
        INTERRUPTS.load(Ordering::SeqCst) == 1,
        "^C did not run the SIGINT handler once",
    )?;
    check(poll_input(0) == 0, "^C left the line it ended readable")
}

fn interrupt_and_restart_a_read() -> Result<(), &'static str> {
    // SAFETY: a plain libc call: SIGINT's handler restarts the calls it interrupts.
    unsafe { siginterrupt(SIGINT, 0) };
    ask_for(request::RESTART);
    let line =
        read_console(64).map_err(|_| "a read ^C interrupted failed instead of restarting")?;
    check(
        line == RESTARTED_READ,
        "the restarted read did not return the next line",
    )?;
    check(
        INTERRUPTS.load(Ordering::SeqCst) == 2,
        "^C during a read did not run the SIGINT handler",
    )
}

fn raw_bytes(original: Termios) -> Result<(), &'static str> {
    let mut raw = original;
    raw.local &= !(ICANON | ECHO);
    raw.control_characters[VMIN] = 1;
    raw.control_characters[VTIME] = 0;
    set(&raw)?;
    check(settings()? == raw, "the raw settings read back changed")?;
    ask_for(request::RAW);
    let mut bytes = Vec::new();
    while bytes.len() < RAW_READ.len() {
        let read = read_console(RAW_READ.len() - bytes.len())
            .map_err(|_| "a non-canonical read failed")?;
        check(!read.is_empty(), "a read with VMIN 1 returned nothing")?;
        bytes.extend(read);
    }
    check(
        bytes == RAW_READ,
        "a non-canonical read did not return the bytes typed",
    )?;
    raw.control_characters[VMIN] = 0;
    set(&raw)?;
    check(
        read_console(8) == Ok(Vec::new()),
        "a read with VMIN 0 and VTIME 0 did not return 0 at once",
    )?;
    raw.control_characters[VTIME] = 1;
    set(&raw)?;
    let started = Instant::now();
    check(
        read_console(8) == Ok(Vec::new()),
        "a read with VTIME 1 did not return 0 when nothing was typed",
    )?;
    check(
        started.elapsed() >= Duration::from_millis(100),
        "a read with VTIME 1 returned before a tenth of a second",
    )
}

fn set(termios: &Termios) -> Result<(), &'static str> {
    // SAFETY: a `struct termios` of musl's layout.
    let result = unsafe { tcsetattr(0, TCSANOW, termios) };
    check(result == 0, "tcsetattr failed on the console")
}
