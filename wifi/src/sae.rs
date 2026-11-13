//! Simultaneous Authentication of Equals, WPA3-Personal's password authentication (IEEE
//! 802.11-2020, 12.4; D285), on P-256 (group 19) with hunting-and-pecking: the password element
//! from the password and both addresses, the commit and confirm exchanges in authentication
//! frames, and the PMK they end with. Every round of the hunt runs whatever the password, so the
//! time taken says nothing about it.

use crate::frames::Address;
use crate::kdf::kdf_sha256;
use alloc::vec::Vec;
use eggos_crypto::HashAlgorithm;
use eggos_crypto::constant_time::equal;
use eggos_crypto::hmac::hmac;
use eggos_crypto::p256::{self, AffinePoint, SCALAR_BYTES};

/// IEEE 802.11's number for the group P-256.
pub const GROUP_P256: u16 = 19;
/// The status an access point answers a commit with when it wants an anti-clogging token.
pub const ANTI_CLOGGING_TOKEN_REQUIRED: u16 = 76;
/// The PMK's bytes.
pub const PMK_BYTES: usize = 32;
const HUNTING_ROUNDS: u8 = 40;
const KCK_BYTES: usize = 32;
const ELEMENT_BYTES: usize = 2 * SCALAR_BYTES;
const CONFIRM_BYTES: usize = 32;
const SCALAR_DRAWS: usize = 64;

/// Why SAE stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaeError {
    /// No password element in the hunt's rounds (about once in 2^40 passwords).
    NoElement,
    /// No usable random scalar in many draws: the randomness is broken.
    BadRandomness,
    /// The peer's commit is for another group, too short, or out of range.
    BadCommit,
    /// The peer's commit is ours sent back.
    Reflected,
    /// The peer's confirm did not match: most often the wrong password.
    BadConfirm,
}

/// A commit's scalar and element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    pub scalar: [u8; SCALAR_BYTES],
    pub element: AffinePoint,
}

impl Commit {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.scalar);
        out.extend_from_slice(&self.element.x);
        out.extend_from_slice(&self.element.y);
    }

    fn read(bytes: &[u8]) -> Option<Self> {
        let scalar: [u8; SCALAR_BYTES] = bytes.get(..SCALAR_BYTES)?.try_into().ok()?;
        let x = bytes.get(SCALAR_BYTES..2 * SCALAR_BYTES)?.try_into().ok()?;
        let y = bytes
            .get(2 * SCALAR_BYTES..SCALAR_BYTES + ELEMENT_BYTES)?
            .try_into()
            .ok()?;
        Some(Self {
            scalar,
            element: AffinePoint { x, y },
        })
    }
}

/// The password element of `password` between stations `own` and `peer`.
pub fn password_element(password: &[u8], own: Address, peer: Address) -> Option<AffinePoint> {
    let (high, low) = if own > peer { (own, peer) } else { (peer, own) };
    let mut key = [0u8; 12];
    key[..6].copy_from_slice(&high);
    key[6..].copy_from_slice(&low);
    let prime = p256::prime();
    let mut found: Option<([u8; SCALAR_BYTES], bool)> = None;
    for counter in 1..=HUNTING_ROUNDS {
        let mut input = Vec::with_capacity(password.len() + 1);
        input.extend_from_slice(password);
        input.push(counter);
        let seed = hmac(HashAlgorithm::Sha256, &key, &input);
        let mut value = [0u8; SCALAR_BYTES];
        kdf_sha256(
            seed.as_bytes(),
            b"SAE Hunting and Pecking",
            &prime,
            &mut value,
        );
        let usable = p256::below_prime(&value) & p256::has_point_at(&value);
        let odd = seed.as_bytes()[31] & 1 == 1;
        if usable && found.is_none() {
            found = Some((value, odd));
        }
    }
    let (x, odd) = found?;
    p256::point_at(&x, odd)
}

/// One side of an SAE exchange.
#[derive(Debug)]
pub struct Sae {
    password_element: AffinePoint,
    random: [u8; SCALAR_BYTES],
    own: Commit,
    peer: Option<Commit>,
    kck: [u8; KCK_BYTES],
    pmk: [u8; PMK_BYTES],
}

fn draw(random: &mut dyn FnMut(&mut [u8])) -> Result<[u8; SCALAR_BYTES], SaeError> {
    for _ in 0..SCALAR_DRAWS {
        let mut value = [0u8; SCALAR_BYTES];
        random(&mut value);
        if p256::scalar_in_range(&value) {
            return Ok(value);
        }
    }
    Err(SaeError::BadRandomness)
}

impl Sae {
    /// Starts SAE for `password` between station `own` and access point `peer`, `random`
    /// filling buffers with the system's randomness.
    pub fn new(
        password: &[u8],
        own: Address,
        peer: Address,
        random: &mut dyn FnMut(&mut [u8]),
    ) -> Result<Self, SaeError> {
        let password_element = password_element(password, own, peer).ok_or(SaeError::NoElement)?;
        for _ in 0..SCALAR_DRAWS {
            let secret = draw(random)?;
            let mask = draw(random)?;
            let scalar = p256::scalar_add(&secret, &mask);
            if !p256::scalar_in_range(&scalar) {
                continue;
            }
            let Some(masked) = p256::multiply(&password_element, &mask) else {
                continue;
            };
            return Ok(Self {
                password_element,
                random: secret,
                own: Commit {
                    scalar,
                    element: p256::negate(&masked),
                },
                peer: None,
                kck: [0; KCK_BYTES],
                pmk: [0; PMK_BYTES],
            });
        }
        Err(SaeError::BadRandomness)
    }

