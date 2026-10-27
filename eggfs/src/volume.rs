use crate::cache::Cache;
use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Block, DIRECT_POINTERS, Geometry, Inode, InodeKind,
    MAX_DIRECTORY_BLOCKS, MAX_NAME, POINTERS_PER_BLOCK, ROOT_INODE, put_u32, record_at,
    record_length, records_end, remove_record, u32_at, write_record, zeroed_block,
};
use crate::{Disk, Error, Kind, generation_of, inode_of, kind_of, node_number};
use alloc::boxed::Box;

pub const OPERATION_RESERVE: usize = 32;

const RELEASE_RESERVE: usize = 8;

const MAX_RUN_BLOCKS: u64 = 64;

pub(crate) const TREE_SPANS: [u64; 3] = [
    POINTERS_PER_BLOCK,
    POINTERS_PER_BLOCK * POINTERS_PER_BLOCK,
    POINTERS_PER_BLOCK * POINTERS_PER_BLOCK * POINTERS_PER_BLOCK,
];

const MAX_FILE_BLOCKS: u64 = DIRECT_POINTERS as u64 + TREE_SPANS[0] + TREE_SPANS[1] + TREE_SPANS[2];

pub(crate) const MAX_FILE_BYTES: u64 = MAX_FILE_BLOCKS * BLOCK_SIZE as u64;

pub(crate) fn tree_first(level: usize) -> u64 {
    DIRECT_POINTERS as u64 + TREE_SPANS[..level].iter().sum::<u64>()
}

/// A node's attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    /// What it is.
    pub kind: Kind,
    /// Its permission bits.
    pub permissions: u32,
    /// Its inode number.
    pub inode: u32,
    /// Its links: 1 for a file, 2 plus its subdirectories for a directory.
    pub links: u32,
    /// Its owner's user id.
    pub owner: u32,
    /// Its group id.
    pub group: u32,
    /// Its size in bytes; a directory's is its blocks'.
    pub size: u64,
    /// When its contents last changed, in seconds since the Unix epoch.
    pub modified: u64,
}

/// One entry of a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry {
    /// The node it names.
    pub node: u64,
    name: [u8; MAX_NAME],
    length: u8,
}

impl DirectoryEntry {
    /// Its name.
    pub fn name(&self) -> &[u8] {
        &self.name[..usize::from(self.length)]
    }
}

#[derive(Debug, Clone, Copy)]
struct Listing {
    directory: u64,
    changes: u64,
    position: u64,
    block_index: u64,
    offset: usize,
}

#[derive(Debug, Clone, Copy)]
struct Found {
    inode: u32,
    directory: bool,
    block: u32,
    offset: usize,
    length: usize,
}

struct DataCache {
    block: u32,
    data: Box<Block>,
}

/// A mounted EggFS volume on disk `D`: see the crate documentation.
pub struct Volume<D: Disk> {
    pub(crate) disk: D,
    pub(crate) geometry: Geometry,
    pub(crate) cache: Cache,
    pub(crate) free_blocks: u64,
    pub(crate) free_inodes: u64,
    pub(crate) sequence: u64,
    pub(crate) broken: bool,
    pub(crate) commits: u64,
    pub(crate) root_generation: u32,
    block_cursor: u32,
    inode_cursor: u32,
    data: Option<DataCache>,
    listing: Option<Listing>,
    changes: u64,
}

impl<D: Disk> core::fmt::Debug for Volume<D> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Volume")
            .field("blocks", &self.geometry.blocks)
            .field("free_blocks", &self.free_blocks)
            .field("commits", &self.commits)
            .finish_non_exhaustive()
    }
}

fn check_new_name(name: &[u8]) -> Result<(), Error> {
    match name {
        [] | b"." | b".." => Err(Error::Exists),
        _ if name.len() > MAX_NAME => Err(Error::NameTooLong),
        _ if name.contains(&b'/') || name.contains(&0) => Err(Error::InvalidArgument),
        _ => Ok(()),
    }
}

fn kind_matches(inode: &Inode, kind: Kind) -> bool {
    matches!(
        (inode.kind, kind),
        (InodeKind::Directory, Kind::Directory) | (InodeKind::Regular, Kind::Regular)
    )
}

fn kind_of_inode(inode: &Inode) -> Kind {
    match inode.kind {
        InodeKind::Directory => Kind::Directory,
        _ => Kind::Regular,
    }
}

impl<D: Disk> Volume<D> {
    pub(crate) fn assemble(disk: D, geometry: Geometry, sequence: u64) -> Self {
        Self {
            disk,
            geometry,
            cache: Cache::new(),
            free_blocks: 0,
            free_inodes: 0,
            sequence,
            broken: false,
            commits: 0,
            root_generation: 0,
            block_cursor: geometry.data_start,
            inode_cursor: 0,
            data: None,
            listing: None,
            changes: 0,
        }
    }

