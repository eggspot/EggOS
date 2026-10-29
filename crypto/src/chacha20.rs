//! The ChaCha20 stream cipher (RFC 8439, section 2.4), the same block function as the
//! kernel's random number generator (D56).

/// The bytes of a key.
pub const KEY_BYTES: usize = 32;
/// The bytes of a nonce.
pub const NONCE_BYTES: usize = 12;
/// The bytes of one block of key stream.
pub const BLOCK_BYTES: usize = 64;

const CONSTANTS: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

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

/// Block `counter` of the key stream for `key` and `nonce`.
pub fn block(key: &[u8; KEY_BYTES], counter: u32, nonce: &[u8; NONCE_BYTES]) -> [u8; BLOCK_BYTES] {
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
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut output = [0u8; BLOCK_BYTES];
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

/// Encrypts or decrypts `data` in place with the key stream from block `counter` on.
pub fn apply_key_stream(
    key: &[u8; KEY_BYTES],
    counter: u32,
    nonce: &[u8; NONCE_BYTES],
    data: &mut [u8],
) {
    for (index, chunk) in data.chunks_mut(BLOCK_BYTES).enumerate() {
        let stream = block(key, counter.wrapping_add(index as u32), nonce);
        for (byte, key) in chunk.iter_mut().zip(stream) {
            *byte ^= key;
        }
    }
}
