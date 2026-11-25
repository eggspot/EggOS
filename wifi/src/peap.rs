//! PEAP version 0's inner phase (Microsoft's MS-PEAP; D779): inside the TLS tunnel the server
//! asks the user's identity and runs MS-CHAPv2, then sends a Result TLV the peer echoes. Inner
//! packets travel without their EAP header except Extensions (type 33) packets; the header is
//! rebuilt from the outer request's identifier.

use crate::eap::{self, code, method};
use crate::mschapv2::{self, AUTHENTICATOR_RESPONSE_BYTES, CHALLENGE_BYTES};
use alloc::vec::Vec;

const RESULT_TLV: u16 = 3;
const TLV_TYPE_MASK: u16 = 0x3fff;
const MANDATORY: u16 = 0x8000;
const RESULT_SUCCESS: u16 = 1;
const RESULT_FAILURE: u16 = 2;

/// Why the inner phase failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerFailure {
    /// The server refused the user name or password.
    WrongPassword,
    /// The server's MS-CHAPv2 proof is wrong: it does not know the password.
    ServerNotTrusted,
    /// The server ended with a failure result.
    Refused,
}

/// What to send back through the tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// These bytes, as one TLS record's data.
    Reply(Vec<u8>),
    /// The inner phase failed.
    Failed(InnerFailure),
}

/// The inner phase's state.
#[derive(Debug)]
pub struct InnerPhase<'a> {
    user: &'a [u8],
    password: &'a [u8],
    peer_challenge: [u8; CHALLENGE_BYTES],
    expected: Option<[u8; AUTHENTICATOR_RESPONSE_BYTES]>,
    proven: bool,
    finished: bool,
}

fn is_whole_packet(inner: &[u8]) -> bool {
    inner.len() >= eap::HEADER_BYTES + 1
        && (code::REQUEST..=code::FAILURE).contains(&inner[0])
        && usize::from(u16::from_be_bytes([inner[2], inner[3]])) == inner.len()
        && inner[4] == method::EXTENSIONS
}

fn compressed(method: u8, data: &[u8]) -> Vec<u8> {
    let mut reply = Vec::with_capacity(1 + data.len());
    reply.push(method);
    reply.extend_from_slice(data);
    reply
}

fn result_status(tlvs: &[u8]) -> Option<u16> {
    let mut rest = tlvs;
    while rest.len() >= 4 {
        let kind = u16::from_be_bytes([rest[0], rest[1]]) & TLV_TYPE_MASK;
        let length = usize::from(u16::from_be_bytes([rest[2], rest[3]]));
        let value = rest.get(4..4 + length)?;
        if kind == RESULT_TLV && length == 2 {
            return Some(u16::from_be_bytes([value[0], value[1]]));
        }
        rest = &rest[4 + length..];
    }
    None
}

impl<'a> InnerPhase<'a> {
    /// The inner phase for `user` and `password`, with `peer_challenge` fresh random bytes.
    pub fn new(user: &'a [u8], password: &'a [u8], peer_challenge: [u8; CHALLENGE_BYTES]) -> Self {
        Self {
            user,
            password,
            peer_challenge,
            expected: None,
            proven: false,
            finished: false,
        }
    }

    /// Whether MS-CHAPv2 succeeded both ways and the server sent a success result.
    pub fn succeeded(&self) -> bool {
        self.proven && self.finished
    }

    /// Whether the server proved it knows the password.
    pub fn server_proven(&self) -> bool {
        self.proven
    }

    /// The answer to one inner packet `inner` the tunnel carried, under outer identifier
    /// `identifier`; `None` when there is nothing to answer (a commitment byte).
    pub fn answer(&mut self, inner: &[u8], identifier: u8) -> Option<Step> {
        if inner.is_empty() || inner == [0] {
            return None;
        }
        if is_whole_packet(inner) {
            let packet = eap::packet(inner)?;
            return Some(self.extensions(packet.identifier, packet.data));
        }
        let (kind, data) = (inner[0], &inner[1..]);
        Some(match kind {
            method::IDENTITY => Step::Reply(compressed(method::IDENTITY, self.user)),
            method::MSCHAPV2 => self.mschapv2(data),
            method::EXTENSIONS => self.extensions(identifier, data),
            _ => Step::Reply(compressed(method::NAK, &[method::MSCHAPV2])),
        })
    }

