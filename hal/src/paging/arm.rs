//! 32-bit Arm (Armv7-A and Armv8-A's AArch32) with the Large Physical Address Extension's
//! long descriptors (Arm ARM B3.6, D474): three levels translating 32-bit addresses into
//! 40-bit physical ones. The descriptors are AArch64's ([`super::aarch64`]: the same valid,
//! table, attribute index, access permission, shareability, access flag, not-global and
//! execute-never bits), so their encodings are shared; the root level holds four entries of
//! 1 GiB each, and `TTBR0` alone translates the whole address space, the kernel in its top
//! gigabyte. Memory types are indexes into `MAIR0`, loaded with [`super::aarch64::MAIR`]'s low
//! word.

use super::aarch64::Aarch64;
use super::{Attributes, Entry, Format};

/// The LPAE format. See the module documentation.
#[derive(Debug)]
pub struct ArmLpae;

const ROOT_LEVEL: usize = 2;
const ROOT_ENTRIES: usize = 4;
const ADDRESS_SPACE: u64 = 1 << 32;

impl Format for ArmLpae {
    const LEVELS: usize = 3;
    const ROOTS: usize = 1;
    const MAX_LEAF_LEVEL: usize = 2;
    const ENCODES_MEMORY_TYPE: bool = true;
    const USER_END: u64 = 0xc000_0000;

    fn root(virt: u64) -> Option<usize> {
        (virt < ADDRESS_SPACE).then_some(0)
    }

    fn table(table: u64, level: usize, user: bool) -> u64 {
        Aarch64::table(table, level, user)
    }

    fn leaf(phys: u64, level: usize, attributes: Attributes) -> u64 {
        Aarch64::leaf(phys, level, attributes)
    }

    fn kept(phys: u64) -> u64 {
        Aarch64::kept(phys)
    }

    fn entries(level: usize) -> usize {
        match level {
            ROOT_LEVEL => ROOT_ENTRIES,
            _ => super::ENTRIES,
        }
    }

    fn decode(entry: u64, level: usize) -> Entry {
        Aarch64::decode(entry, level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paging::tests::round_trips;

    #[test]
    fn the_root_holds_four_gigabytes_and_lower_levels_512_entries() {
        assert_eq!(ArmLpae::entries(2), 4);
        assert_eq!(ArmLpae::entries(1), 512);
        assert_eq!(ArmLpae::index(0xc010_0000, 2), 3);
        assert_eq!(ArmLpae::index(0xc010_0000, 1), 0);
        assert_eq!(ArmLpae::root(0xffff_f000), Some(0));
        assert_eq!(ArmLpae::root(1 << 32), None);
    }

    #[test]
    fn descriptors_are_aarch64_s_and_round_trip() {
        let page = ArmLpae::leaf(0x8020_0000, 0, Attributes::READ_WRITE);
        assert_eq!(page, Aarch64::leaf(0x8020_0000, 0, Attributes::READ_WRITE));
        round_trips::<ArmLpae>();
    }
}
