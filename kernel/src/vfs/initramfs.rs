//! The initramfs (D102): a read-only file system served straight from a `newc` cpio archive
//! ([`super::cpio`]) that `cargo xtask` builds from the repository's `initramfs/` directory and
//! embeds in the kernel image.
//!
//! Mounting reads the archive once and builds an index of its directories and regular files,
//! sorted by parent directory and name, so a directory's entries are contiguous and a lookup
//! is a binary search among them. File contents are never copied: a read returns a slice of
//! the archive. A symbolic link's member holds its target as its data (D170); members of other
//! types (devices, sockets) are skipped.
//!
//! Regular files that share an inode number and say they have more than one link are hard
//! links to one file (D108), as `cpio` and Linux's `gen_init_cpio` write them: the data is in
//! one of them (the last, by those writers), the others are empty, and every name reads that
//! data and reports the same file id, attributes and link count.
//!
//! Member paths may start with `./` or `/`; `.` (or an empty path) is the root itself. Every
//! member's parent directory must come earlier in the archive, components must be non-empty
//! names of at most [`super::NAME_MAX`] bytes other than `.` and `..`, and no path may appear
//! twice: an archive that breaks these rules is refused as a whole.

use super::cpio::{self, Members};
use super::{Attributes, DirectoryEntry, FileSystem, NAME_MAX, Name, NodeKind, PATH_MAX};
use alloc::vec::Vec;
use core::cmp::Ordering;

/// Why an archive cannot be mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// It does not follow the cpio format.
    Archive(cpio::Error),
    /// A member's path is not a valid name (empty component, `.`, `..`, too long).
    BadPath,
    /// A member's parent directory is not in the archive.
    MissingParent,
    /// Two members have the same path.
    Duplicate,
    /// The archive is larger than 4 GiB.
    TooLarge,
    /// There was no memory for the index.
    OutOfMemory,
}

/// The device number the initramfs reports (`st_dev`): an anonymous device, as Linux's rootfs
/// has.
pub const DEVICE: (u32, u32) = (0, 1);

const FILE_TYPE: u32 = 0o170000;
const DIRECTORY: u32 = 0o040000;
const REGULAR: u32 = 0o100000;
const SYMBOLIC_LINK: u32 = 0o120000;

#[derive(Debug, Clone, Copy)]
struct Entry {
    path_start: u32,
    path_length: u16,
    name_offset: u16,
    data_start: u32,
    size: u32,
    parent: u32,
    first_child: u32,
    end_of_children: u32,
    mode: u32,
    owner: u32,
    group: u32,
    modified: u32,
    links: u32,
    pending_link_inode: u32,
    file_id: u32,
}

/// A mounted initramfs: see the module documentation.
#[derive(Debug)]
pub struct Initramfs {
    archive: &'static [u8],
    entries: Vec<Entry>,
}

const ROOT: u32 = 0;

impl Initramfs {
    /// An initramfs with only an empty root directory.
    pub fn empty() -> Self {
        Self {
            archive: &[],
            entries: alloc::vec![Self::root_entry()],
        }
    }

    fn root_entry() -> Entry {
        Entry {
            path_start: 0,
            path_length: 0,
            name_offset: 0,
            data_start: 0,
            size: 0,
            parent: ROOT,
            first_child: 1,
            end_of_children: 1,
            mode: DIRECTORY | 0o755,
            owner: 0,
            group: 0,
            modified: 0,
            links: 2,
            pending_link_inode: 0,
            file_id: ROOT + 1,
        }
    }

