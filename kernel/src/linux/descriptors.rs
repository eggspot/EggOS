//! File descriptors (D101): each process's descriptor table, which its threads share, and the
//! calls that change it: `close`, `dup`, `dup2` (x86_64 only), `dup3`, `fcntl`'s descriptor
//! and status-flag commands, `close_range`, and the `ioctl`s every descriptor answers
//! (`FIOCLEX`, `FIONCLEX`, `FIONBIO`).
//!
//! A descriptor ([`Descriptor`]) names an open file description ([`OpenFile`]): what was
//! opened ([`FileKind`]), its access mode, status flags and offset, which every descriptor
//! made from it with `dup` shares, as POSIX requires; the close-on-exec flag is the
//! descriptor's own. A process starts with descriptors 0, 1 and 2 on one description of the
//! console, open for reading and writing, as Linux starts `init` with `/dev/console` opened
//! once and duplicated: `O_NONBLOCK` set through one of them holds for all three. A new
//! descriptor takes the lowest free number, below the process's `RLIMIT_NOFILE` soft limit.
//!
//! The table ([`DescriptorTable`]) sits behind a leaf lock in an `Arc` that each thread holds
//! ([`Descriptors`], in its `LinuxThread`), as Linux's tasks point to a files table: a thread
//! started with `CLONE_FILES` shares it, one started without gets a copy, as `fork` will. The
//! lock is held only to look a descriptor up or change the table: never while user memory is
//! copied or a thread waits, and no open file description is dropped under it.

use super::pipe::PipeEnd;
use super::{Errno, limits};
use crate::process::Process;
use crate::sync::{LeafSpinLock, LeafSpinLockGuard};
use crate::vfs::Node;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use eggos_hal::Platform;

/// The flags of an open file description (`O_*`), the generic values, which AArch64, RISC-V
/// and x86_64 share.
pub mod open_flag {
    /// The bits that hold the access mode.
    pub const ACCMODE: u32 = 0o3;
    /// Open for reading only.
    pub const RDONLY: u32 = 0o0;
    /// Open for writing only.
    pub const WRONLY: u32 = 0o1;
    /// Open for reading and writing.
    pub const RDWR: u32 = 0o2;
    /// Every write goes to the end of the file.
    pub const APPEND: u32 = 0o2000;
    /// Calls that would wait fail with `EAGAIN` instead.
    pub const NONBLOCK: u32 = 0o4000;
    /// The new descriptor is closed by `execve` (`dup3`, `open`).
    pub const CLOEXEC: u32 = 0o2_000_000;
    /// Create the file if it does not exist.
    pub const CREAT: u32 = 0o100;
    /// With [`CREAT`], fail if the file exists.
    pub const EXCL: u32 = 0o200;
    /// Truncate a regular file opened for writing.
    pub const TRUNC: u32 = 0o1000;
    /// Fail unless the path names a directory.
    #[cfg(target_arch = "aarch64")]
    pub const DIRECTORY: u32 = 0o40_000;
    /// Fail unless the path names a directory.
    #[cfg(not(target_arch = "aarch64"))]
    pub const DIRECTORY: u32 = 0o200_000;
    /// Do not follow a symbolic link in the last component.
    #[cfg(target_arch = "aarch64")]
    pub const NOFOLLOW: u32 = 0o100_000;
    /// Do not follow a symbolic link in the last component.
    #[cfg(not(target_arch = "aarch64"))]
    pub const NOFOLLOW: u32 = 0o400_000;
    /// Open only a location in the file system: no reads or writes.
    pub const PATH: u32 = 0o10_000_000;
    /// An unnamed temporary file in the directory named: its own bit and [`DIRECTORY`].
    pub const TMPFILE: u32 = 0o20_000_000 | DIRECTORY;
    /// The status flags kept and changed (`F_SETFL`): the others Linux lets `F_SETFL` change
    /// (`O_ASYNC`, `O_DIRECT`, `O_NOATIME`) are ignored until something acts on them.
    pub const STATUS: u32 = APPEND | NONBLOCK;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's O_* values for this architecture (AArch64's O_DIRECT differs)");
}

/// `fcntl`'s commands answered (`F_*`), the generic values, which x86_64 shares.
pub mod command {
    /// Duplicate to the lowest free descriptor at least the argument.
    pub const DUPFD: u32 = 0;
    /// Read the descriptor's flags.
    pub const GETFD: u32 = 1;
    /// Set the descriptor's flags.
    pub const SETFD: u32 = 2;
    /// Read the access mode and status flags.
    pub const GETFL: u32 = 3;
    /// Set the status flags.
    pub const SETFL: u32 = 4;
    /// [`DUPFD`], the new descriptor closed on `execve`.
    pub const DUPFD_CLOEXEC: u32 = 1030;
}

/// A descriptor's own flag (`FD_CLOEXEC`): closed by `execve`.
pub const FD_CLOEXEC: u32 = 1;

