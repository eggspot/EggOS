//! The TLS 1.3 client (RFC 8446): one full handshake with X25519 key exchange, the server's
//! certificates checked ([`crate::chain`]) and its signature and Finished verified, then
//! application data in records protected with AES-128-GCM, AES-256-GCM or
//! ChaCha20-Poly1305. No session resumption, no early data, no client certificates (an empty
//! one answers a request), no HelloRetryRequest: X25519 is the only group offered.

use crate::chain::{self, CertificateError, MAX_CHAIN, TrustAnchors};
use crate::x509::{self, PublicKey, SignatureAlgorithm};
use core::fmt;
use eggos_crypto::chacha20poly1305::ChaCha20Poly1305;
use eggos_crypto::ecdsa::Curve;
use eggos_crypto::gcm::AesGcm;
use eggos_crypto::hmac;
use eggos_crypto::sha2::{Digest, HashAlgorithm, Hasher};
use eggos_crypto::{constant_time, x25519};

/// The most bytes of data a record carries.
pub const MAX_PLAINTEXT: usize = 16384;
pub(crate) const MAX_CIPHERTEXT: usize = MAX_PLAINTEXT + 256;
pub(crate) const HEADER_BYTES: usize = 5;
pub(crate) const TAG_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
/// The bytes of the buffer incoming records are read into: two whole records.
pub const INCOMING_BYTES: usize = 2 * (HEADER_BYTES + MAX_CIPHERTEXT);
/// The longest handshake message kept, which bounds the server's certificates.
pub const HANDSHAKE_BYTES: usize = 32 * 1024;
pub(crate) const OUTGOING_BYTES: usize = HEADER_BYTES + MAX_PLAINTEXT + 1 + TAG_BYTES;
/// The bytes of randomness a handshake takes: the client random, the legacy session ID and
/// the X25519 secret.
pub const RANDOMNESS_BYTES: usize = 96;

pub(crate) const CHANGE_CIPHER_SPEC: u8 = 20;
pub(crate) const ALERT: u8 = 21;
pub(crate) const HANDSHAKE: u8 = 22;
pub(crate) const APPLICATION_DATA: u8 = 23;

pub(crate) const CLIENT_HELLO: u8 = 1;
pub(crate) const SERVER_HELLO: u8 = 2;
const NEW_SESSION_TICKET: u8 = 4;
pub(crate) const ENCRYPTED_EXTENSIONS: u8 = 8;
pub(crate) const CERTIFICATE: u8 = 11;
const CERTIFICATE_REQUEST: u8 = 13;
pub(crate) const CERTIFICATE_VERIFY: u8 = 15;
pub(crate) const FINISHED: u8 = 20;
const KEY_UPDATE: u8 = 24;

const SERVER_NAME: u16 = 0;
pub(crate) const SUPPORTED_GROUPS: u16 = 10;
pub(crate) const SIGNATURE_ALGORITHMS: u16 = 13;
pub(crate) const APPLICATION_PROTOCOLS: u16 = 16;
pub(crate) const SUPPORTED_VERSIONS: u16 = 43;
pub(crate) const KEY_SHARE: u16 = 51;
pub(crate) const X25519_GROUP: u16 = 0x001d;
pub(crate) const TLS13: u16 = 0x0304;

const SIGNATURE_SCHEMES: [u16; 8] = [
    0x0403, 0x0503, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601,
];

const RETRY_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// The transport stopped: it failed (and already said why) or was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

/// The bytes under TLS: a connected stream.
pub trait Transport {
    /// Sends all of `bytes`.
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped>;
    /// Receives some bytes into `buffer`: how many, 0 when the peer closed.
    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped>;
}

/// Why a connection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The transport failed and said why.
    Transport,
    /// The server closed the connection during the handshake.
    Closed,
    /// The server sent bytes TLS 1.3 does not allow.
    Malformed,
    /// The server sent a fatal alert with this description.
    Alert(u8),
    /// The server does not speak TLS 1.3.
    NotTls13,
    /// The server wants a key exchange other than X25519.
    NoCommonKeyExchange,
    /// The server chose a cipher suite that was not offered.
    NoCommonCipher,
    /// A record failed its authentication.
    BadRecord,
    /// The server's certificates were refused.
    Certificate(CertificateError),
    /// The server's signature over the handshake is wrong or uses a scheme not offered.
    BadSignature,
    /// The server's Finished message is wrong.
    BadFinished,
    /// A handshake message is longer than the client keeps.
    TooLarge,
}

impl Error {
    /// What went wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::Transport => "the connection failed",
            Self::Closed => "the server closed the connection during the TLS handshake",
            Self::Malformed => "the server sent something TLS 1.3 does not allow",
            Self::Alert(_) => "the server ended the TLS handshake with an alert",
            Self::NotTls13 => "the server does not speak TLS 1.3",
            Self::NoCommonKeyExchange => "the server wants a key exchange EggOS does not offer",
            Self::NoCommonCipher => "the server chose a cipher EggOS did not offer",
            Self::BadRecord => "a TLS record failed its check: the data was changed on the way",
            Self::Certificate(error) => error.description(),
            Self::BadSignature => "the server's TLS signature is wrong",
            Self::BadFinished => "the server's TLS handshake does not check out",
            Self::TooLarge => "the server's TLS handshake message is over 32 KiB",
        }
    }
}

