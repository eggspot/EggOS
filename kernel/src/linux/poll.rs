//! Waiting for descriptors (M6, D61, D71): `poll` (x86_64 only), `ppoll`, `select` (x86_64
//! only) and `pselect6`, the wake lists that every file a program can wait on keeps
//! ([`WakeList`]), and the calls of the event files: `epoll` ([`super::epoll`]), `eventfd`
//! ([`super::eventfd`]), `timerfd` ([`super::timerfd`]) and `signalfd` ([`super::signalfd`]),
//! which [`call`] dispatches. Rust's std polls descriptors 0, 1 and 2 at startup to learn
//! whether they are open, and aborts if the call fails for any reason but a few; C programs
//! and event loops (GLib, Qt, SDL, winit) poll or `epoll_wait` before reading or writing, and
//! `poll` with no descriptors is a common way to sleep.
//!
//! The console is always writable, as it takes every byte at once, and readable when a read
//! would take input without waiting for more (D100): a whole line in canonical mode,
//! otherwise `VMIN` bytes. Pipes report what their ends allow (D125), `/dev/window` reports
//! `POLLIN` while an event waits (D127), `/dev/agent` while bytes from the host or its end wait
//! (D137), sockets what their state allows (D138), and the event files what their own pages
//! say. A descriptor that is not open reports `POLLNVAL`.
//!
//! A call that finds nothing ready waits until its timeout, or for good without one, unless a
//! signal is due first (D71). Pipes, `AF_UNIX` sockets and the event files keep a wake list:
//! a waiting thread puts itself on the lists of exactly the files it watches and is woken by
//! their changes only, each of which costs a relaxed load while nobody waits. The console,
//! windows, `AF_INET` sockets and `/dev/agent` are woken through the console's input key
//! ([`console::INPUT_KEY`]), as before; a wait that watches one of them waits on that key, and
//! one that watches `/dev/agent` looks again at the channel's polling interval, as it has no
//! interrupt yet (issue #97). A timer file's next expiry ends a wait to look again.

use super::descriptors::{Descriptors, EventFile, FileKind, OpenFile};
use super::futex::{self, Key};
use super::signals::{self, Interrupted};
use super::terminal::Waited;
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::{
    Errno, Iovecs, ends_in_user_space, epoll, eventfd, inotify, iovec, limits, number, signalfd,
    timerfd,
};
use crate::console;
use crate::process::Process;
use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;
use eggos_hal::user::UserContext;

/// The events of a `struct pollfd` (`POLL*`), the generic values, which x86_64 shares.
pub mod event {
    /// Data to read.
    pub const IN: u16 = 0x1;
    /// Urgent data to read.
    pub const PRI: u16 = 0x2;
    /// Room to write.
    pub const OUT: u16 = 0x4;
    /// An error (reported whether asked for or not).
    pub const ERR: u16 = 0x8;
    /// The other end hung up (reported whether asked for or not).
    pub const HUP: u16 = 0x10;
    /// The descriptor is not open (reported whether asked for or not).
    pub const NVAL: u16 = 0x20;
    /// Normal data to read: as [`IN`].
    pub const RDNORM: u16 = 0x40;
    /// Priority data to read.
    pub const RDBAND: u16 = 0x80;
    /// Room to write normal data: as [`OUT`].
    pub const WRNORM: u16 = 0x100;
    /// Room to write priority data.
    pub const WRBAND: u16 = 0x200;
    /// The peer shut down writing or closed (a socket).
    pub const RDHUP: u16 = 0x2000;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's POLL* values for this architecture (MIPS and SPARC differ)");
}

const POLLFD_SIZE: usize = 8;

const _: () = assert!(limits::MAX_OPEN_FILES * POLLFD_SIZE as u64 <= usize::MAX as u64);

const STACK_POLLFDS: usize = 8;

const TIMESPEC_SIZE: usize = 16;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

#[cfg(target_arch = "x86_64")]
const MICROSECONDS_PER_SECOND: i64 = 1_000_000;

/// The events that mean data to read.
pub const INPUT_EVENTS: u16 = event::IN | event::RDNORM;

/// The events that mean room to write.
pub const OUTPUT_EVENTS: u16 = event::OUT | event::WRNORM;

const UNWATCHED_LOOK_NANOSECONDS: u64 = 10_000_000;

/// What a [`WakeList`] wakes: a thread waiting in `poll`, `select`, `epoll_wait` or a blocking
/// call, or an `epoll` instance's interest in the file.
pub trait Watcher: Send + Sync {
    /// The watched file changed. Runs with the list's lock held: it takes only leaf locks,
    /// futex buckets and other wake lists, and never waits.
    fn woken(&self);

    /// The `epoll` instance whose interest this watcher is, if it is one.
    fn epoll(&self) -> Option<Arc<epoll::Epoll>> {
        None
    }
}

/// The watchers of one file that can be waited on: see the module documentation. A change
/// to the file, made under the lock its readiness is read under, is followed by [`Self::wake`]
/// with no lock held; a watcher joins ([`Self::watch`]) before it reads the readiness, so a
/// change it does not see wakes it.
#[derive(Debug)]
pub struct WakeList {
    watchers: SpinLock<Vec<Arc<dyn Watcher>>>,
    watched: AtomicBool,
}

impl Default for WakeList {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeList {
    /// A list nobody watches yet.
    pub const fn new() -> Self {
        Self {
            watchers: SpinLock::new(Vec::new()),
            watched: AtomicBool::new(false),
        }
    }

    /// A new list for a file, shared with the watchers that hold on to it.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Wakes every watcher: one relaxed load while nobody watches. Call it after the change,
    /// with no lock held.
    #[inline]
    pub fn wake(&self) {
        if self.watched.load(Ordering::Relaxed) {
            self.wake_watchers();
        }
    }

    #[cold]
    #[inline(never)]
    fn wake_watchers(&self) {
        let watchers = self.watchers.lock();
        for watcher in watchers.iter() {
            watcher.woken();
        }
    }

    /// Whether anybody watches.
    pub fn is_watched(&self) -> bool {
        self.watched.load(Ordering::Relaxed)
    }

