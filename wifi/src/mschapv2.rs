//! MS-CHAPv2 (RFC 2759) inside EAP (draft-kamath-pppext-eap-mschapv2), PEAP's usual inner
//! method (D779): the peer's response to the server's challenge, and the check of the
//! server's own proof that it knows the password too.

use alloc::vec::Vec;
use eggos_crypto::constant_time::equal;
use eggos_crypto::des;
use eggos_crypto::md4::md4;
use eggos_crypto::sha1::Sha1;

/// The bytes of each side's challenge.
pub const CHALLENGE_BYTES: usize = 16;
/// The bytes of the NT response.
pub const NT_RESPONSE_BYTES: usize = 24;
/// The bytes of the server's proof.
pub const AUTHENTICATOR_RESPONSE_BYTES: usize = 20;

/// MS-CHAPv2's operation codes.
pub mod opcode {
    pub const CHALLENGE: u8 = 1;
    pub const RESPONSE: u8 = 2;
    pub const SUCCESS: u8 = 3;
    pub const FAILURE: u8 = 4;
}

const RESPONSE_VALUE_BYTES: u8 = 49;
const MAGIC_SERVER: &[u8] = b"Magic server to client signing constant";
const MAGIC_PAD: &[u8] = b"Pad to make it do more than one iteration";

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finish()
}

/// The user name without a `DOMAIN\` prefix, as the challenge hash takes it.
pub fn user_without_domain(user: &[u8]) -> &[u8] {
    match user.iter().rposition(|&byte| byte == b'\\') {
        Some(at) => &user[at + 1..],
        None => user,
    }
}

/// ChallengeHash: the first 8 bytes of SHA-1 over both challenges and the user name.
pub fn challenge_hash(
    peer: &[u8; CHALLENGE_BYTES],
    authenticator: &[u8; CHALLENGE_BYTES],
    user: &[u8],
) -> [u8; 8] {
    let digest = sha1(&[peer, authenticator, user_without_domain(user)]);
    let mut hash = [0u8; 8];
    hash.copy_from_slice(&digest[..8]);
    hash
}

/// NtPasswordHash: MD4 of the password in UTF-16LE (the password given in UTF-8).
pub fn password_hash(password: &[u8]) -> [u8; 16] {
    let text = core::str::from_utf8(password).unwrap_or("");
    let mut wide = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        wide.extend_from_slice(&unit.to_le_bytes());
    }
    let hash = md4(&[&wide]);
    wide.fill(0);
    hash
}

/// ChallengeResponse: the challenge hash encrypted under the three 7-byte thirds of the
/// zero-padded password hash.
pub fn nt_response(challenge: &[u8; 8], password_hash: &[u8; 16]) -> [u8; NT_RESPONSE_BYTES] {
    let mut padded = [0u8; 21];
    padded[..16].copy_from_slice(password_hash);
    let mut response = [0u8; NT_RESPONSE_BYTES];
    for (third, out) in padded
        .as_chunks::<7>()
        .0
        .iter()
        .zip(response.as_chunks_mut::<8>().0)
    {
        *out = des::encrypt(&des::key_from_seven(third), challenge);
    }
    response
}

/// GenerateAuthenticatorResponse: the proof the server sends back on success.
pub fn authenticator_response(
    password_hash: &[u8; 16],
    nt_response: &[u8; NT_RESPONSE_BYTES],
    challenge: &[u8; 8],
) -> [u8; AUTHENTICATOR_RESPONSE_BYTES] {
    let hash_hash = md4(&[password_hash]);
    let digest = sha1(&[&hash_hash, nt_response, MAGIC_SERVER]);
    sha1(&[&digest, challenge, MAGIC_PAD])
}

/// What the peer sends for a challenge, and the proof it then expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The method's data (opcode onwards) of the response.
    pub data: Vec<u8>,
    pub expected: [u8; AUTHENTICATOR_RESPONSE_BYTES],
}

