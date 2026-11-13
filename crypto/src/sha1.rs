//! SHA-1 (FIPS 180-4), HMAC-SHA1 (RFC 2104) and PBKDF2-HMAC-SHA1 (RFC 8018), which WPA2's key
//! derivation and EAPOL-Key MICs still use (IEEE 802.11-2020, 12.7; D246). Not for anything new.

/// Bytes of a SHA-1 digest.
pub const DIGEST_BYTES: usize = 20;
const BLOCK_BYTES: usize = 64;

/// A SHA-1 computation.
#[derive(Clone)]
pub struct Sha1 {
    state: [u32; 5],
    block: [u8; BLOCK_BYTES],
    filled: usize,
    length: u64,
}

impl core::fmt::Debug for Sha1 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Sha1")
    }
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub const fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xefcd_ab89,
                0x98ba_dcfe,
                0x1032_5476,
                0xc3d2_e1f0,
            ],
            block: [0; BLOCK_BYTES],
            filled: 0,
            length: 0,
        }
    }

    fn compress(&mut self) {
        let mut words = [0u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(
                self.block[index * 4..index * 4 + 4]
                    .try_into()
                    .unwrap_or_default(),
            );
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (index, &word) in words.iter().enumerate() {
            let (mix, constant) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *state = state.wrapping_add(value);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (BLOCK_BYTES - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == BLOCK_BYTES {
                self.compress();
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; DIGEST_BYTES] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != BLOCK_BYTES - 8 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut digest = [0u8; DIGEST_BYTES];
        for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            bytes.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
}

/// SHA-1 of `data`.
pub fn sha1(data: &[u8]) -> [u8; DIGEST_BYTES] {
    let mut hash = Sha1::new();
    hash.update(data);
    hash.finish()
}

/// HMAC-SHA1 of the concatenation of `parts` under `key`.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; DIGEST_BYTES] {
    let mut block = [0u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        block[..DIGEST_BYTES].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(&block.map(|byte| byte ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let inner = inner.finish();
    let mut outer = Sha1::new();
    outer.update(&block.map(|byte| byte ^ 0x5c));
    outer.update(&inner);
    outer.finish()
}

/// PBKDF2-HMAC-SHA1 of `password` and `salt` over `rounds`, filling `out`.
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    for (index, chunk) in out.chunks_mut(DIGEST_BYTES).enumerate() {
        let counter = (index as u32 + 1).to_be_bytes();
        let mut block = hmac_sha1(password, &[salt, &counter]);
        let mut sum = block;
        for _ in 1..rounds {
            block = hmac_sha1(password, &[&block]);
            for (total, byte) in sum.iter_mut().zip(block) {
                *total ^= byte;
            }
        }
        chunk.copy_from_slice(&sum[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha1_matches_fips_180_examples() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn hmac_sha1_matches_rfc_2202() {
        let key = [0x0bu8; 20];
        assert_eq!(
            hex(&hmac_sha1(&key, &[b"Hi There"])),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
    }

    #[test]
    fn pbkdf2_matches_rfc_6070_and_the_wpa2_psk_vector() {
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 2, &mut out);
        assert_eq!(hex(&out), "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957");
        let mut psk = [0u8; 32];
        pbkdf2_hmac_sha1(b"password", b"IEEE", 4096, &mut psk);
        assert_eq!(
            hex(&psk),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
    }
}