/// `close_range`'s flags (`CLOSE_RANGE_*`).
pub mod close_range_flag {
    /// Give the caller a table of its own first.
    pub const UNSHARE: u32 = 1 << 1;
    /// Mark the descriptors close-on-exec instead of closing them.
    pub const CLOEXEC: u32 = 1 << 2;
}

/// The `ioctl`s every descriptor answers, whatever it names, the generic numbers, which x86_64
/// shares.
pub mod request {
    /// Set or clear `O_NONBLOCK` (an `int`, set if not 0).
    pub const FIONBIO: u32 = 0x5421;
    /// Clear the descriptor's close-on-exec flag.
    pub const FIONCLEX: u32 = 0x5450;
    /// Set the descriptor's close-on-exec flag.
    pub const FIOCLEX: u32 = 0x5451;
    /// Set or clear `O_ASYNC` (an `int`): read and ignored, as `F_SETFL` ignores `O_ASYNC`.
    pub const FIOASYNC: u32 = 0x5452;
}

/// What an open file description names: the console, a node of the file system (D102), an
/// end of a pipe (D125), a client of the window server (D127), or the agent channel (D136).
#[derive(Debug, PartialEq, Eq)]
pub enum FileKind {
    /// The console's terminal (D100), Linux's `/dev/console`.
    Console,
    /// A regular file.
    Regular(Node),
    /// A directory, read with `getdents64`.
    Directory(Node),
    /// A location only (`O_PATH`): its status, a start for relative paths, `fchdir`.
    Path(Node),
    /// One end of a pipe.
    Pipe(PipeEnd),
    /// A regular file whose contents were made when it was opened (`/proc`, D125).
    Snapshot(Snapshot),
    /// A client of the window server (`/dev/window`, D127).
    Window(crate::window::Connection),
    /// The agent channel's one opener (`/dev/agent`, D136).
    AgentChannel(super::agent_channel::AgentChannelEnd),
}

/// A regular file's contents as they were made when it was opened ([`crate::vfs::Node::generate`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The file.
    pub node: Node,
    /// Its contents.
    pub bytes: Vec<u8>,
}

impl FileKind {
    /// The node it names, if it names one.
    pub fn node(&self) -> Option<Node> {
        match self {
            Self::Console | Self::Pipe(_) | Self::Window(_) | Self::AgentChannel(_) => None,
            Self::Regular(node) | Self::Directory(node) | Self::Path(node) => Some(*node),
            Self::Snapshot(snapshot) => Some(snapshot.node),
        }
    }
}

/// An open file description: what was opened, how, and where reads and writes go next.
/// Shared by every descriptor duplicated from the one that opened it.
#[derive(Debug)]
pub struct OpenFile {
    kind: FileKind,
    status: AtomicU32,
    offset: AtomicU64,
}

impl OpenFile {
    /// A description of `kind` opened with `flags`, of which it keeps the access mode and the
    /// status flags ([`open_flag::STATUS`]), at offset 0.
    pub fn new(kind: FileKind, flags: u32) -> Self {
        Self {
            kind,
            status: AtomicU32::new(
                flags & (open_flag::ACCMODE | open_flag::STATUS | open_flag::PATH),
            ),
            offset: AtomicU64::new(0),
        }
    }

    /// What it names.
    pub fn kind(&self) -> &FileKind {
        &self.kind
    }

    /// Whether it was opened with `O_PATH`: a location only, which `ioctl` and most `fcntl`
    /// commands refuse with `EBADF`.
    pub fn is_location_only(&self) -> bool {
        matches!(self.kind, FileKind::Path(_))
    }

    /// Its access mode and status flags, as `F_GETFL` reports them (`O_PATH` for a location).
    pub fn status_flags(&self) -> u32 {
        self.status.load(Ordering::Relaxed)
    }

    /// Replaces its status flags with those of `flags` it keeps ([`open_flag::STATUS`]), as
    /// `F_SETFL` does; the access mode stays.
    pub fn set_status_flags(&self, flags: u32) {
        let access = self.status_flags() & open_flag::ACCMODE;
        self.status
            .store(access | flags & open_flag::STATUS, Ordering::Relaxed);
    }

    /// Sets or clears `O_NONBLOCK` (`FIONBIO`).
    pub fn set_nonblocking(&self, nonblocking: bool) {
        match nonblocking {
            true => self.status.fetch_or(open_flag::NONBLOCK, Ordering::Relaxed),
            false => self
                .status
                .fetch_and(!open_flag::NONBLOCK, Ordering::Relaxed),
        };
    }

    /// Whether calls on it that would wait fail with `EAGAIN` instead (`O_NONBLOCK`).
    pub fn is_nonblocking(&self) -> bool {
        self.status_flags() & open_flag::NONBLOCK != 0
    }

    /// Whether it was opened for reading.
    pub fn is_readable(&self) -> bool {
        matches!(
            self.status_flags() & open_flag::ACCMODE,
            open_flag::RDONLY | open_flag::RDWR
        )
    }

