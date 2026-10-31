//! Signing with ECDSA on P-256 (FIPS 186-5) with deterministic nonces (RFC 6979, HMAC-SHA-256),
//! in constant time: the private key and the nonce never choose a branch or a memory address.
//! The curve arithmetic uses the complete addition formulas for prime-order curves (Renes,
//! Costello and Batina, 2016), so no point is a special case.

use crate::hmac::Hmac;
use crate::sha2::HashAlgorithm;

/// The bytes of a private key and of each half of a signature.
pub const SCALAR_BYTES: usize = 32;
/// The bytes of a public key: an uncompressed SEC 1 point, `04 || x || y`.
pub const PUBLIC_KEY_BYTES: usize = 65;
/// The bytes of a signature: `r || s`, each big-endian.
pub const SIGNATURE_BYTES: usize = 64;

type Limbs = [u64; 4];

const PRIME: Limbs = [
    0xffff_ffff_ffff_ffff,
    0x0000_0000_ffff_ffff,
    0x0000_0000_0000_0000,
    0xffff_ffff_0000_0001,
];
const ORDER: Limbs = [
    0xf3b9_cac2_fc63_2551,
    0xbce6_faad_a717_9e84,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_0000_0000,
];
const B: Limbs = [
    0x3bce_3c3e_27d2_604b,
    0x651d_06b0_cc53_b0f6,
    0xb3eb_bd55_7698_86bc,
    0x5ac6_35d8_aa3a_93e7,
];
const BASE_X: Limbs = [
    0xf4a1_3945_d898_c296,
    0x7703_7d81_2deb_33a0,
    0xf8bc_e6e5_63a4_40f2,
    0x6b17_d1f2_e12c_4247,
];
const BASE_Y: Limbs = [
    0xcbb6_4068_37bf_51f5,
    0x2bce_3357_6b31_5ece,
    0x8ee7_eb4a_7c0f_9e16,
    0x4fe3_42e2_fe1a_7f9b,
];

/// A private key that is not a number from 1 to the curve's order minus 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

fn mask(bit: u64) -> u64 {
    0u64.wrapping_sub(bit & 1)
}

fn select(choice: u64, when_set: &Limbs, otherwise: &Limbs) -> Limbs {
    let all = mask(choice);
    core::array::from_fn(|index| (when_set[index] & all) | (otherwise[index] & !all))
}

fn add_limbs(a: &Limbs, b: &Limbs) -> (Limbs, u64) {
    let mut sum = [0u64; 4];
    let mut carry = 0u64;
    for index in 0..4 {
        let wide = u128::from(a[index]) + u128::from(b[index]) + u128::from(carry);
        sum[index] = wide as u64;
        carry = (wide >> 64) as u64;
    }
    (sum, carry)
}

fn subtract_limbs(a: &Limbs, b: &Limbs) -> (Limbs, u64) {
    let mut difference = [0u64; 4];
    let mut borrow = 0u64;
    for index in 0..4 {
        let (first, under_first) = a[index].overflowing_sub(b[index]);
        let (second, under_second) = first.overflowing_sub(borrow);
        difference[index] = second;
        borrow = u64::from(under_first) | u64::from(under_second);
    }
    (difference, borrow)
}

fn is_zero(a: &Limbs) -> u64 {
    let folded = a[0] | a[1] | a[2] | a[3];
    1 ^ ((folded | folded.wrapping_neg()) >> 63)
}

fn from_be(bytes: &[u8; SCALAR_BYTES]) -> Limbs {
    core::array::from_fn(|index| {
        let start = SCALAR_BYTES - 8 * (index + 1);
        let mut limb = [0u8; 8];
        limb.copy_from_slice(&bytes[start..start + 8]);
        u64::from_be_bytes(limb)
    })
}

fn to_be(limbs: &Limbs) -> [u8; SCALAR_BYTES] {
    let mut bytes = [0u8; SCALAR_BYTES];
    for (index, limb) in limbs.iter().enumerate() {
        let start = SCALAR_BYTES - 8 * (index + 1);
        bytes[start..start + 8].copy_from_slice(&limb.to_be_bytes());
    }
    bytes
}

