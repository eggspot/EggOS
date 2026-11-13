//! AMD's video BIOS, the ATOM tables (D448): a PCI expansion ROM whose ATOM ROM header points
//! at a master list of data tables. The display object table lists the board's display paths
//! (each a connector, its encoder and its device tag), and the firmware table its revision and
//! boot clocks. [`AtomBios::parse`] reads both from the image, the layout of AMD's published
//! `atomfirmware.h` (the display object table at 1.4 and 1.5).

/// The most display paths read.
pub const MOST_PATHS: usize = 8;
const ROM_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const ROM_HEADER_POINTER: usize = 0x48;
const ATOM_SIGNATURE: &[u8; 4] = b"ATOM";
const MASTER_DATA_TABLE: usize = 32;
const TABLE_HEADER_BYTES: usize = 4;
const FIRMWARE_INFO: usize = 4;
const DISPLAY_OBJECT_INFO: usize = 22;
const PATH_BYTES: usize = 18;
const OBJECT_TYPE_CONNECTOR: u16 = 3;

/// What a board's connector is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    Dvi,
    Vga,
    Hdmi,
    DisplayPort,
    EmbeddedDisplayPort,
    Lvds,
    Other(u8),
}

impl Connector {
    fn of(object: u16) -> Option<Self> {
        if object >> 12 & 0xf != OBJECT_TYPE_CONNECTOR {
            return None;
        }
        Some(match (object & 0xff) as u8 {
            0x01..=0x04 => Self::Dvi,
            0x05 => Self::Vga,
            0x0c | 0x0d => Self::Hdmi,
            0x13 => Self::DisplayPort,
            0x14 => Self::EmbeddedDisplayPort,
            0x0e => Self::Lvds,
            other => Self::Other(other),
        })
    }

    /// Its name, as `/proc/driver/display` prints it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dvi => "DVI",
            Self::Vga => "VGA",
            Self::Hdmi => "HDMI",
            Self::DisplayPort => "DisplayPort",
            Self::EmbeddedDisplayPort => "eDP",
            Self::Lvds => "LVDS",
            Self::Other(_) => "other",
        }
    }
}

/// One display path: its connector and the encoder that drives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayPath {
    pub connector: Connector,
    pub encoder: u16,
    pub device_tag: u16,
}

/// What the tables say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomBios {
    pub paths: [Option<DisplayPath>; MOST_PATHS],
    pub firmware_revision: Option<u32>,
    /// The engine clock at boot, in kHz.
    pub boot_engine_khz: Option<u32>,
}

fn half(image: &[u8], at: usize) -> Option<u16> {
    let field = image.get(at..at + 2)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
}

fn word(image: &[u8], at: usize) -> Option<u32> {
    let field = image.get(at..at + 4)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn table(image: &[u8], master: usize, index: usize) -> Option<usize> {
    let at = usize::from(half(image, master + TABLE_HEADER_BYTES + 2 * index)?);
    (at != 0 && at < image.len()).then_some(at)
}

impl AtomBios {
    /// The tables of video BIOS image `image`; `None` when it is not an ATOM BIOS.
    pub fn parse(image: &[u8]) -> Option<Self> {
        if image.get(..2)? != ROM_SIGNATURE {
            return None;
        }
        let header = usize::from(half(image, ROM_HEADER_POINTER)?);
        if image.get(header + 4..header + 8)? != ATOM_SIGNATURE {
            return None;
        }
        let master = usize::from(half(image, header + MASTER_DATA_TABLE)?);
        let mut bios = Self {
            paths: [None; MOST_PATHS],
            firmware_revision: None,
            boot_engine_khz: None,
        };
        if let Some(info) = table(image, master, FIRMWARE_INFO) {
            bios.firmware_revision = word(image, info + 4);
            bios.boot_engine_khz = word(image, info + 8).map(|clock| clock.saturating_mul(10));
        }
        if let Some(objects) = table(image, master, DISPLAY_OBJECT_INFO)
            && image.get(objects + 2) == Some(&1)
            && matches!(image.get(objects + 3), Some(4 | 5))
        {
            let count = usize::from(*image.get(objects + 6)?).min(MOST_PATHS);
            for (index, slot) in bios.paths.iter_mut().take(count).enumerate() {
                let at = objects + 8 + index * PATH_BYTES;
                let Some(connector) = half(image, at).and_then(Connector::of) else {
                    continue;
                };
                *slot = Some(DisplayPath {
                    connector,
                    encoder: half(image, at + 4)?,
                    device_tag: half(image, at + 12)?,
                });
            }
        }
        Some(bios)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(image: &mut [u8], at: usize, value: u16) {
        image[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn sample() -> Vec<u8> {
        let mut image = vec![0u8; 0x400];
        image[..2].copy_from_slice(&ROM_SIGNATURE);
        let header = 0x100;
        put16(&mut image, ROM_HEADER_POINTER, header as u16);
        image[header + 4..header + 8].copy_from_slice(ATOM_SIGNATURE);
        let master = 0x180;
        put16(&mut image, header + MASTER_DATA_TABLE, master as u16);
        let firmware = 0x200;
        let objects = 0x240;
        put16(&mut image, master + 4 + 2 * FIRMWARE_INFO, firmware as u16);
        put16(
            &mut image,
            master + 4 + 2 * DISPLAY_OBJECT_INFO,
            objects as u16,
        );
        image[firmware + 4..firmware + 8].copy_from_slice(&0x0011_0022u32.to_le_bytes());
        image[firmware + 8..firmware + 12].copy_from_slice(&80_000u32.to_le_bytes());
        image[objects + 2] = 1;
        image[objects + 3] = 4;
        image[objects + 6] = 2;
        let first = objects + 8;
        put16(&mut image, first, 0x3113);
        put16(&mut image, first + 4, 0x2121);
        put16(&mut image, first + 12, 0x0008);
        let second = first + PATH_BYTES;
        put16(&mut image, second, 0x310c);
        image
    }

    #[test]
    fn a_board_lists_its_connectors_and_boot_clock() {
        let bios = AtomBios::parse(&sample()).unwrap();
        let first = bios.paths[0].unwrap();
        assert_eq!(first.connector, Connector::DisplayPort);
        assert_eq!((first.encoder, first.device_tag), (0x2121, 0x0008));
        assert_eq!(bios.paths[1].unwrap().connector, Connector::Hdmi);
        assert_eq!(bios.paths[2], None);
        assert_eq!(bios.firmware_revision, Some(0x0011_0022));
        assert_eq!(bios.boot_engine_khz, Some(800_000));
    }

    #[test]
    fn an_image_that_is_not_atom_gives_nothing() {
        let mut image = sample();
        image[0x104] = b'X';
        assert_eq!(AtomBios::parse(&image), None);
        assert_eq!(AtomBios::parse(&[0x55]), None);
    }
}
