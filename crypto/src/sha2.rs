//! SHA-256, SHA-384 and SHA-512 (FIPS 180-4).

use core::fmt;

/// The longest digest, SHA-512's.
pub const MAX_OUTPUT_BYTES: usize = 64;
/// The longest block, SHA-384's and SHA-512's.
pub const MAX_BLOCK_BYTES: usize = 128;

/// A hash function of the SHA-2 family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    /// SHA-256: 32-byte digests over 64-byte blocks.
    Sha256,
    /// SHA-384: 48-byte digests over 128-byte blocks.
    Sha384,
    /// SHA-512: 64-byte digests over 128-byte blocks.
    Sha512,
}

impl HashAlgorithm {
    /// The bytes of a digest.
    pub const fn output_bytes(self) -> usize {
        match self {
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    /// The bytes of a block, which HMAC pads its key to.
    pub const fn block_bytes(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha384 | Self::Sha512 => 128,
        }
    }
}

/// A digest of any of the algorithms.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest {
    bytes: [u8; MAX_OUTPUT_BYTES],
    length: usize,
}

impl Digest {
    /// `bytes` as a digest; `None` when longer than [`MAX_OUTPUT_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut digest = Self {
            bytes: [0; MAX_OUTPUT_BYTES],
            length: bytes.len(),
        };
        digest.bytes.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(digest)
    }

    /// The digest's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({} bytes)", self.length)
    }
}

const SHA256_INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_ROUNDS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA512_INITIAL: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SHA384_INITIAL: [u64; 8] = [
    0xcbbb9d5dc1059ed8,
    0x629a292a367cd507,
    0x9159015a3070dd17,
    0x152fecd8f70e5939,
    0x67332667ffc00b31,
    0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7,
    0x47b5481dbefa4fa4,
];

const SHA512_ROUNDS: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

/// SHA-256's running state.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sha256")
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A hash of nothing yet.
    pub const fn new() -> Self {
        Self {
            state: SHA256_INITIAL,
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    /// Hashes `data` after what came before.
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                let block = self.block;
                self.compress(&block);
                self.filled = 0;
            }
        }
    }

    /// The digest of everything hashed.
    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *bytes = word.to_be_bytes();
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0u32; 64];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for t in 16..64 {
            let s0 = schedule[t - 15].rotate_right(7)
                ^ schedule[t - 15].rotate_right(18)
                ^ (schedule[t - 15] >> 3);
            let s1 = schedule[t - 2].rotate_right(17)
                ^ schedule[t - 2].rotate_right(19)
                ^ (schedule[t - 2] >> 10);
            schedule[t] = schedule[t - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[t - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for t in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(SHA256_ROUNDS[t])
                .wrapping_add(schedule[t]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (word, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(value);
        }
    }
}

/// SHA-512's running state, which SHA-384 shares with other initial values.
#[derive(Clone)]
pub struct Sha512 {
    state: [u64; 8],
    block: [u8; 128],
    filled: usize,
    length: u128,
}

impl fmt::Debug for Sha512 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sha512")
    }
}

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha512 {
    /// A SHA-512 hash of nothing yet.
    pub const fn new() -> Self {
        Self::starting_at(SHA512_INITIAL)
    }

    /// A SHA-384 hash of nothing yet: its digest is the first 48 bytes of [`Sha512::finish`].
    pub const fn new_384() -> Self {
        Self::starting_at(SHA384_INITIAL)
    }

    const fn starting_at(state: [u64; 8]) -> Self {
        Self {
            state,
            block: [0; 128],
            filled: 0,
            length: 0,
        }
    }

    /// Hashes `data` after what came before.
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u128);
        while !data.is_empty() {
            let take = (128 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 128 {
                let block = self.block;
                self.compress(&block);
                self.filled = 0;
            }
        }
    }

    /// The 64 bytes of the final state.
    pub fn finish(mut self) -> [u8; 64] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 112 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 64];
        for (bytes, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(self.state) {
            *bytes = word.to_be_bytes();
        }
        out
    }

    fn compress(&mut self, block: &[u8; 128]) {
        let mut schedule = [0u64; 80];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<8>().0) {
            *word = u64::from_be_bytes(*bytes);
        }
        for t in 16..80 {
            let s0 = schedule[t - 15].rotate_right(1)
                ^ schedule[t - 15].rotate_right(8)
                ^ (schedule[t - 15] >> 7);
            let s1 = schedule[t - 2].rotate_right(19)
                ^ schedule[t - 2].rotate_right(61)
                ^ (schedule[t - 2] >> 6);
            schedule[t] = schedule[t - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[t - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for t in 0..80 {
            let sum1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let choice = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(SHA512_ROUNDS[t])
                .wrapping_add(schedule[t]);
            let sum0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (word, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(value);
        }
    }
}

/// A running hash of any of the algorithms.
#[derive(Debug, Clone)]
pub enum Hasher {
    /// SHA-256.
    Sha256(Sha256),
    /// SHA-384.
    Sha384(Sha512),
    /// SHA-512.
    Sha512(Sha512),
}

impl Hasher {
    /// A hash of nothing yet with `algorithm`.
    pub const fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
            HashAlgorithm::Sha384 => Self::Sha384(Sha512::new_384()),
            HashAlgorithm::Sha512 => Self::Sha512(Sha512::new()),
        }
    }

    /// The algorithm.
    pub const fn algorithm(&self) -> HashAlgorithm {
        match self {
            Self::Sha256(_) => HashAlgorithm::Sha256,
            Self::Sha384(_) => HashAlgorithm::Sha384,
            Self::Sha512(_) => HashAlgorithm::Sha512,
        }
    }

    /// Hashes `data` after what came before.
    pub fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha256(state) => state.update(data),
            Self::Sha384(state) | Self::Sha512(state) => state.update(data),
        }
    }

    /// The digest of everything hashed.
    pub fn finish(self) -> Digest {
        let algorithm = self.algorithm();
        let mut digest = Digest {
            bytes: [0; MAX_OUTPUT_BYTES],
            length: algorithm.output_bytes(),
        };
        match self {
            Self::Sha256(state) => digest.bytes[..32].copy_from_slice(&state.finish()),
            Self::Sha384(state) | Self::Sha512(state) => digest.bytes = state.finish(),
        }
        if digest.length < MAX_OUTPUT_BYTES {
            digest.bytes[digest.length..].fill(0);
        }
        digest
    }
}

/// The digest of `data` with `algorithm`.
pub fn digest(algorithm: HashAlgorithm, data: &[u8]) -> Digest {
    let mut hasher = Hasher::new(algorithm);
    hasher.update(data);
    hasher.finish()
}
