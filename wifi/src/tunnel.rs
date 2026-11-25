//! TLS carried in EAP (D779): EggOS's TLS 1.3 client runs over EAP-TLS's or PEAP's framing as
//! if over a stream. EAP is lock-step, one response per request, so what TLS sends waits until
//! it next wants to read; then it goes out as responses, cut into fragments the authenticator
//! acknowledges, and the requests that answer are put back together into TLS's bytes.
//! An EAP success or failure ends the stream and is kept as the outcome.

use crate::eap::{self, code, flags};
use alloc::vec::Vec;
use eggos_tls::{Stopped, Transport};

/// The most TLS bytes put in one response.
pub const FRAGMENT_BYTES: usize = 1300;
/// The most TLS bytes a whole message the authenticator sends may hold.
pub const LARGEST_MESSAGE: usize = 64 * 1024;

/// Moves EAPOL frames to and from the authenticator.
pub trait Exchange {
    /// Sends `eapol` and gives the next EAPOL frame the authenticator sends (an EAP packet,
    /// or the EAPOL-Key frame that starts the four-way handshake); `None` when none came in
    /// time or the network sent the station away.
    fn exchange(&mut self, eapol: &[u8]) -> Option<Vec<u8>>;
}

/// How the authenticator ended the exchange, once it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Success,
    Failure,
    /// No request came in time, or one was not the method's.
    Lost,
}

/// TLS's stream over one EAP method's requests and responses.
pub struct TlsOverEap<'e> {
    exchange: &'e mut dyn Exchange,
    method: u8,
    identifier: u8,
    outgoing: Vec<u8>,
    incoming: Vec<u8>,
    read_at: usize,
    fragmenting: bool,
    outcome: Outcome,
    key_frame: Option<Vec<u8>>,
}

impl core::fmt::Debug for TlsOverEap<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TlsOverEap(method {}, {:?})", self.method, self.outcome)
    }
}

impl<'e> TlsOverEap<'e> {
    /// The stream of `method` (EAP-TLS or PEAP) after its start request `identifier`.
    pub fn new(exchange: &'e mut dyn Exchange, method: u8, identifier: u8) -> Self {
        Self {
            exchange,
            method,
            identifier,
            outgoing: Vec::new(),
            incoming: Vec::new(),
            read_at: 0,
            fragmenting: false,
            outcome: Outcome::Pending,
            key_frame: None,
        }
    }

    /// How the authenticator ended it, or [`Outcome::Pending`].
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// The last request's identifier.
    pub fn identifier(&self) -> u8 {
        self.identifier
    }

    /// The EAPOL-Key frame that came in place of an EAP success, if one did.
    pub fn take_key_frame(&mut self) -> Option<Vec<u8>> {
        self.key_frame.take()
    }

    fn next_response(&mut self) -> Vec<u8> {
        if self.outgoing.is_empty() {
            return eap::tls_response(self.identifier, self.method, 0, None, &[]);
        }
        let whole = self.outgoing.len();
        let piece = whole.min(FRAGMENT_BYTES);
        let more = piece < whole;
        let total = (more && !self.fragmenting).then_some(whole as u32);
        self.fragmenting = more;
        let response = eap::tls_response(
            self.identifier,
            self.method,
            if more { flags::MORE_FRAGMENTS } else { 0 },
            total,
            &self.outgoing[..piece],
        );
        self.outgoing.drain(..piece);
        response
    }

    fn ended(&mut self, outcome: Outcome) -> Stopped {
        self.outcome = outcome;
        Stopped
    }

