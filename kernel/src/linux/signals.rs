//! Signals (M6): their state, `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` (D60),
//! sending and delivering them, `kill`, `tkill`, `tgkill` and `rt_sigpending` (D63), their
//! handlers, which run on Linux's signal frames and return with `rt_sigreturn` (D64), and
//! their `siginfo_t`, queued with each pending signal, several instances of a real-time signal
//! at once, with `rt_sigqueueinfo`, `rt_tgsigqueueinfo` and `rt_sigtimedwait` (D68), and
//! waiting for one to be handled with `rt_sigsuspend` and `pause` (D70) or during `ppoll`
//! with its mask (D71). Rust's
//! std ignores `SIGPIPE` and installs `SIGSEGV` and `SIGBUS` handlers on an alternate stack at
//! startup, glibc blocks and unblocks signals around much of its work, and `abort()` sends
//! itself `SIGABRT` and expects to die of it.
//!
//! A process has an action per signal ([`Action`]), all `SIG_DFL` until it first changes one,
//! which makes its own block on the heap, which also queues its threads' pending signals'
//! [`Info`], each tagged with its thread. Each thread has its own blocked mask, pending signals
//! and alternate stack ([`ThreadSignals`], in its [`LinuxThread`], D91). A signal sent to
//! another thread of the process waits in the process's block until that thread takes it in
//! ([`take_in_sent`], D95): woken from a sleep, interrupted on another CPU, or on its way back
//! from an interrupt or a call that may switch it out. A signal a call makes due (sent to the
//! caller, unblocked, or no longer caught) is delivered before the call returns ([`deliver`]): by its default action or `SIG_IGN`, or by
//! a frame on the program's stack ([`signal_frame`]) from which its handler starts. A fault
//! sends its signal before the program runs again ([`deliver_fault`], D65): to its handler,
//! with the fault's `siginfo_t`, or, as on Linux, by the default action if the program blocks
//! or ignores it.

use super::Errno;
use super::fault::Fault;
use super::signal::{SIGBUS, SIGFPE, SIGILL, SIGKILL, SIGSEGV, SIGSTOP, SIGTRAP};
use super::signal_frame::{self, BadFrame, FpPart, INFO_SIZE, STACK_SIZE, Saved};
use super::thread::LinuxThread;
use crate::process::{Process, ProcessExit};
use crate::thread::{self, FpStart};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::ops::{ControlFlow, Deref, DerefMut};
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::fp::FpState;
use eggos_hal::user::UserContext;

/// How many signals there are: 1 to 64 (`_NSIG`).
pub const COUNT: usize = 64;

/// The first real-time signal (the kernel's `SIGRTMIN`): from it to 64, several instances of a
/// signal may be pending at once, each with its own [`Info`].
pub const FIRST_REAL_TIME: u32 = 32;

/// The size of a signal set (`sigset_t` as the kernel takes it): the calls' `sigsetsize`
/// must be this, but `rt_sigpending`'s may be smaller.
pub const SET_SIZE: u64 = 8;

/// `SIG_DFL`: the signal's default action.
pub const DEFAULT: u64 = 0;
/// `SIG_IGN`: the signal is ignored.
pub const IGNORE: u64 = 1;

/// The flags of a [`Action`] (`SA_*`), the generic values, which x86_64 shares.
pub mod flag {
    pub const NOCLDSTOP: u64 = 0x1;
    pub const NOCLDWAIT: u64 = 0x2;
    pub const SIGINFO: u64 = 0x4;
    /// Never kept: a program sets it to learn whether the kernel clears unknown flags.
    pub const UNSUPPORTED: u64 = 0x400;
    pub const EXPOSE_TAGBITS: u64 = 0x800;
    /// The handler returns through the `restorer` field's code (x86_64 and AArch64 only).
    pub const RESTORER: u64 = 0x0400_0000;
    pub const ONSTACK: u64 = 0x0800_0000;
    pub const RESTART: u64 = 0x1000_0000;
    pub const NODEFER: u64 = 0x4000_0000;
    pub const RESETHAND: u64 = 0x8000_0000;

    /// The flags the kernel keeps; any other bit is cleared, as since Linux 5.11, so the
    /// program sees which ones it lacks.
    pub const KNOWN: u64 = NOCLDSTOP
        | NOCLDWAIT
        | SIGINFO
        | EXPOSE_TAGBITS
        | ONSTACK
        | RESTART
        | NODEFER
        | RESETHAND
        | if super::HAS_RESTORER { RESTORER } else { 0 };
}

/// Whether the architecture's `struct sigaction` has a `sa_restorer` field: RISC-V's does not
/// (its handlers return through the vDSO).
pub const HAS_RESTORER: bool = !cfg!(target_arch = "riscv64");

/// The size of the kernel's `struct sigaction` on the architecture: handler, flags, restorer
/// (if [`HAS_RESTORER`]) and mask, each a 64-bit word.
const ACTION_SIZE: usize = if HAS_RESTORER { 32 } else { 24 };

/// What a signal does when it arrives: the kernel's `struct sigaction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Action {
    /// [`DEFAULT`], [`IGNORE`] or the handler's address.
    pub handler: u64,
    pub flags: u64,
    /// Where the handler returns to, with [`flag::RESTORER`]; always 0 on RISC-V.
    pub restorer: u64,
    /// Signals blocked while the handler runs, bit `n - 1` for signal `n`.
    pub mask: u64,
}

impl Action {
    /// The structure as the program sees it.
    fn to_bytes(self) -> [u8; ACTION_SIZE] {
        let words = if HAS_RESTORER {
            [self.handler, self.flags, self.restorer, self.mask]
        } else {
            [self.handler, self.flags, self.mask, 0]
        };
        let mut bytes = [0; ACTION_SIZE];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *chunk = word.to_ne_bytes();
        }
        bytes
    }

    fn from_bytes(bytes: [u8; ACTION_SIZE]) -> Self {
        let words = bytes.as_chunks::<8>().0;
        let word = |index: usize| u64::from_ne_bytes(words[index]);
        if HAS_RESTORER {
            Self {
                handler: word(0),
                flags: word(1),
                restorer: word(2),
                mask: word(3),
            }
        } else {
            Self {
                handler: word(0),
                flags: word(1),
                restorer: 0,
                mask: word(2),
            }
        }
    }
}

/// The signals no mask may block and no action may catch or ignore.
const UNBLOCKABLE: u64 = bit(SIGKILL) | bit(SIGSTOP);

/// Signal `signal`'s bit in a set (`signal` in 1..=64).
const fn bit(signal: u32) -> u64 {
    1 << (signal - 1)
}

/// A process's signal state: its actions, and the [`Info`] queued with its threads' pending
/// signals. Each thread's own state is a [`ThreadSignals`]; [`LockedSignals`] joins the two.
#[derive(Debug, Default)]
pub struct Signals {
    actions_and_queue: Option<Box<[ActionsAndQueue; 1]>>,
}

/// A thread's signal state with its process's ([`Signals`], whose lock is held), and the
/// thread's id, which tags the [`Info`] queued for it.
#[derive(Debug)]
pub struct LockedSignals<'a> {
    process: &'a mut Signals,
    thread: &'a mut ThreadSignals,
    id: u32,
}

impl Deref for LockedSignals<'_> {
    type Target = Signals;

    fn deref(&self) -> &Signals {
        self.process
    }
}

impl DerefMut for LockedSignals<'_> {
    fn deref_mut(&mut self) -> &mut Signals {
        self.process
    }
}

/// A thread's signal state: what it blocks, what is pending and its alternate stack.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSignals {
    /// The blocked signals, bit `n - 1` for signal `n`; never `SIGKILL` or `SIGSTOP`.
    pub blocked: u64,
    /// The signals sent and not yet delivered, as `blocked`, each once however many of its
    /// instances are queued.
    pub pending: u64,
    /// The alternate stack for handlers with [`flag::ONSTACK`].
    pub alternate_stack: AlternateStack,
}

#[derive(Debug)]
struct ActionsAndQueue {
    handlers: [u64; COUNT],
    flags: [u32; COUNT],
    restorers: [u64; COUNT],
    masks: [u64; COUNT],
    queue: VecDeque<Queued>,
    sent_to_threads: Vec<Queued>,
    threads: Vec<u32>,
    trampoline: u64,
}

/// A pending signal's [`Info`], for the thread whose id is `thread`.
#[derive(Debug, Clone, Copy)]
struct Queued {
    thread: u32,
    info: Info,
}

const _: () = assert!(core::mem::size_of::<ActionsAndQueue>() <= crate::heap::MAX_BLOCK);
const _: () = assert!(flag::KNOWN <= u32::MAX as u64);
const _: () = assert!(core::mem::size_of::<Signals>() == 8);

impl ActionsAndQueue {
    const fn new() -> Self {
        Self {
            handlers: [DEFAULT; COUNT],
            flags: [0; COUNT],
            restorers: [0; COUNT],
            masks: [0; COUNT],
            queue: VecDeque::new(),
            sent_to_threads: Vec::new(),
            threads: Vec::new(),
            trampoline: 0,
        }
    }

    fn action(&self, index: usize) -> Action {
        Action {
            handler: self.handlers[index],
            flags: u64::from(self.flags[index]),
            restorer: self.restorers[index],
            mask: self.masks[index],
        }
    }

    fn set_action(&mut self, index: usize, action: Action) {
        self.handlers[index] = action.handler;
        self.flags[index] = action.flags as u32;
        self.restorers[index] = action.restorer;
        self.masks[index] = action.mask;
    }
}

impl Drop for ActionsAndQueue {
    fn drop(&mut self) {
        uncharge(
            &QUEUED_SIGNALS,
            self.queue.len() + self.sent_to_threads.len(),
        );
    }
}

static QUEUED_SIGNALS: AtomicU64 = AtomicU64::new(0);

fn try_charge(queued: &AtomicU64, limit: u64) -> bool {
    queued
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < limit).then_some(count + 1)
        })
        .is_ok()
}

fn charge(queued: &AtomicU64) {
    queued.fetch_add(1, Ordering::Relaxed);
}

fn uncharge(queued: &AtomicU64, entries: usize) {
    queued.fetch_sub(entries as u64, Ordering::Relaxed);
}

/// How many records ([`KEPT_INFO_SIZE`] bytes each) a drained queue keeps room for.
const DRAINED_QUEUE_CAPACITY: usize = 32;

/// The bytes of a `siginfo_t` the kernel keeps with a pending signal: `si_signo`, `si_errno`,
/// `si_code` and the union of fields after them. A program always gets the rest as zeros.
pub const KEPT_INFO_SIZE: usize = 48;

/// `si_code`: who sent a signal, the generic values, which x86_64 shares.
pub mod code {
    /// With `kill` (`SI_USER`).
    pub const USER: i32 = 0;
    /// By the kernel (`SI_KERNEL`), as for a signal frame that could not be written.
    pub const KERNEL: i32 = 0x80;
    /// With `sigqueue`, through `rt_sigqueueinfo` (`SI_QUEUE`).
    pub const QUEUE: i32 = -1;
    /// With `tkill` or `tgkill` (`SI_TKILL`).
    pub const TKILL: i32 = -6;
}

/// A pending signal's `siginfo_t`, as the kernel keeps it ([`KEPT_INFO_SIZE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info([u8; KEPT_INFO_SIZE]);

impl Info {
    /// Signal `signal` sent with `code` by process `pid`, whose real user id is `uid`.
    pub fn sent(signal: u32, code: i32, pid: u32, uid: u32) -> Self {
        let mut info = Self::without_sender(signal, code);
        info.0[16..20].copy_from_slice(&pid.to_ne_bytes());
        info.0[20..24].copy_from_slice(&uid.to_ne_bytes());
        info
    }

