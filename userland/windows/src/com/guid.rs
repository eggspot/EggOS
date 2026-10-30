//! GUIDs (`CLSID`s and `IID`s): their 16-byte layout and their registry text,
//! `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, as `StringFromGUID2` writes it (upper-case hex) and
//! `CLSIDFromString` reads it (either case), and the interfaces the layer's objects answer to.

/// A GUID as Windows lays it out: a 32-bit, two 16-bit and eight 8-bit fields.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// How many units a GUID's text has, braces included, without a NUL.
pub const TEXT_LENGTH: usize = 38;

const AUTOMATION: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const HEX: &[u8; 16] = b"0123456789ABCDEF";
const DASH_BEFORE_BYTES: [usize; 4] = [4, 6, 8, 10];

/// `IID_NULL` (and `CLSID_NULL`), which `IDispatch`'s callers pass.
pub const NULL: Guid = Guid::new(0, 0, 0, [0; 8]);
/// `IID_IUnknown`.
pub const UNKNOWN: Guid = Guid::new(0, 0, 0, AUTOMATION);
/// `IID_IDispatch`.
pub const DISPATCH: Guid = Guid::new(0x0002_0400, 0, 0, AUTOMATION);
/// `IID_IEnumVARIANT`.
pub const ENUM_VARIANT: Guid = Guid::new(0x0002_0404, 0, 0, AUTOMATION);
/// `IID_IStream`.
pub const STREAM: Guid = Guid::new(0x0000_000c, 0, 0, AUTOMATION);
/// `IID_ISequentialStream`.
pub const SEQUENTIAL_STREAM: Guid = Guid::new(
    0x0c73_3a30,
    0x2a1c,
    0x11ce,
    [0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d],
);

fn hex_value(unit: u16) -> Option<u8> {
    let digit = u8::try_from(unit).ok()?;
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }

    fn text_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&self.data1.to_be_bytes());
        bytes[4..6].copy_from_slice(&self.data2.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.data3.to_be_bytes());
        bytes[8..].copy_from_slice(&self.data4);
        bytes
    }

    fn from_text_bytes(bytes: [u8; 16]) -> Self {
        let mut data4 = [0u8; 8];
        data4.copy_from_slice(&bytes[8..]);
        Self {
            data1: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            data2: u16::from_be_bytes([bytes[4], bytes[5]]),
            data3: u16::from_be_bytes([bytes[6], bytes[7]]),
            data4,
        }
    }

    /// The GUID's text, `{` and `}` included, in upper-case hex.
    pub fn text(&self) -> [u16; TEXT_LENGTH] {
        let mut text = [0u16; TEXT_LENGTH];
        text[0] = u16::from(b'{');
        text[TEXT_LENGTH - 1] = u16::from(b'}');
        let mut at = 1;
        for (index, byte) in self.text_bytes().into_iter().enumerate() {
            if DASH_BEFORE_BYTES.contains(&index) {
                text[at] = u16::from(b'-');
                at += 1;
            }
            text[at] = u16::from(HEX[usize::from(byte >> 4)]);
            text[at + 1] = u16::from(HEX[usize::from(byte & 0xf)]);
            at += 2;
        }
        text
    }

    /// The GUID `text` spells, braces required, hex in either case; `None` for anything else.
    pub fn parse(text: &[u16]) -> Option<Self> {
        if text.len() != TEXT_LENGTH
            || text[0] != u16::from(b'{')
            || text[TEXT_LENGTH - 1] != u16::from(b'}')
        {
            return None;
        }
        let mut bytes = [0u8; 16];
        let mut at = 1;
        for (index, byte) in bytes.iter_mut().enumerate() {
            if DASH_BEFORE_BYTES.contains(&index) {
                if text[at] != u16::from(b'-') {
                    return None;
                }
                at += 1;
            }
            *byte = (hex_value(text[at])? << 4) | hex_value(text[at + 1])?;
            at += 2;
        }
        Some(Self::from_text_bytes(bytes))
    }

    /// The GUID at `pointer`; `None` for null.
    ///
    /// # Safety
    /// `pointer` must be null or point at 16 readable bytes.
    pub unsafe fn read(pointer: *const Guid) -> Option<Self> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: the caller's duty; a program's GUID need not be aligned.
        Some(unsafe { pointer.read_unaligned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_guid_is_written_as_its_registry_text() {
        let dictionary = Guid::new(
            0xee09_b103,
            0x97e0,
            0x11cf,
            [0x97, 0x8f, 0x00, 0xa0, 0x24, 0x63, 0xe0, 0x6f],
        );
        assert_eq!(
            dictionary.text().to_vec(),
            wide("{EE09B103-97E0-11CF-978F-00A02463E06F}")
        );
        assert_eq!(
            DISPATCH.text().to_vec(),
            wide("{00020400-0000-0000-C000-000000000046}")
        );
        assert_eq!(
            SEQUENTIAL_STREAM.text().to_vec(),
            wide("{0C733A30-2A1C-11CE-ADE5-00AA0044773D}")
        );
    }

    #[test]
    fn registry_text_is_read_back_in_either_case() {
        assert_eq!(
            Guid::parse(&wide("{0000000c-0000-0000-c000-000000000046}")),
            Some(STREAM)
        );
        assert_eq!(
            Guid::parse(&wide("{00020404-0000-0000-C000-000000000046}")),
            Some(ENUM_VARIANT)
        );
        for guid in [NULL, UNKNOWN, DISPATCH, STREAM, SEQUENTIAL_STREAM] {
            assert_eq!(Guid::parse(&guid.text()), Some(guid));
        }
    }

    #[test]
    fn malformed_text_is_refused() {
        for text in [
            "00020400-0000-0000-C000-000000000046",
            "{00020400-0000-0000-C000-00000000004}",
            "{00020400-0000-0000-C000-0000000000466}",
            "{00020400_0000-0000-C000-000000000046}",
            "{0002040G-0000-0000-C000-000000000046}",
            "(00020400-0000-0000-C000-000000000046)",
            "",
        ] {
            assert_eq!(Guid::parse(&wide(text)), None, "{text}");
        }
    }

    #[test]
    fn the_layout_is_sixteen_bytes() {
        assert_eq!(core::mem::size_of::<Guid>(), 16);
        let bytes: [u8; 16] = [
            0x00, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0, 0, 0, 0, 0, 0, 0x46,
        ];
        // SAFETY: the array holds 16 bytes.
        let read = unsafe { Guid::read(bytes.as_ptr().cast()) };
        assert_eq!(read, Some(DISPATCH));
        // SAFETY: null is allowed.
        assert_eq!(unsafe { Guid::read(core::ptr::null()) }, None);
    }
}
