//! X.509 certificates (RFC 5280): the fields a TLS client checks, signatures by an issuer's
//! key, and host names against the subject's alternative names (RFC 6125).

use crate::der::{self, BIT_STRING, BOOLEAN, INTEGER, Malformed, OBJECT_IDENTIFIER, OCTET_STRING};
use crate::der::{GENERALIZED_TIME, Reader, SEQUENCE, UTC_TIME};
use eggos_crypto::HashAlgorithm;
use eggos_crypto::ecdsa::{self, Curve};
use eggos_crypto::rsa;
use eggos_crypto::sha2::digest;

const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const RSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
const SHA256_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const SHA384_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const SHA512_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const SUBJECT_ALTERNATIVE_NAME: &[u8] = &[0x55, 0x1d, 0x11];
const EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const CERTIFICATE_POLICIES: &[u8] = &[0x55, 0x1d, 0x20];
const SERVER_AUTHENTICATION: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const ANY_EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

/// `keyUsage`'s `digitalSignature` bit.
pub const DIGITAL_SIGNATURE: u16 = 1 << 15;
/// `keyUsage`'s `keyCertSign` bit.
pub const KEY_CERT_SIGN: u16 = 1 << 10;

/// A signature algorithm: the padding or curve family and the hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// RSASSA-PKCS1-v1_5.
    RsaPkcs1(HashAlgorithm),
    /// RSASSA-PSS with MGF1 over the same hash and a salt as long as the hash.
    RsaPss(HashAlgorithm),
    /// ECDSA; the curve is the key's.
    Ecdsa(HashAlgorithm),
}

/// A subject's public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicKey<'a> {
    /// RSA: the modulus and public exponent, big-endian.
    Rsa {
        modulus: &'a [u8],
        exponent: &'a [u8],
    },
    /// ECDSA on a curve: an uncompressed point.
    Ec { curve: Curve, point: &'a [u8] },
}

/// The fields of a certificate a TLS client uses.
#[derive(Debug, Clone, Copy)]
pub struct Certificate<'a> {
    /// The signed part, `tbsCertificate`, whole.
    pub signed: &'a [u8],
    /// How the issuer signed it.
    pub signature_algorithm: SignatureAlgorithm,
    /// The issuer's signature (for ECDSA, the DER `Ecdsa-Sig-Value`).
    pub signature: &'a [u8],
    /// The issuer's name, as encoded.
    pub issuer: &'a [u8],
    /// The subject's name, as encoded.
    pub subject: &'a [u8],
    /// The first second it is valid, since 1970 in UTC.
    pub not_before: i64,
    /// The last second it is valid.
    pub not_after: i64,
    /// The subject's key.
    pub public_key: PublicKey<'a>,
    /// Whether `basicConstraints` makes it a certificate authority.
    pub is_authority: bool,
    /// `basicConstraints`' `pathLenConstraint`.
    pub path_length: Option<u32>,
    /// `keyUsage`'s bits (bit 15 `digitalSignature` first), when it has the extension.
    pub key_usage: Option<u16>,
    /// Whether `extKeyUsage`, when it has it, allows TLS servers.
    pub serves_tls: bool,
    /// The `subjectAltName`'s `GeneralNames` contents.
    pub alternative_names: Option<&'a [u8]>,
}

/// Why a certificate cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// It is not a well-formed certificate.
    Malformed,
    /// Its key or signature uses an algorithm EggOS does not check.
    UnsupportedAlgorithm,
    /// It has a critical extension EggOS does not process.
    UnknownCriticalExtension,
}

impl From<Malformed> for ParseError {
    fn from(_: Malformed) -> Self {
        Self::Malformed
    }
}

fn hash_of(identifier: &[u8]) -> Option<HashAlgorithm> {
    match identifier {
        SHA256 => Some(HashAlgorithm::Sha256),
        SHA384 => Some(HashAlgorithm::Sha384),
        SHA512 => Some(HashAlgorithm::Sha512),
        _ => None,
    }
}

