use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Block, DIRECT_POINTERS, Geometry, InodeKind, MAX_DIRECTORY_BLOCKS,
    MAX_NAME, POINTERS_PER_BLOCK, ROOT_INODE, record_at, u32_at,
};
use crate::volume::{MAX_FILE_BYTES, Volume, tree_first};
use crate::{Disk, Error};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// What [`Volume::check`] found, all of it consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckReport {
    /// Directories, the root included.
    pub directories: u64,
    /// Regular files.
    pub files: u64,
    /// Blocks in use, the file system's own included.
    pub used_blocks: u64,
}

/// The first inconsistency [`Volume::check`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inconsistency {
    /// What is wrong.
    pub what: &'static str,
    /// The inode it was found at, 0 for none.
    pub inode: u32,
    /// The block it was found at, 0 for none.
    pub block: u32,
}

impl core::fmt::Display for Inconsistency {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "{} (inode {}, block {})",
            self.what, self.inode, self.block
        )
    }
}

fn wrong(what: &'static str, inode: u32, block: u32) -> Inconsistency {
    Inconsistency { what, inode, block }
}

struct Bits(Vec<u64>);

impl Bits {
    fn new(bits: u32) -> Self {
        Self(alloc::vec![0; (bits as usize).div_ceil(64)])
    }

    fn get(&self, bit: u32) -> bool {
        self.0[(bit / 64) as usize] & 1 << (bit % 64) != 0
    }

    fn set(&mut self, bit: u32) {
        self.0[(bit / 64) as usize] |= 1 << (bit % 64);
    }
}

fn mark(used: &mut Bits, geometry: &Geometry, block: u32, inode: u32) -> Result<(), Inconsistency> {
    if !geometry.is_data_block(block) {
        return Err(wrong(
            "a pointer leads outside the data blocks",
            inode,
            block,
        ));
    }
    if used.get(block) {
        return Err(wrong("a block is used twice", inode, block));
    }
    used.set(block);
    Ok(())
}

fn io(inode: u32, block: u32) -> impl Fn(Error) -> Inconsistency {
    move |error| match error {
        Error::Io => wrong("the disk failed a read", inode, block),
        _ => wrong("an inode or a block does not read back", inode, block),
    }
}

fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != b"."
        && name != b".."
        && !name.contains(&b'/')
        && !name.contains(&0)
}

impl<D: Disk> Volume<D> {
    fn copy_block(&mut self, block: u32, inode: u32) -> Result<Block, Inconsistency> {
        let data = self
            .cache
            .get(&mut self.disk, block)
            .map_err(io(inode, block))?;
        Ok(*data)
    }

    fn walk_tree(
        &mut self,
        inode: u32,
        block: u32,
        depth: usize,
        first: u64,
        used: &mut Bits,
        visit: &mut dyn FnMut(u64, u32) -> Result<(), Inconsistency>,
    ) -> Result<(), Inconsistency> {
        let pointers = self.copy_block(block, inode)?;
        let child_span = POINTERS_PER_BLOCK.pow(depth as u32);
        for slot in 0..POINTERS_PER_BLOCK {
            let child = u32_at(&pointers, slot as usize * 4);
            if child == 0 {
                continue;
            }
            let index = first + slot * child_span;
            let geometry = self.geometry;
            mark(used, &geometry, child, inode)?;
            match depth {
                0 => visit(index, child)?,
                _ => self.walk_tree(inode, child, depth - 1, index, used, visit)?,
            }
        }
        Ok(())
    }

    fn walk_blocks(
        &mut self,
        number: u32,
        direct: [u32; DIRECT_POINTERS],
        indirect: [u32; 3],
        used: &mut Bits,
        visit: &mut dyn FnMut(u64, u32) -> Result<(), Inconsistency>,
    ) -> Result<(), Inconsistency> {
        let geometry = self.geometry;
        for (index, &block) in direct.iter().enumerate() {
            if block != 0 {
                mark(used, &geometry, block, number)?;
                visit(index as u64, block)?;
            }
        }
        for (level, &root) in indirect.iter().enumerate() {
            if root != 0 {
                mark(used, &geometry, root, number)?;
                self.walk_tree(number, root, level, tree_first(level), used, visit)?;
            }
        }
        Ok(())
    }

    fn bitmap_matches(
        &mut self,
        start: u32,
        blocks: u32,
        used: &Bits,
        count: u32,
        what: &'static str,
    ) -> Result<u64, Inconsistency> {
        let mut clear = 0;
        for index in 0..blocks {
            let bitmap = self.copy_block(start + index, 0)?;
            for bit in 0..BITS_PER_BLOCK {
                let number = index * BITS_PER_BLOCK + bit;
                let marked = bitmap[(bit / 8) as usize] & 1 << (bit % 8) != 0;
                let expected = number >= count || used.get(number);
                if marked != expected {
                    return Err(wrong(what, 0, number));
                }
                if !marked {
                    clear += 1;
                }
            }
        }
        Ok(clear)
    }