    /// Adds `watcher`: `ENOMEM` without memory for it.
    pub fn watch(&self, watcher: &Arc<dyn Watcher>) -> Result<(), Errno> {
        let mut watchers = self.watchers.lock();
        if watchers.len() == watchers.capacity() {
            let bytes = (watchers.capacity() + 1) * 2 * size_of::<Arc<dyn Watcher>>();
            if !crate::heap::may_grow_for_user(bytes) {
                return Err(Errno::ENOMEM);
            }
            watchers.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        }
        watchers.push(Arc::clone(watcher));
        self.watched.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Removes `watcher` once and returns it, to be dropped with no lock held.
    pub fn unwatch(&self, watcher: &Arc<dyn Watcher>) -> Option<Arc<dyn Watcher>> {
        let mut watchers = self.watchers.lock();
        let index = watchers
            .iter()
            .position(|watching| core::ptr::addr_eq(Arc::as_ptr(watching), Arc::as_ptr(watcher)))?;
        let removed = watchers.swap_remove(index);
        if watchers.is_empty() {
            self.watched.store(false, Ordering::Relaxed);
        }
        Some(removed)
    }

    /// The `epoll` instances that watch the file, for their nesting checks.
    pub fn epolls(&self) -> Result<Vec<Arc<epoll::Epoll>>, Errno> {
        let mut epolls = Vec::new();
        let watchers = self.watchers.lock();
        for watcher in watchers.iter() {
            if let Some(epoll) = watcher.epoll() {
                epolls.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
                epolls.push(epoll);
            }
        }
        drop(watchers);
        Ok(epolls)
    }
}

struct Waiter {
    woken: AtomicBool,
    key: Key,
    wake: fn(Key),
}

impl Watcher for Waiter {
    fn woken(&self) {
        self.woken.store(true, Ordering::Release);
        (self.wake)(self.key);
    }
}

fn wake_key<P: Platform>(key: Key) {
    futex::wake::<P>(key, u32::MAX, u32::MAX);
}

/// The futex key thread `thread` of `process` waits on for its files: a word in the kernel's
/// half, below the words its sleeps use, one per thread, in the process's space, so the
/// process's end wakes it ([`futex::wake_space`]).
pub(super) fn poll_key<P: Platform>(process: &Process<P>, thread: &LinuxThread) -> Key {
    const FIRST_POLL_WORD: u64 = !3 - (1 << 40);
    Key::new(
        process.futex_space(),
        FIRST_POLL_WORD - 4 * u64::from(thread.id()),
        false,
    )
}

/// A wait in progress: the waiter, on the lists of the files it watches if there was memory
/// for it; without, it looks again every [`UNWATCHED_LOOK_NANOSECONDS`].
pub(super) struct Waiting {
    key: Key,
    waiter: Option<Arc<Waiter>>,
    watcher: Option<Arc<dyn Watcher>>,
    watching: bool,
}

impl Waiting {
    /// Starts a wait on `key`, the waiter on each of `lists`.
    pub(super) fn start<'a, P: Platform>(
        key: Key,
        lists: impl Iterator<Item = &'a Arc<WakeList>> + Clone,
    ) -> Self {
        let waiter = crate::heap::may_grow_for_user(size_of::<Waiter>()).then(|| {
            Arc::new(Waiter {
                woken: AtomicBool::new(false),
                key,
                wake: wake_key::<P>,
            })
        });
        let watcher = waiter.clone().map(|waiter| waiter as Arc<dyn Watcher>);
        let watching = watcher
            .as_ref()
            .is_some_and(|watcher| watch_all(lists, watcher));
        Self {
            key,
            waiter,
            watcher,
            watching,
        }
    }

    /// Ends the wait: the waiter leaves `lists`, those it was started with.
    pub(super) fn stop<'a>(self, lists: impl Iterator<Item = &'a Arc<WakeList>>) {
        if let (true, Some(watcher)) = (self.watching, &self.watcher) {
            for list in lists {
                drop(list.unwatch(watcher));
            }
        }
    }

    /// The key it waits on.
    pub(super) fn key(&self) -> Key {
        self.key
    }

    /// Forgets the wakes so far, before the files are looked at again.
    pub(super) fn take_woken(&self) {
        if let Some(waiter) = &self.waiter {
            waiter.woken.swap(false, Ordering::Acquire);
        }
    }

    /// Whether a watched file changed since [`Self::take_woken`].
    pub(super) fn is_woken(&self) -> bool {
        self.waiter
            .as_ref()
            .is_some_and(|waiter| waiter.woken.load(Ordering::Acquire))
    }

    /// When to look again at the latest, for a wait until `deadline`.
    pub(super) fn until<P: Platform>(&self, deadline: Deadline) -> Deadline {
        match self.watching {
            true => deadline,
            false => deadline.min(soon::<P>()),
        }
    }
}

/// A clock tick soon: when a wait that could not record all it watches looks again.
pub(super) fn soon<P: Platform>() -> Deadline {
    deadline_in::<P>(UNWATCHED_LOOK_NANOSECONDS).unwrap_or(0)
}

fn watch_all<'a>(
    lists: impl Iterator<Item = &'a Arc<WakeList>> + Clone,
    watcher: &Arc<dyn Watcher>,
) -> bool {
    for (done, list) in lists.clone().enumerate() {
        if list.watch(watcher).is_err() {
            for list in lists.take(done) {
                drop(list.unwatch(watcher));
            }
            return false;
        }
    }
    true
}

/// Waits once, as `thread`, the calling thread of `process`, on `key` until `woken` says a
/// watched file changed (it runs under the futex bucket's lock too, so it must take no lock a
/// waker holds while it wakes, nor drop anything), the clock reaches `deadline`, or a signal
/// is sent to the thread: `Ready` to look again (`woken`, a signal sent since the thread last
/// looked that is not due, or a wake for another reason), `TimedOut`, or `Signalled` when one
/// is due. Breaks when the process
/// ends. Call it as a blocking system call runs: with interrupts masked and no lock held.
pub(super) fn wait_once<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    key: Key,
    deadline: Deadline,
    woken: impl Fn() -> bool,
) -> ControlFlow<(), Waited> {
    if woken() {
        return ControlFlow::Continue(Waited::Ready);
    }
    if process.has_ended() {
        return ControlFlow::Break(());
    }
    if process.signals_sent() != thread.signals_seen() {
        signals::take_in_sent_now(process, thread);
        return ControlFlow::Continue(match signals::is_due(thread) {
            true => Waited::Signalled,
            false => Waited::Ready,
        });
    }
    if signals::is_due(thread) {
        return ControlFlow::Continue(Waited::Signalled);
    }
    if deadline != Deadline::MAX && P::clock_ticks() >= deadline {
        return ControlFlow::Continue(Waited::TimedOut);
    }
    let unless_signalled_or_ended = unless_signalled_or_ended(process, thread);
    let waited =
        futex::wait_interruptibly::<P>(key, u32::MAX, deadline, thread.wait_slot(), || {
            unless_signalled_or_ended()?;
            match woken() {
                true => Err(Errno::EAGAIN),
                false => Ok(()),
            }
        });
    if waited == Err(Errno::EINTR) {
        signals::take_in_sent_now(process, thread);
    }
    if process.has_ended() {
        return ControlFlow::Break(());
    }
    if signals::is_due(thread) {
        return ControlFlow::Continue(Waited::Signalled);
    }
    ControlFlow::Continue(match waited {
        Err(Errno::ETIMEDOUT) => Waited::TimedOut,
        _ => Waited::Ready,
    })
}

/// Waits once, as `thread`, the calling thread of `process`, for the files whose wake lists
/// are `lists` to change, unless `ready` says what the caller waits for is there already:
/// `Ready` to look again, `TimedOut` once the clock reaches `deadline` (`u64::MAX` for none),
/// `Signalled` when a signal is due; a signal sent to the thread that is not due also ends
/// the wait, so a caller waiting for a blocked signal looks again. Breaks when the process
/// ends. `ready` is not called under any lock of the wait; callers loop until they are done.
/// Call it as a blocking system call runs: with interrupts masked and no lock held.
pub fn wait_for<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    lists: &[&Arc<WakeList>],
    deadline: Deadline,
    ready: impl Fn() -> bool,
) -> ControlFlow<(), Waited> {
    if ready() {
        return ControlFlow::Continue(Waited::Ready);
    }
    let waiting = Waiting::start::<P>(poll_key(process, thread), lists.iter().copied());
    let until = waiting.until::<P>(deadline);
    let waited = match ready() {
        true => ControlFlow::Continue(Waited::Ready),
        false => wait_once::<P>(process, thread, waiting.key(), until, || waiting.is_woken()),
    };
    waiting.stop(lists.iter().copied());
    match waited {
        ControlFlow::Continue(Waited::TimedOut) if until < deadline => {
            ControlFlow::Continue(Waited::Ready)
        }
        waited => waited,
    }
}

/// The wake list of `kind`, if it keeps one: pipes, pseudo-terminals, `AF_UNIX` sockets and
/// the event files.
pub fn wake_list(kind: &FileKind) -> Option<Arc<WakeList>> {
    match kind {
        FileKind::Pipe(end) => Some(Arc::clone(end.wake_list())),
        FileKind::PseudoTerminal(end) => Some(Arc::clone(end.wake_list())),
        FileKind::Socket(socket) => Some(Arc::clone(socket.wake_list())),
        FileKind::Event(event) => Some(Arc::clone(event.wake_list())),
        _ => None,
    }
}