    /// Signal `signal` raised by the kernel ([`code::KERNEL`]).
    pub fn from_kernel(signal: u32) -> Self {
        Self::without_sender(signal, code::KERNEL)
    }

    /// The `siginfo_t` a program queues signal `signal` with: as written, but with `signal` as
    /// its `si_signo`.
    pub fn queued(signal: u32, bytes: [u8; KEPT_INFO_SIZE]) -> Self {
        let mut info = Self(bytes);
        info.0[..4].copy_from_slice(&(signal as i32).to_ne_bytes());
        info
    }

    fn without_sender(signal: u32, code: i32) -> Self {
        let mut info = Self([0; KEPT_INFO_SIZE]);
        info.0[..4].copy_from_slice(&(signal as i32).to_ne_bytes());
        info.0[8..12].copy_from_slice(&code.to_ne_bytes());
        info
    }

    fn lost(signal: u32) -> Self {
        Self::without_sender(signal, code::USER)
    }

    /// Its signal, 1 to 64.
    pub fn signal(&self) -> u32 {
        u32::from_ne_bytes(self.0[..4].try_into().unwrap())
    }

    /// Its `si_code`.
    pub fn code(&self) -> i32 {
        i32::from_ne_bytes(self.0[8..12].try_into().unwrap())
    }

    /// The whole `siginfo_t`, as a handler and `rt_sigtimedwait` give it to the program.
    pub fn to_bytes(self) -> [u8; INFO_SIZE] {
        let mut bytes = [0; INFO_SIZE];
        bytes[..KEPT_INFO_SIZE].copy_from_slice(&self.0);
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    Queued,
    Unobservable,
    Lost,
}

/// A real-time signal could not be queued: the queued signals reached `RLIMIT_SIGPENDING`, or
/// there was no memory for another (`EAGAIN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull;

/// An alternate signal stack: `size` bytes from `base`, or none when `size` is 0.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AlternateStack {
    pub base: u64,
    pub size: u64,
    /// `SS_AUTODISARM`: a handler that starts on the stack disables it until it returns.
    pub autodisarm: bool,
}

/// `sigaltstack`'s flags (`SS_*`).
mod stack_flag {
    /// The thread is running on the stack (reported), or the stack is enabled (accepted as 0).
    pub const ONSTACK: u32 = 1;
    /// No alternate stack.
    pub const DISABLE: u32 = 2;
    pub const AUTODISARM: u32 = 1 << 31;
}

/// The smallest alternate stack (`MINSIGSTKSZ`): room for the kernel's largest signal frame on
/// the architecture. Programs read it as `AT_MINSIGSTKSZ` too.
pub const MIN_STACK_SIZE: u64 = if cfg!(target_arch = "aarch64") {
    5120
} else {
    2048
};
const _: () = assert!(signal_frame::LARGEST_FRAME_SPACE <= MIN_STACK_SIZE);

impl AlternateStack {
    /// Whether the thread, whose stack pointer is `stack`, is on it: above its base and at
    /// most at its top, as the stack grows down (a pointer at the top is the empty stack's).
    /// Never with `SS_AUTODISARM`: the stack is disarmed while a handler runs on it.
    fn is_on(&self, stack: u64) -> bool {
        !self.autodisarm && stack.wrapping_sub(self.base).wrapping_sub(1) < self.size
    }

    /// `stack_t` as the program sees it (from `sigaltstack`, or in a signal frame's
    /// `uc_stack`), for a thread whose stack pointer is `stack`.
    fn to_bytes(self, stack: u64) -> [u8; STACK_SIZE] {
        let mut flags = if self.size == 0 {
            stack_flag::DISABLE
        } else if self.is_on(stack) {
            stack_flag::ONSTACK
        } else {
            0
        };
        if self.autodisarm {
            flags |= stack_flag::AUTODISARM;
        }
        let mut bytes = [0; STACK_SIZE];
        bytes[..8].copy_from_slice(&self.base.to_ne_bytes());
        bytes[8..12].copy_from_slice(&flags.to_ne_bytes());
        bytes[16..].copy_from_slice(&self.size.to_ne_bytes());
        bytes
    }
}

/// Why an action could not be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeError {
    /// No signal has that number, or it is `SIGKILL` or `SIGSTOP`.
    Invalid,
    /// There was no memory for the process's own table.
    OutOfMemory,
}

impl ThreadSignals {
    /// Nothing blocked or pending, no alternate stack.
    pub const fn new() -> Self {
        Self {
            blocked: 0,
            pending: 0,
            alternate_stack: AlternateStack {
                base: 0,
                size: 0,
                autodisarm: false,
            },
        }
    }

    /// The state of a thread that the thread with this state starts (`clone`): the same
    /// blocked signals, nothing pending and, as on Linux for a thread sharing its starter's
    /// memory, no alternate stack.
    pub const fn for_new_thread(&self) -> Self {
        Self {
            blocked: self.blocked,
            ..Self::new()
        }
    }
}

impl Signals {
    /// No handlers and nothing queued.
    pub const fn new() -> Self {
        Self {
            actions_and_queue: None,
        }
    }

    /// The state of the thread whose id is `id` and whose own state is `thread`, joined with
    /// this, its process's, for as long as the caller holds both.
    pub fn with_thread<'a>(
        &'a mut self,
        thread: &'a mut ThreadSignals,
        id: u32,
    ) -> LockedSignals<'a> {
        LockedSignals {
            process: self,
            thread,
            id,
        }
    }

    /// Signal `signal`'s action, or `None` for a number with no signal.
    pub fn action(&self, signal: u32) -> Option<Action> {
        let index = (signal as usize).checked_sub(1).filter(|&i| i < COUNT)?;
        Some(
            self.actions_and_queue
                .as_deref()
                .map_or_else(Action::default, |[block]| block.action(index)),
        )
    }

    /// Whether `signal` (1 to 64) is discarded when it arrives: ignored, by `SIG_IGN` or by
    /// its default action.
    fn ignores(&self, signal: u32) -> bool {
        self.action(signal)
            .is_some_and(|action| match action.handler {
                IGNORE => true,
                DEFAULT => default_action(signal) == DefaultAction::Ignore,
                _ => false,
            })
    }

    /// Gives an empty queue's memory back past [`DRAINED_QUEUE_CAPACITY`] records, so a burst
    /// of queued signals costs little once they are taken, while a signal sent and taken at a
    /// time needs no allocation.
    fn free_drained_queue(&mut self) {
        if let Some([block]) = self.actions_and_queue.as_deref_mut()
            && block.queue.is_empty()
            && block.queue.capacity() > DRAINED_QUEUE_CAPACITY
        {
            block.queue.shrink_to(DRAINED_QUEUE_CAPACITY);
        }
    }

    /// Sets `signal`'s handler to `SIG_DFL`, keeping its flags and mask, as Linux does for
    /// [`flag::RESETHAND`] and a forced signal. Nothing to do without a table: every handler
    /// is `SIG_DFL` then.
    fn reset_handler(&mut self, signal: u32) {
        if let Some([block]) = self.actions_and_queue.as_deref_mut() {
            block.handlers[signal as usize - 1] = DEFAULT;
        }
    }

    /// Counts thread `id`, not the process's first, as one of the process's threads, which
    /// [`Self::send_to_thread`] can reach until [`Self::remove_thread`]. `Err` without memory
    /// for the record.
    pub fn add_thread(&mut self, id: u32) -> Result<(), QueueFull> {
        if self.actions_and_queue.is_none() {
            self.actions_and_queue = Some(actions_and_queue_block().ok_or(QueueFull)?);
        }
        let [block] = &mut **self.actions_and_queue.as_mut().ok_or(QueueFull)?;
        block.threads.try_reserve(1).map_err(|_| QueueFull)?;
        block.threads.push(id);
        Ok(())
    }

    /// Thread `id` has exited: no signal reaches it any more, and those sent to it and not yet
    /// taken in go.
    pub fn remove_thread(&mut self, id: u32) {
        if let Some([block]) = self.actions_and_queue.as_deref_mut() {
            block.threads.retain(|&thread| thread != id);
            let sent = block.sent_to_threads.len();
            block.sent_to_threads.retain(|queued| queued.thread != id);
            uncharge(&QUEUED_SIGNALS, sent - block.sent_to_threads.len());
        }
    }

    /// Whether `id` is one of the process's threads other than its first that has not exited.
    pub fn has_thread(&self, id: u32) -> bool {
        self.actions_and_queue
            .as_deref()
            .is_some_and(|[block]| block.threads.contains(&id))
    }

    /// Sends the signal `info` describes to thread `id`, another of the process's threads,
    /// which takes it in ([`LockedSignals::take_in_sent`]) and handles it as if sent to itself
    /// then ([`LockedSignals::send`]). It counts toward `RLIMIT_SIGPENDING`, `queue_limit`,
    /// meanwhile: a real-time signal that finds the queued signals at the limit is not sent
    /// ([`QueueFull`]), as on Linux, nor is any signal without memory to record it; a standard
    /// signal is queued past the limit, keeping its `siginfo_t`.
    pub fn send_to_thread(
        &mut self,
        id: u32,
        info: Info,
        queue_limit: u64,
    ) -> Result<(), QueueFull> {
        let exempt_from_limit = info.signal() < FIRST_REAL_TIME;
        let [block] = &mut **self.actions_and_queue.as_mut().ok_or(QueueFull)?;
        block
            .sent_to_threads
            .try_reserve(1)
            .map_err(|_| QueueFull)?;
        if exempt_from_limit {
            charge(&QUEUED_SIGNALS);
        } else if !try_charge(&QUEUED_SIGNALS, queue_limit) {
            return Err(QueueFull);
        }
        block.sent_to_threads.push(Queued { thread: id, info });
        Ok(())
    }

    /// The address of the process's return trampoline, 0 until it is mapped.
    pub fn trampoline(&self) -> u64 {
        self.actions_and_queue
            .as_deref()
            .map_or(0, |[block]| block.trampoline)
    }

    /// Records where the return trampoline was mapped. Only a process with a handler, which
    /// has its own table, has one. It stays there: a program that unmaps or replaces the page
    /// breaks its own handlers' return, as it would by unmapping Linux's vDSO.
    pub fn set_trampoline(&mut self, address: u64) {
        if let Some([block]) = self.actions_and_queue.as_deref_mut() {
            block.trampoline = address;
        }
    }
}

