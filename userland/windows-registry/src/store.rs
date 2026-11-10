//! The hive files on disk (D231): [`FileStore`] is the one [`Store`] the Windows layer and
//! Settings' Windows pane both use, over the few file system calls of [`HiveFiles`].
//!
//! - The user's hive is below the home folder and only its owner reads it
//!   ([`USER_HIVE_MODE`]); the machine's is readable by everyone ([`MACHINE_HIVE_MODE`]).
//! - Every read-modify-write holds an exclusive lock on the hive's `.lock` file beside it
//!   ([`LOCK_SUFFIX`]), so two processes never lose each other's change.
//! - A write goes to a temporary file only this process names (the hive's name,
//!   [`TEMPORARY_SUFFIX`], a dot and the process id), made with `O_EXCL`, flushed and renamed
//!   over the hive.
//! - A read goes to the end of the file, never trusting a size read before it, and stops at
//!   [`MAX_HIVE_BYTES`] + 1 bytes, so a file past the cap reads as too large.
//! - The stamp mixes the file's inode with its change time and size, so a hive renamed over
//!   in the same tick still reads as changed.

use alloc::format;
use alloc::vec::Vec;

use crate::MAX_HIVE_BYTES;
use crate::registry::{
    DAMAGED_SUFFIX, HiveFile, MACHINE_HIVE_PATH, Store, StoreError, TEMPORARY_SUFFIX,
    USER_HIVE_PATH,
};

/// What the lock file's name adds to the hive's.
pub const LOCK_SUFFIX: &str = ".lock";
/// The permissions of the user's hive and its lock: its owner's alone.
pub const USER_HIVE_MODE: u32 = 0o600;
/// The permissions of the machine's hive and its lock: every user reads them.
pub const MACHINE_HIVE_MODE: u32 = 0o644;
/// The permissions of the folders made for the user's hive.
pub const USER_FOLDER_MODE: u32 = 0o700;
/// The permissions of the folders made for the machine's hive.
pub const MACHINE_FOLDER_MODE: u32 = 0o755;

const READ_CHUNK_BYTES: usize = 64 * 1024;
const MAX_INTERRUPTED_TRIES: usize = 64;

/// Why a file system call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// Nothing is at the path (`ENOENT`).
    NotFound,
    /// Something is already at the path (`EEXIST`).
    Exists,
    /// A signal ended the call before it did anything (`EINTR`).
    Interrupted,
    /// The disk is full (`ENOSPC`).
    DiskFull,
    /// The file system cannot do it (`ENOSYS`, `EINVAL` from `fsync`).
    Unsupported,
    /// Anything else.
    Failed,
}

/// What [`FileStore`] needs to know about a file to tell when it changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileFacts {
    /// Its inode number, which a rename over it changes.
    pub inode: u64,
    /// Its size in bytes.
    pub size: u64,
    /// When its contents last changed: seconds since the epoch.
    pub modified_seconds: u64,
    /// When its contents last changed: the nanoseconds past those seconds.
    pub modified_nanoseconds: u64,
}

/// An open file.
pub type FileHandle = usize;