    /// Checks that everything the volume holds, its uncommitted changes included, is
    /// consistent: every inode reachable from the root reads back with its parent, kind and
    /// counts right, every pointer leads to a data block used once and within a file's size,
    /// directories have no holes, malformed records or repeated names, and the bitmaps and
    /// free counts mark exactly what is used.
    pub fn check(&mut self) -> Result<CheckReport, Inconsistency> {
        let geometry = self.geometry;
        let mut used = Bits::new(geometry.blocks);
        for block in 0..geometry.data_start {
            used.set(block);
        }
        let mut seen = Bits::new(geometry.inodes);
        seen.set(0);
        seen.set(ROOT_INODE);
        let mut report = CheckReport {
            directories: 0,
            files: 0,
            used_blocks: 0,
        };
        let mut pending: Vec<(u32, u32, bool)> = alloc::vec![(ROOT_INODE, ROOT_INODE, true)];
        while let Some((number, parent, directory)) = pending.pop() {
            let inode = self.inode(number).map_err(io(number, 0))?;
            let expected = match directory {
                true => InodeKind::Directory,
                false => InodeKind::Regular,
            };
            if inode.kind != expected {
                return Err(wrong(
                    "a directory entry's kind is not its inode's",
                    number,
                    0,
                ));
            }
            if inode.parent != parent {
                return Err(wrong(
                    "an inode's parent is not the directory that lists it",
                    number,
                    0,
                ));
            }
            if !directory {
                report.files += 1;
                if inode.size > MAX_FILE_BYTES {
                    return Err(wrong("a file is larger than the format allows", number, 0));
                }
                let mapped = inode.size.div_ceil(BLOCK_SIZE as u64);
                self.walk_blocks(
                    number,
                    inode.direct,
                    inode.indirect,
                    &mut used,
                    &mut |index, block| match index < mapped {
                        true => Ok(()),
                        false => Err(wrong("a file maps a block past its end", number, block)),
                    },
                )?;
                continue;
            }
            report.directories += 1;
            if !inode.size.is_multiple_of(BLOCK_SIZE as u64)
                || inode.size / BLOCK_SIZE as u64 > MAX_DIRECTORY_BLOCKS
            {
                return Err(wrong("a directory's size is not whole blocks", number, 0));
            }
            let length = inode.size / BLOCK_SIZE as u64;
            let mut blocks = Vec::new();
            self.walk_blocks(
                number,
                inode.direct,
                inode.indirect,
                &mut used,
                &mut |index, block| {
                    if index != blocks.len() as u64 || index >= length {
                        return Err(wrong(
                            "a directory has a hole or a block past its end",
                            number,
                            block,
                        ));
                    }
                    blocks.push(block);
                    Ok(())
                },
            )?;
            if blocks.len() as u64 != length {
                return Err(wrong("a directory has a hole", number, 0));
            }
            let mut names = BTreeSet::new();
            let (mut entries, mut subdirectories) = (0u32, 0u32);
            for block in blocks {
                let data = self.copy_block(block, number)?;
                let mut offset = 0;
                loop {
                    let record = record_at(&data, offset)
                        .map_err(|()| wrong("a directory block is malformed", number, block))?;
                    let Some(record) = record else {
                        break;
                    };
                    if !valid_name(record.name) || !names.insert(record.name.to_vec()) {
                        return Err(wrong(
                            "a directory has a bad or repeated name",
                            number,
                            block,
                        ));
                    }
                    if record.inode < 2 || record.inode >= geometry.inodes || seen.get(record.inode)
                    {
                        return Err(wrong(
                            "a directory lists an inode that is reserved or listed elsewhere",
                            number,
                            block,
                        ));
                    }
                    seen.set(record.inode);
                    entries += 1;
                    subdirectories += u32::from(record.directory);
                    pending.push((record.inode, number, record.directory));
                    offset += record.length;
                }
                if data[offset..].iter().any(|&byte| byte != 0) {
                    return Err(wrong(
                        "a directory block has bytes after its last record",
                        number,
                        block,
                    ));
                }
            }
            if entries != inode.entries || subdirectories != inode.child_directories {
                return Err(wrong("a directory's counts are wrong", number, 0));
            }
        }
        let free_blocks = self.bitmap_matches(
            geometry.block_bitmap,
            geometry.block_bitmap_blocks,
            &used,
            geometry.blocks,
            "the block bitmap does not match what is used",
        )?;
        let free_inodes = self.bitmap_matches(
            geometry.inode_bitmap,
            geometry.inode_bitmap_blocks,
            &seen,
            geometry.inodes,
            "the inode bitmap does not match what is used",
        )?;
        if free_blocks != self.free_blocks || free_inodes != self.free_inodes {
            return Err(wrong("the free counts are wrong", 0, 0));
        }
        report.used_blocks = u64::from(geometry.blocks) - free_blocks;
        Ok(report)
    }
}
