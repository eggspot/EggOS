//! Checking ECDSA signatures (FIPS 186-5, SEC 1) on the NIST curves P-256 and P-384, in
//! Jacobian coordinates over [`crate::bignum`]'s Montgomery arithmetic. Only public values:
//! nothing here signs.

use crate::bignum::{Modulus, Number};

/// A curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// P-256 (secp256r1).
    P256,
    /// P-384 (secp384r1).
    P384,
}

struct Parameters {
    prime: &'static [u8],
    b: &'static [u8],
    order: &'static [u8],
    x: &'static [u8],
    y: &'static [u8],
}

const P256: Parameters = Parameters {
    prime: &[
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff,
    ],
    b: &[
        0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86,
        0xbc, 0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6, 0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2,
        0x60, 0x4b,
    ],
    order: &[
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63,
        0x25, 0x51,
    ],
    x: &[
        0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40,
        0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98,
        0xc2, 0x96,
    ],
    y: &[
        0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e,
        0x16, 0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf,
        0x51, 0xf5,
    ],
};

const P384: Parameters = Parameters {
    prime: &[
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xfe, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
        0xff, 0xff, 0xff,
    ],
    b: &[
        0xb3, 0x31, 0x2f, 0xa7, 0xe2, 0x3e, 0xe7, 0xe4, 0x98, 0x8e, 0x05, 0x6b, 0xe3, 0xf8, 0x2d,
        0x19, 0x18, 0x1d, 0x9c, 0x6e, 0xfe, 0x81, 0x41, 0x12, 0x03, 0x14, 0x08, 0x8f, 0x50, 0x13,
        0x87, 0x5a, 0xc6, 0x56, 0x39, 0x8d, 0x8a, 0x2e, 0xd1, 0x9d, 0x2a, 0x85, 0xc8, 0xed, 0xd3,
        0xec, 0x2a, 0xef,
    ],
    order: &[
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc7, 0x63, 0x4d, 0x81, 0xf4, 0x37,
        0x2d, 0xdf, 0x58, 0x1a, 0x0d, 0xb2, 0x48, 0xb0, 0xa7, 0x7a, 0xec, 0xec, 0x19, 0x6a, 0xcc,
        0xc5, 0x29, 0x73,
    ],
    x: &[
        0xaa, 0x87, 0xca, 0x22, 0xbe, 0x8b, 0x05, 0x37, 0x8e, 0xb1, 0xc7, 0x1e, 0xf3, 0x20, 0xad,
        0x74, 0x6e, 0x1d, 0x3b, 0x62, 0x8b, 0xa7, 0x9b, 0x98, 0x59, 0xf7, 0x41, 0xe0, 0x82, 0x54,
        0x2a, 0x38, 0x55, 0x02, 0xf2, 0x5d, 0xbf, 0x55, 0x29, 0x6c, 0x3a, 0x54, 0x5e, 0x38, 0x72,
        0x76, 0x0a, 0xb7,
    ],
    y: &[
        0x36, 0x17, 0xde, 0x4a, 0x96, 0x26, 0x2c, 0x6f, 0x5d, 0x9e, 0x98, 0xbf, 0x92, 0x92, 0xdc,
        0x29, 0xf8, 0xf4, 0x1d, 0xbd, 0x28, 0x9a, 0x14, 0x7c, 0xe9, 0xda, 0x31, 0x13, 0xb5, 0xf0,
        0xb8, 0xc0, 0x0a, 0x60, 0xb1, 0xce, 0x1d, 0x7e, 0x81, 0x9d, 0x7a, 0x43, 0x1d, 0x7c, 0x90,
        0xea, 0x0e, 0x5f,
    ],
};

impl Curve {
    /// The bytes of a coordinate and of a scalar.
    pub const fn scalar_bytes(self) -> usize {
        match self {
            Self::P256 => 32,
            Self::P384 => 48,
        }
    }

    fn parameters(self) -> &'static Parameters {
        match self {
            Self::P256 => &P256,
            Self::P384 => &P384,
        }
    }
}

fn number(bytes: &[u8]) -> Number {
    Number::from_be_bytes(bytes).unwrap_or(Number::ZERO)
}

#[derive(Clone, Copy)]
struct Point {
    x: Number,
    y: Number,
    z: Number,
}

struct Field {
    modulus: Modulus,
    b: Number,
}

impl Field {
    fn is_infinity(point: &Point) -> bool {
        point.z.is_zero()
    }

    fn times_two(&self, a: &Number) -> Number {
        self.modulus.add(a, a)
    }

    fn square(&self, a: &Number) -> Number {
        self.modulus.multiply(a, a)
    }

    fn on_curve(&self, x: &Number, y: &Number) -> bool {
        let m = &self.modulus;
        let y_squared = self.square(y);
        let x_cubed = m.multiply(&self.square(x), x);
        let three_x = m.add(&self.times_two(x), x);
        let right = m.add(&m.subtract(&x_cubed, &three_x), &self.b);
        y_squared == right
    }

