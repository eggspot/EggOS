//! The adapter's firmware file: a header, then type-length-value records, each padded to four
//! bytes. The records that matter here are the firmware's sections for each image (the regular
//! runtime image, the init image), its image loader (AX210 family), its API and capability bits,
//! its command versions, how many channels a scan may name, and a platform NVM some files carry.
//! Records this driver has no use for are skipped. Sections are kept as offsets into the file, so
//! the file's bytes stay the one copy.

use alloc::vec::Vec;
use core::ops::Range;

const MAGIC: u32 = 0x0a4c_5749;
const HEADER_BYTES: usize = 4 + 4 + 64 + 4 + 4 + 8;
const VERSION_AT: usize = 72;
const RECORD_HEADER_BYTES: usize = 8;

const PROBE_MAX_LENGTH: u32 = 6;
const FLAGS: u32 = 18;
const SECTION_REGULAR: u32 = 19;
const SECTION_INIT: u32 = 20;
const SECTION_WOWLAN: u32 = 21;
const PHY_SKU: u32 = 23;
const NUMBER_OF_CPUS: u32 = 27;
const API_CHANGES: u32 = 29;
const ENABLED_CAPABILITIES: u32 = 30;
const SCAN_CHANNELS: u32 = 31;
const PAGING: u32 = 32;
const SECTION_REGULAR_SNIFFER: u32 = 34;
const FIRMWARE_VERSION: u32 = 36;
const COMMAND_VERSIONS: u32 = 48;
const IMAGE_LOADER: u32 = 52;
const PNVM_DATA: u32 = 74;

/// The bits of firmware API flags a file may set.
pub const API_BITS: usize = 128;
/// The bits of firmware capabilities a file may set.
pub const CAPABILITY_BITS: usize = 160;
/// The most sections one image may have.
pub const MAX_SECTIONS: usize = 69;
/// The most command versions a file may list.
pub const MAX_COMMAND_VERSIONS: usize = 400;
/// The most channels a scan request may name.
pub const MAX_SCAN_CHANNELS: u32 = 67;
/// The scan channels a file that does not say allows.
pub const DEFAULT_SCAN_CHANNELS: u32 = 40;

/// The device offset that ends the first CPU's (LMAC) sections and starts the second's (UMAC).
pub const CPU1_CPU2_SEPARATOR: u32 = 0xffff_cccc;
/// The device offset that ends the UMAC's sections and starts the paging ones.
pub const PAGING_SEPARATOR: u32 = 0xaaaa_bbbb;

/// Which of the file's images a section belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Image {
    Regular = 0,
    Init = 1,
    Wowlan = 2,
    RegularSniffer = 3,
}

const IMAGES: usize = 4;

/// One section of an image: where the device loads it, and its bytes in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub device_offset: u32,
    pub bytes: Range<usize>,
}

/// A command's version, as the firmware implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandVersion {
    pub command: u8,
    pub group: u8,
    pub version: u8,
    pub notification: u8,
}

/// The firmware file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    /// Major, minor and API version.
    pub version: (u8, u8, u8),
    pub images: [Vec<Section>; IMAGES],
    pub image_loader: Option<Range<usize>>,
    pub pnvm: Option<Range<usize>>,
    pub api: [u32; API_BITS / 32],
    pub capabilities: [u32; CAPABILITY_BITS / 32],
    pub command_versions: Vec<CommandVersion>,
    pub scan_channels: u32,
    pub probe_max_length: u32,
    pub phy_sku: u32,
    pub flags: u32,
}

/// Why the firmware file was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareError {
    /// It does not start with the firmware header.
    NotFirmware,
    /// A record runs past the end of the file.
    Truncated,
    /// A record holds what its type may not.
    BadRecord(u32),
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

