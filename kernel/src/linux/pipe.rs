//! Pipes (D125): `pipe` (x86_64) and `pipe2`, and `read`, `readv`, `write`, `writev`, `poll`
//! and `fstat` on their ends.
//!
//! A pipe is a bounded ring of [`CAPACITY`] bytes behind a leaf lock, with a count of the open
//! file descriptions of each end. Its two ends are [`PipeEnd`]s in [`FileKind::Pipe`]; when
//! the last description of an end goes, its count drops and waiters look again. A read takes
//! what is there, waits while the pipe is empty and a writer is left, and returns 0 (the end
//! of the file) once none is; a write of at most [`PIPE_BUF`] bytes goes in whole or waits for
//! room, a longer one goes in as room appears, and a write with no reader left fails with
//! `EPIPE` after `SIGPIPE` is sent to the writing thread. `O_NONBLOCK` turns every wait into
//! `EAGAIN` (or what was done so far). User memory is copied through a buffer on the kernel
//! stack, never under the lock. Waiters wait on the console's input key and are woken with
//! its waiters ([`console::wake_input_waiters`]), so `poll` on the console and on pipes is one
//! wait and a process's end wakes them all.

use super::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use super::signals::{self, Interrupted};
use super::terminal::{Waited, wait_for_input};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec};
use crate::console;
use crate::process::Process;
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// How many bytes a pipe holds: Linux's default, sixteen pages.
pub const CAPACITY: usize = 64 * 1024;

/// The longest write that goes into a pipe whole, never mixed with another writer's bytes.
pub const PIPE_BUF: usize = 4096;

const READ_CHUNK: usize = 512;

const SIGPIPE: u32 = 13;

#[derive(Debug)]
struct Ring {
    bytes: Vec<u8>,
    start: usize,
    length: usize,
    readers: u32,
    writers: u32,
}

impl Ring {
    fn room(&self) -> usize {
        CAPACITY - self.length
    }

    fn push(&mut self, bytes: &[u8]) -> usize {
        let count = bytes.len().min(self.room());
        let end = (self.start + self.length) % CAPACITY;
        let first = count.min(CAPACITY - end);
        self.bytes[end..end + first].copy_from_slice(&bytes[..first]);
        self.bytes[..count - first].copy_from_slice(&bytes[first..count]);
        self.length += count;
        count
    }

    fn take(&mut self, buffer: &mut [u8]) -> usize {
        let count = buffer.len().min(self.length);
        let first = count.min(CAPACITY - self.start);
        buffer[..first].copy_from_slice(&self.bytes[self.start..self.start + first]);
        buffer[first..count].copy_from_slice(&self.bytes[..count - first]);
        self.start = (self.start + count) % CAPACITY;
        self.length -= count;
        if self.length == 0 {
            self.start = 0;
        }
        count
    }
}

/// A pipe: see the module documentation.
pub struct Pipe {
    ring: LeafSpinLock<Ring>,
    inode: u64,
    wake: fn(),
}

impl core::fmt::Debug for Pipe {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("Pipe").field(&self.inode).finish()
    }
}

impl Pipe {
    fn new(wake: fn()) -> Result<Arc<Self>, Errno> {
        static NEXT_INODE: AtomicU64 = AtomicU64::new(1);
        if !crate::heap::may_grow_for_user(CAPACITY) {
            return Err(Errno::ENOMEM);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(CAPACITY)
            .map_err(|_| Errno::ENOMEM)?;
        bytes.resize(CAPACITY, 0);
        Ok(Arc::new(Self {
            ring: LeafSpinLock::new_leaf(Ring {
                bytes,
                start: 0,
                length: 0,
                readers: 1,
                writers: 1,
            }),
            inode: NEXT_INODE.fetch_add(1, Ordering::Relaxed),
            wake,
        }))
    }
}

/// One end of a pipe, held by the open file descriptions of that end: the last one dropped
/// closes the end.
pub struct PipeEnd {
    pipe: Arc<Pipe>,
    writes: bool,
}

impl PipeEnd {
    /// The pipe's number, as `fstat` reports it (`st_ino`).
    pub fn inode(&self) -> u64 {
        self.pipe.inode
    }

