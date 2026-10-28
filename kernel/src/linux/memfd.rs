//! Memory files (D139): `memfd_create`, and `read`, `write`, `pread64`, `pwrite64`, `lseek`,
//! `fstat`, `ftruncate`, `fallocate`, `fcntl`'s seals and `mmap` on what it makes.
//!
//! A [`MemoryFile`] is a file with no name, its pages frames of its own taken as they are first
//! written, allocated or mapped: a hole reads as zeros, and a large `ftruncate` takes nothing.
//! Its page list covers only up to the highest page that has a frame. `MAP_SHARED` maps the
//! file's frames into the program at once, through the lent-frames mechanism (D127), so every
//! process that maps the file shares them; a private read-only mapping shares them too, and
//! cannot be made writable. While any mapping exists no frame of the file is freed or replaced:
//! a shrink zeroes the frames past the end and keeps them, so a later grow gives the same
//! frames back, and the list is trimmed when the last mapping goes. Pages a mapping reaches past
//! the end are mapped and read as zeros (no `SIGBUS`), and a grow zeroes what lies between the
//! old end and the new.
//!
//! The file's state is behind one leaf lock, under which only the frame allocator's lock is
//! taken. A program's memory is copied through a buffer on the kernel stack, never while the
//! lock is held: frames are copied into or out of the buffer under the lock, so none is freed
//! under a copy.

use super::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use super::files::{PathBuffer, read_path};
use super::thread::LinuxThread;
use super::{Errno, Iovecs, MAX_RW_COUNT, iovec};
use crate::memory::FRAMES;
use crate::process::{LentFrames, Placement, Process};
use crate::regions::RegionError;
use crate::sync::LeafSpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::Attributes;

/// `memfd_create`'s flags (`MFD_*`).
pub mod flag {
    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = 0x1;
    /// Seals may be added (`F_ADD_SEALS`).
    pub const ALLOW_SEALING: u32 = 0x2;
    /// Huge pages: not supported (`EINVAL`).
    pub const HUGETLB: u32 = 0x4;
    /// Never executable, sealed so: implies [`ALLOW_SEALING`].
    pub const NOEXEC_SEAL: u32 = 0x8;
    /// Executable: accepted, as no memory file is executed.
    pub const EXEC: u32 = 0x10;
}

/// The seals of `F_ADD_SEALS` and `F_GET_SEALS` (`F_SEAL_*`).
pub mod seal {
    /// No more seals.
    pub const SEAL: u32 = 0x1;
    /// The file may not shrink.
    pub const SHRINK: u32 = 0x2;
    /// The file may not grow.
    pub const GROW: u32 = 0x4;
    /// No more writes: not supported (`EINVAL`).
    pub const WRITE: u32 = 0x8;
    /// No more writes through new mappings or calls: not supported (`EINVAL`).
    pub const FUTURE_WRITE: u32 = 0x10;
    /// The mode's executable bits may not change: kept, as nothing changes them.
    pub const EXEC: u32 = 0x20;
    /// The seals kept and enforced.
    pub const KEPT: u32 = SEAL | SHRINK | GROW | EXEC;
}

/// `fcntl`'s seal commands.
pub mod command {
    /// Add seals (`F_ADD_SEALS`).
    pub const ADD_SEALS: u32 = 1033;
    /// Read the seals (`F_GET_SEALS`).
    pub const GET_SEALS: u32 = 1034;
}

/// The longest name `memfd_create` takes: `NAME_MAX` less `memfd:`.
pub const NAME_MAX: usize = 249;

/// The largest size a memory file may have (`EFBIG` past it): the largest offset.
pub const MAX_SIZE: u64 = i64::MAX as u64;

const PAGE: usize = FRAME_SIZE as usize;

const CHUNK: usize = PAGE;

static LAST_INODE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
struct MemoryPages {
    frames: Vec<u64>,
    size: u64,
    seals: u32,
    mappings: u32,
}

/// A memory file: see the module documentation.
pub struct MemoryFile {
    pages: LeafSpinLock<MemoryPages>,
    frame_offset: usize,
    inode: u64,
}

