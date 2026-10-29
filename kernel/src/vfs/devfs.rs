//! The device files (`/dev`, D127, D168): a read-only directory of character devices and links,
//! mounted on `/dev` at boot when the root file system has that directory. Opening a device
//! asks it for an open file description ([`crate::linux::files::openat`]).
//!
//! It holds Linux's memory devices `null`, `zero`, `full`, `random` and `urandom`
//! ([`crate::linux::memory_devices`]), `console` and `tty` (the console, and the caller's
//! controlling terminal, which is the console or none), the links `fd`, `stdin`, `stdout` and
//! `stderr` into `/proc/self/fd`, `window`, the window server's device (docs/windows.md), and
//! `agent`, the agent channel (D136, docs/mcp.md), only while a virtio console gave the kernel
//! one, and the directory `shm`, which the boot mounts a memory file system on for POSIX shared
//! memory (`shm_open`, D169). Each device has Linux's number and mode.

use super::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind};
use alloc::vec::Vec;

const ROOT: u64 = 1;

/// The window server's node, its inode number on the device files' file system.
pub const WINDOW: u64 = 6;

/// The window server's device number (major, minor), from Linux's range for local use.
pub const WINDOW_DEVICE: (u32, u32) = (240, 0);

/// The agent channel's node.
pub const AGENT: u64 = 7;

/// The agent channel's device number (major, minor), next to the window server's.
pub const AGENT_DEVICE: (u32, u32) = (240, 1);

/// `/dev/null`'s device number.
pub const NULL_DEVICE: (u32, u32) = (1, 3);

/// `/dev/zero`'s device number.
pub const ZERO_DEVICE: (u32, u32) = (1, 5);

/// `/dev/full`'s device number.
pub const FULL_DEVICE: (u32, u32) = (1, 7);

/// `/dev/random`'s device number.
pub const RANDOM_DEVICE: (u32, u32) = (1, 8);

/// `/dev/urandom`'s device number.
pub const URANDOM_DEVICE: (u32, u32) = (1, 9);

/// `/dev/tty`'s device number: the caller's controlling terminal.
pub const TTY_DEVICE: (u32, u32) = (5, 0);

/// `/dev/console`'s device number.
pub const CONSOLE_DEVICE: (u32, u32) = (5, 1);

const NULL: u64 = 8;

const ZERO: u64 = 9;

const FULL: u64 = 10;

const RANDOM: u64 = 11;

const URANDOM: u64 = 12;

const TTY: u64 = 13;

const CONSOLE: u64 = 14;

const FD: u64 = 15;

const STDIN: u64 = 16;

const STDOUT: u64 = 17;

const STDERR: u64 = 18;

const SHM: u64 = 19;

const DEVICE: (u32, u32) = (0, 5);

const FILES: [(&[u8], u64); 14] = [
    (b"agent", AGENT),
    (b"console", CONSOLE),
    (b"fd", FD),
    (b"full", FULL),
    (b"null", NULL),
    (b"random", RANDOM),
    (b"shm", SHM),
    (b"stderr", STDERR),
    (b"stdin", STDIN),
    (b"stdout", STDOUT),
    (b"tty", TTY),
    (b"urandom", URANDOM),
    (b"window", WINDOW),
    (b"zero", ZERO),
];

/// Each link and its target.
const LINKS: [(u64, &[u8]); 4] = [
    (FD, b"/proc/self/fd"),
    (STDIN, b"/proc/self/fd/0"),
    (STDOUT, b"/proc/self/fd/1"),
    (STDERR, b"/proc/self/fd/2"),
];

fn is_listed(node: u64) -> bool {
    node != AGENT || crate::agent_channel::is_present()
}

fn listed() -> impl Iterator<Item = &'static (&'static [u8], u64)> {
    FILES.iter().filter(|(_, node)| is_listed(*node))
}

fn link_of(node: u64) -> Option<&'static [u8]> {
    LINKS
        .iter()
        .find(|&&(link, _)| link == node)
        .map(|&(_, target)| target)
}