fn is_woken_by_input(kind: &FileKind) -> bool {
    matches!(
        kind,
        FileKind::Console
            | FileKind::Window(_)
            | FileKind::InetSocket(_)
            | FileKind::AgentChannel(_)
    )
}

/// Whether a wait for `kind` waits on the console's input key: the console, windows,
/// `AF_INET` sockets, `/dev/agent`, and an `epoll` instance that watches one of them.
pub fn uses_input_key(kind: &FileKind) -> bool {
    match kind {
        FileKind::Event(EventFile::Epoll(epoll)) => epoll.uses_input_key(0),
        kind => is_woken_by_input(kind),
    }
}

/// Whether `epoll` must look at `kind` at each wait rather than only when its wake list
/// wakes: it may change with nothing to wake (a timer's expiry, a signal sent, a file woken
/// through the input key, an `epoll` instance's own such files), or it keeps no list.
pub fn needs_looking_at(kind: &FileKind) -> bool {
    !matches!(
        kind,
        FileKind::Event(EventFile::EventFd(_) | EventFile::Inotify(_))
            | FileKind::Pipe(_)
            | FileKind::PseudoTerminal(_)
            | FileKind::Socket(_)
    )
}

/// When `kind` may become ready with nothing to wake its waiters, in clock ticks: a timer's
/// next expiry, `/dev/agent`'s next look, an `epoll` instance's earliest such file.
pub fn wake_deadline<P: Platform>(kind: &FileKind) -> Option<Deadline> {
    match kind {
        FileKind::Event(EventFile::TimerFd(timer)) => timer.next_tick::<P>(),
        FileKind::Event(EventFile::Epoll(epoll)) => epoll.next_wake::<P>(0),
        FileKind::AgentChannel(_) => Some(super::agent_channel::next_poll::<P>()),
        _ => None,
    }
}

/// The files a wait checks under the futex bucket's lock, because the console's input key
/// wakes it for them: each with the events asked of it.
pub type InputFiles = Vec<(Arc<OpenFile>, u16)>;

/// Adds `file`, asked for `requested`, to `files` if a wait for it is woken through the
/// console's input key, and an `epoll` instance's such files. `ENOMEM` without memory.
pub fn add_input_files(
    file: &Arc<OpenFile>,
    requested: u16,
    files: &mut InputFiles,
) -> Result<(), Errno> {
    match file.kind() {
        FileKind::Event(EventFile::Epoll(epoll)) => epoll.add_input_files(files, 0),
        kind if is_woken_by_input(kind) => {
            files.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
            files.push((Arc::clone(file), requested));
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Whether one of `files` reports an event asked of it: read under the futex bucket's lock,
/// so it only looks at the files' own state and drops nothing.
pub fn input_files_ready(files: &InputFiles) -> bool {
    files.iter().any(|(file, requested)| {
        let readable = matches!(file.kind(), FileKind::Console) && console::input_ready();
        events_of(file.kind(), *requested, readable, &|| 0) != 0
    })
}

fn events_of(kind: &FileKind, requested: u16, readable: bool, pending: &dyn Fn() -> u64) -> u16 {
    match kind {
        FileKind::Console if readable => requested & (OUTPUT_EVENTS | INPUT_EVENTS),
        FileKind::Console => requested & OUTPUT_EVENTS,
        FileKind::Regular(_)
        | FileKind::Directory(_)
        | FileKind::Snapshot(_)
        | FileKind::MemoryFile(_)
        | FileKind::MemoryDevice(..) => requested & (OUTPUT_EVENTS | INPUT_EVENTS),
        FileKind::Path(_) => event::NVAL,
        FileKind::Pipe(end) => end.events(requested),
        FileKind::PseudoTerminal(end) => end.events(requested),
        FileKind::Window(connection) => super::window::events(connection, requested),
        FileKind::AgentChannel(_) => agent_events(requested),
        FileKind::Socket(socket) => socket.events(requested),
        FileKind::InetSocket(socket) => socket.events(requested),
        FileKind::Event(EventFile::EventFd(counter)) => counter.events(requested),
        FileKind::Event(EventFile::SignalFd(signals)) => signals.events(requested, pending()),
        FileKind::Event(EventFile::Inotify(instance)) => instance.events(requested),
        FileKind::Event(_) => 0,
    }
}

fn events<P: Platform>(
    file: Option<&FileKind>,
    requested: u16,
    readable: bool,
    pending: &dyn Fn() -> u64,
) -> u16 {
    match file {
        None => event::NVAL,
        Some(FileKind::Event(EventFile::TimerFd(timer))) => timer.events::<P>(requested),
        Some(FileKind::Event(EventFile::Epoll(epoll))) => epoll.events::<P>(requested, pending, 0),
        Some(kind) => events_of(kind, requested, readable, pending),
    }
}

/// The events `kind` reports of `requested` for the thread whose pending signals `pending`
/// gives, as [`ppoll`] would report them: for `epoll` instances.
pub fn file_events<P: Platform>(kind: &FileKind, requested: u16, pending: &dyn Fn() -> u64) -> u16 {
    let readable = matches!(kind, FileKind::Console) && console::input_ready();
    events::<P>(Some(kind), requested, readable, pending)
}

fn agent_events(requested: u16) -> u16 {
    let input = match requested & INPUT_EVENTS != 0 && crate::agent_channel::ready_to_receive() {
        true => requested & INPUT_EVENTS,
        false => 0,
    };
    input | requested & OUTPUT_EVENTS
}

fn pollfd(entry: &[u8; POLLFD_SIZE]) -> (i32, u16) {
    (
        i32::from_ne_bytes(entry[..4].try_into().unwrap()),
        u16::from_ne_bytes(entry[4..6].try_into().unwrap()),
    )
}

fn set_returned(entry: &mut [u8; POLLFD_SIZE], returned: u16) {
    entry[6..].copy_from_slice(&returned.to_ne_bytes());
}

type Count = fn(u16, u16) -> u64;

fn poll_count(_requested: u16, returned: u16) -> u64 {
    u64::from(returned != 0)
}

fn fill_in_events<P: Platform>(
    entries: &mut [[u8; POLLFD_SIZE]],
    files: &Descriptors,
    readable: bool,
    pending: &dyn Fn() -> u64,
    count: Count,
) -> u64 {
    let mut ready = 0;
    for entry in entries {
        let (fd, requested) = pollfd(entry);
        let returned = match fd {
            0.. => {
                let file = files.file(fd as u64).ok();
                events::<P>(
                    file.as_deref().map(OpenFile::kind),
                    requested,
                    readable,
                    pending,
                )
            }
            _ => 0,
        };
        set_returned(entry, returned);
        ready += count(requested, returned);
    }
    ready
}

/// When a wait ends, in clock ticks ([`Platform::clock_ticks`]); `u64::MAX` for never.
pub type Deadline = u64;

/// What a wait for descriptors came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polled {
    /// How many descriptors report events, after the wait if none did at first.
    Ready(u64),
    /// None did, and a signal was due, so the wait did not start or ended.
    Interrupted,
}

/// The deadline of a wait for `nanoseconds`, rounded up so it is never reached early: `None`
/// for none, no wait at all.
pub fn deadline_in<P: Platform>(nanoseconds: u64) -> Option<Deadline> {
    (nanoseconds != 0).then(|| {
        let ticks = mul_div_ceil(nanoseconds, P::clock_frequency(), NANOSECONDS_PER_SECOND);
        P::clock_ticks().saturating_add(ticks)
    })
}

/// `poll(fds, nfds, timeout)`: as [`ppoll`] with a timeout of `timeout` milliseconds (an
/// `int`: negative waits for good, 0 not at all) and no signal mask, failing with `EINTR` when
/// a signal is due, which is delivered after it. x86_64 only: AArch64 and RISC-V have only
/// `ppoll`, which their C libraries' `poll` calls.
#[cfg(target_arch = "x86_64")]
pub fn poll<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fds: u64,
    count: u64,
    timeout: u64,
) -> Result<u64, Errno> {
    match poll_fds(
        process,
        thread,
        fds,
        count,
        milliseconds_deadline::<P>(timeout),
    )? {
        Polled::Ready(ready) => Ok(ready),
        Polled::Interrupted => Err(Errno::EINTR),
    }
}

/// The deadline of a timeout of `timeout` milliseconds, an `int`: negative waits for good, 0
/// not at all.
pub fn milliseconds_deadline<P: Platform>(timeout: u64) -> Option<Deadline> {
    let milliseconds = timeout as u32 as i32;
    if milliseconds < 0 {
        Some(Deadline::MAX)
    } else {
        deadline_in::<P>(milliseconds as u64 * 1_000_000)
    }
}

/// `ppoll(fds, nfds, timeout, sigmask, sigsetsize)`, with the program's registers `context`:
/// reports, in each `struct pollfd` of the `nfds` at `fds`, the events its descriptor has of those
/// asked for (see `events`), and returns how many report any. If none does, it first waits for one
/// until the `struct timespec` at `timeout` has passed, for good if that is null, not at all if it
/// is zero. With a `sigmask`, the signals in that set are blocked instead of the thread's during
/// the call (D71). If none reports events and a signal is due, before or during the wait (another
/// thread sent it, D95), the call delivers it and, once a handler starts, fails with `EINTR`, the
/// handler's frame saving the thread's own mask, as `rt_sigsuspend` does (D70); a signal its action
/// ignores is taken and the call goes on, as Linux restarts it, unless the time left could not be
/// written back (below): then it fails with `EINTR`, as Linux's does, since it cannot restart with
/// the time left. Otherwise the thread's mask is back when it returns, and a signal the call's mask
/// unblocked stays pending. A timeout that is not zero gets the time not slept written back once
/// the signal mask passed its checks, whatever the result: zero after a wait that ended with it,
/// about all of it when something was ready at once (D62); one too long to count in nanoseconds is
/// written back as it was. Linux's system call does that (its C libraries hide it), and ignores a
/// timeout the program cannot write, as here, but for that `EINTR`. As Linux, in this order:
/// `EFAULT` if `timeout` is not the program's memory; `EINVAL` for negative seconds or nanoseconds
/// outside 0 to 999999999; with a `sigmask`, `EINVAL` if `sigsetsize` is not 8 and `EFAULT` if the
/// mask is not the program's memory; `EINVAL` for more descriptors than the open-file limit
/// (`RLIMIT_NOFILE`'s soft limit); `EFAULT` if the array is not the program's writable memory;
/// `ENOMEM` if there is no memory to copy it. `nfds` is an `unsigned int`: the register's upper
/// half is ignored. Sets the call's result; breaks when a signal ends the process.
pub fn ppoll<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fds, count, timeout, mask, set_size, _] = context.syscall_arguments();
    let checked = read_timespec_timeout::<P>(process, timeout).and_then(|timeout| {
        let mask = match mask {
            0 => None,
            mask => Some(signals::read_set(process, mask, set_size)?),
        };
        Ok((timeout, mask))
    });
    let (timeout, mask) = match checked {
        Ok(checked) => checked,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    with_mask(process, thread, context, mask, |process, thread| {
        let polled = poll_fds(process, thread, fds, count, timeout.deadline);
        (polled, timeout.write_back::<P>(process))
    })
}