impl core::fmt::Debug for MemoryFile {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("MemoryFile")
            .field(&self.inode)
            .finish()
    }
}

impl PartialEq for MemoryFile {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other)
    }
}

impl Eq for MemoryFile {}

fn pages_for(size: u64) -> usize {
    usize::try_from(size.div_ceil(FRAME_SIZE)).unwrap_or(usize::MAX)
}

fn allocate_frame() -> Option<u64> {
    FRAMES.lock().as_mut()?.allocate()
}

impl MemoryFile {
    /// An empty memory file, `seals` set: [`seal::SEAL`] unless sealing is allowed.
    pub fn new(seals: u32) -> Arc<Self> {
        let frame_offset = FRAMES
            .lock()
            .as_ref()
            .map_or(0, |frames| frames.physical_memory_offset());
        Arc::new(Self {
            pages: LeafSpinLock::new_leaf(MemoryPages {
                seals,
                ..MemoryPages::default()
            }),
            frame_offset,
            inode: LAST_INODE.fetch_add(1, Ordering::Relaxed) + 1,
        })
    }

    /// Its number, as `fstat` reports it (`st_ino`).
    pub fn inode(&self) -> u64 {
        self.inode
    }

    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.pages.lock().size
    }

    /// Its seals (`F_GET_SEALS`).
    pub fn seals(&self) -> u32 {
        self.pages.lock().seals
    }

    /// The frames that hold its pages now, holes not counted.
    pub fn frames_held(&self) -> usize {
        self.pages
            .lock()
            .frames
            .iter()
            .filter(|&&frame| frame != 0)
            .count()
    }

    /// `F_ADD_SEALS`: `EINVAL` for a seal not kept ([`seal::WRITE`], [`seal::FUTURE_WRITE`],
    /// unknown bits), `EPERM` once [`seal::SEAL`] is set.
    pub fn add_seals(&self, seals: u32) -> Result<(), Errno> {
        if seals & !seal::KEPT != 0 {
            return Err(Errno::EINVAL);
        }
        let mut pages = self.pages.lock();
        if pages.seals & seal::SEAL != 0 {
            return Err(Errno::EPERM);
        }
        pages.seals |= seals;
        Ok(())
    }

    fn at(&self, frame: u64) -> *mut u8 {
        (frame as usize).wrapping_add(self.frame_offset) as *mut u8
    }

    fn zero_in(&self, frame: u64, start: usize, length: usize) {
        debug_assert!(start + length <= PAGE);
        // SAFETY: `frame` is one of this file's frames, held under its lock by the caller, so it
        // stays allocated and is reached through the frame allocator's offset; `start..start +
        // length` lies inside it. A program that maps it may write it meanwhile: the bytes are
        // written through raw pointers only, as kernel copies of user memory are (D94).
        unsafe { eggos_hal::memory::zero(self.at(frame).wrapping_add(start), length) };
    }

    fn copy_from_frame(&self, frame: u64, start: usize, out: &mut [u8]) {
        debug_assert!(start + out.len() <= PAGE);
        // SAFETY: as in `zero_in`; `out` is a kernel buffer, never one of the file's frames.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.at(frame).wrapping_add(start),
                out.as_mut_ptr(),
                out.len(),
            )
        };
    }

    fn copy_into_frame(&self, frame: u64, start: usize, bytes: &[u8]) {
        debug_assert!(start + bytes.len() <= PAGE);
        // SAFETY: as in `zero_in`; `bytes` is a kernel buffer, never one of the file's frames.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.at(frame).wrapping_add(start),
                bytes.len(),
            )
        };
    }

    fn free(frame: u64) {
        let mut frames = FRAMES.lock();
        let Some(frames) = frames.as_mut() else {
            return;
        };
        // SAFETY: `frame` came from `allocate` for this file and is taken out of its list now,
        // when no mapping holds it (no mapping exists, or the file is being dropped), so nothing
        // uses it any more.
        unsafe { frames.free(frame) };
    }

    fn frame_for(&self, pages: &mut MemoryPages, page: usize) -> Option<u64> {
        if page >= pages.frames.len() {
            pages
                .frames
                .try_reserve(page + 1 - pages.frames.len())
                .ok()?;
            pages.frames.resize(page + 1, 0);
        }
        if pages.frames[page] == 0 {
            let frame = allocate_frame()?;
            self.zero_in(frame, 0, PAGE);
            pages.frames[page] = frame;
        }
        Some(pages.frames[page])
    }

    fn zero_range(&self, pages: &MemoryPages, from: u64, to: u64) {
        let mut at = from;
        while at < to {
            let page = (at / FRAME_SIZE) as usize;
            if page >= pages.frames.len() {
                return;
            }
            let start = (at % FRAME_SIZE) as usize;
            let length = (PAGE - start).min((to - at) as usize);
            if pages.frames[page] != 0 {
                self.zero_in(pages.frames[page], start, length);
            }
            at += length as u64;
        }
    }

    fn trim(pages: &mut MemoryPages) {
        if pages.mappings != 0 {
            return;
        }
        let kept = pages_for(pages.size).min(pages.frames.len());
        for frame in pages.frames.drain(kept..) {
            if frame != 0 {
                Self::free(frame);
            }
        }
        if pages.frames.is_empty() {
            pages.frames = Vec::new();
        }
    }

    /// Copies its bytes from `offset` into `out`: how many, fewer at its end, 0 past it.
    pub fn read_at(&self, offset: u64, out: &mut [u8]) -> usize {
        let pages = self.pages.lock();
        let count = pages.size.saturating_sub(offset).min(out.len() as u64) as usize;
        let mut done = 0;
        while done < count {
            let at = offset + done as u64;
            let page = (at / FRAME_SIZE) as usize;
            let start = (at % FRAME_SIZE) as usize;
            let length = (PAGE - start).min(count - done);
            let part = &mut out[done..done + length];
            match pages.frames.get(page).copied().unwrap_or(0) {
                0 => part.fill(0),
                frame => self.copy_from_frame(frame, start, part),
            }
            done += length;
        }
        count
    }

    /// Writes `bytes` at `offset`, growing the file past its end: how many, fewer when frames
    /// ran out after the first. `EFBIG` past [`MAX_SIZE`], `EPERM` for a growth under
    /// [`seal::GROW`], `ENOSPC` without a frame for the first byte.
    pub fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<usize, Errno> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|&end| end <= MAX_SIZE)
            .ok_or(Errno::EFBIG)?;
        let mut pages = self.pages.lock();
        if end > pages.size && pages.seals & seal::GROW != 0 {
            return Err(Errno::EPERM);
        }
        if offset > pages.size {
            self.zero_range(&pages, pages.size, offset);
        }
        let mut done = 0;
        while done < bytes.len() {
            let at = offset + done as u64;
            let page = (at / FRAME_SIZE) as usize;
            let start = (at % FRAME_SIZE) as usize;
            let length = (PAGE - start).min(bytes.len() - done);
            let Some(frame) = self.frame_for(&mut pages, page) else {
                break;
            };
            self.copy_into_frame(frame, start, &bytes[done..done + length]);
            done += length;
        }
        if done == 0 && !bytes.is_empty() {
            Self::trim(&mut pages);
            return Err(Errno::ENOSPC);
        }
        pages.size = pages.size.max(offset + done as u64);
        Ok(done)
    }

    /// `ftruncate`: makes it `length` bytes long, cutting it or adding zeros. `EPERM` for a
    /// shrink under [`seal::SHRINK`] or a growth under [`seal::GROW`], `EFBIG` past
    /// [`MAX_SIZE`]. Frames past the new end are freed unless a mapping holds the file; then
    /// they are zeroed and kept.
    pub fn truncate(&self, length: u64) -> Result<(), Errno> {
        if length > MAX_SIZE {
            return Err(Errno::EFBIG);
        }
        let mut pages = self.pages.lock();
        let size = pages.size;
        if length == size {
            return Ok(());
        }
        if length < size && pages.seals & seal::SHRINK != 0 {
            return Err(Errno::EPERM);
        }
        if length > size && pages.seals & seal::GROW != 0 {
            return Err(Errno::EPERM);
        }
        let reached = (pages.frames.len() as u64).saturating_mul(FRAME_SIZE);
        let (from, to) = (size.min(length), size.max(length).max(reached));
        self.zero_range(&pages, from, to);
        pages.size = length;
        Self::trim(&mut pages);
        Ok(())
    }

    /// `fallocate` with mode 0: gives every page of `offset..offset + length` a frame and the
    /// file at least that size. `EFBIG` past [`MAX_SIZE`], `EPERM` for a growth under
    /// [`seal::GROW`], `ENOSPC` when frames run out.
    pub fn allocate(&self, offset: u64, length: u64) -> Result<(), Errno> {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= MAX_SIZE)
            .ok_or(Errno::EFBIG)?;
        let mut pages = self.pages.lock();
        if end > pages.size && pages.seals & seal::GROW != 0 {
            return Err(Errno::EPERM);
        }
        if end > pages.size {
            self.zero_range(&pages, pages.size, end);
        }
        let first = (offset / FRAME_SIZE) as usize;
        for page in first..pages_for(end) {
            if self.frame_for(&mut pages, page).is_none() {
                Self::trim(&mut pages);
                return Err(Errno::ENOSPC);
            }
        }
        pages.size = pages.size.max(end);
        Ok(())
    }

    /// A mapping of `length` bytes of it from `offset`, both whole pages, whose frames it
    /// takes at once, holes and pages past the end too: `ENOMEM` when frames run out.
    pub fn mapping(
        self: &Arc<Self>,
        offset: u64,
        length: u64,
        writable: bool,
    ) -> Result<Arc<MemoryFileMapping>, Errno> {
        let first = usize::try_from(offset / FRAME_SIZE).map_err(|_| Errno::ENOMEM)?;
        let count = usize::try_from(length / FRAME_SIZE).map_err(|_| Errno::ENOMEM)?;
        let last = first.checked_add(count).ok_or(Errno::ENOMEM)?;
        let mut pages = self.pages.lock();
        for page in first..last {
            if self.frame_for(&mut pages, page).is_none() {
                Self::trim(&mut pages);
                return Err(Errno::ENOMEM);
            }
        }
        pages.mappings += 1;
        drop(pages);
        Ok(Arc::new(MemoryFileMapping {
            file: Arc::clone(self),
            first,
            length,
            writable,
        }))
    }
}

