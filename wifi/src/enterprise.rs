//! 802.1X as a supplicant (D779): the identity exchange, the choice of method (a NAK names
//! ours when the server proposes another), then EAP-TLS (RFC 5216 with TLS 1.3, RFC 9190) or
//! PEAP version 0 with MS-CHAPv2 inside (RFC 9427 for TLS 1.3), ending with the PMK the
//! four-way handshake takes: the first 32 bytes of the MSK, which TLS's exporter gives under
//! `EXPORTER_EAP_TLS_Key_Material` with the method's type as context. The server's certificate
//! must chain to the given roots and name the given domain; there is no way to skip that.

use crate::eap::{self, code, method};
use crate::handshake::PMK_BYTES;
use crate::mschapv2::CHALLENGE_BYTES;
use crate::peap::{InnerFailure, InnerPhase, Step};
use crate::tunnel::{Exchange, Outcome, TlsOverEap};
use alloc::vec::Vec;
use eggos_tls::client::{self, ClientIdentity, Settings};
use eggos_tls::{Buffers, Error, TrustAnchors};

const EXPORTER_LABEL: &[u8] = b"EXPORTER_EAP_TLS_Key_Material";
const KEY_MATERIAL_BYTES: usize = 128;
const MAX_ROUNDS: usize = 32;
const INNER_BYTES: usize = 4096;
const ANONYMOUS: &[u8] = b"anonymous";
const PROTOCOL_VERSION_ALERT: u8 = 70;
const CERTIFICATE_ALERTS: [u8; 8] = [42, 43, 44, 45, 46, 48, 49, 116];

/// How the station proves who it is.
#[derive(Debug, Clone, Copy)]
pub enum Method<'a> {
    /// PEAP with MS-CHAPv2: a user name and password.
    Peap { user: &'a [u8], password: &'a [u8] },
    /// EAP-TLS: a client certificate and its P-256 key.
    Tls { identity: ClientIdentity<'a> },
}

/// What the station signs in with.
#[derive(Debug, Clone, Copy)]
pub struct Credentials<'a> {
    pub method: Method<'a>,
    /// The identity sent in the clear before the tunnel; empty for PEAP means `anonymous`.
    pub outer_identity: &'a [u8],
    /// The name the server's certificate must carry.
    pub server_domain: &'a [u8],
}

/// Why signing in failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    NoAnswer,
    WrongPassword,
    ServerNotTrusted,
    CertificateRefused,
    UnsupportedMethod,
    Refused,
}

impl Failure {
    /// The code `/proc/driver/wifi` and the front-ends show.
    pub fn code(self) -> &'static str {
        match self {
            Self::NoAnswer => "no-answer",
            Self::WrongPassword => "wrong-password",
            Self::ServerNotTrusted => "server-not-trusted",
            Self::CertificateRefused => "certificate-refused",
            Self::UnsupportedMethod => "unsupported-method",
            Self::Refused => "refused",
        }
    }

    /// The same, for people.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::NoAnswer => "the network did not answer the sign-in",
            Self::WrongPassword => "the network did not accept the user name or password",
            Self::ServerNotTrusted => {
                "the server's certificate is not from the given CA or not for the given domain"
            }
            Self::CertificateRefused => "the network refused the client certificate",
            Self::UnsupportedMethod => "the network asks for a sign-in method EggOS does not offer",
            Self::Refused => "the network refused the sign-in",
        }
    }
}

/// A finished sign-in.
#[derive(Debug)]
pub struct Authenticated {
    pub pmk: [u8; PMK_BYTES],
    /// The handshake's first EAPOL-Key frame, when it came before EAP's success.
    pub key_frame: Option<Vec<u8>>,
}

/// The time, roots, buffers and randomness a sign-in uses.
pub struct Context<'c> {
    pub anchors: &'c mut dyn TrustAnchors,
    pub buffers: &'c mut Buffers,
    /// Seconds since 1970 (UTC), for the certificates' validity.
    pub now: i64,
    pub random: &'c mut dyn FnMut(&mut [u8]),
}

impl core::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Context(now {})", self.now)
    }
}