impl From<Stopped> for Error {
    fn from(_: Stopped) -> Self {
        Self::Transport
    }
}

/// A cipher suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    /// `TLS_AES_128_GCM_SHA256`.
    Aes128GcmSha256,
    /// `TLS_AES_256_GCM_SHA384`.
    Aes256GcmSha384,
    /// `TLS_CHACHA20_POLY1305_SHA256`.
    ChaCha20Poly1305Sha256,
}

const OFFERED_SUITES: [u16; 3] = [0x1303, 0x1301, 0x1302];

impl CipherSuite {
    pub(crate) fn from_code(code: u16) -> Option<Self> {
        match code {
            0x1301 => Some(Self::Aes128GcmSha256),
            0x1302 => Some(Self::Aes256GcmSha384),
            0x1303 => Some(Self::ChaCha20Poly1305Sha256),
            _ => None,
        }
    }

    /// The suite's hash.
    pub const fn hash(self) -> HashAlgorithm {
        match self {
            Self::Aes256GcmSha384 => HashAlgorithm::Sha384,
            _ => HashAlgorithm::Sha256,
        }
    }

    const fn key_bytes(self) -> usize {
        match self {
            Self::Aes128GcmSha256 => 16,
            _ => 32,
        }
    }

    /// The suite's name in the IANA registry.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aes128GcmSha256 => "TLS_AES_128_GCM_SHA256",
            Self::Aes256GcmSha384 => "TLS_AES_256_GCM_SHA384",
            Self::ChaCha20Poly1305Sha256 => "TLS_CHACHA20_POLY1305_SHA256",
        }
    }
}

/// HKDF-Expand-Label (RFC 8446 section 7.1) into `out`.
pub fn expand_label(
    hash: HashAlgorithm,
    secret: &[u8],
    label: &[u8],
    context: &[u8],
    out: &mut [u8],
) {
    let length = (out.len() as u16).to_be_bytes();
    let label_length = [(6 + label.len()) as u8];
    let context_length = [context.len() as u8];
    let info: [&[u8]; 6] = [
        &length,
        &label_length,
        b"tls13 ",
        label,
        &context_length,
        context,
    ];
    let _ = hmac::expand(hash, secret, &info, out);
}

/// Derive-Secret (RFC 8446 section 7.1): a secret from `secret`, `label` and a transcript's
/// hash.
pub fn derive_secret(
    hash: HashAlgorithm,
    secret: &[u8],
    label: &[u8],
    transcript: &[u8],
) -> Digest {
    let mut out = [0u8; 64];
    let length = hash.output_bytes();
    expand_label(hash, secret, label, transcript, &mut out[..length]);
    Digest::from_bytes(&out[..length]).unwrap_or_else(|| empty_hash(hash))
}

pub(crate) fn empty_hash(hash: HashAlgorithm) -> Digest {
    Hasher::new(hash).finish()
}

/// The early secret with no pre-shared key: HKDF-Extract of zeros.
pub fn early_secret(hash: HashAlgorithm) -> Digest {
    let zeros = [0u8; 64];
    hmac::extract(hash, &[], &zeros[..hash.output_bytes()])
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
enum Aead {
    Gcm(AesGcm),
    ChaCha(ChaCha20Poly1305),
}

/// One direction's record protection: the AEAD, its IV and the record count.
#[derive(Clone)]
pub struct Protection {
    aead: Aead,
    iv: [u8; NONCE_BYTES],
    sequence: u64,
}

impl fmt::Debug for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Protection(record {})", self.sequence)
    }
}

impl Protection {
    /// The protection keyed by a traffic `secret` of `suite`.
    pub fn new(suite: CipherSuite, secret: &[u8]) -> Self {
        let hash = suite.hash();
        let mut key = [0u8; 32];
        let key = &mut key[..suite.key_bytes()];
        expand_label(hash, secret, b"key", &[], key);
        let mut iv = [0u8; NONCE_BYTES];
        expand_label(hash, secret, b"iv", &[], &mut iv);
        let aead = match suite {
            CipherSuite::ChaCha20Poly1305Sha256 => {
                let mut whole = [0u8; 32];
                whole.copy_from_slice(key);
                Aead::ChaCha(ChaCha20Poly1305::new(&whole))
            }
            _ => match AesGcm::new(key) {
                Ok(cipher) => Aead::Gcm(cipher),
                Err(_) => Aead::ChaCha(ChaCha20Poly1305::new(&[0; 32])),
            },
        };
        Self {
            aead,
            iv,
            sequence: 0,
        }
    }