    /// Whether it was opened for writing.
    pub fn is_writable(&self) -> bool {
        matches!(
            self.status_flags() & open_flag::ACCMODE,
            open_flag::WRONLY | open_flag::RDWR
        )
    }

    /// Where the next read or write goes, for a file that has offsets.
    pub fn offset(&self) -> u64 {
        self.offset.load(Ordering::Relaxed)
    }

    /// Moves where the next read or write goes.
    pub fn set_offset(&self, offset: u64) {
        self.offset.store(offset, Ordering::Relaxed);
    }

    /// Moves the offset from `from` to `to`, unless another call moved it since it was
    /// `from`: then it returns `false` and changes nothing.
    pub fn move_offset(&self, from: u64, to: u64) -> bool {
        self.offset
            .compare_exchange(from, to, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

/// An open descriptor: the open file description it names, and its own close-on-exec flag.
#[derive(Debug, Clone)]
pub struct Descriptor {
    pub file: Arc<OpenFile>,
    pub close_on_exec: bool,
}

const SMALLEST_TABLE: usize = 8;

/// A process's descriptors, by number. See the module documentation.
#[derive(Debug, Default)]
pub struct DescriptorTable {
    slots: Vec<Option<Descriptor>>,
    first_maybe_free: usize,
}

impl DescriptorTable {
    /// A table with no descriptor open.
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            first_maybe_free: 0,
        }
    }

    /// A table with descriptors 0, 1 and 2 open on `file`.
    pub fn with_standard_streams(file: Arc<OpenFile>) -> Self {
        let mut slots = Vec::with_capacity(SMALLEST_TABLE);
        slots.resize_with(3, || {
            Some(Descriptor {
                file: Arc::clone(&file),
                close_on_exec: false,
            })
        });
        Self {
            slots,
            first_maybe_free: 3,
        }
    }

    /// Descriptor `fd`, if it is open.
    pub fn get(&self, fd: u32) -> Option<&Descriptor> {
        self.slots.get(fd as usize)?.as_ref()
    }

    /// Descriptor `fd`, if it is open, to change its flag.
    pub fn get_mut(&mut self, fd: u32) -> Option<&mut Descriptor> {
        self.slots.get_mut(fd as usize)?.as_mut()
    }

    /// Opens the lowest free descriptor at least `lowest` and below `limit` on what `old`
    /// names, with `close_on_exec`, and returns its number (`dup`, `F_DUPFD`). `EBADF` if
    /// `old` is not open, `EMFILE` if every number up to `limit` is taken, `ENOMEM` without
    /// memory for the table to grow.
    pub fn duplicate(
        &mut self,
        old: u32,
        lowest: u32,
        close_on_exec: bool,
        limit: u64,
    ) -> Result<u32, Errno> {
        let file = Arc::clone(&self.get(old).ok_or(Errno::EBADF)?.file);
        self.open_at_least(file, lowest, close_on_exec, limit)
    }

    /// Opens the lowest free descriptor below `limit` on `file`, with `close_on_exec`, and
    /// returns its number (`open`). `EMFILE` if every number up to `limit` is taken, `ENOMEM`
    /// without memory for the table to grow.
    pub fn open(
        &mut self,
        file: Arc<OpenFile>,
        close_on_exec: bool,
        limit: u64,
    ) -> Result<u32, Errno> {
        self.open_at_least(file, 0, close_on_exec, limit)
    }

    fn open_at_least(
        &mut self,
        file: Arc<OpenFile>,
        lowest: u32,
        close_on_exec: bool,
        limit: u64,
    ) -> Result<u32, Errno> {
        let start = (lowest as usize).max(self.first_maybe_free);
        let free = self
            .slots
            .get(start..)
            .and_then(|slots| slots.iter().position(Option::is_none))
            .map_or(self.slots.len().max(start), |index| start + index);
        if free as u64 >= limit {
            return Err(Errno::EMFILE);
        }
        let replaced = self.place(
            free,
            Descriptor {
                file,
                close_on_exec,
            },
            limit,
        )?;
        debug_assert!(replaced.is_none());
        if lowest as usize <= self.first_maybe_free {
            self.first_maybe_free = free + 1;
        }
        Ok(free as u32)
    }

    /// Makes descriptor `new`, below `limit`, name what `old` names, with `close_on_exec`,
    /// closing what `new` named, which it returns for the caller to drop once the table's
    /// lock is released (`dup2`, `dup3`). `EBADF` if `old` is not open, `ENOMEM` without
    /// memory for the table to grow.
    pub fn duplicate_to(
        &mut self,
        old: u32,
        new: u32,
        close_on_exec: bool,
        limit: u64,
    ) -> Result<Option<Descriptor>, Errno> {
        let file = Arc::clone(&self.get(old).ok_or(Errno::EBADF)?.file);
        self.place(
            new as usize,
            Descriptor {
                file,
                close_on_exec,
            },
            limit,
        )
    }

    fn place(
        &mut self,
        fd: usize,
        descriptor: Descriptor,
        limit: u64,
    ) -> Result<Option<Descriptor>, Errno> {
        if fd >= self.slots.len() {
            let wanted = (fd + 1)
                .max(2 * self.slots.len())
                .max(SMALLEST_TABLE)
                .min(usize::try_from(limit).unwrap_or(usize::MAX).max(fd + 1));
            if wanted > self.slots.capacity() {
                self.slots
                    .try_reserve_exact(wanted - self.slots.len())
                    .map_err(|_| Errno::ENOMEM)?;
            }
            self.slots.resize_with(fd + 1, || None);
        }
        Ok(self.slots[fd].replace(descriptor))
    }

    /// Closes descriptor `fd` and returns it, for the caller to drop once the table's lock is
    /// released; `None` if it was not open.
    pub fn remove(&mut self, fd: u32) -> Option<Descriptor> {
        let removed = self.slots.get_mut(fd as usize)?.take()?;
        self.first_maybe_free = self.first_maybe_free.min(fd as usize);
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        Some(removed)
    }

    /// Closes the lowest open descriptor from `first` to `last` and returns it with its number,
    /// for the caller to drop once the table's lock is released (`close_range`).
    pub fn remove_first_in(&mut self, first: u32, last: u32) -> Option<(u32, Descriptor)> {
        let fd = self.first_open_in(first, last, |_| true)?;
        Some((fd, self.remove(fd)?))
    }

    /// Closes the lowest descriptor from `first` up marked close-on-exec and returns it with its
    /// number, for the caller to drop once the table's lock is released (`execve`).
    pub fn remove_first_closed_on_exec(&mut self, first: u32) -> Option<(u32, Descriptor)> {
        let fd = self.first_open_in(first, u32::MAX, |descriptor| descriptor.close_on_exec)?;
        Some((fd, self.remove(fd)?))
    }

    /// Marks every open descriptor from `first` to `last` close-on-exec
    /// (`CLOSE_RANGE_CLOEXEC`).
    pub fn mark_close_on_exec(&mut self, first: u32, last: u32) {
        let end = self.slots.len().min(last as usize + 1);
        for descriptor in self
            .slots
            .get_mut(first as usize..end)
            .into_iter()
            .flatten()
            .flatten()
        {
            descriptor.close_on_exec = true;
        }
    }

    fn first_open_in(
        &self,
        first: u32,
        last: u32,
        wanted: impl Fn(&Descriptor) -> bool,
    ) -> Option<u32> {
        let end = self.slots.len().min(last as usize + 1);
        let slots = self.slots.get(first as usize..end)?;
        let index = slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(&wanted))?;
        Some(first + index as u32)
    }

