//! The Poly1305 one-time authenticator (RFC 8439, section 2.5), in five 26-bit limbs so that
//! every product fits in 64 bits, with the final reduction done by masks.

use core::fmt;

/// The bytes of a key.
pub const KEY_BYTES: usize = 32;
/// The bytes of a tag.
pub const TAG_BYTES: usize = 16;

const LIMB: u64 = 0x3ff_ffff;

/// A running Poly1305 MAC.
#[derive(Clone)]
pub struct Poly1305 {
    r: [u64; 5],
    h: [u64; 5],
    pad: [u64; 4],
    buffer: [u8; 16],
    filled: usize,
}

impl fmt::Debug for Poly1305 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Poly1305")
    }
}

fn word(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
    ]))
}

impl Poly1305 {
    /// A MAC keyed by the one-time `key`: `r` (clamped) then `s`.
    pub fn new(key: &[u8; KEY_BYTES]) -> Self {
        Self {
            r: [
                word(key, 0) & 0x3ff_ffff,
                (word(key, 3) >> 2) & 0x3ff_ff03,
                (word(key, 6) >> 4) & 0x3ff_c0ff,
                (word(key, 9) >> 6) & 0x3f0_3fff,
                (word(key, 12) >> 8) & 0x00f_ffff,
            ],
            h: [0; 5],
            pad: [word(key, 16), word(key, 20), word(key, 24), word(key, 28)],
            buffer: [0; 16],
            filled: 0,
        }
    }

    /// MACs `data` after what came before.
    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = (16 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 16 {
                let block = self.buffer;
                self.block(&block, 1 << 24);
                self.filled = 0;
            }
        }
    }

    fn block(&mut self, block: &[u8; 16], high_bit: u64) {
        let [r0, r1, r2, r3, r4] = self.r;
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let h0 = self.h[0] + (word(block, 0) & LIMB);
        let h1 = self.h[1] + ((word(block, 3) >> 2) & LIMB);
        let h2 = self.h[2] + ((word(block, 6) >> 4) & LIMB);
        let h3 = self.h[3] + ((word(block, 9) >> 6) & LIMB);
        let h4 = self.h[4] + ((word(block, 12) >> 8) | high_bit);
        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;
        let mut carry = d0 >> 26;
        let mut h = [d0 & LIMB, 0, 0, 0, 0];
        d1 += carry;
        carry = d1 >> 26;
        h[1] = d1 & LIMB;
        d2 += carry;
        carry = d2 >> 26;
        h[2] = d2 & LIMB;
        d3 += carry;
        carry = d3 >> 26;
        h[3] = d3 & LIMB;
        d4 += carry;
        carry = d4 >> 26;
        h[4] = d4 & LIMB;
        h[0] += carry * 5;
        carry = h[0] >> 26;
        h[0] &= LIMB;
        h[1] += carry;
        self.h = h;
    }

    /// The tag over everything given.
    pub fn finish(mut self) -> [u8; TAG_BYTES] {
        if self.filled > 0 {
            let mut last = [0u8; 16];
            last[..self.filled].copy_from_slice(&self.buffer[..self.filled]);
            last[self.filled] = 1;
            self.block(&last, 0);
        }
        let mut h = self.h;
        let mut carry = h[1] >> 26;
        h[1] &= LIMB;
        for index in 2..5 {
            h[index] += carry;
            carry = h[index] >> 26;
            h[index] &= LIMB;
        }
        h[0] += carry * 5;
        carry = h[0] >> 26;
        h[0] &= LIMB;
        h[1] += carry;

        let mut g = [0u64; 5];
        g[0] = h[0] + 5;
        carry = g[0] >> 26;
        g[0] &= LIMB;
        for index in 1..4 {
            g[index] = h[index] + carry;
            carry = g[index] >> 26;
            g[index] &= LIMB;
        }
        g[4] = h[4].wrapping_add(carry).wrapping_sub(1 << 26);
        let use_g = (g[4] >> 63).wrapping_sub(1);
        for index in 0..5 {
            h[index] = (h[index] & !use_g) | (g[index] & use_g);
        }

        let words = [
            (h[0] | (h[1] << 26)) & 0xffff_ffff,
            ((h[1] >> 6) | (h[2] << 20)) & 0xffff_ffff,
            ((h[2] >> 12) | (h[3] << 14)) & 0xffff_ffff,
            ((h[3] >> 18) | (h[4] << 8)) & 0xffff_ffff,
        ];
        let mut tag = [0u8; TAG_BYTES];
        let mut sum = 0u64;
        for ((bytes, word), pad) in tag
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(words)
            .zip(self.pad)
        {
            sum = word + pad + (sum >> 32);
            *bytes = (sum as u32).to_le_bytes();
        }
        tag
    }
}

/// The Poly1305 tag of `message` under the one-time `key`.
pub fn tag(key: &[u8; KEY_BYTES], message: &[u8]) -> [u8; TAG_BYTES] {
    let mut mac = Poly1305::new(key);
    mac.update(message);
    mac.finish()
}
