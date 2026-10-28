//! A file system in memory behind [`Files`], for the host tests and for the QEMU test's
//! expected frames: folders, files with their times, several devices (a rename between two is
//! `EXDEV`), read-only places (`EROFS`) and a size limit (`ENOSPC`).

use eggos_app::files::{Files, Handle, Kind, Status};
use eggos_runtime::errno::Errno;
use std::collections::BTreeMap;
use std::vec::Vec;

/// One node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Folder,
    File(Vec<u8>),
    Other,
}

#[derive(Debug, Clone)]
struct Held {
    node: Node,
    permissions: u32,
    modified: u64,
}

#[derive(Debug, Clone)]
struct Open {
    path: Vec<u8>,
    offset: usize,
    writing: bool,
}

/// The file system. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct MemoryFiles {
    nodes: BTreeMap<Vec<u8>, Held>,
    devices: Vec<(Vec<u8>, u64)>,
    read_only: Vec<Vec<u8>>,
    open: Vec<Option<Open>>,
    /// The time new and changed nodes get.
    pub now: u64,
    /// The most file bytes it holds, if limited.
    pub space: Option<usize>,
    /// How many times [`Files::sync`] was called.
    pub syncs: usize,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(slash) => &path[..slash],
        None => b"",
    }
}

fn within(path: &[u8], folder: &[u8]) -> bool {
    folder == b"/"
        || path == folder
        || (path.starts_with(folder) && path.get(folder.len()) == Some(&b'/'))
}

impl MemoryFiles {
    /// An empty file system with only `/`.
    pub fn new() -> Self {
        let mut files = Self::default();
        files.nodes.insert(
            b"/".to_vec(),
            Held {
                node: Node::Folder,
                permissions: 0o755,
                modified: 0,
            },
        );
        files
    }

    /// Adds the folder `path` (and the folders above it).
    pub fn folder(&mut self, path: &str) -> &mut Self {
        let path = path.as_bytes();
        let mut end = 0;
        while end < path.len() {
            end = path[end + 1..]
                .iter()
                .position(|&byte| byte == b'/')
                .map_or(path.len(), |at| end + 1 + at);
            self.nodes.entry(path[..end].to_vec()).or_insert(Held {
                node: Node::Folder,
                permissions: 0o755,
                modified: self.now,
            });
        }
        self
    }

    /// Adds the file `path` holding `bytes`, changed at `modified` (and the folders above it).
    pub fn file(&mut self, path: &str, bytes: &[u8], modified: u64) -> &mut Self {
        let folder = String::from_utf8_lossy(parent(path.as_bytes())).into_owned();
        self.folder(&folder);
        self.nodes.insert(
            path.as_bytes().to_vec(),
            Held {
                node: Node::File(bytes.to_vec()),
                permissions: 0o644,
                modified,
            },
        );
        self
    }

    /// Adds the file at `path`, any bytes, holding `bytes`; its folder must exist.
    pub fn file_at(&mut self, path: &[u8], bytes: &[u8], modified: u64) -> &mut Self {
        self.nodes.insert(
            path.to_vec(),
            Held {
                node: Node::File(bytes.to_vec()),
                permissions: 0o644,
                modified,
            },
        );
        self
    }

    /// Adds `path` as a device or a link: neither a file nor a folder.
    pub fn other(&mut self, path: &str) -> &mut Self {
        self.nodes.insert(
            path.as_bytes().to_vec(),
            Held {
                node: Node::Other,
                permissions: 0o777,
                modified: 0,
            },
        );
        self
    }

    /// Makes everything in `folder` another file system, numbered `device`.
    pub fn device(&mut self, folder: &str, device: u64) -> &mut Self {
        self.devices.push((folder.as_bytes().to_vec(), device));
        self
    }

    /// Makes everything in `folder` read-only.
    pub fn read_only(&mut self, folder: &str) -> &mut Self {
        self.read_only.push(folder.as_bytes().to_vec());
        self
    }