    /// A copy: the same descriptors, with their flags, naming the same open file descriptions
    /// (`clone` without `CLONE_FILES`, `fork`). `ENOMEM` without memory for it.
    pub fn try_clone(&self) -> Result<Self, Errno> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(self.slots.len().max(SMALLEST_TABLE))
            .map_err(|_| Errno::ENOMEM)?;
        slots.extend(self.slots.iter().cloned());
        Ok(Self {
            slots,
            first_maybe_free: self.first_maybe_free,
        })
    }
}

/// A thread's descriptor table, which the threads that share it each hold. See the module
/// documentation.
#[derive(Debug)]
pub struct Descriptors(Arc<LeafSpinLock<DescriptorTable>>);

impl Descriptors {
    /// A new process's table: descriptors 0, 1 and 2 open on one description of the console,
    /// for reading and writing.
    pub fn with_console_streams() -> Self {
        let console = Arc::new(OpenFile::new(FileKind::Console, open_flag::RDWR));
        Self::of(DescriptorTable::with_standard_streams(console))
    }

    /// A handle on `table`, which nobody shares yet.
    pub fn of(table: DescriptorTable) -> Self {
        Self(Arc::new(LeafSpinLock::new_leaf(table)))
    }

    /// The same table, for a thread that shares it (`CLONE_FILES`).
    pub fn share(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// A table of its own, a copy of this one ([`DescriptorTable::try_clone`]).
    pub fn copy(&self) -> Result<Self, Errno> {
        let table = self.0.lock().try_clone()?;
        Ok(Self::of(table))
    }

    /// Whether another thread shares the table.
    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }

    /// The open file description descriptor `fd` names, `EBADF` if it is not open. `fd` is an
    /// `unsigned int`: Linux ignores the register's upper half.
    #[inline]
    pub fn file(&self, fd: u64) -> Result<Arc<OpenFile>, Errno> {
        let table = self.0.lock();
        let descriptor = table.get(fd as u32).ok_or(Errno::EBADF)?;
        Ok(Arc::clone(&descriptor.file))
    }

    /// The table, locked. Never held while user memory is copied, a thread waits, or an
    /// open file description is dropped.
    pub fn lock(&self) -> LeafSpinLockGuard<'_, DescriptorTable> {
        self.0.lock()
    }
}

