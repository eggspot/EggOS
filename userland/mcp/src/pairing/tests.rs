use super::*;

fn agent(id: &str) -> AgentId {
    AgentId::new(id.as_bytes()).unwrap()
}

const NOW: u64 = 1_793_000_000;

fn pending() -> (PendingPairing, Code, Token) {
    let code = typed_code(b"abcd-ef23").unwrap();
    let token = token_from(&[7; TOKEN_RANDOM_BYTES]);
    (
        PendingPairing::new(agent("phone"), &code, token, NOW),
        code,
        token,
    )
}

#[test]
fn codes_use_only_the_unmistakable_characters_and_read_back_as_typed() {
    let code = code_from(&[
        0, 1, 2, 31, 3, 4, 5, 6, 7, 8, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0,
    ])
    .unwrap();
    assert_eq!(&code, b"ABCDEFGH");
    assert!(code_from(&[31; 32]).is_none());
    for byte in 0u8..=255 {
        let code = code_from(&[byte; 32]);
        assert_eq!(code.is_some(), byte & 31 != 31);
        if let Some(code) = code {
            assert!(code.iter().all(|symbol| CODE_ALPHABET.contains(symbol)));
        }
    }
    assert!(!CODE_ALPHABET.iter().any(|symbol| b"01OIL".contains(symbol)));
    assert_eq!(&code_text(b"ABCDEFGH"), b"ABCD-EFGH");
    assert_eq!(typed_code(b" abcd efgh "), Some(*b"ABCDEFGH"));
    assert_eq!(typed_code(b"ABCD-EFGH"), Some(*b"ABCDEFGH"));
    assert_eq!(typed_code(b"ABCD-EFG0"), None);
    assert_eq!(typed_code(b"ABCD-EFG"), None);
    assert_eq!(typed_code(b"ABCD-EFGHJ"), None);
}

#[test]
fn tokens_are_the_prefix_and_43_base64url_characters_and_only_their_hash_is_kept() {
    let token = token_from(&[0xff; TOKEN_RANDOM_BYTES]);
    let text = core::str::from_utf8(&token).unwrap();
    assert!(text.starts_with("eggos_"));
    assert_eq!(&text[6..], "_".repeat(42) + "8");
    let other = token_from(&[0; TOKEN_RANDOM_BYTES]);
    assert_eq!(&other[6..], "A".repeat(43).as_bytes());
    assert_ne!(token_hash(&token), token_hash(&other));
    assert_eq!(token_hash(&token), token_hash(&token));
}

#[test]
fn the_right_code_pairs_once_and_wrong_codes_burn_it_after_five() {
    let (mut waiting, code, token) = pending();
    assert_eq!(
        waiting.attempt(&code_text(&code), NOW + 10),
        Attempt::Paired(agent("phone"), token)
    );
    let (mut waiting, _, _) = pending();
    for left in (1..MAX_WRONG_CODES).rev() {
        assert_eq!(waiting.attempt(b"ZZZZ-ZZZZ", NOW + 1), Attempt::Wrong(left));
    }
    assert_eq!(waiting.attempt(b"not a code", NOW + 1), Attempt::Burned);
    assert_eq!(waiting.attempt(b"ABCD-EF23", NOW + 2), Attempt::Closed);
    assert!(!waiting.is_open(NOW + 2));
    assert_eq!(waiting.wait_before_next(NOW + 1), BURNED_WAIT_S);
    assert_eq!(waiting.wait_before_next(NOW + 1 + BURNED_WAIT_S), 0);
}

#[test]
fn a_code_expires_after_five_minutes() {
    let (mut waiting, code, _) = pending();
    assert_eq!(waiting.expires(), NOW + CODE_LIFETIME_S);
    assert!(waiting.is_open(NOW + CODE_LIFETIME_S - 1));
    assert_eq!(
        waiting.attempt(&code, NOW + CODE_LIFETIME_S),
        Attempt::Closed
    );
    assert_eq!(waiting.wait_before_next(NOW + CODE_LIFETIME_S), 0);
}

#[test]
fn the_pairing_file_reads_back_and_refuses_any_change() {
    let (mut waiting, _, _) = pending();
    waiting.attempt(b"ZZZZ-ZZZZ", NOW + 1);
    let mut out = [0u8; PAIRING_BYTES];
    let length = waiting.encode(&mut out).unwrap();
    let file = &out[..length];
    assert_eq!(PendingPairing::decode(file), Ok(waiting));
    let text = core::str::from_utf8(file).unwrap();
    assert!(text.starts_with("eggos-agent-pairing 1\nphone\t"));
    assert!(text.contains("\teggos_"));
    assert!(text.contains(&format!("\t{}\t1\t-\n", NOW + CODE_LIFETIME_S)));
    let mut changed = file.to_vec();
    let at = text.find("\t1\t-").unwrap() + 1;
    changed[at] = b'0';
    assert_eq!(
        PendingPairing::decode(&changed),
        Err(PairingError::Checksum)
    );
    assert_eq!(
        PendingPairing::decode(&file[..length - 1]),
        Err(PairingError::Checksum)
    );
    assert_eq!(waiting.encode(&mut out[..40]), Err(PairingError::Overflow));
    for _ in 0..MAX_WRONG_CODES {
        waiting.attempt(b"ZZZZ-ZZZZ", NOW + 3);
    }
    let length = waiting.encode(&mut out).unwrap();
    let burned = PendingPairing::decode(&out[..length]).unwrap();
    assert!(!burned.is_open(NOW + 4));
    assert_eq!(burned.wait_before_next(NOW + 4), BURNED_WAIT_S - 1);
}
