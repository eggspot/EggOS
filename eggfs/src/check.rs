use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Block, DIRECT_POINTERS, FIRST_FREE_INODE, Geometry, Inode,
    InodeKind, MAX_DIRECTORY_BLOCKS, MAX_LINK_TARGET, MAX_NAME, POINTERS_PER_BLOCK, ROOT_INODE,
    VERSION, record_at, u32_at,
};
use crate::volume::{MAX_FILE_BYTES, Volume, tree_first};
use crate::{Disk, Error};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// What [`Volume::check`] found, all of it consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckReport {
    /// Directories, the root included.
    pub directories: u64,
    /// Regular files, each counted once however many names it has, orphans not counted.
    pub files: u64,
    /// Symbolic links, counted as files are.
    pub symbolic_links: u64,
    /// Files and symbolic links that have no name and are kept until they are released.
    pub orphans: u64,
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

struct Names {
    more_names: u32,
    parent: u32,
    first_directory: u32,
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

    fn check_contents(
        &mut self,
        number: u32,
        inode: &Inode,
        used: &mut Bits,
    ) -> Result<(), Inconsistency> {
        if inode.size > MAX_FILE_BYTES {
            return Err(wrong("a file is larger than the format allows", number, 0));
        }
        let mapped = inode.size.div_ceil(BLOCK_SIZE as u64);
        self.walk_blocks(
            number,
            inode.direct,
            inode.indirect,
            used,
            &mut |index, block| match index < mapped {
                true => Ok(()),
                false => Err(wrong("a file maps a block past its end", number, block)),
            },
        )?;
        if inode.kind != InodeKind::SymbolicLink {
            return Ok(());
        }
        let length = inode.size as usize;
        if length == 0 || length > MAX_LINK_TARGET {
            return Err(wrong(
                "a symbolic link's target is empty or too long",
                number,
                0,
            ));
        }
        let block = inode.direct[0];
        if block == 0 {
            return Err(wrong("a symbolic link has a hole", number, 0));
        }
        let target = self.read_data_block(block).map_err(io(number, block))?;
        if target[..length].contains(&0) {
            return Err(wrong("a symbolic link's target holds NUL", number, block));
        }
        Ok(())
    }

    fn check_orphans(
        &mut self,
        head: u32,
        seen: &mut Bits,
        used: &mut Bits,
    ) -> Result<u64, Inconsistency> {
        let mut orphans = 0u64;
        let mut number = head;
        while number != 0 {
            if number < FIRST_FREE_INODE || number >= self.geometry.inodes || seen.get(number) {
                return Err(wrong(
                    "the orphan chain names an inode that is reserved, named or listed twice",
                    number,
                    0,
                ));
            }
            seen.set(number);
            let inode = self.inode(number).map_err(io(number, 0))?;
            if !inode.orphan
                || inode.more_names != 0
                || !matches!(inode.kind, InodeKind::Regular | InodeKind::SymbolicLink)
            {
                return Err(wrong(
                    "the orphan chain names an inode that is not an orphan",
                    number,
                    0,
                ));
            }
            self.check_contents(number, &inode, used)?;
            orphans += 1;
            number = inode.next_orphan;
        }
        Ok(orphans)
    }

    fn check_names(
        &mut self,
        named: &BTreeMap<u32, Names>,
        more_records: &BTreeMap<u32, Vec<(u32, InodeKind)>>,
    ) -> Result<(), Inconsistency> {
        for (&number, records) in more_records {
            let inode = self.inode(number).map_err(io(number, 0))?;
            if records.iter().any(|&(_, kind)| kind != inode.kind)
                || inode.kind == InodeKind::Directory
            {
                return Err(wrong(
                    "a directory lists an inode that is reserved or listed elsewhere",
                    number,
                    0,
                ));
            }
            if !named.contains_key(&number) {
                return Err(wrong(
                    "a file has more names than its inode counts",
                    number,
                    0,
                ));
            }
        }
        for (&number, names) in named {
            let records = more_records.get(&number);
            let extra = records.map_or(0, Vec::len);
            if extra as u64 != u64::from(names.more_names) {
                return Err(wrong(
                    "a file's names are not the names its inode counts",
                    number,
                    0,
                ));
            }
            let parent = names.parent;
            let holds_one = names.first_directory == parent
                || records.is_some_and(|records| records.iter().any(|&(at, _)| at == parent));
            if !holds_one {
                return Err(wrong(
                    "an inode's parent is not a directory that lists it",
                    number,
                    0,
                ));
            }
        }
        Ok(())
    }

