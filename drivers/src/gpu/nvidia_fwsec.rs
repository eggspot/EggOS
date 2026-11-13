//! NVIDIA's FWSEC, part 2 of GSP's boot (D512): the firmware in the video BIOS that, run in
//! heavy-secure mode on GSP's Falcon (D497), carves the protected region (WPR2) that the
//! booter later loads GSP's image into. It is found through the BIT's Falcon data token
//! (`0x70`, version 2), whose table lists ucodes by application (FWSEC is `0x85`, its debug
//! build `0x45`, the older firmware-security licence `0x05`), each with a descriptor; version 3
//! descriptors (Ampere on) carry the ucode's load sizes and places, its interface offset and
//! RSA-3K signatures, one per signature version. Pointers in that table count from the
//! extended image of the ROM, after the base image (the PCI expansion ROM images are walked
//! for it). [`prepare_frts`] then patches the ucode's DMEM for the FRTS command: the DMEM
//! mapper the interface table names gets the command and its argument buffer (the region's
//! place in 4 KiB units and its 1 MiB size), and the signature that matches the fuse's ucode
//! version goes where the descriptor says. Layouts are from NVIDIA's open GPU kernel modules
//! (`kern_vbios.c`, `kern_vbios_tu102.c`, `pci_exp_table.h`, MIT, `NVIDIA-NOTICE.md`).

use alloc::vec::Vec;

const ROM_SIGNATURE: u16 = 0xaa55;
const PCI_DATA_POINTER: usize = 0x18;
const PCI_DATA_SIGNATURES: [u32; 3] = [0x5249_4350, 0x5344_504e, 0x5349_4752];
const PCI_DATA_LENGTH: usize = 0x0a;
const PCI_DATA_IMAGE_LENGTH: usize = 0x10;
const PCI_DATA_CODE_TYPE: usize = 0x14;
const PCI_DATA_LAST_IMAGE: usize = 0x15;
const LAST_IMAGE: u8 = 1 << 7;
const EXTENSION_SIGNATURE: u32 = 0x4544_504e;
const EXTENSION_REVISIONS: [u16; 2] = [0x100, 0x101];
const EXTENSION_REVISION: usize = 4;
const EXTENSION_LENGTH: usize = 6;
const EXTENSION_SUBIMAGE_LENGTH: usize = 8;
const EXTENSION_LAST_IMAGE: usize = 0x0a;
const BLOCK_BYTES: usize = 512;
const CODE_TYPE_BASE: u8 = 0x00;
const CODE_TYPE_EXTENDED: u8 = 0xe0;
const MOST_IMAGES: usize = 16;

const BIT_ID: u16 = 0xb8ff;
const BIT_SIGNATURE: u32 = 0x0054_4942;
const BIT_HEADER_SIZE: usize = 8;
const BIT_TOKEN_SIZE: usize = 9;
const BIT_TOKEN_COUNT: usize = 10;
const FALCON_DATA_TOKEN: u8 = 0x70;
const FALCON_DATA_VERSION: u8 = 2;

const APPLICATION_FIRMWARE_SECURITY: u8 = 0x05;
const APPLICATION_FWSEC_DEBUG: u8 = 0x45;
const APPLICATION_FWSEC: u8 = 0x85;
const DESCRIPTOR_VERSION_3: u32 = 3;
const DESCRIPTOR_V3_BYTES: usize = 44;
/// The bytes of one RSA-3K signature.
pub const SIGNATURE_BYTES: usize = 384;