impl LockedSignals<'_> {
    /// The thread's own state.
    pub fn thread(&self) -> ThreadSignals {
        *self.thread
    }

    /// Sets `signal`'s action to `new`, without its unknown flags and with `SIGKILL` and
    /// `SIGSTOP` left out of its mask, and returns the action it had. The first change that
    /// changes anything makes the process's own table. An action that ignores the signal
    /// discards it if pending, blocked or not, as POSIX requires.
    fn set_action(&mut self, signal: u32, new: Action) -> Result<Action, ChangeError> {
        let current = self.action(signal).ok_or(ChangeError::Invalid)?;
        if bit(signal) & UNBLOCKABLE != 0 {
            return Err(ChangeError::Invalid);
        }
        let new = Action {
            flags: new.flags & flag::KNOWN,
            mask: new.mask & !UNBLOCKABLE,
            ..new
        };
        if new != current {
            let [block] = &mut **match &mut self.actions_and_queue {
                Some(block) => block,
                None => self
                    .actions_and_queue
                    .insert(actions_and_queue_block().ok_or(ChangeError::OutOfMemory)?),
            };
            block.set_action(signal as usize - 1, new);
        }
        if self.ignores(signal) {
            self.discard(bit(signal));
        }
        Ok(current)
    }

    /// Sends the signal `info` describes to the thread: pending until [`Self::next_delivery`]
    /// or [`Self::take_waited`] takes it, unless it is ignored and not blocked, which discards
    /// it at once, as on Linux (a blocked signal stays pending even if ignored, and
    /// `rt_sigpending` reports it). As POSIX requires, `SIGCONT` discards pending stop signals
    /// and a stop signal a pending `SIGCONT`, whatever their actions. A standard signal already
    /// pending is not sent again, and keeps the `info` it was first sent with; each instance of
    /// a real-time signal is queued with its own, in the order sent.
    ///
    /// Each queued `info` counts toward `RLIMIT_SIGPENDING`, `queue_limit` here, as on Linux:
    /// a real-time signal not sent with `kill` that finds the queued signals at the limit, or
    /// no memory, is not sent ([`QueueFull`]); any other signal is then pending without its
    /// `info`, and is taken as `SI_USER` from no process. A standard signal sent with `kill` or
    /// by the kernel is queued past the limit. The `info` of a signal that is not blocked is
    /// kept only by a process that has its own actions: in another, it is delivered by its
    /// default action before the program could look at it.
    pub fn send(&mut self, info: Info, queue_limit: u64) -> Result<(), QueueFull> {
        let signal = info.signal();
        if signal == SIGCONT {
            self.discard(STOP_SIGNALS);
        } else if bit(signal) & STOP_SIGNALS != 0 {
            self.discard(bit(SIGCONT));
        }
        let blocked = self.thread.blocked & bit(signal) != 0;
        if !blocked && self.ignores(signal) {
            return Ok(());
        }
        let real_time = signal >= FIRST_REAL_TIME;
        if !real_time && self.thread.pending & bit(signal) != 0 {
            return Ok(());
        }
        let recorded = self.record(info, blocked, queue_limit);
        if recorded == Recorded::Lost && real_time && info.code() != code::USER {
            return Err(QueueFull);
        }
        self.thread.pending |= bit(signal);
        Ok(())
    }

    fn record(&mut self, info: Info, blocked: bool, queue_limit: u64) -> Recorded {
        let exempt_from_limit = info.signal() < FIRST_REAL_TIME && info.code() >= 0;
        if exempt_from_limit {
            charge(&QUEUED_SIGNALS);
        } else if !try_charge(&QUEUED_SIGNALS, queue_limit) {
            return Recorded::Lost;
        }
        if self.actions_and_queue.is_none() && blocked {
            self.actions_and_queue = actions_and_queue_block();
        }
        let thread = self.id;
        let recorded = match self.actions_and_queue.as_deref_mut() {
            None if !blocked => Recorded::Unobservable,
            None => Recorded::Lost,
            Some([block]) => match block.queue.try_reserve(1) {
                Ok(()) => {
                    block.queue.push_back(Queued { thread, info });
                    Recorded::Queued
                }
                Err(_) => Recorded::Lost,
            },
        };
        if recorded != Recorded::Queued {
            uncharge(&QUEUED_SIGNALS, 1);
        }
        recorded
    }

    /// Sends `signal` from the kernel so that it is delivered whatever the program asked, as
    /// Linux's `force_sig`: an ignored or blocked signal's action becomes `SIG_DFL` (which
    /// ends the process for the signals forced) and a blocked one is unblocked.
    pub fn force(&mut self, signal: u32) {
        let index = signal as usize - 1;
        let blocked = self.thread.blocked & bit(signal) != 0;
        if let Some([block]) = self.actions_and_queue.as_deref_mut()
            && (blocked || block.handlers[index] == IGNORE)
        {
            block.handlers[index] = DEFAULT;
        }
        self.thread.blocked &= !bit(signal);
        let _ = self.send(Info::from_kernel(signal), u64::MAX);
    }

    /// Takes in the signals other threads sent this one ([`Signals::send_to_thread`]), in the
    /// order sent, as if sent now, and discards its pending signals that are now ignored, as
    /// POSIX requires once another thread sets `SIG_IGN`, blocked or not. Returns whether
    /// either changed its pending signals.
    pub fn take_in_sent(&mut self) -> bool {
        let before = self.thread.pending;
        let thread = self.id;
        if let Some([block]) = self.actions_and_queue.as_deref_mut()
            && block
                .sent_to_threads
                .iter()
                .any(|queued| queued.thread == thread)
        {
            let mut sent = core::mem::take(&mut block.sent_to_threads);
            let all = sent.len();
            sent.retain(|queued| {
                if queued.thread != thread {
                    return true;
                }
                uncharge(&QUEUED_SIGNALS, 1);
                let _ = self.send(queued.info, u64::MAX);
                false
            });
            debug_assert!(sent.len() < all);
            if let Some([block]) = self.actions_and_queue.as_deref_mut() {
                block.sent_to_threads = sent;
            }
        }
        let mut pending = self.thread.pending;
        let mut ignored = 0;
        while pending != 0 {
            let signal = pending.trailing_zeros() + 1;
            pending &= pending - 1;
            if self.ignores(signal) {
                ignored |= bit(signal);
            }
        }
        if ignored != 0 {
            self.discard(ignored);
        }
        self.thread.pending != before || ignored != 0
    }

    /// Discards the thread's pending signals in `set`, with their queued [`Info`].
    pub fn discard(&mut self, set: u64) {
        self.thread.pending &= !set;
        let thread = self.id;
        if let Some([block]) = self.actions_and_queue.as_deref_mut() {
            let queued = block.queue.len();
            block
                .queue
                .retain(|queued| queued.thread != thread || bit(queued.info.signal()) & set == 0);
            uncharge(&QUEUED_SIGNALS, queued - block.queue.len());
        }
        self.free_drained_queue();
    }

    fn take(&mut self, signal: u32) -> Info {
        let mut more_queued = false;
        let thread = self.id;
        let is_this = |queued: &Queued| queued.thread == thread && queued.info.signal() == signal;
        let taken = self.actions_and_queue.as_deref_mut().and_then(|[block]| {
            let first = block.queue.iter().position(is_this)?;
            let taken = block.queue.remove(first)?;
            uncharge(&QUEUED_SIGNALS, 1);
            more_queued = block.queue.iter().skip(first).any(is_this);
            Some(taken.info)
        });
        if !more_queued {
            self.thread.pending &= !bit(signal);
        }
        self.free_drained_queue();
        taken.unwrap_or_else(|| Info::lost(signal))
    }

    fn first_to_deliver(&self, due: u64) -> u32 {
        if due & bit(SIGKILL) != 0 {
            return SIGKILL;
        }
        if due & SYNCHRONOUS == 0 {
            return first_to_take(due);
        }
        let thread = self.id;
        let first_raised_by_kernel = self.actions_and_queue.as_deref().and_then(|[block]| {
            block.queue.iter().find(|queued| {
                queued.thread == thread
                    && queued.info.code() > 0
                    && bit(queued.info.signal()) & due & SYNCHRONOUS != 0
            })
        });
        match first_raised_by_kernel {
            Some(queued) => queued.info.signal(),
            None => first_to_take(due),
        }
    }

    /// Takes the next pending, unblocked signal, discarding the ignored ones on the way: `Some`
    /// for one that ends the process (default actions Term and Core, `SIGKILL`'s included),
    /// stops it, or runs its handler. The order: `SIGKILL`; then, as on Linux, the synchronous
    /// signals (`SIGILL`, `SIGTRAP`, `SIGBUS`, `SIGFPE`, `SIGSEGV`, `SIGSYS`), the first the
    /// kernel raised (a positive `si_code`) in the order raised, then the lowest, so a fault's
    /// or a failed frame's `SIGSEGV` is never overtaken; then the lowest number, and a
    /// real-time signal's instances in the order sent. A handler with [`flag::RESETHAND`] is
    /// removed as it is taken, as on Linux: its signal's next delivery is by the default
    /// action.
    pub fn next_delivery(&mut self) -> Option<Delivery> {
        loop {
            let due = self.thread.pending & !self.thread.blocked;
            if due == 0 {
                return None;
            }
            let signal = self.first_to_deliver(due);
            let action = self.action(signal).unwrap_or_default();
            let info = self.take(signal);
            let delivery = match action.handler {
                IGNORE => None,
                DEFAULT => match default_action(signal) {
                    DefaultAction::Terminate | DefaultAction::Core => Some(Delivery::Kill(signal)),
                    DefaultAction::Ignore | DefaultAction::Continue => None,
                    DefaultAction::Stop if signal != SIGSTOP => None,
                    DefaultAction::Stop => Some(Delivery::Stop(signal)),
                },
                _ => {
                    if action.flags & flag::RESETHAND != 0 {
                        self.reset_handler(signal);
                    }
                    Some(Delivery::Handler {
                        signal,
                        action,
                        info,
                    })
                }
            };
            if delivery.is_some() {
                return delivery;
            }
        }
    }

    /// Takes the first pending signal in `wanted`, blocked or not, with its [`Info`], as
    /// `rt_sigtimedwait` does: the lowest synchronous one, then the lowest, and a real-time
    /// signal's instances in the order sent. Unlike [`Self::next_delivery`], who raised a
    /// synchronous signal does not matter, as on Linux.
    pub fn take_waited(&mut self, wanted: u64) -> Option<Info> {
        let due = self.thread.pending & wanted;
        (due != 0).then(|| self.take(first_to_take(due)))
    }

    /// The thread enters `action`'s handler for `signal`, its frame written: the action's mask
    /// is blocked too, and the signal itself unless [`flag::NODEFER`]; an alternate stack with
    /// `SS_AUTODISARM` is disabled on entry to the handler, as sigaltstack(2) says, until the
    /// handler returns and `rt_sigreturn` restores it from the frame.
    fn enter_handler(&mut self, signal: u32, action: &Action) {
        let mut blocked = self.thread.blocked | action.mask;
        if action.flags & flag::NODEFER == 0 {
            blocked |= bit(signal);
        }
        self.thread.blocked = blocked & !UNBLOCKABLE;
        if self.thread.alternate_stack.autodisarm {
            self.thread.alternate_stack = AlternateStack::default();
        }
    }

    /// Changes the thread's alternate stack to `size` bytes from `base` with `sigaltstack`'s
    /// `flags`, for a thread whose stack pointer is `stack`: `EPERM` while it is on its
    /// alternate stack, `EINVAL` for unknown flags, `ENOMEM` for a stack smaller than
    /// [`MIN_STACK_SIZE`] (see [`sigaltstack`]). A disabled stack keeps no base or size.
    fn change_alternate_stack(
        &mut self,
        base: u64,
        flags: u32,
        size: u64,
        stack: u64,
    ) -> Result<(), Errno> {
        if self.thread.alternate_stack.is_on(stack) {
            return Err(Errno::EPERM);
        }
        let autodisarm = flags & stack_flag::AUTODISARM != 0;
        self.thread.alternate_stack = match flags & !stack_flag::AUTODISARM {
            0 | stack_flag::ONSTACK if size < MIN_STACK_SIZE => return Err(Errno::ENOMEM),
            0 | stack_flag::ONSTACK => AlternateStack {
                base,
                size,
                autodisarm,
            },
            stack_flag::DISABLE => AlternateStack {
                autodisarm,
                ..AlternateStack::default()
            },
            _ => return Err(Errno::EINVAL),
        };
        Ok(())
    }

    /// What `rt_sigreturn` restores from a handler's frame: the blocked mask, without
    /// `SIGKILL` and `SIGSTOP`, and the alternate stack the `stack_t` at `alternate_stack`
    /// describes, unless changing it fails (the thread, whose stack pointer is `stack`, is on
    /// its alternate stack, or the stack is invalid), which Linux ignores too.
    fn restore(&mut self, blocked: u64, alternate_stack: [u8; STACK_SIZE], stack: u64) {
        self.thread.blocked = blocked & !UNBLOCKABLE;
        let (base, flags, size) = parse_stack(alternate_stack);
        let _ = self.change_alternate_stack(base, flags, size, stack);
    }

    /// The pending signals that are blocked, as `rt_sigpending` reports them.
    fn blocked_pending(&self) -> u64 {
        self.thread.pending & self.thread.blocked
    }
}

