//! AES-CMAC (NIST SP 800-38B, RFC 4493), the MIC of WPA3-Personal's EAPOL-Key frames and of
//! protected group management frames (D285). Constant time: no branch or index depends on the
//! key or the message's bytes.

use crate::aes::{Aes, BLOCK_BYTES};

const RB: u8 = 0x87;

fn doubled(block: &[u8; BLOCK_BYTES]) -> [u8; BLOCK_BYTES] {
    let mut out = [0u8; BLOCK_BYTES];
    let carry = block[0] >> 7;
    for index in 0..BLOCK_BYTES {
        let next = if index + 1 < BLOCK_BYTES { block[index + 1] >> 7 } else { 0 };
        out[index] = (block[index] << 1) | next;
    }
    out[BLOCK_BYTES - 1] ^= RB & 0u8.wrapping_sub(carry);
    out
}

/// The 16-byte CMAC of the concatenation of `parts` under `cipher`.
pub fn cmac(cipher: &Aes, parts: &[&[u8]]) -> [u8; BLOCK_BYTES] {
    let mut zero = [0u8; BLOCK_BYTES];
    cipher.encrypt_block(&mut zero);
    let first_subkey = doubled(&zero);
    let second_subkey = doubled(&first_subkey);
    let total: usize = parts.iter().map(|part| part.len()).sum();
    let mut state = [0u8; BLOCK_BYTES];
    let mut pending = [0u8; BLOCK_BYTES];
    let mut filled = 0usize;
    let mut seen = 0usize;
    for part in parts {
        for &byte in part.iter() {
            if filled == BLOCK_BYTES {
                for index in 0..BLOCK_BYTES {
                    state[index] ^= pending[index];
                }
                cipher.encrypt_block(&mut state);
                filled = 0;
            }
            pending[filled] = byte;
            filled += 1;
            seen += 1;
        }
    }
    debug_assert_eq!(seen, total);
    let complete = total != 0 && filled == BLOCK_BYTES;
    if complete {
        for index in 0..BLOCK_BYTES {
            state[index] ^= pending[index] ^ first_subkey[index];
        }
    } else {
        pending[filled] = 0x80;
        for byte in pending.iter_mut().skip(filled + 1) {
            *byte = 0;
        }
        for index in 0..BLOCK_BYTES {
            state[index] ^= pending[index] ^ second_subkey[index];
        }
    }
    cipher.encrypt_block(&mut state);
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
    }

    #[test]
    fn rfc_4493_examples() {
        let cipher = Aes::new(&hex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap();
        let message = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710");
        assert_eq!(cmac(&cipher, &[]).to_vec(), hex("bb1d6929e95937287fa37d129b756746"));
        assert_eq!(cmac(&cipher, &[&message[..16]]).to_vec(), hex("070a16b46b4d4144f79bdd9dd04a287c"));
        assert_eq!(cmac(&cipher, &[&message[..20], &message[20..40]]).to_vec(), hex("dfa66747de9ae63030ca32611497c827"));
        assert_eq!(cmac(&cipher, &[&message]).to_vec(), hex("51f0bebf7e3b9d92fc49741779363cfe"));
    }
}
