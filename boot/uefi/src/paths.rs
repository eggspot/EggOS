//! The paths of the files the loader reads beside itself (D646): the folder it was started
//! from, read from its loaded image's device path, so an EggOS sharing another system's EFI
//! system partition keeps its kernel in `\EFI\EGGOS\`.

pub const PATH_UNITS: usize = 128;
pub type Path = [u16; PATH_UNITS];

const MEDIA: u8 = 4;
const FILE_PATH: u8 = 4;
const END: u8 = 0x7f;
const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;

/// `text` as a NUL-ended UTF-16 path, cut to fit.
pub fn path(text: &str) -> Path {
    let mut out = [0u16; PATH_UNITS];
    for (slot, unit) in out.iter_mut().zip(text.encode_utf16().take(PATH_UNITS - 1)) {
        *slot = unit;
    }
    out
}

fn length_of(path: &Path) -> usize {
    path.iter().position(|unit| *unit == 0).unwrap_or(PATH_UNITS)
}

/// The file path a device path's bytes name: its media file-path nodes joined with `\`,
/// forward slashes turned to backslashes; `None` when it names none or does not fit.
pub fn file_path_of(device_path: &[u8]) -> Option<Path> {
    let mut out = [0u16; PATH_UNITS];
    let mut used = 0;
    let mut at = 0;
    while let Some(header) = device_path.get(at..at + 4) {
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if length < 4 {
            return None;
        }
        let node = device_path.get(at..at + length)?;
        if header[0] == END {
            break;
        }
        if header[0] == MEDIA && header[1] == FILE_PATH {
            let units = node[4..].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
            if used > 0 && out[used - 1] != BACKSLASH {
                *out.get_mut(used)? = BACKSLASH;
                used += 1;
            }
            for unit in units.take_while(|unit| *unit != 0) {
                if used > 0 && out[used - 1] == BACKSLASH && (unit == BACKSLASH || unit == SLASH) {
                    continue;
                }
                *out.get_mut(used)? = if unit == SLASH { BACKSLASH } else { unit };
                used += 1;
            }
        }
        at += length;
    }
    (used > 0 && used < PATH_UNITS).then_some(out)
}

/// The path of `name` in the folder `file` is in.
pub fn beside(file: &Path, name: &str) -> Option<Path> {
    let length = length_of(file);
    let folder_end = file[..length].iter().rposition(|unit| *unit == BACKSLASH)? + 1;
    let mut out = [0u16; PATH_UNITS];
    out[..folder_end].copy_from_slice(&file[..folder_end]);
    let mut used = folder_end;
    for unit in name.encode_utf16() {
        if used >= PATH_UNITS - 1 {
            return None;
        }
        out[used] = unit;
        used += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: u8, subtype: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![kind, subtype];
        bytes.extend_from_slice(&((payload.len() + 4) as u16).to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn utf16_bytes(text: &str) -> Vec<u8> {
        text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn the_loader_finds_its_folder_and_the_files_beside_it() {
        let mut device_path = node(MEDIA, FILE_PATH, &utf16_bytes("\\EFI\\EGGOS\\BOOTX64.EFI"));
        device_path.extend(node(END, 0xff, &[]));
        let own = file_path_of(&device_path).unwrap();
        assert_eq!(own, path("\\EFI\\EGGOS\\BOOTX64.EFI"));
        assert_eq!(beside(&own, "KERNEL.ELF").unwrap(), path("\\EFI\\EGGOS\\KERNEL.ELF"));
        let mut split = node(MEDIA, FILE_PATH, &utf16_bytes("\\EFI\\EggOS"));
        split.extend(node(MEDIA, FILE_PATH, &utf16_bytes("/BOOTAA64.EFI")));
        split.extend(node(END, 0xff, &[]));
        assert_eq!(file_path_of(&split).unwrap(), path("\\EFI\\EggOS\\BOOTAA64.EFI"));
        let mut hard_drive = node(4, 1, &[0; 38]);
        hard_drive.extend(node(END, 0xff, &[]));
        assert_eq!(file_path_of(&hard_drive), None);
        assert_eq!(beside(&path("BOOTX64.EFI"), "KERNEL.ELF"), None);
        assert_eq!(file_path_of(&[4, 4, 2, 0]), None);
    }
}
