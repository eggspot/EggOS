//! Intel's display microcontroller (DMC) firmware files (D444): a CSS header, a package header
//! listing one firmware per stepping, and each firmware's own header, program and register
//! writes. [`parse`] picks the firmware for a stepping (or the one for any) and hands back
//! its program, the address it loads at, and the writes that start it; the file's layout is
//! Intel's published one, read field by field, little-endian.

/// The most register writes a firmware header lists.
pub const MOST_WRITES: usize = 20;
/// Where a version 1 firmware's program loads (the DMC's program memory).
pub const PROGRAM_BASE_V1: u32 = 0x8_0000;
const CSS_HEADER_BYTES: usize = 128;
const CSS_HEADER_LENGTH: usize = 4;
const PACKAGE_ENTRY_BYTES: usize = 12;
const PACKAGE_ENTRIES_AT: usize = 16;
const PACKAGE_COUNT_AT: usize = 12;
const ANY_STEPPING: u8 = b'*';
const V1_WRITES: usize = 8;
const V1_HEADER_BYTES: usize = 128;
const V3_HEADER_BYTES: usize = 256;
const DMC_SIGNATURE: u32 = 0x4040_3E3E;

/// The firmware chosen from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dmc<'a> {
    /// The program, a whole number of 32-bit words.
    pub program: &'a [u8],
    /// The register its first word loads at; the next words follow, 4 bytes apart.
    pub program_base: u32,
    /// The register writes that start it, `count` of them.
    pub writes: [(u32, u32); MOST_WRITES],
    pub count: usize,
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

/// The firmware in `file` for `stepping` and `substepping` (`b'A'`, `b'0'` and so on), or the
/// one for any stepping; `None` when the file is not one or names none that fits.
pub fn parse(file: &[u8], stepping: u8, substepping: u8) -> Option<Dmc<'_>> {
    let css_length = (word(file, CSS_HEADER_LENGTH)? as usize).checked_mul(4)?;
    let package_at = if css_length == CSS_HEADER_BYTES {
        css_length
    } else {
        CSS_HEADER_BYTES
    };
    let package = file.get(package_at..)?;
    let package_length = usize::from(*package.first()?) * 4;
    let count = word(package, PACKAGE_COUNT_AT)? as usize;
    let entries = package
        .get(PACKAGE_ENTRIES_AT..PACKAGE_ENTRIES_AT + count.checked_mul(PACKAGE_ENTRY_BYTES)?)?;
    let chosen = entries
        .as_chunks::<PACKAGE_ENTRY_BYTES>()
        .0
        .iter()
        .find(|entry| entry[2] == stepping && entry[3] == substepping)
        .or_else(|| {
            entries
                .as_chunks::<PACKAGE_ENTRY_BYTES>()
                .0
                .iter()
                .find(|entry| entry[2] == ANY_STEPPING)
        })?;
    let offset = (word(chosen, 4)? as usize).checked_mul(4)?;
    let header = package.get(package_length.checked_add(offset)?..)?;
    if word(header, 0)? != DMC_SIGNATURE {
        return None;
    }
    let version = *header.get(5)?;
    let program_words = word(header, 12)? as usize;
    let writes_count = word(header, 20)? as usize;
    let (header_bytes, base, addresses_at, data_at, most) = match version {
        1 => (
            V1_HEADER_BYTES,
            PROGRAM_BASE_V1,
            24,
            24 + 4 * V1_WRITES,
            V1_WRITES,
        ),
        _ => (
            V3_HEADER_BYTES,
            word(header, 24)?,
            24 + 4 + 36 + 32,
            24 + 4 + 36 + 32 + 4 * MOST_WRITES,
            MOST_WRITES,
        ),
    };
    if writes_count > most {
        return None;
    }
    let mut writes = [(0, 0); MOST_WRITES];
    for (index, write) in writes.iter_mut().take(writes_count).enumerate() {
        *write = (
            word(header, addresses_at + 4 * index)?,
            word(header, data_at + 4 * index)?,
        );
    }
    let program =
        header.get(header_bytes..header_bytes.checked_add(program_words.checked_mul(4)?)?)?;
    Some(Dmc {
        program,
        program_base: base,
        writes,
        count: writes_count,
    })
}

/// The firmware file for an Intel display device, in the `display-firmware` component's
/// folder (`/usr/lib/firmware/i915`); `None` for a device whose display needs none.
pub fn file_for(device: u16) -> Option<&'static str> {
    match device >> 8 {
        0x9a => Some("tgl_dmc_ver2_12.bin"),
        0x46 if device & 0x00f0 == 0x00a0 => Some("adlp_dmc.bin"),
        0x46 => Some("adls_dmc_ver2_01.bin"),
        0xa7 => Some("adlp_dmc.bin"),
        0x7d => Some("mtl_dmc.bin"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(file: &mut [u8], at: usize, value: u32) {
        file[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn sample() -> Vec<u8> {
        let mut file = vec![0u8; CSS_HEADER_BYTES + 64 + V1_HEADER_BYTES + 8];
        put(&mut file, CSS_HEADER_LENGTH, (CSS_HEADER_BYTES / 4) as u32);
        let package = CSS_HEADER_BYTES;
        file[package] = 64 / 4;
        put(&mut file, package + PACKAGE_COUNT_AT, 2);
        let first = package + PACKAGE_ENTRIES_AT;
        file[first + 2] = b'B';
        file[first + 3] = b'0';
        put(&mut file, first + 4, 0x40);
        file[first + PACKAGE_ENTRY_BYTES + 2] = ANY_STEPPING;
        put(&mut file, first + PACKAGE_ENTRY_BYTES + 4, 0);
        let header = package + 64;
        put(&mut file, header, DMC_SIGNATURE);
        file[header + 5] = 1;
        put(&mut file, header + 12, 2);
        put(&mut file, header + 20, 1);
        put(&mut file, header + 24, 0x8f074);
        put(&mut file, header + 24 + 32, 0x5a5a);
        put(&mut file, header + V1_HEADER_BYTES, 0x1111_1111);
        put(&mut file, header + V1_HEADER_BYTES + 4, 0x2222_2222);
        file
    }

    #[test]
    fn the_firmware_for_any_stepping_is_chosen_when_none_matches() {
        let file = sample();
        let dmc = parse(&file, b'A', b'0').unwrap();
        assert_eq!(dmc.program_base, PROGRAM_BASE_V1);
        assert_eq!(dmc.program.len(), 8);
        assert_eq!(&dmc.program[..4], &0x1111_1111u32.to_le_bytes());
        assert_eq!((dmc.count, dmc.writes[0]), (1, (0x8f074, 0x5a5a)));
    }

    #[test]
    fn a_file_that_is_not_firmware_gives_nothing() {
        assert_eq!(parse(&[0; 16], b'A', b'0'), None);
        let mut file = sample();
        let header = CSS_HEADER_BYTES + 64;
        put(&mut file, header, 0);
        assert_eq!(parse(&file, b'A', b'0'), None);
    }

    #[test]
    fn devices_name_their_files() {
        assert_eq!(file_for(0x9a49), Some("tgl_dmc_ver2_12.bin"));
        assert_eq!(file_for(0x46a6), Some("adlp_dmc.bin"));
        assert_eq!(file_for(0x4680), Some("adls_dmc_ver2_01.bin"));
        assert_eq!(file_for(0x3e92), None);
    }
}