/// The file system calls the hive files need. Paths are byte strings without a NUL.
pub trait HiveFiles {
    /// Opens `path` for reading.
    fn open_to_read(&mut self, path: &[u8]) -> Result<FileHandle, FileError>;
    /// Makes `path` with `mode` and opens it for writing; [`FileError::Exists`] when anything
    /// is there (`O_CREAT | O_EXCL`).
    fn create_new(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError>;
    /// Opens the lock file `path`, making it with `mode` when it is missing, and takes an
    /// exclusive lock on it, waiting while another open file holds one; closing the handle
    /// lets go. [`FileError::Interrupted`] when a signal ended the wait.
    fn lock(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError>;
    /// Reads into `buffer`; 0 at the end.
    fn read(&mut self, file: FileHandle, buffer: &mut [u8]) -> Result<usize, FileError>;
    /// Writes all of `bytes`.
    fn write_all(&mut self, file: FileHandle, bytes: &[u8]) -> Result<(), FileError>;
    /// Writes the file's data to its disk.
    fn sync(&mut self, file: FileHandle) -> Result<(), FileError>;
    /// Closes the file.
    fn close(&mut self, file: FileHandle);
    /// What is known about the file at `path`, following symbolic links; `None` when nothing
    /// is there.
    fn facts(&mut self, path: &[u8]) -> Option<FileFacts>;
    /// Makes the folder `path` with `mode`.
    fn make_directory(&mut self, path: &[u8], mode: u32) -> Result<(), FileError>;
    /// Renames `from` to `to`, replacing a file there.
    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), FileError>;
    /// Removes the file at `path`.
    fn remove(&mut self, path: &[u8]) -> Result<(), FileError>;
    /// This process's id.
    fn process_id(&mut self) -> u32;
    /// The effective user id: 0 is root, who alone changes the machine's hive.
    fn user_id(&mut self) -> u32;
    /// The time now, in 100 ns since 1601.
    fn now(&mut self) -> u64;
}

/// The permissions a hive file and its lock get.
pub const fn hive_mode(file: HiveFile) -> u32 {
    match file {
        HiveFile::User => USER_HIVE_MODE,
        HiveFile::Machine => MACHINE_HIVE_MODE,
    }
}

const fn folder_mode(file: HiveFile) -> u32 {
    match file {
        HiveFile::User => USER_FOLDER_MODE,
        HiveFile::Machine => MACHINE_FOLDER_MODE,
    }
}

const fn slot(file: HiveFile) -> usize {
    match file {
        HiveFile::User => 0,
        HiveFile::Machine => 1,
    }
}

fn store_error(error: FileError) -> StoreError {
    match error {
        FileError::DiskFull => StoreError::DiskFull,
        _ => StoreError::Failed,
    }
}

/// The stamp [`FileStore`] gives a file with these facts.
pub fn stamp_of(facts: FileFacts) -> u64 {
    facts
        .modified_seconds
        .wrapping_mul(1_000_000_007)
        .wrapping_add(facts.modified_nanoseconds)
        .wrapping_add(facts.size.rotate_left(24))
        .wrapping_add(facts.inode.rotate_left(44))
}

/// The hive files through `F`: the user's below the home folder, the machine's at
/// [`MACHINE_HIVE_PATH`].
#[derive(Debug)]
pub struct FileStore<F: HiveFiles> {
    files: F,
    home: Vec<u8>,
    locks: [Option<FileHandle>; 2],
}

impl<F: HiveFiles> FileStore<F> {
    /// The hives of the user whose home folder is `home`, through `files`.
    pub fn new(files: F, home: &[u8]) -> Self {
        let mut home = home.to_vec();
        while home.len() > 1 && home.last() == Some(&b'/') {
            home.pop();
        }
        Self {
            files,
            home,
            locks: [None, None],
        }
    }

    /// The file system calls, as given.
    pub fn files(&mut self) -> &mut F {
        &mut self.files
    }

    /// The path of the hive file with `suffix` added to its name.
    pub fn path(&self, file: HiveFile, suffix: &[u8]) -> Vec<u8> {
        let mut path = Vec::new();
        match file {
            HiveFile::User => {
                path.extend_from_slice(&self.home);
                if path.last() != Some(&b'/') {
                    path.push(b'/');
                }
                path.extend_from_slice(USER_HIVE_PATH.as_bytes());
            }
            HiveFile::Machine => path.extend_from_slice(MACHINE_HIVE_PATH.as_bytes()),
        }
        path.extend_from_slice(suffix);
        path
    }

    /// The path of this process's temporary twin of the hive file.
    pub fn temporary_path(&mut self, file: HiveFile) -> Vec<u8> {
        let suffix = format!("{TEMPORARY_SUFFIX}.{}", self.files.process_id());
        self.path(file, suffix.as_bytes())
    }

