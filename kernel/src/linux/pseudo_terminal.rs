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
use crate::console::line_discipline::{
    LineDiscipline, Received, Taken, Termios, control, input, local, output,
};
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

/// How much room the program side's input has before the controlling side reports `POLLOUT`:
/// a paste's chunk (docs/ux/terminal-selection-v0.3.md section 1.3).
pub const WRITE_ROOM: usize = 1024;

/// The requests only the controlling side answers, each architecture's
/// numbers ([`crate::linux::abi::tty`]).
pub mod request {
    /// The terminal's number (an `unsigned int`): `_IOR('T', 0x30, 4 bytes)`.
    pub const TIOCGPTN: u32 = crate::linux::abi::tty::TIOCGPTN;
    /// Lock (non-zero) or unlock (0) the program side (an `int`): `_IOW('T', 0x31, 4 bytes)`.
    pub const TIOCSPTLCK: u32 = crate::linux::abi::tty::TIOCSPTLCK;
    /// Whether the program side is locked (an `int`): `_IOR('T', 0x39, 4 bytes)`.
    pub const TIOCGPTLCK: u32 = crate::linux::abi::tty::TIOCGPTLCK;
    /// Packet mode on (non-zero) or off (an `int`): `0x5420`.
    pub const TIOCPKT: u32 = crate::linux::abi::tty::TIOCPKT;
    /// Whether packet mode is on (an `int`): `_IOR('T', 0x38, 4 bytes)`.
    pub const TIOCGPKT: u32 = crate::linux::abi::tty::TIOCGPKT;
    /// Opens the program side and returns its descriptor, the argument being `open`'s flags:
    /// `_IO('T', 0x41)`.
    pub const TIOCGPTPEER: u32 = crate::linux::abi::tty::TIOCGPTPEER;
    /// Types the byte the argument points to into the program side's input: `0x5412`, answered
    /// by the program side.
    pub const TIOCSTI: u32 = crate::linux::abi::tty::TIOCSTI;
}

/// The status bytes a packet-mode read returns alone ([`request::TIOCPKT`]).
pub mod packet {
    /// Before data: what follows is output.
    pub const DATA: u8 = 0;
    /// The program side's input was flushed.
    pub const FLUSHREAD: u8 = 1;
    /// Output flow control (`IXON` with `^S` and `^Q`) was turned off.
    pub const NOSTOP: u8 = 0x10;
    /// Output flow control was turned on.
    pub const DOSTOP: u8 = 0x20;
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
    packet: bool,
    packet_status: u8,
}

impl State {
    fn is_hung_up(&self) -> bool {
        self.controlling_sides == 0
    }

    fn program_side_gone(&self) -> bool {
        self.program_side_opened && self.program_sides == 0
    }

    fn input_room(&self) -> usize {
        match self.discipline.drops_when_full() {
            true => CHUNK,
            false => self.discipline.room().saturating_sub(1),
        }
    }

    fn has_output(&self) -> bool {
        self.output.length > 0 || (self.packet && self.packet_status != 0)
    }

