use crate::certificate;
use crate::chain::{self, CertificateError, NoRoots, TrustAnchors};
use crate::client::{
    self, Buffers, CipherSuite, Error, Protection, RANDOMNESS_BYTES, Settings, Stopped, Transport,
    connect,
};
use crate::der::{self, Reader};
use crate::pem::Certificates;
use crate::server;
use crate::x509::{self, Certificate, DIGITAL_SIGNATURE, PublicKey};
use eggos_crypto::HashAlgorithm;

const ROOT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBmjCCAUGgAwIBAgIUL5GE8J+PtqEhWAyl4z1JUR0Ad2owCgYIKoZIzj0EAwIw
GjEYMBYGA1UEAwwPRWdnT1MgVGVzdCBSb290MCAXDTI2MTAyOTE1NDcxOVoYDzIw
NTEwNjIwMTU0NzE5WjAaMRgwFgYDVQQDDA9FZ2dPUyBUZXN0IFJvb3QwWTATBgcq
hkjOPQIBBggqhkjOPQMBBwNCAASoUyaY5CexBuax/NHIVRkoK/DbxXAuUrYTPw6L
DdfyltRPu+/mqvsbHhA3wipj8pTJP0r/ATh8nLTM5UUCElMHo2MwYTAdBgNVHQ4E
FgQUzGy8JZOQxz+ikpMi9dGFTBe2HlQwHwYDVR0jBBgwFoAUzGy8JZOQxz+ikpMi
9dGFTBe2HlQwDwYDVR0TAQH/BAUwAwEB/zAOBgNVHQ8BAf8EBAMCAQYwCgYIKoZI
zj0EAwIDRwAwRAIgCtcbLF9+7QNN2f35OaELClQ8GTHgdhd9um7MNn/wdW8CIBE+
7sGqjhjjHZ93My+AhMj1GyR4dZIGr0oMcwPTQkn/
-----END CERTIFICATE-----
";

const LEAF_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIB0zCCAXqgAwIBAgIUFBsKY7Ds1tz7t64kUbWvASsnjB0wCgYIKoZIzj0EAwIw
GjEYMBYGA1UEAwwPRWdnT1MgVGVzdCBSb290MB4XDTI2MTAyOTE1NDcxOVoXDTQ4
MDkyMzE1NDcxOVowFTETMBEGA1UEAwwKdGVzdC5lZ2dvczBZMBMGByqGSM49AgEG
CCqGSM49AwEHA0IABN7xwsIYBo0K8uRnf/sZuR9LVF3kn0MoAxLuC6y+JJEHQjnc
YYjlUzKbN/M728IYVFI8ah5YAzl5i5QgjHm29CyjgaIwgZ8wKgYDVR0RBCMwIYIK
dGVzdC5lZ2dvc4INKi5maWxlcy5lZ2dvc4cECgACAjAMBgNVHRMBAf8EAjAAMA4G
A1UdDwEB/wQEAwIHgDATBgNVHSUEDDAKBggrBgEFBQcDATAdBgNVHQ4EFgQUBbUB
u8kfQfrR0EXLrYqWEkPpTfQwHwYDVR0jBBgwFoAUzGy8JZOQxz+ikpMi9dGFTBe2
HlQwCgYIKoZIzj0EAwIDRwAwRAIgI/RzXYvCFkyN1UORM2Aev6k+QXohtfWIZdZz
XY2IqmECIBL3chANEk5ojnYEE7gDvW9/rBi6NX8DM86TgSIe0aAd
-----END CERTIFICATE-----
";

const IN_2030: i64 = 1_893_456_000;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn decode(text: &str, piece: usize) -> Vec<Vec<u8>> {
    let mut der = [0u8; 4096];
    let mut decoder = Certificates::new(&mut der);
    let mut found = Vec::new();
    let mut visit = |certificate: &[u8]| {
        found.push(certificate.to_vec());
        false
    };
    for chunk in text.as_bytes().chunks(piece) {
        decoder.feed(chunk, &mut visit);
    }
    decoder.finish(&mut visit);
    found
}

