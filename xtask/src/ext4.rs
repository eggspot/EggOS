//! Reading files out of an ext4 file system image (D188), from the ext4 disk layout the Linux
//! documentation publishes: the superblock, the group descriptors, inodes, extent trees,
//! directories and symbolic links. Read-only, and only what Android's APEX payloads use:
//! extent-mapped files and directories (no block maps, no inline data), fast and slow symbolic
//! links. The test data reader for Android's runtime module (`android_runtime.rs`).

const SUPERBLOCK_AT: usize = 1024;
const MAGIC: u16 = 0xef53;
const ROOT_INODE: u32 = 2;
const INCOMPATIBLE_64BIT: u32 = 0x80;
const SMALL_DESCRIPTOR: usize = 32;
const OLD_INODE_SIZE: usize = 128;
const EXTENTS_FLAG: u32 = 0x8_0000;
const INLINE_DATA_FLAG: u32 = 0x1000_0000;
const EXTENT_MAGIC: u16 = 0xf30a;
const EXTENT_ENTRY: usize = 12;
const UNINITIALIZED_LENGTH: u16 = 32_768;
const INLINE_BLOCK_BYTES: usize = 60;
const TYPE_MASK: u16 = 0xf000;
const TYPE_DIRECTORY: u16 = 0x4000;
const TYPE_FILE: u16 = 0x8000;
const TYPE_LINK: u16 = 0xa000;
const MAX_LINKS_FOLLOWED: usize = 16;
const MAX_EXTENT_DEPTH: u16 = 5;

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Link,
    Other,
}

/// An ext4 image in memory.
#[derive(Debug)]
pub struct Image<'a> {
    bytes: &'a [u8],
    block_size: usize,
    inodes_per_group: u32,
    inode_size: usize,
    descriptor_size: usize,
    descriptors_at: usize,
}

struct Inode<'a> {
    mode: u16,
    size: u64,
    flags: u32,
    block: &'a [u8],
}

