use crate::aes::{self, Aes};
use crate::bignum::{Modulus, Number};
use crate::chacha20;
use crate::chacha20poly1305::ChaCha20Poly1305;
use crate::constant_time::equal;
use crate::ecdsa::{self, Curve};
use crate::gcm::AesGcm;
use crate::hmac::{self, Hmac};
use crate::p256;
use crate::poly1305;
use crate::rsa::{KeyError, PublicKey};
use crate::sha2::{HashAlgorithm, Hasher, digest};
use crate::x25519;

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn array<const N: usize>(text: &str) -> [u8; N] {
    hex(text).try_into().unwrap()
}

#[test]
fn sha2_digests_match_fips_180_examples() {
    let cases = [
        (
            HashAlgorithm::Sha256,
            &b"abc"[..],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            HashAlgorithm::Sha256,
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            HashAlgorithm::Sha256,
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            HashAlgorithm::Sha384,
            b"abc",
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7",
        ),
        (
            HashAlgorithm::Sha512,
            b"abc",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
    ];
    for (algorithm, message, expected) in cases {
        assert_eq!(
            digest(algorithm, message).as_bytes(),
            hex(expected),
            "{algorithm:?}"
        );
    }
}

#[test]
fn a_hash_fed_in_pieces_equals_the_hash_of_the_whole() {
    let message: Vec<u8> = (0..1000u32).map(|value| (value * 7) as u8).collect();
    for algorithm in [
        HashAlgorithm::Sha256,
        HashAlgorithm::Sha384,
        HashAlgorithm::Sha512,
    ] {
        let whole = digest(algorithm, &message);
        let mut hasher = Hasher::new(algorithm);
        for piece in message.chunks(13) {
            hasher.update(piece);
        }
        assert_eq!(hasher.finish(), whole);
    }
}

#[test]
fn hmac_matches_rfc_4231() {
    let key = [0x0b; 20];
    assert_eq!(
        hmac::hmac(HashAlgorithm::Sha256, &key, b"Hi There").as_bytes(),
        hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
    );
    assert_eq!(
        hmac::hmac(HashAlgorithm::Sha384, &key, b"Hi There").as_bytes(),
        hex(
            "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59c\
             faea9ea9076ede7f4af152e8b2fa9cb6"
        )
    );
    assert_eq!(
        hmac::hmac(HashAlgorithm::Sha512, &key, b"Hi There").as_bytes(),
        hex(
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        )
    );
    let mut mac = Hmac::new(HashAlgorithm::Sha256, b"Jefe");
    mac.update(b"what do ya want ");
    mac.update(b"for nothing?");
    assert_eq!(
        mac.finish().as_bytes(),
        hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    let long_key = [0xaa; 200];
    let hashed_key = digest(HashAlgorithm::Sha256, &long_key);
    assert_eq!(
        hmac::hmac(HashAlgorithm::Sha256, &long_key, b"x"),
        hmac::hmac(HashAlgorithm::Sha256, hashed_key.as_bytes(), b"x")
    );
}

#[test]
fn hkdf_matches_rfc_5869_test_case_1() {
    let input = [0x0b; 22];
    let salt = hex("000102030405060708090a0b0c");
    let info = hex("f0f1f2f3f4f5f6f7f8f9");
    let key = hmac::extract(HashAlgorithm::Sha256, &salt, &input);
    assert_eq!(
        key.as_bytes(),
        hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
    );
    let mut out = [0u8; 42];
    hmac::expand(
        HashAlgorithm::Sha256,
        key.as_bytes(),
        &[&info[..5], &info[5..]],
        &mut out,
    )
    .unwrap();
    assert_eq!(
        out.as_slice(),
        hex(
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865"
        )
    );
    let mut too_long = [0u8; 255 * 32 + 1];
    assert!(hmac::expand(HashAlgorithm::Sha256, key.as_bytes(), &[], &mut too_long).is_err());
}

fn field_multiply(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 == 1 {
            product ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    product
}

fn reference_s_box(byte: u8) -> u8 {
    let mut inverse = 1u8;
    for _ in 0..254 {
        inverse = field_multiply(inverse, byte);
    }
    if byte == 0 {
        inverse = 0;
    }
    inverse
        ^ inverse.rotate_left(1)
        ^ inverse.rotate_left(2)
        ^ inverse.rotate_left(3)
        ^ inverse.rotate_left(4)
        ^ 0x63
}

#[test]
fn the_s_box_circuit_matches_the_s_box_for_every_byte() {
    assert_eq!(aes::substitute_byte(0x00), 0x63);
    assert_eq!(aes::substitute_byte(0x01), 0x7c);
    assert_eq!(aes::substitute_byte(0x53), 0xed);
    for byte in 0..=255u8 {
        assert_eq!(
            aes::substitute_byte(byte),
            reference_s_box(byte),
            "{byte:#04x}"
        );
    }
}

#[test]
fn aes_matches_fips_197_appendix_c() {
    let plain: [u8; 16] = array("00112233445566778899aabbccddeeff");
    let mut block = plain;
    Aes::new(&hex("000102030405060708090a0b0c0d0e0f"))
        .unwrap()
        .encrypt_block(&mut block);
    assert_eq!(block.as_slice(), hex("69c4e0d86a7b0430d8cdb78070b4c55a"));
    let mut block = plain;
    Aes::new(&hex(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    ))
    .unwrap()
    .encrypt_block(&mut block);
    assert_eq!(block.as_slice(), hex("8ea2b7ca516745bfeafc49904b496089"));
    assert!(Aes::new(&[0; 24]).is_err());
}

#[test]
fn aes_gcm_matches_the_gcm_specification_s_test_cases() {
    let zero = AesGcm::new(&[0; 16]).unwrap();
    let mut empty: [u8; 0] = [];
    assert_eq!(
        zero.seal(&[0; 12], &[], &mut empty).as_slice(),
        hex("58e2fccefa7e3061367f1d57a4e7455a")
    );
    let mut block = [0u8; 16];
    let tag = zero.seal(&[0; 12], &[], &mut block);
    assert_eq!(block.as_slice(), hex("0388dace60b6a392f328c2b971b2fe78"));
    assert_eq!(tag.as_slice(), hex("ab6e47d42cec13bdf53a67b21257bddf"));

    let cipher = AesGcm::new(&hex("feffe9928665731c6d6a8f9467308308")).unwrap();
    let nonce: [u8; 12] = array("cafebabefacedbaddecaf888");
    let plain = hex(
        "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
         1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
    );
    let cipher_text = hex(
        "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e\
         21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985",
    );
    let mut data = plain.clone();
    let tag = cipher.seal(&nonce, &[], &mut data);
    assert_eq!(data, cipher_text);
    assert_eq!(tag.as_slice(), hex("4d5c2af327cd64a62cf35abd2ba6fab4"));

    let additional = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
    let mut data = plain[..60].to_vec();
    let tag = cipher.seal(&nonce, &additional, &mut data);
    assert_eq!(data, cipher_text[..60]);
    assert_eq!(tag.as_slice(), hex("5bc94fbc3221a5db94fae95ae7121a47"));
    cipher.open(&nonce, &additional, &mut data, &tag).unwrap();
    assert_eq!(data, plain[..60]);

    let mut forged = cipher_text[..60].to_vec();
    forged[3] ^= 1;
    let before = forged.clone();
    assert!(cipher.open(&nonce, &additional, &mut forged, &tag).is_err());
    assert_eq!(forged, before);
}

#[test]
fn chacha20_and_poly1305_match_rfc_8439() {
    let key: [u8; 32] = core::array::from_fn(|index| index as u8);
    let nonce = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
    assert_eq!(
        chacha20::block(&key, 1, &nonce).as_slice(),
        hex(
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        )
    );
    let one_time: [u8; 32] =
        array("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
    assert_eq!(
        poly1305::tag(&one_time, b"Cryptographic Forum Research Group").as_slice(),
        hex("a8061dc1305136c6c22b8baf0c0127a9")
    );
}

#[test]
fn chacha20_poly1305_matches_rfc_8439_section_2_8_2() {
    let key: [u8; 32] = core::array::from_fn(|index| 0x80 + index as u8);
    let nonce: [u8; 12] = array("070000004041424344454647");
    let additional = hex("50515253c0c1c2c3c4c5c6c7");
    let plain = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip \
                  for the future, sunscreen would be it.";
    let aead = ChaCha20Poly1305::new(&key);
    let mut data = plain.to_vec();
    let tag = aead.seal(&nonce, &additional, &mut data);
    assert_eq!(
        data,
        hex(
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
             3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
             92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
             3ff4def08e4b7a9de576d26586cec64b6116"
        )
    );
    assert_eq!(tag.as_slice(), hex("1ae10b594f09e26a7e902ecbd0600691"));
    aead.open(&nonce, &additional, &mut data, &tag).unwrap();
    assert_eq!(data, plain);
    let mut bad_tag = tag;
    bad_tag[15] ^= 0x80;
    let mut sealed = plain.to_vec();
    let tag = aead.seal(&nonce, &additional, &mut sealed);
    assert!(
        aead.open(&nonce, &additional, &mut sealed, &bad_tag)
            .is_err()
    );
    assert!(aead.open(&nonce, b"other", &mut sealed, &tag).is_err());
}

#[test]
fn x25519_matches_rfc_7748() {
    let scalar = array("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
    let u = array("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
    assert_eq!(
        x25519::x25519(&scalar, &u).as_slice(),
        hex("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552")
    );
    let alice = array("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    let bob = array("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    let alice_public = x25519::public_key(&alice);
    let bob_public = x25519::public_key(&bob);
    assert_eq!(
        alice_public.as_slice(),
        hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
    );
    assert_eq!(
        bob_public.as_slice(),
        hex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
    );
    let shared = hex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    assert_eq!(x25519::x25519(&alice, &bob_public).as_slice(), shared);
    assert_eq!(x25519::x25519(&bob, &alice_public).as_slice(), shared);
}

#[test]
fn montgomery_arithmetic_agrees_with_plain_arithmetic() {
    let prime = 1_000_000_007u64;
    let modulus = Modulus::new(&Number::from_u64(prime)).unwrap();
    for (a, b) in [
        (3u64, 5u64),
        (999_999_999, 123_456_789),
        (0, 77),
        (prime - 1, prime - 1),
    ] {
        let (ma, mb) = (
            modulus.to_montgomery(&Number::from_u64(a)),
            modulus.to_montgomery(&Number::from_u64(b)),
        );
        let product = modulus.from_montgomery(&modulus.multiply(&ma, &mb));
        assert_eq!(product, Number::from_u64(a * b % prime));
        let sum = modulus.add(&Number::from_u64(a), &Number::from_u64(b));
        assert_eq!(sum, Number::from_u64((a + b) % prime));
        let difference = modulus.subtract(&Number::from_u64(a), &Number::from_u64(b));
        assert_eq!(difference, Number::from_u64((a + prime - b) % prime));
    }
    let seven = modulus.to_montgomery(&Number::from_u64(7));
    let inverse = modulus.from_montgomery(&modulus.invert_prime(&seven));
    let mut bytes = [0u8; 8];
    inverse.write_be_bytes(&mut bytes);
    assert_eq!(u64::from_be_bytes(bytes) * 7 % prime, 1);
    let big = Number::from_be_bytes(&[0xff; 40]).unwrap();
    assert!(modulus.reduce(&big) < Number::from_u64(prime));
    assert!(Modulus::new(&Number::from_u64(10)).is_none());
    assert!(Number::from_be_bytes(&[1; 513]).is_none());
    assert_eq!(Number::from_be_bytes(&[0, 0, 1, 0]).unwrap().bits(), 9);
}

#[test]
fn the_curves_base_points_are_on_their_curves() {
    assert!(ecdsa::base_point_is_on_curve(Curve::P256));
    assert!(ecdsa::base_point_is_on_curve(Curve::P384));
}

#[test]
fn ecdsa_checks_rfc_6979_signatures_on_p256_and_p384() {
    let p256 = hex(
        "04 60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6\
         7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299",
    );
    let sample = digest(HashAlgorithm::Sha256, b"sample");
    let r = hex("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716");
    let s = hex("f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8");
    assert!(ecdsa::verify(Curve::P256, &p256, sample.as_bytes(), &r, &s));
    let test = digest(HashAlgorithm::Sha256, b"test");
    let r2 = hex("f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d38367");
    let s2 = hex("019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083");
    assert!(ecdsa::verify(Curve::P256, &p256, test.as_bytes(), &r2, &s2));
    assert!(!ecdsa::verify(Curve::P256, &p256, test.as_bytes(), &r, &s));
    let mut bent = s.clone();
    bent[31] ^= 1;
    assert!(!ecdsa::verify(
        Curve::P256,
        &p256,
        sample.as_bytes(),
        &r,
        &bent
    ));
    let mut off_curve = p256.clone();
    off_curve[64] ^= 1;
    assert!(!ecdsa::verify(
        Curve::P256,
        &off_curve,
        sample.as_bytes(),
        &r,
        &s
    ));
    assert!(!ecdsa::verify(
        Curve::P256,
        &p256,
        sample.as_bytes(),
        &[0],
        &s
    ));

    let p384 = hex(
        "04 ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64\
         def8f0ea9055866064a254515480bc13\
         8015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1\
         288b231c3ae0d4fe7344fd2533264720",
    );
    let sample = digest(HashAlgorithm::Sha384, b"sample");
    let r = hex(
        "94edbb92a5ecb8aad4736e56c691916b3f88140666ce9fa73d64c4ea95ad133c\
         81a648152e44acf96e36dd1e80fabe46",
    );
    let s = hex(
        "99ef4aeb15f178cea1fe40db2603138f130e740a19624526203b6351d0a3a94f\
         a329c145786e679e7b82c71a38628ac8",
    );
    assert!(ecdsa::verify(Curve::P384, &p384, sample.as_bytes(), &r, &s));
    assert!(!ecdsa::verify(
        Curve::P256,
        &p384,
        sample.as_bytes(),
        &r,
        &s
    ));
}

const RSA_MODULUS: &str = "\
    d48261cddc8f6cfb7f7842d369c252b04d04f07a3abe254f57c0b175f6a164587309bc69523af554d5d2ac8868a4cc94\
    2caf9913547b6f014c140df3faa90e3280f845d1f20127c53164b3e1cc357afcd34e6a80c56226b7cabc29844b8f9190\
    738f50b19fcad80535204986b705cf03c9323d9d90aa88477bf0ebc9f46967d73cc734e90f665dddfbfc1cd577642f78\
    cb6be09f73cc803ca9c402f659d71d3718ff4cb12d9709966921169c8b9570e75d688188c7ca845e6f7a89be19293559\
    99e39efa6de66b413fd59b68f3fc8fe65549eb3fb0c1ae10dd4a1dde7abf286740c80bdcd61337da15d24a9083ec267f\
    96a516abfae814c4165947ecf4592117";

const RSA_PKCS1_SIGNATURE: &str = "\
    c18150e982f5a09d0a563a0a175b98ee7824a5be2e39a69e7fc8930f5d2748040385db78c41e52251886dd62a3ce7c1b\
    7fef437dcc09200dc36399147aaee3c246eb9d579a5fe9eb5ab16dacbd422538ec15435c37bbd38fce27015353c4914e\
    2ac4ae704a7d74f14dc04d15ca2440de7d953febe7009176745b0757553223482791fd23bb91f74b57bff46fc3652331\
    02557de90ca4d86bb85d446809d79f3d0cc86ea013615828321e265c2a3ccc1bbbe3fb1d81149170b958faa4c03dba03\
    a498f45d0c0407db65afe8a8937f25ebc969c3b2033d79ef675ef7079f02191232d6dd9bde40839101f98ef1f69b796c\
    8efa0f852de2d86e59807d8e7a70fe50";

const RSA_PSS_SIGNATURE: &str = "\
    197f21604ad70c33122791ba0a16629041ffec4a2be852e5e1003dbf4a5032d9121183693f27b4ff414e65d66eba3c77\
    4502705947109a9c3beeb7c498cd5273b11bb58abaa3f16e421c73d9520fe037401467d6bc7f2f88d01c33f9580b9fd1\
    dc84e05cd37f9b1b9612589c1f0e50694a4ecef5c5a014eac8097c52ebceab73bf9e465c11833859b5a74d90f7af3658\
    02ea017fc4dc954a38ab1883f380b659aecf608fda47953544968b295fb2630016e72f6364c029034285bfd5a2178a34\
    cdbf16d1ca3f4dc4069ca4f2aad971e1369240e3fcbdfc61a4f33359fb908f6ccc99ea141be2898042a2530aa69d74a8\
    62fbfb00f7611d58130080f3c242fa98";

#[test]
fn rsa_checks_pkcs1_and_pss_signatures_made_by_another_implementation() {
    let key = PublicKey::new(&hex(RSA_MODULUS), &[1, 0, 1]).unwrap();
    assert_eq!(key.bits(), 2048);
    let message = digest(HashAlgorithm::Sha256, b"EggOS");
    let pkcs1 = hex(RSA_PKCS1_SIGNATURE);
    let pss = hex(RSA_PSS_SIGNATURE);
    assert!(key.verify_pkcs1(HashAlgorithm::Sha256, message.as_bytes(), &pkcs1));
    assert!(key.verify_pss(HashAlgorithm::Sha256, message.as_bytes(), &pss));
    assert!(!key.verify_pkcs1(HashAlgorithm::Sha256, message.as_bytes(), &pss));
    assert!(!key.verify_pss(HashAlgorithm::Sha256, message.as_bytes(), &pkcs1));
    let other = digest(HashAlgorithm::Sha256, b"EggOS!");
    assert!(!key.verify_pkcs1(HashAlgorithm::Sha256, other.as_bytes(), &pkcs1));
    assert!(!key.verify_pss(HashAlgorithm::Sha256, other.as_bytes(), &pss));
    let mut bent = pkcs1.clone();
    bent[100] ^= 4;
    assert!(!key.verify_pkcs1(HashAlgorithm::Sha256, message.as_bytes(), &bent));
    assert!(!key.verify_pkcs1(HashAlgorithm::Sha256, message.as_bytes(), &pkcs1[1..]));
    assert_eq!(
        PublicKey::new(&[0xc3; 128], &[1, 0, 1]).unwrap_err(),
        KeyError::UnsupportedSize
    );
    assert_eq!(
        PublicKey::new(&hex(RSA_MODULUS), &[2]).unwrap_err(),
        KeyError::Malformed
    );
}

#[test]
fn equal_compares_every_byte() {
    assert!(equal(b"abc", b"abc"));
    assert!(!equal(b"abc", b"abd"));
    assert!(!equal(b"abc", b"ab"));
}

#[test]
fn p256_signs_with_rfc_6979_nonces() {
    let private_key: [u8; 32] =
        array("c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721");
    let public_key = p256::public_key(&private_key).unwrap();
    assert_eq!(
        public_key.to_vec(),
        hex(
            "04 60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6\
             7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299",
        )
    );
    let sample = digest(HashAlgorithm::Sha256, b"sample");
    let signature = p256::sign(&private_key, sample.as_bytes()).unwrap();
    assert_eq!(
        signature.to_vec(),
        hex(
            "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716\
             f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8",
        )
    );
    let test = digest(HashAlgorithm::Sha256, b"test");
    let signature = p256::sign(&private_key, test.as_bytes()).unwrap();
    assert_eq!(
        signature.to_vec(),
        hex(
            "f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d38367\
             019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083",
        )
    );
}

#[test]
fn p256_signatures_check_out_and_bad_keys_are_refused() {
    for seed in 1u8..6 {
        let private_key = digest(HashAlgorithm::Sha256, &[seed]);
        let private_key: [u8; 32] = private_key.as_bytes().try_into().unwrap();
        let public_key = p256::public_key(&private_key).unwrap();
        let message = digest(HashAlgorithm::Sha256, &[seed, seed]);
        let signature = p256::sign(&private_key, message.as_bytes()).unwrap();
        assert!(ecdsa::verify(
            Curve::P256,
            &public_key,
            message.as_bytes(),
            &signature[..32],
            &signature[32..]
        ));
    }
    assert_eq!(p256::public_key(&[0; 32]), Err(p256::InvalidKey));
    assert_eq!(p256::sign(&[0xff; 32], &[1; 32]), Err(p256::InvalidKey));
    assert!(p256::private_key_from(&[0xff; 32]).is_none());
    assert_eq!(p256::private_key_from(&[7; 32]), Some([7; 32]));
}
