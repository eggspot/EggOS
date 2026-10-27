const POLYNOMIAL: u32 = 0x82f6_3b78;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ POLYNOMIAL
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// A running CRC-32C (Castagnoli) checksum, as the on-disk format uses it.
#[derive(Debug, Clone, Copy)]
pub struct Crc32c(u32);

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32c {
    /// A checksum over no bytes yet.
    pub const fn new() -> Self {
        Self(!0)
    }

    /// Adds `bytes` to the checksum.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut value = self.0;
        for &byte in bytes {
            value = TABLE[((value ^ u32::from(byte)) & 0xff) as usize] ^ (value >> 8);
        }
        self.0 = value;
    }

    /// The checksum of everything added.
    pub fn finish(self) -> u32 {
        !self.0
    }
}

/// The CRC-32C of `bytes`.
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(bytes);
    crc.finish()
}

const CRC64_POLYNOMIAL: u64 = 0xc96c_5795_d787_0f42;

const CRC64_TABLE: [u64; 256] = {
    let mut table = [0u64; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u64;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ CRC64_POLYNOMIAL
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// A running CRC-64 (the XZ polynomial), the transaction checksum: a different polynomial from
/// the CRC-32C that blocks carry inside, so a change to a block that keeps its own checksum
/// right still changes this one.
#[derive(Debug, Clone, Copy)]
pub struct Crc64(u64);

impl Default for Crc64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc64 {
    /// A checksum over no bytes yet.
    pub const fn new() -> Self {
        Self(!0)
    }

    /// Adds `bytes` to the checksum.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut value = self.0;
        for &byte in bytes {
            value = CRC64_TABLE[((value ^ u64::from(byte)) & 0xff) as usize] ^ (value >> 8);
        }
        self.0 = value;
    }

    /// The checksum of everything added.
    pub fn finish(self) -> u64 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_answers_match_the_castagnoli_check_values() {
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc32c(&[0u8; 32]), 0x8a91_36aa);
        assert_eq!(crc32c(b""), 0);
        let mut split = Crc32c::new();
        split.update(b"1234");
        split.update(b"56789");
        assert_eq!(split.finish(), 0xe306_9283);
        let mut crc = Crc64::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0x995d_c9bb_df19_39fa);
    }

    #[test]
    fn a_change_that_keeps_an_inner_crc32c_right_still_changes_the_crc64() {
        let block = |value: u8| {
            let mut bytes = [0u8; 64];
            bytes[3] = value;
            let inner = crc32c(&bytes[..60]);
            bytes[60..].copy_from_slice(&inner.to_le_bytes());
            bytes
        };
        assert_eq!(crc32c(&block(1)), crc32c(&block(2)));
        let mut one = Crc64::new();
        one.update(&block(1));
        let mut two = Crc64::new();
        two.update(&block(2));
        assert_ne!(one.finish(), two.finish());
    }
}
