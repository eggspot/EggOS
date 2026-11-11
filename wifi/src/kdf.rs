//! IEEE 802.11's key derivation function over HMAC-SHA-256 (IEEE 802.11-2020, 12.7.1.6.2),
//! which SAE and the SHA-256 AKMs derive their keys with (D285).

use eggos_crypto::hmac::Hmac;
use eggos_crypto::HashAlgorithm;

const BLOCK_BYTES: usize = 32;

/// Fills `out` with KDF-SHA-256 of `key`, `label` and `context`, `out.len() * 8` bits long.
pub fn kdf_sha256(key: &[u8], label: &[u8], context: &[u8], out: &mut [u8]) {
    let bits = (out.len() * 8) as u16;
    for (index, chunk) in out.chunks_mut(BLOCK_BYTES).enumerate() {
        let mut mac = Hmac::new(HashAlgorithm::Sha256, key);
        mac.update(&((index + 1) as u16).to_le_bytes());
        mac.update(label);
        mac.update(context);
        mac.update(&bits.to_le_bytes());
        let digest = mac.finish();
        chunk.copy_from_slice(&digest.as_bytes()[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_outputs_extend_shorter_ones_only_through_the_length_field() {
        let mut short = [0u8; 32];
        let mut long = [0u8; 48];
        kdf_sha256(b"key", b"label", b"context", &mut short);
        kdf_sha256(b"key", b"label", b"context", &mut long);
        assert_ne!(short[..], long[..32]);
        let mut again = [0u8; 48];
        kdf_sha256(b"key", b"label", b"context", &mut again);
        assert_eq!(long, again);
    }
}
