//! Files pinned in a git repository that a gitiles server (`android.googlesource.com`) serves
//! (D188): gitiles hands a file at a commit out base64-encoded (`?format=TEXT`), so the
//! download is decoded here and checked against the git blob id that commit records for it
//! (SHA-1 over `blob <length>\0` and the bytes, from git's documented object format), which
//! pins the exact bytes as a SHA-256 of them would.

use std::path::Path;

const SHA1_START: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

/// The address of `path` at `commit` in gitiles `repository` (an `https://` address).
pub fn file_url(repository: &str, commit: &str, path: &str) -> String {
    format!("{repository}/+/{commit}/{path}?format=TEXT")
}

/// Decodes base64 (RFC 4648's alphabet, padded, line breaks ignored).
pub fn decode_base64(text: &[u8]) -> Result<Vec<u8>, String> {
    let value = |byte: u8| -> Result<u32, String> {
        Ok(u32::from(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("base64: byte {byte:#04x} is not in the alphabet")),
        }))
    };
    let symbols: Vec<u8> = text
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !symbols.len().is_multiple_of(4) {
        return Err("base64: the text is not whole groups of four".into());
    }
    let mut bytes = Vec::with_capacity(symbols.len() / 4 * 3);
    let groups = symbols.len() / 4;
    for (index, group) in symbols.as_chunks::<4>().0.iter().enumerate() {
        let padding = group.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != groups) {
            return Err("base64: padding before the end".into());
        }
        let mut word = 0u32;
        for byte in &group[..4 - padding] {
            word = (word << 6) | value(*byte)?;
        }
        word <<= 6 * padding;
        let decoded = [(word >> 16) as u8, (word >> 8) as u8, word as u8];
        bytes.extend_from_slice(&decoded[..3 - padding]);
    }
    Ok(bytes)
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state = SHA1_START;
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let mut words = [0u32; 80];
        for (index, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            words[index] = u32::from_be_bytes(*word);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (mixed, constant) = match index {
                0..20 => ((b & c) | (!b & d), 0x5a82_7999),
                20..40 => (b ^ c ^ d, 0x6ed9_eba1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mixed)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (value, add) in state.iter_mut().zip([a, b, c, d, e]) {
            *value = value.wrapping_add(add);
        }
    }
    let mut digest = [0u8; 20];
    for (bytes, value) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = value.to_be_bytes();
    }
    digest
}

/// The git blob id of `data`: hexadecimal SHA-1 of `blob <length>\0` followed by `data`.
pub fn blob_id(data: &[u8]) -> String {
    let mut object = format!("blob {}\0", data.len()).into_bytes();
    object.extend_from_slice(data);
    sha1(&object)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The file at `url` (a [`file_url`]) whose git blob id is `blob`, kept at `file`: read from
/// there when it is already the pinned blob, else downloaded, decoded and checked first.
pub fn fetch(url: &str, blob: &str, file: &Path) -> Result<Vec<u8>, String> {
    if let Ok(bytes) = std::fs::read(file)
        && blob_id(&bytes) == blob
    {
        return Ok(bytes);
    }
    let encoded = file.with_extension("base64");
    crate::dynamic_linking::download(url, &encoded)?;
    let text = std::fs::read(&encoded).map_err(|e| format!("{}: {e}", encoded.display()))?;
    let bytes = decode_base64(&text).map_err(|e| format!("{url}: {e}"))?;
    let found = blob_id(&bytes);
    if found != blob {
        return Err(format!(
            "{url}: git blob {found}, not the pinned {blob}: the commit is pinned, so the \
             server sent other bytes"
        ));
    }
    std::fs::write(file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_every_padding_and_ignores_line_breaks() {
        assert_eq!(decode_base64(b"").unwrap(), b"");
        assert_eq!(decode_base64(b"Zg==").unwrap(), b"f");
        assert_eq!(decode_base64(b"Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64(b"Zm9v").unwrap(), b"foo");
        assert_eq!(decode_base64(b"Zm9v\nYmFy\n").unwrap(), b"foobar");
        assert_eq!(decode_base64(b"+/+/").unwrap(), [0xfb, 0xff, 0xbf]);
        assert!(decode_base64(b"Zm9").is_err());
        assert!(decode_base64(b"Zg==Zm9v").is_err());
        assert!(decode_base64(b"Zm9*").is_err());
    }

    #[test]
    fn blob_ids_are_git_s() {
        assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            blob_id(b"hello world\n"),
            "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
        );
        assert_eq!(
            sha1(b"abc")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            sha1(two_blocks)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn the_file_address_asks_gitiles_for_base64() {
        assert_eq!(
            file_url("https://example.org/repo", "abc123", "dir/file.apex"),
            "https://example.org/repo/+/abc123/dir/file.apex?format=TEXT"
        );
    }
}
