//! NVIDIA's GPU System Processor firmware, the part that can be read without running it
//! (D469). From Turing on, the display and the rest of the chip are driven through a RISC-V
//! processor on the GPU running NVIDIA's GSP firmware, published for open drivers as ELF
//! files: `.fwimage` is the image GSP runs, `.fwversion` its version, and one
//! `.fwsignature_<family>` per chip family its signature. Which file a board needs follows
//! from its chip, named by `PMC_BOOT_0`, the first register of BAR 0.

/// The register naming the chip.
pub const BOOT_0: usize = 0;
const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const ELF_64: u8 = 2;
const SECTION_HEADERS: usize = 0x28;
const SECTION_HEADER_BYTES: usize = 0x3a;
const SECTION_COUNT: usize = 0x3c;
const SECTION_NAMES: usize = 0x3e;
const NAME: usize = 0;
const OFFSET: usize = 0x18;
const SIZE: usize = 0x20;
const SIGNATURE_PREFIX: &[u8] = b".fwsignature_";

/// What a chip is, from `PMC_BOOT_0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chip {
    /// The chip id, `0x164` for TU104.
    pub id: u16,
}

impl Chip {
    /// The chip `PMC_BOOT_0`'s value names.
    pub fn of_boot_0(boot_0: u32) -> Self {
        Self {
            id: ((boot_0 >> 20) & 0x1ff) as u16,
        }
    }

    /// Its architecture: `0x16` Turing, `0x17` Ampere, `0x19` Ada.
    pub fn architecture(self) -> u16 {
        self.id >> 4
    }

    /// Whether it runs GSP firmware: Turing and later.
    pub fn runs_gsp(self) -> bool {
        self.architecture() >= 0x16
    }

    /// Its name as firmware folders spell it (`tu104`), for the chips GSP firmware is
    /// published for.
    pub fn name(self) -> Option<&'static str> {
        Some(match self.id {
            0x162 => "tu102",
            0x164 => "tu104",
            0x166 => "tu106",
            0x167 => "tu117",
            0x168 => "tu116",
            0x172 => "ga102",
            0x173 => "ga103",
            0x174 => "ga104",
            0x176 => "ga106",
            0x177 => "ga107",
            0x192 => "ad102",
            0x193 => "ad103",
            0x194 => "ad104",
            0x196 => "ad106",
            0x197 => "ad107",
            _ => return None,
        })
    }

    /// The signature family its GSP firmware's section is named for (`tu10x`).
    pub fn signature_family(self) -> Option<&'static str> {
        Some(match self.architecture() {
            0x16 => match self.id {
                0x167 | 0x168 => "tu11x",
                _ => "tu10x",
            },
            0x17 => "ga10x",
            0x19 => "ad10x",
            _ => return None,
        })
    }
}

