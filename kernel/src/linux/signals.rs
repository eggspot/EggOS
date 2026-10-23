//! Signals (M6): their state, `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` (D60),
//! sending and delivering them, `kill`, `tkill`, `tgkill` and `rt_sigpending` (D63), and their
//! handlers, which run on Linux's signal frames and return with `rt_sigreturn` (D64). Rust's
//! std ignores `SIGPIPE` and installs `SIGSEGV` and `SIGBUS` handlers on an alternate stack at
//! startup, glibc blocks and unblocks signals around much of its work, and `abort()` sends
//! itself `SIGABRT` and expects to die of it.
//!
//! A process has an action per signal ([`Action`]), all `SIG_DFL` until it first changes one,
//! which makes its own block on the heap ([`Caught`]). Its thread has a blocked mask, pending
//! signals and an alternate stack ([`ThreadSignals`]), which move to each thread when `clone`
//! exists. A signal a call makes due (sent to the caller, unblocked, or no longer caught) is
//! delivered before the call returns ([`deliver`]): by its default action or `SIG_IGN`, or by
//! a frame on the program's stack ([`signal_frame`]) from which its handler starts. A fault
//! sends its signal before the program runs again ([`deliver_fault`], D65): to its handler,
//! with the fault's `siginfo_t`, or, as on Linux, by the default action if the program blocks
//! or ignores it.

use super::Errno;
use super::fault::Fault;
use super::signal::{SIGKILL, SIGSEGV, SIGSTOP};
use super::signal_frame::{self, BadFrame, STACK_SIZE, Saved};
use crate::process::{Process, ProcessExit};
use crate::thread;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// How many signals there are: 1 to 64 (`_NSIG`).
pub const COUNT: usize = 64;

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

/// The signal state of a process and its one thread.
#[derive(Debug, Default)]
pub struct Signals {
    /// The process's actions and what goes with catching signals ([`Caught`]): every action
    /// [`Action::default`] (`SIG_DFL`) until one first changes, then a block of its own on the
    /// heap. A process costs a word for them, and starting one copies nothing (D59's lesson):
    /// `Process` stays in its heap size class.
    caught: Option<Box<[Caught; 1]>>,
    /// Its thread's.
    thread: ThreadSignals,
}

/// A thread's signal state: what it blocks, what is pending and its alternate stack.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSignals {
    /// The blocked signals, bit `n - 1` for signal `n`; never [`UNBLOCKABLE`]'s.
    pub blocked: u64,
    /// The signals sent and not yet delivered, as `blocked`.
    pub pending: u64,
    /// The alternate stack for handlers with [`flag::ONSTACK`].
    pub alternate_stack: AlternateStack,
}

/// The state of a process that catches signals, out of line ([`Signals::caught`]): its
/// actions, field by field, with the flags as 32-bit words (every flag the kernel keeps fits),
/// so that the block, with the words that go with it, fits the heap's largest size class
/// (2 KiB, as the table of [`Action`]s alone did) instead of taking a frame of its own.
#[derive(Debug, Clone, Copy)]
struct Caught {
    handlers: [u64; COUNT],
    flags: [u32; COUNT],
    restorers: [u64; COUNT],
    masks: [u64; COUNT],
    /// The pending signals sent with `tkill` or `tgkill` ([`Origin::Thread`]), as
    /// [`ThreadSignals::pending`]; the others were sent with `kill` unless in `from_kernel`.
    /// Only a handler sees a signal's origin, so only a process that has changed an action
    /// keeps them: a signal sent before its first change reports [`Origin::Process`]. They
    /// move to each thread with the pending signals when `clone` exists.
    from_thread: u64,
    /// The pending signals the kernel raised ([`Origin::Kernel`]), as `from_thread`.
    from_kernel: u64,
    /// Where the process's return trampoline is mapped ([`super::trampoline`]), 0 until a
    /// handler first needs it. It stays there: a program that unmaps or replaces the page
    /// breaks its own handlers' return, as it would by unmapping Linux's vDSO. `execve` must
    /// reset it with the actions.
    trampoline: u64,
}

const _: () = assert!(core::mem::size_of::<Caught>() <= crate::heap::MAX_BLOCK);
const _: () = assert!(flag::KNOWN <= u32::MAX as u64);
// `Process` sits exactly in a heap size class (D64): the signal state must not grow.
const _: () = assert!(core::mem::size_of::<Signals>() == 48);

