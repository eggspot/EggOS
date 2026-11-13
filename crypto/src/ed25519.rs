//! Checking Ed25519 signatures (RFC 8032 5.1.7, D573): the twisted Edwards curve
//! −x² + y² = 1 + d·x²·y² over 2^255 − 19 in extended coordinates (Hisil, Wong, Carter and
//! Dawson's unified addition, which doubles as well), over [`crate::bignum`]'s Montgomery
//! arithmetic. Only public values: nothing here signs.

use crate::bignum::{Modulus, Number};
use crate::sha2::Sha512;

/// The bytes of a public key.
pub const PUBLIC_KEY_BYTES: usize = 32;
/// The bytes of a signature.
pub const SIGNATURE_BYTES: usize = 64;

const PRIME: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xed,
];
const ORDER: [u8; 32] = [
    0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x14, 0xde, 0xf9, 0xde, 0xa2, 0xf7, 0x9c, 0xd6, 0x58, 0x12, 0x63, 0x1a, 0x5c, 0xf5, 0xd3, 0xed,
];
const D: [u8; 32] = [
    0x52, 0x03, 0x6c, 0xee, 0x2b, 0x6f, 0xfe, 0x73, 0x8c, 0xc7, 0x40, 0x79, 0x77, 0x79, 0xe8, 0x98,
    0x00, 0x70, 0x0a, 0x4d, 0x41, 0x41, 0xd8, 0xab, 0x75, 0xeb, 0x4d, 0xca, 0x13, 0x59, 0x78, 0xa3,
];
const SQRT_MINUS_ONE: [u8; 32] = [
    0x2b, 0x83, 0x24, 0x80, 0x4f, 0xc1, 0xdf, 0x0b, 0x2b, 0x4d, 0x00, 0x99, 0x3d, 0xfb, 0xd7, 0xa7,
    0x2f, 0x43, 0x18, 0x06, 0xad, 0x2f, 0xe4, 0x78, 0xc4, 0xee, 0x1b, 0x27, 0x4a, 0x0e, 0xa0, 0xb0,
];
const BASE_X: [u8; 32] = [
    0x21, 0x69, 0x36, 0xd3, 0xcd, 0x6e, 0x53, 0xfe, 0xc0, 0xa4, 0xe2, 0x31, 0xfd, 0xd6, 0xdc, 0x5c,
    0x69, 0x2c, 0xc7, 0x60, 0x95, 0x25, 0xa7, 0xb2, 0xc9, 0x56, 0x2d, 0x60, 0x8f, 0x25, 0xd5, 0x1a,
];
const BASE_Y: [u8; 32] = [
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x58,
];

#[derive(Clone, Copy)]
struct Point {
    x: Number,
    y: Number,
    z: Number,
    t: Number,
}

struct Field {
    modulus: Modulus,
    d2: Number,
}

fn number(bytes: &[u8]) -> Number {
    Number::from_be_bytes(bytes).unwrap_or(Number::ZERO)
}

fn little_endian(bytes: &[u8]) -> Number {
    let mut reversed = [0u8; 64];
    let length = bytes.len().min(64);
    for (index, &byte) in bytes[..length].iter().enumerate() {
        reversed[length - 1 - index] = byte;
    }
    number(&reversed[..length])
}

impl Field {
    fn identity(&self) -> Point {
        let one = self.modulus.one();
        Point {
            x: Number::ZERO,
            y: one,
            z: one,
            t: Number::ZERO,
        }
    }

    fn add(&self, p: &Point, q: &Point) -> Point {
        let m = &self.modulus;
        let a = m.multiply(&m.subtract(&p.y, &p.x), &m.subtract(&q.y, &q.x));
        let b = m.multiply(&m.add(&p.y, &p.x), &m.add(&q.y, &q.x));
        let c = m.multiply(&m.multiply(&p.t, &self.d2), &q.t);
        let zz = m.multiply(&p.z, &q.z);
        let d = m.add(&zz, &zz);
        let e = m.subtract(&b, &a);
        let f = m.subtract(&d, &c);
        let g = m.add(&d, &c);
        let h = m.add(&b, &a);
        Point {
            x: m.multiply(&e, &f),
            y: m.multiply(&g, &h),
            z: m.multiply(&f, &g),
            t: m.multiply(&e, &h),
        }
    }

    fn multiply(&self, point: &Point, scalar: &Number) -> Point {
        let mut sum = self.identity();
        for bit in (0..scalar.bits()).rev() {
            sum = self.add(&sum, &sum);
            if scalar.bit(bit) {
                sum = self.add(&sum, point);
            }
        }
        sum
    }

