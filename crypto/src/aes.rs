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
    let y14 = u3 ^ u5;
    let y13 = u0 ^ u6;
    let y9 = u0 ^ u3;
    let y8 = u0 ^ u5;
    let t0 = u1 ^ u2;
    let y1 = t0 ^ u7;
    let y4 = y1 ^ u3;
    let y12 = y13 ^ y14;
    let y2 = y1 ^ u0;
    let y5 = y1 ^ u6;
    let y3 = y5 ^ y8;
    let t1 = u4 ^ y12;
    let y15 = t1 ^ u5;
    let y20 = t1 ^ u1;
    let y6 = y15 ^ u7;
    let y10 = y15 ^ t0;
    let y11 = y20 ^ y9;
    let y7 = u7 ^ y11;
    let y17 = y10 ^ y11;
    let y19 = y10 ^ y8;
    let y16 = t0 ^ y11;
    let y21 = y13 ^ y16;
    let y18 = u0 ^ y16;

    let t2 = y12 & y15;
    let t3 = y3 & y6;
    let t4 = t3 ^ t2;
    let t5 = y4 & u7;
    let t6 = t5 ^ t2;
    let t7 = y13 & y16;
    let t8 = y5 & y1;
    let t9 = t8 ^ t7;
    let t10 = y2 & y7;
    let t11 = t10 ^ t7;
    let t12 = y9 & y11;
    let t13 = y14 & y17;
    let t14 = t13 ^ t12;
    let t15 = y8 & y10;
    let t16 = t15 ^ t12;
    let t17 = t4 ^ t14;
    let t18 = t6 ^ t16;
    let t19 = t9 ^ t14;
    let t20 = t11 ^ t16;
    let t21 = t17 ^ y20;
    let t22 = t18 ^ y19;
    let t23 = t19 ^ y21;
    let t24 = t20 ^ y18;

    let t25 = t21 ^ t22;
    let t26 = t21 & t23;
    let t27 = t24 ^ t26;
    let t28 = t25 & t27;
    let t29 = t28 ^ t22;
    let t30 = t23 ^ t24;
    let t31 = t22 ^ t26;
    let t32 = t31 & t30;
    let t33 = t32 ^ t24;
    let t34 = t23 ^ t33;
    let t35 = t27 ^ t33;
    let t36 = t24 & t35;
    let t37 = t36 ^ t34;
    let t38 = t27 ^ t36;
    let t39 = t29 & t38;
    let t40 = t25 ^ t39;

    let t41 = t40 ^ t37;
    let t42 = t29 ^ t33;
    let t43 = t29 ^ t40;
    let t44 = t33 ^ t37;
    let t45 = t42 ^ t41;
    let z0 = t44 & y15;
    let z1 = t37 & y6;
    let z2 = t33 & u7;
    let z3 = t43 & y16;
    let z4 = t40 & y1;
    let z5 = t29 & y7;
    let z6 = t42 & y11;
    let z7 = t45 & y17;
    let z8 = t41 & y10;
    let z9 = t44 & y12;
    let z10 = t37 & y3;
    let z11 = t33 & y4;
    let z12 = t43 & y13;
    let z13 = t40 & y5;
    let z14 = t29 & y2;
    let z15 = t42 & y9;
    let z16 = t45 & y14;
    let z17 = t41 & y8;

    let t46 = z15 ^ z16;
    let t47 = z10 ^ z11;
    let t48 = z5 ^ z13;
    let t49 = z9 ^ z10;
    let t50 = z2 ^ z12;
    let t51 = z2 ^ z5;
    let t52 = z7 ^ z8;
    let t53 = z0 ^ z3;
    let t54 = z6 ^ z7;
    let t55 = z16 ^ z17;
    let t56 = z12 ^ t48;
    let t57 = t50 ^ t53;
    let t58 = z4 ^ t46;
    let t59 = z3 ^ t54;
    let t60 = t46 ^ t57;
    let t61 = z14 ^ t57;
    let t62 = t52 ^ t58;
    let t63 = t49 ^ t58;
    let t64 = z4 ^ t59;
    let t65 = t61 ^ t62;
    let t66 = z1 ^ t63;
    let t67 = t64 ^ t65;

    let s0 = t59 ^ t63;
    let s6 = t56 ^ !t62;
    let s7 = t48 ^ !t60;
    let s3 = t53 ^ t66;
    let s4 = t51 ^ t66;
    let s5 = t47 ^ t65;
    let s1 = t64 ^ !s3;
    let s2 = t55 ^ !t67;
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
