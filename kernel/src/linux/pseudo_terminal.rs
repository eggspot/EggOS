//! Pseudo-terminals (D187): `/dev/ptmx` and `/dev/pts/<n>`, Linux's Unix 98 pseudo-terminals,
//! on which the terminal program runs a shell in a window as the console runs one on the serial
//! line.
//!
//! Opening `/dev/ptmx` makes a pseudo-terminal and returns its controlling side, the one a
//! terminal emulator holds; its number `n` (`TIOCGPTN`) names the program side, `/dev/pts/<n>`,
//! which opens once the controlling side unlocked it (`TIOCSPTLCK` with 0, what `unlockpt`
//! does). What the controlling side writes goes through the program side's line discipline
//! ([`crate::console::line_discipline`], as the console's typed input does: editing, echo and the
//! signal characters, which signal the terminal's foreground group); what the program side writes
//! is processed for output (`OPOST` with `ONLCR` turns `\n` into `\r\n`) into a ring of
//! [`OUTPUT_CAPACITY`] bytes that the controlling side reads, with the echo. Both sides take the
//! terminal requests (settings, window size, `FIONREAD`); a window size set on either sends
//! `SIGWINCH` to the foreground group, as the console's does (D181). The program side is a
//! terminal like the console for job control: a session leader opening it without `O_NOCTTY`, or
//! asking with `TIOCSCTTY`, makes it the session's controlling terminal, and a background group
//! reading it or changing its settings gets `SIGTTIN` or `SIGTTOU`.
//!
//! When the last description of the controlling side closes, the terminal hangs up: its
//! foreground group and its session's leader get `SIGHUP` then `SIGCONT`, the session loses it,
//! reads on the program side return the end of the file and writes fail with `EIO`. When the
//! program side was opened and its last description closes, the controlling side's reads fail
//! with `EIO` once the ring is empty, as on Linux. A terminal's number is free again once both
//! sides are closed; at most [`MAX_PSEUDO_TERMINALS`] exist at once. `/dev/pts` lists the
//! terminals whose controlling side is open.
//!
//! Each terminal's state is behind one leaf lock; user memory is copied through a buffer on the
//! kernel stack, never under it. Waiters, blocked readers and writers and `poll`, `select` and
//! `epoll`, are on the terminal's wake list, which every transfer, change of settings and close
//! wakes.

use super::descriptors::{FileKind, OpenFile, open_flag};
use super::job_control;
use super::poll::{self, WakeList, event};
use super::process_table::{TABLE, Table, Terminal};
use super::signals::{self, Interrupted};
use super::terminal::{self, Line, WINSIZE_SIZE, Waited};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec};
use crate::console::TerminalSignal;
use crate::console::line_discipline::{LineDiscipline, Received, Taken, Termios, control, output};
use crate::process::Process;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The most pseudo-terminals that exist at once.
pub const MAX_PSEUDO_TERMINALS: u32 = 64;

/// How many bytes the program side's output ring holds before its writers wait.
pub const OUTPUT_CAPACITY: usize = 16 * 1024;

/// The speed a pseudo-terminal reports, in baud: Linux's default for one.
pub const SPEED: u32 = 38_400;

/// The requests only the controlling side answers, the generic numbers, which AArch64, RISC-V
/// and x86_64 share.
pub mod request {
    /// The terminal's number (an `unsigned int`): `_IOR('T', 0x30, 4 bytes)`.
    pub const TIOCGPTN: u32 = 0x8004_5430;
    /// Lock (non-zero) or unlock (0) the program side (an `int`): `_IOW('T', 0x31, 4 bytes)`.
    pub const TIOCSPTLCK: u32 = 0x4004_5431;
    /// Whether the program side is locked (an `int`): `_IOR('T', 0x39, 4 bytes)`.
    pub const TIOCGPTLCK: u32 = 0x8004_5439;
}

const B38400: u32 = 0o17;

/// A new pseudo-terminal's settings, Linux's for one: the serial console's but at 38400 baud and
/// without `CLOCAL`.
pub const SETTINGS: Termios = Termios {
    control: B38400 | control::CS8 | control::CREAD | control::HUPCL,
    ..Termios::SERIAL_CONSOLE
};