impl Drop for MemoryFile {
    fn drop(&mut self) {
        let pages = self.pages.get_mut();
        for frame in pages.frames.drain(..) {
            if frame != 0 {
                Self::free(frame);
            }
        }
    }
}

/// One `mmap` of a memory file, which a process lends its frames from: it keeps the file and
/// its frames while any part of the mapping is mapped.
#[derive(Debug)]
pub struct MemoryFileMapping {
    file: Arc<MemoryFile>,
    first: usize,
    length: u64,
    writable: bool,
}

impl LentFrames for MemoryFileMapping {
    fn frame_at(&self, offset: u64) -> u64 {
        let page = self.first + (offset / FRAME_SIZE) as usize;
        self.file.pages.lock().frames[page]
    }

    fn length(&self) -> u64 {
        self.length
    }

    fn writable(&self) -> bool {
        self.writable
    }
}

impl Drop for MemoryFileMapping {
    fn drop(&mut self) {
        let mut pages = self.file.pages.lock();
        pages.mappings -= 1;
        MemoryFile::trim(&mut pages);
    }
}

/// `mmap` of `memory` at `offset` for `length` bytes, with `attributes`, `shared` or a private
/// read-only mapping (which cannot be made writable), where `placement` says: the mapping's
/// start. `ENOMEM` without frames or room, `EEXIST` for a mapping in the way of
/// `MAP_FIXED_NOREPLACE`.
pub fn map<P: Platform>(
    process: &Process<P>,
    memory: &Arc<MemoryFile>,
    placement: Placement,
    length: u64,
    attributes: Attributes,
    offset: u64,
    shared: bool,
) -> Result<u64, Errno> {
    let mapping = memory.mapping(offset, length, shared)?;
    process
        .map_lent_frames(placement, length, attributes, mapping)
        .map_err(|error| match error {
            RegionError::Overlaps => Errno::EEXIST,
            RegionError::Invalid => Errno::EINVAL,
            RegionError::NotCovered
            | RegionError::NoRoom
            | RegionError::OutOfMemory
            | RegionError::NotWritable => Errno::ENOMEM,
        })
}

