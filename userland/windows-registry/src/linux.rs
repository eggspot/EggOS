//! [`HiveFiles`] through Linux system calls, for the Windows layer and Settings: the lock is an
//! open file description lock (`F_OFD_SETLKW`), held by the open file rather than the process,
//! so two threads of one process wait for each other too.

use alloc::vec::Vec;

use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::{call, number};

use crate::store::{FileError, FileFacts, FileHandle, HiveFiles};

const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_RDWR: usize = 2;
const O_CREAT: usize = 0o100;
const O_EXCL: usize = 0o200;
const O_CLOEXEC: usize = 0o2_000_000;
const F_OFD_SETLKW: usize = 38;
const F_WRLCK: i16 = 1;
const SEEK_SET: i16 = 0;
const FLOCK_BYTES: usize = 32;
const CLOCK_REALTIME: usize = 0;
const STAT_WORDS: usize = 18;
const STAT_INODE_WORD: usize = 1;
const STAT_SIZE_WORD: usize = 6;
const STAT_MODIFY_SECONDS_WORD: usize = 11;
const STAT_MODIFY_NANOSECONDS_WORD: usize = 12;
const INTERVALS_PER_SECOND: u64 = 10_000_000;
const NANOSECONDS_PER_INTERVAL: u64 = 100;
const FILETIME_EPOCH_INTERVALS: u64 = 11_644_473_600 * INTERVALS_PER_SECOND;

/// The hive files on this machine's file systems.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxHiveFiles;

fn file_error(errno: Errno) -> FileError {
    match errno {
        Errno::ENOENT => FileError::NotFound,
        Errno::EEXIST => FileError::Exists,
        Errno::EINTR => FileError::Interrupted,
        Errno::ENOSPC => FileError::DiskFull,
        Errno::ENOSYS => FileError::Unsupported,
        _ => FileError::Failed,
    }
}

unsafe fn system(number: usize, arguments: [usize; 6]) -> Result<usize, FileError> {
    // SAFETY: the caller's duty.
    Errno::result(unsafe { call(number, arguments) }).map_err(file_error)
}

fn terminated(path: &[u8]) -> Result<Vec<u8>, FileError> {
    if path.contains(&0) {
        return Err(FileError::Failed);
    }
    let mut owned = Vec::with_capacity(path.len() + 1);
    owned.extend_from_slice(path);
    owned.push(0);
    Ok(owned)
}

fn open(path: &[u8], flags: usize, mode: u32) -> Result<FileHandle, FileError> {
    let path = terminated(path)?;
    let arguments = [
        AT_FDCWD,
        path.as_ptr() as usize,
        flags | O_CLOEXEC,
        mode as usize,
        0,
        0,
    ];
    // SAFETY: `openat` reads the NUL-terminated path `path` holds.
    unsafe { system(number::OPENAT, arguments) }
}

fn write_lock_request() -> [u8; FLOCK_BYTES] {
    let mut request = [0u8; FLOCK_BYTES];
    request[0..2].copy_from_slice(&F_WRLCK.to_ne_bytes());
    request[2..4].copy_from_slice(&SEEK_SET.to_ne_bytes());
    request
}

impl HiveFiles for LinuxHiveFiles {
    fn open_to_read(&mut self, path: &[u8]) -> Result<FileHandle, FileError> {
        open(path, O_RDONLY, 0)
    }