    /// Gives the disk back.
    pub fn into_disk(self) -> D {
        self.disk
    }

    /// The disk, for its owner to look at.
    pub fn disk(&self) -> &D {
        &self.disk
    }

    /// The root directory's node.
    pub fn root(&self) -> u64 {
        node_number(ROOT_INODE, self.root_generation, Kind::Directory)
    }

    /// Blocks of [`crate::BLOCK_SIZE`] bytes in the volume, and how many are free.
    pub fn space(&self) -> (u64, u64) {
        (u64::from(self.geometry.blocks), self.free_blocks)
    }

    /// Inodes in the volume, and how many are free.
    pub fn inodes(&self) -> (u64, u64) {
        (u64::from(self.geometry.inodes), self.free_inodes)
    }

    /// Whether changes wait for a commit.
    pub fn has_changes(&self) -> bool {
        self.cache.dirty_count() > 0
    }

    /// Transactions committed since the volume was mounted.
    pub fn commits(&self) -> u64 {
        self.commits
    }

    pub(crate) fn inode(&mut self, number: u32) -> Result<Inode, Error> {
        if number == 0 || number >= self.geometry.inodes {
            return Err(Error::Corrupt);
        }
        let (block, at) = self.geometry.inode_location(number);
        let data = self.cache.get(&mut self.disk, block)?;
        Inode::decode(number, &data[at..]).ok_or(Error::Corrupt)
    }

    fn write_inode(&mut self, number: u32, inode: &Inode) -> Result<(), Error> {
        let (block, at) = self.geometry.inode_location(number);
        let data = self.cache.get_mut(&mut self.disk, block, false)?;
        inode.encode(number, &mut data[at..]);
        Ok(())
    }

    fn live(&mut self, node: u64) -> Result<(u32, Inode), Error> {
        let number = inode_of(node);
        if number == 0 || number >= self.geometry.inodes {
            return Err(Error::NotFound);
        }
        let inode = self.inode(number)?;
        if !kind_matches(&inode, kind_of(node))
            || inode.generation & 0x7fff_ffff != generation_of(node)
        {
            return Err(Error::NotFound);
        }
        Ok((number, inode))
    }

    fn node(number: u32, inode: &Inode) -> u64 {
        node_number(number, inode.generation, kind_of_inode(inode))
    }

    fn live_directory(&mut self, node: u64) -> Result<(u32, Inode), Error> {
        if kind_of(node) != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        self.live(node)
    }

    fn writable(&self) -> Result<(), Error> {
        match self.broken {
            true => Err(Error::Io),
            false => Ok(()),
        }
    }

    fn ensure_room(&mut self) -> Result<(), Error> {
        let capacity = self.geometry.transaction_capacity() as usize;
        if self.cache.dirty_count() + OPERATION_RESERVE > capacity {
            self.commit()?;
        }
        Ok(())
    }

    fn bit_location(start: u32, index: u32) -> (u32, usize, u8) {
        (
            start + index / BITS_PER_BLOCK,
            (index % BITS_PER_BLOCK / 8) as usize,
            1 << (index % 8),
        )
    }

    fn set_block_bit(&mut self, block: u32, used: bool) -> Result<(), Error> {
        let (bitmap, byte, mask) = Self::bit_location(self.geometry.block_bitmap, block);
        let data = self.cache.get_mut(&mut self.disk, bitmap, true)?;
        match used {
            true => data[byte] |= mask,
            false => data[byte] &= !mask,
        }
        Ok(())
    }

    fn is_fresh(&self, block: u32) -> bool {
        let (bitmap, byte, mask) = Self::bit_location(self.geometry.block_bitmap, block);
        self.cache
            .committed(bitmap)
            .is_some_and(|committed| committed[byte] & mask == 0)
    }

    fn forget_data(&mut self, block: u32) {
        if self
            .data
            .as_ref()
            .is_some_and(|cached| cached.block == block)
        {
            self.data = None;
        }
    }

    fn allocate_block(&mut self) -> Result<u32, Error> {
        let bitmaps = self.geometry.block_bitmap_blocks;
        let cursor = self.block_cursor.min(self.geometry.blocks - 1);
        let first = cursor / BITS_PER_BLOCK;
        for step in 0..=bitmaps {
            let index = (first + step) % bitmaps;
            let bitmap = self.geometry.block_bitmap + index;
            self.cache.get(&mut self.disk, bitmap)?;
            let current = self.cache.data(bitmap).ok_or(Error::Io)?;
            let committed = self.cache.committed(bitmap).unwrap_or(current);
            let start_byte = match step {
                0 => (cursor % BITS_PER_BLOCK / 8) as usize,
                _ => 0,
            };
            let found = (start_byte..BLOCK_SIZE).find_map(|byte| {
                let taken = current[byte] | committed[byte];
                (taken != 0xff).then(|| byte * 8 + (!taken).trailing_zeros() as usize)
            });
            let Some(bit) = found else {
                continue;
            };
            let block = index * BITS_PER_BLOCK + bit as u32;
            if !self.geometry.is_data_block(block) {
                return Err(Error::Corrupt);
            }
            self.set_block_bit(block, true)?;
            self.free_blocks = self.free_blocks.saturating_sub(1);
            self.block_cursor = block + 1;
            self.forget_data(block);
            return Ok(block);
        }
        Err(Error::NoSpace)
    }

