//! The file system as the installer uses it: a few calls over byte paths, made with system
//! calls by the program and by a fake in the host tests, and the fixed path buffer the
//! installer builds paths in.

use eggos_runtime::errno::Errno;

/// The longest path the installer builds, in bytes.
pub const PATH_CAPACITY: usize = 512;

/// What a directory entry or a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// A symbolic link (D170): [`Files::read_link`] reads its target.
    Link,
    /// A device, a socket or anything else the installer does not copy.
    Other,
}

/// What [`Files::status`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub kind: Kind,
    pub size: u64,
    /// The permission bits (`0o755`).
    pub permissions: u32,
    /// When its contents last changed, in seconds since the epoch.
    pub modified: u64,
    /// The device of the file system it is on, its major number in the high 32 bits.
    pub device: u64,
}

/// An open file.
pub type Handle = usize;

/// The calls the installer makes. Paths are absolute byte strings without a NUL.
pub trait Files {
    /// What `path` is, without following a symbolic link at its end.
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno>;
    /// What `path` names, following symbolic links to the end (`stat`): a link's target's
    /// status; `ENOENT` for a link that names nothing, `ELOOP` for one that loops.
    fn status_following(&mut self, path: &[u8]) -> Result<Status, Errno>;
    /// Copies the target of symbolic link `path` into `target` and returns its length:
    /// `EINVAL` for anything but a link, `ENAMETOOLONG` for a target that does not fit.
    fn read_link(&mut self, path: &[u8], target: &mut [u8]) -> Result<usize, Errno>;
    /// Makes `path`, which must not exist, a symbolic link to `target` (`symlink`).
    fn make_link(&mut self, target: &[u8], path: &[u8]) -> Result<(), Errno>;
    /// The name of entry `index` of directory `directory`, `.` and `..` left out, copied into
    /// `name`; `None` past the last. Entries keep their order while the directory is unchanged.
    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno>;
    /// Opens `path` for reading.
    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno>;
    /// Makes `path`, which must not exist, and opens it for writing.
    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno>;
    /// Reads into `buffer`; 0 at the end.
    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno>;
    /// Reads into `buffer` from byte `offset` of the file, leaving its position as it is; 0 at
    /// or past the end. A file system that cannot says `EOPNOTSUPP`, so archives and icons are
    /// not read from it.
    fn read_at(&mut self, file: Handle, offset: u64, buffer: &mut [u8]) -> Result<usize, Errno> {
        let _ = (file, offset, buffer);
        Err(Errno::EOPNOTSUPP)
    }
    /// Writes all of `bytes`.
    fn write(&mut self, file: Handle, bytes: &[u8]) -> Result<(), Errno>;
    /// Writes the file's data to its disk.
    fn sync_file(&mut self, file: Handle) -> Result<(), Errno>;
    fn close(&mut self, file: Handle);
    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno>;
    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno>;
    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno>;
    /// Renames `from` to `to`, replacing a file there.
    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno>;
    /// Commits every file system's changes to its disk.
    fn sync(&mut self);
}

/// A path of at most [`PATH_CAPACITY`] bytes.
#[derive(Clone, Copy)]
pub struct Path {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}

impl core::fmt::Debug for Path {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.as_bytes().escape_ascii(), formatter)
    }
}

impl PartialEq for Path {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Path {}

impl Default for Path {
    fn default() -> Self {
        Self::new()
    }
}

impl Path {
    /// The empty path.
    pub const fn new() -> Self {
        Self {
            bytes: [0; PATH_CAPACITY],
            length: 0,
        }
    }

    /// `bytes` as a path; `None` when it is too long.
    pub fn from(bytes: &[u8]) -> Option<Self> {
        let mut path = Self::new();
        path.append(bytes).then_some(path)
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Adds `bytes` at the end; `false`, and nothing added, when they do not fit.
    pub fn append(&mut self, bytes: &[u8]) -> bool {
        let Some(end) = self
            .length
            .checked_add(bytes.len())
            .filter(|&end| end <= PATH_CAPACITY)
        else {
            return false;
        };
        self.bytes[self.length..end].copy_from_slice(bytes);
        self.length = end;
        true
    }

    /// Adds `/` and `name` (just `name` to an empty path); `false`, and nothing added, when
    /// they do not fit.
    pub fn push(&mut self, name: &[u8]) -> bool {
        let before = self.length;
        let fits = (self.is_empty() || self.append(b"/")) && self.append(name);
        if !fits {
            self.length = before;
        }
        fits
    }

    /// The path with `/` and `name` added; `None` when it does not fit.
    pub fn join(&self, name: &[u8]) -> Option<Self> {
        let mut joined = *self;
        joined.push(name).then_some(joined)
    }

    /// Drops the last component and the `/` before it.
    pub fn pop(&mut self) {
        self.length = self
            .as_bytes()
            .iter()
            .rposition(|&byte| byte == b'/')
            .unwrap_or(0);
    }

    /// The last component.
    pub fn file_name(&self) -> &[u8] {
        file_name(self.as_bytes())
    }
}

/// The last component of `path`, trailing slashes ignored.
pub fn file_name(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    let path = &path[..end];
    let start = path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    &path[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_grow_and_shrink_by_components_and_refuse_what_does_not_fit() {
        let mut path = Path::from(b"/Applications").unwrap();
        assert!(path.push(b"Paint"));
        assert!(path.push(b"Paint.app"));
        assert_eq!(path.as_bytes(), b"/Applications/Paint/Paint.app");
        assert_eq!(path.file_name(), b"Paint.app");
        path.pop();
        assert_eq!(path.as_bytes(), b"/Applications/Paint");
        assert_eq!(path, Path::from(b"/Applications/Paint").unwrap());
        let mut relative = Path::new();
        assert!(relative.push(b"Contents"));
        assert_eq!(relative.as_bytes(), b"Contents");
        relative.pop();
        assert!(relative.is_empty());
        assert!(!path.push(&[b'x'; PATH_CAPACITY]));
        assert_eq!(path.as_bytes(), b"/Applications/Paint");
        assert!(Path::from(&[b'x'; PATH_CAPACITY + 1]).is_none());
        assert_eq!(file_name(b"/media/host/Paint.app/"), b"Paint.app");
        assert_eq!(file_name(b"tool"), b"tool");
    }
}