/// `SIGCONT`'s number.
const SIGCONT: u32 = 18;

const SIGSYS: u32 = 31;

const SYNCHRONOUS: u64 =
    bit(SIGILL) | bit(SIGTRAP) | bit(SIGBUS) | bit(SIGFPE) | bit(SIGSEGV) | bit(SIGSYS);

/// The first signal of the set `due` (not empty) to take: the lowest synchronous one, if any,
/// else the lowest.
fn first_to_take(due: u64) -> u32 {
    let synchronous = due & SYNCHRONOUS;
    let first = if synchronous != 0 { synchronous } else { due };
    first.trailing_zeros() + 1
}

/// The stop signals: `SIGSTOP`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU`.
const STOP_SIGNALS: u64 = bit(SIGSTOP) | bit(20) | bit(21) | bit(22);

/// What delivering a signal does, for [`LockedSignals::next_delivery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The process ends with this signal.
    Kill(u32),
    /// The process stops until `SIGCONT` or `SIGKILL`.
    Stop(u32),
    /// `action`'s handler runs for `signal`, with its `info`.
    Handler {
        signal: u32,
        action: Action,
        info: Info,
    },
}

/// What a signal does when its action is `SIG_DFL` (signal(7)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    /// The process ends with the signal.
    Terminate,
    /// The process ends with the signal and would dump core (there are no core dumps yet).
    Core,
    /// The signal is discarded.
    Ignore,
    /// The process stops until `SIGCONT` or `SIGKILL`.
    Stop,
    /// A stopped process continues; otherwise the signal is discarded.
    Continue,
}

/// Signal `signal`'s default action (1 to 64; the generic numbers, which AArch64, RISC-V and
/// x86_64 share), as signal(7) lists it. Every real-time signal, 32 to 64, terminates.
pub const fn default_action(signal: u32) -> DefaultAction {
    use DefaultAction::*;
    match signal {
        // SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV, SIGXCPU, SIGXFSZ, SIGSYS.
        3..=8 | 11 | 24 | 25 | 31 => Core,
        // SIGCHLD, SIGURG, SIGWINCH.
        17 | 23 | 28 => Ignore,
        // SIGCONT.
        18 => Continue,
        // SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU.
        19..=22 => Stop,
        // SIGHUP, SIGINT, SIGKILL, SIGUSR1, SIGUSR2, SIGPIPE, SIGALRM, SIGTERM, SIGSTKFLT,
        // SIGVTALRM, SIGPROF, SIGIO, SIGPWR, and the real-time signals.
        _ => Terminate,
    }
}

fn actions_and_queue_block() -> Option<Box<[ActionsAndQueue; 1]>> {
    let mut block = Vec::new();
    block.try_reserve_exact(1).ok()?;
    block.push(ActionsAndQueue::new());
    block.into_boxed_slice().try_into().ok()
}

/// The address `address` as an optional pointer: `None` when null.
fn pointer(address: u64) -> Option<u64> {
    (address != 0).then_some(address)
}

/// `rt_sigaction(signal, new, old, sigsetsize)`: sets `signal`'s action to the structure at
/// `new`, unless that is null, and copies the one it had to `old`, unless that is null. As
/// Linux, in this order: `EINVAL` if `sigsetsize` is not 8; `EFAULT` if `new` is not the
/// program's memory; `EINVAL` for a number outside 1 to 64, or a new action for `SIGKILL` or
/// `SIGSTOP` (whose action may still be read); `ENOMEM` if the process's first change finds no
/// memory for its own table; then, with the new action set, `EFAULT` if `old` is not the
/// program's writable memory. `signal` (an `int`) is 32-bit. Unknown flags are cleared, so
/// `old` shows which ones the kernel kept.
pub fn rt_sigaction<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    signal: u64,
    new: u64,
    old: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    if set_size != SET_SIZE {
        return Err(Errno::EINVAL);
    }
    let new = match pointer(new) {
        None => None,
        Some(address) => {
            let mut bytes = [0; ACTION_SIZE];
            process
                .read_user_memory(address, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            Some(Action::from_bytes(bytes))
        }
    };
    let signal = signal as u32;
    // Copied out before the program's memory is touched: the lock is never held across a copy.
    let (previous, ignores) = {
        let mut guard = process.signals().lock();
        let mut signals = thread.signals(&mut guard);
        let previous = match new {
            None => signals.action(signal).ok_or(Errno::EINVAL)?,
            Some(new) => signals
                .set_action(signal, new)
                .map_err(|error| match error {
                    ChangeError::Invalid => Errno::EINVAL,
                    ChangeError::OutOfMemory => Errno::ENOMEM,
                })?,
        };
        (previous, new.is_some() && signals.ignores(signal))
    };
    if ignores && process.threads() > 1 {
        process.record_signal_sent();
    }
    if let Some(address) = pointer(old) {
        process
            .write_user_memory(address, &previous.to_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

/// `rt_sigprocmask`'s `how`.
mod how {
    /// Block the set's signals too.
    pub const BLOCK: u32 = 0;
    /// Unblock the set's signals.
    pub const UNBLOCK: u32 = 1;
    /// Block exactly the set's signals.
    pub const SET_MASK: u32 = 2;
}

/// `rt_sigprocmask(how, set, old, sigsetsize)`: changes the thread's blocked signals by the
/// set at `set` as `how` says, unless `set` is null, and copies the ones it blocked before to
/// `old`, unless that is null. `SIGKILL` and `SIGSTOP` are never blocked; asking is not an
/// error. As Linux, in this order: `EINVAL` if `sigsetsize` is not 8; `EFAULT` if `set` is not
/// the program's memory; `EINVAL` for an unknown `how` (an `int`, only looked at with a set);
/// then, with the mask changed, `EFAULT` if `old` is not the program's writable memory.
pub fn rt_sigprocmask<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    how: u64,
    set: u64,
    old: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    if set_size != SET_SIZE {
        return Err(Errno::EINVAL);
    }
    let set = match pointer(set) {
        None => None,
        Some(address) => {
            let mut bytes = [0; SET_SIZE as usize];
            process
                .read_user_memory(address, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let set = u64::from_ne_bytes(bytes);
            let how = how as u32;
            if how > how::SET_MASK {
                return Err(Errno::EINVAL);
            }
            Some((how, set))
        }
    };
    let previous = {
        let mut guard = process.signals().lock();
        let signals = thread.signals(&mut guard);
        let blocked = &mut signals.thread.blocked;
        let previous = *blocked;
        if let Some((how, set)) = set {
            *blocked = match how {
                how::BLOCK => previous | set,
                how::UNBLOCK => previous & !set,
                _ => set,
            } & !UNBLOCKABLE;
        }
        previous
    };
    if let Some(address) = pointer(old) {
        process
            .write_user_memory(address, &previous.to_ne_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

/// `sigaltstack(new, old)`: sets the thread's alternate stack to the `stack_t` at `new`, unless
/// that is null, and copies the one it had to `old`, unless that is null, its flags saying
/// whether it is disabled or the thread (whose stack pointer is `stack`) is on it. As Linux,
/// in this order: `EFAULT` if `new` is not the program's memory; `EPERM` if the thread is on
/// its alternate stack (which cannot change under a running handler); `EINVAL` for flags other
/// than 0, `SS_ONSTACK` (taken as 0) or `SS_DISABLE`, each with or without `SS_AUTODISARM`;
/// `ENOMEM` for a stack smaller than `MINSIGSTKSZ`; then, with the new stack set, `EFAULT` if
/// `old` is not the program's writable memory. A disabled stack keeps no base or size.
pub fn sigaltstack<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    new: u64,
    old: u64,
    stack: u64,
) -> Result<u64, Errno> {
    let new = match pointer(new) {
        None => None,
        Some(address) => {
            let mut bytes = [0; STACK_SIZE];
            process
                .read_user_memory(address, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            Some(parse_stack(bytes))
        }
    };
    let previous = {
        let mut guard = process.signals().lock();
        let mut signals = thread.signals(&mut guard);
        let current = signals.thread.alternate_stack;
        if let Some((base, flags, size)) = new {
            signals.change_alternate_stack(base, flags, size, stack)?;
        }
        current
    };
    if let Some(address) = pointer(old) {
        process
            .write_user_memory(address, &previous.to_bytes(stack))
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

/// A `stack_t`'s base, flags and size.
fn parse_stack(bytes: [u8; STACK_SIZE]) -> (u64, u32, u64) {
    let base = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let flags = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    let size = u64::from_ne_bytes(bytes[16..].try_into().unwrap());
    (base, flags, size)
}

fn send_to_self<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    signal: u64,
    info: impl FnOnce(u32) -> Info,
) -> Result<u64, Errno> {
    match signal as u32 {
        0 => Ok(0),
        signal if signal as usize <= COUNT => {
            let queue_limit = queue_limit(process);
            thread
                .signals(&mut process.signals().lock())
                .send(info(signal), queue_limit)
                .map(|()| 0)
                .map_err(|QueueFull| Errno::EAGAIN)
        }
        _ => Err(Errno::EINVAL),
    }
}

fn queue_limit<P: Platform>(process: &Process<P>) -> u64 {
    process
        .credentials_and_limits()
        .lock()
        .limits
        .get(super::limits::resource::SIGPENDING)
        .map_or(u64::MAX, |limit| limit.soft)
}

/// What thread `id` of `process` is to a signal sent to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A thread that has not exited: its first thread, or one [`Signals::add_thread`] counted.
    Live,
    /// Its first thread, exited while others go on: as Linux's thread group leader, it still
    /// takes signals, which it never delivers.
    ExitedFirst,
    /// No thread of the process.
    None,
}

fn target<P: Platform>(process: &Process<P>, signals: &Signals, id: u32) -> Target {
    if id == process.id().get() {
        match process.first_thread_status() {
            None => Target::Live,
            Some(_) => Target::ExitedFirst,
        }
    } else if signals.has_thread(id) {
        Target::Live
    } else {
        Target::None
    }
}

/// Sends `signal` to thread `id` of the caller's process, as `tgkill` and `rt_tgsigqueueinfo`
/// do: to the caller as [`send_to_self`]; to another thread, `ESRCH` unless it is one of the
/// process's ([`Target`]), then `EINVAL` for a signal outside 0 to 64, and 0 only checks.
/// `SIGKILL` ends the process at once, as on Linux; another signal waits in the process's
/// signal state until the thread takes it in ([`take_in_sent`]), which it is woken from a
/// sleep and interrupted to do, or, for a first thread that has exited, is accepted and goes
/// nowhere. `EAGAIN` as [`Signals::send_to_thread`] says.
fn send_to_thread<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    id: u32,
    signal: u64,
    info: impl FnOnce(u32) -> Info,
) -> Result<u64, Errno> {
    if id == thread.id() {
        return send_to_self(process, thread, signal, info);
    }
    let queue_limit = queue_limit(process);
    {
        let mut signals = process.signals().lock();
        let target = target(process, &signals, id);
        if target == Target::None {
            return Err(Errno::ESRCH);
        }
        let signal = match signal as u32 {
            0 => return Ok(0),
            signal if signal as usize <= COUNT => signal,
            _ => return Err(Errno::EINVAL),
        };
        if signal == SIGKILL {
            drop(signals);
            let _ = super::end_process::<P, ()>(process, ProcessExit::Killed { signal });
            return Ok(0);
        }
        if target == Target::ExitedFirst {
            return Ok(0);
        }
        signals
            .send_to_thread(id, info(signal), queue_limit)
            .map_err(|QueueFull| Errno::EAGAIN)?;
    }
    process.record_signal_sent();
    super::thread::wake_sleep(process, id);
    Ok(0)
}

/// The process's id as a `pid_t`.
fn own_id<P: Platform>(process: &Process<P>) -> i32 {
    // At most `i32::MAX` (`ProcessId`).
    process.id().get() as i32
}

/// `kill(pid, signal)`: sends `signal` to the process `pid` names, the caller being the only
/// one it can reach yet: its own id; 0, its process group, of which it is the only member (a
/// process is its own group's leader until `setpgid` exists); or minus its id, that group.
/// `ESRCH` for anything else, `-1` (every process but the caller) included, before the signal
/// is looked at, as on Linux; then `EINVAL` for a signal outside 0 to 64, and 0, which only
/// checks the target. `pid` is a `pid_t`: the register's upper half is ignored. The signal's
/// `siginfo_t` says `SI_USER` from the caller.
pub fn kill<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    pid: u64,
    signal: u64,
) -> Result<u64, Errno> {
    let own = own_id(process);
    let pid = pid as u32 as i32;
    // -1 is never a group, even process 1's.
    if pid == -1 || (pid != own && pid != 0 && pid != own.wrapping_neg()) {
        return Err(Errno::ESRCH);
    }
    let uid = real_user_id(process);
    send_to_self(process, thread, signal, |signal| {
        Info::sent(signal, code::USER, own as u32, uid)
    })
}

/// `tkill(tid, signal)`: sends `signal` to thread `tid` of the caller's process, the only
/// threads it reaches yet: `EINVAL` for a `tid` that is not positive, then `ESRCH` for one that
/// is not the process's, then as [`kill`], with `SI_TKILL`. `EAGAIN` for a real-time signal
/// when the queued signals are at `RLIMIT_SIGPENDING` (see [`LockedSignals::send`]).
pub fn tkill<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    tid: u64,
    signal: u64,
) -> Result<u64, Errno> {
    tgkill(
        process,
        thread,
        u64::from(own_id(process) as u32),
        tid,
        signal,
    )
}

/// `tgkill(tgid, tid, signal)`: as [`tkill`], with the thread's process (its thread group)
/// named too: `EINVAL` if either id is not positive, then `ESRCH` unless `tgid` is the
/// caller's process and `tid` one of its threads.
pub fn tgkill<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    tgid: u64,
    tid: u64,
    signal: u64,
) -> Result<u64, Errno> {
    let (tgid, tid) = (tgid as u32 as i32, tid as u32 as i32);
    if tgid <= 0 || tid <= 0 {
        return Err(Errno::EINVAL);
    }
    let own = own_id(process);
    if tgid != own {
        return Err(Errno::ESRCH);
    }
    let uid = real_user_id(process);
    send_to_thread(process, thread, tid as u32, signal, |signal| {
        Info::sent(signal, code::TKILL, own as u32, uid)
    })
}

fn real_user_id<P: Platform>(process: &Process<P>) -> u32 {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(super::credentials::Kind::User)
        .real
}

fn read_queued_info<P: Platform>(
    process: &Process<P>,
    address: u64,
) -> Result<[u8; KEPT_INFO_SIZE], Errno> {
    let mut bytes = [0; KEPT_INFO_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes)
}

fn is_reserved_for_kernel_and_kill(bytes: &[u8; KEPT_INFO_SIZE]) -> bool {
    let code = i32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    code >= 0 || code == code::TKILL
}

/// `rt_sigqueueinfo(tgid, signal, info)`: sends `signal` to process `tgid` with the
/// `siginfo_t` at `info`, as `sigqueue` does with `SI_QUEUE`, the caller being the only
/// process it can reach yet. As Linux, in this order: `EFAULT` if the first
/// [`KEPT_INFO_SIZE`] bytes at `info`, all that is read, are not the program's memory;
/// `EPERM` if its `si_code` is one only the kernel and `kill` send (0 or more, or
/// `SI_TKILL`) and the target is not the caller; `ESRCH` unless the target is the caller (0
/// is no process); `EINVAL` for a signal outside 0 to 64; `EAGAIN` for a real-time signal when
/// the queued signals are at `RLIMIT_SIGPENDING`. Signal 0 only checks. The receiver gets the
/// `siginfo_t` as written, with `signal` as its `si_signo` and only its first
/// [`KEPT_INFO_SIZE`] bytes; Linux's `E2BIG` for a `si_code` it does not know with more bytes
/// is not checked. `tgid` is a `pid_t`.
pub fn rt_sigqueueinfo<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    tgid: u64,
    signal: u64,
    info: u64,
) -> Result<u64, Errno> {
    let bytes = read_queued_info(process, info)?;
    let own = own_id(process);
    if tgid as u32 as i32 != own {
        return Err(if is_reserved_for_kernel_and_kill(&bytes) {
            Errno::EPERM
        } else {
            Errno::ESRCH
        });
    }
    send_to_self(process, thread, signal, |signal| {
        Info::queued(signal, bytes)
    })
}

/// `rt_tgsigqueueinfo(tgid, tid, signal, info)`: as [`rt_sigqueueinfo`], to thread `tid` of
/// process `tgid`, the caller's threads being the only ones it reaches yet: after `EFAULT`,
/// `EINVAL` if either id is not positive, then `EPERM` for a reserved `si_code` unless `tid`
/// is the caller's, then `ESRCH` unless `tgid` is the caller's process and `tid` one of its
/// threads.
pub fn rt_tgsigqueueinfo<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    tgid: u64,
    tid: u64,
    signal: u64,
    info: u64,
) -> Result<u64, Errno> {
    let bytes = read_queued_info(process, info)?;
    let (tgid, tid) = (tgid as u32 as i32, tid as u32 as i32);
    if tgid <= 0 || tid <= 0 {
        return Err(Errno::EINVAL);
    }
    let own = own_id(process);
    let own_thread = tid as u32 == thread.id();
    if !own_thread && is_reserved_for_kernel_and_kill(&bytes) {
        return Err(Errno::EPERM);
    }
    if tgid != own {
        return Err(Errno::ESRCH);
    }
    send_to_thread(process, thread, tid as u32, signal, |signal| {
        Info::queued(signal, bytes)
    })
}

