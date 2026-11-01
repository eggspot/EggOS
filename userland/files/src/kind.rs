//! What an entry is, as Files shows and opens it (docs/ux/desktop-v0.3-gui.md section 8.1
//! items 4 and 11): read from a file's first bytes, never from its name (D134), and for a
//! folder from where it is (an app's folder in `/Applications`) or its name (an `.app`). A
//! symbolic link is what its target is where it really is (docs/ux/files-links-v0.3.md).

use crate::names::{is_within, joined, parent, resolved};
use eggos_app::architecture::Architecture;
use eggos_app::archive;
use eggos_app::files::{self, Files, Path};
use eggos_app::installer::{self, APPLICATIONS};
use eggos_app::messages;
use eggos_app::program;
use eggos_app_manifest::{self as manifest, Source};

/// How many bytes of a file decide what it is.
pub const HEAD_BYTES: usize = 1024;
const ELF: &[u8] = b"\x7fELF";
const MZ: &[u8] = b"MZ";
const MACH_O: [[u8; 4]; 2] = [[0xcf, 0xfa, 0xed, 0xfe], [0xce, 0xfa, 0xed, 0xfe]];
const FAT: [u8; 4] = [0xca, 0xfe, 0xba, 0xbe];
const MOST_FAT_ARCHITECTURES: u32 = 32;

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A folder.
    Folder,
    /// An installed app's folder in `/Applications`, holding a valid `app.manifest`.
    App(Source),
    /// A macOS `.app` bundle outside `/Applications`.
    Bundle,
    /// A program of another system or of Linux.
    Program(Source),
    /// A `.zip` archive, which the installer opens (D148).
    Archive,
    /// A file of UTF-8 text.
    Text,
    /// Any other file.
    File,
    /// A device, a broken link or anything else.
    Other,
}

/// Whether an entry is a symbolic link, and whether what it points to is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Not a link.
    None,
    /// A link to something that is there.
    Sound,
    /// A link to nothing, or one that loops back to itself.
    Broken,
}

impl Kind {
    /// Whether it is a folder of any sort, sorted with the folders.
    pub fn is_folder(self) -> bool {
        matches!(self, Self::Folder | Self::App(_) | Self::Bundle)
    }

    /// The system it was built for, for a program, an app or a bundle: its badge.
    pub fn source(self) -> Option<Source> {
        match self {
            Self::App(source) | Self::Program(source) => Some(source),
            Self::Bundle => Some(Source::Macos),
            _ => None,
        }
    }

    /// Whether `Install...` applies: a program, a bundle or a `.zip`.
    pub fn installs(self) -> bool {
        matches!(self, Self::Program(_) | Self::Bundle | Self::Archive)
    }
}