const INTERFACE_HEADER_BYTES: usize = 4;
const INTERFACE_ENTRY_BYTES: usize = 8;
const DMEM_MAPPER_ID: u32 = 4;
const MAPPER_COMMAND_BUFFER: usize = 8;
const MAPPER_INIT_COMMAND: usize = 44;
const MAPPER_BYTES: usize = 64;
/// FWSEC's command that sets up the FRTS region.
pub const COMMAND_FRTS: u32 = 0x15;
const READ_VBIOS_FLAGS: u32 = 2;
const MEDIA_FRAMEBUFFER: u32 = 2;
/// The FRTS region's size in 4 KiB units: 1 MiB.
pub const FRTS_SIZE_IN_4K: u32 = 0x100;
const FRTS_COMMAND_BYTES: usize = 48;

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Where the extended image's pointers count from in the ROM image `rom`: the extended image's
/// place less the base image's size, or 0 without either.
pub fn expansion_rom_offset(rom: &[u8]) -> Option<usize> {
    if half(rom, 0)? != ROM_SIGNATURE {
        return None;
    }
    let mut block = 0usize;
    let (mut extended, mut base_size) = (0usize, 0usize);
    for _ in 0..MOST_IMAGES {
        let data = block + usize::from(half(rom, block + PCI_DATA_POINTER)?);
        if !PCI_DATA_SIGNATURES.contains(&word(rom, data)?) {
            return None;
        }
        let mut last = rom.get(data + PCI_DATA_LAST_IMAGE)? & LAST_IMAGE != 0;
        let image_length = usize::from(half(rom, data + PCI_DATA_IMAGE_LENGTH)?);
        let mut length = image_length;
        let extension = (data + usize::from(half(rom, data + PCI_DATA_LENGTH)?) + 0xf) & !0xf;
        if word(rom, extension) == Some(EXTENSION_SIGNATURE)
            && EXTENSION_REVISIONS.contains(&half(rom, extension + EXTENSION_REVISION)?)
        {
            let extension_length = usize::from(half(rom, extension + EXTENSION_LENGTH)?);
            length = usize::from(half(rom, extension + EXTENSION_SUBIMAGE_LENGTH)?);
            if EXTENSION_LAST_IMAGE < extension_length {
                last = rom.get(extension + EXTENSION_LAST_IMAGE)? & LAST_IMAGE != 0;
            } else if length < image_length {
                last = false;
            }
        }
        match *rom.get(data + PCI_DATA_CODE_TYPE)? {
            CODE_TYPE_EXTENDED if extended == 0 => extended = block,
            CODE_TYPE_BASE if base_size == 0 => base_size = length * BLOCK_BYTES,
            _ => {}
        }
        if last || length == 0 {
            break;
        }
        block += length * BLOCK_BYTES;
    }
    Some(match extended > 0 && base_size > 0 {
        true => extended - base_size,
        false => 0,
    })
}

fn bit_header(rom: &[u8]) -> Option<usize> {
    (0..rom.len().saturating_sub(BIT_HEADER_SIZE))
        .find(|&at| half(rom, at) == Some(BIT_ID) && word(rom, at + 2) == Some(BIT_SIGNATURE))
}

/// FWSEC as the video BIOS carries it, with a version 3 descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fwsec<'a> {
    /// The ucode: its IMEM bytes, then its DMEM bytes.
    pub ucode: &'a [u8],
    pub imem_physical: u32,
    pub imem_bytes: u32,
    pub imem_virtual: u32,
    pub dmem_physical: u32,
    pub dmem_bytes: u32,
    /// Where in DMEM the signature goes.
    pub signature_place: u32,
    /// Where in DMEM the interface table is.
    pub interface: u32,
    pub engine_mask: u16,
    pub ucode_id: u8,
    /// Which signature versions the signatures are for, a bit each, lowest first.
    pub signature_versions: u16,
    pub signatures: &'a [u8],
}

/// The production FWSEC (or the debug one) of ROM image `rom`, read whole (from `PROM`).
pub fn find(rom: &[u8], debug: bool) -> Option<Fwsec<'_>> {
    let base = expansion_rom_offset(rom)?;
    let bit = bit_header(rom)?;
    let header_size = usize::from(*rom.get(bit + BIT_HEADER_SIZE)?);
    let token_size = usize::from(*rom.get(bit + BIT_TOKEN_SIZE)?);
    let tokens = usize::from(*rom.get(bit + BIT_TOKEN_COUNT)?);
    if token_size < 6 {
        return None;
    }
    let wanted = if debug {
        APPLICATION_FWSEC_DEBUG
    } else {
        APPLICATION_FWSEC
    };
    for index in 0..tokens {
        let token = bit + header_size + index * token_size;
        if *rom.get(token)? != FALCON_DATA_TOKEN || *rom.get(token + 1)? != FALCON_DATA_VERSION {
            continue;
        }
        let data = match token_size >= 8 {
            true => word(rom, token + 4)? as usize,
            false => usize::from(half(rom, token + 4)?),
        };
        let table = base + word(rom, data)? as usize;
        let (version, table_header, entry_size, entries) = (
            *rom.get(table)?,
            usize::from(*rom.get(table + 1)?),
            usize::from(*rom.get(table + 2)?),
            usize::from(*rom.get(table + 3)?),
        );
        if version != 1 || table_header < 6 || entry_size < 6 {
            continue;
        }
        for entry in 0..entries {
            let at = table + table_header + entry * entry_size;
            let application = *rom.get(at)?;
            if application != wanted && application != APPLICATION_FIRMWARE_SECURITY {
                continue;
            }
            if let Some(fwsec) = descriptor(rom, base + word(rom, at + 2)? as usize) {
                return Some(fwsec);
            }
        }
    }
    None
}

