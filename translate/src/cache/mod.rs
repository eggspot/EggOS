//! The translation cache (D191): one `.tx` file per translated module, named by a key that is
//! the SHA-256 of the input file's digest, the translator version and the code-generation
//! mode, so a different CPU or a newer translator never reuses stale code. [`serialize`] and
//! the strict [`parse`] handle the file format; [`policy`] decides names, the size cap and
//! what to remove. Reading, writing (to a temporary name, then a rename) and deleting files
//! are the caller's.
//!
//! The format, little-endian: a 160-byte header (magic `EGGOSTX\0`, format version, mode bits,
//! entries-table bits, translator version, key, input digest, preferred image base, the code's
//! offset and size, the dispatch and trampoline offsets, the table counts and the statistics),
//! zeros up to [`CODE_OFFSET`] so the code can be mapped page-aligned, the code, the entries
//! table's slots, the back-map, the refused sites, the guest-address literal offsets, and a
//! trailing SHA-256 of everything before it.

pub mod policy;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;

use eggos_crypto::sha2::Sha256;

use crate::Refusal;
use crate::codegen::CodegenMode;
use crate::image::{
    BackMapEntry, EntriesTable, RefusedSite, SiteReason, TranslatedImage, TranslationStats,
};

/// The version of the code generator; bumped whenever translated code changes, so cache
/// keys change with it.
pub const TRANSLATOR_VERSION: u32 = 1;

/// The version of the `.tx` layout.
pub const FORMAT_VERSION: u16 = 1;

/// Where the code starts in the file: one 4 KiB page, so it can be mapped directly.
pub const CODE_OFFSET: usize = 4096;

const MAGIC: [u8; 8] = *b"EGGOSTX\0";
const HEADER_BYTES: usize = 160;
const DIGEST_BYTES: usize = 32;
const KEY_DOMAIN: &[u8] = b"eggos-translate cache key\0";
const SITE_BYTES: usize = 8;
const BACK_MAP_BYTES: usize = 8;
const MODE_BITS: u8 = 0x0F;

const REFUSALS: [Refusal; 11] = [
    Refusal::Vex,
    Refusal::Evex,
    Refusal::Xop,
    Refusal::Mmx,
    Refusal::Sse3,
    Refusal::ThreeByteMap,
    Refusal::X87,
    Refusal::AddressSize,
    Refusal::Branch16,
    Refusal::StateSave,
    Refusal::System,
];

/// A cache file's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFile {
    /// The SHA-256 of the input file.
    pub input_digest: [u8; 32],
    /// The CPU the code was generated for.
    pub mode: CodegenMode,
    /// The preferred image base the code's guest addresses are relative to.
    pub image_base: u64,
    /// The translated image.
    pub image: TranslatedImage,
}

impl CacheFile {
    /// Its cache key.
    pub fn key(&self) -> [u8; 32] {
        cache_key(&self.input_digest, self.mode)
    }
}

/// Why a cache file was rejected; any of them means translating again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheError {
    /// The file ends before its declared contents do.
    Truncated,
    /// Not a `.tx` file.
    BadMagic,
    /// A `.tx` layout this translator does not read.
    WrongFormat,
    /// Written by another translator version.
    WrongTranslator,
    /// The trailing checksum does not match.
    Checksum,
    /// The header's key does not match its input digest, translator and mode.
    KeyMismatch,
    /// A field is out of range or inconsistent.
    Malformed,
}

/// The SHA-256 of an input file.
pub fn input_digest(file: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(file);
    hash.finish()
}

/// The mode's bits in the header and the key.
pub const fn mode_bits(mode: CodegenMode) -> u8 {
    (mode.hardware_tso as u8)
        | ((mode.lse as u8) << 1)
        | ((mode.lrcpc as u8) << 2)
        | ((mode.flagm as u8) << 3)
}

/// The mode of header bits; `None` when an unknown bit is set.
pub const fn mode_from_bits(bits: u8) -> Option<CodegenMode> {
    if bits & !MODE_BITS != 0 {
        return None;
    }
    Some(CodegenMode {
        hardware_tso: bits & 1 != 0,
        lse: bits & 2 != 0,
        lrcpc: bits & 4 != 0,
        flagm: bits & 8 != 0,
    })
}

/// The cache key of an input (by its digest) translated for `mode` by this translator.
pub fn cache_key(input_digest: &[u8; 32], mode: CodegenMode) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(KEY_DOMAIN);
    hash.update(input_digest);
    hash.update(&TRANSLATOR_VERSION.to_le_bytes());
    hash.update(&[mode_bits(mode)]);
    hash.finish()
}