    /// Checks that everything the volume holds, its uncommitted changes included, is
    /// consistent: every inode reachable from the root reads back with its parent, kind and
    /// counts right (a file's names as its inode counts them), every pointer leads to a data
    /// block used once and within a file's size, symbolic links hold a valid target,
    /// directories have no holes, malformed records or repeated names, the orphan chain holds
    /// exactly the files that have no name, a version 1 volume uses nothing of version 2, and
    /// the bitmaps and free counts mark exactly what is used.
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
            symbolic_links: 0,
            orphans: 0,
            used_blocks: 0,
        };
        let mut named: BTreeMap<u32, Names> = BTreeMap::new();
        let mut more_records: BTreeMap<u32, Vec<(u32, InodeKind)>> = BTreeMap::new();
        let mut pending: Vec<(u32, u32, InodeKind)> =
            alloc::vec![(ROOT_INODE, ROOT_INODE, InodeKind::Directory)];
        let mut orphan_head = 0;
        while let Some((number, parent, kind)) = pending.pop() {
            let inode = self.inode(number).map_err(io(number, 0))?;
            if inode.kind != kind {
                return Err(wrong(
                    "a directory entry's kind is not its inode's",
                    number,
                    0,
                ));
            }
            if inode.orphan || (number != ROOT_INODE && inode.next_orphan != 0) {
                return Err(wrong("a named inode is marked an orphan", number, 0));
            }
            if number == ROOT_INODE {
                orphan_head = inode.next_orphan;
            }
            if kind != InodeKind::Directory {
                match kind {
                    InodeKind::SymbolicLink => report.symbolic_links += 1,
                    _ => report.files += 1,
                }
                if inode.more_names != 0 || inode.parent != parent {
                    let names = Names {
                        more_names: inode.more_names,
                        parent: inode.parent,
                        first_directory: parent,
                    };
                    named.insert(number, names);
                }
                self.check_contents(number, &inode, &mut used)?;
                continue;
            }
            if inode.parent != parent {
                return Err(wrong(
                    "an inode's parent is not the directory that lists it",
                    number,
                    0,
                ));
            }
            if inode.more_names != 0 {
                return Err(wrong("a directory counts more than one name", number, 0));
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
                    let is_directory = record.kind == InodeKind::Directory;
                    if record.inode < 2
                        || record.inode >= geometry.inodes
                        || (seen.get(record.inode) && is_directory)
                    {
                        return Err(wrong(
                            "a directory lists an inode that is reserved or listed elsewhere",
                            number,
                            block,
                        ));
                    }
                    entries += 1;
                    subdirectories += u32::from(is_directory);
                    match seen.get(record.inode) {
                        true => more_records
                            .entry(record.inode)
                            .or_default()
                            .push((number, record.kind)),
                        false => {
                            seen.set(record.inode);
                            pending.push((record.inode, number, record.kind));
                        }
                    }
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
        self.check_names(&named, &more_records)?;
        report.orphans = self.check_orphans(orphan_head, &mut seen, &mut used)?;
        let hard_links = named.values().any(|names| names.more_names > 0);
        if geometry.version < VERSION
            && (report.symbolic_links > 0 || report.orphans > 0 || hard_links)
        {
            return Err(wrong(
                "a version 1 volume holds symbolic links, hard links or orphans",
                0,
                0,
            ));
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
