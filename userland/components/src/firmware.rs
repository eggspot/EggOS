//! The vendors' firmware the drivers load (D265): files from a tagged linux-firmware release,
//! each pinned by its SHA-256, that eggspot mirrors in its component store as optional
//! components. None of it is in the repository or the base image: the store build fetches the
//! pinned files, and a machine whose store cannot be reached fetches them from linux-firmware
//! itself, checked against the same pins. Each component carries its files' licence texts and
//! their entries from linux-firmware's `WHENCE`.

use crate::index::{self, Component, File};
use alloc::string::String;
use alloc::vec::Vec;

/// The linux-firmware release the files are pinned to.
pub const LINUX_FIRMWARE_TAG: &str = "20261019";
/// Where linux-firmware publishes a release's files, the fallback source.
pub const UPSTREAM: &str = "https://gitlab.com/kernel-firmware/linux-firmware/-/raw/";
/// The folder the drivers load firmware from.
pub const FIRMWARE_FOLDER: &str = "usr/lib/firmware";
/// The name of the file holding a component's entries from linux-firmware's `WHENCE`.
pub const WHENCE_EXCERPT: &str = "WHENCE";

/// One file of linux-firmware's release, by its path in the release and its SHA-256 in hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    /// Its path in the release, `intel/iwlwifi/iwlwifi-cc-a0-77.ucode`.
    pub path: &'static str,
    /// Its bytes' SHA-256, in lowercase hex.
    pub sha256: &'static str,
}

/// linux-firmware's list of every file, its driver and its licence.
pub const WHENCE: Pinned = Pinned {
    path: "WHENCE",
    sha256: "0dd59e46f1e5d33d112236b17cb6de9d8f46fbf8fd09685cb474eb70a55f4da1",
};

/// One optional firmware component.
#[derive(Debug)]
pub struct FirmwareComponent {
    /// Its name, `wifi-firmware`.
    pub name: &'static str,
    /// Its title.
    pub title: &'static str,
    /// One line about it.
    pub summary: &'static str,
    /// The sentence the remove confirmation shows.
    pub on_remove: &'static str,
    /// The folder below [`FIRMWARE_FOLDER`] it owns, which the base links into it.
    pub folder: &'static str,
    /// The architectures whose machines carry the devices.
    pub arches: &'static [&'static str],
    /// The firmware files.
    pub files: &'static [Pinned],
    /// The licence texts that cover them.
    pub licences: &'static [Pinned],
}

/// Every firmware component the store offers.
pub const COMPONENTS: [FirmwareComponent; 2] = [
    FirmwareComponent {
        name: "wifi-firmware",
        title: "Intel Wi-Fi firmware",
        summary: "Firmware for Intel Wi-Fi 6 and 6E adapters (AX200, AX201, AX210, AX211)",
        on_remove: "Intel Wi-Fi adapters stop working until it is installed again.",
        folder: "intel/iwlwifi",
        arches: &["x86_64"],
        files: &[
            Pinned {
                path: "intel/iwlwifi/iwlwifi-cc-a0-77.ucode",
                sha256: "1866a58a1daee56b120362122d5f7548b443b53d33f2558a7e4407d58fd27dab",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-QuZ-a0-hr-b0-77.ucode",
                sha256: "39d79587dd035bc84172b6bcf6de81217560088b0ceb06304d5121ba56db8d83",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-Qu-b0-hr-b0-77.ucode",
                sha256: "df005896aafdf5eb9c427033781890a58c7ab97a8dc1e1ea6b8df4eefc033a9d",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-Qu-c0-hr-b0-77.ucode",
                sha256: "a250e179666288396e19ff15c307edfe8f089341c8713a22b457b7304fc1a6fe",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-ty-a0-gf-a0-77.ucode",
                sha256: "fd8977dbfacfe50b8a6601479aa9ffd21d01e274e4a9efd13935dd9d667fd45b",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-ty-a0-gf-a0.pnvm",
                sha256: "9697e327f8171454b3c19b1ec9449753d5b934d4e91722f469616818a6d477b0",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-so-a0-gf-a0-77.ucode",
                sha256: "87590ea9535a8939fddb296bc7622f3a900f28f47b45cb4e435a2a1d5f496ccd",
            },
            Pinned {
                path: "intel/iwlwifi/iwlwifi-so-a0-gf-a0.pnvm",
                sha256: "78abe08cf33bc209b96a887c7e3db8855a1d9650edacf849c4fa0d4596bcfac5",
            },
        ],
        licences: &[Pinned {
            path: "LICENSES/LICENCE.iwlwifi_firmware",
            sha256: "16d5040c7cf851fc693b7542e20870935b99802533ea1bfd231c377a2305e5c5",
        }],
    },
    FirmwareComponent {
        name: "display-firmware",
        title: "Intel display firmware",
        summary: "Display power-saving firmware for Intel graphics (Tiger Lake to Meteor Lake)",
        on_remove: "Intel displays still work, without their deepest power saving.",
        folder: "i915",
        arches: &["x86_64"],
        files: &[
            Pinned {
                path: "i915/tgl_dmc_ver2_12.bin",
                sha256: "3c013ef0ad96ba73aee8e5bd04a8e27cc9b1c6e9183b1a83ce124485f325afca",
            },
            Pinned {
                path: "i915/adls_dmc_ver2_01.bin",
                sha256: "96e426ed7e80f0cc6644ada1928f4d02b163ff59b14b2339aa28a374d65a5b19",
            },
            Pinned {
                path: "i915/adlp_dmc.bin",
                sha256: "3516de2e134ddcf3b319c75d2e437779fecbd58cbb77234bd6f297c544e92ccb",
            },
            Pinned {
                path: "i915/mtl_dmc.bin",
                sha256: "1b3b826195d50fae6611d29e0eb348e641a3ada3e0436ee0b987124f47a0e0db",
            },
        ],
        licences: &[Pinned {
            path: "LICENSES/LICENSE.i915",
            sha256: "8542aeabf2761935122d693561e16766ce1bcc2b0d003204f9040b7d6d929f2e",
        }],
    },
];