    fn nonce(&mut self) -> [u8; NONCE_BYTES] {
        let mut nonce = self.iv;
        for (byte, count) in nonce[4..].iter_mut().zip(self.sequence.to_be_bytes()) {
            *byte ^= count;
        }
        self.sequence = self.sequence.wrapping_add(1);
        nonce
    }

    /// Protects the record whose header is `header` and whose inner plaintext is `data`, in
    /// place; returns the tag.
    pub fn seal(&mut self, header: &[u8; HEADER_BYTES], data: &mut [u8]) -> [u8; TAG_BYTES] {
        let nonce = self.nonce();
        match &self.aead {
            Aead::Gcm(cipher) => cipher.seal(&nonce, header, data),
            Aead::ChaCha(cipher) => cipher.seal(&nonce, header, data),
        }
    }

    /// Checks and decrypts a protected record's body in place.
    pub fn open(
        &mut self,
        header: &[u8; HEADER_BYTES],
        data: &mut [u8],
        tag: &[u8],
    ) -> Result<(), Error> {
        let nonce = self.nonce();
        let opened = match &self.aead {
            Aead::Gcm(cipher) => cipher.open(&nonce, header, data, tag),
            Aead::ChaCha(cipher) => cipher.open(&nonce, header, data, tag),
        };
        opened.map_err(|_| Error::BadRecord)
    }
}

/// The memory a connection works in: incoming records, handshake messages and an outgoing
/// record.
pub struct Buffers {
    pub(crate) incoming: [u8; INCOMING_BYTES],
    pub(crate) handshake: [u8; HANDSHAKE_BYTES],
    pub(crate) outgoing: [u8; OUTGOING_BYTES],
}

impl fmt::Debug for Buffers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Buffers")
    }
}

impl Default for Buffers {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffers {
    /// Empty buffers.
    pub const fn new() -> Self {
        Self {
            incoming: [0; INCOMING_BYTES],
            handshake: [0; HANDSHAKE_BYTES],
            outgoing: [0; OUTGOING_BYTES],
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Incoming {
    start: usize,
    end: usize,
}

impl Incoming {
    pub(crate) fn next_record(
        &mut self,
        transport: &mut dyn Transport,
        buffer: &mut [u8; INCOMING_BYTES],
    ) -> Result<(u8, usize, usize), Error> {
        loop {
            let available = self.end - self.start;
            if available >= HEADER_BYTES {
                let at = self.start;
                let length = usize::from(u16::from_be_bytes([buffer[at + 3], buffer[at + 4]]));
                if length > MAX_CIPHERTEXT {
                    return Err(Error::Malformed);
                }
                if available >= HEADER_BYTES + length {
                    self.start += HEADER_BYTES + length;
                    return Ok((buffer[at], at, HEADER_BYTES + length));
                }
            }
            if self.start > 0 {
                buffer.copy_within(self.start..self.end, 0);
                self.end -= self.start;
                self.start = 0;
            }
            match transport.receive(&mut buffer[self.end..])? {
                0 => return Err(Error::Closed),
                count => self.end += count,
            }
        }
    }
}

pub(crate) fn unprotect(
    protection: &mut Protection,
    buffer: &mut [u8; INCOMING_BYTES],
    at: usize,
    length: usize,
) -> Result<(u8, usize, usize), Error> {
    let mut header = [0u8; HEADER_BYTES];
    header.copy_from_slice(&buffer[at..at + HEADER_BYTES]);
    if header[0] != APPLICATION_DATA || length < HEADER_BYTES + TAG_BYTES + 1 {
        return Err(Error::Malformed);
    }
    let body = &mut buffer[at + HEADER_BYTES..at + length];
    let (data, tag) = body.split_at_mut(length - HEADER_BYTES - TAG_BYTES);
    protection.open(&header, data, tag)?;
    let end = data
        .iter()
        .rposition(|&byte| byte != 0)
        .ok_or(Error::Malformed)?;
    if end > MAX_PLAINTEXT {
        return Err(Error::Malformed);
    }
    Ok((data[end], at + HEADER_BYTES, end))
}

pub(crate) fn send_protected(
    transport: &mut dyn Transport,
    outgoing: &mut [u8; OUTGOING_BYTES],
    protection: &mut Protection,
    content_type: u8,
    payload: &[u8],
) -> Result<(), Error> {
    let mut pieces = payload.chunks(MAX_PLAINTEXT);
    let mut piece = pieces.next().unwrap_or_default();
    loop {
        let inner = piece.len() + 1;
        let length = inner + TAG_BYTES;
        let header = [APPLICATION_DATA, 3, 3, (length >> 8) as u8, length as u8];
        outgoing[..HEADER_BYTES].copy_from_slice(&header);
        outgoing[HEADER_BYTES..HEADER_BYTES + piece.len()].copy_from_slice(piece);
        outgoing[HEADER_BYTES + piece.len()] = content_type;
        let tag = protection.seal(&header, &mut outgoing[HEADER_BYTES..HEADER_BYTES + inner]);
        outgoing[HEADER_BYTES + inner..HEADER_BYTES + length].copy_from_slice(&tag);
        transport.send(&outgoing[..HEADER_BYTES + length])?;
        match pieces.next() {
            Some(next) => piece = next,
            None => return Ok(()),
        }
    }
}

pub(crate) struct Cursor<'a> {
    pub(crate) data: &'a [u8],
}

impl<'a> Cursor<'a> {
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if self.data.len() < count {
            return Err(Error::Malformed);
        }
        let (taken, rest) = self.data.split_at(count);
        self.data = rest;
        Ok(taken)
    }

    pub(crate) fn number(&mut self, bytes: usize) -> Result<usize, Error> {
        Ok(self
            .take(bytes)?
            .iter()
            .fold(0usize, |value, &byte| (value << 8) | usize::from(byte)))
    }

    pub(crate) fn vector(&mut self, length_bytes: usize) -> Result<&'a [u8], Error> {
        let length = self.number(length_bytes)?;
        self.take(length)
    }

