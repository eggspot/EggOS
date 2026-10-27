//! A writable file system in memory (D125), mounted on `/tmp` and `/home` at boot: directories
//! and regular files that can be created, written, appended to, truncated, renamed and
//! removed. Contents are kept in pages taken as they are written, from a [`Budget`] that the
//! file systems sharing it draw on; a write past it fails with [`Refused::NoSpace`], and
//! removing a file gives its pages back at once.
//!
//! Everything is behind one leaf lock, held only while the file system's own memory is read or
//! changed: never while a program's memory is copied, so reads and writes go through the
//! caller's buffer. A node's number holds its slot, its kind and the slot's generation, which
//! changes when the slot's node is removed: a number kept from before (a working directory, an
//! open file) then names nothing. A file removed while open is gone at once: reads find its
//! end and writes fail, where Linux keeps it until its last close.

use super::{Attributes, DirectoryEntry, FileSystem, NAME_MAX, Name, NodeKind, Refused, WriteAt};
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE_SIZE: usize = 4096;

const NODE_COST: usize = 128;

type Page = Box<[u8]>;

fn zeroed_page() -> Option<Page> {
    let mut page = Vec::new();
    page.try_reserve_exact(PAGE_SIZE).ok()?;
    page.resize(PAGE_SIZE, 0);
    Some(page.into_boxed_slice())
}

/// The memory file systems may take between them.
#[derive(Debug)]
pub struct Budget {
    limit: usize,
    used: AtomicUsize,
}

impl Budget {
    /// A budget of `limit` bytes.
    pub const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
        }
    }

    /// The bytes taken now.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    fn take(&self, bytes: usize) -> Result<(), Refused> {
        self.used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|&total| total <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| Refused::NoSpace)
    }

    fn give_back(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }
}

#[derive(Debug)]
enum Contents {
    Directory(Vec<u32>),
    File { pages: Vec<Option<Page>>, size: u64 },
}

#[derive(Debug)]
struct Inode {
    parent: u32,
    name: Vec<u8>,
    permissions: u32,
    modified: u64,
    contents: Contents,
}

#[derive(Debug, Default)]
struct Slot {
    generation: u32,
    inode: Option<Inode>,
}

#[derive(Debug)]
struct State {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

const DIRECTORY_BIT: u64 = 1 << 32;

fn number(slot: u32, generation: u32, directory: bool) -> u64 {
    u64::from(generation) << 33 | u64::from(directory) << 32 | u64::from(slot)
}

fn slot_of(node: u64) -> (usize, u32) {
    (node as u32 as usize, (node >> 33) as u32)
}

/// A memory file system: see the module documentation.
#[derive(Debug)]
pub struct Tmpfs {
    state: LeafSpinLock<State>,
    budget: &'static Budget,
    device: (u32, u32),
}

impl Tmpfs {
    /// An empty file system, its root `0o1777` (anyone may create in it, as `/tmp` is) or
    /// `permissions`, reporting `device`, drawing on `budget`. `None` without memory for its
    /// root.
    pub fn new(budget: &'static Budget, device: (u32, u32), permissions: u32) -> Option<Self> {
        let mut slots = Vec::new();
        slots.try_reserve(1).ok()?;
        slots.push(Slot {
            generation: 0,
            inode: Some(Inode {
                parent: 0,
                name: Vec::new(),
                permissions,
                modified: 0,
                contents: Contents::Directory(Vec::new()),
            }),
        });
        Some(Self {
            state: LeafSpinLock::new_leaf(State {
                slots,
                free: Vec::new(),
            }),
            budget,
            device,
        })
    }
}

impl State {
    fn inode(&self, node: u64) -> Option<&Inode> {
        let (slot, generation) = slot_of(node);
        let slot = self.slots.get(slot)?;
        (slot.generation == generation).then_some(slot.inode.as_ref()?)
    }

    fn inode_mut(&mut self, node: u64) -> Option<&mut Inode> {
        let (slot, generation) = slot_of(node);
        let slot = self.slots.get_mut(slot)?;
        if slot.generation != generation {
            return None;
        }
        slot.inode.as_mut()
    }

    fn number_of(&self, slot: u32) -> u64 {
        let entry = &self.slots[slot as usize];
        let directory = matches!(
            entry.inode.as_ref().map(|inode| &inode.contents),
            Some(Contents::Directory(_))
        );
        number(slot, entry.generation, directory)
    }