    /// The body of the commit authentication frame: the group, the anti-clogging `token` the
    /// access point asked for (empty at first), the scalar and the element.
    pub fn commit_body(&self, token: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(2 + token.len() + SCALAR_BYTES + ELEMENT_BYTES);
        body.extend_from_slice(&GROUP_P256.to_le_bytes());
        body.extend_from_slice(token);
        self.own.write(&mut body);
        body
    }

    /// Takes the access point's commit body, checks it and derives the keys.
    pub fn receive_commit(&mut self, body: &[u8]) -> Result<(), SaeError> {
        let group = body
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
        if group != Some(GROUP_P256) || body.len() != 2 + SCALAR_BYTES + ELEMENT_BYTES {
            return Err(SaeError::BadCommit);
        }
        let peer = Commit::read(&body[2..]).ok_or(SaeError::BadCommit)?;
        if !p256::scalar_in_range(&peer.scalar) || !p256::on_curve(&peer.element) {
            return Err(SaeError::BadCommit);
        }
        if peer == self.own {
            return Err(SaeError::Reflected);
        }
        let scaled =
            p256::multiply(&self.password_element, &peer.scalar).ok_or(SaeError::BadCommit)?;
        let sum = p256::add(&scaled, &peer.element).ok_or(SaeError::BadCommit)?;
        let shared = p256::multiply(&sum, &self.random).ok_or(SaeError::BadCommit)?;
        let keyseed = hmac(HashAlgorithm::Sha256, &[0u8; 32], &shared.x);
        let context = p256::scalar_add(&self.own.scalar, &peer.scalar);
        let mut keys = [0u8; KCK_BYTES + PMK_BYTES];
        kdf_sha256(keyseed.as_bytes(), b"SAE KCK and PMK", &context, &mut keys);
        self.kck.copy_from_slice(&keys[..KCK_BYTES]);
        self.pmk.copy_from_slice(&keys[KCK_BYTES..]);
        self.peer = Some(peer);
        Ok(())
    }

    fn confirm(&self, send_confirm: u16, first: &Commit, second: &Commit) -> [u8; CONFIRM_BYTES] {
        let mut data = Vec::with_capacity(2 + 2 * (SCALAR_BYTES + ELEMENT_BYTES));
        data.extend_from_slice(&send_confirm.to_le_bytes());
        first.write(&mut data);
        second.write(&mut data);
        let digest = hmac(HashAlgorithm::Sha256, &self.kck, &data);
        digest.as_bytes()[..CONFIRM_BYTES]
            .try_into()
            .unwrap_or([0; CONFIRM_BYTES])
    }

    /// The body of the confirm authentication frame, numbered `send_confirm`; after
    /// [`Self::receive_commit`].
    pub fn confirm_body(&self, send_confirm: u16) -> Option<Vec<u8>> {
        let peer = self.peer?;
        let mut body = Vec::with_capacity(2 + CONFIRM_BYTES);
        body.extend_from_slice(&send_confirm.to_le_bytes());
        body.extend_from_slice(&self.confirm(send_confirm, &self.own, &peer));
        Some(body)
    }

    /// Checks the access point's confirm body: the PMK and its PMKID when it matches.
    pub fn receive_confirm(&self, body: &[u8]) -> Result<([u8; PMK_BYTES], [u8; 16]), SaeError> {
        let peer = self.peer.ok_or(SaeError::BadConfirm)?;
        if body.len() != 2 + CONFIRM_BYTES {
            return Err(SaeError::BadConfirm);
        }
        let send_confirm = u16::from_le_bytes([body[0], body[1]]);
        if !equal(&self.confirm(send_confirm, &peer, &self.own), &body[2..]) {
            return Err(SaeError::BadConfirm);
        }
        let context = p256::scalar_add(&self.own.scalar, &peer.scalar);
        let mut pmkid = [0u8; 16];
        pmkid.copy_from_slice(&context[..16]);
        Ok((self.pmk, pmkid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counting(seed: u8) -> impl FnMut(&mut [u8]) {
        let mut next = seed;
        move |out: &mut [u8]| {
            for byte in out.iter_mut() {
                next = next.wrapping_mul(29).wrapping_add(7);
                *byte = next;
            }
            out[0] &= 0x7f;
        }
    }

    #[test]
    fn two_sides_with_one_password_agree_on_the_pmk() {
        let (station, access_point) = ([2, 0, 0, 0, 0, 1], [2, 0, 0, 0, 0, 2]);
        assert_eq!(
            password_element(b"mekmitasdigoat", station, access_point),
            password_element(b"mekmitasdigoat", access_point, station)
        );
        let mut one = Sae::new(b"mekmitasdigoat", station, access_point, &mut counting(3)).unwrap();
        let mut other =
            Sae::new(b"mekmitasdigoat", access_point, station, &mut counting(91)).unwrap();
        one.receive_commit(&other.commit_body(&[])).unwrap();
        other.receive_commit(&one.commit_body(&[])).unwrap();
        let from_other = other.confirm_body(1).unwrap();
        let from_one = one.confirm_body(1).unwrap();
        assert_eq!(
            one.receive_confirm(&from_other).unwrap(),
            other.receive_confirm(&from_one).unwrap()
        );
        let mut wrong =
            Sae::new(b"another password", access_point, station, &mut counting(5)).unwrap();
        let mut fresh =
            Sae::new(b"mekmitasdigoat", station, access_point, &mut counting(9)).unwrap();
        fresh.receive_commit(&wrong.commit_body(&[])).unwrap();
        wrong.receive_commit(&fresh.commit_body(&[])).unwrap();
        assert_eq!(
            fresh.receive_confirm(&wrong.confirm_body(1).unwrap()),
            Err(SaeError::BadConfirm)
        );
        assert_eq!(
            one.receive_commit(&one.commit_body(&[])),
            Err(SaeError::Reflected)
        );
    }
}
