use super::DecodeErrorKind;

/// The most bytes one x86 instruction may have; a longer one raises `#GP`.
pub const MAX_INSTRUCTION_LENGTH: usize = 15;

#[derive(Debug)]
pub(super) struct Cursor<'a> {
    code: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(code: &'a [u8]) -> Cursor<'a> {
        Cursor { code, position: 0 }
    }

    pub(super) fn position(&self) -> usize {
        self.position
    }

    pub(super) fn peek(&self) -> Result<u8, DecodeErrorKind> {
        if self.position >= MAX_INSTRUCTION_LENGTH {
            return Err(DecodeErrorKind::TooLong);
        }
        self.code
            .get(self.position)
            .copied()
            .ok_or(DecodeErrorKind::Truncated)
    }

    pub(super) fn byte(&mut self) -> Result<u8, DecodeErrorKind> {
        let byte = self.peek()?;
        self.position += 1;
        Ok(byte)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeErrorKind> {
        let mut bytes = [0; N];
        for slot in &mut bytes {
            *slot = self.byte()?;
        }
        Ok(bytes)
    }

    pub(super) fn i8(&mut self) -> Result<i8, DecodeErrorKind> {
        Ok(i8::from_le_bytes(self.array()?))
    }

    pub(super) fn i16(&mut self) -> Result<i16, DecodeErrorKind> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    pub(super) fn i32(&mut self) -> Result<i32, DecodeErrorKind> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(super) fn i64(&mut self) -> Result<i64, DecodeErrorKind> {
        Ok(i64::from_le_bytes(self.array()?))
    }
}
