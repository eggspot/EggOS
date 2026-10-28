//! `/dev/agent` for Linux programs (D136): the agent channel ([`crate::agent_channel`]) as a
//! character device. Only a privileged process may open it, and only one at a time (`EBUSY`);
//! `read` returns the bytes the host sent (waiting for some unless `O_NONBLOCK`), or 0 once
//! when the host side closed; `write` sends all of its bytes, waiting while the host has not
//! taken the last ones; `fstat` reports the character device; `lseek` is `ESPIPE`, `ioctl`
//! `ENOTTY` and `mmap` `ENODEV`. The device does not interrupt yet (issue #97), so a wait polls
//! it: every [`ACTIVE_POLL_MS`] while bytes moved in the last [`ACTIVE_WINDOW_MS`], every
//! [`IDLE_POLL_MS`] otherwise.

use super::descriptors::{FileKind, OpenFile};
use super::signals::{self, Interrupted};
use super::terminal::{Waited, wait_for_input};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, iovec};
use crate::agent_channel::{self, OpenError, Received};
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::time::mul_div_ceil;
use eggos_hal::user::UserContext;

/// How often a wait polls the device while bytes moved lately.
pub const ACTIVE_POLL_MS: u64 = 4;
/// How often a wait polls the device otherwise.
pub const IDLE_POLL_MS: u64 = 32;
/// How long after bytes last moved a wait polls at [`ACTIVE_POLL_MS`].
pub const ACTIVE_WINDOW_MS: u64 = 1000;

const PIECE: usize = 512;

static LAST_TRAFFIC: AtomicU64 = AtomicU64::new(0);

/// What `/dev/agent`'s open file description holds: the channel, which closing the last
/// descriptor on it lets go.
#[derive(Debug, PartialEq, Eq)]
pub struct AgentChannelEnd(());

impl Drop for AgentChannelEnd {
    fn drop(&mut self) {
        agent_channel::close();
    }
}

/// Opens the channel for `process`: `EPERM` unless it is privileged, `ENXIO` without a
/// channel, `EBUSY` while another opener holds it.
pub fn open<P: Platform>(process: &Process<P>) -> Result<FileKind, Errno> {
    let privileged = process
        .credentials_and_limits()
        .lock()
        .credentials
        .is_privileged();
    if !privileged {
        return Err(Errno::EPERM);
    }
    match agent_channel::open() {
        Ok(()) => Ok(FileKind::AgentChannel(AgentChannelEnd(()))),
        Err(OpenError::Absent) => Err(Errno::ENXIO),
        Err(OpenError::Busy) => Err(Errno::EBUSY),
    }
}

fn note_traffic<P: Platform>() {
    LAST_TRAFFIC.store(P::clock_ticks(), Ordering::Relaxed);
}

fn next_poll<P: Platform>() -> u64 {
    let now = P::clock_ticks();
    let frequency = P::clock_frequency();
    let window = mul_div_ceil(ACTIVE_WINDOW_MS, frequency, 1000);
    let interval = match now.saturating_sub(LAST_TRAFFIC.load(Ordering::Relaxed)) < window {
        true => ACTIVE_POLL_MS,
        false => IDLE_POLL_MS,
    };
    now.saturating_add(mul_div_ceil(interval, frequency, 1000))
}

/// `read` or `readv` of `buffers` from `/dev/agent` through `file`; sets the call's result and
/// delivers the signals due after it.
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
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let Some((base, length)) = buffers
        .entries()
        .iter()
        .map(iovec)
        .find(|&(_, length)| length > 0)
    else {
        return ControlFlow::Continue(Some(Ok(0)));
    };
    let mut piece = [0u8; PIECE];
    let wanted = (length as usize).min(PIECE);
    loop {
        match agent_channel::receive(&mut piece[..wanted]) {
            Received::Bytes(count) => {
                note_traffic::<P>();
                let copied = process.write_user_memory(base, &piece[..count]);
                return ControlFlow::Continue(Some(match copied {
                    Ok(()) => Ok(count as u64),
                    Err(_) => Err(Errno::EFAULT),
                }));
            }
            Received::EndOfInput => return ControlFlow::Continue(Some(Ok(0))),
            Received::Nothing => {}
        }
        if file.is_nonblocking() {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let deadline = next_poll::<P>();
        if wait_for_input(process, thread, deadline, agent_channel::ready_to_receive)?
            == Waited::Signalled
        {
            return ControlFlow::Continue(None);
        }
    }
}

/// `write` or `writev` of `buffers` to `/dev/agent`; sets the call's result and delivers the
/// signals due after it.
pub(super) fn write<P: Platform>(
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
        match write_or_wait(process, thread, file, buffers)? {
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

fn write_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let total = buffers.total().min(super::MAX_RW_COUNT);
    let mut piece = [0u8; PIECE];
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
            let size = (length - done).min(PIECE as u64) as usize;
            if process
                .read_user_memory(base + done, &mut piece[..size])
                .is_err()
            {
                return ControlFlow::Continue(Some(partial(written, Errno::EFAULT)));
            }
            let mut sent = 0;
            while sent < size {
                match agent_channel::send(&piece[sent..size]) {
                    Ok(0) => {}
                    Ok(taken) => {
                        sent += taken;
                        written += taken as u64;
                        continue;
                    }
                    Err(_) => return ControlFlow::Continue(Some(partial(written, Errno::EIO))),
                }
                match wait_until_sent(process, thread, file)? {
                    None => continue,
                    Some(Errno::EINTR) => return ControlFlow::Continue(interrupted(written)),
                    Some(error) => return ControlFlow::Continue(Some(partial(written, error))),
                }
            }
            done += size as u64;
        }
    }
    loop {
        match agent_channel::flush() {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return ControlFlow::Continue(Some(partial(written, Errno::EIO))),
        }
        match wait_until_sent(process, thread, file)? {
            None => {}
            Some(Errno::EINTR) => return ControlFlow::Continue(interrupted(written)),
            Some(error) => return ControlFlow::Continue(Some(partial(written, error))),
        }
    }
    note_traffic::<P>();
    ControlFlow::Continue(Some(Ok(written)))
}

fn interrupted(written: u64) -> Option<Result<u64, Errno>> {
    match written {
        0 => None,
        _ => Some(Ok(written)),
    }
}

fn wait_until_sent<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
) -> ControlFlow<(), Option<Errno>> {
    if file.is_nonblocking() {
        return ControlFlow::Continue(Some(Errno::EAGAIN));
    }
    let deadline = next_poll::<P>();
    match wait_for_input(process, thread, deadline, agent_channel::sent_everything)? {
        Waited::Signalled => ControlFlow::Continue(Some(Errno::EINTR)),
        _ => ControlFlow::Continue(None),
    }
}