impl Caught {
    /// Every action `SIG_DFL`, no origins, no trampoline.
    const fn new() -> Self {
        Self {
            handlers: [DEFAULT; COUNT],
            flags: [0; COUNT],
            restorers: [0; COUNT],
            masks: [0; COUNT],
            from_thread: 0,
            from_kernel: 0,
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

    /// Sets action `index` to `action`, whose flags are only [`flag::KNOWN`]'s.
    fn set_action(&mut self, index: usize, action: Action) {
        self.handlers[index] = action.handler;
        self.flags[index] = action.flags as u32;
        self.restorers[index] = action.restorer;
        self.masks[index] = action.mask;
    }
}

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
/// the architecture.
pub const MIN_STACK_SIZE: u64 = if cfg!(target_arch = "aarch64") {
    5120
} else {
    2048
};

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

impl Signals {
    /// No handlers, nothing blocked, no alternate stack.
    pub const fn new() -> Self {
        Self {
            caught: None,
            thread: ThreadSignals {
                blocked: 0,
                pending: 0,
                alternate_stack: AlternateStack {
                    base: 0,
                    size: 0,
                    autodisarm: false,
                },
            },
        }
    }

    /// Signal `signal`'s action, or `None` for a number with no signal.
    pub fn action(&self, signal: u32) -> Option<Action> {
        let index = (signal as usize).checked_sub(1).filter(|&i| i < COUNT)?;
        Some(
            self.caught
                .as_deref()
                .map_or_else(Action::default, |[caught]| caught.action(index)),
        )
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
            let [caught] = &mut **match &mut self.caught {
                Some(caught) => caught,
                None => self.caught.insert(caught_block()?),
            };
            caught.set_action(signal as usize - 1, new);
        }
        if self.ignores(signal) {
            self.discard(bit(signal));
        }
        Ok(current)
    }

    /// Its thread's state.
    pub fn thread(&self) -> ThreadSignals {
        self.thread
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

    /// Sends `signal` (1 to 64) to the thread from `origin`: pending until
    /// [`Self::next_delivery`] takes it, unless it is ignored and not blocked, which discards
    /// it at once, as on Linux (a blocked signal stays pending even if ignored, and
    /// `rt_sigpending` reports it). As POSIX requires, `SIGCONT` discards pending stop signals
    /// and a stop signal a pending `SIGCONT`, whatever their actions. A signal already pending
    /// is not counted twice, and keeps the origin it was first sent from, as Linux keeps the
    /// first `siginfo` of a signal that is not real-time: real-time signals' queue comes later.
    pub fn send(&mut self, signal: u32, origin: Origin) {
        if signal == SIGCONT {
            self.discard(STOP_SIGNALS);
        } else if bit(signal) & STOP_SIGNALS != 0 {
            self.discard(bit(SIGCONT));
        }
        if self.thread.blocked & bit(signal) == 0 && self.ignores(signal) {
            return;
        }
        if self.thread.pending & bit(signal) != 0 {
            return;
        }
        self.thread.pending |= bit(signal);
        if let Some([caught]) = self.caught.as_deref_mut() {
            match origin {
                Origin::Process => {}
                Origin::Thread => caught.from_thread |= bit(signal),
                Origin::Kernel => caught.from_kernel |= bit(signal),
            }
        }
    }

    /// Sends `signal` from the kernel so that it is delivered whatever the program asked, as
    /// Linux's `force_sig`: an ignored or blocked signal's action becomes `SIG_DFL` (which
    /// ends the process for the signals forced) and a blocked one is unblocked.
    pub fn force(&mut self, signal: u32) {
        let index = signal as usize - 1;
        let blocked = self.thread.blocked & bit(signal) != 0;
        if let Some([caught]) = self.caught.as_deref_mut()
            && (blocked || caught.handlers[index] == IGNORE)
        {
            caught.handlers[index] = DEFAULT;
        }
        self.thread.blocked &= !bit(signal);
        self.send(signal, Origin::Kernel);
    }

    /// Discards the pending signals in `set`.
    fn discard(&mut self, set: u64) {
        self.thread.pending &= !set;
        if let Some([caught]) = self.caught.as_deref_mut() {
            caught.from_thread &= !set;
            caught.from_kernel &= !set;
        }
    }

    /// Sets `signal`'s handler to `SIG_DFL`, keeping its flags and mask, as Linux does for
    /// [`flag::RESETHAND`] and a forced signal. Nothing to do without a table: every handler
    /// is `SIG_DFL` then.
    fn reset_handler(&mut self, signal: u32) {
        if let Some([caught]) = self.caught.as_deref_mut() {
            caught.handlers[signal as usize - 1] = DEFAULT;
        }
    }

    /// The address of the process's return trampoline, 0 until it is mapped.
    pub fn trampoline(&self) -> u64 {
        self.caught
            .as_deref()
            .map_or(0, |[caught]| caught.trampoline)
    }

    /// Records where the return trampoline was mapped. Only a process with a handler, which
    /// has its own table, has one.
    pub fn set_trampoline(&mut self, address: u64) {
        if let Some([caught]) = self.caught.as_deref_mut() {
            caught.trampoline = address;
        }
    }

    /// Where pending `signal` came from.
    fn origin(&self, signal: u32) -> Origin {
        match self.caught.as_deref() {
            Some([caught]) if caught.from_kernel & bit(signal) != 0 => Origin::Kernel,
            Some([caught]) if caught.from_thread & bit(signal) != 0 => Origin::Thread,
            _ => Origin::Process,
        }
    }

    /// Takes the next pending, unblocked signal, lowest number first, discarding the ignored
    /// ones on the way: `Some` for one that ends the process (default actions Term and Core,
    /// `SIGKILL`'s included), stops it, or runs its handler. A handler with
    /// [`flag::RESETHAND`] is removed as it is taken, as on Linux: its signal's next delivery
    /// is by the default action.
    pub fn next_delivery(&mut self) -> Option<Delivery> {
        let mut due = self.thread.pending & !self.thread.blocked;
        while due != 0 {
            let signal = due.trailing_zeros() + 1;
            due &= due - 1;
            let action = self.action(signal).unwrap_or_default();
            let delivery = match action.handler {
                IGNORE => None,
                DEFAULT => match default_action(signal) {
                    DefaultAction::Terminate | DefaultAction::Core => Some(Delivery::Kill(signal)),
                    DefaultAction::Ignore | DefaultAction::Continue => None,
                    // A terminal's stop signals are discarded in an orphaned process group,
                    // as POSIX requires, and with no parents every process's group is one.
                    DefaultAction::Stop if signal != SIGSTOP => None,
                    DefaultAction::Stop => Some(Delivery::Stop(signal)),
                },
                _ => {
                    if action.flags & flag::RESETHAND != 0 {
                        self.reset_handler(signal);
                    }
                    let origin = self.origin(signal);
                    Some(Delivery::Handler {
                        signal,
                        action,
                        origin,
                    })
                }
            };
            self.discard(bit(signal));
            if delivery.is_some() {
                return delivery;
            }
        }
        None
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

/// The stop signals: `SIGSTOP`, `SIGTSTP`, `SIGTTIN` and `SIGTTOU`.
const STOP_SIGNALS: u64 = bit(SIGSTOP) | bit(20) | bit(21) | bit(22);

/// What delivering a signal does, for [`Signals::next_delivery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The process ends with this signal.
    Kill(u32),
    /// The process stops until `SIGCONT` or `SIGKILL`.
    Stop(u32),
    /// `action`'s handler runs for `signal`, sent from `origin`.
    Handler {
        signal: u32,
        action: Action,
        origin: Origin,
    },
}

/// Who sent a signal: what its `siginfo`'s `si_code` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A process, with `kill` (`SI_USER`).
    Process,
    /// A thread, with `tkill` or `tgkill` (`SI_TKILL`).
    Thread,
    /// The kernel (`SI_KERNEL`), as for a signal frame that could not be written.
    Kernel,
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

/// A block of default actions on the heap, or [`ChangeError::OutOfMemory`].
fn caught_block() -> Result<Box<[Caught; 1]>, ChangeError> {
    let mut block = Vec::new();
    block
        .try_reserve_exact(1)
        .map_err(|_| ChangeError::OutOfMemory)?;
    block.push(Caught::new());
    // Exactly one, in a buffer of exactly that capacity: no reallocation.
    Ok(block.into_boxed_slice().try_into().unwrap())
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
    let previous = {
        let mut signals = process.signals().lock();
        match new {
            None => signals.action(signal).ok_or(Errno::EINVAL)?,
            Some(new) => signals
                .set_action(signal, new)
                .map_err(|error| match error {
                    ChangeError::Invalid => Errno::EINVAL,
                    ChangeError::OutOfMemory => Errno::ENOMEM,
                })?,
        }
    };
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
        let mut signals = process.signals().lock();
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
        let mut signals = process.signals().lock();
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

/// Signal `signal` (an `int`) as `kill`, `tkill` and `tgkill` take it, once their target was
/// found: sends it to the process's thread from `origin`, or only checks the target for 0.
/// `EINVAL` outside 0 to 64.
fn send_to_self<P: Platform>(
    process: &Process<P>,
    signal: u64,
    origin: Origin,
) -> Result<u64, Errno> {
    match signal as u32 {
        0 => Ok(0),
        signal if signal as usize <= COUNT => {
            process.signals().lock().send(signal, origin);
            Ok(0)
        }
        _ => Err(Errno::EINVAL),
    }
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
/// is looked at, as on Linux; then as [`send_to_self`]. `pid` is a `pid_t`: the register's
/// upper half is ignored.
pub fn kill<P: Platform>(process: &Process<P>, pid: u64, signal: u64) -> Result<u64, Errno> {
    let own = own_id(process);
    let pid = pid as u32 as i32;
    // -1 is never a group, even process 1's.
    if pid == -1 || (pid != own && pid != 0 && pid != own.wrapping_neg()) {
        return Err(Errno::ESRCH);
    }
    send_to_self(process, signal, Origin::Process)
}

/// `tkill(tid, signal)`: sends `signal` to thread `tid`, the caller's one thread being the
/// only one: `EINVAL` for a `tid` that is not positive, then `ESRCH` for one that is not the
/// caller's, then as [`send_to_self`].
pub fn tkill<P: Platform>(process: &Process<P>, tid: u64, signal: u64) -> Result<u64, Errno> {
    tgkill(process, u64::from(own_id(process) as u32), tid, signal)
}

/// `tgkill(tgid, tid, signal)`: as [`tkill`], with the thread's process (its thread group)
/// named too: `EINVAL` if either id is not positive, then `ESRCH` unless both are the
/// caller's.
pub fn tgkill<P: Platform>(
    process: &Process<P>,
    tgid: u64,
    tid: u64,
    signal: u64,
) -> Result<u64, Errno> {
    let (tgid, tid) = (tgid as u32 as i32, tid as u32 as i32);
    if tgid <= 0 || tid <= 0 {
        return Err(Errno::EINVAL);
    }
    let own = own_id(process);
    if tgid != own || tid != own {
        return Err(Errno::ESRCH);
    }
    send_to_self(process, signal, Origin::Thread)
}

/// `rt_sigpending(set, sigsetsize)`: copies the thread's pending signals that are blocked (the
/// others were delivered or wait for their handler) to `set`, its first `sigsetsize` bytes: as
/// on Linux, any size up to 8 is taken, a larger one is `EINVAL`, then `EFAULT` if `set` is
/// not the program's writable memory.
pub fn rt_sigpending<P: Platform>(
    process: &Process<P>,
    set: u64,
    set_size: u64,
) -> Result<u64, Errno> {
    if set_size > SET_SIZE {
        return Err(Errno::EINVAL);
    }
    if !super::ends_in_user_space::<P>(set, set_size) {
        return Err(Errno::EFAULT);
    }
    let pending = process.signals().lock().blocked_pending().to_ne_bytes();
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
pub fn deliver<P: Platform>(process: &Process<P>, context: &mut UserContext) -> ControlFlow<()> {
    deliver_with(process, context, None)
}

/// Sends `fault`'s signal from the kernel, forced as Linux forces a fault's: a program that
/// blocks or ignores it gets `SIG_DFL` and dies of it. Then delivers it as [`deliver`] does,
/// its handler, if any, starting with the fault's `siginfo_t` and machine context. Nothing else
/// is due when a program faults (every due signal was delivered before it ran again), and a
/// handler cannot unblock any, so the fault's signal is the one delivered.
pub fn deliver_fault<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
    fault: &Fault,
) -> ControlFlow<()> {
    process.signals().lock().force(fault.signal);
    deliver_with(process, context, Some(fault))
}

/// [`deliver`], with `fault` the fault whose signal is due, if one is: its handler's frame
/// describes it.
fn deliver_with<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
    mut fault: Option<&Fault>,
) -> ControlFlow<()> {
    loop {
        // The lock is never held across a copy to the program's memory.
        let (delivery, thread) = {
            let mut signals = process.signals().lock();
            (signals.next_delivery(), signals.thread)
        };
        match delivery {
            None => return ControlFlow::Continue(()),
            Some(Delivery::Kill(signal)) => {
                process.exit(ProcessExit::Killed { signal });
                return ControlFlow::Break(());
            }
            // Stopped until `SIGCONT` or `SIGKILL`, which only another process could send:
            // none can reach it yet, so for good, as on Linux when no one sends either.
            Some(Delivery::Stop(_)) => thread::sleep_until::<P>(u64::MAX),
            Some(Delivery::Handler {
                signal,
                action,
                origin,
            }) => {
                let described = fault.take_if(|fault| fault.signal == signal);
                let started = start_handler(
                    process, context, &thread, signal, &action, origin, described,
                );
                let mut signals = process.signals().lock();
                match started {
                    Ok(()) => signals.enter_handler(signal, &action),
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

/// Writes `action`'s frame for `signal` from `origin` (or for `fault`, if the signal is a
/// fault's) and points `context` at its handler, for a thread whose signal state is `thread`: on the alternate stack with [`flag::ONSTACK`], if it has
/// one and is not on it already (then the frame goes below the stack pointer, as it does without
/// the flag). [`BadFrame`] if the frame's memory is not the program's, if it would run off the
/// bottom of the alternate stack the thread is on, or if the handler has no way to return.
fn start_handler<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
    thread: &ThreadSignals,
    signal: u32,
    action: &Action,
    origin: Origin,
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
        blocked: thread.blocked,
        alternate_stack: alternate.to_bytes(stack),
    };
    let (info, trap) = match fault {
        Some(fault) => (signal_frame::fault_info(fault), Some(&fault.trap)),
        None => (
            signal_frame::info(signal, origin, own_id(process) as u32),
            None,
        ),
    };
    let handler = signal_frame::Handler {
        entry: action.handler,
        restorer,
        signal,
    };
    signal_frame::write(process, top, &saved, &info, trap, &handler, &mut registers)?;
    context.set_registers(&registers);
    Ok(())
}

/// `rt_sigreturn()`: a handler returned. Restores the registers, blocked mask and alternate stack
/// its frame saved (see [`signal_frame`]), and returns the restored result register, which the
/// dispatcher writes back unchanged. A frame that is not the program's memory, or whose FP/SIMD
/// records are not in Linux's shape, sends `SIGSEGV` (whatever the program blocked or ignored), as
/// on Linux, and changes nothing else. The signals the restored mask unblocks are delivered next.
pub fn rt_sigreturn<P: Platform>(
    process: &Process<P>,
    context: &mut UserContext,
) -> Result<u64, Errno> {
    let current = context.registers();
    match signal_frame::read(process, &current) {
        Ok(saved) => {
            context.set_registers(&saved.registers);
            process
                .signals()
                .lock()
                .restore(saved.blocked, saved.alternate_stack, context.stack());
            Ok(signal_frame::result(&saved.registers))
        }
        // As Linux, 0: only a `SIGSEGV` handler's frame can show it.
        Err(BadFrame) => {
            process.signals().lock().force(SIGSEGV);
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_signal_starts_with_its_default_action() {
        let signals = Signals::new();
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
        let mut signals = Signals::new();
        // Setting the default action changes nothing: no table is made.
        assert_eq!(
            signals.set_action(13, Action::default()),
            Ok(Action::default())
        );
        assert!(signals.caught.is_none());
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
        assert!(signals.caught.is_none());
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
        let mut signals = Signals::new();
        // Ignored and not blocked: discarded when sent.
        signals.send(SIGCHLD, Origin::Process);
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, ignore).unwrap();
        signals.send(SIGUSR1, Origin::Process);
        assert_eq!(signals.thread.pending, 0);
        // Blocked: pending, ignored or not, and reported; an ignored one is discarded once
        // unblocked, a terminating one ends the process.
        signals.thread.blocked = bit(SIGCHLD) | bit(SIGUSR1) | bit(SIGABRT);
        for signal in [SIGCHLD, SIGUSR1, SIGABRT] {
            signals.send(signal, Origin::Process);
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
        signals.send(SIGABRT, Origin::Process);
        signals.set_action(SIGABRT, ignore).unwrap();
        assert_eq!(signals.thread.pending, 0);
        // A handler runs, with where the signal came from.
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(SIGUSR1, Origin::Thread);
        assert_eq!(signals.blocked_pending(), 0);
        assert_eq!(
            signals.next_delivery(),
            Some(Delivery::Handler {
                signal: SIGUSR1,
                action: handler,
                origin: Origin::Thread
            })
        );
        assert_eq!(signals.thread.pending, 0);
        // The default action again: due.
        signals.set_action(SIGUSR1, Action::default()).unwrap();
        signals.send(SIGUSR1, Origin::Process);
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGUSR1)));
        // Lowest number first; SIGKILL whatever the mask said.
        signals.thread.blocked = 0;
        signals.send(64, Origin::Process);
        signals.send(SIGKILL, Origin::Process);
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(SIGKILL)));
        assert_eq!(signals.next_delivery(), Some(Delivery::Kill(64)));
        // A terminal's stop signals are discarded (an orphaned process group); SIGSTOP stops.
        signals.send(SIGTSTP, Origin::Process);
        assert_eq!(signals.next_delivery(), None);
        signals.send(SIGSTOP, Origin::Process);
        assert_eq!(signals.next_delivery(), Some(Delivery::Stop(SIGSTOP)));
    }

    #[test]
    fn sigcont_and_the_stop_signals_cancel_each_other() {
        let mut signals = Signals::new();
        signals.thread.blocked = !UNBLOCKABLE;
        for signal in [20, 21, 22] {
            signals.send(signal, Origin::Process);
        }
        signals.send(SIGCONT, Origin::Process);
        assert_eq!(signals.thread.pending, bit(SIGCONT));
        signals.send(21, Origin::Process);
        assert_eq!(signals.thread.pending, bit(21));
        // SIGSTOP cannot be blocked: sent, it is pending until delivered.
        signals.send(SIGCONT, Origin::Process);
        signals.send(SIGSTOP, Origin::Process);
        assert_eq!(signals.thread.pending, bit(SIGSTOP));
        signals.send(SIGCONT, Origin::Process);
        assert_eq!(signals.thread.pending, bit(SIGCONT));
        signals.thread.blocked = 0;
        assert_eq!(signals.next_delivery(), None);
        assert_eq!(signals.thread.pending, 0);
    }

    #[test]
    fn a_signal_keeps_the_origin_it_was_first_sent_from() {
        const SIGUSR1: u32 = 10;
        let mut signals = Signals::new();
        signals.thread.blocked = bit(SIGUSR1);
        // A process that catches nothing keeps no origins: only a handler sees one.
        signals.send(SIGUSR1, Origin::Thread);
        assert_eq!(signals.origin(SIGUSR1), Origin::Process);
        signals.discard(bit(SIGUSR1));
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(SIGUSR1, Origin::Thread);
        signals.send(SIGUSR1, Origin::Process);
        assert_eq!(signals.origin(SIGUSR1), Origin::Thread);
        signals.thread.blocked = 0;
        let delivery = signals.next_delivery();
        assert!(matches!(
            delivery,
            Some(Delivery::Handler {
                origin: Origin::Thread,
                ..
            })
        ));
        // Taken: its origin went with it.
        signals.send(SIGUSR1, Origin::Process);
        assert_eq!(signals.origin(SIGUSR1), Origin::Process);
        // Discarded by an action that ignores it: its origin goes too.
        signals.discard(bit(SIGUSR1));
        signals.thread.blocked = bit(SIGUSR1);
        signals.send(SIGUSR1, Origin::Thread);
        let ignore = Action {
            handler: IGNORE,
            ..Action::default()
        };
        signals.set_action(SIGUSR1, ignore).unwrap();
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.thread.blocked = 0;
        signals.send(SIGUSR1, Origin::Process);
        assert_eq!(signals.origin(SIGUSR1), Origin::Process);
    }

    #[test]
    fn handlers_block_their_mask_and_signal_and_may_reset() {
        const SIGSEGV: u32 = 11;
        const SIGUSR1: u32 = 10;
        const SIGUSR2: u32 = 12;
        let mut signals = Signals::new();
        let handler = Action {
            handler: 0x40_1000,
            flags: flag::RESETHAND,
            restorer: 0,
            mask: bit(SIGUSR2) | bit(SIGKILL),
        };
        signals.set_action(SIGUSR1, handler).unwrap();
        signals.send(SIGUSR1, Origin::Process);
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
        let mut signals = Signals::new();
        // Blocked, with a handler: unblocked, by the default action.
        let handler = Action {
            handler: 0x40_1000,
            ..Action::default()
        };
        signals.set_action(SIGSEGV, handler).unwrap();
        signals.thread.blocked = bit(SIGSEGV) | bit(1);
        signals.force(SIGSEGV);
        assert_eq!(signals.thread.blocked, bit(1));
        assert_eq!(signals.origin(SIGSEGV), Origin::Kernel);
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
                origin: Origin::Kernel
            })
        );
    }
}
