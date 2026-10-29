//! The pages of files that `MAP_SHARED` mappings share (D169): what makes a writable shared
//! mapping of a file on a writable file system (tmpfs, `/dev/shm`, EggFS, FAT) work.
//!
//! The first such mapping of a file gives it a [`SharedFile`]: a frame for each page any of its
//! mappings reaches, read from the file when the mapping is made, and lent to every process
//! that maps it ([`crate::process::Process::map_lent_frames`]), so they all see each other's
//! writes at once, and a `fork` keeps them shared. While it exists the frames are the file's
//! newest bytes: `read` and `pread` take the bytes of the pages it holds from them
//! ([`read_over`]), and `write`, `pwrite` and `ftruncate` change them too ([`wrote`],
//! [`truncated`]), so a program sees the same file through its mapping and its descriptor.
//! The frames are written back to the file, only where they differ from it and never past its
//! end, by `msync`, `fsync`, `sync` and when the last mapping goes ([`SharedFile::write_back`]);
//! then the frames go back to the frame allocator.
//!
//! Lock order: a process's regions' and lent ranges' locks, then the list of shared files, then
//! a file's pages, then the file system's own locks and the frame allocator's. None of them is
//! a leaf lock, since file systems take locks of their own under them.

use super::{Node, StorageError, WriteAt};
use crate::heap;
use crate::memory::FRAMES;
use crate::process::LentFrames;
use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use eggos_hal::memory::FRAME_SIZE;

const PAGE: usize = FRAME_SIZE as usize;

const CHUNK: usize = 512;

static FILES: SpinLock<Option<Box<Vec<Arc<SharedFile>>>>> = SpinLock::new(None);

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Why [`mapping`] made none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedError {
    /// No frame or memory for the pages or the bookkeeping (`ENOMEM`).
    OutOfMemory,
    /// The file could not be read (`EIO`).
    Storage,
}

#[derive(Debug, Default)]
struct Pages {
    frames: Vec<u64>,
    mappings: u32,
}

/// The pages `MAP_SHARED` mappings of one file share: see the module documentation.
#[derive(Debug)]
pub struct SharedFile {
    node: Node,
    frame_offset: usize,
    pages: SpinLock<Pages>,
    modified: AtomicU64,
}

/// One `mmap` of a [`SharedFile`], which a process lends its frames from: it keeps the file's
/// pages while any part of the mapping is mapped, in any process.
#[derive(Debug)]
pub struct SharedMapping {
    file: Arc<SharedFile>,
    first: usize,
    length: u64,
    writable: bool,
}

fn at(frame_offset: usize, frame: u64) -> *mut u8 {
    (frame as usize).wrapping_add(frame_offset) as *mut u8
}

fn free(frame: u64) {
    if let Some(frames) = FRAMES.lock().as_mut() {
        // SAFETY: `frame` came from this allocator for a shared file's page and is taken out of
        // its list when no mapping holds it any more, so nothing maps or uses it.
        unsafe { frames.free(frame) };
    }
}