fn signature_algorithm(contents: &[u8]) -> Result<SignatureAlgorithm, ParseError> {
    let mut reader = Reader::new(contents);
    let identifier = reader.expect(OBJECT_IDENTIFIER)?;
    let algorithm = match identifier {
        SHA256_WITH_RSA => SignatureAlgorithm::RsaPkcs1(HashAlgorithm::Sha256),
        SHA384_WITH_RSA => SignatureAlgorithm::RsaPkcs1(HashAlgorithm::Sha384),
        SHA512_WITH_RSA => SignatureAlgorithm::RsaPkcs1(HashAlgorithm::Sha512),
        ECDSA_SHA256 => SignatureAlgorithm::Ecdsa(HashAlgorithm::Sha256),
        ECDSA_SHA384 => SignatureAlgorithm::Ecdsa(HashAlgorithm::Sha384),
        ECDSA_SHA512 => SignatureAlgorithm::Ecdsa(HashAlgorithm::Sha512),
        RSA_PSS => {
            let mut parameters = Reader::new(reader.expect(SEQUENCE)?);
            let hash = Reader::new(parameters.expect(der::explicit(0))?).expect(SEQUENCE)?;
            let hash = hash_of(Reader::new(hash).expect(OBJECT_IDENTIFIER)?)
                .ok_or(ParseError::UnsupportedAlgorithm)?;
            parameters.read()?;
            if let Some(salt) = parameters.optional(der::explicit(2))? {
                let salt = Reader::new(salt).expect(INTEGER)?;
                if salt.len() != 1 || usize::from(salt[0]) != hash.output_bytes() {
                    return Err(ParseError::UnsupportedAlgorithm);
                }
            }
            return Ok(SignatureAlgorithm::RsaPss(hash));
        }
        _ => return Err(ParseError::UnsupportedAlgorithm),
    };
    Ok(algorithm)
}

fn public_key(contents: &[u8]) -> Result<PublicKey<'_>, ParseError> {
    let mut reader = Reader::new(contents);
    let mut algorithm = Reader::new(reader.expect(SEQUENCE)?);
    let key = der::bit_string_bytes(reader.expect(BIT_STRING)?).ok_or(ParseError::Malformed)?;
    match algorithm.expect(OBJECT_IDENTIFIER)? {
        RSA_ENCRYPTION => {
            let mut fields = Reader::new(Reader::new(key).expect(SEQUENCE)?);
            let modulus = der::positive_integer(fields.expect(INTEGER)?).ok_or(Malformed)?;
            let exponent = der::positive_integer(fields.expect(INTEGER)?).ok_or(Malformed)?;
            Ok(PublicKey::Rsa { modulus, exponent })
        }
        EC_PUBLIC_KEY => {
            let curve = match algorithm.expect(OBJECT_IDENTIFIER)? {
                P256 => Curve::P256,
                P384 => Curve::P384,
                _ => return Err(ParseError::UnsupportedAlgorithm),
            };
            Ok(PublicKey::Ec { curve, point: key })
        }
        _ => Err(ParseError::UnsupportedAlgorithm),
    }
}

fn digits(text: &[u8]) -> Option<i64> {
    text.iter().try_fold(0i64, |value, &byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + i64::from(byte - b'0'))
    })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A UTCTime's or GeneralizedTime's moment in seconds since 1970 (UTC).
