//! The ChaCha20-Poly1305 AEAD (RFC 8439, section 2.8).

use crate::AuthenticationFailed;
use crate::chacha20::{self, KEY_BYTES, NONCE_BYTES};
use crate::constant_time::equal;
use crate::poly1305::{self, Poly1305, TAG_BYTES};
use core::fmt;

/// ChaCha20-Poly1305 with one key.
#[derive(Clone)]
pub struct ChaCha20Poly1305 {
    key: [u8; KEY_BYTES],
}

impl fmt::Debug for ChaCha20Poly1305 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChaCha20Poly1305")
    }
}

impl ChaCha20Poly1305 {
    /// The AEAD with `key`.
    pub fn new(key: &[u8; KEY_BYTES]) -> Self {
        Self { key: *key }
    }

    /// Encrypts `data` in place and returns the tag over `additional` data and the ciphertext.
    pub fn seal(
        &self,
        nonce: &[u8; NONCE_BYTES],
        additional: &[u8],
        data: &mut [u8],
    ) -> [u8; TAG_BYTES] {
        chacha20::apply_key_stream(&self.key, 1, nonce, data);
        self.tag(nonce, additional, data)
    }

    /// Checks `tag` over `additional` data and the ciphertext in `data`, then decrypts it in
    /// place; a tag that does not match leaves `data` as it was.
    pub fn open(
        &self,
        nonce: &[u8; NONCE_BYTES],
        additional: &[u8],
        data: &mut [u8],
        tag: &[u8],
    ) -> Result<(), AuthenticationFailed> {
        let expected = self.tag(nonce, additional, data);
        if !equal(&expected, tag) {
            return Err(AuthenticationFailed);
        }
        chacha20::apply_key_stream(&self.key, 1, nonce, data);
        Ok(())
    }

    fn tag(
        &self,
        nonce: &[u8; NONCE_BYTES],
        additional: &[u8],
        ciphertext: &[u8],
    ) -> [u8; TAG_BYTES] {
        let block = chacha20::block(&self.key, 0, nonce);
        let mut one_time = [0u8; poly1305::KEY_BYTES];
        one_time.copy_from_slice(&block[..poly1305::KEY_BYTES]);
        let mut mac = Poly1305::new(&one_time);
        let zeros = [0u8; 16];
        for data in [additional, ciphertext] {
            mac.update(data);
            let padding = (16 - data.len() % 16) % 16;
            mac.update(&zeros[..padding]);
        }
        mac.update(&(additional.len() as u64).to_le_bytes());
        mac.update(&(ciphertext.len() as u64).to_le_bytes());
        mac.finish()
    }
}
