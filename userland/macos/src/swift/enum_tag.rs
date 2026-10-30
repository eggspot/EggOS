//! Single-payload enums as Swift lays them out (D180), the layout `Optional` and every enum with
//! one payload case use: empty cases first take the payload's extra inhabitants (bit patterns
//! no valid payload has), then extra tag bytes after the payload. Case 0 is the payload; empty
//! cases are 1 and up. Also the extra inhabitants of pointers. Pure logic with host tests.

/// How many extra inhabitants a heap pointer has: the values below 4 GiB, as many as fit an
/// `i32`.
pub const POINTER_EXTRA_INHABITANTS: u32 = 0x7fff_ffff;
const LEAST_VALID_POINTER: u64 = 1 << 32;

/// How many tag bytes follow a payload of `size` bytes for `empty_cases` cases the payload's
/// extra inhabitants do not cover, with `payload_cases` payload cases.
pub fn tag_bytes(size: usize, empty_cases: u32, payload_cases: u32) -> usize {
    let mut tags = payload_cases;
    if empty_cases > 0 {
        if size >= 4 {
            tags += 1;
        } else {
            let bits = size as u32 * 8;
            let per_tag = 1u32 << bits;
            tags += empty_cases.div_ceil(per_tag);
        }
    }
    match tags {
        0..=1 => 0,
        2..=255 => 1,
        256..=65535 => 2,
        _ => 4,
    }
}

/// The little-endian integer in the first four (or fewer) bytes of `bytes`.
pub fn read_integer(bytes: &[u8]) -> u32 {
    let mut word = [0u8; 4];
    let length = bytes.len().min(4);
    word[..length].copy_from_slice(&bytes[..length]);
    u32::from_le_bytes(word)
}

/// Writes `value` little-endian into the first four (or fewer) bytes of `bytes` and zeroes the
/// rest.
pub fn write_integer(bytes: &mut [u8], value: u32) {
    let word = value.to_le_bytes();
    let length = bytes.len().min(4);
    bytes[..length].copy_from_slice(&word[..length]);
    bytes[length..].fill(0);
}

/// The case `value` holds: `value` is the payload's `payload_size` bytes then its tag bytes;
/// `extra_inhabitant_tag` reads the payload's extra inhabitant (0 for a valid payload, the
/// inhabitant's index plus one otherwise).
pub fn get_tag(
    value: &[u8],
    payload_size: usize,
    empty_cases: u32,
    extra_inhabitants: u32,
    extra_inhabitant_tag: impl FnOnce(&[u8]) -> u32,
) -> u32 {
    if empty_cases > extra_inhabitants {
        let count = tag_bytes(payload_size, empty_cases - extra_inhabitants, 1);
        let extra = read_integer(value.get(payload_size..payload_size + count).unwrap_or(&[]));
        if extra > 0 {
            let from_tag = if payload_size >= 4 {
                0
            } else {
                (extra - 1) << (payload_size * 8)
            };
            let from_payload = read_integer(&value[..payload_size.min(value.len())]);
            return (from_tag | from_payload) + extra_inhabitants + 1;
        }
    }
    if extra_inhabitants > 0 {
        return extra_inhabitant_tag(&value[..payload_size.min(value.len())]);
    }
    0
}

/// Stores case `case` into `value` (laid out as for [`get_tag`]); `store_extra_inhabitant`
/// writes the payload's extra inhabitant of the given index.
pub fn store_tag(
    value: &mut [u8],
    payload_size: usize,
    case: u32,
    empty_cases: u32,
    extra_inhabitants: u32,
    store_extra_inhabitant: impl FnOnce(&mut [u8], u32),
) {
    let count = if empty_cases > extra_inhabitants {
        tag_bytes(payload_size, empty_cases - extra_inhabitants, 1)
    } else {
        0
    };
    let tag_end = (payload_size + count).min(value.len());
    if case <= extra_inhabitants {
        if count > 0 {
            write_integer(&mut value[payload_size..tag_end], 0);
        }
        if case > 0 {
            store_extra_inhabitant(&mut value[..payload_size], case - 1);
        }
        return;
    }
    let index = case - 1 - extra_inhabitants;
    let (payload_index, tag_index) = if payload_size >= 4 {
        (index, 1)
    } else {
        let bits = payload_size as u32 * 8;
        (index & ((1u32 << bits) - 1), 1 + (index >> bits))
    };
    if payload_size > 0 {
        write_integer(&mut value[..payload_size], payload_index);
    }
    if count > 0 {
        write_integer(&mut value[payload_size..tag_end], tag_index);
    }
}

