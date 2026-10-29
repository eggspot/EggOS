//! X25519 (RFC 7748): Diffie-Hellman on Curve25519 with the Montgomery ladder. The field
//! mod 2^255 - 19 is held in five 51-bit limbs; the ladder swaps with masks and runs the same
//! steps whatever the scalar, so its time does not depend on the secret.

use crate::constant_time::mask_u64;

/// The bytes of a scalar, a public key and a shared secret.
pub const KEY_BYTES: usize = 32;

const MASK: u64 = (1 << 51) - 1;
const BASE_POINT: [u8; KEY_BYTES] = {
    let mut point = [0u8; KEY_BYTES];
    point[0] = 9;
    point
};

#[derive(Clone, Copy)]
struct Element([u64; 5]);

fn load(bytes: &[u8; KEY_BYTES], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

impl Element {
    const ZERO: Self = Self([0; 5]);
    const ONE: Self = Self([1, 0, 0, 0, 0]);

    fn from_bytes(bytes: &[u8; KEY_BYTES]) -> Self {
        Self([
            load(bytes, 0) & MASK,
            (load(bytes, 6) >> 3) & MASK,
            (load(bytes, 12) >> 6) & MASK,
            (load(bytes, 19) >> 1) & MASK,
            (load(bytes, 24) >> 12) & MASK,
        ])
    }

    fn carried(mut limbs: [u64; 5]) -> Self {
        for index in 0..4 {
            limbs[index + 1] += limbs[index] >> 51;
            limbs[index] &= MASK;
        }
        let carry = limbs[4] >> 51;
        limbs[4] &= MASK;
        limbs[0] += carry * 19;
        limbs[1] += limbs[0] >> 51;
        limbs[0] &= MASK;
        Self(limbs)
    }

    fn to_bytes(self) -> [u8; KEY_BYTES] {
        let mut limbs = Self::carried(Self::carried(self.0).0).0;
        let mut at_least_prime = (limbs[0] + 19) >> 51;
        for limb in &limbs[1..] {
            at_least_prime = (limb + at_least_prime) >> 51;
        }
        limbs[0] += 19 * at_least_prime;
        for index in 0..4 {
            limbs[index + 1] += limbs[index] >> 51;
            limbs[index] &= MASK;
        }
        limbs[4] &= MASK;
        let words = [
            limbs[0] | (limbs[1] << 51),
            (limbs[1] >> 13) | (limbs[2] << 38),
            (limbs[2] >> 26) | (limbs[3] << 25),
            (limbs[3] >> 39) | (limbs[4] << 12),
        ];
        let mut bytes = [0u8; KEY_BYTES];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        bytes
    }

    fn add(self, other: Self) -> Self {
        let mut limbs = self.0;
        for (limb, &added) in limbs.iter_mut().zip(&other.0) {
            *limb += added;
        }
        Self::carried(limbs)
    }

    fn subtract(self, other: Self) -> Self {
        let twice_prime = [
            0xf_ffff_ffff_ffda,
            0xf_ffff_ffff_fffe,
            0xf_ffff_ffff_fffe,
            0xf_ffff_ffff_fffe,
            0xf_ffff_ffff_fffe,
        ];
        let mut limbs = [0u64; 5];
        for index in 0..5 {
            limbs[index] = self.0[index] + twice_prime[index] - other.0[index];
        }
        Self::carried(limbs)
    }

    fn multiply(self, other: Self) -> Self {
        let [a0, a1, a2, a3, a4] = self.0.map(u128::from);
        let [b0, b1, b2, b3, b4] = other.0.map(u128::from);
        let (b1n, b2n, b3n, b4n) = (b1 * 19, b2 * 19, b3 * 19, b4 * 19);
        let r0 = a0 * b0 + a1 * b4n + a2 * b3n + a3 * b2n + a4 * b1n;
        let r1 = a0 * b1 + a1 * b0 + a2 * b4n + a3 * b3n + a4 * b2n;
        let r2 = a0 * b2 + a1 * b1 + a2 * b0 + a3 * b4n + a4 * b3n;
        let r3 = a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * b4n;
        let r4 = a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;
        Self::carried_wide([r0, r1, r2, r3, r4])
    }

    fn carried_wide(mut wide: [u128; 5]) -> Self {
        for index in 0..4 {
            wide[index + 1] += wide[index] >> 51;
            wide[index] &= u128::from(MASK);
        }
        let carry = (wide[4] >> 51) as u64;
        let mut limbs = wide.map(|limb| (limb & u128::from(MASK)) as u64);
        limbs[0] += carry * 19;
        limbs[1] += limbs[0] >> 51;
        limbs[0] &= MASK;
        Self(limbs)
    }

    fn square(self) -> Self {
        self.multiply(self)
    }

    fn multiply_small(self, factor: u64) -> Self {
        Self::carried_wide(self.0.map(|limb| u128::from(limb) * u128::from(factor)))
    }

    fn invert(self) -> Self {
        let exponent: [u8; KEY_BYTES] = {
            let mut bytes = [0xff; KEY_BYTES];
            bytes[0] = 0xeb;
            bytes[31] = 0x7f;
            bytes
        };
        let mut result = Self::ONE;
        for bit in (0..255).rev() {
            result = result.square();
            if (exponent[bit / 8] >> (bit % 8)) & 1 == 1 {
                result = result.multiply(self);
            }
        }
        result
    }
}

fn swap(condition: u64, a: &mut Element, b: &mut Element) {
    let mask = mask_u64(condition);
    for (x, y) in a.0.iter_mut().zip(b.0.iter_mut()) {
        let difference = mask & (*x ^ *y);
        *x ^= difference;
        *y ^= difference;
    }
}

/// The X25519 function: `scalar` (clamped as RFC 7748 says) times the point whose
/// u-coordinate is `u`.
pub fn x25519(scalar: &[u8; KEY_BYTES], u: &[u8; KEY_BYTES]) -> [u8; KEY_BYTES] {
    let mut clamped = *scalar;
    clamped[0] &= 248;
    clamped[31] &= 127;
    clamped[31] |= 64;
    let x1 = Element::from_bytes(u);
    let (mut x2, mut z2, mut x3, mut z3) = (Element::ONE, Element::ZERO, x1, Element::ONE);
    let mut swapped = 0u64;
    for bit in (0..255).rev() {
        let current = u64::from((clamped[bit / 8] >> (bit % 8)) & 1);
        swapped ^= current;
        swap(swapped, &mut x2, &mut x3);
        swap(swapped, &mut z2, &mut z3);
        swapped = current;
        let a = x2.add(z2);
        let aa = a.square();
        let b = x2.subtract(z2);
        let bb = b.square();
        let e = aa.subtract(bb);
        let c = x3.add(z3);
        let d = x3.subtract(z3);
        let da = d.multiply(a);
        let cb = c.multiply(b);
        x3 = da.add(cb).square();
        z3 = x1.multiply(da.subtract(cb).square());
        x2 = aa.multiply(bb);
        z2 = e.multiply(aa.add(e.multiply_small(121_665)));
    }
    swap(swapped, &mut x2, &mut x3);
    swap(swapped, &mut z2, &mut z3);
    x2.multiply(z2.invert()).to_bytes()
}

/// The public key of the private `scalar`: `scalar` times the base point, u = 9.
pub fn public_key(scalar: &[u8; KEY_BYTES]) -> [u8; KEY_BYTES] {
    x25519(scalar, &BASE_POINT)
}
