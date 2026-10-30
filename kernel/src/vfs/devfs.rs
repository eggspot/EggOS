//! The device files (`/dev`, D127, D168): a read-only directory of character devices and links,
//! mounted on `/dev` at boot when the root file system has that directory. Opening a device
//! asks it for an open file description ([`crate::linux::files::openat`]).
//!
//! It holds Linux's memory devices `null`, `zero`, `full`, `random` and `urandom`
//! ([`crate::linux::memory_devices`]), `console` and `tty` (the console, and the caller's
//! controlling terminal, which is the console or none), the links `fd`, `stdin`, `stdout` and
//! `stderr` into `/proc/self/fd`, `window`, the window server's device (docs/windows.md), and
//! `agent`, the agent channel (D136, docs/mcp.md), only while a virtio console gave the kernel
//! one, the directory `shm`, which the boot mounts a memory file system on for POSIX shared
//! memory (`shm_open`, D169), and the pseudo-terminals (D187): `ptmx`, which makes one, and the
//! directory `pts`, which lists the program side of each one whose controlling side is open
//! ([`crate::linux::pseudo_terminal`]), and `__properties__`, Android's property area: a
//! read-only regular file of the kernel's property service ([`crate::linux::properties`], D188),
//! `android_log`, the Android log, a read-only regular file of records, and the directory
//! `socket` with `logdw`, the socket Android programs log to ([`crate::linux::android_log`]).
//! Each device has Linux's number and mode.

use super::{
    Attributes, DirectoryEntry, FileSystem, Name, Node, NodeKind, StorageError, Usage, magic,
};
use crate::linux::{android_log, properties, pseudo_terminal};
use alloc::vec::Vec;
use core::fmt::Write;

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

/// `/dev/ptmx`'s node.
pub const PTMX: u64 = 20;

/// `/dev/ptmx`'s device number: a new pseudo-terminal's controlling side.
pub const PTMX_DEVICE: (u32, u32) = (5, 2);

/// The major device number of `/dev/pts/<n>`, whose minor is `n`.
pub const PSEUDO_TERMINAL_MAJOR: u32 = 136;

const PTS: u64 = 21;

const FIRST_PSEUDO_TERMINAL: u64 = 0x100;

/// The node of `/dev/pts/<number>`.
pub const fn pseudo_terminal_node(number: u32) -> u64 {
    FIRST_PSEUDO_TERMINAL + number as u64
}

fn pseudo_terminal_of(node: u64) -> Option<u32> {
    let number = node.checked_sub(FIRST_PSEUDO_TERMINAL)?;
    (number < u64::from(pseudo_terminal::MAX_PSEUDO_TERMINALS)).then_some(number as u32)
}

fn pseudo_terminal_name(number: u32) -> Name {
    let mut text = NameText::default();
    let _ = write!(text, "{number}");
    Name::new(&text.bytes[..text.length])
}

#[derive(Default)]
struct NameText {
    bytes: [u8; 10],
    length: usize,
}

