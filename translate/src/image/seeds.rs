use alloc::vec::Vec;

const RUNTIME_FUNCTION_BYTES: usize = 12;

/// The function starts (`BeginAddress`, relative to the image base) of a PE `.pdata` exception
/// directory: an array of 12-byte `RUNTIME_FUNCTION` entries (begin, end, unwind information).
/// Entries whose begin is not before their end are skipped; a trailing partial entry is
/// ignored.
pub fn pdata_function_starts(pdata: &[u8]) -> Vec<u32> {
    pdata
        .as_chunks::<RUNTIME_FUNCTION_BYTES>()
        .0
        .iter()
        .filter_map(|entry| {
            let begin = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let end = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
            (begin < end).then_some(begin)
        })
        .collect()
}

/// The function starts of a Mach-O `LC_FUNCTION_STARTS` blob, relative to the image base:
/// ULEB128 deltas, the first from `text_rva` (the `__TEXT` segment's address relative to the
/// image base), each next from the previous start, ended by a zero delta or the blob's end.
/// `None` when a number runs past the blob or overflows.
pub fn function_starts(blob: &[u8], text_rva: u32) -> Option<Vec<u32>> {
    let mut starts = Vec::new();
    let mut current = u64::from(text_rva);
    let mut position = 0;
    while position < blob.len() {
        let (delta, used) = uleb128(&blob[position..])?;
        position += used;
        if delta == 0 {
            break;
        }
        current = current.checked_add(delta)?;
        starts.push(u32::try_from(current).ok()?);
    }
    Some(starts)
}

fn uleb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().enumerate() {
        let shift = 7 * index as u32;
        if shift >= 64 {
            return None;
        }
        let bits = u64::from(byte & 0x7F);
        if shift > 0 && bits >> (64 - shift) != 0 {
            return None;
        }
        value |= bits << shift;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}
