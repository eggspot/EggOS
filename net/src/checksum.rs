//! The internet checksum (RFC 1071) that IPv4, ICMP and UDP carry: the ones' complement of the
//! ones' complement sum of the bytes taken as big-endian 16-bit words.

/// Adds `bytes` to the running sum `sum` as big-endian 16-bit words, an odd last byte padded
/// with zero, and folds the carries back in. Any number of calls may build one sum, as long
/// as every call but the last adds an even number of bytes.
pub fn add(sum: u32, bytes: &[u8]) -> u32 {
    let mut sum = fold(sum);
    let (pairs, rest) = bytes.as_chunks::<2>();
    for chunk in pairs.chunks(16_384) {
        for pair in chunk {
            sum += u32::from(u16::from_be_bytes(*pair));
        }
        sum = fold(sum);
    }
    if let [last] = rest {
        sum += u32::from(*last) << 8;
    }
    fold(sum)
}

fn fold(mut sum: u32) -> u32 {
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum
}

/// The checksum of a finished sum: its ones' complement.
pub fn finish(sum: u32) -> u16 {
    !(fold(sum) as u16)
}

/// The checksum of `bytes`; 0 when `bytes` hold a correct checksum of their own.
pub fn of(bytes: &[u8]) -> u16 {
    finish(add(0, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_1071_s_example_sums_and_a_correct_checksum_checks_to_zero() {
        let words = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(add(0, &words), 0xddf2);
        assert_eq!(of(&words), !0xddf2);
        let mut with_checksum = words.to_vec();
        with_checksum.extend_from_slice(&of(&words).to_be_bytes());
        assert_eq!(of(&with_checksum), 0);
    }

    #[test]
    fn an_odd_byte_is_padded_and_sums_can_be_built_in_pieces() {
        assert_eq!(add(0, &[0x12]), 0x1200);
        let whole = [1, 2, 3, 4, 5, 6, 7];
        assert_eq!(add(add(0, &whole[..4]), &whole[4..]), add(0, &whole));
    }

    #[test]
    fn a_large_buffer_of_ones_folds_its_carries() {
        let ones = [0xffu8; 70_000];
        assert_eq!(of(&ones), 0);
    }
}
