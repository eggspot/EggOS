//! HMAC (RFC 2104) and HKDF (RFC 5869) over the SHA-2 hashes.

use crate::sha2::{Digest, HashAlgorithm, Hasher, MAX_BLOCK_BYTES, digest};

/// A running HMAC.
#[derive(Debug, Clone)]
pub struct Hmac {
    inner: Hasher,
    outer: Hasher,
}

impl Hmac {
    /// An HMAC with `algorithm` keyed by `key` (hashed first when longer than a block).
    pub fn new(algorithm: HashAlgorithm, key: &[u8]) -> Self {
        let block_bytes = algorithm.block_bytes();
        let mut padded = [0u8; MAX_BLOCK_BYTES];
        if key.len() > block_bytes {
            let hashed = digest(algorithm, key);
            padded[..hashed.as_bytes().len()].copy_from_slice(hashed.as_bytes());
        } else {
            padded[..key.len()].copy_from_slice(key);
        }
        let mut inner = Hasher::new(algorithm);
        let mut outer = Hasher::new(algorithm);
        let mut inner_pad = [0u8; MAX_BLOCK_BYTES];
        let mut outer_pad = [0u8; MAX_BLOCK_BYTES];
        for index in 0..block_bytes {
            inner_pad[index] = padded[index] ^ 0x36;
            outer_pad[index] = padded[index] ^ 0x5c;
        }
        inner.update(&inner_pad[..block_bytes]);
        outer.update(&outer_pad[..block_bytes]);
        Self { inner, outer }
    }

    /// MACs `data` after what came before.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// The MAC of everything given.
    pub fn finish(self) -> Digest {
        let inner = self.inner.finish();
        let mut outer = self.outer;
        outer.update(inner.as_bytes());
        outer.finish()
    }
}

/// The HMAC of `data` keyed by `key`.
pub fn hmac(algorithm: HashAlgorithm, key: &[u8], data: &[u8]) -> Digest {
    let mut mac = Hmac::new(algorithm, key);
    mac.update(data);
    mac.finish()
}

/// HKDF-Extract: a pseudorandom key from `input` keying material and a `salt` (a string of
/// zeros of the hash's length when empty, as HMAC pads an empty key to).
pub fn extract(algorithm: HashAlgorithm, salt: &[u8], input: &[u8]) -> Digest {
    hmac(algorithm, salt, input)
}

/// More output than HKDF-Expand gives: 255 times the hash's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputTooLong;

/// HKDF-Expand: fills `out` from the pseudorandom key `key` and the info, given in `info`'s
/// pieces, which are concatenated.
pub fn expand(
    algorithm: HashAlgorithm,
    key: &[u8],
    info: &[&[u8]],
    out: &mut [u8],
) -> Result<(), OutputTooLong> {
    let size = algorithm.output_bytes();
    if out.len() > 255 * size {
        return Err(OutputTooLong);
    }
    let mut previous: Option<Digest> = None;
    for (index, chunk) in out.chunks_mut(size).enumerate() {
        let mut mac = Hmac::new(algorithm, key);
        if let Some(previous) = &previous {
            mac.update(previous.as_bytes());
        }
        for piece in info {
            mac.update(piece);
        }
        mac.update(&[(index + 1) as u8]);
        let block = mac.finish();
        chunk.copy_from_slice(&block.as_bytes()[..chunk.len()]);
        previous = Some(block);
    }
    Ok(())
}