    /// The bytes waiting to be read.
    pub fn available(&self) -> usize {
        self.pipe.ring.lock().length
    }

    /// The `poll` events of this end among `requested`, with `POLLHUP` and `POLLERR` always
    /// reported as Linux does: readable when bytes wait, hung up once no writer is left;
    /// writable with room for [`PIPE_BUF`] bytes, an error once no reader is left.
    pub fn events(&self, requested: u16) -> u16 {
        use super::poll::event;
        let ring = self.pipe.ring.lock();
        match self.writes {
            false => {
                let readable = match ring.length > 0 {
                    true => requested & (event::IN | event::RDNORM),
                    false => 0,
                };
                readable | if ring.writers == 0 { event::HUP } else { 0 }
            }
            true if ring.readers == 0 => event::ERR,
            true if ring.room() >= PIPE_BUF => requested & (event::OUT | event::WRNORM),
            true => 0,
        }
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        {
            let mut ring = self.pipe.ring.lock();
            match self.writes {
                true => ring.writers -= 1,
                false => ring.readers -= 1,
            }
        }
        (self.pipe.wake)();
    }
}

impl core::fmt::Debug for PipeEnd {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PipeEnd")
            .field("pipe", &self.pipe)
            .field("writes", &self.writes)
            .finish()
    }
}

impl PartialEq for PipeEnd {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pipe, &other.pipe) && self.writes == other.writes
    }
}

impl Eq for PipeEnd {}

const PIPE2_FLAGS: u32 = open_flag::CLOEXEC | open_flag::NONBLOCK;

/// A new pipe's two open file descriptions, reading then writing, with `O_NONBLOCK` if
/// `flags` has it.
pub fn new_pair<P: Platform>(flags: u32) -> Result<(Arc<OpenFile>, Arc<OpenFile>), Errno> {
    let pipe = Pipe::new(console::wake_input_waiters::<P>)?;
    let status = flags & open_flag::NONBLOCK;
    let reading = PipeEnd {
        pipe: Arc::clone(&pipe),
        writes: false,
    };
    let writing = PipeEnd { pipe, writes: true };
    Ok((
        Arc::new(OpenFile::new(
            FileKind::Pipe(reading),
            open_flag::RDONLY | status,
        )),
        Arc::new(OpenFile::new(
            FileKind::Pipe(writing),
            open_flag::WRONLY | status,
        )),
    ))
}

/// `pipe2(fds, flags)`: a new pipe, its reading end's descriptor then its writing end's
/// written to `fds` as two `int`s. As Linux: `EINVAL` for a flag other than `O_CLOEXEC` and
/// `O_NONBLOCK`, `EMFILE` without two free descriptors, `EFAULT` if `fds` is not the
/// program's writable memory (no descriptor is left open then). x86_64's `pipe` is this with
/// no flags.
pub fn pipe2<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fds: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !PIPE2_FLAGS != 0 {
        return Err(Errno::EINVAL);
    }
    let (reading, writing) = new_pair::<P>(flags)?;
    let close_on_exec = flags & open_flag::CLOEXEC != 0;
    let limit = open_file_limit(process);
    let files = thread.files();
    let read_fd = files.lock().open(reading, close_on_exec, limit)?;
    let write_fd = match files.lock().open(writing, close_on_exec, limit) {
        Ok(fd) => fd,
        Err(error) => {
            let closed = files.lock().remove(read_fd);
            drop(closed);
            return Err(error);
        }
    };
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&read_fd.to_ne_bytes());
    bytes[4..].copy_from_slice(&write_fd.to_ne_bytes());
    if process.write_user_memory(fds, &bytes).is_err() {
        let closed = {
            let mut table = files.lock();
            (table.remove(read_fd), table.remove(write_fd))
        };
        drop(closed);
        return Err(Errno::EFAULT);
    }
    Ok(0)
}