fn with_mask<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    mask: Option<u64>,
    mut attempt: impl FnMut(&Arc<Process<P>>, &mut LinuxThread) -> (Result<Polled, Errno>, bool),
) -> ControlFlow<()> {
    let thread_mask = mask.map(|mask| signals::replace_blocked(process, thread, mask));
    loop {
        let (polled, time_left_written) = attempt(process, thread);
        let result = match polled {
            Ok(Polled::Interrupted) => {
                context.set_syscall_result(Errno::EINTR.result());
                if signals::deliver_restoring_mask(process, thread, context, thread_mask)? {
                    return ControlFlow::Continue(());
                }
                if time_left_written {
                    continue;
                }
                Err(Errno::EINTR)
            }
            Ok(Polled::Ready(ready)) => Ok(ready),
            Err(errno) => Err(errno),
        };
        if let Some(thread_mask) = thread_mask {
            signals::replace_blocked(process, thread, thread_mask);
        }
        context.set_syscall_result(result.unwrap_or_else(Errno::result));
        return ControlFlow::Continue(());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeFormat {
    Timespec,
    #[cfg(target_arch = "x86_64")]
    Timeval,
}

/// A timeout the program gave, read and checked: when the wait ends, and what to write the
/// time left back to.
#[derive(Debug, Clone, Copy)]
pub struct Timeout {
    /// Where the program keeps it: 0 for none.
    address: u64,
    /// The timeout the time left is written back to: `None` for a null or zero one.
    requested: Option<RequestedTimeout>,
    /// When the wait ends: `None` for no wait.
    pub deadline: Option<Deadline>,
    format: TimeFormat,
}

impl Timeout {
    fn write_back<P: Platform>(&self, process: &Process<P>) -> bool {
        write_back_time_left::<P>(
            process,
            self.address,
            &self.requested,
            self.deadline,
            self.format,
        )
    }
}

/// The `struct timespec` timeout at `address`: none (wait for good) if it is null. As Linux:
/// `EFAULT` if it is not the program's memory, `EINVAL` for negative seconds or nanoseconds
/// outside 0 to 999999999.
pub fn read_timespec_timeout<P: Platform>(
    process: &Process<P>,
    address: u64,
) -> Result<Timeout, Errno> {
    if address == 0 {
        return Ok(Timeout {
            address,
            requested: None,
            deadline: Some(Deadline::MAX),
            format: TimeFormat::Timespec,
        });
    }
    let as_written = read_timespec(process, address)?;
    let nanoseconds = parse_timeout(as_written)?;
    Ok(Timeout {
        address,
        requested: (nanoseconds != Some(0)).then_some(RequestedTimeout {
            as_written,
            nanoseconds,
        }),
        deadline: deadline_in::<P>(nanoseconds.unwrap_or(u64::MAX)),
        format: TimeFormat::Timespec,
    })
}

#[cfg(target_arch = "x86_64")]
fn read_timeval_timeout<P: Platform>(process: &Process<P>, address: u64) -> Result<Timeout, Errno> {
    if address == 0 {
        return Ok(Timeout {
            address,
            requested: None,
            deadline: Some(Deadline::MAX),
            format: TimeFormat::Timeval,
        });
    }
    let as_written = read_timespec(process, address)?;
    let seconds = i64::from_ne_bytes(as_written[..8].try_into().unwrap());
    let microseconds = i64::from_ne_bytes(as_written[8..].try_into().unwrap());
    let nanoseconds_part = microseconds % MICROSECONDS_PER_SECOND * 1000;
    let seconds = seconds.checked_add(microseconds / MICROSECONDS_PER_SECOND);
    if seconds.is_some_and(|seconds| seconds < 0) || nanoseconds_part < 0 {
        return Err(Errno::EINVAL);
    }
    let nanoseconds = seconds.and_then(|seconds| {
        (seconds as u64)
            .checked_mul(NANOSECONDS_PER_SECOND)
            .and_then(|whole| whole.checked_add(nanoseconds_part as u64))
    });
    Ok(Timeout {
        address,
        requested: (nanoseconds != Some(0)).then_some(RequestedTimeout {
            as_written,
            nanoseconds,
        }),
        deadline: deadline_in::<P>(nanoseconds.unwrap_or(u64::MAX)),
        format: TimeFormat::Timeval,
    })
}

#[derive(Debug, Clone, Copy)]
struct RequestedTimeout {
    as_written: [u8; TIMESPEC_SIZE],
    nanoseconds: Option<u64>,
}

fn write_back_time_left<P: Platform>(
    process: &Process<P>,
    timeout: u64,
    requested_timeout: &Option<RequestedTimeout>,
    deadline: Option<Deadline>,
    format: TimeFormat,
) -> bool {
    let Some(RequestedTimeout {
        as_written,
        nanoseconds,
    }) = requested_timeout
    else {
        return true;
    };
    let bytes = match (nanoseconds, deadline) {
        (Some(nanoseconds), Some(deadline)) if deadline != Deadline::MAX => {
            let left = deadline.saturating_sub(P::clock_ticks());
            let left =
                mul_div_ceil(left, NANOSECONDS_PER_SECOND, P::clock_frequency()).min(*nanoseconds);
            let fraction = left % NANOSECONDS_PER_SECOND;
            let fraction = match format {
                TimeFormat::Timespec => fraction,
                #[cfg(target_arch = "x86_64")]
                TimeFormat::Timeval => fraction / 1000,
            };
            let mut bytes = [0; TIMESPEC_SIZE];
            bytes[..8].copy_from_slice(&(left / NANOSECONDS_PER_SECOND).to_ne_bytes());
            bytes[8..].copy_from_slice(&fraction.to_ne_bytes());
            bytes
        }
        _ => *as_written,
    };
    process.write_user_memory(timeout, &bytes).is_ok()
}

/// The `struct timespec` at `address`, a timeout, in nanoseconds, or `None` if it is too long
/// for a `u64` (over 584 years). As Linux: `EFAULT` if it is not the program's memory, `EINVAL`
/// for negative seconds or nanoseconds outside 0 to 999999999.
pub fn read_timeout<P: Platform>(process: &Process<P>, address: u64) -> Result<Option<u64>, Errno> {
    parse_timeout(read_timespec(process, address)?)
}

fn read_timespec<P: Platform>(
    process: &Process<P>,
    address: u64,
) -> Result<[u8; TIMESPEC_SIZE], Errno> {
    let mut bytes = [0; TIMESPEC_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes)
}

/// A `struct timespec` in nanoseconds, or `None` if it is too long for a `u64`: `EINVAL` for
/// negative seconds or nanoseconds outside 0 to 999999999.
pub fn parse_timeout(bytes: [u8; TIMESPEC_SIZE]) -> Result<Option<u64>, Errno> {
    let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let nanoseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..NANOSECONDS_PER_SECOND as i64).contains(&nanoseconds) {
        return Err(Errno::EINVAL);
    }
    Ok((seconds as u64)
        .checked_mul(NANOSECONDS_PER_SECOND)
        .and_then(|whole| whole.checked_add(nanoseconds as u64)))
}

fn poll_fds<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    fds: u64,
    count: u64,
    deadline: Option<Deadline>,
) -> Result<Polled, Errno> {
    let count = u64::from(count as u32);
    if count > open_files_soft_limit(process) {
        return Err(Errno::EINVAL);
    }
    let bytes = count as usize * POLLFD_SIZE;
    if !ends_in_user_space::<P>(fds, bytes as u64) {
        return Err(Errno::EFAULT);
    }
    let mut on_stack = [0u8; STACK_POLLFDS * POLLFD_SIZE];
    let mut on_heap = Vec::new();
    let array = if bytes <= on_stack.len() {
        &mut on_stack[..bytes]
    } else {
        on_heap
            .try_reserve_exact(bytes)
            .map_err(|_| Errno::ENOMEM)?;
        on_heap.resize(bytes, 0);
        &mut on_heap[..]
    };
    process
        .read_user_memory(fds, array)
        .map_err(|_| Errno::EFAULT)?;
    let entries = array.as_chunks_mut::<POLLFD_SIZE>().0;
    let polled = poll_entries::<P>(process, thread, entries, deadline, poll_count)?;
    process
        .write_user_memory(fds, array)
        .map_err(|_| Errno::EFAULT)?;
    Ok(polled)
}