/// `memfd_create(name, flags)`: a new empty memory file's descriptor, close-on-exec with
/// `MFD_CLOEXEC`. As Linux: `EINVAL` for an unknown flag, `MFD_HUGETLB`, `MFD_EXEC` with
/// `MFD_NOEXEC_SEAL`, or a name longer than [`NAME_MAX`] bytes; `EFAULT` for a name that is not
/// the program's memory; `EMFILE` without a free descriptor. Without `MFD_ALLOW_SEALING` (or
/// `MFD_NOEXEC_SEAL`, which also seals it never executable), the file is sealed at once.
pub fn memfd_create<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    name: u64,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    let known = flag::CLOEXEC | flag::ALLOW_SEALING | flag::NOEXEC_SEAL | flag::EXEC;
    let both = flag::NOEXEC_SEAL | flag::EXEC;
    if flags & !known != 0 || flags & both == both {
        return Err(Errno::EINVAL);
    }
    let mut buffer = PathBuffer::uninit();
    let name = read_path(process, name, &mut buffer).map_err(|error| match error {
        Errno::ENAMETOOLONG => Errno::EINVAL,
        error => error,
    })?;
    if name.len() > NAME_MAX {
        return Err(Errno::EINVAL);
    }
    let seals = match flags & (flag::ALLOW_SEALING | flag::NOEXEC_SEAL) {
        0 => seal::SEAL,
        _ if flags & flag::NOEXEC_SEAL != 0 => seal::EXEC,
        _ => 0,
    };
    let file = Arc::new(OpenFile::new(
        FileKind::MemoryFile(MemoryFile::new(seals)),
        open_flag::RDWR,
    ));
    let limit = open_file_limit(process);
    let opened = thread
        .files()
        .lock()
        .open(Arc::clone(&file), flags & flag::CLOEXEC != 0, limit);
    drop(file);
    opened.map(u64::from)
}