fn kind_of(node: u64) -> NodeKind {
    match node {
        ROOT | SHM => NodeKind::Directory,
        _ if link_of(node).is_some() => NodeKind::SymbolicLink,
        _ => NodeKind::CharacterDevice,
    }
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
        kind_of(node)
    }

    fn attributes(&self, node: u64) -> Attributes {
        let directory = kind_of(node) == NodeKind::Directory;
        Attributes {
            kind: self.kind(node),
            permissions: match node {
                ROOT => 0o755,
                SHM => 0o1777,
                AGENT | CONSOLE => 0o600,
                _ if link_of(node).is_some() => 0o777,
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
            NULL => Some(NULL_DEVICE),
            ZERO => Some(ZERO_DEVICE),
            FULL => Some(FULL_DEVICE),
            RANDOM => Some(RANDOM_DEVICE),
            URANDOM => Some(URANDOM_DEVICE),
            TTY => Some(TTY_DEVICE),
            CONSOLE => Some(CONSOLE_DEVICE),
            _ => None,
        }
    }

    fn link_target(&self, node: u64, _caller: u32) -> Option<Vec<u8>> {
        link_of(node).map(<[u8]>::to_vec)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        if directory != ROOT {
            return None;
        }
        let &(name, node) = listed().nth(usize::try_from(position).ok()?)?;
        Some(DirectoryEntry {
            name: Name::new(name),
            node,
            kind: kind_of(node),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(files: &DeviceFiles) -> Vec<Vec<u8>> {
        (0..)
            .map_while(|position| files.entry(ROOT, position))
            .map(|entry| entry.name.as_bytes().to_vec())
            .collect()
    }

    #[test]
    fn the_directory_lists_the_window_device_and_its_number() {
        let files = DeviceFiles;
        let _guard = crate::agent_channel::tests_lock();
        crate::agent_channel::uninstall();
        assert_eq!(files.lookup(ROOT, b"window"), Some(WINDOW));
        assert_eq!(files.lookup(ROOT, b"ttyS0"), None);
        assert_eq!(files.kind(WINDOW), NodeKind::CharacterDevice);
        assert_eq!(files.character_device(WINDOW), Some(WINDOW_DEVICE));
        assert_eq!(files.character_device(ROOT), None);
        assert_eq!(files.attributes(WINDOW).permissions, 0o666);
        assert_eq!(
            names(&files),
            [
                &b"console"[..],
                b"fd",
                b"full",
                b"null",
                b"random",
                b"shm",
                b"stderr",
                b"stdin",
                b"stdout",
                b"tty",
                b"urandom",
                b"window",
                b"zero"
            ]
        );
    }

    #[test]
    fn the_memory_devices_and_terminals_have_linux_s_numbers_and_modes() {
        let files = DeviceFiles;
        for (name, device, permissions) in [
            (&b"null"[..], (1, 3), 0o666),
            (b"zero", (1, 5), 0o666),
            (b"full", (1, 7), 0o666),
            (b"random", (1, 8), 0o666),
            (b"urandom", (1, 9), 0o666),
            (b"tty", (5, 0), 0o666),
            (b"console", (5, 1), 0o600),
        ] {
            let node = files.lookup(ROOT, name).unwrap();
            assert_eq!(files.character_device(node), Some(device));
            assert_eq!(files.kind(node), NodeKind::CharacterDevice);
            assert_eq!(files.attributes(node).permissions, permissions);
            assert_eq!(files.name(node).as_bytes(), name);
        }
    }

    #[test]
    fn the_standard_streams_are_links_into_proc_self_fd() {
        let files = DeviceFiles;
        for (name, target) in [
            (&b"fd"[..], &b"/proc/self/fd"[..]),
            (b"stdin", b"/proc/self/fd/0"),
            (b"stdout", b"/proc/self/fd/1"),
            (b"stderr", b"/proc/self/fd/2"),
        ] {
            let node = files.lookup(ROOT, name).unwrap();
            assert_eq!(files.kind(node), NodeKind::SymbolicLink);
            assert_eq!(files.link_target(node, 7), Some(target.to_vec()));
            assert_eq!(files.character_device(node), None);
            assert_eq!(files.attributes(node).permissions, 0o777);
        }
        assert_eq!(files.link_target(NULL, 7), None);
    }

    #[test]
    fn the_agent_channel_is_listed_only_while_there_is_one() {
        let files = DeviceFiles;
        let _guard = crate::agent_channel::tests_lock();
        crate::agent_channel::uninstall();
        assert_eq!(files.lookup(ROOT, b"agent"), None);
        assert!(!names(&files).contains(&b"agent".to_vec()));
        crate::agent_channel::install_fake();
        assert_eq!(files.lookup(ROOT, b"agent"), Some(AGENT));
        let entry = files.entry(ROOT, 0).unwrap();
        assert_eq!((entry.name.as_bytes(), entry.node), (&b"agent"[..], AGENT));
        assert_eq!(files.character_device(AGENT), Some(AGENT_DEVICE));
        assert_eq!(files.attributes(AGENT).permissions, 0o600);
        assert_eq!(files.name(AGENT).as_bytes(), b"agent");
        crate::agent_channel::uninstall();
    }
}
