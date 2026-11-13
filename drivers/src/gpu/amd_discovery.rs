//! AMD's IP discovery table and the display microcontroller's firmware file (D468). Since
//! Navi, an AMD GPU says which blocks it has, their versions and their register bases in a
//! binary at the top of its memory, as AMD's published `discovery.h` lays it out: a binary
//! header listing tables, the IP discovery table listing dies, and each die listing its IP
//! blocks. The display's block is DMU (DCN); its microcontroller, DMCUB, runs firmware from
//! a file whose common header and DMCUB header say where its instructions and data are.

/// The binary's signature.
pub const BINARY_SIGNATURE: u32 = 0x2821_1407;
/// The IP discovery table's signature, `IPDS`.
pub const IP_DISCOVERY_SIGNATURE: u32 = 0x5344_5049;
/// How far below the top of video memory the binary sits.
pub const OFFSET_FROM_TOP: u64 = 64 << 10;
/// The display block's hardware id.
pub const DISPLAY_BLOCK: u16 = 271;
/// The most register bases kept for a block.
pub const MOST_BASES: usize = 8;
const TABLE_LIST: usize = 12;
const IP_DISCOVERY_TABLE: usize = 0;
const DIE_COUNT: usize = 12;
const DIE_LIST: usize = 14;
const MOST_DIES: usize = 16;
const BASE_IS_64_BIT: usize = DIE_LIST + 4 * MOST_DIES;
const IP_HEADER_BYTES: usize = 8;
const PSP_HEADER_BYTES: u32 = 0x100;
const PSP_FOOTER_BYTES: u32 = 0x100;
const COMMON_HEADER_BYTES: usize = 32;

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// One IP block a die has: its hardware id, instance, version and register bases (in dwords).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IpBlock {
    pub hardware_id: u16,
    pub instance: u8,
    pub version: (u8, u8, u8),
    pub bases: [u64; MOST_BASES],
    pub base_count: usize,
}

/// Calls `found` with every IP block the discovery binary `binary` lists; `false` when it is
/// not one.
pub fn blocks(binary: &[u8], found: &mut dyn FnMut(IpBlock)) -> bool {
    walk(binary, found).is_some()
}

fn walk(binary: &[u8], found: &mut dyn FnMut(IpBlock)) -> Option<()> {
    if word(binary, 0)? != BINARY_SIGNATURE {
        return None;
    }
    let table = usize::from(half(binary, TABLE_LIST + 8 * IP_DISCOVERY_TABLE)?);
    if word(binary, table)? != IP_DISCOVERY_SIGNATURE {
        return None;
    }
    let dies = usize::from(half(binary, table + DIE_COUNT)?).min(MOST_DIES);
    let wide = binary
        .get(table + BASE_IS_64_BIT)
        .is_some_and(|flags| flags & 1 != 0);
    for die in 0..dies {
        let mut at = usize::from(half(binary, table + DIE_LIST + 4 * die + 2)?);
        let ips = half(binary, at + 2)?;
        at += 4;
        for _ in 0..ips {
            let count = usize::from(*binary.get(at + 3)?);
            let mut block = IpBlock {
                hardware_id: half(binary, at)?,
                instance: *binary.get(at + 2)?,
                version: (
                    *binary.get(at + 4)?,
                    *binary.get(at + 5)?,
                    *binary.get(at + 6)?,
                ),
                base_count: count.min(MOST_BASES),
                ..IpBlock::default()
            };
            let size = if wide { 8 } else { 4 };
            for index in 0..block.base_count {
                let place = at + IP_HEADER_BYTES + size * index;
                let low = u64::from(word(binary, place)?);
                block.bases[index] = match wide {
                    true => low | u64::from(word(binary, place + 4)?) << 32,
                    false => low,
                };
            }
            found(block);
            at += IP_HEADER_BYTES + size * count;
        }
    }
    Some(())
}

/// The display block a discovery binary lists, if any.
pub fn display_block(binary: &[u8]) -> Option<IpBlock> {
    let mut display = None;
    blocks(binary, &mut |block| {
        if block.hardware_id == DISPLAY_BLOCK && display.is_none() {
            display = Some(block);
        }
    });
    display
}