struct Modulus {
    value: Limbs,
    inverse: u64,
    r_squared: Limbs,
}

impl Modulus {
    fn new(value: Limbs) -> Self {
        let mut inverse = 1u64;
        for _ in 0..6 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(value[0].wrapping_mul(inverse)));
        }
        let mut r_squared = [1u64, 0, 0, 0];
        for _ in 0..512 {
            let (doubled, carry) = add_limbs(&r_squared, &r_squared);
            let (reduced, borrow) = subtract_limbs(&doubled, &value);
            r_squared = select(carry | (1 ^ borrow), &reduced, &doubled);
        }
        Self {
            value,
            inverse: inverse.wrapping_neg(),
            r_squared,
        }
    }

    fn reduce_once(&self, a: &Limbs, carry: u64) -> Limbs {
        let (reduced, borrow) = subtract_limbs(a, &self.value);
        select(carry | (1 ^ borrow), &reduced, a)
    }

    fn multiply(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let mut t = [0u64; 6];
        for i in 0..4 {
            let mut carry = 0u64;
            for j in 0..4 {
                let wide =
                    u128::from(t[j]) + u128::from(a[j]) * u128::from(b[i]) + u128::from(carry);
                t[j] = wide as u64;
                carry = (wide >> 64) as u64;
            }
            let wide = u128::from(t[4]) + u128::from(carry);
            t[4] = wide as u64;
            t[5] = (wide >> 64) as u64;
            let factor = t[0].wrapping_mul(self.inverse);
            let wide = u128::from(t[0]) + u128::from(factor) * u128::from(self.value[0]);
            let mut carry = (wide >> 64) as u64;
            for j in 1..4 {
                let wide = u128::from(t[j])
                    + u128::from(factor) * u128::from(self.value[j])
                    + u128::from(carry);
                t[j - 1] = wide as u64;
                carry = (wide >> 64) as u64;
            }
            let wide = u128::from(t[4]) + u128::from(carry);
            t[3] = wide as u64;
            t[4] = t[5] + (wide >> 64) as u64;
            t[5] = 0;
        }
        self.reduce_once(&[t[0], t[1], t[2], t[3]], t[4])
    }

    fn add(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let (sum, carry) = add_limbs(a, b);
        self.reduce_once(&sum, carry)
    }

    fn subtract(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let (difference, borrow) = subtract_limbs(a, b);
        let (corrected, _) = add_limbs(&difference, &self.value);
        select(borrow, &corrected, &difference)
    }

    fn to_montgomery(&self, a: &Limbs) -> Limbs {
        self.multiply(a, &self.r_squared)
    }

    fn from_montgomery(&self, a: &Limbs) -> Limbs {
        self.multiply(a, &[1, 0, 0, 0])
    }

    fn invert(&self, a: &Limbs) -> Limbs {
        let (exponent, _) = subtract_limbs(&self.value, &[2, 0, 0, 0]);
        let mut result = self.to_montgomery(&[1, 0, 0, 0]);
        for bit in (0..256).rev() {
            result = self.multiply(&result, &result);
            if (exponent[bit / 64] >> (bit % 64)) & 1 == 1 {
                result = self.multiply(&result, a);
            }
        }
        result
    }
}

#[derive(Clone, Copy)]
struct Point {
    x: Limbs,
    y: Limbs,
    z: Limbs,
}

struct Curve {
    field: Modulus,
    b: Limbs,
}

impl Curve {
    fn new() -> Self {
        let field = Modulus::new(PRIME);
        let b = field.to_montgomery(&B);
        Self { field, b }
    }

    fn base(&self) -> Point {
        Point {
            x: self.field.to_montgomery(&BASE_X),
            y: self.field.to_montgomery(&BASE_Y),
            z: self.field.to_montgomery(&[1, 0, 0, 0]),
        }
    }