fn open_files_soft_limit<P: Platform>(process: &Process<P>) -> u64 {
    process
        .credentials_and_limits()
        .lock()
        .limits
        .get(limits::resource::NOFILE)
        .map_or(u64::MAX, |limit| limit.soft)
}

fn poll_entries<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    entries: &mut [[u8; POLLFD_SIZE]],
    deadline: Option<Deadline>,
    count: Count,
) -> Result<Polled, Errno> {
    let mut ready = {
        let view: &LinuxThread = thread;
        let pending = || signals::pending_in(process, view, u64::MAX);
        fill_in_events::<P>(
            entries,
            view.files(),
            console::input_ready(),
            &pending,
            count,
        )
    };
    let mut interrupted = ready == 0 && signals::is_due(thread);
    if !interrupted && let Some(deadline) = deadline.filter(|_| ready == 0) {
        (interrupted, ready) = wait_for_entries::<P>(process, thread, entries, deadline, count)?;
    }
    Ok(match interrupted {
        true => Polled::Interrupted,
        false => Polled::Ready(ready),
    })
}

fn wait_for_entries<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    entries: &mut [[u8; POLLFD_SIZE]],
    deadline: Deadline,
    count: Count,
) -> Result<(bool, u64), Errno> {
    let watched = watched_files(thread.files(), entries)?;
    let mut lists = Vec::new();
    lists
        .try_reserve_exact(watched.len())
        .map_err(|_| Errno::ENOMEM)?;
    lists.extend(
        watched
            .iter()
            .filter_map(|(_, file)| wake_list(file.kind())),
    );
    let by_input = watched.iter().any(|(_, file)| uses_input_key(file.kind()));
    if by_input {
        #[cfg(feature = "qemu-boot-clock")]
        crate::boot_clock::report_first_wait::<P>();
        crate::terminal::console_waited::<P>();
    }
    let key = match by_input {
        true => console::INPUT_KEY,
        false => poll_key(process, thread),
    };
    let waiting = Waiting::start::<P>(key, lists.iter());
    let outcome = loop {
        waiting.take_woken();
        let ready = {
            let view: &LinuxThread = thread;
            let pending = || signals::pending_in(process, view, u64::MAX);
            fill_in_watched::<P>(entries, &watched, &pending, count)
        };
        if ready > 0 {
            break (false, ready);
        }
        if P::clock_ticks() >= deadline {
            break (false, 0);
        }
        let mut input_files = InputFiles::new();
        let mut until = deadline;
        for (index, file) in &watched {
            let (_, requested) = pollfd(&entries[*index]);
            if by_input && add_input_files(file, requested, &mut input_files).is_err() {
                until = until.min(soon::<P>());
            }
            if let Some(wake) = wake_deadline::<P>(file.kind()) {
                until = until.min(wake);
            }
        }
        let until = waiting.until::<P>(until);
        let woken = || waiting.is_woken() || input_files_ready(&input_files);
        match wait_once::<P>(process, thread, key, until, woken) {
            ControlFlow::Break(()) => break (false, 0),
            ControlFlow::Continue(Waited::Signalled) => break (true, 0),
            ControlFlow::Continue(Waited::Ready | Waited::TimedOut) => {}
        }
    };
    waiting.stop(lists.iter());
    drop(lists);
    drop(watched);
    Ok(outcome)
}