const CHUNK: usize = 512;

#[derive(Debug)]
struct Output {
    bytes: Vec<u8>,
    start: usize,
    length: usize,
}

impl Output {
    fn room(&self) -> usize {
        OUTPUT_CAPACITY - self.length
    }

    fn push(&mut self, bytes: &[u8]) -> usize {
        let count = bytes.len().min(self.room());
        let end = (self.start + self.length) % OUTPUT_CAPACITY;
        let first = count.min(OUTPUT_CAPACITY - end);
        self.bytes[end..end + first].copy_from_slice(&bytes[..first]);
        self.bytes[..count - first].copy_from_slice(&bytes[first..count]);
        self.length += count;
        count
    }

    fn take(&mut self, buffer: &mut [u8]) -> usize {
        let count = buffer.len().min(self.length);
        let first = count.min(OUTPUT_CAPACITY - self.start);
        buffer[..first].copy_from_slice(&self.bytes[self.start..self.start + first]);
        buffer[first..count].copy_from_slice(&self.bytes[..count - first]);
        self.start = (self.start + count) % OUTPUT_CAPACITY;
        self.length -= count;
        if self.length == 0 {
            self.start = 0;
        }
        count
    }

    fn put_back(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > self.room() {
            return false;
        }
        self.start = (self.start + OUTPUT_CAPACITY - bytes.len()) % OUTPUT_CAPACITY;
        let first = bytes.len().min(OUTPUT_CAPACITY - self.start);
        self.bytes[self.start..self.start + first].copy_from_slice(&bytes[..first]);
        self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.length += bytes.len();
        true
    }

    fn push_processed(&mut self, settings: Termios, bytes: &[u8]) -> usize {
        let newline_to_crlf = output::OPOST | output::ONLCR;
        if settings.output & newline_to_crlf != newline_to_crlf {
            return self.push(bytes);
        }
        let mut consumed = 0;
        while consumed < bytes.len() {
            let rest = &bytes[consumed..];
            let line = rest
                .iter()
                .position(|&byte| byte == b'\n')
                .unwrap_or(rest.len());
            if line > 0 {
                let pushed = self.push(&rest[..line]);
                consumed += pushed;
                if pushed < line {
                    break;
                }
                continue;
            }
            if self.room() < 2 {
                break;
            }
            self.push(b"\r\n");
            consumed += 1;
        }
        consumed
    }
}

#[derive(Debug)]
struct State {
    discipline: LineDiscipline,
    output: Output,
    window_size: [u8; WINSIZE_SIZE],
    locked: bool,
    controlling_sides: u32,
    program_sides: u32,
    program_side_opened: bool,
}

impl State {
    fn is_hung_up(&self) -> bool {
        self.controlling_sides == 0
    }

    fn program_side_gone(&self) -> bool {
        self.program_side_opened && self.program_sides == 0
    }
}

/// A pseudo-terminal: see the module documentation.
pub struct PseudoTerminal {
    number: u32,
    state: LeafSpinLock<Box<State>>,
    wake_list: Arc<WakeList>,
    hang_up: fn(Terminal),
}

impl core::fmt::Debug for PseudoTerminal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("PseudoTerminal")
            .field(&self.number)
            .finish()
    }
}

struct Numbers {
    terminals: Vec<Weak<PseudoTerminal>>,
}

static NUMBERS: LeafSpinLock<Option<Box<Numbers>>> = LeafSpinLock::new_leaf(None);

