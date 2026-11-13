//! A display's EDID (VESA E-EDID 1.4, D445): the 128-byte base block a monitor or panel
//! describes itself with: its maker, its name and its detailed timings, the first of which is
//! its preferred mode. [`Edid::parse`] checks the header and checksum and reads those.
//! [`Timing::reduced_blanking`] makes a timing for any size the way VESA's CVT 1.2 reduced
//! blanking version 2 does (D521), for a display asked for a size its EDID does not list.

/// The bytes of the base block.
pub const BLOCK_BYTES: usize = 128;
const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
const DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];
const DESCRIPTOR_BYTES: usize = 18;
const NAME_TAG: u8 = 0xfc;
const NAME_BYTES: usize = 13;
const REDUCED_HORIZONTAL_BLANK: u32 = 80;
const REDUCED_HORIZONTAL_SYNC: u32 = 32;
const REDUCED_HORIZONTAL_FRONT_PORCH: u32 = 8;
const REDUCED_VERTICAL_SYNC: u32 = 8;
const REDUCED_VERTICAL_BACK_PORCH: u32 = 6;
const REDUCED_VERTICAL_FRONT_PORCH: u32 = 1;
const REDUCED_LEAST_VERTICAL_BLANK_NS: u64 = 460_000;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// One detailed timing: the picture's size and its blanking and sync, in pixels and lines, and
/// the pixel clock in kilohertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub clock_khz: u32,
    pub width: u32,
    pub height: u32,
    pub horizontal_blank: u32,
    pub vertical_blank: u32,
    pub horizontal_sync_offset: u32,
    pub horizontal_sync_width: u32,
    pub vertical_sync_offset: u32,
    pub vertical_sync_width: u32,
    pub horizontal_sync_positive: bool,
    pub vertical_sync_positive: bool,
}

impl Timing {
    /// Its refresh rate in millihertz.
    pub fn refresh_mhz(&self) -> u32 {
        let total = u64::from(self.width + self.horizontal_blank)
            * u64::from(self.height + self.vertical_blank);
        match total {
            0 => 0,
            total => (u64::from(self.clock_khz) * 1_000_000 / total) as u32,
        }
    }

    /// The CVT reduced blanking version 2 timing of `width` by `height` at `refresh_hz`: 80
    /// blank pixels a line (front porch 8, sync 32, positive), at least 460 microseconds of
    /// blank lines a frame (sync 8, back porch 6, negative), the pixel clock rounded down to a
    /// kilohertz. `None` for an empty size, no refresh, or a frame shorter than the blank.
    pub fn reduced_blanking(width: u32, height: u32, refresh_hz: u32) -> Option<Self> {
        if width == 0 || height == 0 || refresh_hz == 0 {
            return None;
        }
        let frame_ns = NANOSECONDS_PER_SECOND / u64::from(refresh_hz);
        let picture_ns = frame_ns
            .checked_sub(REDUCED_LEAST_VERTICAL_BLANK_NS)
            .filter(|&ns| ns > 0)?;
        let blank_lines = REDUCED_LEAST_VERTICAL_BLANK_NS * u64::from(height) / picture_ns + 1;
        let least_blank =
            REDUCED_VERTICAL_FRONT_PORCH + REDUCED_VERTICAL_SYNC + REDUCED_VERTICAL_BACK_PORCH;
        let vertical_blank = u32::try_from(blank_lines).ok()?.max(least_blank);
        let line = u64::from(width + REDUCED_HORIZONTAL_BLANK);
        let frame = u64::from(height + vertical_blank);
        let clock_khz = u32::try_from(u64::from(refresh_hz) * frame * line / 1000).ok()?;
        Some(Self {
            clock_khz,
            width,
            height,
            horizontal_blank: REDUCED_HORIZONTAL_BLANK,
            vertical_blank,
            horizontal_sync_offset: REDUCED_HORIZONTAL_FRONT_PORCH,
            horizontal_sync_width: REDUCED_HORIZONTAL_SYNC,
            vertical_sync_offset: vertical_blank
                - REDUCED_VERTICAL_SYNC
                - REDUCED_VERTICAL_BACK_PORCH,
            vertical_sync_width: REDUCED_VERTICAL_SYNC,
            horizontal_sync_positive: true,
            vertical_sync_positive: false,
        })
    }
}

/// What a base block says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edid {
    /// The maker's three-letter PNP id.
    pub maker: [u8; 3],
    pub product: u16,
    /// The monitor's name, when a descriptor gives one, and its length.
    pub name: [u8; NAME_BYTES],
    pub name_length: usize,
    /// The preferred mode: the first detailed timing.
    pub preferred: Option<Timing>,
}

