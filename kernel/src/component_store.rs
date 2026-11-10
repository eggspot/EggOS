//! The component store on an attached disk (D238): recognised by its first bytes, so the kernel
//! leaves the disk to the component manager and names it on the console instead of calling it
//! a disk that holds neither EggFS nor FAT.

use eggos_eggfs::Disk;

/// The first bytes of a component store, as `eggos_components::index::MAGIC` writes them.
pub const MAGIC: &[u8; 16] = b"EGGOS-COMPONENTS";

/// The console line for an attached component store.
pub const ATTACHED: &str = "vfs: the component store is attached";

/// Whether `disk`'s first block starts with [`MAGIC`]; a disk that cannot be read is not one.
pub fn holds_store<D: Disk + ?Sized>(disk: &mut D) -> bool {
    if disk.blocks() == 0 {
        return false;
    }
    let mut block = alloc::vec![0u8; eggos_eggfs::BLOCK_SIZE];
    disk.read(0, &mut block).is_ok() && block.starts_with(MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_eggfs::memory::MemoryDisk;

    #[test]
    fn a_disk_starting_with_the_store_s_magic_is_the_store_and_nothing_else_is() {
        let mut image = alloc::vec![0u8; eggos_eggfs::BLOCK_SIZE * 2];
        image[..MAGIC.len()].copy_from_slice(MAGIC);
        assert!(holds_store(&mut MemoryDisk::from_image(image.clone())));
        image[0] = b'X';
        assert!(!holds_store(&mut MemoryDisk::from_image(image)));
        assert!(!holds_store(&mut MemoryDisk::new(2)));
        assert!(!holds_store(&mut MemoryDisk::new(0)));
        let mut short = alloc::vec![0u8; eggos_eggfs::BLOCK_SIZE];
        short[..MAGIC.len() - 1].copy_from_slice(&MAGIC[..MAGIC.len() - 1]);
        assert!(!holds_store(&mut MemoryDisk::from_image(short)));
    }

    #[test]
    fn a_store_disk_is_not_taken_for_eggfs_or_a_blank_disk() {
        let mut image = alloc::vec![0u8; eggos_eggfs::BLOCK_SIZE * 64];
        image[..MAGIC.len()].copy_from_slice(MAGIC);
        let mut disk = MemoryDisk::from_image(image);
        assert_eq!(
            eggos_eggfs::probe(&mut disk),
            Ok(eggos_eggfs::Contents::Other)
        );
        assert!(holds_store(&mut disk));
    }
}
