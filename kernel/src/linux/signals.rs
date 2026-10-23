//! Signal state (M6, D60): `rt_sigaction`, `rt_sigprocmask` and `sigaltstack`. Rust's std
//! ignores `SIGPIPE` and installs `SIGSEGV` and `SIGBUS` handlers on an alternate stack at
//! startup, glibc blocks and unblocks signals around much of its work, and a failed call is
//! fatal to some of them, so the calls work and keep their state before signals are delivered.
//!
//! A process has an action per signal ([`Action`]), all `SIG_DFL` until it first changes one,
//! which makes its own table on the heap. Its thread has a blocked mask and an alternate stack
//! ([`ThreadSignals`]), which move to each thread when `clone` exists. Nothing is delivered
//! yet: a fault still ends the process with its signal whatever the actions and the mask say,
//! which is also what Linux does for a fault whose signal is blocked or ignored. Delivery
//! (signal frames, `rt_sigreturn`, `kill`) is the next step.

use super::Errno;
use super::signal::{SIGKILL, SIGSTOP};
use crate::process::Process;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_hal::Platform;

/// How many signals there are: 1 to 64 (`_NSIG`).
pub const COUNT: usize = 64;

/// The size of a signal set (`sigset_t` as the kernel takes it): the calls' `sigsetsize`
/// must be this.
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
const HAS_RESTORER: bool = !cfg!(target_arch = "riscv64");

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
    /// The process's actions: all [`Action::default`] (`SIG_DFL`) until one first changes,
    /// then a table of its own. A process costs a word for them, and starting one copies
    /// nothing (D59's lesson).
    actions: Option<Box<[Action; COUNT]>>,
    /// Its thread's.
    thread: ThreadSignals,
}

/// A thread's signal state: what it blocks and its alternate stack.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSignals {
    /// The blocked signals, bit `n - 1` for signal `n`; never [`UNBLOCKABLE`]'s.
    pub blocked: u64,
    /// The alternate stack for handlers with [`flag::ONSTACK`].
    pub alternate_stack: AlternateStack,
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

/// The size of `stack_t`: the base, the flags (an `int`, then 4 bytes of padding) and the size.
const STACK_SIZE: usize = 24;

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

    /// `stack_t` as the program sees it, for a thread whose stack pointer is `stack`.
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
            actions: None,
            thread: ThreadSignals {
                blocked: 0,
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
            self.actions
                .as_ref()
                .map_or_else(Action::default, |own| own[index]),
        )
    }

    /// Sets `signal`'s action to `new`, without its unknown flags and with `SIGKILL` and
    /// `SIGSTOP` left out of its mask, and returns the action it had. The first change that
    /// changes anything makes the process's own table.
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
            let own = match &mut self.actions {
                Some(own) => own,
                None => self.actions.insert(own_table()?),
            };
            own[signal as usize - 1] = new;
        }
        Ok(current)
    }

    /// Its thread's state.
    pub fn thread(&self) -> ThreadSignals {
        self.thread
    }
}

/// A table of default actions on the heap, or [`ChangeError::OutOfMemory`].
fn own_table() -> Result<Box<[Action; COUNT]>, ChangeError> {
    let mut actions = Vec::new();
    actions
        .try_reserve_exact(COUNT)
        .map_err(|_| ChangeError::OutOfMemory)?;
    actions.resize(COUNT, Action::default());
    // Exactly `COUNT` actions, in a buffer of exactly that capacity: no reallocation.
    Ok(actions.into_boxed_slice().try_into().unwrap())
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
            let base = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
            let flags = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
            let size = u64::from_ne_bytes(bytes[16..].try_into().unwrap());
            Some((base, flags, size))
        }
    };
    let previous = {
        let mut signals = process.signals().lock();
        let current = signals.thread.alternate_stack;
        if let Some((base, flags, size)) = new {
            if current.is_on(stack) {
                return Err(Errno::EPERM);
            }
            let autodisarm = flags & stack_flag::AUTODISARM != 0;
            signals.thread.alternate_stack = match flags & !stack_flag::AUTODISARM {
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
        assert!(signals.actions.is_none());
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
        assert!(signals.actions.is_none());
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
}