/// What a file whose first bytes are `head` is.
pub fn of_head(head: &[u8]) -> Kind {
    let word = |at: usize| {
        head.get(at..at + 4)
            .map(|bytes| [bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    if head.starts_with(ELF) {
        return Kind::Program(Source::Linux);
    }
    if head.starts_with(MZ) {
        return Kind::Program(Source::Windows);
    }
    if let Some(magic) = word(0) {
        if MACH_O.contains(&magic) {
            return Kind::Program(Source::Macos);
        }
        let architectures = word(4).map_or(0, u32::from_be_bytes);
        if magic == FAT && (1..=MOST_FAT_ARCHITECTURES).contains(&architectures) {
            return Kind::Program(Source::Macos);
        }
    }
    if archive::is_archive(head) {
        return Kind::Archive;
    }
    if head.contains(&0) {
        return Kind::File;
    }
    match core::str::from_utf8(head) {
        Ok(_) => Kind::Text,
        Err(error) if error.error_len().is_none() => Kind::Text,
        Err(_) => Kind::File,
    }
}

/// Reads up to [`HEAD_BYTES`] of the file at `path` into `head`: how many.
pub fn read_head(files: &mut impl Files, path: &[u8], head: &mut [u8; HEAD_BYTES]) -> usize {
    let Ok(file) = files.open(path) else {
        return 0;
    };
    let mut filled = 0;
    while filled < head.len() {
        match files.read(file, &mut head[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    files.close(file);
    filled
}

/// The app whose folder is `path` in `/Applications`: its source, if its manifest is valid.
pub fn app_folder(files: &mut impl Files, path: &[u8]) -> Option<Source> {
    let name = files::file_name(path);
    if name.is_empty() || crate::names::parent(path) != APPLICATIONS {
        return None;
    }
    let mut file = [0u8; manifest::LARGEST + 1];
    let length = installer::read_manifest(files, name, &mut file)?;
    manifest::parse(&file[..length], name).map(|app| app.source)
}

/// What the entry named `name` in `folder` is, given what its status says.
pub fn of_entry(files: &mut impl Files, folder: &Path, name: &[u8], kind: files::Kind) -> Kind {
    let Some(path) = joined(folder, name) else {
        return Kind::Other;
    };
    match kind {
        files::Kind::Directory => {
            if let Some(source) = app_folder(files, path.as_bytes()) {
                return Kind::App(source);
            }
            match installer::is_bundle_name(name) && !is_within(path.as_bytes(), APPLICATIONS) {
                true => Kind::Bundle,
                false => Kind::Folder,
            }
        }
        files::Kind::File => {
            let mut head = [0u8; HEAD_BYTES];
            let length = read_head(files, path.as_bytes(), &mut head);
            of_head(&head[..length])
        }
        files::Kind::Link | files::Kind::Other => Kind::Other,
    }
}

/// What the target of the symbolic link at `path` is, as if it were listed where it really
/// is; [`Kind::Other`] when it cannot be found.
pub fn of_target(files: &mut impl Files, path: &[u8]) -> Kind {
    let Ok(real) = resolved(files, path) else {
        return Kind::Other;
    };
    let Ok(status) = files.status(real.as_bytes()) else {
        return Kind::Other;
    };
    let Some(folder) = Path::from(parent(real.as_bytes())) else {
        return Kind::Other;
    };
    of_entry(files, &folder, real.file_name(), status.kind)
}

/// What Properties says an entry is: `Directory`, `Text`, `File`, `Windows program, x86_64`,
/// `macOS program, arm64`, `Linux program` or `App, Windows`; a program this machine cannot
/// run is named by its system alone. A `link` says `Link to ` before what its target is, or
/// `Broken link`.
pub fn describe(
    files: &mut impl Files,
    path: &[u8],
    kind: Kind,
    link: Link,
    machine: Architecture,
) -> messages::Line {
    let mut line = messages::Line::new();
    match link {
        Link::None => {}
        Link::Sound => line.push_bytes(b"Link to "),
        Link::Broken => {
            line.push_bytes(b"Broken link");
            return line;
        }
    }
    match kind {
        Kind::Folder | Kind::Bundle => line.push_bytes(b"Directory"),
        Kind::App(source) => {
            line.push_bytes(b"App, ");
            line.push_bytes(source.label().as_bytes());
        }
        Kind::Text => line.push_bytes(b"Text"),
        Kind::Archive => line.push_bytes(b"Zip archive"),
        Kind::File | Kind::Other => line.push_bytes(b"File"),
        Kind::Program(source) => {
            let mut head = [0u8; program::HEAD_BYTES];
            let length = match files.open(path) {
                Ok(file) => {
                    let read = files.read(file, &mut head).unwrap_or(0);
                    files.close(file);
                    read
                }
                Err(_) => 0,
            };
            match program::recognise(&head[..length], machine) {
                Ok(recognised) => line.push_bytes(messages::kind(recognised).as_str().as_bytes()),
                Err(_) => {
                    line.push_bytes(source.label().as_bytes());
                    line.push_bytes(b" program");
                }
            }
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_told_by_their_first_bytes() {
        assert_eq!(of_head(b"\x7fELF\x02\x01"), Kind::Program(Source::Linux));
        assert_eq!(of_head(b"MZ\x90\x00"), Kind::Program(Source::Windows));
        assert_eq!(
            of_head(&[0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1]),
            Kind::Program(Source::Macos)
        );
        assert_eq!(
            of_head(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]),
            Kind::Program(Source::Macos)
        );
        assert_eq!(of_head(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52]), Kind::File);
    }

    #[test]
    fn text_is_utf8_without_nul_even_when_the_head_cuts_a_character() {
        assert_eq!(of_head(b"hello\nworld\n"), Kind::Text);
        assert_eq!(of_head(b""), Kind::Text);
        assert_eq!(of_head(b"caf\xc3"), Kind::Text);
        assert_eq!(of_head(b"a\0b"), Kind::File);
        assert_eq!(of_head(b"\xff\xfe"), Kind::File);
    }

    #[test]
    fn kinds_say_what_they_install_and_sort_with() {
        assert!(Kind::Bundle.is_folder());
        assert!(Kind::App(Source::Linux).is_folder());
        assert!(Kind::Program(Source::Linux).installs());
        assert!(!Kind::App(Source::Windows).installs());
        assert_eq!(Kind::Bundle.source(), Some(Source::Macos));
        assert_eq!(Kind::Text.source(), None);
        assert!(Kind::Archive.installs());
        assert_eq!(of_head(b"PK\x03\x04\x14\x00"), Kind::Archive);
        assert_eq!(of_head(b"PK\x05\x06\x00\x00"), Kind::Archive);
    }

    #[test]
    fn a_link_is_what_its_target_is_where_it_really_is() {
        let mut files = crate::memory::MemoryFiles::new();
        files
            .file(
                "/Applications/Paint/app.manifest",
                b"eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\n",
                1,
            )
            .file("/home/root/notes.txt", b"hi\n", 1)
            .folder("/home/root/Old.app")
            .link("/home/root/Paint link", "/Applications/Paint", 1)
            .link("/home/root/Old link", "Old.app", 1)
            .link("/home/root/notes link.txt", "notes.txt", 1)
            .link("/home/root/gone", "nothing", 1);
        let mut of = |path: &[u8]| of_target(&mut files, path);
        assert_eq!(of(b"/home/root/Paint link"), Kind::App(Source::Windows));
        assert_eq!(of(b"/home/root/Old link"), Kind::Bundle);
        assert_eq!(of(b"/home/root/notes link.txt"), Kind::Text);
        assert_eq!(of(b"/home/root/gone"), Kind::Other);
    }

    #[test]
    fn properties_say_link_to_before_the_targets_kind() {
        let mut files = crate::memory::MemoryFiles::new();
        files.file("/home/root/notes.txt", b"hi\n", 1);
        let machine = Architecture::X86_64;
        let path = b"/home/root/notes.txt";
        let said = |line: messages::Line| line.as_str().to_owned();
        assert_eq!(
            said(describe(&mut files, path, Kind::Text, Link::Sound, machine)),
            "Link to Text"
        );
        assert_eq!(
            said(describe(
                &mut files,
                path,
                Kind::Folder,
                Link::Sound,
                machine
            )),
            "Link to Directory"
        );
        assert_eq!(
            said(describe(
                &mut files,
                path,
                Kind::Other,
                Link::Broken,
                machine
            )),
            "Broken link"
        );
        assert_eq!(
            said(describe(&mut files, path, Kind::Text, Link::None, machine)),
            "Text"
        );
    }
}
