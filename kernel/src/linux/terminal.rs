//! Terminals as programs see them (M6, D61, D100, D187): `read` and `readv` through a terminal's
//! line discipline ([`crate::console::line_discipline`]), `ioctl`, and `lseek`, which a terminal
//! refuses (D74), on any descriptor that names the console (D101) or the program side of a
//! pseudo-terminal ([`super::pseudo_terminal`]), both a [`Line`]. C libraries ask whether a
//! descriptor is a terminal before they choose how to buffer it (musl's stdout and `isatty`
//! with `TIOCGWINSZ`, glibc's `isatty` and `tcgetattr` with `TCGETS`), so a program that
//! writes to a terminal line-buffers as it would on a Linux serial console.
//!
//! The requests answered:
//! - `TCGETS` and `TCGETS2`: the terminal's settings, the console's a Linux serial console's at
//!   115200 baud until a program changes them; `TCGETS2` adds the speed, which does not change.
//! - `TCSETS`, `TCSETSW` and `TCSETSF`, and their `termios2` forms: change them. Output is
//!   written at once, so `TCSETSW` does not wait; `TCSETSF` discards the unread input first.
//!   Every field is kept as given; output is processed by the UART driver whatever the output
//!   flags say, and the line's speed and format do not change.
//! - `TCFLSH`: discards the unread input (`TCIFLUSH`, `TCIOFLUSH`); output is never queued.
//! - `TIOCGWINSZ` and `TIOCSWINSZ`: the window size, zero until a program sets one, as on a
//!   serial line, whose size the kernel cannot know (the graphical console's grid and a
//!   terminal program set it). A change sends `SIGWINCH` to the terminal's foreground group
//!   (D181).
//! - `FIONREAD`: the bytes a read would take without waiting.
//! - `TIOCSCTTY`, `TIOCGPGRP`, `TIOCSPGRP` and `TIOCGSID`: the terminal as a session's
//!   controlling terminal and its foreground process group ([`super::job_control`], D105),
//!   which the signal characters (`^C`, `^\`) signal.
//!
//! Every other request is `ENOTTY`, as Linux answers a request a terminal does not know. The
//! requests every descriptor answers (`FIONBIO`, `FIOCLEX`, `FIONCLEX`) come first
//! ([`super::descriptors`]).
//!
//! A process in a background group of a terminal's session that reads the terminal is sent
//! `SIGTTIN`, and one that changes its settings, discards its input or sets its foreground
//! group `SIGTTOU` ([`job_control::terminal_access`], D181). A pseudo-terminal whose controlling
//! side closed is hung up: its reads return the end of the file at once.

use super::descriptors::OpenFile;
use super::futex;
use super::process_table::Terminal;
use super::signals::{self, Interrupted};
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec, job_control, poll};
use crate::console;
use crate::console::line_discipline::{Taken, Termios};
use crate::process::Process;
use crate::sync::SpinLock;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::user::UserContext;

/// The requests answered, the generic numbers, which AArch64, RISC-V and x86_64 share.
pub mod request {
    /// Read the settings (`struct termios`).
    pub const TCGETS: u32 = 0x5401;
    /// Change the settings.
    pub const TCSETS: u32 = 0x5402;
    /// Change the settings once output is written.
    pub const TCSETSW: u32 = 0x5403;
    /// Change the settings once output is written, discarding the unread input.
    pub const TCSETSF: u32 = 0x5404;
    /// Discard unread input or unwritten output (an `int` saying which).
    pub const TCFLSH: u32 = 0x540b;
    /// Read the window size (`struct winsize`).
    pub const TIOCGWINSZ: u32 = 0x5413;
    /// Set the window size.
    pub const TIOCSWINSZ: u32 = 0x5414;
    /// How many bytes wait to be read (an `int`).
    pub const FIONREAD: u32 = 0x541b;
    /// Read the settings with their speeds (`struct termios2`): `_IOR('T', 0x2a, 44 bytes)`.
    pub const TCGETS2: u32 = 0x802c_542a;
    /// [`TCSETS`] with a `struct termios2`: `_IOW('T', 0x2b, 44 bytes)`.
    pub const TCSETS2: u32 = 0x402c_542b;
    /// [`TCSETSW`] with a `struct termios2`.
    pub const TCSETSW2: u32 = 0x402c_542c;
    /// [`TCSETSF`] with a `struct termios2`.
    pub const TCSETSF2: u32 = 0x402c_542d;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's terminal ioctl numbers for this architecture");
}

