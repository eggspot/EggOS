//! Writing a FAT32 volume (Microsoft's FAT specification, 1.03) with a few files in a few
//! folders, for EggOS's EFI system partition: `cargo xtask usb-image` and the installer
//! (D241). Short 8.3 names only, which UEFI's paths for EggOS need.

#![cfg_attr(not(test), no_std)]

pub const SECTOR_BYTES: usize = 512;
pub const MAX_CLUSTER_BYTES: usize = SECTOR_BYTES * 8;
const SMALL_VOLUME_SECTORS: u32 = 532_480;
const RESERVED_SECTORS: u32 = 32;
const FATS: u32 = 2;
const ROOT_CLUSTER: u32 = 2;
const END_OF_CHAIN: u32 = 0x0fff_ffff;
const DIRECTORY_ENTRY_BYTES: usize = 32;
const ATTRIBUTE_DIRECTORY: u8 = 0x10;
const ATTRIBUTE_ARCHIVE: u8 = 0x20;
const ATTRIBUTE_VOLUME_LABEL: u8 = 0x08;
const MIN_CLUSTERS: u32 = 65_525;
pub const MAX_FOLDERS: usize = 8;
pub const MAX_FILES: usize = 8;

/// A file to put on the volume: its path with `/` between folders, in 8.3 names.
#[derive(Debug, Clone, Copy)]
pub struct File<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

/// Why a volume could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// Too few sectors for FAT32's smallest volume, or too many files for them.
    TooSmall,
    /// A name that is not a valid 8.3 name, or too many folders or files.
    BadName,
    /// The writer failed.
    Write(E),
}

/// The 11 bytes of an 8.3 name, upper case.
pub fn short_name(name: &str) -> Option<[u8; 11]> {
    let (base, extension) = match name.rsplit_once('.') {
        Some((base, extension)) => (base, extension),
        None => (name, ""),
    };
    let allowed = |byte: &u8| byte.is_ascii_alphanumeric() || b"_-~!#$%&'()@^`{}".contains(byte);
    if base.is_empty()
        || base.len() > 8
        || extension.len() > 3
        || !base.bytes().all(|byte| allowed(&byte))
        || !extension.bytes().all(|byte| allowed(&byte))
    {
        return None;
    }
    let mut out = [b' '; 11];
    for (slot, byte) in out.iter_mut().zip(base.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    for (slot, byte) in out[8..].iter_mut().zip(extension.bytes()) {
        *slot = byte.to_ascii_uppercase();
    }
    Some(out)
}

/// Where the volume's parts are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub total_sectors: u32,
    pub sectors_per_cluster: u32,
    pub fat_sectors: u32,
    pub clusters: u32,
}

impl Geometry {
    pub fn new(total_sectors: u32) -> Option<Self> {
        let sectors_per_cluster = match total_sectors <= SMALL_VOLUME_SECTORS {
            true => 1,
            false => (MAX_CLUSTER_BYTES / SECTOR_BYTES) as u32,
        };
        let mut fat_sectors = 1;
        for _ in 0..8 {
            let data = total_sectors.checked_sub(RESERVED_SECTORS + FATS * fat_sectors)?;
            let clusters = data / sectors_per_cluster;
            fat_sectors = ((clusters + 2) * 4).div_ceil(SECTOR_BYTES as u32);
        }
        let data = total_sectors.checked_sub(RESERVED_SECTORS + FATS * fat_sectors)?;
        let clusters = data / sectors_per_cluster;
        (clusters >= MIN_CLUSTERS).then_some(Self {
            total_sectors,
            sectors_per_cluster,
            fat_sectors,
            clusters,
        })
    }

    /// The bytes of one cluster.
    pub fn cluster_bytes(&self) -> usize {
        SECTOR_BYTES * self.sectors_per_cluster as usize
    }

    fn data_start(&self) -> u32 {
        RESERVED_SECTORS + FATS * self.fat_sectors
    }