fn root() -> Vec<u8> {
    decode(ROOT_PEM, 1000).remove(0)
}

fn leaf() -> Vec<u8> {
    decode(LEAF_PEM, 1000).remove(0)
}

struct Roots(Vec<Vec<u8>>);

impl TrustAnchors for Roots {
    fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
        Ok(self.0.iter().any(|root| visit(root)))
    }
}

struct Unreadable;

impl TrustAnchors for Unreadable {
    fn each(&mut self, _: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
        Err(NoRoots)
    }
}

#[test]
fn pem_blocks_decode_whatever_the_pieces_and_text_around_them() {
    let text = format!("# roots\n{ROOT_PEM}junk line\n{}", LEAF_PEM.trim_end());
    for piece in [1, 7, 64, 5000] {
        let found = decode(&text, piece);
        assert_eq!(found.len(), 2, "pieces of {piece}");
        assert_eq!(found[0], root());
        assert_eq!(found[1][0], der::SEQUENCE);
    }
    let crlf = ROOT_PEM.replace('\n', "\r\n");
    assert_eq!(decode(&crlf, 3), vec![root()]);
    let mut small = [0u8; 100];
    let mut decoder = Certificates::new(&mut small);
    let mut seen = 0;
    decoder.feed(ROOT_PEM.as_bytes(), &mut |_| {
        seen += 1;
        false
    });
    assert_eq!(seen, 0, "a certificate longer than the buffer is skipped");
}

#[test]
fn der_lengths_must_be_minimal_and_whole() {
    let mut reader = Reader::new(&[0x04, 0x02, 0xaa, 0xbb, 0x05, 0x00]);
    assert_eq!(reader.expect(0x04).unwrap(), &[0xaa, 0xbb]);
    assert_eq!(reader.expect(0x05).unwrap(), &[] as &[u8]);
    assert!(reader.is_empty());
    assert!(Reader::new(&[0x04, 0x81, 0x02, 0, 0]).read().is_err());
    assert!(Reader::new(&[0x04, 0x80]).read().is_err());
    assert!(Reader::new(&[0x04, 0x03, 0]).read().is_err());
    assert!(Reader::new(&[0x1f, 0x01, 0]).read().is_err());
    assert_eq!(der::positive_integer(&[0, 0x80]), Some(&[0x80][..]));
    assert_eq!(der::positive_integer(&[0x80]), None);
}

#[test]
fn times_convert_to_seconds_since_1970() {
    assert_eq!(x509::time(der::UTC_TIME, b"700101000000Z"), Some(0));
    assert_eq!(x509::time(der::UTC_TIME, b"300101000000Z"), Some(IN_2030));
    assert_eq!(
        x509::time(der::GENERALIZED_TIME, b"20300101000000Z"),
        Some(IN_2030)
    );
    assert_eq!(
        x509::time(der::UTC_TIME, b"491231235959Z"),
        Some(2_524_607_999)
    );
    assert_eq!(x509::time(der::UTC_TIME, b"300101000000+0100"), None);
    assert_eq!(x509::time(der::UTC_TIME, b"301301000000Z"), None);
}