    fn children(&self, directory: u64) -> Result<&[u32], Refused> {
        match &self.inode(directory).ok_or(Refused::NotFound)?.contents {
            Contents::Directory(children) => Ok(children),
            Contents::File { .. } => Err(Refused::NotDirectory),
        }
    }

    fn child(&self, directory: u64, name: &[u8]) -> Result<Option<u32>, Refused> {
        Ok(self.children(directory)?.iter().copied().find(|&slot| {
            self.slots[slot as usize]
                .inode
                .as_ref()
                .is_some_and(|inode| inode.name == name)
        }))
    }

    fn is_ancestor(&self, ancestor: u32, mut slot: u32) -> bool {
        loop {
            if slot == ancestor {
                return true;
            }
            if slot == 0 {
                return false;
            }
            slot = self.slots[slot as usize]
                .inode
                .as_ref()
                .map_or(0, |inode| inode.parent);
        }
    }

    fn detach(&mut self, parent: u32, slot: u32) {
        if let Some(Inode {
            contents: Contents::Directory(children),
            ..
        }) = self.slots[parent as usize].inode.as_mut()
        {
            children.retain(|&child| child != slot);
        }
    }

    fn free_slot(&mut self, slot: u32, budget: &Budget) {
        let entry = &mut self.slots[slot as usize];
        if let Some(inode) = entry.inode.take() {
            let pages = match &inode.contents {
                Contents::File { pages, .. } => pages.iter().flatten().count(),
                Contents::Directory(_) => 0,
            };
            budget.give_back(pages * PAGE_SIZE + NODE_COST + inode.name.len());
        }
        entry.generation = entry.generation.wrapping_add(1);
        if self.free.try_reserve(1).is_ok() {
            self.free.push(slot);
        }
    }
}

fn check_name(name: &[u8]) -> Result<(), Refused> {
    match name {
        [] | b"." | b".." => Err(Refused::Exists),
        _ if name.len() > NAME_MAX => Err(Refused::NameTooLong),
        _ if name.contains(&b'/') || name.contains(&0) => Err(Refused::InvalidArgument),
        _ => Ok(()),
    }
}

fn page_range(offset: u64, length: usize) -> (usize, usize, usize) {
    let page = (offset / PAGE_SIZE as u64) as usize;
    let within = (offset % PAGE_SIZE as u64) as usize;
    (page, within, length.min(PAGE_SIZE - within))
}

impl FileSystem for Tmpfs {
    fn root(&self) -> u64 {
        DIRECTORY_BIT
    }

    fn device(&self) -> (u32, u32) {
        self.device
    }

