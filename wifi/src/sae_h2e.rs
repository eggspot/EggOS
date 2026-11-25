//! SAE's hash-to-element (D786; IEEE 802.11-2020, 12.4.4.2.3), which 6 GHz networks require and
//! WPA3 access points prefer: the password's element `PT` from the SSID and password by two
//! simplified SWU mappings onto P-256, then for each peer `PWE = val * PT` with `val` from both
//! addresses, instead of hunting and pecking. Its field arithmetic is `eggos-crypto`'s
//! `bignum`, which is not constant-time: the time it takes may tell an observer on the machine
//! something about the password (recorded in D786).

use crate::frames::Address;
use eggos_crypto::HashAlgorithm;
use eggos_crypto::bignum::{Modulus, Number};
use eggos_crypto::hmac;
use eggos_crypto::p256::{self, AffinePoint, SCALAR_BYTES};

/// The status code of an SAE commit using hash-to-element.
pub const STATUS_HASH_TO_ELEMENT: u16 = 126;
const B: [u8; SCALAR_BYTES] = [
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86, 0xbc,
    0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6, 0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
];
const Z_NEGATED: u64 = 10;
const Z_TIMES_A: u64 = 30;
const A_NEGATED: u64 = 3;
const VALUE_BYTES: usize = 48;

fn bytes_of(number: &Number) -> [u8; SCALAR_BYTES] {
    let mut out = [0u8; SCALAR_BYTES];
    number.write_be_bytes(&mut out);
    out
}

/// The simplified SWU mapping of field element `u` (any bytes, reduced mod p) onto P-256.
fn sswu(u_bytes: &[u8]) -> Option<AffinePoint> {
    let prime = Number::from_be_bytes(&p256::prime())?;
    let field = Modulus::new(&prime)?;
    let u = field.to_montgomery(&Number::from_be_bytes(u_bytes)?);
    let z = field.subtract(
        &field.to_montgomery(&Number::from_u64(0)),
        &field.to_montgomery(&Number::from_u64(Z_NEGATED)),
    );
    let u_squared = field.multiply(&u, &u);
    let z_u2 = field.multiply(&z, &u_squared);
    let m = field.add(&field.multiply(&z_u2, &z_u2), &z_u2);
    let b = field.to_montgomery(&Number::from_be_bytes(&B)?);
    let m_plain = field.from_montgomery(&m);
    let x1 = match m_plain.is_zero() {
        true => {
            let za = field.to_montgomery(&Number::from_u64(Z_TIMES_A));
            field.multiply(&b, &field.invert_prime(&za))
        }
        false => {
            let three = field.to_montgomery(&Number::from_u64(A_NEGATED));
            let b_over_three = field.multiply(&b, &field.invert_prime(&three));
            let t = field.add(&field.one(), &field.invert_prime(&m));
            field.multiply(&b_over_three, &t)
        }
    };
    let x2 = field.multiply(&z_u2, &x1);
    let x1_bytes = bytes_of(&field.from_montgomery(&x1));
    let x2_bytes = bytes_of(&field.from_montgomery(&x2));
    let x = match p256::has_point_at(&x1_bytes) {
        true => x1_bytes,
        false => x2_bytes,
    };
    let u_reduced = bytes_of(&field.reduce(&Number::from_be_bytes(u_bytes)?));
    let odd = u_reduced[SCALAR_BYTES - 1] & 1 == 1;
    p256::point_at(&x, odd)
}

/// The password's element `PT` for network `ssid`, with the password identifier `identifier`
/// when the network uses one.
pub fn password_token(ssid: &[u8], password: &[u8], identifier: &[u8]) -> Option<AffinePoint> {
    let mut input = alloc::vec::Vec::with_capacity(password.len() + identifier.len());
    input.extend_from_slice(password);
    input.extend_from_slice(identifier);
    let seed = hmac::extract(HashAlgorithm::Sha256, ssid, &input);
    let mut points = [None, None];
    for (index, label) in [b"SAE Hash to Element u1 P1", b"SAE Hash to Element u2 P2"]
        .iter()
        .enumerate()
    {
        let mut value = [0u8; VALUE_BYTES];
        hmac::expand(HashAlgorithm::Sha256, seed.as_bytes(), &[label.as_slice()], &mut value)
            .ok()?;
        points[index] = sswu(&value);
    }
    p256::add(&points[0]?, &points[1]?)
}

fn reduce_below(value: &mut [u8; SCALAR_BYTES], limit: &[u8; SCALAR_BYTES]) {
    if *value < *limit {
        return;
    }
    let mut borrow = 0i16;
    for index in (0..SCALAR_BYTES).rev() {
        let difference = i16::from(value[index]) - i16::from(limit[index]) - borrow;
        value[index] = difference.rem_euclid(256) as u8;
        borrow = i16::from(difference < 0);
    }
}

fn add_one(value: &mut [u8; SCALAR_BYTES]) {
    for byte in value.iter_mut().rev() {
        let (sum, carried) = byte.overflowing_add(1);
        *byte = sum;
        if !carried {
            return;
        }
    }
}

/// The password element between `own` and `peer` from `token` ([`password_token`]).
pub fn password_element(token: &AffinePoint, own: Address, peer: Address) -> Option<AffinePoint> {
    let (high, low) = if own > peer { (own, peer) } else { (peer, own) };
    let mut addresses = [0u8; 12];
    addresses[..6].copy_from_slice(&high);
    addresses[6..].copy_from_slice(&low);
    let digest = hmac::extract(HashAlgorithm::Sha256, &[0u8; SCALAR_BYTES], &addresses);
    let mut value: [u8; SCALAR_BYTES] = digest.as_bytes().try_into().ok()?;
    let mut order_less_one = p256::order();
    order_less_one[SCALAR_BYTES - 1] -= 1;
    reduce_below(&mut value, &order_less_one);
    add_one(&mut value);
    p256::multiply(token, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_and_element_are_points_on_the_curve() {
        let token = password_token(b"byteme", b"mekmitasdigoat", b"").unwrap();
        assert!(p256::on_curve(&token));
        let element = password_element(&token, [2; 6], [1; 6]).unwrap();
        assert!(p256::on_curve(&element));
        assert_eq!(
            password_element(&token, [1; 6], [2; 6]),
            Some(element)
        );
    }
}