    /// The node at `path`.
    pub fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path.as_bytes()).map(|held| &held.node)
    }

    /// The contents of the file at `path`.
    pub fn contents(&self, path: &str) -> Option<&[u8]> {
        match self.node(path) {
            Some(Node::File(bytes)) => Some(bytes),
            _ => None,
        }
    }

    /// Every path under `folder`, in order, `folder` left out.
    pub fn paths_under(&self, folder: &str) -> Vec<String> {
        self.nodes
            .keys()
            .filter(|path| within(path, folder.as_bytes()) && path.as_slice() != folder.as_bytes())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect()
    }

    /// How many files are open.
    pub fn open_count(&self) -> usize {
        self.open.iter().filter(|open| open.is_some()).count()
    }

    fn device_of(&self, path: &[u8]) -> u64 {
        self.devices
            .iter()
            .filter(|(folder, _)| within(path, folder))
            .max_by_key(|(folder, _)| folder.len())
            .map_or(0, |(_, device)| *device)
    }

    fn writable(&self, path: &[u8]) -> Result<(), Errno> {
        match self.read_only.iter().any(|folder| within(path, folder)) {
            true => Err(Errno::EROFS),
            false => Ok(()),
        }
    }

    fn used(&self) -> usize {
        self.nodes
            .values()
            .map(|held| match &held.node {
                Node::File(bytes) => bytes.len(),
                _ => 0,
            })
            .sum()
    }

    fn children(&self, folder: &[u8]) -> Vec<Vec<u8>> {
        self.nodes
            .keys()
            .filter(|path| path.as_slice() != b"/" && parent(path) == folder)
            .cloned()
            .collect()
    }

    fn add(&mut self, path: &[u8], node: Node, permissions: u32) -> Result<(), Errno> {
        self.writable(path)?;
        if self.nodes.contains_key(path) {
            return Err(Errno::EEXIST);
        }
        match self.nodes.get(parent(path)).map(|held| &held.node) {
            Some(Node::Folder) => {}
            Some(_) => return Err(Errno::ENOTDIR),
            None => return Err(Errno::ENOENT),
        }
        self.nodes.insert(
            path.to_vec(),
            Held {
                node,
                permissions,
                modified: self.now,
            },
        );
        Ok(())
    }

    fn file_of(&self, handle: Handle, writing: bool) -> Result<Open, Errno> {
        match self.open.get(handle) {
            Some(Some(open)) if open.writing == writing => Ok(open.clone()),
            _ => Err(Errno(9)),
        }
    }
}

impl Files for MemoryFiles {
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno> {
        let held = self.nodes.get(path).ok_or(Errno::ENOENT)?;
        let (kind, size) = match &held.node {
            Node::Folder => (Kind::Directory, 0),
            Node::File(bytes) => (Kind::File, bytes.len() as u64),
            Node::Other => (Kind::Other, 0),
        };
        Ok(Status {
            kind,
            size,
            permissions: held.permissions,
            modified: held.modified,
            device: self.device_of(path),
        })
    }

    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno> {
        match self.nodes.get(directory).map(|held| &held.node) {
            Some(Node::Folder) => {}
            Some(_) => return Err(Errno::ENOTDIR),
            None => return Err(Errno::ENOENT),
        }
        Ok(self.children(directory).get(index).map(|path| {
            let file = &path[parent(path).len() + usize::from(parent(path) != b"/")..];
            name[..file.len()].copy_from_slice(file);
            file.len()
        }))
    }

    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno> {
        match self.nodes.get(path).map(|held| &held.node) {
            Some(Node::File(_)) => {}
            Some(_) => return Err(Errno::EISDIR),
            None => return Err(Errno::ENOENT),
        }
        self.open.push(Some(Open {
            path: path.to_vec(),
            offset: 0,
            writing: false,
        }));
        Ok(self.open.len() - 1)
    }

    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno> {
        self.add(path, Node::File(Vec::new()), permissions)?;
        self.open.push(Some(Open {
            path: path.to_vec(),
            offset: 0,
            writing: true,
        }));
        Ok(self.open.len() - 1)
    }

    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno> {
        let open = self.file_of(file, false)?;
        let Some(Node::File(bytes)) = self.nodes.get(&open.path).map(|held| &held.node) else {
            return Err(Errno::EIO);
        };
        let count = bytes.len().saturating_sub(open.offset).min(buffer.len());
        buffer[..count].copy_from_slice(&bytes[open.offset..open.offset + count]);
        if let Some(Some(open)) = self.open.get_mut(file) {
            open.offset += count;
        }
        Ok(count)
    }