/// The open-file limit of `process`: its `RLIMIT_NOFILE` soft limit, one more than the highest
/// descriptor it may open.
pub fn open_file_limit<P: Platform>(process: &Process<P>) -> u64 {
    process
        .credentials_and_limits()
        .lock()
        .limits
        .get(limits::resource::NOFILE)
        .map_or(limits::MAX_OPEN_FILES, |limit| limit.soft)
        .min(limits::MAX_OPEN_FILES)
}

/// `close(fd)`: closes descriptor `fd`, `EBADF` if it is not open.
pub fn close(files: &Descriptors, fd: u64) -> Result<u64, Errno> {
    let closed = files.lock().remove(fd as u32);
    closed.map(|_| 0).ok_or(Errno::EBADF)
}

/// `dup(old)`: opens the lowest free descriptor on what `old` names, without close-on-exec,
/// and returns it. As Linux: `EBADF` if `old` is not open, `EMFILE` if every descriptor below
/// the open-file limit is taken.
pub fn dup<P: Platform>(process: &Process<P>, files: &Descriptors, old: u64) -> Result<u64, Errno> {
    let limit = open_file_limit(process);
    let fd = files.lock().duplicate(old as u32, 0, false, limit)?;
    Ok(u64::from(fd))
}

/// `dup3(old, new, flags)`: makes descriptor `new` name what `old` names, closing what it
/// named first, close-on-exec if `flags` is `O_CLOEXEC`, and returns `new`. As Linux, in this
/// order: `EINVAL` for any other flag, and when `old` is `new`; `EBADF` if `new` is not below
/// the open-file limit or `old` is not open. All three arguments are 32-bit.
pub fn dup3<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    old: u64,
    new: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !open_flag::CLOEXEC != 0 || old as u32 == new as u32 {
        return Err(Errno::EINVAL);
    }
    duplicate_to(process, files, old as u32, new as u32, flags != 0)
}

/// `dup2(old, new)`: as [`dup3`] without flags, but `old` and `new` may be the same: then it
/// returns `new` if `old` is open, `EBADF` otherwise. x86_64 only: AArch64 and RISC-V have only
/// `dup3`, which their C libraries' `dup2` calls.
#[cfg(target_arch = "x86_64")]
pub fn dup2<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    old: u64,
    new: u64,
) -> Result<u64, Errno> {
    let (old, new) = (old as u32, new as u32);
    if old == new {
        let open = files.lock().get(old).is_some();
        return open.then_some(u64::from(new)).ok_or(Errno::EBADF);
    }
    duplicate_to(process, files, old, new, false)
}

fn duplicate_to<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    old: u32,
    new: u32,
    close_on_exec: bool,
) -> Result<u64, Errno> {
    let limit = open_file_limit(process);
    if u64::from(new) >= limit {
        return Err(Errno::EBADF);
    }
    let replaced = files.lock().duplicate_to(old, new, close_on_exec, limit)?;
    drop(replaced);
    Ok(u64::from(new))
}

/// `fcntl(fd, command, argument)`. As Linux: `EBADF` if `fd` is not open, whatever the
/// command; then:
/// - `F_DUPFD` and `F_DUPFD_CLOEXEC`: as `dup`, to the lowest free descriptor at least
///   `argument`, close-on-exec with `F_DUPFD_CLOEXEC`; `EINVAL` if `argument` is not below the
///   open-file limit (a negative one never is), `EMFILE` if every descriptor from it up to the
///   limit is taken;
/// - `F_GETFD` and `F_SETFD`: the descriptor's flags, `FD_CLOEXEC` the only one (other bits
///   set are ignored);
/// - `F_GETFL` and `F_SETFL`: the open file description's access mode and status flags, of
///   which `F_SETFL` changes `O_APPEND` and `O_NONBLOCK` and ignores the rest;
/// - any other command: `EINVAL`.
///
/// `fd`, `command` and `argument` are 32-bit: the commands answered take an `int`, and C
/// libraries pass it in a register whose upper half is undefined.
pub fn fcntl<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    fd: u64,
    command: u64,
    argument: u64,
) -> Result<u64, Errno> {
    let fd = fd as u32;
    let open = |table: &DescriptorTable| table.get(fd).ok_or(Errno::EBADF).map(|_| ());
    let argument = argument as u32;
    match command as u32 {
        command @ (command::DUPFD | command::DUPFD_CLOEXEC) => {
            open(&files.lock())?;
            let limit = open_file_limit(process);
            if u64::from(argument) >= limit {
                return Err(Errno::EINVAL);
            }
            let close_on_exec = command == command::DUPFD_CLOEXEC;
            let new = files.lock().duplicate(fd, argument, close_on_exec, limit)?;
            Ok(u64::from(new))
        }
        command::GETFD => {
            let close_on_exec = files.lock().get(fd).ok_or(Errno::EBADF)?.close_on_exec;
            Ok(u64::from(close_on_exec))
        }
        command::SETFD => {
            let mut table = files.lock();
            let descriptor = table.get_mut(fd).ok_or(Errno::EBADF)?;
            descriptor.close_on_exec = argument & FD_CLOEXEC != 0;
            Ok(0)
        }
        command::GETFL => {
            let flags = files
                .lock()
                .get(fd)
                .ok_or(Errno::EBADF)?
                .file
                .status_flags();
            Ok(u64::from(flags))
        }
        command::SETFL => {
            let table = files.lock();
            let file = &table.get(fd).ok_or(Errno::EBADF)?.file;
            if file.is_location_only() {
                return Err(Errno::EBADF);
            }
            file.set_status_flags(argument);
            Ok(0)
        }
        _ => {
            let table = files.lock();
            match table.get(fd).ok_or(Errno::EBADF)?.file.is_location_only() {
                true => Err(Errno::EBADF),
                false => Err(Errno::EINVAL),
            }
        }
    }
}