mod flush {
    pub const INPUT: u32 = 0;
    pub const OUTPUT: u32 = 1;
    pub const BOTH: u32 = 2;
}

const TERMIOS_AND_TWO_SPEEDS_SIZE: usize = Termios::SIZE + 8;

const SERIAL_CONSOLE_BAUD: u32 = 115_200;

/// The size of a `struct winsize`: rows, columns, then the width and height in pixels, each
/// an `unsigned short`.
pub const WINSIZE_SIZE: usize = 8;

const READ_CHUNK: usize = 256;

const TENTH_OF_A_SECOND_NS: u64 = 100_000_000;

static CONSOLE_WINDOW_SIZE: SpinLock<[u8; WINSIZE_SIZE]> = SpinLock::new([0; WINSIZE_SIZE]);

/// A terminal's input side as reads and `ioctl` see it: the console ([`Console`]) or the
/// program side of a pseudo-terminal ([`super::pseudo_terminal::PseudoTerminal`]).
pub trait Line {
    /// The terminal, for job control.
    fn terminal(&self) -> Terminal;
    /// The speed `TCGETS2` reports, in baud.
    fn speed(&self) -> u32;
    /// The settings.
    fn settings(&self) -> Termios;
    /// Changes the settings, discarding the unread input first if `flush`, and wakes the
    /// readers.
    fn set_settings<P: Platform>(&self, settings: Termios, flush: bool);
    /// Discards the unread input and the line being edited.
    fn flush_input(&self);
    /// Whether a read would find input.
    fn is_readable(&self) -> bool;
    /// The unread bytes a non-canonical read can take.
    fn available(&self) -> usize;
    /// The bytes a read can take without waiting.
    fn pending(&self) -> usize;
    /// Moves unread input into `buffer`; never waits.
    fn take(&self, buffer: &mut [u8]) -> Taken;
    /// Whether the terminal hung up, so a read returns the end of the file at once.
    fn is_hung_up(&self) -> bool;
    /// The window size, a `struct winsize`.
    fn window_size(&self) -> [u8; WINSIZE_SIZE];
    /// Sets the window size; returns the one before.
    fn replace_window_size(&self, size: [u8; WINSIZE_SIZE]) -> [u8; WINSIZE_SIZE];
    /// Waits as [`wait_for_input`] does, for this terminal's input.
    fn wait<P: Platform>(
        &self,
        process: &Process<P>,
        thread: &mut LinuxThread,
        deadline: u64,
        ready: impl Fn() -> bool,
    ) -> ControlFlow<(), Waited>;
}

/// The console as a [`Line`].
#[derive(Debug, Clone, Copy)]
pub struct Console;

impl Line for Console {
    #[inline(always)]
    fn terminal(&self) -> Terminal {
        Terminal::Console
    }

    fn speed(&self) -> u32 {
        SERIAL_CONSOLE_BAUD
    }

    #[inline(always)]
    fn settings(&self) -> Termios {
        console::settings()
    }

    fn set_settings<P: Platform>(&self, settings: Termios, flush: bool) {
        console::set_settings::<P>(settings, flush);
    }

    fn flush_input(&self) {
        console::flush_input();
    }

    #[inline(always)]
    fn is_readable(&self) -> bool {
        console::input_ready()
    }

    #[inline(always)]
    fn available(&self) -> usize {
        console::available_input()
    }

    fn pending(&self) -> usize {
        console::pending_input()
    }

    #[inline(always)]
    fn take(&self, buffer: &mut [u8]) -> Taken {
        console::take_input(buffer)
    }

    #[inline(always)]
    fn is_hung_up(&self) -> bool {
        false
    }

    fn window_size(&self) -> [u8; WINSIZE_SIZE] {
        *CONSOLE_WINDOW_SIZE.lock()
    }

    fn replace_window_size(&self, size: [u8; WINSIZE_SIZE]) -> [u8; WINSIZE_SIZE] {
        core::mem::replace(&mut *CONSOLE_WINDOW_SIZE.lock(), size)
    }

    #[inline(always)]
    fn wait<P: Platform>(
        &self,
        process: &Process<P>,
        thread: &mut LinuxThread,
        deadline: u64,
        ready: impl Fn() -> bool,
    ) -> ControlFlow<(), Waited> {
        wait_for_input(process, thread, deadline, ready)
    }
}