    /// Mounts `archive`, refusing it as the module documentation says.
    pub fn parse(archive: &'static [u8]) -> Result<Self, Error> {
        if u32::try_from(archive.len()).is_err() {
            return Err(Error::TooLarge);
        }
        let mut entries = Vec::new();
        entries.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        entries.push(Self::root_entry());
        for member in Members::of(archive) {
            let member = member.map_err(Error::Archive)?;
            let kind = member.mode & FILE_TYPE;
            let path = normalized(member.name);
            let path_start = path.as_ptr() as usize - archive.as_ptr() as usize;
            if path.is_empty() {
                if kind != DIRECTORY {
                    return Err(Error::BadPath);
                }
                entries[0].mode = member.mode;
                entries[0].owner = member.owner;
                entries[0].group = member.group;
                entries[0].modified = member.modified;
                continue;
            }
            if !matches!(kind, DIRECTORY | REGULAR | SYMBOLIC_LINK) {
                continue;
            }
            if path.len() >= PATH_MAX
                || path.split(|&byte| byte == b'/').any(|name| {
                    name.is_empty() || name == b"." || name == b".." || name.len() > NAME_MAX
                })
            {
                return Err(Error::BadPath);
            }
            let name_offset = path
                .iter()
                .rposition(|&byte| byte == b'/')
                .map_or(0, |at| at + 1);
            entries.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
            entries.push(Entry {
                path_start: path_start as u32,
                path_length: path.len() as u16,
                name_offset: name_offset as u16,
                data_start: member.data_offset as u32,
                size: match kind {
                    REGULAR | SYMBOLIC_LINK => member.data.len() as u32,
                    _ => 0,
                },
                parent: ROOT,
                first_child: 0,
                end_of_children: 0,
                mode: member.mode,
                owner: member.owner,
                group: member.group,
                modified: member.modified,
                links: match kind {
                    DIRECTORY => 2,
                    _ => 1,
                },
                pending_link_inode: match kind {
                    REGULAR if member.links > 1 => member.inode,
                    _ => 0,
                },
                file_id: 0,
            });
        }
        let mut file_system = Self { archive, entries };
        file_system.index()?;
        file_system.join_hard_links();
        Ok(file_system)
    }

    fn index(&mut self) -> Result<(), Error> {
        let archive = self.archive;
        let path =
            |entry: &Entry| &archive[entry.path_start as usize..][..entry.path_length as usize];
        let directory_of = |entry: &Entry| {
            let path = path(entry);
            &path[..(entry.name_offset as usize).saturating_sub(1)]
        };
        let name_of = |entry: &Entry| &path(entry)[entry.name_offset as usize..];
        let order = |a: &Entry, b: &Entry| {
            directory_of(a)
                .cmp(directory_of(b))
                .then_with(|| name_of(a).cmp(name_of(b)))
        };
        self.entries[1..].sort_unstable_by(order);
        if self.entries[1..]
            .windows(2)
            .any(|pair| order(&pair[0], &pair[1]) == Ordering::Equal)
        {
            return Err(Error::Duplicate);
        }
        let count = self.entries.len();
        for index in 0..count {
            if self.entries[index].mode & FILE_TYPE != DIRECTORY {
                continue;
            }
            let own_path = path(&self.entries[index]);
            let rest = &self.entries[1..];
            let first = rest.partition_point(|entry| directory_of(entry) < own_path);
            let end = rest.partition_point(|entry| directory_of(entry) <= own_path);
            self.entries[index].first_child = first as u32 + 1;
            self.entries[index].end_of_children = end as u32 + 1;
        }
        for index in 1..count {
            let directory = directory_of(&self.entries[index]);
            let parent = self.find(directory).ok_or(Error::MissingParent)?;
            if self.entries[parent as usize].mode & FILE_TYPE != DIRECTORY {
                return Err(Error::MissingParent);
            }
            if self.entries[parent as usize].path_start > self.entries[index].path_start
                && parent != ROOT
            {
                return Err(Error::MissingParent);
            }
            self.entries[index].parent = parent;
            if self.entries[index].mode & FILE_TYPE == DIRECTORY {
                self.entries[parent as usize].links += 1;
            }
        }
        Ok(())
    }

    fn join_hard_links(&mut self) {
        let count = self.entries.len();
        for node in 0..count {
            self.entries[node].file_id = node as u32 + 1;
        }
        for first in 0..count {
            let inode = self.entries[first].pending_link_inode;
            if inode == 0 {
                continue;
            }
            let names = self.entries[first..]
                .iter()
                .filter(|entry| entry.pending_link_inode == inode);
            let holder = names
                .clone()
                .filter(|entry| entry.size > 0)
                .max_by_key(|entry| entry.data_start)
                .copied()
                .unwrap_or(self.entries[first]);
            let links = names.count() as u32;
            for entry in &mut self.entries[first..] {
                if entry.pending_link_inode != inode {
                    continue;
                }
                *entry = Entry {
                    data_start: holder.data_start,
                    size: holder.size,
                    mode: holder.mode,
                    owner: holder.owner,
                    group: holder.group,
                    modified: holder.modified,
                    links,
                    pending_link_inode: 0,
                    file_id: first as u32 + 1,
                    ..*entry
                };
            }
        }
    }