    fn mschapv2(&mut self, data: &[u8]) -> Step {
        match data.first().copied() {
            Some(mschapv2::opcode::CHALLENGE) => {
                let answer = mschapv2::answer_challenge(
                    data,
                    self.user,
                    self.password,
                    &self.peer_challenge,
                );
                match answer {
                    Some(answer) => {
                        self.expected = Some(answer.expected);
                        Step::Reply(compressed(method::MSCHAPV2, &answer.data))
                    }
                    None => Step::Failed(InnerFailure::Refused),
                }
            }
            Some(mschapv2::opcode::SUCCESS) => match &self.expected {
                Some(expected) if mschapv2::success_proves(data, expected) => {
                    self.proven = true;
                    Step::Reply(compressed(method::MSCHAPV2, &[mschapv2::opcode::SUCCESS]))
                }
                _ => Step::Failed(InnerFailure::ServerNotTrusted),
            },
            Some(mschapv2::opcode::FAILURE) => Step::Failed(InnerFailure::WrongPassword),
            _ => Step::Failed(InnerFailure::Refused),
        }
    }

    fn extensions(&mut self, identifier: u8, tlvs: &[u8]) -> Step {
        let status = result_status(tlvs);
        let ours = match status {
            Some(RESULT_SUCCESS) if self.proven => RESULT_SUCCESS,
            _ => RESULT_FAILURE,
        };
        if ours == RESULT_SUCCESS {
            self.finished = true;
        }
        let mut tlv = Vec::with_capacity(6);
        tlv.extend_from_slice(&(MANDATORY | RESULT_TLV).to_be_bytes());
        tlv.extend_from_slice(&2u16.to_be_bytes());
        tlv.extend_from_slice(&ours.to_be_bytes());
        let reply = eap::write_packet(code::RESPONSE, identifier, Some(method::EXTENSIONS), &tlv);
        match (status, ours) {
            (Some(RESULT_FAILURE), _) if !self.proven && self.expected.is_some() => {
                Step::Failed(InnerFailure::WrongPassword)
            }
            _ => Step::Reply(reply),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_then_a_challenge_then_a_result_the_server_proved() {
        let mut phase = InnerPhase::new(b"User", b"clientPass", [0x21; 16]);
        assert_eq!(
            phase.answer(&[method::IDENTITY], 4),
            Some(Step::Reply(vec![method::IDENTITY, b'U', b's', b'e', b'r']))
        );
        let mut challenge = vec![method::MSCHAPV2, mschapv2::opcode::CHALLENGE, 9, 0, 21, 16];
        challenge.extend_from_slice(&[0x5b; 16]);
        let Some(Step::Reply(response)) = phase.answer(&challenge, 5) else {
            panic!("no response");
        };
        assert_eq!(response[0], method::MSCHAPV2);
        assert_eq!(response[1], mschapv2::opcode::RESPONSE);
        assert!(!phase.server_proven());
        let result = eap::write_packet(
            code::REQUEST,
            7,
            Some(method::EXTENSIONS),
            &[0x80, 0x03, 0, 2, 0, 1],
        );
        let Some(Step::Reply(echo)) = phase.answer(&result, 7) else {
            panic!("no echo");
        };
        assert_eq!(&echo[eap::HEADER_BYTES + 1..], &[0x80, 0x03, 0, 2, 0, 2]);
        assert!(!phase.succeeded());
    }

    #[test]
    fn an_mschapv2_failure_is_a_wrong_password_and_a_bad_proof_an_untrusted_server() {
        let mut phase = InnerPhase::new(b"u", b"p", [1; 16]);
        assert_eq!(
            phase.answer(&[method::MSCHAPV2, mschapv2::opcode::FAILURE, 1, 0, 4], 1),
            Some(Step::Failed(InnerFailure::WrongPassword))
        );
        let mut success = vec![method::MSCHAPV2, mschapv2::opcode::SUCCESS, 1, 0, 46];
        success.extend_from_slice(&[b'S', b'=']);
        success.extend_from_slice(&[b'0'; 40]);
        assert_eq!(
            phase.answer(&success, 1),
            Some(Step::Failed(InnerFailure::ServerNotTrusted))
        );
        assert_eq!(phase.answer(&[0], 1), None);
    }
}