/// Sets the console's window size to the graphical terminal's grid, `rows` by `columns` cells
/// of `cell_width` by `cell_height` pixels, as `TIOCGWINSZ` reports it. Nobody is told: for the
/// terminal's first size; [`resize_window`] tells the foreground programs of a change.
pub fn set_window_size(rows: u16, columns: u16, cell_width: u16, cell_height: u16) {
    let _ = Console.replace_window_size(window_size(rows, columns, cell_width, cell_height));
}

/// [`set_window_size`] for a terminal whose grid changed while programs run: the console's
/// foreground group is sent `SIGWINCH` if the size is not what it was (D181).
pub fn resize_window<P: Platform>(rows: u16, columns: u16, cell_width: u16, cell_height: u16) {
    let size = window_size(rows, columns, cell_width, cell_height);
    if Console.replace_window_size(size) != size {
        job_control::send_to_foreground::<P>(Terminal::Console, super::signal::SIGWINCH);
    }
}

fn window_size(rows: u16, columns: u16, cell_width: u16, cell_height: u16) -> [u8; WINSIZE_SIZE] {
    let mut size = [0; WINSIZE_SIZE];
    let fields = [
        rows,
        columns,
        columns.saturating_mul(cell_width),
        rows.saturating_mul(cell_height),
    ];
    for (bytes, field) in size.as_chunks_mut::<2>().0.iter_mut().zip(fields) {
        *bytes = field.to_ne_bytes();
    }
    size
}

/// Whether `request` changes a terminal, so a process in a background group of its session
/// gets `SIGTTOU` for it (D181): the settings, discarding input, the foreground group.
pub fn changes_the_terminal(request: u64) -> bool {
    matches!(
        request as u32,
        request::TCSETS
            | request::TCSETSW
            | request::TCSETSF
            | request::TCSETS2
            | request::TCSETSW2
            | request::TCSETSF2
            | request::TCFLSH
            | job_control::request::TIOCSPGRP
    )
}

/// `ioctl(fd, request, argument)` on a descriptor that names terminal `line`, which the caller
/// looked up: see the module documentation. As Linux: `ENOTTY` for a request a terminal does
/// not answer; `EFAULT` if `argument` is not the program's memory (writable, for a request
/// that reports something); `EINVAL` for a `TCFLSH` argument other than the three. `request`
/// is 32-bit: the register's upper half is ignored.
pub fn ioctl<P: Platform, L: Line>(
    process: &Process<P>,
    line: &L,
    request: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let write = |bytes: &[u8]| {
        process
            .write_user_memory(argument, bytes)
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT)
    };
    let set = |flush: bool| {
        let mut bytes = [0; Termios::SIZE];
        process
            .read_user_memory(argument, &mut bytes)
            .map_err(|_| Errno::EFAULT)?;
        line.set_settings::<P>(Termios::from_bytes(&bytes), flush);
        Ok(0)
    };
    if let Some(result) = job_control::ioctl(process, line.terminal(), request as u32, argument) {
        return result;
    }
    match request as u32 {
        request::TCGETS => write(&line.settings().to_bytes()),
        request::TCGETS2 => {
            let speed = line.speed().to_ne_bytes();
            let mut bytes = [0; TERMIOS_AND_TWO_SPEEDS_SIZE];
            bytes[..Termios::SIZE].copy_from_slice(&line.settings().to_bytes());
            bytes[Termios::SIZE..Termios::SIZE + 4].copy_from_slice(&speed);
            bytes[Termios::SIZE + 4..].copy_from_slice(&speed);
            write(&bytes)
        }
        request::TCSETS | request::TCSETSW | request::TCSETS2 | request::TCSETSW2 => set(false),
        request::TCSETSF | request::TCSETSF2 => set(true),
        request::TCFLSH => match argument as u32 {
            flush::INPUT | flush::BOTH => {
                line.flush_input();
                Ok(0)
            }
            flush::OUTPUT => Ok(0),
            _ => Err(Errno::EINVAL),
        },
        request::TIOCGWINSZ => write(&line.window_size()),
        request::TIOCSWINSZ => {
            let mut size = [0; WINSIZE_SIZE];
            process
                .read_user_memory(argument, &mut size)
                .map_err(|_| Errno::EFAULT)?;
            if line.replace_window_size(size) != size {
                job_control::send_to_foreground::<P>(line.terminal(), super::signal::SIGWINCH);
            }
            Ok(0)
        }
        request::FIONREAD => {
            let pending = i32::try_from(line.pending()).unwrap_or(i32::MAX);
            write(&pending.to_ne_bytes())
        }
        _ => Err(Errno::ENOTTY),
    }
}

