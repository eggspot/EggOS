//! `eventfd` (x86_64 only) and `eventfd2` (eventfd(2)): a file that holds a 64-bit counter,
//! which event loops (GLib's main context, Qt's event dispatcher, winit's waker) write to wake
//! another thread's `poll` or `epoll_wait`. A read takes the counter and makes it 0, or, with
//! `EFD_SEMAPHORE`, takes 1 from it; it waits while the counter is 0, unless `O_NONBLOCK`
//! (`EAGAIN`). A write adds to it and waits while the sum would pass the largest value,
//! `0xfffffffffffffffe`, unless `O_NONBLOCK` (`EAGAIN`). Both move exactly 8 bytes, `EINVAL`
//! for a smaller buffer, and a write of `u64::MAX` is `EINVAL`. `poll` reports it readable
//! while the counter is not 0 and writable while 1 more fits. Waiters are on its wake list
//! ([`WakeList`]), woken by every read and write.

use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::poll::{self, INPUT_EVENTS, OUTPUT_EVENTS, WakeList};
use super::terminal::Waited;
use super::thread::LinuxThread;
use super::{Errno, Iovecs};
use crate::process::Process;
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// `eventfd2`'s flags (`EFD_*`).
pub mod flag {
    use super::open_flag;

    /// A read takes 1 instead of the whole counter.
    pub const SEMAPHORE: u32 = 1;
    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = open_flag::CLOEXEC;
    /// Reads and writes that would wait fail with `EAGAIN` instead.
    pub const NONBLOCK: u32 = open_flag::NONBLOCK;
}

/// The largest value the counter holds.
pub const MAX_VALUE: u64 = u64::MAX - 1;

/// How many bytes a read or write moves: one `uint64_t`.
pub const VALUE_SIZE: u64 = 8;

/// An `eventfd`'s counter: see the module documentation.
#[derive(Debug)]
pub struct EventFd {
    counter: LeafSpinLock<u64>,
    semaphore: bool,
    wake_list: Arc<WakeList>,
}

impl EventFd {
    /// A counter at `initial`, read as a semaphore if `semaphore`.
    pub fn new(initial: u64, semaphore: bool) -> Arc<Self> {
        Arc::new(Self {
            counter: LeafSpinLock::new_leaf(initial),
            semaphore,
            wake_list: WakeList::shared(),
        })
    }

    /// The threads and `epoll` instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// The counter now.
    pub fn value(&self) -> u64 {
        *self.counter.lock()
    }

    /// The `poll` events of `requested` it reports: readable while the counter is not 0,
    /// writable while 1 more fits.
    pub fn events(&self, requested: u16) -> u16 {
        let value = *self.counter.lock();
        let mut events = 0;
        if value > 0 {
            events |= requested & INPUT_EVENTS;
        }
        if value < MAX_VALUE {
            events |= requested & OUTPUT_EVENTS;
        }
        events
    }

    /// What a read takes: the counter, made 0, or 1 as a semaphore; `None` while it is 0.
    pub fn take(&self) -> Option<u64> {
        let taken = {
            let mut counter = self.counter.lock();
            match *counter {
                0 => None,
                value => {
                    let taken = if self.semaphore { 1 } else { value };
                    *counter = value - taken;
                    Some(taken)
                }
            }
        };
        if taken.is_some() {
            self.wake_list.wake();
        }
        taken
    }

    /// Adds `value` if the sum stays at most [`MAX_VALUE`]: whether it did.
    pub fn add(&self, value: u64) -> bool {
        let added = {
            let mut counter = self.counter.lock();
            match MAX_VALUE - *counter >= value {
                true => {
                    *counter += value;
                    true
                }
                false => false,
            }
        };
        if added {
            self.wake_list.wake();
        }
        added
    }

    fn has_room_for(&self, value: u64) -> bool {
        MAX_VALUE - *self.counter.lock() >= value
    }
}

impl Drop for EventFd {
    fn drop(&mut self) {
        self.wake_list.wake();
    }
}

