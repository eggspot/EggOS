//! EggOS's UEFI boot entry as bytes (D652, D781): the `Boot####` load option named `EggOS`, the
//! device-path nodes it starts (a GPT partition, a file on it, the end), and `BootOrder` with a
//! number put first. EggOS's loader writes one for itself through boot services; `disk install`
//! writes one through `/sys/firmware/efi/efivars` after a whole-disk install. Nothing here
//! allocates or touches the firmware, so the host tests every rule.

#![cfg_attr(not(test), no_std)]

/// A load option's attribute: the firmware may start it.
pub const LOAD_OPTION_ACTIVE: u32 = 1;
/// EggOS's entries' description.
pub const OUR_DESCRIPTION: &str = "EggOS";
/// The end-of-device-path node.
pub const END_NODE: [u8; 4] = [0x7f, 0xff, 4, 0];
/// The bytes of a hard-drive media node.
pub const HARD_DRIVE_NODE_BYTES: usize = 42;
/// UEFI's global variable vendor, whose `Boot####` and `BootOrder` hold the boot entries, in the
/// byte order UEFI stores it.
pub const GLOBAL_VARIABLE: [u8; 16] = [
    0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c,
];
/// The attributes boot entries are stored with: kept across restarts, seen by boot and runtime
/// services.
pub const BOOT_VARIABLE_ATTRIBUTES: u32 = 1 | 2 | 4;

const MEDIA: u8 = 4;
const HARD_DRIVE: u8 = 1;
const FILE_PATH: u8 = 4;
const GPT_FORMAT: u8 = 2;
const GUID_SIGNATURE: u8 = 2;

/// The node naming GPT partition `number` (from 1) of `sectors` 512-byte sectors from
/// `first_sector`, with unique GUID `unique` as the partition table stores it.
pub fn hard_drive_node(
    number: u32,
    first_sector: u64,
    sectors: u64,
    unique: &[u8; 16],
) -> [u8; HARD_DRIVE_NODE_BYTES] {
    let mut node = [0u8; HARD_DRIVE_NODE_BYTES];
    node[0] = MEDIA;
    node[1] = HARD_DRIVE;
    node[2..4].copy_from_slice(&(HARD_DRIVE_NODE_BYTES as u16).to_le_bytes());
    node[4..8].copy_from_slice(&number.to_le_bytes());
    node[8..16].copy_from_slice(&first_sector.to_le_bytes());
    node[16..24].copy_from_slice(&sectors.to_le_bytes());
    node[24..40].copy_from_slice(unique);
    node[40] = GPT_FORMAT;
    node[41] = GUID_SIGNATURE;
    node
}

/// The node naming file `path` (`\EFI\BOOT\BOOTX64.EFI`), in `out`; its length, or `None`
/// when it does not fit.
pub fn file_path_node(path: &str, out: &mut [u8]) -> Option<usize> {
    let length = 4 + 2 * (path.encode_utf16().count() + 1);
    let out = out.get_mut(..length)?;
    out[0] = MEDIA;
    out[1] = FILE_PATH;
    out[2..4].copy_from_slice(&u16::try_from(length).ok()?.to_le_bytes());
    for (at, unit) in path.encode_utf16().chain([0]).enumerate() {
        out[4 + 2 * at..6 + 2 * at].copy_from_slice(&unit.to_le_bytes());
    }
    Some(length)
}

/// An active load option named [`OUR_DESCRIPTION`] that starts `device_path`, in `out`; its
/// length, or `None` when it does not fit.
pub fn load_option(device_path: &[u8], out: &mut [u8]) -> Option<usize> {
    let path_length = u16::try_from(device_path.len()).ok()?;
    let description = OUR_DESCRIPTION.encode_utf16().chain([0]);
    let description_bytes = 2 * (OUR_DESCRIPTION.len() + 1);
    let length = 6 + description_bytes + device_path.len();
    let out = out.get_mut(..length)?;
    out[..4].copy_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
    out[4..6].copy_from_slice(&path_length.to_le_bytes());
    for (at, unit) in description.enumerate() {
        out[6 + 2 * at..8 + 2 * at].copy_from_slice(&unit.to_le_bytes());
    }
    out[6 + description_bytes..].copy_from_slice(device_path);
    Some(length)
}