#[test]
fn certificates_parse_with_their_names_keys_and_extensions() {
    let root_der = root();
    let leaf_der = leaf();
    let root = Certificate::parse(&root_der).unwrap();
    let leaf = Certificate::parse(&leaf_der).unwrap();
    assert!(root.is_authority);
    assert_eq!(root.issuer, root.subject);
    assert_eq!(leaf.issuer, root.subject);
    assert!(!leaf.is_authority);
    assert!(leaf.serves_tls);
    assert!(leaf.key_usage.unwrap() & DIGITAL_SIGNATURE != 0);
    assert!(matches!(leaf.public_key, PublicKey::Ec { point, .. } if point.len() == 65));
    assert!(leaf.is_valid_at(IN_2030));
    assert!(!leaf.is_valid_at(0));
    assert!(root.not_after > leaf.not_after);
    assert!(leaf.is_signed_by(&root.public_key));
    assert!(root.is_signed_by(&root.public_key));
    assert!(!root.is_signed_by(&leaf.public_key));
    for (host, named) in [
        (&b"test.eggos"[..], true),
        (b"TEST.Eggos", true),
        (b"test.eggos.", true),
        (b"a.files.eggos", true),
        (b"files.eggos", false),
        (b"a.b.files.eggos", false),
        (b"10.0.2.2", true),
        (b"10.0.2.3", false),
        (b"other.eggos", false),
    ] {
        assert_eq!(
            leaf.names_host(host),
            named,
            "{}",
            String::from_utf8_lossy(host)
        );
    }
    let mut damaged = leaf_der.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    let damaged = Certificate::parse(&damaged).unwrap();
    assert!(!damaged.is_signed_by(&root.public_key));
    assert!(Certificate::parse(&leaf_der[..leaf_der.len() - 1]).is_err());
}

#[test]
fn a_chain_is_trusted_only_up_to_a_root_for_its_host_and_time() {
    let (root, leaf) = (root(), leaf());
    let mut roots = Roots(vec![root.clone()]);
    let check = |chain: &[&[u8]], host: &[u8], now: i64, roots: &mut dyn TrustAnchors| {
        chain::verify(chain, host, now, roots).map(|_| ())
    };
    assert_eq!(check(&[&leaf], b"test.eggos", IN_2030, &mut roots), Ok(()));
    assert_eq!(
        check(&[&leaf, &root], b"x.files.eggos", IN_2030, &mut roots),
        Ok(())
    );
    assert_eq!(
        check(&[&leaf], b"evil.eggos", IN_2030, &mut roots),
        Err(CertificateError::WrongHost)
    );
    assert_eq!(
        check(&[&leaf], b"test.eggos", 1_700_000_000, &mut roots),
        Err(CertificateError::NotYetValid)
    );
    assert_eq!(
        check(&[&leaf], b"test.eggos", 2_600_000_000, &mut roots),
        Err(CertificateError::Expired)
    );
    assert_eq!(
        check(
            &[&leaf],
            b"test.eggos",
            IN_2030,
            &mut Roots(vec![leaf.clone()])
        ),
        Err(CertificateError::UntrustedIssuer)
    );
    assert_eq!(
        check(
            &[&leaf, &root],
            b"test.eggos",
            IN_2030,
            &mut Roots(Vec::new())
        ),
        Err(CertificateError::NoRoots)
    );
    assert_eq!(
        check(&[&leaf], b"test.eggos", IN_2030, &mut Unreadable),
        Err(CertificateError::NoRoots)
    );
    assert_eq!(
        check(&[], b"test.eggos", IN_2030, &mut roots),
        Err(CertificateError::BadChain)
    );
    assert_eq!(
        check(&[&root], b"test.eggos", IN_2030, &mut roots),
        Err(CertificateError::WrongHost)
    );
}

#[test]
fn a_path_search_over_mutually_issuing_authorities_checks_each_pair_at_most_once() {
    let mut checks = 0;
    let anchored = [false; chain::MAX_CHAIN];
    let found = chain::reaches_root(chain::MAX_CHAIN, &anchored, |child, authority, _| {
        checks += 1;
        child != authority
    });
    assert!(!found);
    assert!(checks <= chain::MAX_CHAIN * (chain::MAX_CHAIN - 1));
}

