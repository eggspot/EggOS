//! Windows' special folders on EggOS (D178), for `SHGetFolderPathW` (by `CSIDL`) and
//! `SHGetKnownFolderPath` (by `KNOWNFOLDERID`): the user's folders are the home folder's (the
//! one account, D132), named as the Files app names them, the application data folders are
//! the freedesktop.org base directories (`.config`, `.local/share`), programs are
//! `/Applications` (D131) and the Windows and system folders are where the layer's own
//! libraries live (`/usr/lib/eggos`). The folders "for all users" are the user's own.

/// A special folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Profile,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Templates,
    RoamingAppData,
    LocalAppData,
    StartMenu,
    Programs,
    Startup,
    ProgramFiles,
    Windows,
    System,
    Fonts,
}

/// Where a folder is: under the home folder, or at an absolute path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Home(&'static [u8]),
    Absolute(&'static [u8]),
}

impl Folder {
    /// Where the folder is.
    pub fn place(self) -> Place {
        match self {
            Folder::Profile => Place::Home(b""),
            Folder::Desktop => Place::Home(b"Desktop"),
            Folder::Documents => Place::Home(b"Documents"),
            Folder::Downloads => Place::Home(b"Downloads"),
            Folder::Music => Place::Home(b"Music"),
            Folder::Pictures => Place::Home(b"Pictures"),
            Folder::Videos => Place::Home(b"Videos"),
            Folder::Templates => Place::Home(b"Templates"),
            Folder::RoamingAppData => Place::Home(b".config"),
            Folder::LocalAppData => Place::Home(b".local/share"),
            Folder::StartMenu | Folder::Programs => Place::Home(b".local/share/applications"),
            Folder::Startup => Place::Home(b".config/autostart"),
            Folder::ProgramFiles => Place::Absolute(b"/Applications"),
            Folder::Windows | Folder::System => Place::Absolute(b"/usr/lib/eggos"),
            Folder::Fonts => Place::Absolute(b"/usr/share/fonts"),
        }
    }
}

/// The folder `CSIDL` value `csidl` names (its flags in the high byte left out).
pub fn by_csidl(csidl: u32) -> Option<Folder> {
    Some(match csidl & 0xff {
        0x00 | 0x10 | 0x19 => Folder::Desktop,
        0x02 | 0x17 => Folder::Programs,
        0x05 | 0x2e => Folder::Documents,
        0x07 | 0x18 => Folder::Startup,
        0x0b | 0x16 => Folder::StartMenu,
        0x0d | 0x35 => Folder::Music,
        0x0e | 0x37 => Folder::Videos,
        0x14 => Folder::Fonts,
        0x15 | 0x2d => Folder::Templates,
        0x1a => Folder::RoamingAppData,
        0x1c => Folder::LocalAppData,
        0x24 => Folder::Windows,
        0x25 | 0x29 => Folder::System,
        0x26 | 0x2a => Folder::ProgramFiles,
        0x27 | 0x36 => Folder::Pictures,
        0x28 => Folder::Profile,
        _ => return None,
    })
}

const fn guid(first: u32, second: u16, third: u16, rest: [u8; 8]) -> [u8; 16] {
    let a = first.to_le_bytes();
    let b = second.to_le_bytes();
    let c = third.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], rest[0], rest[1], rest[2], rest[3],
        rest[4], rest[5], rest[6], rest[7],
    ]
}

