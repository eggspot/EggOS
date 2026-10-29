//! Checking RSA signatures (RFC 8017): RSASSA-PKCS1-v1_5 and RSASSA-PSS with MGF1 over the
//! same hash and a salt as long as the hash, as TLS 1.3 and certificates use them. Only
//! public keys: nothing here signs or decrypts.

use crate::bignum::{MAX_BYTES, Modulus, Number};
use crate::sha2::{HashAlgorithm, Hasher, MAX_OUTPUT_BYTES, digest};

/// The smallest modulus accepted, in bits.
pub const MIN_MODULUS_BITS: usize = 2048;

/// An RSA public key.
#[derive(Debug, Clone, Copy)]
pub struct PublicKey {
    modulus: Modulus,
    exponent: Number,
    length: usize,
}

/// Why a key is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// The modulus is shorter than 2048 bits or longer than 4096.
    UnsupportedSize,
    /// The modulus is even or the exponent is not an odd number above 1.
    Malformed,
}

const SHA256_PREFIX: &[u8] = &[
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];
const SHA384_PREFIX: &[u8] = &[
    0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05,
    0x00, 0x04, 0x30,
];
const SHA512_PREFIX: &[u8] = &[
    0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05,
    0x00, 0x04, 0x40,
];

impl PublicKey {
    /// The key with `modulus` and `exponent`, both big-endian.
    pub fn new(modulus: &[u8], exponent: &[u8]) -> Result<Self, KeyError> {
        let value = Number::from_be_bytes(modulus).ok_or(KeyError::UnsupportedSize)?;
        let bits = value.bits();
        if !(MIN_MODULUS_BITS..=MAX_BYTES * 8).contains(&bits) {
            return Err(KeyError::UnsupportedSize);
        }
        let exponent = Number::from_be_bytes(exponent).ok_or(KeyError::Malformed)?;
        if !exponent.bit(0) || exponent.bits() < 2 {
            return Err(KeyError::Malformed);
        }
        let modulus = Modulus::new(&value).ok_or(KeyError::Malformed)?;
        Ok(Self {
            modulus,
            exponent,
            length: bits.div_ceil(8),
        })
    }

    /// The modulus's bits.
    pub fn bits(&self) -> usize {
        self.modulus.bits()
    }

    fn open(&self, signature: &[u8], out: &mut [u8; MAX_BYTES]) -> Option<usize> {
        if signature.len() != self.length {
            return None;
        }
        let value = Number::from_be_bytes(signature)?;
        if value >= *self.modulus.value() {
            return None;
        }
        let m = &self.modulus;
        let opened = m.from_montgomery(&m.power(&m.to_montgomery(&value), &self.exponent));
        opened.write_be_bytes(&mut out[..self.length]);
        Some(self.length)
    }

    /// Whether `signature` is this key's RSASSA-PKCS1-v1_5 signature of a message whose
    /// `algorithm` hash is `digest`.
    pub fn verify_pkcs1(&self, algorithm: HashAlgorithm, digest: &[u8], signature: &[u8]) -> bool {
        let mut opened = [0u8; MAX_BYTES];
        let Some(length) = self.open(signature, &mut opened) else {
            return false;
        };
        let prefix = match algorithm {
            HashAlgorithm::Sha256 => SHA256_PREFIX,
            HashAlgorithm::Sha384 => SHA384_PREFIX,
            HashAlgorithm::Sha512 => SHA512_PREFIX,
        };
        if digest.len() != algorithm.output_bytes() || length < prefix.len() + digest.len() + 11 {
            return false;
        }
        let mut expected = [0u8; MAX_BYTES];
        let padding_end = length - prefix.len() - digest.len() - 1;
        expected[1] = 1;
        expected[2..padding_end].fill(0xff);
        expected[padding_end] = 0;
        expected[padding_end + 1..padding_end + 1 + prefix.len()].copy_from_slice(prefix);
        expected[length - digest.len()..length].copy_from_slice(digest);
        opened[..length] == expected[..length]
    }

    /// Whether `signature` is this key's RSASSA-PSS signature of a message whose `algorithm`
    /// hash is `digest`, with MGF1 over the same hash and a salt as long as the hash.
    pub fn verify_pss(&self, algorithm: HashAlgorithm, digest: &[u8], signature: &[u8]) -> bool {
        let mut opened = [0u8; MAX_BYTES];
        let Some(length) = self.open(signature, &mut opened) else {
            return false;
        };
        let hash_length = algorithm.output_bytes();
        let salt_length = hash_length;
        let encoded_bits = self.bits() - 1;
        let encoded_length = encoded_bits.div_ceil(8);
        if digest.len() != hash_length || encoded_length < hash_length + salt_length + 2 {
            return false;
        }
        let skipped = length - encoded_length;
        if opened[..skipped].iter().any(|&byte| byte != 0) {
            return false;
        }
        let encoded = &mut opened[skipped..length];
        if encoded[encoded_length - 1] != 0xbc {
            return false;
        }
        let unused_bits = 8 * encoded_length - encoded_bits;
        let top_mask = 0xffu8 >> unused_bits;
        if encoded[0] & !top_mask != 0 {
            return false;
        }
        let database_length = encoded_length - hash_length - 1;
        let (database, rest) = encoded.split_at_mut(database_length);
        let hash = &rest[..hash_length];
        mask_generation(algorithm, hash, database);
        database[0] &= top_mask;
        let padding = database_length - salt_length - 1;
        if database[..padding].iter().any(|&byte| byte != 0) || database[padding] != 1 {
            return false;
        }
        let salt = &database[padding + 1..];
        let mut hasher = Hasher::new(algorithm);
        hasher.update(&[0u8; 8]);
        hasher.update(digest);
        hasher.update(salt);
        hasher.finish().as_bytes() == hash
    }
}

fn mask_generation(algorithm: HashAlgorithm, seed: &[u8], out: &mut [u8]) {
    let mut input = [0u8; MAX_OUTPUT_BYTES + 4];
    input[..seed.len()].copy_from_slice(seed);
    for (counter, chunk) in out.chunks_mut(algorithm.output_bytes()).enumerate() {
        input[seed.len()..seed.len() + 4].copy_from_slice(&(counter as u32).to_be_bytes());
        let mask = digest(algorithm, &input[..seed.len() + 4]);
        for (byte, mask) in chunk.iter_mut().zip(mask.as_bytes()) {
            *byte ^= mask;
        }
    }
}
