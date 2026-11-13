//! Reading AML's encodings (ACPI 6.5, 20.2): bytes and little-endian integers, package lengths,
//! name strings and strings, from a stream that never reads past its end.

use crate::Error;
use crate::name::{NameSeg, NameString};
use alloc::vec::Vec;

const DUAL_NAME_PREFIX: u8 = 0x2e;
const MULTI_NAME_PREFIX: u8 = 0x2f;
const ROOT_CHAR: u8 = b'\\';
const PARENT_PREFIX: u8 = b'^';
const NULL_NAME: u8 = 0x00;

/// AML from `at` to the end of `code`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stream<'a> {
    code: &'a [u8],
    pub(crate) at: usize,
}

impl<'a> Stream<'a> {
    pub(crate) fn new(code: &'a [u8], at: usize) -> Self {
        Self { code, at }
    }

    /// The same stream, ending at `end`.
    pub(crate) fn until(&self, end: usize) -> Self {
        Self {
            code: &self.code[..end.min(self.code.len())],
            at: self.at,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.at >= self.code.len()
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.code.get(self.at).copied()
    }

    pub(crate) fn byte(&mut self) -> Result<u8, Error> {
        let byte = self.peek().ok_or(Error::UnexpectedEnd)?;
        self.at += 1;
        Ok(byte)
    }

    pub(crate) fn bytes(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(count).ok_or(Error::UnexpectedEnd)?;
        let bytes = self.code.get(self.at..end).ok_or(Error::UnexpectedEnd)?;
        self.at = end;
        Ok(bytes)
    }

    pub(crate) fn word(&mut self) -> Result<u16, Error> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn dword(&mut self) -> Result<u32, Error> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn qword(&mut self) -> Result<u64, Error> {
        let low = u64::from(self.dword()?);
        let high = u64::from(self.dword()?);
        Ok(high << 32 | low)
    }

    pub(crate) fn skip_to(&mut self, at: usize) {
        self.at = at.min(self.code.len());
    }

    /// A `PkgLength`'s value: one byte of up to 63, or a lead byte whose top two bits count the
    /// bytes that follow, its low four bits the value's lowest.
    pub(crate) fn package_length(&mut self) -> Result<u32, Error> {
        let lead = self.byte()?;
        let following = lead >> 6;
        if following == 0 {
            return Ok(u32::from(lead & 0x3f));
        }
        let mut length = u32::from(lead & 0x0f);
        for index in 0..following {
            length |= u32::from(self.byte()?) << (4 + 8 * u32::from(index));
        }
        Ok(length)
    }

    /// Reads a `PkgLength` and returns where its package ends, which counts from the length's
    /// first byte.
    pub(crate) fn package_end(&mut self) -> Result<usize, Error> {
        let start = self.at;
        let end = start
            .checked_add(self.package_length()? as usize)
            .ok_or(Error::UnexpectedEnd)?;
        if end > self.code.len() || end < self.at {
            return Err(Error::UnexpectedEnd);
        }
        Ok(end)
    }

    /// Whether a `NameString` starts here: a lead name character or a prefix.
    pub(crate) fn at_name(&self) -> bool {
        self.peek().is_some_and(|byte| {
            matches!(byte, b'A'..=b'Z' | b'_' | ROOT_CHAR | PARENT_PREFIX)
                || byte == DUAL_NAME_PREFIX
                || byte == MULTI_NAME_PREFIX
        })
    }

    fn name_seg(&mut self) -> Result<NameSeg, Error> {
        let bytes = self.bytes(4)?;
        let segment = [bytes[0], bytes[1], bytes[2], bytes[3]];
        let lead = matches!(segment[0], b'A'..=b'Z' | b'_');
        let rest = segment[1..]
            .iter()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'));
        match lead && rest {
            true => Ok(NameSeg(segment)),
            false => Err(Error::MalformedName),
        }
    }

    pub(crate) fn name_string(&mut self) -> Result<NameString, Error> {
        let mut name = NameString::default();
        match self.peek() {
            Some(ROOT_CHAR) => {
                self.at += 1;
                name.root = true;
            }
            _ => {
                while self.peek() == Some(PARENT_PREFIX) {
                    self.at += 1;
                    name.parents = name.parents.saturating_add(1);
                }
            }
        }
        let count = match self.peek().ok_or(Error::UnexpectedEnd)? {
            NULL_NAME => {
                self.at += 1;
                0
            }
            DUAL_NAME_PREFIX => {
                self.at += 1;
                2
            }
            MULTI_NAME_PREFIX => {
                self.at += 1;
                usize::from(self.byte()?)
            }
            _ => 1,
        };
        name.segments = Vec::with_capacity(count);
        for _ in 0..count {
            name.segments.push(self.name_seg()?);
        }
        Ok(name)
    }

    /// An `AsciiCharList` up to its terminating zero, which is consumed.
    pub(crate) fn string(&mut self) -> Result<&'a [u8], Error> {
        let rest = self.code.get(self.at..).ok_or(Error::UnexpectedEnd)?;
        let length = rest
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(Error::UnexpectedEnd)?;
        self.at += length + 1;
        Ok(&rest[..length])
    }
}