fn new_state() -> Result<Box<State>, Errno> {
    let size = OUTPUT_CAPACITY + core::mem::size_of::<State>();
    if !crate::heap::may_grow_for_user(size) {
        return Err(Errno::ENOMEM);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(OUTPUT_CAPACITY)
        .map_err(|_| Errno::ENOMEM)?;
    bytes.resize(OUTPUT_CAPACITY, 0);
    let mut discipline = LineDiscipline::new();
    discipline.set_settings(SETTINGS, true);
    Ok(Box::new(State {
        discipline,
        output: Output {
            bytes,
            start: 0,
            length: 0,
        },
        window_size: [0; WINSIZE_SIZE],
        locked: true,
        controlling_sides: 1,
        program_sides: 0,
        program_side_opened: false,
    }))
}

fn new_terminal(hang_up: fn(Terminal)) -> Result<Arc<PseudoTerminal>, Errno> {
    let state = new_state()?;
    let wake_list = WakeList::shared();
    let mut numbers = NUMBERS.lock();
    let numbers = numbers.get_or_insert_with(|| {
        Box::new(Numbers {
            terminals: Vec::new(),
        })
    });
    let free = numbers
        .terminals
        .iter()
        .position(|terminal| terminal.strong_count() == 0);
    let number = match free {
        Some(number) => number,
        None if numbers.terminals.len() < MAX_PSEUDO_TERMINALS as usize => {
            numbers
                .terminals
                .try_reserve(1)
                .map_err(|_| Errno::ENOMEM)?;
            numbers.terminals.push(Weak::new());
            numbers.terminals.len() - 1
        }
        None => return Err(Errno::ENOSPC),
    };
    let terminal = Arc::new(PseudoTerminal {
        number: number as u32,
        state: LeafSpinLock::new_leaf(state),
        wake_list,
        hang_up,
    });
    numbers.terminals[number] = Arc::downgrade(&terminal);
    Ok(terminal)
}

fn find(number: u32) -> Option<Arc<PseudoTerminal>> {
    NUMBERS
        .lock()
        .as_ref()?
        .terminals
        .get(usize::try_from(number).ok()?)?
        .upgrade()
}

/// Whether pseudo-terminal `number` is listed in `/dev/pts`: its controlling side is open.
pub fn is_listed(number: u32) -> bool {
    find(number).is_some_and(|terminal| !terminal.state.lock().is_hung_up())
}

/// The number of the `position`th pseudo-terminal `/dev/pts` lists, in increasing order.
pub fn listed(position: usize) -> Option<u32> {
    let count = NUMBERS
        .lock()
        .as_ref()
        .map_or(0, |numbers| numbers.terminals.len());
    (0..count as u32)
        .filter(|&number| is_listed(number))
        .nth(position)
}

/// Which side of a pseudo-terminal a description is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The controlling side, `/dev/ptmx`'s, which a terminal emulator holds.
    Controlling,
    /// The program side, `/dev/pts/<n>`, a terminal for the programs it runs.
    Program,
}

/// One side of a pseudo-terminal, held by the open file descriptions of that side: the last
/// one dropped closes the side.
pub struct PseudoTerminalEnd {
    terminal: Arc<PseudoTerminal>,
    side: Side,
}

impl core::fmt::Debug for PseudoTerminalEnd {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PseudoTerminalEnd")
            .field("terminal", &self.terminal)
            .field("side", &self.side)
            .finish()
    }
}

impl PseudoTerminalEnd {
    /// The terminal's number, `n` of `/dev/pts/<n>`.
    pub fn number(&self) -> u32 {
        self.terminal.number
    }

    /// Which side it is.
    pub fn side(&self) -> Side {
        self.side
    }

    /// The threads and `epoll` instances that wait for the terminal, either side.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.terminal.wake_list
    }

    /// The `poll` events of this side among `requested`, with `POLLHUP` always reported as
    /// Linux does: the controlling side is readable when output waits and hung up once the
    /// program side was opened and closed again, and always writable; the program side is
    /// readable when a read would not wait, writable with room for a line's end, and hung up
    /// once the controlling side closed.
    pub fn events(&self, requested: u16) -> u16 {
        let state = self.terminal.state.lock();
        let readable = requested & (event::IN | event::RDNORM);
        let writable = requested & (event::OUT | event::WRNORM);
        match self.side {
            Side::Controlling => {
                let mut events = writable;
                if state.output.length > 0 {
                    events |= readable;
                }
                if state.program_side_gone() {
                    events |= event::HUP;
                }
                events
            }
            Side::Program if state.is_hung_up() => readable | event::HUP,
            Side::Program => {
                let mut events = 0;
                if state.discipline.is_readable() {
                    events |= readable;
                }
                if state.output.room() >= 2 {
                    events |= writable;
                }
                events
            }
        }
    }
}