/// `rt_sigtimedwait(set, info, timeout, sigsetsize)`: takes the first pending signal in the
/// set at `set`, blocked or not, in the order delivery takes them, copies its `siginfo_t` to
/// `info` unless that is null, and returns its number. If none is pending, it first waits
/// until the `struct timespec` at `timeout` has passed (on the monotonic clock, rounded up to
/// a clock tick), for good if that is null, not at all if it is zero, then fails with `EAGAIN`.
/// A signal another thread sends wakes it: one in the set is taken, any other due one makes
/// it fail with `EINTR` and is delivered (D95), and it waits on otherwise. As Linux,
/// in this order: `EINVAL` if `sigsetsize` is not 8; `EFAULT` if the set is not the program's
/// memory; `EFAULT` if `timeout` is not, and `EINVAL` for negative seconds or nanoseconds
/// outside 0 to 999999999; then, with the signal taken, `EFAULT` if `info` is not the program's
/// writable memory, and the signal is lost. `SIGKILL` and `SIGSTOP` in the set are ignored.
pub fn rt_sigtimedwait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    set: u64,
    info: u64,
    timeout: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    let wanted = read_set(process, set, set_size)? & !UNBLOCKABLE;
    let deadline = match pointer(timeout) {
        None => Some(u64::MAX),
        Some(address) => super::poll::deadline_in::<P>(
            super::poll::read_timeout(process, address)?.unwrap_or(u64::MAX),
        ),
    };
    let take = |thread: &mut LinuxThread| {
        thread
            .signals(&mut process.signals().lock())
            .take_waited(wanted)
    };
    let taken = loop {
        if let Some(taken) = take(thread) {
            break taken;
        }
        let deadline = deadline.ok_or(Errno::EAGAIN)?;
        if is_due(thread) {
            return Err(Errno::EINTR);
        }
        if P::clock_ticks() >= deadline {
            return Err(Errno::EAGAIN);
        }
        if super::thread::sleep_until::<P>(process, thread, deadline).is_break() {
            return Err(Errno::EINTR);
        }
    };
    if let Some(address) = pointer(info) {
        process
            .write_user_memory(address, &taken.to_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(u64::from(taken.signal()))
}

/// The signal set at `address`, of `sigsetsize` bytes, as the calls that take one with no
/// `how` read it: `EINVAL` if the size is not 8, then `EFAULT` if the set is not the program's
/// memory.
pub fn read_set<P: Platform>(
    process: &Process<P>,
    address: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    if set_size != SET_SIZE {
        return Err(Errno::EINVAL);
    }
    let mut bytes = [0; SET_SIZE as usize];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(u64::from_ne_bytes(bytes))
}

/// `rt_sigsuspend(mask, sigsetsize)`, with the program's registers `context`: blocks the
/// signals in the set at `mask` instead of the thread's (never `SIGKILL` or `SIGSTOP`) until a
/// signal starts its handler, then fails with `EINTR`. The handler's frame holds the mask the
/// thread had before, which `rt_sigreturn` restores, as on Linux. A signal its action ignores
/// is taken and the wait goes on, as Linux restarts the call after one. Another thread's
/// signal wakes it (D95); with none due under the new mask it waits for good, as on Linux when
/// none comes. As Linux, with the mask unchanged: `EINVAL` if `sigsetsize` is not 8, then
/// `EFAULT` if the set is not the program's memory. Sets the call's result;
/// breaks when a signal ends the process.
pub fn rt_sigsuspend<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [mask, set_size, ..] = context.syscall_arguments();
    match read_set(process, mask, set_size) {
        Ok(mask) => suspend(process, thread, context, Some(mask)),
        Err(errno) => {
            context.set_syscall_result(errno.result());
            ControlFlow::Continue(())
        }
    }
}

/// `pause()`, with the program's registers `context`: waits, with the thread's mask, until a
/// signal starts its handler, then fails with `EINTR`, as [`rt_sigsuspend`] does. x86_64 only:
/// AArch64's and RISC-V's C libraries call `rt_sigsuspend` with the current mask instead.
#[cfg(target_arch = "x86_64")]
pub fn pause<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    suspend(process, thread, context, None)
}

/// What [`rt_sigsuspend`] and x86_64's `pause` share: blocks `mask`, if any, instead of the
/// thread's mask, then delivers what is due until a handler starts, waiting between rounds.
fn suspend<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    mask: Option<u64>,
) -> ControlFlow<()> {
    context.set_syscall_result(Errno::EINTR.result());
    let thread_mask = match mask {
        Some(mask) => replace_blocked(process, thread, mask),
        None => thread.signal_state().blocked,
    };
    while !deliver_restoring_mask(process, thread, context, Some(thread_mask))? {
        super::thread::sleep_until::<P>(process, thread, u64::MAX)?;
    }
    ControlFlow::Continue(())
}

/// Blocks the signals in `mask` (never `SIGKILL` or `SIGSTOP`) instead of the thread's, for a
/// call that waits with a mask of its own, and returns the mask the thread had.
pub fn replace_blocked<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    mask: u64,
) -> u64 {
    core::mem::replace(
        &mut thread.signals(&mut process.signals().lock()).thread.blocked,
        mask & !UNBLOCKABLE,
    )
}