    /// The byte offset of cluster `cluster` in the volume.
    pub fn cluster_offset(&self, cluster: u32) -> u64 {
        (u64::from(self.data_start())
            + u64::from(cluster - 2) * u64::from(self.sectors_per_cluster))
            * SECTOR_BYTES as u64
    }
}

#[derive(Clone, Copy)]
struct Folder<'a> {
    path: &'a str,
    cluster: u32,
    parent: u32,
}

/// Writes a FAT32 volume of `total_sectors` holding `files` through `write(offset, bytes)`,
/// offsets in bytes from the volume's start, `hidden_sectors` before it on its disk.
pub fn write_volume<E>(
    total_sectors: u32,
    hidden_sectors: u32,
    volume_id: u32,
    files: &[File<'_>],
    write: &mut dyn FnMut(u64, &[u8]) -> Result<(), E>,
) -> Result<(), Error<E>> {
    let geometry = Geometry::new(total_sectors).ok_or(Error::TooSmall)?;
    if files.len() > MAX_FILES {
        return Err(Error::BadName);
    }
    let mut folders = [Folder {
        path: "",
        cluster: ROOT_CLUSTER,
        parent: 0,
    }; MAX_FOLDERS];
    let mut folder_count = 1;
    let mut next_cluster = ROOT_CLUSTER + 1;
    for file in files {
        let mut at = 0;
        while let Some(slash) = file.path[at..].find('/') {
            let path = &file.path[..at + slash];
            let name = &file.path[at..at + slash];
            short_name(name).ok_or(Error::BadName)?;
            if !folders[..folder_count]
                .iter()
                .any(|folder| folder.path == path)
            {
                if folder_count == MAX_FOLDERS {
                    return Err(Error::BadName);
                }
                let parent_path = &file.path[..at.saturating_sub(1)];
                let parent = folders[..folder_count]
                    .iter()
                    .find(|folder| folder.path == parent_path)
                    .map_or(ROOT_CLUSTER, |folder| folder.cluster);
                folders[folder_count] = Folder {
                    path,
                    cluster: next_cluster,
                    parent,
                };
                folder_count += 1;
                next_cluster += 1;
            }
            at += slash + 1;
        }
        short_name(&file.path[at..]).ok_or(Error::BadName)?;
    }
    let mut first_clusters = [0u32; MAX_FILES];
    for (index, file) in files.iter().enumerate() {
        let clusters = (file.bytes.len().div_ceil(geometry.cluster_bytes())) as u32;
        first_clusters[index] = if clusters == 0 { 0 } else { next_cluster };
        next_cluster += clusters;
    }
    if next_cluster - 2 > geometry.clusters {
        return Err(Error::TooSmall);
    }

    let mut boot = [0u8; SECTOR_BYTES];
    boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"EGGOS   ");
    boot[11..13].copy_from_slice(&(SECTOR_BYTES as u16).to_le_bytes());
    boot[13] = geometry.sectors_per_cluster as u8;
    boot[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    boot[16] = FATS as u8;
    boot[21] = 0xf8;
    boot[24..26].copy_from_slice(&63u16.to_le_bytes());
    boot[26..28].copy_from_slice(&255u16.to_le_bytes());
    boot[28..32].copy_from_slice(&hidden_sectors.to_le_bytes());
    boot[32..36].copy_from_slice(&total_sectors.to_le_bytes());
    boot[36..40].copy_from_slice(&geometry.fat_sectors.to_le_bytes());
    boot[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    boot[48..50].copy_from_slice(&1u16.to_le_bytes());
    boot[50..52].copy_from_slice(&6u16.to_le_bytes());
    boot[64] = 0x80;
    boot[66] = 0x29;
    boot[67..71].copy_from_slice(&volume_id.to_le_bytes());
    boot[71..82].copy_from_slice(b"EGGOS      ");
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510] = 0x55;
    boot[511] = 0xaa;
    let mut information = [0u8; SECTOR_BYTES];
    information[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    information[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    let free = geometry.clusters - (next_cluster - 2);
    information[488..492].copy_from_slice(&free.to_le_bytes());
    information[492..496].copy_from_slice(&next_cluster.to_le_bytes());
    information[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
    let zero = [0u8; SECTOR_BYTES];
    for sector in 0..RESERVED_SECTORS {
        let bytes = match sector {
            0 | 6 => &boot,
            1 | 7 => &information,
            _ => &zero,
        };
        write(u64::from(sector) * SECTOR_BYTES as u64, bytes).map_err(Error::Write)?;
    }

    let mut sector = [0u8; SECTOR_BYTES];
    let entries_per_sector = (SECTOR_BYTES / 4) as u32;
    let entry_of = |cluster: u32| -> u32 {
        match cluster {
            0 => 0x0fff_fff8,
            1 => END_OF_CHAIN,
            _ if cluster < ROOT_CLUSTER + folder_count as u32 => END_OF_CHAIN,
            _ => {
                for (index, file) in files.iter().enumerate() {
                    let first = first_clusters[index];
                    let count = file.bytes.len().div_ceil(geometry.cluster_bytes()) as u32;
                    if first != 0 && (first..first + count).contains(&cluster) {
                        return if cluster + 1 == first + count {
                            END_OF_CHAIN
                        } else {
                            cluster + 1
                        };
                    }
                }
                0
            }
        }
    };
    for fat in 0..FATS {
        let fat_start =
            u64::from(RESERVED_SECTORS + fat * geometry.fat_sectors) * SECTOR_BYTES as u64;
        for index in 0..geometry.fat_sectors {
            let first = index * entries_per_sector;
            for (slot, entry) in sector.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let cluster = first + slot as u32;
                let value = if cluster < next_cluster {
                    entry_of(cluster)
                } else {
                    0
                };
                entry.copy_from_slice(&value.to_le_bytes());
            }
            write(fat_start + u64::from(index) * SECTOR_BYTES as u64, &sector)
                .map_err(Error::Write)?;
        }
    }

    for folder in &folders[..folder_count] {
        let mut cluster = [0u8; MAX_CLUSTER_BYTES];
        let cluster_bytes = geometry.cluster_bytes();
        let mut slot = 0;
        let mut overflowed = false;
        let mut entry = |name: [u8; 11], attribute: u8, first: u32, size: u32| {
            let at = slot * DIRECTORY_ENTRY_BYTES;
            if at + DIRECTORY_ENTRY_BYTES > cluster_bytes {
                overflowed = true;
                return;
            }
            cluster[at..at + 11].copy_from_slice(&name);
            cluster[at + 11] = attribute;
            cluster[at + 20..at + 22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            cluster[at + 24..at + 26].copy_from_slice(&0x5b21u16.to_le_bytes());
            cluster[at + 26..at + 28].copy_from_slice(&(first as u16).to_le_bytes());
            cluster[at + 28..at + 32].copy_from_slice(&size.to_le_bytes());
            slot += 1;
        };
        if folder.cluster == ROOT_CLUSTER {
            entry(*b"EGGOS      ", ATTRIBUTE_VOLUME_LABEL, 0, 0);
        } else {
            entry(*b".          ", ATTRIBUTE_DIRECTORY, folder.cluster, 0);
            let parent = if folder.parent == ROOT_CLUSTER {
                0
            } else {
                folder.parent
            };
            entry(*b"..         ", ATTRIBUTE_DIRECTORY, parent, 0);
        }
        for child in &folders[1..folder_count] {
            if child.parent == folder.cluster {
                let name = child.path.rsplit('/').next().unwrap_or(child.path);
                if let Some(short) = short_name(name) {
                    entry(short, ATTRIBUTE_DIRECTORY, child.cluster, 0);
                }
            }
        }
        for (index, file) in files.iter().enumerate() {
            let (folder_path, name) = file.path.rsplit_once('/').unwrap_or(("", file.path));
            if folder_path == folder.path
                && let Some(short) = short_name(name)
            {
                entry(
                    short,
                    ATTRIBUTE_ARCHIVE,
                    first_clusters[index],
                    file.bytes.len() as u32,
                );
            }
        }
        if overflowed {
            return Err(Error::BadName);
        }
        write(
            geometry.cluster_offset(folder.cluster),
            &cluster[..cluster_bytes],
        )
        .map_err(Error::Write)?;
    }

    for (index, file) in files.iter().enumerate() {
        let first = first_clusters[index];
        if first == 0 {
            continue;
        }
        let mut offset = 0;
        let mut cluster = first;
        while offset < file.bytes.len() {
            let end = (offset + geometry.cluster_bytes()).min(file.bytes.len());
            let mut block = [0u8; MAX_CLUSTER_BYTES];
            block[..end - offset].copy_from_slice(&file.bytes[offset..end]);
            write(
                geometry.cluster_offset(cluster),
                &block[..geometry.cluster_bytes()],
            )
            .map_err(Error::Write)?;
            offset = end;
            cluster += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_are_upper_case_and_padded() {
        assert_eq!(short_name("bootx64.efi"), Some(*b"BOOTX64 EFI"));
        assert_eq!(short_name("EGGOS"), Some(*b"EGGOS      "));
        assert_eq!(short_name("toolongname.txt"), None);
        assert_eq!(short_name("a b"), None);
    }

    #[test]
    fn a_256_mib_volume_has_fat32_geometry() {
        let geometry = Geometry::new(524_288).unwrap();
        assert!(geometry.clusters >= MIN_CLUSTERS);
        assert_eq!(geometry.cluster_bytes(), SECTOR_BYTES);
        assert_eq!(
            Geometry::new(4 << 21).unwrap().cluster_bytes(),
            MAX_CLUSTER_BYTES
        );
        assert!(Geometry::new(65_536).is_none());
    }

    #[test]
    fn a_written_volume_holds_its_files_where_its_directory_says() {
        let mut image = vec![0u8; 524_288 * SECTOR_BYTES];
        let kernel = vec![0x7fu8; 10_000];
        let files = [
            File {
                path: "EFI/BOOT/BOOTX64.EFI",
                bytes: b"MZ loader",
            },
            File {
                path: "EGGOS/KERNEL.ELF",
                bytes: &kernel,
            },
        ];
        write_volume::<()>(524_288, 2048, 0x1234_5678, &files, &mut |offset, bytes| {
            let start = offset as usize;
            image[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(&image[82..90], b"FAT32   ");
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        let geometry = Geometry::new(524_288).unwrap();
        let root = geometry.cluster_offset(ROOT_CLUSTER) as usize;
        let names: Vec<&[u8]> = (0..4)
            .map(|slot| &image[root + slot * 32..root + slot * 32 + 11])
            .collect();
        assert!(names.contains(&&b"EFI        "[..]));
        assert!(names.contains(&&b"EGGOS      "[..]));
        let eggos_entry = (0..4)
            .find(|&slot| {
                &image[root + slot * 32..root + slot * 32 + 11] == b"EGGOS      "
                    && image[root + slot * 32 + 11] == ATTRIBUTE_DIRECTORY
            })
            .unwrap();
        let at = root + eggos_entry * 32;
        let cluster = u32::from(u16::from_le_bytes([image[at + 26], image[at + 27]]));
        let folder = geometry.cluster_offset(cluster) as usize;
        let kernel_at = folder + 2 * 32;
        assert_eq!(&image[kernel_at..kernel_at + 11], b"KERNEL  ELF");
        let size = u32::from_le_bytes(image[kernel_at + 28..kernel_at + 32].try_into().unwrap());
        assert_eq!(size, 10_000);
        let first = u32::from(u16::from_le_bytes([
            image[kernel_at + 26],
            image[kernel_at + 27],
        ]));
        let data = geometry.cluster_offset(first) as usize;
        assert_eq!(image[data], 0x7f);
    }
}