    fn double(&self, point: &Point) -> Point {
        if Self::is_infinity(point) || point.y.is_zero() {
            return Point {
                x: Number::ZERO,
                y: Number::ZERO,
                z: Number::ZERO,
            };
        }
        let m = &self.modulus;
        let delta = self.square(&point.z);
        let gamma = self.square(&point.y);
        let beta = m.multiply(&point.x, &gamma);
        let product = m.multiply(&m.subtract(&point.x, &delta), &m.add(&point.x, &delta));
        let alpha = m.add(&self.times_two(&product), &product);
        let four_beta = self.times_two(&self.times_two(&beta));
        let x = m.subtract(&self.square(&alpha), &self.times_two(&four_beta));
        let z = m.subtract(
            &m.subtract(&self.square(&m.add(&point.y, &point.z)), &gamma),
            &delta,
        );
        let gamma_squared = self.square(&gamma);
        let eight_gamma_squared = self.times_two(&self.times_two(&self.times_two(&gamma_squared)));
        let y = m.subtract(
            &m.multiply(&alpha, &m.subtract(&four_beta, &x)),
            &eight_gamma_squared,
        );
        Point { x, y, z }
    }

    fn add(&self, p: &Point, q: &Point) -> Point {
        if Self::is_infinity(p) {
            return *q;
        }
        if Self::is_infinity(q) {
            return *p;
        }
        let m = &self.modulus;
        let z1z1 = self.square(&p.z);
        let z2z2 = self.square(&q.z);
        let u1 = m.multiply(&p.x, &z2z2);
        let u2 = m.multiply(&q.x, &z1z1);
        let s1 = m.multiply(&m.multiply(&p.y, &q.z), &z2z2);
        let s2 = m.multiply(&m.multiply(&q.y, &p.z), &z1z1);
        let h = m.subtract(&u2, &u1);
        if h.is_zero() {
            if s1 == s2 {
                return self.double(p);
            }
            return Point {
                x: Number::ZERO,
                y: Number::ZERO,
                z: Number::ZERO,
            };
        }
        let i = self.square(&self.times_two(&h));
        let j = m.multiply(&h, &i);
        let r = self.times_two(&m.subtract(&s2, &s1));
        let v = m.multiply(&u1, &i);
        let x = m.subtract(&m.subtract(&self.square(&r), &j), &self.times_two(&v));
        let y = m.subtract(
            &m.multiply(&r, &m.subtract(&v, &x)),
            &self.times_two(&m.multiply(&s1, &j)),
        );
        let z_sum = self.square(&m.add(&p.z, &q.z));
        let z = m.multiply(&m.subtract(&m.subtract(&z_sum, &z1z1), &z2z2), &h);
        Point { x, y, z }
    }
}

/// Whether `signature` (`r` and `s`, big-endian) is a valid ECDSA signature by `public_key`
/// (an uncompressed SEC 1 point, `04 || x || y`) over a message whose hash is `digest`.
pub fn verify(curve: Curve, public_key: &[u8], digest: &[u8], r: &[u8], s: &[u8]) -> bool {
    let size = curve.scalar_bytes();
    let parameters = curve.parameters();
    if public_key.len() != 1 + 2 * size || public_key[0] != 4 {
        return false;
    }
    let prime = number(parameters.prime);
    let order = number(parameters.order);
    let (Some(field_modulus), Some(scalars)) = (Modulus::new(&prime), Modulus::new(&order)) else {
        return false;
    };
    let field = Field {
        b: field_modulus.to_montgomery(&number(parameters.b)),
        modulus: field_modulus,
    };
    let (Some(r), Some(s)) = (Number::from_be_bytes(r), Number::from_be_bytes(s)) else {
        return false;
    };
    if r.is_zero() || s.is_zero() || r >= order || s >= order {
        return false;
    }
    let qx = number(&public_key[1..1 + size]);
    let qy = number(&public_key[1 + size..]);
    if qx >= prime || qy >= prime {
        return false;
    }
    let m = &field.modulus;
    let one = m.one();
    let q = Point {
        x: m.to_montgomery(&qx),
        y: m.to_montgomery(&qy),
        z: one,
    };
    if !field.on_curve(&q.x, &q.y) {
        return false;
    }
    let g = Point {
        x: m.to_montgomery(&number(parameters.x)),
        y: m.to_montgomery(&number(parameters.y)),
        z: one,
    };
    let e = scalars.reduce(&number(&digest[..digest.len().min(size)]));
    let w = scalars.invert_prime(&scalars.to_montgomery(&s));
    let u1 = scalars.from_montgomery(&scalars.multiply(&scalars.to_montgomery(&e), &w));
    let u2 = scalars.from_montgomery(&scalars.multiply(&scalars.to_montgomery(&r), &w));
    let both = field.add(&g, &q);
    let mut sum = Point {
        x: Number::ZERO,
        y: Number::ZERO,
        z: Number::ZERO,
    };
    for bit in (0..u1.bits().max(u2.bits())).rev() {
        sum = field.double(&sum);
        match (u1.bit(bit), u2.bit(bit)) {
            (true, true) => sum = field.add(&sum, &both),
            (true, false) => sum = field.add(&sum, &g),
            (false, true) => sum = field.add(&sum, &q),
            (false, false) => {}
        }
    }
    if Field::is_infinity(&sum) {
        return false;
    }
    let z_inverse = m.invert_prime(&sum.z);
    let x = m.from_montgomery(&m.multiply(&sum.x, &field.square(&z_inverse)));
    scalars.reduce(&x) == r
}

/// Whether the curve's base point satisfies its equation: a check of the constants.
pub fn base_point_is_on_curve(curve: Curve) -> bool {
    let parameters = curve.parameters();
    let Some(modulus) = Modulus::new(&number(parameters.prime)) else {
        return false;
    };
    let field = Field {
        b: modulus.to_montgomery(&number(parameters.b)),
        modulus,
    };
    let m = &field.modulus;
    field.on_curve(
        &m.to_montgomery(&number(parameters.x)),
        &m.to_montgomery(&number(parameters.y)),
    )
}