/// `fcntl`'s seal commands on `file`: `F_GET_SEALS` the seals, `F_ADD_SEALS` more of them
/// ([`MemoryFile::add_seals`]). `EINVAL` for a file that is not a memory file, `EPERM` for
/// `F_ADD_SEALS` without write access.
pub fn seal_command(file: &OpenFile, command: u32, argument: u32) -> Result<u64, Errno> {
    let FileKind::MemoryFile(memory) = file.kind() else {
        return Err(Errno::EINVAL);
    };
    match command {
        command::GET_SEALS => Ok(u64::from(memory.seals())),
        _ if !file.is_writable() => Err(Errno::EPERM),
        _ => memory.add_seals(argument).map(|()| 0),
    }
}

/// `fallocate`'s mode bits Linux knows (`FALLOC_FL_*`).
pub const FALLOCATE_MODES: u64 = 0x7f;

/// `fallocate(fd, mode, offset, length)`: on a memory file with mode 0, gives the range frames
/// and the file at least its end ([`MemoryFile::allocate`]). As Linux, in this order:
/// `EOPNOTSUPP` for an unknown mode bit, `EINVAL` for a negative offset or a length of 0 or
/// less, `EBADF` for a descriptor not open for writing, `ESPIPE` for a pipe, `EISDIR` for a
/// directory, `ENODEV` for anything else not a regular file (a socket, the console, a device),
/// `EFBIG` past the largest size, `EOPNOTSUPP` for another mode or another file system.
pub fn fallocate(
    thread: &LinuxThread,
    fd: u64,
    mode: u64,
    offset: u64,
    length: u64,
) -> Result<u64, Errno> {
    let mode = mode as u32 as u64;
    if mode & !FALLOCATE_MODES != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    let (offset, length) = (offset as i64, length as i64);
    if offset < 0 || length <= 0 {
        return Err(Errno::EINVAL);
    }
    let file = thread.files().file(fd)?;
    if !file.is_writable() || file.is_location_only() {
        return Err(Errno::EBADF);
    }
    let memory = match file.kind() {
        FileKind::Pipe(_) => return Err(Errno::ESPIPE),
        FileKind::Directory(_) => return Err(Errno::EISDIR),
        FileKind::MemoryFile(memory) => memory,
        FileKind::Regular(_) | FileKind::Snapshot(_) => return Err(Errno::EOPNOTSUPP),
        _ => return Err(Errno::ENODEV),
    };
    if (offset as u64)
        .checked_add(length as u64)
        .is_none_or(|end| end > MAX_SIZE)
    {
        return Err(Errno::EFBIG);
    }
    if mode != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    memory.allocate(offset as u64, length as u64).map(|()| 0)
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    memory: &MemoryFile,
    offset: u64,
    buffers: impl Iterator<Item = (u64, u64)>,
) -> Result<u64, Errno> {
    let mut chunk = [0u8; CHUNK];
    let mut total = 0u64;
    for (base, length) in buffers {
        let length = length.min(MAX_RW_COUNT - total);
        let mut done = 0;
        while done < length {
            let at = base + done;
            let wanted = (length - done).min(FRAME_SIZE - at % FRAME_SIZE) as usize;
            let count = memory.read_at(offset + total, &mut chunk[..wanted]);
            if count == 0 {
                return Ok(total);
            }
            if process.write_user_memory(at, &chunk[..count]).is_err() {
                return match total {
                    0 => Err(Errno::EFAULT),
                    _ => Ok(total),
                };
            }
            done += count as u64;
            total += count as u64;
            if count < wanted {
                return Ok(total);
            }
        }
    }
    Ok(total)
}