    fn fill(&mut self) -> Result<(), Stopped> {
        self.incoming.clear();
        self.read_at = 0;
        loop {
            let response = self.next_response();
            let sending_more = !self.outgoing.is_empty();
            let Some(answer) = self.exchange.exchange(&response) else {
                return Err(self.ended(Outcome::Lost));
            };
            if eap::eapol_type(&answer) == Some(eap::EAPOL_KEY) {
                self.key_frame = Some(answer);
                return Err(self.ended(Outcome::Success));
            }
            let Some(packet) = eap::from_eapol(&answer) else {
                return Err(self.ended(Outcome::Lost));
            };
            match packet.code {
                code::SUCCESS => return Err(self.ended(Outcome::Success)),
                code::FAILURE => return Err(self.ended(Outcome::Failure)),
                code::REQUEST if packet.method == Some(self.method) => {}
                _ => return Err(self.ended(Outcome::Lost)),
            }
            self.identifier = packet.identifier;
            let Some(message) = eap::tls_message(packet.data) else {
                return Err(self.ended(Outcome::Lost));
            };
            if sending_more {
                continue;
            }
            if self.incoming.len() + message.data.len() > LARGEST_MESSAGE {
                return Err(self.ended(Outcome::Lost));
            }
            self.incoming.extend_from_slice(message.data);
            if message.flags & flags::MORE_FRAGMENTS == 0 {
                return Ok(());
            }
        }
    }
}

impl Transport for TlsOverEap<'_> {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        self.outgoing.extend_from_slice(bytes);
        Ok(())
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        if self.read_at >= self.incoming.len() {
            self.fill()?;
        }
        let count = buffer.len().min(self.incoming.len() - self.read_at);
        buffer[..count].copy_from_slice(&self.incoming[self.read_at..self.read_at + count]);
        self.read_at += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eap::{method, write_packet};
    use alloc::collections::VecDeque;

    struct Script {
        sent: Vec<Vec<u8>>,
        answers: VecDeque<Vec<u8>>,
    }

    impl Exchange for Script {
        fn exchange(&mut self, eapol: &[u8]) -> Option<Vec<u8>> {
            self.sent.push(eapol.to_vec());
            self.answers.pop_front()
        }
    }

    fn request(identifier: u8, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut body = vec![flags];
        body.extend_from_slice(data);
        eap::eapol(&write_packet(
            code::REQUEST,
            identifier,
            Some(method::TLS),
            &body,
        ))
    }

    #[test]
    fn long_sends_go_in_acknowledged_fragments_and_replies_are_joined() {
        let mut script = Script {
            sent: Vec::new(),
            answers: VecDeque::from([
                request(2, 0, b""),
                request(3, flags::MORE_FRAGMENTS, b"hel"),
                request(4, 0, b"lo"),
                eap::eapol(&write_packet(code::SUCCESS, 4, None, &[])),
            ]),
        };
        let mut tunnel = TlsOverEap::new(&mut script, method::TLS, 1);
        tunnel.send(&vec![7u8; FRAGMENT_BYTES + 10]).unwrap();
        let mut buffer = [0u8; 16];
        assert_eq!(tunnel.receive(&mut buffer).unwrap(), 5);
        assert_eq!(&buffer[..5], b"hello");
        assert!(tunnel.receive(&mut buffer).is_err());
        assert_eq!(tunnel.outcome(), Outcome::Success);
        let sent = &script.sent;
        assert_eq!(sent.len(), 4);
        let first = eap::from_eapol(&sent[0]).unwrap();
        assert_eq!(first.identifier, 1);
        let framing = eap::tls_message(first.data).unwrap();
        assert_eq!(
            framing.flags,
            flags::MORE_FRAGMENTS | flags::LENGTH_INCLUDED
        );
        assert_eq!(framing.data.len(), FRAGMENT_BYTES);
        let second = eap::from_eapol(&sent[1]).unwrap();
        assert_eq!(second.identifier, 2);
        assert_eq!(eap::tls_message(second.data).unwrap().data.len(), 10);
        let acknowledgement = eap::from_eapol(&sent[2]).unwrap();
        assert_eq!(acknowledgement.identifier, 3);
        assert!(eap::tls_message(acknowledgement.data).unwrap().data.is_empty());
    }
}
