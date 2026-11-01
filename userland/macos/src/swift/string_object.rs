//! Swift's `String` as the two words Swift's ABI fixes on 64-bit platforms (D180):
//! `_countAndFlagsBits` and `_object`. Up to 15 UTF-8 bytes live in the words themselves (a small
//! string, its length in the top byte's low nibble); longer ones point at their bytes, 32 bytes
//! into an object (the bias): an immortal literal in the program's text, or a native storage
//! object the runtime allocates with the bytes after its header. Pure logic with host tests.

/// The most UTF-8 bytes a small string holds.
pub const SMALL_CAPACITY: usize = 15;
/// How far a large string's bytes lie past the address in its object word.
pub const NATIVE_BIAS: u64 = 32;
/// The count bits of `_countAndFlagsBits`.
pub const COUNT_MASK: u64 = 0x0000_ffff_ffff_ffff;
/// The address bits of a large string's object word.
pub const ADDRESS_MASK: u64 = 0x0fff_ffff_ffff_ffff;

/// `_countAndFlagsBits`' flags.
pub mod flags {
    pub const ASCII: u64 = 1 << 63;
    pub const NFC: u64 = 1 << 62;
    pub const NATIVE: u64 = 1 << 61;
    pub const TAIL_ALLOCATED: u64 = 1 << 60;
}

/// The object word's discriminator bits.
pub mod discriminator {
    pub const IMMORTAL: u64 = 1 << 63;
    pub const SMALL_ASCII_OR_BRIDGED: u64 = 1 << 62;
    pub const SMALL: u64 = 1 << 61;
    pub const FOREIGN: u64 = 1 << 60;
    pub const SMALL_COUNT_SHIFT: u32 = 56;
}

/// A `String` value: what Swift passes in two registers.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawString {
    pub count_and_flags: u64,
    pub object: u64,
}

/// The empty string, as Swift's own literal `""` is.
#[cfg(test)]
pub const EMPTY: RawString = RawString {
    count_and_flags: 0,
    object: discriminator::IMMORTAL | discriminator::SMALL_ASCII_OR_BRIDGED | discriminator::SMALL,
};

/// Where a string's UTF-8 bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// In the two words: [`small_bytes`] has them.
    Small { length: usize },
    /// Contiguous at `address`: a literal or a native storage object's tail.
    Contiguous {
        address: u64,
        length: usize,
        native: bool,
    },
    /// A shared, foreign or bridged string (an NSString's), which the runtime does not read
    /// yet.
    Other,
}

/// How `raw` keeps its bytes.
pub fn form(raw: RawString) -> Form {
    if raw.object & discriminator::SMALL != 0 {
        let length = (raw.object >> discriminator::SMALL_COUNT_SHIFT) & 0xf;
        return Form::Small {
            length: length as usize,
        };
    }
    let foreign = raw.object & discriminator::FOREIGN != 0;
    let tail = raw.count_and_flags & flags::TAIL_ALLOCATED != 0;
    if foreign || !tail {
        return Form::Other;
    }
    Form::Contiguous {
        address: (raw.object & ADDRESS_MASK) + NATIVE_BIAS,
        length: (raw.count_and_flags & COUNT_MASK) as usize,
        native: raw.count_and_flags & flags::NATIVE != 0
            && raw.object & discriminator::IMMORTAL == 0,
    }
}

/// The count of UTF-8 bytes in `raw`, whatever its form.
pub fn length(raw: RawString) -> usize {
    match form(raw) {
        Form::Small { length } => length,
        _ => (raw.count_and_flags & COUNT_MASK) as usize,
    }
}

/// A small string's 16 bytes as they lie in memory; the first [`length`] are its UTF-8.
pub fn small_bytes(raw: RawString) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&raw.count_and_flags.to_le_bytes());
    bytes[8..].copy_from_slice(&raw.object.to_le_bytes());
    bytes
}

/// Whether every byte is ASCII.
pub fn is_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte < 0x80)
}