/// `read` or `readv` of `buffers` from `memory` through `file`, from its offset, which moves
/// past what was read, as a regular file's read does ([`super::files::read_regular`]).
pub(super) fn read<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    memory: &MemoryFile,
    buffers: &Iovecs,
) -> Result<u64, Errno> {
    loop {
        let offset = file.offset();
        let total = copy_out(process, memory, offset, buffers.entries().iter().map(iovec))?;
        if file.move_offset(offset, offset + total) {
            return Ok(total);
        }
    }
}

/// `pread64` of `count` bytes from `offset` into `buffer`, the offset left as it is.
pub fn pread<P: Platform>(
    process: &Process<P>,
    memory: &MemoryFile,
    buffer: u64,
    count: u64,
    offset: u64,
) -> Result<u64, Errno> {
    copy_out(process, memory, offset, core::iter::once((buffer, count)))
}

/// `write`, `writev` (at the description's offset, or its end with `O_APPEND`, moving the
/// offset past what was written) or `pwrite64` (`at`, the offset left) of `buffers` to `memory`
/// through `file`. As Linux: `EFAULT` if nothing could be copied, the file's errors
/// ([`MemoryFile::write_at`]) before the first byte.
pub(super) fn write<P: Platform>(
    process: &Process<P>,
    file: &OpenFile,
    memory: &MemoryFile,
    buffers: &Iovecs,
    at: Option<u64>,
) -> Result<u64, Errno> {
    let append = file.status_flags() & open_flag::APPEND != 0;
    let mut offset = match (append, at) {
        (true, None) => memory.size(),
        (_, Some(at)) => at,
        (false, None) => file.offset(),
    };
    let mut chunk = [0u8; CHUNK];
    let mut written = 0u64;
    let partial = |written: u64, error: Errno| match written {
        0 => Err(error),
        _ => Ok(written),
    };
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(MAX_RW_COUNT - written);
        let mut done = 0;
        while done < length {
            let size = (length - done).min(FRAME_SIZE - (base + done) % FRAME_SIZE) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                return partial(written, Errno::EFAULT);
            }
            let count = match memory.write_at(offset, &chunk[..size]) {
                Ok(count) => count,
                Err(error) => return partial(written, error),
            };
            offset += count as u64;
            if at.is_none() {
                file.set_offset(offset);
            }
            written += count as u64;
            done += count as u64;
            if count < size {
                return Ok(written);
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
