//! Writing `.zip` archives of test data (D148): every entry stored, from PKWARE's public
//! APPNOTE, with Unix modes, so the installer's reader is tested on an archive made by the host.

use std::path::Path;

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;
const VERSION: u16 = 20;
const MADE_BY_UNIX: u16 = 3 << 8 | VERSION;
const DIRECTORY_MODE: u32 = 0o040_755;
const FILE_MODE: u32 = 0o100_644;
const PROGRAM_MODE: u32 = 0o100_755;
const PROGRAMS_FOLDER: &str = "/Contents/MacOS/";

/// One entry: its path in the archive (a folder's ends with `/`), its data and its Unix mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub data: Vec<u8>,
    pub mode: u32,
}

/// The archive holding `members`, in order, each stored.
pub fn stored(members: &[Member]) -> Result<Vec<u8>, String> {
    let mut file = Vec::new();
    let mut central = Vec::new();
    for member in members {
        let local = u32::try_from(file.len()).map_err(|_| "the archive passed 4 GiB")?;
        let size = u32::try_from(member.data.len())
            .map_err(|_| format!("{} passed 4 GiB", member.name))?;
        let name_length = u16::try_from(member.name.len())
            .map_err(|_| format!("the name {} is too long", member.name))?;
        let mut fields = Vec::new();
        fields.extend_from_slice(&VERSION.to_le_bytes());
        fields.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        fields.extend_from_slice(&eggos_png::crc32(&member.data).to_le_bytes());
        fields.extend_from_slice(&size.to_le_bytes());
        fields.extend_from_slice(&size.to_le_bytes());
        fields.extend_from_slice(&name_length.to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());
        file.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
        file.extend_from_slice(&fields);
        file.extend_from_slice(member.name.as_bytes());
        file.extend_from_slice(&member.data);
        central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
        central.extend_from_slice(&MADE_BY_UNIX.to_le_bytes());
        central.extend_from_slice(&fields);
        central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&(member.mode << 16).to_le_bytes());
        central.extend_from_slice(&local.to_le_bytes());
        central.extend_from_slice(member.name.as_bytes());
    }
    let count = u16::try_from(members.len()).map_err(|_| "too many entries for a zip")?;
    let directory = u32::try_from(file.len()).map_err(|_| "the archive passed 4 GiB")?;
    let directory_size = u32::try_from(central.len()).map_err(|_| "the archive passed 4 GiB")?;
    file.extend_from_slice(&central);
    file.extend_from_slice(&END_SIGNATURE.to_le_bytes());
    file.extend_from_slice(&[0, 0, 0, 0]);
    file.extend_from_slice(&count.to_le_bytes());
    file.extend_from_slice(&count.to_le_bytes());
    file.extend_from_slice(&directory_size.to_le_bytes());
    file.extend_from_slice(&directory.to_le_bytes());
    file.extend_from_slice(&0u16.to_le_bytes());
    Ok(file)
}

/// `root` and everything in it, folders first and names in order, named from `prefix`: a
/// folder `prefix/`, a file `prefix/inner/name`; a file in a bundle's `Contents/MacOS` is a
/// program.
pub fn folder_members(root: &Path, prefix: &str) -> Result<Vec<Member>, String> {
    let mut members = vec![Member {
        name: format!("{prefix}/"),
        data: Vec::new(),
        mode: DIRECTORY_MODE,
    }];
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if path.is_dir() {
            members.extend(folder_members(&path, &name)?);
        } else {
            let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mode = match name.contains(PROGRAMS_FOLDER) {
                true => PROGRAM_MODE,
                false => FILE_MODE,
            };
            members.push(Member { name, data, mode });
        }
    }
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_app::archive::{self, Archive};
    use eggos_app::reader::Bytes;
    use eggos_png::inflate::{Sink, Window};

    struct Collect(Vec<u8>);

    impl Sink for Collect {
        fn write(&mut self, bytes: &[u8]) -> bool {
            self.0.extend_from_slice(bytes);
            true
        }
    }

    #[test]
    fn the_installer_reads_back_what_is_written() {
        let members = [
            Member {
                name: "Tool.app/".to_owned(),
                data: Vec::new(),
                mode: DIRECTORY_MODE,
            },
            Member {
                name: "Tool.app/Contents/MacOS/Tool".to_owned(),
                data: b"program bytes".to_vec(),
                mode: PROGRAM_MODE,
            },
        ];
        let zip = stored(&members).unwrap();
        let mut reader = Bytes(&zip);
        let opened = Archive::open(&mut reader).unwrap();
        assert_eq!(opened.entries(), 2);
        let mut cursor = opened.first();
        let folder = opened.next(&mut reader, &mut cursor).unwrap().unwrap();
        assert!(folder.is_directory());
        assert_eq!(folder.name(), b"Tool.app");
        let program = opened.next(&mut reader, &mut cursor).unwrap().unwrap();
        assert_eq!(program.permissions(), Some(0o755));
        let mut window = Box::new(Window::new());
        let mut out = Collect(Vec::new());
        archive::extract(&mut reader, &program, &mut window, &mut out).unwrap();
        assert_eq!(out.0, b"program bytes");
        assert!(opened.next(&mut reader, &mut cursor).unwrap().is_none());
    }

    #[test]
    fn a_folder_becomes_its_members_in_order() {
        let root = std::env::temp_dir().join(format!("eggos-zip-test-{}", std::process::id()));
        let programs = root.join("Contents").join("MacOS");
        std::fs::create_dir_all(&programs).unwrap();
        std::fs::write(root.join("Contents").join("Info.plist"), b"<dict/>").unwrap();
        std::fs::write(programs.join("Tool"), b"x").unwrap();
        let members = folder_members(&root, "Tool.app").unwrap();
        let names: Vec<(&str, u32)> = members
            .iter()
            .map(|member| (member.name.as_str(), member.mode))
            .collect();
        assert_eq!(
            names,
            [
                ("Tool.app/", DIRECTORY_MODE),
                ("Tool.app/Contents/", DIRECTORY_MODE),
                ("Tool.app/Contents/Info.plist", FILE_MODE),
                ("Tool.app/Contents/MacOS/", DIRECTORY_MODE),
                ("Tool.app/Contents/MacOS/Tool", PROGRAM_MODE),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
