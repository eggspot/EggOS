//! The TLS 1.3 server (RFC 8446) EggOS's own services answer with: one full handshake with
//! X25519 key exchange, its self-signed P-256 certificate ([`crate::certificate`]) and an ECDSA
//! signature over the handshake, then the same records as the client ([`Session`]). No
//! HelloRetryRequest (a client must offer an X25519 key share), no session tickets, no early
//! data, no client certificates.

use crate::client::{
    APPLICATION_PROTOCOLS, Buffers, CERTIFICATE, CERTIFICATE_VERIFY, CHANGE_CIPHER_SPEC,
    CLIENT_HELLO, CipherSuite, Cursor, ENCRYPTED_EXTENSIONS, Error as RecordError, FINISHED,
    HANDSHAKE, HEADER_BYTES, Handshaking, KEY_SHARE, Protection, SERVER_HELLO,
    SIGNATURE_ALGORITHMS, SUPPORTED_GROUPS, SUPPORTED_VERSIONS, Session, TLS13, Transcript,
    Transport, Writer, X25519_GROUP, derive_secret, early_secret, empty_hash, expand_label,
    send_protected,
};
use eggos_crypto::p256::{self, SCALAR_BYTES};
use eggos_crypto::{constant_time, hmac, x25519};

/// The bytes of randomness a handshake takes: the server random and the X25519 secret.
pub const RANDOMNESS_BYTES: usize = 64;
/// The longest application protocol name kept from a client's list.
pub const MAX_PROTOCOL_BYTES: usize = 32;

const ECDSA_P256_SHA256: u16 = 0x0403;
const SUITE_PREFERENCE: [u16; 3] = [0x1301, 0x1303, 0x1302];
const SERVER_CONTEXT: &[u8] = b"TLS 1.3, server CertificateVerify\0";

/// Why a client's handshake failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The transport failed and said why.
    Transport,
    /// The client closed the connection during the handshake.
    Closed,
    /// The client sent bytes TLS 1.3 does not allow.
    Malformed,
    /// The client sent a fatal alert with this description.
    Alert(u8),
    /// The client does not offer TLS 1.3.
    NotTls13,
    /// The client offers no X25519 key share.
    NoCommonKeyExchange,
    /// The client offers no cipher suite the server has.
    NoCommonCipher,
    /// The client does not accept ECDSA with P-256 and SHA-256.
    NoCommonSignature,
    /// The client offers application protocols and none is the server's.
    NoCommonProtocol,
    /// A record failed its authentication.
    BadRecord,
    /// The client's Finished message is wrong: it does not have the same keys.
    BadFinished,
    /// A handshake message is longer than the server keeps.
    TooLarge,
    /// The server's private key cannot sign.
    BadKey,
}

impl Error {
    /// What went wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::Transport => "the connection failed",
            Self::Closed => "the client closed the connection during the TLS handshake",
            Self::Malformed => "the client sent something TLS 1.3 does not allow",
            Self::Alert(_) => "the client ended the TLS handshake with an alert",
            Self::NotTls13 => "the client does not speak TLS 1.3",
            Self::NoCommonKeyExchange => "the client offers no X25519 key exchange",
            Self::NoCommonCipher => "the client offers no cipher EggOS has",
            Self::NoCommonSignature => "the client does not accept ECDSA P-256 signatures",
            Self::NoCommonProtocol => {
                "the client wants an application protocol EggOS does not speak"
            }
            Self::BadRecord => "a TLS record failed its check: the data was changed on the way",
            Self::BadFinished => "the client's TLS handshake does not check out",
            Self::TooLarge => "the client's TLS handshake message is over 32 KiB",
            Self::BadKey => "the server's private key cannot sign",
        }
    }
}

impl From<RecordError> for Error {
    fn from(error: RecordError) -> Self {
        match error {
            RecordError::Transport => Self::Transport,
            RecordError::Closed => Self::Closed,
            RecordError::Alert(description) => Self::Alert(description),
            RecordError::BadRecord => Self::BadRecord,
            RecordError::TooLarge => Self::TooLarge,
            _ => Self::Malformed,
        }
    }
}

impl From<crate::client::Stopped> for Error {
    fn from(_: crate::client::Stopped) -> Self {
        Self::Transport
    }
}

/// What the server brings to a handshake.
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    /// The server's certificate, DER.
    pub certificate: &'a [u8],
    /// The private key of the certificate's public key.
    pub private_key: &'a [u8; SCALAR_BYTES],
    /// The one application protocol the server speaks, such as `http/1.1`.
    pub protocol: &'a [u8],
}

struct ClientHello {
    session_id: [u8; 32],
    session_id_length: usize,
    suite: CipherSuite,
    key_share: [u8; 32],
    protocol_offered: bool,
}

