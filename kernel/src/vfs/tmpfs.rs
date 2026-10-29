//! A writable file system in memory (D125), mounted on `/tmp` at boot, and on `/home` when no
//! disk holds it (D131): directories
//! and regular files that can be created, written, appended to, truncated, renamed and
//! removed. Contents are kept in pages taken as they are written, from a [`Budget`] that the
//! file systems sharing it draw on; a write past it fails with [`Refused::NoSpace`], and
//! removing a file gives its pages back at once.
//!
//! A socket file (`NodeKind::Socket`, D138) is a node with no contents that `bind` makes: its
//! kind is in its number, as a directory's is.
//!
//! Everything is behind one leaf lock, held only while the file system's own memory is read or
//! changed: never while a program's memory is copied, so reads and writes go through the
//! caller's buffer. A node's number holds its slot, its kind and the slot's generation, which
//! changes when the slot's node is removed: a number kept from before (a working directory, an
//! open file) then names nothing.
//!
//! Names live in their directories, so a file or a symbolic link may have several (D170): its
//! inode counts them and records one directory holding one of them as its parent. A symbolic
//! link keeps its target. A file whose last name goes while it is held ([`FileSystem::hold`]:
//! open, mapped, running) keeps its contents, readable and writable through its number, until
//! its last hold is released, as on Linux.

use super::{
    Attributes, DirectoryEntry, FileSystem, NAME_MAX, Name, NodeKind, Refused, StorageError,
    WriteAt,
};
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE_SIZE: usize = 4096;

const NODE_COST: usize = 128;

type Page = Box<[u8]>;

const INDEX_ENTRY_COST: usize = core::mem::size_of::<Option<Page>>();

fn index_cost(pages: &Vec<Option<Page>>) -> usize {
    pages.capacity() * INDEX_ENTRY_COST
}

fn settle_index(budget: &Budget, pages: &Vec<Option<Page>>, charged: usize) {
    let cost = index_cost(pages);
    if cost < charged {
        budget.give_back(charged - cost);
    }
}

fn grow_index(budget: &Budget, pages: &mut Vec<Option<Page>>, entries: usize) -> bool {
    let charged = index_cost(pages);
    if pages.try_reserve(entries - pages.len()).is_err() {
        return false;
    }
    let grown = index_cost(pages) - charged;
    if budget.take(grown).is_err() {
        pages.shrink_to_fit();
        settle_index(budget, pages, charged);
        return false;
    }
    pages.resize_with(entries, || None);
    true
}

fn zeroed_page() -> Option<Page> {
    if !crate::heap::may_grow_for_user(PAGE_SIZE) {
        return None;
    }
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
    Directory(Vec<Child>),
    File { pages: Vec<Option<Page>>, size: u64 },
    Link(Box<[u8]>),
}

#[derive(Debug)]
struct Child {
    slot: u32,
    name: Vec<u8>,
}

#[derive(Debug)]
struct Inode {
    parent: u32,
    links: u32,
    holds: u32,
    permissions: u32,
    modified: u64,
    socket: bool,
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

const SOCKET_BIT: u64 = 1 << 33;

const LINK_BIT: u64 = 1 << 34;

const GENERATION_SHIFT: u32 = 35;

const GENERATION_MASK: u32 = (1 << (64 - GENERATION_SHIFT)) - 1;

/// The most names a file or link has (`EMLINK` past it), as EggFS allows.
pub const MAX_LINKS: u32 = 65_000;

/// The longest symbolic link target kept, in bytes (`PATH_MAX` without its NUL).
pub const MAX_LINK_TARGET: usize = super::PATH_MAX - 1;

fn number(slot: u32, generation: u32, kind: NodeKind) -> u64 {
    let kind = match kind {
        NodeKind::Directory => DIRECTORY_BIT,
        NodeKind::Socket => SOCKET_BIT,
        NodeKind::SymbolicLink => LINK_BIT,
        NodeKind::Regular | NodeKind::CharacterDevice => 0,
    };
    u64::from(generation & GENERATION_MASK) << GENERATION_SHIFT | kind | u64::from(slot)
}

fn slot_of(node: u64) -> (usize, u32) {
    (node as u32 as usize, (node >> GENERATION_SHIFT) as u32)
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
                links: 1,
                holds: 0,
                permissions,
                modified: 0,
                socket: false,
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
        let kind = match entry.inode.as_ref() {
            Some(Inode {
                contents: Contents::Directory(_),
                ..
            }) => NodeKind::Directory,
            Some(Inode {
                contents: Contents::Link(_),
                ..
            }) => NodeKind::SymbolicLink,
            Some(Inode { socket: true, .. }) => NodeKind::Socket,
            _ => NodeKind::Regular,
        };
        number(slot, entry.generation, kind)
    }