    fn find(&self, path: &[u8]) -> Option<u32> {
        let mut node = ROOT;
        if path.is_empty() {
            return Some(node);
        }
        for name in path.split(|&byte| byte == b'/') {
            node = self.child(node, name)?;
        }
        Some(node)
    }

    fn path(&self, entry: &Entry) -> &'static [u8] {
        let archive: &'static [u8] = self.archive;
        &archive[entry.path_start as usize..][..entry.path_length as usize]
    }

    fn entry_name(&self, entry: &Entry) -> &'static [u8] {
        &self.path(entry)[entry.name_offset as usize..]
    }

    fn child(&self, directory: u32, name: &[u8]) -> Option<u32> {
        let parent = self.entries.get(directory as usize)?;
        let children = self
            .entries
            .get(parent.first_child as usize..parent.end_of_children as usize)?;
        let index = children
            .binary_search_by(|entry| self.entry_name(entry).cmp(name))
            .ok()?;
        Some(parent.first_child + index as u32)
    }

    /// How many nodes it has, its root included.
    pub fn node_count(&self) -> usize {
        self.entries.len()
    }

    fn entry_at(&self, node: u64) -> &Entry {
        &self.entries[node as usize]
    }
}

fn normalized(name: &[u8]) -> &[u8] {
    let mut path = name;
    loop {
        if let Some(rest) = path.strip_prefix(b"./") {
            path = rest;
        } else if let Some(rest) = path.strip_prefix(b"/") {
            path = rest;
        } else {
            break;
        }
    }
    match path {
        b"." => &path[..0],
        _ => path,
    }
}