/// The extra inhabitant tag of the heap pointer `value` (0 when it is a valid pointer), with
/// `reserved_low_bits` low bits that tagged Objective-C pointers use.
pub fn pointer_tag(value: u64, reserved_low_bits: u32) -> u32 {
    if value >= LEAST_VALID_POINTER {
        return 0;
    }
    if value & ((1u64 << reserved_low_bits) - 1) != 0 {
        return 0;
    }
    (value >> reserved_low_bits) as u32 + 1
}

/// The pointer value of the extra inhabitant `index`.
pub fn pointer_inhabitant(index: u32, reserved_low_bits: u32) -> u64 {
    u64::from(index) << reserved_low_bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &[u8]) -> u32 {
        0
    }

    #[test]
    fn an_optional_without_extra_inhabitants_takes_a_tag_byte() {
        assert_eq!(tag_bytes(8, 1, 1), 1);
        assert_eq!(tag_bytes(1, 1, 1), 1);
        assert_eq!(tag_bytes(1, 300, 1), 1);
        assert_eq!(tag_bytes(1, 300 * 256, 1), 2);
        assert_eq!(tag_bytes(8, 0, 1), 0);
        let mut value = [7u8; 9];
        store_tag(&mut value, 8, 1, 1, 0, |_, _| {});
        assert_eq!(value, [0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(get_tag(&value, 8, 1, 0, none), 1);
        let mut payload = [5u8, 0, 0, 0, 0, 0, 0, 0, 9];
        store_tag(&mut payload, 8, 0, 1, 0, |_, _| {});
        assert_eq!(payload[8], 0);
        assert_eq!(get_tag(&payload, 8, 1, 0, none), 0);
    }

    #[test]
    fn nested_optionals_count_their_cases_in_the_payload_and_tag() {
        let mut value = [0u8; 10];
        store_tag(&mut value, 9, 1, 1, 0, |_, _| {});
        assert_eq!(value[9], 1);
        assert_eq!(get_tag(&value, 9, 1, 0, none), 1);
        let mut small = [0u8; 2];
        store_tag(&mut small, 1, 258, 300, 0, |_, _| {});
        assert_eq!(small, [1, 2]);
        assert_eq!(get_tag(&small, 1, 300, 0, none), 258);
    }

    #[test]
    fn extra_inhabitants_cover_empty_cases_without_a_tag_byte() {
        let mut value = [0u8; 8];
        let mut stored = None;
        store_tag(
            &mut value,
            8,
            1,
            1,
            POINTER_EXTRA_INHABITANTS,
            |_, index| stored = Some(index),
        );
        assert_eq!(stored, Some(0));
        let tag = get_tag(&value, 8, 1, POINTER_EXTRA_INHABITANTS, |payload| {
            pointer_tag(u64::from_le_bytes(payload.try_into().unwrap()), 0)
        });
        assert_eq!(tag, 1);
    }

    #[test]
    fn pointers_below_four_gigabytes_are_extra_inhabitants() {
        assert_eq!(pointer_tag(0, 0), 1);
        assert_eq!(pointer_tag(5, 0), 6);
        assert_eq!(pointer_tag(0x1_0000_4000, 0), 0);
        assert_eq!(pointer_tag(4, 1), 3);
        assert_eq!(pointer_tag(5, 1), 0);
        assert_eq!(pointer_inhabitant(2, 1), 4);
        assert_eq!(pointer_inhabitant(0, 0), 0);
    }
}