#[test]
fn a_path_through_intermediates_honours_each_authoritys_path_length() {
    let issuer = [1, 2, 3, usize::MAX];
    let mut anchored = [false; chain::MAX_CHAIN];
    anchored[3] = true;
    let search = |path_length: [usize; 4]| {
        chain::reaches_root(4, &anchored, |child, authority, below| {
            issuer[child] == authority && below <= path_length[authority]
        })
    };
    assert!(search([0, 0, 1, 2]));
    assert!(search([0, 5, 5, 5]));
    assert!(!search([0, 0, 0, 2]));
    assert!(!search([0, 0, 1, 1]));
    let unanchored = [false; chain::MAX_CHAIN];
    assert!(!chain::reaches_root(
        4,
        &unanchored,
        |child, authority, _| { issuer[child] == authority }
    ));
}

#[test]
fn the_key_schedule_matches_rfc_8448() {
    let early = client::early_secret(HashAlgorithm::Sha256);
    assert_eq!(
        early.as_bytes(),
        hex("33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a")
    );
    let empty = eggos_crypto::sha2::digest(HashAlgorithm::Sha256, b"");
    let derived = client::derive_secret(
        HashAlgorithm::Sha256,
        early.as_bytes(),
        b"derived",
        empty.as_bytes(),
    );
    assert_eq!(
        derived.as_bytes(),
        hex("6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba")
    );
}

#[test]
fn records_sealed_by_one_side_open_on_the_other_only_unchanged() {
    for suite in [
        CipherSuite::Aes128GcmSha256,
        CipherSuite::Aes256GcmSha384,
        CipherSuite::ChaCha20Poly1305Sha256,
    ] {
        let secret = [7u8; 48];
        let secret = &secret[..suite.hash().output_bytes()];
        let mut sender = Protection::new(suite, secret);
        let mut receiver = Protection::new(suite, secret);
        let header = [23, 3, 3, 0, 22];
        for round in 0..3u8 {
            let mut data = [round, 1, 2, 3, 4, 23];
            let tag = sender.seal(&header, &mut data);
            assert_ne!(data[..5], [round, 1, 2, 3, 4]);
            receiver.open(&header, &mut data, &tag).unwrap();
            assert_eq!(data, [round, 1, 2, 3, 4, 23], "{}", suite.name());
        }
        let mut data = [9u8; 6];
        let tag = sender.seal(&header, &mut data);
        data[0] ^= 1;
        assert_eq!(
            receiver.open(&header, &mut data, &tag),
            Err(Error::BadRecord)
        );
    }
}

fn randomness() -> [u8; RANDOMNESS_BYTES] {
    core::array::from_fn(|index| index as u8)
}

#[test]
fn the_client_hello_offers_tls_1_3_x25519_and_names_the_host() {
    let hello = client::client_hello_for_tests(b"example.com", &randomness());
    assert_eq!(hello[0], 1);
    let length =
        (usize::from(hello[1]) << 16) | (usize::from(hello[2]) << 8) | usize::from(hello[3]);
    assert_eq!(length + 4, hello.len());
    assert_eq!(&hello[4..6], &[3, 3]);
    assert_eq!(&hello[6..38], &randomness()[..32]);
    assert_eq!(hello[38], 32);
    let contains = |needle: &[u8]| hello.windows(needle.len()).any(|window| window == needle);
    assert!(contains(b"example.com"));
    assert!(contains(&[0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04]));
    assert!(contains(&[0x00, 0x06, 0x13, 0x03, 0x13, 0x01, 0x13, 0x02]));
    assert!(contains(b"http/1.1"));
    let by_address = client::client_hello_for_tests(b"10.0.2.2", &randomness());
    assert!(!by_address.windows(8).any(|window| window == b"10.0.2.2"));
}

struct Scripted {
    answer: Vec<u8>,
    sent: Vec<u8>,
}

impl Transport for Scripted {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        self.sent.extend_from_slice(bytes);
        Ok(())
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        let count = self.answer.len().min(buffer.len()).min(5);
        buffer[..count].copy_from_slice(&self.answer[..count]);
        self.answer.drain(..count);
        Ok(count)
    }
}