    fn free_block(&mut self, block: u32) -> Result<(), Error> {
        if !self.geometry.is_data_block(block) {
            return Err(Error::Corrupt);
        }
        self.set_block_bit(block, false)?;
        self.free_blocks += 1;
        self.cache.forget(block);
        self.forget_data(block);
        Ok(())
    }

    fn allocate_inode(&mut self) -> Result<u32, Error> {
        let inodes = self.geometry.inodes;
        let bitmaps = self.geometry.inode_bitmap_blocks;
        let first = self.inode_cursor.min(inodes - 1) / BITS_PER_BLOCK;
        for step in 0..=bitmaps {
            let index = (first + step) % bitmaps;
            let bitmap = self.geometry.inode_bitmap + index;
            let data = self.cache.get(&mut self.disk, bitmap)?;
            let found = data
                .iter()
                .position(|&byte| byte != 0xff)
                .map(|byte| byte * 8 + (!data[byte]).trailing_zeros() as usize);
            let Some(bit) = found else {
                continue;
            };
            let number = index * BITS_PER_BLOCK + bit as u32;
            if number < crate::layout::FIRST_FREE_INODE || number >= inodes {
                return Err(Error::Corrupt);
            }
            let (bitmap, byte, mask) = Self::bit_location(self.geometry.inode_bitmap, number);
            self.cache.get_mut(&mut self.disk, bitmap, false)?[byte] |= mask;
            self.free_inodes = self.free_inodes.saturating_sub(1);
            self.inode_cursor = number;
            return Ok(number);
        }
        Err(Error::NoSpace)
    }

    fn release_inode_number(&mut self, number: u32) -> Result<(), Error> {
        let (bitmap, byte, mask) = Self::bit_location(self.geometry.inode_bitmap, number);
        self.cache.get_mut(&mut self.disk, bitmap, false)?[byte] &= !mask;
        self.free_inodes += 1;
        Ok(())
    }

    fn pointer_in(&mut self, block: u32, slot: u64) -> Result<u32, Error> {
        if !self.geometry.is_data_block(block) {
            return Err(Error::Corrupt);
        }
        let data = self.cache.get(&mut self.disk, block)?;
        Ok(u32_at(&data[..], slot as usize * 4))
    }

    fn set_pointer_in(&mut self, block: u32, slot: u64, value: u32) -> Result<(), Error> {
        let data = self.cache.get_mut(&mut self.disk, block, false)?;
        put_u32(&mut data[..], slot as usize * 4, value);
        Ok(())
    }

    fn path(index: u64) -> Result<(usize, u64), Error> {
        if index < DIRECT_POINTERS as u64 {
            return Ok((usize::MAX, index));
        }
        for (level, span) in TREE_SPANS.iter().enumerate() {
            let first = tree_first(level);
            if index < first + span {
                return Ok((level, index - first));
            }
        }
        Err(Error::TooBig)
    }

    pub(crate) fn locate(&mut self, inode: &Inode, index: u64) -> Result<u32, Error> {
        let (level, within) = Self::path(index)?;
        if level == usize::MAX {
            return Ok(inode.direct[within as usize]);
        }
        let mut block = inode.indirect[level];
        for depth in (0..=level).rev() {
            if block == 0 {
                return Ok(0);
            }
            let slot = within / POINTERS_PER_BLOCK.pow(depth as u32) % POINTERS_PER_BLOCK;
            block = self.pointer_in(block, slot)?;
        }
        Ok(block)
    }

    fn new_pointer_block(&mut self) -> Result<u32, Error> {
        let block = self.allocate_block()?;
        self.cache.insert_zeroed(block);
        Ok(block)
    }

    fn set_mapping(&mut self, inode: &mut Inode, index: u64, value: u32) -> Result<u32, Error> {
        let (level, within) = Self::path(index)?;
        if level == usize::MAX {
            let previous = inode.direct[within as usize];
            inode.direct[within as usize] = value;
            return Ok(previous);
        }
        if inode.indirect[level] == 0 {
            inode.indirect[level] = self.new_pointer_block()?;
        }
        let mut block = inode.indirect[level];
        for depth in (1..=level).rev() {
            let slot = within / POINTERS_PER_BLOCK.pow(depth as u32) % POINTERS_PER_BLOCK;
            let mut child = self.pointer_in(block, slot)?;
            if child == 0 {
                child = self.new_pointer_block()?;
                self.set_pointer_in(block, slot, child)?;
            }
            block = child;
        }
        let slot = within % POINTERS_PER_BLOCK;
        let previous = self.pointer_in(block, slot)?;
        self.set_pointer_in(block, slot, value)?;
        Ok(previous)
    }