fn parse_client_hello(body: &[u8], protocol: &[u8]) -> Result<ClientHello, Error> {
    let mut cursor = Cursor { data: body };
    cursor.take(2)?;
    cursor.take(32)?;
    let session_id = cursor.vector(1)?;
    if session_id.len() > 32 {
        return Err(Error::Malformed);
    }
    let suites = cursor.vector(2)?;
    if cursor.vector(1)? != [0] {
        return Err(Error::Malformed);
    }
    let mut extensions = Cursor {
        data: cursor.vector(2)?,
    };
    cursor.finish()?;
    let offered = |code: u16| lists_code(suites, code);
    let suite = SUITE_PREFERENCE
        .into_iter()
        .find(|&code| offered(code))
        .and_then(CipherSuite::from_code)
        .ok_or(Error::NoCommonCipher)?;
    let mut tls13 = false;
    let mut x25519_offered = false;
    let mut signs = false;
    let mut key_share = None;
    let mut protocol_offered = None;
    while !extensions.data.is_empty() {
        let kind = extensions.number(2)? as u16;
        let mut data = Cursor {
            data: extensions.vector(2)?,
        };
        match kind {
            SUPPORTED_VERSIONS => {
                let versions = data.vector(1)?;
                tls13 = lists_code(versions, TLS13);
            }
            SUPPORTED_GROUPS => {
                let groups = data.vector(2)?;
                x25519_offered = lists_code(groups, X25519_GROUP);
            }
            SIGNATURE_ALGORITHMS => {
                let schemes = data.vector(2)?;
                signs = lists_code(schemes, ECDSA_P256_SHA256);
            }
            KEY_SHARE => {
                let mut shares = Cursor {
                    data: data.vector(2)?,
                };
                while !shares.data.is_empty() {
                    let group = shares.number(2)? as u16;
                    let share = shares.vector(2)?;
                    if group == X25519_GROUP && share.len() == 32 && key_share.is_none() {
                        let mut copied = [0u8; 32];
                        copied.copy_from_slice(share);
                        key_share = Some(copied);
                    }
                }
            }
            APPLICATION_PROTOCOLS => {
                let mut names = Cursor {
                    data: data.vector(2)?,
                };
                let mut found = false;
                while !names.data.is_empty() {
                    found |= names.vector(1)? == protocol;
                }
                protocol_offered = Some(found);
            }
            _ => continue,
        }
        data.finish()?;
    }
    if !tls13 {
        return Err(Error::NotTls13);
    }
    if !signs {
        return Err(Error::NoCommonSignature);
    }
    let key_share = match key_share {
        Some(share) if x25519_offered => share,
        _ => return Err(Error::NoCommonKeyExchange),
    };
    if protocol_offered == Some(false) {
        return Err(Error::NoCommonProtocol);
    }
    let mut hello = ClientHello {
        session_id: [0; 32],
        session_id_length: session_id.len(),
        suite,
        key_share,
        protocol_offered: protocol_offered.is_some(),
    };
    hello.session_id[..session_id.len()].copy_from_slice(session_id);
    Ok(hello)
}

fn lists_code(list: &[u8], code: u16) -> bool {
    list.as_chunks::<2>()
        .0
        .iter()
        .any(|pair| u16::from_be_bytes(*pair) == code)
}

fn write_server_hello(
    out: &mut [u8],
    random: &[u8],
    hello: &ClientHello,
    key_share: &[u8; 32],
) -> usize {
    let mut writer = Writer { out, length: 0 };
    writer.put(&[SERVER_HELLO]);
    let message = writer.open(3);
    writer.put_u16(0x0303);
    writer.put(random);
    writer.put(&[hello.session_id_length as u8]);
    writer.put(&hello.session_id[..hello.session_id_length]);
    writer.put_u16(match hello.suite {
        CipherSuite::Aes128GcmSha256 => 0x1301,
        CipherSuite::Aes256GcmSha384 => 0x1302,
        CipherSuite::ChaCha20Poly1305Sha256 => 0x1303,
    });
    writer.put(&[0]);
    let extensions = writer.open(2);
    writer.put_u16(SUPPORTED_VERSIONS);
    writer.put(&[0, 2]);
    writer.put_u16(TLS13);
    writer.put_u16(KEY_SHARE);
    writer.put(&[0, 36]);
    writer.put_u16(X25519_GROUP);
    writer.put(&[0, 32]);
    writer.put(key_share);
    writer.close(extensions, 2);
    writer.close(message, 3);
    writer.length
}

