//! The device files (`/dev`, D127): a read-only directory of character devices, mounted on
//! `/dev` at boot when the root file system has that directory. Opening one asks the device
//! for an open file description ([`crate::linux::files::openat`]). For now it holds
//! `window`, the window server's device (docs/windows.md).

use super::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind};

const ROOT: u64 = 1;

/// The window server's node, its inode number on the device files' file system.
pub const WINDOW: u64 = 6;

/// The window server's device number (major, minor), from Linux's range for local use.
pub const WINDOW_DEVICE: (u32, u32) = (240, 0);

const DEVICE: (u32, u32) = (0, 5);

const FILES: [(&[u8], u64); 1] = [(b"window", WINDOW)];

/// The device files' file system.
#[derive(Debug, Default)]
pub struct DeviceFiles;

impl FileSystem for DeviceFiles {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn kind(&self, node: u64) -> NodeKind {
        match node {
            ROOT => NodeKind::Directory,
            _ => NodeKind::CharacterDevice,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let directory = node == ROOT;
        Attributes {
            kind: self.kind(node),
            permissions: if directory { 0o755 } else { 0o666 },
            file_id: node,
            links: if directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: 0,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        if directory != ROOT {
            return None;
        }
        FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|&(_, node)| node)
    }

    fn parent(&self, _node: u64) -> u64 {
        ROOT
    }

    fn name(&self, node: u64) -> Name {
        Name::new(
            FILES
                .iter()
                .find(|&&(_, file)| file == node)
                .map_or(&[][..], |(name, _)| name),
        )
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn character_device(&self, node: u64) -> Option<(u32, u32)> {
        (node == WINDOW).then_some(WINDOW_DEVICE)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        if directory != ROOT {
            return None;
        }
        let &(name, node) = FILES.get(usize::try_from(position).ok()?)?;
        Some(DirectoryEntry {
            name: Name::new(name),
            node,
            kind: NodeKind::CharacterDevice,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_lists_the_window_device_and_its_number() {
        let files = DeviceFiles;
        assert_eq!(files.lookup(ROOT, b"window"), Some(WINDOW));
        assert_eq!(files.lookup(ROOT, b"console"), None);
        assert_eq!(files.kind(WINDOW), NodeKind::CharacterDevice);
        assert_eq!(files.character_device(WINDOW), Some(WINDOW_DEVICE));
        assert_eq!(files.character_device(ROOT), None);
        assert_eq!(files.attributes(WINDOW).permissions, 0o666);
        let entry = files.entry(ROOT, 0).unwrap();
        assert_eq!(
            (entry.name.as_bytes(), entry.node),
            (&b"window"[..], WINDOW)
        );
        assert!(files.entry(ROOT, 1).is_none());
    }
}
