//! AES-128 and AES-256 encryption (FIPS 197), bitsliced so that no table is indexed by a
//! secret: the state's 16 bytes are held as 8 planes of 16 bits (plane `i` holds bit `i` of
//! every byte, byte `column * 4 + row` at bit `column * 4 + row`), and SubBytes is Boyar and
//! Peralta's circuit for the S-box, computed on all 16 bytes at once with logic operations.
//! Only encryption is here: GCM and counter mode need nothing else.

use core::fmt;

/// The bytes of a block.
pub const BLOCK_BYTES: usize = 16;

type Planes = [u16; 8];

/// An expanded AES key.
#[derive(Clone)]
pub struct Aes {
    round_keys: [Planes; 15],
    rounds: usize,
}

impl fmt::Debug for Aes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Aes({} rounds)", self.rounds)
    }
}

/// A key that is neither 16 nor 32 bytes long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadKeyLength;

const ROUND_CONSTANTS: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

impl Aes {
    /// The cipher for `key`: 16 bytes for AES-128, 32 for AES-256.
    pub fn new(key: &[u8]) -> Result<Self, BadKeyLength> {
        let (key_words, rounds) = match key.len() {
            16 => (4, 10),
            32 => (8, 14),
            _ => return Err(BadKeyLength),
        };
        let mut words = [[0u8; 4]; 60];
        for (word, bytes) in words.iter_mut().zip(key.as_chunks::<4>().0) {
            *word = *bytes;
        }
        for index in key_words..4 * (rounds + 1) {
            let mut temporary = words[index - 1];
            if index.is_multiple_of(key_words) {
                temporary.rotate_left(1);
                temporary = substitute_word(temporary);
                temporary[0] ^= ROUND_CONSTANTS[index / key_words - 1];
            } else if key_words > 6 && index % key_words == 4 {
                temporary = substitute_word(temporary);
            }
            let earlier = words[index - key_words];
            for (byte, (&a, &b)) in words[index].iter_mut().zip(earlier.iter().zip(&temporary)) {
                *byte = a ^ b;
            }
        }
        let mut round_keys = [[0u16; 8]; 15];
        for (round, planes) in round_keys.iter_mut().enumerate().take(rounds + 1) {
            let mut block = [0u8; BLOCK_BYTES];
            for (bytes, word) in block
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(&words[4 * round..4 * round + 4])
            {
                *bytes = *word;
            }
            *planes = bitslice(&block);
        }
        Ok(Self { round_keys, rounds })
    }

    /// Encrypts one block in place.
    pub fn encrypt_block(&self, block: &mut [u8; BLOCK_BYTES]) {
        let mut state = bitslice(block);
        add_round_key(&mut state, &self.round_keys[0]);
        for round in 1..self.rounds {
            state = substitute(state);
            shift_rows(&mut state);
            mix_columns(&mut state);
            add_round_key(&mut state, &self.round_keys[round]);
        }
        state = substitute(state);
        shift_rows(&mut state);
        add_round_key(&mut state, &self.round_keys[self.rounds]);
        *block = unbitslice(&state);
    }
}

fn bitslice(block: &[u8; BLOCK_BYTES]) -> Planes {
    let mut planes = [0u16; 8];
    for (position, &byte) in block.iter().enumerate() {
        for (bit, plane) in planes.iter_mut().enumerate() {
            *plane |= u16::from((byte >> bit) & 1) << position;
        }
    }
    planes
}

fn unbitslice(planes: &Planes) -> [u8; BLOCK_BYTES] {
    let mut block = [0u8; BLOCK_BYTES];
    for (position, byte) in block.iter_mut().enumerate() {
        for (bit, plane) in planes.iter().enumerate() {
            *byte |= (((plane >> position) & 1) as u8) << bit;
        }
    }
    block
}

fn add_round_key(state: &mut Planes, key: &Planes) {
    for (plane, key) in state.iter_mut().zip(key) {
        *plane ^= key;
    }
}

fn substitute_word(word: [u8; 4]) -> [u8; 4] {
    let mut block = [0u8; BLOCK_BYTES];
    block[..4].copy_from_slice(&word);
    let substituted = unbitslice(&substitute(bitslice(&block)));
    [
        substituted[0],
        substituted[1],
        substituted[2],
        substituted[3],
    ]
}