fn server_hello(random: [u8; 32], extensions: &[u8]) -> Vec<u8> {
    let mut body = vec![3, 3];
    body.extend_from_slice(&random);
    body.push(32);
    body.extend_from_slice(&randomness()[32..64]);
    body.extend_from_slice(&[0x13, 0x01, 0]);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(extensions);
    let mut message = vec![2, 0, (body.len() >> 8) as u8, body.len() as u8];
    message.extend_from_slice(&body);
    let mut record = vec![22, 3, 3, (message.len() >> 8) as u8, message.len() as u8];
    record.extend_from_slice(&message);
    record
}

fn handshake_with(answer: Vec<u8>) -> Error {
    let mut transport = Scripted {
        answer,
        sent: Vec::new(),
    };
    let mut buffers = Box::new(Buffers::new());
    let settings = Settings {
        host: b"test.eggos",
        now: IN_2030,
    };
    let result = connect(
        &mut transport,
        &mut buffers,
        &settings,
        &randomness(),
        &mut Roots(vec![root()]),
    );
    assert_eq!(transport.sent[0], 22, "the ClientHello went out first");
    result.err().unwrap()
}

#[test]
fn a_handshake_fails_with_the_reason_the_server_gives() {
    assert_eq!(handshake_with(Vec::new()), Error::Closed);
    assert_eq!(
        handshake_with(vec![21, 3, 3, 0, 2, 2, 40]),
        Error::Alert(40)
    );
    assert_eq!(handshake_with(server_hello([1; 32], &[])), Error::NotTls13);
    let retry = [
        0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8,
        0x91, 0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8,
        0x33, 0x9c,
    ];
    assert_eq!(
        handshake_with(server_hello(retry, &[0, 43, 0, 2, 3, 4])),
        Error::NoCommonKeyExchange
    );
    let mut secp256r1 = vec![0, 43, 0, 2, 3, 4, 0, 51, 0, 6, 0, 23, 0, 2, 4, 0];
    assert_eq!(
        handshake_with(server_hello([1; 32], &secp256r1)),
        Error::NoCommonKeyExchange
    );
    secp256r1.truncate(6);
    assert_eq!(
        handshake_with(server_hello([1; 32], &secp256r1)),
        Error::Malformed
    );
    assert_eq!(handshake_with(vec![22, 3, 3, 0x42, 0x01]), Error::Malformed);
}

#[test]
fn errors_describe_themselves() {
    for error in [
        Error::Closed,
        Error::Certificate(CertificateError::NoRoots),
        Error::Alert(40),
        Error::BadRecord,
    ] {
        assert!(!error.description().is_empty());
    }
    assert!(
        CertificateError::Expired
            .description()
            .contains("date --sync")
    );
}

const SERVER_KEY: [u8; 32] = [
    0xc9, 0xaf, 0xa9, 0xd8, 0x45, 0xba, 0x75, 0x16, 0x6b, 0x5c, 0x21, 0x57, 0x67, 0xb1, 0xd6, 0x93,
    0x4e, 0x50, 0xc3, 0xdb, 0x36, 0xe8, 0x9b, 0x12, 0x7b, 0x8a, 0x62, 0x2b, 0x12, 0x0f, 0x67, 0x21,
];
const OCTOBER_2026: i64 = 1_793_000_000;