/// The first `Boot####` number `taken` says is free.
pub fn first_free(mut taken: impl FnMut(u16) -> bool) -> Option<u16> {
    (0..=u16::MAX).find(|number| !taken(*number))
}

/// `BootOrder` with `number` first and the rest after it in their order, in `out`; its length
/// in bytes, or `None` when it does not fit.
pub fn order_with_first(order: &[u8], number: u16, out: &mut [u8]) -> Option<usize> {
    let rest = order
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .filter(|other| *other != number);
    let mut length = 0;
    for entry in core::iter::once(number).chain(rest) {
        out.get_mut(length..length + 2)?.copy_from_slice(&entry.to_le_bytes());
        length += 2;
    }
    Some(length)
}

/// `Boot####`'s name for `number`, four upper-case hexadecimal digits.
pub fn boot_variable_name(number: u16) -> [u8; 8] {
    let mut name = *b"Boot0000";
    for (at, shift) in [12u16, 8, 4, 0].into_iter().enumerate() {
        let digit = ((number >> shift) & 0xf) as u8;
        name[4 + at] = match digit {
            0..=9 => b'0' + digit,
            _ => b'A' + digit - 10,
        };
    }
    name
}

/// Whether load option `option` is one of EggOS's (named [`OUR_DESCRIPTION`]) starting a file
/// on the partition with unique GUID `unique`.
pub fn is_ours_on(option: &[u8], unique: &[u8; 16]) -> bool {
    let Some(&[low, high]) = option.get(4..6) else {
        return false;
    };
    let path_length = u16::from_le_bytes([low, high]);
    let description: [u16; 6] = [0x45, 0x67, 0x67, 0x4f, 0x53, 0];
    let mut at = 6;
    for unit in description {
        if option.get(at..at + 2) != Some(&unit.to_le_bytes()[..]) {
            return false;
        }
        at += 2;
    }
    let Some(path) = option.get(at..at + usize::from(path_length)) else {
        return false;
    };
    path.windows(16).any(|window| window == unique)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_entry_goes_first_and_takes_a_free_number() {
        let mut out = [0u8; 8];
        let order = [1u8, 0, 3, 0, 2, 0];
        assert_eq!(order_with_first(&order, 3, &mut out), Some(6));
        assert_eq!(out[..6], [3, 0, 1, 0, 2, 0]);
        assert_eq!(order_with_first(&order, 4, &mut out[..6]), None);
        assert_eq!(first_free(|number| number < 3), Some(3));
        assert_eq!(&boot_variable_name(0x00ab), b"Boot00AB");
    }

    #[test]
    fn a_whole_disk_entry_names_the_partition_and_the_loader() {
        let unique = [7u8; 16];
        let node = hard_drive_node(1, 2048, 409_600, &unique);
        assert_eq!(&node[..4], &[4, 1, 42, 0]);
        assert_eq!(u64::from_le_bytes(node[8..16].try_into().unwrap()), 2048);
        let mut file = [0u8; 128];
        let file_length = file_path_node("\\EFI\\BOOT\\BOOTX64.EFI", &mut file).unwrap();
        assert_eq!(file_length, 4 + 2 * 22);
        let mut path = node.to_vec();
        path.extend_from_slice(&file[..file_length]);
        path.extend_from_slice(&END_NODE);
        let mut option = [0u8; 256];
        let length = load_option(&path, &mut option).unwrap();
        assert!(is_ours_on(&option[..length], &unique));
        assert!(!is_ours_on(&option[..length], &[8u8; 16]));
        assert_eq!(&option[..4], &LOAD_OPTION_ACTIVE.to_le_bytes());
    }
}