    fn same(&self, p: &Point, q: &Point) -> bool {
        let m = &self.modulus;
        m.multiply(&p.x, &q.z) == m.multiply(&q.x, &p.z)
            && m.multiply(&p.y, &q.z) == m.multiply(&q.y, &p.z)
    }

    /// The point a 32-byte encoding names (RFC 8032 5.1.3), in Montgomery form; `None` for
    /// one that is no point.
    fn decode(&self, encoded: &[u8; 32]) -> Option<Point> {
        let m = &self.modulus;
        let sign = encoded[31] >> 7;
        let mut bytes = *encoded;
        bytes[31] &= 0x7f;
        let y_plain = little_endian(&bytes);
        if y_plain >= *m.value() {
            return None;
        }
        let one = m.one();
        let y = m.to_montgomery(&y_plain);
        let y2 = m.multiply(&y, &y);
        let u = m.subtract(&y2, &one);
        let d = m.to_montgomery(&number(&D));
        let v = m.add(&m.multiply(&d, &y2), &one);
        let v3 = m.multiply(&m.multiply(&v, &v), &v);
        let v7 = m.multiply(&m.multiply(&v3, &v3), &v);
        let exponent = number(&PRIME).shift_right(3);
        let candidate = m.multiply(&m.multiply(&u, &v3), &self.power(&m.multiply(&u, &v7), &exponent));
        let check = m.multiply(&v, &m.multiply(&candidate, &candidate));
        let x = if check == u {
            candidate
        } else if check == m.subtract(&Number::ZERO, &u) {
            m.multiply(&candidate, &m.to_montgomery(&number(&SQRT_MINUS_ONE)))
        } else {
            return None;
        };
        let x_plain = m.from_montgomery(&x);
        if x_plain.is_zero() && sign == 1 {
            return None;
        }
        let x = match u8::from(x_plain.bit(0)) == sign {
            true => x,
            false => m.subtract(&Number::ZERO, &x),
        };
        Some(Point {
            x,
            y,
            z: one,
            t: m.multiply(&x, &y),
        })
    }

    fn power(&self, base: &Number, exponent: &Number) -> Number {
        let m = &self.modulus;
        let mut result = m.one();
        for bit in (0..exponent.bits()).rev() {
            result = m.multiply(&result, &result);
            if exponent.bit(bit) {
                result = m.multiply(&result, base);
            }
        }
        result
    }
}

/// Whether `signature` is a valid Ed25519 signature of `message` by `public_key` (RFC 8032
/// 5.1.7, the cofactorless check `[S]B = R + [k]A`).
pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (
        <&[u8; PUBLIC_KEY_BYTES]>::try_from(public_key),
        <&[u8; SIGNATURE_BYTES]>::try_from(signature),
    ) else {
        return false;
    };
    let (Some(modulus), Some(scalars)) = (Modulus::new(&number(&PRIME)), Modulus::new(&number(&ORDER)))
    else {
        return false;
    };
    let d = modulus.to_montgomery(&number(&D));
    let field = Field {
        d2: modulus.add(&d, &d),
        modulus,
    };
    let r_bytes: &[u8; 32] = signature[..32].try_into().unwrap_or(&[0; 32]);
    let s = little_endian(&signature[32..]);
    if s >= number(&ORDER) {
        return false;
    }
    let (Some(a), Some(r)) = (field.decode(key), field.decode(r_bytes)) else {
        return false;
    };
    let mut hash = Sha512::new();
    hash.update(r_bytes);
    hash.update(key);
    hash.update(message);
    let digest = hash.finish();
    let k = scalars.reduce(&little_endian(digest.as_ref()));
    let m = &field.modulus;
    let base_x = m.to_montgomery(&number(&BASE_X));
    let base_y = m.to_montgomery(&number(&BASE_Y));
    let base = Point {
        x: base_x,
        y: base_y,
        z: m.one(),
        t: m.multiply(&base_x, &base_y),
    };
    let left = field.multiply(&base, &s);
    let right = field.add(&r, &field.multiply(&a, &k));
    field.same(&left, &right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rfc_8032_s_first_test_vector_checks_and_a_changed_one_does_not() {
        let key = hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let signature = hex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );
        assert!(verify(&key, b"", &signature));
        let mut changed = signature.clone();
        changed[0] ^= 1;
        assert!(!verify(&key, b"", &changed));
        assert!(!verify(&key, b"x", &signature));
    }
}
