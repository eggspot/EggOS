//! `signalfd` (x86_64 only) and `signalfd4` (signalfd(2)): signals read as records from a
//! file, as event loops (GLib's `g_unix_signal_add`, systemd, SDL) take them through `poll` or
//! `epoll` instead of handlers. A signalfd holds a set of signals (never `SIGKILL` or
//! `SIGSTOP`); a read takes the pending ones in the set, the reading thread's and those sent to
//! its process, each as a 128-byte `struct signalfd_siginfo`, as many as fit, waiting for one
//! while none is pending unless `O_NONBLOCK` (`EAGAIN`); `poll` reports it readable while one
//! is pending to the looking thread. Programs block the set first, so the signals stay
//! pending for it. A signal sent to a waiting thread ends its wait, blocked or not, so a
//! thread waiting in `poll`, `epoll_wait` or a read looks again; the file's wake list
//! ([`WakeList`]) wakes waiters when its set changes.

use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::poll::{self, INPUT_EVENTS, WakeList};
use super::signal::{SIGBUS, SIGCHLD, SIGFPE, SIGILL, SIGKILL, SIGSEGV, SIGSTOP, SIGTRAP};
use super::signals::{self, Info, code};
use super::terminal::Waited;
use super::thread::LinuxThread;
use super::{Errno, Iovecs};
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// `signalfd4`'s flags (`SFD_*`).
pub mod flag {
    use super::open_flag;

    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = open_flag::CLOEXEC;
    /// Reads that would wait fail with `EAGAIN` instead.
    pub const NONBLOCK: u32 = open_flag::NONBLOCK;
}

/// The size of a `struct signalfd_siginfo`.
pub const RECORD_SIZE: usize = 128;

const CODES_WITH_VALUE: [i32; 3] = [code::QUEUE, -2, -3];

const UNTAKEABLE: u64 = 1 << (SIGKILL - 1) | 1 << (SIGSTOP - 1);

/// A signalfd: see the module documentation.
#[derive(Debug)]
pub struct SignalFd {
    mask: AtomicU64,
    wake_list: Arc<WakeList>,
}

impl SignalFd {
    /// A signalfd for the signals in `mask`, bit `n - 1` for signal `n`.
    pub fn new(mask: u64) -> Arc<Self> {
        Arc::new(Self {
            mask: AtomicU64::new(mask & !UNTAKEABLE),
            wake_list: WakeList::shared(),
        })
    }

    /// The threads and `epoll` instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// Its signals.
    pub fn mask(&self) -> u64 {
        self.mask.load(Ordering::Relaxed)
    }

    /// Changes its signals to those in `mask`, and wakes its waiters to look again.
    pub fn set_mask(&self, mask: u64) {
        self.mask.store(mask & !UNTAKEABLE, Ordering::Relaxed);
        self.wake_list.wake();
    }

    /// The `poll` events of `requested` it reports to a thread to which (or to whose
    /// process) the signals in `pending` are pending: readable while one of its signals is.
    pub fn events(&self, requested: u16, pending: u64) -> u16 {
        match pending & self.mask() {
            0 => 0,
            _ => requested & INPUT_EVENTS,
        }
    }
}

/// `signalfd4(fd, mask, sizemask, flags)`: with `fd` -1, a new signalfd for the signal set at
/// `mask` (`SIGKILL` and `SIGSTOP` left out, as on Linux), open for reading, its descriptor
/// returned; otherwise the signalfd `fd` names takes that set (the flags are not looked at
/// then), and `fd` is returned. As Linux, in this order: `EINVAL` for flags other than
/// `SFD_CLOEXEC` and `SFD_NONBLOCK`, `EINVAL` if `sizemask` is not 8, `EFAULT` if the set is
/// not the program's memory, then `EBADF` if `fd` is not open, `EINVAL` if it is not a
/// signalfd, or for a new one `EMFILE` without a free descriptor and `ENOMEM` without memory.
/// x86_64's `signalfd(fd, mask, sizemask)` is this with no flags.
pub fn signalfd4<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    mask: u64,
    size: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !(flag::CLOEXEC | flag::NONBLOCK) != 0 {
        return Err(Errno::EINVAL);
    }
    let mask = signals::read_set(process, mask, size)?;
    let fd = fd as u32 as i32;
    if fd != -1 {
        let file = thread.files().file(u64::from(fd as u32))?;
        return match file.kind() {
            FileKind::Event(EventFile::SignalFd(signal_file)) => {
                signal_file.set_mask(mask);
                Ok(u64::from(fd as u32))
            }
            _ => Err(Errno::EINVAL),
        };
    }
    if !crate::heap::may_grow_for_user(size_of::<SignalFd>() + size_of::<WakeList>()) {
        return Err(Errno::ENOMEM);
    }
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::SignalFd(SignalFd::new(mask))),
        open_flag::RDONLY | flags & flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & flag::CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