    fn kind(&self, node: u64) -> NodeKind {
        match node & DIRECTORY_BIT != 0 {
            true => NodeKind::Directory,
            false => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let state = self.state.lock();
        let kind = self.kind(node);
        let Some(inode) = state.inode(node) else {
            return Attributes {
                kind,
                permissions: 0,
                file_id: node as u32 as u64 + 1,
                links: 0,
                owner: 0,
                group: 0,
                size: 0,
                modified: 0,
            };
        };
        let (links, size) = match &inode.contents {
            Contents::Directory(children) => {
                let subdirectories = children
                    .iter()
                    .filter(|&&child| state.number_of(child) & DIRECTORY_BIT != 0)
                    .count();
                (2 + subdirectories as u32, 0)
            }
            Contents::File { size, .. } => (1, *size),
        };
        Attributes {
            kind,
            permissions: inode.permissions,
            file_id: node as u32 as u64 + 1,
            links,
            owner: 0,
            group: 0,
            size,
            modified: inode.modified,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        let state = self.state.lock();
        let slot = state.child(directory, name).ok()??;
        Some(state.number_of(slot))
    }

    fn parent(&self, node: u64) -> u64 {
        let state = self.state.lock();
        let parent = state.inode(node).map_or(0, |inode| inode.parent);
        state.number_of(parent)
    }

    fn name(&self, node: u64) -> Name {
        let state = self.state.lock();
        Name::new(state.inode(node).map_or(&[][..], |inode| &inode.name))
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn copies_contents(&self) -> bool {
        true
    }

    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> usize {
        let state = self.state.lock();
        let Some(Inode {
            contents: Contents::File { pages, size },
            ..
        }) = state.inode(node)
        else {
            return 0;
        };
        if offset >= *size {
            return 0;
        }
        let wanted = buffer
            .len()
            .min((*size - offset).min(usize::MAX as u64) as usize);
        let (page, within, length) = page_range(offset, wanted);
        match pages.get(page).and_then(Option::as_ref) {
            Some(page) => buffer[..length].copy_from_slice(&page[within..within + length]),
            None => buffer[..length].fill(0),
        }
        length
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let state = self.state.lock();
        let slot = *state
            .children(directory)
            .ok()?
            .get(usize::try_from(position).ok()?)?;
        let node = state.number_of(slot);
        Some(DirectoryEntry {
            name: Name::new(&state.slots[slot as usize].inode.as_ref()?.name),
            node,
            kind: self.kind(node),
        })
    }

    fn is_writable(&self) -> bool {
        true
    }

    fn write(
        &self,
        node: u64,
        at: WriteAt,
        bytes: &[u8],
        now: u64,
    ) -> Result<(usize, u64), Refused> {
        let mut state = self.state.lock();
        let inode = state.inode_mut(node).ok_or(Refused::NotFound)?;
        let Contents::File { pages, size } = &mut inode.contents else {
            return Err(Refused::IsDirectory);
        };
        let offset = match at {
            WriteAt::Offset(offset) => offset,
            WriteAt::End => *size,
        };
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|&end| end <= i64::MAX as u64)
            .ok_or(Refused::TooBig)?;
        let mut written = 0;
        while written < bytes.len() {
            let (page, within, length) = page_range(offset + written as u64, bytes.len() - written);
            if pages.len() <= page {
                if pages.try_reserve(page + 1 - pages.len()).is_err() {
                    break;
                }
                pages.resize_with(page + 1, || None);
            }
            if pages[page].is_none() {
                if self.budget.take(PAGE_SIZE).is_err() {
                    break;
                }
                match zeroed_page() {
                    Some(zeroed) => pages[page] = Some(zeroed),
                    None => {
                        self.budget.give_back(PAGE_SIZE);
                        break;
                    }
                }
            }
            if let Some(page) = pages[page].as_mut() {
                page[within..within + length].copy_from_slice(&bytes[written..written + length]);
            }
            written += length;
        }
        if written == 0 && !bytes.is_empty() {
            return Err(Refused::NoSpace);
        }
        let reached = offset + written as u64;
        if reached > *size {
            *size = reached;
        }
        inode.modified = now;
        debug_assert!(reached <= end);
        Ok((written, reached))
    }

    fn truncate(&self, node: u64, length: u64, now: u64) -> Result<(), Refused> {
        if length > i64::MAX as u64 {
            return Err(Refused::TooBig);
        }
        let mut state = self.state.lock();
        let inode = state.inode_mut(node).ok_or(Refused::NotFound)?;
        let Contents::File { pages, size } = &mut inode.contents else {
            return Err(Refused::IsDirectory);
        };
        let kept = (length as usize).div_ceil(PAGE_SIZE);
        if kept < pages.len() {
            let freed = pages[kept..].iter().flatten().count();
            pages.truncate(kept);
            pages.shrink_to_fit();
            self.budget.give_back(freed * PAGE_SIZE);
        }
        let within = (length % PAGE_SIZE as u64) as usize;
        if length < *size
            && within > 0
            && let Some(Some(last)) = pages.get_mut(kept - 1)
        {
            last[within..].fill(0);
        }
        *size = length;
        inode.modified = now;
        Ok(())
    }

    fn create(
        &self,
        directory: u64,
        name: &[u8],
        kind: NodeKind,
        permissions: u32,
        now: u64,
    ) -> Result<u64, Refused> {
        check_name(name)?;
        let mut state = self.state.lock();
        if state.child(directory, name)?.is_some() {
            return Err(Refused::Exists);
        }
        let parent = slot_of(directory).0 as u32;
        let cost = NODE_COST + name.len();
        self.budget.take(cost)?;
        let mut stored = Vec::new();
        let made = stored
            .try_reserve_exact(name.len())
            .ok()
            .and_then(|()| state.slots.try_reserve(1).ok())
            .and_then(|()| match state.inode_mut(directory) {
                Some(Inode {
                    contents: Contents::Directory(children),
                    ..
                }) => children.try_reserve(1).ok(),
                _ => None,
            });
        if made.is_none() {
            self.budget.give_back(cost);
            return Err(Refused::NoSpace);
        }
        stored.extend_from_slice(name);
        let inode = Inode {
            parent,
            name: stored,
            permissions: permissions & 0o7777,
            modified: now,
            contents: match kind {
                NodeKind::Directory => Contents::Directory(Vec::new()),
                NodeKind::Regular => Contents::File {
                    pages: Vec::new(),
                    size: 0,
                },
            },
        };
        let slot = match state.free.pop() {
            Some(slot) => {
                state.slots[slot as usize].inode = Some(inode);
                slot
            }
            None => {
                state.slots.push(Slot {
                    generation: 0,
                    inode: Some(inode),
                });
                (state.slots.len() - 1) as u32
            }
        };
        if let Some(parent) = state.inode_mut(directory) {
            if let Contents::Directory(children) = &mut parent.contents {
                children.push(slot);
            }
            parent.modified = now;
        }
        Ok(state.number_of(slot))
    }

    fn remove(
        &self,
        directory: u64,
        name: &[u8],
        directories: bool,
        now: u64,
    ) -> Result<(), Refused> {
        let mut state = self.state.lock();
        let slot = state.child(directory, name)?.ok_or(Refused::NotFound)?;
        match (
            &state.slots[slot as usize]
                .inode
                .as_ref()
                .ok_or(Refused::NotFound)?
                .contents,
            directories,
        ) {
            (Contents::Directory(children), true) if !children.is_empty() => {
                return Err(Refused::NotEmpty);
            }
            (Contents::Directory(_), false) => return Err(Refused::IsDirectory),
            (Contents::File { .. }, true) => return Err(Refused::NotDirectory),
            _ => {}
        }
        let parent = slot_of(directory).0 as u32;
        state.detach(parent, slot);
        state.free_slot(slot, self.budget);
        if let Some(parent) = state.inode_mut(directory) {
            parent.modified = now;
        }
        Ok(())
    }

    fn rename(
        &self,
        from: u64,
        from_name: &[u8],
        to: u64,
        to_name: &[u8],
        no_replace: bool,
        now: u64,
    ) -> Result<(), Refused> {
        check_name(to_name).map_err(|refused| match refused {
            Refused::Exists => Refused::Busy,
            refused => refused,
        })?;
        let mut state = self.state.lock();
        let moved = state.child(from, from_name)?.ok_or(Refused::NotFound)?;
        state.children(to)?;
        let moved_is_directory = state.number_of(moved) & DIRECTORY_BIT != 0;
        let to_slot = slot_of(to).0 as u32;
        if moved_is_directory && state.is_ancestor(moved, to_slot) {
            return Err(Refused::InvalidArgument);
        }
        if let Some(replaced) = state.child(to, to_name)? {
            if replaced == moved {
                return Ok(());
            }
            if no_replace {
                return Err(Refused::Exists);
            }
            let replaced_is_directory = state.number_of(replaced) & DIRECTORY_BIT != 0;
            match (moved_is_directory, replaced_is_directory) {
                (true, false) => return Err(Refused::NotDirectory),
                (false, true) => return Err(Refused::IsDirectory),
                (true, true) if !state.children(state.number_of(replaced))?.is_empty() => {
                    return Err(Refused::NotEmpty);
                }
                _ => {}
            }
            state.detach(to_slot, replaced);
            state.free_slot(replaced, self.budget);
        }
        let old_length = state.slots[moved as usize]
            .inode
            .as_ref()
            .map_or(0, |inode| inode.name.len());
        let grown = to_name.len().saturating_sub(old_length);
        self.budget.take(grown)?;
        let mut name = Vec::new();
        let room = name.try_reserve_exact(to_name.len()).is_ok()
            && match state.inode_mut(to) {
                Some(Inode {
                    contents: Contents::Directory(children),
                    ..
                }) => children.try_reserve(1).is_ok(),
                _ => false,
            };
        if !room {
            self.budget.give_back(grown);
            return Err(Refused::NoSpace);
        }
        name.extend_from_slice(to_name);
        let from_slot = slot_of(from).0 as u32;
        state.detach(from_slot, moved);
        if let Some(Inode {
            contents: Contents::Directory(children),
            modified,
            ..
        }) = state.inode_mut(to)
        {
            children.push(moved);
            *modified = now;
        }
        if let Some(inode) = state.slots[moved as usize].inode.as_mut() {
            inode.parent = to_slot;
            inode.name = name;
        }
        self.budget
            .give_back(old_length.saturating_sub(to_name.len()));
        if let Some(parent) = state.inode_mut(from) {
            parent.modified = now;
        }
        Ok(())
    }

    fn set_modified(&self, node: u64, time: u64) -> Result<(), Refused> {
        let mut state = self.state.lock();
        state.inode_mut(node).ok_or(Refused::NotFound)?.modified = time;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfs(limit: usize) -> &'static Tmpfs {
        let budget = Box::leak(Box::new(Budget::new(limit)));
        Box::leak(Box::new(Tmpfs::new(budget, (0, 30), 0o1777).unwrap()))
    }

    fn read_all(fs: &Tmpfs, node: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buffer = [0u8; 1000];
        loop {
            let read = fs.read_into(node, out.len() as u64, &mut buffer);
            if read == 0 {
                return out;
            }
            out.extend_from_slice(&buffer[..read]);
        }
    }

    #[test]
    fn files_are_created_written_appended_and_read_back_across_pages() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let file = fs.create(root, b"x", NodeKind::Regular, 0o644, 5).unwrap();
        assert_eq!(fs.lookup(root, b"x"), Some(file));
        assert_eq!(fs.kind(file), NodeKind::Regular);
        assert_eq!(fs.write(file, WriteAt::Offset(0), b"a\n", 6), Ok((2, 2)));
        assert_eq!(fs.write(file, WriteAt::End, b"b\n", 7), Ok((2, 4)));
        assert_eq!(read_all(fs, file), b"a\nb\n");
        let big: Vec<u8> = (0..10_000u32).map(|index| index as u8).collect();
        assert_eq!(
            fs.write(file, WriteAt::Offset(4), &big[..5000], 8)
                .unwrap()
                .0,
            5000
        );
        let mut done = 5000;
        while done < big.len() {
            done += fs.write(file, WriteAt::End, &big[done..], 8).unwrap().0;
        }
        let back = read_all(fs, file);
        assert_eq!(&back[4..], &big[..]);
        let attributes = fs.attributes(file);
        assert_eq!(
            (attributes.size, attributes.modified, attributes.permissions),
            (10_004, 8, 0o644)
        );
        assert_eq!(fs.budget.used(), NODE_COST + 1 + 3 * PAGE_SIZE);
    }

    #[test]
    fn writing_past_the_end_leaves_zeros_and_truncating_frees_pages() {
        let fs = tmpfs(1 << 20);
        let file = fs
            .create(fs.root(), b"sparse", NodeKind::Regular, 0o600, 0)
            .unwrap();
        fs.write(file, WriteAt::Offset(3 * PAGE_SIZE as u64 + 1), b"z", 0)
            .unwrap();
        assert_eq!(fs.budget.used(), NODE_COST + 6 + PAGE_SIZE);
        let back = read_all(fs, file);
        assert_eq!(back.len(), 3 * PAGE_SIZE + 2);
        assert!(back[..3 * PAGE_SIZE + 1].iter().all(|&byte| byte == 0));
        fs.truncate(file, 1, 0).unwrap();
        assert_eq!(read_all(fs, file), b"\0");
        assert_eq!(fs.budget.used(), NODE_COST + 6);
        fs.write(file, WriteAt::Offset(0), b"abcdef", 0).unwrap();
        fs.truncate(file, 2, 0).unwrap();
        fs.truncate(file, 4, 0).unwrap();
        assert_eq!(read_all(fs, file), b"ab\0\0");
    }

    #[test]
    fn the_budget_bounds_what_is_kept_and_removing_gives_it_back() {
        let fs = tmpfs(NODE_COST + 1 + 2 * PAGE_SIZE);
        let file = fs
            .create(fs.root(), b"f", NodeKind::Regular, 0o644, 0)
            .unwrap();
        let bytes = vec![1u8; 3 * PAGE_SIZE];
        assert_eq!(
            fs.write(file, WriteAt::Offset(0), &bytes, 0),
            Ok((2 * PAGE_SIZE, 2 * PAGE_SIZE as u64))
        );
        assert_eq!(
            fs.write(file, WriteAt::End, &bytes, 0),
            Err(Refused::NoSpace)
        );
        assert_eq!(
            fs.create(fs.root(), b"g", NodeKind::Regular, 0o644, 0),
            Err(Refused::NoSpace)
        );
        fs.remove(fs.root(), b"f", false, 0).unwrap();
        assert_eq!(fs.budget.used(), 0);
        assert_eq!(fs.lookup(fs.root(), b"f"), None);
        assert_eq!(read_all(fs, file), b"");
        assert_eq!(
            fs.write(file, WriteAt::End, b"x", 0),
            Err(Refused::NotFound)
        );
        let again = fs
            .create(fs.root(), b"g", NodeKind::Regular, 0o644, 0)
            .unwrap();
        assert_ne!(again, file);
        assert_eq!(fs.attributes(file).links, 0);
    }

    #[test]
    fn directories_hold_entries_and_are_removed_only_when_empty() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let home = fs
            .create(root, b"u", NodeKind::Directory, 0o755, 0)
            .unwrap();
        assert_eq!(fs.kind(home), NodeKind::Directory);
        let file = fs.create(home, b"f", NodeKind::Regular, 0o644, 0).unwrap();
        assert_eq!(
            fs.create(home, b"f", NodeKind::Regular, 0o644, 0),
            Err(Refused::Exists)
        );
        assert_eq!(
            fs.create(file, b"x", NodeKind::Regular, 0o644, 0),
            Err(Refused::NotDirectory)
        );
        assert_eq!(fs.parent(file), home);
        assert_eq!(fs.parent(home), root);
        assert_eq!(fs.name(file).as_bytes(), b"f");
        assert_eq!(fs.attributes(root).links, 3);
        let entry = fs.entry(home, 0).unwrap();
        assert_eq!(
            (entry.name.as_bytes(), entry.node, entry.kind),
            (&b"f"[..], file, NodeKind::Regular)
        );
        assert_eq!(fs.entry(home, 1), None);
        assert_eq!(fs.remove(root, b"u", true, 0), Err(Refused::NotEmpty));
        assert_eq!(fs.remove(root, b"u", false, 0), Err(Refused::IsDirectory));
        assert_eq!(fs.remove(home, b"f", true, 0), Err(Refused::NotDirectory));
        fs.remove(home, b"f", false, 0).unwrap();
        fs.remove(root, b"u", true, 0).unwrap();
        assert_eq!(fs.lookup(home, b"f"), None);
        assert_eq!(fs.entry(home, 0), None);
        assert_eq!(fs.remove(root, b"u", true, 0), Err(Refused::NotFound));
        for bad in [&b""[..], b".", b".."] {
            assert_eq!(
                fs.create(root, bad, NodeKind::Regular, 0, 0),
                Err(Refused::Exists)
            );
        }
    }

