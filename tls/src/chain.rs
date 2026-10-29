//! Checking a server's certificates (RFC 5280 section 6, as browsers apply it to TLS): the
//! first names the host, is valid now and may serve TLS; a path leads from it through
//! certificate authorities the server sent to a trusted root's key. Names are compared as
//! encoded.

use crate::x509::{Certificate, KEY_CERT_SIGN, ParseError, PublicKey};

/// The most certificates a server may send.
pub const MAX_CHAIN: usize = 10;

/// The trusted roots: certificates whose keys end a path.
pub trait TrustAnchors {
    /// Calls `visit` with each root certificate's DER until it returns true; returns whether
    /// it did, or `Err` when the roots cannot be read.
    fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots>;
}

/// The roots cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRoots;

/// Why a server's certificates are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateError {
    /// The server sent none, or more than [`MAX_CHAIN`].
    BadChain,
    /// A certificate is not well formed.
    Malformed,
    /// A certificate uses an algorithm EggOS does not check.
    UnsupportedAlgorithm,
    /// A certificate has a critical extension EggOS does not process.
    UnknownCriticalExtension,
    /// The server's certificate is for another host.
    WrongHost,
    /// The server's certificate has expired.
    Expired,
    /// The server's certificate is not valid yet.
    NotYetValid,
    /// The server's certificate may not serve TLS.
    NotForServers,
    /// No path leads to a trusted root.
    UntrustedIssuer,
    /// No roots could be read.
    NoRoots,
}

impl CertificateError {
    /// What is wrong, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::BadChain => "the server sent no certificates, or too many",
            Self::Malformed => "the server's certificate is malformed",
            Self::UnsupportedAlgorithm => {
                "the server's certificate uses a key or signature EggOS does not check"
            }
            Self::UnknownCriticalExtension => {
                "the server's certificate has a critical extension EggOS does not process"
            }
            Self::WrongHost => "the server's certificate is for another host",
            Self::Expired => {
                "the server's certificate has expired (or the clock is wrong: try date --sync)"
            }
            Self::NotYetValid => {
                "the server's certificate is not valid yet (or the clock is wrong: try date --sync)"
            }
            Self::NotForServers => "the server's certificate may not be used by a server",
            Self::UntrustedIssuer => "the server's certificate is not issued by a trusted root",
            Self::NoRoots => {
                "no trusted root certificates: put them, as PEM, in /etc/ssl/roots.pem or in roots.pem \
                 in the host folder"
            }
        }
    }
}

impl From<ParseError> for CertificateError {
    fn from(error: ParseError) -> Self {
        match error {
            ParseError::Malformed => Self::Malformed,
            ParseError::UnsupportedAlgorithm => Self::UnsupportedAlgorithm,
            ParseError::UnknownCriticalExtension => Self::UnknownCriticalExtension,
        }
    }
}

fn may_issue(authority: &Certificate<'_>, below: usize, now: i64) -> bool {
    authority.is_authority
        && authority.is_valid_at(now)
        && authority
            .key_usage
            .is_none_or(|usage| usage & KEY_CERT_SIGN != 0)
        && authority
            .path_length
            .is_none_or(|length| below <= length as usize)
}

/// Checks the server's certificates, `chain` (its own first), for `host` at `now` (seconds
/// since 1970) against `anchors`; returns the server certificate's key.
pub fn verify<'a>(
    chain: &[&'a [u8]],
    host: &[u8],
    now: i64,
    anchors: &mut dyn TrustAnchors,
) -> Result<PublicKey<'a>, CertificateError> {
    if chain.is_empty() || chain.len() > MAX_CHAIN {
        return Err(CertificateError::BadChain);
    }
    let mut parsed: [Option<Certificate<'a>>; MAX_CHAIN] = [None; MAX_CHAIN];
    for (slot, der) in parsed.iter_mut().zip(chain) {
        *slot = Some(Certificate::parse(der)?);
    }
    let count = chain.len();
    let leaf = parsed[0].ok_or(CertificateError::Malformed)?;
    if !leaf.names_host(host) {
        return Err(CertificateError::WrongHost);
    }
    if now < leaf.not_before {
        return Err(CertificateError::NotYetValid);
    }
    if now > leaf.not_after {
        return Err(CertificateError::Expired);
    }
    let signs = leaf
        .key_usage
        .is_none_or(|usage| usage & crate::x509::DIGITAL_SIGNATURE != 0);
    if !leaf.serves_tls || !signs {
        return Err(CertificateError::NotForServers);
    }

    let mut anchored = [false; MAX_CHAIN];
    let mut any_root = false;
    let mut visit = |root: &[u8]| {
        any_root = true;
        let Ok(root) = Certificate::parse(root) else {
            return false;
        };
        for (index, certificate) in parsed.iter().enumerate().take(count) {
            if let Some(certificate) = certificate
                && !anchored[index]
                && certificate.issuer == root.subject
                && certificate.is_signed_by(&root.public_key)
            {
                anchored[index] = true;
            }
        }
        anchored[0]
    };
    anchors
        .each(&mut visit)
        .map_err(|NoRoots| CertificateError::NoRoots)?;
    if !any_root {
        return Err(CertificateError::NoRoots);
    }
    match reaches_root(&parsed[..count], &anchored, 0, 1, now) {
        true => Ok(leaf.public_key),
        false => Err(CertificateError::UntrustedIssuer),
    }
}

fn reaches_root(
    chain: &[Option<Certificate<'_>>],
    anchored: &[bool; MAX_CHAIN],
    index: usize,
    visited: u32,
    now: i64,
) -> bool {
    if anchored[index] {
        return true;
    }
    let Some(certificate) = chain[index] else {
        return false;
    };
    let below = visited.count_ones() as usize - 1;
    chain.iter().enumerate().any(|(candidate, authority)| {
        visited & (1 << candidate) == 0
            && authority.is_some_and(|authority| {
                authority.subject == certificate.issuer
                    && may_issue(&authority, below, now)
                    && certificate.is_signed_by(&authority.public_key)
                    && reaches_root(chain, anchored, candidate, visited | (1 << candidate), now)
            })
    })
}