impl PartialEq for PseudoTerminalEnd {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.terminal, &other.terminal) && self.side == other.side
    }
}

impl Eq for PseudoTerminalEnd {}

impl Drop for PseudoTerminalEnd {
    fn drop(&mut self) {
        let hung_up = {
            let mut state = self.terminal.state.lock();
            match self.side {
                Side::Controlling => {
                    state.controlling_sides -= 1;
                    state.is_hung_up()
                }
                Side::Program => {
                    state.program_sides -= 1;
                    false
                }
            }
        };
        if hung_up {
            (self.terminal.hang_up)(Terminal::Pseudo(self.terminal.number));
        }
        self.terminal.wake_list.wake();
    }
}

fn hang_up<P: Platform>(terminal: Terminal) {
    job_control::hang_up::<P>(terminal);
}

/// `open` of `/dev/ptmx`: a new pseudo-terminal, its program side locked, and its controlling
/// side. `ENOSPC` once [`MAX_PSEUDO_TERMINALS`] exist, `ENOMEM` without memory.
pub fn open_controlling_side<P: Platform>() -> Result<FileKind, Errno> {
    let terminal = new_terminal(hang_up::<P>)?;
    TABLE.lock().release(terminal.terminal());
    Ok(FileKind::PseudoTerminal(PseudoTerminalEnd {
        terminal,
        side: Side::Controlling,
    }))
}

/// `open` of `/dev/pts/<number>` by `process` with `flags`: the program side, which becomes the
/// caller's controlling terminal if it leads a session that has none, the terminal is no
/// session's and `flags` lacks `O_NOCTTY` (and it has not hung up meanwhile). As Linux: `ENXIO`
/// for no such terminal, `EIO` while it is locked or once its controlling side closed.
pub fn open_program_side<P: Platform>(
    process: &Process<P>,
    number: u32,
    flags: u32,
) -> Result<FileKind, Errno> {
    let terminal = find(number).ok_or(Errno::ENXIO)?;
    {
        let mut state = terminal.state.lock();
        if state.locked || state.is_hung_up() {
            return Err(Errno::EIO);
        }
        state.program_sides += 1;
        state.program_side_opened = true;
    }
    let end = PseudoTerminalEnd {
        terminal,
        side: Side::Program,
    };
    if flags & open_flag::NOCTTY == 0 {
        end.terminal
            .give_if_free(&mut TABLE.lock(), process.id().get());
    }
    Ok(FileKind::PseudoTerminal(end))
}

impl PseudoTerminal {
    fn give_if_free(&self, table: &mut Table, caller: u32) {
        if !self.is_hung_up() {
            table.give_if_free(self.terminal(), caller);
        }
    }

    fn become_controlling_terminal(
        &self,
        table: &mut Table,
        caller: u32,
        steal: bool,
    ) -> Result<(), Errno> {
        if self.is_hung_up() {
            return Err(Errno::EIO);
        }
        table.take(self.terminal(), caller, steal)
    }

    fn receive(&self, bytes: &[u8]) -> Received {
        let mut state = self.state.lock();
        let State {
            discipline, output, ..
        } = &mut **state;
        let settings = discipline.settings();
        discipline.receive(bytes, &mut |echo| {
            output.push_processed(settings, echo);
        })
    }

    fn take_output(&self, buffer: &mut [u8]) -> usize {
        self.state.lock().output.take(buffer)
    }
}

impl Line for PseudoTerminal {
    fn terminal(&self) -> Terminal {
        Terminal::Pseudo(self.number)
    }

    fn speed(&self) -> u32 {
        SPEED
    }

    fn settings(&self) -> Termios {
        self.state.lock().discipline.settings()
    }

    fn set_settings<P: Platform>(&self, settings: Termios, flush: bool) {
        self.state.lock().discipline.set_settings(settings, flush);
        self.wake_list.wake();
    }

    fn flush_input(&self) {
        self.state.lock().discipline.flush();
    }

    fn is_readable(&self) -> bool {
        self.state.lock().discipline.is_readable()
    }

    fn available(&self) -> usize {
        self.state.lock().discipline.available()
    }

    fn pending(&self) -> usize {
        self.state.lock().discipline.pending()
    }