    fn make_folders(&mut self, file: HiveFile) {
        let path = self.path(file, b"");
        let first = match file {
            HiveFile::User => self.home.len() + 1,
            HiveFile::Machine => 1,
        };
        for (at, byte) in path.iter().enumerate().skip(first) {
            if *byte == b'/' {
                let _ = self.files.make_directory(&path[..at], folder_mode(file));
            }
        }
    }

    fn create_temporary(&mut self, file: HiveFile, path: &[u8]) -> Result<FileHandle, FileError> {
        match self.files.create_new(path, hive_mode(file)) {
            Err(FileError::Exists) => {
                self.files.remove(path)?;
                self.files.create_new(path, hive_mode(file))
            }
            created => created,
        }
    }
}

impl<F: HiveFiles> Drop for FileStore<F> {
    fn drop(&mut self) {
        for held in &mut self.locks {
            if let Some(handle) = held.take() {
                self.files.close(handle);
            }
        }
    }
}

impl<F: HiveFiles> Store for FileStore<F> {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path(file, b"");
        let handle = match self.files.open_to_read(&path) {
            Ok(handle) => handle,
            Err(FileError::NotFound) => return Ok(None),
            Err(_) => return Err(StoreError::Failed),
        };
        let mut bytes = Vec::new();
        let mut interrupted = 0;
        let read = loop {
            let room = (MAX_HIVE_BYTES + 1 - bytes.len()).min(READ_CHUNK_BYTES);
            if room == 0 {
                break Ok(());
            }
            let start = bytes.len();
            if bytes.try_reserve(room).is_err() {
                break Err(StoreError::Failed);
            }
            bytes.resize(start + room, 0);
            match self.files.read(handle, &mut bytes[start..]) {
                Ok(0) => {
                    bytes.truncate(start);
                    break Ok(());
                }
                Ok(count) => bytes.truncate(start + count.min(room)),
                Err(FileError::Interrupted) if interrupted < MAX_INTERRUPTED_TRIES => {
                    interrupted += 1;
                    bytes.truncate(start);
                }
                Err(_) => break Err(StoreError::Failed),
            }
        };
        self.files.close(handle);
        read.map(|()| Some(bytes))
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        let path = self.path(file, b"");
        self.files.facts(&path).map(stamp_of)
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        self.make_folders(file);
        let path = self.temporary_path(file);
        let handle = self.create_temporary(file, &path).map_err(store_error)?;
        let written =
            self.files
                .write_all(handle, bytes)
                .and_then(|()| match self.files.sync(handle) {
                    Err(FileError::Unsupported) => Ok(()),
                    synced => synced,
                });
        self.files.close(handle);
        if written.is_err() {
            let _ = self.files.remove(&path);
        }
        written.map_err(store_error)
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let from = self.temporary_path(file);
        let to = self.path(file, b"");
        let renamed = self.files.rename(&from, &to);
        if renamed.is_err() {
            let _ = self.files.remove(&from);
        }
        renamed.map_err(store_error)
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let from = self.path(file, b"");
        let to = self.path(file, DAMAGED_SUFFIX.as_bytes());
        match self.files.rename(&from, &to) {
            Ok(()) | Err(FileError::NotFound) => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }

    fn lock(&mut self, file: HiveFile) -> Result<(), StoreError> {
        if self.locks[slot(file)].is_some() {
            return Ok(());
        }
        self.make_folders(file);
        let path = self.path(file, LOCK_SUFFIX.as_bytes());
        let mut tries = 0;
        let handle = loop {
            match self.files.lock(&path, hive_mode(file)) {
                Err(FileError::Interrupted) if tries < MAX_INTERRUPTED_TRIES => tries += 1,
                locked => break locked.map_err(store_error)?,
            }
        };
        self.locks[slot(file)] = Some(handle);
        Ok(())
    }

    fn unlock(&mut self, file: HiveFile) {
        if let Some(handle) = self.locks[slot(file)].take() {
            self.files.close(handle);
        }
    }

    fn user_id(&mut self) -> u32 {
        self.files.user_id()
    }

    fn now(&mut self) -> u64 {
        self.files.now()
    }
}