/// The response to a challenge request's data (opcode onwards) for `user` and `password`,
/// with `peer_challenge` fresh random bytes; `None` when it is not a challenge.
pub fn answer_challenge(
    data: &[u8],
    user: &[u8],
    password: &[u8],
    peer_challenge: &[u8; CHALLENGE_BYTES],
) -> Option<Answer> {
    if *data.first()? != opcode::CHALLENGE || usize::from(*data.get(4)?) != CHALLENGE_BYTES {
        return None;
    }
    let identifier = data[1];
    let authenticator: [u8; CHALLENGE_BYTES] = data.get(5..5 + CHALLENGE_BYTES)?.try_into().ok()?;
    let challenge = challenge_hash(peer_challenge, &authenticator, user);
    let hash = password_hash(password);
    let response = nt_response(&challenge, &hash);
    let expected = authenticator_response(&hash, &response, &challenge);
    let length = 4 + 1 + usize::from(RESPONSE_VALUE_BYTES) + user.len();
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&[opcode::RESPONSE, identifier]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.push(RESPONSE_VALUE_BYTES);
    out.extend_from_slice(peer_challenge);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&response);
    out.push(0);
    out.extend_from_slice(user);
    Some(Answer {
        data: out,
        expected,
    })
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

/// Whether a success request's data (opcode onwards) carries `expected` as its `S=` proof.
pub fn success_proves(data: &[u8], expected: &[u8; AUTHENTICATOR_RESPONSE_BYTES]) -> bool {
    if data.first() != Some(&opcode::SUCCESS) {
        return false;
    }
    let message = data.get(4..).unwrap_or_default();
    let Some(at) = message.windows(2).position(|pair| pair == b"S=") else {
        return false;
    };
    let Some(digits) = message.get(at + 2..at + 2 + 2 * AUTHENTICATOR_RESPONSE_BYTES) else {
        return false;
    };
    let mut proof = [0u8; AUTHENTICATOR_RESPONSE_BYTES];
    for (byte, pair) in proof.iter_mut().zip(digits.as_chunks::<2>().0) {
        let (Some(high), Some(low)) = (hex_value(pair[0]), hex_value(pair[1])) else {
            return false;
        };
        *byte = (high << 4) | low;
    }
    equal(&proof, expected)
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
    fn rfc_2759_s_example_computes_each_step() {
        let authenticator: [u8; 16] = hex("5B5D7C7D7B3F2F3E3C2C602132262628")
            .try_into()
            .unwrap();
        let peer: [u8; 16] = hex("21402324255E262A28295F2B3A337C7E")
            .try_into()
            .unwrap();
        let challenge = challenge_hash(&peer, &authenticator, b"User");
        assert_eq!(challenge.to_vec(), hex("D02E4386BCE91226"));
        let hash = password_hash(b"clientPass");
        assert_eq!(hash.to_vec(), hex("44EBBA8D5312B8D611474411F56989AE"));
        let response = nt_response(&challenge, &hash);
        assert_eq!(
            response.to_vec(),
            hex("82309ECD8D708B5EA08FAA3981CD83544233114A3D85D6DF")
        );
        let proof = authenticator_response(&hash, &response, &challenge);
        assert_eq!(
            proof.to_vec(),
            hex("407A5589115FD0D6209F510FE9C04566932CDA56")
        );
        let mut success = vec![opcode::SUCCESS, 1, 0, 0];
        success.extend_from_slice(b"S=407A5589115FD0D6209F510FE9C04566932CDA56 M=OK");
        assert!(success_proves(&success, &proof));
        success[10] = b'0';
        assert!(!success_proves(&success, &proof));
    }

    #[test]
    fn a_challenge_gets_a_49_byte_value_and_the_domain_stays_in_the_name() {
        let mut challenge = vec![opcode::CHALLENGE, 5, 0, 26, 16];
        challenge.extend_from_slice(&[0x11; 16]);
        challenge.extend_from_slice(b"srv");
        let answer = answer_challenge(&challenge, b"CORP\\ana", b"secret", &[0x22; 16]).unwrap();
        assert_eq!(answer.data[0], opcode::RESPONSE);
        assert_eq!(answer.data[1], 5);
        assert_eq!(answer.data[4], 49);
        assert!(answer.data.ends_with(b"CORP\\ana"));
        assert_eq!(
            usize::from(u16::from_be_bytes([answer.data[2], answer.data[3]])),
            answer.data.len()
        );
        assert_eq!(user_without_domain(b"CORP\\ana"), b"ana");
    }
}