impl Write for NameText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let room = self
            .bytes
            .get_mut(self.length..end)
            .ok_or(core::fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

fn parse_number(name: &[u8]) -> Option<u32> {
    if name.is_empty() || name.len() > 9 || (name.len() > 1 && name[0] == b'0') {
        return None;
    }
    name.iter().try_fold(0u32, |number, &digit| {
        digit
            .is_ascii_digit()
            .then(|| number * 10 + u32::from(digit - b'0'))
    })
}

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

const PROPERTIES: u64 = 22;

const SOCKETS: u64 = 23;

const LOG_WRITER: u64 = 24;

const ANDROID_LOG: u64 = 25;

const DEVICE: (u32, u32) = (0, 5);

const FILES: [(&[u8], u64); 19] = [
    (eggos_android_properties::AREA_NAME, PROPERTIES),
    (b"agent", AGENT),
    (eggos_android_log::LOG_NAME, ANDROID_LOG),
    (b"console", CONSOLE),
    (b"fd", FD),
    (b"full", FULL),
    (b"null", NULL),
    (b"ptmx", PTMX),
    (b"pts", PTS),
    (b"random", RANDOM),
    (b"shm", SHM),
    (eggos_android_log::SOCKETS_NAME, SOCKETS),
    (b"stderr", STDERR),
    (b"stdin", STDIN),
    (b"stdout", STDOUT),
    (b"tty", TTY),
    (b"urandom", URANDOM),
    (b"window", WINDOW),
    (b"zero", ZERO),
];

const LINKS: [(u64, &[u8]); 4] = [
    (FD, b"/proc/self/fd"),
    (STDIN, b"/proc/self/fd/0"),
    (STDOUT, b"/proc/self/fd/1"),
    (STDERR, b"/proc/self/fd/2"),
];

/// Whether `node` is `/dev/__properties__`, the property area.
pub fn is_property_area(node: Node) -> bool {
    node.file_system().device() == DEVICE && node.id() == PROPERTIES
}

/// Whether `node` is `/dev/socket/logdw`, Android's log socket, which the kernel takes
/// datagrams on itself.
pub fn is_log_writer(node: Node) -> bool {
    node.file_system().device() == DEVICE && node.id() == LOG_WRITER
}

/// Whether `node` is `/dev/android_log`, the Android log.
pub fn is_android_log(node: Node) -> bool {
    node.file_system().device() == DEVICE && node.id() == ANDROID_LOG
}

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
        ROOT | SHM | PTS | SOCKETS => NodeKind::Directory,
        PROPERTIES | ANDROID_LOG => NodeKind::Regular,
        LOG_WRITER => NodeKind::Socket,
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

    fn usage(&self) -> Usage {
        Usage::without_blocks(magic::TMPFS, true)
    }

    fn kind(&self, node: u64) -> NodeKind {
        kind_of(node)
    }

    fn attributes(&self, node: u64) -> Attributes {
        let directory = kind_of(node) == NodeKind::Directory;
        Attributes {
            kind: self.kind(node),
            permissions: match node {
                ROOT | PTS | SOCKETS => 0o755,
                SHM => 0o1777,
                PROPERTIES | ANDROID_LOG => 0o444,
                LOG_WRITER => 0o222,
                AGENT | CONSOLE => 0o600,
                _ if link_of(node).is_some() => 0o777,
                _ if pseudo_terminal_of(node).is_some() => 0o620,
                _ => 0o666,
            },
            file_id: node,
            links: if directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: match node {
                PROPERTIES => properties::size(),
                ANDROID_LOG => android_log::size(),
                _ => 0,
            },
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        match directory {
            ROOT => listed()
                .find(|(file, _)| *file == name)
                .map(|&(_, node)| node),
            PTS => parse_number(name)
                .filter(|&number| pseudo_terminal::is_listed(number))
                .map(pseudo_terminal_node),
            SOCKETS if name == eggos_android_log::WRITER_NAME => Some(LOG_WRITER),
            _ => None,
        }
    }

    fn parent(&self, node: u64) -> u64 {
        match (node, pseudo_terminal_of(node)) {
            (_, Some(_)) => PTS,
            (LOG_WRITER, None) => SOCKETS,
            (_, None) => ROOT,
        }
    }

    fn name(&self, node: u64) -> Name {
        if let Some(number) = pseudo_terminal_of(node) {
            return pseudo_terminal_name(number);
        }
        if node == LOG_WRITER {
            return Name::new(eggos_android_log::WRITER_NAME);
        }
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

    fn copies_contents(&self) -> bool {
        true
    }

    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
        Ok(match node {
            PROPERTIES => properties::read_into(offset, buffer),
            ANDROID_LOG => android_log::read_into(offset, buffer),
            _ => 0,
        })
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
            PTMX => Some(PTMX_DEVICE),
            _ => pseudo_terminal_of(node).map(|number| (PSEUDO_TERMINAL_MAJOR, number)),
        }
    }

    fn link_target(&self, node: u64, _caller: u32) -> Option<Vec<u8>> {
        link_of(node).map(<[u8]>::to_vec)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let position = usize::try_from(position).ok()?;
        match directory {
            ROOT => {
                let &(name, node) = listed().nth(position)?;
                Some(DirectoryEntry {
                    name: Name::new(name),
                    node,
                    kind: kind_of(node),
                })
            }
            PTS => {
                let number = pseudo_terminal::listed(position)?;
                Some(DirectoryEntry {
                    name: pseudo_terminal_name(number),
                    node: pseudo_terminal_node(number),
                    kind: NodeKind::CharacterDevice,
                })
            }
            SOCKETS if position == 0 => Some(DirectoryEntry {
                name: Name::new(eggos_android_log::WRITER_NAME),
                node: LOG_WRITER,
                kind: NodeKind::Socket,
            }),
            _ => None,
        }
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
                &b"__properties__"[..],
                b"android_log",
                b"console",
                b"fd",
                b"full",
                b"null",
                b"ptmx",
                b"pts",
                b"random",
                b"shm",
                b"socket",
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
            (b"ptmx", (5, 2), 0o666),
        ] {
            let node = files.lookup(ROOT, name).unwrap();
            assert_eq!(files.character_device(node), Some(device));
            assert_eq!(files.kind(node), NodeKind::CharacterDevice);
            assert_eq!(files.attributes(node).permissions, permissions);
            assert_eq!(files.name(node).as_bytes(), name);
        }
    }

    #[test]
    fn the_property_area_is_a_read_only_regular_file_of_root_s() {
        let files = DeviceFiles;
        let node = files.lookup(ROOT, b"__properties__").unwrap();
        assert_eq!(files.kind(node), NodeKind::Regular);
        assert_eq!(files.character_device(node), None);
        let attributes = files.attributes(node);
        assert_eq!(
            (attributes.permissions, attributes.owner, attributes.group),
            (0o444, 0, 0)
        );
        assert!(attributes.size >= eggos_android_properties::SMALLEST_AREA as u64);
        let mut header = [0; 16];
        assert_eq!(files.read_into(node, 0, &mut header).ok(), Some(16));
        assert_eq!(
            &header[8..12],
            &eggos_android_properties::MAGIC.to_ne_bytes()
        );
    }

    #[test]
    fn the_android_log_is_a_read_only_file_and_its_socket_is_in_the_socket_folder() {
        let files = DeviceFiles;
        let log = files.lookup(ROOT, b"android_log").unwrap();
        assert_eq!(files.kind(log), NodeKind::Regular);
        assert_eq!(files.attributes(log).permissions, 0o444);
        let sockets = files.lookup(ROOT, b"socket").unwrap();
        assert_eq!(files.kind(sockets), NodeKind::Directory);
        let writer = files.lookup(sockets, b"logdw").unwrap();
        assert_eq!(files.kind(writer), NodeKind::Socket);
        assert_eq!(files.attributes(writer).permissions, 0o222);
        assert_eq!(files.parent(writer), sockets);
        assert_eq!(files.name(writer).as_bytes(), b"logdw");
        assert_eq!(files.lookup(sockets, b"logdr"), None);
        let entry = files.entry(sockets, 0).unwrap();
        assert_eq!((entry.name.as_bytes(), entry.node), (&b"logdw"[..], writer));
        assert!(files.entry(sockets, 1).is_none());
    }

    #[test]
    fn pts_lists_numbers_by_name_and_finds_nothing_else() {
        let files = DeviceFiles;
        let pts = files.lookup(ROOT, b"pts").unwrap();
        assert_eq!(files.kind(pts), NodeKind::Directory);
        assert_eq!(files.attributes(pts).permissions, 0o755);
        for name in [&b"07"[..], b"x", b"", b"4294967296", b"64"] {
            assert_eq!(files.lookup(pts, name), None, "{name:?}");
        }
        assert_eq!(parse_number(b"0"), Some(0));
        assert_eq!(parse_number(b"63"), Some(63));
        assert_eq!(parse_number(b"6a"), None);
        let node = pseudo_terminal_node(12);
        assert_eq!(files.name(node).as_bytes(), b"12");
        assert_eq!(files.parent(node), pts);
        assert_eq!(files.kind(node), NodeKind::CharacterDevice);
        assert_eq!(files.character_device(node), Some((136, 12)));
        assert_eq!(files.attributes(node).permissions, 0o620);
        assert_eq!(pseudo_terminal_of(pseudo_terminal_node(64)), None);
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
        let entry = files.entry(ROOT, 1).unwrap();
        assert_eq!((entry.name.as_bytes(), entry.node), (&b"agent"[..], AGENT));
        assert_eq!(files.character_device(AGENT), Some(AGENT_DEVICE));
        assert_eq!(files.attributes(AGENT).permissions, 0o600);
        assert_eq!(files.name(AGENT).as_bytes(), b"agent");
        crate::agent_channel::uninstall();
    }
}