/// The `struct signalfd_siginfo` of a signal taken with `info`: its number, error and code;
/// the sender's process and user ids for a signal a process sent; with them the value
/// `sigqueue` sent (`ssi_int`, `ssi_ptr`), or a child's status and times for `SIGCHLD`; the
/// faulting address for a fault's signal.
pub fn record(info: Info) -> [u8; RECORD_SIZE] {
    let siginfo = info.to_bytes();
    let mut record = [0u8; RECORD_SIZE];
    record[..12].copy_from_slice(&siginfo[..12]);
    let signal = info.signal();
    let code = info.code();
    let fault = [SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGTRAP].contains(&signal);
    match code {
        _ if fault && code > 0 => record[72..80].copy_from_slice(&siginfo[16..24]),
        _ if signal == SIGCHLD && code > 0 => {
            record[12..20].copy_from_slice(&siginfo[16..24]);
            record[40..44].copy_from_slice(&siginfo[24..28]);
            record[56..64].copy_from_slice(&siginfo[32..40]);
            record[64..72].copy_from_slice(&siginfo[40..48]);
        }
        code::KERNEL => {}
        _ => {
            record[12..20].copy_from_slice(&siginfo[16..24]);
            if CODES_WITH_VALUE.contains(&code) {
                record[44..48].copy_from_slice(&siginfo[24..28]);
                record[48..56].copy_from_slice(&siginfo[24..32]);
            }
        }
    }
    record
}

/// `read` or `readv` of `buffers` from `signal_file`, whose description is `file`: takes the
/// pending signals in its set, as many records as fit, and returns their size, waiting for one
/// unless `O_NONBLOCK` (`EAGAIN`); `EINVAL` for room for less than one record, `EFAULT` if the
/// first one cannot be written (the signal is lost, as on Linux). A signal that is not in the
/// set and not blocked interrupts the wait as it does a pipe's. Sets the call's result.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    signal_file: &SignalFd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let records = buffers.total() / RECORD_SIZE as u64;
    if records == 0 {
        context.set_syscall_result(Errno::EINVAL.result());
        return ControlFlow::Continue(());
    }
    poll::blocking(process, thread, context, |process, thread| {
        loop {
            let mut taken = 0;
            while taken < records {
                let Some(info) = signals::take_for_signalfd(process, thread, signal_file.mask())
                else {
                    break;
                };
                let offset = taken * RECORD_SIZE as u64;
                match poll::copy_to_buffers(process, buffers, offset, &record(info)) {
                    Ok(()) => taken += 1,
                    Err(errno) if taken == 0 => return ControlFlow::Continue(Some(Err(errno))),
                    Err(_) => break,
                }
            }
            if taken > 0 {
                return ControlFlow::Continue(Some(Ok(taken * RECORD_SIZE as u64)));
            }
            if file.is_nonblocking() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            let lists = [signal_file.wake_list()];
            if poll::wait_for(process, thread, &lists, u64::MAX, || false)? == Waited::Signalled {
                return ControlFlow::Continue(None);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::poll::event;
    use crate::linux::signal::SIGUSR1;

    #[test]
    fn a_record_carries_the_sender_and_the_queued_value() {
        let sent = record(Info::sent(SIGUSR1, code::TKILL, 42, 1000));
        assert_eq!(&sent[..4], &SIGUSR1.to_ne_bytes());
        assert_eq!(&sent[8..12], &code::TKILL.to_ne_bytes());
        assert_eq!(&sent[12..16], &42u32.to_ne_bytes());
        assert_eq!(&sent[16..20], &1000u32.to_ne_bytes());
        assert_eq!(&sent[20..], &[0u8; RECORD_SIZE - 20][..]);
        let mut queued = Info::sent(40, code::QUEUE, 7, 0).to_bytes();
        queued[24..32].copy_from_slice(&0x1234_5678u64.to_ne_bytes());
        let mut kept = [0u8; signals::KEPT_INFO_SIZE];
        kept.copy_from_slice(&queued[..signals::KEPT_INFO_SIZE]);
        let queued = record(Info::queued(40, kept));
        assert_eq!(&queued[44..48], &0x1234_5678u32.to_ne_bytes());
        assert_eq!(&queued[48..56], &0x1234_5678u64.to_ne_bytes());
        let kernel = record(Info::from_kernel(SIGUSR1));
        assert_eq!(&kernel[12..], &[0u8; RECORD_SIZE - 12][..]);
    }

    #[test]
    fn a_signalfd_never_holds_sigkill_or_sigstop_and_reports_its_own_signals() {
        let signal_file = SignalFd::new(u64::MAX);
        assert_eq!(signal_file.mask(), !UNTAKEABLE);
        signal_file.set_mask(1 << (SIGUSR1 - 1));
        assert_eq!(signal_file.events(event::IN, 1 << (SIGUSR1 - 1)), event::IN);
        assert_eq!(signal_file.events(event::IN, 1 << (SIGCHLD - 1)), 0);
        assert_eq!(signal_file.events(event::OUT, u64::MAX), 0);
    }
}