    pub(crate) fn finish(&self) -> Result<(), Error> {
        match self.data.is_empty() {
            true => Ok(()),
            false => Err(Error::Malformed),
        }
    }
}

pub(crate) struct Writer<'a> {
    pub(crate) out: &'a mut [u8],
    pub(crate) length: usize,
}

impl Writer<'_> {
    pub(crate) fn put(&mut self, bytes: &[u8]) {
        let end = (self.length + bytes.len()).min(self.out.len());
        let count = end - self.length;
        self.out[self.length..end].copy_from_slice(&bytes[..count]);
        self.length = end;
    }

    pub(crate) fn put_u16(&mut self, value: u16) {
        self.put(&value.to_be_bytes());
    }

    pub(crate) fn open(&mut self, length_bytes: usize) -> usize {
        let at = self.length;
        self.put(&[0, 0, 0][..length_bytes]);
        at
    }

    pub(crate) fn close(&mut self, at: usize, length_bytes: usize) {
        let length = self.length - at - length_bytes;
        for index in 0..length_bytes {
            if let Some(byte) = self.out.get_mut(at + index) {
                *byte = (length >> (8 * (length_bytes - 1 - index))) as u8;
            }
        }
    }
}

fn is_ipv4_literal(host: &[u8]) -> bool {
    !host.is_empty()
        && host
            .iter()
            .all(|&byte| byte.is_ascii_digit() || byte == b'.')
}

fn write_client_hello(
    out: &mut [u8],
    host: &[u8],
    randomness: &[u8; RANDOMNESS_BYTES],
    key_share: &[u8; 32],
) -> usize {
    let mut writer = Writer { out, length: 0 };
    writer.put(&[CLIENT_HELLO]);
    let message = writer.open(3);
    writer.put_u16(0x0303);
    writer.put(&randomness[..32]);
    writer.put(&[32]);
    writer.put(&randomness[32..64]);
    writer.put_u16(2 * OFFERED_SUITES.len() as u16);
    for suite in OFFERED_SUITES {
        writer.put_u16(suite);
    }
    writer.put(&[1, 0]);
    let extensions = writer.open(2);
    if !is_ipv4_literal(host) {
        writer.put_u16(SERVER_NAME);
        let extension = writer.open(2);
        let list = writer.open(2);
        writer.put(&[0]);
        writer.put_u16(host.len() as u16);
        writer.put(host);
        writer.close(list, 2);
        writer.close(extension, 2);
    }
    writer.put_u16(SUPPORTED_VERSIONS);
    writer.put(&[0, 3, 2]);
    writer.put_u16(TLS13);
    writer.put_u16(SUPPORTED_GROUPS);
    writer.put(&[0, 4, 0, 2]);
    writer.put_u16(X25519_GROUP);
    writer.put_u16(SIGNATURE_ALGORITHMS);
    writer.put_u16(2 + 2 * SIGNATURE_SCHEMES.len() as u16);
    writer.put_u16(2 * SIGNATURE_SCHEMES.len() as u16);
    for scheme in SIGNATURE_SCHEMES {
        writer.put_u16(scheme);
    }
    writer.put_u16(KEY_SHARE);
    writer.put(&[0, 38, 0, 36]);
    writer.put_u16(X25519_GROUP);
    writer.put(&[0, 32]);
    writer.put(key_share);
    writer.put_u16(APPLICATION_PROTOCOLS);
    writer.put(&[0, 11, 0, 9, 8]);
    writer.put(b"http/1.1");
    writer.close(extensions, 2);
    writer.close(message, 3);
    writer.length
}

struct ServerHello<'a> {
    suite: CipherSuite,
    key_share: &'a [u8],
}

