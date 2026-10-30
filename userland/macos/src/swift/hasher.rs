//! Swift's `Hasher` (D180): SipHash-1-3 over a 72-byte state the size of Swift's (a tail buffer
//! and eight words, four of them reserved), keyed by the process's execution seed and the
//! hasher's own seed. Programs only see the state's size: every operation is a runtime call, so
//! the layout inside is the runtime's own. Pure logic with host tests.

const INITIAL: [u64; 4] = [
    0x736f_6d65_7073_6575,
    0x646f_7261_6e64_6f6d,
    0x6c79_6765_6e65_7261,
    0x7465_6462_7974_6573,
];
const PENDING_SHIFT: u32 = 56;
const FINALIZATION_ROUNDS: usize = 3;

/// A hasher's state, laid out as Swift's `Hasher` is sized.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasherState {
    tail: u64,
    v: [u64; 4],
    _reserved: [u64; 4],
}

fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13) ^ v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16) ^ v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21) ^ v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17) ^ v[2];
    v[2] = v[2].rotate_left(32);
}

impl HasherState {
    /// A hasher keyed by the execution seed `key` and the hasher's `seed`.
    pub fn new(key: [u64; 2], seed: u64) -> Self {
        let (k0, k1) = (key[0], key[1] ^ seed);
        Self {
            tail: 0,
            v: [
                INITIAL[0] ^ k0,
                INITIAL[1] ^ k1,
                INITIAL[2] ^ k0,
                INITIAL[3] ^ k1,
            ],
            _reserved: [0; 4],
        }
    }

    fn compress(&mut self, block: u64) {
        self.v[3] ^= block;
        round(&mut self.v);
        self.v[0] ^= block;
    }

    /// Feeds `bytes`, in order.
    pub fn combine_bytes(&mut self, bytes: &[u8]) {
        let total = (self.tail >> PENDING_SHIFT) as u8;
        let mut pending = u32::from(total % 8);
        let mut buffer = self.tail & ((1u64 << PENDING_SHIFT) - 1);
        let mut rest = bytes;
        if pending == 0 {
            while rest.len() >= 8 {
                let (block, after) = rest.split_at(8);
                self.compress(u64::from_le_bytes(block.try_into().unwrap_or([0; 8])));
                rest = after;
            }
        }
        for &byte in rest {
            buffer |= u64::from(byte) << (8 * pending);
            pending += 1;
            if pending == 8 {
                self.compress(buffer);
                buffer = 0;
                pending = 0;
            }
        }
        let total = total.wrapping_add(bytes.len() as u8);
        self.tail = buffer | (u64::from(total) << PENDING_SHIFT);
    }

    /// Feeds the 8 bytes of `value`, little-endian.
    pub fn combine_u64(&mut self, value: u64) {
        self.combine_bytes(&value.to_le_bytes());
    }

    /// The hash of everything fed: SipHash-1-3's three finishing rounds.
    pub fn finalize(&mut self) -> u64 {
        let total = self.tail >> PENDING_SHIFT;
        let pending = self.tail & ((1u64 << PENDING_SHIFT) - 1);
        let last = pending | (total << PENDING_SHIFT);
        self.compress(last);
        self.v[2] ^= 0xff;
        for _ in 0..FINALIZATION_ROUNDS {
            round(&mut self.v);
        }
        self.v[0] ^ self.v[1] ^ self.v[2] ^ self.v[3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(seed: u64, parts: &[&[u8]]) -> u64 {
        let mut state = HasherState::new([1, 2], seed);
        for part in parts {
            state.combine_bytes(part);
        }
        state.finalize()
    }

    #[test]
    fn the_state_is_as_large_as_swifts_hasher() {
        assert_eq!(core::mem::size_of::<HasherState>(), 72);
    }

    #[test]
    fn how_bytes_are_split_does_not_change_the_hash() {
        let whole = hash(7, &[b"the quick brown fox jumps"]);
        assert_eq!(whole, hash(7, &[b"the", b" quick brown", b" fox jumps"]));
        assert_eq!(
            whole,
            hash(7, &[b"t", b"he quick", b" brown fox ", b"jumps"])
        );
        let mut state = HasherState::new([1, 2], 7);
        state.combine_u64(u64::from_le_bytes(*b"12345678"));
        assert_eq!(state.finalize(), hash(7, &[b"1234", b"5678"]));
    }

    #[test]
    fn contents_order_seed_and_key_all_matter() {
        let base = hash(7, &[b"ab"]);
        assert_ne!(base, hash(7, &[b"ba"]));
        assert_ne!(base, hash(8, &[b"ab"]));
        assert_ne!(base, hash(7, &[b"ab\0"]));
        let mut other_key = HasherState::new([3, 2], 7);
        other_key.combine_bytes(b"ab");
        assert_ne!(base, other_key.finalize());
        assert_eq!(base, hash(7, &[b"a", b"b"]));
    }
}