/// `close_range(first, last, flags)`, by the thread whose table is `files`: closes every open
/// descriptor from `first` to `last`, or with `CLOSE_RANGE_CLOEXEC` marks them close-on-exec;
/// with `CLOSE_RANGE_UNSHARE`, the caller first gets a table of its own if it shares one, which
/// it is given back to keep. Descriptors not open in the range are skipped. As Linux: `EINVAL`
/// for another flag or `first` above `last`, `ENOMEM` without memory for the copy. All three
/// arguments are 32-bit.
pub fn close_range(
    files: &Descriptors,
    first: u64,
    last: u64,
    flags: u64,
) -> Result<Option<Descriptors>, Errno> {
    let (first, last, flags) = (first as u32, last as u32, flags as u32);
    let known = close_range_flag::UNSHARE | close_range_flag::CLOEXEC;
    if flags & !known != 0 || first > last {
        return Err(Errno::EINVAL);
    }
    let own = match flags & close_range_flag::UNSHARE != 0 && files.is_shared() {
        true => Some(files.copy()?),
        false => None,
    };
    let files = own.as_ref().unwrap_or(files);
    if flags & close_range_flag::CLOEXEC != 0 {
        files.lock().mark_close_on_exec(first, last);
        return Ok(own);
    }
    let mut next = first;
    loop {
        let closed = files.lock().remove_first_in(next, last);
        let Some((fd, descriptor)) = closed else {
            break;
        };
        drop(descriptor);
        if fd == last {
            break;
        }
        next = fd + 1;
    }
    Ok(own)
}