fn parse_server_hello<'a>(body: &'a [u8], session_id: &[u8]) -> Result<ServerHello<'a>, Error> {
    let mut cursor = Cursor { data: body };
    cursor.take(2)?;
    if cursor.take(32)? == RETRY_RANDOM {
        return Err(Error::NoCommonKeyExchange);
    }
    let echoed = cursor.vector(1)?;
    let suite = cursor.number(2)? as u16;
    if cursor.take(1)? != [0] {
        return Err(Error::Malformed);
    }
    let mut extensions = Cursor {
        data: cursor.vector(2)?,
    };
    cursor.finish()?;
    let mut version = None;
    let mut key_share = None;
    while !extensions.data.is_empty() {
        let kind = extensions.number(2)? as u16;
        let mut data = Cursor {
            data: extensions.vector(2)?,
        };
        match kind {
            SUPPORTED_VERSIONS => version = Some(data.number(2)? as u16),
            KEY_SHARE => {
                if data.number(2)? as u16 != X25519_GROUP {
                    return Err(Error::NoCommonKeyExchange);
                }
                key_share = Some(data.vector(2)?);
            }
            _ => continue,
        }
        data.finish()?;
    }
    if version != Some(TLS13) {
        return Err(Error::NotTls13);
    }
    if echoed != session_id {
        return Err(Error::Malformed);
    }
    let suite = CipherSuite::from_code(suite).ok_or(Error::NoCommonCipher)?;
    match key_share {
        Some(key_share) if key_share.len() == 32 => Ok(ServerHello { suite, key_share }),
        _ => Err(Error::Malformed),
    }
}

#[derive(Clone)]
pub(crate) struct Transcript {
    sha256: Hasher,
    sha384: Hasher,
}

impl Transcript {
    pub(crate) fn new() -> Self {
        Self {
            sha256: Hasher::new(HashAlgorithm::Sha256),
            sha384: Hasher::new(HashAlgorithm::Sha384),
        }
    }

    pub(crate) fn update(&mut self, message: &[u8]) {
        self.sha256.update(message);
        self.sha384.update(message);
    }

    pub(crate) fn hash(&self, hash: HashAlgorithm) -> Digest {
        match hash {
            HashAlgorithm::Sha384 => self.sha384.clone().finish(),
            _ => self.sha256.clone().finish(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Ec(Curve),
    Rsa,
}

struct LeafKey {
    kind: KeyKind,
    bytes: [u8; 640],
    first: usize,
    second: usize,
}

impl LeafKey {
    fn copy(key: &PublicKey<'_>) -> Result<Self, Error> {
        let unsupported = Error::Certificate(CertificateError::UnsupportedAlgorithm);
        let mut copied = Self {
            kind: KeyKind::Rsa,
            bytes: [0; 640],
            first: 0,
            second: 0,
        };
        let (first, second): (&[u8], &[u8]) = match key {
            PublicKey::Ec { curve, point } => {
                copied.kind = KeyKind::Ec(*curve);
                (point, &[])
            }
            PublicKey::Rsa { modulus, exponent } => (modulus, exponent),
        };
        if first.len() + second.len() > copied.bytes.len() {
            return Err(unsupported);
        }
        copied.bytes[..first.len()].copy_from_slice(first);
        copied.bytes[first.len()..first.len() + second.len()].copy_from_slice(second);
        copied.first = first.len();
        copied.second = second.len();
        Ok(copied)
    }

    fn key(&self) -> PublicKey<'_> {
        match self.kind {
            KeyKind::Ec(curve) => PublicKey::Ec {
                curve,
                point: &self.bytes[..self.first],
            },
            KeyKind::Rsa => PublicKey::Rsa {
                modulus: &self.bytes[..self.first],
                exponent: &self.bytes[self.first..self.first + self.second],
            },
        }
    }
}

fn scheme_algorithm(scheme: u16, kind: KeyKind) -> Option<SignatureAlgorithm> {
    match (scheme, kind) {
        (0x0403, KeyKind::Ec(Curve::P256)) => {
            Some(SignatureAlgorithm::Ecdsa(HashAlgorithm::Sha256))
        }
        (0x0503, KeyKind::Ec(Curve::P384)) => {
            Some(SignatureAlgorithm::Ecdsa(HashAlgorithm::Sha384))
        }
        (0x0804, KeyKind::Rsa) => Some(SignatureAlgorithm::RsaPss(HashAlgorithm::Sha256)),
        (0x0805, KeyKind::Rsa) => Some(SignatureAlgorithm::RsaPss(HashAlgorithm::Sha384)),
        (0x0806, KeyKind::Rsa) => Some(SignatureAlgorithm::RsaPss(HashAlgorithm::Sha512)),
        _ => None,
    }
}

/// What the client brings to a handshake.
#[derive(Debug, Clone, Copy)]
pub struct Settings<'a> {
    /// The server's host name (or dotted IPv4 address), which its certificate must name.
    pub host: &'a [u8],
    /// The time, in seconds since 1970 (UTC), the certificates must be valid at.
    pub now: i64,
}

pub(crate) struct Handshaking {
    pub(crate) incoming: Incoming,
    pub(crate) filled: usize,
}

impl Handshaking {
    pub(crate) fn next_message(
        &mut self,
        transport: &mut dyn Transport,
        buffers: &mut Buffers,
        mut protection: Option<&mut Protection>,
    ) -> Result<usize, Error> {
        loop {
            if self.filled >= 4 {
                let length = (usize::from(buffers.handshake[1]) << 16)
                    | (usize::from(buffers.handshake[2]) << 8)
                    | usize::from(buffers.handshake[3]);
                if length + 4 > HANDSHAKE_BYTES {
                    return Err(Error::TooLarge);
                }
                if self.filled >= length + 4 {
                    return Ok(length + 4);
                }
            }
            let (kind, at, length) = self
                .incoming
                .next_record(transport, &mut buffers.incoming)?;
            let (kind, start, data_length) = match (kind, protection.as_deref_mut()) {
                (CHANGE_CIPHER_SPEC, _)
                    if length == HEADER_BYTES + 1 && buffers.incoming[at + HEADER_BYTES] == 1 =>
                {
                    continue;
                }
                (CHANGE_CIPHER_SPEC, _) => return Err(Error::Malformed),
                (APPLICATION_DATA, Some(protection)) => {
                    unprotect(protection, &mut buffers.incoming, at, length)?
                }
                (HANDSHAKE, None) | (ALERT, _) => (kind, at + HEADER_BYTES, length - HEADER_BYTES),
                _ => return Err(Error::Malformed),
            };
            let data = &buffers.incoming[start..start + data_length];
            match kind {
                HANDSHAKE => {
                    let end = self.filled + data.len();
                    if end > HANDSHAKE_BYTES {
                        return Err(Error::TooLarge);
                    }
                    buffers.handshake[self.filled..end].copy_from_slice(data);
                    self.filled = end;
                }
                ALERT => return Err(Error::Alert(data.get(1).copied().unwrap_or(0))),
                _ => return Err(Error::Malformed),
            }
        }
    }