    fn identity(&self) -> Point {
        Point {
            x: [0; 4],
            y: self.field.to_montgomery(&[1, 0, 0, 0]),
            z: [0; 4],
        }
    }

    fn add(&self, p: &Point, q: &Point) -> Point {
        let f = &self.field;
        let mut t0 = f.multiply(&p.x, &q.x);
        let mut t1 = f.multiply(&p.y, &q.y);
        let mut t2 = f.multiply(&p.z, &q.z);
        let mut t3 = f.add(&p.x, &p.y);
        let mut t4 = f.add(&q.x, &q.y);
        t3 = f.multiply(&t3, &t4);
        t4 = f.add(&t0, &t1);
        t3 = f.subtract(&t3, &t4);
        t4 = f.add(&p.y, &p.z);
        let mut x3 = f.add(&q.y, &q.z);
        t4 = f.multiply(&t4, &x3);
        x3 = f.add(&t1, &t2);
        t4 = f.subtract(&t4, &x3);
        x3 = f.add(&p.x, &p.z);
        let mut y3 = f.add(&q.x, &q.z);
        x3 = f.multiply(&x3, &y3);
        y3 = f.add(&t0, &t2);
        y3 = f.subtract(&x3, &y3);
        let mut z3 = f.multiply(&self.b, &t2);
        x3 = f.subtract(&y3, &z3);
        z3 = f.add(&x3, &x3);
        x3 = f.add(&x3, &z3);
        z3 = f.subtract(&t1, &x3);
        x3 = f.add(&t1, &x3);
        y3 = f.multiply(&self.b, &y3);
        t1 = f.add(&t2, &t2);
        t2 = f.add(&t1, &t2);
        y3 = f.subtract(&y3, &t2);
        y3 = f.subtract(&y3, &t0);
        t1 = f.add(&y3, &y3);
        y3 = f.add(&t1, &y3);
        t1 = f.add(&t0, &t0);
        t0 = f.add(&t1, &t0);
        t0 = f.subtract(&t0, &t2);
        t1 = f.multiply(&t4, &y3);
        t2 = f.multiply(&t0, &y3);
        y3 = f.multiply(&x3, &z3);
        y3 = f.add(&y3, &t2);
        x3 = f.multiply(&t3, &x3);
        x3 = f.subtract(&x3, &t1);
        z3 = f.multiply(&t4, &z3);
        t1 = f.multiply(&t3, &t0);
        z3 = f.add(&z3, &t1);
        Point {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    fn multiply_base(&self, scalar: &Limbs) -> Point {
        let base = self.base();
        let mut sum = self.identity();
        for bit in (0..256).rev() {
            sum = self.add(&sum, &sum);
            let added = self.add(&sum, &base);
            let chosen = (scalar[bit / 64] >> (bit % 64)) & 1;
            sum = Point {
                x: select(chosen, &added.x, &sum.x),
                y: select(chosen, &added.y, &sum.y),
                z: select(chosen, &added.z, &sum.z),
            };
        }
        sum
    }

    fn affine(&self, point: &Point) -> (Limbs, Limbs) {
        let f = &self.field;
        let inverse = f.invert(&point.z);
        (
            f.from_montgomery(&f.multiply(&point.x, &inverse)),
            f.from_montgomery(&f.multiply(&point.y, &inverse)),
        )
    }
}

fn private_scalar(private_key: &[u8; SCALAR_BYTES]) -> Result<Limbs, InvalidKey> {
    let scalar = from_be(private_key);
    let (_, below_order) = subtract_limbs(&scalar, &ORDER);
    match is_zero(&scalar) | (1 ^ below_order) {
        0 => Ok(scalar),
        _ => Err(InvalidKey),
    }
}

/// The public key of `private_key`.
pub fn public_key(private_key: &[u8; SCALAR_BYTES]) -> Result<[u8; PUBLIC_KEY_BYTES], InvalidKey> {
    let scalar = private_scalar(private_key)?;
    let curve = Curve::new();
    let (x, y) = curve.affine(&curve.multiply_base(&scalar));
    let mut key = [0u8; PUBLIC_KEY_BYTES];
    key[0] = 4;
    key[1..33].copy_from_slice(&to_be(&x));
    key[33..].copy_from_slice(&to_be(&y));
    Ok(key)
}

fn reduced_digest(order: &Modulus, digest: &[u8]) -> Limbs {
    let mut bytes = [0u8; SCALAR_BYTES];
    let kept = digest.len().min(SCALAR_BYTES);
    bytes[SCALAR_BYTES - kept..].copy_from_slice(&digest[..kept]);
    order.reduce_once(&from_be(&bytes), 0)
}

struct Nonces {
    key: [u8; 32],
    value: [u8; 32],
}

impl Nonces {
    fn mac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = Hmac::new(HashAlgorithm::Sha256, key);
        for part in parts {
            mac.update(part);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(mac.finish().as_bytes());
        out
    }

    fn new(private_key: &[u8; SCALAR_BYTES], digest: &[u8; SCALAR_BYTES]) -> Self {
        let mut nonces = Self {
            key: [0; 32],
            value: [1; 32],
        };
        for separator in [0u8, 1] {
            nonces.key = Self::mac(
                &nonces.key,
                &[&nonces.value, &[separator], private_key, digest],
            );
            nonces.value = Self::mac(&nonces.key, &[&nonces.value]);
        }
        nonces
    }

    fn next(&mut self) -> Limbs {
        loop {
            self.value = Self::mac(&self.key, &[&self.value]);
            let candidate = from_be(&self.value);
            let (_, below_order) = subtract_limbs(&candidate, &ORDER);
            let usable = (1 ^ is_zero(&candidate)) & below_order;
            if usable == 1 {
                return candidate;
            }
            self.key = Self::mac(&self.key, &[&self.value, &[0]]);
            self.value = Self::mac(&self.key, &[&self.value]);
        }
    }

    fn retry(&mut self) {
        self.key = Self::mac(&self.key, &[&self.value, &[0]]);
        self.value = Self::mac(&self.key, &[&self.value]);
    }
}

/// The signature `r || s` by `private_key` over a message whose SHA-256 hash is `digest`,
/// with the nonce RFC 6979 derives from the key and the digest.
pub fn sign(
    private_key: &[u8; SCALAR_BYTES],
    digest: &[u8],
) -> Result<[u8; SIGNATURE_BYTES], InvalidKey> {
    let scalar = private_scalar(private_key)?;
    let curve = Curve::new();
    let order = Modulus::new(ORDER);
    let message = reduced_digest(&order, digest);
    let mut nonces = Nonces::new(private_key, &to_be(&message));
    loop {
        let nonce = nonces.next();
        let (x, _) = curve.affine(&curve.multiply_base(&nonce));
        let r = order.reduce_once(&x, 0);
        let r_montgomery = order.to_montgomery(&r);
        let sum = order.add(
            &order.to_montgomery(&message),
            &order.multiply(&r_montgomery, &order.to_montgomery(&scalar)),
        );
        let inverse = order.invert(&order.to_montgomery(&nonce));
        let s = order.from_montgomery(&order.multiply(&inverse, &sum));
        if (is_zero(&r) | is_zero(&s)) == 1 {
            nonces.retry();
            continue;
        }
        let mut signature = [0u8; SIGNATURE_BYTES];
        signature[..SCALAR_BYTES].copy_from_slice(&to_be(&r));
        signature[SCALAR_BYTES..].copy_from_slice(&to_be(&s));
        return Ok(signature);
    }
}

/// A private key from 32 bytes of the system's randomness, or `None` when they are not a
/// valid key (about once in 2^32 draws): draw again.
pub fn private_key_from(randomness: &[u8; SCALAR_BYTES]) -> Option<[u8; SCALAR_BYTES]> {
    private_scalar(randomness).ok().map(|_| *randomness)
}
