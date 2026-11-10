//! AES-CCM (RFC 3610) with 8-byte tags and 13-byte nonces (`L` = 2), as CCMP, WPA2's data
//! confidentiality protocol, uses it (IEEE 802.11-2020, 12.5.3; D246).

use crate::AuthenticationFailed;
use crate::aes::{Aes, BLOCK_BYTES};
use crate::constant_time::equal;

pub const NONCE_BYTES: usize = 13;
pub const TAG_BYTES: usize = 8;
const LENGTH_BYTES: usize = 2;

fn xor_into(block: &mut [u8; BLOCK_BYTES], bytes: &[u8]) {
    for (byte, other) in block.iter_mut().zip(bytes) {
        *byte ^= other;
    }
}

fn counter_block(nonce: &[u8; NONCE_BYTES], counter: u16) -> [u8; BLOCK_BYTES] {
    let mut block = [0u8; BLOCK_BYTES];
    block[0] = (LENGTH_BYTES - 1) as u8;
    block[1..14].copy_from_slice(nonce);
    block[14..].copy_from_slice(&counter.to_be_bytes());
    block
}

fn authenticate(cipher: &Aes, nonce: &[u8; NONCE_BYTES], aad: &[u8], data: &[u8]) -> [u8; BLOCK_BYTES] {
    let mut mac = [0u8; BLOCK_BYTES];
    let adata = if aad.is_empty() { 0 } else { 0x40 };
    mac[0] = adata | ((((TAG_BYTES - 2) / 2) as u8) << 3) | (LENGTH_BYTES - 1) as u8;
    mac[1..14].copy_from_slice(nonce);
    mac[14..].copy_from_slice(&(data.len() as u16).to_be_bytes());
    cipher.encrypt_block(&mut mac);
    if !aad.is_empty() {
        let mut header = [0u8; BLOCK_BYTES];
        header[..2].copy_from_slice(&(aad.len() as u16).to_be_bytes());
        let first = aad.len().min(BLOCK_BYTES - 2);
        header[2..2 + first].copy_from_slice(&aad[..first]);
        xor_into(&mut mac, &header);
        cipher.encrypt_block(&mut mac);
        for chunk in aad[first..].chunks(BLOCK_BYTES) {
            xor_into(&mut mac, chunk);
            cipher.encrypt_block(&mut mac);
        }
    }
    for chunk in data.chunks(BLOCK_BYTES) {
        xor_into(&mut mac, chunk);
        cipher.encrypt_block(&mut mac);
    }
    mac
}

fn apply_keystream(cipher: &Aes, nonce: &[u8; NONCE_BYTES], data: &mut [u8]) {
    for (index, chunk) in data.chunks_mut(BLOCK_BYTES).enumerate() {
        let mut stream = counter_block(nonce, index as u16 + 1);
        cipher.encrypt_block(&mut stream);
        for (byte, key) in chunk.iter_mut().zip(stream) {
            *byte ^= key;
        }
    }
}

fn tag_of(cipher: &Aes, nonce: &[u8; NONCE_BYTES], mac: &[u8; BLOCK_BYTES]) -> [u8; TAG_BYTES] {
    let mut first = counter_block(nonce, 0);
    cipher.encrypt_block(&mut first);
    core::array::from_fn(|index| mac[index] ^ first[index])
}

/// Encrypts `data` in place and returns its tag over `aad` and `data`. `data` is at most 64 KiB.
pub fn seal(cipher: &Aes, nonce: &[u8; NONCE_BYTES], aad: &[u8], data: &mut [u8]) -> [u8; TAG_BYTES] {
    let mac = authenticate(cipher, nonce, aad, data);
    apply_keystream(cipher, nonce, data);
    tag_of(cipher, nonce, &mac)
}

/// Decrypts `data` in place when `tag` matches; on a mismatch `data` is zeroed.
pub fn open(
    cipher: &Aes,
    nonce: &[u8; NONCE_BYTES],
    aad: &[u8],
    data: &mut [u8],
    tag: &[u8; TAG_BYTES],
) -> Result<(), AuthenticationFailed> {
    apply_keystream(cipher, nonce, data);
    let mac = authenticate(cipher, nonce, aad, data);
    if equal(&tag_of(cipher, nonce, &mac), tag) {
        Ok(())
    } else {
        data.fill(0);
        Err(AuthenticationFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_3610_packet_vector_1() {
        let key: [u8; 16] = core::array::from_fn(|index| 0xc0 + index as u8);
        let cipher = Aes::new(&key).unwrap();
        let nonce = [0x00, 0x00, 0x00, 0x03, 0x02, 0x01, 0x00, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5];
        let aad: [u8; 8] = core::array::from_fn(|index| index as u8);
        let mut data: [u8; 23] = core::array::from_fn(|index| 8 + index as u8);
        let plain = data;
        let tag = seal(&cipher, &nonce, &aad, &mut data);
        assert_eq!(
            data,
            [
                0x58, 0x8c, 0x97, 0x9a, 0x61, 0xc6, 0x63, 0xd2, 0xf0, 0x66, 0xd0, 0xc2, 0xc0, 0xf9,
                0x89, 0x80, 0x6d, 0x5f, 0x6b, 0x61, 0xda, 0xc3, 0x84
            ]
        );
        assert_eq!(tag, [0x17, 0xe8, 0xd1, 0x2c, 0xfd, 0xf9, 0x26, 0xe0]);
        open(&cipher, &nonce, &aad, &mut data, &tag).unwrap();
        assert_eq!(data, plain);
        let mut changed = data;
        let tag = seal(&cipher, &nonce, &aad, &mut changed);
        changed[0] ^= 1;
        assert!(open(&cipher, &nonce, &aad, &mut changed, &tag).is_err());
    }
}