fn read_file(node: Node, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
    let mut filled = 0;
    while filled < buffer.len() {
        let at = offset.saturating_add(filled as u64);
        match node
            .file_system()
            .read_into(node.id(), at, &mut buffer[filled..])?
        {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

fn find(files: &[Arc<SharedFile>], node: Node) -> Option<&Arc<SharedFile>> {
    files.iter().find(|file| file.node == node)
}

fn shared(node: Node) -> Option<Arc<SharedFile>> {
    let files = FILES.lock();
    find(files.as_deref()?, node).cloned()
}

impl SharedFile {
    fn new(node: Node, now: u64) -> Self {
        let frame_offset = FRAMES
            .lock()
            .as_ref()
            .map_or(0, |frames| frames.physical_memory_offset());
        Self {
            node,
            frame_offset,
            pages: SpinLock::new(Pages::default()),
            modified: AtomicU64::new(now),
        }
    }

    fn fill(&self, first: usize, last: usize) -> Result<(), SharedError> {
        let mut pages = self.pages.lock();
        if pages.frames.len() < last {
            let added = last - pages.frames.len();
            if !heap::may_grow_for_user(added.saturating_mul(size_of::<u64>())) {
                return Err(SharedError::OutOfMemory);
            }
            pages
                .frames
                .try_reserve(added)
                .map_err(|_| SharedError::OutOfMemory)?;
            pages.frames.resize(last, 0);
        }
        for page in first..last {
            if pages.frames[page] != 0 {
                continue;
            }
            let frame = FRAMES
                .lock()
                .as_mut()
                .and_then(|frames| frames.allocate_for_user())
                .ok_or(SharedError::OutOfMemory)?;
            let bytes = at(self.frame_offset, frame);
            // SAFETY: the frame was just allocated, so nothing else uses it, and the allocator
            // reaches it through its offset, `frame_offset`.
            let buffer = unsafe { core::slice::from_raw_parts_mut(bytes, PAGE) };
            let read = match read_file(self.node, page as u64 * FRAME_SIZE, buffer) {
                Ok(read) => read,
                Err(StorageError) => {
                    free(frame);
                    return Err(SharedError::Storage);
                }
            };
            buffer[read..].fill(0);
            pages.frames[page] = frame;
        }
        Ok(())
    }

    fn copy_out(&self, pages: &Pages, offset: u64, buffer: &mut [u8]) {
        let mut done = 0;
        while done < buffer.len() {
            let here = offset + done as u64;
            let page = (here / FRAME_SIZE) as usize;
            let start = (here % FRAME_SIZE) as usize;
            let length = (PAGE - start).min(buffer.len() - done);
            if let Some(&frame) = pages.frames.get(page).filter(|&&frame| frame != 0) {
                // SAFETY: the frame is the file's, kept while its pages' lock is held, reached
                // through the allocator's offset; `start + length` is within it, and `buffer` is
                // a kernel buffer, never one of the frames. Programs that map it may write it
                // meanwhile: it is read through raw pointers only, as kernel copies of user
                // memory are (D94).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        at(self.frame_offset, frame).wrapping_add(start),
                        buffer[done..].as_mut_ptr(),
                        length,
                    )
                };
            }
            done += length;
        }
    }

    fn copy_in(&self, pages: &Pages, offset: u64, bytes: &[u8]) {
        let mut done = 0;
        while done < bytes.len() {
            let here = offset + done as u64;
            let page = (here / FRAME_SIZE) as usize;
            let start = (here % FRAME_SIZE) as usize;
            let length = (PAGE - start).min(bytes.len() - done);
            if let Some(&frame) = pages.frames.get(page).filter(|&&frame| frame != 0) {
                // SAFETY: as in `copy_out`, the other way round.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes[done..].as_ptr(),
                        at(self.frame_offset, frame).wrapping_add(start),
                        length,
                    )
                };
            }
            done += length;
        }
    }

    fn zero_from(&self, pages: &Pages, length: u64) {
        let reached = pages.frames.len() as u64 * FRAME_SIZE;
        let mut here = length;
        while here < reached {
            let page = (here / FRAME_SIZE) as usize;
            let start = (here % FRAME_SIZE) as usize;
            if pages.frames[page] != 0 {
                // SAFETY: as in `copy_out`: `start..PAGE` is within the file's frame.
                unsafe {
                    eggos_hal::memory::zero(
                        at(self.frame_offset, pages.frames[page]).wrapping_add(start),
                        PAGE - start,
                    )
                };
            }
            here += (PAGE - start) as u64;
        }
    }

    /// Writes the pages it holds to its file where they differ from what the file holds, up to
    /// the file's end: [`StorageError`] if the file could not be read or written.
    pub fn write_back(&self) -> Result<(), StorageError> {
        let size = self.node.attributes().size;
        let now = self.modified.load(Ordering::Relaxed);
        let file_system = self.node.file_system();
        let pages = self.pages.lock();
        let mut result = Ok(());
        let mut mine = [0u8; CHUNK];
        let mut theirs = [0u8; CHUNK];
        for (page, &frame) in pages.frames.iter().enumerate() {
            let start = page as u64 * FRAME_SIZE;
            if start >= size {
                break;
            }
            if frame == 0 {
                continue;
            }
            let end = (start + FRAME_SIZE).min(size);
            let mut here = start;
            while here < end {
                let length = ((end - here) as usize).min(CHUNK);
                self.copy_out(&pages, here, &mut mine[..length]);
                let same = read_file(self.node, here, &mut theirs[..length])
                    .is_ok_and(|read| read == length && mine[..length] == theirs[..length]);
                if !same {
                    let to = WriteAt::Offset(here);
                    match file_system.write(self.node.id(), to, &mine[..length], now) {
                        Ok((written, _)) if written == length => {}
                        _ => result = Err(StorageError),
                    }
                }
                here += length as u64;
            }
        }
        result
    }

    fn release(self: &Arc<Self>) {
        let last = {
            let mut pages = self.pages.lock();
            pages.mappings -= 1;
            pages.mappings == 0
        };
        if !last {
            return;
        }
        let _ = self.write_back();
        let frames = {
            let mut files = FILES.lock();
            let mut pages = self.pages.lock();
            if pages.mappings != 0 {
                return;
            }
            if let Some(files) = files.as_deref_mut() {
                files.retain(|file| !Arc::ptr_eq(file, self));
            }
            LIVE.fetch_sub(1, Ordering::Relaxed);
            core::mem::take(&mut pages.frames)
        };
        for frame in frames.into_iter().filter(|&frame| frame != 0) {
            free(frame);
        }
    }
}