/// The `ioctl`s every open descriptor answers ([`request`]): whether it is one, and its result
/// if so. `FIOCLEX` and `FIONCLEX` set and clear descriptor `fd`'s close-on-exec flag; `FIONBIO`
/// sets `O_NONBLOCK` on `file`, what `fd` names, if the `int` at `argument` is not 0 and clears
/// it otherwise; `FIOASYNC` reads its `int` and changes nothing. `EFAULT` if the `int` is not the
/// program's memory.
pub fn descriptor_ioctl<P: Platform>(
    process: &Process<P>,
    files: &Descriptors,
    file: &OpenFile,
    fd: u64,
    request: u64,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    match request as u32 {
        request @ (request::FIOCLEX | request::FIONCLEX) => {
            let mut table = files.lock();
            Some(match table.get_mut(fd as u32) {
                Some(descriptor) => {
                    descriptor.close_on_exec = request == request::FIOCLEX;
                    Ok(0)
                }
                None => Err(Errno::EBADF),
            })
        }
        request @ (request::FIONBIO | request::FIOASYNC) => {
            let mut value = [0; 4];
            Some(match process.read_user_memory(argument, &mut value) {
                Ok(()) => {
                    if request == request::FIONBIO {
                        file.set_nonblocking(i32::from_ne_bytes(value) != 0);
                    }
                    Ok(0)
                }
                Err(_) => Err(Errno::EFAULT),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> Arc<OpenFile> {
        Arc::new(OpenFile::new(FileKind::Console, open_flag::RDWR))
    }

    fn open(table: &DescriptorTable) -> Vec<u32> {
        (0..64).filter(|&fd| table.get(fd).is_some()).collect()
    }

    const LIMIT: u64 = 1024;

    #[test]
    fn a_new_table_has_the_standard_streams_on_one_description() {
        let file = console();
        let table = DescriptorTable::with_standard_streams(Arc::clone(&file));
        assert_eq!(open(&table), [0, 1, 2]);
        for fd in 0..3 {
            let descriptor = table.get(fd).unwrap();
            assert!(Arc::ptr_eq(&descriptor.file, &file));
            assert!(!descriptor.close_on_exec);
        }
        assert_eq!(Arc::strong_count(&file), 4);
    }

    #[test]
    fn new_descriptors_take_the_lowest_free_number() {
        let mut table = DescriptorTable::with_standard_streams(console());
        assert_eq!(table.duplicate(0, 0, false, LIMIT), Ok(3));
        assert_eq!(table.duplicate(0, 0, false, LIMIT), Ok(4));
        assert!(table.remove(1).is_some());
        assert!(table.remove(3).is_some());
        assert_eq!(table.duplicate(2, 0, false, LIMIT), Ok(1));
        assert_eq!(table.duplicate(2, 0, false, LIMIT), Ok(3));
        assert_eq!(table.duplicate(2, 0, false, LIMIT), Ok(5));
        assert_eq!(open(&table), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_duplicate_at_least_a_number_skips_the_free_ones_below_it() {
        let mut table = DescriptorTable::with_standard_streams(console());
        assert!(table.remove(0).is_some());
        assert_eq!(table.duplicate(1, 10, false, LIMIT), Ok(10));
        assert_eq!(table.duplicate(1, 10, false, LIMIT), Ok(11));
        assert_eq!(table.duplicate(1, 1, false, LIMIT), Ok(3));
        assert_eq!(table.duplicate(1, 0, false, LIMIT), Ok(0));
        assert_eq!(open(&table), [0, 1, 2, 3, 10, 11]);
    }

    #[test]
    fn the_open_file_limit_bounds_new_descriptors_with_emfile() {
        let mut table = DescriptorTable::with_standard_streams(console());
        assert_eq!(table.duplicate(0, 0, false, 4), Ok(3));
        assert_eq!(table.duplicate(0, 0, false, 4), Err(Errno::EMFILE));
        assert_eq!(table.duplicate(0, 4, false, 5), Ok(4));
        assert_eq!(table.duplicate(0, 0, false, 5), Err(Errno::EMFILE));
        assert!(table.remove(1).is_some());
        assert_eq!(table.duplicate(0, 0, false, 5), Ok(1));
    }

    #[test]
    fn duplicating_or_closing_a_descriptor_that_is_not_open_is_ebadf() {
        let mut table = DescriptorTable::with_standard_streams(console());
        assert_eq!(table.duplicate(3, 0, false, LIMIT), Err(Errno::EBADF));
        assert_eq!(
            table
                .duplicate_to(7, 1, false, LIMIT)
                .map(|old| old.is_some()),
            Err(Errno::EBADF)
        );
        assert!(table.remove(3).is_none());
        assert!(table.remove(u32::MAX).is_none());
        assert!(table.remove(2).is_some());
        assert!(table.remove(2).is_none());
        assert_eq!(table.duplicate(2, 0, false, LIMIT), Err(Errno::EBADF));
    }

    #[test]
    fn duplicates_share_the_description_and_its_flags_and_offset_but_not_close_on_exec() {
        let mut table = DescriptorTable::with_standard_streams(console());
        let fd = table.duplicate(0, 0, true, LIMIT).unwrap();
        let (original, copy) = (table.get(0).unwrap(), table.get(fd).unwrap());
        assert!(Arc::ptr_eq(&original.file, &copy.file));
        assert!(!original.close_on_exec);
        assert!(copy.close_on_exec);
        copy.file
            .set_status_flags(open_flag::NONBLOCK | open_flag::APPEND);
        copy.file.set_offset(42);
        assert!(original.file.is_nonblocking());
        assert_eq!(original.file.offset(), 42);
        assert_eq!(
            original.file.status_flags(),
            open_flag::RDWR | open_flag::NONBLOCK | open_flag::APPEND
        );
    }

    #[test]
    fn set_status_flags_keeps_the_access_mode_and_only_append_and_nonblock() {
        let file = OpenFile::new(FileKind::Console, open_flag::WRONLY);
        file.set_status_flags(u32::MAX);
        assert_eq!(
            file.status_flags(),
            open_flag::WRONLY | open_flag::APPEND | open_flag::NONBLOCK
        );
        assert!(file.is_writable() && !file.is_readable());
        file.set_status_flags(0);
        assert_eq!(file.status_flags(), open_flag::WRONLY);
        file.set_nonblocking(true);
        assert!(file.is_nonblocking());
        file.set_nonblocking(false);
        assert_eq!(file.status_flags(), open_flag::WRONLY);
        let read_only = OpenFile::new(FileKind::Console, open_flag::RDONLY);
        assert!(read_only.is_readable() && !read_only.is_writable());
    }

    #[test]
    fn duplicating_to_a_number_closes_what_it_named_and_grows_the_table() {
        let mut table = DescriptorTable::with_standard_streams(console());
        let other = Arc::new(OpenFile::new(FileKind::Console, open_flag::RDONLY));
        table.slots[1] = Some(Descriptor {
            file: Arc::clone(&other),
            close_on_exec: true,
        });
        let replaced = table.duplicate_to(0, 1, false, LIMIT).unwrap().unwrap();
        assert!(Arc::ptr_eq(&replaced.file, &other));
        assert!(!table.get(1).unwrap().close_on_exec);
        assert!(table.duplicate_to(0, 40, true, LIMIT).unwrap().is_none());
        assert!(table.get(40).unwrap().close_on_exec);
        assert_eq!(table.duplicate(0, 0, false, LIMIT), Ok(3));
        assert_eq!(open(&table), [0, 1, 2, 3, 40]);
    }

    #[test]
    fn closing_the_highest_descriptors_shrinks_the_table() {
        let mut table = DescriptorTable::with_standard_streams(console());
        table.duplicate_to(0, 30, false, LIMIT).unwrap();
        assert_eq!(table.slots.len(), 31);
        assert!(table.remove(30).is_some());
        assert_eq!(table.slots.len(), 3);
        assert!(table.remove(1).is_some());
        assert_eq!(table.slots.len(), 3);
    }

    #[test]
    fn close_range_closes_only_the_open_descriptors_in_the_range() {
        let mut table = DescriptorTable::with_standard_streams(console());
        for fd in [5, 6, 9] {
            table.duplicate_to(0, fd, false, LIMIT).unwrap();
        }
        let mut closed = Vec::new();
        let mut next = 2;
        while let Some((fd, _)) = table.remove_first_in(next, 6) {
            closed.push(fd);
            next = fd + 1;
        }
        assert_eq!(closed, [2, 5, 6]);
        assert_eq!(open(&table), [0, 1, 9]);
        assert!(table.remove_first_in(10, u32::MAX).is_none());
        assert_eq!(
            table.remove_first_in(0, u32::MAX).map(|(fd, _)| fd),
            Some(0)
        );
    }

    #[test]
    fn close_on_exec_is_marked_over_a_range_and_those_marked_are_closed() {
        let mut table = DescriptorTable::with_standard_streams(console());
        table.duplicate_to(0, 8, false, LIMIT).unwrap();
        table.mark_close_on_exec(1, 5);
        table.mark_close_on_exec(8, u32::MAX);
        let marked: Vec<u32> = (0..10)
            .filter(|&fd| table.get(fd).is_some_and(|d| d.close_on_exec))
            .collect();
        assert_eq!(marked, [1, 2, 8]);
        let mut closed = Vec::new();
        while let Some((fd, _)) = table.remove_first_closed_on_exec(0) {
            closed.push(fd);
        }
        assert_eq!(closed, [1, 2, 8]);
        assert_eq!(open(&table), [0]);
    }

    #[test]
    fn a_copied_table_names_the_same_descriptions_but_changes_on_its_own() {
        let files = Descriptors::with_console_streams();
        files.lock().get_mut(2).unwrap().close_on_exec = true;
        let copy = files.copy().unwrap();
        assert!(!files.is_shared() && !copy.is_shared());
        assert!(Arc::ptr_eq(&files.file(0).unwrap(), &copy.file(0).unwrap()));
        assert!(copy.lock().get(2).unwrap().close_on_exec);
        assert!(copy.lock().remove(1).is_some());
        assert!(files.file(1).is_ok());
        assert_eq!(copy.file(1).map(|_| ()), Err(Errno::EBADF));
        let shared = files.share();
        assert!(files.is_shared());
        assert!(shared.lock().remove(0).is_some());
        assert_eq!(files.file(0).map(|_| ()), Err(Errno::EBADF));
    }

    #[test]
    fn lookups_ignore_the_upper_half_of_the_descriptor_register() {
        let files = Descriptors::with_console_streams();
        assert!(files.file(1 << 32 | 2).is_ok());
        assert_eq!(files.file(3).map(|_| ()), Err(Errno::EBADF));
        assert_eq!(files.file(u64::MAX).map(|_| ()), Err(Errno::EBADF));
    }

    #[test]
    fn close_range_refuses_unknown_flags_and_a_reversed_range_then_unshares_on_request() {
        const UNSHARE: u64 = close_range_flag::UNSHARE as u64;
        const CLOEXEC: u64 = close_range_flag::CLOEXEC as u64;
        let files = Descriptors::with_console_streams();
        assert_eq!(
            close_range(&files, 0, 2, 1).map(|own| own.is_some()),
            Err(Errno::EINVAL)
        );
        assert_eq!(
            close_range(&files, 3, 2, 0).map(|own| own.is_some()),
            Err(Errno::EINVAL)
        );
        let shared = files.share();
        let own = close_range(&shared, 1, u64::from(u32::MAX), UNSHARE)
            .unwrap()
            .unwrap();
        assert_eq!(open(&own.lock()), [0]);
        assert_eq!(open(&files.lock()), [0, 1, 2]);
        drop(shared);
        assert!(close_range(&files, 0, 0, UNSHARE).unwrap().is_none());
        assert_eq!(open(&files.lock()), [1, 2]);
        assert!(close_range(&files, 0, 9, CLOEXEC).unwrap().is_none());
        assert!(files.lock().get(2).unwrap().close_on_exec);
    }
}
