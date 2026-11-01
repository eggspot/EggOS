//! File locks (D171): `flock`'s whole-file locks and `fcntl`'s record locks, which cargo,
//! rustc's incremental sessions, package managers and databases take on their files.
//!
//! - **`flock`** locks a whole file, shared or exclusive, for an open file description: every
//!   descriptor `dup` or `fork` made from it shares the lock, which goes with `LOCK_UN` or the
//!   description's last close. Converting a lock first lets go of the one held, as on Linux.
//! - **Record locks** (`F_SETLK`, `F_SETLKW`, `F_GETLK`) lock byte ranges, read or write, for
//!   a process: they go when it unlocks them, closes any descriptor of the file with `close`,
//!   or ends. Open-file-description locks (`F_OFD_*`) are record locks owned by the
//!   description instead, as `flock`'s are. A range may run to the end of the largest file;
//!   a process's own locks on a file are split, replaced and merged as Linux does.
//! - **Conflicts.** Two locks of different owners on the same file, of the same family
//!   (`flock`, or record), conflict when their ranges overlap and either is exclusive. A
//!   request that conflicts fails with `EAGAIN` (`LOCK_NB`, `F_SETLK`), or waits until the
//!   file's locks change, then looks again (`flock`, `F_SETLKW`): a signal interrupts the wait,
//!   which is restarted after an `SA_RESTART` handler, as on Linux.
//! - **Deadlocks** (D173). A process's `F_SETLKW` that would wait for a lock whose holder waits,
//!   directly or through at most [`MAX_DEADLOCK_STEPS`] other waiting processes, for a lock the
//!   caller holds fails with `EDEADLK` instead, as Linux's; open-file-description and `flock`
//!   waits are not followed, as on Linux.
//! - **Closing** (D173): a process's record locks on a file go with any of its descriptors for
//!   it closed by `close`, `close_range`, `dup2` or `dup3` over it, or `execve`'s close-on-exec.
//!
//! A file is a node (whatever path or descriptor reaches it), a memory file, or for anything
//! else its open file description. Every lock is in one table behind a lock, made by the first
//! lock taken, which a change holds for a few instructions; a count of them is kept apart, so
//! closing a file while no lock exists costs a relaxed load.

use super::Errno;
use super::descriptors::{Descriptors, FileKind, OpenFile};
use super::poll::{self, WakeList};
use super::signals::{self, Interrupted};
use super::terminal::Waited;
use super::thread::LinuxThread;
use crate::process::Process;
use crate::sync::SpinLock;
use crate::vfs::Node;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// `flock`'s operations (`LOCK_*`).
pub mod operation {
    /// A shared lock.
    pub const SHARED: u32 = 1;
    /// An exclusive lock.
    pub const EXCLUSIVE: u32 = 2;
    /// Fail with `EWOULDBLOCK` instead of waiting.
    pub const NONBLOCK: u32 = 4;
    /// Let go of the lock.
    pub const UNLOCK: u32 = 8;
}

/// `fcntl`'s lock commands (`F_*`), the generic values, which x86_64 shares.
pub mod command {
    /// Which lock would stand in the way of one.
    pub const GETLK: u32 = 5;
    /// Takes or lets go of a record lock, failing at once on a conflict.
    pub const SETLK: u32 = 6;
    /// Takes or lets go of a record lock, waiting out a conflict.
    pub const SETLKW: u32 = 7;
    /// [`GETLK`] for open-file-description locks.
    pub const OFD_GETLK: u32 = 36;
    /// [`SETLK`] for open-file-description locks.
    pub const OFD_SETLK: u32 = 37;
    /// [`SETLKW`] for open-file-description locks.
    pub const OFD_SETLKW: u32 = 38;
}

/// `struct flock`'s `l_type` (`F_RDLCK`, `F_WRLCK`, `F_UNLCK`).
pub mod lock_type {
    /// A read (shared) lock.
    pub const READ: i16 = 0;
    /// A write (exclusive) lock.
    pub const WRITE: i16 = 1;
    /// No lock: a request to let go, or `F_GETLK`'s answer that nothing stands in the way.
    pub const UNLOCK: i16 = 2;
}

/// The size of `struct flock` on 64-bit machines.
pub const FLOCK_SIZE: usize = 32;

/// The largest offset, where a range "to the end of the file" ends (`OFFSET_MAX`).
pub const OFFSET_MAX: u64 = i64::MAX as u64;