/// `read` or `readv` of `buffers` from pipe end `end` of `file`, which the caller checked is
/// open for reading; sets the call's result and delivers the signals due after it.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    end: &PipeEnd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match read_or_wait(process, thread, file, end, buffers)? {
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
    end: &PipeEnd,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let count = buffers.total().min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    loop {
        if let Some(result) = take_into(process, end, buffers, count) {
            return ControlFlow::Continue(Some(result));
        }
        if file.is_nonblocking() {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let ready = || {
            let ring = end.pipe.ring.lock();
            ring.length > 0 || ring.writers == 0
        };
        if wait_for_input(process, thread, u64::MAX, ready)? == Waited::Signalled {
            return ControlFlow::Continue(None);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taken {
    Bytes(usize),
    Empty,
    Ended,
}

impl PipeEnd {
    fn take(&self, buffer: &mut [u8]) -> Taken {
        let mut ring = self.pipe.ring.lock();
        match ring.take(buffer) {
            0 if ring.writers == 0 => Taken::Ended,
            0 => Taken::Empty,
            taken => Taken::Bytes(taken),
        }
    }
}

fn take_into<P: Platform>(
    process: &Process<P>,
    end: &PipeEnd,
    buffers: &Iovecs,
    count: u64,
) -> Option<Result<u64, Errno>> {
    let mut chunk = [0u8; READ_CHUNK];
    let mut copied = 0;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(count - copied);
        let mut done = 0;
        while done < length {
            let wanted = (length - done).min(READ_CHUNK as u64) as usize;
            let taken = match end.take(&mut chunk[..wanted]) {
                Taken::Bytes(taken) => taken,
                Taken::Empty if copied == 0 => return None,
                Taken::Empty | Taken::Ended => return Some(Ok(copied)),
            };
            (end.pipe.wake)();
            if process
                .write_user_memory(base + done, &chunk[..taken])
                .is_err()
            {
                return Some(match copied {
                    0 => Err(Errno::EFAULT),
                    _ => Ok(copied),
                });
            }
            done += taken as u64;
            copied += taken as u64;
        }
        if copied == count {
            break;
        }
    }
    Some(Ok(copied))
}

/// `write` or `writev` of `buffers` to pipe end `end` of `file`, which the caller checked is
/// open for writing; sets the call's result and delivers the signals due after it, `SIGPIPE`
/// among them when no reader is left.
pub(super) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    end: &PipeEnd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let result = loop {
        match write_or_wait(process, thread, file, end, buffers)? {
            Some(result) => break result,
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    if result == Err(Errno::EPIPE) {
        signals::send_from_kernel_to_self(process, thread, SIGPIPE);
    }
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn write_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    file: &OpenFile,
    end: &PipeEnd,
    buffers: &Iovecs,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let total = buffers.total().min(MAX_RW_COUNT);
    let whole = total <= PIPE_BUF as u64;
    let mut chunk = [0u8; PIPE_BUF];
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
            let size = (length - done).min(PIPE_BUF as u64) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                return ControlFlow::Continue(Some(partial(written, Errno::EFAULT)));
            }
            let mut sent = 0;
            while sent < size {
                let needed = if whole { size - sent } else { 1 };
                let pushed = {
                    let mut ring = end.pipe.ring.lock();
                    if ring.readers == 0 {
                        return ControlFlow::Continue(Some(partial(written, Errno::EPIPE)));
                    }
                    match ring.room() >= needed {
                        true => ring.push(&chunk[sent..size]),
                        false => 0,
                    }
                };
                if pushed > 0 {
                    (end.pipe.wake)();
                    sent += pushed;
                    written += pushed as u64;
                    continue;
                }
                if file.is_nonblocking() {
                    return ControlFlow::Continue(Some(partial(written, Errno::EAGAIN)));
                }
                let ready = || {
                    let ring = end.pipe.ring.lock();
                    ring.readers == 0 || ring.room() >= needed
                };
                if wait_for_input(process, thread, u64::MAX, ready)? == Waited::Signalled {
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

    fn ring() -> Ring {
        Ring {
            bytes: alloc::vec![0; CAPACITY],
            start: 0,
            length: 0,
            readers: 1,
            writers: 1,
        }
    }

    #[test]
    fn bytes_come_out_in_the_order_they_went_in_across_the_ring_s_end() {
        let mut ring = ring();
        let first: Vec<u8> = (0..CAPACITY - 10).map(|index| index as u8).collect();
        assert_eq!(ring.push(&first), first.len());
        let mut out = alloc::vec![0; CAPACITY - 20];
        assert_eq!(ring.take(&mut out), CAPACITY - 20);
        assert_eq!(out, first[..CAPACITY - 20]);
        assert_eq!(ring.push(b"0123456789abcdefghijklmnopqrstuvwxyz"), 36);
        let mut rest = [0; 46];
        assert_eq!(ring.take(&mut rest), 46);
        assert_eq!(&rest[..10], &first[CAPACITY - 20..]);
        assert_eq!(&rest[10..], b"0123456789abcdefghijklmnopqrstuvwxyz");
        assert_eq!(ring.length, 0);
        assert_eq!(ring.start, 0);
    }

    #[test]
    fn a_full_ring_takes_no_more_and_an_empty_one_gives_nothing() {
        let mut ring = ring();
        let full = alloc::vec![7; CAPACITY + 5];
        assert_eq!(ring.push(&full), CAPACITY);
        assert_eq!(ring.room(), 0);
        assert_eq!(ring.push(b"x"), 0);
        let mut out = alloc::vec![0; CAPACITY + 5];
        assert_eq!(ring.take(&mut out), CAPACITY);
        assert_eq!(ring.take(&mut out), 0);
    }

    #[test]
    fn an_empty_pipe_with_a_writer_says_wait_and_one_without_says_its_end() {
        fn no_wake() {}
        let pipe = Pipe::new(no_wake).unwrap();
        let reading = PipeEnd {
            pipe: Arc::clone(&pipe),
            writes: false,
        };
        let writing = PipeEnd {
            pipe: Arc::clone(&pipe),
            writes: true,
        };
        let mut buffer = [0u8; 8];
        assert_eq!(reading.take(&mut buffer), Taken::Empty);
        pipe.ring.lock().push(b"abc");
        assert_eq!(reading.take(&mut buffer), Taken::Bytes(3));
        assert_eq!(reading.take(&mut buffer), Taken::Empty);
        pipe.ring.lock().push(b"z");
        drop(writing);
        assert_eq!(reading.take(&mut buffer), Taken::Bytes(1));
        assert_eq!(reading.take(&mut buffer), Taken::Ended);
    }

    #[test]
    fn ends_report_poll_events_and_closing_one_end_is_seen_by_the_other() {
        use super::super::poll::event;
        fn no_wake() {}
        let pipe = Pipe::new(no_wake).unwrap();
        let reading = PipeEnd {
            pipe: Arc::clone(&pipe),
            writes: false,
        };
        let writing = PipeEnd {
            pipe: Arc::clone(&pipe),
            writes: true,
        };
        let all = u16::MAX;
        assert_eq!(reading.events(all), 0);
        assert_eq!(writing.events(all), event::OUT | event::WRNORM);
        pipe.ring.lock().push(b"hello");
        assert_eq!(reading.events(event::IN), event::IN);
        assert_eq!(reading.available(), 5);
        drop(writing);
        assert_eq!(reading.events(event::IN), event::IN | event::HUP);
        let writing = PipeEnd {
            pipe: Arc::clone(&pipe),
            writes: true,
        };
        pipe.ring.lock().writers += 1;
        drop(reading);
        assert_eq!(writing.events(event::OUT), event::ERR);
        assert_ne!(
            writing,
            PipeEnd {
                pipe: Pipe::new(no_wake).unwrap(),
                writes: true
            }
        );
    }
}