/// Where a GSP firmware file's parts are: each as its start and length in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GspFirmware {
    pub image: (usize, usize),
    pub version: Option<(usize, usize)>,
    pub signature: Option<(usize, usize)>,
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn double(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The parts of GSP firmware file `file` for signature family `family`; `None` when it is
/// not a 64-bit ELF file with a `.fwimage` section inside it.
pub fn gsp_firmware(file: &[u8], family: &str) -> Option<GspFirmware> {
    if file.get(..4)? != ELF_MAGIC || *file.get(4)? != ELF_64 {
        return None;
    }
    let headers = usize::try_from(double(file, SECTION_HEADERS)?).ok()?;
    let header_bytes = usize::from(half(file, SECTION_HEADER_BYTES)?);
    let count = usize::from(half(file, SECTION_COUNT)?);
    let names_index = usize::from(half(file, SECTION_NAMES)?);
    let section = |index: usize| -> Option<(u32, usize, usize)> {
        let at = headers.checked_add(index.checked_mul(header_bytes)?)?;
        let offset = usize::try_from(double(file, at + OFFSET)?).ok()?;
        let size = usize::try_from(double(file, at + SIZE)?).ok()?;
        offset.checked_add(size).filter(|&end| end <= file.len())?;
        Some((word(file, at + NAME)?, offset, size))
    };
    let (_, names, names_size) = section(names_index)?;
    let name_of = |name: u32| -> Option<&[u8]> {
        let start = names.checked_add(name as usize)?;
        let text = file.get(start..names + names_size)?;
        Some(&text[..text.iter().position(|&byte| byte == 0)?])
    };
    let mut firmware = GspFirmware {
        image: (0, 0),
        version: None,
        signature: None,
    };
    let mut has_image = false;
    for index in 0..count {
        let Some((name, offset, size)) = section(index) else {
            continue;
        };
        let Some(name) = name_of(name) else {
            continue;
        };
        match name {
            b".fwimage" => {
                firmware.image = (offset, size);
                has_image = true;
            }
            b".fwversion" => firmware.version = Some((offset, size)),
            name if name.strip_prefix(SIGNATURE_PREFIX) == Some(family.as_bytes()) => {
                firmware.signature = Some((offset, size));
            }
            _ => {}
        }
    }
    has_image.then_some(firmware)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_0_names_the_chip_its_architecture_and_firmware_family() {
        let chip = Chip::of_boot_0(0x1640_00a1);
        assert_eq!(chip.id, 0x164);
        assert_eq!(chip.name(), Some("tu104"));
        assert_eq!(chip.signature_family(), Some("tu10x"));
        assert!(chip.runs_gsp());
        assert_eq!(Chip::of_boot_0(0x1680_00a1).signature_family(), Some("tu11x"));
        assert!(!Chip::of_boot_0(0x1400_00a1).runs_gsp());
    }

    fn elf(sections: &[(&str, &[u8])]) -> Vec<u8> {
        let mut file = vec![0u8; 64];
        file[..4].copy_from_slice(ELF_MAGIC);
        file[4] = ELF_64;
        let mut names = vec![0u8];
        let mut placed = Vec::new();
        for (name, body) in sections {
            placed.push((names.len() as u32, file.len(), body.len()));
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            file.extend_from_slice(body);
        }
        let names_at = file.len();
        file.extend_from_slice(&names);
        placed.push((0, names_at, names.len()));
        let headers = file.len();
        for (name, offset, size) in &placed {
            let mut header = vec![0u8; 64];
            header[..4].copy_from_slice(&name.to_le_bytes());
            header[OFFSET..OFFSET + 8].copy_from_slice(&(*offset as u64).to_le_bytes());
            header[SIZE..SIZE + 8].copy_from_slice(&(*size as u64).to_le_bytes());
            file.extend(header);
        }
        file[SECTION_HEADERS..SECTION_HEADERS + 8].copy_from_slice(&(headers as u64).to_le_bytes());
        file[SECTION_HEADER_BYTES..SECTION_HEADER_BYTES + 2].copy_from_slice(&64u16.to_le_bytes());
        file[SECTION_COUNT..SECTION_COUNT + 2]
            .copy_from_slice(&(placed.len() as u16).to_le_bytes());
        file[SECTION_NAMES..SECTION_NAMES + 2]
            .copy_from_slice(&((placed.len() - 1) as u16).to_le_bytes());
        file
    }

    #[test]
    fn the_image_version_and_the_family_s_signature_are_found() {
        let file = elf(&[
            (".fwimage", &[1, 2, 3, 4]),
            (".fwversion", b"535"),
            (".fwsignature_ga10x", &[9; 8]),
            (".fwsignature_tu10x", &[7; 16]),
        ]);
        let firmware = gsp_firmware(&file, "tu10x").unwrap();
        assert_eq!(&file[firmware.image.0..][..firmware.image.1], &[1, 2, 3, 4]);
        let (at, length) = firmware.signature.unwrap();
        assert_eq!(&file[at..at + length], &[7; 16]);
        assert_eq!(firmware.version.map(|(_, length)| length), Some(3));
        assert_eq!(gsp_firmware(&elf(&[(".text", &[0])]), "tu10x"), None);
    }
}