    pub(crate) fn consume(&mut self, buffers: &mut Buffers, length: usize) {
        buffers.handshake.copy_within(length..self.filled, 0);
        self.filled -= length;
    }
}

/// An established connection, the client's ([`connect`]) or the server's
/// ([`crate::server::accept`]).
pub struct Session<'b> {
    pub(crate) buffers: &'b mut Buffers,
    pub(crate) incoming: Incoming,
    pub(crate) filled: usize,
    pub(crate) suite: CipherSuite,
    pub(crate) read: Protection,
    pub(crate) write: Protection,
    pub(crate) read_secret: Digest,
    pub(crate) write_secret: Digest,
    pub(crate) pending_start: usize,
    pub(crate) pending_end: usize,
    pub(crate) closed: bool,
}

impl fmt::Debug for Session<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Session({})", self.suite.name())
    }
}

/// Runs the handshake for `settings` over `transport` in `buffers`, with `randomness` fresh
/// from the system's generator and the server's certificates checked against `anchors`.
pub fn connect<'b>(
    transport: &mut dyn Transport,
    buffers: &'b mut Buffers,
    settings: &Settings<'_>,
    randomness: &[u8; RANDOMNESS_BYTES],
    anchors: &mut dyn TrustAnchors,
) -> Result<Session<'b>, Error> {
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&randomness[64..]);
    let key_share = x25519::public_key(&secret);
    let mut transcript = Transcript::new();
    let hello_length = write_client_hello(
        &mut buffers.outgoing[HEADER_BYTES..],
        settings.host,
        randomness,
        &key_share,
    );
    transcript.update(&buffers.outgoing[HEADER_BYTES..HEADER_BYTES + hello_length]);
    buffers.outgoing[..HEADER_BYTES].copy_from_slice(&[
        HANDSHAKE,
        3,
        1,
        (hello_length >> 8) as u8,
        hello_length as u8,
    ]);
    transport.send(&buffers.outgoing[..HEADER_BYTES + hello_length])?;

    let mut state = Handshaking {
        incoming: Incoming::default(),
        filled: 0,
    };
    let length = state.next_message(transport, buffers, None)?;
    if buffers.handshake[0] != SERVER_HELLO {
        return Err(Error::Malformed);
    }
    let (suite, shared) = {
        let hello = parse_server_hello(&buffers.handshake[4..length], &randomness[32..64])?;
        let mut theirs = [0u8; 32];
        theirs.copy_from_slice(hello.key_share);
        (hello.suite, x25519::x25519(&secret, &theirs))
    };
    if constant_time::equal(&shared, &[0; 32]) {
        return Err(Error::NoCommonKeyExchange);
    }
    transcript.update(&buffers.handshake[..length]);
    state.consume(buffers, length);
    if state.filled != 0 {
        return Err(Error::Malformed);
    }

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

    let mut requested: Option<([u8; 255], usize)> = None;
    let mut leaf: Option<LeafKey> = None;
    let mut verified = false;
    let mut expected = ENCRYPTED_EXTENSIONS;
    loop {
        let length = state.next_message(transport, buffers, Some(&mut server_protection))?;
        let kind = buffers.handshake[0];
        let body = &buffers.handshake[4..length];
        match (kind, expected) {
            (ENCRYPTED_EXTENSIONS, ENCRYPTED_EXTENSIONS) => {
                let mut cursor = Cursor { data: body };
                cursor.vector(2)?;
                cursor.finish()?;
                expected = CERTIFICATE;
            }
            (CERTIFICATE_REQUEST, CERTIFICATE) if requested.is_none() => {
                let mut cursor = Cursor { data: body };
                let context = cursor.vector(1)?;
                let mut copied = [0u8; 255];
                copied[..context.len()].copy_from_slice(context);
                requested = Some((copied, context.len()));
            }
            (CERTIFICATE, CERTIFICATE) => {
                let mut cursor = Cursor { data: body };
                if !cursor.vector(1)?.is_empty() {
                    return Err(Error::Malformed);
                }
                let mut list = Cursor {
                    data: cursor.vector(3)?,
                };
                cursor.finish()?;
                let mut chain: [&[u8]; MAX_CHAIN] = [&[]; MAX_CHAIN];
                let mut count = 0;
                while !list.data.is_empty() {
                    let certificate = list.vector(3)?;
                    list.vector(2)?;
                    let slot = chain
                        .get_mut(count)
                        .ok_or(Error::Certificate(CertificateError::BadChain))?;
                    *slot = certificate;
                    count += 1;
                }
                let key = chain::verify(&chain[..count], settings.host, settings.now, anchors)
                    .map_err(Error::Certificate)?;
                leaf = Some(LeafKey::copy(&key)?);
                expected = CERTIFICATE_VERIFY;
            }
            (CERTIFICATE_VERIFY, CERTIFICATE_VERIFY) => {
                let mut cursor = Cursor { data: body };
                let scheme = cursor.number(2)? as u16;
                let signature = cursor.vector(2)?;
                cursor.finish()?;
                let leaf = leaf.as_ref().ok_or(Error::Malformed)?;
                let algorithm = scheme_algorithm(scheme, leaf.kind).ok_or(Error::BadSignature)?;
                let mut content = [0x20u8; 64 + 34 + 64];
                let context = b"TLS 1.3, server CertificateVerify\0";
                content[64..64 + context.len()].copy_from_slice(context);
                let digest = transcript.hash(hash);
                let end = 64 + context.len() + digest.as_bytes().len();
                content[64 + context.len()..end].copy_from_slice(digest.as_bytes());
                if !x509::verify(&leaf.key(), algorithm, &content[..end], signature) {
                    return Err(Error::BadSignature);
                }
                verified = true;
                expected = FINISHED;
            }
            (FINISHED, FINISHED) if verified => {
                let mut key = [0u8; 64];
                let key = &mut key[..hash.output_bytes()];
                expand_label(hash, server_handshake.as_bytes(), b"finished", &[], key);
                let wanted = hmac::hmac(hash, key, transcript.hash(hash).as_bytes());
                if !constant_time::equal(wanted.as_bytes(), body) {
                    return Err(Error::BadFinished);
                }
                transcript.update(&buffers.handshake[..length]);
                state.consume(buffers, length);
                if state.filled != 0 {
                    return Err(Error::Malformed);
                }
                break;
            }
            _ => return Err(Error::Malformed),
        }
        transcript.update(&buffers.handshake[..length]);
        state.consume(buffers, length);
    }

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

    transport.send(&[CHANGE_CIPHER_SPEC, 3, 3, 0, 1, 1])?;
    if let Some((context, context_length)) = requested {
        let mut message = [0u8; 4 + 1 + 255 + 3];
        message[0] = CERTIFICATE;
        let body_length = 1 + context_length + 3;
        message[2] = (body_length >> 8) as u8;
        message[3] = body_length as u8;
        message[4] = context_length as u8;
        message[5..5 + context_length].copy_from_slice(&context[..context_length]);
        let message = &message[..4 + body_length];
        transcript.update(message);
        send_protected(
            transport,
            &mut buffers.outgoing,
            &mut client_protection,
            HANDSHAKE,
            message,
        )?;
    }
    let mut key = [0u8; 64];
    let key = &mut key[..hash.output_bytes()];
    expand_label(hash, client_handshake.as_bytes(), b"finished", &[], key);
    let verify_data = hmac::hmac(hash, key, transcript.hash(hash).as_bytes());
    let mut finished = [0u8; 4 + 64];
    finished[0] = FINISHED;
    finished[3] = hash.output_bytes() as u8;
    finished[4..4 + hash.output_bytes()].copy_from_slice(verify_data.as_bytes());
    send_protected(
        transport,
        &mut buffers.outgoing,
        &mut client_protection,
        HANDSHAKE,
        &finished[..4 + hash.output_bytes()],
    )?;

    Ok(Session {
        incoming: state.incoming,
        filled: state.filled,
        suite,
        read: Protection::new(suite, server_application.as_bytes()),
        write: Protection::new(suite, client_application.as_bytes()),
        read_secret: server_application,
        write_secret: client_application,
        pending_start: 0,
        pending_end: 0,
        closed: false,
        buffers,
    })
}