/// `eventfd2(initval, flags)`: a new counter at `initval` (an `unsigned int`), open for reading
/// and writing, its descriptor returned. As Linux: `EINVAL` for flags other than
/// `EFD_SEMAPHORE`, `EFD_CLOEXEC` and `EFD_NONBLOCK`, `EMFILE` without a free descriptor,
/// `ENOMEM` without memory. x86_64's `eventfd(initval)` is this with no flags.
pub fn eventfd2<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    initial: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !(flag::SEMAPHORE | flag::CLOEXEC | flag::NONBLOCK) != 0 {
        return Err(Errno::EINVAL);
    }
    if !crate::heap::may_grow_for_user(size_of::<EventFd>() + size_of::<WakeList>()) {
        return Err(Errno::ENOMEM);
    }
    let counter = EventFd::new(u64::from(initial as u32), flags & flag::SEMAPHORE != 0);
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::EventFd(counter)),
        open_flag::RDWR | flags & flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & flag::CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

/// `read` or `readv` of `buffers` from `counter`, whose description is `file`: the counter, 8
/// bytes, as the module documentation says. Sets the call's result.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    counter: &EventFd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    if buffers.total() < VALUE_SIZE {
        context.set_syscall_result(Errno::EINVAL.result());
        return ControlFlow::Continue(());
    }
    poll::blocking(process, thread, context, |process, thread| {
        loop {
            if let Some(value) = counter.take() {
                let copied = poll::copy_to_buffers(process, buffers, 0, &value.to_ne_bytes());
                return ControlFlow::Continue(Some(copied.map(|()| VALUE_SIZE)));
            }
            if file.is_nonblocking() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            let lists = [counter.wake_list()];
            let ready = || counter.value() > 0;
            if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
                return ControlFlow::Continue(None);
            }
        }
    })
}

/// `write` or `writev` of `buffers` to `counter`, whose description is `file`: adds the 8 bytes
/// at their start, as the module documentation says. As Linux: `EINVAL` for fewer than 8 bytes,
/// `EFAULT` if they are not the program's memory, `EINVAL` for `u64::MAX`. Sets the call's
/// result.
pub(super) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    counter: &EventFd,
    buffers: &Iovecs,
) -> ControlFlow<()> {
    let mut bytes = [0u8; VALUE_SIZE as usize];
    let value = match buffers.total() < VALUE_SIZE {
        true => Err(Errno::EINVAL),
        false => poll::copy_from_buffers(&**process, buffers, &mut bytes)
            .map(|()| u64::from_ne_bytes(bytes))
            .and_then(|value| match value {
                u64::MAX => Err(Errno::EINVAL),
                value => Ok(value),
            }),
    };
    let value = match value {
        Ok(value) => value,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    poll::blocking(process, thread, context, |process, thread| {
        loop {
            if counter.add(value) {
                return ControlFlow::Continue(Some(Ok(VALUE_SIZE)));
            }
            if file.is_nonblocking() {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            let lists = [counter.wake_list()];
            let ready = || counter.has_room_for(value);
            if poll::wait_for(process, thread, &lists, u64::MAX, ready)? == Waited::Signalled {
                return ControlFlow::Continue(None);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::poll::event;

    #[test]
    fn a_counter_is_taken_whole_or_one_at_a_time_as_a_semaphore() {
        let counter = EventFd::new(0, false);
        assert_eq!(counter.take(), None);
        assert!(counter.add(3));
        assert!(counter.add(4));
        assert_eq!(counter.take(), Some(7));
        assert_eq!(counter.take(), None);
        let semaphore = EventFd::new(2, true);
        assert_eq!(semaphore.take(), Some(1));
        assert_eq!(semaphore.take(), Some(1));
        assert_eq!(semaphore.take(), None);
    }

    #[test]
    fn the_counter_stops_below_u64_max_and_reports_readable_and_writable() {
        let counter = EventFd::new(0, false);
        assert_eq!(counter.events(event::IN | event::OUT), event::OUT);
        assert!(counter.add(MAX_VALUE));
        assert_eq!(counter.events(event::IN | event::OUT), event::IN);
        assert!(!counter.add(1));
        assert!(counter.add(0));
        assert_eq!(counter.take(), Some(MAX_VALUE));
        assert!(counter.add(MAX_VALUE - 1));
        assert!(counter.add(1));
        assert!(!counter.add(1));
    }
}