/// The firmware component named `name`.
pub fn find(name: &str) -> Option<&'static FirmwareComponent> {
    COMPONENTS.iter().find(|component| component.name == name)
}

/// The firmware components machines of `arch` may need.
pub fn for_arch(arch: &str) -> impl Iterator<Item = &'static FirmwareComponent> + '_ {
    COMPONENTS
        .iter()
        .filter(move |component| component.arches.contains(&arch))
}

/// Where linux-firmware publishes `file` of the pinned release.
pub fn upstream_url(file: &Pinned) -> String {
    let mut url = String::from(UPSTREAM);
    url.push_str(LINUX_FIRMWARE_TAG);
    url.push('/');
    url.push_str(file.path);
    url
}

/// The folder `component` owns, below `/`: `usr/lib/firmware/i915`.
pub fn folder_path(component: &FirmwareComponent) -> String {
    let mut path = String::from(FIRMWARE_FOLDER);
    path.push('/');
    path.push_str(component.folder);
    path
}

fn in_folder(component: &FirmwareComponent, name: &str) -> String {
    let mut path = folder_path(component);
    path.push('/');
    path.push_str(name);
    path
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The hex of `digest`.
pub fn hex(digest: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// Whether `bytes` are the pinned `file`.
pub fn matches_pin(file: &Pinned, bytes: &[u8]) -> bool {
    hex(&index::sha256(bytes)) == file.sha256
}

/// The entries of `whence` (linux-firmware's `WHENCE`) for `files`: of each driver's block that
/// lists one of them, its header, its licence lines and notes, and the paragraphs of those files,
/// leaving out the paragraphs of the driver's other files.
pub fn whence_entries(whence: &str, files: &[Pinned]) -> String {
    let mut excerpt = String::new();
    for block in blocks(whence) {
        let paragraphs: Vec<&str> = block.split("\n\n").map(|paragraph| paragraph.trim_matches('\n')).collect();
        let ours = |paragraph: &str| {
            paragraph
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("File:"))
                .map(str::trim)
                .is_some_and(|listed| files.iter().any(|file| file.path == listed))
        };
        if !paragraphs.iter().any(|paragraph| ours(paragraph)) {
            continue;
        }
        for paragraph in paragraphs {
            if !paragraph.is_empty() && (!paragraph.starts_with("File:") || ours(paragraph)) {
                excerpt.push_str(paragraph);
                excerpt.push_str("\n\n");
            }
        }
        excerpt.push_str(SEPARATOR);
        excerpt.push_str("\n\n");
    }
    excerpt
}

const SEPARATOR: &str = "--------------------------------------------------------------------------";

fn is_separator(line: &str) -> bool {
    line.len() >= 60 && line.bytes().all(|byte| byte == b'-')
}

fn blocks(whence: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut block = String::new();
    for line in whence.lines() {
        if is_separator(line) {
            blocks.push(core::mem::take(&mut block));
        } else {
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks.push(block);
    blocks
}

/// Why a firmware component's files could not be gathered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gathering {
    /// The file could not be fetched.
    Unreachable(&'static str),
    /// The file fetched is not the pinned one.
    NotPinned(&'static str),
}

/// `component`'s files below `/` with their bytes: its firmware, its licence texts and its
/// `WHENCE` entries, all in its folder. `fetch` gets a file of the pinned release; every file is
/// checked against its pin.
pub fn gather(
    component: &FirmwareComponent,
    fetch: &mut dyn FnMut(&Pinned) -> Option<Vec<u8>>,
) -> Result<Vec<(String, Vec<u8>)>, Gathering> {
    let mut fetch_pinned = |file: &Pinned| -> Result<Vec<u8>, Gathering> {
        let bytes = fetch(file).ok_or(Gathering::Unreachable(file.path))?;
        match matches_pin(file, &bytes) {
            true => Ok(bytes),
            false => Err(Gathering::NotPinned(file.path)),
        }
    };
    let mut gathered = Vec::new();
    for file in component.files {
        let path = in_folder(component, &file.path[component.folder.len() + 1..]);
        gathered.push((path, fetch_pinned(file)?));
    }
    for licence in component.licences {
        gathered.push((in_folder(component, base_name(licence.path)), fetch_pinned(licence)?));
    }
    let whence = fetch_pinned(&WHENCE)?;
    let excerpt = whence_entries(&String::from_utf8_lossy(&whence), component.files);
    gathered.push((in_folder(component, WHENCE_EXCERPT), excerpt.into_bytes()));
    Ok(gathered)
}

/// `component`'s record for a store, its files at offset 0 until [`index::store_image`] places
/// them.
pub fn record(component: &FirmwareComponent, gathered: &[(String, Vec<u8>)]) -> Component {
    Component {
        name: component.name.into(),
        title: component.title.into(),
        summary: component.summary.into(),
        on_remove: component.on_remove.into(),
        needs: Vec::new(),
        files: gathered
            .iter()
            .map(|(path, bytes)| File {
                path: path.clone(),
                mode: 0o644,
                offset: 0,
                length: bytes.len() as u64,
                sha256: index::sha256(bytes),
            })
            .collect(),
    }
}
