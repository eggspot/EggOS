use crate::{
    ATTRIBUTE_ARCHIVE, ATTRIBUTE_DIRECTORY, DIRECTORY_ENTRY_BYTES, END_OF_CHAIN, Error, File, MAX_FILES, SECTOR_BYTES,
    Writer, short_name,
};

const CLUSTER_MASK: u32 = 0x0fff_ffff;
const FIRST_END_OF_CHAIN: u32 = 0x0fff_fff8;
const BAD_CLUSTER: u32 = 0x0fff_fff7;
const ATTRIBUTE_LONG_NAME: u8 = 0x0f;
const DELETED: u8 = 0xe5;
const ENTRIES_PER_SECTOR: usize = SECTOR_BYTES / DIRECTORY_ENTRY_BYTES;
const MOST_CHAIN: u32 = 1 << 22;

/// Where [`add_files`] reads the volume's bytes: at an offset from the volume's start.
pub type Reader<'a, E> = dyn FnMut(u64, &mut [u8]) -> Result<(), E> + 'a;

#[derive(Debug, Clone, Copy)]
struct Volume {
    sectors_per_cluster: u32,
    reserved: u32,
    fats: u32,
    fat_sectors: u32,
    root_cluster: u32,
    clusters: u32,
    information_sector: u32,
}

impl Volume {
    fn data_start(&self) -> u32 {
        self.reserved + self.fats * self.fat_sectors
    }

    fn sector_of(&self, cluster: u32, sector: u32) -> u64 {
        (u64::from(self.data_start()) + u64::from(cluster - 2) * u64::from(self.sectors_per_cluster) + u64::from(sector))
            * SECTOR_BYTES as u64
    }

    fn fat_offset(&self, copy: u32, cluster: u32) -> u64 {
        u64::from(self.reserved + copy * self.fat_sectors) * SECTOR_BYTES as u64 + u64::from(cluster) * 4
    }

    fn valid(&self, cluster: u32) -> bool {
        (2..self.clusters + 2).contains(&cluster)
    }
}

struct Access<'r, 'w, 'a, E> {
    read: &'r mut Reader<'a, E>,
    write: &'w mut Writer<'a, E>,
    volume: Volume,
    next_free_hint: u32,
}