/// How many waiting processes a deadlock check follows from the lock in the way, as Linux's
/// `MAX_DEADLK_ITERATIONS`: a longer chain is not reported.
pub const MAX_DEADLOCK_STEPS: usize = 10;

const SEEK_SET: i16 = 0;
const SEEK_CUR: i16 = 1;
const SEEK_END: i16 = 2;

/// What a lock is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedFile {
    /// A file system's node.
    Node(Node),
    /// Something else, by its address: a memory file, or an open file description.
    Object(usize),
}

/// Who holds a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A process, by id: record locks.
    Process(u32),
    /// An open file description, by address: `flock`'s and open-file-description locks.
    Description(usize),
}

/// Whether a lock lets others hold one too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// Others may hold shared locks beside it (`LOCK_SH`, `F_RDLCK`).
    Shared,
    /// Nobody else may hold one (`LOCK_EX`, `F_WRLCK`).
    Exclusive,
}

/// Which family a lock is of: the two never conflict with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `flock`'s whole-file locks.
    Whole,
    /// `fcntl`'s record locks.
    Record,
}

/// One lock: see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lock {
    /// What it is on.
    pub file: LockedFile,
    /// Its family.
    pub family: Family,
    /// Who holds it.
    pub owner: Owner,
    /// Whether it is shared.
    pub sharing: Sharing,
    /// Its first byte.
    pub start: u64,
    /// Its last byte, [`OFFSET_MAX`] for the end of the file.
    pub end: u64,
}

impl Lock {
    fn is_held_by(&self, file: LockedFile, family: Family, owner: Owner) -> bool {
        self.file == file && self.family == family && self.owner == owner
    }

    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.start <= end && start <= self.end
    }

    fn conflicts_with(&self, other: &Lock) -> bool {
        self.file == other.file
            && self.family == other.family
            && self.owner != other.owner
            && self.overlaps(other.start, other.end)
            && (self.sharing == Sharing::Exclusive || other.sharing == Sharing::Exclusive)
    }
}

/// A thread waiting for a record lock of its process (`F_SETLKW`), which deadlock checks follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waiter {
    /// The waiting thread.
    pub thread: u32,
    /// The lock it waits to take.
    pub wanted: Lock,
}

/// Every lock held, and the processes' threads waiting for record locks.
#[derive(Debug, Default)]
pub struct Table {
    locks: Vec<Lock>,
    waiters: Vec<Waiter>,
}

impl Table {
    /// A table with no lock.
    pub const fn new() -> Self {
        Self {
            locks: Vec::new(),
            waiters: Vec::new(),
        }
    }

    /// The locks held, in no order.
    pub fn locks(&self) -> &[Lock] {
        &self.locks
    }

    /// The threads waiting for record locks, in no order.
    pub fn waiters(&self) -> &[Waiter] {
        &self.waiters
    }

    /// Whether `wanted`, a process's record lock that `in_the_way` stands in the way of, would
    /// wait in a cycle: the holder of `in_the_way` waits for a lock held by a process that
    /// waits ... for one `wanted`'s owner holds, within [`MAX_DEADLOCK_STEPS`] steps (`EDEADLK`).
    /// Always `false` for open-file-description and `flock` locks, as on Linux.
    pub fn would_deadlock(&self, wanted: &Lock, in_the_way: &Lock) -> bool {
        if wanted.family != Family::Record || !matches!(wanted.owner, Owner::Process(_)) {
            return false;
        }
        let mut holder = in_the_way.owner;
        for _ in 0..MAX_DEADLOCK_STEPS {
            if holder == wanted.owner {
                return true;
            }
            let Some(waiting) = self
                .waiters
                .iter()
                .find(|waiter| waiter.wanted.owner == holder)
            else {
                return false;
            };
            let Some(next) = self.conflict(&waiting.wanted) else {
                return false;
            };
            holder = next.owner;
        }
        false
    }

    /// Records that `thread` waits to take `wanted`, a record lock of its process. `ENOLCK`
    /// without memory for the record.
    pub fn wait(&mut self, thread: u32, wanted: Lock) -> Result<(), Errno> {
        self.waiters.try_reserve(1).map_err(|_| Errno::ENOLCK)?;
        self.waiters.push(Waiter { thread, wanted });
        Ok(())
    }

    /// Forgets that `thread` waits for a lock.
    pub fn stop_waiting(&mut self, thread: u32) {
        self.waiters.retain(|waiter| waiter.thread != thread);
    }