fn site_bytes(site: &RefusedSite) -> [u8; SITE_BYTES] {
    let (kind, detail) = match site.reason {
        SiteReason::Refused(refusal) => (
            1,
            REFUSALS
                .iter()
                .position(|&known| known == refusal)
                .unwrap_or(0) as u8,
        ),
        SiteReason::NotLowered => (2, 0),
    };
    let rva = site.rva.to_le_bytes();
    [rva[0], rva[1], rva[2], rva[3], site.length, kind, detail, 0]
}

fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// The bytes of a `.tx` file.
pub fn serialize(file: &CacheFile) -> Vec<u8> {
    let image = &file.image;
    let stats = &image.stats;
    let mut bytes = Vec::with_capacity(
        CODE_OFFSET
            + image.code.len()
            + 8 * image.entries.slots().len()
            + BACK_MAP_BYTES * image.back_map.len()
            + SITE_BYTES * image.refused_sites.len()
            + 4 * image.guest_address_literals.len()
            + DIGEST_BYTES,
    );
    bytes.extend_from_slice(&MAGIC);
    put_u16(&mut bytes, FORMAT_VERSION);
    bytes.push(mode_bits(file.mode));
    bytes.push(image.entries.bits());
    put_u32(&mut bytes, TRANSLATOR_VERSION);
    bytes.extend_from_slice(&file.key());
    bytes.extend_from_slice(&file.input_digest);
    put_u64(&mut bytes, file.image_base);
    put_u32(&mut bytes, CODE_OFFSET as u32);
    put_u32(&mut bytes, image.code.len() as u32);
    put_u32(&mut bytes, image.dispatch);
    put_u32(&mut bytes, image.enter);
    put_u32(&mut bytes, image.back_map.len() as u32);
    put_u32(&mut bytes, image.refused_sites.len() as u32);
    put_u32(&mut bytes, image.guest_address_literals.len() as u32);
    put_u32(&mut bytes, 0);
    put_u64(&mut bytes, stats.guest_bytes);
    for value in [
        stats.blocks,
        stats.instructions,
        stats.entries,
        stats.jump_tables,
        stats.swept_entries,
        stats.unresolved_jumps,
        stats.veneers,
        0,
    ] {
        put_u32(&mut bytes, value);
    }
    bytes.resize(CODE_OFFSET, 0);
    bytes.extend_from_slice(&image.code);
    for &slot in image.entries.slots() {
        put_u64(&mut bytes, slot);
    }
    for entry in &image.back_map {
        put_u32(&mut bytes, entry.host);
        put_u32(&mut bytes, entry.rva);
    }
    for site in &image.refused_sites {
        bytes.extend_from_slice(&site_bytes(site));
    }
    for &literal in &image.guest_address_literals {
        put_u32(&mut bytes, literal);
    }
    let mut hash = Sha256::new();
    hash.update(&bytes);
    let checksum = hash.finish();
    bytes.extend_from_slice(&checksum);
    bytes
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], CacheError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(CacheError::Malformed)?;
        let taken = self
            .bytes
            .get(self.position..end)
            .ok_or(CacheError::Truncated)?;
        self.position = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CacheError> {
        let mut array = [0; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }

    fn u8(&mut self) -> Result<u8, CacheError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, CacheError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn count(&mut self) -> Result<usize, CacheError> {
        usize::try_from(self.u32()?).map_err(|_| CacheError::Malformed)
    }
}

fn site_from(bytes: &[u8]) -> Result<RefusedSite, CacheError> {
    let rva = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let reason = match (bytes[5], bytes[6]) {
        (1, detail) => SiteReason::Refused(
            *REFUSALS
                .get(usize::from(detail))
                .ok_or(CacheError::Malformed)?,
        ),
        (2, 0) => SiteReason::NotLowered,
        _ => return Err(CacheError::Malformed),
    };
    if bytes[7] != 0 {
        return Err(CacheError::Malformed);
    }
    Ok(RefusedSite {
        rva,
        length: bytes[4],
        reason,
    })
}