    fn take(&self, buffer: &mut [u8]) -> Taken {
        self.state.lock().discipline.take(buffer)
    }

    fn is_hung_up(&self) -> bool {
        self.state.lock().is_hung_up()
    }

    fn window_size(&self) -> [u8; WINSIZE_SIZE] {
        self.state.lock().window_size
    }

    fn replace_window_size(&self, size: [u8; WINSIZE_SIZE]) -> [u8; WINSIZE_SIZE] {
        core::mem::replace(&mut self.state.lock().window_size, size)
    }

    fn wait<P: Platform>(
        &self,
        process: &Process<P>,
        thread: &mut LinuxThread,
        deadline: u64,
        ready: impl Fn() -> bool,
    ) -> ControlFlow<(), Waited> {
        poll::wait_for(process, thread, &[&self.wake_list], deadline, ready)
    }
}

/// `ioctl(fd, request, argument)` on side `end` of a pseudo-terminal, `fd` of `process`, of
/// which `thread` is the calling thread. The controlling side answers [`request`]'s requests,
/// `FIONREAD` with the output waiting, and `TIOCGPGRP` and `TIOCGSID` whoever asks (`ENOTTY`
/// while no session has the terminal), refuses `TIOCSCTTY` and `TIOCSPGRP` (`ENOTTY`); both
/// sides answer the terminal requests of [`terminal::ioctl`], the program side as the console
/// does, `SIGTTOU` included, but once hung up its job-control requests fail as Linux's do
/// (`EIO`, `ENOTTY` for `TIOCSPGRP`), `TIOCSCTTY` checked under the process table's lock.
pub(super) fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    end: &PseudoTerminalEnd,
    request: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let pseudo = &*end.terminal;
    let write_int = |value: u32| {
        process
            .write_user_memory(argument, &value.to_ne_bytes())
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT)
    };
    if end.side == Side::Program {
        match request as u32 {
            job_control::request::TIOCSCTTY => {
                let steal = job_control::may_steal(process, argument);
                let caller = process.id().get();
                return pseudo
                    .become_controlling_terminal(&mut TABLE.lock(), caller, steal)
                    .map(|()| 0);
            }
            job_control::request::TIOCSPGRP if pseudo.is_hung_up() => return Err(Errno::ENOTTY),
            job_control::request::TIOCGPGRP | job_control::request::TIOCGSID
                if pseudo.is_hung_up() =>
            {
                return Err(Errno::EIO);
            }
            _ => {}
        }
        if terminal::changes_the_terminal(request) {
            let signal = super::signal::SIGTTOU;
            match job_control::terminal_access(process, thread, pseudo.terminal(), signal) {
                job_control::Access::Allowed => {}
                job_control::Access::Refused => return Err(Errno::EIO),
                job_control::Access::Signalled => return Err(job_control::SIGNALLED),
            }
        }
        return terminal::ioctl(process, pseudo, request, argument);
    }
    match request as u32 {
        request::TIOCGPTN => write_int(pseudo.number),
        request::TIOCSPTLCK => {
            let mut lock = [0u8; 4];
            process
                .read_user_memory(argument, &mut lock)
                .map_err(|_| Errno::EFAULT)?;
            pseudo.state.lock().locked = i32::from_ne_bytes(lock) != 0;
            Ok(0)
        }
        request::TIOCGPTLCK => write_int(u32::from(pseudo.state.lock().locked)),
        terminal::request::FIONREAD => {
            let waiting = pseudo.state.lock().output.length;
            write_int(u32::try_from(waiting).unwrap_or(u32::MAX))
        }
        job_control::request::TIOCGPGRP | job_control::request::TIOCGSID => {
            let controlled = TABLE.lock().session_and_foreground(pseudo.terminal());
            let (session, foreground) = controlled.ok_or(Errno::ENOTTY)?;
            match request as u32 == job_control::request::TIOCGSID {
                true => write_int(session),
                false => write_int(foreground),
            }
        }
        job_control::request::TIOCSCTTY | job_control::request::TIOCSPGRP => Err(Errno::ENOTTY),
        _ => terminal::ioctl(process, pseudo, request, argument),
    }
}