    fn room_while_releasing(
        &mut self,
        number: u32,
        inode: &mut Inode,
        index: u64,
    ) -> Result<(), Error> {
        let capacity = self.geometry.transaction_capacity() as usize;
        if self.cache.dirty_count() + RELEASE_RESERVE > capacity {
            inode.size = inode.size.min((index + 1) * BLOCK_SIZE as u64);
            self.write_inode(number, inode)?;
            self.commit()?;
        }
        Ok(())
    }

    fn release_from(&mut self, number: u32, inode: &mut Inode, keep: u64) -> Result<(), Error> {
        for level in (0..TREE_SPANS.len()).rev() {
            let first = tree_first(level);
            let root = inode.indirect[level];
            if root == 0 || keep >= first + TREE_SPANS[level] {
                continue;
            }
            self.release_tree(number, inode, root, level, first, keep)?;
            if first >= keep {
                self.free_block(root)?;
                inode.indirect[level] = 0;
            }
        }
        for index in (keep.min(DIRECT_POINTERS as u64)..DIRECT_POINTERS as u64).rev() {
            let block = inode.direct[index as usize];
            if block != 0 {
                self.room_while_releasing(number, inode, index)?;
                self.free_block(block)?;
                inode.direct[index as usize] = 0;
            }
        }
        Ok(())
    }

    fn release_tree(
        &mut self,
        number: u32,
        inode: &mut Inode,
        block: u32,
        depth: usize,
        first: u64,
        keep: u64,
    ) -> Result<(), Error> {
        let child_span = POINTERS_PER_BLOCK.pow(depth as u32);
        for slot in (0..POINTERS_PER_BLOCK).rev() {
            let child_first = first + slot * child_span;
            if child_first + child_span <= keep {
                break;
            }
            let child = self.pointer_in(block, slot)?;
            if child == 0 {
                continue;
            }
            if depth == 0 {
                self.room_while_releasing(number, inode, child_first)?;
                self.free_block(child)?;
                self.set_pointer_in(block, slot, 0)?;
                continue;
            }
            self.release_tree(number, inode, child, depth - 1, child_first, keep)?;
            if child_first >= keep {
                self.free_block(child)?;
                self.set_pointer_in(block, slot, 0)?;
            }
        }
        Ok(())
    }

    fn read_data_block(&mut self, block: u32) -> Result<&Block, Error> {
        if !self
            .data
            .as_ref()
            .is_some_and(|cached| cached.block == block)
        {
            if !self.geometry.is_data_block(block) {
                return Err(Error::Corrupt);
            }
            let mut data = match self.data.take() {
                Some(cached) => cached.data,
                None => zeroed_block(),
            };
            let read = self.disk.read(u64::from(block), &mut data[..]);
            if read.is_err() {
                return Err(Error::Io);
            }
            self.data = Some(DataCache { block, data });
        }
        Ok(&self.data.as_ref().ok_or(Error::Io)?.data)
    }

    /// Attributes of `node`.
    pub fn attributes(&mut self, node: u64) -> Result<Attributes, Error> {
        let (number, inode) = self.live(node)?;
        let kind = kind_of_inode(&inode);
        Ok(Attributes {
            kind,
            permissions: u32::from(inode.permissions),
            inode: number,
            links: match kind {
                Kind::Directory => 2 + inode.child_directories,
                Kind::Regular => 1,
            },
            owner: inode.owner,
            group: inode.group,
            size: inode.size,
            modified: inode.modified,
        })
    }

    fn directory_blocks(inode: &Inode) -> Result<u64, Error> {
        let blocks = inode.size / BLOCK_SIZE as u64;
        if !inode.size.is_multiple_of(BLOCK_SIZE as u64) || blocks > MAX_DIRECTORY_BLOCKS {
            return Err(Error::Corrupt);
        }
        Ok(blocks)
    }

    fn directory_block(&mut self, inode: &Inode, index: u64) -> Result<u32, Error> {
        match self.locate(inode, index)? {
            0 => Err(Error::Corrupt),
            block if self.geometry.is_data_block(block) => Ok(block),
            _ => Err(Error::Corrupt),
        }
    }

