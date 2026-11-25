//! MD4 (RFC 1320), which MS-CHAPv2 hashes passwords with (RFC 2759; D779). Broken as a hash:
//! never for anything new.

/// Bytes of an MD4 digest.
pub const DIGEST_BYTES: usize = 16;
const BLOCK_BYTES: usize = 64;
const SECOND_ROUND: u32 = 0x5a82_7999;
const THIRD_ROUND: u32 = 0x6ed9_eba1;
const SECOND_ORDER: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
const THIRD_ORDER: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];

fn compress(state: &mut [u32; 4], block: &[u8; BLOCK_BYTES]) {
    let mut words = [0u32; 16];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let mut r = *state;
    let step = |r: &mut [u32; 4], mixed: u32, word: u32, constant: u32, shift: u32| {
        let value = r[0]
            .wrapping_add(mixed)
            .wrapping_add(word)
            .wrapping_add(constant)
            .rotate_left(shift);
        *r = [r[3], value, r[1], r[2]];
    };
    for index in 0..16 {
        let mixed = (r[1] & r[2]) | (!r[1] & r[3]);
        step(&mut r, mixed, words[index], 0, [3, 7, 11, 19][index % 4]);
    }
    for index in 0..16 {
        let mixed = (r[1] & r[2]) | (r[1] & r[3]) | (r[2] & r[3]);
        let word = words[SECOND_ORDER[index]];
        step(&mut r, mixed, word, SECOND_ROUND, [3, 5, 9, 13][index % 4]);
    }
    for index in 0..16 {
        let mixed = r[1] ^ r[2] ^ r[3];
        let word = words[THIRD_ORDER[index]];
        step(&mut r, mixed, word, THIRD_ROUND, [3, 9, 11, 15][index % 4]);
    }
    for (value, added) in state.iter_mut().zip(r) {
        *value = value.wrapping_add(added);
    }
}

/// The MD4 digest of the concatenation of `parts`.
pub fn md4(parts: &[&[u8]]) -> [u8; DIGEST_BYTES] {
    let mut state = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut block = [0u8; BLOCK_BYTES];
    let mut filled = 0;
    let mut length = 0u64;
    for part in parts {
        for &byte in *part {
            block[filled] = byte;
            filled += 1;
            if filled == BLOCK_BYTES {
                compress(&mut state, &block);
                filled = 0;
            }
        }
        length += part.len() as u64;
    }
    block[filled] = 0x80;
    block[filled + 1..].fill(0);
    if filled + 1 > BLOCK_BYTES - 8 {
        compress(&mut state, &block);
        block.fill(0);
    }
    block[BLOCK_BYTES - 8..].copy_from_slice(&(length * 8).to_le_bytes());
    compress(&mut state, &block);
    let mut digest = [0u8; DIGEST_BYTES];
    for (bytes, value) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = value.to_le_bytes();
    }
    digest
}