impl LentFrames for SharedMapping {
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

    fn write_back(&self, now: u64) -> Result<(), StorageError> {
        self.file.modified.store(now, Ordering::Relaxed);
        self.file.write_back()
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        self.file.release();
    }
}

fn file_for(node: Node, now: u64) -> Result<Arc<SharedFile>, SharedError> {
    let mut files = FILES.lock();
    if files.is_none() {
        if !heap::may_grow_for_user(size_of::<Vec<Arc<SharedFile>>>()) {
            return Err(SharedError::OutOfMemory);
        }
        *files = Some(Box::new(Vec::new()));
    }
    let Some(list) = files.as_deref_mut() else {
        return Err(SharedError::OutOfMemory);
    };
    if let Some(file) = find(list, node) {
        file.pages.lock().mappings += 1;
        file.modified.store(now, Ordering::Relaxed);
        return Ok(Arc::clone(file));
    }
    if !heap::may_grow_for_user(size_of::<SharedFile>() + size_of::<Arc<SharedFile>>()) {
        return Err(SharedError::OutOfMemory);
    }
    list.try_reserve(1).map_err(|_| SharedError::OutOfMemory)?;
    let file = Arc::new(SharedFile::new(node, now));
    file.pages.lock().mappings = 1;
    list.push(Arc::clone(&file));
    LIVE.fetch_add(1, Ordering::Relaxed);
    Ok(file)
}

/// A shared mapping of `length` bytes of regular file `node` from `offset`, both whole pages,
/// writable by the program if `writable`, made at `now` (seconds since the epoch, the time
/// its write-back gives the file): the file's [`SharedFile`], made if it has none, with a frame
/// for each page of the range, read from the file now (zeros past its end).
pub fn mapping(
    node: Node,
    offset: u64,
    length: u64,
    writable: bool,
    now: u64,
) -> Result<Arc<SharedMapping>, SharedError> {
    let first = usize::try_from(offset / FRAME_SIZE).map_err(|_| SharedError::OutOfMemory)?;
    let count = usize::try_from(length / FRAME_SIZE).map_err(|_| SharedError::OutOfMemory)?;
    let last = first.checked_add(count).ok_or(SharedError::OutOfMemory)?;
    if !heap::may_grow_for_user(size_of::<SharedMapping>()) {
        return Err(SharedError::OutOfMemory);
    }
    let mapping = Arc::new(SharedMapping {
        file: file_for(node, now)?,
        first,
        length,
        writable,
    });
    mapping.file.fill(first, last)?;
    Ok(mapping)
}

/// Whether some shared mapping holds pages of `node` now.
pub fn is_mapped(node: Node) -> bool {
    LIVE.load(Ordering::Relaxed) != 0 && shared(node).is_some()
}

/// After `buffer` was read from regular file `node` at `offset`: puts the bytes of the pages a
/// shared mapping holds over it, as they are newer than the file's.
#[inline]
pub fn read_over(node: Node, offset: u64, buffer: &mut [u8]) {
    if LIVE.load(Ordering::Relaxed) != 0 {
        read_over_mapped(node, offset, buffer);
    }
}

#[cold]
#[inline(never)]
fn read_over_mapped(node: Node, offset: u64, buffer: &mut [u8]) {
    if let Some(file) = shared(node) {
        let pages = file.pages.lock();
        file.copy_out(&pages, offset, buffer);
    }
}

/// After `bytes` were written to regular file `node` at `offset`: writes them to the pages a
/// shared mapping holds too.
#[inline]
pub fn wrote(node: Node, offset: u64, bytes: &[u8]) {
    if LIVE.load(Ordering::Relaxed) != 0 {
        wrote_mapped(node, offset, bytes);
    }
}