fn der_integer(out: &mut [u8], at: usize, value: &[u8]) -> usize {
    let first = value
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(value.len() - 1);
    let kept = &value[first..];
    let padded = usize::from(kept[0] & 0x80 != 0);
    out[at] = 0x02;
    out[at + 1] = (kept.len() + padded) as u8;
    out[at + 2] = 0;
    out[at + 2 + padded..at + 2 + padded + kept.len()].copy_from_slice(kept);
    2 + padded + kept.len()
}

fn signature_der(signature: &[u8; p256::SIGNATURE_BYTES], out: &mut [u8; 72]) -> usize {
    let mut length = 2;
    length += der_integer(out, length, &signature[..SCALAR_BYTES]);
    length += der_integer(out, length, &signature[SCALAR_BYTES..]);
    out[0] = 0x30;
    out[1] = (length - 2) as u8;
    length
}

fn send_message(
    transport: &mut dyn Transport,
    buffers: &mut Buffers,
    protection: &mut Protection,
    transcript: &mut Transcript,
    message: &[u8],
) -> Result<(), Error> {
    transcript.update(message);
    send_protected(
        transport,
        &mut buffers.outgoing,
        protection,
        HANDSHAKE,
        message,
    )?;
    Ok(())
}

fn finished_message(
    suite: CipherSuite,
    traffic_secret: &[u8],
    transcript: &Transcript,
) -> ([u8; 4 + 64], usize) {
    let hash = suite.hash();
    let mut key = [0u8; 64];
    let key = &mut key[..hash.output_bytes()];
    expand_label(hash, traffic_secret, b"finished", &[], key);
    let verify_data = hmac::hmac(hash, key, transcript.hash(hash).as_bytes());
    let mut finished = [0u8; 4 + 64];
    finished[0] = FINISHED;
    finished[3] = hash.output_bytes() as u8;
    finished[4..4 + hash.output_bytes()].copy_from_slice(verify_data.as_bytes());
    (finished, 4 + hash.output_bytes())
}