/// The DMCUB firmware file's name for display block version `version`, as the component
/// store mirrors it: `dcn_3_0_1_dmcub.bin`.
pub fn dmcub_file_name(version: (u8, u8, u8)) -> alloc::string::String {
    alloc::format!("dcn_{}_{}_{}_dmcub.bin", version.0, version.1, version.2)
}

/// What a DMCUB firmware file holds: the display IP version it is for, its version, and
/// where its instructions and constants (without the security processor's header and footer)
/// and its data are in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmcubFirmware {
    pub ip_version: (u16, u16),
    pub version: u32,
    pub instructions: (usize, usize),
    pub data: (usize, usize),
}

/// The DMCUB firmware file `file` describes; `None` when its headers do not fit it.
pub fn dmcub_firmware(file: &[u8]) -> Option<DmcubFirmware> {
    let size = word(file, 0)? as usize;
    if size > file.len() || size < COMMON_HEADER_BYTES + 8 {
        return None;
    }
    let ip_version = (half(file, 12)?, half(file, 14)?);
    let version = word(file, 16)?;
    let array = word(file, 24)? as usize;
    let instruction_bytes = word(file, COMMON_HEADER_BYTES)?;
    let data_bytes = word(file, COMMON_HEADER_BYTES + 4)? as usize;
    let inner = instruction_bytes.checked_sub(PSP_HEADER_BYTES + PSP_FOOTER_BYTES)? as usize;
    let instructions = (array + PSP_HEADER_BYTES as usize, inner);
    let data = (array + instruction_bytes as usize, data_bytes);
    let fits =
        |(start, length): (usize, usize)| start.checked_add(length).is_some_and(|end| end <= size);
    (fits(instructions) && fits(data)).then_some(DmcubFirmware {
        ip_version,
        version,
        instructions,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(bytes: &mut Vec<u8>, at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(bytes: &mut Vec<u8>, at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn binary() -> Vec<u8> {
        let mut bytes = vec![0u8; 512];
        put32(&mut bytes, 0, BINARY_SIGNATURE);
        put16(&mut bytes, TABLE_LIST, 0x40);
        put32(&mut bytes, 0x40, IP_DISCOVERY_SIGNATURE);
        put16(&mut bytes, 0x40 + DIE_COUNT, 1);
        put16(&mut bytes, 0x40 + DIE_LIST + 2, 0x100);
        put16(&mut bytes, 0x102, 2);
        put16(&mut bytes, 0x104, 11);
        bytes[0x107] = 1;
        bytes[0x108..0x10b].copy_from_slice(&[10, 3, 0]);
        put32(&mut bytes, 0x10c, 0x1260);
        put16(&mut bytes, 0x110, DISPLAY_BLOCK);
        bytes[0x113] = 2;
        bytes[0x114..0x117].copy_from_slice(&[3, 0, 1]);
        put32(&mut bytes, 0x118, 0x12);
        put32(&mut bytes, 0x11c, 0xc0);
        bytes
    }

    #[test]
    fn the_display_block_is_found_with_its_version_and_bases() {
        let display = display_block(&binary()).unwrap();
        assert_eq!(display.version, (3, 0, 1));
        assert_eq!(display.bases[..display.base_count], [0x12, 0xc0]);
        let mut all = Vec::new();
        assert!(blocks(&binary(), &mut |block| all.push(block.hardware_id)));
        assert_eq!(all, vec![11, DISPLAY_BLOCK]);
        assert!(!blocks(&[0u8; 64], &mut |_| {}));
    }

    #[test]
    fn a_dmcub_file_s_instructions_skip_the_security_header_and_footer() {
        let mut file = vec![0u8; 0x1000];
        put32(&mut file, 0, 0x1000);
        put16(&mut file, 12, 3);
        put16(&mut file, 14, 1);
        put32(&mut file, 16, 0x0400_0300);
        put32(&mut file, 24, 0x100);
        put32(&mut file, 32, 0x600);
        put32(&mut file, 36, 0x80);
        let firmware = dmcub_firmware(&file).unwrap();
        assert_eq!(firmware.ip_version, (3, 1));
        assert_eq!(firmware.instructions, (0x200, 0x400));
        assert_eq!(firmware.data, (0x700, 0x80));
        put32(&mut file, 36, 0x1000);
        assert_eq!(dmcub_firmware(&file), None);
    }
}
