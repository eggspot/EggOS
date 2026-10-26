//! The console as a terminal (M6, D61, D100): `read` from descriptors 0 to 2 through the
//! console's line discipline ([`crate::console::line_discipline`]), `ioctl` on them, and
//! `lseek`, which a terminal refuses (D74). C libraries ask whether a descriptor is a terminal
//! before they choose how to buffer it (musl's stdout and `isatty` with `TIOCGWINSZ`, glibc's
//! `isatty` and `tcgetattr` with `TCGETS`), so a program that writes to the console
//! line-buffers as it would on a Linux serial console.
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
//! control (`TIOCGPGRP`, `TIOCSPGRP`) waits for sessions, and the descriptor flags (`FIONBIO`,
//! `FIOCLEX`, `FIONCLEX`) for a descriptor table.
//!
//! The thread that last read the console or polled it for input is its foreground: the
//! signal characters (`^C`, `^\`) signal its process ([`console::Foreground`]), until process
//! groups name a foreground group.

use super::futex::{self, WaitSlot};
use super::signals::{self, Interrupted};
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::{Errno, MAX_RW_COUNT, ends_in_user_space, is_console, poll, signal};
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

/// `ioctl(fd, request, argument)` on the console: see the module documentation. As Linux:
/// `EBADF` for a descriptor that is not open, before the request is looked at; `ENOTTY` for a
/// request the console does not answer; `EFAULT` if `argument` is not the program's memory
/// (writable, for a request that reports something); `EINVAL` for a `TCFLSH` argument other
/// than the three. `fd` and `request` are 32-bit: the registers' upper halves are ignored.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    fd: u64,
    request: u64,
    argument: u64,
) -> Result<u64, Errno> {
    if !is_console(fd) {
        return Err(Errno::EBADF);
    }
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

/// `lseek(fd, offset, whence)` on the console, which a terminal cannot do. As Linux: `EBADF`
/// for a descriptor that is not open, `EINVAL` for a `whence` it does not know (an `unsigned
/// int`), then `ESPIPE`; the offset is never looked at.
pub fn lseek(fd: u64, whence: u64) -> Result<u64, Errno> {
    if !is_console(fd) {
        Err(Errno::EBADF)
    } else if whence as u32 > SEEK_LAST {
        Err(Errno::EINVAL)
    } else {
        Err(Errno::ESPIPE)
    }
}

/// `read(fd, buffer, count)`, with the program's registers `context`: takes the console's
/// input as its line discipline says. In canonical mode it waits for a line and returns it,
/// or as much of it as `count` allows (the rest is read next), or 0 for an end of file typed
/// at the start of a line. Otherwise `VMIN` and `VTIME` decide (termios(3)): with both 0 it
/// returns what is there, maybe nothing; with `VMIN` 0 it waits up to `VTIME` tenths of a
/// second for a byte; with `VTIME` 0 it waits for `VMIN` bytes (or `count`, if fewer); with
/// both, for a first byte, then until `VMIN` bytes or `VTIME` passes with none arriving. The
/// calling thread becomes the console's foreground ([`console::Foreground`]).
///
/// As Linux: `EBADF` for a descriptor that is not open, `EFAULT` if the buffer does not end
/// within user space, then 0 for a `count` of 0; at most 0x7ffff000 bytes; if part of
/// the buffer is not the program's writable memory, the bytes before it, or `EFAULT` if there
/// are none (what was taken for the rest is lost). A signal due before input is taken
/// interrupts it, as Linux's `-ERESTARTSYS`: it is made again once the handler returns if the
/// handler's action has `SA_RESTART`, and fails with `EINTR` otherwise; if no handler starts,
/// it goes on. A signal that came with the input is delivered once the input is taken. Sets
/// the call's result; breaks when the process ends.
pub fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fd, buffer, count, ..] = context.syscall_arguments();
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: fd,
    };
    let result = loop {
        match read_or_wait(process, thread, fd, buffer, count)? {
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
    fd: u64,
    buffer: u64,
    count: u64,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    if !is_console(fd) {
        return ControlFlow::Continue(Some(Err(Errno::EBADF)));
    }
    if !ends_in_user_space::<P>(buffer, count) {
        return ControlFlow::Continue(Some(Err(Errno::EFAULT)));
    }
    let count = count.min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    become_foreground(process, thread);
    let settings = console::settings();
    let waited = if settings.is_canonical() {
        wait_for_input(process, thread, u64::MAX, console::input_ready)?
    } else {
        wait_for_bytes(process, thread, settings, count)?
    };
    ControlFlow::Continue(match waited {
        Waited::Signalled => None,
        Waited::Ready | Waited::TimedOut => Some(take_into(process, buffer, count)),
    })
}

fn wait_for_bytes<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    settings: Termios,
    count: u64,
) -> ControlFlow<(), Waited> {
    let (minimum, time) = settings.minimum_and_time();
    let wanted = usize::try_from(count).map_or(usize::from(minimum), |count| {
        count.min(usize::from(minimum))
    });
    let interval =
        || poll::deadline_in::<P>(u64::from(time) * TENTH_OF_A_SECOND_NS).unwrap_or(u64::MAX);
    match (minimum, time) {
        (0, 0) => ControlFlow::Continue(Waited::Ready),
        (0, _) => wait_for_input(process, thread, interval(), || {
            console::available_input() > 0
        }),
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

fn take_into<P: Platform>(process: &Process<P>, buffer: u64, count: u64) -> Result<u64, Errno> {
    let mut chunk = [0u8; READ_CHUNK];
    let mut copied = 0;
    while copied < count {
        let address = buffer + copied;
        let wanted = (count - copied)
            .min(READ_CHUNK as u64)
            .min(FRAME_SIZE - address % FRAME_SIZE) as usize;
        let taken = console::take_input(&mut chunk[..wanted]);
        if process
            .write_user_memory(address, &chunk[..taken.length])
            .is_err()
        {
            return match copied {
                0 => Err(Errno::EFAULT),
                _ => Ok(copied),
            };
        }
        copied += taken.length as u64;
        if taken.line_ended || taken.length < wanted {
            break;
        }
    }
    Ok(copied)
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

/// Makes `thread`, of `process`, the console's foreground, if it is not already.
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
    fn the_termios2_size_is_termios_and_two_speeds() {
        assert_eq!(TERMIOS2_SIZE, 44);
        assert_eq!(request::TCGETS2 >> 16 & 0x3fff, 44);
        assert_eq!(request::TCSETS2 >> 16 & 0x3fff, 44);
    }
}
