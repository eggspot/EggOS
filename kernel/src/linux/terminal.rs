//! The console as a terminal (M6, D61, D100): `read` and `readv` through the console's line
//! discipline ([`crate::console::line_discipline`]), `ioctl`, and `lseek`, which a terminal
//! refuses (D74), on any descriptor that names the console (D101). C libraries ask whether a
//! descriptor is a terminal before they choose how to buffer it (musl's stdout and `isatty`
//! with `TIOCGWINSZ`, glibc's `isatty` and `tcgetattr` with `TCGETS`), so a program that
//! writes to the console line-buffers as it would on a Linux serial console.
//!
//! The requests answered:
//! - `TCGETS` and `TCGETS2`: the console's settings, a Linux serial console's at 115200 baud
//!   until a program changes them; `TCGETS2` adds the speed, which is always 115200.
//! - `TCSETS`, `TCSETSW` and `TCSETSF`, and their `termios2` forms: change them. Output is
//!   written at once, so `TCSETSW` does not wait; `TCSETSF` discards the unread input first.
//!   Every field is kept as given; output is processed by the UART driver whatever the output
//!   flags say, and the line's speed and format do not change.
//! - `TCFLSH`: discards the unread input (`TCIFLUSH`, `TCIOFLUSH`); output is never queued.
//! - `TIOCGWINSZ` and `TIOCSWINSZ`: the window size, zero until a program sets one, as on a
//!   serial line, whose size the kernel cannot know. Nobody is told of a change (`SIGWINCH`).
//! - `FIONREAD`: the bytes a read would take without waiting.
//!
//! Every other request is `ENOTTY`, as Linux answers a request a terminal does not know: job
//! control (`TIOCGPGRP`, `TIOCSPGRP`) waits for sessions. The requests every descriptor
//! answers (`FIONBIO`, `FIOCLEX`, `FIONCLEX`) come first ([`super::descriptors`]).
//!
//! The thread that last read the console or polled it for input is its foreground: the
//! signal characters (`^C`, `^\`) signal its process ([`console::Foreground`]), until process
//! groups name a foreground group.

use super::descriptors::OpenFile;
use super::futex::{self, WaitSlot};
use super::signals::{self, Interrupted};
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec, poll, signal};
use crate::console::line_discipline::Termios;
use crate::console::{self, TerminalSignal};
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

/// The size of `struct termios2`: `struct termios`, then the input and output speeds.
const TERMIOS2_SIZE: usize = Termios::SIZE + 8;

/// The console's speed, in baud: what QEMU's and most boards' serial consoles run at.
const BAUD: u32 = 115_200;

/// The size of a `struct winsize`: rows, columns, then the width and height in pixels, each
/// an `unsigned short`.
const WINSIZE_SIZE: usize = 8;

const READ_CHUNK: usize = 256;

const TENTH_OF_A_SECOND_NS: u64 = 100_000_000;

/// The console's window size, as `TIOCSWINSZ` last set it: the console is one for the whole
/// machine. Never held while user memory is copied.
static WINDOW_SIZE: SpinLock<[u8; WINSIZE_SIZE]> = SpinLock::new([0; WINSIZE_SIZE]);

