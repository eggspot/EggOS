//! BLAKE2s (RFC 7693) with a 32-byte digest, keyed or not: the random number generator's
//! entropy extractor (D56). Inputs are hashed as they arrive, so any amount of seed
//! material costs one 64-byte buffer.

/// The bytes of a digest, and of the longest key.
pub const DIGEST_SIZE: usize = 32;

/// The bytes of one compressed block.
const BLOCK_SIZE: usize = 64;

/// The initialization vector (RFC 7693, section 2.6): SHA-256's.
const IV: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// The message word schedule of each of the ten rounds (RFC 7693, section 2.7).
const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

/// A BLAKE2s-256 hash in progress.
#[derive(Clone)]
pub struct Blake2s {
    /// The chained state `h`.
    state: [u32; 8],
    /// Bytes compressed so far: the counter `t`.
    compressed: u64,
    /// The block not compressed yet: the last one is compressed with the final flag.
    buffer: [u8; BLOCK_SIZE],
    /// How much of `buffer` is filled.
    buffered: usize,
}

impl Blake2s {
    /// An unkeyed hash.
    pub const fn new() -> Self {
        Self::with_key_length(0)
    }

    /// A keyed hash (RFC 7693, section 3.3: the key, zero-padded, is the first block).
    ///
    /// # Panics
    /// If `key` is longer than [`DIGEST_SIZE`] bytes.
    pub fn keyed(key: &[u8]) -> Self {
        assert!(
            key.len() <= DIGEST_SIZE,
            "a BLAKE2s key is at most 32 bytes"
        );
        let mut hash = Self::with_key_length(key.len());
        if !key.is_empty() {
            hash.buffer[..key.len()].copy_from_slice(key);
            hash.buffered = BLOCK_SIZE;
        }
        hash
    }

    const fn with_key_length(key_length: usize) -> Self {
        let mut state = IV;
        // The parameter block's first word: digest length, key length, fanout 1, depth 1.
        state[0] ^= 0x0101_0000 ^ ((key_length as u32) << 8) ^ DIGEST_SIZE as u32;
        Self {
            state,
            compressed: 0,
            buffer: [0; BLOCK_SIZE],
            buffered: 0,
        }
    }

    /// Hashes `bytes` after everything so far.
    pub fn update(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.buffered == BLOCK_SIZE {
                self.compressed += BLOCK_SIZE as u64;
                compress(&mut self.state, &self.buffer, self.compressed, false);
                self.buffered = 0;
            }
            let take = bytes.len().min(BLOCK_SIZE - self.buffered);
            self.buffer[self.buffered..][..take].copy_from_slice(&bytes[..take]);
            self.buffered += take;
            bytes = &bytes[take..];
        }
    }

    /// The digest of everything hashed.
    pub fn finalize(mut self) -> [u8; DIGEST_SIZE] {
        self.compressed += self.buffered as u64;
        self.buffer[self.buffered..].fill(0);
        compress(&mut self.state, &self.buffer, self.compressed, true);
        let mut digest = [0; DIGEST_SIZE];
        for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *bytes = word.to_le_bytes();
        }
        digest
    }
}

/// Wipes the state and the buffer, which may hold secret input or be derived from it (seed
/// material, keys): with volatile stores, which the compiler cannot drop as it may drop a
/// plain store to a value about to go away.
impl Drop for Blake2s {
    fn drop(&mut self) {
        for word in &mut self.state {
            // SAFETY: `word` is a valid, aligned, exclusive reference.
            unsafe { core::ptr::write_volatile(word, 0) };
        }
        eggos_hal::random::wipe(&mut self.buffer);
    }
}

/// Shows nothing of the state: it may be derived from secrets.
impl core::fmt::Debug for Blake2s {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Blake2s").finish_non_exhaustive()
    }
}

impl Default for Blake2s {
    fn default() -> Self {
        Self::new()
    }
}

/// The mixing function `G` (RFC 7693, section 3.1).
#[inline(always)]
fn mix(v: &mut [u32; 16], [a, b, c, d]: [usize; 4], x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// The compression function `F` (RFC 7693, section 3.2) on `block`, with `counter` bytes
/// hashed so far, this block's included.
fn compress(state: &mut [u32; 8], block: &[u8; BLOCK_SIZE], counter: u64, last: bool) {
    let mut m = [0u32; 16];
    for (word, bytes) in m.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let mut v = [0u32; 16];
    v[..8].copy_from_slice(state);
    v[8..].copy_from_slice(&IV);
    v[12] ^= counter as u32;
    v[13] ^= (counter >> 32) as u32;
    if last {
        v[14] = !v[14];
    }
    for s in &SIGMA {
        mix(&mut v, [0, 4, 8, 12], m[s[0]], m[s[1]]);
        mix(&mut v, [1, 5, 9, 13], m[s[2]], m[s[3]]);
        mix(&mut v, [2, 6, 10, 14], m[s[4]], m[s[5]]);
        mix(&mut v, [3, 7, 11, 15], m[s[6]], m[s[7]]);
        mix(&mut v, [0, 5, 10, 15], m[s[8]], m[s[9]]);
        mix(&mut v, [1, 6, 11, 12], m[s[10]], m[s[11]]);
        mix(&mut v, [2, 7, 8, 13], m[s[12]], m[s[13]]);
        mix(&mut v, [3, 4, 9, 14], m[s[14]], m[s[15]]);
    }
    for (i, word) in state.iter_mut().enumerate() {
        *word ^= v[i] ^ v[i + 8];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: [u8; DIGEST_SIZE]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn hash(bytes: &[u8]) -> String {
        let mut hash = Blake2s::new();
        hash.update(bytes);
        hex(hash.finalize())
    }

    #[test]
    fn abc_matches_rfc_7693_appendix_b() {
        assert_eq!(
            hash(b"abc"),
            "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"
        );
    }

    #[test]
    fn the_empty_message_has_the_published_digest() {
        assert_eq!(
            hash(b""),
            "69217a3079908094e11121d042354a7c1f55b6482ca1a51e1b250dfd1ed0eef9"
        );
    }

    #[test]
    fn a_keyed_hash_of_a_full_block_has_the_published_digest() {
        // The BLAKE2 reference test vectors: key 00..1f, message 00..3f.
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let message: [u8; 64] = core::array::from_fn(|i| i as u8);
        let mut hash = Blake2s::keyed(&key);
        hash.update(&message);
        assert_eq!(
            hex(hash.finalize()),
            "8975b0577fd35566d750b362b0897a26c399136df07bababbde6203ff2954ed4"
        );
    }

    #[test]
    fn input_split_anywhere_hashes_the_same() {
        let message: Vec<u8> = (0..200u32).map(|i| (i * 7) as u8).collect();
        let whole = hash(&message);
        for split in [0, 1, 63, 64, 65, 128, 199, 200] {
            let mut hash = Blake2s::new();
            hash.update(&message[..split]);
            hash.update(&message[split..]);
            assert_eq!(hex(hash.finalize()), whole, "split at {split}");
        }
    }
}
