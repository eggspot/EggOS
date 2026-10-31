//! EggOS's TLS 1.3 client (D159), written from RFC 8446 over [`eggos_crypto`]: the handshake
//! with X25519 key exchange, AES-GCM and ChaCha20-Poly1305 records ([`client`]), and the
//! server's certificate checked against the trusted roots ([`x509`], [`chain`]) read from PEM
//! ([`pem`]) in DER ([`der`]). EggOS's own self-signed server certificate is built here too
//! ([`certificate`], D193).
//!
//! Nothing here allocates, reads a clock or touches a socket: the caller passes the time, the
//! randomness, the buffers, the roots and a [`client::Transport`] for the bytes, so the same
//! code runs in programs without an allocator and in host tests.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod certificate;
pub mod chain;
pub mod client;
pub mod der;
pub mod pem;
pub mod x509;

#[cfg(test)]
mod tests;

pub use chain::{CertificateError, TrustAnchors};
pub use client::{Buffers, Error, Session, Stopped, Transport};