impl FileSystem for Initramfs {
    fn root(&self) -> u64 {
        u64::from(ROOT)
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    #[inline]
    fn kind(&self, node: u64) -> NodeKind {
        match self.entry_at(node).mode & FILE_TYPE {
            DIRECTORY => NodeKind::Directory,
            SYMBOLIC_LINK => NodeKind::SymbolicLink,
            _ => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let entry = self.entry_at(node);
        Attributes {
            kind: self.kind(node),
            permissions: entry.mode & 0o7777,
            file_id: u64::from(entry.file_id),
            links: entry.links,
            owner: entry.owner,
            group: entry.group,
            size: u64::from(entry.size),
            modified: u64::from(entry.modified),
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        self.child(directory as u32, name).map(u64::from)
    }

    fn parent(&self, node: u64) -> u64 {
        u64::from(self.entry_at(node).parent)
    }

    fn name(&self, node: u64) -> Name {
        Name::new(self.entry_name(self.entry_at(node)))
    }

    #[inline]
    fn read(&self, node: u64, offset: u64, length: usize) -> &[u8] {
        let entry = self.entry_at(node);
        let size = u64::from(entry.size);
        if offset >= size {
            return &[];
        }
        let start = entry.data_start as usize + offset as usize;
        let length = (size - offset).min(length as u64) as usize;
        &self.archive[start..start + length]
    }

    fn link_target(&self, node: u64, _caller: u32) -> Option<Vec<u8>> {
        if self.kind(node) != NodeKind::SymbolicLink {
            return None;
        }
        let entry = self.entry_at(node);
        let target = &self.archive[entry.data_start as usize..][..entry.size as usize];
        let mut copy = Vec::new();
        copy.try_reserve_exact(target.len()).ok()?;
        copy.extend_from_slice(target);
        Some(copy)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let parent = self.entry_at(directory);
        let index = u64::from(parent.first_child).checked_add(position)?;
        if index >= u64::from(parent.end_of_children) {
            return None;
        }
        Some(DirectoryEntry {
            name: self.name(index),
            node: index,
            kind: self.kind(index),
        })
    }
}

/// The archive embedded in the kernel image: built by `cargo xtask` from `initramfs/` and
/// named by `EGGOS_INITRAMFS` for kernel builds; a small fixed archive for host tests, and an
/// empty one for other host builds.
pub fn embedded() -> &'static [u8] {
    #[cfg(target_os = "none")]
    {
        include_bytes!(env!(
            "EGGOS_INITRAMFS",
            "build kernels through cargo xtask, which builds the initramfs archive first"
        ))
    }
    #[cfg(all(not(target_os = "none"), test))]
    {
        tests::host_archive()
    }
    #[cfg(all(not(target_os = "none"), not(test)))]
    {
        &[]
    }
}

#[cfg(test)]
pub mod tests {
    use super::super::cpio::tests::archive;
    use super::*;
    use alloc::boxed::Box;

    /// The archive host tests mount as the root, like the repository's `initramfs/`.
    pub fn host_archive() -> &'static [u8] {
        static ARCHIVE: std::sync::OnceLock<&'static [u8]> = std::sync::OnceLock::new();
        ARCHIVE.get_or_init(|| {
            let program = crate::selftest::linux_start_image::<crate::tests::TestPlatform>();
            let bytes = archive(&[
                (".", 0o040755, 1_000, b""),
                ("etc", 0o040755, 1_000, b""),
                ("etc/hostname", 0o100644, 1_700_000_000, b"eggos\n"),
                ("etc/motd", 0o100644, 1_700_000_000, b"Welcome to EggOS.\n"),
                ("usr", 0o040755, 1_000, b""),
                ("usr/libexec", 0o040755, 1_000, b""),
                ("usr/libexec/program", 0o100755, 1_000, &program),
                ("usr/libexec/script", 0o100755, 1_000, b"#!/bin/sh\n"),
                (
                    "usr/libexec/runs",
                    0o100755,
                    1_000,
                    b"#!/usr/libexec/program  -x \n",
                ),
                (
                    "usr/libexec/loop",
                    0o100755,
                    1_000,
                    b"#!/usr/libexec/loop\n",
                ),
                ("usr/libexec/bare", 0o100755, 1_000, b"#!\n"),
                (
                    "usr/libexec/to-text",
                    0o100755,
                    1_000,
                    b"#! /etc/hostname\n",
                ),
                ("usr/libexec/text", 0o100755, 1_000, b"not a program\n"),
                ("usr/share", 0o040755, 1_000, b""),
                ("usr/share/eggos", 0o040755, 1_000, b""),
                ("usr/share/eggos/about", 0o100644, 1_000, b"EggOS\n"),
            ]);
            Box::leak(bytes.into_boxed_slice())
        })
    }

    fn mount(entries: &[cpio::tests::Entry<'_>]) -> Result<Initramfs, Error> {
        let bytes: &'static [u8] = Box::leak(archive(entries).into_boxed_slice());
        Initramfs::parse(bytes)
    }

    fn names(file_system: &Initramfs, directory: u64) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
        (0..)
            .map_while(|position| file_system.entry(directory, position))
            .map(|entry| entry.name.as_bytes().to_vec())
            .collect()
    }

    #[test]
    fn the_index_lists_each_directory_s_entries_sorted_and_finds_them() {
        let file_system = mount(&[
            ("./etc", 0o040755, 0, b""),
            ("/etc/b", 0o100644, 5, b"bb"),
            ("etc/a", 0o100600, 0, b"a"),
            ("etc/sub", 0o040700, 0, b""),
            ("zz", 0o100644, 0, b""),
            ("etc/sub/c", 0o100644, 0, b"c"),
            ("link", 0o120777, 0, b"etc"),
        ])
        .unwrap();
        let root = file_system.root();
        assert_eq!(names(&file_system, root), [&b"etc"[..], b"link", b"zz"]);
        let etc = file_system.lookup(root, b"etc").unwrap();
        assert_eq!(names(&file_system, etc), [&b"a"[..], b"b", b"sub"]);
        let b = file_system.lookup(etc, b"b").unwrap();
        assert_eq!(file_system.read(b, 0, 10), b"bb");
        assert_eq!(file_system.read(b, 1, 10), b"b");
        assert_eq!(file_system.read(b, 2, 10), b"");
        assert_eq!(file_system.read(b, u64::MAX, 10), b"");
        assert_eq!(file_system.parent(b), etc);
        assert_eq!(file_system.parent(etc), root);
        assert_eq!(file_system.parent(root), root);
        let attributes = file_system.attributes(b);
        assert_eq!(attributes.kind, NodeKind::Regular);
        assert_eq!((attributes.permissions, attributes.size), (0o644, 2));
        assert_eq!((attributes.links, attributes.modified), (1, 5));
        assert_eq!(file_system.attributes(etc).links, 3);
        assert_eq!(file_system.attributes(root).links, 3);
        let link = file_system.lookup(root, b"link").unwrap();
        assert_eq!(file_system.kind(link), NodeKind::SymbolicLink);
        assert_eq!(file_system.attributes(link).size, 3);
        assert_eq!(
            file_system.link_target(link, crate::vfs::KERNEL_CALLER),
            Some(b"etc".to_vec())
        );
        assert_eq!(file_system.link_target(b, crate::vfs::KERNEL_CALLER), None);
        assert_eq!(file_system.lookup(etc, b"zz"), None);
        let sub = file_system.lookup(etc, b"sub").unwrap();
        assert_eq!(file_system.name(sub).as_bytes(), b"sub");
        assert!(file_system.lookup(sub, b"c").is_some());
        assert_eq!(file_system.entry(sub, 1), None);
        assert_eq!(file_system.entry(sub, u64::MAX), None);
    }

    #[test]
    fn the_root_entry_sets_the_root_s_attributes_and_an_empty_archive_has_an_empty_root() {
        let file_system = mount(&[(".", 0o040700, 9, b"")]).unwrap();
        let root = file_system.attributes(file_system.root());
        assert_eq!((root.permissions, root.modified), (0o700, 9));
        let empty = Initramfs::empty();
        assert_eq!(names(&empty, empty.root()), alloc::vec::Vec::<&[u8]>::new());
        assert_eq!(
            mount(&[(".", 0o100644, 0, b"")]).err(),
            Some(Error::BadPath)
        );
    }

    #[test]
    fn archives_breaking_the_rules_are_refused() {
        let refused = |entries: &[cpio::tests::Entry<'_>]| mount(entries).err();
        assert_eq!(
            refused(&[("etc/x", 0o100644, 0, b"")]),
            Some(Error::MissingParent)
        );
        assert_eq!(
            refused(&[("etc/x", 0o100644, 0, b""), ("etc", 0o040755, 0, b"")]),
            Some(Error::MissingParent)
        );
        assert_eq!(
            refused(&[("f", 0o100644, 0, b""), ("f/x", 0o100644, 0, b"")]),
            Some(Error::MissingParent)
        );
        assert_eq!(
            refused(&[("a", 0o100644, 0, b""), ("a", 0o100644, 0, b"")]),
            Some(Error::Duplicate)
        );
        for bad in ["a//b", "a/./b", "a/../b", "a/"] {
            assert_eq!(
                refused(&[("a", 0o040755, 0, b""), (bad, 0o100644, 0, b"")]),
                Some(Error::BadPath),
                "{bad}"
            );
        }
        let long = "n".repeat(NAME_MAX + 1);
        assert_eq!(
            refused(&[(long.as_str(), 0o100644, 0, b"")]),
            Some(Error::BadPath)
        );
        let mut truncated = archive(&[("a", 0o100644, 0, b"abc")]);
        truncated.truncate(100);
        let truncated: &'static [u8] = Box::leak(truncated.into_boxed_slice());
        assert_eq!(
            Initramfs::parse(truncated).err(),
            Some(Error::Archive(cpio::Error::Truncated))
        );
    }

    #[test]
    fn hard_links_share_the_data_attributes_and_file_id_of_their_file() {
        let bytes = cpio::tests::archive_with_inodes(&[
            (("bin", 0o040755, 0, b""), 1, 2),
            (("bin/ls", 0o100755, 0, b""), 7, 3),
            (("bin/cat", 0o100755, 0, b""), 7, 3),
            (("bin/coreutils", 0o100755, 9, b"program"), 7, 3),
            (("bin/alone", 0o100644, 0, b"own"), 8, 2),
            (("bin/plain", 0o100644, 0, b"plain"), 7, 1),
        ]);
        let file_system = Initramfs::parse(Box::leak(bytes.into_boxed_slice())).unwrap();
        let bin = file_system.lookup(file_system.root(), b"bin").unwrap();
        let node = |name: &[u8]| file_system.lookup(bin, name).unwrap();
        let program = file_system.attributes(node(b"coreutils"));
        assert_eq!((program.links, program.size, program.modified), (3, 7, 9));
        for name in [&b"ls"[..], b"cat", b"coreutils"] {
            assert_eq!(file_system.read(node(name), 0, 64), b"program");
            assert_eq!(file_system.attributes(node(name)), program);
        }
        let alone = file_system.attributes(node(b"alone"));
        assert_eq!((alone.links, alone.size), (1, 3));
        let plain = file_system.attributes(node(b"plain"));
        assert_eq!((plain.links, plain.size), (1, 5));
        assert_ne!(plain.file_id, program.file_id);
        assert_ne!(alone.file_id, program.file_id);
        assert_eq!(file_system.read(node(b"plain"), 0, 64), b"plain");
    }

    #[test]
    fn the_host_archive_mounts() {
        let file_system = Initramfs::parse(host_archive()).unwrap();
        assert_eq!(file_system.node_count(), 16);
    }
}