    /// A lock that stands in the way of `wanted`, if one does.
    pub fn conflict(&self, wanted: &Lock) -> Option<Lock> {
        self.locks
            .iter()
            .find(|held| held.conflicts_with(wanted))
            .copied()
    }

    /// Gives `wanted`'s owner `wanted`, in place of what it held of that range of the file,
    /// merged with its locks of the same kind that touch it. The caller checked that nothing
    /// stands in the way. `ENOLCK` without memory for it.
    pub fn set(&mut self, wanted: Lock) -> Result<(), Errno> {
        self.locks.try_reserve(2).map_err(|_| Errno::ENOLCK)?;
        self.cut(
            wanted.file,
            wanted.family,
            wanted.owner,
            wanted.start,
            wanted.end,
        );
        let (mut start, mut end) = (wanted.start, wanted.end);
        let mut index = 0;
        while index < self.locks.len() {
            let held = self.locks[index];
            let touches =
                held.end.checked_add(1) == Some(start) || end.checked_add(1) == Some(held.start);
            if held.is_held_by(wanted.file, wanted.family, wanted.owner)
                && held.sharing == wanted.sharing
                && touches
            {
                start = start.min(held.start);
                end = end.max(held.end);
                self.locks.swap_remove(index);
            } else {
                index += 1;
            }
        }
        self.locks.push(Lock {
            start,
            end,
            ..wanted
        });
        Ok(())
    }

    /// Lets go of what `owner` holds of bytes `start` to `end` of `file` in `family`, splitting
    /// a lock that reaches past them: whether anything went. `ENOLCK` without memory for a
    /// split.
    pub fn unlock(
        &mut self,
        file: LockedFile,
        family: Family,
        owner: Owner,
        start: u64,
        end: u64,
    ) -> Result<bool, Errno> {
        self.locks.try_reserve(1).map_err(|_| Errno::ENOLCK)?;
        Ok(self.cut(file, family, owner, start, end))
    }

    fn cut(
        &mut self,
        file: LockedFile,
        family: Family,
        owner: Owner,
        start: u64,
        end: u64,
    ) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < self.locks.len() {
            let held = self.locks[index];
            if !held.is_held_by(file, family, owner) || !held.overlaps(start, end) {
                index += 1;
                continue;
            }
            changed = true;
            let before = (held.start < start).then(|| Lock {
                end: start - 1,
                ..held
            });
            let after = (held.end > end).then(|| Lock {
                start: end + 1,
                ..held
            });
            match (before, after) {
                (Some(before), Some(after)) => {
                    self.locks[index] = before;
                    self.locks.push(after);
                    index += 1;
                }
                (Some(part), None) | (None, Some(part)) => {
                    self.locks[index] = part;
                    index += 1;
                }
                (None, None) => {
                    self.locks.swap_remove(index);
                }
            }
        }
        changed
    }

    /// Lets go of every lock `keep` refuses: whether any went.
    pub fn remove_unless(&mut self, keep: impl Fn(&Lock) -> bool) -> bool {
        let before = self.locks.len();
        self.locks.retain(keep);
        self.locks.len() != before
    }
}

static TABLE: SpinLock<Option<Box<Table>>> = SpinLock::new(None);

static LOCKS: AtomicUsize = AtomicUsize::new(0);

static WAITERS: SpinLock<Option<Arc<WakeList>>> = SpinLock::new(None);

fn change<R>(changing: impl FnOnce(&mut Table) -> R) -> Option<R> {
    let mut slot = TABLE.lock();
    let table = slot.as_mut()?;
    let result = changing(table);
    LOCKS.store(table.locks.len(), Ordering::Relaxed);
    Some(result)
}

fn change_made<R>(changing: impl FnOnce(&mut Table) -> R) -> Result<R, Errno> {
    if TABLE.lock().is_none() {
        if !crate::heap::may_grow_for_user(size_of::<Table>()) {
            return Err(Errno::ENOLCK);
        }
        let made = Box::new(Table::new());
        let mut slot = TABLE.lock();
        if slot.is_none() {
            *slot = Some(made);
        }
    }
    change(changing).ok_or(Errno::ENOLCK)
}

fn conflict(wanted: &Lock) -> Option<Lock> {
    TABLE
        .lock()
        .as_ref()
        .and_then(|table| table.conflict(wanted))
}

fn waiters() -> Arc<WakeList> {
    if let Some(list) = WAITERS.lock().as_ref() {
        return Arc::clone(list);
    }
    let made = WakeList::shared();
    Arc::clone(WAITERS.lock().get_or_insert(made))
}