    fn find_entry(
        &mut self,
        directory: &Inode,
        name: &[u8],
        inode: Option<u32>,
    ) -> Result<Option<Found>, Error> {
        for index in 0..Self::directory_blocks(directory)? {
            let block = self.directory_block(directory, index)?;
            let data = self.cache.get(&mut self.disk, block)?;
            let mut offset = 0;
            while let Some(record) = record_at(&data[..], offset).map_err(|()| Error::Corrupt)? {
                if record.name == name && inode.is_none_or(|wanted| wanted == record.inode) {
                    return Ok(Some(Found {
                        inode: record.inode,
                        directory: record.directory,
                        block,
                        offset,
                        length: record.length,
                    }));
                }
                offset += record.length;
            }
        }
        Ok(None)
    }

    fn add_entry(
        &mut self,
        directory: &mut Inode,
        name: &[u8],
        inode: u32,
        is_directory: bool,
    ) -> Result<(), Error> {
        let needed = record_length(name);
        let blocks = Self::directory_blocks(directory)?;
        for index in 0..blocks {
            let block = self.directory_block(directory, index)?;
            let end = records_end(&self.cache.get(&mut self.disk, block)?[..])
                .map_err(|()| Error::Corrupt)?;
            if end + needed <= BLOCK_SIZE {
                let data = self.cache.get_mut(&mut self.disk, block, false)?;
                write_record(&mut data[..], end, inode, is_directory, name);
                self.changes += 1;
                return Ok(());
            }
        }
        if blocks >= MAX_DIRECTORY_BLOCKS {
            return Err(Error::NoSpace);
        }
        let block = self.new_pointer_block()?;
        let data = self.cache.insert_zeroed(block);
        write_record(&mut data[..], 0, inode, is_directory, name);
        if let Err(error) = self.set_mapping(directory, blocks, block) {
            self.free_block(block)?;
            return Err(error);
        }
        directory.size += BLOCK_SIZE as u64;
        self.changes += 1;
        Ok(())
    }

    fn remove_found(&mut self, found: Found) -> Result<(), Error> {
        let data = self.cache.get_mut(&mut self.disk, found.block, false)?;
        let end = records_end(&data[..]).map_err(|()| Error::Corrupt)?;
        if found.offset + found.length > end {
            return Err(Error::Corrupt);
        }
        remove_record(&mut data[..], found.offset, found.length, end);
        self.changes += 1;
        Ok(())
    }

    fn child_node(&mut self, found: &Found) -> Result<u64, Error> {
        if found.inode >= self.geometry.inodes || found.inode < ROOT_INODE {
            return Err(Error::Corrupt);
        }
        let child = self.inode(found.inode)?;
        let expected = match found.directory {
            true => Kind::Directory,
            false => Kind::Regular,
        };
        if !kind_matches(&child, expected) {
            return Err(Error::Corrupt);
        }
        Ok(Self::node(found.inode, &child))
    }

    /// The node `name` names in `directory`, if there is one.
    pub fn lookup(&mut self, directory: u64, name: &[u8]) -> Result<Option<u64>, Error> {
        let (_, inode) = self.live_directory(directory)?;
        match self.find_entry(&inode, name, None)? {
            Some(found) => self.child_node(&found).map(Some),
            None => Ok(None),
        }
    }

    /// The directory that holds `node`; the root for the root.
    pub fn parent(&mut self, node: u64) -> Result<u64, Error> {
        let (number, inode) = self.live(node)?;
        if number == ROOT_INODE {
            return Ok(self.root());
        }
        let parent = self.inode(inode.parent)?;
        if parent.kind != InodeKind::Directory {
            return Err(Error::Corrupt);
        }
        Ok(Self::node(inode.parent, &parent))
    }

    /// Copies the name `node` has in its parent into `out` and returns its length: 0 for the
    /// root.
    pub fn name(&mut self, node: u64, out: &mut [u8; MAX_NAME]) -> Result<usize, Error> {
        let (number, inode) = self.live(node)?;
        if number == ROOT_INODE {
            return Ok(0);
        }
        let parent = self.inode(inode.parent)?;
        if parent.kind != InodeKind::Directory {
            return Err(Error::Corrupt);
        }
        for index in 0..Self::directory_blocks(&parent)? {
            let block = self.directory_block(&parent, index)?;
            let data = self.cache.get(&mut self.disk, block)?;
            let mut offset = 0;
            while let Some(record) = record_at(&data[..], offset).map_err(|()| Error::Corrupt)? {
                if record.inode == number {
                    out[..record.name.len()].copy_from_slice(record.name);
                    return Ok(record.name.len());
                }
                offset += record.length;
            }
        }
        Err(Error::Corrupt)
    }