impl Firmware {
    /// Reads the firmware file `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Self, FirmwareError> {
        if bytes.len() < HEADER_BYTES || word(bytes, 0) != Some(0) || word(bytes, 4) != Some(MAGIC) {
            return Err(FirmwareError::NotFirmware);
        }
        let version = word(bytes, VERSION_AT).ok_or(FirmwareError::NotFirmware)?;
        let mut firmware = Firmware {
            version: ((version >> 24) as u8, (version >> 16) as u8, (version >> 8) as u8),
            images: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            image_loader: None,
            pnvm: None,
            api: [0; API_BITS / 32],
            capabilities: [0; CAPABILITY_BITS / 32],
            command_versions: Vec::new(),
            scan_channels: DEFAULT_SCAN_CHANNELS,
            probe_max_length: 0,
            phy_sku: 0,
            flags: 0,
        };
        let mut at = HEADER_BYTES;
        while bytes.len().saturating_sub(at) >= RECORD_HEADER_BYTES {
            let kind = word(bytes, at).ok_or(FirmwareError::Truncated)?;
            let length = word(bytes, at + 4).ok_or(FirmwareError::Truncated)? as usize;
            let start = at + RECORD_HEADER_BYTES;
            let end = start.checked_add(length).filter(|&end| end <= bytes.len()).ok_or(FirmwareError::Truncated)?;
            firmware.record(kind, start..end, bytes)?;
            let padded = length.next_multiple_of(4);
            if start + padded > bytes.len() {
                break;
            }
            at = start + padded;
        }
        Ok(firmware)
    }

    fn record(&mut self, kind: u32, range: Range<usize>, bytes: &[u8]) -> Result<(), FirmwareError> {
        let data = &bytes[range.clone()];
        let bad = || FirmwareError::BadRecord(kind);
        let first = || word(data, 0).ok_or(FirmwareError::BadRecord(kind));
        match kind {
            SECTION_REGULAR => self.section(Image::Regular, range, data, kind)?,
            SECTION_INIT => self.section(Image::Init, range, data, kind)?,
            SECTION_WOWLAN => self.section(Image::Wowlan, range, data, kind)?,
            SECTION_REGULAR_SNIFFER => self.section(Image::RegularSniffer, range, data, kind)?,
            PROBE_MAX_LENGTH => self.probe_max_length = first()?,
            FLAGS => self.flags = first()?,
            PHY_SKU => self.phy_sku = first()?,
            NUMBER_OF_CPUS => {
                if !(1..=2).contains(&first()?) {
                    return Err(bad());
                }
            }
            API_CHANGES | ENABLED_CAPABILITIES => {
                if data.len() != 8 {
                    return Err(bad());
                }
                let (index, bits) = (first()? as usize, word(data, 4).ok_or_else(bad)?);
                let set: &mut [u32] = match kind {
                    API_CHANGES => &mut self.api,
                    _ => &mut self.capabilities,
                };
                *set.get_mut(index).ok_or_else(bad)? = bits;
            }
            SCAN_CHANNELS => {
                let channels = first()?;
                if channels > MAX_SCAN_CHANNELS {
                    return Err(bad());
                }
                self.scan_channels = channels;
            }
            PAGING | FIRMWARE_VERSION => {
                first()?;
            }
            COMMAND_VERSIONS => {
                if !self.command_versions.is_empty() || data.len() / 4 > MAX_COMMAND_VERSIONS {
                    return Err(bad());
                }
                self.command_versions = data
                    .chunks_exact(4)
                    .map(|entry| CommandVersion {
                        command: entry[0],
                        group: entry[1],
                        version: entry[2],
                        notification: entry[3],
                    })
                    .collect();
            }
            IMAGE_LOADER => self.image_loader = Some(range),
            PNVM_DATA => {
                if self.pnvm.is_none() {
                    self.pnvm = Some(range);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn section(&mut self, image: Image, range: Range<usize>, data: &[u8], kind: u32) -> Result<(), FirmwareError> {
        let sections = &mut self.images[image as usize];
        if data.len() < 4 || sections.len() >= MAX_SECTIONS {
            return Err(FirmwareError::BadRecord(kind));
        }
        sections.push(Section {
            device_offset: word(data, 0).ok_or(FirmwareError::BadRecord(kind))?,
            bytes: range.start + 4..range.end,
        });
        Ok(())
    }

    /// Whether API flag `bit` is set.
    pub fn has_api(&self, bit: usize) -> bool {
        self.api.get(bit / 32).is_some_and(|word| word & (1 << (bit % 32)) != 0)
    }

    /// Whether capability `bit` is set.
    pub fn has_capability(&self, bit: usize) -> bool {
        self.capabilities.get(bit / 32).is_some_and(|word| word & (1 << (bit % 32)) != 0)
    }

    /// The version the firmware implements of command `command` in group `group`, if it says.
    pub fn command_version(&self, group: u8, command: u8) -> Option<u8> {
        self.command_versions
            .iter()
            .find(|entry| entry.group == group && entry.command == command)
            .map(|entry| entry.version)
    }

    /// `image`'s sections split at the separators: the first CPU's (LMAC), the second's
    /// (UMAC), and the paging ones, each without its separator.
    pub fn split(&self, image: Image) -> [&[Section]; 3] {
        let sections = &self.images[image as usize][..];
        let is_separator = |section: &Section| {
            section.device_offset == CPU1_CPU2_SEPARATOR || section.device_offset == PAGING_SEPARATOR
        };
        let run = |from: usize| {
            let from = from.min(sections.len());
            let length = sections[from..].iter().take_while(|section| !is_separator(section)).count();
            &sections[from..from + length]
        };
        let lmac = run(0);
        let umac = run(lmac.len() + 1);
        let paging = run(lmac.len() + umac.len() + 2);
        [lmac, umac, paging]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn file(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[4..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[VERSION_AT..VERSION_AT + 4].copy_from_slice(&0x4d00_4d00u32.to_le_bytes());
        for (kind, data) in records {
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(data);
            bytes.resize(bytes.len().next_multiple_of(4), 0);
        }
        bytes
    }

    fn section(offset: u32, body: &[u8]) -> Vec<u8> {
        let mut data = offset.to_le_bytes().to_vec();
        data.extend_from_slice(body);
        data
    }

    #[test]
    fn sections_split_at_the_separators_into_lmac_umac_and_paging() {
        let bytes = file(&[
            (SECTION_REGULAR, section(0x0040_0000, b"lmac one")),
            (SECTION_REGULAR, section(0x0040_8000, b"lmac two")),
            (SECTION_REGULAR, section(CPU1_CPU2_SEPARATOR, b"")),
            (SECTION_REGULAR, section(0x0080_0000, b"umac")),
            (SECTION_REGULAR, section(PAGING_SEPARATOR, b"")),
            (SECTION_REGULAR, section(0x0000_0000, b"page")),
            (SECTION_INIT, section(0x0040_0000, b"init")),
            (999, b"unknown, skipped".to_vec()),
        ]);
        let firmware = Firmware::parse(&bytes).unwrap();
        assert_eq!(firmware.version, (0x4d, 0, 0x4d));
        let [lmac, umac, paging] = firmware.split(Image::Regular);
        assert_eq!(lmac.len(), 2);
        assert_eq!(&bytes[lmac[1].bytes.clone()], b"lmac two");
        assert_eq!(umac.len(), 1);
        assert_eq!(&bytes[umac[0].bytes.clone()], b"umac");
        assert_eq!(paging.len(), 1);
        assert_eq!(firmware.split(Image::Init)[0].len(), 1);
    }

    #[test]
    fn api_capabilities_and_command_versions_are_kept() {
        let mut api = 1u32.to_le_bytes().to_vec();
        api.extend_from_slice(&(1u32 << 3).to_le_bytes());
        let mut capa = 0u32.to_le_bytes().to_vec();
        capa.extend_from_slice(&(1u32 << 5).to_le_bytes());
        let bytes = file(&[
            (API_CHANGES, api),
            (ENABLED_CAPABILITIES, capa),
            (COMMAND_VERSIONS, vec![0x0c, 0x01, 3, 0, 0x03, 0x00, 12, 2]),
            (SCAN_CHANNELS, 67u32.to_le_bytes().to_vec()),
            (IMAGE_LOADER, b"loader".to_vec()),
        ]);
        let firmware = Firmware::parse(&bytes).unwrap();
        assert!(firmware.has_api(35) && !firmware.has_api(3));
        assert!(firmware.has_capability(5));
        assert_eq!(firmware.command_version(0x01, 0x0c), Some(3));
        assert_eq!(firmware.command_version(0x00, 0x03), Some(12));
        assert_eq!(firmware.command_version(0x02, 0x0c), None);
        assert_eq!(firmware.scan_channels, 67);
        assert_eq!(&bytes[firmware.image_loader.unwrap()], b"loader");
    }

    #[test]
    fn bad_headers_records_and_lengths_are_refused() {
        assert_eq!(Firmware::parse(b"short"), Err(FirmwareError::NotFirmware));
        let mut bytes = file(&[]);
        bytes[4] ^= 1;
        assert_eq!(Firmware::parse(&bytes), Err(FirmwareError::NotFirmware));
        let mut bytes = file(&[(SCAN_CHANNELS, 68u32.to_le_bytes().to_vec())]);
        assert_eq!(Firmware::parse(&bytes), Err(FirmwareError::BadRecord(SCAN_CHANNELS)));
        bytes = file(&[(SECTION_REGULAR, section(1, b"x"))]);
        let length_at = HEADER_BYTES + 4;
        bytes[length_at..length_at + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(Firmware::parse(&bytes), Err(FirmwareError::Truncated));
    }
}
