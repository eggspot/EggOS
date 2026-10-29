//! AES in Galois/Counter Mode (NIST SP 800-38D) with 96-bit nonces and 16-byte tags, as TLS
//! uses it. GHASH multiplies bit by bit with masks, so its time does not depend on the hash
//! key or the data.

use crate::AuthenticationFailed;
use crate::aes::{Aes, BLOCK_BYTES, BadKeyLength};
use crate::constant_time::{equal, mask_u128};
use core::fmt;

/// The bytes of a nonce.
pub const NONCE_BYTES: usize = 12;
/// The bytes of a tag.
pub const TAG_BYTES: usize = 16;

const REDUCTION: u128 = 0xe1 << 120;

/// AES-GCM with one key.
#[derive(Clone)]
pub struct AesGcm {
    aes: Aes,
    hash_key: u128,
}

impl fmt::Debug for AesGcm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AesGcm")
    }
}

impl AesGcm {
    /// AES-GCM with `key`: 16 bytes for AES-128-GCM, 32 for AES-256-GCM.
    pub fn new(key: &[u8]) -> Result<Self, BadKeyLength> {
        let aes = Aes::new(key)?;
        let mut zero = [0u8; BLOCK_BYTES];
        aes.encrypt_block(&mut zero);
        Ok(Self {
            aes,
            hash_key: u128::from_be_bytes(zero),
        })
    }

    /// Encrypts `data` in place and returns the tag over `additional` data and the ciphertext.
    pub fn seal(
        &self,
        nonce: &[u8; NONCE_BYTES],
        additional: &[u8],
        data: &mut [u8],
    ) -> [u8; TAG_BYTES] {
        self.apply_key_stream(nonce, data);
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
        self.apply_key_stream(nonce, data);
        Ok(())
    }

    fn apply_key_stream(&self, nonce: &[u8; NONCE_BYTES], data: &mut [u8]) {
        for (index, chunk) in data.chunks_mut(BLOCK_BYTES).enumerate() {
            let mut stream = counter_block(nonce, (index as u32).wrapping_add(2));
            self.aes.encrypt_block(&mut stream);
            for (byte, key) in chunk.iter_mut().zip(stream) {
                *byte ^= key;
            }
        }
    }

    fn tag(&self, nonce: &[u8; NONCE_BYTES], additional: &[u8], ciphertext: &[u8]) -> [u8; 16] {
        let mut hash = 0u128;
        for data in [additional, ciphertext] {
            for chunk in data.chunks(BLOCK_BYTES) {
                let mut block = [0u8; BLOCK_BYTES];
                block[..chunk.len()].copy_from_slice(chunk);
                hash = multiply(hash ^ u128::from_be_bytes(block), self.hash_key);
            }
        }
        let lengths = ((additional.len() as u128 * 8) << 64) | (ciphertext.len() as u128 * 8);
        hash = multiply(hash ^ lengths, self.hash_key);
        let mut first = counter_block(nonce, 1);
        self.aes.encrypt_block(&mut first);
        (u128::from_be_bytes(first) ^ hash).to_be_bytes()
    }
}

fn counter_block(nonce: &[u8; NONCE_BYTES], counter: u32) -> [u8; BLOCK_BYTES] {
    let mut block = [0u8; BLOCK_BYTES];
    block[..NONCE_BYTES].copy_from_slice(nonce);
    block[NONCE_BYTES..].copy_from_slice(&counter.to_be_bytes());
    block
}

/// The product of `x` and `y` in GF(2^128) as GCM defines it (bit 0 the most significant).
pub(crate) fn multiply(x: u128, y: u128) -> u128 {
    let mut product = 0u128;
    let mut shifted = y;
    for bit in (0..128).rev() {
        product ^= shifted & mask_u128(x >> bit);
        let carry = mask_u128(shifted);
        shifted = (shifted >> 1) ^ (REDUCTION & carry);
    }
    product
}
