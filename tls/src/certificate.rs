//! EggOS's own self-signed server certificate (RFC 5280): an ECDSA P-256 key signing a
//! certificate that names the machine's host names and IPv4 addresses, for the TLS server a
//! person pins by its SHA-256 fingerprint. Also the certificate in PEM, and the fingerprint as
//! people read it.

use crate::der::{BIT_STRING, BOOLEAN, GENERALIZED_TIME, INTEGER, OBJECT_IDENTIFIER};
use crate::der::{OCTET_STRING, SEQUENCE, SET, UTC_TIME, explicit};
use eggos_crypto::HashAlgorithm;
use eggos_crypto::p256::{self, InvalidKey, PUBLIC_KEY_BYTES, SCALAR_BYTES};
use eggos_crypto::sha2::digest;

/// The most bytes a certificate built here takes.
pub const MAX_CERTIFICATE_BYTES: usize = 1536;
/// The most names a certificate carries.
pub const MAX_NAMES: usize = 8;
/// The longest host name kept in a certificate, in bytes.
pub const MAX_HOST_BYTES: usize = 63;
/// The bytes of a serial number.
pub const SERIAL_BYTES: usize = 16;
/// The bytes of a fingerprint as people read it: 32 pairs of hex digits joined by colons.
pub const FINGERPRINT_TEXT_BYTES: usize = 32 * 3 - 1;

const UTF8_STRING: u8 = 0x0c;
const DNS_NAME: u8 = 0x82;
const IP_ADDRESS: u8 = 0x87;
const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256_CURVE: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const SUBJECT_ALTERNATIVE_NAME: &[u8] = &[0x55, 0x1d, 0x11];
const SERVER_AUTHENTICATION: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const DIGITAL_SIGNATURE_ONLY: &[u8] = &[7, 0x80];
const LAST_UTC_TIME_YEAR: i64 = 2049;
const PEM_LINE_BYTES: usize = 64;

/// A name the certificate holds for its server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Name<'a> {
    /// A DNS host name, such as `eggos.local`.
    Host(&'a [u8]),
    /// An IPv4 address.
    Ipv4([u8; 4]),
}

/// Why a certificate could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
    /// The private key is not a P-256 key.
    InvalidKey,
    /// No names, more than [`MAX_NAMES`], or a host name that is empty, too long or not
    /// letters, digits, dots and dashes.
    BadNames,
    /// The validity ends before it starts.
    BadValidity,
    /// The certificate does not fit [`MAX_CERTIFICATE_BYTES`] or the output.
    TooLarge,
}

impl From<InvalidKey> for BuildError {
    fn from(_: InvalidKey) -> Self {
        Self::InvalidKey
    }
}

/// What the certificate says.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The private key that signs and whose public key the certificate holds.
    pub private_key: &'a [u8; SCALAR_BYTES],
    /// The names, the first also the subject's common name.
    pub names: &'a [Name<'a>],
    /// A random serial number.
    pub serial: [u8; SERIAL_BYTES],
    /// When the certificate starts to be valid, in seconds since 1970 (UTC).
    pub not_before: i64,
    /// When it ends.
    pub not_after: i64,
}

struct Der<const N: usize> {
    bytes: [u8; N],
    length: usize,
    overflowed: bool,
}

