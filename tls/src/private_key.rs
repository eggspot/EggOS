//! A P-256 private key from PEM text (D779), for a client certificate's signature: SEC1's
//! `EC PRIVATE KEY` (RFC 5915) or PKCS #8's unencrypted `PRIVATE KEY` (RFC 5958) holding one.
//! Encrypted keys and other curves or algorithms are refused.

use crate::der::{self, Reader};

const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const SEC1_BEGIN: &[u8] = b"-----BEGIN EC PRIVATE KEY-----";
const PKCS8_BEGIN: &[u8] = b"-----BEGIN PRIVATE KEY-----";
const SCALAR_BYTES: usize = 32;
const LARGEST_DER: usize = 512;

/// Why a key file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// No `EC PRIVATE KEY` or `PRIVATE KEY` block.
    NotFound,
    /// The block is not a well-formed key.
    Malformed,
    /// The key is not on P-256, or not an elliptic-curve key.
    Unsupported,
}

impl KeyError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::NotFound => "the file holds no unencrypted private key",
            Self::Malformed => "the private key is malformed",
            Self::Unsupported => "the private key is not a P-256 key",
        }
    }
}

fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

fn decode_block(text: &[u8], begin: &[u8], out: &mut [u8; LARGEST_DER]) -> Option<usize> {
    let start = text.windows(begin.len()).position(|window| window == begin)? + begin.len();
    let body = &text[start..];
    let end = body.windows(5).position(|window| window == b"-----")?;
    let (mut bits, mut sextets, mut length) = (0u32, 0u32, 0usize);
    let mut put = |byte: u8, length: &mut usize| -> Option<()> {
        *out.get_mut(*length)? = byte;
        *length += 1;
        Some(())
    };
    for &byte in &body[..end] {
        if matches!(byte, b'=' | b' ' | b'\t' | b'\r' | b'\n') {
            continue;
        }
        bits = (bits << 6) | sextet(byte)?;
        sextets += 1;
        if sextets == 4 {
            put((bits >> 16) as u8, &mut length)?;
            put((bits >> 8) as u8, &mut length)?;
            put(bits as u8, &mut length)?;
            bits = 0;
            sextets = 0;
        }
    }
    match sextets {
        2 => put((bits >> 4) as u8, &mut length)?,
        3 => {
            put((bits >> 10) as u8, &mut length)?;
            put((bits >> 2) as u8, &mut length)?;
        }
        _ => {}
    }
    Some(length)
}

fn scalar_from(octets: &[u8]) -> Result<[u8; SCALAR_BYTES], KeyError> {
    if octets.len() > SCALAR_BYTES {
        return Err(KeyError::Malformed);
    }
    let mut scalar = [0u8; SCALAR_BYTES];
    scalar[SCALAR_BYTES - octets.len()..].copy_from_slice(octets);
    Ok(scalar)
}

/// The key in SEC1's `ECPrivateKey` DER; `curve_known` when an outer PKCS #8 already named
/// P-256, so the optional parameters may be left out.
pub fn from_sec1(der_bytes: &[u8], curve_known: bool) -> Result<[u8; SCALAR_BYTES], KeyError> {
    let malformed = |_| KeyError::Malformed;
    let mut outer = Reader::new(der_bytes);
    let mut key = Reader::new(outer.expect(der::SEQUENCE).map_err(malformed)?);
    if key.expect(der::INTEGER).map_err(malformed)? != [1] {
        return Err(KeyError::Malformed);
    }
    let scalar = scalar_from(key.expect(der::OCTET_STRING).map_err(malformed)?)?;
    let parameters = key.optional(der::explicit(0)).map_err(malformed)?;
    match parameters {
        Some(parameters) => {
            let mut parameters = Reader::new(parameters);
            if parameters.expect(der::OBJECT_IDENTIFIER).map_err(malformed)? != PRIME256V1 {
                return Err(KeyError::Unsupported);
            }
        }
        None if !curve_known => return Err(KeyError::Unsupported),
        None => {}
    }
    Ok(scalar)
}

/// The key in PKCS #8's `PrivateKeyInfo` DER.
pub fn from_pkcs8(der_bytes: &[u8]) -> Result<[u8; SCALAR_BYTES], KeyError> {
    let malformed = |_| KeyError::Malformed;
    let mut outer = Reader::new(der_bytes);
    let mut info = Reader::new(outer.expect(der::SEQUENCE).map_err(malformed)?);
    if info.expect(der::INTEGER).map_err(malformed)? != [0] {
        return Err(KeyError::Malformed);
    }
    let mut algorithm = Reader::new(info.expect(der::SEQUENCE).map_err(malformed)?);
    if algorithm.expect(der::OBJECT_IDENTIFIER).map_err(malformed)? != EC_PUBLIC_KEY {
        return Err(KeyError::Unsupported);
    }
    if algorithm.expect(der::OBJECT_IDENTIFIER).map_err(malformed)? != PRIME256V1 {
        return Err(KeyError::Unsupported);
    }
    from_sec1(info.expect(der::OCTET_STRING).map_err(malformed)?, true)
}

/// The P-256 private key in PEM `text`.
pub fn p256_from_pem(text: &[u8]) -> Result<[u8; SCALAR_BYTES], KeyError> {
    let mut der_bytes = [0u8; LARGEST_DER];
    if let Some(length) = decode_block(text, SEC1_BEGIN, &mut der_bytes) {
        return from_sec1(&der_bytes[..length], false);
    }
    if let Some(length) = decode_block(text, PKCS8_BEGIN, &mut der_bytes) {
        return from_pkcs8(&der_bytes[..length]);
    }
    Err(KeyError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC1: &[u8] = &[
        0x30, 0x31, 0x02, 0x01, 0x01, 0x04, 0x20, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
        0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01, 0xa0, 0x0a, 0x06, 0x08, 0x2a, 0x86,
        0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
    ];

    #[test]
    fn a_sec1_key_names_its_curve_and_a_pkcs8_key_wraps_it() {
        let scalar = from_sec1(SEC1, false).unwrap();
        assert_eq!(scalar[0], 0x11);
        assert_eq!(scalar[31], 0x01);
        let mut bare = SEC1[..2 + 3 + 2 + 32].to_vec();
        bare[1] = (bare.len() - 2) as u8;
        assert_eq!(from_sec1(&bare, false), Err(KeyError::Unsupported));
        assert_eq!(from_sec1(&bare, true), Ok(scalar));
        let mut pkcs8 = vec![0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07];
        pkcs8.extend_from_slice(EC_PUBLIC_KEY);
        pkcs8.extend_from_slice(&[0x06, 0x08]);
        pkcs8.extend_from_slice(PRIME256V1);
        pkcs8.extend_from_slice(&[0x04, SEC1.len() as u8]);
        pkcs8.extend_from_slice(SEC1);
        let mut info = vec![0x30, pkcs8.len() as u8];
        info.extend_from_slice(&pkcs8);
        assert_eq!(from_pkcs8(&info), Ok(scalar));
    }

    #[test]
    fn pem_text_is_found_between_its_markers() {
        let text = b"junk\n-----BEGIN EC PRIVATE KEY-----\n\
            MDECAQEEIBEiM0RVZneImaq7zN3u/wARIjNEVWZ3iJmqu8zd7v8BoAoGCCqGSM49\n\
            AwEH\n-----END EC PRIVATE KEY-----\n";
        assert_eq!(p256_from_pem(text), from_sec1(SEC1, false));
        assert_eq!(p256_from_pem(b"nothing"), Err(KeyError::NotFound));
    }
}
