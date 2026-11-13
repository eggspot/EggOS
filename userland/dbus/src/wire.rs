//! The D-Bus wire format: values aligned to their size from the start of the message, in the
//! byte order the message's first byte names, and type signatures.

use alloc::vec::Vec;

/// The most bytes an array may hold.
pub const MAX_ARRAY_BYTES: usize = 1 << 26;
/// The most bytes a signature may hold.
pub const MAX_SIGNATURE_BYTES: usize = 255;
/// The deepest nesting of arrays, structs and variants.
pub const MAX_DEPTH: usize = 64;

/// The byte order of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

impl Endian {
    /// The byte order a message's first byte names.
    pub fn from_flag(flag: u8) -> Option<Self> {
        match flag {
            b'l' => Some(Self::Little),
            b'B' => Some(Self::Big),
            _ => None,
        }
    }

    /// The first byte of a message in this order.
    pub fn flag(self) -> u8 {
        match self {
            Self::Little => b'l',
            Self::Big => b'B',
        }
    }
}

/// Why bytes are not a valid D-Bus value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Truncated,
    Padding,
    String,
    Signature,
    ObjectPath,
    Boolean,
    TooLong,
    TooDeep,
}

/// The alignment of the type whose code is `code`.
pub fn alignment(code: u8) -> usize {
    match code {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

fn is_basic(code: u8) -> bool {
    matches!(
        code,
        b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b'h' | b's' | b'o' | b'g'
    )
}

/// Where the single complete type starting at `start` of `signature` ends.
pub fn type_end(signature: &[u8], start: usize) -> Result<usize, Problem> {
    type_end_at(signature, start, 0, 0)
}

fn type_end_at(signature: &[u8], start: usize, arrays: usize, structs: usize) -> Result<usize, Problem> {
    if arrays > 32 || structs > 32 {
        return Err(Problem::TooDeep);
    }
    let code = *signature.get(start).ok_or(Problem::Signature)?;
    match code {
        _ if is_basic(code) || code == b'v' => Ok(start + 1),
        b'a' => match signature.get(start + 1) {
            Some(b'{') => {
                let key = start + 2;
                if !signature.get(key).copied().is_some_and(is_basic) {
                    return Err(Problem::Signature);
                }
                let value_end = type_end_at(signature, key + 1, arrays + 1, structs + 1)?;
                match signature.get(value_end) {
                    Some(b'}') => Ok(value_end + 1),
                    _ => Err(Problem::Signature),
                }
            }
            _ => type_end_at(signature, start + 1, arrays + 1, structs),
        },
        b'(' => {
            let mut at = start + 1;
            if signature.get(at) == Some(&b')') {
                return Err(Problem::Signature);
            }
            while signature.get(at) != Some(&b')') {
                at = type_end_at(signature, at, arrays, structs + 1)?;
            }
            Ok(at + 1)
        }
        _ => Err(Problem::Signature),
    }
}

/// Whether `signature` is a valid signature: complete types, at most 255 bytes.
pub fn valid_signature(signature: &[u8]) -> bool {
    if signature.len() > MAX_SIGNATURE_BYTES {
        return false;
    }
    let mut at = 0;
    while at < signature.len() {
        match type_end(signature, at) {
            Ok(end) => at = end,
            Err(_) => return false,
        }
    }
    true
}

/// Whether `path` is a valid object path: `/`, or `/`-separated non-empty elements of
/// `[A-Za-z0-9_]` with no trailing `/`.
pub fn valid_object_path(path: &[u8]) -> bool {
    if path == b"/" {
        return true;
    }
    path.first() == Some(&b'/')
        && path[1..]
            .split(|&byte| byte == b'/')
            .all(|element| !element.is_empty() && element.iter().all(|&byte| byte.is_ascii_alphanumeric() || byte == b'_'))
}

/// Writes values in one byte order, aligned from the start of what it writes.
#[derive(Debug, Clone)]
pub struct Writer {
    bytes: Vec<u8>,
    endian: Endian,
}

impl Writer {
    /// An empty writer in `endian` order.
    pub fn new(endian: Endian) -> Self {
        Self {
            bytes: Vec::new(),
            endian,
        }
    }

    /// The byte order it writes.
    pub fn endian(&self) -> Endian {
        self.endian
    }

    /// What it wrote.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// What it wrote, taken.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Pads with zeros to a multiple of `to`.
    pub fn align(&mut self, to: usize) {
        while self.bytes.len() % to != 0 {
            self.bytes.push(0);
        }
    }

    /// A byte (`y`).
    pub fn byte(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// A 32-bit unsigned number (`u`), aligned.
    pub fn u32(&mut self, value: u32) {
        self.align(4);
        let bytes = match self.endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        self.bytes.extend_from_slice(&bytes);
    }

    /// A boolean (`b`).
    pub fn boolean(&mut self, value: bool) {
        self.u32(u32::from(value));
    }

    /// A 64-bit unsigned number (`t`), aligned.
    pub fn u64(&mut self, value: u64) {
        self.align(8);
        let bytes = match self.endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        self.bytes.extend_from_slice(&bytes);
    }

    /// A string (`s`) or an object path (`o`): its length, its bytes and a NUL.
    pub fn string(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.bytes.extend_from_slice(value);
        self.bytes.push(0);
    }

    /// A signature (`g`): its one-byte length, its bytes and a NUL.
    pub fn signature(&mut self, value: &[u8]) {
        self.bytes.push(value.len() as u8);
        self.bytes.extend_from_slice(value);
        self.bytes.push(0);
    }

    /// Starts an array whose elements align to `element_alignment`: where its length goes
    /// and where its elements start, for [`Writer::end_array`].
    pub fn begin_array(&mut self, element_alignment: usize) -> (usize, usize) {
        self.u32(0);
        let length_at = self.bytes.len() - 4;
        self.align(element_alignment);
        (length_at, self.bytes.len())
    }

    /// Ends the array [`Writer::begin_array`] started, writing its length.
    pub fn end_array(&mut self, (length_at, start): (usize, usize)) {
        let length = (self.bytes.len() - start) as u32;
        let bytes = match self.endian {
            Endian::Little => length.to_le_bytes(),
            Endian::Big => length.to_be_bytes(),
        };
        self.bytes[length_at..length_at + 4].copy_from_slice(&bytes);
    }

    /// Bytes already in this order and alignment, as they are.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

/// Reads values in one byte order, aligned from the start of `bytes`.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    endian: Endian,
}

impl<'a> Reader<'a> {
    /// A reader of `bytes` in `endian` order, starting at `at`.
    pub fn new(bytes: &'a [u8], endian: Endian, at: usize) -> Self {
        Self { bytes, at, endian }
    }

    /// Where it is.
    pub fn position(&self) -> usize {
        self.at
    }

    /// Skips the zero padding to a multiple of `to`.
    pub fn align(&mut self, to: usize) -> Result<(), Problem> {
        let end = self.at.next_multiple_of(to);
        let padding = self.bytes.get(self.at..end).ok_or(Problem::Truncated)?;
        if padding.iter().any(|&byte| byte != 0) {
            return Err(Problem::Padding);
        }
        self.at = end;
        Ok(())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Problem> {
        let end = self.at.checked_add(count).ok_or(Problem::Truncated)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Problem::Truncated)?;
        self.at = end;
        Ok(bytes)
    }

    /// A byte.
    pub fn byte(&mut self) -> Result<u8, Problem> {
        Ok(self.take(1)?[0])
    }

    /// A 16-bit number.
    pub fn u16(&mut self) -> Result<u16, Problem> {
        self.align(2)?;
        let bytes: [u8; 2] = self.take(2)?.try_into().map_err(|_| Problem::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        })
    }

    /// A 32-bit number.
    pub fn u32(&mut self) -> Result<u32, Problem> {
        self.align(4)?;
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| Problem::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        })
    }

    /// A 64-bit number.
    pub fn u64(&mut self) -> Result<u64, Problem> {
        self.align(8)?;
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| Problem::Truncated)?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(bytes),
            Endian::Big => u64::from_be_bytes(bytes),
        })
    }

    /// A boolean: 0 or 1.
    pub fn boolean(&mut self) -> Result<bool, Problem> {
        match self.u32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Problem::Boolean),
        }
    }

    /// A string: UTF-8 with no NUL inside, and its NUL after.
    pub fn string(&mut self) -> Result<&'a [u8], Problem> {
        let length = self.u32()? as usize;
        let bytes = self.take(length)?;
        if self.byte()? != 0 || bytes.contains(&0) || core::str::from_utf8(bytes).is_err() {
            return Err(Problem::String);
        }
        Ok(bytes)
    }

    /// An object path.
    pub fn object_path(&mut self) -> Result<&'a [u8], Problem> {
        let path = self.string()?;
        match valid_object_path(path) {
            true => Ok(path),
            false => Err(Problem::ObjectPath),
        }
    }

    /// A signature.
    pub fn signature(&mut self) -> Result<&'a [u8], Problem> {
        let length = usize::from(self.byte()?);
        let bytes = self.take(length)?;
        if self.byte()? != 0 || !valid_signature(bytes) {
            return Err(Problem::Signature);
        }
        Ok(bytes)
    }

    /// Starts an array of elements aligned to `element_alignment`: where it ends.
    pub fn array(&mut self, element_alignment: usize) -> Result<usize, Problem> {
        let length = self.u32()? as usize;
        if length > MAX_ARRAY_BYTES {
            return Err(Problem::TooLong);
        }
        self.align(element_alignment)?;
        let end = self.at + length;
        if end > self.bytes.len() {
            return Err(Problem::Truncated);
        }
        Ok(end)
    }

    /// Skips one value of the single complete type `signature`, checking it.
    pub fn skip(&mut self, signature: &[u8]) -> Result<(), Problem> {
        self.skip_at(signature, 0, 0).map(|_| ())
    }

    fn skip_at(&mut self, signature: &[u8], start: usize, depth: usize) -> Result<usize, Problem> {
        if depth > MAX_DEPTH {
            return Err(Problem::TooDeep);
        }
        let code = *signature.get(start).ok_or(Problem::Signature)?;
        match code {
            b'y' => {
                self.byte()?;
            }
            b'n' | b'q' => {
                self.u16()?;
            }
            b'b' => {
                self.boolean()?;
            }
            b'i' | b'u' | b'h' => {
                self.u32()?;
            }
            b'x' | b't' | b'd' => {
                self.u64()?;
            }
            b's' => {
                self.string()?;
            }
            b'o' => {
                self.object_path()?;
            }
            b'g' => {
                self.signature()?;
            }
            b'v' => {
                let inner = self.signature()?;
                if inner.is_empty() || type_end(inner, 0)? != inner.len() {
                    return Err(Problem::Signature);
                }
                self.skip_at(inner, 0, depth + 1)?;
            }
            b'a' => {
                let element = start + 1;
                let element_code = *signature.get(element).ok_or(Problem::Signature)?;
                let end = self.array(alignment(element_code))?;
                let array_end = type_end(signature, start)?;
                while self.at < end {
                    self.skip_at(signature, element, depth + 1)?;
                }
                if self.at != end {
                    return Err(Problem::Truncated);
                }
                return Ok(array_end);
            }
            b'(' | b'{' => {
                self.align(8)?;
                let close = if code == b'(' { b')' } else { b'}' };
                let mut at = start + 1;
                while signature.get(at) != Some(&close) {
                    at = self.skip_at(signature, at, depth + 1)?;
                }
                return Ok(at + 1);
            }
            _ => return Err(Problem::Signature),
        }
        type_end(signature, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_complete_types() {
        for good in [&b""[..], b"s", b"a{sv}", b"(ii)as", b"aa{s(uv)}", b"v"] {
            assert!(valid_signature(good), "{:?}", core::str::from_utf8(good));
        }
        for bad in [&b"a"[..], b"()", b"{sv}", b"a{vs}", b"(i", b"z", b"a{sii}"] {
            assert!(!valid_signature(bad), "{:?}", core::str::from_utf8(bad));
        }
    }

    #[test]
    fn written_values_read_back_in_both_orders() {
        for endian in [Endian::Little, Endian::Big] {
            let mut writer = Writer::new(endian);
            writer.byte(7);
            writer.string(b"egg");
            let array = writer.begin_array(8);
            writer.align(8);
            writer.string(b"key");
            writer.signature(b"u");
            writer.u32(9);
            writer.end_array(array);
            writer.u64(1 << 40);
            let bytes = writer.into_bytes();
            let mut reader = Reader::new(&bytes, endian, 0);
            assert_eq!(reader.byte(), Ok(7));
            assert_eq!(reader.string(), Ok(&b"egg"[..]));
            reader.skip(b"a{sv}").unwrap();
            assert_eq!(reader.u64(), Ok(1 << 40));
        }
    }

    #[test]
    fn object_paths_follow_the_rules() {
        assert!(valid_object_path(b"/"));
        assert!(valid_object_path(b"/org/freedesktop/DBus"));
        assert!(!valid_object_path(b"/org/"));
        assert!(!valid_object_path(b"org"));
        assert!(!valid_object_path(b"/a//b"));
        assert!(!valid_object_path(b"/a-b"));
    }
}