impl<const N: usize> Der<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
            overflowed: false,
        }
    }

    fn raw(&mut self, data: &[u8]) {
        match self.bytes.get_mut(self.length..self.length + data.len()) {
            Some(space) => {
                space.copy_from_slice(data);
                self.length += data.len();
            }
            None => self.overflowed = true,
        }
    }

    fn element(&mut self, tag: u8, parts: &[&[u8]]) {
        let length: usize = parts.iter().map(|part| part.len()).sum();
        self.raw(&[tag]);
        match length {
            0..=0x7f => self.raw(&[length as u8]),
            0x80..=0xff => self.raw(&[0x81, length as u8]),
            _ => self.raw(&[0x82, (length >> 8) as u8, length as u8]),
        }
        for part in parts {
            self.raw(part);
        }
    }

    fn wrap<const M: usize>(&mut self, tag: u8, inner: &Der<M>) {
        self.overflowed |= inner.overflowed;
        self.element(tag, &[inner.as_bytes()]);
    }

    fn integer(&mut self, big_endian: &[u8]) {
        let first = big_endian
            .iter()
            .position(|&byte| byte != 0)
            .unwrap_or(big_endian.len().saturating_sub(1));
        let kept = &big_endian[first..];
        match kept.first() {
            Some(&byte) if byte & 0x80 != 0 => self.element(INTEGER, &[&[0], kept]),
            Some(_) => self.element(INTEGER, &[kept]),
            None => self.element(INTEGER, &[&[0]]),
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn put_digits(out: &mut [u8], value: i64) {
    let mut value = value;
    for byte in out.iter_mut().rev() {
        *byte = b'0' + (value % 10) as u8;
        value /= 10;
    }
}

fn time<const N: usize>(der: &mut Der<N>, seconds: i64) {
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let of_day = seconds.rem_euclid(86_400);
    let mut text = [b'Z'; 15];
    let (tag, year_digits) = match (1950..=LAST_UTC_TIME_YEAR).contains(&year) {
        true => (UTC_TIME, 2),
        false => (GENERALIZED_TIME, 4),
    };
    put_digits(
        &mut text[..year_digits],
        year.rem_euclid(10_i64.pow(year_digits as u32)),
    );
    let rest = &mut text[year_digits..];
    for (at, value) in [month, day, of_day / 3600, of_day % 3600 / 60, of_day % 60]
        .into_iter()
        .enumerate()
    {
        put_digits(&mut rest[2 * at..2 * at + 2], value);
    }
    der.element(tag, &[&text[..year_digits + 11]]);
}

fn is_host_name(host: &[u8]) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && host
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

fn common_name(name: &Name<'_>, text: &mut [u8; 15]) -> usize {
    match name {
        Name::Host(host) => {
            let length = host.len().min(text.len());
            text[..length].copy_from_slice(&host[..length]);
            length
        }
        Name::Ipv4(address) => {
            let mut length = 0;
            for (index, part) in address.iter().enumerate() {
                if index > 0 {
                    text[length] = b'.';
                    length += 1;
                }
                let digits = match part {
                    0..=9 => 1,
                    10..=99 => 2,
                    _ => 3,
                };
                put_digits(&mut text[length..length + digits], i64::from(*part));
                length += digits;
            }
            length
        }
    }
}

fn distinguished_name<const N: usize>(der: &mut Der<N>, first: &Name<'_>) {
    let mut text = [0u8; 15];
    let mut long = [0u8; MAX_HOST_BYTES];
    let value: &[u8] = match first {
        Name::Host(host) => {
            long[..host.len()].copy_from_slice(host);
            &long[..host.len()]
        }
        Name::Ipv4(_) => {
            let length = common_name(first, &mut text);
            &text[..length]
        }
    };
    let mut attribute = Der::<128>::new();
    attribute.element(OBJECT_IDENTIFIER, &[COMMON_NAME]);
    attribute.element(UTF8_STRING, &[value]);
    let mut set = Der::<160>::new();
    set.wrap(SEQUENCE, &attribute);
    let mut name = Der::<192>::new();
    name.wrap(SET, &set);
    der.wrap(SEQUENCE, &name);
}

fn extension<const N: usize, const M: usize>(
    der: &mut Der<N>,
    identifier: &[u8],
    critical: bool,
    value: &Der<M>,
) {
    let mut fields = Der::<640>::new();
    fields.element(OBJECT_IDENTIFIER, &[identifier]);
    if critical {
        fields.element(BOOLEAN, &[&[0xff]]);
    }
    fields.wrap(OCTET_STRING, value);
    der.wrap(SEQUENCE, &fields);
}

fn extensions<const N: usize>(der: &mut Der<N>, names: &[Name<'_>]) {
    let mut list = Der::<768>::new();
    let mut usage = Der::<8>::new();
    usage.element(BIT_STRING, &[DIGITAL_SIGNATURE_ONLY]);
    extension(&mut list, KEY_USAGE, true, &usage);
    let mut purposes = Der::<16>::new();
    purposes.element(OBJECT_IDENTIFIER, &[SERVER_AUTHENTICATION]);
    let mut purpose_list = Der::<24>::new();
    purpose_list.wrap(SEQUENCE, &purposes);
    extension(&mut list, EXTENDED_KEY_USAGE, false, &purpose_list);
    let mut alternatives = Der::<576>::new();
    for name in names {
        match name {
            Name::Host(host) => alternatives.element(DNS_NAME, &[host]),
            Name::Ipv4(address) => alternatives.element(IP_ADDRESS, &[address]),
        }
    }
    let mut alternative_list = Der::<600>::new();
    alternative_list.wrap(SEQUENCE, &alternatives);
    extension(
        &mut list,
        SUBJECT_ALTERNATIVE_NAME,
        false,
        &alternative_list,
    );
    let mut sequence = Der::<800>::new();
    sequence.wrap(SEQUENCE, &list);
    der.wrap(explicit(3), &sequence);
}

fn algorithm<const N: usize>(der: &mut Der<N>) {
    let mut identifier = Der::<16>::new();
    identifier.element(OBJECT_IDENTIFIER, &[ECDSA_SHA256]);
    der.wrap(SEQUENCE, &identifier);
}

/// Builds and signs the certificate `request` describes into `out`; returns its DER length.
pub fn self_signed(request: &Request<'_>, out: &mut [u8]) -> Result<usize, BuildError> {
    let names_fit = !request.names.is_empty() && request.names.len() <= MAX_NAMES;
    let hosts_fit = request.names.iter().all(|name| match name {
        Name::Host(host) => is_host_name(host),
        Name::Ipv4(_) => true,
    });
    if !names_fit || !hosts_fit {
        return Err(BuildError::BadNames);
    }
    if request.not_after < request.not_before {
        return Err(BuildError::BadValidity);
    }
    let public_key: [u8; PUBLIC_KEY_BYTES] = p256::public_key(request.private_key)?;
    let mut serial = request.serial;
    serial[0] = (serial[0] & 0x7f) | 0x40;

    let mut fields = Der::<{ MAX_CERTIFICATE_BYTES }>::new();
    fields.element(explicit(0), &[&[INTEGER, 1, 2]]);
    fields.integer(&serial);
    algorithm(&mut fields);
    distinguished_name(&mut fields, &request.names[0]);
    let mut validity = Der::<40>::new();
    time(&mut validity, request.not_before);
    time(&mut validity, request.not_after);
    fields.wrap(SEQUENCE, &validity);
    distinguished_name(&mut fields, &request.names[0]);
    let mut key_algorithm = Der::<32>::new();
    key_algorithm.element(OBJECT_IDENTIFIER, &[EC_PUBLIC_KEY]);
    key_algorithm.element(OBJECT_IDENTIFIER, &[P256_CURVE]);
    let mut key_info = Der::<128>::new();
    key_info.wrap(SEQUENCE, &key_algorithm);
    key_info.element(BIT_STRING, &[&[0], &public_key]);
    fields.wrap(SEQUENCE, &key_info);
    extensions(&mut fields, request.names);
    let mut signed = Der::<{ MAX_CERTIFICATE_BYTES }>::new();
    signed.wrap(SEQUENCE, &fields);

    let hashed = digest(HashAlgorithm::Sha256, signed.as_bytes());
    let signature = p256::sign(request.private_key, hashed.as_bytes())?;
    let mut halves = Der::<80>::new();
    halves.integer(&signature[..SCALAR_BYTES]);
    halves.integer(&signature[SCALAR_BYTES..]);
    let mut value = Der::<96>::new();
    value.wrap(SEQUENCE, &halves);

    let mut certificate = Der::<{ MAX_CERTIFICATE_BYTES }>::new();
    certificate.raw(signed.as_bytes());
    algorithm(&mut certificate);
    certificate.element(BIT_STRING, &[&[0], value.as_bytes()]);
    let mut whole = Der::<{ MAX_CERTIFICATE_BYTES }>::new();
    whole.wrap(SEQUENCE, &certificate);
    if whole.overflowed || signed.overflowed {
        return Err(BuildError::TooLarge);
    }
    let bytes = whole.as_bytes();
    out.get_mut(..bytes.len())
        .ok_or(BuildError::TooLarge)?
        .copy_from_slice(bytes);
    Ok(bytes.len())
}

/// The SHA-256 fingerprint of a certificate's DER bytes.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest(HashAlgorithm::Sha256, der).as_bytes());
    out
}

/// `fingerprint` as people read it: upper-case hex pairs joined by colons.
pub fn fingerprint_text(fingerprint: &[u8; 32]) -> [u8; FINGERPRINT_TEXT_BYTES] {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = [b':'; FINGERPRINT_TEXT_BYTES];
    for (index, byte) in fingerprint.iter().enumerate() {
        text[3 * index] = DIGITS[usize::from(byte >> 4)];
        text[3 * index + 1] = DIGITS[usize::from(byte & 15)];
    }
    text
}

/// The bytes [`pem`] writes for a certificate of `der_bytes`.
pub const fn pem_bytes(der_bytes: usize) -> usize {
    let base64 = der_bytes.div_ceil(3) * 4;
    28 + base64 + base64.div_ceil(PEM_LINE_BYTES) + 26
}

/// Writes a certificate's DER bytes as PEM into `out`; returns the length, or `None` when
/// `out` is shorter than [`pem_bytes`].
pub fn pem(der: &[u8], out: &mut [u8]) -> Option<usize> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if out.len() < pem_bytes(der.len()) {
        return None;
    }
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        out[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    put(b"-----BEGIN CERTIFICATE-----\n");
    let mut column = 0;
    for chunk in der.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0u32, |value, (index, &byte)| {
                value | (u32::from(byte) << (16 - 8 * index))
            });
        let mut quad = [b'='; 4];
        for index in 0..=chunk.len() {
            quad[index] = ALPHABET[((triple >> (18 - 6 * index)) & 63) as usize];
        }
        put(&quad);
        column += 4;
        if column == PEM_LINE_BYTES {
            put(b"\n");
            column = 0;
        }
    }
    if column != 0 {
        put(b"\n");
    }
    put(b"-----END CERTIFICATE-----\n");
    Some(length)
}