/// Reads a `.tx` file, checking everything: the magic, the format and translator versions,
/// the checksum, the key, every size and offset, and that nothing follows the checksum.
pub fn parse(bytes: &[u8]) -> Result<CacheFile, CacheError> {
    if bytes.len() < MAGIC.len() {
        return Err(CacheError::Truncated);
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(CacheError::BadMagic);
    }
    let mut reader = Reader {
        bytes,
        position: MAGIC.len(),
    };
    if reader.u16()? != FORMAT_VERSION {
        return Err(CacheError::WrongFormat);
    }
    let mode = reader.u8()?;
    let bits = reader.u8()?;
    if reader.u32()? != TRANSLATOR_VERSION {
        return Err(CacheError::WrongTranslator);
    }
    let key: [u8; 32] = reader.array()?;
    let input_digest: [u8; 32] = reader.array()?;
    let image_base = reader.u64()?;
    let code_offset = reader.count()?;
    let code_bytes = reader.count()?;
    let dispatch = reader.u32()?;
    let enter = reader.u32()?;
    let back_map_count = reader.count()?;
    let site_count = reader.count()?;
    let literal_count = reader.count()?;
    let reserved = reader.u32()?;
    let guest_bytes = reader.u64()?;
    let mut counters = [0u32; 8];
    for counter in &mut counters {
        *counter = reader.u32()?;
    }
    if reader.position != HEADER_BYTES {
        return Err(CacheError::Malformed);
    }
    if bits >= 32 {
        return Err(CacheError::Malformed);
    }
    let slots = 1usize << bits;
    let body = slots
        .checked_mul(8)
        .and_then(|total| total.checked_add(code_bytes))
        .and_then(|total| total.checked_add(back_map_count.checked_mul(BACK_MAP_BYTES)?))
        .and_then(|total| total.checked_add(site_count.checked_mul(SITE_BYTES)?))
        .and_then(|total| total.checked_add(literal_count.checked_mul(4)?))
        .and_then(|total| total.checked_add(CODE_OFFSET + DIGEST_BYTES))
        .ok_or(CacheError::Malformed)?;
    if bytes.len() < body {
        return Err(CacheError::Truncated);
    }
    if bytes.len() > body {
        return Err(CacheError::Malformed);
    }
    let (content, checksum) = bytes.split_at(body - DIGEST_BYTES);
    let mut hash = Sha256::new();
    hash.update(content);
    if hash.finish()[..] != checksum[..] {
        return Err(CacheError::Checksum);
    }
    let mode = mode_from_bits(mode).ok_or(CacheError::Malformed)?;
    if cache_key(&input_digest, mode) != key {
        return Err(CacheError::KeyMismatch);
    }
    let code_end = code_bytes as u64;
    let in_code = |offset: u32| offset % 4 == 0 && u64::from(offset) < code_end;
    if code_offset != CODE_OFFSET
        || reserved != 0
        || counters[7] != 0
        || code_bytes % 8 != 0
        || !in_code(dispatch)
        || !in_code(enter)
        || bytes[HEADER_BYTES..CODE_OFFSET]
            .iter()
            .any(|&byte| byte != 0)
    {
        return Err(CacheError::Malformed);
    }
    reader.position = CODE_OFFSET;
    let code = reader.take(code_bytes)?.to_vec();
    let mut slot_values = Vec::with_capacity(slots);
    for _ in 0..slots {
        slot_values.push(reader.u64()?);
    }
    let entries = EntriesTable::from_slots(bits, slot_values).ok_or(CacheError::Malformed)?;
    if entries
        .slots()
        .iter()
        .any(|&slot| slot as u32 != crate::image::EMPTY_KEY && !in_code((slot >> 32) as u32))
    {
        return Err(CacheError::Malformed);
    }
    let mut back_map = Vec::with_capacity(back_map_count);
    for _ in 0..back_map_count {
        let host = reader.u32()?;
        let rva = reader.u32()?;
        if !in_code(host)
            || back_map
                .last()
                .is_some_and(|last: &BackMapEntry| last.host > host)
        {
            return Err(CacheError::Malformed);
        }
        back_map.push(BackMapEntry { host, rva });
    }
    let mut refused_sites = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        refused_sites.push(site_from(reader.take(SITE_BYTES)?)?);
    }
    let mut guest_address_literals = Vec::with_capacity(literal_count);
    for _ in 0..literal_count {
        let literal = reader.u32()?;
        if literal % 8 != 0 || u64::from(literal) + 8 > code_end {
            return Err(CacheError::Malformed);
        }
        guest_address_literals.push(literal);
    }
    Ok(CacheFile {
        input_digest,
        mode,
        image_base,
        image: TranslatedImage {
            code,
            dispatch,
            enter,
            entries,
            back_map,
            refused_sites,
            guest_address_literals,
            stats: TranslationStats {
                guest_bytes,
                blocks: counters[0],
                instructions: counters[1],
                entries: counters[2],
                jump_tables: counters[3],
                swept_entries: counters[4],
                unresolved_jumps: counters[5],
                veneers: counters[6],
            },
        },
    })
}