fn u16_at(bytes: &[u8], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn volume_of(boot: &[u8; SECTOR_BYTES]) -> Option<Volume> {
    let bytes_per_sector = u16_at(boot, 11);
    let sectors_per_cluster = u32::from(boot[13]);
    let reserved = u16_at(boot, 14);
    let fats = u32::from(boot[16]);
    let root_entries = u16_at(boot, 17);
    let fat16_sectors = u16_at(boot, 22);
    let total = match u16_at(boot, 19) {
        0 => u32_at(boot, 32),
        small => small,
    };
    let fat_sectors = u32_at(boot, 36);
    let fat32 = bytes_per_sector == SECTOR_BYTES as u32
        && sectors_per_cluster.is_power_of_two()
        && sectors_per_cluster <= 128
        && reserved > 0
        && (1..=2).contains(&fats)
        && root_entries == 0
        && fat16_sectors == 0
        && fat_sectors > 0
        && boot[510..512] == [0x55, 0xaa];
    if !fat32 {
        return None;
    }
    let data = total.checked_sub(reserved + fats * fat_sectors)?;
    let clusters = (data / sectors_per_cluster).min(fat_sectors * (SECTOR_BYTES as u32 / 4) - 2);
    Some(Volume {
        sectors_per_cluster,
        reserved,
        fats,
        fat_sectors,
        root_cluster: u32_at(boot, 44),
        clusters,
        information_sector: u16_at(boot, 48),
    })
}

impl<E> Access<'_, '_, '_, E> {
    fn read_sector(&mut self, offset: u64, sector: &mut [u8; SECTOR_BYTES]) -> Result<(), Error<E>> {
        (self.read)(offset, sector).map_err(Error::Read)
    }

    fn write_bytes(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error<E>> {
        (self.write)(offset, bytes).map_err(Error::Write)
    }

    fn entry(&mut self, cluster: u32) -> Result<u32, Error<E>> {
        let mut bytes = [0u8; 4];
        (self.read)(self.volume.fat_offset(0, cluster), &mut bytes).map_err(Error::Read)?;
        Ok(u32::from_le_bytes(bytes) & CLUSTER_MASK)
    }

    fn set_entry(&mut self, cluster: u32, value: u32) -> Result<(), Error<E>> {
        let mut old = [0u8; 4];
        (self.read)(self.volume.fat_offset(0, cluster), &mut old).map_err(Error::Read)?;
        let kept = u32::from_le_bytes(old) & !CLUSTER_MASK;
        let bytes = (kept | (value & CLUSTER_MASK)).to_le_bytes();
        for copy in 0..self.volume.fats {
            self.write_bytes(self.volume.fat_offset(copy, cluster), &bytes)?;
        }
        Ok(())
    }

    fn next_in_chain(&mut self, cluster: u32) -> Result<Option<u32>, Error<E>> {
        let next = self.entry(cluster)?;
        match next {
            _ if next >= FIRST_END_OF_CHAIN => Ok(None),
            BAD_CLUSTER => Err(Error::NotFat32),
            _ if self.volume.valid(next) => Ok(Some(next)),
            _ => Err(Error::NotFat32),
        }
    }

    fn free_cluster(&mut self) -> Result<u32, Error<E>> {
        let count = self.volume.clusters;
        for step in 0..count {
            let cluster = 2 + (self.next_free_hint - 2 + step) % count;
            if self.entry(cluster)? == 0 {
                self.next_free_hint = if cluster + 1 < count + 2 { cluster + 1 } else { 2 };
                return Ok(cluster);
            }
        }
        Err(Error::Full)
    }

    fn zero_cluster(&mut self, cluster: u32) -> Result<(), Error<E>> {
        let zero = [0u8; SECTOR_BYTES];
        for sector in 0..self.volume.sectors_per_cluster {
            self.write_bytes(self.volume.sector_of(cluster, sector), &zero)?;
        }
        Ok(())
    }

    fn free_chain(&mut self, first: u32) -> Result<(), Error<E>> {
        let mut cluster = first;
        for _ in 0..MOST_CHAIN {
            if !self.volume.valid(cluster) {
                return Ok(());
            }
            let next = self.next_in_chain(cluster)?;
            self.set_entry(cluster, 0)?;
            match next {
                Some(following) => cluster = following,
                None => return Ok(()),
            }
        }
        Err(Error::NotFat32)
    }

    fn find(&mut self, folder: u32, name: &[u8; 11]) -> Result<Option<(u64, [u8; DIRECTORY_ENTRY_BYTES])>, Error<E>> {
        let mut cluster = folder;
        let mut sector_bytes = [0u8; SECTOR_BYTES];
        for _ in 0..MOST_CHAIN {
            for sector in 0..self.volume.sectors_per_cluster {
                let offset = self.volume.sector_of(cluster, sector);
                self.read_sector(offset, &mut sector_bytes)?;
                for slot in 0..ENTRIES_PER_SECTOR {
                    let entry = &sector_bytes[slot * DIRECTORY_ENTRY_BYTES..(slot + 1) * DIRECTORY_ENTRY_BYTES];
                    if entry[0] == 0 {
                        return Ok(None);
                    }
                    if entry[0] != DELETED && entry[11] != ATTRIBUTE_LONG_NAME && entry[..11] == name[..] {
                        let mut found = [0u8; DIRECTORY_ENTRY_BYTES];
                        found.copy_from_slice(entry);
                        return Ok(Some((offset + (slot * DIRECTORY_ENTRY_BYTES) as u64, found)));
                    }
                }
            }
            match self.next_in_chain(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(None),
            }
        }
        Err(Error::NotFat32)
    }

    fn free_slot(&mut self, folder: u32) -> Result<u64, Error<E>> {
        let mut cluster = folder;
        let mut sector_bytes = [0u8; SECTOR_BYTES];
        for _ in 0..MOST_CHAIN {
            for sector in 0..self.volume.sectors_per_cluster {
                let offset = self.volume.sector_of(cluster, sector);
                self.read_sector(offset, &mut sector_bytes)?;
                for slot in 0..ENTRIES_PER_SECTOR {
                    let first = sector_bytes[slot * DIRECTORY_ENTRY_BYTES];
                    if first == 0 || first == DELETED {
                        return Ok(offset + (slot * DIRECTORY_ENTRY_BYTES) as u64);
                    }
                }
            }
            match self.next_in_chain(cluster)? {
                Some(next) => cluster = next,
                None => {
                    let added = self.free_cluster()?;
                    self.set_entry(added, END_OF_CHAIN)?;
                    self.zero_cluster(added)?;
                    self.set_entry(cluster, added)?;
                    return Ok(self.volume.sector_of(added, 0));
                }
            }
        }
        Err(Error::NotFat32)
    }

    fn put_entry(&mut self, at: u64, name: &[u8; 11], attribute: u8, first: u32, size: u32) -> Result<(), Error<E>> {
        let mut entry = [0u8; DIRECTORY_ENTRY_BYTES];
        entry[..11].copy_from_slice(name);
        entry[11] = attribute;
        entry[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
        entry[24..26].copy_from_slice(&0x5b21u16.to_le_bytes());
        entry[26..28].copy_from_slice(&(first as u16).to_le_bytes());
        entry[28..32].copy_from_slice(&size.to_le_bytes());
        self.write_bytes(at, &entry)
    }

    fn folder(&mut self, parent: u32, name: &[u8; 11]) -> Result<u32, Error<E>> {
        if let Some((_, entry)) = self.find(parent, name)? {
            if entry[11] & ATTRIBUTE_DIRECTORY == 0 {
                return Err(Error::BadName);
            }
            return Ok((u16_at(&entry, 20) << 16) | u16_at(&entry, 26));
        }
        let cluster = self.free_cluster()?;
        self.set_entry(cluster, END_OF_CHAIN)?;
        self.zero_cluster(cluster)?;
        let base = self.volume.sector_of(cluster, 0);
        self.put_entry(base, b".          ", ATTRIBUTE_DIRECTORY, cluster, 0)?;
        let parent_reference = if parent == self.volume.root_cluster { 0 } else { parent };
        self.put_entry(base + DIRECTORY_ENTRY_BYTES as u64, b"..         ", ATTRIBUTE_DIRECTORY, parent_reference, 0)?;
        let slot = self.free_slot(parent)?;
        self.put_entry(slot, name, ATTRIBUTE_DIRECTORY, cluster, 0)?;
        Ok(cluster)
    }

    fn file(&mut self, folder: u32, name: &[u8; 11], bytes: &[u8]) -> Result<(), Error<E>> {
        let existing = self.find(folder, name)?;
        if let Some((_, entry)) = existing {
            if entry[11] & ATTRIBUTE_DIRECTORY != 0 {
                return Err(Error::BadName);
            }
            let first = (u16_at(&entry, 20) << 16) | u16_at(&entry, 26);
            if first != 0 {
                self.free_chain(first)?;
            }
        }
        let cluster_bytes = SECTOR_BYTES * self.volume.sectors_per_cluster as usize;
        let mut first = 0;
        let mut previous = 0;
        for piece in bytes.chunks(cluster_bytes) {
            let cluster = self.free_cluster()?;
            self.set_entry(cluster, END_OF_CHAIN)?;
            if previous == 0 {
                first = cluster;
            } else {
                self.set_entry(previous, cluster)?;
            }
            for (sector, part) in piece.chunks(SECTOR_BYTES).enumerate() {
                let mut block = [0u8; SECTOR_BYTES];
                block[..part.len()].copy_from_slice(part);
                self.write_bytes(self.volume.sector_of(cluster, sector as u32), &block)?;
            }
            previous = cluster;
        }
        let size = u32::try_from(bytes.len()).map_err(|_| Error::TooSmall)?;
        let at = match existing {
            Some((at, _)) => at,
            None => self.free_slot(folder)?,
        };
        self.put_entry(at, name, ATTRIBUTE_ARCHIVE, first, size)
    }
}

/// Adds `files` (paths with `/` between folders, 8.3 names) to an existing FAT32 volume read
/// through `read` and written through `write`, offsets in bytes from the volume's start: the
/// folders are made when missing, a file already there is replaced, and everything else on the
/// volume is left as it was (D645). FSInfo's free count and next free cluster are marked
/// unknown, as the FAT specification allows, so the next system to mount the volume counts
/// them again.
///
/// # Errors
///
/// A volume that is not FAT32 with 512-byte sectors or whose chains are damaged
/// (`NotFat32`), a name that is not 8.3 or that names a file where a folder is needed
/// (`BadName`), no free cluster (`Full`), or a failed read or write.
pub fn add_files<'a, E>(files: &[File<'_>], read: &mut Reader<'a, E>, write: &mut Writer<'a, E>) -> Result<(), Error<E>> {
    if files.len() > MAX_FILES {
        return Err(Error::BadName);
    }
    let mut boot = [0u8; SECTOR_BYTES];
    read(0, &mut boot).map_err(Error::Read)?;
    let volume = volume_of(&boot).ok_or(Error::NotFat32)?;
    if !volume.valid(volume.root_cluster) {
        return Err(Error::NotFat32);
    }
    let mut access = Access {
        read,
        write,
        volume,
        next_free_hint: 2,
    };
    for file in files {
        let mut folder = volume.root_cluster;
        let mut parts = file.path.split('/').peekable();
        while let Some(part) = parts.next() {
            let name = short_name(part).ok_or(Error::BadName)?;
            if parts.peek().is_some() {
                folder = access.folder(folder, &name)?;
            } else {
                access.file(folder, &name, file.bytes)?;
            }
        }
    }
    if volume.information_sector != 0 && volume.information_sector != 0xffff {
        let mut information = [0u8; SECTOR_BYTES];
        let offset = u64::from(volume.information_sector) * SECTOR_BYTES as u64;
        access.read_sector(offset, &mut information)?;
        if u32_at(&information, 0) == 0x4161_5252 && u32_at(&information, 484) == 0x6141_7272 {
            information[488..496].fill(0xff);
            access.write_bytes(offset, &information)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Geometry, write_volume};

    fn volume_with(files: &[File<'_>]) -> Vec<u8> {
        let mut image = vec![0u8; 524_288 * SECTOR_BYTES];
        write_volume::<()>(524_288, 2048, 7, files, &mut |offset, bytes| {
            let start = offset as usize;
            image[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        image
    }

    fn add(image: &mut Vec<u8>, files: &[File<'_>]) -> Result<(), Error<()>> {
        let shared = core::cell::RefCell::new(image);
        add_files::<()>(
            files,
            &mut |offset, buffer| {
                let start = offset as usize;
                buffer.copy_from_slice(&shared.borrow()[start..start + buffer.len()]);
                Ok(())
            },
            &mut |offset, bytes| {
                let start = offset as usize;
                shared.borrow_mut()[start..start + bytes.len()].copy_from_slice(bytes);
                Ok(())
            },
        )
    }

    fn lookup(image: &[u8], path: &str) -> Option<(u32, u32)> {
        let geometry = Geometry::new(524_288).unwrap();
        let fat = 32 * SECTOR_BYTES;
        let mut folder = 2;
        let mut found = None;
        for part in path.split('/') {
            let name = short_name(part)?;
            found = None;
            let mut cluster = folder;
            'chain: loop {
                let at = geometry.cluster_offset(cluster) as usize;
                for slot in 0..geometry.cluster_bytes() / DIRECTORY_ENTRY_BYTES {
                    let entry = &image[at + slot * 32..at + slot * 32 + 32];
                    if entry[0] == 0 {
                        break 'chain;
                    }
                    if entry[..11] == name {
                        let first = (u16_at(entry, 20) << 16) | u16_at(entry, 26);
                        found = Some((first, u32_at(entry, 28)));
                        break 'chain;
                    }
                }
                let next = u32_at(image, fat + cluster as usize * 4) & CLUSTER_MASK;
                if next >= FIRST_END_OF_CHAIN {
                    break;
                }
                cluster = next;
            }
            folder = found?.0;
        }
        found
    }

    #[test]
    fn files_are_added_beside_the_ones_there_and_replaced_when_written_again() {
        let mut image = volume_with(&[File {
            path: "EFI/BOOT/BOOTX64.EFI",
            bytes: b"other loader",
        }]);
        let kernel = vec![0x5au8; 3000];
        add(
            &mut image,
            &[
                File { path: "EFI/EGGOS/BOOTX64.EFI", bytes: b"MZ eggos" },
                File { path: "EFI/EGGOS/KERNEL.ELF", bytes: &kernel },
            ],
        )
        .unwrap();
        let geometry = Geometry::new(524_288).unwrap();
        let (other, _) = lookup(&image, "EFI/BOOT/BOOTX64.EFI").unwrap();
        assert_eq!(&image[geometry.cluster_offset(other) as usize..][..12], b"other loader");
        let (first, size) = lookup(&image, "EFI/EGGOS/KERNEL.ELF").unwrap();
        assert_eq!(size, 3000);
        assert_eq!(image[geometry.cluster_offset(first) as usize], 0x5a);
        add(&mut image, &[File { path: "EFI/EGGOS/KERNEL.ELF", bytes: b"short" }]).unwrap();
        let (_, size) = lookup(&image, "EFI/EGGOS/KERNEL.ELF").unwrap();
        assert_eq!(size, 5);
        let information = SECTOR_BYTES;
        assert_eq!(u32_at(&image, information + 488), u32::MAX);
        assert_eq!(add(&mut image, &[File { path: "EFI/EGGOS/KERNEL.ELF/X", bytes: b"" }]), Err(Error::BadName));
        assert_eq!(add(&mut vec![0u8; 4096 * SECTOR_BYTES], &[]), Err(Error::NotFat32));
    }

    #[test]
    fn a_full_folder_grows_by_a_cluster() {
        let mut image = volume_with(&[]);
        let names: Vec<String> = (0..20).map(|number| format!("F{number}.TXT")).collect();
        for chunk in names.chunks(MAX_FILES) {
            let paths: Vec<String> = chunk.iter().map(|name| format!("MANY/{name}")).collect();
            let files: Vec<File<'_>> = paths.iter().map(|path| File { path, bytes: b"x" }).collect();
            add(&mut image, &files).unwrap();
        }
        assert!(lookup(&image, "MANY/F19.TXT").is_some());
    }
}
