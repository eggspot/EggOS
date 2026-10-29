//! EggOS's cryptography (D159), written from the specifications: SHA-256, SHA-384 and SHA-512
//! (FIPS 180-4, [`sha2`]), HMAC and HKDF (RFC 2104, RFC 5869, [`hmac`]), AES (FIPS 197,
//! [`aes`]) in GCM (NIST SP 800-38D, [`gcm`]), ChaCha20 and Poly1305 (RFC 8439, [`chacha20`],
//! [`poly1305`], [`chacha20poly1305`]), X25519 (RFC 7748, [`x25519`]), and the checks of
//! signatures that certificates and TLS servers make: ECDSA on P-256 and P-384 (FIPS 186-5,
//! [`ecdsa`]) and RSA with PKCS #1 v1.5 and PSS padding (RFC 8017, [`rsa`]), over
//! [`bignum`]'s modular arithmetic.
//!
//! Code that handles secrets (hashing, MACs, AES, GCM, ChaCha20, Poly1305, X25519) takes the
//! same time and touches the same memory whatever the secret is: no branch and no table
//! index depends on it ([`constant_time`]). Signature checks only handle public values and
//! are not constant-time. Nothing here allocates.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop)]

pub mod aes;
pub mod bignum;
pub mod chacha20;
pub mod chacha20poly1305;
pub mod constant_time;
pub mod ecdsa;
pub mod gcm;
pub mod hmac;
pub mod poly1305;
pub mod rsa;
pub mod sha2;
pub mod x25519;

#[cfg(test)]
mod tests;

pub use sha2::{Digest, HashAlgorithm, Hasher};

/// An authenticated message whose tag did not match: it was damaged or forged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed;