    /// Entry `position` of `directory`, in the order the directory keeps them.
    pub fn entry(
        &mut self,
        directory: u64,
        position: u64,
    ) -> Result<Option<DirectoryEntry>, Error> {
        let (_, inode) = self.live_directory(directory)?;
        let blocks = Self::directory_blocks(&inode)?;
        let (mut at, mut index, mut offset) = match self.listing {
            Some(listing)
                if listing.directory == directory
                    && listing.changes == self.changes
                    && listing.position <= position =>
            {
                (listing.position, listing.block_index, listing.offset)
            }
            _ => (0, 0, 0),
        };
        while index < blocks {
            let block = self.directory_block(&inode, index)?;
            let data = self.cache.get(&mut self.disk, block)?;
            let record = record_at(&data[..], offset).map_err(|()| Error::Corrupt)?;
            let Some(record) = record else {
                index += 1;
                offset = 0;
                continue;
            };
            if at < position {
                at += 1;
                offset += record.length;
                continue;
            }
            let mut name = [0u8; MAX_NAME];
            name[..record.name.len()].copy_from_slice(record.name);
            let length = record.name.len() as u8;
            let found = Found {
                inode: record.inode,
                directory: record.directory,
                block,
                offset,
                length: record.length,
            };
            let node = self.child_node(&found)?;
            self.listing = Some(Listing {
                directory,
                changes: self.changes,
                position,
                block_index: index,
                offset,
            });
            return Ok(Some(DirectoryEntry { node, name, length }));
        }
        Ok(None)
    }