impl Session<'_> {
    /// The cipher suite the server chose.
    pub fn suite(&self) -> CipherSuite {
        self.suite
    }

    /// Sends `data` in records.
    pub fn write(&mut self, transport: &mut dyn Transport, data: &[u8]) -> Result<(), Error> {
        send_protected(
            transport,
            &mut self.buffers.outgoing,
            &mut self.write,
            APPLICATION_DATA,
            data,
        )
    }

    /// Receives data into `out`: how many bytes, 0 once the peer closed (with
    /// `close_notify`, or by closing the connection).
    pub fn read(&mut self, transport: &mut dyn Transport, out: &mut [u8]) -> Result<usize, Error> {
        loop {
            if self.pending_start < self.pending_end {
                let count = out.len().min(self.pending_end - self.pending_start);
                out[..count].copy_from_slice(
                    &self.buffers.incoming[self.pending_start..self.pending_start + count],
                );
                self.pending_start += count;
                return Ok(count);
            }
            if self.closed {
                return Ok(0);
            }
            let (_, at, length) = match self
                .incoming
                .next_record(transport, &mut self.buffers.incoming)
            {
                Ok(record) => record,
                Err(Error::Closed) => {
                    self.closed = true;
                    return Ok(0);
                }
                Err(error) => return Err(error),
            };
            if self.buffers.incoming[at] == CHANGE_CIPHER_SPEC {
                return Err(Error::Malformed);
            }
            let (kind, start, data_length) =
                unprotect(&mut self.read, &mut self.buffers.incoming, at, length)?;
            match kind {
                APPLICATION_DATA => {
                    self.pending_start = start;
                    self.pending_end = start + data_length;
                }
                ALERT => {
                    let description = self.buffers.incoming.get(start + 1).copied().unwrap_or(0);
                    self.closed = true;
                    if description != 0 {
                        return Err(Error::Alert(description));
                    }
                }
                HANDSHAKE => self.post_handshake(transport, start, data_length)?,
                _ => return Err(Error::Malformed),
            }
        }
    }

    fn post_handshake(
        &mut self,
        transport: &mut dyn Transport,
        start: usize,
        length: usize,
    ) -> Result<(), Error> {
        let end = self.filled + length;
        if end > HANDSHAKE_BYTES {
            return Err(Error::TooLarge);
        }
        self.buffers.handshake[self.filled..end]
            .copy_from_slice(&self.buffers.incoming[start..start + length]);
        self.filled = end;
        while self.filled >= 4 {
            let handshake = &self.buffers.handshake;
            let size = (usize::from(handshake[1]) << 16)
                | (usize::from(handshake[2]) << 8)
                | usize::from(handshake[3]);
            if size + 4 > HANDSHAKE_BYTES {
                return Err(Error::TooLarge);
            }
            if self.filled < size + 4 {
                break;
            }
            match (handshake[0], size) {
                (NEW_SESSION_TICKET, _) => {}
                (KEY_UPDATE, 1) => {
                    let requested = handshake[4] == 1;
                    let hash = self.suite.hash();
                    self.read_secret = next_secret(hash, &self.read_secret);
                    self.read = Protection::new(self.suite, self.read_secret.as_bytes());
                    if requested {
                        send_protected(
                            transport,
                            &mut self.buffers.outgoing,
                            &mut self.write,
                            HANDSHAKE,
                            &[KEY_UPDATE, 0, 0, 1, 0],
                        )?;
                        self.write_secret = next_secret(hash, &self.write_secret);
                        self.write = Protection::new(self.suite, self.write_secret.as_bytes());
                    }
                }
                _ => return Err(Error::Malformed),
            }
            self.buffers.handshake.copy_within(size + 4..self.filled, 0);
            self.filled -= size + 4;
        }
        Ok(())
    }

    /// Tells the peer this side sends nothing more (`close_notify`).
    pub fn close(&mut self, transport: &mut dyn Transport) -> Result<(), Error> {
        send_protected(
            transport,
            &mut self.buffers.outgoing,
            &mut self.write,
            ALERT,
            &[1, 0],
        )
    }
}

fn next_secret(hash: HashAlgorithm, secret: &Digest) -> Digest {
    derive_secret(hash, secret.as_bytes(), b"traffic upd", &[])
}

#[cfg(test)]
pub(crate) fn client_hello_for_tests(host: &[u8], randomness: &[u8; RANDOMNESS_BYTES]) -> Vec<u8> {
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&randomness[64..]);
    let mut out = vec![0u8; 1024];
    let length = write_client_hello(&mut out, host, randomness, &x25519::public_key(&secret));
    out.truncate(length);
    out
}
