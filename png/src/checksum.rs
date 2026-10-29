const CRC_POLYNOMIAL: u32 = 0xedb8_8320;
const ADLER_MODULUS: u32 = 65521;
const ADLER_RUN: u32 = 5552;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                CRC_POLYNOMIAL ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = crc_table();

/// The CRC-32 (ISO 3309, as PNG chunks carry it) of `crc`'s data followed by `data`; `crc` is
/// 0 for no data before.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    !data.iter().fold(!crc, |crc, &byte| {
        CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// A running Adler-32 checksum (RFC 1950), as a zlib stream ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adler32 {
    a: u32,
    b: u32,
    run: u32,
}

impl Adler32 {
    /// The checksum of no data.
    pub const fn new() -> Self {
        Self { a: 1, b: 0, run: 0 }
    }

    /// Adds `data`.
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.a += u32::from(byte);
            self.b += self.a;
            self.run += 1;
            if self.run == ADLER_RUN {
                self.a %= ADLER_MODULUS;
                self.b %= ADLER_MODULUS;
                self.run = 0;
            }
        }
    }

    /// The checksum of the data added so far.
    pub fn value(&self) -> u32 {
        (self.b % ADLER_MODULUS) << 16 | (self.a % ADLER_MODULUS)
    }
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}