    /// Copies regular file `node`'s contents from `offset` into `buffer`: how many bytes, 0 at
    /// its end. Whole blocks that lie in a row on the disk go in one read, partial ones through
    /// a one-block cache.
    pub fn read(&mut self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, Error> {
        let (_, inode) = self.live(node)?;
        if inode.kind != InodeKind::Regular {
            return Err(Error::IsDirectory);
        }
        if offset >= inode.size || buffer.is_empty() {
            return Ok(0);
        }
        let wanted = (inode.size - offset).min(buffer.len() as u64) as usize;
        let index = offset / BLOCK_SIZE as u64;
        let within = (offset % BLOCK_SIZE as u64) as usize;
        let block = self.locate(&inode, index)?;
        if within == 0 && wanted >= BLOCK_SIZE && block != 0 {
            let mut count = 1u64;
            while count < MAX_RUN_BLOCKS && (count as usize + 1) * BLOCK_SIZE <= wanted {
                if self.locate(&inode, index + count)? != block + count as u32 {
                    break;
                }
                count += 1;
            }
            if !self.geometry.is_data_block(block)
                || !self.geometry.is_data_block(block + count as u32 - 1)
            {
                return Err(Error::Corrupt);
            }
            let length = count as usize * BLOCK_SIZE;
            self.disk
                .read(u64::from(block), &mut buffer[..length])
                .map_err(|_| Error::Io)?;
            return Ok(length);
        }
        let length = (BLOCK_SIZE - within).min(wanted);
        match block {
            0 => buffer[..length].fill(0),
            block => {
                let data = self.read_data_block(block)?;
                buffer[..length].copy_from_slice(&data[within..within + length]);
            }
        }
        Ok(length)
    }

    /// Makes an empty `kind` named `name` in `directory` with `permissions` at time `now`.
    pub fn create(
        &mut self,
        directory: u64,
        name: &[u8],
        kind: Kind,
        permissions: u32,
        now: u64,
    ) -> Result<u64, Error> {
        check_new_name(name)?;
        self.writable()?;
        self.ensure_room()?;
        let (parent_number, mut parent) = self.live_directory(directory)?;
        if self.find_entry(&parent, name, None)?.is_some() {
            return Err(Error::Exists);
        }
        let number = self.allocate_inode()?;
        let generation = match self.inode(number) {
            Ok(previous) => previous.generation.wrapping_add(1),
            Err(_) => 1,
        };
        let is_directory = kind == Kind::Directory;
        if let Err(error) = self.add_entry(&mut parent, name, number, is_directory) {
            self.release_inode_number(number)?;
            self.write_inode(parent_number, &parent)?;
            return Err(error);
        }
        let inode_kind = match kind {
            Kind::Directory => InodeKind::Directory,
            Kind::Regular => InodeKind::Regular,
        };
        let inode = Inode::new(
            inode_kind,
            (permissions & 0o7777) as u16,
            parent_number,
            generation,
            now,
        );
        self.write_inode(number, &inode)?;
        parent.entries += 1;
        if is_directory {
            parent.child_directories += 1;
        }
        parent.modified = now;
        self.write_inode(parent_number, &parent)?;
        Ok(Self::node(number, &inode))
    }

    fn write_block(
        &mut self,
        inode: &mut Inode,
        index: u64,
        within: usize,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let old = self.locate(inode, index)?;
        let whole = within == 0 && bytes.len() == BLOCK_SIZE;
        if old != 0 && !self.geometry.is_data_block(old) {
            return Err(Error::Corrupt);
        }
        if old != 0 && self.is_fresh(old) {
            self.forget_data(old);
            if whole {
                return self
                    .disk
                    .write(u64::from(old), bytes)
                    .map_err(|_| Error::Io);
            }
            let mut buffer = zeroed_block();
            self.disk
                .read(u64::from(old), &mut buffer[..])
                .map_err(|_| Error::Io)?;
            buffer[within..within + bytes.len()].copy_from_slice(bytes);
            return self
                .disk
                .write(u64::from(old), &buffer[..])
                .map_err(|_| Error::Io);
        }
        let new = self.allocate_block()?;
        let written = match whole {
            true => self.disk.write(u64::from(new), bytes),
            false => {
                let mut buffer = zeroed_block();
                if old != 0 {
                    buffer.copy_from_slice(self.read_data_block(old)?);
                }
                buffer[within..within + bytes.len()].copy_from_slice(bytes);
                self.disk.write(u64::from(new), &buffer[..])
            }
        };
        if written.is_err() {
            self.free_block(new)?;
            return Err(Error::Io);
        }
        match self.set_mapping(inode, index, new) {
            Ok(0) => Ok(()),
            Ok(previous) => self.free_block(previous),
            Err(error) => {
                self.free_block(new)?;
                Err(error)
            }
        }
    }

    /// Writes `bytes` to regular file `node` at `offset`, or at its end for `None`, at time
    /// `now`: how many it wrote, fewer when the disk filled, and the offset after them.
    pub fn write(
        &mut self,
        node: u64,
        offset: Option<u64>,
        bytes: &[u8],
        now: u64,
    ) -> Result<(usize, u64), Error> {
        self.writable()?;
        self.ensure_room()?;
        let (number, mut inode) = self.live(node)?;
        if inode.kind != InodeKind::Regular {
            return Err(Error::IsDirectory);
        }
        let offset = offset.unwrap_or(inode.size);
        offset
            .checked_add(bytes.len() as u64)
            .filter(|&end| end <= MAX_FILE_BYTES)
            .ok_or(Error::TooBig)?;
        let mut written = 0;
        let mut failure = None;
        while written < bytes.len() {
            let position = offset + written as u64;
            if written > 0 {
                let capacity = self.geometry.transaction_capacity() as usize;
                if self.cache.dirty_count() + OPERATION_RESERVE > capacity {
                    inode.size = inode.size.max(position);
                    inode.modified = now;
                    self.write_inode(number, &inode)?;
                    self.commit()?;
                }
            }
            let index = position / BLOCK_SIZE as u64;
            let within = (position % BLOCK_SIZE as u64) as usize;
            let length = (BLOCK_SIZE - within).min(bytes.len() - written);
            match self.write_block(&mut inode, index, within, &bytes[written..written + length]) {
                Ok(()) => written += length,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let reached = offset + written as u64;
        inode.size = inode.size.max(reached);
        if written > 0 {
            inode.modified = now;
        }
        self.write_inode(number, &inode)?;
        match (written, failure) {
            (0, Some(error)) if !bytes.is_empty() => Err(error),
            _ => Ok((written, reached)),
        }
    }

    /// Makes regular file `node` `length` bytes long at time `now`, cutting it or adding zeros.
    pub fn truncate(&mut self, node: u64, length: u64, now: u64) -> Result<(), Error> {
        self.writable()?;
        self.ensure_room()?;
        let (number, mut inode) = self.live(node)?;
        if inode.kind != InodeKind::Regular {
            return Err(Error::IsDirectory);
        }
        if length > MAX_FILE_BYTES {
            return Err(Error::TooBig);
        }
        if length < inode.size {
            let within = (length % BLOCK_SIZE as u64) as usize;
            let index = length / BLOCK_SIZE as u64;
            if within != 0 && self.locate(&inode, index)? != 0 {
                let zeros = [0u8; BLOCK_SIZE];
                if let Err(error) = self.write_block(&mut inode, index, within, &zeros[within..]) {
                    self.write_inode(number, &inode)?;
                    return Err(error);
                }
            }
            self.release_from(number, &mut inode, length.div_ceil(BLOCK_SIZE as u64))?;
        }
        inode.size = length;
        inode.modified = now;
        self.write_inode(number, &inode)
    }

    /// Sets when `node`'s contents last changed.
    pub fn set_modified(&mut self, node: u64, time: u64) -> Result<(), Error> {
        self.writable()?;
        self.ensure_room()?;
        let (number, mut inode) = self.live(node)?;
        inode.modified = time;
        self.write_inode(number, &inode)
    }

    fn discard_inode(&mut self, number: u32) -> Result<(), Error> {
        let mut inode = self.inode(number)?;
        self.release_from(number, &mut inode, 0)?;
        inode.kind = InodeKind::Free;
        inode.size = 0;
        inode.entries = 0;
        inode.child_directories = 0;
        inode.generation = inode.generation.wrapping_add(1);
        self.write_inode(number, &inode)?;
        self.release_inode_number(number)
    }

    /// Removes `name` from `directory` at time `now`: an empty directory if `directories`,
    /// else a file.
    pub fn remove(
        &mut self,
        directory: u64,
        name: &[u8],
        directories: bool,
        now: u64,
    ) -> Result<(), Error> {
        self.writable()?;
        self.ensure_room()?;
        let (parent_number, parent) = self.live_directory(directory)?;
        let found = self
            .find_entry(&parent, name, None)?
            .ok_or(Error::NotFound)?;
        self.child_node(&found)?;
        match (found.directory, directories) {
            (true, false) => return Err(Error::IsDirectory),
            (false, true) => return Err(Error::NotDirectory),
            (true, true) if self.inode(found.inode)?.entries > 0 => return Err(Error::NotEmpty),
            _ => {}
        }
        let mut child = self.inode(found.inode)?;
        self.release_from(found.inode, &mut child, 0)?;
        child.size = 0;
        self.write_inode(found.inode, &child)?;
        self.remove_found(found)?;
        let mut parent = self.inode(parent_number)?;
        parent.entries = parent.entries.saturating_sub(1);
        if found.directory {
            parent.child_directories = parent.child_directories.saturating_sub(1);
        }
        parent.modified = now;
        self.write_inode(parent_number, &parent)?;
        self.discard_inode(found.inode)
    }

    fn is_ancestor(&mut self, ancestor: u32, mut inode: u32) -> Result<bool, Error> {
        for _ in 0..self.geometry.inodes {
            if inode == ancestor {
                return Ok(true);
            }
            if inode == ROOT_INODE {
                return Ok(false);
            }
            inode = self.inode(inode)?.parent;
        }
        Err(Error::Corrupt)
    }

    /// Moves `from_name` in `from` to `to_name` in `to` at time `now`, replacing what is there
    /// unless `no_replace`, as Linux's `rename` does.
    pub fn rename(
        &mut self,
        from: u64,
        from_name: &[u8],
        to: u64,
        to_name: &[u8],
        no_replace: bool,
        now: u64,
    ) -> Result<(), Error> {
        check_new_name(to_name).map_err(|error| match error {
            Error::Exists => Error::Busy,
            error => error,
        })?;
        self.writable()?;
        self.ensure_room()?;
        let (from_number, from_inode) = self.live_directory(from)?;
        let moved = self
            .find_entry(&from_inode, from_name, None)?
            .ok_or(Error::NotFound)?;
        self.child_node(&moved)?;
        let (to_number, to_inode) = self.live_directory(to)?;
        if moved.directory && self.is_ancestor(moved.inode, to_number)? {
            return Err(Error::InvalidArgument);
        }
        let replaced = self.find_entry(&to_inode, to_name, None)?;
        if let Some(replaced) = replaced {
            if replaced.inode == moved.inode {
                return Ok(());
            }
            if no_replace {
                return Err(Error::Exists);
            }
            self.child_node(&replaced)?;
            match (moved.directory, replaced.directory) {
                (true, false) => return Err(Error::NotDirectory),
                (false, true) => return Err(Error::IsDirectory),
                (true, true) if self.inode(replaced.inode)?.entries > 0 => {
                    return Err(Error::NotEmpty);
                }
                _ => {}
            }
        }
        let mut to_inode = to_inode;
        if let Err(error) = self.add_entry(&mut to_inode, to_name, moved.inode, moved.directory) {
            self.write_inode(to_number, &to_inode)?;
            return Err(error);
        }
        if replaced.is_none() {
            to_inode.entries += 1;
            if moved.directory {
                to_inode.child_directories += 1;
            }
        }
        to_inode.modified = now;
        self.write_inode(to_number, &to_inode)?;
        let mut from_inode = self.inode(from_number)?;
        let old = self
            .find_entry(&from_inode, from_name, Some(moved.inode))?
            .ok_or(Error::Corrupt)?;
        self.remove_found(old)?;
        from_inode = self.inode(from_number)?;
        from_inode.entries = from_inode.entries.saturating_sub(1);
        if moved.directory {
            from_inode.child_directories = from_inode.child_directories.saturating_sub(1);
        }
        from_inode.modified = now;
        self.write_inode(from_number, &from_inode)?;
        if let Some(replaced) = replaced {
            let to_inode = self.inode(to_number)?;
            let entry = self
                .find_entry(&to_inode, to_name, Some(replaced.inode))?
                .ok_or(Error::Corrupt)?;
            self.remove_found(entry)?;
            self.discard_inode(replaced.inode)?;
        }
        let mut moved_inode = self.inode(moved.inode)?;
        moved_inode.parent = to_number;
        self.write_inode(moved.inode, &moved_inode)
    }
}