/// Signs in over `exchange`, starting from `first` (the first EAPOL frame the authenticator
/// sent) or, without one, from an EAPOL-Start.
pub fn authenticate(
    exchange: &mut dyn Exchange,
    first: Option<Vec<u8>>,
    credentials: &Credentials<'_>,
    context: &mut Context<'_>,
) -> Result<Authenticated, Failure> {
    let wanted = match credentials.method {
        Method::Peap { .. } => method::PEAP,
        Method::Tls { .. } => method::TLS,
    };
    let outer_identity = match (credentials.outer_identity, credentials.method) {
        ([], Method::Peap { .. }) => ANONYMOUS,
        (identity, _) => identity,
    };
    let mut frame = match first {
        Some(frame) => frame,
        None => exchange
            .exchange(&eap::eapol_start())
            .ok_or(Failure::NoAnswer)?,
    };
    let mut refused_theirs = false;
    for _ in 0..MAX_ROUNDS {
        let packet = eap::from_eapol(&frame).ok_or(Failure::Refused)?;
        let reply = match (packet.code, packet.method) {
            (code::FAILURE, _) if refused_theirs => return Err(Failure::UnsupportedMethod),
            (code::REQUEST, Some(method::IDENTITY)) => {
                eap::response(packet.identifier, method::IDENTITY, outer_identity)
            }
            (code::REQUEST, Some(method::NOTIFICATION)) => {
                eap::response(packet.identifier, method::NOTIFICATION, &[])
            }
            (code::REQUEST, Some(proposed)) if proposed == wanted => {
                let starts = eap::tls_message(packet.data)
                    .is_some_and(|message| message.flags & eap::flags::START != 0);
                if !starts {
                    return Err(Failure::Refused);
                }
                return run_method(exchange, packet.identifier, credentials, context);
            }
            (code::REQUEST, Some(_)) => {
                refused_theirs = true;
                eap::nak(packet.identifier, wanted)
            }
            _ => return Err(Failure::Refused),
        };
        frame = exchange.exchange(&reply).ok_or(Failure::NoAnswer)?;
    }
    Err(Failure::Refused)
}

fn tls_failure(error: Error, outcome: Outcome, with_certificate: bool) -> Failure {
    match error {
        Error::Certificate(_) | Error::BadSignature | Error::BadFinished => {
            Failure::ServerNotTrusted
        }
        Error::NotTls13 | Error::NoCommonKeyExchange | Error::NoCommonCipher => {
            Failure::UnsupportedMethod
        }
        Error::Alert(PROTOCOL_VERSION_ALERT) => Failure::UnsupportedMethod,
        Error::Alert(alert) if with_certificate && CERTIFICATE_ALERTS.contains(&alert) => {
            Failure::CertificateRefused
        }
        Error::BadClientKey => Failure::CertificateRefused,
        Error::Transport => match outcome {
            Outcome::Failure if with_certificate => Failure::CertificateRefused,
            Outcome::Lost | Outcome::Pending => Failure::NoAnswer,
            _ => Failure::Refused,
        },
        _ => Failure::Refused,
    }
}