    fn children(&self, directory: u64) -> Result<&[Child], Refused> {
        match &self.inode(directory).ok_or(Refused::NotFound)?.contents {
            Contents::Directory(children) => Ok(children),
            Contents::File { .. } | Contents::Link(_) => Err(Refused::NotDirectory),
        }
    }

    fn children_of_slot(&self, slot: u32) -> &[Child] {
        match self.slots[slot as usize].inode.as_ref() {
            Some(Inode {
                contents: Contents::Directory(children),
                ..
            }) => children,
            _ => &[],
        }
    }

    fn child(&self, directory: u64, name: &[u8]) -> Result<Option<u32>, Refused> {
        Ok(self
            .children(directory)?
            .iter()
            .find(|child| child.name == name)
            .map(|child| child.slot))
    }

    fn name_of(&self, slot: u32) -> &[u8] {
        let Some(inode) = self.slots[slot as usize].inode.as_ref() else {
            return &[];
        };
        self.children_of_slot(inode.parent)
            .iter()
            .find(|child| child.slot == slot)
            .map_or(&[][..], |child| &child.name)
    }

    fn is_directory_slot(&self, slot: u32) -> bool {
        self.number_of(slot) & DIRECTORY_BIT != 0
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

    fn detach(&mut self, parent: u32, name: &[u8]) -> Option<Child> {
        let Some(Inode {
            contents: Contents::Directory(children),
            ..
        }) = self.slots[parent as usize].inode.as_mut()
        else {
            return None;
        };
        let at = children.iter().position(|child| child.name == name)?;
        Some(children.remove(at))
    }

    fn holder_of(&self, slot: u32) -> Option<u32> {
        (0..self.slots.len() as u32).find(|&directory| {
            self.children_of_slot(directory)
                .iter()
                .any(|child| child.slot == slot)
        })
    }

    fn forget_name(&mut self, slot: u32, directory: u32, budget: &Budget) {
        if self.is_directory_slot(slot) {
            self.free_slot(slot, budget);
            return;
        }
        let Some(inode) = self.slots[slot as usize].inode.as_mut() else {
            return;
        };
        inode.links = inode.links.saturating_sub(1);
        let (links, holds, parent) = (inode.links, inode.holds, inode.parent);
        if links == 0 {
            if holds == 0 {
                self.free_slot(slot, budget);
            }
            return;
        }
        let still_named_there = self
            .children_of_slot(directory)
            .iter()
            .any(|child| child.slot == slot);
        if parent == directory
            && !still_named_there
            && let Some(holder) = self.holder_of(slot)
            && let Some(inode) = self.slots[slot as usize].inode.as_mut()
        {
            inode.parent = holder;
        }
    }

    fn free_slot(&mut self, slot: u32, budget: &Budget) {
        let entry = &mut self.slots[slot as usize];
        if let Some(inode) = entry.inode.take() {
            let contents = match &inode.contents {
                Contents::File { pages, .. } => {
                    pages.iter().flatten().count() * PAGE_SIZE + index_cost(pages)
                }
                Contents::Link(target) => target.len(),
                Contents::Directory(_) => 0,
            };
            budget.give_back(contents + NODE_COST);
        }
        entry.generation = entry.generation.wrapping_add(1) & GENERATION_MASK;
        if self.free.try_reserve(1).is_ok() {
            self.free.push(slot);
        }
    }

    fn add_inode(&mut self, inode: Inode) -> u32 {
        match self.free.pop() {
            Some(slot) => {
                self.slots[slot as usize].inode = Some(inode);
                slot
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    inode: Some(inode),
                });
                (self.slots.len() - 1) as u32
            }
        }
    }

    fn reserve_entry(&mut self, directory: u64) -> bool {
        self.slots.try_reserve(1).is_ok()
            && match self.inode_mut(directory) {
                Some(Inode {
                    contents: Contents::Directory(children),
                    ..
                }) => children.try_reserve(1).is_ok(),
                _ => false,
            }
    }

    fn add_child(&mut self, directory: u64, child: Child, now: u64) {
        if let Some(parent) = self.inode_mut(directory) {
            if let Contents::Directory(children) = &mut parent.contents {
                children.push(child);
            }
            parent.modified = now;
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

impl Tmpfs {
    fn make(
        &self,
        directory: u64,
        name: &[u8],
        contents: Contents,
        permissions: u32,
        socket: bool,
        now: u64,
    ) -> Result<u64, Refused> {
        let extra_cost = match &contents {
            Contents::Link(target) => target.len(),
            Contents::Directory(_) | Contents::File { .. } => 0,
        };
        let mut state = self.state.lock();
        if state.child(directory, name)?.is_some() {
            return Err(Refused::Exists);
        }
        let parent = slot_of(directory).0 as u32;
        let cost = NODE_COST + name.len() + extra_cost;
        self.budget.take(cost)?;
        let mut stored = Vec::new();
        if stored.try_reserve_exact(name.len()).is_err() || !state.reserve_entry(directory) {
            self.budget.give_back(cost);
            return Err(Refused::NoSpace);
        }
        stored.extend_from_slice(name);
        let slot = state.add_inode(Inode {
            parent,
            links: 1,
            holds: 0,
            permissions: permissions & 0o7777,
            modified: now,
            socket,
            contents,
        });
        state.add_child(directory, Child { slot, name: stored }, now);
        Ok(state.number_of(slot))
    }
}

impl FileSystem for Tmpfs {
    fn root(&self) -> u64 {
        DIRECTORY_BIT
    }

    fn device(&self) -> (u32, u32) {
        self.device
    }

    fn kind(&self, node: u64) -> NodeKind {
        if node & DIRECTORY_BIT != 0 {
            NodeKind::Directory
        } else if node & SOCKET_BIT != 0 {
            NodeKind::Socket
        } else if node & LINK_BIT != 0 {
            NodeKind::SymbolicLink
        } else {
            NodeKind::Regular
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
                    .filter(|child| state.is_directory_slot(child.slot))
                    .count();
                (2 + subdirectories as u32, 0)
            }
            Contents::File { size, .. } => (inode.links, *size),
            Contents::Link(target) => (inode.links, target.len() as u64),
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
        match state.inode(node) {
            Some(_) => Name::new(state.name_of(slot_of(node).0 as u32)),
            None => Name::new(b""),
        }
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn copies_contents(&self) -> bool {
        true
    }

    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
        let state = self.state.lock();
        let Some(Inode {
            contents: Contents::File { pages, size },
            ..
        }) = state.inode(node)
        else {
            return Ok(0);
        };
        if offset >= *size {
            return Ok(0);
        }
        let wanted = buffer
            .len()
            .min((*size - offset).min(usize::MAX as u64) as usize);
        let (page, within, length) = page_range(offset, wanted);
        match pages.get(page).and_then(Option::as_ref) {
            Some(page) => buffer[..length].copy_from_slice(&page[within..within + length]),
            None => buffer[..length].fill(0),
        }
        Ok(length)
    }

    fn link_target(&self, node: u64, _caller: u32) -> Option<Vec<u8>> {
        let state = self.state.lock();
        let Some(Inode {
            contents: Contents::Link(target),
            ..
        }) = state.inode(node)
        else {
            return None;
        };
        let mut copy = Vec::new();
        copy.try_reserve_exact(target.len()).ok()?;
        copy.extend_from_slice(target);
        Some(copy)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let state = self.state.lock();
        let child = state
            .children(directory)
            .ok()?
            .get(usize::try_from(position).ok()?)?;
        let node = state.number_of(child.slot);
        Some(DirectoryEntry {
            name: Name::new(&child.name),
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
        let (pages, size) = match &mut inode.contents {
            Contents::File { pages, size } => (pages, size),
            Contents::Directory(_) => return Err(Refused::IsDirectory),
            Contents::Link(_) => return Err(Refused::InvalidArgument),
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
        let index_before = pages.len();
        while written < bytes.len() {
            let (page, within, length) = page_range(offset + written as u64, bytes.len() - written);
            if pages.len() <= page && !grow_index(self.budget, pages, page + 1) {
                break;
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
        let reached = offset + written as u64;
        let needed = index_before.max((reached.max(*size) as usize).div_ceil(PAGE_SIZE));
        if pages.len() > needed {
            let charged = index_cost(pages);
            pages.truncate(needed);
            pages.shrink_to_fit();
            settle_index(self.budget, pages, charged);
        }
        if written == 0 && !bytes.is_empty() {
            return Err(Refused::NoSpace);
        }
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
        let (pages, size) = match &mut inode.contents {
            Contents::File { pages, size } => (pages, size),
            Contents::Directory(_) => return Err(Refused::IsDirectory),
            Contents::Link(_) => return Err(Refused::InvalidArgument),
        };
        let kept = (length as usize).div_ceil(PAGE_SIZE);
        if kept < pages.len() {
            let freed = pages[kept..].iter().flatten().count();
            let charged = index_cost(pages);
            pages.truncate(kept);
            pages.shrink_to_fit();
            self.budget.give_back(freed * PAGE_SIZE);
            settle_index(self.budget, pages, charged);
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
        let contents = match kind {
            NodeKind::CharacterDevice | NodeKind::SymbolicLink => {
                return Err(Refused::InvalidArgument);
            }
            NodeKind::Directory => Contents::Directory(Vec::new()),
            NodeKind::Regular | NodeKind::Socket => Contents::File {
                pages: Vec::new(),
                size: 0,
            },
        };
        let socket = kind == NodeKind::Socket;
        self.make(directory, name, contents, permissions, socket, now)
    }

    fn symlink(
        &self,
        directory: u64,
        name: &[u8],
        target: &[u8],
        now: u64,
    ) -> Result<u64, Refused> {
        check_name(name)?;
        if target.is_empty() || target.len() > MAX_LINK_TARGET || target.contains(&0) {
            return Err(Refused::InvalidArgument);
        }
        let mut stored = Vec::new();
        stored
            .try_reserve_exact(target.len())
            .map_err(|_| Refused::NoSpace)?;
        stored.extend_from_slice(target);
        let contents = Contents::Link(stored.into_boxed_slice());
        self.make(directory, name, contents, 0o777, false, now)
    }

    fn link(&self, node: u64, directory: u64, name: &[u8], now: u64) -> Result<(), Refused> {
        check_name(name)?;
        let mut state = self.state.lock();
        let inode = state.inode(node).ok_or(Refused::NotFound)?;
        if matches!(inode.contents, Contents::Directory(_)) {
            return Err(Refused::NotPermitted);
        }
        if inode.links == 0 {
            return Err(Refused::NotFound);
        }
        if inode.links >= MAX_LINKS {
            return Err(Refused::TooManyLinks);
        }
        if state.child(directory, name)?.is_some() {
            return Err(Refused::Exists);
        }
        self.budget.take(name.len())?;
        let mut stored = Vec::new();
        if stored.try_reserve_exact(name.len()).is_err() || !state.reserve_entry(directory) {
            self.budget.give_back(name.len());
            return Err(Refused::NoSpace);
        }
        stored.extend_from_slice(name);
        let slot = slot_of(node).0 as u32;
        if let Some(inode) = state.inode_mut(node) {
            inode.links += 1;
        }
        state.add_child(directory, Child { slot, name: stored }, now);
        Ok(())
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
            (Contents::File { .. } | Contents::Link(_), true) => {
                return Err(Refused::NotDirectory);
            }
            _ => {}
        }
        let parent = slot_of(directory).0 as u32;
        if let Some(child) = state.detach(parent, name) {
            self.budget.give_back(child.name.len());
        }
        state.forget_name(slot, parent, self.budget);
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
        let moved_is_directory = state.is_directory_slot(moved);
        let to_slot = slot_of(to).0 as u32;
        if moved_is_directory && state.is_ancestor(moved, to_slot) {
            return Err(Refused::InvalidArgument);
        }
        let replaced = state.child(to, to_name)?;
        if let Some(replaced) = replaced {
            if replaced == moved {
                return Ok(());
            }
            if no_replace {
                return Err(Refused::Exists);
            }
            let replaced_is_directory = state.is_directory_slot(replaced);
            match (moved_is_directory, replaced_is_directory) {
                (true, false) => return Err(Refused::NotDirectory),
                (false, true) => return Err(Refused::IsDirectory),
                (true, true) if !state.children(state.number_of(replaced))?.is_empty() => {
                    return Err(Refused::NotEmpty);
                }
                _ => {}
            }
        }
        let old_length = from_name.len();
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
        if let Some(replaced) = replaced {
            if let Some(child) = state.detach(to_slot, to_name) {
                self.budget.give_back(child.name.len());
            }
            state.forget_name(replaced, to_slot, self.budget);
        }
        name.extend_from_slice(to_name);
        let from_slot = slot_of(from).0 as u32;
        state.detach(from_slot, from_name);
        if let Some(Inode {
            contents: Contents::Directory(children),
            modified,
            ..
        }) = state.inode_mut(to)
        {
            children.push(Child { slot: moved, name });
            *modified = now;
        }
        if let Some(inode) = state.slots[moved as usize].inode.as_mut() {
            inode.parent = to_slot;
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

    fn set_permissions(&self, node: u64, permissions: u32) -> Result<(), Refused> {
        let mut state = self.state.lock();
        let inode = state.inode_mut(node).ok_or(Refused::NotFound)?;
        if matches!(inode.contents, Contents::Link(_)) {
            return Err(Refused::InvalidArgument);
        }
        inode.permissions = permissions & 0o7777;
        Ok(())
    }

    fn hold(&self, node: u64) {
        let mut state = self.state.lock();
        if let Some(inode) = state.inode_mut(node)
            && !matches!(inode.contents, Contents::Directory(_))
        {
            inode.holds = inode.holds.saturating_add(1);
        }
    }

    fn release(&self, node: u64) {
        let mut state = self.state.lock();
        let Some(inode) = state.inode_mut(node) else {
            return;
        };
        if matches!(inode.contents, Contents::Directory(_)) {
            return;
        }
        inode.holds = inode.holds.saturating_sub(1);
        if inode.holds == 0 && inode.links == 0 {
            state.free_slot(slot_of(node).0 as u32, self.budget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfs(limit: usize) -> &'static Tmpfs {
        let budget = Box::leak(Box::new(Budget::new(limit)));
        Box::leak(Box::new(Tmpfs::new(budget, (0, 30), 0o1777).unwrap()))
    }

    fn index_charged(fs: &Tmpfs, node: u64) -> usize {
        match &fs.state.lock().inode(node).unwrap().contents {
            Contents::File { pages, .. } => index_cost(pages),
            Contents::Directory(_) | Contents::Link(_) => 0,
        }
    }

    fn read_all(fs: &Tmpfs, node: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buffer = [0u8; 1000];
        loop {
            let read = fs.read_into(node, out.len() as u64, &mut buffer).unwrap();
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
        assert_eq!(
            fs.budget.used(),
            NODE_COST + 1 + 3 * PAGE_SIZE + index_charged(fs, file)
        );
    }

    #[test]
    fn writing_past_the_end_leaves_zeros_and_truncating_frees_pages() {
        let fs = tmpfs(1 << 20);
        let file = fs
            .create(fs.root(), b"sparse", NodeKind::Regular, 0o600, 0)
            .unwrap();
        fs.write(file, WriteAt::Offset(3 * PAGE_SIZE as u64 + 1), b"z", 0)
            .unwrap();
        assert!(index_charged(fs, file) >= 4 * INDEX_ENTRY_COST);
        assert_eq!(
            fs.budget.used(),
            NODE_COST + 6 + PAGE_SIZE + index_charged(fs, file)
        );
        let back = read_all(fs, file);
        assert_eq!(back.len(), 3 * PAGE_SIZE + 2);
        assert!(back[..3 * PAGE_SIZE + 1].iter().all(|&byte| byte == 0));
        fs.truncate(file, 1, 0).unwrap();
        assert_eq!(read_all(fs, file), b"\0");
        assert_eq!(fs.budget.used(), NODE_COST + 6 + index_charged(fs, file));
        assert_eq!(index_charged(fs, file), INDEX_ENTRY_COST);
        fs.write(file, WriteAt::Offset(0), b"abcdef", 0).unwrap();
        fs.truncate(file, 2, 0).unwrap();
        fs.truncate(file, 4, 0).unwrap();
        assert_eq!(read_all(fs, file), b"ab\0\0");
    }

    #[test]
    fn the_budget_bounds_what_is_kept_and_removing_gives_it_back() {
        let fs = tmpfs(NODE_COST + 1 + 2 * PAGE_SIZE + 8 * INDEX_ENTRY_COST);
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
    fn a_write_far_past_the_end_pays_for_its_page_index_and_fails_past_the_budget() {
        let fs = tmpfs(64 * 1024);
        let file = fs
            .create(fs.root(), b"far", NodeKind::Regular, 0o644, 0)
            .unwrap();
        let base = fs.budget.used();
        assert_eq!(
            fs.write(file, WriteAt::Offset(1 << 30), b"x", 0),
            Err(Refused::NoSpace)
        );
        assert_eq!(fs.budget.used(), base);
        assert_eq!(index_charged(fs, file), 0);
        assert_eq!(fs.attributes(file).size, 0);
        let offset = 1000 * PAGE_SIZE as u64;
        assert_eq!(
            fs.write(file, WriteAt::Offset(offset), b"x", 0),
            Ok((1, offset + 1))
        );
        assert!(index_charged(fs, file) >= 1001 * INDEX_ENTRY_COST);
        assert_eq!(fs.budget.used(), base + PAGE_SIZE + index_charged(fs, file));
        fs.remove(fs.root(), b"far", false, 0).unwrap();
        assert_eq!(fs.budget.used(), 0);
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
    fn a_rename_without_room_for_its_new_name_keeps_what_it_would_replace() {
        let fs = tmpfs(2 * NODE_COST + 2);
        let root = fs.root();
        fs.create(root, b"a", NodeKind::Regular, 0o644, 0).unwrap();
        let kept = fs.create(root, b"b", NodeKind::Regular, 0o644, 0).unwrap();
        assert_eq!(
            fs.rename(root, b"a", root, b"b-longer", false, 0),
            Err(Refused::NoSpace)
        );
        fs.create(root, b"bb", NodeKind::Regular, 0o644, 0)
            .unwrap_err();
        assert_eq!(fs.lookup(root, b"b"), Some(kept));
        fs.rename(root, b"a", root, b"b", false, 0).unwrap();
        assert_eq!(fs.budget.used(), NODE_COST + 1);
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

    #[test]
    fn socket_nodes_have_their_kind_in_their_number_and_go_like_files() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let socket = fs.create(root, b"s", NodeKind::Socket, 0o755, 3).unwrap();
        assert_eq!(fs.kind(socket), NodeKind::Socket);
        assert_eq!(fs.lookup(root, b"s"), Some(socket));
        let attributes = fs.attributes(socket);
        assert_eq!((attributes.kind, attributes.size), (NodeKind::Socket, 0));
        assert_eq!(fs.entry(root, 0).unwrap().kind, NodeKind::Socket);
        assert_eq!(
            fs.create(root, b"s", NodeKind::Socket, 0o755, 3),
            Err(Refused::Exists)
        );
        assert_eq!(fs.remove(root, b"s", true, 4), Err(Refused::NotDirectory));
        fs.remove(root, b"s", false, 4).unwrap();
        assert_eq!(fs.lookup(root, b"s"), None);
        let again = fs.create(root, b"s", NodeKind::Socket, 0o755, 5).unwrap();
        assert_ne!(again, socket);
        assert_eq!(fs.attributes(socket).links, 0);
    }

    #[test]
    fn a_slot_s_generation_wraps_within_its_bits_and_its_last_number_no_longer_matches() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let first = fs.create(root, b"f", NodeKind::Regular, 0o644, 0).unwrap();
        let (slot, _) = slot_of(first);
        fs.state.lock().slots[slot].generation = GENERATION_MASK;
        let last = fs.state.lock().number_of(slot as u32);
        assert_eq!(slot_of(last), (slot, GENERATION_MASK));
        assert_eq!(fs.kind(last), NodeKind::Regular);
        assert_eq!(fs.attributes(last).links, 1);
        fs.remove(root, b"f", false, 0).unwrap();
        let wrapped = fs.create(root, b"g", NodeKind::Socket, 0o644, 0).unwrap();
        assert_eq!(slot_of(wrapped), (slot, 0));
        assert_eq!(fs.kind(wrapped), NodeKind::Socket);
        assert_eq!(fs.attributes(wrapped).links, 1);
        assert_eq!(fs.attributes(last).links, 0);
        assert_ne!(wrapped, first);
    }

    #[test]
    fn symbolic_links_keep_their_target_and_go_like_files() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let link = fs.symlink(root, b"l", b"../x/y", 4).unwrap();
        assert_eq!(fs.kind(link), NodeKind::SymbolicLink);
        assert_eq!(fs.lookup(root, b"l"), Some(link));
        assert_eq!(fs.link_target(link, 0), Some(b"../x/y".to_vec()));
        let attributes = fs.attributes(link);
        assert_eq!(
            (attributes.size, attributes.permissions, attributes.links),
            (6, 0o777, 1)
        );
        assert_eq!(fs.entry(root, 0).unwrap().kind, NodeKind::SymbolicLink);
        assert_eq!(fs.budget.used(), NODE_COST + 1 + 6);
        assert_eq!(fs.symlink(root, b"l", b"z", 4), Err(Refused::Exists));
        for bad in [&b""[..], b"a\0b"] {
            assert_eq!(
                fs.symlink(root, b"m", bad, 4),
                Err(Refused::InvalidArgument)
            );
        }
        let long = vec![b'a'; MAX_LINK_TARGET + 1];
        assert_eq!(
            fs.symlink(root, b"m", &long, 4),
            Err(Refused::InvalidArgument)
        );
        assert_eq!(
            fs.write(link, WriteAt::End, b"x", 5),
            Err(Refused::InvalidArgument)
        );
        assert_eq!(
            fs.set_permissions(link, 0o600),
            Err(Refused::InvalidArgument)
        );
        assert_eq!(fs.remove(root, b"l", true, 5), Err(Refused::NotDirectory));
        fs.remove(root, b"l", false, 5).unwrap();
        assert_eq!(fs.link_target(link, 0), None);
        assert_eq!(fs.budget.used(), 0);
    }

    #[test]
    fn hard_links_count_names_and_the_file_goes_with_its_last_name() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let dir = fs
            .create(root, b"d", NodeKind::Directory, 0o755, 0)
            .unwrap();
        let file = fs.create(root, b"a", NodeKind::Regular, 0o644, 0).unwrap();
        fs.write(file, WriteAt::End, b"data", 1).unwrap();
        fs.link(file, dir, b"b", 2).unwrap();
        fs.link(file, root, b"c", 2).unwrap();
        assert_eq!(fs.lookup(dir, b"b"), Some(file));
        assert_eq!(fs.attributes(file).links, 3);
        assert_eq!(fs.link(file, dir, b"b", 2), Err(Refused::Exists));
        assert_eq!(fs.link(dir, root, b"e", 2), Err(Refused::NotPermitted));
        assert_eq!(fs.entry(dir, 0).unwrap().name.as_bytes(), b"b");
        fs.remove(root, b"a", false, 3).unwrap();
        assert_eq!(fs.attributes(file).links, 2);
        assert_eq!(read_all(fs, file), b"data");
        assert_eq!(fs.name(file).as_bytes(), b"c");
        fs.rename(root, b"c", dir, b"b", false, 4).unwrap();
        assert_eq!(fs.lookup(root, b"c"), Some(file), "both names are one file");
        fs.remove(root, b"c", false, 5).unwrap();
        assert_eq!(fs.parent(file), dir);
        assert_eq!(fs.name(file).as_bytes(), b"b");
        fs.remove(dir, b"b", false, 6).unwrap();
        assert_eq!(fs.attributes(file).links, 0);
        fs.remove(root, b"d", true, 7).unwrap();
        assert_eq!(fs.budget.used(), 0);
        let other = fs.create(root, b"o", NodeKind::Regular, 0o644, 0).unwrap();
        fs.state.lock().inode_mut(other).unwrap().links = MAX_LINKS;
        assert_eq!(fs.link(other, root, b"p", 0), Err(Refused::TooManyLinks));
    }

    #[test]
    fn a_file_removed_while_held_stays_until_its_last_release() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let file = fs.create(root, b"f", NodeKind::Regular, 0o644, 0).unwrap();
        fs.write(file, WriteAt::End, b"kept", 1).unwrap();
        fs.hold(file);
        fs.hold(file);
        fs.remove(root, b"f", false, 2).unwrap();
        assert_eq!(fs.lookup(root, b"f"), None);
        assert_eq!(fs.attributes(file).links, 0);
        assert_eq!(read_all(fs, file), b"kept");
        assert_eq!(fs.write(file, WriteAt::End, b"!", 3), Ok((1, 5)));
        assert_eq!(fs.link(file, root, b"back", 3), Err(Refused::NotFound));
        fs.release(file);
        assert_eq!(read_all(fs, file), b"kept!");
        fs.release(file);
        assert_eq!(read_all(fs, file), b"");
        assert_eq!(fs.budget.used(), 0);
        let replaced = fs.create(root, b"r", NodeKind::Regular, 0o644, 0).unwrap();
        fs.write(replaced, WriteAt::End, b"old", 4).unwrap();
        fs.hold(replaced);
        let new = fs.create(root, b"n", NodeKind::Regular, 0o644, 0).unwrap();
        fs.rename(root, b"n", root, b"r", false, 5).unwrap();
        assert_eq!(fs.lookup(root, b"r"), Some(new));
        assert_eq!(read_all(fs, replaced), b"old");
        fs.release(replaced);
        assert_eq!(fs.attributes(replaced).links, 0);
        let directory = fs
            .create(root, b"d", NodeKind::Directory, 0o755, 0)
            .unwrap();
        fs.hold(directory);
        fs.remove(root, b"d", true, 6).unwrap();
        fs.release(directory);
        assert_eq!(fs.lookup(root, b"d"), None);
    }

    #[test]
    fn permissions_change_on_files_and_directories() {
        let fs = tmpfs(1 << 20);
        let root = fs.root();
        let file = fs.create(root, b"f", NodeKind::Regular, 0o644, 9).unwrap();
        fs.set_permissions(file, 0o10_4755).unwrap();
        let attributes = fs.attributes(file);
        assert_eq!((attributes.permissions, attributes.modified), (0o4755, 9));
        fs.set_permissions(root, 0o700).unwrap();
        assert_eq!(fs.attributes(root).permissions, 0o700);
        fs.remove(root, b"f", false, 0).unwrap();
        assert_eq!(fs.set_permissions(file, 0o600), Err(Refused::NotFound));
    }
}