/// Whether a signal is pending and not blocked: due, so a wait that has not started yet does
/// not start, as on Linux.
pub fn is_due(thread: &LinuxThread) -> bool {
    let signals = thread.signal_state();
    signals.pending & !signals.blocked != 0
}

/// Takes in the signals other threads sent the thread since it last looked, if any
/// ([`LockedSignals::take_in_sent`]): whether its pending signals changed.
pub fn take_in_sent<P: Platform>(process: &Process<P>, thread: &mut LinuxThread) -> bool {
    let sent = process.signals_sent();
    if sent == thread.signals_seen() {
        return false;
    }
    thread.set_signals_seen(sent);
    thread.signals(&mut process.signals().lock()).take_in_sent()
}

/// On the way back to user mode (an interrupt, a call that may have switched the thread out),
/// delivers the signals other threads sent the thread, if any were sent since it last looked,
/// as [`deliver`] does; a load and a compare when none were.
#[inline]
pub fn deliver_sent<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    if process.signals_sent() == thread.signals_seen() {
        return ControlFlow::Continue(());
    }
    take_in_and_deliver(process, thread, context)
}

#[cold]
#[inline(never)]
fn take_in_and_deliver<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    take_in_sent(process, thread);
    deliver(process, thread, context)
}

/// [`deliver_sent`] for an interrupt: out of line, so the thread loop, which the system calls
/// share, keeps its registers.
#[inline(never)]
pub fn deliver_sent_on_interrupt<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    deliver_sent(process, thread, context)
}

/// Delivers the signals due during a call that blocks a mask of its own, as [`deliver`] does,
/// the first frame written saving `thread_mask`, the mask the thread had before the call, if
/// the call changed it. Continues with whether a handler started.
pub fn deliver_restoring_mask<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    thread_mask: Option<u64>,
) -> ControlFlow<(), bool> {
    deliver_with(process, thread, context, None, thread_mask)
}

/// `rt_sigpending(set, sigsetsize)`: copies the thread's pending signals that are blocked (the
/// others were delivered or wait for their handler) to `set`, its first `sigsetsize` bytes: as
/// on Linux, any size up to 8 is taken, a larger one is `EINVAL`, then `EFAULT` if `set` is
/// not the program's writable memory.
pub fn rt_sigpending<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    set: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    if set_size > SET_SIZE {
        return Err(Errno::EINVAL);
    }
    if !super::ends_in_user_space::<P>(set, set_size) {
        return Err(Errno::EFAULT);
    }
    let pending = thread
        .signals(&mut process.signals().lock())
        .blocked_pending()
        .to_ne_bytes();
    process
        .write_user_memory(set, &pending[..set_size as usize])
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

/// Delivers the signals a call just made due (sent to the caller, unblocked, or no longer
/// caught), before the call returns to the program, as POSIX requires: ends the process for
/// one whose action does, stops it for `SIGSTOP`, or writes a frame and starts the program in
/// the handler (`context` is the program's registers, the call's result already set). Every
/// due signal with a handler gets a frame, each on top of the one before, so the last one's
/// handler runs first, as on Linux. Only the calls that can make a signal due run this, so
/// the system-call path pays nothing; signals from other processes and timers will need a
/// check on the way back to user mode.
pub fn deliver<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    deliver_with(process, thread, context, None, None)?;
    ControlFlow::Continue(())
}

/// Sends `fault`'s signal from the kernel, forced as Linux forces a fault's: a program that
/// blocks or ignores it gets `SIG_DFL` and dies of it. Then delivers it as [`deliver`] does,
/// its handler, if any, starting with the fault's `siginfo_t` and machine context. Nothing else
/// is due when a program faults (every due signal was delivered before it ran again, and
/// starting a handler only blocks more), so the fault's signal is the one delivered.
pub fn deliver_fault<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    fault: &Fault,
) -> ControlFlow<()> {
    thread
        .signals(&mut process.signals().lock())
        .force(fault.signal);
    deliver_with(process, thread, context, Some(fault), None)?;
    ControlFlow::Continue(())
}

/// [`deliver`], with `fault` the fault whose signal is due, if one is: its handler's frame
/// describes it. The first frame written saves `mask_to_restore`, if any, instead of the
/// thread's mask, as Linux's does after `rt_sigsuspend` changed it. Continues with whether a
/// handler started.
fn deliver_with<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    mut fault: Option<&Fault>,
    mut mask_to_restore: Option<u64>,
) -> ControlFlow<(), bool> {
    let mut started_handler = false;
    loop {
        // The lock is never held across a copy to the program's memory.
        let (delivery, state) = {
            let mut guard = process.signals().lock();
            let mut signals = thread.signals(&mut guard);
            (signals.next_delivery(), signals.thread())
        };
        match delivery {
            None => return ControlFlow::Continue(started_handler),
            Some(Delivery::Kill(signal)) => {
                return super::end_process(process, ProcessExit::Killed { signal });
            }
            Some(Delivery::Stop(_)) => loop {
                super::thread::sleep_until::<P>(process, thread, u64::MAX)?;
            },
            Some(Delivery::Handler {
                signal,
                action,
                info,
            }) => {
                let described = fault.take_if(|fault| fault.signal == signal);
                let saved_mask = mask_to_restore.unwrap_or(state.blocked);
                let started = start_handler(
                    process, context, &state, saved_mask, &action, &info, described,
                );
                let mut guard = process.signals().lock();
                let mut signals = thread.signals(&mut guard);
                match started {
                    Ok(()) => {
                        signals.enter_handler(signal, &action);
                        started_handler = true;
                        mask_to_restore = None;
                    }
                    // As Linux: `SIGSEGV`, by its default action if it was the signal whose
                    // frame failed.
                    Err(BadFrame) => {
                        if signal == SIGSEGV {
                            signals.reset_handler(SIGSEGV);
                        }
                        signals.force(SIGSEGV);
                    }
                }
            }
        }
    }
}

/// Writes `action`'s frame for the signal `info` describes (or for `fault`, if the signal is a
/// fault's), saving `saved_mask` as the mask to restore, and points `context` at its handler,
/// for a thread whose signal state is `thread`:
/// on the alternate stack with [`flag::ONSTACK`], if it has one and is not on it already (then
/// the frame goes below the stack pointer, as it does without the flag). [`BadFrame`] if the
/// frame's memory is not the program's, if it would run off the bottom of the alternate stack
/// the thread is on, or if the handler has no way to return.
fn start_handler<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
    thread: &ThreadSignals,
    saved_mask: u64,
    action: &Action,
    info: &Info,
    fault: Option<&Fault>,
) -> Result<(), BadFrame> {
    let restorer = if action.flags & flag::RESTORER != 0 {
        action.restorer
    } else {
        super::trampoline::address(process).ok_or(BadFrame)?
    };
    let mut registers = context.registers();
    let stack = context.stack();
    let alternate = &thread.alternate_stack;
    let already_on = alternate.is_on(stack);
    let switch = action.flags & flag::ONSTACK != 0 && alternate.size != 0 && !already_on;
    let top = if switch {
        alternate.base.wrapping_add(alternate.size)
    } else {
        stack.wrapping_sub(signal_frame::RED_ZONE)
    };
    // A nested handler's frame must stay on the alternate stack, as on Linux.
    if already_on && !alternate.is_on(signal_frame::lowest_address(top)) {
        return Err(BadFrame);
    }
    let saved = Saved {
        registers,
        blocked: saved_mask,
        alternate_stack: alternate.to_bytes(stack),
        fp: thread::with_running_fp::<P, _>(|state| signal_frame::fp_part(state))
            .unwrap_or_else(signal_frame::initial_fp_part),
    };
    let (info, trap) = match fault {
        Some(fault) => (signal_frame::fault_info(fault), Some(&fault.trap)),
        None => (info.to_bytes(), None),
    };
    let handler = signal_frame::Handler {
        entry: action.handler,
        restorer,
        signal: u32::from_ne_bytes(info[..4].try_into().unwrap()),
    };
    signal_frame::write(process, top, &saved, &info, trap, &handler, &mut registers)?;
    if signal_frame::HANDLER_STARTS_WITH_INITIAL_FP {
        thread::with_running_fp::<P, _>(|state| *state = FpState::new());
    }
    context.set_registers(&registers);
    Ok(())
}

/// Gives the running thread the FP/SIMD registers a returning handler's frame holds (D82),
/// first giving it FP/SIMD registers if it has none and the frame holds other values than a
/// new program's. [`BadFrame`], changing nothing, if a load would fault on them, or if there
/// is no memory for them.
fn load_frame_fp<P: Platform>(part: &FpPart) -> Result<(), BadFrame> {
    let load = |state: &mut FpState| signal_frame::load_fp_part(part, state);
    if let Some(loaded) = thread::with_running_fp::<P, _>(load) {
        return loaded;
    }
    if *part == signal_frame::initial_fp_part() {
        return Ok(());
    }
    match thread::start_fp::<P>() {
        FpStart::Started => thread::with_running_fp::<P, _>(load)
            .expect("a thread that just got FP/SIMD registers has them"),
        FpStart::AlreadyHad => unreachable!("with_running_fp found no FP/SIMD registers"),
        FpStart::KernelThread => Ok(()),
        FpStart::OutOfMemory => Err(BadFrame),
    }
}