fn run_method(
    exchange: &mut dyn Exchange,
    identifier: u8,
    credentials: &Credentials<'_>,
    context: &mut Context<'_>,
) -> Result<Authenticated, Failure> {
    let (kind, identity) = match credentials.method {
        Method::Peap { .. } => (method::PEAP, None),
        Method::Tls { identity } => (method::TLS, Some(identity)),
    };
    let mut tunnel = TlsOverEap::new(exchange, kind, identifier);
    let settings = Settings {
        host: credentials.server_domain,
        now: context.now,
    };
    let mut randomness = [0u8; client::RANDOMNESS_BYTES];
    (context.random)(&mut randomness);
    let connected = client::connect_as(
        &mut tunnel,
        context.buffers,
        &settings,
        &randomness,
        context.anchors,
        identity.as_ref(),
    );
    randomness.fill(0);
    let mut session = match connected {
        Ok(session) => session,
        Err(error) => return Err(tls_failure(error, tunnel.outcome(), identity.is_some())),
    };
    match credentials.method {
        Method::Tls { .. } => {
            let mut data = [0u8; 16];
            for _ in 0..MAX_ROUNDS {
                match session.read(&mut tunnel, &mut data) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            match tunnel.outcome() {
                Outcome::Success => {}
                Outcome::Failure => return Err(Failure::CertificateRefused),
                _ => return Err(Failure::NoAnswer),
            }
        }
        Method::Peap { user, password } => {
            let mut challenge = [0u8; CHALLENGE_BYTES];
            (context.random)(&mut challenge);
            let mut phase = InnerPhase::new(user, password, challenge);
            let mut inner = alloc::vec![0u8; INNER_BYTES];
            for _ in 0..MAX_ROUNDS {
                let length = match session.read(&mut tunnel, &mut inner) {
                    Ok(0) | Err(_) => break,
                    Ok(length) => length,
                };
                match phase.answer(&inner[..length], tunnel.identifier()) {
                    None => {}
                    Some(Step::Reply(bytes)) => {
                        if session.write(&mut tunnel, &bytes).is_err() {
                            break;
                        }
                    }
                    Some(Step::Failed(InnerFailure::WrongPassword)) => {
                        return Err(Failure::WrongPassword);
                    }
                    Some(Step::Failed(InnerFailure::ServerNotTrusted)) => {
                        return Err(Failure::ServerNotTrusted);
                    }
                    Some(Step::Failed(InnerFailure::Refused)) => return Err(Failure::Refused),
                }
            }
            match tunnel.outcome() {
                Outcome::Success if phase.succeeded() => {}
                Outcome::Success => return Err(Failure::ServerNotTrusted),
                Outcome::Failure if phase.server_proven() => return Err(Failure::Refused),
                Outcome::Failure => return Err(Failure::WrongPassword),
                _ => return Err(Failure::NoAnswer),
            }
        }
    }
    let mut material = [0u8; KEY_MATERIAL_BYTES];
    if !session.export(EXPORTER_LABEL, &[kind], &mut material) {
        return Err(Failure::Refused);
    }
    let mut pmk = [0u8; PMK_BYTES];
    pmk.copy_from_slice(&material[..PMK_BYTES]);
    material.fill(0);
    Ok(Authenticated {
        pmk,
        key_frame: tunnel.take_key_frame(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    struct NoRoots;

    impl TrustAnchors for NoRoots {
        fn each(
            &mut self,
            _: &mut dyn FnMut(&[u8]) -> bool,
        ) -> Result<bool, eggos_tls::chain::NoRoots> {
            Ok(false)
        }
    }

    fn request(identifier: u8, kind: u8, data: &[u8]) -> Vec<u8> {
        eap::eapol(&eap::write_packet(
            code::REQUEST,
            identifier,
            Some(kind),
            data,
        ))
    }

    #[test]
    fn identity_goes_out_and_another_method_is_refused_with_a_nak() {
        let mut script = Script {
            sent: Vec::new(),
            answers: VecDeque::from([
                request(2, 21, &[0x20]),
                eap::eapol(&eap::write_packet(code::FAILURE, 2, None, &[])),
            ]),
        };
        let credentials = Credentials {
            method: Method::Peap {
                user: b"ana",
                password: b"secret",
            },
            outer_identity: b"",
            server_domain: b"radius.example.com",
        };
        let mut buffers = Buffers::new();
        let mut random = |out: &mut [u8]| out.fill(9);
        let mut context = Context {
            anchors: &mut NoRoots,
            buffers: &mut buffers,
            now: 0,
            random: &mut random,
        };
        let outcome = authenticate(
            &mut script,
            Some(request(1, method::IDENTITY, b"")),
            &credentials,
            &mut context,
        );
        assert_eq!(outcome.err(), Some(Failure::UnsupportedMethod));
        let identity = eap::from_eapol(&script.sent[0]).unwrap();
        assert_eq!(identity.data, b"anonymous");
        let nak = eap::from_eapol(&script.sent[1]).unwrap();
        assert_eq!(nak.method, Some(method::NAK));
        assert_eq!(nak.data, [method::PEAP]);
    }
}