    fn create_new(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError> {
        open(path, O_WRONLY | O_CREAT | O_EXCL, mode)
    }

    fn lock(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError> {
        let handle = open(path, O_RDWR | O_CREAT, mode)?;
        let request = write_lock_request();
        let arguments = [handle, F_OFD_SETLKW, request.as_ptr() as usize, 0, 0, 0];
        // SAFETY: `fcntl(F_OFD_SETLKW)` reads one 32-byte `struct flock`, which `request` is.
        match unsafe { system(number::FCNTL, arguments) } {
            Ok(_) => Ok(handle),
            Err(error) => {
                self.close(handle);
                Err(error)
            }
        }
    }

    fn read(&mut self, file: FileHandle, buffer: &mut [u8]) -> Result<usize, FileError> {
        let arguments = [file, buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0];
        // SAFETY: `read` writes at most `buffer.len()` bytes into the borrowed buffer.
        unsafe { system(number::READ, arguments) }
    }

    fn write_all(&mut self, file: FileHandle, mut bytes: &[u8]) -> Result<(), FileError> {
        while !bytes.is_empty() {
            let arguments = [file, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0];
            // SAFETY: `write` only reads the borrowed bytes.
            match unsafe { system(number::WRITE, arguments) } {
                Ok(0) => return Err(FileError::Failed),
                Ok(written) => bytes = &bytes[written.min(bytes.len())..],
                Err(FileError::Interrupted) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn sync(&mut self, file: FileHandle) -> Result<(), FileError> {
        // SAFETY: `fsync` touches no memory.
        let synced = Errno::result(unsafe { call(number::FSYNC, [file, 0, 0, 0, 0, 0]) });
        match synced {
            Ok(_) => Ok(()),
            Err(Errno::ENOSYS | Errno::EINVAL) => Err(FileError::Unsupported),
            Err(errno) => Err(file_error(errno)),
        }
    }

    fn close(&mut self, file: FileHandle) {
        // SAFETY: `close` touches no memory.
        let _ = unsafe { system(number::CLOSE, [file, 0, 0, 0, 0, 0]) };
    }

    fn facts(&mut self, path: &[u8]) -> Option<FileFacts> {
        let path = terminated(path).ok()?;
        let mut words = [0u64; STAT_WORDS];
        let arguments = [
            AT_FDCWD,
            path.as_ptr() as usize,
            words.as_mut_ptr() as usize,
            0,
            0,
            0,
        ];
        // SAFETY: `newfstatat` reads the NUL-terminated path and writes one `struct stat` (144
        // bytes on x86_64, 128 elsewhere) into the borrowed 144 bytes of `words`.
        unsafe { system(number::NEWFSTATAT, arguments) }.ok()?;
        Some(FileFacts {
            inode: words[STAT_INODE_WORD],
            size: words[STAT_SIZE_WORD],
            modified_seconds: words[STAT_MODIFY_SECONDS_WORD],
            modified_nanoseconds: words[STAT_MODIFY_NANOSECONDS_WORD],
        })
    }

    fn make_directory(&mut self, path: &[u8], mode: u32) -> Result<(), FileError> {
        let path = terminated(path)?;
        let arguments = [AT_FDCWD, path.as_ptr() as usize, mode as usize, 0, 0, 0];
        // SAFETY: `mkdirat` reads the NUL-terminated path.
        unsafe { system(number::MKDIRAT, arguments) }.map(|_| ())
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), FileError> {
        let from = terminated(from)?;
        let to = terminated(to)?;
        let arguments = [
            AT_FDCWD,
            from.as_ptr() as usize,
            AT_FDCWD,
            to.as_ptr() as usize,
            0,
            0,
        ];
        // SAFETY: `renameat2` reads the two NUL-terminated paths.
        unsafe { system(number::RENAMEAT2, arguments) }.map(|_| ())
    }

    fn remove(&mut self, path: &[u8]) -> Result<(), FileError> {
        let path = terminated(path)?;
        let arguments = [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0];
        // SAFETY: `unlinkat` reads the NUL-terminated path.
        unsafe { system(number::UNLINKAT, arguments) }.map(|_| ())
    }

    fn process_id(&mut self) -> u32 {
        // SAFETY: `getpid` touches no memory.
        unsafe { system(number::GETPID, [0; 6]) }.unwrap_or(0) as u32
    }

    fn user_id(&mut self) -> u32 {
        // SAFETY: `geteuid` touches no memory.
        unsafe { system(number::GETEUID, [0; 6]) }.unwrap_or(0) as u32
    }

    fn now(&mut self) -> u64 {
        let mut time = [0u64; 2];
        let arguments = [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0];
        // SAFETY: `clock_gettime` writes one 16-byte `struct timespec` into `time`.
        match unsafe { system(number::CLOCK_GETTIME, arguments) } {
            Ok(_) => {
                time[0] * INTERVALS_PER_SECOND
                    + time[1] / NANOSECONDS_PER_INTERVAL
                    + FILETIME_EPOCH_INTERVALS
            }
            Err(_) => FILETIME_EPOCH_INTERVALS,
        }
    }
}