pub fn time(tag: u8, text: &[u8]) -> Option<i64> {
    let (year, rest) = match (tag, text.len()) {
        (UTC_TIME, 13) => {
            let year = digits(&text[..2])?;
            (
                if year >= 50 { 1900 + year } else { 2000 + year },
                &text[2..],
            )
        }
        (GENERALIZED_TIME, 15) => (digits(&text[..4])?, &text[4..]),
        _ => return None,
    };
    if rest[10] != b'Z' {
        return None;
    }
    let field = |at: usize| digits(&rest[at..at + 2]);
    let (month, day, hour, minute, second) =
        (field(0)?, field(2)?, field(4)?, field(6)?, field(8)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn read_time(reader: &mut Reader<'_>) -> Result<i64, ParseError> {
    let element = reader.read()?;
    time(element.tag, element.contents).ok_or(ParseError::Malformed)
}

impl<'a> Certificate<'a> {
    /// Parses a certificate's DER.
    pub fn parse(der: &'a [u8]) -> Result<Self, ParseError> {
        let mut outer = Reader::new(der);
        let mut certificate = Reader::new(outer.expect(SEQUENCE)?);
        outer.finish()?;
        let signed_element = certificate.read()?;
        if signed_element.tag != SEQUENCE {
            return Err(ParseError::Malformed);
        }
        let signature_algorithm = signature_algorithm(certificate.expect(SEQUENCE)?)?;
        let signature =
            der::bit_string_bytes(certificate.expect(BIT_STRING)?).ok_or(ParseError::Malformed)?;
        certificate.finish()?;

        let mut fields = Reader::new(signed_element.contents);
        fields.optional(der::explicit(0))?;
        fields.expect(INTEGER)?;
        if signature_algorithm != self::signature_algorithm(fields.expect(SEQUENCE)?)? {
            return Err(ParseError::Malformed);
        }
        let issuer = fields.read()?;
        let mut validity = Reader::new(fields.expect(SEQUENCE)?);
        let not_before = read_time(&mut validity)?;
        let not_after = read_time(&mut validity)?;
        let subject = fields.read()?;
        if issuer.tag != SEQUENCE || subject.tag != SEQUENCE {
            return Err(ParseError::Malformed);
        }
        let public_key = public_key(fields.expect(SEQUENCE)?)?;
        fields.optional(der::implicit(1))?;
        fields.optional(der::implicit(2))?;
        let mut parsed = Self {
            signed: signed_element.whole,
            signature_algorithm,
            signature,
            issuer: issuer.whole,
            subject: subject.whole,
            not_before,
            not_after,
            public_key,
            is_authority: false,
            path_length: None,
            key_usage: None,
            serves_tls: true,
            alternative_names: None,
        };
        if let Some(extensions) = fields.optional(der::explicit(3))? {
            let mut list = Reader::new(Reader::new(extensions).expect(SEQUENCE)?);
            while !list.is_empty() {
                parsed.extension(list.expect(SEQUENCE)?)?;
            }
        }
        fields.finish()?;
        Ok(parsed)
    }

    fn extension(&mut self, contents: &'a [u8]) -> Result<(), ParseError> {
        let mut reader = Reader::new(contents);
        let identifier = reader.expect(OBJECT_IDENTIFIER)?;
        let critical = match reader.optional(BOOLEAN)? {
            Some([0xff]) => true,
            Some(_) => return Err(ParseError::Malformed),
            None => false,
        };
        let value = reader.expect(OCTET_STRING)?;
        reader.finish()?;
        let mut value = Reader::new(value);
        match identifier {
            BASIC_CONSTRAINTS => {
                let mut fields = Reader::new(value.expect(SEQUENCE)?);
                self.is_authority = fields.optional(BOOLEAN)? == Some(&[0xff][..]);
                if let Some(length) = fields.optional(INTEGER)? {
                    let length = der::positive_integer(length).ok_or(Malformed)?;
                    self.path_length = Some(if length.len() > 1 {
                        u32::MAX
                    } else {
                        u32::from(length[0])
                    });
                }
            }
            KEY_USAGE => {
                let bits = value.expect(BIT_STRING)?;
                let first = bits.get(1).copied().unwrap_or(0);
                let second = bits.get(2).copied().unwrap_or(0);
                self.key_usage = Some(u16::from_be_bytes([first, second]));
            }
            SUBJECT_ALTERNATIVE_NAME => {
                self.alternative_names = Some(value.expect(SEQUENCE)?);
            }
            EXTENDED_KEY_USAGE => {
                let mut purposes = Reader::new(value.expect(SEQUENCE)?);
                let mut serves = false;
                while !purposes.is_empty() {
                    let purpose = purposes.expect(OBJECT_IDENTIFIER)?;
                    serves |= purpose == SERVER_AUTHENTICATION || purpose == ANY_EXTENDED_KEY_USAGE;
                }
                self.serves_tls = serves;
            }
            CERTIFICATE_POLICIES => {}
            _ if critical => return Err(ParseError::UnknownCriticalExtension),
            _ => {}
        }
        Ok(())
    }

    /// Whether the certificate is valid at `now` (seconds since 1970).
    pub fn is_valid_at(&self, now: i64) -> bool {
        (self.not_before..=self.not_after).contains(&now)
    }

    /// Whether `issuer`'s key made this certificate's signature.
    pub fn is_signed_by(&self, issuer: &PublicKey<'_>) -> bool {
        verify(
            issuer,
            self.signature_algorithm,
            self.signed,
            self.signature,
        )
    }

    /// Whether the certificate names `host` (a DNS name or a dotted IPv4 address) among its
    /// subject's alternative names; wildcards (`*.example.com`) cover one leftmost label.
    pub fn names_host(&self, host: &[u8]) -> bool {
        let Some(names) = self.alternative_names else {
            return false;
        };
        let address = ipv4(host);
        let mut reader = Reader::new(names);
        while let Ok(name) = reader.read() {
            let matched = match name.tag {
                0x82 => address.is_none() && dns_name_matches(name.contents, host),
                0x87 => address.is_some_and(|address| name.contents == address),
                _ => false,
            };
            if matched {
                return true;
            }
        }
        false
    }
}

fn ipv4(text: &[u8]) -> Option<[u8; 4]> {
    let mut address = [0u8; 4];
    let mut parts = text.split(|&byte| byte == b'.');
    for slot in &mut address {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        *slot = u8::try_from(digits(part)?).ok()?;
    }
    parts.next().is_none().then_some(address)
}

fn dns_name_matches(pattern: &[u8], host: &[u8]) -> bool {
    let host = host.strip_suffix(b".").unwrap_or(host);
    match pattern.strip_prefix(b"*.") {
        Some(suffix) => match host.iter().position(|&byte| byte == b'.') {
            Some(dot) => {
                dot > 0 && suffix.contains(&b'.') && host[dot + 1..].eq_ignore_ascii_case(suffix)
            }
            None => false,
        },
        None => !pattern.contains(&b'*') && pattern.eq_ignore_ascii_case(host),
    }
}

fn ecdsa_signature(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut outer = Reader::new(der);
    let mut fields = Reader::new(outer.expect(SEQUENCE).ok()?);
    outer.finish().ok()?;
    let r = der::positive_integer(fields.expect(INTEGER).ok()?)?;
    let s = der::positive_integer(fields.expect(INTEGER).ok()?)?;
    fields.finish().ok()?;
    Some((r, s))
}

/// Whether `signature` by `key` with `algorithm` signs `message`. ECDSA signatures are DER
/// `Ecdsa-Sig-Value`s, as in certificates and TLS.
pub fn verify(
    key: &PublicKey<'_>,
    algorithm: SignatureAlgorithm,
    message: &[u8],
    signature: &[u8],
) -> bool {
    match (key, algorithm) {
        (PublicKey::Ec { curve, point }, SignatureAlgorithm::Ecdsa(hash)) => {
            let Some((r, s)) = ecdsa_signature(signature) else {
                return false;
            };
            ecdsa::verify(*curve, point, digest(hash, message).as_bytes(), r, s)
        }
        (PublicKey::Rsa { modulus, exponent }, SignatureAlgorithm::RsaPkcs1(hash)) => {
            rsa::PublicKey::new(modulus, exponent).is_ok_and(|key| {
                key.verify_pkcs1(hash, digest(hash, message).as_bytes(), signature)
            })
        }
        (PublicKey::Rsa { modulus, exponent }, SignatureAlgorithm::RsaPss(hash)) => {
            rsa::PublicKey::new(modulus, exponent)
                .is_ok_and(|key| key.verify_pss(hash, digest(hash, message).as_bytes(), signature))
        }
        _ => false,
    }
}
