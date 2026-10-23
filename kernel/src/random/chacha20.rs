//! The ChaCha20 block function (RFC 8439, section 2.3): 64 bytes of key stream from a 256-bit
//! key, a 32-bit block counter and a 96-bit nonce. The random number generator's output
//! (D56); nothing here encrypts.

/// "expand 32-byte k", the constant first row of the state.
const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

/// The bytes of one block.
pub const BLOCK_SIZE: usize = 64;

/// The bytes of a key.
pub const KEY_SIZE: usize = 32;

/// The quarter round (RFC 8439, section 2.1) on four words of the state.
#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

/// Block `counter` of the key stream for `key` and `nonce`: 20 rounds over the state, the
/// input state added back, serialized little-endian.
pub fn block(key: &[u8; KEY_SIZE], counter: u32, nonce: &[u8; 12]) -> [u8; BLOCK_SIZE] {
    let mut input = [0u32; 16];
    input[..4].copy_from_slice(&CONSTANTS);
    for (word, bytes) in input[4..12].iter_mut().zip(key.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    input[12] = counter;
    for (word, bytes) in input[13..].iter_mut().zip(nonce.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let mut state = input;
    for _ in 0..10 {
        // A column round, then a diagonal round.
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut output = [0u8; BLOCK_SIZE];
    for ((bytes, word), start) in output
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(state)
        .zip(input)
    {
        *bytes = word.wrapping_add(start).to_le_bytes();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn the_quarter_round_matches_rfc_8439_section_2_1_1() {
        let mut state = [0u32; 16];
        state[..4].copy_from_slice(&[0x1111_1111, 0x0102_0304, 0x9b8d_6f43, 0x0123_4567]);
        quarter_round(&mut state, 0, 1, 2, 3);
        assert_eq!(
            state[..4],
            [0xea2a_92f4, 0xcb1c_f8ce, 0x4581_472e, 0x5881_c4bb]
        );
    }

    #[test]
    fn the_block_matches_rfc_8439_section_2_3_2() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let nonce = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        assert_eq!(
            block(&key, 1, &nonce).as_slice(),
            hex(
                "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
                 d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
            )
        );
    }

    #[test]
    fn the_zero_key_block_matches_rfc_8439_appendix_a_1() {
        assert_eq!(
            block(&[0; 32], 0, &[0; 12]).as_slice(),
            hex(
                "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7\
                 da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586"
            )
        );
    }
}
