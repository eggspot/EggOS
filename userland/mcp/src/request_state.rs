//! The `requestState` of the stateless era's `input_required` results (D578): what a call
//! that needs the person's yes hands the client, which asks its user and sends the call again
//! with the answer and this text. It names the agent, the call (a hash of the tool's name and
//! its arguments), when it expires and a number used once, and carries an HMAC-SHA256 over
//! them under a key the server drew at start, so a client can neither forge nor alter one; the
//! server keeps the numbers already used, so one cannot be replayed. Written as base64url.

use crate::grants::AgentId;
use eggos_crypto::constant_time;
use eggos_crypto::hmac::hmac;
use eggos_crypto::sha2::{HashAlgorithm, Sha256};

/// The key's length.
pub const KEY_BYTES: usize = 32;
/// How long a question stays answerable, in seconds.
pub const LIFETIME_SECONDS: u64 = 300;
/// The most bytes of the text.
pub const MAX_TEXT_BYTES: usize = 192;

const VERSION: u8 = 1;
const MAC_BYTES: usize = 32;
const MAX_PAYLOAD_BYTES: usize = 1 + 1 + 32 + 32 + 8 + 8;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// What a `requestState` vouches for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    pub agent: AgentId,
    pub call: [u8; 32],
    pub expires: u64,
    pub nonce: u64,
}

/// The hash naming one call: the tool's name, a NUL, and its arguments' JSON as sent.
pub fn call_hash(tool: &[u8], arguments: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(tool);
    hash.update(&[0]);
    hash.update(arguments);
    hash.finish()
}

fn payload(claim: &Claim, out: &mut [u8]) -> usize {
    let agent = claim.agent.as_bytes();
    out[0] = VERSION;
    out[1] = agent.len() as u8;
    let mut at = 2;
    out[at..at + agent.len()].copy_from_slice(agent);
    at += agent.len();
    out[at..at + 32].copy_from_slice(&claim.call);
    at += 32;
    out[at..at + 8].copy_from_slice(&claim.expires.to_be_bytes());
    at += 8;
    out[at..at + 8].copy_from_slice(&claim.nonce.to_be_bytes());
    at + 8
}

fn mac(key: &[u8; KEY_BYTES], payload: &[u8]) -> [u8; MAC_BYTES] {
    let digest = hmac(HashAlgorithm::Sha256, key, payload);
    let mut mac = [0u8; MAC_BYTES];
    mac.copy_from_slice(&digest.as_bytes()[..MAC_BYTES]);
    mac
}

/// Writes `claim`, signed with `key`, as text into `out`; its length.
pub fn seal(key: &[u8; KEY_BYTES], claim: &Claim, out: &mut [u8; MAX_TEXT_BYTES]) -> usize {
    let mut bytes = [0u8; MAX_PAYLOAD_BYTES + MAC_BYTES];
    let length = payload(claim, &mut bytes);
    let signature = mac(key, &bytes[..length]);
    bytes[length..length + MAC_BYTES].copy_from_slice(&signature);
    encode(&bytes[..length + MAC_BYTES], out)
}

/// The claim `text` carries when `key` signed it; `None` for anything else.
pub fn open(key: &[u8; KEY_BYTES], text: &[u8]) -> Option<Claim> {
    let mut bytes = [0u8; MAX_PAYLOAD_BYTES + MAC_BYTES];
    let length = decode(text, &mut bytes)?;
    let (body, signature) = bytes[..length].split_at_checked(length.checked_sub(MAC_BYTES)?)?;
    if !constant_time::equal(&mac(key, body), signature) {
        return None;
    }
    let (&version, rest) = body.split_first()?;
    let (&agent_length, rest) = rest.split_first()?;
    if version != VERSION {
        return None;
    }
    let (agent, rest) = rest.split_at_checked(usize::from(agent_length))?;
    let (call, rest) = rest.split_at_checked(32)?;
    let (expires, nonce) = rest.split_at_checked(8)?;
    Some(Claim {
        agent: AgentId::stored(agent)?,
        call: call.try_into().ok()?,
        expires: u64::from_be_bytes(expires.try_into().ok()?),
        nonce: u64::from_be_bytes(nonce.try_into().ok()?),
    })
}

fn encode(bytes: &[u8], out: &mut [u8; MAX_TEXT_BYTES]) -> usize {
    let mut length = 0;
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..=chunk.len() {
            if let Some(slot) = out.get_mut(length) {
                *slot = ALPHABET[((word >> (18 - 6 * index)) & 63) as usize];
                length += 1;
            }
        }
    }
    length
}

fn decode(text: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut bits = 0u32;
    let mut count = 0;
    let mut length = 0;
    for &byte in text {
        let value = ALPHABET.iter().position(|&letter| letter == byte)? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            *out.get_mut(length)? = (bits >> count) as u8;
            length += 1;
        }
    }
    Some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; KEY_BYTES] = [7; KEY_BYTES];

    fn claim() -> Claim {
        Claim {
            agent: AgentId::new(b"laptop").unwrap(),
            call: call_hash(b"fs_write", br#"{"path":"/home/a"}"#),
            expires: 1_800_000_000,
            nonce: 42,
        }
    }

    #[test]
    fn a_sealed_claim_opens_with_its_key_only() {
        let mut text = [0u8; MAX_TEXT_BYTES];
        let length = seal(&KEY, &claim(), &mut text);
        assert!(text[..length].iter().all(|byte| ALPHABET.contains(byte)));
        assert_eq!(open(&KEY, &text[..length]), Some(claim()));
        assert_eq!(open(&[8; KEY_BYTES], &text[..length]), None);
    }

    #[test]
    fn a_changed_or_cut_state_does_not_open() {
        let mut text = [0u8; MAX_TEXT_BYTES];
        let length = seal(&KEY, &claim(), &mut text);
        for at in [0, length / 2, length - 2] {
            let mut changed = text;
            changed[at] = if changed[at] == b'A' { b'B' } else { b'A' };
            assert_eq!(open(&KEY, &changed[..length]), None);
        }
        assert_eq!(open(&KEY, &text[..length - 4]), None);
        assert_eq!(open(&KEY, b"not base64!"), None);
    }

    #[test]
    fn the_call_hash_tells_calls_apart() {
        assert_ne!(call_hash(b"fs_write", b"{}"), call_hash(b"fs_delete", b"{}"));
        assert_ne!(call_hash(b"a", b"b{}"), call_hash(b"ab", b"{}"));
    }
}
