use crate::ls::record_names;
use crate::system::{DIRECTORY, FILE_TYPE, SYMBOLIC_LINK, SymbolicLinks, System};
use core::ffi::CStr;
use eggos_app::files::{Files, Handle, Kind, PATH_CAPACITY, Status};
use eggos_runtime::errno::Errno;

const REGULAR: u32 = 0o100_000;
const ENTRIES_CHUNK: usize = 2048;
const NEW_DIRECTORY_MODE: u32 = 0o777;

pub const ABSOLUTE_CAPACITY: usize = 1024;

pub fn absolute_path<S: System>(
    system: &mut S,
    path: &[u8],
    out: &mut [u8; ABSOLUTE_CAPACITY],
) -> Option<usize> {
    if path.first() == Some(&b'/') {
        out.get_mut(..path.len())?.copy_from_slice(path);
        return Some(path.len());
    }
    let mut length = system.working_directory(out).ok()?;
    if out[..length].last() != Some(&b'/') {
        *out.get_mut(length)? = b'/';
        length += 1;
    }
    out.get_mut(length..length + path.len())?
        .copy_from_slice(path);
    Some(length + path.len())
}

pub struct SystemFiles<'a, S: System> {
    system: &'a mut S,
}

struct CPath {
    bytes: [u8; PATH_CAPACITY + 1],
}

impl CPath {
    fn new(path: &[u8]) -> Result<Self, Errno> {
        if path.len() > PATH_CAPACITY || path.contains(&0) {
            return Err(Errno::ENAMETOOLONG);
        }
        let mut bytes = [0u8; PATH_CAPACITY + 1];
        bytes[..path.len()].copy_from_slice(path);
        Ok(Self { bytes })
    }

    fn as_c_str(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.bytes).unwrap_or(c"")
    }
}

impl<'a, S: System> SystemFiles<'a, S> {
    pub fn new(system: &'a mut S) -> Self {
        Self { system }
    }

    fn status_with(&mut self, path: &[u8], links: SymbolicLinks) -> Result<Status, Errno> {
        let path = CPath::new(path)?;
        let status = self.system.status(None, path.as_c_str(), links)?;
        Ok(files_status(&status))
    }
}

fn files_status(status: &crate::system::Status) -> Status {
    let kind = match status.mode & FILE_TYPE {
        DIRECTORY => Kind::Directory,
        REGULAR => Kind::File,
        SYMBOLIC_LINK => Kind::Link,
        _ => Kind::Other,
    };
    Status {
        kind,
        size: status.size,
        permissions: status.mode & 0o7777,
        modified: status.modified.max(0) as u64,
        device: 0,
    }
}