fn descriptor(rom: &[u8], at: usize) -> Option<Fwsec<'_>> {
    let header = word(rom, at)?;
    let version = header >> 8 & 0xff;
    let size = (header >> 16) as usize;
    if header & 1 == 0 || version != DESCRIPTOR_VERSION_3 || size < DESCRIPTOR_V3_BYTES {
        return None;
    }
    let field = |index: usize| word(rom, at + 4 * index);
    let stored = field(1)? as usize;
    let image = at + size;
    let ucode = rom.get(image..image + stored.next_multiple_of(256))?;
    let fwsec = Fwsec {
        ucode,
        signature_place: field(2)?,
        interface: field(3)?,
        imem_physical: field(4)?,
        imem_bytes: field(5)?,
        imem_virtual: field(6)?,
        dmem_physical: field(7)?,
        dmem_bytes: field(8)?,
        engine_mask: half(rom, at + 36)?,
        ucode_id: *rom.get(at + 38)?,
        signature_versions: half(rom, at + 40)?,
        signatures: rom.get(at + DESCRIPTOR_V3_BYTES..at + size)?,
    };
    let data_end = (fwsec.imem_bytes as usize).checked_add(fwsec.dmem_bytes as usize)?;
    let signature_end = (fwsec.imem_bytes + fwsec.signature_place) as usize + SIGNATURE_BYTES;
    (data_end <= ucode.len() && signature_end <= ucode.len()).then_some(fwsec)
}

/// Why FWSEC's command could not be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareError {
    /// The interface table or the DMEM mapper is not where it should be.
    NoInterface,
    /// No signature is for the fuse's ucode version.
    NoSignature,
}

/// FWSEC's DMEM, ready to run the FRTS command for a region at `offset` bytes into video memory
/// (a multiple of 4 KiB), signed for ucode version `fuse_version` (the fuse's for FWSEC's
/// ucode id).
pub fn prepare_frts(
    fwsec: &Fwsec<'_>,
    offset: u64,
    fuse_version: u32,
) -> Result<Vec<u8>, PrepareError> {
    let start = fwsec.imem_bytes as usize;
    let mut dmem = fwsec.ucode[start..start + fwsec.dmem_bytes as usize].to_vec();
    let interface = fwsec.interface as usize;
    let count = usize::from(*dmem.get(interface + 3).ok_or(PrepareError::NoInterface)?);
    if count < 2 {
        return Err(PrepareError::NoInterface);
    }
    let mapper = (0..count)
        .map(|index| interface + INTERFACE_HEADER_BYTES + index * INTERFACE_ENTRY_BYTES)
        .find(|&at| word(&dmem, at) == Some(DMEM_MAPPER_ID))
        .and_then(|at| word(&dmem, at + 4))
        .map(|place| place as usize)
        .filter(|&place| place + MAPPER_BYTES <= dmem.len())
        .ok_or(PrepareError::NoInterface)?;
    dmem[mapper + MAPPER_INIT_COMMAND..][..4].copy_from_slice(&COMMAND_FRTS.to_le_bytes());
    let buffer = word(&dmem, mapper + MAPPER_COMMAND_BUFFER).ok_or(PrepareError::NoInterface)?;
    let command = frts_command(offset);
    dmem.get_mut(buffer as usize..buffer as usize + command.len())
        .ok_or(PrepareError::NoInterface)?
        .copy_from_slice(&command);
    let signature = signature_for(fwsec, fuse_version).ok_or(PrepareError::NoSignature)?;
    let place = fwsec.signature_place as usize;
    dmem.get_mut(place..place + SIGNATURE_BYTES)
        .ok_or(PrepareError::NoInterface)?
        .copy_from_slice(signature);
    Ok(dmem)
}

