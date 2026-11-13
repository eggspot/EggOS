//! NVIDIA's video BIOS (D449), from the layouts NVIDIA publishes in `open-gpu-doc` (MIT): the
//! BIOS Information Table (BIT, found by its `\xff\xb8BIT` signature) whose `i` token gives
//! the BIOS version, and the Display Configuration Block (DCB 4.x, pointed at from byte 0x36)
//! whose entries list the board's display outputs. [`NvidiaBios::parse`] reads both.

/// The most outputs read.
pub const MOST_OUTPUTS: usize = 16;
const ROM_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const BIT_SIGNATURE: &[u8; 6] = b"\xff\xb8BIT\0";
const BIT_HEADER_BYTES: usize = 12;
const BIT_INFO_TOKEN: u8 = b'i';
const DCB_POINTER: usize = 0x36;
const DCB_SIGNATURE: u32 = 0x4edc_bdcb;
const DCB_VERSION_4: u8 = 0x40;
const DCB_END: u8 = 0xe;
const DCB_SKIP: u8 = 0xf;

/// What a DCB output is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Analog,
    Tmds,
    Lvds,
    DisplayPort,
    Other(u8),
}

impl Output {
    fn of(kind: u8) -> Self {
        match kind {
            0x0 => Self::Analog,
            0x2 => Self::Tmds,
            0x3 => Self::Lvds,
            0x6 => Self::DisplayPort,
            other => Self::Other(other),
        }
    }

    /// Its name, as `/proc/driver/display` prints it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Analog => "VGA",
            Self::Tmds => "DVI/HDMI",
            Self::Lvds => "LVDS",
            Self::DisplayPort => "DisplayPort",
            Self::Other(_) => "other",
        }
    }
}

/// What the tables say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvidiaBios {
    /// The BIOS version, as its `i` token gives it.
    pub version: Option<u32>,
    pub outputs: [Option<Output>; MOST_OUTPUTS],
}

fn half(image: &[u8], at: usize) -> Option<u16> {
    let field = image.get(at..at + 2)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
}

fn word(image: &[u8], at: usize) -> Option<u32> {
    let field = image.get(at..at + 4)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn bit_version(image: &[u8]) -> Option<u32> {
    let at = image
        .windows(BIT_SIGNATURE.len())
        .position(|window| window == BIT_SIGNATURE)?;
    let header_size = usize::from(*image.get(at + 8)?);
    let token_size = usize::from(*image.get(at + 9)?);
    let tokens = usize::from(*image.get(at + 10)?);
    let first = at + header_size.max(BIT_HEADER_BYTES);
    (0..tokens).find_map(|index| {
        let token = first + index * token_size;
        (*image.get(token)? == BIT_INFO_TOKEN)
            .then(|| half(image, token + 4).and_then(|data| word(image, usize::from(data))))
            .flatten()
    })
}

impl NvidiaBios {
    /// The tables of video BIOS image `image`; `None` when it is not a video BIOS.
    pub fn parse(image: &[u8]) -> Option<Self> {
        if image.get(..2)? != ROM_SIGNATURE {
            return None;
        }
        let mut bios = Self {
            version: bit_version(image),
            outputs: [None; MOST_OUTPUTS],
        };
        let dcb = usize::from(half(image, DCB_POINTER)?);
        if dcb != 0
            && image
                .get(dcb)
                .is_some_and(|version| *version >= DCB_VERSION_4)
            && word(image, dcb + 6) == Some(DCB_SIGNATURE)
        {
            let header = usize::from(*image.get(dcb + 1)?);
            let count = usize::from(*image.get(dcb + 2)?);
            let size = usize::from(*image.get(dcb + 3)?).max(4);
            let mut slots = bios.outputs.iter_mut();
            for index in 0..count {
                let entry = word(image, dcb + header + index * size)?;
                let kind = (entry & 0xf) as u8;
                match kind {
                    DCB_END => break,
                    DCB_SKIP => continue,
                    kind => match slots.next() {
                        Some(slot) => *slot = Some(Output::of(kind)),
                        None => break,
                    },
                }
            }
        }
        Some(bios)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut image = vec![0u8; 0x400];
        image[..2].copy_from_slice(&ROM_SIGNATURE);
        let bit = 0x80;
        image[bit..bit + 6].copy_from_slice(BIT_SIGNATURE);
        image[bit + 8] = 12;
        image[bit + 9] = 6;
        image[bit + 10] = 2;
        let tokens = bit + 12;
        image[tokens] = b'C';
        image[tokens + 6] = BIT_INFO_TOKEN;
        image[tokens + 6 + 4..tokens + 6 + 6].copy_from_slice(&0x200u16.to_le_bytes());
        image[0x200..0x204].copy_from_slice(&0x8604_2100u32.to_le_bytes());
        let dcb = 0x300;
        image[DCB_POINTER..DCB_POINTER + 2].copy_from_slice(&(dcb as u16).to_le_bytes());
        image[dcb] = DCB_VERSION_4;
        image[dcb + 1] = 0x17;
        image[dcb + 2] = 4;
        image[dcb + 3] = 8;
        image[dcb + 6..dcb + 10].copy_from_slice(&DCB_SIGNATURE.to_le_bytes());
        let entries = dcb + 0x17;
        for (index, kind) in [0x6u32, 0xf, 0x2, 0xe].into_iter().enumerate() {
            image[entries + 8 * index..entries + 8 * index + 4]
                .copy_from_slice(&kind.to_le_bytes());
        }
        image
    }

    #[test]
    fn a_board_lists_its_outputs_and_version() {
        let bios = NvidiaBios::parse(&sample()).unwrap();
        assert_eq!(bios.version, Some(0x8604_2100));
        assert_eq!(bios.outputs[0], Some(Output::DisplayPort));
        assert_eq!(bios.outputs[1], Some(Output::Tmds));
        assert_eq!(bios.outputs[2], None);
    }

    #[test]
    fn an_image_that_is_not_a_bios_gives_nothing() {
        assert_eq!(NvidiaBios::parse(&[0; 4]), None);
    }
}