const SEEK_HOLE: u32 = 4;

/// `lseek(fd, offset, whence)` on a descriptor that names a terminal, a pipe or a socket, which
/// cannot be done. As Linux: `EINVAL` for a `whence` it does not know (an `unsigned int`; the
/// last it knows is `SEEK_HOLE`), then `ESPIPE`; the offset is never looked at.
pub fn lseek(whence: u64) -> Result<u64, Errno> {
    if whence as u32 > SEEK_HOLE {
        Err(Errno::EINVAL)
    } else {
        Err(Errno::ESPIPE)
    }
}

/// `read` or `readv` of `buffers` from terminal `line` through `file`, which the caller checked
/// is open for reading; sets the call's result and delivers the signals due after it: see the
/// module documentation.
pub(super) fn read<P: Platform, L: Line>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    line: &L,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match job_control::terminal_access(process, thread, line.terminal(), super::signal::SIGTTIN)
        {
            job_control::Access::Allowed => {}
            job_control::Access::Refused => break Err(Errno::EIO),
            job_control::Access::Signalled => {
                return job_control::restart_after_signal(process, thread, context);
            }
        }
        match read_or_wait(process, thread, file, line, buffers)? {
            Some(result) => break result,
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn read_or_wait<P: Platform, L: Line>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    line: &L,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let count = buffers.total().min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    let mut deadline = None;
    loop {
        let settings = line.settings();
        let waited = if file.is_nonblocking() {
            if !finds_input_at_once(line, settings) && !line.is_hung_up() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            Waited::Ready
        } else if settings.is_canonical() {
            line.wait(process, thread, u64::MAX, || {
                line.is_readable() || line.is_hung_up()
            })?
        } else {
            wait_for_bytes(process, thread, line, settings, count, &mut deadline)?
        };
        if waited == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
        let took = take_into(process, line, buffers, count);
        if read_is_over(settings, waited, took.consumed) || line.is_hung_up() {
            return ControlFlow::Continue(Some(took.result));
        }
    }
}

fn finds_input_at_once<L: Line>(line: &L, settings: Termios) -> bool {
    if settings.is_canonical() {
        line.is_readable()
    } else {
        settings.minimum_and_time() == (0, 0) || line.available() > 0
    }
}

fn read_is_over(settings: Termios, waited: Waited, consumed: bool) -> bool {
    let (minimum, time) = settings.minimum_and_time();
    consumed
        || (!settings.is_canonical() && minimum == 0 && (time == 0 || waited == Waited::TimedOut))
}

fn wait_for_bytes<P: Platform, L: Line>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    line: &L,
    settings: Termios,
    count: u64,
    deadline: &mut Option<u64>,
) -> ControlFlow<(), Waited> {
    let (minimum, time) = settings.minimum_and_time();
    let wanted = usize::try_from(count).map_or(usize::from(minimum), |count| {
        count.min(usize::from(minimum))
    });
    let interval =
        || poll::deadline_in::<P>(u64::from(time) * TENTH_OF_A_SECOND_NS).unwrap_or(u64::MAX);
    let has = |least: usize| line.available() >= least || line.is_hung_up();
    match (minimum, time) {
        (0, 0) => ControlFlow::Continue(Waited::Ready),
        (0, _) => {
            let deadline = *deadline.get_or_insert_with(interval);
            line.wait(process, thread, deadline, || has(1))
        }
        (_, 0) => line.wait(process, thread, u64::MAX, || has(wanted)),
        _ => {
            let first = line.wait(process, thread, u64::MAX, || has(1))?;
            if first != Waited::Ready {
                return ControlFlow::Continue(first);
            }
            loop {
                let arrived = line.available();
                if arrived >= wanted || line.is_hung_up() {
                    return ControlFlow::Continue(Waited::Ready);
                }
                let waited = line.wait(process, thread, interval(), || has(arrived + 1))?;
                if waited != Waited::Ready {
                    return ControlFlow::Continue(waited);
                }
            }
        }
    }
}

struct Took {
    result: Result<u64, Errno>,
    consumed: bool,
}