/// `ioctl(fd, request, argument)` on a descriptor that names the console, which the caller
/// looked up: see the module documentation. As Linux: `ENOTTY` for a request the console does
/// not answer; `EFAULT` if `argument` is not the program's memory (writable, for a request
/// that reports something); `EINVAL` for a `TCFLSH` argument other than the three. `request`
/// is 32-bit: the register's upper half is ignored.
pub fn ioctl<P: Platform>(process: &Process<P>, request: u64, argument: u64) -> Result<u64, Errno> {
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
        console::set_settings::<P>(Termios::from_bytes(&bytes), flush);
        Ok(0)
    };
    match request as u32 {
        request::TCGETS => write(&console::settings().to_bytes()),
        request::TCGETS2 => {
            let mut bytes = [0; TERMIOS2_SIZE];
            bytes[..Termios::SIZE].copy_from_slice(&console::settings().to_bytes());
            bytes[Termios::SIZE..Termios::SIZE + 4].copy_from_slice(&BAUD.to_ne_bytes());
            bytes[Termios::SIZE + 4..].copy_from_slice(&BAUD.to_ne_bytes());
            write(&bytes)
        }
        request::TCSETS | request::TCSETSW | request::TCSETS2 | request::TCSETSW2 => set(false),
        request::TCSETSF | request::TCSETSF2 => set(true),
        request::TCFLSH => match argument as u32 {
            flush::INPUT | flush::BOTH => {
                console::flush_input();
                Ok(0)
            }
            flush::OUTPUT => Ok(0),
            _ => Err(Errno::EINVAL),
        },
        request::TIOCGWINSZ => {
            let size = *WINDOW_SIZE.lock();
            write(&size)
        }
        request::TIOCSWINSZ => {
            let mut size = [0; WINSIZE_SIZE];
            process
                .read_user_memory(argument, &mut size)
                .map_err(|_| Errno::EFAULT)?;
            *WINDOW_SIZE.lock() = size;
            Ok(0)
        }
        request::FIONREAD => {
            let pending = i32::try_from(console::pending_input()).unwrap_or(i32::MAX);
            write(&pending.to_ne_bytes())
        }
        _ => Err(Errno::ENOTTY),
    }
}

/// The last `whence` `lseek` knows: `SEEK_HOLE` (after `SEEK_SET`, `SEEK_CUR`, `SEEK_END` and
/// `SEEK_DATA`).
const SEEK_LAST: u32 = 4;

/// `lseek(fd, offset, whence)` on a descriptor that names the console, which a terminal
/// cannot do. As Linux: `EINVAL` for a `whence` it does not know (an `unsigned int`), then
/// `ESPIPE`; the offset is never looked at.
pub fn lseek(whence: u64) -> Result<u64, Errno> {
    if whence as u32 > SEEK_LAST {
        Err(Errno::EINVAL)
    } else {
        Err(Errno::ESPIPE)
    }
}

/// `read` or `readv` of `file`, the console, into `buffers`, with the program's registers
/// `context`; the caller checked the descriptor and the buffers. Takes the console's input as
/// its line discipline says. In canonical mode it waits for a line and returns it, or as much
/// of it as the buffers hold (the rest is read next), or 0 for an end of file typed at the
/// start of a line. Otherwise `VMIN` and `VTIME` decide (termios(3)): with both 0 it returns
/// what is there, maybe nothing; with `VMIN` 0 it waits up to `VTIME` tenths of a second from
/// the call for a byte; with `VTIME` 0 it waits for `VMIN` bytes (or as many as the buffers
/// hold, if fewer); with both, for a first byte, then until `VMIN` bytes or `VTIME` passes with
/// none arriving. With `O_NONBLOCK` it never waits: where it would, it returns what is there,
/// or fails with `EAGAIN` if nothing is. The calling thread becomes the console's foreground
/// ([`console::Foreground`]).
///
/// As Linux: 0 for buffers of no bytes; at most 0x7ffff000 bytes; the buffers filled one after
/// another; if part of them is not the program's writable memory, the bytes before it, or
/// `EFAULT` if there are none (what was taken for the rest is lost). A signal due before input
/// is taken interrupts it, as Linux's `-ERESTARTSYS`: it is made again once the handler returns
/// if the handler's action has `SA_RESTART`, and fails with `EINTR` otherwise; if no handler
/// starts, it goes on. A signal that came with the input is delivered once the input is taken.
/// Sets the call's result; breaks when the process ends.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match read_or_wait(process, thread, file, buffers)? {
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

fn read_or_wait<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let count = buffers.total().min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    become_foreground(process, thread);
    let mut deadline = None;
    loop {
        let settings = console::settings();
        let waited = if file.is_nonblocking() {
            if !finds_input_at_once(settings) {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            Waited::Ready
        } else if settings.is_canonical() {
            wait_for_input(process, thread, u64::MAX, console::input_ready)?
        } else {
            wait_for_bytes(process, thread, settings, count, &mut deadline)?
        };
        if waited == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
        let took = take_into(process, buffers, count);
        if read_is_over(settings, waited, took.consumed) {
            return ControlFlow::Continue(Some(took.result));
        }
    }
}

