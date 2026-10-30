//! `String.Index` as Swift's ABI lays it out on 64-bit platforms (D180): the UTF-8 offset in
//! bits 16-63, a transcoded offset in bits 14-15, a cached character stride in bits 8-13 (zero
//! when unknown), and flags in bits 0-3: aligned to a scalar, aligned to a character, and which
//! encodings the offset is valid in (UTF-8, UTF-16; the start of a string is valid in both).
//! The runtime makes character-aligned UTF-8 indices with no cached stride. Pure logic with
//! host tests against the values `swiftc`-built programs print.

const SCALAR_ALIGNED: u64 = 1;
const CHARACTER_ALIGNED: u64 = 2;
const UTF8: u64 = 4;
const UTF16: u64 = 8;
const OFFSET_SHIFT: u32 = 16;
const TRANSCODED_MASK: u64 = 0b11 << 14;

/// The UTF-8 offset of `index`.
pub fn offset(index: u64) -> usize {
    (index >> OFFSET_SHIFT) as usize
}

/// A character-aligned index at UTF-8 `offset`.
pub fn character(offset: usize) -> u64 {
    let flags = if offset == 0 {
        SCALAR_ALIGNED | CHARACTER_ALIGNED | UTF8 | UTF16
    } else {
        SCALAR_ALIGNED | CHARACTER_ALIGNED | UTF8
    };
    ((offset as u64) << OFFSET_SHIFT) | flags
}

/// Whether `index` is known to start a character.
pub fn is_character_aligned(index: u64) -> bool {
    index & CHARACTER_ALIGNED != 0
}

/// Whether `index` is an offset into UTF-8 (rather than a UTF-16 index of a bridged string, or
/// an index inside a scalar's transcoding).
pub fn is_utf8(index: u64) -> bool {
    index & TRANSCODED_MASK == 0 && (index & UTF8 != 0 || index & UTF16 == 0 || offset(index) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_indices_swift_programs_make() {
        assert_eq!(character(0), 0xf);
        assert_eq!(character(36), 0x24_0007);
        assert_eq!(offset(0x1_0207), 1);
        assert_eq!(offset(0x3_0107), 3);
        assert!(is_character_aligned(0x4_0107));
        assert!(!is_character_aligned(0x2_0004));
        assert!(!is_character_aligned(0x1_0005));
        assert!(is_utf8(0x2_0004));
        assert!(is_utf8(0xf));
        assert!(!is_utf8(0x2_0008));
        assert!(!is_utf8(0x2_4004));
    }
}