/// `read` or `readv` of `buffers` from side `end` of `file`, which the caller checked is open
/// for reading; sets the call's result and delivers the signals due after it. The program side
/// reads as the console does ([`terminal::read`]); the controlling side takes the output,
/// waiting while there is none, and fails with `EIO` once the program side was opened and
/// closed again.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    end: &PseudoTerminalEnd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    if end.side == Side::Program {
        return terminal::read(process, thread, context, file, &*end.terminal, buffers);
    }
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match read_output_or_wait(process, thread, file, &end.terminal, buffers)? {
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

fn read_output_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let count = buffers.total().min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    loop {
        if let Some(result) = take_output_into(process, terminal, buffers, count) {
            return ControlFlow::Continue(Some(result));
        }
        if terminal.state.lock().program_side_gone() {
            return ControlFlow::Continue(Some(Err(Errno::EIO)));
        }
        if file.is_nonblocking() {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let ready = || {
            let state = terminal.state.lock();
            state.output.length > 0 || state.program_side_gone()
        };
        let lists = [&terminal.wake_list];
        if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
    }
}

fn take_output_into<P: Platform>(
    process: &Process<P>,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
    count: u64,
) -> Option<Result<u64, Errno>> {
    let mut chunk = [0u8; CHUNK];
    let mut copied = 0;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(count - copied);
        let mut done = 0;
        while done < length {
            let wanted = (length - done).min(CHUNK as u64) as usize;
            let taken = terminal.take_output(&mut chunk[..wanted]);
            if taken == 0 {
                return match copied {
                    0 => None,
                    _ => Some(Ok(copied)),
                };
            }
            if process
                .write_user_memory(base + done, &chunk[..taken])
                .is_err()
            {
                terminal.state.lock().output.put_back(&chunk[..taken]);
                return Some(match copied {
                    0 => Err(Errno::EFAULT),
                    _ => Ok(copied),
                });
            }
            terminal.wake_list.wake();
            done += taken as u64;
            copied += taken as u64;
        }
        if copied == count {
            break;
        }
    }
    Some(Ok(copied))
}

/// `write` or `writev` of `buffers` to side `end` of `file`, which the caller checked is open
/// for writing; sets the call's result and delivers the signals due after it. The controlling
/// side's bytes go through the line discipline at once, never waiting (bytes past its capacity
/// are dropped, as a serial line's are), and the signal characters among them signal the
/// foreground group; the program side's are processed for output into the ring, waiting for
/// room, and fail with `EIO` once the controlling side closed.
pub(super) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    end: &PseudoTerminalEnd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let result = match end.side {
        Side::Controlling => type_into::<P>(process, &end.terminal, buffers),
        Side::Program => {
            let interrupted = Interrupted::RestartsIfAsked {
                number: context.syscall_number(),
                first_argument: context.syscall_arguments()[0],
            };
            loop {
                match write_output_or_wait(process, thread, file, &end.terminal, buffers)? {
                    Some(result) => break result,
                    None => {
                        if signals::deliver_interrupting(process, thread, context, interrupted)? {
                            return ControlFlow::Continue(());
                        }
                    }
                }
            }
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn type_into<P: Platform>(
    process: &Process<P>,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
) -> Result<u64, Errno> {
    let total = buffers.total().min(MAX_RW_COUNT);
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    let mut received = Received::default();
    let mut failed = false;
    'entries: for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(total - written);
        let mut done = 0;
        while done < length {
            let size = (length - done).min(CHUNK as u64) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                failed = true;
                break 'entries;
            }
            received |= terminal.receive(&chunk[..size]);
            done += size as u64;
            written += size as u64;
        }
    }
    signal_typed::<P>(terminal.terminal(), received);
    terminal.wake_list.wake();
    match (failed, written) {
        (true, 0) => Err(Errno::EFAULT),
        _ => Ok(written),
    }
}

fn signal_typed<P: Platform>(terminal: Terminal, received: Received) {
    for (typed, signal) in [
        (received.interrupt, TerminalSignal::Interrupt),
        (received.quit, TerminalSignal::Quit),
        (received.suspend, TerminalSignal::Suspend),
    ] {
        if typed {
            job_control::signal_foreground::<P>(terminal, signal);
        }
    }
}