fn wake_waiters() {
    let list = WAITERS.lock().as_ref().map(Arc::clone);
    if let Some(list) = list {
        list.wake();
    }
}

fn locked_file(file: &OpenFile) -> LockedFile {
    match file.kind() {
        FileKind::MemoryFile(memory) => LockedFile::Object(Arc::as_ptr(memory) as usize),
        kind => match kind.node() {
            Some(node) => LockedFile::Node(node),
            None => LockedFile::Object(file as *const OpenFile as usize),
        },
    }
}

fn description(file: &OpenFile) -> Owner {
    Owner::Description(file as *const OpenFile as usize)
}

/// Lets go of the locks the open file description `file` holds: called as it goes, once its
/// last descriptor is closed.
pub fn description_closed(file: &OpenFile) {
    if LOCKS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let owner = description(file);
    let object = LockedFile::Object(file as *const OpenFile as usize);
    let keep = |lock: &Lock| lock.owner != owner && lock.file != object;
    if change(|table| table.remove_unless(keep)) == Some(true) {
        wake_waiters();
    }
}

/// Lets go of the record locks process `pid` holds on what `file` names, as closing any of
/// its descriptors for the file does: `close`, `close_range`, `dup2` and `dup3` over a
/// descriptor, and `execve`'s close-on-exec.
pub fn descriptor_closed(pid: u32, file: &OpenFile) {
    if LOCKS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let locked = locked_file(file);
    let owner = Owner::Process(pid);
    let keep = |lock: &Lock| lock.owner != owner || lock.file != locked;
    if change(|table| table.remove_unless(keep)) == Some(true) {
        wake_waiters();
    }
}

/// Lets go of the record locks process `pid` held, as it ends.
pub fn process_ended(pid: u32) {
    if LOCKS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let owner = Owner::Process(pid);
    if change(|table| table.remove_unless(|lock| lock.owner != owner)) == Some(true) {
        wake_waiters();
    }
}

/// `close(fd)` by process `pid`: closes descriptor `fd` (`EBADF` if it is not open) and lets
/// go of the record locks `pid` holds on its file.
pub fn close(pid: u32, files: &Descriptors, fd: u64) -> Result<u64, Errno> {
    let closed = files.lock().remove(fd as u32).ok_or(Errno::EBADF)?;
    descriptor_closed(pid, &closed.file);
    drop(closed);
    Ok(0)
}

/// Whether `fcntl`'s `command` (an `int`) is one of the lock commands this module answers.
pub fn is_lock_command(command: u64) -> bool {
    matches!(
        command as u32,
        command::GETLK
            | command::SETLK
            | command::SETLKW
            | command::OFD_GETLK
            | command::OFD_SETLK
            | command::OFD_SETLKW
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Take { wanted: Lock, wait: bool },
    Let { lock: Lock },
}

/// `flock(fd, operation)`: see the module documentation. As Linux: `EBADF` if `fd` is not
/// open or is a location only (`O_PATH`), `EINVAL` for an operation that is not one of
/// `LOCK_SH`, `LOCK_EX` and `LOCK_UN` with or without `LOCK_NB`, `EWOULDBLOCK` (`EAGAIN`)
/// for a conflict with `LOCK_NB`, `EINTR` for a wait a signal ended, `ENOLCK` without memory.
/// Sets its result and delivers the signals due after it.
pub fn flock<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fd, operation, ..] = context.syscall_arguments();
    let request = flock_request(thread, fd, operation as u32);
    run(
        process,
        thread,
        context,
        request.map(|request| (request, None)),
    )
}

fn flock_request(thread: &LinuxThread, fd: u64, operation: u32) -> Result<Request, Errno> {
    let file = thread.files().file(fd)?;
    if file.is_location_only() {
        return Err(Errno::EBADF);
    }
    let sharing = match operation & !operation::NONBLOCK {
        operation::SHARED => Some(Sharing::Shared),
        operation::EXCLUSIVE => Some(Sharing::Exclusive),
        operation::UNLOCK => None,
        _ => return Err(Errno::EINVAL),
    };
    let lock = Lock {
        file: locked_file(&file),
        family: Family::Whole,
        owner: description(&file),
        sharing: sharing.unwrap_or(Sharing::Shared),
        start: 0,
        end: OFFSET_MAX,
    };
    if sharing.is_none() {
        return Ok(Request::Let { lock });
    }
    let other_kind = |held: &Lock| {
        held.is_held_by(lock.file, lock.family, lock.owner) && held.sharing != lock.sharing
    };
    if change(|table| table.remove_unless(|held| !other_kind(held))) == Some(true) {
        wake_waiters();
    }
    Ok(Request::Take {
        wanted: lock,
        wait: operation & operation::NONBLOCK == 0,
    })
}