/// Answers a client's handshake over `transport` in `buffers`, as `identity`, with
/// `randomness` fresh from the system's generator.
pub fn accept<'b>(
    transport: &mut dyn Transport,
    buffers: &'b mut Buffers,
    identity: &Identity<'_>,
    randomness: &[u8; RANDOMNESS_BYTES],
) -> Result<Session<'b>, Error> {
    let mut state = Handshaking {
        incoming: Default::default(),
        filled: 0,
    };
    let length = state.next_message(transport, buffers, None)?;
    if buffers.handshake[0] != CLIENT_HELLO {
        return Err(Error::Malformed);
    }
    let hello = parse_client_hello(&buffers.handshake[4..length], identity.protocol)?;
    let mut transcript = Transcript::new();
    transcript.update(&buffers.handshake[..length]);
    state.consume(buffers, length);
    if state.filled != 0 {
        return Err(Error::Malformed);
    }

    let mut secret = [0u8; 32];
    secret.copy_from_slice(&randomness[32..]);
    let shared = x25519::x25519(&secret, &hello.key_share);
    if constant_time::equal(&shared, &[0; 32]) {
        return Err(Error::NoCommonKeyExchange);
    }
    let hello_length = write_server_hello(
        &mut buffers.outgoing[HEADER_BYTES..],
        &randomness[..32],
        &hello,
        &x25519::public_key(&secret),
    );
    transcript.update(&buffers.outgoing[HEADER_BYTES..HEADER_BYTES + hello_length]);
    buffers.outgoing[..HEADER_BYTES].copy_from_slice(&[
        HANDSHAKE,
        3,
        3,
        (hello_length >> 8) as u8,
        hello_length as u8,
    ]);
    transport.send(&buffers.outgoing[..HEADER_BYTES + hello_length])?;
    transport.send(&[CHANGE_CIPHER_SPEC, 3, 3, 0, 1, 1])?;

    let suite = hello.suite;
    let hash = suite.hash();
    let derived = derive_secret(
        hash,
        early_secret(hash).as_bytes(),
        b"derived",
        empty_hash(hash).as_bytes(),
    );
    let handshake_secret = hmac::extract(hash, derived.as_bytes(), &shared);
    let hello_hash = transcript.hash(hash);
    let client_handshake = derive_secret(
        hash,
        handshake_secret.as_bytes(),
        b"c hs traffic",
        hello_hash.as_bytes(),
    );
    let server_handshake = derive_secret(
        hash,
        handshake_secret.as_bytes(),
        b"s hs traffic",
        hello_hash.as_bytes(),
    );
    let derived = derive_secret(
        hash,
        handshake_secret.as_bytes(),
        b"derived",
        empty_hash(hash).as_bytes(),
    );
    let zeros = [0u8; 64];
    let master = hmac::extract(hash, derived.as_bytes(), &zeros[..hash.output_bytes()]);
    let mut server_protection = Protection::new(suite, server_handshake.as_bytes());
    let mut client_protection = Protection::new(suite, client_handshake.as_bytes());

    let mut extensions = [0u8; 8 + MAX_PROTOCOL_BYTES];
    let extensions_length = {
        let mut writer = Writer {
            out: &mut extensions,
            length: 0,
        };
        writer.put(&[ENCRYPTED_EXTENSIONS]);
        let message = writer.open(3);
        let list = writer.open(2);
        if hello.protocol_offered {
            writer.put_u16(APPLICATION_PROTOCOLS);
            let extension = writer.open(2);
            let names = writer.open(2);
            writer.put(&[identity.protocol.len() as u8]);
            writer.put(identity.protocol);
            writer.close(names, 2);
            writer.close(extension, 2);
        }
        writer.close(list, 2);
        writer.close(message, 3);
        writer.length
    };
    send_message(
        transport,
        buffers,
        &mut server_protection,
        &mut transcript,
        &extensions[..extensions_length],
    )?;

    let certificate_length = identity.certificate.len();
    let body_length = 1 + 3 + 3 + certificate_length + 2;
    let message_length = 4 + body_length;
    if message_length > buffers.handshake.len() {
        return Err(Error::TooLarge);
    }
    {
        let message = &mut buffers.handshake[..message_length];
        message[0] = CERTIFICATE;
        message[1..4].copy_from_slice(&(body_length as u32).to_be_bytes()[1..]);
        message[4] = 0;
        message[5..8].copy_from_slice(&((certificate_length + 5) as u32).to_be_bytes()[1..]);
        message[8..11].copy_from_slice(&(certificate_length as u32).to_be_bytes()[1..]);
        message[11..11 + certificate_length].copy_from_slice(identity.certificate);
        message[11 + certificate_length..].copy_from_slice(&[0, 0]);
    }
    transcript.update(&buffers.handshake[..message_length]);
    send_protected(
        transport,
        &mut buffers.outgoing,
        &mut server_protection,
        HANDSHAKE,
        &buffers.handshake[..message_length],
    )?;

    let mut content = [0x20u8; 64 + 34 + 64];
    content[64..64 + SERVER_CONTEXT.len()].copy_from_slice(SERVER_CONTEXT);
    let digest = transcript.hash(hash);
    let end = 64 + SERVER_CONTEXT.len() + digest.as_bytes().len();
    content[64 + SERVER_CONTEXT.len()..end].copy_from_slice(digest.as_bytes());
    let signed = eggos_crypto::sha2::digest(eggos_crypto::HashAlgorithm::Sha256, &content[..end]);
    let signature =
        p256::sign(identity.private_key, signed.as_bytes()).map_err(|_| Error::BadKey)?;
    let mut der = [0u8; 72];
    let der_length = signature_der(&signature, &mut der);
    let mut verify = [0u8; 4 + 4 + 72];
    verify[0] = CERTIFICATE_VERIFY;
    verify[3] = (4 + der_length) as u8;
    verify[4..6].copy_from_slice(&ECDSA_P256_SHA256.to_be_bytes());
    verify[6..8].copy_from_slice(&(der_length as u16).to_be_bytes());
    verify[8..8 + der_length].copy_from_slice(&der[..der_length]);
    send_message(
        transport,
        buffers,
        &mut server_protection,
        &mut transcript,
        &verify[..8 + der_length],
    )?;

    let (finished, finished_length) =
        finished_message(suite, server_handshake.as_bytes(), &transcript);
    send_message(
        transport,
        buffers,
        &mut server_protection,
        &mut transcript,
        &finished[..finished_length],
    )?;
    let finished_hash = transcript.hash(hash);
    let client_application = derive_secret(
        hash,
        master.as_bytes(),
        b"c ap traffic",
        finished_hash.as_bytes(),
    );
    let server_application = derive_secret(
        hash,
        master.as_bytes(),
        b"s ap traffic",
        finished_hash.as_bytes(),
    );

    let length = state.next_message(transport, buffers, Some(&mut client_protection))?;
    if buffers.handshake[0] != FINISHED {
        return Err(Error::Malformed);
    }
    let (wanted, wanted_length) = finished_message(suite, client_handshake.as_bytes(), &transcript);
    if !constant_time::equal(&wanted[..wanted_length], &buffers.handshake[..length]) {
        return Err(Error::BadFinished);
    }
    state.consume(buffers, length);

    Ok(Session {
        incoming: state.incoming,
        filled: state.filled,
        suite,
        read: Protection::new(suite, client_application.as_bytes()),
        write: Protection::new(suite, server_application.as_bytes()),
        read_secret: client_application,
        write_secret: server_application,
        pending_start: 0,
        pending_end: 0,
        closed: false,
        buffers,
    })
}
