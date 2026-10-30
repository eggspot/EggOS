//! The cache directory on the disk, `/var/cache/eggos/translate`, through Linux system calls.

use crate::cache_files::{self, CacheFiles};
use alloc::string::String;
use alloc::vec::Vec;
use eggos_runtime::syscall::call;
use eggos_translate::cache::policy;

const MKDIRAT: usize = 34;
const UNLINKAT: usize = 35;
const STATFS: usize = 43;
const OPENAT: usize = 56;
const CLOSE: usize = 57;
const GETDENTS64: usize = 61;
const READ: usize = 63;
const WRITE: usize = 64;
const NEWFSTATAT: usize = 79;
const FSTAT: usize = 80;
const UTIMENSAT: usize = 88;
const CLOCK_GETTIME: usize = 113;
const RENAMEAT2: usize = 276;
const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0o100;
const O_EXCL: usize = 0o200;
const O_DIRECTORY: usize = 0o40_000;
const O_CLOEXEC: usize = 0o2_000_000;
const CREATE_MODE: usize = 0o666;
const DIRECTORY_MODE: usize = 0o755;
const CLOCK_REALTIME: usize = 0;
const STAT_WORDS: usize = 16;
const STAT_SIZE_WORD: usize = 6;
const STAT_MODIFY_TIME_WORD: usize = 11;
const STATFS_WORDS: usize = 15;
const STATFS_BLOCK_SIZE: usize = 1;
const STATFS_BLOCKS: usize = 2;
const DIRECTORY_BUFFER_BYTES: usize = 4096;

/// The cache directory on the disk.
#[derive(Debug, Default, Clone, Copy)]
pub struct Disk;

fn result(value: isize) -> Option<usize> {
    usize::try_from(value).ok()
}

fn open(path: &[u8], flags: usize) -> Option<usize> {
    // SAFETY: `openat` reads the NUL-terminated path the caller built and changes no memory.
    result(unsafe {
        call(
            OPENAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                flags | O_CLOEXEC,
                CREATE_MODE,
                0,
                0,
            ],
        )
    })
}

fn close(fd: usize) -> bool {
    // SAFETY: closing a descriptor this module opened changes no memory.
    result(unsafe { call(CLOSE, [fd, 0, 0, 0, 0, 0]) }).is_some()
}

fn stat_words(fd: usize, name: Option<&[u8]>) -> Option<[u64; STAT_WORDS]> {
    let mut words = [0u64; STAT_WORDS];
    let out = words.as_mut_ptr() as usize;
    let made = match name {
        // SAFETY: `newfstatat` reads the NUL-terminated name and writes one 128-byte
        // `struct stat` into `words`.
        Some(name) => unsafe { call(NEWFSTATAT, [fd, name.as_ptr() as usize, out, 0, 0, 0]) },
        // SAFETY: `fstat` writes one 128-byte `struct stat` into `words`.
        None => unsafe { call(FSTAT, [fd, out, 0, 0, 0, 0]) },
    };
    result(made).map(|_| words)
}

fn make_directories() {
    for path in cache_files::directories_to_create() {
        // SAFETY: `mkdirat` reads the NUL-terminated path and changes no memory.
        let _ = unsafe {
            call(
                MKDIRAT,
                [AT_FDCWD, path.as_ptr() as usize, DIRECTORY_MODE, 0, 0, 0],
            )
        };
    }
}