fn watched_files(
    files: &Descriptors,
    entries: &[[u8; POLLFD_SIZE]],
) -> Result<Vec<(usize, Arc<OpenFile>)>, Errno> {
    let mut watched = Vec::new();
    let bytes = entries.len() * size_of::<(usize, Arc<OpenFile>)>();
    if !crate::heap::may_grow_for_user(bytes) {
        return Err(Errno::ENOMEM);
    }
    watched
        .try_reserve_exact(entries.len())
        .map_err(|_| Errno::ENOMEM)?;
    for (index, entry) in entries.iter().enumerate() {
        let (fd, _) = pollfd(entry);
        if fd < 0 {
            continue;
        }
        if let Ok(file) = files.file(fd as u64) {
            watched.push((index, file));
        }
    }
    Ok(watched)
}

fn fill_in_watched<P: Platform>(
    entries: &mut [[u8; POLLFD_SIZE]],
    watched: &[(usize, Arc<OpenFile>)],
    pending: &dyn Fn() -> u64,
    count: Count,
) -> u64 {
    let readable = console::input_ready();
    for (index, file) in watched {
        let entry = &mut entries[*index];
        let (_, requested) = pollfd(entry);
        let returned = events::<P>(Some(file.kind()), requested, readable, pending);
        set_returned(entry, returned);
    }
    entries
        .iter()
        .map(|entry| {
            let (_, requested) = pollfd(entry);
            count(requested, u16::from_ne_bytes([entry[6], entry[7]]))
        })
        .sum()
}

const SELECT_READ: u16 = INPUT_EVENTS | event::RDBAND | event::HUP | event::ERR;
const SELECT_WRITE: u16 = OUTPUT_EVENTS | event::WRBAND | event::ERR;
const SELECT_EXCEPT: u16 = event::PRI;

const READ_REQUEST: u16 = INPUT_EVENTS | event::RDBAND;
const WRITE_REQUEST: u16 = OUTPUT_EVENTS | event::WRBAND;
const EXCEPT_REQUEST: u16 = event::PRI;

fn select_bits(requested: u16, returned: u16) -> [bool; 3] {
    [
        requested & event::IN != 0 && returned & SELECT_READ != 0,
        requested & event::OUT != 0 && returned & SELECT_WRITE != 0,
        requested & event::PRI != 0 && returned & SELECT_EXCEPT != 0,
    ]
}

fn select_count(requested: u16, returned: u16) -> u64 {
    select_bits(requested, returned)
        .iter()
        .map(|&set| u64::from(set))
        .sum()
}

const STACK_SET_WORDS: usize = 16;

fn select_fds<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    nfds: u64,
    sets: [u64; 3],
    deadline: Option<Deadline>,
) -> Result<Polled, Errno> {
    let nfds = nfds as u32 as i32;
    if nfds < 0 {
        return Err(Errno::EINVAL);
    }
    let count = (nfds as u64)
        .min(open_files_soft_limit(process))
        .min(limits::MAX_OPEN_FILES) as usize;
    let words = count.div_ceil(64);
    let mut on_stack = [0u64; 3 * STACK_SET_WORDS];
    let mut on_heap = Vec::new();
    let bits = if words <= STACK_SET_WORDS {
        &mut on_stack[..3 * words]
    } else {
        on_heap
            .try_reserve_exact(3 * words)
            .map_err(|_| Errno::ENOMEM)?;
        on_heap.resize(3 * words, 0);
        &mut on_heap[..]
    };
    for (set, &address) in sets.iter().enumerate() {
        if address == 0 || words == 0 {
            continue;
        }
        let mut word = [0u8; 8];
        for index in 0..words {
            let at = address
                .checked_add(8 * index as u64)
                .ok_or(Errno::EFAULT)?;
            process
                .read_user_memory(at, &mut word)
                .map_err(|_| Errno::EFAULT)?;
            bits[set * words + index] = u64::from_ne_bytes(word);
        }
    }
    let is_set =
        |bits: &[u64], set: usize, fd: usize| bits[set * words + fd / 64] >> (fd % 64) & 1 != 0;
    let mut selected = 0;
    for fd in 0..count {
        if (0..3).any(|set| is_set(bits, set, fd)) {
            thread.files().file(fd as u64)?;
            selected += 1;
        }
    }
    let mut on_stack_entries = [[0u8; POLLFD_SIZE]; STACK_POLLFDS];
    let mut on_heap_entries = Vec::new();
    let entries = if selected <= STACK_POLLFDS {
        &mut on_stack_entries[..selected]
    } else {
        on_heap_entries
            .try_reserve_exact(selected)
            .map_err(|_| Errno::ENOMEM)?;
        on_heap_entries.resize(selected, [0u8; POLLFD_SIZE]);
        &mut on_heap_entries[..]
    };
    let mut next = 0;
    for fd in 0..count {
        let requested = [READ_REQUEST, WRITE_REQUEST, EXCEPT_REQUEST]
            .iter()
            .enumerate()
            .filter(|&(set, _)| is_set(bits, set, fd))
            .fold(0, |requested, (_, request)| requested | request);
        if requested != 0 {
            entries[next][..4].copy_from_slice(&(fd as i32).to_ne_bytes());
            entries[next][4..6].copy_from_slice(&requested.to_ne_bytes());
            next += 1;
        }
    }
    let polled = poll_entries::<P>(process, thread, entries, deadline, select_count)?;
    if polled == Polled::Interrupted {
        return Ok(polled);
    }
    bits.fill(0);
    for entry in entries.iter() {
        let (fd, requested) = pollfd(entry);
        let returned = u16::from_ne_bytes([entry[6], entry[7]]);
        for (set, ready) in select_bits(requested, returned).into_iter().enumerate() {
            if ready {
                bits[set * words + fd as usize / 64] |= 1 << (fd as usize % 64);
            }
        }
    }
    for (set, &address) in sets.iter().enumerate() {
        if address == 0 || words == 0 {
            continue;
        }
        for index in 0..words {
            let at = address
                .checked_add(8 * index as u64)
                .ok_or(Errno::EFAULT)?;
            process
                .write_user_memory(at, &bits[set * words + index].to_ne_bytes())
                .map_err(|_| Errno::EFAULT)?;
        }
    }
    Ok(polled)
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)`, with the program's registers
/// `context`: as [`pselect6`] with the `struct timeval` at `timeout` (null: wait for good;
/// whole seconds of microseconds count as seconds; `EFAULT` if it is not the program's memory,
/// `EINVAL` if negative, before the sets are read) and no signal mask; the time left is written
/// back as `pselect6` writes it, in microseconds. x86_64 only: AArch64's and RISC-V's C
/// libraries call `pselect6`. Sets the call's result; breaks when a signal ends the process.
#[cfg(target_arch = "x86_64")]
pub fn select<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [nfds, read, write, except, timeout, _] = context.syscall_arguments();
    let timeout = match read_timeval_timeout::<P>(process, timeout) {
        Ok(timeout) => timeout,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    with_mask(process, thread, context, None, |process, thread| {
        let selected = select_fds(
            process,
            thread,
            nfds,
            [read, write, except],
            timeout.deadline,
        );
        (selected, timeout.write_back::<P>(process))
    })
}

const SIGSET_ARGUMENTS_SIZE: usize = 16;

/// `pselect6(nfds, readfds, writefds, exceptfds, timeout, sig)`, with the program's registers
/// `context`: reports, in the three `fd_set`s (each `nfds` bits rounded up to 64-bit words,
/// null for none), the descriptors ready to read (data, a hang-up or an error), to write (room
/// or an error) and with priority data, as `poll` reports them, clearing the others, and
/// returns how many bits are set. If none is, it first waits as `ppoll` does until the `struct
/// timespec` at `timeout` has passed, for good if that is null, not at all if it is zero; `sig`,
/// unless null, points to a signal set's address and size: with a set, the signals in it are
/// blocked instead of the thread's during the call, as [`ppoll`]'s mask. The time left is
/// written back as `ppoll` writes it, and a signal interrupts the call as it does `ppoll`
/// (Linux's `-ERESTARTNOHAND`), leaving the sets as they were. As Linux, in this order: `EFAULT`
/// if `sig` is not the program's memory; the timeout's `EFAULT` and `EINVAL`; with a set,
/// `EINVAL` if its size is not 8, `EFAULT` if it is not the program's memory; `EINVAL` for a
/// negative `nfds` (one above the open-file limit is cut to it); `EFAULT` if a set is not the
/// program's memory; `EBADF` for a descriptor in a set that is not open; `ENOMEM` without
/// memory to copy them; `EFAULT` if a set cannot be written back. Sets the call's result;
/// breaks when a signal ends the process.
pub fn pselect6<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [nfds, read, write, except, timeout, sig] = context.syscall_arguments();
    let checked = (|| -> Result<(Timeout, Option<u64>), Errno> {
        let set = match sig {
            0 => None,
            sig => {
                let mut bytes = [0u8; SIGSET_ARGUMENTS_SIZE];
                process
                    .read_user_memory(sig, &mut bytes)
                    .map_err(|_| Errno::EFAULT)?;
                let address = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
                let size = u64::from_ne_bytes(bytes[8..].try_into().unwrap());
                (address != 0).then_some((address, size))
            }
        };
        let timeout = read_timespec_timeout::<P>(process, timeout)?;
        let mask = match set {
            Some((address, size)) => Some(signals::read_set(process, address, size)?),
            None => None,
        };
        Ok((timeout, mask))
    })();
    let (timeout, mask) = match checked {
        Ok(checked) => checked,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    with_mask(process, thread, context, mask, |process, thread| {
        let selected = select_fds(
            process,
            thread,
            nfds,
            [read, write, except],
            timeout.deadline,
        );
        (selected, timeout.write_back::<P>(process))
    })
}