/// The S-box on every byte at once: Boyar and Peralta's circuit ("A depth-16 circuit for the
/// AES S-box", 2011), with `u0` the most significant bit and `s0` the result's.
pub(crate) fn substitute(planes: Planes) -> Planes {
    let [u7, u6, u5, u4, u3, u2, u1, u0] = planes;
    let t1 = u0 ^ u3;
    let t2 = u0 ^ u5;
    let t3 = u0 ^ u6;
    let t4 = u3 ^ u5;
    let t5 = u4 ^ u6;
    let t6 = t1 ^ t5;
    let t7 = u1 ^ u2;
    let t8 = u7 ^ t6;
    let t9 = u7 ^ t7;
    let t10 = t6 ^ t7;
    let t11 = u1 ^ u5;
    let t12 = u2 ^ u5;
    let t13 = t3 ^ t4;
    let t14 = t6 ^ t11;
    let t15 = t5 ^ t11;
    let t16 = t5 ^ t12;
    let t17 = t9 ^ t16;
    let t18 = u3 ^ u7;
    let t19 = t7 ^ t18;
    let t20 = t1 ^ t19;
    let t21 = u6 ^ u7;
    let t22 = t7 ^ t21;
    let t23 = t2 ^ t22;
    let t24 = t2 ^ t10;
    let t25 = t20 ^ t17;
    let t26 = t3 ^ t16;
    let t27 = t1 ^ t12;
    let d = u7;

    let m1 = t13 & t6;
    let m2 = t23 & t8;
    let m3 = t14 ^ m1;
    let m4 = t19 & d;
    let m5 = m4 ^ m1;
    let m6 = t3 & t16;
    let m7 = t22 & t9;
    let m8 = t26 ^ m6;
    let m9 = t20 & t17;
    let m10 = m9 ^ m6;
    let m11 = t1 & t15;
    let m12 = t4 & t27;
    let m13 = m12 ^ m11;
    let m14 = t2 & t10;
    let m15 = m14 ^ m11;
    let m16 = m3 ^ m2;
    let m17 = m5 ^ t24;
    let m18 = m8 ^ m7;
    let m19 = m10 ^ m15;
    let m20 = m16 ^ m13;
    let m21 = m17 ^ m15;
    let m22 = m18 ^ m13;
    let m23 = m19 ^ t25;
    let m24 = m22 ^ m23;
    let m25 = m22 & m20;
    let m26 = m21 ^ m25;
    let m27 = m20 ^ m21;
    let m28 = m23 ^ m25;
    let m29 = m28 & m27;
    let m30 = m26 & m24;
    let m31 = m20 & m23;
    let m32 = m27 ^ m31;
    let m33 = m27 ^ m25;
    let m34 = m21 & m22;
    let m35 = m24 ^ m34;
    let m36 = m24 ^ m25;
    let m37 = m21 ^ m29;
    let m38 = m32 & m33;
    let m39 = m23 ^ m30;
    let m40 = m35 & m36;
    let m41 = m38 ^ m40;
    let m42 = m37 ^ m39;
    let m43 = m37 ^ m38;
    let m44 = m39 ^ m40;
    let m45 = m42 ^ m41;
    let m46 = m44 & t6;
    let m47 = m40 & t8;
    let m48 = m39 & d;
    let m49 = m43 & t16;
    let m50 = m38 & t9;
    let m51 = m37 & t17;
    let m52 = m42 & t15;
    let m53 = m45 & t27;
    let m54 = m41 & t10;
    let m55 = m44 & t13;
    let m56 = m40 & t23;
    let m57 = m39 & t19;
    let m58 = m43 & t3;
    let m59 = m38 & t22;
    let m60 = m37 & t20;
    let m61 = m42 & t1;
    let m62 = m45 & t4;
    let m63 = m41 & t2;

    let l0 = m61 ^ m62;
    let l1 = m50 ^ m56;
    let l2 = m46 ^ m48;
    let l3 = m47 ^ m55;
    let l4 = m54 ^ m58;
    let l5 = m49 ^ m61;
    let l6 = m62 ^ l5;
    let l7 = m46 ^ l3;
    let l8 = m51 ^ m59;
    let l9 = m52 ^ m53;
    let l10 = m53 ^ l4;
    let l11 = m60 ^ l2;
    let l12 = m48 ^ m51;
    let l13 = m50 ^ l0;
    let l14 = m52 ^ m61;
    let l15 = m55 ^ l1;
    let l16 = m56 ^ l0;
    let l17 = m57 ^ l1;
    let l18 = m58 ^ l8;
    let l19 = m63 ^ l4;
    let l20 = l0 ^ l1;
    let l21 = l1 ^ l7;
    let l22 = l3 ^ l12;
    let l23 = l18 ^ l2;
    let l24 = l15 ^ l9;
    let l25 = l6 ^ l10;
    let l26 = l7 ^ l9;
    let l27 = l8 ^ l10;
    let l28 = l11 ^ l14;
    let l29 = l11 ^ l17;

    let s0 = l6 ^ l24;
    let s1 = !(l16 ^ l26);
    let s2 = !(l19 ^ l28);
    let s3 = l6 ^ l21;
    let s4 = l20 ^ l22;
    let s5 = l25 ^ l29;
    let s6 = !(l13 ^ l27);
    let s7 = !(l6 ^ l23);
    [s7, s6, s5, s4, s3, s2, s1, s0]
}

fn shift_rows(state: &mut Planes) {
    for plane in state.iter_mut() {
        let p = *plane;
        *plane = (p & 0x1111)
            | (p & 0x2222).rotate_right(4)
            | (p & 0x4444).rotate_right(8)
            | (p & 0x8888).rotate_right(12);
    }
}

fn next_row(plane: u16) -> u16 {
    ((plane >> 1) & 0x7777) | ((plane << 3) & 0x8888)
}

fn row_after_next(plane: u16) -> u16 {
    ((plane >> 2) & 0x3333) | ((plane << 2) & 0xcccc)
}

fn previous_row(plane: u16) -> u16 {
    ((plane >> 3) & 0x1111) | ((plane << 1) & 0xeeee)
}

fn mix_columns(state: &mut Planes) {
    let a = *state;
    let mut b = [0u16; 8];
    let mut t = [0u16; 8];
    for bit in 0..8 {
        b[bit] = next_row(a[bit]);
        t[bit] = a[bit] ^ b[bit];
    }
    let doubled = [
        t[7],
        t[0] ^ t[7],
        t[1],
        t[2] ^ t[7],
        t[3] ^ t[7],
        t[4],
        t[5],
        t[6],
    ];
    for bit in 0..8 {
        state[bit] = doubled[bit] ^ b[bit] ^ row_after_next(a[bit]) ^ previous_row(a[bit]);
    }
}

#[cfg(test)]
pub(crate) fn substitute_byte(byte: u8) -> u8 {
    let mut block = [0u8; BLOCK_BYTES];
    block[0] = byte;
    unbitslice(&substitute(bitslice(&block)))[0]
}