fn own_certificate(names: &[certificate::Name<'_>]) -> Vec<u8> {
    let request = certificate::Request {
        private_key: &SERVER_KEY,
        names,
        serial: [0x9a; certificate::SERIAL_BYTES],
        not_before: OCTOBER_2026,
        not_after: OCTOBER_2026 + 10 * 365 * 86_400,
    };
    let mut out = [0u8; certificate::MAX_CERTIFICATE_BYTES];
    let length = certificate::self_signed(&request, &mut out).unwrap();
    out[..length].to_vec()
}

#[test]
fn eggos_signs_its_own_certificate_and_a_client_pinning_it_accepts_it() {
    let names = [
        certificate::Name::Host(b"eggos.local"),
        certificate::Name::Ipv4([10, 0, 2, 15]),
    ];
    let der = own_certificate(&names);
    let parsed = Certificate::parse(&der).unwrap();
    assert!(parsed.is_valid_at(OCTOBER_2026 + 86_400));
    assert!(!parsed.is_valid_at(OCTOBER_2026 - 1));
    assert!(parsed.names_host(b"eggos.local"));
    assert!(parsed.names_host(b"10.0.2.15"));
    assert!(!parsed.names_host(b"other.local"));
    assert!(parsed.is_signed_by(&parsed.public_key));
    let mut pinned = Roots(vec![der.clone()]);
    for host in [&b"eggos.local"[..], b"10.0.2.15"] {
        assert!(chain::verify(&[&der], host, OCTOBER_2026 + 60, &mut pinned).is_ok());
    }
    assert_eq!(
        chain::verify(
            &[&der],
            b"eggos.local",
            OCTOBER_2026 + 60,
            &mut Roots(vec![root()])
        )
        .err(),
        Some(CertificateError::UntrustedIssuer)
    );
    assert_eq!(der, own_certificate(&names));
}

#[test]
fn eggos_certificates_cross_2050_in_generalized_time_and_refuse_bad_requests() {
    let names = [certificate::Name::Host(b"eggos.local")];
    let request = certificate::Request {
        private_key: &SERVER_KEY,
        names: &names,
        serial: [1; certificate::SERIAL_BYTES],
        not_before: OCTOBER_2026,
        not_after: 2_556_143_999,
    };
    let mut out = [0u8; certificate::MAX_CERTIFICATE_BYTES];
    let length = certificate::self_signed(&request, &mut out).unwrap();
    let parsed = Certificate::parse(&out[..length]).unwrap();
    assert!(parsed.is_valid_at(2_556_143_999));
    assert!(!parsed.is_valid_at(2_556_144_000));
    for bad in [
        certificate::Request {
            names: &[],
            ..request
        },
        certificate::Request {
            names: &[certificate::Name::Host(b"bad name")],
            ..request
        },
    ] {
        assert_eq!(
            certificate::self_signed(&bad, &mut out),
            Err(certificate::BuildError::BadNames)
        );
    }
    assert_eq!(
        certificate::self_signed(
            &certificate::Request {
                not_after: OCTOBER_2026 - 1,
                ..request
            },
            &mut out
        ),
        Err(certificate::BuildError::BadValidity)
    );
    assert_eq!(
        certificate::self_signed(
            &certificate::Request {
                private_key: &[0; 32],
                ..request
            },
            &mut out
        ),
        Err(certificate::BuildError::InvalidKey)
    );
    assert_eq!(
        certificate::self_signed(&request, &mut out[..100]),
        Err(certificate::BuildError::TooLarge)
    );
}

#[test]
fn eggos_certificates_read_back_from_their_pem_and_show_their_fingerprint() {
    let der = own_certificate(&[certificate::Name::Ipv4([192, 168, 1, 20])]);
    let mut text = vec![0u8; certificate::pem_bytes(der.len())];
    let length = certificate::pem(&der, &mut text).unwrap();
    assert_eq!(length, text.len());
    assert!(certificate::pem(&der, &mut text[..length - 1]).is_none());
    let text = String::from_utf8(text).unwrap();
    assert!(text.lines().all(|line| line.len() <= 64));
    assert_eq!(decode(&text, 13), vec![der.clone()]);
    let shown = certificate::fingerprint_text(&certificate::fingerprint(&der));
    let shown = core::str::from_utf8(&shown).unwrap();
    assert_eq!(shown.len(), 95);
    assert_eq!(shown.split(':').count(), 32);
    assert!(
        shown
            .split(':')
            .all(|pair| pair.len() == 2 && pair.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
}

struct Pipe {
    outgoing: std::sync::mpsc::Sender<Vec<u8>>,
    incoming: std::sync::mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Transport for Pipe {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        self.outgoing.send(bytes.to_vec()).map_err(|_| Stopped)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        if self.pending.is_empty() {
            match self.incoming.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

fn pipes() -> (Pipe, Pipe) {
    let (to_server, from_client) = std::sync::mpsc::channel();
    let (to_client, from_server) = std::sync::mpsc::channel();
    (
        Pipe {
            outgoing: to_server,
            incoming: from_server,
            pending: Vec::new(),
        },
        Pipe {
            outgoing: to_client,
            incoming: from_client,
            pending: Vec::new(),
        },
    )
}

#[test]
fn eggos_clients_and_eggos_servers_talk_through_a_pinned_certificate() {
    let names = [certificate::Name::Host(b"eggos.local")];
    let der = own_certificate(&names);
    let (mut client_end, mut server_end) = pipes();
    let served = der.clone();
    let server = std::thread::spawn(move || {
        let mut buffers = Box::new(Buffers::new());
        let identity = server::Identity {
            certificate: &served,
            private_key: &SERVER_KEY,
            protocol: b"http/1.1",
        };
        let mut session =
            server::accept(&mut server_end, &mut buffers, &identity, &[0x42; 64]).unwrap();
        let mut request = [0u8; 64];
        let count = session.read(&mut server_end, &mut request).unwrap();
        assert_eq!(&request[..count], b"POST /mcp");
        session.write(&mut server_end, b"200 OK").unwrap();
        session.close(&mut server_end).unwrap();
        session.suite()
    });
    let mut buffers = Box::new(Buffers::new());
    let settings = Settings {
        host: b"eggos.local",
        now: OCTOBER_2026 + 60,
    };
    let mut pinned = Roots(vec![der]);
    let mut session = connect(
        &mut client_end,
        &mut buffers,
        &settings,
        &[0x17; RANDOMNESS_BYTES],
        &mut pinned,
    )
    .unwrap();
    session.write(&mut client_end, b"POST /mcp").unwrap();
    let mut answer = [0u8; 64];
    let count = session.read(&mut client_end, &mut answer).unwrap();
    assert_eq!(&answer[..count], b"200 OK");
    assert_eq!(session.read(&mut client_end, &mut answer).unwrap(), 0);
    assert_eq!(server.join().unwrap(), session.suite());
}

#[test]
fn eggos_servers_refuse_clients_without_tls_13_or_x25519() {
    let names = [certificate::Name::Host(b"eggos.local")];
    let der = own_certificate(&names);
    let identity = server::Identity {
        certificate: &der,
        private_key: &SERVER_KEY,
        protocol: b"http/1.1",
    };
    let hello = client::client_hello_for_tests(b"eggos.local", &[0x17; RANDOMNESS_BYTES]);
    let mut without_x25519 = hello.clone();
    let share = without_x25519
        .windows(4)
        .rposition(|window| window == [0x00, 0x1d, 0x00, 0x20])
        .unwrap();
    without_x25519[share + 1] = 0x17;
    let mut old_version = hello.clone();
    let version = old_version
        .windows(5)
        .position(|window| window == [0x00, 0x2b, 0x00, 0x03, 0x02])
        .unwrap();
    old_version[version + 6] = 0x03;
    for (message, wanted) in [
        (without_x25519, server::Error::NoCommonKeyExchange),
        (old_version, server::Error::NotTls13),
    ] {
        let (mut client_end, mut server_end) = pipes();
        let mut record = vec![22, 3, 1, (message.len() >> 8) as u8, message.len() as u8];
        record.extend_from_slice(&message);
        client_end.send(&record).unwrap();
        let mut buffers = Box::new(Buffers::new());
        let refused = server::accept(&mut server_end, &mut buffers, &identity, &[0x42; 64]);
        assert_eq!(refused.err(), Some(wanted));
    }
}