impl CacheFiles for Disk {
    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let fd = open(&cache_files::file_path(name), O_RDONLY)?;
        let size = stat_words(fd, None).map(|words| words[STAT_SIZE_WORD] as usize);
        let mut bytes = Vec::new();
        if let Some(size) = size {
            bytes.resize(size, 0);
            let mut filled = 0;
            while filled < size {
                let rest = &mut bytes[filled..];
                // SAFETY: `read` writes at most `rest.len()` bytes into `rest`.
                let read =
                    unsafe { call(READ, [fd, rest.as_mut_ptr() as usize, rest.len(), 0, 0, 0]) };
                match result(read) {
                    Some(0) | None => break,
                    Some(read) => filled += read,
                }
            }
            bytes.truncate(filled);
        }
        close(fd);
        size.map(|_| bytes)
    }

    fn create(&mut self, name: &str, bytes: &[u8]) -> bool {
        make_directories();
        let Some(fd) = open(&cache_files::file_path(name), O_WRONLY | O_CREAT | O_EXCL) else {
            return false;
        };
        let mut rest = bytes;
        let mut written = true;
        while !rest.is_empty() {
            // SAFETY: `write` reads at most `rest.len()` bytes from `rest`.
            let wrote = unsafe { call(WRITE, [fd, rest.as_ptr() as usize, rest.len(), 0, 0, 0]) };
            match result(wrote) {
                Some(0) | None => {
                    written = false;
                    break;
                }
                Some(count) => rest = &rest[count..],
            }
        }
        close(fd) && written
    }

    fn rename(&mut self, from: &str, to: &str) -> bool {
        let from = cache_files::file_path(from);
        let to = cache_files::file_path(to);
        // SAFETY: `renameat2` reads the two NUL-terminated paths and changes no memory.
        result(unsafe {
            call(
                RENAMEAT2,
                [
                    AT_FDCWD,
                    from.as_ptr() as usize,
                    AT_FDCWD,
                    to.as_ptr() as usize,
                    0,
                    0,
                ],
            )
        })
        .is_some()
    }

    fn remove(&mut self, name: &str) {
        let path = cache_files::file_path(name);
        // SAFETY: `unlinkat` reads the NUL-terminated path and changes no memory.
        let _ = unsafe { call(UNLINKAT, [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0]) };
    }

    fn list(&mut self) -> Vec<policy::CachedFile> {
        let mut files = Vec::new();
        let Some(fd) = open(&cache_files::directory_path(), O_RDONLY | O_DIRECTORY) else {
            return files;
        };
        let mut buffer = [0u8; DIRECTORY_BUFFER_BYTES];
        loop {
            // SAFETY: `getdents64` writes at most the buffer's length of records into it.
            let filled = unsafe {
                call(
                    GETDENTS64,
                    [fd, buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0],
                )
            };
            let Some(filled) = result(filled).filter(|&filled| filled > 0) else {
                break;
            };
            for name in cache_files::entry_names(&buffer[..filled]) {
                let Ok(text) = core::str::from_utf8(name) else {
                    continue;
                };
                let mut stat_name = Vec::from(name);
                stat_name.push(0);
                let Some(words) = stat_words(fd, Some(&stat_name)) else {
                    continue;
                };
                files.push(policy::CachedFile {
                    name: String::from(text),
                    bytes: words[STAT_SIZE_WORD],
                    last_launch: words[STAT_MODIFY_TIME_WORD],
                    in_use: false,
                });
            }
        }
        close(fd);
        files
    }

    fn touch(&mut self, name: &str) {
        let path = cache_files::file_path(name);
        // SAFETY: `utimensat` with no times reads the NUL-terminated path and sets both of the
        // file's times to now; it changes no memory.
        let _ = unsafe { call(UTIMENSAT, [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0]) };
    }

    fn cap(&mut self) -> u64 {
        make_directories();
        let directory = cache_files::directory_path();
        let mut words = [0u64; STATFS_WORDS];
        // SAFETY: `statfs` reads the NUL-terminated path and writes one 120-byte
        // `struct statfs` into `words`.
        let made = unsafe {
            call(
                STATFS,
                [
                    directory.as_ptr() as usize,
                    words.as_mut_ptr() as usize,
                    0,
                    0,
                    0,
                    0,
                ],
            )
        };
        match result(made) {
            Some(_) => {
                policy::cap_bytes(words[STATFS_BLOCKS].saturating_mul(words[STATFS_BLOCK_SIZE]))
            }
            None => 0,
        }
    }

    fn now(&mut self) -> u64 {
        let mut time = [0i64; 2];
        // SAFETY: `clock_gettime` writes one `struct timespec` into `time`.
        let _ = unsafe {
            call(
                CLOCK_GETTIME,
                [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
            )
        };
        time[0].max(0) as u64
    }
}