fn frts_command(offset: u64) -> [u8; FRTS_COMMAND_BYTES] {
    let mut command = [0u8; FRTS_COMMAND_BYTES];
    let mut put = |at: usize, value: u32| command[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put(0, 1);
    put(4, 24);
    put(20, READ_VBIOS_FLAGS);
    put(24, 1);
    put(28, 20);
    put(32, (offset >> 12) as u32);
    put(36, FRTS_SIZE_IN_4K);
    put(40, MEDIA_FRAMEBUFFER);
    command
}

/// The signature for ucode version `fuse_version`: the signatures are kept in order of their
/// versions' bits, one for each bit set.
pub fn signature_for<'a>(fwsec: &Fwsec<'a>, fuse_version: u32) -> Option<&'a [u8]> {
    let wanted = 1u32.checked_shl(fuse_version)?;
    let versions = u32::from(fwsec.signature_versions);
    if versions & wanted == 0 {
        return None;
    }
    let before = (versions & (wanted - 1)).count_ones() as usize;
    fwsec
        .signatures
        .get(before * SIGNATURE_BYTES..(before + 1) * SIGNATURE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn put16(rom: &mut [u8], at: usize, value: u16) {
        rom[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(rom: &mut [u8], at: usize, value: u32) {
        rom[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn image(rom: &mut [u8], at: usize, blocks: u16, code_type: u8, last: bool) {
        put16(rom, at, ROM_SIGNATURE);
        put16(rom, at + PCI_DATA_POINTER, 0x40);
        put32(rom, at + 0x40, PCI_DATA_SIGNATURES[0]);
        put16(rom, at + 0x40 + PCI_DATA_LENGTH, 0x18);
        put16(rom, at + 0x40 + PCI_DATA_IMAGE_LENGTH, blocks);
        rom[at + 0x40 + PCI_DATA_CODE_TYPE] = code_type;
        rom[at + 0x40 + PCI_DATA_LAST_IMAGE] = if last { LAST_IMAGE } else { 0 };
    }

    fn sample() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        image(&mut rom, 0, 4, CODE_TYPE_BASE, false);
        image(&mut rom, 0x800, 4, 0x03, false);
        image(&mut rom, 0x1000, 0x38, CODE_TYPE_EXTENDED, true);
        let bit = 0x100;
        put16(&mut rom, bit, BIT_ID);
        put32(&mut rom, bit + 2, BIT_SIGNATURE);
        rom[bit + BIT_HEADER_SIZE] = 12;
        rom[bit + BIT_TOKEN_SIZE] = 8;
        rom[bit + BIT_TOKEN_COUNT] = 1;
        let token = bit + 12;
        rom[token] = FALCON_DATA_TOKEN;
        rom[token + 1] = FALCON_DATA_VERSION;
        put16(&mut rom, token + 2, 4);
        put32(&mut rom, token + 4, 0x200);
        let base = 0x1000 - 0x800;
        put32(&mut rom, 0x200, 0x100);
        let table = base + 0x100;
        rom[table..table + 6].copy_from_slice(&[1, 6, 6, 1, 3, 44]);
        rom[table + 6] = APPLICATION_FWSEC;
        put32(&mut rom, table + 8, 0x200);
        let descriptor = base + 0x200;
        let size = DESCRIPTOR_V3_BYTES + 2 * SIGNATURE_BYTES;
        put32(&mut rom, descriptor, 1 | 3 << 8 | (size as u32) << 16);
        put32(&mut rom, descriptor + 4, 0x600);
        put32(&mut rom, descriptor + 8, 0x100);
        put32(&mut rom, descriptor + 12, 0x40);
        put32(&mut rom, descriptor + 20, 0x200);
        put32(&mut rom, descriptor + 32, 0x400);
        put16(&mut rom, descriptor + 40, 0b1010);
        for (index, byte) in [0x11u8, 0x22].into_iter().enumerate() {
            let at = descriptor + DESCRIPTOR_V3_BYTES + index * SIGNATURE_BYTES;
            rom[at..at + SIGNATURE_BYTES].fill(byte);
        }
        let dmem = descriptor + size + 0x200;
        rom[dmem + 0x40..dmem + 0x44].copy_from_slice(&[1, 4, 8, 2]);
        put32(&mut rom, dmem + 0x44, 1);
        put32(&mut rom, dmem + 0x4c, DMEM_MAPPER_ID);
        put32(&mut rom, dmem + 0x50, 0x80);
        put32(&mut rom, dmem + 0x80 + MAPPER_COMMAND_BUFFER, 0x300);
        rom
    }

    #[test]
    fn fwsec_is_found_through_the_bit_and_the_extended_image() {
        let rom = sample();
        assert_eq!(expansion_rom_offset(&rom), Some(0x800));
        let fwsec = find(&rom, false).unwrap();
        assert_eq!((fwsec.imem_bytes, fwsec.dmem_bytes), (0x200, 0x400));
        assert_eq!(fwsec.interface, 0x40);
        assert_eq!(fwsec.ucode.len(), 0x600);
        assert!(find(&rom, true).is_none());
    }

    #[test]
    fn the_frts_command_and_the_fuse_s_signature_are_patched_into_dmem() {
        let rom = sample();
        let fwsec = find(&rom, false).unwrap();
        assert_eq!(signature_for(&fwsec, 3).unwrap()[0], 0x22);
        assert_eq!(signature_for(&fwsec, 1).unwrap()[0], 0x11);
        assert!(signature_for(&fwsec, 2).is_none());
        let dmem = prepare_frts(&fwsec, 0x7_ff00_0000, 3).unwrap();
        assert_eq!(word(&dmem, 0x80 + MAPPER_INIT_COMMAND), Some(COMMAND_FRTS));
        assert_eq!(word(&dmem, 0x300 + 32), Some(0x7f_f000));
        assert_eq!(word(&dmem, 0x300 + 36), Some(FRTS_SIZE_IN_4K));
        assert_eq!(dmem[0x100], 0x22);
        assert_eq!(prepare_frts(&fwsec, 0, 2), Err(PrepareError::NoSignature));
    }
}