fn write_output_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let total = buffers.total().min(MAX_RW_COUNT);
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    let partial = |written: u64, error: Errno| match written {
        0 => Err(error),
        _ => Ok(written),
    };
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(total - written);
        let mut done = 0;
        while done < length {
            let size = (length - done).min(CHUNK as u64) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                return ControlFlow::Continue(Some(partial(written, Errno::EFAULT)));
            }
            let mut sent = 0;
            while sent < size {
                let pushed = {
                    let mut state = terminal.state.lock();
                    if state.is_hung_up() {
                        return ControlFlow::Continue(Some(partial(written, Errno::EIO)));
                    }
                    let settings = state.discipline.settings();
                    state.output.push_processed(settings, &chunk[sent..size])
                };
                if pushed > 0 {
                    terminal.wake_list.wake();
                    sent += pushed;
                    written += pushed as u64;
                    continue;
                }
                if file.is_nonblocking() {
                    return ControlFlow::Continue(Some(partial(written, Errno::EAGAIN)));
                }
                let ready = || {
                    let state = terminal.state.lock();
                    state.is_hung_up() || state.output.room() >= 2
                };
                let lists = [&terminal.wake_list];
                if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
                    return ControlFlow::Continue(match written {
                        0 => None,
                        _ => Some(Ok(written)),
                    });
                }
            }
            done += size as u64;
        }
    }
    ControlFlow::Continue(Some(Ok(written)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::line_discipline::local;

    fn no_hang_up(_terminal: Terminal) {}

    static HUNG_UP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);

    fn record_hang_up(terminal: Terminal) {
        if let Terminal::Pseudo(number) = terminal {
            HUNG_UP.store(number, core::sync::atomic::Ordering::Relaxed);
        }
    }

    fn controlling(terminal: &Arc<PseudoTerminal>) -> PseudoTerminalEnd {
        PseudoTerminalEnd {
            terminal: Arc::clone(terminal),
            side: Side::Controlling,
        }
    }

    fn program(terminal: &Arc<PseudoTerminal>) -> PseudoTerminalEnd {
        let mut state = terminal.state.lock();
        state.program_sides += 1;
        state.program_side_opened = true;
        drop(state);
        PseudoTerminalEnd {
            terminal: Arc::clone(terminal),
            side: Side::Program,
        }
    }

    fn output_of(terminal: &PseudoTerminal) -> Vec<u8> {
        let mut buffer = alloc::vec![0u8; OUTPUT_CAPACITY];
        let length = terminal.take_output(&mut buffer);
        buffer.truncate(length);
        buffer
    }

    #[test]
    fn typed_lines_are_edited_echoed_and_read_by_the_program_side() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let _controlling = controlling(&terminal);
        let received = terminal.receive(b"lx\x7fs\r");
        assert!(received.readable && !received.interrupt);
        assert_eq!(output_of(&terminal), b"lx\x08 \x08s\r\n");
        assert!(terminal.is_readable());
        let mut line = [0u8; 8];
        let taken = Line::take(&*terminal, &mut line);
        assert_eq!(&line[..taken.length], b"ls\n");
        assert!(terminal.receive(b"\x03").interrupt);
        assert_eq!(terminal.settings(), SETTINGS);
        assert_eq!(terminal.speed(), 38_400);
    }

    #[test]
    fn program_output_turns_newlines_into_crlf_unless_output_processing_is_off() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let _controlling = controlling(&terminal);
        {
            let mut state = terminal.state.lock();
            let settings = state.discipline.settings();
            assert_eq!(state.output.push_processed(settings, b"a\nb\n\n"), 5);
        }
        assert_eq!(output_of(&terminal), b"a\r\nb\r\n\r\n");
        let mut raw = SETTINGS;
        raw.output &= !output::OPOST;
        raw.local &= !local::ICANON;
        terminal.state.lock().discipline.set_settings(raw, false);
        let pushed = terminal.state.lock().output.push_processed(raw, b"x\ny");
        assert_eq!(pushed, 3);
        assert_eq!(output_of(&terminal), b"x\ny");
    }

    #[test]
    fn bytes_put_back_are_taken_again_first_even_across_the_ring_s_end() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let _controlling = controlling(&terminal);
        let mut state = terminal.state.lock();
        let filler = alloc::vec![b'.'; OUTPUT_CAPACITY - 2];
        assert_eq!(state.output.push(&filler), OUTPUT_CAPACITY - 2);
        let mut taken = alloc::vec![0u8; OUTPUT_CAPACITY - 3];
        assert_eq!(state.output.take(&mut taken), OUTPUT_CAPACITY - 3);
        assert_eq!(state.output.push(b"abcd"), 4);
        let mut chunk = [0u8; 4];
        assert_eq!(state.output.take(&mut chunk), 4);
        assert_eq!(&chunk, b".abc");
        assert!(state.output.put_back(&chunk));
        let mut again = [0u8; 8];
        assert_eq!(state.output.take(&mut again), 5);
        assert_eq!(&again[..5], b".abcd");
        let full = alloc::vec![b'x'; OUTPUT_CAPACITY];
        assert_eq!(state.output.push(&full), OUTPUT_CAPACITY);
        assert!(!state.output.put_back(b"y"));
    }

    #[test]
    fn a_full_ring_takes_only_what_fits_and_never_half_a_line_end() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let _controlling = controlling(&terminal);
        let mut state = terminal.state.lock();
        let filler = alloc::vec![b'.'; OUTPUT_CAPACITY - 1];
        assert_eq!(state.output.push(&filler), OUTPUT_CAPACITY - 1);
        assert_eq!(state.output.push_processed(SETTINGS, b"\n"), 0);
        assert_eq!(state.output.push_processed(SETTINGS, b"zz"), 1);
        assert_eq!(state.output.room(), 0);
    }

    #[test]
    fn closing_the_controlling_side_hangs_up_and_frees_the_number_once_both_sides_close() {
        let terminal = new_terminal(record_hang_up).unwrap();
        let number = terminal.number;
        let controlling_side = controlling(&terminal);
        let program_side = program(&terminal);
        assert!(is_listed(number));
        assert!(!terminal.is_hung_up());
        assert_eq!(program_side.events(u16::MAX) & event::HUP, 0);
        drop(controlling_side);
        assert_eq!(HUNG_UP.load(core::sync::atomic::Ordering::Relaxed), number);
        assert!(terminal.is_hung_up());
        assert!(!is_listed(number));
        assert_eq!(
            program_side.events(event::IN | event::OUT),
            event::IN | event::HUP
        );
        drop(program_side);
        let freed = Arc::downgrade(&terminal);
        drop(terminal);
        assert!(freed.upgrade().is_none(), "nothing holds a closed terminal");
    }

    #[test]
    fn the_controlling_side_sees_output_and_the_program_side_s_close() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        let all = event::IN | event::OUT;
        assert_eq!(controlling_side.events(all), event::OUT);
        let program_side = program(&terminal);
        terminal.state.lock().output.push(b"$ ");
        assert_eq!(controlling_side.events(all), all);
        drop(program_side);
        assert_eq!(controlling_side.events(all), all | event::HUP);
        assert!(terminal.state.lock().program_side_gone());
        drop(controlling_side);
        assert!(terminal.is_hung_up());
    }

    #[test]
    fn a_hung_up_terminal_becomes_no_session_s_controlling_terminal() {
        let mut table = Table::new();
        table.add(140, None).unwrap();
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        let program_side = program(&terminal);
        drop(controlling_side);
        assert_eq!(
            terminal.become_controlling_terminal(&mut table, 140, false),
            Err(Errno::EIO)
        );
        terminal.give_if_free(&mut table, 140);
        assert_eq!(table.session_and_foreground(terminal.terminal()), None);
        assert_eq!(table.controlling_terminal_of(140), None);
        drop(program_side);
        let live = new_terminal(no_hang_up).unwrap();
        let _live_controlling_side = controlling(&live);
        assert_eq!(
            live.become_controlling_terminal(&mut table, 140, false),
            Ok(())
        );
        assert_eq!(
            table.session_and_foreground(live.terminal()),
            Some((140, 140))
        );
    }
}