/// Copies `bytes` to the program's buffers `buffers`, from `offset` bytes into them: `EFAULT`
/// if they are not its writable memory. The caller checked that they hold that many.
pub(super) fn copy_to_buffers<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    offset: u64,
    bytes: &[u8],
) -> Result<(), Errno> {
    let mut skip = offset;
    let mut copied = 0;
    for entry in buffers.entries() {
        if copied == bytes.len() {
            break;
        }
        let (base, length) = iovec(entry);
        if skip >= length {
            skip -= length;
            continue;
        }
        let room = (length - skip).min((bytes.len() - copied) as u64) as usize;
        process
            .write_user_memory(base + skip, &bytes[copied..copied + room])
            .map_err(|_| Errno::EFAULT)?;
        copied += room;
        skip = 0;
    }
    Ok(())
}

/// Copies the first `bytes.len()` bytes of the program's buffers `buffers` into `bytes`:
/// `EFAULT` if they are not its memory. The caller checked that they hold that many.
pub(super) fn copy_from_buffers<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    bytes: &mut [u8],
) -> Result<(), Errno> {
    let mut copied = 0;
    for entry in buffers.entries() {
        if copied == bytes.len() {
            break;
        }
        let (base, length) = iovec(entry);
        let room = length.min((bytes.len() - copied) as u64) as usize;
        process
            .read_user_memory(base, &mut bytes[copied..copied + room])
            .map_err(|_| Errno::EFAULT)?;
        copied += room;
    }
    Ok(())
}

/// What one attempt of a blocking read or write came to: its result, or `None` when a signal
/// is due; breaks when the process ends.
pub type Attempted = ControlFlow<(), Option<Result<u64, Errno>>>;

/// Runs `attempt` until it comes to a result, as a blocking read or write does: `None` from it
/// means a signal is due, which is delivered, and the call restarts after a handler with
/// `SA_RESTART`, fails with `EINTR` after one without (Linux's `-ERESTARTSYS`), and goes on if
/// none started. Sets the call's result and delivers the signals due after it; breaks when a
/// signal ends the process.
pub(super) fn blocking<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    mut attempt: impl FnMut(&Process<P>, &mut LinuxThread) -> Attempted,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match attempt(&**process, thread)? {
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

/// `read` or `readv` of `buffers` from the event file `event` of `file`, which the caller
/// checked is open for reading: an `eventfd`'s counter, a `timerfd`'s expirations, a
/// `signalfd`'s signals, an `inotify` instance's records; `EINVAL` from an `epoll` instance, as
/// on Linux. Sets the call's result.
pub(super) fn read_event<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    event: &EventFile,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    match event {
        EventFile::EventFd(counter) => {
            eventfd::read(process, thread, context, file, counter, buffers)
        }
        EventFile::TimerFd(timer) => timerfd::read(process, thread, context, file, timer, buffers),
        EventFile::SignalFd(signal_file) => {
            signalfd::read(process, thread, context, file, signal_file, buffers)
        }
        EventFile::Inotify(instance) => {
            inotify::read(process, thread, context, file, instance, buffers)
        }
        EventFile::Epoll(_) => {
            context.set_syscall_result(Errno::EINVAL.result());
            ControlFlow::Continue(())
        }
    }
}

/// `write` or `writev` of `buffers` to the event file `event` of `file`, which the caller
/// checked is open for writing: an `eventfd`'s counter; `EINVAL` for the others, as on Linux.
/// Sets the call's result.
pub(super) fn write_event<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    event: &EventFile,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    match event {
        EventFile::EventFd(counter) => {
            eventfd::write(process, thread, context, file, counter, buffers)
        }
        EventFile::TimerFd(_)
        | EventFile::SignalFd(_)
        | EventFile::Epoll(_)
        | EventFile::Inotify(_) => {
            context.set_syscall_result(Errno::EINVAL.result());
            ControlFlow::Continue(())
        }
    }
}