    fn write(&mut self, file: Handle, bytes: &[u8]) -> Result<(), Errno> {
        let open = self.file_of(file, true)?;
        if self
            .space
            .is_some_and(|space| self.used() + bytes.len() > space)
        {
            return Err(Errno::ENOSPC);
        }
        let now = self.now;
        if let Some(held) = self.nodes.get_mut(&open.path)
            && let Node::File(contents) = &mut held.node
        {
            contents.extend_from_slice(bytes);
            held.modified = now;
        }
        Ok(())
    }

    fn sync_file(&mut self, file: Handle) -> Result<(), Errno> {
        match self.open.get(file) {
            Some(Some(_)) => Ok(()),
            _ => Err(Errno(9)),
        }
    }

    fn close(&mut self, file: Handle) {
        assert!(
            self.open.get(file).is_some_and(Option::is_some),
            "closed twice"
        );
        self.open[file] = None;
    }

    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.add(path, Node::Folder, 0o755)
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.writable(path)?;
        match self.nodes.get(path).map(|held| &held.node) {
            None => Err(Errno::ENOENT),
            Some(Node::Folder) => Err(Errno::EISDIR),
            Some(_) => {
                self.nodes.remove(path);
                Ok(())
            }
        }
    }

    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.writable(path)?;
        match self.nodes.get(path).map(|held| &held.node) {
            None => Err(Errno::ENOENT),
            Some(Node::Folder) if path == b"/" => Err(Errno(16)),
            Some(Node::Folder) if !self.children(path).is_empty() => Err(Errno::ENOTEMPTY),
            Some(Node::Folder) => {
                self.nodes.remove(path);
                Ok(())
            }
            Some(_) => Err(Errno::ENOTDIR),
        }
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno> {
        self.writable(from)?;
        self.writable(to)?;
        let moved = self.nodes.get(from).cloned().ok_or(Errno::ENOENT)?;
        if self.device_of(from) != self.device_of(to) {
            return Err(Errno::EXDEV);
        }
        if from == to {
            return Ok(());
        }
        if moved.node == Node::Folder && within(to, from) {
            return Err(Errno::EINVAL);
        }
        match self.nodes.get(parent(to)).map(|held| &held.node) {
            Some(Node::Folder) => {}
            Some(_) => return Err(Errno::ENOTDIR),
            None => return Err(Errno::ENOENT),
        }
        match (self.nodes.get(to).map(|held| &held.node), &moved.node) {
            (Some(Node::Folder), Node::Folder) if !self.children(to).is_empty() => {
                return Err(Errno::ENOTEMPTY);
            }
            (Some(Node::Folder), Node::File(_) | Node::Other) => return Err(Errno::EISDIR),
            (Some(Node::File(_) | Node::Other), Node::Folder) => return Err(Errno::ENOTDIR),
            _ => {}
        }
        let inside: Vec<Vec<u8>> = self
            .nodes
            .keys()
            .filter(|path| within(path, from))
            .cloned()
            .collect();
        for path in inside {
            if let Some(held) = self.nodes.remove(&path) {
                self.nodes.insert([to, &path[from.len()..]].concat(), held);
            }
        }
        Ok(())
    }

    fn sync(&mut self) {
        self.syncs += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_move_whole_folders_and_refuse_other_devices() {
        let mut files = MemoryFiles::new();
        files
            .file("/home/root/a/x", b"x", 5)
            .folder("/tmp")
            .device("/tmp", 2);
        files.rename(b"/home/root/a", b"/home/root/b").unwrap();
        assert_eq!(files.contents("/home/root/b/x"), Some(&b"x"[..]));
        assert_eq!(files.node("/home/root/a"), None);
        assert_eq!(files.rename(b"/home/root/b", b"/tmp/b"), Err(Errno::EXDEV));
        assert_eq!(
            files.rename(b"/home/root/b", b"/home/root/b/c"),
            Err(Errno::EINVAL)
        );
        let mut name = [0u8; 256];
        assert_eq!(files.entry(b"/", 0, &mut name), Ok(Some(4)));
        assert_eq!(&name[..4], b"home");
        assert_eq!(files.status(b"/tmp").unwrap().device, 2);
    }
}