/// `bytes` as a small string, if they fit.
pub fn small(bytes: &[u8]) -> Option<RawString> {
    if bytes.len() > SMALL_CAPACITY {
        return None;
    }
    let mut all = [0u8; 16];
    all[..bytes.len()].copy_from_slice(bytes);
    let mut top = discriminator::IMMORTAL | discriminator::SMALL;
    if is_ascii(bytes) {
        top |= discriminator::SMALL_ASCII_OR_BRIDGED;
    }
    let count = (bytes.len() as u64) << discriminator::SMALL_COUNT_SHIFT;
    let low = u64::from_le_bytes(all[..8].try_into().unwrap_or([0; 8]));
    let high = u64::from_le_bytes(all[8..].try_into().unwrap_or([0; 8]));
    Some(RawString {
        count_and_flags: low,
        object: (high & !(0xffu64 << 56)) | top | count,
    })
}

fn large_flags(length: usize, ascii: bool) -> u64 {
    let mut value = (length as u64 & COUNT_MASK) | flags::TAIL_ALLOCATED;
    if ascii {
        value |= flags::ASCII | flags::NFC;
    }
    value
}

/// A native string of `length` bytes in the storage object at `storage`.
pub fn native(storage: u64, length: usize, ascii: bool) -> RawString {
    RawString {
        count_and_flags: large_flags(length, ascii) | flags::NATIVE,
        object: storage & ADDRESS_MASK,
    }
}

/// An immortal literal of `length` bytes at `address`, as the compiler emits one.
#[cfg(test)]
pub fn literal(address: u64, length: usize, ascii: bool) -> RawString {
    RawString {
        count_and_flags: large_flags(length, ascii),
        object: (address.wrapping_sub(NATIVE_BIAS) & ADDRESS_MASK) | discriminator::IMMORTAL,
    }
}

/// `__StringStorage`'s real capacity word for `tail` allocated bytes (one of them the NUL).
pub fn capacity_word(tail: usize) -> u64 {
    tail as u64 & COUNT_MASK
}

/// How many bytes a storage with the real capacity word `word` can hold, its NUL aside.
pub fn capacity_of(word: u64) -> usize {
    ((word & COUNT_MASK) as usize).saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_strings_match_what_swiftc_emits() {
        let square = small(b"square").unwrap();
        assert_eq!(square.count_and_flags, 0x0000_6572_6175_7173);
        assert_eq!(square.object, 0xe600_0000_0000_0000);
        assert_eq!(small(b" ").unwrap().object, 0xe100_0000_0000_0000);
        assert_eq!(small(b"").unwrap(), EMPTY);
        assert_eq!(form(square), Form::Small { length: 6 });
        assert_eq!(&small_bytes(square)[..6], b"square");
        let accented = small("h\u{e9}".as_bytes()).unwrap();
        assert_eq!(accented.object >> 56, 0xa3);
        let full = small(b"0123456789abcde").unwrap();
        assert_eq!(&small_bytes(full)[..15], b"0123456789abcde");
        assert_eq!(length(full), 15);
        assert!(small(b"0123456789abcdef").is_none());
    }

    #[test]
    fn literals_and_native_strings_point_32_bytes_before_their_bytes() {
        let text = literal(0x1_0000_4020, 18, true);
        assert_eq!(text.count_and_flags, 0xd000_0000_0000_0012);
        assert_eq!(text.object, 0x8000_0001_0000_4000);
        assert_eq!(
            form(text),
            Form::Contiguous {
                address: 0x1_0000_4020,
                length: 18,
                native: false
            }
        );
        let stored = native(0x1_04c2_a3f0, 23, true);
        assert_eq!(stored.count_and_flags, 0xf000_0000_0000_0017);
        assert_eq!(stored.object, 0x1_04c2_a3f0);
        assert_eq!(
            form(stored),
            Form::Contiguous {
                address: 0x1_04c2_a410,
                length: 23,
                native: true
            }
        );
        assert_eq!(
            native(0x2000, 20, false).count_and_flags,
            0x3000_0000_0000_0014
        );
    }

    #[test]
    fn bridged_and_foreign_strings_are_not_read() {
        let bridged = RawString {
            count_and_flags: 30,
            object: discriminator::SMALL_ASCII_OR_BRIDGED | discriminator::FOREIGN | 0x1000,
        };
        assert_eq!(form(bridged), Form::Other);
        let shared = RawString {
            count_and_flags: 30,
            object: 0x1000,
        };
        assert_eq!(form(shared), Form::Other);
        assert_eq!(length(shared), 30);
    }

    #[test]
    fn storage_capacity_leaves_room_for_the_nul() {
        assert_eq!(capacity_of(capacity_word(32)), 31);
        assert_eq!(capacity_of(0), 0);
    }
}