fn take_into<P: Platform, L: Line>(
    process: &Process<P>,
    line: &L,
    buffers: &Iovecs,
    count: u64,
) -> Took {
    let mut chunk = [0u8; READ_CHUNK];
    let mut copied = 0;
    let mut consumed = false;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(count - copied);
        let mut done = 0;
        while done < length {
            let address = base + done;
            let wanted = (length - done)
                .min(READ_CHUNK as u64)
                .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
            let taken = line.take(&mut chunk[..wanted]);
            consumed |= taken.length > 0 || taken.line_ended;
            if taken.length > 0
                && process
                    .write_user_memory(address, &chunk[..taken.length])
                    .is_err()
            {
                let result = match copied {
                    0 => Err(Errno::EFAULT),
                    _ => Ok(copied),
                };
                return Took { result, consumed };
            }
            done += taken.length as u64;
            copied += taken.length as u64;
            if taken.line_ended || taken.length < wanted {
                return Took {
                    result: Ok(copied),
                    consumed,
                };
            }
        }
        if copied == count {
            break;
        }
    }
    Took {
        result: Ok(copied),
        consumed,
    }
}

/// How a wait for a terminal's input ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// What the waiter wanted is there.
    Ready,
    /// The deadline came first.
    TimedOut,
    /// A signal is due to the waiting thread, which it has taken in.
    Signalled,
}

/// Waits, as `thread`, the calling thread of `process`, until `ready` says the console's
/// input is what it wants, until the clock reaches `deadline` ([`Platform::clock_ticks`],
/// `u64::MAX` for none), or until a signal is due to it ([`signals::is_due`], taken in from
/// what other threads and the console sent). Woken by the console's interrupt
/// ([`console::INPUT_KEY`]) and, for a signal, through the thread's wait slot. Breaks when
/// the process ends. Call it as a blocking system call runs: with interrupts masked and no
/// lock held.
pub fn wait_for_input<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    deadline: u64,
    ready: impl Fn() -> bool,
) -> ControlFlow<(), Waited> {
    loop {
        if ready() {
            return ControlFlow::Continue(Waited::Ready);
        }
        if process.has_ended() {
            return ControlFlow::Break(());
        }
        signals::take_in_sent(process, thread);
        if signals::is_due(thread) {
            return ControlFlow::Continue(Waited::Signalled);
        }
        if deadline != u64::MAX && P::clock_ticks() >= deadline {
            return ControlFlow::Continue(Waited::TimedOut);
        }
        #[cfg(feature = "qemu-boot-clock")]
        crate::boot_clock::report_first_wait::<P>();
        crate::terminal::console_waited::<P>();
        let unless_signalled_or_ended = unless_signalled_or_ended(process, thread);
        let waited = futex::wait_interruptibly::<P>(
            console::INPUT_KEY,
            u32::MAX,
            deadline,
            thread.wait_slot(),
            || {
                unless_signalled_or_ended()?;
                match ready() {
                    true => Err(Errno::EAGAIN),
                    false => Ok(()),
                }
            },
        );
        if waited == Err(Errno::EINTR) {
            signals::take_in_sent_now(process, thread);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_woken_for_input_that_went_first_waits_again_instead_of_returning_zero() {
        use crate::console::line_discipline::{index, local};
        let canonical = Termios::SERIAL_CONSOLE;
        let raw = |minimum, time| {
            let mut settings = canonical;
            settings.local &= !local::ICANON;
            settings.control_characters[index::VMIN] = minimum;
            settings.control_characters[index::VTIME] = time;
            settings
        };
        for settings in [canonical, raw(1, 0), raw(1, 5), raw(0, 5)] {
            assert!(
                !read_is_over(settings, Waited::Ready, false),
                "{settings:?}"
            );
            assert!(read_is_over(settings, Waited::Ready, true), "{settings:?}");
        }
        assert!(read_is_over(raw(0, 5), Waited::TimedOut, false));
        assert!(read_is_over(raw(0, 0), Waited::Ready, false));
    }

    #[test]
    fn a_read_with_vmin_and_vtime_whose_bytes_another_reader_took_waits_for_a_first_byte_again() {
        use crate::console::line_discipline::{index, local};
        let mut settings = Termios::SERIAL_CONSOLE;
        settings.local &= !local::ICANON;
        settings.control_characters[index::VMIN] = 1;
        settings.control_characters[index::VTIME] = 5;
        assert!(!read_is_over(settings, Waited::TimedOut, false));
        assert!(read_is_over(settings, Waited::TimedOut, true));
    }

    #[test]
    fn the_termios2_size_is_termios_and_two_speeds() {
        assert_eq!(TERMIOS_AND_TWO_SPEEDS_SIZE, 44);
        assert_eq!(request::TCGETS2 >> 16 & 0x3fff, 44);
        assert_eq!(request::TCSETS2 >> 16 & 0x3fff, 44);
    }
}