/// The calls of the event files and `select`, `pselect6`: `epoll_create`, `epoll_create1`,
/// `epoll_ctl`, `epoll_wait`, `epoll_pwait`, `epoll_pwait2`, `eventfd`, `eventfd2`,
/// `timerfd_create`, `timerfd_settime`, `timerfd_gettime`, `signalfd`, `signalfd4`,
/// `inotify_init`, `inotify_init1`, `inotify_add_watch` and `inotify_rm_watch`, with
/// the program's registers `context`, whose result it sets. One function out of line for all
/// of them, as `socket::call`, so the system-call path stays as it was. The calls that wait
/// deliver the signals due after them; breaks when one ends the process.
#[inline(never)]
pub fn call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [first, second, third, fourth, ..] = context.syscall_arguments();
    let waited = match number {
        number::PSELECT6 => Some(pselect6(process, thread, context)),
        #[cfg(target_arch = "x86_64")]
        number::SELECT => Some(select(process, thread, context)),
        number::EPOLL_PWAIT => Some(epoll::epoll_pwait(process, thread, context)),
        number::EPOLL_PWAIT2 => Some(epoll::epoll_pwait2(process, thread, context)),
        #[cfg(target_arch = "x86_64")]
        number::EPOLL_WAIT => Some(epoll::epoll_wait(process, thread, context)),
        _ => None,
    };
    if let Some(flow) = waited {
        return match flow {
            ControlFlow::Continue(()) => signals::deliver(process, thread, context),
            ended => ended,
        };
    }
    let result = match number {
        number::EPOLL_CREATE1 => epoll::epoll_create1(process, thread, first),
        #[cfg(target_arch = "x86_64")]
        number::EPOLL_CREATE => epoll::epoll_create(process, thread, first),
        number::EPOLL_CTL => epoll::epoll_ctl(process, thread, first, second, third, fourth),
        number::EVENTFD2 => eventfd::eventfd2(process, thread, first, second),
        #[cfg(target_arch = "x86_64")]
        number::EVENTFD => eventfd::eventfd2(process, thread, first, 0),
        number::TIMERFD_CREATE => timerfd::timerfd_create(process, thread, first, second),
        number::TIMERFD_SETTIME => {
            timerfd::timerfd_settime::<P>(process, thread, first, second, third, fourth)
        }
        number::TIMERFD_GETTIME => timerfd::timerfd_gettime::<P>(process, thread, first, second),
        number::SIGNALFD4 => signalfd::signalfd4(process, thread, first, second, third, fourth),
        #[cfg(target_arch = "x86_64")]
        number::SIGNALFD => signalfd::signalfd4(process, thread, first, second, third, 0),
        number::INOTIFY_INIT1 => inotify::inotify_init1(process, thread, first),
        #[cfg(target_arch = "x86_64")]
        number::INOTIFY_INIT => inotify::inotify_init1(process, thread, 0),
        number::INOTIFY_ADD_WATCH => {
            inotify::inotify_add_watch(process, thread, first, second, third)
        }
        number::INOTIFY_RM_WATCH => inotify::inotify_rm_watch(thread, first, second),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPlatform;

    fn no_signals() -> u64 {
        0
    }

    #[test]
    fn the_agent_channel_is_writable_and_readable_only_with_bytes_or_its_end_waiting() {
        let _serial = crate::agent_channel::tests_lock();
        crate::agent_channel::install_fake();
        let both = event::IN | event::OUT;
        assert_eq!(agent_events(both), event::OUT);
        assert_eq!(agent_events(event::IN), 0);
        assert_eq!(agent_events(event::WRNORM), event::WRNORM);
        crate::agent_channel::uninstall();
        assert_eq!(agent_events(both), both);
    }

    #[test]
    fn negative_descriptors_are_skipped_and_closed_ones_report_pollnval() {
        let files = Descriptors::with_console_streams();
        assert!(files.lock().remove(1).is_some());
        let mut entries = [-1, 1, 3, i32::MIN, 0].map(|fd| {
            let mut entry = [0; POLLFD_SIZE];
            entry[..4].copy_from_slice(&fd.to_ne_bytes());
            entry[4..6].copy_from_slice(&event::OUT.to_ne_bytes());
            entry
        });
        assert_eq!(
            fill_in_events::<TestPlatform>(&mut entries, &files, false, &no_signals, poll_count),
            3
        );
        let returned = entries.map(|entry| u16::from_ne_bytes([entry[6], entry[7]]));
        assert_eq!(returned, [0, event::NVAL, event::NVAL, 0, event::OUT]);
    }

    #[test]
    fn select_counts_each_set_whose_events_came_and_ignores_hang_ups_it_did_not_ask_for() {
        assert_eq!(select_count(READ_REQUEST, event::IN), 1);
        assert_eq!(select_count(READ_REQUEST, event::HUP), 1);
        assert_eq!(select_count(WRITE_REQUEST, event::HUP), 0);
        assert_eq!(select_count(WRITE_REQUEST, event::ERR), 1);
        assert_eq!(
            select_count(READ_REQUEST | WRITE_REQUEST, event::IN | event::OUT),
            2
        );
        assert_eq!(select_count(EXCEPT_REQUEST, event::IN), 0);
        assert_eq!(select_count(EXCEPT_REQUEST | READ_REQUEST, event::PRI), 1);
    }

    struct Counting(core::sync::atomic::AtomicU32);

    impl Watcher for Counting {
        fn woken(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_wake_list_wakes_its_watchers_until_they_leave_and_costs_nothing_unwatched() {
        let list = WakeList::new();
        assert!(!list.is_watched());
        list.wake();
        let counting = Arc::new(Counting(core::sync::atomic::AtomicU32::new(0)));
        let watcher: Arc<dyn Watcher> = counting.clone();
        list.watch(&watcher).unwrap();
        list.watch(&watcher).unwrap();
        assert!(list.is_watched());
        list.wake();
        assert_eq!(counting.0.load(Ordering::Relaxed), 2);
        assert!(list.unwatch(&watcher).is_some());
        list.wake();
        assert_eq!(counting.0.load(Ordering::Relaxed), 3);
        assert!(list.unwatch(&watcher).is_some());
        assert!(list.unwatch(&watcher).is_none());
        assert!(!list.is_watched());
        list.wake();
        assert_eq!(counting.0.load(Ordering::Relaxed), 3);
        assert_eq!(Arc::strong_count(&counting), 2);
    }

    #[test]
    fn a_pipe_wakes_only_the_watchers_of_its_own_list() {
        let (reading, writing) = super::super::pipe::new_pair(0).unwrap();
        let (_, other) = super::super::pipe::new_pair(0).unwrap();
        let counting = Arc::new(Counting(core::sync::atomic::AtomicU32::new(0)));
        let watcher: Arc<dyn Watcher> = counting.clone();
        let list = wake_list(reading.kind()).unwrap();
        let other_list = wake_list(other.kind()).unwrap();
        assert!(Arc::ptr_eq(&list, &wake_list(writing.kind()).unwrap()));
        assert!(!Arc::ptr_eq(&list, &other_list));
        list.watch(&watcher).unwrap();
        drop(other);
        assert_eq!(counting.0.load(Ordering::Relaxed), 0);
        drop(writing);
        assert_eq!(counting.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            events::<TestPlatform>(Some(reading.kind()), event::IN, false, &no_signals),
            event::HUP
        );
        drop(list.unwatch(&watcher));
    }

    #[test]
    fn socket_readiness_follows_its_peer() {
        use super::super::descriptors::open_flag;
        use super::super::unix_socket::{self, PeerCredentials, Side, UnixSocket};
        let _serial = unix_socket::tests_lock();
        let (first, second) = UnixSocket::pair(PeerCredentials::NONE);
        let second_file = OpenFile::new(FileKind::Socket(Arc::clone(&second)), open_flag::RDWR);
        let counting = Arc::new(Counting(core::sync::atomic::AtomicU32::new(0)));
        let watcher: Arc<dyn Watcher> = counting.clone();
        second.wake_list().watch(&watcher).unwrap();
        let readable =
            || events::<TestPlatform>(Some(second_file.kind()), event::IN, false, &no_signals);
        assert_eq!(readable(), 0);
        let (connection, side) = first.connected().unwrap();
        assert_eq!(side, Side::First);
        assert_eq!(connection.send(side, b"x", &mut None), Ok(1));
        connection.changed();
        assert_eq!(counting.0.load(Ordering::Relaxed), 1);
        assert_eq!(readable(), event::IN);
        drop(connection);
        drop(first);
        assert!(counting.0.load(Ordering::Relaxed) >= 2);
        drop(second.wake_list().unwatch(&watcher));
    }

    #[test]
    fn closed_descriptors_report_pollnval_whatever_was_asked() {
        assert_eq!(
            events::<TestPlatform>(None, 0, true, &no_signals),
            event::NVAL
        );
        assert_eq!(
            events::<TestPlatform>(None, event::IN, true, &no_signals),
            event::NVAL
        );
    }

    #[test]
    fn the_console_has_room_to_write_when_asked_and_data_to_read_only_when_readable() {
        let all = u16::MAX;
        let console = Some(&FileKind::Console);
        let asked =
            |requested, readable| events::<TestPlatform>(console, requested, readable, &no_signals);
        assert_eq!(asked(0, true), 0);
        assert_eq!(asked(event::IN | event::RDNORM | event::PRI, false), 0);
        assert_eq!(
            asked(event::IN | event::RDNORM | event::PRI, true),
            event::IN | event::RDNORM
        );
        assert_eq!(asked(event::OUT, true), event::OUT);
        assert_eq!(asked(all, false), event::OUT | event::WRNORM);
        assert_eq!(
            asked(all, true),
            event::OUT | event::WRNORM | event::IN | event::RDNORM
        );
    }
}
