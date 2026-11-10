//! AES key wrap (RFC 3394), which WPA2's EAPOL-Key frames carry the group key in (D246).

use crate::aes::Aes;

const DEFAULT_IV: [u8; 8] = [0xa6; 8];

/// Why unwrapping failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwrapError {
    /// Not a whole number of 8-byte blocks, or fewer than two of key.
    BadLength,
    /// The integrity check failed: the wrong key, or a changed message.
    Corrupted,
}

/// Unwraps `wrapped` (8 bytes longer than the key) with `kek` into `out`.
pub fn unwrap(kek: &Aes, wrapped: &[u8], out: &mut [u8]) -> Result<(), UnwrapError> {
    let blocks = wrapped.len() / 8;
    if !wrapped.len().is_multiple_of(8) || blocks < 3 || out.len() != wrapped.len() - 8 {
        return Err(UnwrapError::BadLength);
    }
    let count = blocks - 1;
    let mut check: [u8; 8] = wrapped[..8].try_into().map_err(|_| UnwrapError::BadLength)?;
    out.copy_from_slice(&wrapped[8..]);
    for round in (0..6).rev() {
        for index in (0..count).rev() {
            let step = (count * round + index + 1) as u64;
            let mut block = [0u8; 16];
            let mixed = u64::from_be_bytes(check) ^ step;
            block[..8].copy_from_slice(&mixed.to_be_bytes());
            block[8..].copy_from_slice(&out[index * 8..index * 8 + 8]);
            kek.decrypt_block(&mut block);
            check.copy_from_slice(&block[..8]);
            out[index * 8..index * 8 + 8].copy_from_slice(&block[8..]);
        }
    }
    if check != DEFAULT_IV {
        out.fill(0);
        return Err(UnwrapError::Corrupted);
    }
    Ok(())
}

/// Wraps `key` (a whole number of 8-byte blocks, at least two) with `kek` into `out`, 8 bytes
/// longer.
pub fn wrap(kek: &Aes, key: &[u8], out: &mut [u8]) -> Result<(), UnwrapError> {
    let count = key.len() / 8;
    if !key.len().is_multiple_of(8) || count < 2 || out.len() != key.len() + 8 {
        return Err(UnwrapError::BadLength);
    }
    let mut check = DEFAULT_IV;
    out[8..].copy_from_slice(key);
    for round in 0..6 {
        for index in 0..count {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&check);
            block[8..].copy_from_slice(&out[8 + index * 8..16 + index * 8]);
            kek.encrypt_block(&mut block);
            let step = (count * round + index + 1) as u64;
            check = (u64::from_be_bytes(block[..8].try_into().unwrap_or_default()) ^ step).to_be_bytes();
            out[8 + index * 8..16 + index * 8].copy_from_slice(&block[8..]);
        }
    }
    out[..8].copy_from_slice(&check);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap()).collect()
    }

    #[test]
    fn rfc_3394_section_4_1_wraps_and_unwraps() {
        let kek = Aes::new(&bytes("000102030405060708090A0B0C0D0E0F")).unwrap();
        let key = bytes("00112233445566778899AABBCCDDEEFF");
        let mut wrapped = [0u8; 24];
        wrap(&kek, &key, &mut wrapped).unwrap();
        assert_eq!(wrapped.to_vec(), bytes("1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5"));
        let mut out = [0u8; 16];
        unwrap(&kek, &wrapped, &mut out).unwrap();
        assert_eq!(out.to_vec(), key);
        wrapped[3] ^= 1;
        assert_eq!(unwrap(&kek, &wrapped, &mut out), Err(UnwrapError::Corrupted));
    }
}