/// `fcntl(fd, command, flock)` for a lock command ([`is_lock_command`]): see the module
/// documentation. As Linux: `EBADF` if `fd` is not open or is a location only; `EFAULT` for
/// `flock`; `EINVAL` for an unknown `l_type` or `l_whence`, `F_UNLCK` with `F_GETLK`, an
/// `l_pid` other than 0 with the open-file-description commands, or a range starting before
/// offset 0; `EOVERFLOW` for a range past the largest offset; `EBADF` for a read lock on a
/// file not open for reading or a write lock on one not open for writing; then `EAGAIN` for
/// a conflict with `F_SETLK`, `EINTR` for a wait a signal ended, `ENOLCK` without memory.
/// `F_GETLK` writes back the lock in the way (`l_pid` its process, -1 for a description's),
/// or `F_UNLCK` in `l_type`. Sets its result and delivers the signals due after it.
pub fn fcntl<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [fd, command, address, ..] = context.syscall_arguments();
    let request = record_request(process, thread, fd, command as u32, address);
    run(process, thread, context, request)
}

fn record_request<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    command: u32,
    address: u64,
) -> Result<(Request, Option<u64>), Errno> {
    let file = thread.files().file(fd)?;
    if file.is_location_only() {
        return Err(Errno::EBADF);
    }
    let mut bytes = [0u8; FLOCK_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let described = Described::read(&bytes);
    let descriptions = matches!(
        command,
        command::OFD_GETLK | command::OFD_SETLK | command::OFD_SETLKW
    );
    if descriptions && described.pid != 0 {
        return Err(Errno::EINVAL);
    }
    let sharing = match described.kind {
        lock_type::READ => Some(Sharing::Shared),
        lock_type::WRITE => Some(Sharing::Exclusive),
        lock_type::UNLOCK => None,
        _ => return Err(Errno::EINVAL),
    };
    let base = match described.whence {
        SEEK_SET => 0,
        SEEK_CUR => file.offset(),
        SEEK_END => file_size(&file),
        _ => return Err(Errno::EINVAL),
    };
    let (start, end) = range(base, described.start, described.length)?;
    let owner = match descriptions {
        true => description(&file),
        false => Owner::Process(process.id().get()),
    };
    let lock = Lock {
        file: locked_file(&file),
        family: Family::Record,
        owner,
        sharing: sharing.unwrap_or(Sharing::Shared),
        start,
        end,
    };
    match command {
        command::GETLK | command::OFD_GETLK => match sharing {
            None => Err(Errno::EINVAL),
            Some(_) => Ok((
                Request::Take {
                    wanted: lock,
                    wait: false,
                },
                Some(address),
            )),
        },
        _ => {
            let allowed = match sharing {
                None => true,
                Some(Sharing::Shared) => file.is_readable(),
                Some(Sharing::Exclusive) => file.is_writable(),
            };
            if !allowed {
                return Err(Errno::EBADF);
            }
            let wait = matches!(command, command::SETLKW | command::OFD_SETLKW);
            Ok((
                match sharing {
                    None => Request::Let { lock },
                    Some(_) => Request::Take { wanted: lock, wait },
                },
                None,
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Described {
    kind: i16,
    whence: i16,
    start: i64,
    length: i64,
    pid: i32,
}

impl Described {
    fn read(bytes: &[u8; FLOCK_SIZE]) -> Self {
        let half = |at: usize| i16::from_ne_bytes([bytes[at], bytes[at + 1]]);
        let word = |at: usize| {
            let mut value = [0u8; 8];
            value.copy_from_slice(&bytes[at..at + 8]);
            i64::from_ne_bytes(value)
        };
        let mut pid = [0u8; 4];
        pid.copy_from_slice(&bytes[24..28]);
        Self {
            kind: half(0),
            whence: half(2),
            start: word(8),
            length: word(16),
            pid: i32::from_ne_bytes(pid),
        }
    }

    fn write(self, bytes: &mut [u8; FLOCK_SIZE]) {
        bytes[0..2].copy_from_slice(&self.kind.to_ne_bytes());
        bytes[2..4].copy_from_slice(&self.whence.to_ne_bytes());
        bytes[8..16].copy_from_slice(&self.start.to_ne_bytes());
        bytes[16..24].copy_from_slice(&self.length.to_ne_bytes());
        bytes[24..28].copy_from_slice(&self.pid.to_ne_bytes());
    }

    fn of(lock: &Lock) -> Self {
        Self {
            kind: match lock.sharing {
                Sharing::Shared => lock_type::READ,
                Sharing::Exclusive => lock_type::WRITE,
            },
            whence: SEEK_SET,
            start: lock.start as i64,
            length: match lock.end {
                OFFSET_MAX => 0,
                end => (end - lock.start + 1) as i64,
            },
            pid: match lock.owner {
                Owner::Process(pid) => pid as i32,
                Owner::Description(_) => -1,
            },
        }
    }
}

fn file_size(file: &OpenFile) -> u64 {
    match file.kind() {
        FileKind::MemoryFile(memory) => memory.size(),
        kind => kind.node().map_or(0, |node| node.attributes().size),
    }
}

/// The first and last byte of a range of `length` bytes from `base + start`, as Linux reads
/// `struct flock`: a length of 0 runs to the end of the largest file, a negative one ends
/// just before the start.
pub fn range(base: u64, start: i64, length: i64) -> Result<(u64, u64), Errno> {
    let base = i64::try_from(base).map_err(|_| Errno::EOVERFLOW)?;
    let first = base.checked_add(start).ok_or(Errno::EOVERFLOW)?;
    let (first, last) = match length {
        0 => (first, i64::MAX),
        length if length > 0 => (
            first,
            first.checked_add(length - 1).ok_or(Errno::EOVERFLOW)?,
        ),
        length => (
            first.checked_add(length).ok_or(Errno::EINVAL)?,
            first.checked_sub(1).ok_or(Errno::EINVAL)?,
        ),
    };
    if first < 0 {
        return Err(Errno::EINVAL);
    }
    Ok((first as u64, last as u64))
}

fn run<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    request: Result<(Request, Option<u64>), Errno>,
) -> ControlFlow<()> {
    let interrupted = Interrupted::RestartsIfAsked {
        number: context.syscall_number(),
        first_argument: context.syscall_arguments()[0],
    };
    let mut waited = false;
    let result = match request {
        Err(error) => Err(error),
        Ok((Request::Let { lock }, _)) => {
            let unlocked = change(|table| {
                table.unlock(lock.file, lock.family, lock.owner, lock.start, lock.end)
            })
            .unwrap_or(Ok(false));
            if unlocked == Ok(true) {
                wake_waiters();
            }
            unlocked.map(|_| 0)
        }
        Ok((Request::Take { wanted, .. }, Some(answer))) => {
            answer_conflict(process, answer, conflict(&wanted))
        }
        Ok((Request::Take { wanted, wait }, None)) => loop {
            let thread_id = thread.id();
            let followed = wait
                && wanted.family == Family::Record
                && matches!(wanted.owner, Owner::Process(_));
            let taken = change_made(|table| match table.conflict(&wanted) {
                Some(in_the_way) if followed && table.would_deadlock(&wanted, &in_the_way) => {
                    Err(Errno::EDEADLK)
                }
                Some(_) if followed => table.wait(thread_id, wanted).map(|()| false),
                Some(_) => Ok(false),
                None => table.set(wanted).map(|()| true),
            })
            .and_then(|taken| taken);
            match taken {
                Ok(true) => break Ok(0),
                Err(error) => break Err(error),
                Ok(false) if !wait => break Err(Errno::EAGAIN),
                Ok(false) => {}
            }
            waited = true;
            let list = waiters();
            let lists = [&list];
            let free = || conflict(&wanted).is_none();
            let woken = poll::wait_for(process, thread, &lists, u64::MAX, free);
            if followed {
                change(|table| table.stop_waiting(thread_id));
            }
            if woken? == Waited::Signalled
                && signals::deliver_interrupting(process, thread, context, interrupted)?
            {
                return ControlFlow::Continue(());
            }
        },
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    if !waited {
        return ControlFlow::Continue(());
    }
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn answer_conflict<P: Platform>(
    process: &Process<P>,
    answer: u64,
    found: Option<Lock>,
) -> Result<u64, Errno> {
    let mut bytes = [0u8; FLOCK_SIZE];
    process
        .read_user_memory(answer, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    match found {
        Some(lock) => Described::of(&lock).write(&mut bytes),
        None => bytes[0..2].copy_from_slice(&lock_type::UNLOCK.to_ne_bytes()),
    }
    process
        .write_user_memory(answer, &bytes)
        .map(|()| 0)
        .map_err(|_| Errno::EFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: LockedFile = LockedFile::Object(0x1000);
    const OTHER_FILE: LockedFile = LockedFile::Object(0x2000);
    const ONE: Owner = Owner::Process(7);
    const TWO: Owner = Owner::Process(8);

    fn record(owner: Owner, sharing: Sharing, start: u64, end: u64) -> Lock {
        Lock {
            file: FILE,
            family: Family::Record,
            owner,
            sharing,
            start,
            end,
        }
    }

    fn held(table: &Table, owner: Owner) -> Vec<(Sharing, u64, u64)> {
        let mut locks: Vec<_> = table
            .locks()
            .iter()
            .filter(|lock| lock.owner == owner)
            .map(|lock| (lock.sharing, lock.start, lock.end))
            .collect();
        locks.sort_by_key(|&(_, start, _)| start);
        locks
    }

    #[test]
    fn shared_locks_coexist_and_an_exclusive_one_conflicts_with_any_other_owner_s() {
        let mut table = Table::new();
        table.set(record(ONE, Sharing::Shared, 0, 99)).unwrap();
        assert_eq!(table.conflict(&record(TWO, Sharing::Shared, 50, 60)), None);
        let found = table.conflict(&record(TWO, Sharing::Exclusive, 99, 120));
        assert_eq!(found, Some(record(ONE, Sharing::Shared, 0, 99)));
        assert_eq!(
            table.conflict(&record(TWO, Sharing::Exclusive, 100, 120)),
            None
        );
        assert_eq!(
            table.conflict(&record(ONE, Sharing::Exclusive, 0, 99)),
            None
        );
        let elsewhere = Lock {
            file: OTHER_FILE,
            ..record(TWO, Sharing::Exclusive, 0, OFFSET_MAX)
        };
        assert_eq!(table.conflict(&elsewhere), None);
        let whole = Lock {
            family: Family::Whole,
            ..record(TWO, Sharing::Exclusive, 0, OFFSET_MAX)
        };
        assert_eq!(
            table.conflict(&whole),
            None,
            "flock and record locks never meet"
        );
    }

    #[test]
    fn a_lock_inside_another_of_the_same_owner_splits_it_and_unlocking_cuts_it() {
        let mut table = Table::new();
        table.set(record(ONE, Sharing::Shared, 0, 99)).unwrap();
        table.set(record(ONE, Sharing::Exclusive, 40, 59)).unwrap();
        assert_eq!(
            held(&table, ONE),
            [
                (Sharing::Shared, 0, 39),
                (Sharing::Exclusive, 40, 59),
                (Sharing::Shared, 60, 99)
            ]
        );
        assert_eq!(table.unlock(FILE, Family::Record, ONE, 30, 69), Ok(true));
        assert_eq!(
            held(&table, ONE),
            [(Sharing::Shared, 0, 29), (Sharing::Shared, 70, 99)]
        );
        assert_eq!(table.unlock(FILE, Family::Record, TWO, 0, 99), Ok(false));
        assert_eq!(
            table.unlock(FILE, Family::Record, ONE, 0, OFFSET_MAX),
            Ok(true)
        );
        assert!(table.locks().is_empty());
    }

    #[test]
    fn touching_locks_of_the_same_kind_merge() {
        let mut table = Table::new();
        table.set(record(ONE, Sharing::Shared, 0, 9)).unwrap();
        table.set(record(ONE, Sharing::Shared, 20, 29)).unwrap();
        table.set(record(ONE, Sharing::Shared, 10, 19)).unwrap();
        assert_eq!(held(&table, ONE), [(Sharing::Shared, 0, 29)]);
        table
            .set(record(ONE, Sharing::Exclusive, 30, OFFSET_MAX))
            .unwrap();
        assert_eq!(
            held(&table, ONE),
            [
                (Sharing::Shared, 0, 29),
                (Sharing::Exclusive, 30, OFFSET_MAX)
            ]
        );
    }

    #[test]
    fn an_owner_s_locks_go_together() {
        let mut table = Table::new();
        table.set(record(ONE, Sharing::Shared, 0, 9)).unwrap();
        table.set(record(TWO, Sharing::Shared, 0, 9)).unwrap();
        assert!(table.remove_unless(|lock| lock.owner != ONE));
        assert!(held(&table, ONE).is_empty());
        assert_eq!(held(&table, TWO), [(Sharing::Shared, 0, 9)]);
        assert!(!table.remove_unless(|lock| lock.owner != ONE));
    }

    #[test]
    fn a_wait_that_closes_a_cycle_of_waiting_processes_is_a_deadlock() {
        const THREE: Owner = Owner::Process(9);
        let mut table = Table::new();
        table.set(record(ONE, Sharing::Exclusive, 0, 9)).unwrap();
        table.set(record(TWO, Sharing::Exclusive, 10, 19)).unwrap();
        table
            .set(record(THREE, Sharing::Exclusive, 20, 29))
            .unwrap();
        let one_wants = record(ONE, Sharing::Exclusive, 10, 10);
        let in_the_way = table.conflict(&one_wants).unwrap();
        assert!(!table.would_deadlock(&one_wants, &in_the_way));
        table.wait(70, one_wants).unwrap();
        let two_wants = record(TWO, Sharing::Shared, 20, 20);
        let in_the_way = table.conflict(&two_wants).unwrap();
        assert!(!table.would_deadlock(&two_wants, &in_the_way));
        table.wait(80, two_wants).unwrap();
        let three_wants = record(THREE, Sharing::Exclusive, 5, 5);
        let in_the_way = table.conflict(&three_wants).unwrap();
        assert!(
            table.would_deadlock(&three_wants, &in_the_way),
            "three waits for one, who waits for two, who waits for three"
        );
        table.stop_waiting(80);
        assert_eq!(table.waiters().len(), 1);
        assert!(!table.would_deadlock(&three_wants, &in_the_way));
    }

    #[test]
    fn description_locks_are_never_followed_for_deadlocks() {
        let mut table = Table::new();
        let description = Owner::Description(0x40);
        table.set(record(ONE, Sharing::Exclusive, 0, 9)).unwrap();
        table
            .set(record(description, Sharing::Exclusive, 10, 19))
            .unwrap();
        table
            .wait(70, record(ONE, Sharing::Exclusive, 10, 10))
            .unwrap();
        let wanted = record(description, Sharing::Exclusive, 0, 0);
        let in_the_way = table.conflict(&wanted).unwrap();
        assert!(!table.would_deadlock(&wanted, &in_the_way));
        let whole = Lock {
            family: Family::Whole,
            ..record(TWO, Sharing::Exclusive, 0, OFFSET_MAX)
        };
        assert!(!table.would_deadlock(&whole, &whole));
    }

    #[test]
    fn ranges_are_read_as_linux_reads_struct_flock() {
        assert_eq!(range(0, 0, 0), Ok((0, OFFSET_MAX)));
        assert_eq!(range(100, 10, 5), Ok((110, 114)));
        assert_eq!(range(100, 0, -10), Ok((90, 99)));
        assert_eq!(range(5, 0, -10), Err(Errno::EINVAL));
        assert_eq!(range(0, -1, 1), Err(Errno::EINVAL));
        assert_eq!(range(0, i64::MAX, 2), Err(Errno::EOVERFLOW));
        assert_eq!(range(u64::MAX, 0, 1), Err(Errno::EOVERFLOW));
    }

    #[test]
    fn struct_flock_round_trips_and_describes_the_lock_in_the_way() {
        let mut bytes = [0u8; FLOCK_SIZE];
        let described = Described {
            kind: lock_type::WRITE,
            whence: SEEK_END,
            start: -4,
            length: 4,
            pid: 0,
        };
        described.write(&mut bytes);
        assert_eq!(Described::read(&bytes), described);
        let found = Described::of(&record(ONE, Sharing::Shared, 10, OFFSET_MAX));
        assert_eq!(
            (
                found.kind,
                found.whence,
                found.start,
                found.length,
                found.pid
            ),
            (lock_type::READ, SEEK_SET, 10, 0, 7)
        );
        let description = Lock {
            owner: Owner::Description(0x40),
            ..record(ONE, Sharing::Exclusive, 3, 4)
        };
        let found = Described::of(&description);
        assert_eq!((found.length, found.pid), (2, -1));
    }

    #[test]
    fn only_the_lock_commands_are_this_module_s() {
        for command in [5, 6, 7, 36, 37, 38] {
            assert!(is_lock_command(1 << 32 | command));
        }
        for command in [0, 1, 2, 3, 4, 8, 1030, 1033] {
            assert!(!is_lock_command(command));
        }
    }
}