impl<S: System> Files for SystemFiles<'_, S> {
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno> {
        self.status_with(path, SymbolicLinks::DoNotFollow)
    }

    fn status_following(&mut self, path: &[u8]) -> Result<Status, Errno> {
        self.status_with(path, SymbolicLinks::Follow)
    }

    fn read_link(&mut self, path: &[u8], target: &mut [u8]) -> Result<usize, Errno> {
        let path = CPath::new(path)?;
        let length = self.system.read_link(path.as_c_str(), target)?;
        match length < target.len() {
            true => Ok(length),
            false => Err(Errno::ENAMETOOLONG),
        }
    }

    fn make_link(&mut self, target: &[u8], path: &[u8]) -> Result<(), Errno> {
        let (target, path) = (CPath::new(target)?, CPath::new(path)?);
        self.system.make_link(target.as_c_str(), path.as_c_str())
    }

    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno> {
        let path = CPath::new(directory)?;
        let descriptor = self.system.open(path.as_c_str())?;
        let mut seen = 0;
        let mut chunk = [0u8; ENTRIES_CHUNK];
        let found = loop {
            let length = match self.system.directory_entries(descriptor, &mut chunk) {
                Ok(0) => break Ok(None),
                Ok(length) => length,
                Err(error) => break Err(error),
            };
            let hit = record_names(&chunk[..length])
                .filter(|entry| !matches!(*entry, b"." | b".."))
                .find(|_| {
                    seen += 1;
                    seen > index
                });
            if let Some(entry) = hit {
                let length = entry.len().min(name.len());
                name[..length].copy_from_slice(&entry[..length]);
                break Ok(Some(length));
            }
        };
        self.system.close(descriptor);
        found
    }

    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno> {
        let path = CPath::new(path)?;
        self.system.open(path.as_c_str())
    }

    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno> {
        match self.status(path) {
            Ok(_) => return Err(Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(error) => return Err(error),
        }
        let path = CPath::new(path)?;
        self.system.create(path.as_c_str(), permissions, true)
    }

    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno> {
        loop {
            match self.system.read(file, buffer) {
                Err(Errno::EINTR) => {}
                other => return other,
            }
        }
    }

    fn write(&mut self, file: Handle, bytes: &[u8]) -> Result<(), Errno> {
        self.system.write(file, bytes)
    }

    fn sync_file(&mut self, _file: Handle) -> Result<(), Errno> {
        Ok(())
    }

    fn close(&mut self, file: Handle) {
        self.system.close(file);
    }

    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        let path = CPath::new(path)?;
        self.system
            .make_directory(path.as_c_str(), NEW_DIRECTORY_MODE)
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno> {
        let path = CPath::new(path)?;
        self.system.remove(path.as_c_str(), false)
    }

    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        let path = CPath::new(path)?;
        self.system.remove(path.as_c_str(), true)
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno> {
        let (from, to) = (CPath::new(from)?, CPath::new(to)?);
        self.system.rename(from.as_c_str(), to.as_c_str())
    }

    fn sync(&mut self) {
        self.system.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    #[test]
    fn the_utilities_system_serves_as_files_for_the_shared_logic() {
        let mut fake = Fake::default()
            .with_directory("/home", 0)
            .with_file("/home/a", 0o640, b"abc", 7)
            .with_directory("/home/d", 3);
        let mut files = SystemFiles::new(&mut fake);
        let status = files.status(b"/home/a").unwrap();
        assert_eq!(
            (status.kind, status.size, status.permissions),
            (Kind::File, 3, 0o640)
        );
        assert_eq!(status.modified, 7);
        assert_eq!(files.status(b"/home/d").unwrap().kind, Kind::Directory);
        let mut name = [0u8; 256];
        let mut names = Vec::new();
        let mut index = 0;
        while let Some(length) = files.entry(b"/home", index, &mut name).unwrap() {
            names.push(String::from_utf8_lossy(&name[..length]).into_owned());
            index += 1;
        }
        names.sort();
        assert_eq!(names, ["a", "d"]);
        assert_eq!(files.create(b"/home/a", 0o644), Err(Errno::EEXIST));
        let file = files.create(b"/home/b", 0o644).unwrap();
        files.write(file, b"xy").unwrap();
        files.close(file);
        assert_eq!(files.status(b"/home/b").unwrap().size, 2);
        files.rename(b"/home/b", b"/home/d/b").unwrap();
        files.remove_file(b"/home/d/b").unwrap();
        files.remove_directory(b"/home/d").unwrap();
        assert_eq!(files.status(b"/home/d"), Err(Errno::ENOENT));
    }

    #[test]
    fn links_are_seen_as_links_and_followed_only_when_asked() {
        let mut fake = Fake::default()
            .with_directory("/home", 0)
            .with_file("/home/a", 0o640, b"abc", 7)
            .with_link("/home/to-a", "a");
        let mut files = SystemFiles::new(&mut fake);
        let link = files.status(b"/home/to-a").unwrap();
        assert_eq!((link.kind, link.size), (Kind::Link, 1));
        let through = files.status_following(b"/home/to-a").unwrap();
        assert_eq!((through.kind, through.size), (Kind::File, 3));
        let mut target = [0u8; 8];
        assert_eq!(files.read_link(b"/home/to-a", &mut target), Ok(1));
        assert_eq!(&target[..1], b"a");
        assert_eq!(files.read_link(b"/home/a", &mut target), Err(Errno::EINVAL));
        assert_eq!(
            files.read_link(b"/home/to-a", &mut [0u8; 1]),
            Err(Errno::ENAMETOOLONG)
        );
        files.make_link(b"../home/a", b"/home/up").unwrap();
        assert_eq!(files.make_link(b"x", b"/home/up"), Err(Errno::EEXIST));
        assert_eq!(fake.link_target("/home/up"), Some(b"../home/a".to_vec()));
    }
}