#[cold]
#[inline(never)]
fn wrote_mapped(node: Node, offset: u64, bytes: &[u8]) {
    if let Some(file) = shared(node) {
        let pages = file.pages.lock();
        file.copy_in(&pages, offset, bytes);
    }
}

/// After regular file `node` was made `length` bytes long: zeroes what the pages a shared
/// mapping holds have from there on, which is no longer the file's.
pub fn truncated(node: Node, length: u64) {
    if LIVE.load(Ordering::Relaxed) == 0 {
        return;
    }
    if let Some(file) = shared(node) {
        let pages = file.pages.lock();
        file.zero_from(&pages, length);
    }
}

/// Writes the pages shared mappings of `node` hold back to it (`fsync`), if any.
pub fn write_back(node: Node) -> Result<(), StorageError> {
    if LIVE.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    shared(node).map_or(Ok(()), |file| file.write_back())
}

/// Writes the pages every shared mapping holds back to their files (`sync`).
pub fn write_back_all() -> Result<(), StorageError> {
    if LIVE.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    let files = FILES.lock();
    let mut result = Ok(());
    for file in files.as_deref().into_iter().flatten() {
        if file.write_back().is_err() {
            result = Err(StorageError);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::tmpfs::{Budget, Tmpfs};
    use crate::vfs::{FileSystem, NodeKind};

    fn tmpfs() -> &'static Tmpfs {
        let budget: &'static Budget = Box::leak(Box::new(Budget::new(1 << 20)));
        Box::leak(Box::new(Tmpfs::new(budget, (0, 99), 0o755).unwrap()))
    }

    fn contents(node: Node) -> Vec<u8> {
        let mut bytes = alloc::vec![0; node.attributes().size as usize];
        let read = read_file(node, 0, &mut bytes).unwrap();
        bytes.truncate(read);
        bytes
    }

    #[test]
    fn mappings_share_pages_that_reads_and_writes_see_and_the_last_writes_back() {
        let _frames = crate::thread::host::frames_for_test();
        let free = || FRAMES.lock().as_ref().unwrap().free_frames();
        let before = free();
        let fs = tmpfs();
        let id = fs
            .create(fs.root(), b"shared", NodeKind::Regular, 0o600, 0)
            .unwrap();
        fs.write(id, WriteAt::Offset(0), b"hello, file", 0).unwrap();
        let node = Node::root_of(fs).with_id(id);
        let first = mapping(node, 0, 2 * FRAME_SIZE, true, 7).unwrap();
        let second = mapping(node, FRAME_SIZE, FRAME_SIZE, true, 7).unwrap();
        assert!(is_mapped(node));
        assert_eq!(second.frame_at(0), first.frame_at(FRAME_SIZE));
        assert_eq!(free(), before - 2);
        let page = at(first.file.frame_offset, first.frame_at(0));
        // SAFETY: the mapping's first frame, reached through the allocator's offset.
        unsafe { page.copy_from_nonoverlapping(b"HELLO".as_ptr(), 5) };
        let mut read = *b"???????????";
        read_over(node, 0, &mut read);
        assert_eq!(&read, b"HELLO, file");
        assert_eq!(contents(node), b"hello, file", "nothing written back yet");
        fs.write(id, WriteAt::Offset(7), b"F", 0).unwrap();
        wrote(node, 7, b"F");
        let mut read = [0u8; 11];
        read_over(node, 0, &mut read);
        assert_eq!(&read, b"HELLO, File");
        first.write_back(9).unwrap();
        assert_eq!(contents(node), b"HELLO, File");
        assert_eq!(fs.attributes(id).modified, 9);
        fs.truncate(id, 3, 0).unwrap();
        truncated(node, 3);
        let mut read = [1u8; 5];
        read_over(node, 0, &mut read);
        assert_eq!(&read, b"HEL\0\0");
        drop(first);
        assert!(is_mapped(node));
        // SAFETY: as above, through the second mapping's frame, page 1 of the file.
        unsafe { at(second.file.frame_offset, second.frame_at(0)).write(b'!') };
        drop(second);
        assert!(!is_mapped(node));
        assert_eq!(
            contents(node),
            b"HEL",
            "nothing past the end is written back"
        );
        assert_eq!(free(), before);
    }
}