const KNOWN_FOLDERS: [([u8; 16], Folder); 22] = [
    (
        guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        ),
        Folder::Profile,
    ),
    (
        guid(
            0xb4bfcc3a,
            0xdb2c,
            0x424c,
            [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41],
        ),
        Folder::Desktop,
    ),
    (
        guid(
            0xc4aa340d,
            0xf20f,
            0x4863,
            [0xaf, 0xef, 0xf8, 0x7e, 0xf2, 0xe6, 0xba, 0x25],
        ),
        Folder::Desktop,
    ),
    (
        guid(
            0xfdd39ad0,
            0x238f,
            0x46af,
            [0xad, 0xb4, 0x6c, 0x85, 0x48, 0x03, 0x69, 0xc7],
        ),
        Folder::Documents,
    ),
    (
        guid(
            0x374de290,
            0x123f,
            0x4565,
            [0x91, 0x64, 0x39, 0xc4, 0x92, 0x5e, 0x46, 0x7b],
        ),
        Folder::Downloads,
    ),
    (
        guid(
            0x4bd8d571,
            0x6d19,
            0x48d3,
            [0xbe, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0e, 0x43],
        ),
        Folder::Music,
    ),
    (
        guid(
            0x33e28130,
            0x4e1e,
            0x4676,
            [0x83, 0x5a, 0x98, 0x39, 0x5c, 0x3b, 0xc3, 0xbb],
        ),
        Folder::Pictures,
    ),
    (
        guid(
            0x18989b1d,
            0x99b5,
            0x455b,
            [0x84, 0x1c, 0xab, 0x7c, 0x74, 0xe4, 0xdd, 0xfc],
        ),
        Folder::Videos,
    ),
    (
        guid(
            0xa63293e8,
            0x664e,
            0x48db,
            [0xa0, 0x79, 0xdf, 0x75, 0x9e, 0x05, 0x09, 0xf7],
        ),
        Folder::Templates,
    ),
    (
        guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        ),
        Folder::RoamingAppData,
    ),
    (
        guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        ),
        Folder::LocalAppData,
    ),
    (
        guid(
            0x625b53c3,
            0xab48,
            0x4ec1,
            [0xba, 0x1f, 0xa1, 0xef, 0x41, 0x46, 0xfc, 0x19],
        ),
        Folder::StartMenu,
    ),
    (
        guid(
            0xa77f5d77,
            0x2e2b,
            0x44c3,
            [0xa6, 0xa2, 0xab, 0xa6, 0x01, 0x05, 0x4a, 0x51],
        ),
        Folder::Programs,
    ),
    (
        guid(
            0xb97d20bb,
            0xf46a,
            0x4c97,
            [0xba, 0x10, 0x5e, 0x36, 0x08, 0x43, 0x08, 0x54],
        ),
        Folder::Startup,
    ),
    (
        guid(
            0x905e63b6,
            0xc1bf,
            0x494e,
            [0xb2, 0x9c, 0x65, 0xb7, 0x32, 0xd3, 0xd2, 0x1a],
        ),
        Folder::ProgramFiles,
    ),
    (
        guid(
            0x6d809377,
            0x6af0,
            0x444b,
            [0x89, 0x57, 0xa3, 0x77, 0x3f, 0x02, 0x20, 0x0e],
        ),
        Folder::ProgramFiles,
    ),
    (
        guid(
            0x7c5a40ef,
            0xa0fb,
            0x4bfc,
            [0x87, 0x4a, 0xc0, 0xf2, 0xe0, 0xb9, 0xfa, 0x8e],
        ),
        Folder::ProgramFiles,
    ),
    (
        guid(
            0xf38bf404,
            0x1d43,
            0x42f2,
            [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23],
        ),
        Folder::Windows,
    ),
    (
        guid(
            0x1ac14e77,
            0x02e7,
            0x4e5d,
            [0xb7, 0x44, 0x2e, 0xb1, 0xae, 0x51, 0x98, 0xb7],
        ),
        Folder::System,
    ),
    (
        guid(
            0xd65231b0,
            0xb2f1,
            0x4857,
            [0xa4, 0xce, 0xa8, 0xe7, 0xc6, 0xea, 0x7d, 0x27],
        ),
        Folder::System,
    ),
    (
        guid(
            0xfd228cb7,
            0xae11,
            0x4ae3,
            [0x86, 0x4c, 0x16, 0xf3, 0x91, 0x0a, 0xb8, 0xfe],
        ),
        Folder::Fonts,
    ),
    (
        guid(
            0x82a5ea35,
            0xd9cd,
            0x47c5,
            [0x96, 0x29, 0xe1, 0x5d, 0x2f, 0x71, 0x4e, 0x6e],
        ),
        Folder::Startup,
    ),
];

/// The folder the `KNOWNFOLDERID` `id` (its 16 bytes as they lie in memory) names.
pub fn by_known_folder_id(id: &[u8; 16]) -> Option<Folder> {
    KNOWN_FOLDERS
        .iter()
        .find(|(known, _)| known == id)
        .map(|(_, folder)| *folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csidl_values_name_folders_with_their_flags_left_out() {
        assert_eq!(by_csidl(0x0005), Some(Folder::Documents));
        assert_eq!(by_csidl(0x8000 | 0x001a), Some(Folder::RoamingAppData));
        assert_eq!(by_csidl(0x0026), Some(Folder::ProgramFiles));
        assert_eq!(
            by_csidl(0x0024).map(Folder::place),
            Some(Place::Absolute(b"/usr/lib/eggos"))
        );
        assert_eq!(by_csidl(0x0003), None);
    }

    #[test]
    fn known_folder_ids_are_read_as_guids_lie_in_memory() {
        let documents = [
            0xd0, 0x9a, 0xd3, 0xfd, 0x8f, 0x23, 0xaf, 0x46, 0xad, 0xb4, 0x6c, 0x85, 0x48, 0x03,
            0x69, 0xc7,
        ];
        assert_eq!(by_known_folder_id(&documents), Some(Folder::Documents));
        assert_eq!(Folder::Documents.place(), Place::Home(b"Documents"));
        assert_eq!(by_known_folder_id(&[0; 16]), None);
    }
}