    #[test]
    fn renaming_moves_and_replaces_as_linux_s_rename_does() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let a = fs
            .create(root, b"a", NodeKind::Directory, 0o755, 0)
            .unwrap();
        let b = fs.create(a, b"b", NodeKind::Directory, 0o755, 0).unwrap();
        let y = fs.create(root, b"y", NodeKind::Regular, 0o644, 0).unwrap();
        fs.write(y, WriteAt::End, b"data", 0).unwrap();
        let z = fs.create(a, b"z", NodeKind::Regular, 0o644, 0).unwrap();
        assert_eq!(
            fs.rename(root, b"y", a, b"z", true, 0),
            Err(Refused::Exists)
        );
        fs.rename(root, b"y", a, b"z", false, 3).unwrap();
        assert_eq!(fs.lookup(a, b"z"), Some(y));
        assert_eq!(fs.lookup(root, b"y"), None);
        assert_eq!(read_all(fs, y), b"data");
        assert_eq!(fs.attributes(z).links, 0);
        assert_eq!(
            fs.rename(root, b"a", b, b"a", false, 0),
            Err(Refused::InvalidArgument)
        );
        assert_eq!(
            fs.rename(a, b"z", root, b"a", false, 0),
            Err(Refused::IsDirectory)
        );
        assert_eq!(
            fs.rename(root, b"a", a, b"z", false, 0),
            Err(Refused::InvalidArgument)
        );
        fs.create(root, b"c", NodeKind::Directory, 0o755, 0)
            .unwrap();
        assert_eq!(
            fs.rename(root, b"c", a, b"z", false, 0),
            Err(Refused::NotDirectory)
        );
        assert_eq!(
            fs.rename(root, b"missing", a, b"q", false, 0),
            Err(Refused::NotFound)
        );
        fs.rename(a, b"z", a, b"z", false, 0).unwrap();
        fs.rename(a, b"b", root, b"longer name", false, 0).unwrap();
        assert_eq!(fs.parent(b), root);
        assert_eq!(fs.name(b).as_bytes(), b"longer name");
    }
}