fn timing(bytes: &[u8]) -> Option<Timing> {
    let clock = u32::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    if clock == 0 {
        return None;
    }
    let low_high = |low: u8, high: u8, shift: u32| u32::from(low) | u32::from(high) << shift;
    Some(Timing {
        clock_khz: clock * 10,
        width: low_high(bytes[2], bytes[4] >> 4, 8),
        horizontal_blank: low_high(bytes[3], bytes[4] & 0x0f, 8),
        height: low_high(bytes[5], bytes[7] >> 4, 8),
        vertical_blank: low_high(bytes[6], bytes[7] & 0x0f, 8),
        horizontal_sync_offset: low_high(bytes[8], (bytes[11] >> 6) & 0x3, 8),
        horizontal_sync_width: low_high(bytes[9], (bytes[11] >> 4) & 0x3, 8),
        vertical_sync_offset: u32::from(bytes[10] >> 4) | u32::from((bytes[11] >> 2) & 0x3) << 4,
        vertical_sync_width: u32::from(bytes[10] & 0x0f) | u32::from(bytes[11] & 0x3) << 4,
        horizontal_sync_positive: bytes[17] & 0x02 != 0,
        vertical_sync_positive: bytes[17] & 0x04 != 0,
    })
}

impl Edid {
    /// The base block `block`, if its header and checksum are right.
    pub fn parse(block: &[u8]) -> Option<Self> {
        let block = block.get(..BLOCK_BYTES)?;
        if block[..8] != HEADER || block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
        {
            return None;
        }
        let packed = u16::from_be_bytes([block[8], block[9]]);
        let letter = |shift: u16| b'@' + ((packed >> shift) & 0x1f) as u8;
        let mut edid = Self {
            maker: [letter(10), letter(5), letter(0)],
            product: u16::from_le_bytes([block[10], block[11]]),
            name: [0; NAME_BYTES],
            name_length: 0,
            preferred: None,
        };
        for at in DESCRIPTORS {
            let descriptor = &block[at..at + DESCRIPTOR_BYTES];
            if descriptor[0] != 0 || descriptor[1] != 0 {
                if edid.preferred.is_none() {
                    edid.preferred = timing(descriptor);
                }
                continue;
            }
            if descriptor[3] == NAME_TAG {
                let text = &descriptor[5..5 + NAME_BYTES];
                let length = text
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .unwrap_or(NAME_BYTES);
                edid.name[..length].copy_from_slice(&text[..length]);
                edid.name_length = length;
            }
        }
        Some(edid)
    }

    /// The monitor's name, or empty.
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_length])
            .unwrap_or("")
            .trim_end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> [u8; BLOCK_BYTES] {
        let mut block = [0u8; BLOCK_BYTES];
        block[..8].copy_from_slice(&HEADER);
        block[8..10].copy_from_slice(&0x10acu16.to_be_bytes());
        block[10..12].copy_from_slice(&0x4142u16.to_le_bytes());
        let timing = [
            0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0, 0, 0, 0, 0,
            0x1e,
        ];
        block[54..72].copy_from_slice(&timing);
        block[72..77].copy_from_slice(&[0, 0, 0, NAME_TAG, 0]);
        block[77..90].copy_from_slice(b"DELL U2415\n  ");
        let sum = block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        block[127] = 0u8.wrapping_sub(sum);
        block
    }

    #[test]
    fn a_monitor_says_its_maker_name_and_preferred_mode() {
        let edid = Edid::parse(&sample()).unwrap();
        assert_eq!(&edid.maker, b"DEL");
        assert_eq!(edid.name(), "DELL U2415");
        let preferred = edid.preferred.unwrap();
        assert_eq!((preferred.width, preferred.height), (1920, 1080));
        assert_eq!(preferred.clock_khz, 148_500);
        assert_eq!(preferred.horizontal_blank, 280);
        assert_eq!(preferred.vertical_blank, 45);
        assert_eq!(preferred.refresh_mhz(), 60_000);
        assert!(preferred.horizontal_sync_positive && preferred.vertical_sync_positive);
    }

    #[test]
    fn reduced_blanking_matches_cvt_for_1080p_at_60() {
        let timing = Timing::reduced_blanking(1920, 1080, 60).unwrap();
        assert_eq!(timing.clock_khz, 133_320);
        assert_eq!((timing.horizontal_blank, timing.vertical_blank), (80, 31));
        assert_eq!(
            (timing.vertical_sync_offset, timing.vertical_sync_width),
            (17, 8)
        );
        assert!(timing.horizontal_sync_positive && !timing.vertical_sync_positive);
        assert_eq!(
            Timing::reduced_blanking(640, 2, 60).unwrap().vertical_blank,
            15
        );
        assert_eq!(Timing::reduced_blanking(0, 1080, 60), None);
        assert_eq!(Timing::reduced_blanking(1920, 1080, 5000), None);
    }

    #[test]
    fn a_bad_header_or_checksum_gives_nothing() {
        let mut block = sample();
        block[127] = block[127].wrapping_add(1);
        assert_eq!(Edid::parse(&block), None);
        assert_eq!(Edid::parse(&[0; 64]), None);
    }
}