impl Inode<'_> {
    fn kind(&self) -> Kind {
        match self.mode & TYPE_MASK {
            TYPE_DIRECTORY => Kind::Directory,
            TYPE_FILE => Kind::File,
            TYPE_LINK => Kind::Link,
            _ => Kind::Other,
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("ext4: a read at {at} is past the end"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("ext4: a read at {at} is past the end"))
}

impl<'a> Image<'a> {
    /// The image in `bytes`; an error if its superblock is not ext4's.
    pub fn open(bytes: &'a [u8]) -> Result<Self, String> {
        let superblock = bytes
            .get(SUPERBLOCK_AT..SUPERBLOCK_AT + 1024)
            .ok_or("ext4: the image is shorter than its superblock")?;
        if u16_at(superblock, 0x38)? != MAGIC {
            return Err("ext4: no ext4 superblock (another file system, such as EROFS?)".into());
        }
        let log_block_size = u32_at(superblock, 0x18)?;
        if log_block_size > 6 {
            return Err(format!(
                "ext4: block size 1024 << {log_block_size} is too large"
            ));
        }
        let block_size = 1024usize << log_block_size;
        let first_data_block = u32_at(superblock, 0x14)? as usize;
        let inodes_per_group = u32_at(superblock, 0x28)?;
        let inode_size = match u32_at(superblock, 0x4c)? {
            0 => OLD_INODE_SIZE,
            _ => usize::from(u16_at(superblock, 0x58)?),
        };
        let descriptor_size = match u32_at(superblock, 0x60)? & INCOMPATIBLE_64BIT {
            0 => SMALL_DESCRIPTOR,
            _ => usize::from(u16_at(superblock, 0xfe)?).max(SMALL_DESCRIPTOR),
        };
        if inodes_per_group == 0 || inode_size < OLD_INODE_SIZE {
            return Err("ext4: the superblock's inode geometry is invalid".into());
        }
        Ok(Self {
            bytes,
            block_size,
            inodes_per_group,
            inode_size,
            descriptor_size,
            descriptors_at: (first_data_block + 1) * block_size,
        })
    }

    fn block(&self, number: u64) -> Result<&'a [u8], String> {
        let start = usize::try_from(number)
            .ok()
            .and_then(|number| number.checked_mul(self.block_size))
            .ok_or_else(|| format!("ext4: block {number} is out of range"))?;
        self.bytes
            .get(start..start + self.block_size)
            .ok_or_else(|| format!("ext4: block {number} is past the end of the image"))
    }

    fn inode(&self, number: u32) -> Result<Inode<'a>, String> {
        let index = number
            .checked_sub(1)
            .ok_or("ext4: inode 0 does not exist")?;
        let group = (index / self.inodes_per_group) as usize;
        let descriptor = self.descriptors_at + group * self.descriptor_size;
        let low = u64::from(u32_at(self.bytes, descriptor + 0x08)?);
        let high = match self.descriptor_size > SMALL_DESCRIPTOR {
            true => u64::from(u32_at(self.bytes, descriptor + 0x28)?),
            false => 0,
        };
        let table = ((high << 32) | low) as usize * self.block_size;
        let at = table + (index % self.inodes_per_group) as usize * self.inode_size;
        let inode = self
            .bytes
            .get(at..at + OLD_INODE_SIZE)
            .ok_or_else(|| format!("ext4: inode {number} is past the end of the image"))?;
        let size = (u64::from(u32_at(inode, 0x6c)?) << 32) | u64::from(u32_at(inode, 0x04)?);
        Ok(Inode {
            mode: u16_at(inode, 0x00)?,
            size,
            flags: u32_at(inode, 0x20)?,
            block: &inode[0x28..0x28 + INLINE_BLOCK_BYTES],
        })
    }

    fn data(&self, inode: &Inode) -> Result<Vec<u8>, String> {
        let size = usize::try_from(inode.size).map_err(|_| "ext4: a file is too large")?;
        if inode.kind() == Kind::Link && inode.flags & EXTENTS_FLAG == 0 {
            return inode
                .block
                .get(..size)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| "ext4: a fast symbolic link is longer than its inode".into());
        }
        if inode.flags & INLINE_DATA_FLAG != 0 || inode.flags & EXTENTS_FLAG == 0 {
            return Err("ext4: only extent-mapped files are read".into());
        }
        let mut data = vec![0u8; size];
        self.copy_extents(inode.block, &mut data, MAX_EXTENT_DEPTH)?;
        Ok(data)
    }

    fn copy_extents(&self, node: &[u8], data: &mut [u8], levels: u16) -> Result<(), String> {
        if u16_at(node, 0)? != EXTENT_MAGIC {
            return Err("ext4: an extent tree node has no extent magic".into());
        }
        let entries = usize::from(u16_at(node, 2)?);
        let depth = u16_at(node, 6)?;
        if depth > levels {
            return Err("ext4: an extent tree is too deep".into());
        }
        for entry in 0..entries {
            let at = EXTENT_ENTRY * (entry + 1);
            let first = u64::from(u32_at(node, at)?);
            if depth > 0 {
                let leaf =
                    (u64::from(u16_at(node, at + 8)?) << 32) | u64::from(u32_at(node, at + 4)?);
                self.copy_extents(self.block(leaf)?, data, depth - 1)?;
                continue;
            }
            let length = u16_at(node, at + 4)?;
            if length > UNINITIALIZED_LENGTH {
                continue;
            }
            let start = (u64::from(u16_at(node, at + 6)?) << 32) | u64::from(u32_at(node, at + 8)?);
            for offset in 0..u64::from(length) {
                let Some(into) = usize::try_from(first + offset)
                    .ok()
                    .and_then(|block| block.checked_mul(self.block_size))
                    .filter(|into| *into < data.len())
                else {
                    break;
                };
                let count = self.block_size.min(data.len() - into);
                data[into..into + count].copy_from_slice(&self.block(start + offset)?[..count]);
            }
        }
        Ok(())
    }

    fn entries(&self, directory: &Inode) -> Result<Vec<(String, u32)>, String> {
        let data = self.data(directory)?;
        let mut entries = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let inode = u32_at(&data, at)?;
            let record_length = usize::from(u16_at(&data, at + 4)?);
            let name_length = usize::from(data[at + 6]);
            if record_length < 8 {
                return Err("ext4: a directory entry has a record length under 8".into());
            }
            if inode != 0 {
                let name = data
                    .get(at + 8..at + 8 + name_length)
                    .ok_or("ext4: a directory entry's name is past its directory")?;
                entries.push((String::from_utf8_lossy(name).into_owned(), inode));
            }
            at += record_length;
        }
        Ok(entries)
    }

    fn lookup(&self, path: &str) -> Result<u32, String> {
        let mut pending: Vec<String> = path
            .split('/')
            .filter(|part| !part.is_empty())
            .rev()
            .map(str::to_owned)
            .collect();
        let mut trail = vec![ROOT_INODE];
        let mut followed = 0;
        while let Some(part) = pending.pop() {
            let directory = *trail.last().unwrap_or(&ROOT_INODE);
            match part.as_str() {
                "." => continue,
                ".." => {
                    if trail.len() > 1 {
                        trail.pop();
                    }
                    continue;
                }
                _ => {}
            }
            let inode = self.inode(directory)?;
            if inode.kind() != Kind::Directory {
                return Err(format!(
                    "ext4: `{path}`: a part before `{part}` is no directory"
                ));
            }
            let number = self
                .entries(&inode)?
                .into_iter()
                .find(|(name, _)| *name == part)
                .map(|(_, number)| number)
                .ok_or_else(|| format!("ext4: `{path}`: no `{part}`"))?;
            let found = self.inode(number)?;
            if found.kind() != Kind::Link {
                trail.push(number);
                continue;
            }
            followed += 1;
            if followed > MAX_LINKS_FOLLOWED {
                return Err(format!("ext4: `{path}`: too many symbolic links"));
            }
            let target = String::from_utf8_lossy(&self.data(&found)?).into_owned();
            if target.starts_with('/') {
                trail.truncate(1);
            }
            pending.extend(
                target
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .rev()
                    .map(str::to_owned),
            );
        }
        Ok(*trail.last().unwrap_or(&ROOT_INODE))
    }

    /// The contents of the file at `path` (from the image's root, symbolic links followed).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let inode = self.inode(self.lookup(path)?)?;
        match inode.kind() {
            Kind::File => self.data(&inode),
            _ => Err(format!("ext4: `{path}` is not a file")),
        }
    }

    /// The names in the directory at `path` and what each is, without `.` and `..`.
    pub fn list(&self, path: &str) -> Result<Vec<(String, Kind)>, String> {
        let inode = self.inode(self.lookup(path)?)?;
        if inode.kind() != Kind::Directory {
            return Err(format!("ext4: `{path}` is not a directory"));
        }
        self.entries(&inode)?
            .into_iter()
            .filter(|(name, _)| name != "." && name != "..")
            .map(|(name, number)| Ok((name, self.inode(number)?.kind())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: usize = 1024;
    const TABLE_BLOCK: usize = 3;
    const INODE_SIZE: usize = 128;

    fn put(image: &mut [u8], at: usize, bytes: &[u8]) {
        image[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn extent_header(entries: u16, depth: u16) -> Vec<u8> {
        let mut header = EXTENT_MAGIC.to_le_bytes().to_vec();
        header.extend(entries.to_le_bytes());
        header.extend(4u16.to_le_bytes());
        header.extend(depth.to_le_bytes());
        header.extend(0u32.to_le_bytes());
        header
    }

    fn leaf(first: u32, length: u16, start: u32) -> Vec<u8> {
        let mut extent = first.to_le_bytes().to_vec();
        extent.extend(length.to_le_bytes());
        extent.extend(0u16.to_le_bytes());
        extent.extend(start.to_le_bytes());
        extent
    }

    fn inode(image: &mut [u8], number: usize, mode: u16, size: usize, flags: u32, block: &[u8]) {
        let at = TABLE_BLOCK * BLOCK + (number - 1) * INODE_SIZE;
        put(image, at, &mode.to_le_bytes());
        put(image, at + 0x04, &(size as u32).to_le_bytes());
        put(image, at + 0x20, &flags.to_le_bytes());
        put(image, at + 0x28, block);
    }

    fn extents_file(image: &mut [u8], number: usize, mode: u16, size: usize, data_block: u32) {
        let mut block = extent_header(1, 0);
        block.extend(leaf(0, 1, data_block));
        inode(image, number, mode, size, EXTENTS_FLAG, &block);
    }

    fn directory(image: &mut [u8], block: usize, entries: &[(&str, u32)]) {
        let mut at = block * BLOCK;
        for (index, (name, number)) in entries.iter().enumerate() {
            let length = match index + 1 == entries.len() {
                true => block * BLOCK + BLOCK - at,
                false => (8 + name.len()).next_multiple_of(4),
            };
            put(image, at, &number.to_le_bytes());
            put(image, at + 4, &(length as u16).to_le_bytes());
            image[at + 6] = name.len() as u8;
            put(image, at + 8, name.as_bytes());
            at += length;
        }
    }

    fn image() -> Vec<u8> {
        let mut image = vec![0u8; 12 * BLOCK];
        put(&mut image, SUPERBLOCK_AT + 0x14, &1u32.to_le_bytes());
        put(&mut image, SUPERBLOCK_AT + 0x28, &16u32.to_le_bytes());
        put(&mut image, SUPERBLOCK_AT + 0x38, &MAGIC.to_le_bytes());
        put(&mut image, SUPERBLOCK_AT + 0x4c, &1u32.to_le_bytes());
        put(
            &mut image,
            SUPERBLOCK_AT + 0x58,
            &(INODE_SIZE as u16).to_le_bytes(),
        );
        put(
            &mut image,
            2 * BLOCK + 0x08,
            &(TABLE_BLOCK as u32).to_le_bytes(),
        );
        extents_file(&mut image, 2, TYPE_DIRECTORY | 0o755, BLOCK, 5);
        directory(
            &mut image,
            5,
            &[
                (".", 2),
                ("..", 2),
                ("lib", 11),
                ("link", 13),
                ("bin", 14),
                ("deep", 15),
            ],
        );
        extents_file(&mut image, 11, TYPE_DIRECTORY | 0o755, BLOCK, 6);
        directory(&mut image, 6, &[(".", 11), ("..", 2), ("libc.so", 12)]);
        extents_file(&mut image, 12, TYPE_FILE | 0o644, 5, 7);
        put(&mut image, 7 * BLOCK, b"hello");
        inode(&mut image, 13, TYPE_LINK | 0o777, 11, 0, b"lib/libc.so");
        inode(&mut image, 14, TYPE_LINK | 0o777, 4, 0, b"/lib");
        let mut root = extent_header(1, 1);
        root.extend(0u32.to_le_bytes());
        root.extend(8u32.to_le_bytes());
        root.extend([0u8; 4]);
        inode(
            &mut image,
            15,
            TYPE_FILE | 0o644,
            BLOCK + 3,
            EXTENTS_FLAG,
            &root,
        );
        let mut node = extent_header(1, 0);
        node.extend(leaf(0, 2, 9));
        put(&mut image, 8 * BLOCK, &node);
        image[9 * BLOCK..10 * BLOCK].fill(b'a');
        put(&mut image, 10 * BLOCK, b"end");
        image
    }

    #[test]
    fn files_are_read_through_directories_and_symbolic_links() {
        let bytes = image();
        let image = Image::open(&bytes).unwrap();
        assert_eq!(image.read("lib/libc.so").unwrap(), b"hello");
        assert_eq!(image.read("/link").unwrap(), b"hello");
        assert_eq!(image.read("bin/libc.so").unwrap(), b"hello");
        assert_eq!(image.read("lib/../bin/./libc.so").unwrap(), b"hello");
        assert!(image.read("lib/missing").is_err());
        assert!(image.read("lib").is_err());
    }

    #[test]
    fn an_extent_tree_with_an_index_level_maps_every_block() {
        let bytes = image();
        let deep = Image::open(&bytes).unwrap().read("deep").unwrap();
        assert_eq!(deep.len(), BLOCK + 3);
        assert!(deep[..BLOCK].iter().all(|byte| *byte == b'a'));
        assert_eq!(&deep[BLOCK..], b"end");
    }

    #[test]
    fn a_directory_lists_its_names_and_kinds() {
        let bytes = image();
        let image = Image::open(&bytes).unwrap();
        assert_eq!(
            image.list("/").unwrap(),
            [
                ("lib".to_owned(), Kind::Directory),
                ("link".to_owned(), Kind::Link),
                ("bin".to_owned(), Kind::Link),
                ("deep".to_owned(), Kind::File),
            ]
        );
        assert_eq!(
            image.list("bin").unwrap(),
            [("libc.so".to_owned(), Kind::File)]
        );
        assert!(image.list("deep").is_err());
    }

    #[test]
    fn an_image_without_the_ext4_magic_is_refused() {
        let mut bytes = image();
        bytes[SUPERBLOCK_AT + 0x38] = 0;
        assert!(Image::open(&bytes).is_err());
        assert!(Image::open(&bytes[..1500]).is_err());
    }
}