/// Whether a read with `settings` finds what it would wait for without waiting: a line in
/// canonical mode; otherwise a byte, or anything at all with `VMIN` and `VTIME` both 0, when
/// it never waits.
fn finds_input_at_once(settings: Termios) -> bool {
    if settings.is_canonical() {
        console::input_ready()
    } else {
        settings.minimum_and_time() == (0, 0) || console::available_input() > 0
    }
}

/// Whether a read that waited as `waited` says, with `settings`, ends with what it took: it
/// does once it consumed input (bytes, or a line's end or an end of file), and with `VMIN` 0
/// once `VTIME` passed or when `VTIME` is 0 too. Otherwise the input it was woken for went
/// first (another reader took it, or `^C` discarded it), and it waits again rather than return
/// a false end of file: with `VMIN` and `VTIME` both set, for a first byte again, as the timer
/// runs only between bytes.
fn read_is_over(settings: Termios, waited: Waited, consumed: bool) -> bool {
    let (minimum, time) = settings.minimum_and_time();
    consumed
        || (!settings.is_canonical() && minimum == 0 && (time == 0 || waited == Waited::TimedOut))
}

/// Waits for what a non-canonical read with `settings` of up to `count` bytes waits for. With
/// `VMIN` 0 the wait ends by `deadline`, set from `VTIME` on the read's first wait and kept for
/// its others, so a read woken for input another reader took waits no longer than its time.
fn wait_for_bytes<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
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
    match (minimum, time) {
        (0, 0) => ControlFlow::Continue(Waited::Ready),
        (0, _) => {
            let deadline = *deadline.get_or_insert_with(interval);
            wait_for_input(process, thread, deadline, || console::available_input() > 0)
        }
        (_, 0) => wait_for_input(process, thread, u64::MAX, || {
            console::available_input() >= wanted
        }),
        _ => {
            let first =
                wait_for_input(process, thread, u64::MAX, || console::available_input() > 0)?;
            if first != Waited::Ready {
                return ControlFlow::Continue(first);
            }
            loop {
                let arrived = console::available_input();
                if arrived >= wanted {
                    return ControlFlow::Continue(Waited::Ready);
                }
                let waited = wait_for_input(process, thread, interval(), || {
                    console::available_input() > arrived
                })?;
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

/// Takes up to `count` bytes of input into `buffers`, one after another, a page-bounded chunk
/// at a time, ending at a line's end in canonical mode or once no more input is there.
fn take_into<P: Platform>(process: &Process<P>, buffers: &Iovecs, count: u64) -> Took {
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
            let taken = console::take_input(&mut chunk[..wanted]);
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

/// How a wait for console input ended.
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

/// Makes `thread`, of `process`, the console's foreground, if it is not already, unless the
/// process has ended meanwhile (another thread's end forgot it first), so the console never
/// keeps an ended process.
pub fn become_foreground<P: Platform>(process: &Arc<Process<P>>, thread: &LinuxThread) {
    let reader = console::reader_id(process.id().get(), thread.id());
    if console::is_foreground(reader) {
        return;
    }
    console::set_foreground(
        reader,
        Arc::new(ConsoleReader {
            process: Arc::clone(process),
            wait_slot: Arc::clone(thread.wait_slot()),
        }),
    );
    if process.has_ended() {
        console::forget_foreground(process.id().get());
    }
}

struct ConsoleReader<P: Platform> {
    process: Arc<Process<P>>,
    wait_slot: Arc<WaitSlot>,
}

impl<P: Platform> console::Foreground for ConsoleReader<P> {
    fn send(&self, sent: TerminalSignal) {
        let number = match sent {
            TerminalSignal::Interrupt => signal::SIGINT,
            TerminalSignal::Quit => signal::SIGQUIT,
        };
        signals::send_from_terminal::<P>(&self.process, &self.wait_slot, number);
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
        assert_eq!(TERMIOS2_SIZE, 44);
        assert_eq!(request::TCGETS2 >> 16 & 0x3fff, 44);
        assert_eq!(request::TCSETS2 >> 16 & 0x3fff, 44);
    }
}