/// `rt_sigreturn()`: a handler returned. Restores the registers, blocked mask and alternate stack
/// its frame saved (see [`signal_frame`]), its FP/SIMD registers too, and returns the restored
/// result register, which the dispatcher writes back unchanged. A frame that is not the program's
/// memory, whose FP/SIMD records are not in Linux's shape, or whose FP/SIMD values a load would
/// fault on, sends `SIGSEGV` (whatever the program blocked or ignored), as on Linux, and changes
/// nothing else. The signals the restored mask unblocks are delivered next.
pub fn rt_sigreturn<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> Result<u64, Errno> {
    let current = context.registers();
    match signal_frame::read(process, &current).and_then(|saved| {
        load_frame_fp::<P>(&saved.fp)?;
        Ok(saved)
    }) {
        Ok(saved) => {
            context.set_registers(&saved.registers);
            thread.signals(&mut process.signals().lock()).restore(
                saved.blocked,
                saved.alternate_stack,
                context.stack(),
            );
            Ok(signal_frame::result(&saved.registers))
        }
        // As Linux, 0: only a `SIGSEGV` handler's frame can show it.
        Err(BadFrame) => {
            thread.signals(&mut process.signals().lock()).force(SIGSEGV);
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_LIMIT: u64 = u64::MAX;

    fn by_kill(signal: u32) -> Info {
        Info::sent(signal, code::USER, 7, 0)
    }

    fn by_tkill(signal: u32) -> Info {
        Info::sent(signal, code::TKILL, 7, 0)
    }

    #[test]
    fn every_signal_starts_with_its_default_action() {
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let signals = process.with_thread(&mut thread, 1);
        for signal in 1..=COUNT as u32 {
            assert_eq!(signals.action(signal), Some(Action::default()));
        }
        assert_eq!(signals.action(0), None);
        assert_eq!(signals.action(COUNT as u32 + 1), None);
        assert_eq!(signals.action(u32::MAX), None);
        assert_eq!(signals.thread(), ThreadSignals::default());
    }

    #[test]
    fn actions_change_except_sigkill_s_and_sigstop_s() {
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        // Setting the default action changes nothing: no table is made.
        assert_eq!(
            signals.set_action(13, Action::default()),
            Ok(Action::default())
        );
        assert!(signals.actions_and_queue.is_none());
        for signal in [SIGKILL, SIGSTOP, 0, 65] {
            let ignore = Action {
                handler: IGNORE,
                ..Action::default()
            };
            assert_eq!(
                signals.set_action(signal, ignore),
                Err(ChangeError::Invalid)
            );
        }
        assert!(signals.actions_and_queue.is_none());
        // A handler: unknown flags cleared, SIGKILL and SIGSTOP left out of its mask.
        let handler = Action {
            handler: 0x40_1000,
            flags: flag::SIGINFO | flag::ONSTACK | flag::UNSUPPORTED | 1 << 40,
            restorer: 0,
            mask: u64::MAX,
        };
        assert_eq!(signals.set_action(11, handler), Ok(Action::default()));
        let kept = signals.action(11).unwrap();
        assert_eq!(kept.flags, flag::SIGINFO | flag::ONSTACK);
        assert_eq!(kept.mask, !UNBLOCKABLE);
        assert_eq!(signals.set_action(11, Action::default()), Ok(kept));
        assert_eq!(signals.action(11), Some(Action::default()));
        assert_eq!(signals.action(64), Some(Action::default()));
    }

    #[test]
    fn the_structures_are_linux_s() {
        let action = Action {
            handler: 1,
            flags: 2,
            restorer: if HAS_RESTORER { 3 } else { 0 },
            mask: 4,
        };
        let bytes = action.to_bytes();
        let words: Vec<u64> = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&word| u64::from_ne_bytes(word))
            .collect();
        if HAS_RESTORER {
            assert_eq!(words, [1, 2, 3, 4]);
        } else {
            assert_eq!(words, [1, 2, 4]);
        }
        assert_eq!(Action::from_bytes(bytes), action);
        let stack = AlternateStack {
            base: 0x7000_0000,
            size: 0x4000,
            autodisarm: false,
        };
        let bytes = stack.to_bytes(0x7000_1000);
        assert_eq!(&bytes[..8], &0x7000_0000u64.to_ne_bytes());
        assert_eq!(&bytes[8..12], &stack_flag::ONSTACK.to_ne_bytes());
        assert_eq!(&bytes[12..16], &[0; 4]);
        assert_eq!(&bytes[16..], &0x4000u64.to_ne_bytes());
        let flags = |stack: AlternateStack, sp| {
            u32::from_ne_bytes(stack.to_bytes(sp)[8..12].try_into().unwrap())
        };
        // Its top is on it (the stack grows down); its base and below, and above its top,
        // are off it.
        assert_eq!(flags(stack, 0x7000_4000), stack_flag::ONSTACK);
        assert_eq!(flags(stack, 0x7000_4001), 0);
        assert_eq!(flags(stack, 0x7000_0000), 0);
        assert_eq!(flags(stack, 0x6fff_ffff), 0);
        // A disarmed stack is never reported as in use.
        let disarmed = AlternateStack {
            autodisarm: true,
            ..stack
        };
        assert_eq!(flags(disarmed, 0x7000_1000), stack_flag::AUTODISARM);
        assert_eq!(flags(AlternateStack::default(), 0), stack_flag::DISABLE);
    }

    #[test]
    fn default_actions_are_signal_7_s() {
        use DefaultAction::*;
        // signal(7)'s table, by number.
        let core = [3, 4, 5, 6, 7, 8, 11, 24, 25, 31];
        let stop = [19, 20, 21, 22];
        let expected = |signal| {
            if core.contains(&signal) {
                Core
            } else if [17, 23, 28].contains(&signal) {
                Ignore
            } else if signal == 18 {
                Continue
            } else if stop.contains(&signal) {
                Stop
            } else {
                Terminate
            }
        };
        for signal in 1..=COUNT as u32 {
            assert_eq!(default_action(signal), expected(signal), "{signal}");
        }
        assert_eq!(default_action(SIGKILL), Terminate);
        assert_eq!(default_action(SIGSTOP), Stop);
    }

    #[test]
    fn signals_are_delivered_by_their_actions() {
        const SIGABRT: u32 = 6;
        const SIGUSR1: u32 = 10;
        const SIGCHLD: u32 = 17;
        const SIGTSTP: u32 = 20;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        // Ignored and not blocked: discarded when sent.
        signals.send(by_kill(SIGCHLD), NO_LIMIT).unwrap();
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, ignore).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, 0);
        // Blocked: pending, ignored or not, and reported; an ignored one is discarded once
        // unblocked, a terminating one ends the process.
        signals.thread.blocked = bit(SIGCHLD) | bit(SIGUSR1) | bit(SIGABRT);
        for signal in [SIGCHLD, SIGUSR1, SIGABRT] {
            signals.send(by_kill(signal), NO_LIMIT).unwrap();
        }
        assert_eq!(signals.blocked_pending(), signals.thread.blocked);
        assert_eq!(signals.next_delivery(), None);
        signals.thread.blocked = 0;
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGABRT)));
        // The ignored ones after it go on the next look.
        assert_eq!(signals.next_delivery(), None);
        assert_eq!(signals.thread.pending, 0);
        // Setting an action that ignores a pending signal discards it, blocked or not.
        signals.thread.blocked = bit(SIGABRT);
        signals.send(by_kill(SIGABRT), NO_LIMIT).unwrap();
        signals.set_action(SIGABRT, ignore).unwrap();
        assert_eq!(signals.thread.pending, 0);
        // A handler runs, with where the signal came from.
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        assert_eq!(signals.blocked_pending(), 0);
        assert_eq!(
            signals.next_delivery(),
            Some(Delivery::Handler {
                signal: SIGUSR1,
                action: handler,
                info: by_tkill(SIGUSR1)
            })
        );
        assert_eq!(signals.thread.pending, 0);
        // The default action again: due.
        signals.set_action(SIGUSR1, Action::default()).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGUSR1)));
        // Lowest number first; SIGKILL whatever the mask said.
        signals.thread.blocked = 0;
        signals.send(by_kill(64), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGKILL), NO_LIMIT).unwrap();
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGKILL)));
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(64)));
        // A terminal's stop signals are discarded (an orphaned process group); SIGSTOP stops.
        signals.send(by_kill(SIGTSTP), NO_LIMIT).unwrap();
        assert_eq!(signals.next_delivery(), None);
        signals.send(by_kill(SIGSTOP), NO_LIMIT).unwrap();
        assert_eq!(signals.next_delivery(), Some(Delivery::Stop(SIGSTOP)));
    }

    #[test]
    fn sigkill_then_synchronous_signals_the_kernel_raised_then_those_sent_then_lowest_first() {
        const SIGHUP: u32 = 1;
        const SIGUSR1: u32 = 10;
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        for signal in [SIGHUP, SIGTRAP, SIGUSR1, SIGSEGV] {
            signals.set_action(signal, handler).unwrap();
        }
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGHUP), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGTRAP), NO_LIMIT).unwrap();
        signals.force(SIGSEGV);
        let order: Vec<u32> = core::iter::from_fn(|| match signals.next_delivery() {
            Some(Delivery::Handler { signal, .. }) => Some(signal),
            _ => None,
        })
        .collect();
        assert_eq!(order, [SIGSEGV, SIGTRAP, SIGHUP, SIGUSR1]);
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        signals.force(SIGSEGV);
        signals.send(by_kill(SIGKILL), NO_LIMIT).unwrap();
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGKILL)));
    }

    #[test]
    fn synchronous_signals_the_kernel_raised_are_delivered_in_the_order_raised() {
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        for signal in [SIGILL, SIGSEGV] {
            signals.set_action(signal, handler).unwrap();
        }
        signals.thread.blocked = bit(SIGILL) | bit(SIGSEGV);
        signals.send(Info::from_kernel(SIGSEGV), NO_LIMIT).unwrap();
        signals.send(Info::from_kernel(SIGILL), NO_LIMIT).unwrap();
        signals.thread.blocked = 0;
        let order: Vec<u32> = core::iter::from_fn(|| match signals.next_delivery() {
            Some(Delivery::Handler { signal, .. }) => Some(signal),
            _ => None,
        })
        .collect();
        assert_eq!(order, [SIGSEGV, SIGILL]);
    }

    #[test]
    fn sigtimedwait_takes_the_lowest_synchronous_signal_whoever_raised_it() {
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(SIGILL) | bit(SIGSEGV);
        signals.send(Info::from_kernel(SIGSEGV), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGILL), NO_LIMIT).unwrap();
        let order: Vec<u32> = core::iter::from_fn(|| signals.take_waited(!UNBLOCKABLE))
            .map(|info| info.signal())
            .collect();
        assert_eq!(order, [SIGILL, SIGSEGV]);
    }

    #[test]
    fn a_drained_queue_gives_its_memory_back() {
        const RT: u32 = 40;
        let capacity = |signals: &Signals| {
            signals
                .actions_and_queue
                .as_deref()
                .map(|[block]| block.queue.capacity())
        };
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(RT) | bit(RT + 1);
        for value in 0..100 {
            signals.send(queued(RT, value), NO_LIMIT).unwrap();
        }
        signals.send(queued(RT + 1, 0), NO_LIMIT).unwrap();
        assert!(capacity(&signals) >= Some(101));
        while signals.take_waited(bit(RT)).is_some() {}
        assert!(capacity(&signals) >= Some(101));
        signals
            .set_action(
                RT + 1,
                Action {
                    handler: IGNORE,
                    ..Action::default()
                },
            )
            .unwrap();
        let drained = capacity(&signals).unwrap();
        assert!((1..=DRAINED_QUEUE_CAPACITY).contains(&drained));
        signals.send(queued(RT, 0), NO_LIMIT).unwrap();
        assert_eq!(signals.take_waited(bit(RT)), Some(queued(RT, 0)));
        assert_eq!(capacity(&signals), Some(drained));
    }

    #[test]
    fn sigcont_and_the_stop_signals_cancel_each_other() {
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = !UNBLOCKABLE;
        for signal in [20, 21, 22] {
            signals.send(by_kill(signal), NO_LIMIT).unwrap();
        }
        signals.send(by_kill(SIGCONT), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, bit(SIGCONT));
        signals.send(by_kill(21), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, bit(21));
        // SIGSTOP cannot be blocked: sent, it is pending until delivered.
        signals.send(by_kill(SIGCONT), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGSTOP), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, bit(SIGSTOP));
        signals.send(by_kill(SIGCONT), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, bit(SIGCONT));
        signals.thread.blocked = 0;
        assert_eq!(signals.next_delivery(), None);
        assert_eq!(signals.thread.pending, 0);
    }

    #[test]
    fn a_standard_signal_keeps_the_info_it_was_first_sent_with() {
        const SIGUSR1: u32 = 10;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(SIGUSR1);
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        assert_eq!(signals.thread.pending, bit(SIGUSR1));
        assert_eq!(signals.take_waited(bit(SIGUSR1)), Some(by_tkill(SIGUSR1)));
        assert_eq!(signals.take_waited(bit(SIGUSR1)), None);
        assert_eq!(signals.thread.pending, 0);
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        signals.thread.blocked = 0;
        assert_eq!(
            signals.next_delivery(),
            Some(Delivery::Handler {
                signal: SIGUSR1,
                action: handler,
                info: by_tkill(SIGUSR1)
            })
        );
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        assert!(matches!(
            signals.next_delivery(),
            Some(Delivery::Handler { info, .. }) if info == by_kill(SIGUSR1)
        ));
        signals.thread.blocked = bit(SIGUSR1);
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, ignore).unwrap();
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        assert_eq!(signals.take_waited(bit(SIGUSR1)), Some(by_kill(SIGUSR1)));
    }

    #[test]
    fn a_signal_not_blocked_keeps_no_info_without_actions_of_its_own() {
        const SIGUSR1: u32 = 10;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        assert!(signals.actions_and_queue.is_none());
        assert_eq!(signals.take_waited(bit(SIGUSR1)), Some(Info::lost(SIGUSR1)));
        signals.thread.blocked = bit(SIGUSR1);
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        assert!(signals.actions_and_queue.is_some());
        assert_eq!(signals.take_waited(bit(SIGUSR1)), Some(by_tkill(SIGUSR1)));
    }

    fn queued(signal: u32, value: u64) -> Info {
        let mut bytes = [0xa5; KEPT_INFO_SIZE];
        bytes[8..12].copy_from_slice(&code::QUEUE.to_ne_bytes());
        bytes[24..32].copy_from_slice(&value.to_ne_bytes());
        Info::queued(signal, bytes)
    }

    #[test]
    fn real_time_signals_queue_each_instance_with_its_info_in_order() {
        const RT: u32 = 40;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(RT) | bit(RT + 1);
        signals.send(queued(RT + 1, 9), NO_LIMIT).unwrap();
        for value in 1..=3 {
            signals.send(queued(RT, value), NO_LIMIT).unwrap();
        }
        signals.send(by_kill(RT), NO_LIMIT).unwrap();
        assert_eq!(signals.blocked_pending(), bit(RT) | bit(RT + 1));
        let [block] = signals.actions_and_queue.as_deref().unwrap();
        assert_eq!(block.queue.len(), 5);
        let taken: Vec<Info> =
            core::iter::from_fn(|| signals.take_waited(bit(RT) | bit(RT + 1))).collect();
        assert_eq!(
            taken,
            [
                queued(RT, 1),
                queued(RT, 2),
                queued(RT, 3),
                by_kill(RT),
                queued(RT + 1, 9)
            ]
        );
        assert_eq!(signals.thread.pending, 0);
        let bytes = queued(RT, 1).to_bytes();
        assert_eq!(bytes[..4], (RT as i32).to_ne_bytes());
        assert_eq!(bytes[12..16], [0xa5; 4]);
        assert_eq!(bytes[KEPT_INFO_SIZE..], [0; INFO_SIZE - KEPT_INFO_SIZE]);
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(RT, handler).unwrap();
        signals.send(queued(RT, 1), NO_LIMIT).unwrap();
        signals.send(queued(RT, 2), NO_LIMIT).unwrap();
        signals.thread.blocked = 0;
        let Some(Delivery::Handler { info, action, .. }) = signals.next_delivery() else {
            panic!("no handler ran");
        };
        assert_eq!(info, queued(RT, 1));
        assert_eq!(signals.thread.pending, bit(RT));
        signals.enter_handler(RT, &action);
        assert_eq!(signals.next_delivery(), None);
        signals.thread.blocked = 0;
        assert!(matches!(
            signals.next_delivery(),
            Some(Delivery::Handler { info, .. }) if info == queued(RT, 2)
        ));
        assert_eq!(signals.thread.pending, 0);
    }

    #[test]
    fn a_standard_signal_keeps_its_first_info_and_sigcont_drops_queued_stop_signals() {
        const SIGUSR1: u32 = 10;
        const SIGTSTP: u32 = 20;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(SIGUSR1) | bit(SIGTSTP) | bit(SIGCONT);
        signals.send(by_tkill(SIGUSR1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGTSTP), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGCONT), NO_LIMIT).unwrap();
        let [block] = signals.actions_and_queue.as_deref().unwrap();
        assert_eq!(
            block
                .queue
                .iter()
                .map(|queued| queued.info)
                .collect::<Vec<_>>(),
            [by_tkill(SIGUSR1), by_kill(SIGCONT)]
        );
        let taken: Vec<Info> = core::iter::from_fn(|| signals.take_waited(!UNBLOCKABLE)).collect();
        assert_eq!(taken, [by_tkill(SIGUSR1), by_kill(SIGCONT)]);
        assert_eq!(signals.thread.pending, 0);
    }

    #[test]
    fn ignoring_a_real_time_signal_discards_every_instance() {
        const RT: u32 = 50;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(RT, ignore).unwrap();
        signals.thread.blocked = bit(RT);
        signals.send(queued(RT, 1), NO_LIMIT).unwrap();
        signals.send(queued(RT, 2), NO_LIMIT).unwrap();
        assert_eq!(signals.blocked_pending(), bit(RT));
        signals.thread.blocked = 0;
        assert_eq!(signals.next_delivery(), None);
        assert_eq!(signals.thread.pending, 0);
        let [block] = signals.actions_and_queue.as_deref().unwrap();
        assert!(block.queue.is_empty());
        signals.thread.blocked = bit(RT);
        signals.send(queued(RT, 1), NO_LIMIT).unwrap();
        signals.send(queued(RT, 2), NO_LIMIT).unwrap();
        signals.set_action(RT, Action::default()).unwrap();
        signals.set_action(RT, ignore).unwrap();
        assert_eq!(signals.thread.pending, 0);
        let [block] = signals.actions_and_queue.as_deref().unwrap();
        assert!(block.queue.is_empty());
    }

    #[test]
    fn a_full_queue_refuses_real_time_signals_except_kill_s_and_loses_the_others_info() {
        const SIGUSR1: u32 = 10;
        const RT: u32 = 33;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(SIGUSR1) | bit(RT) | bit(SIGSEGV);
        assert_eq!(signals.send(queued(RT, 1), 0), Err(QueueFull));
        assert_eq!(signals.send(by_tkill(RT), 0), Err(QueueFull));
        assert_eq!(signals.thread.pending, 0);
        signals.send(by_kill(RT), 0).unwrap();
        signals.send(queued(SIGUSR1, 1), 0).unwrap();
        signals.send(Info::from_kernel(SIGSEGV), 0).unwrap();
        assert_eq!(
            signals.thread.pending,
            bit(SIGUSR1) | bit(RT) | bit(SIGSEGV)
        );
        assert_eq!(signals.take_waited(bit(RT)), Some(Info::lost(RT)));
        assert_eq!(signals.take_waited(bit(SIGUSR1)), Some(Info::lost(SIGUSR1)));
        assert_eq!(
            signals.take_waited(bit(SIGSEGV)),
            Some(Info::from_kernel(SIGSEGV))
        );
        signals.send(queued(RT, 1), NO_LIMIT).unwrap();
        signals.send(by_kill(RT), 0).unwrap();
        assert_eq!(signals.take_waited(bit(RT)), Some(queued(RT, 1)));
        assert_eq!(signals.take_waited(bit(RT)), None);
    }

    #[test]
    fn the_queue_counts_up_to_the_limit_and_frees_what_it_took() {
        let queued = AtomicU64::new(0);
        assert!(try_charge(&queued, 2));
        assert!(try_charge(&queued, 2));
        assert!(!try_charge(&queued, 2));
        charge(&queued);
        assert_eq!(queued.load(Ordering::Relaxed), 3);
        uncharge(&queued, 2);
        assert!(!try_charge(&queued, 1));
        assert!(try_charge(&queued, 2));
        uncharge(&queued, 2);
        assert_eq!(queued.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn sigtimedwait_takes_synchronous_signals_first_and_never_sigkill_or_sigstop() {
        const SIGUSR1: u32 = 10;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        signals.thread.blocked = bit(SIGUSR1) | bit(SIGTRAP) | bit(40);
        signals.send(queued(40, 1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        signals.send(by_kill(SIGTRAP), NO_LIMIT).unwrap();
        let everything = !UNBLOCKABLE;
        let order: Vec<u32> = core::iter::from_fn(|| signals.take_waited(everything))
            .map(|info| info.signal())
            .collect();
        assert_eq!(order, [SIGTRAP, SIGUSR1, 40]);
        assert_eq!(signals.take_waited(everything), None);
    }

    #[test]
    fn handlers_block_their_mask_and_signal_and_may_reset() {
        const SIGSEGV: u32 = 11;
        const SIGUSR1: u32 = 10;
        const SIGUSR2: u32 = 12;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        let handler = Action {
            handler: 0x40_1000,
            flags: flag::RESETHAND,
            restorer: 0,
            mask: bit(SIGUSR2) | bit(SIGKILL),
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(by_kill(SIGUSR1), NO_LIMIT).unwrap();
        let Some(Delivery::Handler { action, .. }) = signals.next_delivery() else {
            panic!("no handler ran");
        };
        assert_eq!(
            action,
            Action {
                mask: bit(SIGUSR2),
                ..handler
            }
        );
        // Removed as it was taken: the next one is by the default action.
        assert_eq!(
            signals.action(SIGUSR1),
            Some(Action {
                handler: DEFAULT,
                ..action
            })
        );
        signals.enter_handler(SIGUSR1, &action);
        assert_eq!(signals.thread.blocked, bit(SIGUSR1) | bit(SIGUSR2));
        // With SA_NODEFER the signal itself stays unblocked; SIGKILL never is.
        signals.thread.blocked = 0;
        let nodefer = Action {
            flags: flag::NODEFER,
            mask: bit(SIGKILL) | bit(SIGSEGV),
            ..handler
        };
        signals.enter_handler(SIGUSR1, &nodefer);
        assert_eq!(signals.thread.blocked, bit(SIGSEGV));
        // An alternate stack is kept while a handler runs, unless it has SS_AUTODISARM:
        // then it is disabled.
        signals
            .change_alternate_stack(0x7000_0000, 0, MIN_STACK_SIZE, 0)
            .unwrap();
        let kept = signals.thread.alternate_stack;
        signals.enter_handler(SIGUSR1, &handler);
        assert_eq!(signals.thread.alternate_stack, kept);
        signals
            .change_alternate_stack(0x7000_0000, stack_flag::AUTODISARM, MIN_STACK_SIZE, 0)
            .unwrap();
        let armed = signals.thread.alternate_stack;
        signals.enter_handler(SIGUSR1, &handler);
        assert_eq!(signals.thread.alternate_stack, AlternateStack::default());
        // rt_sigreturn restores the mask, without SIGKILL, and the stack the frame saved.
        signals.restore(u64::MAX, armed.to_bytes(0), 0);
        assert_eq!(signals.thread.blocked, !UNBLOCKABLE);
        assert_eq!(signals.thread.alternate_stack, armed);
        // Not while the thread is on its alternate stack, nor an invalid one: as Linux, the
        // stack is left as it was.
        let plain = AlternateStack {
            autodisarm: false,
            ..armed
        };
        signals.thread.alternate_stack = plain;
        signals.restore(0, AlternateStack::default().to_bytes(0), 0x7000_0100);
        assert_eq!(signals.thread.alternate_stack, plain);
        signals.restore(0, AlternateStack { size: 1, ..plain }.to_bytes(0), 0);
        assert_eq!(signals.thread.alternate_stack, plain);
        assert_eq!(signals.thread.blocked, 0);
    }

    #[test]
    fn forced_signals_are_delivered_whatever_the_program_asked() {
        const SIGSEGV: u32 = 11;
        let (mut process, mut thread) = (Signals::new(), ThreadSignals::new());
        let mut signals = process.with_thread(&mut thread, 1);
        // Blocked, with a handler: unblocked, by the default action.
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGSEGV, handler).unwrap();
        signals.thread.blocked = bit(SIGSEGV) | bit(1);
        signals.force(SIGSEGV);
        assert_eq!(signals.thread.blocked, bit(1));
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGSEGV)));
        // Ignored: by the default action.
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(SIGSEGV, ignore).unwrap();
        signals.force(SIGSEGV);
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGSEGV)));
        // Caught and not blocked: its handler runs.
        signals.set_action(SIGSEGV, handler).unwrap();
        signals.force(SIGSEGV);
        assert_eq!(
            signals.next_delivery(),
            Some(Delivery::Handler {
                signal: SIGSEGV,
                action: handler,
                info: Info::from_kernel(SIGSEGV)
            })
        );
    }
}
