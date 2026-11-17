//! EggOS's firmware boot entry (D652; installer dual boot v0.1 section 6): the `Boot####`
//! load option the loader writes for itself when it starts from `\EFI\EGGOS\` and the firmware
//! has none, put first in `BootOrder`. Only bytes here, so the host tests every rule.

use crate::menu::{LOAD_OPTION_ACTIVE, LoadOption, OUR_DESCRIPTION};

const END: u8 = 0x7f;
const END_ENTIRE: u8 = 0xff;
const END_NODE: [u8; 4] = [END, END_ENTIRE, 4, 0];
const SHARED_FOLDER: &str = "\\EFI\\EGGOS\\";

/// Whether the loader's own path, as UTF-16, is in `\EFI\EGGOS\`, where an entry of its own is
/// wanted; a USB stick's `\EFI\BOOT\` is not.
pub fn wants_entry(own_path: &[u16]) -> bool {
    let folder = SHARED_FOLDER.encode_utf16();
    let mut units = own_path.iter().take_while(|unit| **unit != 0);
    folder.into_iter().all(|wanted| {
        units.next().is_some_and(|unit| {
            let upper = |unit: u16| if (0x61..=0x7a).contains(&unit) { unit - 0x20 } else { unit };
            upper(*unit) == upper(wanted)
        })
    })
}

/// The nodes of a device path before its end node.
pub fn without_end(device_path: &[u8]) -> &[u8] {
    let mut at = 0;
    while let Some(header) = device_path.get(at..at + 4) {
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if (header[0] == END && header[1] == END_ENTIRE) || length < 4 {
            break;
        }
        at += length;
    }
    &device_path[..at.min(device_path.len())]
}

/// The full device path of the loader's file: its partition's nodes, then its file-path nodes,
/// then one end node, in `out`; its length, or `None` when it does not fit.
pub fn file_device_path(partition: &[u8], file: &[u8], out: &mut [u8]) -> Option<usize> {
    let partition = without_end(partition);
    let file = without_end(file);
    let length = partition.len() + file.len() + END_NODE.len();
    let out = out.get_mut(..length)?;
    out[..partition.len()].copy_from_slice(partition);
    out[partition.len()..partition.len() + file.len()].copy_from_slice(file);
    out[length - END_NODE.len()..].copy_from_slice(&END_NODE);
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

/// Whether a firmware load option already starts the loader's file: named EggOS, and its
/// device path ends with the file's path nodes.
pub fn starts_file(option: &LoadOption<'_>, file: &[u8]) -> bool {
    let file = without_end(file);
    let nodes = without_end(option.device_path);
    option.description.as_str() == OUR_DESCRIPTION && !file.is_empty() && nodes.ends_with(file)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::load_option_of;

    const PARTITION: [u8; 46] = {
        let mut nodes = [0u8; 46];
        nodes[0] = 4;
        nodes[1] = 1;
        nodes[2] = 42;
        nodes[42] = END;
        nodes[43] = END_ENTIRE;
        nodes[44] = 4;
        nodes
    };

    fn file_nodes() -> Vec<u8> {
        let path: Vec<u8> = "\\EFI\\EGGOS\\EGGOSX64.EFI"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut nodes = vec![4, 4];
        nodes.extend_from_slice(&((path.len() + 4) as u16).to_le_bytes());
        nodes.extend_from_slice(&path);
        nodes.extend_from_slice(&END_NODE);
        nodes
    }

    #[test]
    fn the_entry_starts_the_loader_s_file_on_its_partition() {
        let file = file_nodes();
        let mut path = [0u8; 256];
        let path_length = file_device_path(&PARTITION, &file, &mut path).unwrap();
        assert_eq!(path_length, 42 + file.len());
        let mut bytes = [0u8; 512];
        let length = load_option(&path[..path_length], &mut bytes).unwrap();
        let option = load_option_of(&bytes[..length]).unwrap();
        assert_eq!(option.description.as_str(), "EggOS");
        assert_eq!(option.attributes, LOAD_OPTION_ACTIVE);
        assert!(starts_file(&option, &file));
        assert!(crate::menu::starts_a_file_on_a_partition(option.device_path));
        assert_eq!(load_option(&path[..path_length], &mut [0u8; 8]), None);
    }

    #[test]
    fn only_the_shared_folder_wants_an_entry() {
        let units = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        assert!(wants_entry(&units("\\EFI\\EggOS\\EGGOSX64.EFI")));
        assert!(!wants_entry(&units("\\EFI\\BOOT\\BOOTX64.EFI")));
        assert!(!wants_entry(&units("\\EFI")));
    }

    #[test]
    fn the_new_entry_goes_first_and_takes_a_free_number() {
        let mut out = [0u8; 8];
        let order = [1u8, 0, 3, 0, 2, 0];
        assert_eq!(order_with_first(&order, 3, &mut out), Some(6));
        assert_eq!(out[..6], [3, 0, 1, 0, 2, 0]);
        assert_eq!(order_with_first(&order, 4, &mut out[..6]), None);
        assert_eq!(first_free(|number| number < 3), Some(3));
    }
}