    fn report(&mut self, status: u8) {
        if self.packet {
            self.packet_status |= status;
        }
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
        packet: false,
        packet_status: 0,
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
    /// Linux does: the controlling side is readable when output (or, in packet mode, a status)
    /// waits, hung up once the program side was opened and closed again, and writable while the
    /// program side's input has [`WRITE_ROOM`] bytes of room, or while it is in canonical mode
    /// with no whole line unread, when writes never wait; the program side is
    /// readable when a read would not wait, writable with room for a line's end, and hung up
    /// once the controlling side closed.
    pub fn events(&self, requested: u16) -> u16 {
        let state = self.terminal.state.lock();
        let readable = requested & (event::IN | event::RDNORM);
        let writable = requested & (event::OUT | event::WRNORM);
        match self.side {
            Side::Controlling => {
                let mut events = 0;
                if state.discipline.room() >= WRITE_ROOM || state.discipline.drops_when_full() {
                    events |= writable;
                }
                if state.has_output() {
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

    fn receive_as_room_allows(&self, bytes: &[u8]) -> (usize, Received) {
        let mut state = self.state.lock();
        let State {
            discipline, output, ..
        } = &mut **state;
        let mut consumed = 0;
        let mut received = Received::default();
        while consumed < bytes.len() {
            let count = match discipline.room().saturating_sub(1) {
                0 if discipline.drops_when_full() => 1,
                room => room.min(bytes.len() - consumed),
            };
            if count == 0 {
                break;
            }
            let settings = discipline.settings();
            received |= discipline.receive(&bytes[consumed..consumed + count], &mut |echo| {
                output.push_processed(settings, echo);
            });
            consumed += count;
        }
        (consumed, received)
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
        {
            let mut state = self.state.lock();
            let flow = |termios: Termios| termios.input & input::IXON != 0;
            let before = flow(state.discipline.settings());
            state.discipline.set_settings(settings, flush);
            if before != flow(settings) {
                state.report(match flow(settings) {
                    true => packet::DOSTOP,
                    false => packet::NOSTOP,
                });
            }
            if flush {
                state.report(packet::FLUSHREAD);
            }
        }
        self.wake_list.wake();
    }

    fn flush_input(&self) {
        {
            let mut state = self.state.lock();
            state.discipline.flush();
            state.report(packet::FLUSHREAD);
        }
        self.wake_list.wake();
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
        let taken = self.state.lock().discipline.take(buffer);
        if taken.length > 0 {
            self.wake_list.wake();
        }
        taken
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
/// Packet mode (`TIOCPKT`) puts a status byte before each read on the controlling side;
/// `TIOCGPTPEER` opens the program side as a new descriptor, its flags only an access mode,
/// `O_NOCTTY`, `O_NONBLOCK` and `O_CLOEXEC` (`EINVAL` for any other); `TIOCSTI` types a byte
/// into the program side only through a description `file` open for reading, for a caller whose
/// controlling terminal it is, and fails with `EPERM` otherwise (Linux's answer to a caller it
/// does not let type), so no program can type into another session's terminal.
pub(super) fn ioctl<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    file: &OpenFile,
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
            request::TIOCSTI => return type_one::<P>(process, file, pseudo, argument),
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
        request::TIOCPKT => {
            let mut on = [0u8; 4];
            process
                .read_user_memory(argument, &mut on)
                .map_err(|_| Errno::EFAULT)?;
            let mut state = pseudo.state.lock();
            state.packet = i32::from_ne_bytes(on) != 0;
            state.packet_status = 0;
            Ok(0)
        }
        request::TIOCGPKT => write_int(u32::from(pseudo.state.lock().packet)),
        request::TIOCGPTPEER => open_peer(process, thread, pseudo, argument),
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
        let packet = terminal.state.lock().packet;
        let taken = match packet {
            true => take_packet_into(process, terminal, buffers, count),
            false => take_output_into(process, terminal, buffers, count),
        };
        if let Some(result) = taken {
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
            state.has_output() || state.program_side_gone()
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

fn take_packet_into<P: Platform>(
    process: &Process<P>,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
    count: u64,
) -> Option<Result<u64, Errno>> {
    let (base, length) = buffers
        .entries()
        .iter()
        .map(iovec)
        .find(|&(_, length)| length > 0)?;
    let room = length.min(count).min(CHUNK as u64) as usize;
    let mut chunk = [0u8; CHUNK];
    let (size, data) = {
        let mut state = terminal.state.lock();
        if state.packet_status != 0 {
            chunk[0] = core::mem::take(&mut state.packet_status);
            (1, false)
        } else if state.output.length > 0 {
            chunk[0] = packet::DATA;
            (1 + state.output.take(&mut chunk[1..room.max(1)]), true)
        } else {
            return None;
        }
    };
    if process.write_user_memory(base, &chunk[..size]).is_err() {
        let mut state = terminal.state.lock();
        match data {
            true => {
                state.output.put_back(&chunk[1..size]);
            }
            false => state.packet_status |= chunk[0],
        }
        return Some(Err(Errno::EFAULT));
    }
    terminal.wake_list.wake();
    Some(Ok(size as u64))
}

fn may_type_into(
    table: &Table,
    caller: u32,
    opened_for_reading: bool,
    terminal: Terminal,
) -> Result<(), Errno> {
    match opened_for_reading && table.controlling_terminal_of(caller) == Some(terminal) {
        true => Ok(()),
        false => Err(Errno::EPERM),
    }
}

fn type_one<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    terminal: &PseudoTerminal,
    argument: u64,
) -> Result<u64, Errno> {
    let caller = process.id().get();
    may_type_into(
        &TABLE.lock(),
        caller,
        file.is_readable(),
        terminal.terminal(),
    )?;
    let mut byte = [0u8; 1];
    process
        .read_user_memory(argument, &mut byte)
        .map_err(|_| Errno::EFAULT)?;
    let received = terminal.receive(&byte);
    signal_typed::<P>(terminal.terminal(), received);
    terminal.wake_list.wake();
    Ok(0)
}

const PEER_FLAGS: u32 =
    open_flag::ACCMODE | open_flag::NOCTTY | open_flag::NONBLOCK | open_flag::CLOEXEC;

fn peer_flags(argument: u64) -> Result<u32, Errno> {
    let flags = argument as u32;
    match flags & !PEER_FLAGS {
        0 => Ok(flags),
        _ => Err(Errno::EINVAL),
    }
}

fn open_peer<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    terminal: &PseudoTerminal,
    argument: u64,
) -> Result<u64, Errno> {
    let flags = peer_flags(argument)?;
    let kind = open_program_side(process, terminal.number, flags)?;
    let file = Arc::new(OpenFile::new(kind, flags));
    let limit = super::descriptors::open_file_limit(process);
    let close_on_exec = flags & open_flag::CLOEXEC != 0;
    let opened = thread
        .files()
        .lock()
        .open(Arc::clone(&file), close_on_exec, limit);
    drop(file);
    Ok(u64::from(opened?))
}

/// `write` or `writev` of `buffers` to side `end` of `file`, which the caller checked is open
/// for writing; sets the call's result and delivers the signals due after it. The controlling
/// side's bytes go through the line discipline as it has room, waiting for room (or failing
/// with `EAGAIN` when non-blocking) once it has none, and the signal characters among them
/// signal the foreground group. In canonical mode with no whole line unread they never wait,
/// as on Linux: a line longer than the input drops what does not fit, keeping room for the
/// line's end, and the signal and erase characters still get through; the program side's are processed for output into the ring,
/// waiting for room, and fail with `EIO` once the controlling side closed. With `TOSTOP`, a
/// background process writing to the program side of its controlling terminal gets `SIGTTOU`
/// as Linux does.
pub(super) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    end: &PseudoTerminalEnd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let terminal = &*end.terminal;
    let stops_background = terminal.settings().local & local::TOSTOP != 0;
    let access = match (end.side, stops_background) {
        (Side::Program, true) => job_control::terminal_access(
            process,
            thread,
            terminal.terminal(),
            super::signal::SIGTTOU,
        ),
        _ => job_control::Access::Allowed,
    };
    let result = match access {
        job_control::Access::Signalled => {
            return job_control::restart_after_signal(process, thread, context);
        }
        job_control::Access::Refused => Err(Errno::EIO),
        job_control::Access::Allowed => loop {
            let attempt = match end.side {
                Side::Controlling => {
                    type_into_or_wait::<P>(process, thread, file, terminal, buffers)?
                }
                Side::Program => write_output_or_wait(process, thread, file, terminal, buffers)?,
            };
            match attempt {
                Some(result) => break result,
                None => {
                    if signals::deliver_interrupting(process, thread, context, interrupted)? {
                        return ControlFlow::Continue(());
                    }
                }
            }
        },
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn type_into_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    terminal: &PseudoTerminal,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let total = buffers.total().min(MAX_RW_COUNT);
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    let mut received = Received::default();
    let mut failed = false;
    let mut full = false;
    'entries: for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(total - written);
        let mut done = 0;
        while done < length {
            let room = {
                let state = terminal.state.lock();
                if state.program_side_gone() && written == 0 {
                    return ControlFlow::Continue(Some(Err(Errno::EIO)));
                }
                state.input_room()
            };
            if room == 0 {
                if written > 0 || file.is_nonblocking() {
                    full = true;
                    break 'entries;
                }
                let ready = || {
                    let state = terminal.state.lock();
                    state.input_room() > 0 || state.program_side_gone()
                };
                let lists = [&terminal.wake_list];
                if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
                    return ControlFlow::Continue(None);
                }
                continue;
            }
            let size = (length - done).min(CHUNK as u64).min(room as u64) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                failed = true;
                break 'entries;
            }
            let (consumed, typed) = terminal.receive_as_room_allows(&chunk[..size]);
            received |= typed;
            done += consumed as u64;
            written += consumed as u64;
        }
    }
    signal_typed::<P>(terminal.terminal(), received);
    terminal.wake_list.wake();
    ControlFlow::Continue(Some(match (failed, full, written) {
        (true, _, 0) => Err(Errno::EFAULT),
        (false, true, 0) => Err(Errno::EAGAIN),
        _ => Ok(written),
    }))
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

    #[test]
    fn the_controlling_side_is_writable_only_with_a_chunk_of_room() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        let mut raw = SETTINGS;
        raw.local &= !(local::ICANON | local::ECHO);
        terminal.state.lock().discipline.set_settings(raw, false);
        assert_eq!(controlling_side.events(event::OUT), event::OUT);
        terminal.receive(&[b'x'; 1100]);
        assert_eq!(
            controlling_side.events(event::OUT),
            0,
            "under WRITE_ROOM left"
        );
        let mut taken = [0u8; 1100];
        assert_eq!(Line::take(&*terminal, &mut taken).length, 1100);
        assert_eq!(controlling_side.events(event::OUT), event::OUT);
    }

    #[test]
    fn a_paste_longer_than_a_canonical_line_never_blocks_its_end_or_an_interrupt() {
        use crate::console::line_discipline::CAPACITY;
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        let paste = alloc::vec![b'x'; 2 * 1024 + CHUNK];
        assert_eq!(terminal.receive_as_room_allows(&paste).0, paste.len());
        assert!(!terminal.is_readable());
        assert_eq!(controlling_side.events(event::OUT), event::OUT);
        assert_eq!(terminal.state.lock().input_room(), CHUNK);
        let (consumed, received) = terminal.receive_as_room_allows(b"\n");
        assert_eq!(consumed, 1);
        assert!(received.readable);
        let mut line = alloc::vec![0u8; CAPACITY + 1];
        let taken = Line::take(&*terminal, &mut line);
        assert_eq!(taken.length, CAPACITY);
        assert!(line[..CAPACITY - 1].iter().all(|&byte| byte == b'x'));
        assert_eq!(line[CAPACITY - 1], b'\n');
        assert_eq!(terminal.receive_as_room_allows(&paste).0, paste.len());
        let (consumed, received) = terminal.receive_as_room_allows(b"\x03");
        assert_eq!(consumed, 1);
        assert!(received.interrupt);
        assert_eq!(terminal.state.lock().discipline.room(), CAPACITY);
    }

    #[test]
    fn a_whole_line_waiting_makes_a_canonical_writer_wait_for_room() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        let mut typed = b"ab\n".to_vec();
        typed.extend_from_slice(&[b'c'; 4096]);
        let (consumed, _) = terminal.receive_as_room_allows(&typed);
        assert!(consumed < typed.len());
        assert_eq!(terminal.state.lock().input_room(), 0);
        assert_eq!(controlling_side.events(event::OUT), 0);
        let mut line = [0u8; 8];
        let taken = Line::take(&*terminal, &mut line).length;
        assert_eq!(&line[..taken], b"ab\n");
        assert_eq!(controlling_side.events(event::OUT), event::OUT);
    }

    #[test]
    fn packet_mode_reports_flushes_and_flow_control_as_status_bytes() {
        let terminal = new_terminal(no_hang_up).unwrap();
        let controlling_side = controlling(&terminal);
        Line::flush_input(&*terminal);
        assert!(!terminal.state.lock().has_output(), "not in packet mode");
        terminal.state.lock().packet = true;
        Line::flush_input(&*terminal);
        assert_eq!(terminal.state.lock().packet_status, packet::FLUSHREAD);
        assert_eq!(controlling_side.events(event::IN), event::IN);
        terminal.state.lock().packet_status = 0;
        let mut no_flow = SETTINGS;
        no_flow.input &= !input::IXON;
        Line::set_settings::<crate::tests::TestPlatform>(&*terminal, no_flow, false);
        assert_eq!(terminal.state.lock().packet_status, packet::NOSTOP);
        terminal.state.lock().packet_status = 0;
        Line::set_settings::<crate::tests::TestPlatform>(&*terminal, SETTINGS, false);
        assert_eq!(terminal.state.lock().packet_status, packet::DOSTOP);
    }

    #[test]
    fn only_a_process_of_the_terminal_s_own_session_types_into_it() {
        let mut table = Table::new();
        table.add(140, None).unwrap();
        table.add(150, None).unwrap();
        let terminal = new_terminal(no_hang_up).unwrap();
        let _controlling_side = controlling(&terminal);
        terminal
            .become_controlling_terminal(&mut table, 140, false)
            .unwrap();
        assert_eq!(
            may_type_into(&table, 140, true, terminal.terminal()),
            Ok(())
        );
        assert_eq!(
            may_type_into(&table, 140, false, terminal.terminal()),
            Err(Errno::EPERM),
            "a description open only for writing"
        );
        assert_eq!(
            may_type_into(&table, 150, true, terminal.terminal()),
            Err(Errno::EPERM)
        );
        let other = new_terminal(no_hang_up).unwrap();
        let _other_controlling_side = controlling(&other);
        assert_eq!(
            may_type_into(&table, 140, true, other.terminal()),
            Err(Errno::EPERM)
        );
    }

    #[test]
    fn tiocgptpeer_takes_an_access_mode_noctty_nonblock_and_cloexec_only() {
        let allowed =
            open_flag::RDWR | open_flag::NOCTTY | open_flag::NONBLOCK | open_flag::CLOEXEC;
        assert_eq!(peer_flags(u64::from(allowed)), Ok(allowed));
        assert_eq!(peer_flags(u64::from(open_flag::RDONLY)), Ok(0));
        for refused in [
            open_flag::CREAT,
            open_flag::APPEND,
            open_flag::TRUNC,
            open_flag::PATH,
        ] {
            assert_eq!(
                peer_flags(u64::from(open_flag::RDWR | refused)),
                Err(Errno::EINVAL)
            );
        }
    }
}
