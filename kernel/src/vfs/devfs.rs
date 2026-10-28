//! The device files (`/dev`, D127): a read-only directory of character devices, mounted on
//! `/dev` at boot when the root file system has that directory. Opening one asks the device
//! for an open file description ([`crate::linux::files::openat`]). It holds `window`, the
//! window server's device (docs/windows.md), and `agent`, the agent channel (D136, docs/mcp.md),
//! only while a virtio console gave the kernel one.

use super::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind};

const ROOT: u64 = 1;

/// The window server's node, its inode number on the device files' file system.
pub const WINDOW: u64 = 6;

/// The window server's device number (major, minor), from Linux's range for local use.
pub const WINDOW_DEVICE: (u32, u32) = (240, 0);

/// The agent channel's node.
pub const AGENT: u64 = 7;

/// The agent channel's device number (major, minor), next to the window server's.
pub const AGENT_DEVICE: (u32, u32) = (240, 1);

const DEVICE: (u32, u32) = (0, 5);

const FILES: [(&[u8], u64); 2] = [(b"window", WINDOW), (b"agent", AGENT)];

fn is_listed(node: u64) -> bool {
    node != AGENT || crate::agent_channel::is_present()
}

fn listed() -> impl Iterator<Item = &'static (&'static [u8], u64)> {
    FILES.iter().filter(|(_, node)| is_listed(*node))
}

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
            permissions: match node {
                ROOT => 0o755,
                AGENT => 0o600,
                _ => 0o666,
            },
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
        listed()
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
        match node {
            WINDOW => Some(WINDOW_DEVICE),
            AGENT => Some(AGENT_DEVICE),
            _ => None,
        }
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        if directory != ROOT {
            return None;
        }
        let &(name, node) = listed().nth(usize::try_from(position).ok()?)?;
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
        let _guard = crate::agent_channel::tests_lock();
        crate::agent_channel::uninstall();
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

    #[test]
    fn the_agent_channel_is_listed_only_while_there_is_one() {
        let files = DeviceFiles;
        let _guard = crate::agent_channel::tests_lock();
        crate::agent_channel::uninstall();
        assert_eq!(files.lookup(ROOT, b"agent"), None);
        assert!(files.entry(ROOT, 1).is_none());
        crate::agent_channel::install_fake();
        assert_eq!(files.lookup(ROOT, b"agent"), Some(AGENT));
        let entry = files.entry(ROOT, 1).unwrap();
        assert_eq!((entry.name.as_bytes(), entry.node), (&b"agent"[..], AGENT));
        assert_eq!(files.character_device(AGENT), Some(AGENT_DEVICE));
        assert_eq!(files.attributes(AGENT).permissions, 0o600);
        assert_eq!(files.name(AGENT).as_bytes(), b"agent");
        crate::agent_channel::uninstall();
    }
}
